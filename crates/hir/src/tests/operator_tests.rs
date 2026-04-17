//! End-to-end tests: operators dispatch via stdlib trait impls, the whole
//! pipeline (parse → typer → HIR gen → inline → MIR) runs, and the MIR
//! interpreter returns the right value.
//!
//! These tests load the real stdlib U4 impls from
//! `examples/new_syntax/std/{Ops,U4,Bool}.ct` — so if the stdlib is wrong
//! or the compiler drops a trait somewhere in the chain, a test here
//! breaks.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::{
    gen_function_with_natives, hir_to_mir, inline_program, BuiltinNatives, HirFunction,
};

/// Load a stdlib file from the examples dir.
fn load_stdlib(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/new_syntax")
        .join(path);
    std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("read {}: {}", full.display(), e))
        .replace('\r', "")
}

/// Build a program combining the stdlib core (System, Ops, Bool, U4) with
/// extra user code. Returns a name→HirFunction map that the inliner can
/// consume.
fn compile_program(extra: &str) -> HashMap<typer::FnSig, HirFunction> {
    let system_src = load_stdlib("std/System.ct");
    let bool_src = load_stdlib("std/Bool.ct");
    let ops_src = load_stdlib("std/Ops.ct");
    let u4_src = load_stdlib("std/U4.ct");

    let parsed = [
        ("std/System.ct", new_parser::parse(&system_src)),
        ("std/Ops.ct", new_parser::parse(&ops_src)),
        ("std/Bool.ct", new_parser::parse(&bool_src)),
        ("std/U4.ct", new_parser::parse(&u4_src)),
        ("user.ct", new_parser::parse(extra)),
    ];
    for (name, r) in &parsed {
        if let Err(e) = r {
            panic!("parse `{}` failed: {:?}", name, e);
        }
    }
    let files: Vec<(String, Vec<_>)> = parsed
        .into_iter()
        .map(|(name, r)| (name.to_string(), r.unwrap()))
        .collect();
    let as_refs: Vec<(&str, &[_])> = files
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_slice()))
        .collect();

    let reg = typer::TypeRegistry::from_files(&as_refs)
        .unwrap_or_else(|e| panic!("typer: {:?}", e));
    let db = typer::FunctionDB::from_registry(&reg)
        .unwrap_or_else(|e| panic!("fn_db: {:?}", e));

    let natives = BuiltinNatives::new();
    let mut out = HashMap::new();
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            let hir = gen_function_with_natives(k, s, &reg, &db, Some(&natives))
                .unwrap_or_else(|e| panic!("hir {:?}: {}", k, e));
            out.insert(k.clone(), hir);
        }
    }
    out
}

/// Run a program: inline from `entry` into a single function, convert to
/// MIR, execute in the MIR interpreter, and return the output cell values
/// (from the caller's output slot range).
fn run_program(extra: &str, entry: &typer::FnSig, args: &[u8]) -> Vec<u8> {
    let fns = compile_program(extra);
    let inlined = inline_program(&fns, entry)
        .unwrap_or_else(|e| panic!("inline: {}", e));
    let mir_block = hir_to_mir(&inlined.body)
        .unwrap_or_else(|e| panic!("mir conv: {}", e));

    struct Null;
    impl mir::RunContext for Null {
        fn input(&mut self) -> u8 { 0 }
        fn print(&mut self, _: char) {}
    }
    let mut state = mir::MemoryState::new(
        (inlined.slot_count as usize + 16).max(64),
        4,
    );
    // Seed input slots with the given args.
    for (i, v) in args.iter().enumerate() {
        state.set_mem(i as u32, *v);
    }
    let mut ctx = Null;
    state.execute_block(&mir_block, &mut ctx);

    // Output slots are right after input slots.
    let input_count = inlined.sig.input_count as usize;
    let output_count = inlined.sig.output_count as usize;
    (0..output_count)
        .map(|i| state.get_mem((input_count + i) as u32))
        .collect()
}

// ---------- Add / Sub -------------------------------------------------------

#[test]
fn add_operator_dispatches_via_trait_and_computes_correctly() {
    // `fn test(U4 a, U4 b): U4 { a + b }` — 3 + 4 = 7.
    let src = "extension U4 { fn test(U4 a, U4 b): U4 { a + b } }";
    let entry = typer::FnSig::new("U4", "test");
    let out = run_program(src, &entry, &[3, 4]);
    assert_eq!(out, vec![7]);
}

#[test]
fn add_operator_saturates_to_u4_modulo() {
    // U4 wraps at 16 (cell boundary). 12 + 7 = 19 → 19 mod 16 = 3.
    let src = "extension U4 { fn test(U4 a, U4 b): U4 { a + b } }";
    let entry = typer::FnSig::new("U4", "test");
    let out = run_program(src, &entry, &[12, 7]);
    assert_eq!(out, vec![3]);
}

#[test]
fn sub_operator_dispatches_via_trait() {
    let src = "extension U4 { fn test(U4 a, U4 b): U4 { a - b } }";
    let entry = typer::FnSig::new("U4", "test");
    let out = run_program(src, &entry, &[9, 3]);
    assert_eq!(out, vec![6]);
}

// ---------- AddAssign / SubAssign ------------------------------------------

#[test]
fn add_assign_mutates_in_place() {
    // `fn test(mut U4 a, U4 b): U4 { a += b; a }`
    let src = "extension U4 { fn test(mut U4 a, U4 b): U4 { a += b; a } }";
    let entry = typer::FnSig::new("U4", "test");
    let out = run_program(src, &entry, &[2, 5]);
    assert_eq!(out, vec![7]);
}

#[test]
fn sub_assign_mutates_in_place() {
    let src = "extension U4 { fn test(mut U4 a, U4 b): U4 { a -= b; a } }";
    let entry = typer::FnSig::new("U4", "test");
    let out = run_program(src, &entry, &[10, 4]);
    assert_eq!(out, vec![6]);
}

// ---------- Eq / Ne ---------------------------------------------------------

#[test]
fn eq_operator_returns_1_for_equal() {
    let src = "extension U4 { fn test(U4 a, U4 b): Bool { a == b } }";
    let entry = typer::FnSig::new("U4", "test");
    let out = run_program(src, &entry, &[5, 5]);
    assert_eq!(out, vec![1]);
}

#[test]
fn eq_operator_returns_0_for_not_equal() {
    let src = "extension U4 { fn test(U4 a, U4 b): Bool { a == b } }";
    let entry = typer::FnSig::new("U4", "test");
    let out = run_program(src, &entry, &[5, 6]);
    assert_eq!(out, vec![0]);
}

#[test]
fn ne_operator_inverts_eq() {
    let src = "extension U4 { fn test(U4 a, U4 b): Bool { a != b } }";
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[5, 5]), vec![0]);
    assert_eq!(run_program(src, &entry, &[5, 6]), vec![1]);
}

// ---------- Ord -------------------------------------------------------------

#[test]
fn gt_operator_dispatches_via_trait() {
    let src = "extension U4 { fn test(U4 a, U4 b): Bool { a > b } }";
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[7, 3]), vec![1]);
    assert_eq!(run_program(src, &entry, &[3, 7]), vec![0]);
    assert_eq!(run_program(src, &entry, &[5, 5]), vec![0]);
}

#[test]
fn lt_operator_dispatches_via_trait() {
    let src = "extension U4 { fn test(U4 a, U4 b): Bool { a < b } }";
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[3, 7]), vec![1]);
    assert_eq!(run_program(src, &entry, &[7, 3]), vec![0]);
    assert_eq!(run_program(src, &entry, &[5, 5]), vec![0]);
}

#[test]
fn ge_operator_dispatches_via_trait() {
    let src = "extension U4 { fn test(U4 a, U4 b): Bool { a >= b } }";
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[7, 3]), vec![1]);
    assert_eq!(run_program(src, &entry, &[3, 7]), vec![0]);
    assert_eq!(run_program(src, &entry, &[5, 5]), vec![1]);
}

#[test]
fn le_operator_dispatches_via_trait() {
    let src = "extension U4 { fn test(U4 a, U4 b): Bool { a <= b } }";
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[3, 7]), vec![1]);
    assert_eq!(run_program(src, &entry, &[7, 3]), vec![0]);
    assert_eq!(run_program(src, &entry, &[5, 5]), vec![1]);
}

// ---------- Cross-trait interaction ---------------------------------------

#[test]
fn operators_compose_and_short_circuit() {
    // `a > b && a != 0`
    let src = "extension U4 { fn test(U4 a, U4 b): Bool { a > b && a != 0 } }";
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[5, 3]), vec![1]);
    assert_eq!(run_program(src, &entry, &[0, 1]), vec![0], "0 > 1 false");
    assert_eq!(run_program(src, &entry, &[3, 5]), vec![0], "3 > 5 false");
}

#[test]
fn operators_chain_with_arithmetic_and_comparison() {
    // `(a + b) > 5`
    let src = "extension U4 { fn test(U4 a, U4 b): Bool { (a + b) > 5 } }";
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[3, 3]), vec![1], "6 > 5");
    assert_eq!(run_program(src, &entry, &[2, 2]), vec![0], "4 > 5 false");
}

#[test]
fn operator_dispatch_produces_no_remaining_calls() {
    // Sanity: after the full pipeline, the MIR has no artifacts of
    // unresolved calls. This guards against the impl/trait linkage
    // silently breaking.
    let src = "extension U4 { fn test(U4 a, U4 b): U4 { a + b } }";
    let fns = compile_program(src);
    let entry = typer::FnSig::new("U4", "test");
    let inlined = inline_program(&fns, &entry).expect("inline");
    let check = |ops: &[crate::HirOp]| -> bool {
        ops.iter().any(|op| matches!(op, crate::HirOp::Call { .. }))
    };
    fn contains_call(b: &crate::HirBlock, check: &dyn Fn(&[crate::HirOp]) -> bool) -> bool {
        if check(&b.ops) {
            return true;
        }
        for op in &b.ops {
            match op {
                crate::HirOp::If0(_, a, b) => {
                    if contains_call(a, check) || contains_call(b, check) {
                        return true;
                    }
                }
                crate::HirOp::Loop(x) | crate::HirOp::Block(x) => {
                    if contains_call(x, check) {
                        return true;
                    }
                }
                crate::HirOp::Match(_, arms) => {
                    for (x, _) in arms {
                        if contains_call(x, check) {
                            return true;
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }
    assert!(!contains_call(&inlined.body, &check));
}
