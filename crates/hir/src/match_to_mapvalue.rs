//! Match-to-MapValue rewrite.
//!
//! Recognises the common shape `match s { v0 => Set(d, k0), v1 =>
//! Set(d, k1), … }` (every arm body is a single `Set` of the same
//! destination slot to a single u4 constant) and rewrites it as a
//! single [`HirOp::MapValue`] with the corresponding 16-entry table.
//!
//! The collapsed MapValue is *much* tighter at the bytecode level
//! (the MIR-to-LIR pass detects the inc/dec shape — and a future
//! generic table emitter could go further) and gives the optimizer
//! a single pure node to reason about: constant fold, identity
//! elimination, fusion of consecutive maps.
//!
//! # What qualifies
//!
//! 1. A `Match(scrutinee, arms)` where every arm is a `HirBlock`
//!    whose `ops == [HirOp::Set(d, k)]` for the same destination
//!    slot `d`. (Wildcard arms count: their `Set` to `d` fills the
//!    table entries for all values they match.)
//! 2. The arm's `result_slot` is either `None` or `Some(d)` — an
//!    arm that writes the result through some other channel can't
//!    be expressed as a MapValue.
//! 3. The 16 cell values 0..=15 are fully covered by the union of
//!    arm value lists. Anything missing falls back to leaving the
//!    Match alone (the source-level shape was incomplete).
//!
//! # What it doesn't try yet
//!
//! - Match arms that are `[Set(d, _), Copy/Set(other, _)]` — the
//!   body must be a single op.
//! - Folding through a `Block` wrapper.
//! - Recognising `Set(d, scrutinee_value)` (identity table) — that
//!   would need to know each arm's matched value as the constant.

use crate::ir::{HirBlock, HirOp, SlotId};

/// Walk `block` recursively, replacing every qualifying Match with
/// a MapValue. Returns `(rewritten, count_replaced)`.
pub fn rewrite_block(block: HirBlock) -> (HirBlock, usize) {
    let mut count = 0;
    let ops = block
        .ops
        .into_iter()
        .map(|op| {
            let (new_op, c) = rewrite_op(op);
            count += c;
            new_op
        })
        .collect();
    (
        HirBlock {
            ops,
            result_slot: block.result_slot,
        },
        count,
    )
}

fn rewrite_op(op: HirOp) -> (HirOp, usize) {
    match op {
        HirOp::Match(scrutinee, arms) => {
            // Recurse into arm bodies first.
            let mut total = 0;
            let mut new_arms = Vec::with_capacity(arms.len());
            for (body, values) in arms {
                let (new_body, c) = rewrite_block(body);
                total += c;
                new_arms.push((new_body, values));
            }
            // Try to collapse this Match.
            if let Some(mv) = try_collapse(scrutinee, &new_arms) {
                (mv, total + 1)
            } else {
                (HirOp::Match(scrutinee, new_arms), total)
            }
        }
        HirOp::Loop(b) => {
            let (b, c) = rewrite_block(b);
            (HirOp::Loop(b), c)
        }
        HirOp::Block(b) => {
            let (b, c) = rewrite_block(b);
            (HirOp::Block(b), c)
        }
        other => (other, 0),
    }
}

/// If every arm body is exactly `Set(d, k)` for the same `d`, build
/// the 16-entry table and return a `MapValue`. Otherwise `None`.
fn try_collapse(
    scrutinee: SlotId,
    arms: &[(HirBlock, Vec<u8>)],
) -> Option<HirOp> {
    if arms.is_empty() {
        return None;
    }
    let mut common_dst: Option<SlotId> = None;
    let mut table: [Option<u8>; 16] = [None; 16];
    for (body, values) in arms {
        // The arm body must be exactly one `Set(d, k)` op, with a
        // matching `result_slot` (None, or Some(d)).
        if body.ops.len() != 1 {
            return None;
        }
        let HirOp::Set(d, k) = &body.ops[0] else {
            return None;
        };
        if let Some(prev) = common_dst {
            if prev != *d {
                return None;
            }
        } else {
            common_dst = Some(*d);
        }
        if let Some(rs) = body.result_slot {
            if rs != *d {
                return None;
            }
        }
        for v in values {
            if *v > 15 {
                return None;
            }
            // First-arm-wins (matches HIR Match semantics).
            if table[*v as usize].is_none() {
                table[*v as usize] = Some(*k & 0x0F);
            }
        }
    }
    let dst = common_dst?;
    // Fill any gaps with `dst`'s prior value via identity? Can't —
    // we don't know prior value. So require full coverage.
    let mut out = [0u8; 16];
    for i in 0..16 {
        match table[i] {
            Some(v) => out[i] = v,
            None => return None,
        }
    }
    Some(HirOp::MapValue(scrutinee, dst, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;

    fn arm(values: Vec<u8>, dst: u32, k: u8) -> (HirBlock, Vec<u8>) {
        (
            HirBlock {
                ops: vec![HirOp::Set(SlotId(dst), k)],
                result_slot: None,
            },
            values,
        )
    }

    #[test]
    fn collapses_constant_table() {
        // Boolean-shaped: 0 => 0, _ => 1.
        let block = HirBlock {
            ops: vec![HirOp::Match(
                SlotId(5),
                vec![
                    arm(vec![0], 7, 0),
                    arm((1..=15).collect(), 7, 1),
                ],
            )],
            result_slot: None,
        };
        let (out, n) = rewrite_block(block);
        assert_eq!(n, 1);
        let HirOp::MapValue(src, dst, table) = &out.ops[0] else {
            panic!("expected MapValue, got {:?}", out.ops[0]);
        };
        assert_eq!(*src, SlotId(5));
        assert_eq!(*dst, SlotId(7));
        assert_eq!(table[0], 0);
        for i in 1..=15 {
            assert_eq!(table[i], 1);
        }
    }

    #[test]
    fn rejects_mixed_dst() {
        let block = HirBlock {
            ops: vec![HirOp::Match(
                SlotId(5),
                vec![
                    arm(vec![0], 7, 0),
                    arm((1..=15).collect(), 8, 1),
                ],
            )],
            result_slot: None,
        };
        let (out, n) = rewrite_block(block);
        assert_eq!(n, 0);
        assert!(matches!(out.ops[0], HirOp::Match(_, _)));
    }

    #[test]
    fn rejects_incomplete_coverage() {
        let block = HirBlock {
            ops: vec![HirOp::Match(
                SlotId(5),
                vec![arm(vec![0, 1, 2], 7, 9)],
            )],
            result_slot: None,
        };
        let (_, n) = rewrite_block(block);
        assert_eq!(n, 0);
    }

    #[test]
    fn recurses_into_loop() {
        let inner = HirBlock {
            ops: vec![HirOp::Match(
                SlotId(0),
                vec![
                    arm(vec![0], 1, 0),
                    arm((1..=15).collect(), 1, 1),
                ],
            )],
            result_slot: None,
        };
        let block = HirBlock {
            ops: vec![HirOp::Loop(inner)],
            result_slot: None,
        };
        let (_, n) = rewrite_block(block);
        assert_eq!(n, 1);
    }
}
