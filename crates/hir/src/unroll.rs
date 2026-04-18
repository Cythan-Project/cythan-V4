//! Loop unrolling.
//!
//! Replace `Loop { body }` with `Loop { body; body; … ; body }` —
//! `factor` straight-line copies of the body inside a single
//! wrapping `Loop`. A loop that ran `k` iterations in the original
//! now performs `ceil(k / factor)` backward jumps, the rest being
//! straight-line execution.
//!
//! The payoff isn't the branch savings alone: with the body
//! duplicated inline, downstream passes (domain-based
//! specialization, constant folding, LVA) get to see and fold
//! across the repeated structure. A loop of the shape
//!
//! ```text
//! i = 0; Loop { if i >= 9: Break; work(i); i += 1 }
//! ```
//!
//! becomes, after unrolling + specialization, a fully-straightened
//! `work(0); work(1); …; work(8)` — because the specializer can
//! track `i`'s value through each of the 16 copies.
//!
//! # Correctness
//!
//! Pure body duplication is sound because every HIR control-flow
//! op is unlabelled:
//!
//! * `Break` / `Continue` target the nearest enclosing `Loop`.
//!   All copies sit inside the same enclosing `Loop`, so the
//!   target is the same for every copy. `Break` exits the
//!   (unrolled) loop; `Continue` restarts at the top of copy 1.
//!   The latter behaves identically to the original (where
//!   `Continue` restarts the single-copy body).
//! * `Stop` exits the program; unchanged.
//! * `Skip` exits the nearest enclosing `Block`. Duplication
//!   doesn't introduce new `Block`s, so the target is preserved.
//! * Slot state flows from copy `n` to copy `n+1` exactly as it
//!   flowed from iteration `n` to iteration `n+1` in the
//!   original (HIR has one slot namespace per function, not
//!   per iteration).
//!
//! # Heuristic: innermost-only
//!
//! Unrolling a loop that itself contains a `Loop` would blow up
//! as `factor ^ depth`. We recurse into loop bodies first so
//! every innermost loop is unrolled, then refuse to unroll any
//! outer loop whose (rewritten) body still contains a `Loop`.
//! That caps the worst-case code growth at one level of
//! unrolling.

use crate::ir::{HirBlock, HirOp};

/// Default unroll factor. 16 is the size of a `u4` cell's value
/// domain: any loop that iterates fewer than 16 times with a
/// cell-tracked counter collapses into a single round after
/// unroll + specialization.
pub const DEFAULT_UNROLL_FACTOR: usize = 16;

/// Stats produced by [`unroll_loops_with_stats`].
#[derive(Debug, Default, Clone, Copy)]
pub struct UnrollStats {
    /// Number of (innermost) `Loop` ops whose body was duplicated.
    pub loops_unrolled: u32,
    /// Sum of `body_ops * (factor - 1)` across every unrolled
    /// loop. A rough "how much code did this add" figure.
    pub ops_added: u32,
}

/// Unroll every innermost `Loop` in `block` by `factor` copies.
/// `factor <= 1` is a no-op (returns the block unchanged).
pub fn unroll_loops(block: HirBlock, factor: usize) -> HirBlock {
    unroll_loops_with_stats(block, factor).0
}

pub fn unroll_loops_with_stats(
    block: HirBlock,
    factor: usize,
) -> (HirBlock, UnrollStats) {
    let mut stats = UnrollStats::default();
    let new_block = if factor <= 1 {
        block
    } else {
        unroll_block(block, factor, &mut stats)
    };
    (new_block, stats)
}

fn unroll_block(b: HirBlock, factor: usize, stats: &mut UnrollStats) -> HirBlock {
    let ops = b
        .ops
        .into_iter()
        .map(|op| unroll_op(op, factor, stats))
        .collect();
    HirBlock {
        ops,
        result_slot: b.result_slot,
    }
}

fn unroll_op(op: HirOp, factor: usize, stats: &mut UnrollStats) -> HirOp {
    match op {
        HirOp::Loop(body) => {
            // Recurse first so every innermost loop is unrolled
            // before we decide whether to unroll this one.
            let body = unroll_block(body, factor, stats);
            if block_contains_loop(&body) {
                HirOp::Loop(body)
            } else {
                let body_len = body.ops.len();
                let mut unrolled = Vec::with_capacity(body_len * factor);
                for _ in 0..factor {
                    unrolled.extend(body.ops.iter().cloned());
                }
                stats.loops_unrolled += 1;
                stats.ops_added += (body_len * (factor - 1)) as u32;
                HirOp::Loop(HirBlock {
                    ops: unrolled,
                    result_slot: body.result_slot,
                })
            }
        }
        HirOp::Block(b) => HirOp::Block(unroll_block(b, factor, stats)),
        HirOp::Match(s, arms) => HirOp::Match(
            s,
            arms.into_iter()
                .map(|(b, vs)| (unroll_block(b, factor, stats), vs))
                .collect(),
        ),
        other => other,
    }
}

fn block_contains_loop(b: &HirBlock) -> bool {
    b.ops.iter().any(op_contains_loop)
}

fn op_contains_loop(op: &HirOp) -> bool {
    match op {
        HirOp::Loop(_) => true,
        HirOp::Block(b) => block_contains_loop(b),
        HirOp::Match(_, arms) => arms.iter().any(|(b, _)| block_contains_loop(b)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;

    #[test]
    fn factor_one_is_noop() {
        let inner = HirBlock {
            ops: vec![HirOp::Inc(SlotId(0)), HirOp::Break],
            result_slot: None,
        };
        let b = HirBlock {
            ops: vec![HirOp::Loop(inner.clone())],
            result_slot: None,
        };
        let (out, stats) = unroll_loops_with_stats(b, 1);
        assert_eq!(stats.loops_unrolled, 0);
        let HirOp::Loop(body) = &out.ops[0] else {
            panic!();
        };
        assert_eq!(body, &inner);
    }

    #[test]
    fn unrolls_innermost_loop() {
        let inner_body = HirBlock {
            ops: vec![HirOp::Inc(SlotId(0))],
            result_slot: None,
        };
        let b = HirBlock {
            ops: vec![HirOp::Loop(inner_body)],
            result_slot: None,
        };
        let (out, stats) = unroll_loops_with_stats(b, 4);
        assert_eq!(stats.loops_unrolled, 1);
        assert_eq!(stats.ops_added, 3);
        let HirOp::Loop(body) = &out.ops[0] else {
            panic!();
        };
        assert_eq!(body.ops.len(), 4);
        for op in &body.ops {
            assert_eq!(op, &HirOp::Inc(SlotId(0)));
        }
    }

    #[test]
    fn does_not_unroll_outer_of_nested() {
        let inner = HirBlock {
            ops: vec![HirOp::Inc(SlotId(1)), HirOp::Break],
            result_slot: None,
        };
        let outer = HirBlock {
            ops: vec![HirOp::Loop(inner), HirOp::Dec(SlotId(0))],
            result_slot: None,
        };
        let b = HirBlock {
            ops: vec![HirOp::Loop(outer)],
            result_slot: None,
        };
        let (out, stats) = unroll_loops_with_stats(b, 3);
        // Only the inner loop is innermost — exactly one unroll.
        assert_eq!(stats.loops_unrolled, 1);
        let HirOp::Loop(outer_body) = &out.ops[0] else {
            panic!();
        };
        // Outer body still has 2 ops: [Loop(inner_unrolled), Dec].
        assert_eq!(outer_body.ops.len(), 2);
        let HirOp::Loop(inner_body) = &outer_body.ops[0] else {
            panic!();
        };
        // Inner unrolled to 3 copies of [Inc, Break] = 6 ops.
        assert_eq!(inner_body.ops.len(), 6);
    }

    #[test]
    fn recurses_into_block_and_match() {
        let inner = HirBlock {
            ops: vec![HirOp::Set(SlotId(0), 1)],
            result_slot: None,
        };
        let b = HirBlock {
            ops: vec![HirOp::Match(
                SlotId(1),
                vec![(
                    HirBlock {
                        ops: vec![HirOp::Loop(inner)],
                        result_slot: None,
                    },
                    vec![0],
                )],
            )],
            result_slot: None,
        };
        let (_, stats) = unroll_loops_with_stats(b, 2);
        assert_eq!(stats.loops_unrolled, 1);
    }

    #[test]
    fn empty_body_still_counts_as_unrolled() {
        let empty = HirBlock { ops: vec![], result_slot: None };
        let b = HirBlock {
            ops: vec![HirOp::Loop(empty)],
            result_slot: None,
        };
        let (out, stats) = unroll_loops_with_stats(b, 5);
        assert_eq!(stats.loops_unrolled, 1);
        assert_eq!(stats.ops_added, 0);
        let HirOp::Loop(body) = &out.ops[0] else {
            panic!();
        };
        assert!(body.ops.is_empty());
    }
}
