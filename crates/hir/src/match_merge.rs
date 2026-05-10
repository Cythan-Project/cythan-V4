//! Adjacent-Match merging + boolean-temp absorption.
//!
//! Two rewrites on `Match` chains, both sound purely from a
//! syntactic view (they never inspect callee bodies, arm domains
//! across gaps, etc.):
//!
//! 1. **Same-scrutinee adjacency merge.** Consecutive
//!    `Match(s, …)` on the same slot, with no intervening op,
//!    collapse into one n-way match via the Cartesian product of
//!    the two arms' value sets. Intersections-empty arms drop.
//!
//! 2. **Boolean-temp absorption.** `Match(s1, …)` followed by
//!    `Match(s2, …)` where `s1 ≠ s2` but **every arm of the first
//!    match ends with `Set(s2, c)`** for a literal `c`. The
//!    second match is then fully determined by the first (each
//!    outer arm deterministically selects one inner arm), so we
//!    splice the chosen inner arm's body into each outer arm and
//!    drop the second match entirely.
//!
//! # Why this matters
//!
//! After inlining + constant folding, operator-trait expansion
//! of `a == C1 || a == C2` produces a chain of `Match(a, …)`
//! writing boolean temps, with the `||` short-circuit building
//! another match on those temps. The existing specializer tracks
//! per-slot Domains but not cross-slot correlations, so the
//! temp-driven branch stays nested forever.
//!
//! Absorbing the temp-driven `Match(tmp, …)` into the original
//! `Match(a, …)` collapses one layer. Re-running specialize and
//! merge after absorb then collapses chained patterns
//! (`a==X || a==Y || a==Z`) layer by layer.
//!
//! # Generality
//!
//! Both rewrites are intentionally syntactic — they only need
//! to inspect op shape and the slot each op writes — so they
//! fire regardless of whether the literals came from the user,
//! a `const`, or materialised from an inlined method return.
//! Anything that lands as a concrete match after specialization
//! is eligible.
//!
//! # Algorithm
//!
//! Walk `block.ops`. For each pair `(op[i], op[i+1])`:
//!
//! * Same-scrutinee → **merge** (Cartesian product).
//! * Different scrutinee but `op[i+1]`'s scrutinee is fully
//!   determined (constant in every arm of `op[i]`) → **absorb**.
//! * Otherwise → pass through.
//!
//! Recurses into Loop/Block bodies and Match arms so every
//! nested scope is eligible. Idempotent.

use crate::ir::{HirBlock, HirOp, SlotId};

pub fn merge_adjacent_matches(block: HirBlock) -> HirBlock {
    let result_slot = block.result_slot;
    let ops = merge_ops(block.ops);
    HirBlock { ops, result_slot }
}


fn merge_ops(ops: Vec<HirOp>) -> Vec<HirOp> {
    // First recurse so inner nesting is already compacted, then
    // flatten any `Match(s, …)` whose arm ends with a same-slot
    // `Match(s, …)`.
    let mut lowered: Vec<HirOp> = ops
        .into_iter()
        .map(|op| match op {
            HirOp::Loop(b) => HirOp::Loop(merge_adjacent_matches(b)),
            HirOp::Block(b) => HirOp::Block(merge_adjacent_matches(b)),
            HirOp::Match(s, arms) => {
                let arms: Vec<(HirBlock, Vec<u8>)> = arms
                    .into_iter()
                    .map(|(b, v)| (merge_adjacent_matches(b), v))
                    .collect();
                HirOp::Match(s, flatten_inner_same_slot(s, arms))
            }
            other => other,
        })
        .collect();

    // Now walk pairwise and merge / absorb eligible neighbours.
    // The outer pipeline re-runs merge after specialize, so one
    // round here per invocation is enough.
    let mut out: Vec<HirOp> = Vec::with_capacity(lowered.len());
    for op in lowered.drain(..) {
        let action = classify_pair(out.last(), &op);
        match action {
            PairAction::SameSlot => {
                let prev = out.pop().unwrap();
                let (HirOp::Match(s, prev_arms), HirOp::Match(_, curr_arms)) = (prev, op) else {
                    unreachable!()
                };
                out.push(merge_same_slot(s, prev_arms, curr_arms));
            }
            PairAction::Absorb => {
                let prev = out.pop().unwrap();
                let (HirOp::Match(s1, prev_arms), HirOp::Match(s2, curr_arms)) = (prev, op)
                else {
                    unreachable!()
                };
                // `try_absorb` re-checks the precondition and
                // returns the absorbed op; fallback (shouldn't
                // happen given classify_pair said yes) keeps both.
                match try_absorb(s1, &prev_arms, s2, &curr_arms) {
                    Some(absorbed) => out.push(absorbed),
                    None => {
                        out.push(HirOp::Match(s1, prev_arms));
                        out.push(HirOp::Match(s2, curr_arms));
                    }
                }
            }
            PairAction::Passthrough => out.push(op),
        }
    }
    out
}

enum PairAction {
    SameSlot,
    Absorb,
    Passthrough,
}

fn classify_pair(prev: Option<&HirOp>, curr: &HirOp) -> PairAction {
    let (HirOp::Match(s_prev, prev_arms), HirOp::Match(s_curr, curr_arms)) =
        (match prev { Some(o) => o, None => return PairAction::Passthrough }, curr)
    else {
        return PairAction::Passthrough;
    };
    if s_prev == s_curr {
        return PairAction::SameSlot;
    }
    if try_absorb(*s_prev, prev_arms, *s_curr, curr_arms).is_some() {
        return PairAction::Absorb;
    }
    PairAction::Passthrough
}

fn merge_same_slot(
    scrut: SlotId,
    prev_arms: Vec<(HirBlock, Vec<u8>)>,
    curr_arms: Vec<(HirBlock, Vec<u8>)>,
) -> HirOp {
    let mut new_arms: Vec<(HirBlock, Vec<u8>)> = Vec::new();
    for (prev_body, prev_vals) in &prev_arms {
        for (curr_body, curr_vals) in &curr_arms {
            let intersection: Vec<u8> = prev_vals
                .iter()
                .copied()
                .filter(|v| curr_vals.contains(v))
                .collect();
            if intersection.is_empty() {
                continue;
            }
            let mut ops = prev_body.ops.clone();
            ops.extend(curr_body.ops.clone());
            let merged_block = HirBlock {
                ops,
                // The source arms were statements; their result
                // slots are irrelevant because the match scrutinee
                // dictates control flow. Keep prev's choice as a
                // best-effort.
                result_slot: prev_body.result_slot,
            };
            new_arms.push((merged_block, intersection));
        }
    }
    HirOp::Match(scrut, new_arms)
}

/// When an arm of `Match(s, …)` ends with `Match(s, inner_arms)`
/// (same scrutinee) and no prior op in the arm writes `s`, the
/// inner arms can be hoisted: replace the outer arm with one new
/// outer arm per inner arm, with value sets intersected. The
/// prefix ops (everything in the outer arm before the inner
/// match) are duplicated into each new arm body — safe because
/// they ran before the inner dispatch in the original too.
fn flatten_inner_same_slot(
    scrut: SlotId,
    arms: Vec<(HirBlock, Vec<u8>)>,
) -> Vec<(HirBlock, Vec<u8>)> {
    let mut out: Vec<(HirBlock, Vec<u8>)> = Vec::with_capacity(arms.len());
    for (body, vals) in arms {
        let Some((last, rest)) = body.ops.split_last() else {
            out.push((body, vals));
            continue;
        };
        let HirOp::Match(inner_scrut, inner_arms) = last else {
            out.push((body, vals));
            continue;
        };
        if *inner_scrut != scrut {
            out.push((body, vals));
            continue;
        }
        if rest.iter().any(|op| op_may_write(op, scrut)) {
            // Prefix could change the scrutinee; flattening would
            // change semantics.
            out.push((body, vals));
            continue;
        }
        let prefix: Vec<HirOp> = rest.to_vec();
        let result_slot = body.result_slot;
        for (inner_body, inner_vals) in inner_arms {
            let intersection: Vec<u8> = vals
                .iter()
                .copied()
                .filter(|v| inner_vals.contains(v))
                .collect();
            if intersection.is_empty() {
                continue;
            }
            let mut new_ops = prefix.clone();
            new_ops.extend(inner_body.ops.clone());
            out.push((
                HirBlock {
                    ops: new_ops,
                    result_slot,
                },
                intersection,
            ));
        }
    }
    out
}

/// If every arm of the first match ends with `Set(s2, c)` for
/// a constant `c`, splice the matching arm of the second match
/// into each outer arm and drop the second match.
fn try_absorb(
    s1: SlotId,
    prev_arms: &[(HirBlock, Vec<u8>)],
    s2: SlotId,
    curr_arms: &[(HirBlock, Vec<u8>)],
) -> Option<HirOp> {
    if s1 == s2 {
        return None; // same-slot merge covers this
    }
    // Each outer arm must deterministically set `s2` to a known
    // constant as its final op.
    let outer_determined: Vec<u8> = prev_arms
        .iter()
        .map(|(b, _)| arm_last_set_of(b, s2))
        .collect::<Option<Vec<_>>>()?;

    // Additionally require that `s2` isn't written anywhere
    // earlier in the arm body — otherwise its value at arm exit
    // could be tied to a conditional path. `arm_last_set_of`
    // already checks for mid-body writes.

    let mut new_arms: Vec<(HirBlock, Vec<u8>)> = Vec::with_capacity(prev_arms.len());
    for ((prev_body, prev_vals), c) in prev_arms.iter().zip(outer_determined.iter()) {
        let inner = curr_arms
            .iter()
            .find(|(_, vals)| vals.contains(c))
            .map(|(b, _)| b.clone());
        let mut ops = prev_body.ops.clone();
        if let Some(inner_body) = inner {
            ops.extend(inner_body.ops);
        }
        // If no inner arm covered `c`, the original program would
        // also have no matching arm to fire — behaviour is
        // implementation-defined, so leaving the outer arm body
        // alone is the safest equivalent.
        let merged = HirBlock {
            ops,
            result_slot: prev_body.result_slot,
        };
        new_arms.push((merged, prev_vals.clone()));
    }
    Some(HirOp::Match(s1, new_arms))
}

/// `Some(c)` if `body`'s final op is `Set(slot, c)` and no
/// earlier op writes `slot`; `None` otherwise.
fn arm_last_set_of(body: &HirBlock, slot: SlotId) -> Option<u8> {
    let (last, rest) = body.ops.split_last()?;
    let c = match last {
        HirOp::Set(s, v) if *s == slot => *v,
        _ => return None,
    };
    for op in rest {
        if op_may_write(op, slot) {
            return None;
        }
    }
    Some(c)
}

/// Conservative "does this op possibly write `slot`?". Any
/// uncertainty returns `true` — safe to skip absorption.
fn op_may_write(op: &HirOp, slot: SlotId) -> bool {
    match op {
        HirOp::Set(s, _) => *s == slot,
        HirOp::Copy(dst, _) => *dst == slot,
        HirOp::MapValue(_, dst, _) => *dst == slot,
        HirOp::ReadRegister(dst, _) => *dst == slot,
        HirOp::Match(_, arms) => arms
            .iter()
            .any(|(b, _)| b.ops.iter().any(|op| op_may_write(op, slot))),
        HirOp::Loop(b) | HirOp::Block(b) => {
            b.ops.iter().any(|op| op_may_write(op, slot))
        }
        // Post-inline the program has no `Call` ops left, but
        // pre-inline it could. Be conservative.
        HirOp::Call { ret, .. } => ret.contains(&slot),
        HirOp::WriteRegister(_, _) => false,
        HirOp::Break | HirOp::Continue | HirOp::Stop | HirOp::Skip => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_two_adjacent_matches_on_same_slot() {
        let others35: Vec<u8> = (0..=15u8).filter(|v| *v != 3).collect();
        let others6: Vec<u8> = (0..=15u8).filter(|v| *v != 6).collect();
        let block = HirBlock {
            ops: vec![
                HirOp::Match(
                    SlotId(0),
                    vec![
                        (
                            HirBlock {
                                ops: vec![HirOp::Set(SlotId(1), 1)],
                                result_slot: None,
                            },
                            vec![3],
                        ),
                        (
                            HirBlock {
                                ops: vec![HirOp::Set(SlotId(1), 0)],
                                result_slot: None,
                            },
                            others35,
                        ),
                    ],
                ),
                HirOp::Match(
                    SlotId(0),
                    vec![
                        (
                            HirBlock {
                                ops: vec![HirOp::Set(SlotId(2), 1)],
                                result_slot: None,
                            },
                            vec![6],
                        ),
                        (
                            HirBlock {
                                ops: vec![HirOp::Set(SlotId(2), 0)],
                                result_slot: None,
                            },
                            others6,
                        ),
                    ],
                ),
            ],
            result_slot: None,
        };
        let merged = merge_adjacent_matches(block);
        assert_eq!(merged.ops.len(), 1, "two matches should collapse to one");
        let HirOp::Match(s, arms) = &merged.ops[0] else {
            panic!("expected Match");
        };
        assert_eq!(*s, SlotId(0));
        assert_eq!(
            arms.len(),
            3,
            "expected 3 arms after Cartesian merge (got {})",
            arms.len()
        );
        let three_arm = arms
            .iter()
            .find(|(_, vs)| vs == &vec![3])
            .expect("arm for scrutinee=3");
        assert_eq!(
            three_arm.0.ops,
            vec![HirOp::Set(SlotId(1), 1), HirOp::Set(SlotId(2), 0)]
        );
        let six_arm = arms
            .iter()
            .find(|(_, vs)| vs == &vec![6])
            .expect("arm for scrutinee=6");
        assert_eq!(
            six_arm.0.ops,
            vec![HirOp::Set(SlotId(1), 0), HirOp::Set(SlotId(2), 1)]
        );
    }

    #[test]
    fn does_not_merge_matches_on_different_scrutinees() {
        let block = HirBlock {
            ops: vec![
                HirOp::Match(
                    SlotId(0),
                    vec![(HirBlock::new(), vec![0])],
                ),
                HirOp::Match(
                    SlotId(1),
                    vec![(HirBlock::new(), vec![0])],
                ),
            ],
            result_slot: None,
        };
        let merged = merge_adjacent_matches(block);
        // With different scrutinees and empty outer-arm bodies,
        // there's nothing to absorb (no Set(s2, c) in the outer
        // arm), so both matches must stay.
        assert_eq!(merged.ops.len(), 2, "different scrutinees with no determination must stay separate");
    }

    #[test]
    fn recurses_into_nested_blocks() {
        let inner = HirBlock {
            ops: vec![
                HirOp::Match(SlotId(0), vec![(HirBlock::new(), vec![0])]),
                HirOp::Match(SlotId(0), vec![(HirBlock::new(), vec![0])]),
            ],
            result_slot: None,
        };
        let outer = HirBlock {
            ops: vec![HirOp::Loop(inner)],
            result_slot: None,
        };
        let merged = merge_adjacent_matches(outer);
        let HirOp::Loop(inner) = &merged.ops[0] else {
            panic!();
        };
        assert_eq!(inner.ops.len(), 1, "nested matches should merge too");
    }

    #[test]
    fn absorbs_boolean_temp_match() {
        // Match(s0, [[3]→s1=1, others→s1=0])
        // Match(s1, [[0]→s2=9, [1..=15]→s2=7])
        // After absorb: Match(s0, [[3]→(s1=1; s2=7), others→(s1=0; s2=9)])
        let others3: Vec<u8> = (0..=15u8).filter(|v| *v != 3).collect();
        let nonzero: Vec<u8> = (1..=15u8).collect();
        let block = HirBlock {
            ops: vec![
                HirOp::Match(
                    SlotId(0),
                    vec![
                        (
                            HirBlock {
                                ops: vec![HirOp::Set(SlotId(1), 1)],
                                result_slot: None,
                            },
                            vec![3],
                        ),
                        (
                            HirBlock {
                                ops: vec![HirOp::Set(SlotId(1), 0)],
                                result_slot: None,
                            },
                            others3,
                        ),
                    ],
                ),
                HirOp::Match(
                    SlotId(1),
                    vec![
                        (
                            HirBlock {
                                ops: vec![HirOp::Set(SlotId(2), 9)],
                                result_slot: None,
                            },
                            vec![0],
                        ),
                        (
                            HirBlock {
                                ops: vec![HirOp::Set(SlotId(2), 7)],
                                result_slot: None,
                            },
                            nonzero,
                        ),
                    ],
                ),
            ],
            result_slot: None,
        };
        let merged = merge_adjacent_matches(block);
        assert_eq!(merged.ops.len(), 1, "second match should be absorbed");
        let HirOp::Match(s, arms) = &merged.ops[0] else {
            panic!("expected Match");
        };
        assert_eq!(*s, SlotId(0));
        let three_arm = arms
            .iter()
            .find(|(_, vs)| vs == &vec![3])
            .expect("arm for scrutinee=3");
        assert_eq!(
            three_arm.0.ops,
            vec![HirOp::Set(SlotId(1), 1), HirOp::Set(SlotId(2), 7)],
            "s0=3 → s1=1 → s2=7"
        );
        let others_arm = arms
            .iter()
            .find(|(_, vs)| vs != &vec![3])
            .expect("arm for scrutinee=others");
        assert_eq!(
            others_arm.0.ops,
            vec![HirOp::Set(SlotId(1), 0), HirOp::Set(SlotId(2), 9)],
            "s0∈others → s1=0 → s2=9"
        );
    }

    #[test]
    fn flattens_same_slot_match_at_arm_tail() {
        // Match(s0, [
        //   [3]      → A,
        //   [others] → { Set(s1, 0); Match(s0, [[6]→B, [others6]→C]) }
        // ])
        // should flatten to:
        // Match(s0, [
        //   [3]            → A,
        //   [6]            → { Set(s1, 0); B },   // 6 ∈ others of 3
        //   [others_3_6]   → { Set(s1, 0); C },
        // ])
        let others3: Vec<u8> = (0..=15u8).filter(|v| *v != 3).collect();
        let others6: Vec<u8> = (0..=15u8).filter(|v| *v != 6).collect();
        let outer = HirBlock {
            ops: vec![HirOp::Match(
                SlotId(0),
                vec![
                    (
                        HirBlock {
                            ops: vec![HirOp::Set(SlotId(9), 1)],
                            result_slot: None,
                        },
                        vec![3],
                    ),
                    (
                        HirBlock {
                            ops: vec![
                                HirOp::Set(SlotId(1), 0),
                                HirOp::Match(
                                    SlotId(0),
                                    vec![
                                        (
                                            HirBlock {
                                                ops: vec![HirOp::Set(SlotId(2), 1)],
                                                result_slot: None,
                                            },
                                            vec![6],
                                        ),
                                        (
                                            HirBlock {
                                                ops: vec![HirOp::Set(SlotId(2), 0)],
                                                result_slot: None,
                                            },
                                            others6,
                                        ),
                                    ],
                                ),
                            ],
                            result_slot: None,
                        },
                        others3,
                    ),
                ],
            )],
            result_slot: None,
        };
        let merged = merge_adjacent_matches(outer);
        let HirOp::Match(s, arms) = &merged.ops[0] else {
            panic!("expected Match");
        };
        assert_eq!(*s, SlotId(0));
        assert_eq!(arms.len(), 3, "expected 3 flat arms, got {}", arms.len());
        let arm_3 = arms.iter().find(|(_, v)| v == &vec![3]).unwrap();
        assert_eq!(arm_3.0.ops, vec![HirOp::Set(SlotId(9), 1)]);
        let arm_6 = arms.iter().find(|(_, v)| v == &vec![6]).unwrap();
        assert_eq!(
            arm_6.0.ops,
            vec![HirOp::Set(SlotId(1), 0), HirOp::Set(SlotId(2), 1)]
        );
        let arm_others = arms
            .iter()
            .find(|(_, v)| v.len() > 1)
            .expect("leftover arm");
        assert_eq!(
            arm_others.0.ops,
            vec![HirOp::Set(SlotId(1), 0), HirOp::Set(SlotId(2), 0)]
        );
    }

    #[test]
    fn does_not_absorb_when_outer_arm_doesnt_fully_determine_temp() {
        // Outer arm doesn't end with Set(s1, _), so s1's value
        // at arm-exit isn't fully determined.
        let block = HirBlock {
            ops: vec![
                HirOp::Match(
                    SlotId(0),
                    vec![(HirBlock::new(), vec![0])], // no Set(s1, _) at all
                ),
                HirOp::Match(
                    SlotId(1),
                    vec![(HirBlock::new(), vec![0])],
                ),
            ],
            result_slot: None,
        };
        let merged = merge_adjacent_matches(block);
        assert_eq!(merged.ops.len(), 2, "absorb should bail when temp isn't determined");
    }
}
