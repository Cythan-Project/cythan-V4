//! Step 4.5: struct and enum construction.

use crate::ir::*;
use crate::tests::compile;

#[test]
fn struct_literal_copies_fields_at_correct_offsets() {
    // fn make(U4 a, U4 b): U8 { U8 { lower: a, higher: b } }
    // slots: s0=a, s1=b, s2=_ret.lower, s3=_ret.higher
    let hir = compile(
        r#"
        struct U8 { U4 lower, U4 higher, }
        extension U8 {
            fn make(U4 a, U4 b): Self {
                Self { lower: a, higher: b, }
            }
        }
        "#,
        "U8",
        "make",
    );
    let copies: Vec<_> = hir
        .body
        .ops
        .iter()
        .filter_map(|op| match op {
            HirOp::Copy(a, b) => Some((*a, *b)),
            _ => None,
        })
        .collect();
    // Expect Copy(s2, s0) [lower from a] and Copy(s3, s1) [higher from b].
    assert!(copies.contains(&(SlotId(2), SlotId(0))));
    assert!(copies.contains(&(SlotId(3), SlotId(1))));
}

#[test]
fn struct_literal_with_literal_values_uses_set() {
    let hir = compile(
        r#"
        struct U8 { U4 lower, U4 higher, }
        extension U8 {
            fn new(): Self { Self { lower: 0, higher: 0, } }
        }
        "#,
        "U8",
        "new",
    );
    let sets: Vec<_> = hir
        .body
        .ops
        .iter()
        .filter_map(|op| match op {
            HirOp::Set(s, v) => Some((*s, *v)),
            _ => None,
        })
        .collect();
    // _ret starts at s0; lower=s0, higher=s1.
    assert!(sets.contains(&(SlotId(0), 0)));
    assert!(sets.contains(&(SlotId(1), 0)));
}

#[test]
fn enum_unit_variant_sets_discriminant_only() {
    // Cell::Empty on return slot: Set(s0, 0).
    let hir = compile(
        r#"
        enum Cell { Empty, O, X, }
        extension Cell { fn empty(): Self { Self::Empty } }
        "#,
        "Cell",
        "empty",
    );
    let sets: Vec<_> = hir
        .body
        .ops
        .iter()
        .filter_map(|op| match op {
            HirOp::Set(s, v) => Some((*s, *v)),
            _ => None,
        })
        .collect();
    assert!(sets.contains(&(SlotId(0), 0)));
}

#[test]
fn enum_variant_o_sets_discr_1() {
    let hir = compile(
        r#"
        enum Cell { Empty, O, X, }
        extension Cell { fn o(): Self { Self::O } }
        "#,
        "Cell",
        "o",
    );
    let sets: Vec<_> = hir
        .body
        .ops
        .iter()
        .filter_map(|op| match op {
            HirOp::Set(s, v) => Some((*s, *v)),
            _ => None,
        })
        .collect();
    assert!(sets.contains(&(SlotId(0), 1)));
}

#[test]
fn enum_variant_with_data_sets_discr_and_data() {
    // enum E { A(U4), B }; fn wrap(U4 x): E { E::A(x) }
    //   discr=1 cell, data=1 cell (max of variant data sizes).
    //   return slot starts at s1 (after x=s0). _ret.discr=s1, _ret.data=s2.
    //   Set(s1, 0) [A=0], then Copy(s2, s0) [data from x].
    let hir = compile(
        r#"
        enum E { A(U4), B, }
        extension E {
            fn wrap(U4 x): Self { Self::A(x) }
        }
        "#,
        "E",
        "wrap",
    );
    let ops = &hir.body.ops;
    let saw_discr_set = ops.iter().any(|op| matches!(op, HirOp::Set(SlotId(1), 0)));
    let saw_data_copy = ops
        .iter()
        .any(|op| matches!(op, HirOp::Copy(a, b) if *a == SlotId(2) && *b == SlotId(0)));
    assert!(saw_discr_set, "missing discriminant Set");
    assert!(saw_data_copy, "missing data Copy");
}
