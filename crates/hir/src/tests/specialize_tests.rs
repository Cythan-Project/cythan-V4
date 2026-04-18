//! Specialization pass tests — flow-sensitive constant propagation
//! and match folding on hand-built HIR blocks.

use crate::ir::*;
use crate::specialize::specialize_to_fixpoint;

fn sid(n: u32) -> SlotId { SlotId(n) }
fn blk(ops: Vec<HirOp>) -> HirBlock {
    HirBlock { ops, result_slot: None }
}

/// The hand-built `if_zero` shape — scrutinee `s`, then / else.
fn if_zero(s: SlotId, then_: Vec<HirOp>, else_: Vec<HirOp>) -> HirOp {
    HirOp::if_zero(s, blk(then_), blk(else_))
}

#[test]
fn constant_propagates_through_copy() {
    // Set(s0, 7); Copy(s1, s0)
    //   →  Set(s0, 7); Set(s1, 7)
    let input = blk(vec![
        HirOp::Set(sid(0), 7),
        HirOp::Copy(sid(1), sid(0)),
    ]);
    let out = specialize_to_fixpoint(input);
    assert_eq!(
        out.ops,
        vec![HirOp::Set(sid(0), 7), HirOp::Set(sid(1), 7)]
    );
}

#[test]
fn match_folds_when_scrutinee_is_known() {
    // Set(s0, 0); if_zero(s0, then=[Set(s1,10)], else=[Set(s1,99)])
    //   scrutinee is known == 0 → splice `then` body, drop `else`.
    let input = blk(vec![
        HirOp::Set(sid(0), 0),
        if_zero(
            sid(0),
            vec![HirOp::Set(sid(1), 10)],
            vec![HirOp::Set(sid(1), 99)],
        ),
    ]);
    let out = specialize_to_fixpoint(input);
    assert!(
        out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 10))),
        "then-branch body missing: {:#?}",
        out.ops
    );
    assert!(
        out.ops.iter().all(|op| !matches!(op, HirOp::Set(SlotId(1), 99))),
        "else-branch leaked: {:#?}",
        out.ops
    );
    // Match should be gone entirely.
    assert!(
        !out.ops.iter().any(|op| matches!(op, HirOp::Match(..))),
        "match should have been folded"
    );
}

#[test]
fn match_folds_when_scrutinee_is_nonzero_constant() {
    // Set(s0, 5); if_zero(s0, then=[Set(s1,10)], else=[Set(s1,99)])
    //   scrutinee is 5 ∈ 1..=15 → the else arm wins.
    let input = blk(vec![
        HirOp::Set(sid(0), 5),
        if_zero(
            sid(0),
            vec![HirOp::Set(sid(1), 10)],
            vec![HirOp::Set(sid(1), 99)],
        ),
    ]);
    let out = specialize_to_fixpoint(input);
    assert!(
        out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 99))),
        "else-branch body missing"
    );
    assert!(
        out.ops.iter().all(|op| !matches!(op, HirOp::Set(SlotId(1), 10))),
        "then-branch leaked"
    );
}

#[test]
fn arm_local_knowledge_folds_nested_match() {
    // if_zero(s0,
    //     then = [ if_zero(s0, [Set(s1, 1)], [Set(s1, 2)]) ],
    //     else = [ Set(s1, 3) ]
    // )
    // Inside the outer `then`, the pass learns `s0 == 0`; the
    // nested `if_zero(s0, …)` then folds to its own `then` arm
    // (since 0 ∈ [0]).
    let input = blk(vec![if_zero(
        sid(0),
        vec![if_zero(
            sid(0),
            vec![HirOp::Set(sid(1), 1)],
            vec![HirOp::Set(sid(1), 2)],
        )],
        vec![HirOp::Set(sid(1), 3)],
    )]);
    let out = specialize_to_fixpoint(input);
    // Walk the result to find arm[0] (values `[0]`) and check it
    // no longer contains a Match.
    let HirOp::Match(_, arms) = &out.ops[0] else {
        panic!("outer Match should survive — scrutinee unknown");
    };
    let then_arm = &arms[0].0;
    assert!(
        !then_arm.ops.iter().any(|op| matches!(op, HirOp::Match(..))),
        "nested match should have folded away inside the `s0 == 0` arm: {:#?}",
        then_arm.ops
    );
    assert!(
        then_arm.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 1))),
        "the right nested arm body should have been spliced in"
    );
}

#[test]
fn inc_dec_track_known_value() {
    // Set(s0, 5); Inc(s0); Copy(s1, s0)
    //   →  Set(s0, 5); Inc(s0); Set(s1, 6)   (s0 is 6 after Inc)
    let input = blk(vec![
        HirOp::Set(sid(0), 5),
        HirOp::Inc(sid(0)),
        HirOp::Copy(sid(1), sid(0)),
    ]);
    let out = specialize_to_fixpoint(input);
    assert!(
        out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 6))),
        "Copy should have collapsed to Set(s1, 6): {:#?}",
        out.ops
    );
}

#[test]
fn loop_boundary_clears_known_values() {
    // Set(s0, 3); Loop { Inc(s0) }; Copy(s1, s0)
    //   After the loop, s0 could be anything — the Copy must NOT
    //   collapse to a Set(s1, 3).
    let input = blk(vec![
        HirOp::Set(sid(0), 3),
        HirOp::Loop(blk(vec![HirOp::Inc(sid(0))])),
        HirOp::Copy(sid(1), sid(0)),
    ]);
    let out = specialize_to_fixpoint(input);
    assert!(
        out.ops.iter().any(|op| matches!(op, HirOp::Copy(SlotId(1), SlotId(0)))),
        "Copy must survive after the loop — s0 is unknown: {:#?}",
        out.ops
    );
}

#[test]
fn write_register_with_known_slot_collapses_to_literal() {
    // Set(s0, 7); WriteRegister(1, slot=s0)
    //   →  Set(s0, 7); WriteRegister(1, literal=7)
    use either::Either;
    let input = blk(vec![
        HirOp::Set(sid(0), 7),
        HirOp::WriteRegister(1, Either::Right(sid(0))),
    ]);
    let out = specialize_to_fixpoint(input);
    let has_literal = out.ops.iter().any(
        |op| matches!(op, HirOp::WriteRegister(1, Either::Left(7))),
    );
    assert!(
        has_literal,
        "WriteRegister should have collapsed to literal: {:#?}",
        out.ops
    );
}
