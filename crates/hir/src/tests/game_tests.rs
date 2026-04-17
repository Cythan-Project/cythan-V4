//! Attempt to compile example games end-to-end. Each test loads the
//! stdlib + the game source, runs the full pipeline, and asserts on
//! whatever partial progress we can achieve today. Failing tests identify
//! the next gap to close.

use std::path::PathBuf;

fn load(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/new_syntax")
        .join(path);
    std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("read {}: {}", full.display(), e))
        .replace('\r', "")
}

/// Parse stdlib core + game file and try to build the TypeRegistry. Returns
/// either the built registry or the list of errors.
fn try_build_morpion_registry() -> Result<typer::TypeRegistry, Vec<typer::TyperError>> {
    let parts = [
        ("std/System.ct", load("std/System.ct")),
        ("std/Ops.ct", load("std/Ops.ct")),
        ("std/Bool.ct", load("std/Bool.ct")),
        ("std/U4.ct", load("std/U4.ct")),
        ("std/U8.ct", load("std/U8.ct")),
        ("std/Array.ct", load("std/Array.ct")),
        ("Morpion.ct", load("Morpion.ct")),
    ];
    let parsed: Vec<_> = parts
        .iter()
        .map(|(name, src)| {
            (
                name.to_string(),
                new_parser::parse(src)
                    .unwrap_or_else(|e| panic!("parse `{}` failed: {:?}", name, e)),
            )
        })
        .collect();
    let as_refs: Vec<(&str, &[_])> = parsed
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_slice()))
        .collect();
    typer::TypeRegistry::from_files(&as_refs)
}

#[test]
fn morpion_parses() {
    let src = load("Morpion.ct");
    new_parser::parse(&src).expect("parse Morpion");
}

#[test]
fn morpion_typer_diagnostic() {
    match try_build_morpion_registry() {
        Ok(reg) => {
            // Morpion's struct field `Array<Cell, 9, U4> grid` should now
            // have size 9 (cell-size) and be laid out starting at offset 0.
            let morpion = reg.types.get("Morpion").expect("Morpion type");
            match &morpion.kind {
                typer::TypeKind::Struct(typer::StructKind::Concrete(layout)) => {
                    assert_eq!(layout.size, 9, "Array<Cell, 9, U4> ≡ 9 cells");
                    assert_eq!(layout.fields.len(), 1);
                    assert_eq!(layout.fields[0].name, "grid");
                    assert_eq!(layout.fields[0].offset, 0);
                    assert_eq!(layout.fields[0].size, 9);
                }
                other => panic!("Morpion should be concrete struct, got {:?}", other),
            }
        }
        Err(errors) => {
            let first = errors.first().expect("at least one error");
            panic!("typer: {}", first.message);
        }
    }
}

#[test]
fn morpion_fn_db_diagnostic() {
    // Next stage: build the FunctionDB. Fails at the first HIR-relevant
    // typing problem; reports which function couldn't flatten.
    let reg = try_build_morpion_registry()
        .unwrap_or_else(|e| panic!("typer: {:?}", e));
    match typer::FunctionDB::from_registry(&reg) {
        Ok(db) => {
            // `Morpion::main` is the game's entry point.
            let key = typer::FnSig::new("Morpion", "main");
            let f = db.get(&key).expect("Morpion::main in DB");
            assert!(
                matches!(f, typer::Fn::Simple(_)),
                "Morpion::main should be Simple (Morpion is non-generic)"
            );
        }
        Err(errors) => {
            let first = errors.first().expect("at least one error");
            panic!("fn_db: {}", first.message);
        }
    }
}

/// Compile the whole program's Simple functions to HIR and return the map.
fn compile_morpion_hir() -> std::collections::HashMap<typer::FnSig, crate::HirFunction> {
    let reg = try_build_morpion_registry()
        .unwrap_or_else(|e| panic!("typer: {:?}", e));
    let db = typer::FunctionDB::from_registry(&reg)
        .unwrap_or_else(|e| panic!("fn_db: {:?}", e));
    let natives = crate::BuiltinNatives::new();
    let mut out = std::collections::HashMap::new();
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            match crate::gen_function_with_natives(k, s, &reg, &db, Some(&natives)) {
                Ok(hir) => {
                    out.insert(k.clone(), hir);
                }
                Err(e) => panic!("hir {:?}: {}", k, e),
            }
        }
    }
    out
}

#[test]
#[ignore = "pending: monomorphization of Array<Cell, 9, U4> and receiver-type-args through HIR gen (tasks #45, #46)"]
fn morpion_inline_end_to_end() {
    // Last stage: inline from `Morpion::main` and produce a flat HIR. This
    // currently fails because `Array::new()`, `self.grid.getDyn(pos)`, etc.
    // resolve to `Fn::Templated` entries (Array<T, E, F>) that the inliner
    // can't dispatch without monomorphization. The Phase-7 `BuiltinNatives`
    // *do* know how to emit HIR for Array methods, but the HIR generator
    // doesn't yet thread the concrete receiver type args ([Cell, 9, U4])
    // into `NativeCall.receiver_type_args` for method calls, so the
    // natives can't fire either.
    //
    // Unignore this test after:
    //   1. LocalBinding / infer_expr_type carry Vec<ConcreteTemplateArg>
    //      for generic-typed values.
    //   2. gen_method_call populates receiver_type_args from them.
    //   3. The inliner either resolves templated calls via natives or
    //      falls back to the monomorphize() path in Phase 6.2.
    let fns = compile_morpion_hir();
    let entry = typer::FnSig::new("Morpion", "main");
    let _ = crate::inline_program(&fns, &entry).expect("inline");
}

#[test]
fn morpion_hir_gen_diagnostic() {
    // Attempt HIR gen on every Simple function. First failing function
    // names the gap to close next.
    let reg = try_build_morpion_registry()
        .unwrap_or_else(|e| panic!("typer: {:?}", e));
    let db = typer::FunctionDB::from_registry(&reg)
        .unwrap_or_else(|e| panic!("fn_db: {:?}", e));
    let natives = crate::BuiltinNatives::new();

    let mut failures: Vec<(String, String)> = Vec::new();
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            if let Err(e) = crate::gen_function_with_natives(k, s, &reg, &db, Some(&natives)) {
                failures.push((format!("{}::{}", k.type_name, k.method_name), e.to_string()));
            }
        }
    }

    if !failures.is_empty() {
        let mut msg = String::from("HIR gen failures:\n");
        for (name, e) in &failures {
            msg.push_str(&format!("  {}: {}\n", name, e));
        }
        panic!("{}", msg);
    }
}
