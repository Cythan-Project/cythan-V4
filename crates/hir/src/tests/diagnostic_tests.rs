//! Tests for rich, structured compiler diagnostics.
//!
//! Each test covers one error kind and asserts on:
//!   * the stable `DiagCode` (so LSP clients and docs can link to it)
//!   * the shape of labels (primary vs secondary) where applicable
//!   * the presence of notes/helps where we promise them
//!
//! The tests drive the typer + HIR gen directly so we don't depend on
//! the full compile-and-run harness, which keeps failures tightly
//! localised to "which diagnostic was produced".

use std::collections::HashMap;

use errors::{codes, Diagnostic, LabelKind, Severity};

fn parse(src: &str) -> Vec<new_parser::ast::Spanned<new_parser::ast::Item>> {
    new_parser::parse(src).expect("parse")
}

/// Build a registry from one or more named files and return any typer
/// diagnostic — the first error that came out of `from_files`.
fn typer_diagnostic(files: &[(&str, &str)]) -> Diagnostic {
    let parsed: Vec<(String, Vec<_>)> = files
        .iter()
        .map(|(n, s)| (n.to_string(), parse(s)))
        .collect();
    let as_refs: Vec<(&str, &[_])> = parsed.iter().map(|(n, v)| (n.as_str(), v.as_slice())).collect();
    let errs = typer::TypeRegistry::from_files(&as_refs).expect_err("expected typer error");
    let first = errs.into_iter().next().expect("at least one error");
    first.into_diagnostic("<synthetic>")
}

/// Compile an HIR for `(type, method)` and capture the first HIR
/// error. Panics if compilation succeeds.
fn hir_diagnostic(src: &str, ty: &str, method: &str) -> Diagnostic {
    let items = parse(src);
    let reg = typer::TypeRegistry::from_items(&items).expect("typer");
    let db = typer::FunctionDB::from_registry(&reg).expect("fn_db");
    let natives = crate::BuiltinNatives::new();
    let key = typer::FnSig::new(ty, method);
    let s = match db.get(&key) {
        Some(typer::Fn::Simple(s)) => s.clone(),
        _ => panic!("{}::{} is not a Simple function in the DB", ty, method),
    };
    let err = crate::gen_function_with_natives(&key, &s, &reg, &db, Some(&natives))
        .expect_err("expected HIR error");
    err.into_diagnostic("<synthetic>")
}

fn assert_diag(diag: &Diagnostic, severity: Severity, code: errors::DiagCode) {
    assert_eq!(diag.severity, severity, "severity mismatch for {:?}", diag);
    assert_eq!(
        diag.code.as_ref(),
        Some(&code),
        "code mismatch; diag = {:#?}",
        diag
    );
}

fn primary_count(diag: &Diagnostic) -> usize {
    diag.labels.iter().filter(|l| l.kind == LabelKind::Primary).count()
}
fn secondary_count(diag: &Diagnostic) -> usize {
    diag.labels.iter().filter(|l| l.kind == LabelKind::Secondary).count()
}

// =========================================================================
// E0003 — duplicate type definition (within one file)
// =========================================================================
#[test]
fn e0003_duplicate_struct_within_file() {
    let diag = typer_diagnostic(&[(
        "a.ct",
        "struct Foo { U4 v, }\nstruct Foo { U4 w, }\n",
    )]);
    assert_diag(&diag, Severity::Error, codes::E_DUPLICATE_TYPE);
    assert_eq!(primary_count(&diag), 1, "one primary label expected");
    assert_eq!(
        secondary_count(&diag),
        1,
        "secondary label should point at the first definition"
    );
    let secondary = diag
        .labels
        .iter()
        .find(|l| l.kind == LabelKind::Secondary)
        .unwrap();
    assert!(
        secondary.message.contains("first"),
        "secondary label should say 'first defined here'; got: {:?}",
        secondary.message
    );
    assert!(
        diag.notes.iter().any(|n| n.contains("once per scope")),
        "expected an explanatory note"
    );
    assert!(
        !diag.helps.is_empty(),
        "expected at least one help suggestion"
    );
}

// =========================================================================
// E0004 — duplicate trait definition (within one file)
// =========================================================================
#[test]
fn e0004_duplicate_trait_within_file() {
    let diag = typer_diagnostic(&[(
        "a.ct",
        "trait Foo { fn bar(self): U4; }\ntrait Foo { fn bar(self): U4; }\n",
    )]);
    assert_diag(&diag, Severity::Error, codes::E_DUPLICATE_TRAIT);
    assert_eq!(primary_count(&diag), 1);
    assert_eq!(secondary_count(&diag), 1);
}

// =========================================================================
// E0001 — unknown type with "did you mean?" suggestion
// =========================================================================
#[test]
fn e0001_unknown_type_field_access_suggests_similar() {
    // `Fooo` looks like `Foo` — the suggester should pick it up.
    let src = r#"
        struct Foo { U4 v, }
        extension U4 {
            fn go(): U4 {
                Fooo f = Foo { v: 1, };
                f.v
            }
        }
    "#;
    let diag = hir_diagnostic(src, "U4", "go");
    assert_diag(&diag, Severity::Error, codes::E_UNKNOWN_TYPE);
    assert!(
        diag.helps.iter().any(|h| h.contains("Foo")),
        "did-you-mean suggestion for `Foo` missing; helps={:?}",
        diag.helps
    );
}

// =========================================================================
// E0011 — mutability violation
// =========================================================================
#[test]
fn e0011_assign_to_immutable_local() {
    let src = r#"
        extension U4 {
            fn go(): U4 {
                U4 x = 5;
                x = 6;
                x
            }
        }
    "#;
    let diag = hir_diagnostic(src, "U4", "go");
    assert_diag(&diag, Severity::Error, codes::E_MUTABILITY);
    assert!(
        diag.helps.iter().any(|h| h.contains("mut")),
        "expected `consider declaring ... mut` help; got {:?}",
        diag.helps
    );
    assert_eq!(primary_count(&diag), 1);
}

// =========================================================================
// E0015 — unknown field with suggestion
// =========================================================================
#[test]
fn e0015_unknown_field_suggests_similar() {
    let src = r#"
        struct Pair { U4 first, U4 second, }
        extension U4 {
            fn go(): U4 {
                Pair p = Pair { first: 1, second: 2, };
                p.frist
            }
        }
    "#;
    let diag = hir_diagnostic(src, "U4", "go");
    assert_diag(&diag, Severity::Error, codes::E_UNKNOWN_FIELD);
    assert!(
        diag.helps.iter().any(|h| h.contains("first")),
        "expected `did you mean first?` help; got {:?}",
        diag.helps
    );
}

// =========================================================================
// W0001 — unused variable warning (and its `_name` suppression)
// =========================================================================
fn compile_fn_warnings(src: &str, ty: &str, method: &str) -> Vec<Diagnostic> {
    let items = parse(src);
    let reg = typer::TypeRegistry::from_items(&items).expect("typer");
    let db = typer::FunctionDB::from_registry(&reg).expect("fn_db");
    let natives = crate::BuiltinNatives::new();
    let key = typer::FnSig::new(ty, method);
    let s = match db.get(&key) {
        Some(typer::Fn::Simple(s)) => s.clone(),
        _ => panic!("{}::{} not Simple", ty, method),
    };
    let hir = crate::gen_function_with_natives(&key, &s, &reg, &db, Some(&natives)).expect("hir");
    hir.warnings
}

#[test]
fn w0001_unused_local_warns() {
    let warnings = compile_fn_warnings(
        r#"
            extension U4 {
                fn go(): U4 {
                    U4 unused = 5;
                    U4 used = 3;
                    used
                }
            }
        "#,
        "U4",
        "go",
    );
    let unused_warn = warnings
        .iter()
        .find(|d| d.code == Some(codes::W_UNUSED_VARIABLE))
        .expect("expected W0001 warning");
    assert_eq!(unused_warn.severity, Severity::Warning);
    assert!(
        unused_warn.message.contains("unused variable"),
        "header mismatch: {:?}",
        unused_warn
    );
    assert!(
        unused_warn.message.contains("`unused`"),
        "should name the variable: {:?}",
        unused_warn
    );
    // `used` is read and should NOT warn.
    assert!(
        !warnings.iter().any(|d| d.message.contains("`used`")),
        "`used` should not be flagged; warnings={:#?}",
        warnings
    );
    // Help suggests the `_name` trick.
    assert!(
        unused_warn.helps.iter().any(|h| h.contains("_unused")),
        "expected _name suppression help"
    );
}

#[test]
fn w0001_underscore_prefix_suppresses_warning() {
    // `_name` means "I know it's unused" — the lint must be silent.
    let warnings = compile_fn_warnings(
        r#"
            extension U4 {
                fn go(): U4 {
                    U4 _discarded = 5;
                    3
                }
            }
        "#,
        "U4",
        "go",
    );
    assert!(
        warnings.is_empty(),
        "`_discarded` should not warn; got {:#?}",
        warnings
    );
}

// =========================================================================
// Rendering — the plain-text form includes the code and labels.
// Guards the shape CLIs and LSPs will serialize.
// =========================================================================
#[test]
fn render_plain_has_stable_fields() {
    let diag = typer_diagnostic(&[(
        "a.ct",
        "struct Foo { U4 v, }\nstruct Foo { U4 w, }\n",
    )]);
    let txt = errors::render_plain(&diag);
    assert!(txt.starts_with("error[E0003]:"), "header missing: {}", txt);
    assert!(txt.contains("duplicate definition"), "message missing: {}", txt);
    assert!(txt.contains("-->"), "primary label marker missing: {}", txt);
}

#[test]
fn render_ariadne_produces_non_empty_text() {
    let diag = typer_diagnostic(&[(
        "a.ct",
        "struct Foo { U4 v, }\nstruct Foo { U4 w, }\n",
    )]);
    let mut sources = HashMap::new();
    sources.insert(
        "a.ct".to_string(),
        "struct Foo { U4 v, }\nstruct Foo { U4 w, }\n".to_string(),
    );
    let txt = errors::render_diag(&diag, &sources);
    assert!(!txt.is_empty());
    // Ariadne colors each source character independently with ANSI
    // escapes, so individual words aren't contiguous substrings of
    // the raw output. Instead we check distinctive rendered phrases
    // (the message is emitted whole, un-colored).
    assert!(
        txt.contains("duplicate definition"),
        "rendered diagnostic should carry the header message; got:\n{}",
        txt
    );
    assert!(
        txt.contains("first defined here"),
        "rendered diagnostic should carry the secondary label; got:\n{}",
        txt
    );
    assert!(
        txt.contains("a.ct"),
        "rendered diagnostic should cite the file; got:\n{}",
        txt
    );
}
