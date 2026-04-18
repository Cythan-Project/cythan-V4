//! HIR specialization pass — flow-sensitive constant propagation.
//!
//! Runs on the **inlined** program body (after
//! `inline_program_full`) and before `hir_to_mir`. Extends the
//! per-function `opt::optimize_block` by threading known values
//! through the control flow: entering a match arm whose values list
//! is a singleton means the scrutinee is known to equal that value
//! inside the arm, even when the original call site didn't make
//! that visible.
//!
//! # What it folds
//!
//! 1. **Constant propagation through `Copy`.** If the source slot's
//!    value is known, rewrite `Copy(dst, src)` as `Set(dst, v)` and
//!    track `dst`'s new value.
//! 2. **`Inc` / `Dec` on known slots.** Apply the update to the
//!    tracked value so downstream reads still benefit.
//! 3. **`Match` folding on known scrutinees.** When the scrutinee is
//!    a known constant, pick the arm whose values contain it and
//!    splice its body in place of the `Match`. Other arms become
//!    dead code.
//! 4. **Arm-local scrutinee knowledge.** When the scrutinee is
//!    *unknown* but an arm's values list is a singleton, specialize
//!    that arm's body with `scrutinee == value` in context.
//!
//! # What it *doesn't* fold (yet)
//!
//! * Loop bodies — a slot written inside a loop can be anything at
//!   the next iteration, so the pass clears the context on entry
//!   and on exit conservatively.
//! * Match arms whose values are multi-element (e.g. `[1..=15]`) —
//!   the scrutinee becomes "known-not 0" not "known-equal N"; this
//!   pass doesn't track negative/range knowledge. A future
//!   extension could represent "forbidden values" sets.
//! * Inter-block joins — when two branches end with the same
//!   constant for a slot, the context after the `Match` could
//!   preserve that knowledge. This pass pessimistically clears.
//!
//! # Idempotence
//!
//! `specialize_to_fixpoint` iterates up to 8 rounds; each round's
//! output feeds the next. In practice 2-3 rounds suffice — the cap
//! is a safety net.

use std::collections::HashMap;

use either::Either;

use crate::ir::{HirBlock, HirOp, SlotId};

/// Run specialization until a fixpoint (or a safety cap).
pub fn specialize_to_fixpoint(mut block: HirBlock) -> HirBlock {
    for _ in 0..8 {
        let before = block.clone();
        block = specialize_block(block, &Ctx::default());
        if block == before {
            break;
        }
    }
    block
}

/// Known-value context — per-slot tracked values at a program point.
/// Flow-sensitive: updated as the walker moves through ops.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Ctx {
    known: HashMap<SlotId, u8>,
}

impl Ctx {
    fn forget(&mut self, slot: SlotId) {
        self.known.remove(&slot);
    }
    fn set(&mut self, slot: SlotId, value: u8) {
        self.known.insert(slot, value);
    }
    fn get(&self, slot: SlotId) -> Option<u8> {
        self.known.get(&slot).copied()
    }
}

fn specialize_block(block: HirBlock, inbound: &Ctx) -> HirBlock {
    let mut ctx = inbound.clone();
    let mut out: Vec<HirOp> = Vec::with_capacity(block.ops.len());
    for op in block.ops {
        specialize_op(op, &mut ctx, &mut out);
    }
    HirBlock {
        ops: out,
        result_slot: block.result_slot,
    }
}

fn specialize_op(op: HirOp, ctx: &mut Ctx, out: &mut Vec<HirOp>) {
    match op {
        HirOp::Set(slot, v) => {
            ctx.set(slot, v);
            out.push(HirOp::Set(slot, v));
        }
        HirOp::Copy(dst, src) => match ctx.get(src) {
            Some(v) => {
                // Src is known — collapse to a direct Set.
                ctx.set(dst, v);
                out.push(HirOp::Set(dst, v));
            }
            None => {
                ctx.forget(dst);
                out.push(HirOp::Copy(dst, src));
            }
        },
        HirOp::Inc(slot) => {
            if let Some(v) = ctx.get(slot) {
                ctx.set(slot, v.wrapping_add(1) % 16);
            }
            out.push(HirOp::Inc(slot));
        }
        HirOp::Dec(slot) => {
            if let Some(v) = ctx.get(slot) {
                ctx.set(slot, v.wrapping_sub(1) % 16);
            }
            out.push(HirOp::Dec(slot));
        }
        HirOp::Match(scrutinee, arms) => {
            // Case 1: scrutinee is known — pick the matching arm's
            // body and splice it in directly (dropping the rest as
            // dead code).
            if let Some(v) = ctx.get(scrutinee) {
                for (body, values) in arms {
                    if values.contains(&v) {
                        let specialized = specialize_block(body, ctx);
                        // Splice body ops into our output, then join
                        // ctx. We can't track the arm's final ctx
                        // precisely without re-walking, so a safe
                        // over-approximation: clear every slot the
                        // body mutates.
                        let mutated = slots_mutated(&specialized);
                        out.extend(specialized.ops);
                        for s in mutated {
                            ctx.forget(s);
                        }
                        return;
                    }
                }
                // No arm matched — keep the Match as-is (dead branch).
                out.push(HirOp::Match(scrutinee, Vec::new()));
                return;
            }
            // Case 2: scrutinee unknown — specialize each arm with
            // its value constraint and keep the Match node. Each
            // arm's body may mutate slots; we clear them post-match
            // for a safe join.
            let mut specialized: Vec<(HirBlock, Vec<u8>)> = Vec::with_capacity(arms.len());
            let mut mutated_across_arms = std::collections::HashSet::new();
            for (body, values) in arms {
                let mut arm_ctx = ctx.clone();
                if values.len() == 1 {
                    arm_ctx.set(scrutinee, values[0]);
                }
                let specialized_body = specialize_block(body, &arm_ctx);
                for s in slots_mutated(&specialized_body) {
                    mutated_across_arms.insert(s);
                }
                specialized.push((specialized_body, values));
            }
            for s in mutated_across_arms {
                ctx.forget(s);
            }
            out.push(HirOp::Match(scrutinee, specialized));
        }
        HirOp::Loop(body) => {
            // A loop body may execute zero or more times and may
            // mutate any of its slots. The safest pre-state is
            // "nothing known"; outside the loop, the same applies.
            let mutated = slots_mutated(&body);
            let specialized = specialize_block(body, &Ctx::default());
            out.push(HirOp::Loop(specialized));
            for s in mutated {
                ctx.forget(s);
            }
        }
        HirOp::Block(body) => {
            let specialized = specialize_block(body, ctx);
            // Block may mutate slots via Set/Copy/Inc/Dec; drop
            // knowledge of anything it touched.
            for s in slots_mutated(&specialized) {
                ctx.forget(s);
            }
            out.push(HirOp::Block(specialized));
        }
        HirOp::Break | HirOp::Continue | HirOp::Stop | HirOp::Skip => {
            out.push(op);
        }
        HirOp::ReadRegister(dst, r) => {
            // Reading a register always produces a fresh value we
            // can't predict.
            ctx.forget(dst);
            out.push(HirOp::ReadRegister(dst, r));
        }
        HirOp::WriteRegister(r, src) => {
            // If the immediate-source slot is known, collapse the
            // write to a literal.
            match src {
                Either::Right(slot) => {
                    if let Some(v) = ctx.get(slot) {
                        out.push(HirOp::WriteRegister(r, Either::Left(v)));
                    } else {
                        out.push(HirOp::WriteRegister(r, Either::Right(slot)));
                    }
                }
                Either::Left(v) => {
                    out.push(HirOp::WriteRegister(r, Either::Left(v)));
                }
            }
        }
        HirOp::Call { target, args, ret } => {
            for r in &ret {
                ctx.forget(*r);
            }
            out.push(HirOp::Call { target, args, ret });
        }
    }
}

/// Set of slots that some op in `block` writes to, recursively.
/// Used to decide what knowledge to drop when joining control flow
/// around a nested block (match arm, loop body, inner block).
fn slots_mutated(block: &HirBlock) -> std::collections::HashSet<SlotId> {
    let mut out = std::collections::HashSet::new();
    for op in &block.ops {
        collect_mutated(op, &mut out);
    }
    out
}

fn collect_mutated(op: &HirOp, out: &mut std::collections::HashSet<SlotId>) {
    match op {
        HirOp::Set(s, _) | HirOp::Copy(s, _) | HirOp::Inc(s) | HirOp::Dec(s) => {
            out.insert(*s);
        }
        HirOp::ReadRegister(s, _) => {
            out.insert(*s);
        }
        HirOp::Match(_, arms) => {
            for (b, _) in arms {
                for o in &b.ops {
                    collect_mutated(o, out);
                }
            }
        }
        HirOp::Loop(b) | HirOp::Block(b) => {
            for o in &b.ops {
                collect_mutated(o, out);
            }
        }
        HirOp::Call { ret, .. } => {
            for r in ret {
                out.insert(*r);
            }
        }
        _ => {}
    }
}
