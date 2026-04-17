//! End-to-end tests for the `Output`-parametrized operator traits
//! (`trait Add<Output>`, `trait Sub<Output>`) and the
//! `<Self as Trait>::Output` qualified-path syntax.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::{
    gen_function_with_natives, hir_to_mir, inline_program_full, BuiltinNatives, HirFunction,
};

fn load(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/new_syntax")
        .join(path);
    std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("read {}: {}", full.display(), e))
        .replace('\r', "")
}

fn compile_program(
    extra: &str,
) -> (
    typer::TypeRegistry,
    typer::FunctionDB,
    HashMap<typer::FnSig, HirFunction>,
) {
    let parts = [
        ("std/System.ct", load("std/System.ct")),
        ("std/Ops.ct", load("std/Ops.ct")),
        ("std/Bool.ct", load("std/Bool.ct")),
        ("std/U4.ct", load("std/U4.ct")),
        ("std/U8.ct", load("std/U8.ct")),
        ("std/Array.ct", load("std/Array.ct")),
        ("user.ct", extra.to_string()),
    ];
    let parsed: Vec<_> = parts
        .iter()
        .map(|(name, src)| {
            (
                name.to_string(),
                new_parser::parse(src).unwrap_or_else(|e| panic!("parse `{}`: {:?}", name, e)),
            )
        })
        .collect();
    let as_refs: Vec<(&str, &[_])> = parsed
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_slice()))
        .collect();
    let reg = typer::TypeRegistry::from_files(&as_refs).expect("typer");
    let db = typer::FunctionDB::from_registry(&reg).expect("fn_db");
    let natives = BuiltinNatives::new();
    let mut out = HashMap::new();
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            let hir = gen_function_with_natives(k, s, &reg, &db, Some(&natives)).expect("hir");
            out.insert(k.clone(), hir);
        }
    }
    (reg, db, out)
}

fn run_program(extra: &str, entry: &typer::FnSig) -> Vec<u8> {
    let (reg, db, fns) = compile_program(extra);
    let inlined = inline_program_full(&fns, entry, Some(&reg), Some(&db)).expect("inline");
    let mir_block = hir_to_mir(&inlined.body).expect("mir");
    struct Null;
    impl mir::RunContext for Null {
        fn input(&mut self) -> u8 { 0 }
        fn print(&mut self, _: char) {}
    }
    let mut state = mir::MemoryState::new((inlined.slot_count as usize + 64).max(256), 4);
    let mut ctx = Null;
    state.execute_block(&mir_block, &mut ctx);
    let input_count = inlined.sig.input_count as usize;
    let output_count = inlined.sig.output_count as usize;
    (0..output_count)
        .map(|i| state.get_mem((input_count + i) as u32))
        .collect()
}

// =========================================================================
// Baseline: stdlib `impl Add<U4> for U4` still works via `+` sugar after
// the Ops.ct refactor.
// =========================================================================

#[test]
fn plain_u4_plus_still_works() {
    let src = r#"
        extension U4 {
            fn test(): U4 { 3 + 5 }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![8]);
}

#[test]
fn plain_u4_minus_still_works() {
    let src = r#"
        extension U4 {
            fn test(): U4 { 9 - 4 }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![5]);
}

// =========================================================================
// `<Self as Add>::Output` parses and resolves in a return-type position.
// =========================================================================

#[test]
fn qualified_path_output_in_return_type() {
    // `fn my_add(self, U4 other): <Self as Add>::Output`.
    // With the stdlib `impl Add<U4> for U4`, this resolves to `U4`.
    let src = r#"
        extension U4 {
            fn my_add(self, U4 other): <Self as Add>::Output {
                self + other
            }
            fn test(): U4 {
                U4 x = 4;
                x.my_add(5)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![9]);
}

// =========================================================================
// Custom Add with a custom Output: `impl Add<U4> for Wrap` returns a U4
// instead of the receiver type.
// =========================================================================

#[test]
fn add_with_custom_output_type() {
    // `Wrap` has one U4 field; `impl Add<U4> for Wrap` returns U4 (the
    // sum of the wrapped value and the rhs). A plain `w + w2` desugars
    // to `Wrap::add(w, w2): U4`.
    let src = r#"
        struct Wrap { U4 v, }
        extension Wrap {
            fn new(U4 v): Self { Self { v: v, } }
        }
        impl Add<U4> for Wrap {
            fn add(self, Wrap other): U4 {
                self.v + other.v
            }
        }
        extension U4 {
            fn test(): U4 {
                Wrap a = Wrap::new(2);
                Wrap b = Wrap::new(3);
                a + b
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![5]);
}

// =========================================================================
// Widening Add: `impl Add<U8> for U4Pair`. The binop return size must
// come from the impl's Output (U8 = 2 cells), not from the lhs.
// =========================================================================

#[test]
fn add_with_widening_output() {
    // `Widener` has one U4 field; `impl Add<U8> for Widener` adds the
    // wrapped values into the *lower* nibble of a U8 (higher always 0).
    // The caller gets a 2-cell return, so both cells must be emitted.
    let src = r#"
        struct Widener { U4 v, }
        extension Widener {
            fn new(U4 v): Self { Self { v: v, } }
        }
        impl Add<U8> for Widener {
            fn add(self, Widener other): U8 {
                U8 { lower: self.v + other.v, higher: 0, }
            }
        }
        extension U4 {
            fn test(): U4 {
                Widener a = Widener::new(3);
                Widener b = Widener::new(4);
                U8 sum = a + b;
                sum.lower
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// `<T as Sub>::Output` qualified path at a type position (local binding).
// =========================================================================

#[test]
fn qualified_path_in_local_binding() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                <U4 as Sub>::Output r = 9 - 4;
                r
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![5]);
}

// =========================================================================
// Operator dispatch on a generic struct still works with the new
// `Add<Output>` form.
// =========================================================================

#[test]
fn generic_struct_add_impl_with_output() {
    let src = r#"
        struct Pair<T> { T a, T b, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        impl Add<Pair<U4>> for Pair<U4> {
            fn add(self, Pair<U4> other): Pair<U4> {
                Pair::new(self.a + other.a, self.b + other.b)
            }
        }
        extension U4 {
            fn test(): U4 {
                Pair<U4> p = Pair::new(1, 2);
                Pair<U4> q = Pair::new(3, 4);
                Pair<U4> r = p + q;
                r.a + r.b
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![10]);
}
