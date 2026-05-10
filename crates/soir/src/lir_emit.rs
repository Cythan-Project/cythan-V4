//! `soir::Graph` → `Vec<lir::CompilableInstruction>` (CFG-style).
//!
//! # Why bypass MIR
//!
//! MIR is structured (`Mir::Block` / `Mir::Loop` / `Mir::Match` with
//! per-arm bodies, no goto). Sharing a sub-graph across multiple
//! incoming control paths requires either textual duplication
//! (exponential MIR for nested matches in inlined graphs) or a
//! relooper-style trampoline emitted on top.
//!
//! LIR has the primitives we actually need: `Label`, `Jump(label)`,
//! `If0(var, label)`, `Match(var, [Option<Label>; 16])`. Emitting
//! a CFG to LIR gives us automatic sharing — each soir control
//! node is emitted ONCE under its own label, and every transition
//! becomes a jump. The pattern is identical to how LLVM / Cranelift
//! / GCC schedule SSA: emit each block once, terminators are
//! explicit jumps, phis become `Copy`s on the predecessor's exit
//! edge.
//!
//! # Algorithm
//!
//! Worklist of soir control nodes. For each:
//!
//! 1. Emit `Label(label_for_node)`.
//! 2. Emit any pure-data ops needed by the node's terminator
//!    (Const → `Copy`, Inc/Dec → `Increment`/`Decrement`).
//! 3. Emit phi `Copy` ops for any phi at the node's successor
//!    region — they live on this pred's exit edge.
//! 4. Emit the terminator: `Jump` for sequential/merge, `If0`
//!    for `If`, `Match` for `Match`, `Stop` for `Stop`/`Return`.
//! 5. Push the successor(s) onto the worklist.
//!
//! # Slot allocation
//!
//! Each soir value-producing node gets a unique LIR `Var` slot.
//! Inputs land at slots `0..input_count` (matching `Param(i)`
//! projections). Outputs at `input_count..total_slots()`. Temps
//! after that.

use std::collections::{HashMap, HashSet, VecDeque};

use lir::{AsmValue, CompilableInstruction, Counter, Label, LabelType, Number, Var};

use crate::ir::{Graph, NodeId, NodeKind, ProjKind};

/// Hard ceiling on emitted LIR instructions. The CFG-style
/// emission is bounded by graph size (each node emits a constant
/// number of LIR ops), so a graph of N nodes should produce
/// O(N) LIR. Hitting this means a bug.
pub const LIR_EMIT_LIMIT: usize = 5_000_000;

pub fn schedule_lir(g: &Graph) -> Vec<CompilableInstruction> {
    let mut s = LirEmit::new(g);
    let start_ctrl = s
        .find_proj(g.start(), &ProjKind::StartCtrl)
        .expect("graph missing StartCtrl");
    // Initial: jump from "before any code" to the entry block.
    let entry_label = s.label_for(start_ctrl);
    s.out.push(CompilableInstruction::Jump(entry_label));
    s.queue.push_back(start_ctrl);
    while let Some(node) = s.queue.pop_front() {
        if !s.emitted.insert(node) {
            continue;
        }
        if s.out.len() > LIR_EMIT_LIMIT {
            panic!(
                "soir::lir_emit: exceeded LIR_EMIT_LIMIT ({}) — likely a \
                 bug in the worklist or value emission",
                LIR_EMIT_LIMIT
            );
        }
        s.emit_block(node);
    }
    s.out
}

struct LirEmit<'g> {
    g: &'g Graph,
    counter: Counter,
    /// One LIR Label per soir control node — emitted exactly
    /// once at that node's block start. Terminators in other
    /// blocks `Jump` to it.
    labels: HashMap<NodeId, Label>,
    /// Stable LIR slot per soir value-producing node.
    slot_of: HashMap<NodeId, u32>,
    next_slot: u32,
    /// Pure-data nodes already emitted ANYWHERE — their LIR
    /// slot holds the right value, no need to re-emit.
    /// (LIR variables are global, so once a value is materialised
    /// it persists across labels.)
    materialised_value: HashSet<NodeId>,
    /// Control nodes whose blocks have been emitted.
    emitted: HashSet<NodeId>,
    queue: VecDeque<NodeId>,
    out: Vec<CompilableInstruction>,
}

impl<'g> LirEmit<'g> {
    fn new(g: &'g Graph) -> Self {
        let sig = g.sig();
        let mut slot_of: HashMap<NodeId, u32> = HashMap::new();
        let mut materialised_value: HashSet<NodeId> = HashSet::new();
        // Seed input-slot bindings from Proj(Param(i)).
        for (id, n) in g.iter() {
            if let NodeKind::Proj {
                of,
                kind: ProjKind::Param(i),
            } = &n.kind
            {
                if *of == g.start() {
                    slot_of.insert(id, *i);
                    // Param values are pre-materialised by the caller.
                    materialised_value.insert(id);
                }
            }
        }
        let next_slot = sig.total_slots();
        Self {
            g,
            counter: Counter::new(),
            labels: HashMap::new(),
            slot_of,
            next_slot,
            materialised_value,
            emitted: HashSet::new(),
            queue: VecDeque::new(),
            out: Vec::new(),
        }
    }

    fn label_for(&mut self, node: NodeId) -> Label {
        if let Some(l) = self.labels.get(&node) {
            return l.clone();
        }
        let lt = match &self.g.get(node).kind {
            NodeKind::Loop { .. } => LabelType::LoopStart,
            NodeKind::If { .. } | NodeKind::Match { .. } => LabelType::IfStart,
            NodeKind::BlockExit { .. } => LabelType::BlockEnd,
            NodeKind::LoopExit { .. } => LabelType::LoopEnd,
            _ => LabelType::Match,
        };
        let l = Label::alloc(&mut self.counter, lt);
        self.labels.insert(node, l.clone());
        l
    }

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

    fn ensure_value(&mut self, node: NodeId) -> u32 {
        // For nodes that are *non-recomputable* (Param,
        // ReadRegVal, Phi/EffPhi, CallRet, Eff): the slot is
        // populated externally — by the caller, by an effectful
        // instruction, or by a `Copy` on the pred edge. We just
        // return the slot.
        //
        // For pure recomputable ops (Const, Inc/Dec/Add/Sub/Eq):
        // we ALWAYS re-emit at each use site. The CFG-style
        // emission jumps freely between blocks; a pure op
        // emitted in block A isn't guaranteed to have run by
        // the time its slot is read in block B (different jump
        // path). Re-emitting at every use is dumb but always
        // correct — proper GCM would place each pure op at the
        // dominator of its uses. M9c work.
        let kind = self.g.get(node).kind.clone();
        match kind {
            NodeKind::Const(v) => {
                let slot = self.slot(node);
                self.out.push(CompilableInstruction::Copy(
                    Var(slot as usize),
                    AsmValue::Number(Number(v)),
                ));
                slot
            }
            NodeKind::Phi { .. } | NodeKind::EffPhi { .. } => self.slot(node),
            NodeKind::Proj {
                kind: ProjKind::Param(_),
                ..
            } => *self.slot_of.get(&node).expect("Param pre-seeded"),
            NodeKind::Proj {
                kind: ProjKind::ReadRegVal,
                ..
            } => self.slot(node),
            NodeKind::Proj {
                kind: ProjKind::CallRet(_),
                ..
            } => panic!("soir::lir_emit: Call survived to LIR — inline first"),
            NodeKind::Proj {
                kind: ProjKind::Eff | ProjKind::StartEff,
                ..
            } => self.slot(node),
            NodeKind::Add(_, _) | NodeKind::Sub(_, _) | NodeKind::Eq(_, _) => {
                panic!(
                    "soir::lir_emit: {:?} has no direct LIR lowering yet — \
                     M6/M7 rewrites should expand it earlier",
                    kind
                );
            }
            other => panic!(
                "soir::lir_emit: ensure_value on non-data node {:?}",
                other
            ),
        }
    }

    /// Emit the phi `Copy` ops that belong on the edge from
    /// `pred` to `region`.
    fn emit_phi_copies_for_pred(&mut self, region: NodeId, pred: NodeId) {
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
            let val_opt = match &self.g.get(phi).kind {
                NodeKind::Phi { values, .. } => values.get(idx).copied().flatten(),
                _ => None,
            };
            if let Some(src) = val_opt {
                let src_slot = self.ensure_value(src);
                let dst_slot = self.slot(phi);
                if src_slot != dst_slot {
                    self.out.push(CompilableInstruction::Copy(
                        Var(dst_slot as usize),
                        AsmValue::Var(Var(src_slot as usize)),
                    ));
                }
                // Mark the phi as "materialised" globally — its
                // slot now holds a valid value at any label that
                // can be jumped to from this pred.
                self.materialised_value.insert(phi);
            }
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
                        "soir::lir_emit: pred {} not found in {} preds {:?}",
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
                        "soir::lir_emit: pred {} not loop {} entry/back",
                        pred, region
                    )
                }
            }
            k => panic!("soir::lir_emit: pred_index on {:?}", k.tag()),
        }
    }

    /// All immediate control successors of `node`, in a stable
    /// order. Used to enqueue blocks for emission.
    fn ctrl_successors(&self, node: NodeId) -> Vec<NodeId> {
        let mut found: Vec<NodeId> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        for &u in &self.g.get(node).users {
            if !seen.insert(u) {
                continue;
            }
            if self.consumes_ctrl(u, node) {
                found.push(u);
            }
        }
        found
    }

    fn ctrl_successor(&self, node: NodeId) -> NodeId {
        let succs = self.ctrl_successors(node);
        if succs.len() != 1 {
            panic!(
                "soir::lir_emit: expected exactly 1 ctrl successor for {} ({}), got {:?}",
                node,
                self.g.get(node).kind.tag(),
                succs,
            );
        }
        succs[0]
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

    fn jump_to(&mut self, target: NodeId) {
        let l = self.label_for(target);
        self.out.push(CompilableInstruction::Jump(l));
        self.queue.push_back(target);
    }

    fn emit_block(&mut self, node: NodeId) {
        let label = self.label_for(node);
        self.out.push(CompilableInstruction::Label(label));
        let kind = self.g.get(node).kind.clone();
        match kind {
            // ---- pass-through control nodes (no body, single
            //      successor) — emit phi copies + jump.
            NodeKind::Proj {
                kind:
                    ProjKind::StartCtrl
                    | ProjKind::IfTrue
                    | ProjKind::IfFalse
                    | ProjKind::MatchArm(_),
                ..
            }
            | NodeKind::Region { .. }
            | NodeKind::BlockExit { .. }
            | NodeKind::LoopExit { .. }
            | NodeKind::Block { .. }
            | NodeKind::Loop { .. } => {
                let succ = self.ctrl_successor(node);
                self.emit_phi_copies_for_pred(succ, node);
                self.jump_to(succ);
            }
            // ---- effectful control-pinned ops.
            NodeKind::ReadReg { reg, .. } => {
                let val_proj = self
                    .find_proj(node, &ProjKind::ReadRegVal)
                    .expect("ReadReg without ReadRegVal proj");
                let slot = self.slot(val_proj);
                self.out.push(CompilableInstruction::ReadRegister(
                    Var(slot as usize),
                    Number(reg),
                ));
                self.materialised_value.insert(val_proj);
                let succ = self.ctrl_successor(node);
                self.emit_phi_copies_for_pred(succ, node);
                self.jump_to(succ);
            }
            NodeKind::WriteReg { reg, val, .. } => {
                let val_slot = self.ensure_value(val);
                self.out.push(CompilableInstruction::WriteRegister(
                    Number(reg),
                    AsmValue::Var(Var(val_slot as usize)),
                ));
                let succ = self.ctrl_successor(node);
                self.emit_phi_copies_for_pred(succ, node);
                self.jump_to(succ);
            }
            // ---- branches.
            NodeKind::If { cond, .. } => {
                let c_slot = self.ensure_value(cond);
                let true_proj = self
                    .find_proj(node, &ProjKind::IfTrue)
                    .expect("If missing IfTrue proj");
                let false_proj = self
                    .find_proj(node, &ProjKind::IfFalse)
                    .expect("If missing IfFalse proj");
                let true_label = self.label_for(true_proj);
                let false_label = self.label_for(false_proj);
                // `If0(var, label)` jumps to `label` when var == 0.
                // We want cond==0 → false branch, cond!=0 → true.
                self.out
                    .push(CompilableInstruction::If0(Var(c_slot as usize), false_label));
                self.out.push(CompilableInstruction::Jump(true_label));
                self.queue.push_back(true_proj);
                self.queue.push_back(false_proj);
            }
            NodeKind::Match {
                scrut,
                ref arm_values,
                ..
            } => {
                let s_slot = self.ensure_value(scrut);
                // Build the 16-element jump table. Each arm proj
                // has its own label; values not covered by any
                // arm are unreachable (we'll point them at an
                // arbitrary arm to satisfy LIR's `[Option<Label>; 16]`
                // — value-set arithmetic guarantees they're dead).
                let mut targets: [Option<Label>; 16] = Default::default();
                let mut fallback: Option<Label> = None;
                for (i, vals) in arm_values.iter().enumerate() {
                    let arm_proj = self
                        .find_proj(node, &ProjKind::MatchArm(i as u32))
                        .expect("Match missing arm proj");
                    let arm_label = self.label_for(arm_proj);
                    if fallback.is_none() {
                        fallback = Some(arm_label.clone());
                    }
                    for &v in vals {
                        if (v as usize) < 16 {
                            targets[v as usize] = Some(arm_label.clone());
                        }
                    }
                    self.queue.push_back(arm_proj);
                }
                // Fill any uncovered slots with the fallback so
                // every u4 value has a defined target. They're
                // statically unreachable by construction.
                if let Some(fb) = fallback {
                    for slot in targets.iter_mut() {
                        if slot.is_none() {
                            *slot = Some(fb.clone());
                        }
                    }
                }
                self.out
                    .push(CompilableInstruction::Match(Var(s_slot as usize), targets));
            }
            // ---- terminals.
            NodeKind::Stop { .. } => {
                self.out.push(CompilableInstruction::Stop);
            }
            NodeKind::Return { values, .. } => {
                let sig = self.g.sig().clone();
                for (i, v) in values.iter().enumerate() {
                    let src = self.ensure_value(*v);
                    let dst = sig.input_count + i as u32;
                    if src != dst {
                        self.out.push(CompilableInstruction::Copy(
                            Var(dst as usize),
                            AsmValue::Var(Var(src as usize)),
                        ));
                    }
                }
                self.out.push(CompilableInstruction::Stop);
            }
            NodeKind::Call { .. } => panic!(
                "soir::lir_emit: Call node reached LIR — inline before scheduling"
            ),
            other => panic!(
                "soir::lir_emit: unexpected control node {:?}",
                other.tag()
            ),
        }
    }
}
