//! Step 4.3: if/else, loop, match, return, break, continue.

use crate::ir::*;
use crate::tests::compile;

/// Locate the first `HirOp::if_zero`-shaped `Match` in `ops`.
/// HIR has no `If0` variant; zero-vs-nonzero branches are expressed
/// as a two-arm `Match` (`[0]` then `1..=15`).
fn find_if0(ops: &[HirOp]) -> Option<(&SlotId, &HirBlock, &HirBlock)> {
    for op in ops {
        if let HirOp::Match(s, arms) = op {
            if arms.len() == 2 && arms[0].1 == vec![0u8] {
                let rest: Vec<u8> = (1u8..=15u8).collect();
                if arms[1].1 == rest {
                    return Some((s, &arms[0].0, &arms[1].0));
                }
            }
        }
    }
    None
}

#[test]
fn if_else_compiles_to_if0_with_matched_result_slots() {
    // `fn pick(self): Self { if self { Self::X } else { Self::Empty } }` on Cell.
    let hir = compile(
        r#"
        enum Cell { Empty, O, X, }
        extension Cell {
            fn pick(self): Self {
                if self { Self::X } else { Self::Empty }
            }
        }
        "#,
        "Cell",
        "pick",
    );
    // _ret is s1 (after self=s0). Both arms should target it.
    let (_cond, when_zero, when_nonzero) = find_if0(&hir.body.ops).expect("If0");
    assert_eq!(when_zero.result_slot, Some(SlotId(1)));
    assert_eq!(when_nonzero.result_slot, Some(SlotId(1)));
}

#[test]
fn loop_emits_loop_op() {
    let hir = compile(
        r#"
        extension U4 {
            fn spin() {
                loop { break; }
            }
        }
        "#,
        "U4",
        "spin",
    );
    let has_loop = hir.body.ops.iter().any(|op| matches!(op, HirOp::Loop(_)));
    assert!(has_loop);
}

#[test]
fn break_continue_in_loop_emit_ops() {
    let hir = compile(
        r#"
        extension U4 {
            fn spin() {
                loop {
                    continue;
                    break;
                }
            }
        }
        "#,
        "U4",
        "spin",
    );
    let lop = hir.body.ops.iter().find_map(|op| match op {
        HirOp::Loop(b) => Some(b),
        _ => None,
    }).unwrap();
    assert!(lop.ops.iter().any(|op| matches!(op, HirOp::Break)));
    assert!(lop.ops.iter().any(|op| matches!(op, HirOp::Continue)));
}

#[test]
fn return_with_value_writes_output_and_stops() {
    let hir = compile(
        r#"
        extension U4 {
            fn five(): Self {
                return 5;
            }
        }
        "#,
        "U4",
        "five",
    );
    // Sequence must be Set(s0, 5); Stop.
    let mut saw_set = false;
    let mut saw_stop = false;
    for op in &hir.body.ops {
        match op {
            HirOp::Set(SlotId(0), 5) => saw_set = true,
            HirOp::Stop => saw_stop = true,
            _ => {}
        }
    }
    assert!(saw_set && saw_stop, "ops: {:#?}", hir.body.ops);
}

#[test]
fn return_without_value_in_void_fn_emits_stop() {
    let hir = compile(
        r#"
        extension U4 {
            fn go() {
                return;
            }
        }
        "#,
        "U4",
        "go",
    );
    assert!(hir.body.ops.iter().any(|op| matches!(op, HirOp::Stop)));
}

#[test]
fn match_emits_match_op_with_correct_arms() {
    // On Cell, discriminants Empty=0, O=1, X=2.
    let hir = compile(
        r#"
        enum Cell { Empty, O, X, }
        struct Bool { U4 value, }
        extension Cell {
            fn is_empty(self): Bool {
                match self {
                    Self::Empty => true,
                    _ => false,
                }
            }
        }
        "#,
        "Cell",
        "is_empty",
    );
    // Expect exactly one Match op.
    let m = hir.body.ops.iter().find_map(|op| match op {
        HirOp::Match(s, arms) => Some((*s, arms.clone())),
        _ => None,
    });
    let (_scrut_slot, arms) = m.expect("Match op");
    assert_eq!(arms.len(), 2);
    // First arm matches discriminant 0 (Empty).
    assert_eq!(arms[0].1, vec![0]);
    // Wildcard arm matches all others in [0, 16), except the already-explicit 0.
    let wildcard: std::collections::HashSet<u8> = arms[1].1.iter().copied().collect();
    assert!(!wildcard.contains(&0));
    assert!(wildcard.contains(&1));
    assert!(wildcard.contains(&2));
}

#[test]
fn if_else_chain_nests_if0() {
    let hir = compile(
        r#"
        struct Bool { U4 value, }
        extension Bool {
            fn pick(self): U4 {
                if self { 1 } else if self { 2 } else { 3 }
            }
        }
        "#,
        "Bool",
        "pick",
    );
    let (_, _, when_nonzero) = find_if0(&hir.body.ops).expect("outer If0");
    // when_nonzero is the "then" branch = Set(_ret, 1). The else branch has
    // the nested if (so the outer "when_zero" block contains another If0).
    // Actually per my encoding: when_zero == else-branch, when_nonzero == then-branch.
    let (_, inner_zero, inner_nonzero) = find_if0(
        &find_if0(&hir.body.ops).unwrap().1.ops,
    )
    .expect("nested If0 inside outer else branch");
    let _ = (inner_zero, inner_nonzero, when_nonzero);
}
