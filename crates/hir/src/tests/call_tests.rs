//! Step 4.4: method calls compile to Call ops.

use crate::ir::*;
use crate::tests::compile;

#[test]
fn zero_arg_method_call_emits_call_op() {
    // fn caller(): U4 { U4::zero() }
    let hir = compile(
        r#"
        extension U4 {
            fn zero(): Self { 0 }
            fn caller(): U4 { U4::zero() }
        }
        "#,
        "U4",
        "caller",
    );
    let call = hir
        .body
        .ops
        .iter()
        .find_map(|op| match op {
            HirOp::Call { target, args, ret } => Some((target.clone(), args.clone(), ret.clone())),
            _ => None,
        })
        .expect("Call op");
    assert_eq!(call.0.type_name, "U4");
    assert_eq!(call.0.method_name, "zero");
    assert!(call.1.is_empty(), "no args");
    assert_eq!(call.2, vec![SlotId(0)], "ret writes to _ret slot");
}

#[test]
fn method_call_with_self_receiver_flattens_args() {
    // fn run(self): U4 { self.copy() }
    let hir = compile(
        r#"
        extension U4 {
            fn copy(self): Self { self }
            fn run(self): U4 { self.copy() }
        }
        "#,
        "U4",
        "run",
    );
    // slots: s0=self, s1=_ret. Receiver gets evaluated into a temp slot first.
    let call = hir.body.ops.iter().find_map(|op| match op {
        HirOp::Call { target, args, ret } => Some((target.clone(), args.clone(), ret.clone())),
        _ => None,
    }).expect("Call");
    assert_eq!(call.0.method_name, "copy");
    // At least one arg slot (the receiver).
    assert_eq!(call.1.len(), 1);
    assert_eq!(call.2.len(), 1, "returns 1 cell");
}

#[test]
fn method_call_with_argument() {
    // fn equals(self, Self other): Bool { ... }; fn check(self): Bool { self.equals(self) }
    let hir = compile(
        r#"
        struct Bool { U4 value, }
        extension U4 {
            fn equals(self, Self other): Bool { true }
            fn check(self): Bool { self.equals(self) }
        }
        "#,
        "U4",
        "check",
    );
    let call = hir.body.ops.iter().find_map(|op| match op {
        HirOp::Call { target, args, ret } => Some((target.clone(), args.clone(), ret.clone())),
        _ => None,
    }).expect("Call");
    assert_eq!(call.0.method_name, "equals");
    // recv=1 cell + arg=1 cell = 2 total args.
    assert_eq!(call.1.len(), 2);
    assert_eq!(call.2.len(), 1);
}

#[test]
fn static_call_emits_typename_in_fnref() {
    let hir = compile(
        r#"
        extension U4 {
            fn input(): Self { 0 }
            fn run(): U4 { U4::input() }
        }
        "#,
        "U4",
        "run",
    );
    let call = hir.body.ops.iter().find_map(|op| match op {
        HirOp::Call { target, .. } => Some(target.clone()),
        _ => None,
    }).expect("Call");
    assert_eq!(call.type_name, "U4");
    assert_eq!(call.method_name, "input");
}

#[test]
fn template_args_are_lowered_to_concrete_args() {
    // self.get<0>() — template arg is a numeric value.
    let hir = compile(
        r#"
        struct Bool { U4 value, }
        extension U4 {
            fn get<N>(self): Self { self }
            fn caller(self): Self { self.get<3>() }
        }
        "#,
        "U4",
        "caller",
    );
    let call = hir.body.ops.iter().find_map(|op| match op {
        HirOp::Call { target, .. } => Some(target.clone()),
        _ => None,
    }).expect("Call");
    assert_eq!(call.method_name, "get");
    assert_eq!(call.template_args.len(), 1);
    assert_eq!(call.template_args[0], ConcreteTemplateArg::Value(3));
}
