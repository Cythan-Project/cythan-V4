//! Phase 7: native provider tests.
//!
//! Most tests drive the HIR generator with `BuiltinNatives` and assert that
//! the expected low-level ops (WriteRegister/ReadRegister/Inc/Match/...) are
//! emitted in place of Call ops.

use either::Either;

use crate::gen::gen_function_with_natives;
use crate::ir::*;
use crate::natives::{BuiltinNatives, NativeCall, NativeEmitter, NativeProvider};

// Helper: compile a method with the builtin native provider active.
fn compile_with_natives(
    src: &str,
    type_name: &str,
    method: &str,
) -> HirFunction {
    let items = new_parser::parse(src).expect("parse");
    let reg = typer::TypeRegistry::from_items(&items).expect("typer");
    let db = typer::FunctionDB::from_registry(&reg).expect("fn_db");
    let key = typer::FnSig::new(type_name, method);
    let typer::Fn::Simple(s) = db.get(&key).expect("fn") else {
        panic!("not simple");
    };
    gen_function_with_natives(&key, s, &reg, &db, Some(&BuiltinNatives::new())).expect("hir")
}

fn ops_list(hir: &HirFunction) -> &[HirOp] {
    &hir.body.ops
}

// ---------- Step 7.1: trait + plumbing -------------------------------------

#[test]
fn builtin_has_method_catalog() {
    let p = BuiltinNatives::new();
    // System register ops + Array layout primitives are the full native surface.
    // Operators (eq/add/sub/...) are stdlib, NOT natives — assert that.
    assert!(p.has_method("System", "setRegister"));
    assert!(p.has_method("System", "getRegister"));
    assert!(p.has_method("System", "debug"));
    // Array methods are NOT natives — they're synthesized by the
    // monomorphizer (`array_synth`). Assert that.
    assert!(!p.has_method("Array", "set"));
    assert!(!p.has_method("Array", "get"));
    assert!(!p.has_method("Array", "len"));
    assert!(!p.has_method("Array", "new"));
    assert!(!p.has_method("U4", "inc"), "U4::inc is stdlib, not native");
    assert!(!p.has_method("U4", "dec"), "U4::dec is stdlib, not native");
    assert!(!p.has_method("U4", "add"), "U4::add is stdlib, not native");
    assert!(!p.has_method("U4", "eq"), "U4::eq is stdlib, not native");
    assert!(!p.has_method("U4", "not_a_native"));
    assert!(!p.has_method("Foo", "anything"));
}

#[test]
fn provider_emits_directly_via_api_system() {
    // Direct invocation of BuiltinNatives: System::setRegister<0>(value).
    let reg = typer::TypeRegistry::new();
    let p = BuiltinNatives::new();

    let mut ops: Vec<HirOp> = Vec::new();
    let mut next_slot: u32 = 10;
    {
        let mut em = NativeEmitter {
            ops: &mut ops,
            registry: &reg,
            next_slot: &mut next_slot,
        };
        let arg_slots = [SlotId(3)];
        let ret_slots: [SlotId; 0] = [];
        let call = NativeCall {
            type_name: "System",
            method: "setRegister",
            template_args: &[ConcreteTemplateArg::Value(0)],
            arg_slots: &arg_slots,
            ret_slots: &ret_slots,
            receiver_type_args: &[],
            receiver_cell_count: 0,
            registry: &reg,
        };
        p.generate(call, &mut em).expect("native setRegister");
    }
    assert_eq!(
        ops,
        vec![HirOp::WriteRegister(0, either::Either::Right(SlotId(3)))]
    );
}

// ---------- Step 7.3: System::setRegister / getRegister -------------------

#[test]
fn system_set_register_emits_write_register() {
    let src = r#"
        struct System {}
        extension System {
            fn setRegister<N>(U4 value) {}
            fn poke(U4 v) { System::setRegister<0>(v); }
        }
    "#;
    let hir = compile_with_natives(src, "System", "poke");
    let found = hir.body.ops.iter().any(|op| matches!(
        op,
        HirOp::WriteRegister(0, Either::Right(_))
    ));
    assert!(found, "expected WriteRegister(0, Either::Right(..)): {:#?}", hir.body.ops);
    assert!(!hir.body.ops.iter().any(|op| matches!(op, HirOp::Call { .. })));
}

#[test]
fn system_set_register_with_different_index() {
    let src = r#"
        struct System {}
        extension System {
            fn setRegister<N>(U4 value) {}
            fn poke3(U4 v) { System::setRegister<3>(v); }
        }
    "#;
    let hir = compile_with_natives(src, "System", "poke3");
    assert!(hir.body.ops.iter().any(|op| matches!(
        op,
        HirOp::WriteRegister(3, Either::Right(_))
    )));
}

#[test]
fn system_get_register_emits_read_register() {
    let src = r#"
        struct System {}
        extension System {
            fn getRegister<N>(): U4 { 0 }
            fn fetch(): U4 { System::getRegister<2>() }
        }
    "#;
    let hir = compile_with_natives(src, "System", "fetch");
    let found = hir.body.ops.iter().any(|op| matches!(op, HirOp::ReadRegister(_, 2)));
    assert!(found, "expected ReadRegister(.., 2): {:#?}", hir.body.ops);
}

#[test]
fn system_set_register_rejects_bad_register() {
    // Passing an out-of-range register index like 7 should produce an HIR error.
    let src = r#"
        struct System {}
        extension System {
            fn setRegister<N>(U4 value) {}
            fn bad(U4 v) { System::setRegister<7>(v); }
        }
    "#;
    let items = new_parser::parse(src).unwrap();
    let reg = typer::TypeRegistry::from_items(&items).unwrap();
    let db = typer::FunctionDB::from_registry(&reg).unwrap();
    let key = typer::FnSig::new("System", "bad");
    let typer::Fn::Simple(s) = db.get(&key).unwrap() else {
        panic!();
    };
    let err = gen_function_with_natives(&key, s, &reg, &db, Some(&BuiltinNatives::new()));
    assert!(err.is_err(), "expected error for out-of-range register");
    assert!(err.unwrap_err().to_string().contains("out of range"));
}

// ---------- Step 7.3: debug/debugType no-ops -------------------------------

#[test]
fn system_debug_emits_no_ops() {
    let src = r#"
        struct System {}
        extension System {
            fn debug<T>(T a) {}
            fn caller() { System::debug<U4>(5); }
        }
    "#;
    let hir = compile_with_natives(src, "System", "caller");
    // After native resolution, no Call op remains. The arg eval may still
    // allocate slots but the only "action" is the Set for the literal 5.
    assert!(!hir.body.ops.iter().any(|op| matches!(op, HirOp::Call { .. })));
}

