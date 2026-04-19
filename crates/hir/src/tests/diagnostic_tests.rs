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
// E0013 — wrong argument count (method + static call)
// =========================================================================
#[test]
fn e0013_method_too_few_args() {
    // Define a type locally so the registry sees its methods
    // without needing the full stdlib loaded.
    let src = r#"
        struct Box { U4 v, }
        extension Box {
            fn add(mut self, U4 n): U4 { 0 }
            fn go(mut self): U4 {
                self.add();
                0
            }
        }
    "#;
    let diag = hir_diagnostic(src, "Box", "go");
    assert_diag(&diag, Severity::Error, codes::E_WRONG_ARG_COUNT);
    assert!(
        diag.message.contains("takes 1 argument"),
        "header should name the expected count; got {:?}",
        diag.message
    );
    assert!(
        diag.message.contains("0 were supplied"),
        "header should name the actual count; got {:?}",
        diag.message
    );
    assert!(
        diag.helps.iter().any(|h| h.contains("signature")),
        "expected a signature help; got {:?}",
        diag.helps
    );
    assert!(
        diag.helps.iter().any(|h| h.contains("supply")),
        "expected an actionable supply-more help; got {:?}",
        diag.helps
    );
    assert_eq!(primary_count(&diag), 1);
}

#[test]
fn e0013_method_too_many_args() {
    let src = r#"
        struct Box { U4 v, }
        extension Box {
            fn add(mut self, U4 n): U4 { 0 }
            fn go(mut self): U4 {
                self.add(1, 2);
                0
            }
        }
    "#;
    let diag = hir_diagnostic(src, "Box", "go");
    assert_diag(&diag, Severity::Error, codes::E_WRONG_ARG_COUNT);
    assert!(
        diag.helps.iter().any(|h| h.contains("remove")),
        "expected a remove-extras help; got {:?}",
        diag.helps
    );
}

// =========================================================================
// E0010 — argument type mismatch
// =========================================================================
#[test]
fn e0010_wrong_arg_type_struct_vs_u4() {
    let src = r#"
        struct Bar { U4 v, }
        extension Bar {
            fn takesBar(self, Bar other): U4 { 0 }
            fn go(self): U4 {
                U4 i = 0;
                self.takesBar(i);
                0
            }
        }
    "#;
    let diag = hir_diagnostic(src, "Bar", "go");
    assert_diag(&diag, Severity::Error, codes::E_TYPE_MISMATCH);
    assert!(
        diag.message.contains("expected `Bar`"),
        "message should name expected type; got {:?}",
        diag.message
    );
    assert!(
        diag.message.contains("found `U4`"),
        "message should name found type; got {:?}",
        diag.message
    );
    assert!(
        diag.notes.iter().any(|n| n.contains("declares")),
        "expected an explanatory note; got {:?}",
        diag.notes
    );
    assert!(
        !diag.helps.is_empty(),
        "expected at least one help; got {:?}",
        diag.helps
    );
}

#[test]
fn e0010_number_literal_overflows_u4() {
    let src = r#"
        struct Box { U4 v, }
        extension Box {
            fn take(self, U4 n): U4 { 0 }
            fn go(self): U4 {
                self.take(87);
                0
            }
        }
    "#;
    let diag = hir_diagnostic(src, "Box", "go");
    assert_diag(&diag, Severity::Error, codes::E_TYPE_MISMATCH);
    assert!(
        diag.message.contains("87") && diag.message.contains("U4"),
        "should cite the value and the type; got {:?}",
        diag.message
    );
    assert!(
        diag.notes.iter().any(|n| n.contains("4-bit")),
        "expected a note about the cell width; got {:?}",
        diag.notes
    );
    assert!(
        diag.helps.iter().any(|h| h.contains("U8") || h.contains("clamp")),
        "expected a widen-or-clamp help; got {:?}",
        diag.helps
    );
}

// =========================================================================
// E0016 — undefined variable with typo suggestion
// =========================================================================
#[test]
fn e0016_undefined_variable_suggests_similar() {
    let src = r#"
        extension U4 {
            fn go(): U4 {
                U4 caca = 1;
                cac
            }
        }
    "#;
    let diag = hir_diagnostic(src, "U4", "go");
    assert_diag(&diag, Severity::Error, codes::E_UNKNOWN_VARIABLE);
    assert!(
        diag.message.contains("`cac`"),
        "should name the missing variable; got {:?}",
        diag.message
    );
    assert!(
        diag.helps.iter().any(|h| h.contains("`caca`")),
        "expected a `did you mean caca?` help; got {:?}",
        diag.helps
    );
}

#[test]
fn e0016_undefined_variable_no_bindings_suggests_declaration() {
    let src = r#"
        extension U4 {
            fn go(): U4 {
                x
            }
        }
    "#;
    let diag = hir_diagnostic(src, "U4", "go");
    assert_diag(&diag, Severity::Error, codes::E_UNKNOWN_VARIABLE);
    // No bindings in scope — help should point at the declaration shape.
    assert!(
        diag.helps.iter().any(|h| h.contains("declare")),
        "expected a declare-it help; got {:?}",
        diag.helps
    );
}

// =========================================================================
// E0014 — unknown method with typo suggestion
// =========================================================================
#[test]
fn e0014_unknown_method_suggests_similar() {
    let src = r#"
        struct Box { U4 v, }
        extension Box {
            fn get(self): U4 { self.v }
            fn go(self): U4 {
                self.gat();
                0
            }
        }
    "#;
    let diag = hir_diagnostic(src, "Box", "go");
    assert_diag(&diag, Severity::Error, codes::E_UNKNOWN_METHOD);
    assert!(
        diag.helps.iter().any(|h| h.contains("`get`")),
        "expected a `did you mean get?` help; got {:?}",
        diag.helps
    );
}

// =========================================================================
// E0015 — missing / duplicate / unknown field in struct literal
// =========================================================================
#[test]
fn e0015_missing_fields_in_struct_literal() {
    let src = r#"
        struct Pair { U4 a, U4 b, }
        extension U4 {
            fn go(): U4 {
                Pair p = Pair { a: 1 };
                p.a
            }
        }
    "#;
    let diag = hir_diagnostic(src, "U4", "go");
    assert_diag(&diag, Severity::Error, codes::E_UNKNOWN_FIELD);
    assert!(
        diag.message.contains("`b`"),
        "should name the missing field; got {:?}",
        diag.message
    );
    assert!(
        diag.helps.iter().any(|h| h.contains("add `b:")),
        "expected an actionable add-missing help; got {:?}",
        diag.helps
    );
}

#[test]
fn e0015_duplicate_field_in_struct_literal() {
    let src = r#"
        struct Pair { U4 a, U4 b, }
        extension U4 {
            fn go(): U4 {
                Pair p = Pair { a: 1, a: 2, b: 3 };
                p.a
            }
        }
    "#;
    let diag = hir_diagnostic(src, "U4", "go");
    assert_diag(&diag, Severity::Error, codes::E_UNKNOWN_FIELD);
    assert!(
        diag.message.contains("twice"),
        "should flag the duplicate; got {:?}",
        diag.message
    );
}

#[test]
fn e0015_unknown_field_in_struct_literal_suggests_similar() {
    // Include every required field + one typo'd one, so we hit
    // the unknown-field path rather than the missing-fields
    // path.
    let src = r#"
        struct Triple { U4 first, U4 second, U4 third, }
        extension U4 {
            fn go(): U4 {
                Triple p = Triple { first: 1, second: 2, third: 3, frist: 0 };
                p.first
            }
        }
    "#;
    let diag = hir_diagnostic(src, "U4", "go");
    assert_diag(&diag, Severity::Error, codes::E_UNKNOWN_FIELD);
    // Either a duplicate-field hit (p.first twice) or a typo
    // hit — both happen here because `frist` isn't in the
    // struct. We rely on the implementation order: duplicate
    // check runs before per-field lookups, so this actually
    // fires the unknown-field branch.
    assert!(
        diag.message.contains("unknown") || diag.message.contains("no field"),
        "expected an unknown-field error; got {:?}",
        diag.message
    );
    assert!(
        diag.helps.iter().any(|h| h.contains("`first`")),
        "expected a `did you mean first?` help; got {:?}",
        diag.helps
    );
}

// =========================================================================
// E0002 — unknown trait in impl
// =========================================================================
#[test]
fn e0002_unknown_trait_in_impl() {
    let diag = typer_diagnostic(&[(
        "a.ct",
        "struct Foo { U4 v, }\nimpl NoSuchTrait for Foo { }\n",
    )]);
    assert_diag(&diag, Severity::Error, codes::E_UNKNOWN_TRAIT);
    assert!(
        diag.message.contains("NoSuchTrait"),
        "message should name the trait; got {:?}",
        diag.message
    );
    assert!(
        diag.helps.iter().any(|h| h.contains("trait") || h.contains("use")),
        "expected an actionable help; got {:?}",
        diag.helps
    );
}

// =========================================================================
// E0012 — impl of trait missing required method
// =========================================================================
#[test]
fn e0012_impl_missing_method() {
    let diag = typer_diagnostic(&[(
        "a.ct",
        "trait HasFoo { fn foo(self): U4; fn bar(self): U4; }\n\
         struct Thing { U4 v, }\n\
         impl HasFoo for Thing { fn foo(self): U4 { 0 } }\n",
    )]);
    assert_diag(&diag, Severity::Error, codes::E_MISSING_IMPL);
    assert!(
        diag.message.contains("`bar`"),
        "should name the missing method; got {:?}",
        diag.message
    );
    assert!(
        diag.helps.iter().any(|h| h.contains("fn bar")),
        "expected a stub-body help; got {:?}",
        diag.helps
    );
}

// =========================================================================
// E0005 — duplicate inherent method
// =========================================================================
#[test]
fn e0005_duplicate_method() {
    let diag = typer_diagnostic(&[(
        "a.ct",
        "struct Foo { U4 v, }\n\
         extension Foo {\n\
             fn dup(self): U4 { 1 }\n\
             fn dup(self): U4 { 2 }\n\
         }\n",
    )]);
    assert_diag(&diag, Severity::Error, codes::E_DUPLICATE_METHOD);
    assert!(
        diag.message.contains("`Foo::dup`"),
        "should name the method; got {:?}",
        diag.message
    );
    assert_eq!(secondary_count(&diag), 1, "expected a first-defined-here label");
    assert!(
        diag.helps.iter().any(|h| h.contains("rename") || h.contains("merge")),
        "expected a rename/merge help; got {:?}",
        diag.helps
    );
}

// =========================================================================
// E0017 — unknown enum variant
// =========================================================================
#[test]
fn e0017_unknown_variant_suggests_similar() {
    let src = r#"
        enum Color { Red, Green, Blue, }
        extension Color {
            fn go(self): U4 {
                Color c = Color::Reed;
                0
            }
        }
    "#;
    let diag = hir_diagnostic(src, "Color", "go");
    assert_diag(&diag, Severity::Error, codes::E_UNKNOWN_VARIANT);
    assert!(
        diag.helps.iter().any(|h| h.contains("`Red`")),
        "expected `did you mean Red?` help; got {:?}",
        diag.helps
    );
}

// =========================================================================
// E0018 — `break` / `continue` outside any enclosing loop
// =========================================================================
#[test]
fn e0018_break_outside_loop() {
    let src = r#"
        extension U4 {
            fn go(): U4 {
                break;
                0
            }
        }
    "#;
    let diag = hir_diagnostic(src, "U4", "go");
    assert_diag(&diag, Severity::Error, codes::E_CONTROL_FLOW);
    assert!(
        diag.message.contains("`break`"),
        "should name the keyword; got {:?}",
        diag.message
    );
    assert!(
        diag.helps.iter().any(|h| h.contains("loop")),
        "expected a loop help; got {:?}",
        diag.helps
    );
    assert!(
        diag.notes.iter().any(|n| n.contains("enclosing")),
        "expected an explanatory note; got {:?}",
        diag.notes
    );
}

#[test]
fn e0018_continue_outside_loop() {
    let src = r#"
        extension U4 {
            fn go(): U4 {
                continue;
                0
            }
        }
    "#;
    let diag = hir_diagnostic(src, "U4", "go");
    assert_diag(&diag, Severity::Error, codes::E_CONTROL_FLOW);
    assert!(
        diag.message.contains("`continue`"),
        "should name the keyword; got {:?}",
        diag.message
    );
}

// =========================================================================
// E0015 — duplicate struct field declaration (not just in literal)
// =========================================================================
#[test]
fn e0015_duplicate_field_in_struct_def() {
    let diag = typer_diagnostic(&[(
        "a.ct",
        "struct Foo { U4 v, U4 v, }\n",
    )]);
    assert_diag(&diag, Severity::Error, codes::E_UNKNOWN_FIELD);
    assert!(
        diag.message.contains("duplicate field `v`"),
        "should name the field; got {:?}",
        diag.message
    );
    assert_eq!(
        secondary_count(&diag),
        1,
        "expected a first-declared-here label"
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
