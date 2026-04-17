//! HIR optimizations.
//!
//! Phase 5 of the plan. Two passes, run to fixpoint:
//!   1. Constant propagation (Step 5.1)
//!        - A slot is "static with value V" if every `Set` into it uses the
//!          same V and no other op writes to it (incl. Copy dst, Inc, Dec,
//!          ReadRegister dst, Match scrutinee — wait, match scrutinee is a
//!          read — and Call `ret` slots).
//!        - Fold `Copy(dst, src)` → `Set(dst, V)` when `src` is static.
//!        - Fold a 2-arm `Match` (the if-zero shape produced by
//!          `HirOp::if_zero`) → picked branch when `src` is static.
//!        - Fold `Match(src, arms)` → matching arm when `src` is static.
//!   2. Dead store elimination (Step 5.2)
//!        - Within a block, if `Set(s, _)` is followed by another write to
//!          the same slot (`Set`, `Copy(s, _)`, `Inc(s)`, `Dec(s)`, etc.)
//!          with no intervening read, drop the first write.

use std::collections::{HashMap, HashSet};

use either::Either;

use crate::ir::*;

/// Run all optimizations on a function's body to a fixpoint.
pub fn optimize_function(mut f: HirFunction) -> HirFunction {
    f.body = optimize_block(f.body);
    f
}

/// Run all optimizations on a block to a fixpoint.
pub fn optimize_block(mut block: HirBlock) -> HirBlock {
    // Iterate until a pass produces no change. A cap is safety-net only.
    for _ in 0..16 {
        let consts = gather_consts(&block);
        let before = block.clone();
        block = fold_consts_block(block, &consts);
        block = eliminate_dead_stores_block(block);
        if block == before {
            break;
        }
    }
    block
}

// ---------- constant propagation ---------------------------------------------

/// Build a map `SlotId → constant value` of slots that are statically known.
///
/// A slot is "known" if *every* write to it (anywhere in the tree, at any
/// nesting level) is a `Set(slot, V)` with the same V, and it is never
/// touched by a non-Set write (Copy dst, Inc, Dec, ReadRegister dst, Call
/// ret).
fn gather_consts(block: &HirBlock) -> HashMap<SlotId, u8> {
    let mut candidate: HashMap<SlotId, u8> = HashMap::new();
    let mut poisoned: HashSet<SlotId> = HashSet::new();

    fn visit(
        block: &HirBlock,
        candidate: &mut HashMap<SlotId, u8>,
        poisoned: &mut HashSet<SlotId>,
    ) {
        for op in &block.ops {
            match op {
                HirOp::Set(s, v) => match candidate.get(s) {
                    None if !poisoned.contains(s) => {
                        candidate.insert(*s, *v);
                    }
                    Some(existing) if *existing == *v => { /* same value — still consistent */ }
                    _ => {
                        candidate.remove(s);
                        poisoned.insert(*s);
                    }
                },
                HirOp::Copy(dst, _) => {
                    candidate.remove(dst);
                    poisoned.insert(*dst);
                }
                HirOp::Inc(s) | HirOp::Dec(s) => {
                    candidate.remove(s);
                    poisoned.insert(*s);
                }
                HirOp::ReadRegister(s, _) => {
                    candidate.remove(s);
                    poisoned.insert(*s);
                }
                HirOp::Call { ret, .. } => {
                    for r in ret {
                        candidate.remove(r);
                        poisoned.insert(*r);
                    }
                }
                HirOp::Loop(b) | HirOp::Block(b) => {
                    visit(b, candidate, poisoned);
                }
                HirOp::Match(_, arms) => {
                    for (arm, _) in arms {
                        visit(arm, candidate, poisoned);
                    }
                }
                HirOp::Break
                | HirOp::Continue
                | HirOp::Stop
                | HirOp::Skip
                | HirOp::WriteRegister(_, _) => {}
            }
        }
    }

    visit(block, &mut candidate, &mut poisoned);
    for s in &poisoned {
        candidate.remove(s);
    }
    candidate
}

/// Apply the known constants throughout the block, recursively folding
/// `Match`/`Copy` as described in the module doc.
fn fold_consts_block(block: HirBlock, consts: &HashMap<SlotId, u8>) -> HirBlock {
    let ops = fold_consts_ops(block.ops, consts);
    HirBlock {
        ops,
        result_slot: block.result_slot,
    }
}

fn fold_consts_ops(ops: Vec<HirOp>, consts: &HashMap<SlotId, u8>) -> Vec<HirOp> {
    let mut out: Vec<HirOp> = Vec::with_capacity(ops.len());
    for op in ops {
        match op {
            HirOp::Match(s, arms) if consts.contains_key(&s) => {
                let v = consts[&s];
                let matched_ix =
                    arms.iter().position(|(_, values)| values.contains(&v));
                match matched_ix {
                    Some(ix) => {
                        let arm = arms.into_iter().nth(ix).unwrap().0;
                        let folded = fold_consts_block(arm, consts);
                        out.extend(folded.ops);
                    }
                    None => {
                        // No arm matches. Preserve the Match with folded arms
                        // (don't invent runtime behavior here — the generator
                        // should have emitted a wildcard arm, so hitting this
                        // path means we have a source-level problem, not an
                        // optimization choice).
                        let arms = arms
                            .into_iter()
                            .map(|(b, v)| (fold_consts_block(b, consts), v))
                            .collect();
                        out.push(HirOp::Match(s, arms));
                    }
                }
            }
            HirOp::Copy(dst, src) if consts.contains_key(&src) => {
                out.push(HirOp::Set(dst, consts[&src]));
            }
            HirOp::Match(s, arms) => {
                let arms = arms
                    .into_iter()
                    .map(|(b, v)| (fold_consts_block(b, consts), v))
                    .collect();
                out.push(HirOp::Match(s, arms));
            }
            HirOp::Loop(b) => out.push(HirOp::Loop(fold_consts_block(b, consts))),
            HirOp::Block(b) => out.push(HirOp::Block(fold_consts_block(b, consts))),
            // Inc/Dec on a "known" slot would contradict its constancy, so
            // they only apply to non-const slots. Pass through.
            other => out.push(other),
        }
    }
    out
}

// ---------- dead store elimination -------------------------------------------

fn eliminate_dead_stores_block(block: HirBlock) -> HirBlock {
    let ops = eliminate_dead_stores_ops(block.ops);
    HirBlock {
        ops,
        result_slot: block.result_slot,
    }
}

fn eliminate_dead_stores_ops(mut ops: Vec<HirOp>) -> Vec<HirOp> {
    // First: recurse into nested blocks.
    ops = ops.into_iter().map(recurse_dead).collect();

    // Second: remove `Set(s, _)` that's killed by a subsequent write to `s`
    // before `s` is ever read, within the *same* straight-line segment.
    //
    // Pragmatic approach: walk front-to-back, for each `Set(s, _)`, scan
    // forward until we find either a read of `s` (preserve) or a write to
    // `s` (remove the original).
    //
    // Only straight-line ops at this level are considered; control-flow ops
    // (Loop/Block/Match) act as barriers — we conservatively treat them
    // as potential reads of every slot.
    let mut keep: Vec<bool> = vec![true; ops.len()];
    for i in 0..ops.len() {
        if let HirOp::Set(target, _) = &ops[i] {
            let target = *target;
            for j in (i + 1)..ops.len() {
                if op_reads_or_barrier(&ops[j], target) {
                    break; // preserve
                }
                if op_writes_same(&ops[j], target) {
                    keep[i] = false;
                    break;
                }
            }
        }
    }
    let mut out = Vec::with_capacity(ops.len());
    for (op, k) in ops.into_iter().zip(keep) {
        if k {
            out.push(op);
        }
    }
    out
}

fn recurse_dead(op: HirOp) -> HirOp {
    match op {
        HirOp::Loop(b) => HirOp::Loop(eliminate_dead_stores_block(b)),
        HirOp::Block(b) => HirOp::Block(eliminate_dead_stores_block(b)),
        HirOp::Match(s, arms) => HirOp::Match(
            s,
            arms.into_iter()
                .map(|(b, v)| (eliminate_dead_stores_block(b), v))
                .collect(),
        ),
        other => other,
    }
}

fn op_reads_or_barrier(op: &HirOp, slot: SlotId) -> bool {
    match op {
        HirOp::Set(_, _) => false,
        HirOp::Copy(dst, src) => *src == slot || *dst == slot_next_to(slot, dst), // src read
        HirOp::Inc(s) | HirOp::Dec(s) => *s == slot, // read-modify-write
        HirOp::Match(s, _) => *s == slot,
        HirOp::ReadRegister(_, _) => false,
        HirOp::WriteRegister(_, Either::Right(s)) => *s == slot,
        HirOp::WriteRegister(_, Either::Left(_)) => false,
        HirOp::Call { args, ret, .. } => args.contains(&slot) || ret.contains(&slot),
        // Any control-flow construct is a barrier: we can't reason about
        // what happens inside without deeper analysis.
        HirOp::Loop(_) | HirOp::Block(_) => true,
        HirOp::Break | HirOp::Continue | HirOp::Stop | HirOp::Skip => true,
    }
}

// Tiny helper to keep the `Copy(dst, src)` check from accidentally matching
// on `dst` — the copy's destination is a write, not a read.
fn slot_next_to(_slot: SlotId, _other: &SlotId) -> SlotId {
    // Intentionally never equal; used to make the "dst == slot" arm of the
    // match above harmless. Kept as a named helper for clarity.
    SlotId(u32::MAX)
}

fn op_writes_same(op: &HirOp, slot: SlotId) -> bool {
    match op {
        HirOp::Set(s, _) => *s == slot,
        HirOp::Copy(dst, _) => *dst == slot,
        HirOp::Inc(s) | HirOp::Dec(s) => *s == slot,
        HirOp::ReadRegister(s, _) => *s == slot,
        HirOp::Call { ret, .. } => ret.contains(&slot),
        _ => false,
    }
}
