//! Additional edge-case tests. One-off scenarios that exercise corners
//! of the language — enums with non-trivial payloads, match in unusual
//! positions, patterns feeding downstream arithmetic, and feature
//! interactions (match + generics, if-let + operators, etc.).

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
// Match with many variants and a wildcard catch-all at the end.
// =========================================================================

#[test]
fn match_five_variants_with_wildcard() {
    let src = r#"
        enum Color { Red, Green, Blue, Yellow, Purple, }
        extension U4 {
            fn code(Color c): U4 {
                match c {
                    Color::Red => 1,
                    Color::Green => 2,
                    Color::Blue => 3,
                    _ => 9,
                }
            }
            fn test(): U4 {
                U4::code(Color::Purple) + U4::code(Color::Green)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![11]);
}

// =========================================================================
// Enum with explicit discriminant values.
// =========================================================================

#[test]
fn match_enum_with_explicit_discriminants() {
    let src = r#"
        enum Tag {
            A = 1,
            B = 3,
            C = 7,
        }
        extension U4 {
            fn test(): U4 {
                Tag t = Tag::B;
                match t {
                    Tag::A => 10,
                    Tag::B => 5,
                    Tag::C => 0,
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![5]);
}

// =========================================================================
// Enum variant with a struct payload — match extracts and reads fields.
// =========================================================================

#[test]
fn enum_variant_with_struct_payload() {
    let src = r#"
        struct Pt { U4 x, U4 y, }
        enum Shape {
            Dot(Pt),
            Origin,
        }
        extension U4 {
            fn test(): U4 {
                Shape s = Shape::Dot(Pt { x: 2, y: 5, });
                match s {
                    Shape::Dot(p) => p.x + p.y,
                    Shape::Origin => 0,
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// Match used as a sub-expression inside arithmetic.
// =========================================================================

#[test]
fn match_result_in_arithmetic() {
    let src = r#"
        enum Sign { Plus, Minus, }
        extension U4 {
            fn test(): U4 {
                Sign s = Sign::Plus;
                match s {
                    Sign::Plus => 3,
                    Sign::Minus => 1,
                } + 4
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// Match inside a mut-self method — the match's arms mutate self.
// =========================================================================

#[test]
fn match_arms_mutate_self() {
    let src = r#"
        enum Op { Inc, Dec, }
        struct Counter { U4 v, }
        extension Counter {
            fn new(): Self { Self { v: 5, } }
            fn apply(mut self, Op op) {
                match op {
                    Op::Inc => { self.v += 1; },
                    Op::Dec => { self.v -= 1; },
                }
            }
        }
        extension U4 {
            fn test(): U4 {
                mut Counter c = Counter::new();
                c.apply(Op::Inc);
                c.apply(Op::Inc);
                c.apply(Op::Dec);
                c.v
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![6]);
}

// =========================================================================
// `if let` whose scrutinee is a method call.
// =========================================================================

#[test]
fn if_let_scrutinee_is_method_call() {
    let src = r#"
        use Option;
        extension U4 {
            fn maybe(self): Option<U4> {
                if self == 0 {
                    Option<U4>::none()
                } else {
                    Option::some(self + 1)
                }
            }
            fn test(): U4 {
                U4 x = 3;
                if let Option::Some(v) = x.maybe() { v } else { 9 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![4]);
}

// =========================================================================
// `if let` whose scrutinee is a field access on a struct.
// =========================================================================

#[test]
fn if_let_scrutinee_is_field_access() {
    let src = r#"
        use Option;
        struct Holder { Option<U4> maybe, U4 tag, }
        extension U4 {
            fn test(): U4 {
                Holder h = Holder {
                    maybe: Option::some(6),
                    tag: 3,
                };
                if let Option::Some(v) = h.maybe {
                    v + h.tag
                } else {
                    0
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![9]);
}

// =========================================================================
// Nested match: outer enum's payload arm is itself matched.
// =========================================================================

#[test]
fn nested_match_with_early_return() {
    let src = r#"
        use Option;
        enum Msg { Inner(Option<U4>), Unknown, }
        extension U4 {
            fn dispatch(Msg m): U4 {
                match m {
                    Msg::Inner(o) => match o {
                        Option::Some(v) => v,
                        Option::None => 7,
                    },
                    Msg::Unknown => 0,
                }
            }
            fn test(): U4 {
                U4::dispatch(Msg::Inner(Option::some(3)))
                    + U4::dispatch(Msg::Inner(Option<U4>::none()))
                    + U4::dispatch(Msg::Unknown)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 3 + 7 + 0 = 10
    assert_eq!(run_program(src, &entry), vec![10]);
}

// =========================================================================
// Match returning multi-cell payload (U8). All arms must write both cells.
// =========================================================================

#[test]
fn match_arms_return_multi_cell_payload() {
    let src = r#"
        enum Which { A, B, }
        extension U4 {
            fn test(): U4 {
                Which w = Which::B;
                U8 u = match w {
                    Which::A => U8 { lower: 1, higher: 2, },
                    Which::B => U8 { lower: 3, higher: 4, },
                };
                u.higher + u.lower
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// Option<Pair<U4>> — generic enum carrying a generic struct payload.
// =========================================================================

#[test]
fn option_holding_generic_struct_payload() {
    let src = r#"
        use Option;
        struct Pair<T> { T a, T b, }
        extension Pair<T> {
            fn new(T a, T b): Self { Self { a: a, b: b, } }
        }
        extension U4 {
            fn test(): U4 {
                Option<Pair<U4>> op = Option::some(Pair::new(3, 4));
                match op {
                    Option::Some(p) => p.a + p.b,
                    Option::None => 0,
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// Generic function that matches on a generic enum.
// =========================================================================

#[test]
fn generic_function_matches_generic_enum() {
    let src = r#"
        use Option;
        extension U4 {
            fn or_default<T>(Option<T> o, T dflt): T {
                if let Option::Some(v) = o { v } else { dflt }
            }
            fn test(): U4 {
                U4 a = U4::or_default<U4>(Option::some(6), 0);
                U4 b = U4::or_default<U4>(Option<U4>::none(), 3);
                a + b
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![9]);
}

// =========================================================================
// Struct containing an Option field — both variants used via distinct
// instances.
// =========================================================================

#[test]
fn struct_with_option_field_two_instances() {
    let src = r#"
        use Option;
        struct Maybe { Option<U4> inner, }
        extension U4 {
            fn peek(Maybe m, U4 dflt): U4 {
                if let Option::Some(v) = m.inner { v } else { dflt }
            }
            fn test(): U4 {
                Maybe a = Maybe { inner: Option::some(5), };
                Maybe b = Maybe { inner: Option<U4>::none(), };
                U4::peek(a, 0) + U4::peek(b, 2)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// Match with a payload binding used in a compound expression, not just
// returned directly.
// =========================================================================

#[test]
fn match_binding_used_in_arithmetic_inside_arm() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option::some(4);
                match o {
                    Option::Some(v) => v + v + 1,
                    Option::None => 0,
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![9]);
}

// =========================================================================
// Multiple `if let` statements in sequence updating a shared local.
// =========================================================================

#[test]
fn multiple_if_lets_in_sequence() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                mut U4 acc = 0;
                Option<U4> a = Option::some(2);
                Option<U4> b = Option<U4>::none();
                Option<U4> c = Option::some(5);
                if let Option::Some(v) = a { acc += v; }
                if let Option::Some(v) = b { acc += v; }
                if let Option::Some(v) = c { acc += v; }
                acc
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 2 + 5 = 7
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// `if let` feeding arithmetic feeding struct literal field.
// =========================================================================

#[test]
fn if_let_result_into_struct_literal_field() {
    let src = r#"
        use Option;
        struct Acc { U4 total, }
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option::some(6);
                Acc a = Acc {
                    total: if let Option::Some(v) = o { v + 1 } else { 0 },
                };
                a.total
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// Match with a unit variant as one arm and payload-carrying variant in
// another; unit arm doesn't allocate a binding.
// =========================================================================

#[test]
fn mixed_unit_and_payload_variants_in_match() {
    let src = r#"
        enum State { Idle, Counting(U4), }
        extension U4 {
            fn test(): U4 {
                State s1 = State::Idle;
                State s2 = State::Counting(4);
                U4 a = match s1 {
                    State::Idle => 9,
                    State::Counting(n) => n,
                };
                U4 b = match s2 {
                    State::Idle => 9,
                    State::Counting(n) => n,
                };
                a + b
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 9 + 4 = 13
    assert_eq!(run_program(src, &entry), vec![13]);
}

// =========================================================================
// Enum with three unit variants — pure discriminant, no payload. Ensures
// the 0-data-size layout works end-to-end.
// =========================================================================

#[test]
fn all_unit_enum_roundtrip() {
    let src = r#"
        enum Dir { Up, Down, Right, }
        extension U4 {
            fn test(): U4 {
                Dir d = Dir::Down;
                match d {
                    Dir::Up => 1,
                    Dir::Down => 2,
                    Dir::Right => 3,
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![2]);
}

// =========================================================================
// Match used as the RHS of a declaration whose type is a struct literal
// receiver (forces the layout sizing to flow top-down).
// =========================================================================

#[test]
fn match_rhs_of_declaration_initializing_struct() {
    let src = r#"
        struct Box { U4 v, }
        extension U4 {
            fn test(): U4 {
                U4 cond = 1;
                Box b = if cond == 1 { Box { v: 7, } } else { Box { v: 0, } };
                b.v
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// Chained Option: if_let only takes the Some, pass to another if_let.
// Tests binding scoping across sequential conditional blocks.
// =========================================================================

#[test]
fn sequential_if_lets_with_distinct_bindings() {
    let src = r#"
        use Option;
        extension U4 {
            fn test(): U4 {
                Option<U4> a = Option::some(2);
                Option<U4> b = Option::some(3);
                mut U4 sum = 0;
                if let Option::Some(x) = a {
                    if let Option::Some(y) = b {
                        sum = x + y;
                    }
                }
                sum
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![5]);
}

// =========================================================================
// If-let as a condition whose ELSE branch is another full match.
// =========================================================================

#[test]
fn if_let_else_is_full_match() {
    let src = r#"
        use Option;
        enum Back { One, Two, }
        extension U4 {
            fn test(): U4 {
                Option<U4> o = Option<U4>::none();
                Back b = Back::Two;
                if let Option::Some(v) = o {
                    v
                } else {
                    match b {
                        Back::One => 1,
                        Back::Two => 2,
                    }
                }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![2]);
}

// =========================================================================
// Recursive-ish: if let's then-branch updates a counter in a loop.
// =========================================================================

#[test]
fn if_let_in_loop_updates_counter() {
    let src = r#"
        use Option;
        extension U4 {
            fn get_opt(U4 i): Option<U4> {
                if i < 3 { Option::some(1) } else { Option<U4>::none() }
            }
            fn test(): U4 {
                mut U4 i = 0;
                mut U4 total = 0;
                loop {
                    if let Option::Some(v) = U4::get_opt(i) {
                        total += v;
                    } else {
                        break;
                    }
                    i += 1;
                }
                total
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // i=0 → Some(1), total=1; i=1 → Some(1), total=2; i=2 → Some(1),
    // total=3; i=3 → None → break.
    assert_eq!(run_program(src, &entry), vec![3]);
}
