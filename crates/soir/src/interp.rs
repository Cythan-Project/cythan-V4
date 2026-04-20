//! Graph-walking interpreter for soir.
//!
//! Milestone M2: the "nothing broken" gate. Given a `Graph`
//! (post-inline — no `Call` nodes), seed `Param(i)` values from
//! the caller's inputs and walk the control subgraph, evaluating
//! pure data on demand, running effectful nodes against a user-
//! supplied `RunContext`, and following the branch picked by each
//! `If` / `Match`.
//!
//! # Pure-value evaluation
//!
//! Pure nodes (Const, Inc/Dec/Add/Sub/Eq, Phi) are memoized in a
//! `vals` map. The cache is invalidated when we re-enter a loop
//! header via its backedge — any `Phi` at that header resolves to
//! its backedge value, and downstream pure computations that
//! consumed the stale phi need to be recomputed. Simplest approach
//! is to drop the entire cache on a loop re-entry; this hurts a
//! schedule-based interpreter but M2 only needs correctness.
//!
//! # Phi resolution
//!
//! Phis live at a `Region` or `Loop` node. On entering that
//! region, we know which pred brought us in (tracked as
//! `prev_ctrl`). The phi's value is `values[pred_index]`.
//!
//! # Non-support
//!
//! `Call` nodes in the graph cause a panic — inlining (M4) must
//! run first for the interpreter to see a flat graph. Synthetic
//! tests and post-inline real programs both satisfy that.

use std::collections::HashMap;

use mir::RunContext;

use crate::ir::{Graph, NodeId, NodeKind, ProjKind};

/// Step ceiling to stop a runaway loop from hanging tests.
pub const DEFAULT_STEP_LIMIT: usize = 5_000_000;

/// What happened in a single control step.
enum Step {
    Continue(NodeId),
    Return(Vec<u8>),
    Halt,
}

/// Run the graph against `ctx`. `inputs` provides one u4 byte per
/// function input cell (in `FlatSig::input_count` order). Returns
/// the output values on `Return`, `None` on `Stop`.
pub fn run<C: RunContext>(
    g: &Graph,
    inputs: &[u8],
    ctx: &mut C,
) -> RunResult {
    run_with_limit(g, inputs, ctx, DEFAULT_STEP_LIMIT)
}

pub fn run_with_limit<C: RunContext>(
    g: &Graph,
    inputs: &[u8],
    ctx: &mut C,
    step_limit: usize,
) -> RunResult {
    let mut interp = Interp::new(g);
    interp.seed_params(inputs);
    let start_ctrl = interp
        .find_proj(g.start(), &ProjKind::StartCtrl)
        .expect("graph missing StartCtrl");
    interp.prev_ctrl = g.start();
    let mut current = start_ctrl;
    let mut steps: usize = 0;
    loop {
        if step_limit != 0 && steps >= step_limit {
            return RunResult {
                values: Vec::new(),
                halted: false,
                aborted_by_limit: true,
                steps,
            };
        }
        steps += 1;
        match interp.step(current, ctx) {
            Step::Continue(next) => {
                interp.prev_ctrl = current;
                current = next;
            }
            Step::Return(values) => {
                return RunResult {
                    values,
                    halted: false,
                    aborted_by_limit: false,
                    steps,
                }
            }
            Step::Halt => {
                return RunResult {
                    values: Vec::new(),
                    halted: true,
                    aborted_by_limit: false,
                    steps,
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub values: Vec<u8>,
    pub halted: bool,
    pub aborted_by_limit: bool,
    pub steps: usize,
}

struct Interp<'g> {
    g: &'g Graph,
    prev_ctrl: NodeId,
    /// Memoized values for pure nodes + ReadRegVal projections.
    /// Cleared on region/loop re-entry so phis and their
    /// transitive consumers re-resolve.
    vals: HashMap<NodeId, u8>,
    /// Function parameters. Never cleared — seeded once at start
    /// and looked up directly from `val()`.
    params: HashMap<NodeId, u8>,
    /// VM-visible registers. Matches the MIR interpreter's
    /// 4-register setup: reg 0 is the IO control register, 1/2
    /// hold the high/low nibbles of the byte to print (or the
    /// byte just read), 3 is a scratch slot used by stdlib print.
    registers: [u8; 4],
    /// Nodes currently on the `val()` recursion stack — cycle
    /// detector. A well-formed graph shouldn't have pure-data
    /// cycles; if one sneaks in, we catch it before the stack
    /// overflows.
    val_in_progress: std::collections::HashSet<NodeId>,
}

impl<'g> Interp<'g> {
    fn new(g: &'g Graph) -> Self {
        Self {
            g,
            prev_ctrl: NodeId::INVALID,
            vals: HashMap::new(),
            params: HashMap::new(),
            registers: [0; 4],
            val_in_progress: std::collections::HashSet::new(),
        }
    }

    fn seed_params(&mut self, inputs: &[u8]) {
        for (i, v) in inputs.iter().enumerate() {
            if let Some(proj) = self.find_proj(
                self.g.start(),
                &ProjKind::Param(i as u32),
            ) {
                self.params.insert(proj, *v);
            }
        }
    }

    /// Find the unique `Proj { of: source, kind: looking_for }` in
    /// `source`'s user list. Returns `None` if no such proj exists.
    fn find_proj(&self, source: NodeId, looking_for: &ProjKind) -> Option<NodeId> {
        for &u in &self.g.get(source).users {
            if let NodeKind::Proj { kind, .. } = &self.g.get(u).kind {
                if kind == looking_for {
                    return Some(u);
                }
            }
        }
        None
    }

    /// Value of a pure/effect-producing data node. Follows pure
    /// computation recursively; Phi resolves via `prev_ctrl`.
    fn val(&mut self, id: NodeId) -> u8 {
        if let Some(&v) = self.params.get(&id) {
            return v;
        }
        if let Some(&v) = self.vals.get(&id) {
            return v;
        }
        // Cycle detector: pure-data chains shouldn't reference
        // themselves. Phis break the cycle via the eager
        // resolution at region entry, so any cycle reaching this
        // point is a builder bug — panic instead of hanging.
        if !self.val_in_progress.insert(id) {
            panic!(
                "soir interp: data-dependency cycle at {} — val() called \
                 recursively on the same node",
                id
            );
        }
        let v = match self.g.get(id).kind.clone() {
            NodeKind::Const(v) => v,
            NodeKind::Inc(a) => (self.val(a) + 1) & 0x0F,
            NodeKind::Dec(a) => (self.val(a).wrapping_sub(1)) & 0x0F,
            NodeKind::Add(a, b) => (self.val(a) + self.val(b)) & 0x0F,
            NodeKind::Sub(a, b) => (self.val(a).wrapping_sub(self.val(b))) & 0x0F,
            NodeKind::Eq(a, b) => {
                if self.val(a) == self.val(b) {
                    1
                } else {
                    0
                }
            }
            NodeKind::Proj { kind: ProjKind::ReadRegVal, of: _ } => {
                panic!(
                    "soir interp: ReadRegVal for {} not set; \
                     effectful node should have been walked first",
                    id
                );
            }
            NodeKind::Proj { kind: ProjKind::Param(i), of: _ } => {
                panic!(
                    "soir interp: Param({}) proj {} unbound — caller didn't \
                     provide enough inputs",
                    i, id
                );
            }
            NodeKind::Phi { .. } | NodeKind::EffPhi { .. } => {
                // Phis must have been resolved at region entry
                // — we should never fall through to `val()` for
                // one. If this fires it means a phi is being
                // consumed from outside its region's control
                // subtree, which is a builder bug.
                panic!(
                    "soir interp: unresolved phi {} — region entry must \
                     eagerly populate phis before control advances",
                    id
                );
            }
            NodeKind::Proj {
                kind: ProjKind::Eff | ProjKind::StartEff,
                of: _,
            } => 0,
            NodeKind::Proj { kind: ProjKind::CallRet(_), of: _ } => {
                panic!("soir interp: Call return {} reached pre-inline", id)
            }
            other => panic!(
                "soir interp: val({}) on non-data node {:?}",
                id, other
            ),
        };
        self.vals.insert(id, v);
        self.val_in_progress.remove(&id);
        v
    }

    /// Resolve a Phi by picking `values[i]` where `i` is the index
    /// of `prev_ctrl` in the region's pred list. Called at region
    /// entry when `prev_ctrl` still points at the actual
    /// predecessor; later calls to `val()` just hit the cache.
    fn eval_phi(&mut self, phi: NodeId) -> u8 {
        let (region, values) = match self.g.get(phi).kind.clone() {
            NodeKind::Phi { region, values } => (region, values),
            NodeKind::EffPhi { region, effs } => (region, effs),
            k => panic!("eval_phi on non-phi {}: {:?}", phi, k),
        };
        let idx = self.pred_index(region, self.prev_ctrl);
        match values.get(idx).copied().flatten() {
            Some(v) => {
                // EffPhi: the value is an effect token, opaque.
                // Reading it as a u8 returns 0 — we never use it
                // as a data value, just memoize to suppress the
                // "unresolved phi" panic elsewhere.
                if matches!(
                    self.g.get(v).kind,
                    NodeKind::Proj { kind: ProjKind::Eff, .. } | NodeKind::EffPhi { .. }
                ) {
                    0
                } else {
                    self.val(v)
                }
            }
            None => panic!(
                "soir interp: phi {} at region {} missing value for pred idx {}",
                phi, region, idx
            ),
        }
    }

    fn pred_index(&self, region: NodeId, pred: NodeId) -> usize {
        match &self.g.get(region).kind {
            NodeKind::Region { preds }
            | NodeKind::BlockExit { preds, .. }
            | NodeKind::LoopExit { preds, .. } => preds
                .iter()
                .position(|p| *p == pred)
                .unwrap_or_else(|| {
                    panic!(
                        "soir interp: pred {} not found in region {} preds {:?}",
                        pred, region, preds
                    )
                }),
            NodeKind::Loop { entry, back } => {
                if *entry == pred {
                    0
                } else if Some(pred) == *back {
                    1
                } else {
                    panic!(
                        "soir interp: pred {} is neither entry ({}) nor back \
                         ({:?}) of loop {}",
                        pred, entry, back, region
                    )
                }
            }
            k => panic!("soir interp: pred_index on non-region {:?}", k),
        }
    }

    /// Execute one control step. Returns what to do next.
    fn step<C: RunContext>(&mut self, current: NodeId, ctx: &mut C) -> Step {
        let kind = self.g.get(current).kind.clone();
        match kind {
            NodeKind::Proj {
                kind: ProjKind::StartCtrl,
                ..
            }
            | NodeKind::Proj {
                kind: ProjKind::IfTrue,
                ..
            }
            | NodeKind::Proj {
                kind: ProjKind::IfFalse,
                ..
            }
            | NodeKind::Proj {
                kind: ProjKind::MatchArm(_),
                ..
            } => {
                // These are "arm entry" control nodes. Pass through
                // to the next control-consuming user.
                Step::Continue(self.ctrl_successor(current))
            }
            NodeKind::Block { .. } => {
                // Block is a scope marker; pure control
                // pass-through for the interpreter.
                Step::Continue(self.ctrl_successor(current))
            }
            NodeKind::BlockExit { .. } | NodeKind::LoopExit { .. } => {
                // Phi/EffPhi at a BlockExit / LoopExit resolve the
                // same way as at a Region — the pred tells us
                // which lane to pick. Eager resolution mirrors
                // the Region case below.
                let phis: Vec<NodeId> = self
                    .g
                    .get(current)
                    .users
                    .iter()
                    .copied()
                    .filter(|u| {
                        matches!(
                            self.g.get(*u).kind,
                            NodeKind::Phi { .. } | NodeKind::EffPhi { .. }
                        )
                    })
                    .collect();
                for phi in phis {
                    let v = self.eval_phi(phi);
                    self.vals.insert(phi, v);
                }
                Step::Continue(self.ctrl_successor(current))
            }
            NodeKind::Region { .. } | NodeKind::Loop { .. } => {
                // Resolve every phi attached to this region using
                // the *current* prev_ctrl. If we waited and
                // resolved phis on demand, prev_ctrl would have
                // advanced to the region itself by the time a
                // pure consumer fired.
                //
                // Order matters on a `Loop`: the backedge value of
                // a phi may reference the phi itself transitively
                // (e.g. `phi_new = Inc(phi_old)`). We MUST compute
                // new phi values using the prior iteration's
                // cached values, then swap them in. Clearing
                // before resolving would nuke the prior phi and
                // make the backedge unresolvable.
                let phis: Vec<NodeId> = self
                    .g
                    .get(current)
                    .users
                    .iter()
                    .copied()
                    .filter(|u| {
                        matches!(
                            self.g.get(*u).kind,
                            NodeKind::Phi { .. } | NodeKind::EffPhi { .. }
                        )
                    })
                    .collect();
                let mut new_phi_values: Vec<(NodeId, u8)> =
                    Vec::with_capacity(phis.len());
                for phi in &phis {
                    new_phi_values.push((*phi, self.eval_phi(*phi)));
                }
                if matches!(kind, NodeKind::Loop { .. }) {
                    // Discard prior iteration's memoized values
                    // (their phi inputs are about to change).
                    self.vals.clear();
                }
                for (id, v) in new_phi_values {
                    self.vals.insert(id, v);
                }
                Step::Continue(self.ctrl_successor(current))
            }
            NodeKind::If { cond, .. } => {
                let c = self.val(cond);
                let proj = if c != 0 {
                    self.find_proj(current, &ProjKind::IfTrue)
                } else {
                    self.find_proj(current, &ProjKind::IfFalse)
                };
                Step::Continue(proj.expect("If missing arm proj"))
            }
            NodeKind::Match {
                scrut, arm_values, ..
            } => {
                let s = self.val(scrut);
                let arm = arm_values
                    .iter()
                    .position(|vals| vals.contains(&s))
                    .unwrap_or_else(|| {
                        panic!(
                            "soir interp: match value {} covered by no arm (node {})",
                            s, current
                        )
                    });
                let proj = self
                    .find_proj(current, &ProjKind::MatchArm(arm as u32))
                    .expect("Match missing arm proj");
                Step::Continue(proj)
            }
            NodeKind::ReadReg { reg, .. } => {
                // MIR semantics: reading reg N reads registers[N].
                // Value propagated via the `ReadRegVal` proj.
                let v = self.read_register(reg, ctx);
                if let Some(proj) = self.find_proj(current, &ProjKind::ReadRegVal) {
                    self.vals.insert(proj, v);
                }
                Step::Continue(self.ctrl_successor(current))
            }
            NodeKind::WriteReg { reg, val, .. } => {
                let v = self.val(val);
                self.write_register(reg, v, ctx);
                Step::Continue(self.ctrl_successor(current))
            }
            NodeKind::Stop { .. } => Step::Halt,
            NodeKind::Return { values, .. } => {
                let outs: Vec<u8> = values.iter().map(|v| self.val(*v)).collect();
                Step::Return(outs)
            }
            NodeKind::Call { .. } => panic!(
                "soir interp: Call reached; run after M4 inlining, or \
                 resolve calls before invoking the interpreter"
            ),
            k => panic!(
                "soir interp: unreachable control step at {} (kind {})",
                current,
                k.tag()
            ),
        }
    }

    /// Pick the unique user of `node` that consumes it as a control
    /// edge. Panics if zero or multiple such users exist (builder
    /// invariant: each control node has exactly one control
    /// successor, except branches which go through their Proj
    /// arms).
    fn ctrl_successor(&self, node: NodeId) -> NodeId {
        let mut found: Option<NodeId> = None;
        let mut seen: std::collections::HashSet<NodeId> =
            std::collections::HashSet::new();
        for &u in &self.g.get(node).users {
            if !seen.insert(u) {
                continue;
            }
            if self.consumes_ctrl(u, node) {
                if let Some(prev) = found {
                    panic!(
                        "soir interp: multiple ctrl successors for {}: {} and {}",
                        node, prev, u
                    );
                }
                found = Some(u);
            }
        }
        found.unwrap_or_else(|| panic!("soir interp: no ctrl successor for {}", node))
    }

    /// Does `user` consume `node` as a control edge?
    fn consumes_ctrl(&self, user: NodeId, node: NodeId) -> bool {
        match &self.g.get(user).kind {
            NodeKind::Region { preds } => preds.contains(&node),
            NodeKind::BlockExit { preds, .. } => preds.contains(&node),
            NodeKind::LoopExit { preds, .. } => preds.contains(&node),
            NodeKind::Loop { entry, back } => *entry == node || *back == Some(node),
            NodeKind::Block { ctrl } => *ctrl == node,
            NodeKind::If { ctrl, .. } => *ctrl == node,
            NodeKind::Match { ctrl, .. } => *ctrl == node,
            NodeKind::ReadReg { ctrl, .. } => *ctrl == node,
            NodeKind::WriteReg { ctrl, .. } => *ctrl == node,
            NodeKind::Call { ctrl, .. } => *ctrl == node,
            NodeKind::Stop { ctrl, .. } => *ctrl == node,
            NodeKind::Return { ctrl, .. } => *ctrl == node,
            NodeKind::Proj { of, kind } => {
                *of == node
                    && matches!(
                        kind,
                        ProjKind::IfTrue
                            | ProjKind::IfFalse
                            | ProjKind::MatchArm(_)
                            | ProjKind::StartCtrl
                    )
            }
            _ => false,
        }
    }

    fn read_register<C: RunContext>(&self, reg: u8, _ctx: &mut C) -> u8 {
        // Mirror MIR: `ReadRegister(reg)` just pulls the current
        // value out of our register file. Side-effecting reads
        // (input) are triggered via `WriteRegister(0, 2)` below.
        self.registers[reg as usize]
    }

    fn write_register<C: RunContext>(&mut self, reg: u8, val: u8, ctx: &mut C) {
        if reg == 0 {
            if val == 1 {
                let a = self.registers[1];
                let b = self.registers[2];
                let byte = ((a % 16) * 16) + (b % 16);
                ctx.print(byte);
            } else if val == 2 {
                let o = ctx.input();
                // MIR writes: high = o/16, low = o%16 — match
                // byte-to-nibble split exactly.
                self.registers[1] = o / 16;
                self.registers[2] = o % 16;
            }
        }
        self.registers[reg as usize] = val;
    }
}
