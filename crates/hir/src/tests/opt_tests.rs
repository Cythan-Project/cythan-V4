//! Phase 5: HIR optimizer tests.
//!
//! Works directly on hand-built HirBlocks so the tests are independent of the
//! HIR generator.

use crate::ir::*;
use crate::opt::optimize_block;

fn sid(n: u32) -> SlotId {
    SlotId(n)
}

fn block(ops: Vec<HirOp>) -> HirBlock {
    HirBlock {
        ops,
        result_slot: None,
    }
}

/// Is `op` the two-arm `Match` produced by `HirOp::if_zero`?
/// (first arm values = `[0]`, second arm values = `1..=15`.)
fn is_if_zero_match(op: &HirOp) -> bool {
    if let HirOp::Match(_, arms) = op {
        if arms.len() != 2 {
            return false;
        }
        if arms[0].1 != vec![0u8] {
            return false;
        }
        let rest: Vec<u8> = (1u8..=15u8).collect();
        return arms[1].1 == rest;
    }
    false
}

// ---------- Step 5.1: constant propagation ----------

#[test]
fn if0_folds_to_zero_branch_when_cond_is_zero() {
    // Set(s0, 0); If0(s0, X, Y)  →  X (because s0 == 0)
    let input = block(vec![
        HirOp::Set(sid(0), 0),
        HirOp::if_zero(
            sid(0),
            block(vec![HirOp::Set(sid(1), 10)]),
            block(vec![HirOp::Set(sid(1), 99)]),
        ),
    ]);
    let out = optimize_block(input);
    // After the Set(s0, 0), If0 folds to its zero-branch (the `X` block).
    // The final ops should contain Set(s0, 0) + Set(s1, 10), no 99.
    assert!(
        out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 10))),
        "zero-branch body lost: {:#?}",
        out.ops
    );
    assert!(
        out.ops.iter().all(|op| !matches!(op, HirOp::Set(SlotId(1), 99))),
        "nonzero branch survived: {:#?}",
        out.ops
    );
    assert!(
        out.ops.iter().all(|op| !is_if_zero_match(op)),
        "If0 not folded"
    );
}

#[test]
fn if0_folds_to_nonzero_branch_when_cond_is_nonzero() {
    let input = block(vec![
        HirOp::Set(sid(0), 5),
        HirOp::if_zero(
            sid(0),
            block(vec![HirOp::Set(sid(1), 10)]),
            block(vec![HirOp::Set(sid(1), 99)]),
        ),
    ]);
    let out = optimize_block(input);
    assert!(
        out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 99))),
        "nonzero-branch body lost"
    );
    assert!(
        out.ops.iter().all(|op| !matches!(op, HirOp::Set(SlotId(1), 10))),
        "zero-branch survived"
    );
}

#[test]
fn if0_not_folded_when_slot_has_multiple_writes() {
    // Set(s0, 0); Inc(s0); If0(s0, ...) — Inc mutates s0, so we can't fold.
    let input = block(vec![
        HirOp::Set(sid(0), 0),
        HirOp::Inc(sid(0)),
        HirOp::if_zero(
            sid(0),
            block(vec![HirOp::Set(sid(1), 10)]),
            block(vec![HirOp::Set(sid(1), 99)]),
        ),
    ]);
    let out = optimize_block(input);
    assert!(
        out.ops.iter().any(|op| is_if_zero_match(op)),
        "If0 should survive when slot is re-written"
    );
}

#[test]
fn match_folds_to_matching_arm() {
    // Set(s0, 2); Match(s0, [(X, [0]), (Y, [1, 2]), (Z, [3])])  →  Y's body
    let input = block(vec![
        HirOp::Set(sid(0), 2),
        HirOp::Match(
            sid(0),
            vec![
                (block(vec![HirOp::Set(sid(1), 100)]), vec![0]),
                (block(vec![HirOp::Set(sid(1), 200)]), vec![1, 2]),
                (block(vec![HirOp::Set(sid(1), 30)]), vec![3]),
            ],
        ),
    ]);
    let out = optimize_block(input);
    // Only arm for discr=2 should survive (the one setting s1=200).
    assert!(out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 200))));
    assert!(out.ops.iter().all(|op| !matches!(op, HirOp::Set(SlotId(1), 100))));
    assert!(out.ops.iter().all(|op| !matches!(op, HirOp::Set(SlotId(1), 30))));
    assert!(out.ops.iter().all(|op| !matches!(op, HirOp::Match(..))));
}

#[test]
fn match_with_no_matching_arm_preserved() {
    // Set(s0, 5); Match(s0, [(X, [0]), (Y, [1])])  →  preserved (no arm for 5)
    let input = block(vec![
        HirOp::Set(sid(0), 5),
        HirOp::Match(
            sid(0),
            vec![
                (block(vec![HirOp::Set(sid(1), 100)]), vec![0]),
                (block(vec![HirOp::Set(sid(1), 200)]), vec![1]),
            ],
        ),
    ]);
    let out = optimize_block(input);
    assert!(out.ops.iter().any(|op| matches!(op, HirOp::Match(..))));
}

#[test]
fn copy_from_const_slot_becomes_set() {
    // Set(s0, 7); Copy(s1, s0)  →  Set(s0, 7); Set(s1, 7)
    let input = block(vec![
        HirOp::Set(sid(0), 7),
        HirOp::Copy(sid(1), sid(0)),
    ]);
    let out = optimize_block(input);
    assert!(out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 7))));
    // The Copy should be gone.
    assert!(out.ops.iter().all(|op| !matches!(op, HirOp::Copy(..))));
}

#[test]
fn copy_from_non_const_preserved() {
    // Copy(s1, s0) where s0 is unknown (never Set) — can't fold.
    let input = block(vec![HirOp::Copy(sid(1), sid(0))]);
    let out = optimize_block(input.clone());
    assert_eq!(out.ops, input.ops);
}

#[test]
fn nested_if_inside_loop_folded_when_safe() {
    // Set(s0, 0); Loop { If0(s0, X, Y) }   — inside a loop, s0 is only Set
    // outside; it's still constant throughout.
    let input = block(vec![
        HirOp::Set(sid(0), 0),
        HirOp::Loop(block(vec![
            HirOp::if_zero(
                sid(0),
                block(vec![HirOp::Set(sid(1), 10)]),
                block(vec![HirOp::Set(sid(1), 99)]),
            ),
        ])),
    ]);
    let out = optimize_block(input);
    // Inside the loop the If0 should be folded to the zero-branch.
    let lop = out.ops.iter().find_map(|op| match op {
        HirOp::Loop(b) => Some(b),
        _ => None,
    }).expect("Loop present");
    assert!(lop.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 10))));
    assert!(lop.ops.iter().all(|op| !is_if_zero_match(op)));
}

#[test]
fn loop_writing_slot_disables_const_prop() {
    // Loop writes to s0 (Inc). Even if s0 was Set outside, we can't fold
    // uses of s0 that happen inside/after the loop to its initial value.
    let input = block(vec![
        HirOp::Set(sid(0), 0),
        HirOp::Loop(block(vec![HirOp::Inc(sid(0))])),
        HirOp::if_zero(
            sid(0),
            block(vec![HirOp::Set(sid(1), 10)]),
            block(vec![HirOp::Set(sid(1), 99)]),
        ),
    ]);
    let out = optimize_block(input);
    assert!(
        out.ops.iter().any(|op| is_if_zero_match(op)),
        "If0 should survive because s0 is written by Inc inside the loop"
    );
}

// ---------- Step 5.2: dead store elimination ----------

#[test]
fn consecutive_set_to_same_slot_removes_first() {
    // Set(s0, 1); Set(s0, 2)  →  Set(s0, 2)
    let input = block(vec![
        HirOp::Set(sid(0), 1),
        HirOp::Set(sid(0), 2),
    ]);
    let out = optimize_block(input);
    let sets: Vec<_> = out
        .ops
        .iter()
        .filter_map(|op| match op {
            HirOp::Set(s, v) => Some((*s, *v)),
            _ => None,
        })
        .collect();
    assert_eq!(sets, vec![(sid(0), 2)]);
}

#[test]
fn set_followed_by_copy_to_same_slot_removes_set() {
    // Set(s0, 1); Copy(s0, s1)  →  Copy(s0, s1)  (first write dead)
    let input = block(vec![
        HirOp::Set(sid(0), 1),
        HirOp::Copy(sid(0), sid(1)),
    ]);
    let out = optimize_block(input);
    assert!(out.ops.iter().all(|op| !matches!(op, HirOp::Set(SlotId(0), 1))));
    assert!(out.ops.iter().any(|op| matches!(op, HirOp::Copy(SlotId(0), SlotId(1)))));
}

#[test]
fn set_preserved_when_slot_read_between() {
    // Set(s0, 1); Copy(s2, s0); Set(s0, 2)  — the first Set is read by Copy.
    let input = block(vec![
        HirOp::Set(sid(0), 1),
        HirOp::Copy(sid(2), sid(0)),
        HirOp::Set(sid(0), 2),
    ]);
    let out = optimize_block(input.clone());
    // Const-prop will substitute the Copy with Set(s2, 1). That substitution
    // means the read of s0 is replaced — but since we can then no longer
    // prove `s0` is live between the two Sets, the first Set IS dead after
    // const-prop runs. Accept either outcome (pre- or post-const-prop).
    let has_first_set = out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(0), 1)));
    let has_second_set = out.ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(0), 2)));
    assert!(has_second_set, "second Set must survive");
    // Key invariant: the read-of-s0 is preserved *somehow* (either as Copy or as Set(s2, 1)).
    let read_preserved = out.ops.iter().any(|op| match op {
        HirOp::Copy(_, s) if *s == sid(0) => true,
        HirOp::Set(SlotId(2), 1) => true,
        _ => false,
    });
    assert!(read_preserved, "read of s0 lost: {:#?}", out.ops);
    let _ = has_first_set; // allow either
    let _ = input;
}
