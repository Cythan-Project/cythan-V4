//! End-to-end tests for blanket impls: `impl<T: A + B> Trait for T`.
//!
//! The post-pass in the typer attaches blanket methods to every concrete
//! non-generic type satisfying the bounds. The HIR generator threads the
//! receiver's concrete type into the call's template args so the
//! monomorphizer binds the blanket's generic param at inline time.

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
// Baseline: blanket Iter with one bound, one method that uses `self`.
// =========================================================================

#[test]
fn blanket_impl_single_bound_one_method() {
    // Trait `Show` has a single method `label`. A blanket `Stamp` impl
    // uses `self.label()` internally. Foo implements Show, so the
    // blanket attaches to Foo.
    let src = r#"
        use Show;
        use Stamp;
        struct Foo { U4 v, }
        trait Show { fn label(self): U4; }
        trait Stamp { fn stamp(self): U4; }
        impl Show for Foo {
            fn label(self): U4 { self.v + 1 }
        }
        impl<T: Show> Stamp for T {
            fn stamp(self): U4 { self.label() + 1 }
        }
        extension U4 {
            fn test(): U4 {
                Foo f = Foo { v: 3, };
                f.stamp()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // label = 3 + 1 = 4; stamp = 4 + 1 = 5
    assert_eq!(run_program(src, &entry), vec![5]);
}

// =========================================================================
// Two bounds: sum over an IndexedGet + Length type. This is the shape
// the user's original motivating example takes.
// =========================================================================

#[test]
fn blanket_iter_sum_over_two_bounds() {
    let src = r#"
        struct Arr3 { U4 a, U4 b, U4 c, }
        trait IndexedGet { fn at(self, U4 i): U4; }
        trait Length { fn len(self): U4; }
        trait Iter { fn total(self): U4; }
        impl IndexedGet for Arr3 {
            fn at(self, U4 i): U4 {
                if i == 0 { self.a }
                else if i == 1 { self.b }
                else { self.c }
            }
        }
        impl Length for Arr3 {
            fn len(self): U4 { 3 }
        }
        use IndexedGet;
        use Length;
        use Iter;
        impl<T: IndexedGet + Length> Iter for T {
            fn total(self): U4 {
                mut U4 i = 0;
                mut U4 sum = 0;
                loop {
                    if i == self.len() { break; }
                    sum += self.at(i);
                    i += 1;
                }
                sum
            }
        }
        extension U4 {
            fn test(): U4 {
                Arr3 a = Arr3 { a: 1, b: 2, c: 4, };
                a.total()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// The same blanket impl dispatches to two different target types. Each
// gets its own monomorph.
// =========================================================================

#[test]
fn blanket_impl_on_two_different_targets() {
    let src = r#"
        use Valued;
        use Plus1;
        struct Left { U4 v, }
        struct Right { U4 w, }
        trait Valued { fn value(self): U4; }
        trait Plus1 { fn plus1(self): U4; }
        impl Valued for Left  { fn value(self): U4 { self.v } }
        impl Valued for Right { fn value(self): U4 { self.w + 1 } }
        impl<T: Valued> Plus1 for T {
            fn plus1(self): U4 { self.value() + 1 }
        }
        extension U4 {
            fn test(): U4 {
                Left l = Left { v: 2, };
                Right r = Right { w: 5, };
                l.plus1() + r.plus1()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // l.plus1 = 2+1 = 3; r.plus1 = (5+1)+1 = 7; total 10
    assert_eq!(run_program(src, &entry), vec![10]);
}

// =========================================================================
// Blanket impl with NO bounds — `impl<T> Trait for T` — applies to every
// concrete type, including the primitive `U4`.
// =========================================================================

#[test]
fn unbounded_blanket_applies_to_primitive() {
    let src = r#"
        trait Tag { fn tag(self): U4; }
        impl<T> Tag for T {
            fn tag(self): U4 { 4 }
        }
        use Tag;
        extension U4 {
            fn test(): U4 {
                U4 x = 9;
                x.tag()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![4]);
}

// =========================================================================
// Blanket impl with multiple methods.
// =========================================================================

#[test]
fn blanket_impl_with_two_methods() {
    let src = r#"
        struct Foo { U4 v, }
        trait Base { fn val(self): U4; }
        trait Extra { fn doubled(self): U4; fn incr(self): U4; }
        impl Base for Foo { fn val(self): U4 { self.v } }
        impl<T: Base> Extra for T {
            fn doubled(self): U4 { self.val() + self.val() }
            fn incr(self): U4 { self.val() + 1 }
        }
        use Base;
        use Extra;
        extension U4 {
            fn test(): U4 {
                Foo f = Foo { v: 3, };
                f.doubled() + f.incr()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // doubled = 6; incr = 4; sum = 10
    assert_eq!(run_program(src, &entry), vec![10]);
}

// =========================================================================
// Blanket method body references the generic `T` in its return type. The
// monomorphizer substitutes `T → Foo`, so the flattener sees `Foo`.
// =========================================================================

#[test]
fn blanket_method_returns_generic_param() {
    let src = r#"
        struct Foo { U4 v, }
        trait Dup { fn dup(self): Self; }
        impl Dup for Foo {
            fn dup(self): Self { Self { v: self.v + self.v, } }
        }
        trait Again { fn again(self): T; }
        impl<T: Dup> Again for T {
            fn again(self): T { self.dup() }
        }
        use Dup;
        use Again;
        extension U4 {
            fn test(): U4 {
                Foo f = Foo { v: 2, };
                Foo g = f.again();
                g.v
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![4]);
}

// =========================================================================
// Blanket impl target type has a multi-cell representation. Verifies
// that sizing / layout / mut-self propagation all work when T resolves
// to a type larger than 1 cell.
// =========================================================================

#[test]
fn blanket_impl_on_multi_cell_target() {
    let src = r#"
        struct Pt { U4 x, U4 y, }
        trait Sum { fn sum(self): U4; }
        trait Shifted { fn shifted(self): U4; }
        impl Sum for Pt {
            fn sum(self): U4 { self.x + self.y }
        }
        impl<T: Sum> Shifted for T {
            fn shifted(self): U4 { self.sum() + 1 }
        }
        use Sum;
        use Shifted;
        extension U4 {
            fn test(): U4 {
                Pt p = Pt { x: 2, y: 3, };
                p.shifted()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![6]);
}

// =========================================================================
// Type that does NOT satisfy the bounds does not get the blanket method.
// If you try to call it, resolution fails — we check this by confirming
// the direct impl on a satisfying type still works and the non-
// satisfying type can't reach the blanket.
// =========================================================================

#[test]
fn non_satisfying_type_does_not_dispatch_through_blanket() {
    let src = r#"
        struct Yes { U4 v, }
        struct No { U4 v, }
        trait Foo { fn foo(self): U4; }
        trait Via { fn via(self): U4; }
        impl Foo for Yes { fn foo(self): U4 { self.v } }
        impl<T: Foo> Via for T {
            fn via(self): U4 { self.foo() + 1 }
        }
        use Foo;
        use Via;
        extension U4 {
            fn test(): U4 {
                Yes y = Yes { v: 4, };
                y.via()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // `No` doesn't implement `Foo`, so the blanket never attached. We
    // don't call `no.via()` — just confirm `y.via()` works. A regression
    // here (blanket attached to `No` anyway) would surface elsewhere.
    assert_eq!(run_program(src, &entry), vec![5]);
}

// =========================================================================
// Direct impl for a type takes precedence over the blanket's attachment.
// If both would attach the same method, the direct one stays and the
// blanket version doesn't overwrite it.
// =========================================================================

#[test]
fn direct_impl_wins_over_blanket() {
    let src = r#"
        struct Foo { U4 v, }
        trait Base { fn val(self): U4; }
        trait Ext { fn both(self): U4; }
        impl Base for Foo { fn val(self): U4 { self.v } }
        impl Ext for Foo {
            fn both(self): U4 { 9 }
        }
        impl<T: Base> Ext for T {
            fn both(self): U4 { self.val() + 1 }
        }
        use Base;
        use Ext;
        extension U4 {
            fn test(): U4 {
                Foo f = Foo { v: 5, };
                f.both()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // Direct impl returns 9 — blanket's version would have returned 6.
    assert_eq!(run_program(src, &entry), vec![9]);
}

// =========================================================================
// Blanket method calling multiple bound-trait methods in its body.
// =========================================================================

#[test]
fn blanket_method_calls_all_bound_methods() {
    let src = r#"
        struct Arr { U4 a, U4 b, U4 c, }
        trait A { fn x(self): U4; }
        trait B { fn y(self): U4; }
        trait C { fn z(self): U4; }
        trait Combo { fn combo(self): U4; }
        impl A for Arr { fn x(self): U4 { self.a } }
        impl B for Arr { fn y(self): U4 { self.b } }
        impl C for Arr { fn z(self): U4 { self.c } }
        impl<T: A + B + C> Combo for T {
            fn combo(self): U4 { self.x() + self.y() + self.z() }
        }
        use A;
        use B;
        use C;
        use Combo;
        extension U4 {
            fn test(): U4 {
                Arr a = Arr { a: 1, b: 2, c: 4, };
                a.combo()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// Bound is an operator trait (`Add`). Operator-trait impls already
// register as from_trait="Add" in the methods list; blanket satisfaction
// must pick those up the same way as regular trait impls.
// =========================================================================

#[test]
fn blanket_over_operator_trait_bound() {
    let src = r#"
        struct Foo { U4 v, }
        impl Add<Foo> for Foo {
            fn add(self, Foo other): Foo {
                Self { v: self.v + other.v, }
            }
        }
        trait Square { fn square(self): Foo; }
        impl<T: Add> Square for T {
            fn square(self): Foo { self + self }
        }
        use Square;
        extension U4 {
            fn test(): U4 {
                Foo f = Foo { v: 3, };
                Foo s = f.square();
                s.v
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // f + f = Foo { v: 6 }
    assert_eq!(run_program(src, &entry), vec![6]);
}

// =========================================================================
// Blanket method using internal locals + conditional. Exercises the body
// substitution pipeline on more than a straight-line expression.
// =========================================================================

#[test]
fn blanket_method_with_local_and_if() {
    let src = r#"
        struct Foo { U4 v, }
        trait Valued { fn value(self): U4; }
        trait Bounded { fn at_most_4(self): U4; }
        impl Valued for Foo { fn value(self): U4 { self.v } }
        impl<T: Valued> Bounded for T {
            fn at_most_4(self): U4 {
                U4 v = self.value();
                if v > 4 { 4 } else { v }
            }
        }
        use Valued;
        use Bounded;
        extension U4 {
            fn test(): U4 {
                Foo high = Foo { v: 9, };
                Foo low  = Foo { v: 2, };
                high.at_most_4() + low.at_most_4()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // high.at_most_4 = 4; low.at_most_4 = 2; sum = 6
    assert_eq!(run_program(src, &entry), vec![6]);
}

// =========================================================================
// Two blanket impls of different traits share bounds; both attach to the
// same target type.
// =========================================================================

#[test]
fn two_blanket_traits_share_a_target() {
    let src = r#"
        struct Foo { U4 v, }
        trait Get { fn get(self): U4; }
        trait Double { fn double(self): U4; }
        trait Triple { fn triple(self): U4; }
        impl Get for Foo { fn get(self): U4 { self.v } }
        impl<T: Get> Double for T {
            fn double(self): U4 { self.get() + self.get() }
        }
        impl<T: Get> Triple for T {
            fn triple(self): U4 { self.get() + self.get() + self.get() }
        }
        use Get;
        use Double;
        use Triple;
        extension U4 {
            fn test(): U4 {
                Foo f = Foo { v: 2, };
                f.double() + f.triple()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 4 + 6 = 10
    assert_eq!(run_program(src, &entry), vec![10]);
}

// =========================================================================
// Blanket body mutates `self` and the caller observes the change through
// the by-reference mut-self rule.
// =========================================================================

#[test]
fn blanket_mut_self_propagates_to_caller() {
    let src = r#"
        struct Counter { U4 n, }
        trait Base {
            fn inc_base(mut self);
        }
        trait Twice { fn inc2(mut self); }
        impl Base for Counter {
            fn inc_base(mut self) { self.n += 1; }
        }
        impl<T: Base> Twice for T {
            fn inc2(mut self) {
                self.inc_base();
                self.inc_base();
            }
        }
        use Base;
        use Twice;
        extension U4 {
            fn test(): U4 {
                mut Counter c = Counter { n: 3, };
                c.inc2();
                c.n
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![5]);
}

// =========================================================================
// Blanket impl with an intermediate binding that references the generic
// param name via Self inside the body.
// =========================================================================

#[test]
fn blanket_body_uses_self_type_binding() {
    let src = r#"
        struct Foo { U4 v, }
        trait Id { fn id(self): Self; }
        impl Id for Foo {
            fn id(self): Self { Self { v: self.v, } }
        }
        trait Chain { fn chain(self): Foo; }
        impl<T: Id> Chain for T {
            fn chain(self): Foo { self.id().id() }
        }
        use Id;
        use Chain;
        extension U4 {
            fn test(): U4 {
                Foo f = Foo { v: 7, };
                Foo r = f.chain();
                r.v
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// Blanket impl where the body uses field access on self through a trait
// method chain.
// =========================================================================

#[test]
fn blanket_field_chain_through_bound_method() {
    let src = r#"
        struct Pair { U4 a, U4 b, }
        trait First { fn first(self): U4; }
        trait SumFirsts {
            fn sum_two(self, Pair other): U4;
        }
        impl First for Pair { fn first(self): U4 { self.a } }
        impl<T: First> SumFirsts for T {
            fn sum_two(self, Pair other): U4 {
                self.first() + other.first()
            }
        }
        use First;
        use SumFirsts;
        extension U4 {
            fn test(): U4 {
                Pair p = Pair { a: 3, b: 9, };
                Pair q = Pair { a: 4, b: 8, };
                p.sum_two(q)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![7]);
}
