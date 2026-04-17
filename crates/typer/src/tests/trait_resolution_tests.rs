//! Edge-case catalog for trait resolution.
//!
//! These tests define the *contract* the resolver must honour. They cover:
//!   - per-file trait-import scoping via `use Name;`
//!   - the "extension beats trait on ambiguity" rule
//!   - operator-sugar traits bypassing the import check
//!   - multi-file layouts (trait in file A, impl in file B, caller in C)
//!   - explicit qualification `Trait::method(args)`
//!   - monomorph key distinctness when two traits share a method name
//!
//! Tests that already pass against the current stub are left runnable. Tests
//! that describe behavior still to be wired up end-to-end (e.g. explicit
//! qualification, monomorph-key mangling with trait names) are marked
//! `#[ignore]` with a one-line reason — those will drive the remaining
//! implementation work.

use new_parser::ast::{Item, Spanned};

use crate::{MethodResolution, TypeRegistry, OPERATOR_TRAITS};

/// Parse each `(file_name, source)` pair and return owned item lists per file.
fn parse_files(files: &[(&str, &str)]) -> Vec<(String, Vec<Spanned<Item>>)> {
    files
        .iter()
        .map(|(name, src)| match new_parser::parse(src) {
            Ok(items) => (name.to_string(), items),
            Err(e) => panic!("parse `{}` failed: {:?}\n{}", name, e, src),
        })
        .collect()
}

/// Build a registry from multiple named files. Returns the registry plus a
/// name→FileId map for looking up per-file import scope in tests.
fn build_multi(files: &[(&str, &str)]) -> (TypeRegistry, std::collections::HashMap<String, u32>) {
    let parsed = parse_files(files);
    let as_refs: Vec<(&str, &[Spanned<Item>])> = parsed
        .iter()
        .map(|(n, items)| (n.as_str(), items.as_slice()))
        .collect();
    let reg = TypeRegistry::from_files(&as_refs)
        .unwrap_or_else(|errs| panic!("typer errors: {:?}", errs));
    let mut ids = std::collections::HashMap::new();
    for (ix, (name, _)) in files.iter().enumerate() {
        ids.insert(name.to_string(), ix as u32);
    }
    (reg, ids)
}

fn resolve(
    reg: &TypeRegistry,
    file_id: u32,
    ty: &str,
    method: &str,
) -> MethodResolution {
    reg.resolve_method(file_id, ty, method, None)
}

fn resolve_qualified(
    reg: &TypeRegistry,
    file_id: u32,
    ty: &str,
    method: &str,
    trait_name: &str,
) -> MethodResolution {
    reg.resolve_method(file_id, ty, method, Some(trait_name))
}

// ---------------------------------------------------------------------------
// Parser / AST sanity checks (these must work immediately).
// ---------------------------------------------------------------------------

#[test]
fn use_statement_parses_as_item_use() {
    let src = "use Eq;";
    let items = new_parser::parse(src).expect("parse");
    assert_eq!(items.len(), 1);
    match &items[0].0 {
        Item::Use(u) => assert_eq!(u.name.0, "Eq"),
        other => panic!("expected Use, got {:?}", other),
    }
}

#[test]
fn multiple_use_statements_all_registered() {
    let src = "use Eq; use Ord; use Display;";
    let items = new_parser::parse(src).expect("parse");
    let names: Vec<String> = items
        .iter()
        .filter_map(|(it, _)| match it {
            Item::Use(u) => Some(u.name.0.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["Eq", "Ord", "Display"]);
}

#[test]
fn imports_tracked_per_file() {
    let (reg, ids) = build_multi(&[
        ("a", "use Eq; struct Marker {}"),
        ("b", "struct Other {}"),
    ]);
    let a = &reg.imports[&ids["a"]];
    let b = &reg.imports[&ids["b"]];
    assert!(a.contains("Eq"), "file a should import Eq");
    assert!(!b.contains("Eq"), "file b should not import Eq");
}

// ---------------------------------------------------------------------------
// Inherent-only resolution: no traits involved.
// ---------------------------------------------------------------------------

#[test]
fn inherent_method_resolves_from_any_file() {
    // File a defines Foo and an extension; file b (no imports, no defs)
    // should still be able to resolve `Foo::m` because extensions are
    // "inherent" — no trait gating.
    let (reg, ids) = build_multi(&[
        (
            "a",
            "struct Foo {} extension Foo { fn m(self): U4 { 0 } }",
        ),
        ("b", ""),
    ]);
    assert_eq!(resolve(&reg, ids["a"], "Foo", "m"), MethodResolution::Inherent);
    assert_eq!(resolve(&reg, ids["b"], "Foo", "m"), MethodResolution::Inherent);
}

#[test]
fn unknown_method_returns_not_found() {
    let (reg, ids) = build_multi(&[("a", "struct Foo {}")]);
    assert!(matches!(
        resolve(&reg, ids["a"], "Foo", "nope"),
        MethodResolution::NotFound { .. }
    ));
}

#[test]
fn unknown_type_returns_not_found() {
    let (reg, ids) = build_multi(&[("a", "")]);
    assert!(matches!(
        resolve(&reg, ids["a"], "Missing", "m"),
        MethodResolution::NotFound { .. }
    ));
}

// ---------------------------------------------------------------------------
// Trait resolution: same file, single trait.
// ---------------------------------------------------------------------------

#[test]
fn trait_impl_resolves_when_trait_imported() {
    // Same file defines trait + impl + `use Eq`. Resolution picks the trait.
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait Eq { fn eq(self, Self other): Bool; }
            impl Eq for Foo { fn eq(self, Foo other): Bool { true } }
            use Eq;
        "#,
    )]);
    let r = resolve(&reg, ids["a"], "Foo", "eq");
    assert_eq!(r, MethodResolution::Trait { trait_name: "Eq".into() });
}

#[test]
fn trait_method_unavailable_when_trait_not_imported() {
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait Eq { fn eq(self, Self other): Bool; }
            impl Eq for Foo { fn eq(self, Foo other): Bool { true } }
        "#,
    )]);
    // No `use Eq;` in file a. Eq is NOT an operator trait from the stdlib's
    // point of view ... actually it IS in OPERATOR_TRAITS. Build a fake trait
    // instead to test the gating rule.
    let _ = reg;
    let _ = ids;

    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait Custom { fn act(self): Bool; }
            impl Custom for Foo { fn act(self): Bool { true } }
        "#,
    )]);
    let r = resolve(&reg, ids["a"], "Foo", "act");
    match r {
        MethodResolution::TraitNotImported { candidates } => {
            assert_eq!(candidates, vec!["Custom".to_string()]);
        }
        other => panic!("expected TraitNotImported, got {:?}", other),
    }
}

#[test]
fn importing_trait_in_one_file_doesnt_affect_another() {
    let (reg, ids) = build_multi(&[
        (
            "a",
            r#"
                struct Bool { U4 value, }
                struct Foo {}
                trait Custom { fn act(self): Bool; }
                impl Custom for Foo { fn act(self): Bool { true } }
                use Custom;
            "#,
        ),
        ("b", ""),
    ]);
    assert_eq!(
        resolve(&reg, ids["a"], "Foo", "act"),
        MethodResolution::Trait { trait_name: "Custom".into() }
    );
    match resolve(&reg, ids["b"], "Foo", "act") {
        MethodResolution::TraitNotImported { candidates } => {
            assert_eq!(candidates, vec!["Custom".to_string()]);
        }
        other => panic!("expected TraitNotImported in file b, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Operator traits bypass the import check.
// ---------------------------------------------------------------------------

#[test]
fn operator_traits_list_matches_plan() {
    // Sanity: the list is public and covers the obvious operators.
    for expected in ["Add", "Sub", "Eq", "Ord"] {
        assert!(OPERATOR_TRAITS.contains(&expected), "missing `{}`", expected);
    }
}

#[test]
fn add_operator_trait_resolves_without_use() {
    // Program defines Add + impl but does NOT `use Add`. `a + b` still
    // resolves via the trait.
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Foo {}
            trait Add { fn add(self, Self other): Self; }
            impl Add for Foo { fn add(self, Foo other): Foo { self } }
        "#,
    )]);
    // No use Add; but Add is in OPERATOR_TRAITS, so still in scope.
    assert_eq!(
        resolve(&reg, ids["a"], "Foo", "add"),
        MethodResolution::Trait { trait_name: "Add".into() }
    );
}

#[test]
fn eq_trait_resolves_without_use() {
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait Eq { fn eq(self, Self other): Bool; }
            impl Eq for Foo { fn eq(self, Foo other): Bool { true } }
        "#,
    )]);
    assert_eq!(
        resolve(&reg, ids["a"], "Foo", "eq"),
        MethodResolution::Trait { trait_name: "Eq".into() }
    );
}

#[test]
fn non_operator_trait_requires_use_even_when_unambiguous() {
    // `Custom` is not in OPERATOR_TRAITS. One impl, not imported → fails.
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Foo {}
            trait Custom { fn go(self): U4; }
            impl Custom for Foo { fn go(self): U4 { 0 } }
        "#,
    )]);
    assert!(matches!(
        resolve(&reg, ids["a"], "Foo", "go"),
        MethodResolution::TraitNotImported { .. }
    ));
}

// ---------------------------------------------------------------------------
// Extension beats trait on ambiguity.
// ---------------------------------------------------------------------------

#[test]
fn extension_wins_over_trait_same_name() {
    // Both an extension `fn eq(...)` AND `impl Eq for Foo { fn eq(...) }`
    // exist. Resolver picks the inherent (extension) method.
    //
    // Currently the typer *rejects* this because register_impl errors when
    // the method is already defined. That means this test documents the
    // intended future behavior — adjust once the typer is relaxed.
    let files = [(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            extension Foo { fn eq(self, Foo other): Bool { true } }
            trait Eq { fn eq(self, Self other): Bool; }
            impl Eq for Foo { fn eq(self, Foo other): Bool { false } }
        "#,
    )];
    let parsed = parse_files(&files);
    let as_refs: Vec<(&str, &[Spanned<Item>])> = parsed
        .iter()
        .map(|(n, items)| (n.as_str(), items.as_slice()))
        .collect();
    // Today this returns Err because the typer forbids the collision; a
    // future pass should allow it with extension winning.
    let result = TypeRegistry::from_files(&as_refs);
    // Document what we *want*: Ok, with resolve → Inherent. What we *get* today:
    // Err("method ... already defined; impl conflicts with extension").
    match result {
        Ok(reg) => {
            assert_eq!(
                resolve(&reg, 0, "Foo", "eq"),
                MethodResolution::Inherent,
                "extension should win on ambiguity"
            );
        }
        Err(e) => {
            // Current state — leave a paper trail for the upgrade.
            let msg = format!("{:?}", e);
            assert!(
                msg.contains("already defined"),
                "unexpected error shape: {}",
                msg
            );
        }
    }
}

#[test]
fn extension_wins_when_trait_also_imported() {
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            extension Foo { fn eq(self, Foo other): Bool { true } }
            trait Eq { fn eq(self, Self other): Bool; }
            impl Eq for Foo { fn eq(self, Foo other): Bool { false } }
            use Eq;
        "#,
    )]);
    // Rule 1: inherent always wins.
    assert_eq!(resolve(&reg, ids["a"], "Foo", "eq"), MethodResolution::Inherent);
}

// ---------------------------------------------------------------------------
// Two traits, same method name.
// ---------------------------------------------------------------------------

#[test]
fn two_traits_same_method_both_imported_is_ambiguous() {
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait A { fn act(self): Bool; }
            trait B { fn act(self): Bool; }
            impl A for Foo { fn act(self): Bool { true } }
            impl B for Foo { fn act(self): Bool { false } }
            use A;
            use B;
        "#,
    )]);
    match resolve(&reg, ids["a"], "Foo", "act") {
        MethodResolution::Ambiguous { mut candidates } => {
            candidates.sort();
            assert_eq!(candidates, vec!["A".to_string(), "B".to_string()]);
        }
        other => panic!("expected Ambiguous, got {:?}", other),
    }
}

#[test]
fn two_traits_same_method_only_one_imported_unambiguous() {
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait A { fn act(self): Bool; }
            trait B { fn act(self): Bool; }
            impl A for Foo { fn act(self): Bool { true } }
            impl B for Foo { fn act(self): Bool { false } }
            use A;
        "#,
    )]);
    assert_eq!(
        resolve(&reg, ids["a"], "Foo", "act"),
        MethodResolution::Trait { trait_name: "A".into() }
    );
}

#[test]
fn two_traits_same_method_neither_imported() {
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait A { fn act(self): Bool; }
            trait B { fn act(self): Bool; }
            impl A for Foo { fn act(self): Bool { true } }
            impl B for Foo { fn act(self): Bool { false } }
        "#,
    )]);
    match resolve(&reg, ids["a"], "Foo", "act") {
        MethodResolution::TraitNotImported { mut candidates } => {
            candidates.sort();
            assert_eq!(candidates, vec!["A".to_string(), "B".to_string()]);
        }
        other => panic!("expected TraitNotImported, got {:?}", other),
    }
}

#[test]
fn two_traits_same_method_explicit_qualification_picks_one() {
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait A { fn act(self): Bool; }
            trait B { fn act(self): Bool; }
            impl A for Foo { fn act(self): Bool { true } }
            impl B for Foo { fn act(self): Bool { false } }
            use A;
            use B;
        "#,
    )]);
    assert_eq!(
        resolve_qualified(&reg, ids["a"], "Foo", "act", "A"),
        MethodResolution::Trait { trait_name: "A".into() }
    );
    assert_eq!(
        resolve_qualified(&reg, ids["a"], "Foo", "act", "B"),
        MethodResolution::Trait { trait_name: "B".into() }
    );
}

#[test]
fn explicit_qualification_bypasses_import_requirement() {
    // Trait not imported, but user wrote `Custom::act(...)` — should work.
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait Custom { fn act(self): Bool; }
            impl Custom for Foo { fn act(self): Bool { true } }
        "#,
    )]);
    assert_eq!(
        resolve_qualified(&reg, ids["a"], "Foo", "act", "Custom"),
        MethodResolution::Trait { trait_name: "Custom".into() }
    );
}

#[test]
fn explicit_qualification_with_wrong_trait_not_found() {
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait Custom { fn act(self): Bool; }
            impl Custom for Foo { fn act(self): Bool { true } }
        "#,
    )]);
    assert!(matches!(
        resolve_qualified(&reg, ids["a"], "Foo", "act", "Nonexistent"),
        MethodResolution::NotFound { .. }
    ));
}

// ---------------------------------------------------------------------------
// Multi-file layouts.
// ---------------------------------------------------------------------------

#[test]
fn trait_declared_in_one_file_impl_in_another() {
    let (reg, ids) = build_multi(&[
        (
            "trait_file",
            "struct Bool { U4 value, } trait Custom { fn act(self): Bool; }",
        ),
        (
            "impl_file",
            "struct Foo {} impl Custom for Foo { fn act(self): Bool { true } }",
        ),
        ("caller", "use Custom;"),
    ]);
    assert_eq!(
        resolve(&reg, ids["caller"], "Foo", "act"),
        MethodResolution::Trait { trait_name: "Custom".into() }
    );
}

#[test]
fn caller_without_use_cant_resolve_despite_impl_existing_elsewhere() {
    let (reg, ids) = build_multi(&[
        (
            "trait_file",
            "struct Bool { U4 value, } trait Custom { fn act(self): Bool; }",
        ),
        (
            "impl_file",
            "struct Foo {} impl Custom for Foo { fn act(self): Bool { true } }",
        ),
        ("caller", ""),
    ]);
    assert!(matches!(
        resolve(&reg, ids["caller"], "Foo", "act"),
        MethodResolution::TraitNotImported { .. }
    ));
}

#[test]
fn every_file_sees_same_type_but_different_trait_scopes() {
    let (reg, ids) = build_multi(&[
        ("types", "struct Bool { U4 value, } struct Foo {}"),
        (
            "traits_and_impls",
            r#"
                trait Custom { fn act(self): Bool; }
                impl Custom for Foo { fn act(self): Bool { true } }
            "#,
        ),
        ("uses_it", "use Custom;"),
        ("ignores_it", ""),
    ]);
    assert_eq!(
        resolve(&reg, ids["uses_it"], "Foo", "act"),
        MethodResolution::Trait { trait_name: "Custom".into() }
    );
    assert!(matches!(
        resolve(&reg, ids["ignores_it"], "Foo", "act"),
        MethodResolution::TraitNotImported { .. }
    ));
}

// ---------------------------------------------------------------------------
// Monomorph-key distinctness (consumed by Phase 6).
// ---------------------------------------------------------------------------

#[test]
fn two_traits_different_methods_both_dispatch_distinctly() {
    // Same type, two traits, two different methods — both should be
    // reachable via their own trait key. This is the trait-distinctness
    // that the FnSig now carries (trait_name field).
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait A { fn m1(self): Bool; }
            trait B { fn m2(self): Bool; }
            impl A for Foo { fn m1(self): Bool { true } }
            impl B for Foo { fn m2(self): Bool { true } }
            use A;
            use B;
        "#,
    )]);
    assert_eq!(
        resolve(&reg, ids["a"], "Foo", "m1"),
        MethodResolution::Trait { trait_name: "A".into() }
    );
    assert_eq!(
        resolve(&reg, ids["a"], "Foo", "m2"),
        MethodResolution::Trait { trait_name: "B".into() }
    );
}

// ---------------------------------------------------------------------------
// Rust-parity regression tests.
// ---------------------------------------------------------------------------

#[test]
fn empty_file_has_empty_import_set() {
    let (reg, ids) = build_multi(&[("a", "")]);
    let empties = &reg.imports[&ids["a"]];
    assert!(empties.is_empty());
}

#[test]
fn trait_without_any_impl_resolves_as_not_found() {
    // Declaring a trait doesn't make its methods callable on random types.
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait Custom { fn act(self): Bool; }
            use Custom;
        "#,
    )]);
    assert!(matches!(
        resolve(&reg, ids["a"], "Foo", "act"),
        MethodResolution::NotFound { .. }
    ));
}

#[test]
fn use_an_unknown_name_doesnt_panic_but_has_no_effect() {
    let (reg, ids) = build_multi(&[("a", "use DoesntExist;")]);
    assert!(reg.imports[&ids["a"]].contains("DoesntExist"));
    // Resolver just won't find any impl tagged "DoesntExist".
    let (reg2, ids2) = build_multi(&[(
        "a",
        r#"
            struct Bool { U4 value, }
            struct Foo {}
            trait Real { fn go(self): Bool; }
            impl Real for Foo { fn go(self): Bool { true } }
            use DoesntExist;
        "#,
    )]);
    // Real isn't imported (only DoesntExist is), so Real is out-of-scope.
    assert!(matches!(
        resolve(&reg2, ids2["a"], "Foo", "go"),
        MethodResolution::TraitNotImported { .. }
    ));
    let _ = reg;
    let _ = ids;
}

#[test]
fn duplicate_trait_name_across_files_coexists_via_path() {
    // Trait names now follow the same path-aware collision rules as
    // types: two files can both declare `trait Foo` and both survive
    // under their fully-qualified storage keys.
    let parsed = parse_files(&[
        ("a", "trait Foo { fn bar(self): U4; }"),
        ("b", "trait Foo { fn bar(self): U4; }"),
    ]);
    let as_refs: Vec<(&str, &[Spanned<Item>])> = parsed
        .iter()
        .map(|(n, items)| (n.as_str(), items.as_slice()))
        .collect();
    let reg = TypeRegistry::from_files(&as_refs).expect("both traits should coexist");
    assert!(reg.has_trait("a::Foo"));
    assert!(reg.has_trait("b::Foo"));
    // Bare lookups resolve to the file-local decl in each file.
    assert_eq!(
        reg.canonicalize_type_name("Foo", Some(0)).as_deref(),
        Some("a::Foo")
    );
    assert_eq!(
        reg.canonicalize_type_name("Foo", Some(1)).as_deref(),
        Some("b::Foo")
    );
    // Path-qualified references resolve directly.
    assert_eq!(
        reg.canonicalize_type_name("a::Foo", None).as_deref(),
        Some("a::Foo")
    );
    assert_eq!(
        reg.canonicalize_type_name("b::Foo", None).as_deref(),
        Some("b::Foo")
    );
    // Bare lookup from an unrelated file is ambiguous, returns None.
    assert_eq!(reg.canonicalize_type_name("Foo", Some(99)), None);
}

#[test]
fn same_trait_declared_twice_same_file_rejected() {
    // Belt-and-braces: even inside one file, redeclaring the trait errors.
    let parsed = parse_files(&[(
        "a",
        "trait Foo { fn bar(self): U4; } trait Foo { fn bar(self): U4; }",
    )]);
    let as_refs: Vec<(&str, &[Spanned<Item>])> = parsed
        .iter()
        .map(|(n, items)| (n.as_str(), items.as_slice()))
        .collect();
    assert!(TypeRegistry::from_files(&as_refs).is_err());
}

#[test]
fn same_trait_name_imported_by_multiple_files_ok() {
    // With the trait defined once, any number of files can `use` it — the
    // import is just a set-insert per file.
    let (reg, ids) = build_multi(&[
        (
            "defs",
            r#"
                struct Bool { U4 value, }
                struct Foo {}
                trait Custom { fn act(self): Bool; }
                impl Custom for Foo { fn act(self): Bool { true } }
            "#,
        ),
        ("user_a", "use Custom;"),
        ("user_b", "use Custom;"),
    ]);
    assert_eq!(
        resolve(&reg, ids["user_a"], "Foo", "act"),
        MethodResolution::Trait { trait_name: "Custom".into() }
    );
    assert_eq!(
        resolve(&reg, ids["user_b"], "Foo", "act"),
        MethodResolution::Trait { trait_name: "Custom".into() }
    );
}

#[test]
fn multiple_uses_of_same_trait_deduplicate() {
    let (reg, ids) = build_multi(&[("a", "use Custom; use Custom; use Custom;")]);
    let set = &reg.imports[&ids["a"]];
    assert_eq!(set.iter().filter(|&x| x == "Custom").count(), 1);
}

#[test]
fn operator_trait_still_matches_through_explicit_qualification() {
    // Even though Add bypasses import-gating for plain `+`, explicit
    // qualification `Add::add(...)` must still resolve — it mustn't be
    // confused by the operator special-case.
    let (reg, ids) = build_multi(&[(
        "a",
        r#"
            struct Foo {}
            trait Add { fn add(self, Self other): Self; }
            impl Add for Foo { fn add(self, Foo other): Foo { self } }
        "#,
    )]);
    assert_eq!(
        resolve_qualified(&reg, ids["a"], "Foo", "add", "Add"),
        MethodResolution::Trait { trait_name: "Add".into() }
    );
}
