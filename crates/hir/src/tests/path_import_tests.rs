//! Path-based imports and references.
//!
//! Each file gets a module path derived from its name (`std/Array.ct`
//! → `std::Array`). Types declared in that file remain accessible by
//! their bare name (back-compat) AND by their full path. `use a::b::X;`
//! acts as an explicit alias equivalent to bringing `X` into scope.
//!
//! Tests cover:
//!   - qualified references at type positions
//!   - `use` with a path
//!   - collisions (duplicate bare names across files) — currently a
//!     hard error from the registry
//!   - path through trait bounds
//!   - rename-via-use

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

/// Build a registry over the given list of (file_name, source) pairs.
/// Unlike the other test harnesses, this one lets the individual test
/// specify its own file names so we can construct scenarios where two
/// files' module paths matter (collisions, cross-file paths, etc.).
fn compile_files(
    parts: &[(&str, String)],
    entry: &typer::FnSig,
) -> Result<Vec<u8>, String> {
    let mut parsed: Vec<(String, Vec<new_parser::ast::Spanned<new_parser::ast::Item>>)> =
        Vec::with_capacity(parts.len());
    for (name, src) in parts {
        let items = new_parser::parse(src)
            .map_err(|e| format!("parse `{}`: {:?}", name, e))?;
        parsed.push((name.to_string(), items));
    }
    let as_refs: Vec<(&str, &[_])> = parsed
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_slice()))
        .collect();
    let reg = typer::TypeRegistry::from_files(&as_refs).map_err(|e| format!("typer: {:?}", e))?;
    let db = typer::FunctionDB::from_registry(&reg).map_err(|e| format!("fn_db: {:?}", e))?;
    let natives = BuiltinNatives::new();
    let mut out = HashMap::new();
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            let hir = gen_function_with_natives(k, s, &reg, &db, Some(&natives))
                .map_err(|e| format!("hir: {}", e))?;
            out.insert(k.clone(), hir);
        }
    }
    let inlined = inline_program_full(&out, entry, Some(&reg), Some(&db))
        .map_err(|e| format!("inline: {}", e))?;
    let mir_block = hir_to_mir(&inlined.body).map_err(|e| format!("mir: {}", e))?;
    struct Null;
    impl mir::RunContext for Null {
        fn input(&mut self) -> u8 { 0 }
        fn print(&mut self, _: u8) {}
    }
    let mut state = mir::MemoryState::new_with_limit((inlined.slot_count as usize + 64).max(256), 4, 5_000_000);
    let mut ctx = Null;
    state.execute_block(&mir_block, &mut ctx);
    let input_count = inlined.sig.input_count as usize;
    let output_count = inlined.sig.output_count as usize;
    Ok((0..output_count)
        .map(|i| state.get_mem((input_count + i) as u32))
        .collect::<Vec<u8>>())
}

fn minimal_stdlib() -> Vec<(&'static str, String)> {
    vec![
        ("std/System.ct", load("std/System.ct")),
        ("std/Ops.ct", load("std/Ops.ct")),
        ("std/Bool.ct", load("std/Bool.ct")),
        ("std/U4.ct", load("std/U4.ct")),
        ("std/U8.ct", load("std/U8.ct")),
        ("std/Array.ct", load("std/Array.ct")),
    ]
}

// =========================================================================
// Path-qualified reference: the user writes `user::Foo` explicitly.
// The leaf `Foo` is the registered bare name; the path acts as sugar.
// =========================================================================

#[test]
fn qualified_path_reference() {
    // Path in both a type-annotation AND a struct-literal position.
    let mut parts = minimal_stdlib();
    parts.push((
        "user.ct",
        r#"
            struct Foo { U4 v, }
            extension U4 {
                fn test(): U4 {
                    user::Foo f = user::Foo { v: 7, };
                    f.v
                }
            }
        "#
        .to_string(),
    ));
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(compile_files(&parts, &entry).unwrap(), vec![7]);
}

// =========================================================================
// `use path::Name;` brings `Name` into scope. The user code uses the
// short form `Name` directly even though the import was qualified.
// =========================================================================

#[test]
fn use_with_path_imports_type_by_short_name() {
    let lib = (
        "lib/ui.ct",
        r#"
            struct Widget { U4 n, }
            extension Widget {
                fn new(U4 n): Self { Self { n: n, } }
            }
        "#
        .to_string(),
    );
    let user = (
        "user.ct",
        r#"
            use lib::ui::Widget;
            extension U4 {
                fn test(): U4 {
                    Widget w = Widget::new(5);
                    w.n
                }
            }
        "#
        .to_string(),
    );
    let mut parts = minimal_stdlib();
    parts.push(lib);
    parts.push(user);
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(compile_files(&parts, &entry).unwrap(), vec![5]);
}

// =========================================================================
// Referencing a type by its path in a function signature and at call
// sites simultaneously.
// =========================================================================

#[test]
fn qualified_reference_in_sig_and_body() {
    let lib = (
        "lib/geom.ct",
        r#"
            struct Pt { U4 x, U4 y, }
        "#
        .to_string(),
    );
    let user = (
        "user.ct",
        r#"
            extension U4 {
                fn sum(lib::geom::Pt p): U4 { p.x + p.y }
                fn test(): U4 {
                    lib::geom::Pt p = lib::geom::Pt { x: 3, y: 4, };
                    U4::sum(p)
                }
            }
        "#
        .to_string(),
    );
    let mut parts = minimal_stdlib();
    parts.push(lib);
    parts.push(user);
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(compile_files(&parts, &entry).unwrap(), vec![7]);
}

// =========================================================================
// Collision: two files each declare a struct `Foo`. Both coexist — each
// file references its own `Foo` by bare name (resolved via file-local
// declarations), and a third file disambiguates via path or `use`.
// =========================================================================

#[test]
fn duplicate_bare_name_across_files_coexist() {
    let a = (
        "lib/a.ct",
        r#"
            struct Foo { U4 v, }
            extension Foo {
                fn make(): Self { Self { v: 1, } }
            }
        "#
        .to_string(),
    );
    let b = (
        "lib/b.ct",
        r#"
            struct Foo { U4 w, }
            extension Foo {
                fn make(): Self { Self { w: 2, } }
            }
        "#
        .to_string(),
    );
    // A third file picks one of them by full path and verifies both
    // are reachable via their FQ names.
    let user = (
        "user.ct",
        r#"
            extension U4 {
                fn test(): U4 {
                    lib::a::Foo fa = lib::a::Foo::make();
                    lib::b::Foo fb = lib::b::Foo::make();
                    fa.v + fb.w
                }
            }
        "#
        .to_string(),
    );
    let mut parts = minimal_stdlib();
    parts.push(a);
    parts.push(b);
    parts.push(user);
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(compile_files(&parts, &entry).unwrap(), vec![3]);
}

#[test]
fn duplicate_bare_name_bare_references_resolve_locally() {
    // Each file uses its OWN `Foo` by bare name. The resolver must
    // prefer file-local declarations over the ambiguous global.
    let a = (
        "lib/a.ct",
        r#"
            struct Foo { U4 v, }
            extension Foo {
                fn make(): Self { Self { v: 5, } }
                fn val(self): U4 { self.v }
            }
        "#
        .to_string(),
    );
    let b = (
        "lib/b.ct",
        r#"
            struct Foo { U4 w, }
            extension Foo {
                fn make(): Self { Self { w: 7, } }
                fn val(self): U4 { self.w }
            }
        "#
        .to_string(),
    );
    let user = (
        "user.ct",
        r#"
            extension U4 {
                fn test(): U4 {
                    lib::a::Foo fa = lib::a::Foo::make();
                    lib::b::Foo fb = lib::b::Foo::make();
                    fa.val() + fb.val()
                }
            }
        "#
        .to_string(),
    );
    let mut parts = minimal_stdlib();
    parts.push(a);
    parts.push(b);
    parts.push(user);
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(compile_files(&parts, &entry).unwrap(), vec![12]);
}

// =========================================================================
// Same file, same type declared twice — also an error, referencing the
// bare name.
// =========================================================================

#[test]
fn duplicate_within_single_file_errors() {
    let a = (
        "a.ct",
        r#"
            struct Dup { U4 v, }
            struct Dup { U4 w, }
        "#
        .to_string(),
    );
    let user = (
        "user.ct",
        r#"
            extension U4 {
                fn test(): U4 { 0 }
            }
        "#
        .to_string(),
    );
    let mut parts = minimal_stdlib();
    parts.push(a);
    parts.push(user);
    let entry = typer::FnSig::new("U4", "test");
    let err = compile_files(&parts, &entry).unwrap_err();
    assert!(err.contains("duplicate"));
    assert!(err.contains("Dup"));
}

// =========================================================================
// Path reference where the path doesn't match any registered alias.
// Resolution falls back to the bare leaf when the leaf exists — a
// permissive rule that keeps existing tests working even when the user
// made up a path prefix. Confirmed by using a nonsense prefix.
// =========================================================================

#[test]
fn path_with_wrong_prefix_falls_back_to_bare_leaf() {
    let lib = (
        "lib/helpers.ct",
        r#"
            struct Foo { U4 v, }
            extension Foo {
                fn new(U4 v): Self { Self { v: v, } }
            }
        "#
        .to_string(),
    );
    let user = (
        "user.ct",
        r#"
            extension U4 {
                fn test(): U4 {
                    // Path with a prefix that doesn't match the
                    // declaring file's module — resolver falls back
                    // to the unambiguous bare `Foo` since only one
                    // type with that leaf exists.
                    nonsense::prefix::Foo f = Foo::new(9);
                    f.v
                }
            }
        "#
        .to_string(),
    );
    let mut parts = minimal_stdlib();
    parts.push(lib);
    parts.push(user);
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(compile_files(&parts, &entry).unwrap(), vec![9]);
}

// =========================================================================
// Path reference to a trait in a `use` — the trait comes into scope so
// operator sugar picks it up.
// =========================================================================

#[test]
fn use_path_trait_brings_it_into_scope() {
    let lib = (
        "lib/math.ct",
        r#"
            trait Triple { fn triple(self): U4; }
            impl Triple for U4 {
                fn triple(self): U4 { self + self + self }
            }
        "#
        .to_string(),
    );
    let user = (
        "user.ct",
        r#"
            use lib::math::Triple;
            extension U4 {
                fn test(): U4 {
                    U4 x = 2;
                    x.triple()
                }
            }
        "#
        .to_string(),
    );
    let mut parts = minimal_stdlib();
    parts.push(lib);
    parts.push(user);
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(compile_files(&parts, &entry).unwrap(), vec![6]);
}

// =========================================================================
// Two identical paths in two different test files — the aliases
// idempotently resolve.
// =========================================================================

#[test]
fn multiple_files_agree_on_path_form() {
    let lib = (
        "lib/containers.ct",
        r#"
            struct Box<T> { T v, }
            extension Box<T> {
                fn new(T v): Self { Self { v: v, } }
            }
        "#
        .to_string(),
    );
    let helpers = (
        "helpers.ct",
        r#"
            extension U4 {
                fn unwrap(lib::containers::Box<U4> b): U4 { b.v }
            }
        "#
        .to_string(),
    );
    let user = (
        "user.ct",
        r#"
            extension U4 {
                fn test(): U4 {
                    // Full path in both positions.
                    lib::containers::Box<U4> b = lib::containers::Box::new(6);
                    U4::unwrap(b)
                }
            }
        "#
        .to_string(),
    );
    let mut parts = minimal_stdlib();
    parts.push(lib);
    parts.push(helpers);
    parts.push(user);
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(compile_files(&parts, &entry).unwrap(), vec![6]);
}

// =========================================================================
// `use` of a nonexistent type path — currently silent since `use` just
// records an alias. Calling code that then references the short name
// fails at the reference site. This test documents that failure path.
// =========================================================================

#[test]
fn use_of_nonexistent_path_then_use_bare_fails() {
    let user = (
        "user.ct",
        r#"
            use nothing::Nope;
            extension U4 {
                fn test(): U4 {
                    Nope n = Nope { };
                    0
                }
            }
        "#
        .to_string(),
    );
    let mut parts = minimal_stdlib();
    parts.push(user);
    let entry = typer::FnSig::new("U4", "test");
    let err = compile_files(&parts, &entry).unwrap_err();
    assert!(
        err.contains("Nope") || err.contains("unknown"),
        "expected an unknown-type error mentioning `Nope`, got: {}",
        err
    );
}

// =========================================================================
// Path reference inside a trait bound.
// =========================================================================

#[test]
fn path_in_trait_bound() {
    let lib = (
        "lib/tags.ct",
        r#"
            trait Tag { fn tag(self): U4; }
            impl Tag for U4 {
                fn tag(self): U4 { self + 1 }
            }
        "#
        .to_string(),
    );
    let user = (
        "user.ct",
        r#"
            use lib::tags::Tag;
            trait Proxy { fn proxy(self): U4; }
            impl<T: lib::tags::Tag> Proxy for T {
                fn proxy(self): U4 { self.tag() + 10 }
            }
            use Proxy;
            extension U4 {
                fn test(): U4 {
                    U4 x = 4;
                    x.proxy()
                }
            }
        "#
        .to_string(),
    );
    let mut parts = minimal_stdlib();
    parts.push(lib);
    parts.push(user);
    let entry = typer::FnSig::new("U4", "test");
    // 4 + 1 + 10 = 15 (fits in u4).
    assert_eq!(compile_files(&parts, &entry).unwrap(), vec![15]);
}
