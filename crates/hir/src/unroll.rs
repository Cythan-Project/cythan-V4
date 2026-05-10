//! Loop unrolling (conditional on provable early-exit).
//!
//! Replace `Loop { body }` with `Loop { body; body; … ; body }`
//! when, and only when, the specializer can prove that at least
//! one copy always exits the loop (via `Break` / `Stop`) before
//! the end of the unrolled sequence — so the copies past that
//! exit collapse into dead code and downstream passes compact
//! the program.
//!
//! Unconditional unrolling with `factor = 16` inflates the HIR
//! 4–9× on the test games with no VM-step improvement: dead
//! copies past a dynamic `Break` still get emitted as bytecode
//! because the specializer can't prove they're unreachable.
//! The conditional form speculatively unrolls each innermost
//! loop, runs the specializer + optimizer on the unrolled body
//! alone, and **keeps the unroll only when folding actually
//! shrunk the body below its literal `original_ops * factor`
//! size**. Otherwise the original loop is restored, avoiding
//! gratuitous growth.
//!
//! # Correctness of the body duplication
//!
//! Sound because every HIR control-flow op is unlabelled:
//! * `Break` / `Continue` target the nearest enclosing `Loop`.
//!   Every copy lives inside the same enclosing `Loop`, so
//!   targets are preserved.
//! * `Stop` exits the program — unchanged.
//! * `Skip` exits the nearest enclosing `Block` — duplication
//!   doesn't reparent it.
//! * Slot state flows between copies just as it flows between
//!   iterations of the original loop (HIR has one slot namespace
//!   per function, not per iteration).
//!
//! # Heuristic: innermost-only
//!
//! To cap worst-case growth at one level we only consider loops
//! whose bodies contain no nested `Loop` ops. We still recurse
//! into nested bodies first so inner loops get their own
//! unroll decision.

use std::collections::HashMap;

use crate::exit_domains::FnExitDomains;
use crate::ir::{count_ops, HirBlock, HirOp};
use crate::opt::optimize_block;
use crate::specialize::specialize_to_fixpoint_full;

/// Default unroll factor. 16 matches the `u4` cell's value
/// domain: a loop whose counter walks through `< 16` values
/// before exiting is fully straightened.
pub const DEFAULT_UNROLL_FACTOR: usize = 16;

/// Stats produced by [`unroll_loops_with_stats`].
#[derive(Debug, Default, Clone, Copy)]
pub struct UnrollStats {
    /// Loops whose body was actually duplicated (speculative
    /// unroll survived the early-exit check).
    pub loops_unrolled: u32,
    /// Loops where the speculative unroll was reverted because
    /// no early-exit could be proven.
    pub loops_reverted: u32,
    /// Net HIR op delta contributed by retained unrolls.
    pub ops_added: u32,
}

/// Speculatively unroll every innermost `Loop` by `factor`
/// copies, keeping the unroll iff the specializer-driven
/// early-exit check indicates folding.
///
/// `summaries` threads function exit-domain summaries into the
/// speculative specializer so callers see tighter Domains post-
/// Call and more folds become provable.
pub fn unroll_loops(block: HirBlock, factor: usize) -> HirBlock {
    unroll_loops_with_stats(block, factor, None).0
}

pub fn unroll_loops_with_summaries(
    block: HirBlock,
    factor: usize,
    summaries: &HashMap<typer::FnSig, FnExitDomains>,
) -> HirBlock {
    unroll_loops_with_stats(block, factor, Some(summaries)).0
}

pub fn unroll_loops_with_stats(
    block: HirBlock,
    factor: usize,
    summaries: Option<&HashMap<typer::FnSig, FnExitDomains>>,
) -> (HirBlock, UnrollStats) {
    let mut stats = UnrollStats::default();
    let new_block = if factor <= 1 {
        block
    } else {
        unroll_block(block, factor, summaries, &mut stats)
    };
    (new_block, stats)
}

fn unroll_block(
    b: HirBlock,
    factor: usize,
    summaries: Option<&HashMap<typer::FnSig, FnExitDomains>>,
    stats: &mut UnrollStats,
) -> HirBlock {
    let ops = b
        .ops
        .into_iter()
        .map(|op| unroll_op(op, factor, summaries, stats))
        .collect();
    HirBlock {
        ops,
        result_slot: b.result_slot,
    }
}

fn unroll_op(
    op: HirOp,
    factor: usize,
    summaries: Option<&HashMap<typer::FnSig, FnExitDomains>>,
    stats: &mut UnrollStats,
) -> HirOp {
    match op {
        HirOp::Loop(body) => {
            // Recurse first so innermost loops are processed
            // before we decide on the outer ones.
            let body = unroll_block(body, factor, summaries, stats);
            if block_contains_loop(&body) {
                // Not innermost — leave alone.
                return HirOp::Loop(body);
            }
            match try_unroll_body(body.clone(), factor, summaries) {
                Some(unrolled) => {
                    let orig_len = body.ops.len();
                    stats.loops_unrolled += 1;
                    stats.ops_added +=
                        (orig_len.saturating_mul(factor.saturating_sub(1))) as u32;
                    HirOp::Loop(unrolled)
                }
                None => {
                    stats.loops_reverted += 1;
                    HirOp::Loop(body)
                }
            }
        }
        HirOp::Block(b) => HirOp::Block(unroll_block(b, factor, summaries, stats)),
        HirOp::Match(s, arms) => HirOp::Match(
            s,
            arms.into_iter()
                .map(|(b, vs)| (unroll_block(b, factor, summaries, stats), vs))
                .collect(),
        ),
        other => other,
    }
}

/// Speculative unroll: build the `factor`-copy body, run
/// specialize + optimize on it in isolation, and return the
/// raw unrolled body **only if** folding shrunk the result.
/// Returns `None` when no early exit was proven and the outer
/// pipeline should keep the original loop.
fn try_unroll_body(
    body: HirBlock,
    factor: usize,
    summaries: Option<&HashMap<typer::FnSig, FnExitDomains>>,
) -> Option<HirBlock> {
    // Heuristic gate: a loop with no `Break` anywhere can't
    // benefit from the early-exit check (Continue/Stop are
    // possible but Continue doesn't create dead tail; Stop does
    // but is rare). Skip cheaply.
    if !block_contains_break_or_stop(&body) {
        return None;
    }

    let unrolled = duplicate(body, factor);
    let literal_size = count_ops(&unrolled);
    // Probe: run the specializer + optimizer on a clone. If a
    // copy's `Break` is provably unconditional, later copies
    // become dead code and folding shrinks the body.
    let probe = specialize_to_fixpoint_full(unrolled.clone(), &[], summaries);
    let probe = optimize_block(probe);
    if count_ops(&probe) < literal_size {
        Some(unrolled)
    } else {
        None
    }
}

fn duplicate(body: HirBlock, factor: usize) -> HirBlock {
    let body_len = body.ops.len();
    let mut out = Vec::with_capacity(body_len * factor);
    for _ in 0..factor {
        out.extend(body.ops.iter().cloned());
    }
    HirBlock {
        ops: out,
        result_slot: body.result_slot,
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

fn block_contains_break_or_stop(b: &HirBlock) -> bool {
    b.ops.iter().any(op_contains_break_or_stop)
}

/// Does this op potentially short-circuit out of its enclosing
/// loop on early iterations? The unroller only fires when at
/// least one such op exists, because without one every copy of
/// the duplicated body is semantically identical — no dead-tail
/// to fold, no size shrink.
///
/// `Skip` counts alongside `Break` and `Stop`: after the HIR
/// inliner rewrites each inlined callee's `return` into
/// `Block { ... Skip }`, the stdlib's lockstep-decrement loops
/// (U4::eq / Add / Sub / …) contain `Skip`s in their arm bodies
/// that let the surrounding Block exit early. Excluding `Skip`
/// from this check was why the unroller never fired on any
/// inlined call site — which is what made Morpion explode into
/// thousands of un-optimised lockstep loops.
fn op_contains_break_or_stop(op: &HirOp) -> bool {
    match op {
        HirOp::Break | HirOp::Stop | HirOp::Skip => true,
        HirOp::Block(b) => block_contains_break_or_stop(b),
        HirOp::Match(_, arms) => arms.iter().any(|(b, _)| block_contains_break_or_stop(b)),
        HirOp::Loop(b) => block_contains_break_or_stop(b),
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
            ops: vec![HirOp::inc(SlotId(0)), HirOp::Break],
            result_slot: None,
        };
        let b = HirBlock {
            ops: vec![HirOp::Loop(inner.clone())],
            result_slot: None,
        };
        let (out, stats) = unroll_loops_with_stats(b, 1, None);
        assert_eq!(stats.loops_unrolled, 0);
        assert_eq!(stats.loops_reverted, 0);
        let HirOp::Loop(body) = &out.ops[0] else {
            panic!();
        };
        assert_eq!(body, &inner);
    }

    #[test]
    fn unrolls_when_specializer_can_fold_break() {
        // Loop { Set s0=0; Match s0 { 0 => Break; * => ... } }
        // The set-then-match folds → Break becomes unconditional
        // → copies after copy 1 are dead. Unroll should be kept.
        let body = HirBlock {
            ops: vec![
                HirOp::Set(SlotId(0), 0),
                HirOp::Match(
                    SlotId(0),
                    vec![
                        (
                            HirBlock {
                                ops: vec![HirOp::Break],
                                result_slot: None,
                            },
                            vec![0],
                        ),
                        (
                            HirBlock {
                                ops: vec![HirOp::inc(SlotId(1))],
                                result_slot: None,
                            },
                            (1..=15).collect(),
                        ),
                    ],
                ),
            ],
            result_slot: None,
        };
        let b = HirBlock {
            ops: vec![HirOp::Loop(body)],
            result_slot: None,
        };
        let (_, stats) = unroll_loops_with_stats(b, 4, None);
        assert_eq!(stats.loops_unrolled, 1);
    }

    #[test]
    fn reverts_when_specializer_cannot_fold() {
        // A body whose only control is an unconditional Break.
        // Unrolling it 4x yields `Break; Break; Break; Break`,
        // which the specializer + DSE don't further reduce
        // (neither pass models reachability past a Break).
        // Literal unrolled size equals probe size → revert.
        let body = HirBlock {
            ops: vec![HirOp::Break],
            result_slot: None,
        };
        let b = HirBlock {
            ops: vec![HirOp::Loop(body)],
            result_slot: None,
        };
        let (_, stats) = unroll_loops_with_stats(b, 4, None);
        assert_eq!(stats.loops_unrolled, 0);
        assert_eq!(stats.loops_reverted, 1);
    }

    #[test]
    fn does_not_unroll_outer_of_nested() {
        // Outer loop contains an inner loop → not innermost, skip.
        let inner = HirBlock {
            ops: vec![
                HirOp::Set(SlotId(0), 0),
                HirOp::Match(
                    SlotId(0),
                    vec![(
                        HirBlock {
                            ops: vec![HirOp::Break],
                            result_slot: None,
                        },
                        vec![0],
                    )],
                ),
            ],
            result_slot: None,
        };
        let outer = HirBlock {
            ops: vec![HirOp::Loop(inner), HirOp::dec(SlotId(0))],
            result_slot: None,
        };
        let b = HirBlock {
            ops: vec![HirOp::Loop(outer)],
            result_slot: None,
        };
        let (_, stats) = unroll_loops_with_stats(b, 4, None);
        // Only the inner loop is innermost. The outer loop is
        // not even tried (nested). Inner may unroll (depending
        // on fold-check). `loops_unrolled + loops_reverted`
        // should equal 1 — just the inner.
        assert_eq!(stats.loops_unrolled + stats.loops_reverted, 1);
    }

    #[test]
    fn no_break_means_no_attempt() {
        // No Break/Stop anywhere → gate trips, unroll skipped.
        let body = HirBlock {
            ops: vec![HirOp::inc(SlotId(0))],
            result_slot: None,
        };
        let b = HirBlock {
            ops: vec![HirOp::Loop(body)],
            result_slot: None,
        };
        let (_, stats) = unroll_loops_with_stats(b, 4, None);
        assert_eq!(stats.loops_unrolled, 0);
        assert_eq!(stats.loops_reverted, 1);
    }
}
