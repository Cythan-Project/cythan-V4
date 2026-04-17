//! Stress tests for template argument handling across every construct
//! that can carry templates: methods, traits, enums, structs, and their
//! nestings. The centerpiece is the "triple nesting" scenario: a struct
//! parametrized by `T` that implements a trait parametrized by `U` whose
//! method is parametrized by `V`. Each test exercises exactly one
//! template-resolution path so a regression lands on a named target.
//!
//! Harness is copied from `arraylist_tests.rs` — compile stdlib + extra
//! source, inline from the test entry, run the MIR interpreter, assert
//! on output cells.

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
        ("std/ArrayList.ct", load("std/ArrayList.ct")),
        ("std/Option.ct", load("std/Option.ct")),
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

fn try_run_program(extra: &str, entry: &typer::FnSig) -> Result<Vec<u8>, String> {
    // Catch panics from compile_program / inline — returned as Err so tests
    // can document "this path is not yet supported" without tanking the run.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (reg, db, fns) = compile_program(extra);
        let inlined = inline_program_full(&fns, entry, Some(&reg), Some(&db))
            .map_err(|e| format!("inline: {}", e))?;
        let mir_block = hir_to_mir(&inlined.body).map_err(|e| format!("mir: {}", e))?;
        struct Null;
        impl mir::RunContext for Null {
            fn input(&mut self) -> u8 { 0 }
            fn print(&mut self, _: char) {}
        }
        let mut state =
            mir::MemoryState::new((inlined.slot_count as usize + 64).max(256), 4);
        let mut ctx = Null;
        state.execute_block(&mir_block, &mut ctx);
        let input_count = inlined.sig.input_count as usize;
        let output_count = inlined.sig.output_count as usize;
        Ok((0..output_count)
            .map(|i| state.get_mem((input_count + i) as u32))
            .collect::<Vec<u8>>())
    }));
    match result {
        Ok(r) => r,
        Err(p) => Err(format!("panic: {:?}", p.downcast_ref::<String>())),
    }
}

fn run_program(extra: &str, entry: &typer::FnSig) -> Vec<u8> {
    try_run_program(extra, entry).unwrap_or_else(|e| panic!("{}", e))
}

// =========================================================================
// Structs with multiple template args (baseline — ArrayList exists, but
// these re-exercise "same T appears in multiple field positions").
// =========================================================================

#[test]
fn struct_template_arg_reused_in_two_fields() {
    // Pair<T> has two T fields at different offsets — round-trip both.
    let src = r#"
        struct Pair<T> { T a, T b, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        extension U4 {
            fn test(): U4 {
                mut Pair<U4> p = Pair::new(3, 5);
                p.a + p.b
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![8]);
}

#[test]
fn struct_template_arg_with_multi_cell_element() {
    // Same struct instantiated with U8 (2 cells). Tests field-offset math
    // when the template arg has size > 1.
    let src = r#"
        struct Pair<T> { T a, T b, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        extension U4 {
            fn test(): U4 {
                mut Pair<U8> p = Pair::new(
                    U8 { lower: 1, higher: 2, },
                    U8 { lower: 3, higher: 4, },
                );
                p.a.higher + p.b.lower
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 2 + 3 = 5
    assert_eq!(run_program(src, &entry), vec![5]);
}

#[test]
fn struct_with_two_template_params() {
    let src = r#"
        struct Two<A, B> { A first, B second, }
        extension Two<A, B> {
            fn new(A a, B b): Self { Self { first: a, second: b, } }
        }
        extension U4 {
            fn test(): U4 {
                mut Two<U4, U8> t = Two::new(7, U8 { lower: 1, higher: 6, });
                t.first + t.second.higher
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 7 + 6 = 13
    assert_eq!(run_program(src, &entry), vec![13]);
}

// =========================================================================
// Generic struct whose field type is itself a generic struct.
// =========================================================================

#[test]
fn struct_field_is_generic_struct() {
    // Holder<T> wraps a Pair<T>. Exercises recursive sizing through two
    // user-defined templated struct layers.
    let src = r#"
        struct Pair<T> { T a, T b, }
        struct Holder<T> { Pair<T> inner, T tag, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        extension Holder<T> {
            fn new(T a, T b, T tag): Self {
                Self { inner: Pair::new(a, b), tag: tag, }
            }
        }
        extension U4 {
            fn test(): U4 {
                mut Holder<U4> h = Holder::new(2, 3, 9);
                h.inner.a + h.inner.b + h.tag
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![14]);
}

// =========================================================================
// Method-level template args on a non-generic receiver.
// =========================================================================

#[test]
fn method_template_arg_type_identity() {
    // id<T> — the simplest possible method-level template. The call site
    // pins T to U4 explicitly. The HIR gen must route the template arg
    // into the callee's sig so the param slot gets the right size.
    let src = r#"
        extension U4 {
            fn id<T>(T x): T { x }
            fn test(): U4 {
                U4::id<U4>(7)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

#[test]
fn method_template_arg_type_u8_element() {
    // Same method, but pinned to U8 — a 2-cell type. Checks sig-subst
    // produces the right arg/return sizes.
    let src = r#"
        extension U4 {
            fn id<T>(T x): T { x }
            fn test(): U4 {
                U4::id<U8>(U8 { lower: 4, higher: 11, }).higher
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![11]);
}

// =========================================================================
// Generic enums.
// =========================================================================

#[test]
fn generic_enum_option_is_none() {
    // Built-in stdlib: Option<T>. Make a None<U4> and check is_none.
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option::none();
                if o.is_none() { 1 } else { 0 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![1]),
        Err(e) => panic!("generic enum Option<U4> not yet supported end-to-end: {}", e),
    }
}

#[test]
fn generic_enum_option_some_is_not_none() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option::some(5);
                if o.is_none() { 1 } else { 0 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![0]),
        Err(e) => panic!("generic enum Option<U4> not yet supported end-to-end: {}", e),
    }
}

// =========================================================================
// Traits with template arguments.
// =========================================================================

#[test]
fn trait_with_template_arg_dispatch() {
    // Convert<To> is a trait parametrized by the target type. A single
    // impl pins To to U8; the typer accepts the trait-with-template-arg
    // form and the inliner monomorphizes the impl method.
    let src = r#"
        use Convert;
        trait Convert<To> {
            fn convert(self): To;
        }
        impl Convert<U8> for U4 {
            fn convert(self): U8 { U8 { lower: self, higher: 0, } }
        }
        extension U4 {
            fn test(): U4 {
                U4 x = 3;
                U8 y = x.convert();
                y.lower
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![3]),
        Err(e) => panic!("trait with template arg not yet supported end-to-end: {}", e),
    }
}

// =========================================================================
// Triple nesting: struct<T> implements trait<U> with method<V>.
// =========================================================================

#[test]
fn triple_nested_templates() {
    // Container<T> — struct template.
    // Transform<U>  — trait template, method apply<V>(V, T) : U(1-cell) — method template.
    //
    // We pin every layer explicitly at the call site so the resolver has
    // zero inference to do. If this compiles, the pipeline correctly
    // threads template bindings through all three scopes (struct, trait,
    // method).
    let src = r#"
        use Transform;
        struct Container<T> { T value, }
        extension Container<T> {
            fn new(T v): Self { Self { value: v, } }
            fn get(self): T { self.value }
        }
        trait Transform<U> {
            fn apply<V>(self, V v): U;
        }
        impl Transform<U4> for Container<U4> {
            fn apply<V>(self, V v): U4 { self.get() }
        }
        extension U4 {
            fn test(): U4 {
                mut Container<U4> c = Container::new(9);
                c.apply<U8>(U8 { lower: 1, higher: 2, })
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![9]),
        Err(e) => panic!("triple-nested templates not yet supported: {}", e),
    }
}

// =========================================================================
// Struct templates — value-only and mixed kinds.
//
// The parser accepts both type params `<T>` and value params `<N>` (used
// by Array's `<T, N, F>`). These tests exercise the value-only and mixed
// cases on user-defined structs so the registry's layout computation is
// hit with each kind.
// =========================================================================

#[test]
fn struct_with_value_template_only() {
    // `Buf<N>` — a user struct whose only template is an integer. The
    // field `data: Array<U4, N, U4>` forwards N into the native Array
    // sizer. Sizing Buf<3> should produce 3 cells.
    let src = r#"
        struct Buf<N> { Array<U4, N, U4> data, }
        extension Buf<N> {
            fn new(): Self { Self { data: Array::new(), } }
            fn put(mut self, U4 i, U4 v) { self.data.set(i, v); }
            fn got(self, U4 i): U4 { self.data.get(i) }
        }
        extension U4 {
            fn test(): U4 {
                mut Buf<3> b = Buf::new();
                b.put(0, 2);
                b.put(1, 5);
                b.got(0) + b.got(1)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![7]),
        Err(e) => panic!("value-only template struct not yet supported: {}", e),
    }
}

#[test]
fn struct_mixing_type_and_value_templates() {
    // Like Buf but T is also a template.
    let src = r#"
        struct TypedBuf<T, N> { Array<T, N, U4> data, }
        extension TypedBuf<T, N> {
            fn new(): Self { Self { data: Array::new(), } }
        }
        extension U4 {
            fn test(): U4 {
                mut TypedBuf<U4, 3> b = TypedBuf::new();
                0
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![0]),
        Err(e) => panic!("mixed type+value template struct: {}", e),
    }
}

#[test]
fn struct_with_three_type_params() {
    let src = r#"
        struct Triple<A, B, C> { A x, B y, C z, }
        extension Triple<A, B, C> {
            fn new(A a, B b, C c): Self { Self { x: a, y: b, z: c, } }
        }
        extension U4 {
            fn test(): U4 {
                mut Triple<U4, U4, U4> t = Triple::new(1, 2, 3);
                t.x + t.y + t.z
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![6]),
        Err(e) => panic!("three type params: {}", e),
    }
}

#[test]
fn struct_same_template_reused_as_array_index_type() {
    // T appears *both* as the element type and as the index type of the
    // backing Array. This is the exact pattern `ArrayList<Index, N, T>`
    // uses, but in a simpler form so a failure here isolates the issue.
    let src = r#"
        struct Ix<T, N> { Array<T, N, T> data, }
        extension Ix<T, N> {
            fn new(): Self { Self { data: Array::new(), } }
            fn at(self, T i): T { self.data.get(i) }
            fn put(mut self, T i, T v) { self.data.set(i, v); }
        }
        extension U4 {
            fn test(): U4 {
                mut Ix<U4, 4> x = Ix::new();
                x.put(1, 9);
                x.at(1)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![9]),
        Err(e) => panic!("same T as element and index type: {}", e),
    }
}

// =========================================================================
// Multiple monomorphs of the same generic struct in one function.
// Forces the mono cache to produce distinct entries for Pair<U4> and
// Pair<U8> and route calls correctly between them.
// =========================================================================

#[test]
fn two_monomorphs_of_same_generic_in_one_function() {
    let src = r#"
        struct Pair<T> { T a, T b, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        extension U4 {
            fn test(): U4 {
                mut Pair<U4> p4 = Pair::new(2, 3);
                mut Pair<U8> p8 = Pair::new(
                    U8 { lower: 1, higher: 4, },
                    U8 { lower: 5, higher: 6, },
                );
                p4.a + p4.b + p8.a.higher
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![9]),
        Err(e) => panic!("two monomorphs in one fn: {}", e),
    }
}

// =========================================================================
// Deeply nested template instantiation (three levels of ArrayList).
// Stresses recursive sizing + inliner monomorph cache.
// =========================================================================

#[test]
fn triple_nested_arraylist_three_levels_deep() {
    // Outer holds ArrayLists-of-ArrayLists-of-U4. Deepest level has one
    // value pushed; walk all three levels of `get` in the assertion.
    // Break each level into its own local binding so failures narrow
    // down to a specific layer.
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 2, ArrayList<U4, 2, ArrayList<U4, 3, U4>>> outer = ArrayList::new();
                mut ArrayList<U4, 2, ArrayList<U4, 3, U4>> mid = ArrayList::new();
                mut ArrayList<U4, 3, U4> inner = ArrayList::new();
                inner.push(7);
                mid.push(inner);
                outer.push(mid);
                mut ArrayList<U4, 2, ArrayList<U4, 3, U4>> got_mid = outer.get(0);
                mut ArrayList<U4, 3, U4> got_inner = got_mid.get(0);
                got_inner.get(0)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// Generic struct containing a generic struct (different T outside/inside).
// =========================================================================

#[test]
fn outer_and_inner_generic_with_different_t() {
    // Outer<U4> wrapping a Pair<U8>. Template args do NOT compose trivially:
    // the outer's T is unrelated to the inner's T.
    let src = r#"
        struct Pair<T> { T a, T b, }
        struct Outer<U> { Pair<U8> p, U tag, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        extension Outer<U> {
            fn new(U8 a, U8 b, U tag): Self {
                Self { p: Pair::new(a, b), tag: tag, }
            }
        }
        extension U4 {
            fn test(): U4 {
                mut Outer<U4> o = Outer::new(
                    U8 { lower: 1, higher: 2, },
                    U8 { lower: 3, higher: 4, },
                    9,
                );
                o.p.a.higher + o.tag
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 2 + 9 = 11
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![11]),
        Err(e) => panic!("outer/inner different T: {}", e),
    }
}

// =========================================================================
// Generic enums — variants and matching.
// =========================================================================

#[test]
fn generic_enum_option_match_payload() {
    // Build Some(7), extract the payload via a user-defined match arm.
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option::some(7);
                match o {
                    Option::Some(v) => v,
                    Option::None => 0,
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![7]),
        Err(e) => panic!("match on Option<U4> payload: {}", e),
    }
}

#[test]
fn generic_enum_option_with_multi_cell_payload() {
    // Some(U8). Sizing must account for the 2-cell payload.
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U8> o = Option::some(U8 { lower: 3, higher: 8, });
                match o {
                    Option::Some(v) => v.higher,
                    Option::None => 0,
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![8]),
        Err(e) => panic!("Option<U8> multi-cell payload: {}", e),
    }
}

#[test]
fn generic_enum_with_two_template_params() {
    // Result<T, E>. Two type templates on an enum. Construct an Ok, pull
    // the value out.
    let src = r#"
        enum Result<T, E> {
            Ok(T),
            Err(E),
        }
        extension Result<T, E> {
            fn ok(T t): Self { Self::Ok(t) }
        }
        extension U4 {
            fn test(): U4 {
                Result<U4, U8> r = Result::ok(5);
                match r {
                    Result::Ok(v) => v,
                    Result::Err(_) => 0,
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![5]),
        Err(e) => panic!("Result<T,E> two-template enum: {}", e),
    }
}

#[test]
fn generic_enum_option_nested_in_option() {
    // Option<Option<U4>> — the outer payload is itself a generic enum.
    // Size computation chains through two monomorphs.
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<Option<U4>> oo = Option::some(Option::some(4));
                match oo {
                    Option::Some(inner) => match inner {
                        Option::Some(v) => v,
                        Option::None => 0,
                    },
                    Option::None => 0,
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![4]),
        Err(e) => panic!("Option<Option<U4>>: {}", e),
    }
}

// =========================================================================
// Method-level templates — variations.
// =========================================================================

#[test]
fn method_template_value_arg() {
    // Method templated over an integer. The N can't appear as an
    // expression in the body (the parser treats it as an identifier, not
    // a constant), but the template can still influence the monomorph
    // key — two different N values produce two distinct monomorphs.
    // Here we exercise that by pinning a `Buf<N>`-returning factory.
    let src = r#"
        struct Buf<N> { Array<U4, N, U4> data, }
        extension Buf<N> {
            fn new(): Self { Self { data: Array::new(), } }
        }
        extension U4 {
            fn mk<N>(): Buf<N> { Buf<N>::new() }
            fn test(): U4 {
                mut Buf<3> b = U4::mk<3>();
                b.data.set(0, 8);
                b.data.get(0)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![8]),
        Err(e) => panic!("method value template: {}", e),
    }
}

#[test]
fn method_template_two_type_params() {
    let src = r#"
        extension U4 {
            fn first<A, B>(A a, B b): A { a }
            fn test(): U4 {
                U4::first<U4, U8>(6, U8 { lower: 1, higher: 2, })
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![6]),
        Err(e) => panic!("method two type templates: {}", e),
    }
}

#[test]
fn method_template_returns_generic_struct() {
    // `wrap<T>(T x): Pair<T>` — a method whose return type is built from
    // its own template. Both the arg-size and return-size paths must
    // substitute T.
    let src = r#"
        struct Pair<T> { T a, T b, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        extension U4 {
            fn wrap<T>(T x): Pair<T> { Pair::new(x, x) }
            fn test(): U4 {
                Pair<U4> p = U4::wrap<U4>(4);
                p.a + p.b
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![8]),
        Err(e) => panic!("method returning generic struct: {}", e),
    }
}

#[test]
fn method_template_on_generic_receiver() {
    // `Container<T>::map<U>(self, U u): U` — the method's template is
    // distinct from the receiver's. The FnRef must carry *both* — first
    // the receiver's, then the method's.
    let src = r#"
        struct Container<T> { T value, }
        extension Container<T> {
            fn new(T v): Self { Self { value: v, } }
            fn map<U>(self, U u): U { u }
        }
        extension U4 {
            fn test(): U4 {
                mut Container<U4> c = Container::new(1);
                c.map<U4>(9)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![9]),
        Err(e) => panic!("method template on generic receiver: {}", e),
    }
}

#[test]
fn method_template_param_shadows_struct_template() {
    // Struct uses `T`; method declares its own `T` that shadows it. If
    // the resolver uses lexical scoping correctly, the method's T binds
    // to whatever the call site pins it to.
    let src = r#"
        struct Box<T> { T v, }
        extension Box<T> {
            fn new(T v): Self { Self { v: v, } }
            // Inner T shadows outer T — this method's input is totally
            // independent of the struct's parameterization.
            fn swap<T>(self, T other): T { other }
        }
        extension U4 {
            fn test(): U4 {
                mut Box<U4> b = Box::new(1);
                b.swap<U4>(7)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![7]),
        Err(e) => panic!("method T shadowing struct T: {}", e),
    }
}

// =========================================================================
// Trait templates — value, multiple, dispatch.
// =========================================================================

#[test]
fn trait_with_value_template_arg() {
    // `Indexed<N>` — trait whose template is a value. One impl pins N
    // to a specific value; the typer and inliner must accept and route
    // the value template through.
    let src = r#"
        use Indexed;
        trait Indexed<N> {
            fn at(self): U4;
        }
        impl Indexed<3> for U4 {
            fn at(self): U4 { self + 1 }
        }
        extension U4 {
            fn test(): U4 {
                U4 x = 4;
                x.at()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![5]),
        Err(e) => panic!("trait value template: {}", e),
    }
}

#[test]
fn trait_with_two_type_templates() {
    let src = r#"
        use Map;
        trait Map<I, O> {
            fn map(self, I i): O;
        }
        impl Map<U4, U8> for U4 {
            fn map(self, U4 i): U8 { U8 { lower: self, higher: i, } }
        }
        extension U4 {
            fn test(): U4 {
                U4 x = 2;
                U8 y = x.map(5);
                y.higher + y.lower
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![7]),
        Err(e) => panic!("trait two type templates: {}", e),
    }
}

// =========================================================================
// Operator impl on a generic struct. Forces the dispatcher to find an
// `impl Add for Pair<U4>` — matching both the trait head *and* the
// concrete template args.
// =========================================================================

#[test]
fn add_impl_on_generic_struct() {
    let src = r#"
        struct Pair<T> { T a, T b, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        impl Add for Pair<U4> {
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
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![10]),
        Err(e) => panic!("Add impl on Pair<U4>: {}", e),
    }
}

// =========================================================================
// ArrayList of a user-defined generic struct. Requires sizing
// `Array<Pair<U4>, 3, U4>` recursively through user-defined layouts.
// =========================================================================

#[test]
fn arraylist_of_generic_struct_element() {
    let src = r#"
        struct Pair<T> { T a, T b, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 3, Pair<U4>> list = ArrayList::new();
                list.push(Pair::new(1, 2));
                list.push(Pair::new(3, 4));
                list.get(1).a + list.get(1).b + list.get(0).a
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![8]),
        Err(e) => panic!("ArrayList of Pair<U4>: {}", e),
    }
}

// =========================================================================
// Generic struct containing an ArrayList. Same plumbing, different
// direction of nesting.
// =========================================================================

#[test]
fn generic_struct_containing_arraylist_field() {
    let src = r#"
        struct Named<T> { ArrayList<U4, 3, T> items, U4 id, }
        extension Named<T> {
            fn new(U4 id): Self {
                Self { items: ArrayList::new(), id: id, }
            }
        }
        extension U4 {
            fn test(): U4 {
                mut Named<U4> n = Named::new(7);
                n.items.push(2);
                n.items.push(3);
                n.items.get(0) + n.items.get(1) + n.id
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![12]),
        Err(e) => panic!("struct Named containing ArrayList: {}", e),
    }
}

// =========================================================================
// Struct field that is Option<T>.
// =========================================================================

#[test]
fn struct_with_option_field() {
    let src = r#"
        use Option;
        struct Maybe<T> { Option<T> inner, }
        extension Maybe<T> {
            fn of(T t): Self { Self { inner: Option::some(t), } }
        }
        extension U4 {
            fn test(): U4 {
                Maybe<U4> m = Maybe::of(3);
                match m.inner {
                    Option::Some(v) => v,
                    Option::None => 0,
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![3]),
        Err(e) => panic!("struct with Option<T> field: {}", e),
    }
}

// =========================================================================
// Template value = 1 (minimum non-trivial size).
// =========================================================================

#[test]
fn arraylist_capacity_one_push_get() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 1, U4> list = ArrayList::new();
                list.push(9);
                list.get(0)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![9]);
}

// =========================================================================
// Method template whose template arg appears in a local binding's type.
// Tests that the substitution walks into local decls, not just params.
// =========================================================================

#[test]
fn method_template_local_binding_uses_template() {
    let src = r#"
        extension U4 {
            fn echo<T>(T x): T {
                mut T local = x;
                local
            }
            fn test(): U4 {
                U4::echo<U4>(6)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![6]),
        Err(e) => panic!("method template in local binding type: {}", e),
    }
}

// =========================================================================
// Same generic method called with two different T in one function body.
// =========================================================================

#[test]
fn method_template_two_different_monomorphs_in_one_body() {
    let src = r#"
        extension U4 {
            fn id<T>(T x): T { x }
            fn test(): U4 {
                U4 a = U4::id<U4>(2);
                U8 b = U4::id<U8>(U8 { lower: 3, higher: 4, });
                a + b.higher
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![6]),
        Err(e) => panic!("two monomorphs of id<T> in one body: {}", e),
    }
}

// =========================================================================
// Extension method that returns a *different* monomorph of Self.
// `Container<U4>::to_u8(self): Container<U8>`. The callee's return type
// involves a concrete instantiation of the same generic as self.
// =========================================================================

#[test]
fn method_returns_different_monomorph_of_self_generic() {
    let src = r#"
        struct Container<T> { T value, }
        extension Container<T> {
            fn new(T v): Self { Self { value: v, } }
        }
        extension Container<U4> {
            fn to_u8(self): Container<U8> {
                Container::new(U8 { lower: self.value, higher: 0, })
            }
        }
        extension U4 {
            fn test(): U4 {
                Container<U4> c = Container::new(5);
                Container<U8> d = c.to_u8();
                d.value.lower
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![5]),
        Err(e) => panic!("Container<U4>::to_u8 -> Container<U8>: {}", e),
    }
}

// =========================================================================
// Match on generic enum inside a generic method. Two layers of template
// resolution need to cooperate: the method-level T and the Option<T>'s T.
// =========================================================================

#[test]
fn generic_method_matches_generic_enum() {
    let src = r#"
        use Option;
        extension U4 {
            fn unwrap_or<T>(Option<T> o, T dflt): T {
                match o {
                    Option::Some(v) => v,
                    Option::None => dflt,
                }
            }
            fn test(): U4 {
                U4::unwrap_or<U4>(Option::some(7), 0)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![7]),
        Err(e) => panic!("generic method matching generic enum: {}", e),
    }
}

// =========================================================================
// Bare `Self { .. }` construction inside a method whose *outer* template
// scope differs from the struct's. Ensures `Self` resolves to the
// fully-qualified instantiation, not just the bare head.
// =========================================================================

#[test]
fn self_construction_in_generic_extension() {
    // Pair<T> constructed via `Self { .. }` literal. The resolver must
    // fill in T from the surrounding extension binding, not leave it
    // generic.
    let src = r#"
        struct Pair<T> { T a, T b, }
        extension Pair<T> {
            fn zero(T z): Self { Self { a: z, b: z, } }
        }
        extension U4 {
            fn test(): U4 {
                Pair<U4> p = Pair::zero(4);
                p.a + p.b
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![8]),
        Err(e) => panic!("Self literal in generic extension: {}", e),
    }
}

// =========================================================================
// Generic struct's method passes `self` by value to another generic
// method. Arg sizing must resolve via the binding's template_args.
// =========================================================================

#[test]
fn generic_method_passes_self_to_another_generic_method() {
    let src = r#"
        struct Box<T> { T v, }
        extension Box<T> {
            fn new(T v): Self { Self { v: v, } }
            fn first(self): T { self.v }
            fn double_first(self): T { self.first() }
        }
        extension U4 {
            fn test(): U4 {
                Box<U4> b = Box::new(7);
                b.double_first()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![7]),
        Err(e) => panic!("generic method calling generic method with self: {}", e),
    }
}

// =========================================================================
// Template arg used only in arg, not in fields. Ensures the sig-subst
// pass reaches arg positions even when the struct layout ignores T.
// =========================================================================

#[test]
fn template_arg_only_in_method_not_in_field_layout() {
    let src = r#"
        struct Tag<T> { U4 id, }
        extension Tag<T> {
            fn new(U4 id): Self { Self { id: id, } }
            fn use_t(self, T input): U4 { self.id }
        }
        extension U4 {
            fn test(): U4 {
                Tag<U4> t = Tag::new(6);
                t.use_t(0)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![6]),
        Err(e) => panic!("T only in method arg, not fields: {}", e),
    }
}

// =========================================================================
// Generic method returning Self (i.e., same generic instantiation as the
// receiver). The inliner must reuse the receiver's template_args for the
// return-size computation.
// =========================================================================

#[test]
fn generic_method_returns_self_instantiation() {
    let src = r#"
        struct Box<T> { T v, }
        extension Box<T> {
            fn new(T v): Self { Self { v: v, } }
            fn copy(self): Self { Self { v: self.v, } }
        }
        extension U4 {
            fn test(): U4 {
                Box<U4> b = Box::new(4);
                Box<U4> c = b.copy();
                c.v
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![4]),
        Err(e) => panic!("generic method returning Self: {}", e),
    }
}

// =========================================================================
// Generic type flowing through an `if` expression — both branches must
// produce the same concrete type, and the temp slot size must match.
// =========================================================================

#[test]
fn generic_value_through_if_expression() {
    let src = r#"
        struct Box<T> { T v, }
        extension Box<T> {
            fn new(T v): Self { Self { v: v, } }
        }
        extension U4 {
            fn test(): U4 {
                U4 cond = 1;
                Box<U4> b = if cond { Box::new(3) } else { Box::new(8) };
                b.v
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![3]),
        Err(e) => panic!("generic value through if expr: {}", e),
    }
}

// =========================================================================
// Generic value updated inside a loop body. Mutability + generic binding
// must coexist across iterations.
// =========================================================================

#[test]
fn generic_binding_mutated_in_loop() {
    let src = r#"
        struct Box<T> { T v, }
        extension Box<T> {
            fn new(T v): Self { Self { v: v, } }
            fn set(mut self, T v) { self.v = v; }
        }
        extension U4 {
            fn test(): U4 {
                mut Box<U4> b = Box::new(0);
                mut U4 i = 0;
                loop {
                    if i == 4 { break; }
                    i += 1;
                    b.set(i);
                }
                b.v
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![4]),
        Err(e) => panic!("mut generic in loop: {}", e),
    }
}

// =========================================================================
// Array of Option<T> — pairs the Array native sizing with the generic
// enum sizing path.
// =========================================================================

#[test]
fn array_of_generic_enum_element() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                mut Array<Option<U4>, 3, U4> arr = Array::new();
                arr.set(0, Option::some(7));
                match arr.get(0) {
                    Option::Some(v) => v,
                    Option::None => 0,
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![7]),
        Err(e) => panic!("Array<Option<U4>, 3, U4>: {}", e),
    }
}

// =========================================================================
// Edge: size-0 and size-1 at the extremes.
// =========================================================================

#[test]
fn arraylist_of_bool_count_true() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, Bool> list = ArrayList::new();
                list.push(true);
                list.push(true);
                list.push(false);
                list.push(true);
                mut U4 i = 0;
                mut U4 n = 0;
                loop {
                    if i == list.len() { break; }
                    if list.get(i) { n += 1; }
                    i += 1;
                }
                n
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![3]);
}

// =========================================================================
// Mutable self through a chain of generic methods.
// =========================================================================

#[test]
fn mut_self_chain_through_two_generic_methods() {
    let src = r#"
        struct Counter<T> { T v, }
        extension Counter<T> {
            fn new(T v): Self { Self { v: v, } }
        }
        extension Counter<U4> {
            fn bump(mut self) { self.v += 1; }
            fn bump_twice(mut self) {
                self.bump();
                self.bump();
            }
        }
        extension U4 {
            fn test(): U4 {
                mut Counter<U4> c = Counter::new(0);
                c.bump_twice();
                c.v
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![2]),
        Err(e) => panic!("mut self chain through two generic methods: {}", e),
    }
}

// =========================================================================
// Return statement inside a generic method with an early-exit branch.
// Monomorphization must thread the early-return slot correctly.
// =========================================================================

#[test]
fn generic_method_with_early_return() {
    let src = r#"
        struct Box<T> { T v, }
        extension Box<T> {
            fn new(T v): Self { Self { v: v, } }
        }
        extension Box<U4> {
            fn limited(self, U4 cap): U4 {
                if self.v > cap { return cap; }
                self.v
            }
        }
        extension U4 {
            fn test(): U4 {
                Box<U4> b = Box::new(9);
                b.limited(5)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![5]),
        Err(e) => panic!("generic method early return: {}", e),
    }
}

// =========================================================================
// Operator desugar using a trait on a generic struct: `p += q` where
// `p` is `Pair<U4>` and AddAssign is impl'd only for this instantiation.
// =========================================================================

#[test]
fn add_assign_impl_on_generic_struct() {
    let src = r#"
        struct Pair<T> { T a, T b, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        impl AddAssign for Pair<U4> {
            fn add_assign(mut self, Pair<U4> other) {
                self.a += other.a;
                self.b += other.b;
            }
        }
        extension U4 {
            fn test(): U4 {
                mut Pair<U4> p = Pair::new(1, 2);
                Pair<U4> q = Pair::new(3, 4);
                p += q;
                p.a + p.b
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![10]),
        Err(e) => panic!("AddAssign on generic struct: {}", e),
    }
}

// =========================================================================
// Eq impl on a generic struct, then use `==` in a conditional. The
// operator-desugar machinery must find the right impl.
// =========================================================================

#[test]
fn eq_impl_on_generic_struct_used_in_if() {
    let src = r#"
        struct Pair<T> { T a, T b, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        impl Eq for Pair<U4> {
            fn eq(self, Pair<U4> other): Bool {
                if self.a == other.a {
                    if self.b == other.b { true } else { false }
                } else { false }
            }
            fn ne(self, Pair<U4> other): Bool {
                if self.eq(other) { false } else { true }
            }
        }
        extension U4 {
            fn test(): U4 {
                Pair<U4> p = Pair::new(2, 3);
                Pair<U4> q = Pair::new(2, 3);
                if p == q { 1 } else { 0 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![1]),
        Err(e) => panic!("Eq impl on generic struct used via `==`: {}", e),
    }
}

// =========================================================================
// Template value arg = 0. Edge of the value-template range — `Array` of
// size 0, wrapping `ArrayList` of capacity 0. Both should size to zero
// cells without panicking.
// =========================================================================

#[test]
fn arraylist_with_zero_capacity_is_full() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 0, U4> list = ArrayList::new();
                if list.is_full() { 1 } else { 0 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![1]),
        Err(e) => panic!("zero-capacity ArrayList edge: {}", e),
    }
}

// =========================================================================
// Same generic struct used twice with the same T — cache should hit, not
// re-synthesize. (Correctness-only assertion; cache metrics aren't
// visible here, but we verify that two independent instances stay
// independent.)
// =========================================================================

#[test]
fn two_independent_values_same_monomorph() {
    let src = r#"
        struct Box<T> { T v, }
        extension Box<T> {
            fn new(T v): Self { Self { v: v, } }
        }
        extension U4 {
            fn test(): U4 {
                mut Box<U4> a = Box::new(3);
                mut Box<U4> b = Box::new(5);
                a.v + b.v
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![8]),
        Err(e) => panic!("two Box<U4> independent values: {}", e),
    }
}

// =========================================================================
// Struct field of type Array parametrized by the struct's template arg.
// Variant of ArrayList's pattern but with a different field layout.
// =========================================================================

#[test]
fn struct_with_array_field_using_template_arg() {
    let src = r#"
        struct Bag<T, N> {
            Array<T, N, U4> items,
            U4 count,
        }
        extension Bag<T, N> {
            fn new(): Self { Self { items: Array::new(), count: 0, } }
        }
        extension Bag<U4, 4> {
            fn put(mut self, T v) {
                self.items.set(self.count, v);
                self.count += 1;
            }
        }
        extension U4 {
            fn test(): U4 {
                mut Bag<U4, 4> b = Bag::new();
                b.put(2);
                b.put(5);
                b.items.get(0) + b.items.get(1) + b.count
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![9]),
        Err(e) => panic!("Bag<T,N> with Array<T,N,U4> field: {}", e),
    }
}

// =========================================================================
// Generic struct with field depending on value template via Array.
// =========================================================================

#[test]
fn struct_size_scales_with_value_template() {
    // Two instantiations with different N — verify each has its own
    // layout. Simultaneous in one function body.
    let src = r#"
        struct Buf<N> { Array<U4, N, U4> data, }
        extension Buf<N> {
            fn new(): Self { Self { data: Array::new(), } }
        }
        extension U4 {
            fn test(): U4 {
                mut Buf<2> small = Buf::new();
                mut Buf<4> big = Buf::new();
                small.data.set(0, 1);
                big.data.set(3, 9);
                small.data.get(0) + big.data.get(3)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![10]),
        Err(e) => panic!("two different N for Buf: {}", e),
    }
}

// =========================================================================
// Inference corner: `Array::new()` on the RHS of a declaration whose LHS
// pins the type. Already works in stdlib; adding the explicit check.
// =========================================================================

#[test]
fn array_new_inferred_from_binding_type() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 3, U4> a = Array::new();
                a.set(2, 9);
                a.get(2)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![9]);
}

// =========================================================================
// Extension<T> providing a method that uses *two* distinct template arg
// combinations on a callee.
// =========================================================================

#[test]
fn method_calls_two_monomorphs_of_helper() {
    let src = r#"
        extension U4 {
            fn id<T>(T x): T { x }
            fn both(): U4 {
                U4 a = U4::id<U4>(3);
                U8 b = U4::id<U8>(U8 { lower: 2, higher: 5, });
                a + b.higher
            }
            fn test(): U4 { U4::both() }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    match try_run_program(src, &entry) {
        Ok(v) => assert_eq!(v, vec![8]),
        Err(e) => panic!("indirect method call into two monomorphs: {}", e),
    }
}
