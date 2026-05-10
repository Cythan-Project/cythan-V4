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
        HirOp::inc(sid(0)),
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
fn mutated_slot_is_forgotten_after_loop() {
    // Set(s0, 3); Loop { Inc(s0) }; Copy(s1, s0)
    //   After the loop, s0 could be anything (Inc ran N times) —
    //   the Copy must NOT collapse to a Set(s1, 3).
    let input = blk(vec![
        HirOp::Set(sid(0), 3),
        HirOp::Loop(blk(vec![HirOp::inc(sid(0))])),
        HirOp::Copy(sid(1), sid(0)),
    ]);
    let out = specialize_to_fixpoint(input);
    assert!(
        out.ops.iter().any(|op| matches!(op, HirOp::Copy(SlotId(1), SlotId(0)))),
        "Copy must survive — s0 is mutated inside the loop: {:#?}",
        out.ops
    );
}

#[test]
fn readonly_slot_keeps_domain_inside_loop() {
    // Set(s0, 5); Loop { Inc(s1); if_zero(s0, [Break], []) }
    //   s0 is NOT mutated inside the loop — its domain {5}
    //   survives into the body. The nested if_zero's scrutinee
    //   is known to be `5` (nonzero), so the then (Break) arm
    //   is dead; the whole if_zero folds to its else body
    //   (which is empty). The loop body specializes to just
    //   `Inc(s1)`.
    let input = blk(vec![
        HirOp::Set(sid(0), 5),
        HirOp::Loop(blk(vec![
            HirOp::inc(sid(1)),
            if_zero(sid(0), vec![HirOp::Break], vec![]),
        ])),
    ]);
    let out = specialize_to_fixpoint(input);
    let HirOp::Loop(body) = &out.ops[1] else {
        panic!("Loop missing at index 1");
    };
    assert!(
        !body.ops.iter().any(|op| matches!(op, HirOp::Match(..))),
        "nested if_zero on read-only s0==5 should have folded: {:#?}",
        body.ops
    );
    assert!(
        !body.ops.iter().any(|op| matches!(op, HirOp::Break)),
        "dead then-branch body (Break) should have been dropped"
    );
}

#[test]
fn dse_sees_through_match_that_doesnt_read_slot() {
    // Set(v0, 1); Match(cond, [... none read v0 ...]); Copy(v0, v1)
    // Previously the Match was an opaque barrier; the recursive
    // reader-check lets DSE look inside each arm and confirm v0
    // is untouched, so the initial Set is still dead.
    use crate::opt::optimize_block;
    let input = blk(vec![
        HirOp::Set(sid(0), 1), // dead: v0 never read before Copy below
        HirOp::Match(
            sid(2),
            vec![
                (blk(vec![HirOp::Set(sid(3), 0)]), vec![0]),
                (blk(vec![HirOp::inc(sid(3))]), (1..=15).collect()),
            ],
        ),
        HirOp::Copy(sid(0), sid(1)),
    ]);
    let out = optimize_block(input);
    assert!(
        !out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(0), 1))),
        "DSE should have killed the dead Set through the transparent Match: {:#?}",
        out.ops
    );
}

#[test]
fn dse_preserves_set_read_inside_match_arm() {
    // Same shape but an arm reads v0 — Set must survive.
    use crate::opt::optimize_block;
    let input = blk(vec![
        HirOp::Set(sid(0), 1),
        HirOp::Match(
            sid(2),
            vec![
                (blk(vec![HirOp::Copy(sid(3), sid(0))]), vec![0]), // reads v0
                (blk(vec![HirOp::inc(sid(3))]), (1..=15).collect()),
            ],
        ),
        HirOp::Copy(sid(0), sid(1)),
    ]);
    let out = optimize_block(input);
    assert!(
        out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(0), 1))),
        "DSE must NOT kill Set(v0, 1) — it's read inside the arm: {:#?}",
        out.ops
    );
}

#[test]
fn identity_copy_is_removed() {
    // Copy(x, x) is a no-op; the pass should drop it.
    let input = blk(vec![
        HirOp::Set(sid(0), 3),
        HirOp::Copy(sid(0), sid(0)),
        HirOp::inc(sid(0)),
    ]);
    let out = specialize_to_fixpoint(input);
    assert!(
        !out.ops.iter().any(|op| matches!(op, HirOp::Copy(s, t) if s == t)),
        "identity Copy(x, x) should be gone: {:#?}",
        out.ops
    );
    assert_eq!(
        out.ops.len(),
        2,
        "expected exactly Set + Inc after removing the no-op Copy"
    );
}

#[test]
fn dse_removes_set_killed_by_later_copy() {
    // The user's example: Set(v0, 1); <code without v0>; Copy(v0, v1).
    // The trailing Copy writes v0 without anyone having read it in
    // between — the initial Set is dead and should go away.
    //
    // Note: this exercises `hir::opt::optimize_block`'s DSE, which
    // `new_pipeline::compile` runs after the specialize pass.
    // Keeps the test in the specialize suite so the DSE contract
    // stays visible next to the pass it backs up.
    use crate::opt::optimize_block;
    let input = blk(vec![
        HirOp::Set(sid(0), 1),
        HirOp::Set(sid(2), 5),  // arbitrary filler that doesn't touch v0
        HirOp::Copy(sid(0), sid(1)),
    ]);
    let out = optimize_block(input);
    assert!(
        !out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(0), 1))),
        "dead Set(v0, 1) should have been eliminated: {:#?}",
        out.ops
    );
    assert!(
        out.ops.iter().any(|op| matches!(op, HirOp::Copy(SlotId(0), SlotId(1)))),
        "the killing Copy must survive"
    );
}

#[test]
fn readonly_slot_keeps_domain_across_loop_exit() {
    // Set(s0, 7); Loop { Inc(s1); Break }; Copy(s2, s0)
    //   s0 read-only inside the loop, so its domain {7} survives
    //   the loop exit — the Copy should collapse to Set(s2, 7).
    let input = blk(vec![
        HirOp::Set(sid(0), 7),
        HirOp::Loop(blk(vec![HirOp::inc(sid(1)), HirOp::Break])),
        HirOp::Copy(sid(2), sid(0)),
    ]);
    let out = specialize_to_fixpoint(input);
    assert!(
        out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(2), 7))),
        "Copy must collapse to Set(s2, 7) — s0 is read-only: {:#?}",
        out.ops
    );
}

#[test]
fn nested_if_zero_in_else_arm_folds_to_else_body() {
    // if_zero(s0,
    //   then = [],
    //   else = [ if_zero(s0, then=[Set(s1,10)], else=[Set(s1,99)]) ],
    // )
    // In the outer `else` arm, s0 ∈ {1..=15}. The nested `if_zero`
    // on the same slot can prove its `then` arm (`s0 == 0`) dead,
    // so the whole nested match specializes to just its `else`
    // body: Set(s1, 99).
    let input = blk(vec![if_zero(
        sid(0),
        vec![], // then: nothing
        vec![if_zero(
            sid(0),
            vec![HirOp::Set(sid(1), 10)],
            vec![HirOp::Set(sid(1), 99)],
        )],
    )]);
    let out = specialize_to_fixpoint(input);
    // The outer Match survives (scrutinee unknown at entry), but
    // its `else` arm body should no longer contain any Match —
    // only the `Set(s1, 99)` from the spliced inner `else`.
    let HirOp::Match(_, arms) = &out.ops[0] else {
        panic!("expected outer Match to survive");
    };
    let else_arm = &arms[1].0;
    assert!(
        !else_arm.ops.iter().any(|op| matches!(op, HirOp::Match(..))),
        "nested if_zero should have folded in the else arm: {:#?}",
        else_arm.ops
    );
    assert!(
        else_arm.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 99))),
        "expected Set(s1, 99) from the nested else arm"
    );
    assert!(
        !else_arm.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 10))),
        "Set(s1, 10) from the dead nested then arm must be gone"
    );
}

#[test]
fn match_arm_with_unreachable_values_is_pruned() {
    // Set(s0, 0) → s0 is known {0}. A Match with arms
    // `[3]` (dead) and `[0,1,2]` should drop the first arm.
    let input = blk(vec![
        HirOp::Set(sid(0), 0),
        HirOp::Match(
            sid(0),
            vec![
                (blk(vec![HirOp::Set(sid(1), 30)]), vec![3]),
                (blk(vec![HirOp::Set(sid(1), 99)]), vec![0, 1, 2]),
            ],
        ),
    ]);
    let out = specialize_to_fixpoint(input);
    // The 0 arm entirely contains {0}, so it fires — the pass
    // splices its body (Set(s1, 99)) in place and drops the Match.
    assert!(
        out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 99))),
        "spliced body missing"
    );
    assert!(
        out.ops.iter().all(|op| !matches!(op, HirOp::Set(SlotId(1), 30))),
        "dead arm survived"
    );
}

#[test]
fn forbidden_value_narrows_intersecting_arms() {
    // Enter the `else` arm of an if_zero, where s0 ∈ {1..=15}.
    // Inside, a Match(s0, [[0, 5], [10]]) has an arm `[0, 5]` —
    // intersected with {1..=15} that's just `{5}`, but the pass
    // doesn't rewrite value lists (arm shapes stay). It just
    // confirms no arm is entirely dead: both `[0, 5]` and `[10]`
    // intersect {1..=15}, so both survive.
    //
    // The interesting part: add an arm `[0]` — wholly outside
    // {1..=15} — and watch it disappear.
    let input = blk(vec![if_zero(
        sid(0),
        vec![], // then
        vec![HirOp::Match(
            sid(0),
            vec![
                (blk(vec![HirOp::Set(sid(1), 1)]), vec![0]), // dead
                (blk(vec![HirOp::Set(sid(1), 2)]), vec![7]),
                (blk(vec![HirOp::Set(sid(1), 3)]), vec![0, 10]), // intersects
            ],
        )],
    )]);
    let out = specialize_to_fixpoint(input);
    // The outer Match survives; look into its else arm.
    let HirOp::Match(_, arms) = &out.ops[0] else {
        panic!("outer Match expected");
    };
    let else_body = &arms[1].0;
    // Find the inner Match — it should have 2 arms now (first dead).
    let inner = else_body
        .ops
        .iter()
        .find_map(|op| match op {
            HirOp::Match(_, a) => Some(a),
            _ => None,
        })
        .expect("inner Match should survive");
    assert_eq!(
        inner.len(),
        2,
        "one dead arm should have been pruned; got {:#?}",
        inner
    );
    // The remaining arm bodies must include both Set(s1,2) and Set(s1,3).
    let has_2 = inner
        .iter()
        .any(|(b, _)| b.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 2))));
    let has_3 = inner
        .iter()
        .any(|(b, _)| b.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 3))));
    assert!(has_2 && has_3);
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
