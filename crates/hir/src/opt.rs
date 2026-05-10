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
                HirOp::MapValue(_src, dst, _) => {
                    candidate.remove(dst);
                    poisoned.insert(*dst);
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

/// Live-variable-based dead-write elimination. A write (`Set`,
/// `Copy`, `Inc`, `Dec`, `ReadRegister`) whose target isn't read on
/// any path from the op to the block's end (given some live-at-exit
/// set) is removed.
///
/// Unlike `eliminate_dead_stores_block`, this catches writes that
/// are never read at all — not just the ones immediately overwritten.
///
/// `live_at_exit` is the set of slots the caller considers
/// observable after this block. At the top-level program body,
/// that's the output slots (cells written to by `_ret` params).
pub fn eliminate_dead_writes(block: HirBlock, live_at_exit: &HashSet<SlotId>) -> HirBlock {
    let ctx = ExitCtx::default();
    let ops = ldve_ops(block.ops, live_at_exit, &ctx);
    HirBlock {
        ops,
        result_slot: block.result_slot,
    }
}

/// Liveness at the various non-fall-through exits available at the
/// current nesting level. Each is the live-at point reached when
/// the corresponding control-flow op is executed.
///
/// `None` means "no enclosing construct of this kind" — executing
/// the op would be ill-formed at this position. We treat such cases
/// as `{}` (nothing live).
#[derive(Default, Clone)]
struct ExitCtx {
    /// Live at the program-end (Stop). Always `{}`.
    /// (Held implicitly — Stop just resets to empty.)
    /// Live just past the nearest enclosing `Loop` (where `Break` jumps to).
    break_to: Option<HashSet<SlotId>>,
    /// Live at the start of the nearest enclosing `Loop` body
    /// (where `Continue` jumps to). Conservatively the body's
    /// live-at-exit (which includes everything the body reads).
    continue_to: Option<HashSet<SlotId>>,
    /// Live just past the nearest enclosing `Block` (where `Skip` jumps to).
    skip_to: Option<HashSet<SlotId>>,
}

fn ldve_ops(
    ops: Vec<HirOp>,
    live_at_exit: &HashSet<SlotId>,
    ctx: &ExitCtx,
) -> Vec<HirOp> {
    // Walk backward. `live` is the liveness at the *current* point
    // (just before the next op walked but after all ops already
    // walked). Control-flow exits (Break/Continue/Stop/Skip) RESET
    // `live` to the appropriate exit-point liveness, since code
    // after them is unreachable.
    let mut live: HashSet<SlotId> = live_at_exit.clone();
    let mut rev_kept: Vec<Option<HirOp>> = Vec::with_capacity(ops.len());
    for op in ops.into_iter().rev() {
        rev_kept.push(process_op_backward(op, &mut live, ctx));
    }
    rev_kept.into_iter().rev().flatten().collect()
}

fn process_op_backward(
    op: HirOp,
    live: &mut HashSet<SlotId>,
    ctx: &ExitCtx,
) -> Option<HirOp> {
    match op {
        HirOp::Set(s, v) => {
            if !live.contains(&s) {
                None
            } else {
                live.remove(&s);
                Some(HirOp::Set(s, v))
            }
        }
        HirOp::Copy(dst, src) => {
            if !live.contains(&dst) {
                None
            } else {
                live.remove(&dst);
                live.insert(src);
                Some(HirOp::Copy(dst, src))
            }
        }
        HirOp::MapValue(src, dst, table) => {
            if !live.contains(&dst) {
                None
            } else {
                live.remove(&dst);
                live.insert(src);
                Some(HirOp::MapValue(src, dst, table))
            }
        }
        HirOp::ReadRegister(dst, r) => {
            if !live.contains(&dst) {
                None
            } else {
                live.remove(&dst);
                Some(HirOp::ReadRegister(dst, r))
            }
        }
        HirOp::WriteRegister(r, src) => {
            // Side effect (print / input trigger) — always keep.
            if let Either::Right(s) = &src {
                live.insert(*s);
            }
            Some(HirOp::WriteRegister(r, src))
        }
        HirOp::Match(s, arms) => {
            // Each arm's fall-through live_at_exit = current `live`.
            // Arms with internal Break/Continue/Stop/Skip will
            // reset their walking-live as they go, so passing the
            // fall-through value is correct for arms that *do*
            // fall through and harmless for ones that don't.
            let arms_new: Vec<(HirBlock, Vec<u8>)> = arms
                .into_iter()
                .map(|(b, vs)| {
                    let new_b = HirBlock {
                        ops: ldve_ops(b.ops, live, ctx),
                        result_slot: b.result_slot,
                    };
                    (new_b, vs)
                })
                .collect();
            // Post-walk: the scrutinee is read; we conservatively
            // treat anything still read in any arm as live before
            // the match (some of those reads may be unreachable in
            // a given arm, but this is sound and cheap).
            for (b, _) in &arms_new {
                for s2 in slots_read_recursive(b) {
                    live.insert(s2);
                }
            }
            live.insert(s);
            Some(HirOp::Match(s, arms_new))
        }
        HirOp::Loop(body) => {
            // Body's fall-through is unreachable (loops only exit
            // via Break/Continue/Stop). Set live_at_exit_of_body
            // to a conservative superset that's also valid as
            // continue_to (loop-start liveness): the *outer*
            // live (where a Break would jump) ∪ everything the
            // body reads. That guarantees writes feeding next-
            // iteration reads are preserved.
            let mut body_live = live.clone();
            for s2 in slots_read_recursive(&body) {
                body_live.insert(s2);
            }
            let body_ctx = ExitCtx {
                break_to: Some(live.clone()),
                continue_to: Some(body_live.clone()),
                skip_to: ctx.skip_to.clone(),
            };
            let body_new = HirBlock {
                ops: ldve_ops(body.ops, &body_live, &body_ctx),
                result_slot: body.result_slot,
            };
            for s2 in slots_read_recursive(&body_new) {
                live.insert(s2);
            }
            Some(HirOp::Loop(body_new))
        }
        HirOp::Block(body) => {
            // Block falls through normally; Skip jumps past it.
            // Both land at the post-Block live = current `live`.
            let body_ctx = ExitCtx {
                break_to: ctx.break_to.clone(),
                continue_to: ctx.continue_to.clone(),
                skip_to: Some(live.clone()),
            };
            let body_new = HirBlock {
                ops: ldve_ops(body.ops, live, &body_ctx),
                result_slot: body.result_slot,
            };
            for s2 in slots_read_recursive(&body_new) {
                live.insert(s2);
            }
            Some(HirOp::Block(body_new))
        }
        HirOp::Break => {
            *live = ctx.break_to.clone().unwrap_or_default();
            Some(HirOp::Break)
        }
        HirOp::Continue => {
            *live = ctx.continue_to.clone().unwrap_or_default();
            Some(HirOp::Continue)
        }
        HirOp::Stop => {
            live.clear();
            Some(HirOp::Stop)
        }
        HirOp::Skip => {
            *live = ctx.skip_to.clone().unwrap_or_default();
            Some(HirOp::Skip)
        }
        HirOp::Call { args, ret, target } => {
            for r in &ret {
                live.remove(r);
            }
            for a in &args {
                live.insert(*a);
            }
            Some(HirOp::Call { args, ret, target })
        }
    }
}

/// Recursively collect every slot any op in `block` reads.
fn slots_read_recursive(block: &HirBlock) -> HashSet<SlotId> {
    let mut out = HashSet::new();
    for op in &block.ops {
        collect_read(op, &mut out);
    }
    out
}

fn collect_read(op: &HirOp, out: &mut HashSet<SlotId>) {
    match op {
        HirOp::Copy(_, src) => {
            out.insert(*src);
        }
        HirOp::MapValue(src, _, _) => {
            out.insert(*src);
        }
        HirOp::Match(s, arms) => {
            out.insert(*s);
            for (b, _) in arms {
                for o in &b.ops {
                    collect_read(o, out);
                }
            }
        }
        HirOp::Loop(b) | HirOp::Block(b) => {
            for o in &b.ops {
                collect_read(o, out);
            }
        }
        HirOp::Call { args, .. } => {
            for a in args {
                out.insert(*a);
            }
        }
        HirOp::WriteRegister(_, Either::Right(s)) => {
            out.insert(*s);
        }
        _ => {}
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

/// Does `op` or anything nested inside it read `slot`? Used by DSE
/// to decide whether a `Set(slot, _)` is still live past this op.
///
/// `Break` / `Continue` / `Stop` / `Skip` don't read or write the
/// slot but they **exit the enclosing block**, which means any
/// straight-line write after them is unreachable — those are still
/// "barriers" for the DSE's unconditional-write search.
fn op_reads_or_barrier(op: &HirOp, slot: SlotId) -> bool {
    match op {
        HirOp::Set(_, _) => false,
        HirOp::Copy(dst, src) => *src == slot || *dst == slot_next_to(slot, dst),
        HirOp::MapValue(src, _, _) => *src == slot,
        // Recurse into arms: Match is only a barrier if the slot
        // is actually read (as scrutinee or inside any arm body).
        HirOp::Match(s, arms) => {
            *s == slot
                || arms
                    .iter()
                    .any(|(b, _)| block_reads_or_exits(b, slot))
        }
        HirOp::ReadRegister(_, _) => false,
        HirOp::WriteRegister(_, Either::Right(s)) => *s == slot,
        HirOp::WriteRegister(_, Either::Left(_)) => false,
        HirOp::Call { args, ret, .. } => args.contains(&slot) || ret.contains(&slot),
        // Loop / Block: recurse. A Loop that doesn't touch `slot`
        // is transparent to DSE of `slot`; one that reads or
        // exits must be a barrier.
        HirOp::Loop(b) | HirOp::Block(b) => block_reads_or_exits(b, slot),
        HirOp::Break | HirOp::Continue | HirOp::Stop | HirOp::Skip => true,
    }
}

/// Recursively: does `block` read `slot` anywhere, or contain an
/// unconditional early exit (Break/Continue/Stop/Skip)?
fn block_reads_or_exits(block: &HirBlock, slot: SlotId) -> bool {
    block
        .ops
        .iter()
        .any(|op| op_reads_or_barrier(op, slot))
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
        HirOp::MapValue(_, dst, _) => *dst == slot,
        HirOp::ReadRegister(s, _) => *s == slot,
        HirOp::Call { ret, .. } => ret.contains(&slot),
        _ => false,
    }
}
