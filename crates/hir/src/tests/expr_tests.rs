//! Step 4.2: literals, variables, field access, declarations.

use crate::ir::*;
use crate::tests::compile;

fn find_set(ops: &[HirOp]) -> Vec<(SlotId, u8)> {
    ops.iter()
        .filter_map(|op| match op {
            HirOp::Set(s, v) => Some((*s, *v)),
            _ => None,
        })
        .collect()
}

fn find_copy(ops: &[HirOp]) -> Vec<(SlotId, SlotId)> {
    ops.iter()
        .filter_map(|op| match op {
            HirOp::Copy(a, b) => Some((*a, *b)),
            _ => None,
        })
        .collect()
}

#[test]
fn literal_number_sets_return_slot() {
    // fn zero(): Self { 0 }   on U4
    // input_count=0, output_count=1, so return slot is s0.
    let hir = compile("extension U4 { fn zero(): Self { 0 } }", "U4", "zero");
    assert_eq!(hir.sig.input_count, 0);
    assert_eq!(hir.sig.output_count, 1);
    let sets = find_set(&hir.body.ops);
    assert_eq!(sets, vec![(SlotId(0), 0)]);
}

#[test]
fn variable_read_copies_to_return_slot() {
    // fn identity(self): Self { self }   on U4
    // s0 = self (input), s1 = _ret (output), body copies s1 from s0.
    let hir = compile(
        "extension U4 { fn identity(self): Self { self } }",
        "U4",
        "identity",
    );
    let copies = find_copy(&hir.body.ops);
    assert_eq!(copies, vec![(SlotId(1), SlotId(0))]);
}

#[test]
fn field_read_uses_computed_offset() {
    // fn get_higher(self): U4 { self.higher }  on U8 (lower=0, higher=1)
    // s0 = self.lower, s1 = self.higher, s2 = _ret
    // body: Copy(s2, s1)
    let hir = compile(
        r#"
        struct U8 { U4 lower, U4 higher, }
        extension U8 { fn get_higher(self): U4 { self.higher } }
        "#,
        "U8",
        "get_higher",
    );
    let copies = find_copy(&hir.body.ops);
    assert_eq!(copies, vec![(SlotId(2), SlotId(1))]);
}

#[test]
fn field_read_lower_is_offset_zero() {
    let hir = compile(
        r#"
        struct U8 { U4 lower, U4 higher, }
        extension U8 { fn get_lower(self): U4 { self.lower } }
        "#,
        "U8",
        "get_lower",
    );
    let copies = find_copy(&hir.body.ops);
    assert_eq!(copies, vec![(SlotId(2), SlotId(0))]);
}

#[test]
fn declaration_allocates_slot_and_sets_it() {
    // fn foo(): U4 { U4 x = 5; x }
    // Return slot = s0. A new local `x` allocates s1. Body:
    //   Set(s1, 5);  Copy(s0, s1)
    let hir = compile(
        "extension U4 { fn foo(): U4 { U4 x = 5; x } }",
        "U4",
        "foo",
    );
    // Expect a Set(s1, 5) and Copy(s0, s1) in order.
    let mut seen_set = false;
    let mut seen_copy = false;
    for op in &hir.body.ops {
        match op {
            HirOp::Set(_, 5) => seen_set = true,
            HirOp::Copy(a, b) if *a == SlotId(0) => {
                assert!(seen_set, "Copy before Set");
                assert_eq!(*b, SlotId(1));
                seen_copy = true;
            }
            _ => {}
        }
    }
    assert!(seen_set && seen_copy, "ops: {:#?}", hir.body.ops);
}

#[test]
fn bool_literal_true_sets_one() {
    // Bool return, `fn t(): Self { true }` desugars to Set(_ret, 1)
    let hir = compile(
        r#"
        struct Bool { U4 value, }
        extension Bool { fn t(): Self { true } }
        "#,
        "Bool",
        "t",
    );
    let sets = find_set(&hir.body.ops);
    assert!(sets.contains(&(SlotId(0), 1)));
}

#[test]
fn bool_literal_false_sets_zero() {
    let hir = compile(
        r#"
        struct Bool { U4 value, }
        extension Bool { fn f(): Self { false } }
        "#,
        "Bool",
        "f",
    );
    let sets = find_set(&hir.body.ops);
    assert!(sets.contains(&(SlotId(0), 0)));
}

#[test]
fn slot_count_tracks_allocations() {
    // 2 input slots (U8 self) + 1 output (U4) + declarations
    let hir = compile(
        r#"
        struct U8 { U4 lower, U4 higher, }
        extension U8 {
            fn thing(self): U4 {
                U4 a = 1;
                U4 b = 2;
                a
            }
        }
        "#,
        "U8",
        "thing",
    );
    // self(2) + _ret(1) + a(1) + b(1) = 5 slots.
    assert_eq!(hir.slot_count, 5);
}
