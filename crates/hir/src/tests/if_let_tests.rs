//! End-to-end tests for `if let PAT = EXPR { THEN } ( else ELSE )?`.
//!
//! `if let` desugars to a `match` at parse time — no new AST node. Each
//! test confirms that the desugar produces the right runtime behaviour
//! for a common enum-pattern use case.

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

fn run_program(extra: &str, entry: &typer::FnSig) -> Vec<u8> {
    let (reg, db, fns) = compile_program(extra);
    let inlined = inline_program_full(&fns, entry, Some(&reg), Some(&db)).expect("inline");
    let mir_block = hir_to_mir(&inlined.body).expect("mir");
    struct Null;
    impl mir::RunContext for Null {
        fn input(&mut self) -> u8 { 0 }
        fn print(&mut self, _: char) {}
    }
    let mut state = mir::MemoryState::new_with_limit((inlined.slot_count as usize + 64).max(256), 4, 5_000_000);
    let mut ctx = Null;
    state.execute_block(&mir_block, &mut ctx);
    let input_count = inlined.sig.input_count as usize;
    let output_count = inlined.sig.output_count as usize;
    (0..output_count)
        .map(|i| state.get_mem((input_count + i) as u32))
        .collect()
}

// =========================================================================
// Basic `if let` on Option<U4> — Some path extracts the payload.
// =========================================================================

#[test]
fn if_let_option_some_extracts_payload() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option::some(7);
                if let Option::Some(v) = o {
                    v
                } else {
                    0
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// `if let` on Option::None — fall through to else.
// =========================================================================

#[test]
fn if_let_option_none_takes_else() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option::none();
                if let Option::Some(v) = o {
                    v
                } else {
                    9
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![9]);
}

// =========================================================================
// `if let` with a custom enum.
// =========================================================================

#[test]
fn if_let_custom_enum_with_payload() {
    let src = r#"
        enum Msg {
            Text(U4),
            Silent,
        }
        extension U4 {
            fn test(): U4 {
                Msg m = Msg::Text(5);
                if let Msg::Text(n) = m {
                    n + 1
                } else {
                    0
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![6]);
}

// =========================================================================
// `if let` without an else branch — sets dst on match, leaves it as-is
// on no-match.
// =========================================================================

#[test]
fn if_let_without_else_as_statement() {
    // Used for side effects: update a local iff the pattern matches.
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option::some(4);
                mut U4 out = 0;
                if let Option::Some(v) = o {
                    out = v;
                }
                out
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![4]);
}

#[test]
fn if_let_without_else_none_leaves_fallback() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option::none();
                mut U4 out = 9;
                if let Option::Some(v) = o {
                    out = v;
                }
                out
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![9]);
}

// =========================================================================
// `if let` binding is usable in the body of the then-branch.
// =========================================================================

#[test]
fn if_let_binding_shadows_outer_name() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                U4 v = 1;
                Option<U4> o = Option::some(8);
                if let Option::Some(v) = o {
                    v
                } else {
                    v
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![8]);
}

// =========================================================================
// Nested `if let` inside another `if let`'s then-branch.
// =========================================================================

#[test]
fn nested_if_let() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<Option<U4>> oo = Option::some(Option::some(5));
                if let Option::Some(inner) = oo {
                    if let Option::Some(n) = inner {
                        n
                    } else {
                        0
                    }
                } else {
                    0
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![5]);
}

// =========================================================================
// `else if let` chain — parsing accepts the else's rhs as another if let.
// =========================================================================

#[test]
fn else_if_let_chain() {
    let src = r#"
        enum Three {
            A(U4),
            B(U4),
            C,
        }
        extension U4 {
            fn test(): U4 {
                Three t = Three::B(7);
                if let Three::A(n) = t {
                    n
                } else if let Three::B(n) = t {
                    n + 1
                } else {
                    0
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![8]);
}

// =========================================================================
// `if let` inside a loop, break out when the pattern matches.
// =========================================================================

#[test]
fn if_let_inside_loop_break_on_match() {
    let src = r#"
        use Option;
        extension U4 {
            fn pull(U4 i): Option<U4> {
                if i == 3 {
                    Option::some(10)
                } else {
                    Option<U4>::none()
                }
            }
            fn test(): U4 {
                mut U4 i = 0;
                mut U4 got = 0;
                loop {
                    if let Option::Some(v) = U4::pull(i) {
                        got = v;
                        break;
                    }
                    i += 1;
                    if i == 10 { break; }
                }
                got
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // i hits 3, pull returns Some(42) — but 42 clamps to u4 low nibble (10).
    // Actually: values are 4-bit. 42 as u4 = 42 % 16 = 10.
    assert_eq!(run_program(src, &entry), vec![10]);
}

// =========================================================================
// `if let` with a wildcard binding `_` — pattern matches but payload is
// discarded.
// =========================================================================

#[test]
fn if_let_with_wildcard_binding() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option::some(3);
                if let Option::Some(_) = o { 1 } else { 0 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![1]);
}

// =========================================================================
// `if let` on a unit variant (no payload) — binding is omitted.
// =========================================================================

#[test]
fn if_let_on_unit_variant() {
    let src = r#"
        enum Flag { On, Off, }
        extension U4 {
            fn test(): U4 {
                Flag f = Flag::On;
                if let Flag::On = f { 7 } else { 0 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// `if let` on a multi-cell payload.
// =========================================================================

#[test]
fn if_let_on_multi_cell_payload() {
    let src = r#"
        enum Wrap { Big(U8), None_, }
        extension U4 {
            fn test(): U4 {
                Wrap w = Wrap::Big(U8 { lower: 3, higher: 5, });
                if let Wrap::Big(u) = w {
                    u.higher
                } else {
                    0
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![5]);
}

// =========================================================================
// `if let` result composed with arithmetic outside the conditional.
// =========================================================================

#[test]
fn if_let_result_used_in_arithmetic() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option::some(2);
                U4 base = if let Option::Some(v) = o { v } else { 0 };
                base + 5
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}
