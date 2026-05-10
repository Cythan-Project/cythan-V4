//! Scheduler: soir `Graph` → `MirCodeBlock`.
//!
//! # Milestone-M3 scope
//!
//! This is the "rollout gate" — once it passes, the soir backend
//! can emit correct MIR for every post-inline graph, and future
//! milestones become additive. True GCM (loop-invariant code
//! motion via dominator trees) is deferred; the initial scheduler
//! walks the control subgraph directly and emits pure ops
//! just-in-time before their first use in each MIR block.
//!
//! # Approach
//!
//! Because the SoN graphs we produce are *built from structured
//! HIR*, the control subgraph is reducible and cleanly structured:
//!
//! * Every `Match` arm reconvenes at a single post-match `Region`.
//! * Every `Loop` body either flows back to the header (natural
//!   fall-through or `Continue`) or jumps to a post-loop merge
//!   (`Break`).
//!
//! Knowing these invariants, we recursively emit MIR by walking
//! the control chain until we hit a known "stop boundary" (post-
//! match region, loop header, post-loop region).
//!
//! # Slot allocation
//!
//! Each value-producing node gets a `u32` slot. Inputs land at
//! slots `0..input_count` (matching `Param(i)` projections);
//! outputs at `input_count..total_slots()`; temps from there on.
//!
//! # Phi lowering
//!
//! A `Phi { region, values }` at a merge point gets a temp slot.
//! At the end of each pred block — *before* control flows into
//! `region` — we emit `Copy(phi_slot, slot_of(values[i]))` for
//! the pred at index `i`.
//!
//! For effect phis, no MIR emission is needed: effect tokens only
//! matter for scheduling order, which is already fixed by the
//! control walk.
//!
//! # Unsupported (for M3)
//!
//! `Call` nodes: the post-inline graphs targeted by M3 have no
//! `Call`. Panic if we see one; M4 inlining removes them.

use std::collections::{HashMap, HashSet};

use either::Either;
use mir::{Mir, MirCodeBlock};

use crate::ir::{Graph, NodeId, NodeKind, ProjKind};

/// Hard ceiling on total control steps the scheduler will take
/// across a single `schedule()` call. Bounds work when the
/// scheduler walks a deeply-nested graph (each arm produces its
/// own copy of downstream code, so for deep nesting the MIR
/// output is exponential in nesting depth). 5M is enough for
/// simple programs through Morpion-complexity; graphs bigger than
/// that need a smarter scheduler (emit shared sub-graphs into
/// `Mir::Block` with `Mir::Skip` callers).
pub const SCHEDULE_STEP_LIMIT: usize = 5_000_000;

/// Hard ceiling on the size of the MIR `Vec<Mir>` the scheduler
/// will accumulate. Bounds the RAM footprint of scheduling
/// exponentially-deep graphs — a single `Mir::Match` per level
/// with N arms and per-arm replicated code can produce output
/// growing as N^depth. This is a pragmatic guard; a proper fix
/// is to emit shared sub-graphs into a `Mir::Block` and `Mir::Skip`
/// to it from each arm.
pub const SCHEDULE_MIR_SIZE_LIMIT: usize = 200_000;

/// Max depth for `ensure_value`'s recursive data-dependency
/// traversal. Pure-op chains shouldn't go deep; if this trips it
/// means the data graph has a cycle the scheduler can't lower.
pub const ENSURE_VALUE_MAX_DEPTH: usize = 4096;

/// Convert a single-function `Graph` into a `MirCodeBlock`.
pub fn schedule(g: &Graph) -> MirCodeBlock {
    let mut s = Scheduler::new(g);
    let start_ctrl = s
        .find_proj(g.start(), &ProjKind::StartCtrl)
        .expect("graph missing StartCtrl");
    let mut out = MirCodeBlock(Vec::new());
    s.walk_chain(start_ctrl, StopAt::ReturnOrStop, &mut out);
    out
}

/// What makes the current control walk stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum StopAt {
    /// Only terminal ops (Return / Stop) end the walk. Used for
    /// the top-level function body.
    ReturnOrStop,
    /// Stop when control reaches the given node (a `Region` that
    /// is the post-match merge, or a `Loop` header).
    AtNode(NodeId),
}

struct Scheduler<'g> {
    g: &'g Graph,
    /// Stable slot per value-producing node.
    slot_of: HashMap<NodeId, u32>,
    /// Next free slot index for temps.
    next_slot: u32,
    /// Pure ops already emitted in the *current* block. Reset at
    /// every block boundary so each block is self-contained (no
    /// cross-block value implicit sharing — loops and branches
    /// need fresh ops per basic block).
    emitted_here: std::collections::HashSet<NodeId>,
    /// Stack of enclosing loop contexts. The topmost entry tells
    /// us what `Break`-like and `Continue`-like transitions look
    /// like so we can emit `Mir::Break` / `Mir::Continue`.
    loops: Vec<LoopFrame>,
    /// Stack of enclosing `Block` scopes. Used to emit
    /// `Mir::Skip` when control crosses into a block's
    /// `BlockExit`.
    block_exits: Vec<NodeId>,
    /// Total control steps taken across all walks. Bounded by
    /// `SCHEDULE_STEP_LIMIT` so a buggy scheduler fails fast
    /// instead of hanging.
    step_budget: usize,
    /// Nodes currently on the `ensure_value` recursion stack.
    /// Used to detect data-dependency cycles that would otherwise
    /// blow the stack or loop silently.
    value_in_progress: HashSet<NodeId>,
    /// Memoized results from `find_merge_region`. Each branch
    /// node's post-merge region is deterministic from the graph;
    /// without this cache we'd recompute it every time we hit
    /// the same Match, and deeply nested graphs explode into
    /// O(matches × arms × graph_size) work.
    merge_cache: HashMap<NodeId, Option<NodeId>>,
    /// Total MIR ops emitted across the whole schedule (sums
    /// into nested arm/body blocks). Bounded by
    /// `SCHEDULE_MIR_SIZE_LIMIT` to cap RAM on exponentially-
    /// deep graphs.
    mir_emitted: usize,
    /// Memoised walk_chain output for walks that are "scope-
    /// independent" — they emit no `Mir::Skip` / `Break` /
    /// `Continue` that would target an enclosing scope. Such
    /// walks produce identical MIR regardless of the caller's
    /// scope stack, so a small (start, stop)-keyed cache is
    /// sound. This is the key M9 optimization: post-inline
    /// graphs have many inlined callee subgraphs reached via
    /// distinct-but-structurally-identical arm paths; each
    /// callee body walk is self-contained (terminates at its
    /// own Return/Stop), so the first walk populates this cache
    /// and every subsequent walk of the same callee subgraph
    /// hits.
    walk_cache: HashMap<(NodeId, StopAt), MirCodeBlock>,
    /// Running total of cached MIR ops; bounded by
    /// `WALK_CACHE_OP_BUDGET` so cache growth can't OOM.
    walk_cache_ops: usize,
}

/// Cap on how much MIR (in ops) the walk cache is allowed to
/// hold. 1M × ~100B = ~100MB worst-case — cacheable entries
/// are scope-independent so this almost always covers every
/// inlined callee body exactly once.
pub const WALK_CACHE_OP_BUDGET: usize = 200_000;

#[derive(Debug, Clone)]
struct LoopFrame {
    header: NodeId,
    /// Post-loop Region: control flowing into this means "break".
    /// `None` for an infinite loop that never breaks (unusual).
    /// Currently unused — reserved for M5+ Break lowering across
    /// nested loops where a Break's target isn't the innermost
    /// loop (labelled breaks, future extension).
    #[allow(dead_code)]
    exit: Option<NodeId>,
}

impl<'g> Scheduler<'g> {
    fn new(g: &'g Graph) -> Self {
        let sig = g.sig();
        let mut slot_of: HashMap<NodeId, u32> = HashMap::new();
        // Seed param slots from Proj(Param(i)).
        for (id, n) in g.iter() {
            if let NodeKind::Proj {
                of,
                kind: ProjKind::Param(i),
            } = &n.kind
            {
                if *of == g.start() {
                    slot_of.insert(id, *i);
                }
            }
        }
        let next_slot = sig.total_slots();
        Self {
            g,
            slot_of,
            next_slot,
            emitted_here: HashSet::new(),
            loops: Vec::new(),
            block_exits: Vec::new(),
            step_budget: 0,
            value_in_progress: HashSet::new(),
            merge_cache: HashMap::new(),
            mir_emitted: 0,
            walk_cache: HashMap::new(),
            walk_cache_ops: 0,
        }
    }

    /// Look up or assign a MIR slot for `node`.
    fn slot(&mut self, node: NodeId) -> u32 {
        if let Some(&s) = self.slot_of.get(&node) {
            return s;
        }
        let s = self.next_slot;
        self.next_slot += 1;
        self.slot_of.insert(node, s);
        s
    }

    fn find_proj(&self, source: NodeId, kind: &ProjKind) -> Option<NodeId> {
        for &u in &self.g.get(source).users {
            if let NodeKind::Proj { kind: k, .. } = &self.g.get(u).kind {
                if k == kind {
                    return Some(u);
                }
            }
        }
        None
    }

    /// Ensure `node`'s value is materialized into its slot in the
    /// current block. Pure ops are emitted JIT just before first
    /// use. For already-emitted nodes (Param, prior ops in this
    /// block), just return the slot.
    fn ensure_value(&mut self, node: NodeId, out: &mut MirCodeBlock) -> u32 {
        if let Some(&s) = self.slot_of.get(&node) {
            if self.emitted_here.contains(&node) {
                return s;
            }
            if self.is_param(node) {
                self.emitted_here.insert(node);
                return s;
            }
        }
        // Cycle + depth guard: `value_in_progress` catches pure
        // data cycles that would otherwise blow the stack; the
        // depth check catches chains that are long enough to
        // suggest a malformed graph rather than a real program.
        if !self.value_in_progress.insert(node) {
            panic!(
                "soir::schedule: data-dependency cycle detected at {} — \
                 pure data chain loops back on itself",
                node
            );
        }
        if self.value_in_progress.len() > ENSURE_VALUE_MAX_DEPTH {
            panic!(
                "soir::schedule: ensure_value depth exceeded {} at node {} — \
                 pure-op chain longer than any plausible program",
                ENSURE_VALUE_MAX_DEPTH, node
            );
        }
        let kind = self.g.get(node).kind.clone();
        let slot = self.slot(node);
        match kind {
            NodeKind::Const(v) => self.emit(out, Mir::Set(slot, v)),
            NodeKind::Proj {
                of,
                kind: ProjKind::ReadRegVal,
            } => {
                // Value is defined by the effectful ReadReg; its
                // emission point in the control chain materialises
                // the slot directly. Just mark as emitted so we
                // don't re-enter here.
                let _ = of;
            }
            NodeKind::Phi { .. } | NodeKind::EffPhi { .. } => {
                // Phis get their values via Copy at pred exits;
                // reading them from their slot just works.
            }
            NodeKind::Proj {
                kind: ProjKind::CallRet(_),
                ..
            } => {
                // Same as ReadRegVal — the Call instruction wrote
                // this slot. Just use it.
            }
            NodeKind::Proj {
                kind: ProjKind::Eff | ProjKind::StartEff,
                ..
            } => {
                // Effect tokens are scheduling artefacts, not
                // data. Nothing to emit; no slot needed.
            }
            NodeKind::Proj {
                kind: ProjKind::Param(_),
                ..
            } => {
                // Param slot is already seeded; nothing to emit.
            }
            NodeKind::Add(_, _) | NodeKind::Sub(_, _) | NodeKind::Eq(_, _) => {
                panic!(
                    "soir::schedule: {:?} not lowerable to MIR; \
                     expected to be replaced by a Match/loop after \
                     M6 pattern rewrites",
                    kind.tag()
                );
            }
            other => panic!(
                "soir::schedule: ensure_value on non-data node {:?}",
                other.tag()
            ),
        }
        self.emitted_here.insert(node);
        self.value_in_progress.remove(&node);
        slot
    }

    fn is_param(&self, node: NodeId) -> bool {
        matches!(
            self.g.get(node).kind,
            NodeKind::Proj {
                kind: ProjKind::Param(_),
                ..
            }
        )
    }

    /// Walk a straight-line (or nested) chain of control nodes
    /// starting at `start`, appending MIR ops to `out`. Stops at
    /// the first control node matching `stop`.
    ///
    /// Tracks `last_ctrl` so that when we stop at a merge point,
    /// we can emit Copy ops for any phis at that merge using the
    /// correct pred.
    fn walk_chain(&mut self, start: NodeId, stop: StopAt, out: &mut MirCodeBlock) {
        // Same (start, stop) always produces the same MIR,
        // full stop. `Mir::Skip` / `Break` / `Continue` inside
        // the cached output unwind to the NEAREST enclosing
        // `Mir::Block` / `Mir::Loop` at runtime — which is
        // exactly the caller's splice context. So enclosing
        // scope state doesn't affect the cached MIR's meaning;
        // the walk output is a pure function of its (start,
        // stop) pair.
        //
        // This is the key M9 insight: post-inline graphs have
        // the same sub-structures reached from many outer arm
        // paths. Without caching, each path re-walks the
        // sub-structure independently — exponential emission.
        // With caching, every (start, stop) walk hits after
        // the first population, turning the exponential into
        // linear.
        let key = (start, stop);
        if let Some(cached) = self.walk_cache.get(&key) {
            out.0.extend(cached.0.iter().cloned());
            self.mir_emitted += cached.0.len();
            return;
        }

        let mut local = MirCodeBlock(Vec::new());
        self.walk_chain_inner(start, stop, &mut local);
        let local_len = local.0.len();
        if self.walk_cache_ops + local_len <= WALK_CACHE_OP_BUDGET {
            self.walk_cache.insert(key, local.clone());
            self.walk_cache_ops += local_len;
        }
        out.0.extend(local.0);
    }

    fn walk_chain_inner(&mut self, start: NodeId, stop: StopAt, out: &mut MirCodeBlock) {
        let mut current = start;
        let mut last_ctrl = NodeId::INVALID;
        // Within one walk we should visit each control node at
        // most once. Bump-set on entry and panic on revisit —
        // catches cycles immediately instead of letting them
        // burn the 50M-step global budget.
        let mut visited: HashSet<NodeId> = HashSet::new();
        loop {
            self.step_budget += 1;
            if self.step_budget > SCHEDULE_STEP_LIMIT {
                panic!(
                    "soir::schedule: exceeded SCHEDULE_STEP_LIMIT ({}) — last walk: \
                     start={}, stop={:?}, cur={}",
                    SCHEDULE_STEP_LIMIT, start, stop, current
                );
            }
            if self.mir_emitted > SCHEDULE_MIR_SIZE_LIMIT {
                panic!(
                    "soir::schedule: exceeded SCHEDULE_MIR_SIZE_LIMIT ({}) — \
                     the graph is producing exponentially many MIR ops. Likely \
                     a deeply nested post-inline graph; needs a sharing-aware \
                     scheduler (emit shared sub-graphs into `Mir::Block` once).",
                    SCHEDULE_MIR_SIZE_LIMIT,
                );
            }
            if !visited.insert(current) {
                panic!(
                    "soir::schedule: cycle in walk_chain (start={}, stop={:?}, \
                     revisit at {})",
                    start, stop, current
                );
            }
            // Primary stop: the caller's designated end-of-walk
            // (post-match merge, loop header, block exit). Natural
            // fall-through into this node emits nothing beyond phi
            // copies — the caller's surrounding construct handles
            // the MIR op.
            if let StopAt::AtNode(limit) = stop {
                if current == limit {
                    if last_ctrl.is_valid() {
                        self.emit_phi_copies_for_pred(limit, last_ctrl, out);
                    }
                    return;
                }
            }
            // Secondary stops: enclosing scopes. If control is
            // about to cross INTO an outer scope's merge/exit,
            // emit the corresponding MIR terminator and stop.
            //
            // Scanned innermost-first. `Mir::Skip` unwinds to the
            // nearest Block; `Mir::Break` / `Mir::Continue` target
            // the enclosing loop.
            if self.block_exits.iter().rev().any(|e| *e == current) {
                if last_ctrl.is_valid() {
                    self.emit_phi_copies_for_pred(current, last_ctrl, out);
                }
                self.emit(out, Mir::Skip);
                return;
            }
            // Loop backedge / exit detection, only for *enclosing*
            // loops (the Loop node itself as a fresh target is
            // handled by the normal dispatch below).
            let mut hit_loop_edge: Option<bool> = None;
            for frame in self.loops.iter().rev() {
                if frame.header == current {
                    hit_loop_edge = Some(true); // continue
                    break;
                }
                if Some(current) == frame.exit {
                    hit_loop_edge = Some(false); // break
                    break;
                }
            }
            if let Some(is_continue) = hit_loop_edge {
                if is_continue {
                    if last_ctrl.is_valid() {
                        self.emit_phi_copies_for_pred(current, last_ctrl, out);
                    }
                    self.emit(out, Mir::Continue);
                } else {
                    self.emit(out, Mir::Break);
                }
                return;
            }
            let kind = self.g.get(current).kind.clone();
            match kind {
                NodeKind::Proj {
                    kind:
                        ProjKind::StartCtrl
                        | ProjKind::IfTrue
                        | ProjKind::IfFalse
                        | ProjKind::MatchArm(_),
                    ..
                } => {
                    last_ctrl = current;
                    current = self.ctrl_successor(current);
                }
                NodeKind::Region { .. } | NodeKind::LoopExit { .. } => {
                    last_ctrl = current;
                    current = self.ctrl_successor(current);
                }
                NodeKind::Block { .. } => {
                    // Entry to a block scope. Emit a Mir::Block
                    // around the body, walk the body with the
                    // paired BlockExit as the stop boundary.
                    let exit = self.find_block_exit(current);
                    let body_start = self.ctrl_successor(current);
                    let mut body = MirCodeBlock(Vec::new());
                    let saved = std::mem::take(&mut self.emitted_here);
                    self.block_exits.push(exit);
                    self.walk_chain(body_start, StopAt::AtNode(exit), &mut body);
                    self.block_exits.pop();
                    self.emitted_here = saved;
                    self.emit(out, Mir::Block(body));
                    last_ctrl = exit;
                    current = self.ctrl_successor(exit);
                }
                NodeKind::BlockExit { .. } => {
                    // Stepping THROUGH a block exit (as opposed
                    // to stopping at it, handled by the StopAt
                    // check at the top of the loop). This
                    // happens when the walker is past the block
                    // scope — just forward control.
                    last_ctrl = current;
                    current = self.ctrl_successor(current);
                }
                NodeKind::Loop { .. } => {
                    // Backedge re-entry of an enclosing loop is
                    // caught by the secondary-stop check at the
                    // top of this loop — if we got here, this is
                    // a FRESH loop.
                    self.emit_phi_entry_copies(current, out);
                    let exit = self.find_loop_exit(current);
                    let body_start = self.ctrl_successor(current);
                    let mut body = MirCodeBlock(Vec::new());
                    let saved_emitted = std::mem::take(&mut self.emitted_here);
                    self.loops.push(LoopFrame {
                        header: current,
                        exit,
                    });
                    self.walk_chain(body_start, StopAt::AtNode(current), &mut body);
                    self.loops.pop();
                    self.emitted_here = saved_emitted;
                    self.emit(out, Mir::Loop(body));
                    // Continue after the loop's exit Region.
                    match exit {
                        Some(e) => {
                            last_ctrl = e;
                            current = self.ctrl_successor(e);
                        }
                        None => return,
                    }
                }
                NodeKind::If { cond, .. } => {
                    let c_slot = self.ensure_value(cond, out);
                    let true_proj = self
                        .find_proj(current, &ProjKind::IfTrue)
                        .expect("If missing IfTrue proj");
                    let false_proj = self
                        .find_proj(current, &ProjKind::IfFalse)
                        .expect("If missing IfFalse proj");
                    let post = self.find_merge_region(current);
                    let stop_for_arms = post
                        .map(StopAt::AtNode)
                        .unwrap_or(StopAt::ReturnOrStop);
                    let mut t = MirCodeBlock(Vec::new());
                    let mut e = MirCodeBlock(Vec::new());
                    let saved = std::mem::take(&mut self.emitted_here);
                    self.walk_chain(true_proj, stop_for_arms, &mut t);
                    self.emitted_here = std::collections::HashSet::new();
                    self.walk_chain(false_proj, stop_for_arms, &mut e);
                    self.emitted_here = saved;
                    // Mir::If0 runs then-branch when cond==0;
                    // IfTrue fires when cond != 0, so swap arms.
                    self.emit(out, Mir::If0(c_slot, e, t));
                    match post {
                        Some(p) => {
                            last_ctrl = p;
                            current = self.ctrl_successor(p);
                        }
                        None => return,
                    }
                }
                NodeKind::Match {
                    scrut,
                    ref arm_values,
                    ..
                } => {
                    let s_slot = self.ensure_value(scrut, out);
                    let post = self.find_merge_region(current);
                    let stop_for_arms = post
                        .map(StopAt::AtNode)
                        .unwrap_or(StopAt::ReturnOrStop);
                    let mut arms: Vec<(MirCodeBlock, Vec<u8>)> = Vec::new();
                    for (i, vals) in arm_values.iter().enumerate() {
                        let arm_proj = self
                            .find_proj(current, &ProjKind::MatchArm(i as u32))
                            .expect("Match missing arm proj");
                        let mut arm_block = MirCodeBlock(Vec::new());
                        let saved = std::mem::take(&mut self.emitted_here);
                        self.walk_chain(arm_proj, stop_for_arms, &mut arm_block);
                        self.emitted_here = saved;
                        arms.push((arm_block, vals.clone()));
                    }
                    self.emit(out, Mir::Match(s_slot, arms));
                    match post {
                        Some(p) => {
                            last_ctrl = p;
                            current = self.ctrl_successor(p);
                        }
                        None => return,
                    }
                }
                NodeKind::ReadReg { reg, .. } => {
                    let val_proj = self
                        .find_proj(current, &ProjKind::ReadRegVal)
                        .expect("ReadReg missing ReadRegVal proj");
                    let slot = self.slot(val_proj);
                    self.emitted_here.insert(val_proj);
                    self.emit(out, Mir::ReadRegister(slot, reg));
                    last_ctrl = current;
                    current = self.ctrl_successor(current);
                }
                NodeKind::WriteReg { reg, val, .. } => {
                    let val_slot = self.ensure_value(val, out);
                    self.emit(out, Mir::WriteRegister(reg, Either::Right(val_slot)));
                    last_ctrl = current;
                    current = self.ctrl_successor(current);
                }
                NodeKind::Stop { .. } => {
                    self.emit(out, Mir::Stop);
                    return;
                }
                NodeKind::Return { values, .. } => {
                    let sig = self.g.sig().clone();
                    for (i, v) in values.iter().enumerate() {
                        let src = self.ensure_value(*v, out);
                        let dst = sig.input_count + i as u32;
                        if src != dst {
                            self.emit(out, Mir::Copy(dst, src));
                        }
                    }
                    return;
                }
                NodeKind::Call { .. } => panic!(
                    "soir::schedule: Call node reached; inline first (M4)"
                ),
                other => panic!(
                    "soir::schedule: unexpected control node {:?}",
                    other.tag()
                ),
            }
        }
    }

    /// Push a Mir op into `out` and bump the global emit counter.
    /// Centralising through this helper gives the scheduler a
    /// single place to enforce the MIR-size ceiling.
    fn emit(&mut self, out: &mut MirCodeBlock, op: Mir) {
        out.0.push(op);
        self.mir_emitted += 1;
    }

    /// Find the unique user of `node` that consumes it as a
    /// control edge. Deduplicates: a user can appear in the list
    /// twice when a node references `node` in multiple input
    /// positions (e.g. `BlockExit { preds: [node], block: node }`
    /// for an empty-body Block), but that's one logical successor.
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
                        "soir::schedule: multiple ctrl successors for {}: {} and {}",
                        node, prev, u
                    );
                }
                found = Some(u);
            }
        }
        found.unwrap_or_else(|| {
            panic!("soir::schedule: no ctrl successor for {}", node)
        })
    }

    fn consumes_ctrl(&self, user: NodeId, node: NodeId) -> bool {
        match &self.g.get(user).kind {
            NodeKind::Region { preds } => preds.contains(&node),
            NodeKind::BlockExit { preds, .. } => preds.contains(&node),
            NodeKind::LoopExit { preds, .. } => preds.contains(&node),
            NodeKind::Loop { entry, back } => *entry == node || *back == Some(node),
            NodeKind::Block { ctrl }
            | NodeKind::If { ctrl, .. }
            | NodeKind::Match { ctrl, .. }
            | NodeKind::ReadReg { ctrl, .. }
            | NodeKind::WriteReg { ctrl, .. }
            | NodeKind::Call { ctrl, .. }
            | NodeKind::Stop { ctrl, .. }
            | NodeKind::Return { ctrl, .. } => *ctrl == node,
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

    /// Given a branch (`If` / `Match`), find the unique `Region`
    /// where all arms reconverge. Returns `None` if the branch has
    /// no merge (every arm terminates via Return/Stop/Break).
    /// Result is memoized in `merge_cache` — deeply nested control
    /// graphs revisit the same branch many times during walks
    /// from different outer arms.
    fn find_merge_region(&mut self, branch: NodeId) -> Option<NodeId> {
        if let Some(&r) = self.merge_cache.get(&branch) {
            return r;
        }
        let r = self.find_merge_region_uncached(branch);
        self.merge_cache.insert(branch, r);
        r
    }

    fn find_merge_region_uncached(&self, branch: NodeId) -> Option<NodeId> {
        // BFS from each arm proj until we hit a Region reachable
        // from at least two arms — that's the merge. For our
        // structured graphs, every arm either reaches the same
        // merge or terminates.
        //
        // Simplification: walk from any arm proj, skipping
        // pass-through controls. If we ever hit a Region whose
        // preds include other arms' terminal nodes, that's the
        // merge.
        let arms: Vec<NodeId> = self
            .g
            .get(branch)
            .users
            .iter()
            .copied()
            .filter(|u| {
                matches!(
                    self.g.get(*u).kind,
                    NodeKind::Proj {
                        kind: ProjKind::IfTrue
                            | ProjKind::IfFalse
                            | ProjKind::MatchArm(_),
                        ..
                    }
                )
            })
            .collect();
        if arms.is_empty() {
            return None;
        }
        // Walk each arm's control chain, noting every Region
        // encountered; intersect.
        let mut regions_per_arm: Vec<std::collections::HashSet<NodeId>> = Vec::new();
        for arm in &arms {
            let mut set = std::collections::HashSet::new();
            self.collect_reachable_regions(*arm, &mut set);
            regions_per_arm.push(set);
        }
        // Intersection: first region common to all arms.
        let mut common = regions_per_arm[0].clone();
        for s in &regions_per_arm[1..] {
            common.retain(|r| s.contains(r));
        }
        // Only a plain `Region` qualifies as a match's "natural
        // fall-through" merge. If every arm instead escapes via
        // Skip (to a `BlockExit`) or Break (to a `LoopExit`),
        // the Match has no natural merge — arms terminate
        // themselves with the appropriate Mir op, and the walker
        // uses secondary-stop handling for that.
        common
            .into_iter()
            .find(|r| matches!(self.g.get(*r).kind, NodeKind::Region { .. }))
    }

    fn collect_reachable_regions(
        &self,
        start: NodeId,
        out: &mut std::collections::HashSet<NodeId>,
    ) {
        let mut seen: std::collections::HashSet<NodeId> =
            std::collections::HashSet::new();
        self.collect_reachable_regions_impl(start, out, &mut seen);
    }

    /// Worker that threads a shared `seen` set across recursive
    /// arm traversals. The original version had `seen` local to
    /// each call, which made deeply nested branches explored
    /// exponentially — `seen` MUST span the whole traversal so
    /// shared successor sub-graphs are visited once.
    fn collect_reachable_regions_impl(
        &self,
        start: NodeId,
        out: &mut std::collections::HashSet<NodeId>,
        seen: &mut std::collections::HashSet<NodeId>,
    ) {
        let mut current = start;
        loop {
            if !seen.insert(current) {
                return;
            }
            match &self.g.get(current).kind {
                NodeKind::Region { .. }
                | NodeKind::BlockExit { .. }
                | NodeKind::LoopExit { .. } => {
                    out.insert(current);
                    current = self.ctrl_successor(current);
                }
                NodeKind::Loop { .. } => {
                    return;
                }
                NodeKind::Block { .. } => {
                    current = self.ctrl_successor(current);
                }
                NodeKind::If { .. } | NodeKind::Match { .. } => {
                    let arms: Vec<NodeId> = self
                        .g
                        .get(current)
                        .users
                        .iter()
                        .copied()
                        .filter(|u| {
                            matches!(
                                self.g.get(*u).kind,
                                NodeKind::Proj {
                                    kind: ProjKind::IfTrue
                                        | ProjKind::IfFalse
                                        | ProjKind::MatchArm(_),
                                    ..
                                }
                            )
                        })
                        .collect();
                    for a in arms {
                        self.collect_reachable_regions_impl(a, out, seen);
                    }
                    return;
                }
                NodeKind::Return { .. } | NodeKind::Stop { .. } => return,
                _ => {
                    current = self.ctrl_successor(current);
                }
            }
        }
    }

    /// Find the `BlockExit` paired with a given `Block` node.
    /// Every `BlockExit` carries a back-reference to its owning
    /// `Block`, so this is a scan of `block_node`'s users.
    fn find_block_exit(&self, block_node: NodeId) -> NodeId {
        for &u in &self.g.get(block_node).users {
            if let NodeKind::BlockExit { block, .. } = &self.g.get(u).kind {
                if *block == block_node {
                    return u;
                }
            }
        }
        panic!(
            "soir::schedule: Block {} has no paired BlockExit — builder invariant broken",
            block_node
        );
    }

    /// Find the `LoopExit` paired with a given `Loop` node.
    /// Uses the explicit back-reference embedded in each
    /// `LoopExit` — same pattern as `find_block_exit`.
    /// Returns `None` for an infinite loop (no break_preds, so
    /// no LoopExit was ever allocated).
    fn find_loop_exit(&self, loop_node: NodeId) -> Option<NodeId> {
        for &u in &self.g.get(loop_node).users {
            if let NodeKind::LoopExit { loop_node: lp, .. } = &self.g.get(u).kind {
                if *lp == loop_node {
                    return Some(u);
                }
            }
        }
        None
    }

    /// Is `node` reachable from `loop_node`'s body (i.e., a
    /// descendant in the control subgraph, not crossing out of
    /// the loop)? Now only used for edge-case debugging — the
    /// explicit `LoopExit` back-reference removed the original
    /// caller.
    #[allow(dead_code)]
    fn is_inside_loop(&self, node: NodeId, loop_node: NodeId) -> bool {
        // Conservative check: walk backward from `node` through
        // control preds. If we reach `loop_node`, yes.
        let mut stack = vec![node];
        let mut seen: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        while let Some(n) = stack.pop() {
            if !seen.insert(n) {
                continue;
            }
            if n == loop_node {
                return true;
            }
            match &self.g.get(n).kind {
                NodeKind::Region { preds } | NodeKind::BlockExit { preds, .. } => {
                    stack.extend(preds)
                }
                NodeKind::Loop { entry, back } => {
                    stack.push(*entry);
                    let _ = back;
                }
                NodeKind::Block { ctrl }
                | NodeKind::If { ctrl, .. }
                | NodeKind::Match { ctrl, .. }
                | NodeKind::ReadReg { ctrl, .. }
                | NodeKind::WriteReg { ctrl, .. }
                | NodeKind::Call { ctrl, .. }
                | NodeKind::Stop { ctrl, .. }
                | NodeKind::Return { ctrl, .. } => stack.push(*ctrl),
                NodeKind::Proj { of, .. } => stack.push(*of),
                _ => {}
            }
        }
        false
    }

    /// Emit Copy ops for every `Phi` at `region` using the value
    /// lane corresponding to `pred`.
    fn emit_phi_copies_for_pred(
        &mut self,
        region: NodeId,
        pred: NodeId,
        out: &mut MirCodeBlock,
    ) {
        let phis: Vec<NodeId> = self
            .g
            .get(region)
            .users
            .iter()
            .copied()
            .filter(|u| matches!(self.g.get(*u).kind, NodeKind::Phi { .. }))
            .collect();
        if phis.is_empty() {
            return;
        }
        let idx = self.pred_index(region, pred);
        for phi in phis {
            let (src_val_opt, dst_slot) = match &self.g.get(phi).kind {
                NodeKind::Phi { values, .. } => {
                    (values.get(idx).copied().flatten(), self.slot(phi))
                }
                _ => unreachable!(),
            };
            if let Some(src) = src_val_opt {
                let src_slot = self.ensure_value(src, out);
                if src_slot != dst_slot {
                    self.emit(out, Mir::Copy(dst_slot, src_slot));
                }
            }
        }
    }

    /// Emit entry-value Copy ops for phis at a Loop header.
    /// Called *before* the `Mir::Loop` opens.
    fn emit_phi_entry_copies(&mut self, loop_header: NodeId, out: &mut MirCodeBlock) {
        let entry_pred = match &self.g.get(loop_header).kind {
            NodeKind::Loop { entry, .. } => *entry,
            _ => panic!(),
        };
        self.emit_phi_copies_for_pred(loop_header, entry_pred, out);
    }

    fn pred_index(&self, region: NodeId, pred: NodeId) -> usize {
        match &self.g.get(region).kind {
            NodeKind::Region { preds }
            | NodeKind::BlockExit { preds, .. }
            | NodeKind::LoopExit { preds, .. } => preds
                .iter()
                .position(|p| *p == pred)
                .expect("pred in region"),
            NodeKind::Loop { entry, back } => {
                if *entry == pred {
                    0
                } else if Some(pred) == *back {
                    1
                } else {
                    panic!(
                        "soir::schedule: pred_index({}, {}): {} is not a pred of the loop",
                        region, pred, pred
                    )
                }
            }
            _ => panic!(),
        }
    }
}
