//! Phase 3 Steps 3.1 + 3.2: FunctionDB construction and population.

use crate::tests::parse_src;
use crate::{Fn, FnSig, FunctionDB, TypeRegistry};

fn build(src: &str) -> FunctionDB {
    let items = parse_src(src);
    let reg = TypeRegistry::from_items(&items).unwrap();
    FunctionDB::from_registry(&reg).unwrap_or_else(|e| panic!("FunctionDB errors: {:?}", e))
}

// ---------- Step 3.1: basic API ----------

#[test]
fn empty_db_has_no_functions() {
    let db = FunctionDB::new();
    assert!(db.functions.is_empty());
}

#[test]
fn fnsig_equality_and_hash() {
    let a = FnSig::new("Bool", "not");
    let b = FnSig::new("Bool", "not");
    let c = FnSig::new("Bool", "print");
    assert_eq!(a, b);
    assert_ne!(a, c);

    // Works as a HashMap key.
    use std::collections::HashMap;
    let mut m = HashMap::new();
    m.insert(a.clone(), 1);
    assert_eq!(m.get(&b), Some(&1));
    assert_eq!(m.get(&c), None);
}

// ---------- Step 3.2: populate from registry ----------

#[test]
fn extension_methods_are_registered_simple() {
    let db = build(
        r#"
        struct Bool { U4 value, }
        extension Bool {
            fn not(self): Self { self }
            fn print(self) {}
        }
        "#,
    );
    let not = db.get(&FnSig::new("Bool", "not")).expect("Bool::not");
    assert!(matches!(not, Fn::Simple(_)));
    let print = db.get(&FnSig::new("Bool", "print")).expect("Bool::print");
    assert!(matches!(print, Fn::Simple(_)));
}

#[test]
fn methods_on_generic_type_are_templated() {
    let db = build("struct Array<T, E, F> {} extension Array<T, E, F> { fn len(self) {} }");
    let len = db.get(&FnSig::new("Array", "len")).unwrap();
    match len {
        Fn::Templated(t) => {
            assert_eq!(t.templates, vec!["T", "E", "F"]);
            assert_eq!(t.type_name, "Array");
        }
        other => panic!("expected Templated, got {:?}", other),
    }
}

#[test]
fn fn_level_templates_force_templated_even_on_concrete_type() {
    let db = build("struct System {} extension System { fn debug<T>(T a) {} }");
    let f = db.get(&FnSig::new("System", "debug")).unwrap();
    match f {
        Fn::Templated(t) => assert_eq!(t.templates, vec!["T"]),
        other => panic!("expected Templated, got {:?}", other),
    }
}

#[test]
fn both_type_and_fn_templates_combine() {
    let db = build(
        r#"
        struct Array<T, E, F> {}
        extension Array<T, E, F> {
            fn get<N>(self): T {}
        }
        "#,
    );
    let f = db.get(&FnSig::new("Array", "get")).unwrap();
    match f {
        Fn::Templated(t) => assert_eq!(t.templates, vec!["T", "E", "F", "N"]),
        other => panic!("expected Templated, got {:?}", other),
    }
}

#[test]
fn impl_methods_are_registered_and_track_trait() {
    let db = build(
        r#"
        struct Bool { U4 value, }
        enum Cell { Empty, O, X, }
        trait Eq { fn eq(self, Self other): Bool; }
        impl Eq for Cell {
            fn eq(self, Cell other): Bool { self as U4 == other as U4 }
        }
        "#,
    );
    // Trait methods are keyed by their trait, so use `new_trait`.
    let eq = db
        .get(&FnSig::new_trait("Cell", "eq", "Eq"))
        .expect("Cell::eq via Eq");
    match eq {
        Fn::Simple(s) => {
            assert_eq!(s.from_trait.as_deref(), Some("Eq"));
            assert_eq!(s.type_name, "Cell");
        }
        other => panic!("expected Simple, got {:?}", other),
    }
    // The inherent-shaped key should NOT match this trait-only method.
    assert!(db.get(&FnSig::new("Cell", "eq")).is_none());
}

#[test]
fn two_types_share_method_name_without_conflict() {
    let db = build(
        r#"
        struct Foo {}
        struct Bar {}
        extension Foo { fn run(self) {} }
        extension Bar { fn run(self) {} }
        "#,
    );
    assert!(db.get(&FnSig::new("Foo", "run")).is_some());
    assert!(db.get(&FnSig::new("Bar", "run")).is_some());
    assert_eq!(db.functions.len(), 2);
}
