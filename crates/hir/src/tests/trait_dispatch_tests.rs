//! End-to-end tests for trait-aware dispatch through the HIR generator,
//! MonomorphKey mangling, and the inliner.

use std::collections::HashMap;

use crate::{
    gen_function, hir_to_mir, inline_program, ConcreteTemplateArg, ConcreteType, FnRef,
    HirFunction, HirOp, MonomorphKey,
};

// ---------------------------------------------------------------------------
// MonomorphKey distinguishes same method name on different traits.
// ---------------------------------------------------------------------------

#[test]
fn monomorph_key_for_same_method_under_different_traits_is_distinct() {
    // (type=Foo, method=act, no templates): inherent vs via trait A vs via trait B
    let inherent = MonomorphKey {
        sig: typer::FnSig::new("Foo", "act"),
        template_args: vec![],
    };
    let via_a = MonomorphKey {
        sig: typer::FnSig::new_trait("Foo", "act", "A"),
        template_args: vec![],
    };
    let via_b = MonomorphKey {
        sig: typer::FnSig::new_trait("Foo", "act", "B"),
        template_args: vec![],
    };
    assert_ne!(inherent.mangled(), via_a.mangled());
    assert_ne!(via_a.mangled(), via_b.mangled());
    // Mangled FnSigs carry the trait too (the inliner will see it).
    assert_eq!(via_a.mangled().trait_name, Some("A".to_string()));
    assert_eq!(via_b.mangled().trait_name, Some("B".to_string()));
    assert_eq!(inherent.mangled().trait_name, None);
}

#[test]
fn monomorph_key_mangling_preserves_trait_with_templates() {
    // Trait + template args together should still produce a stable key.
    let key = MonomorphKey {
        sig: typer::FnSig::new_trait("Array", "get", "Indexable"),
        template_args: vec![
            ConcreteTemplateArg::Type(ConcreteType {
                name: "U4".into(),
                args: vec![],
            }),
            ConcreteTemplateArg::Value(9),
        ],
    };
    let mangled = key.mangled();
    assert_eq!(mangled.type_name, "Array<U4,9>");
    assert_eq!(mangled.method_name, "get");
    assert_eq!(mangled.trait_name, Some("Indexable".to_string()));
}

// ---------------------------------------------------------------------------
// HIR generator routes calls through resolve_method.
// ---------------------------------------------------------------------------

fn compile_all(src: &str) -> HashMap<typer::FnSig, HirFunction> {
    let items = new_parser::parse(src).expect("parse");
    let reg = typer::TypeRegistry::from_items(&items).expect("typer");
    let db = typer::FunctionDB::from_registry(&reg).expect("fn_db");
    let mut out = HashMap::new();
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            let hir = gen_function(k, s, &reg, &db).expect("hir gen");
            out.insert(k.clone(), hir);
        }
    }
    out
}

fn find_call_target(hir: &HirFunction, method: &str) -> Option<FnRef> {
    fn find(ops: &[HirOp], method: &str) -> Option<FnRef> {
        for op in ops {
            match op {
                HirOp::Call { target, .. } if target.method_name == method => {
                    return Some(target.clone())
                }
                HirOp::Loop(b) | HirOp::Block(b) => {
                    if let Some(r) = find(&b.ops, method) {
                        return Some(r);
                    }
                }
                HirOp::Match(_, arms) => {
                    for (arm, _) in arms {
                        if let Some(r) = find(&arm.ops, method) {
                            return Some(r);
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }
    find(&hir.body.ops, method)
}

#[test]
fn hir_gen_tags_fn_ref_with_resolved_trait() {
    // Single file: Bool is declared, trait Add is used (as an operator
    // trait it doesn't need `use`), impl provides `add`, caller calls
    // `x.add(y)`. HIR-emitted FnRef should carry `trait_name = Some("Add")`.
    let src = r#"
        struct Bool { U4 value, }
        struct Foo {}
        trait Add { fn add(self, Self other): Self; }
        impl Add for Foo { fn add(self, Foo other): Foo { self } }
        extension Foo {
            fn run(self, Foo other): Self { self.add(other) }
        }
    "#;
    let fns = compile_all(src);
    let caller = fns.get(&typer::FnSig::new("Foo", "run")).unwrap();
    let target = find_call_target(caller, "add").expect("Call to add");
    assert_eq!(target.type_name, "Foo");
    assert_eq!(target.trait_name, Some("Add".to_string()));
}

#[test]
fn hir_gen_leaves_trait_name_none_for_inherent_methods() {
    let src = r#"
        extension U4 {
            fn helper(U4 x): Self { x }
            fn caller(): U4 { U4::helper(3) }
        }
    "#;
    let fns = compile_all(src);
    let caller = fns.get(&typer::FnSig::new("U4", "caller")).unwrap();
    let target = find_call_target(caller, "helper").expect("Call to helper");
    assert_eq!(target.trait_name, None);
}

#[test]
fn hir_gen_distinguishes_two_traits_same_method() {
    // When a type has both `impl A for Foo { fn act }` and
    // `impl B for Foo { fn act }`, and both traits are imported, calling
    // `x.act()` is ambiguous — the resolver doesn't tag a trait_name (it
    // returns Ambiguous), so the FnRef gets `trait_name = None`. That's the
    // correct behaviour: the call *should* fail later (or require explicit
    // qualification).
    let src = r#"
        struct Bool { U4 value, }
        struct Foo {}
        trait A { fn act(self): Bool; }
        trait B { fn act(self): Bool; }
        impl A for Foo { fn act(self): Bool { true } }
        impl B for Foo { fn act(self): Bool { false } }
        use A;
        use B;
        extension Foo { fn caller(self): Bool { self.act() } }
    "#;
    let fns = compile_all(src);
    let caller = fns.get(&typer::FnSig::new("Foo", "caller")).unwrap();
    let target = find_call_target(caller, "act").expect("Call to act");
    // trait_name = None means the generator did NOT commit to A or B —
    // any downstream resolver will see the ambiguous call and can fail
    // with a diagnostic. This is the correct outcome.
    assert_eq!(target.trait_name, None);
}

#[test]
fn inliner_resolves_trait_keyed_calls() {
    // Full pipeline: declare trait + impl, call operator-style, inline,
    // check that the inliner finds the right body.
    let src = r#"
        struct Bool { U4 value, }
        struct Foo {}
        trait Add { fn add(self, Self other): Self; }
        impl Add for Foo { fn add(self, Foo other): Foo { self } }
        extension Foo {
            fn run(Foo a, Foo b): Self { a.add(b) }
        }
    "#;
    let fns = compile_all(src);
    let entry = typer::FnSig::new("Foo", "run");
    let inlined = inline_program(&fns, &entry).expect("inline");
    // After inlining there must be no Call ops left — the impl was found
    // via the trait-keyed lookup.
    fn contains_call(block: &crate::HirBlock) -> bool {
        block.ops.iter().any(|op| match op {
            HirOp::Call { .. } => true,
            HirOp::Loop(b) | HirOp::Block(b) => contains_call(b),
            HirOp::Match(_, arms) => arms.iter().any(|(b, _)| contains_call(b)),
            _ => false,
        })
    }
    assert!(
        !contains_call(&inlined.body),
        "Call remained after inlining: {:#?}",
        inlined.body.ops
    );
    // And the resulting MIR builds fine.
    hir_to_mir(&inlined.body).expect("convert to mir");
}
