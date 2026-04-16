//! Step 4.1: HIR data structures.

use crate::ir::*;

#[test]
fn slot_id_displays_as_s_n() {
    assert_eq!(SlotId(0).to_string(), "s0");
    assert_eq!(SlotId(42).to_string(), "s42");
}

#[test]
fn hir_block_new_is_empty() {
    let b = HirBlock::new();
    assert!(b.ops.is_empty());
    assert!(b.result_slot.is_none());
}

#[test]
fn hir_block_with_result_sets_result_slot() {
    let b = HirBlock::with_result(SlotId(5));
    assert_eq!(b.result_slot, Some(SlotId(5)));
    assert!(b.ops.is_empty());
}

#[test]
fn hir_ops_clone_and_compare() {
    let op = HirOp::Set(SlotId(0), 3);
    let cloned = op.clone();
    assert_eq!(op, cloned);

    let call = HirOp::Call {
        target: FnRef {
            type_name: "U4".into(),
            method_name: "zero".into(),
            template_args: vec![],
        },
        args: vec![SlotId(0)],
        ret: vec![SlotId(1)],
    };
    assert_eq!(call, call.clone());
}

#[test]
fn fnref_with_template_args() {
    let r = FnRef {
        type_name: "Array".into(),
        method_name: "get".into(),
        template_args: vec![
            ConcreteTemplateArg::Type(ConcreteType {
                name: "U4".into(),
                args: vec![],
            }),
            ConcreteTemplateArg::Value(9),
        ],
    };
    assert_eq!(r.template_args.len(), 2);
}
