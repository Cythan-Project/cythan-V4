//! Step 2.4: extension merging.

use crate::tests::parse_src;
use crate::TypeRegistry;

fn method_names(r: &TypeRegistry, ty: &str) -> Vec<String> {
    r.types
        .get(ty)
        .unwrap()
        .methods
        .iter()
        .map(|m| m.function.sig.name.0.clone())
        .collect()
}

#[test]
fn single_extension_methods_attach_to_type() {
    let items = parse_src(
        r#"
        struct Bool { U4 value, }
        extension Bool {
            fn not(self): Self { self }
            fn print(self) {}
        }
        "#,
    );
    let r = TypeRegistry::from_items(&items).unwrap();
    let methods = method_names(&r, "Bool");
    assert_eq!(methods, vec!["not", "print"]);
}

#[test]
fn two_extensions_non_overlapping_merge() {
    let items = parse_src(
        r#"
        struct Foo {}
        extension Foo {
            fn a(self) {}
        }
        extension Foo {
            fn b(self) {}
        }
        "#,
    );
    let r = TypeRegistry::from_items(&items).unwrap();
    let methods = method_names(&r, "Foo");
    assert_eq!(methods, vec!["a", "b"]);
}

#[test]
fn duplicate_method_across_extensions_rejected() {
    let items = parse_src(
        r#"
        struct Foo {}
        extension Foo {
            fn shared(self) {}
        }
        extension Foo {
            fn shared(self) {}
        }
        "#,
    );
    let err = TypeRegistry::from_items(&items).unwrap_err();
    assert!(
        err.iter().any(|e| e.message.contains("duplicate method")),
        "expected duplicate-method error, got: {:?}",
        err
    );
}

#[test]
fn extension_on_unknown_type_rejected() {
    let items = parse_src(
        r#"
        extension Missing {
            fn oops(self) {}
        }
        "#,
    );
    let err = TypeRegistry::from_items(&items).unwrap_err();
    assert!(
        err.iter().any(|e| e.message.contains("not a known type")),
        "expected unknown-type error, got: {:?}",
        err
    );
}

#[test]
fn extension_methods_carry_file_id() {
    // Single-file path uses file_id 0 via `from_items`.
    let items = parse_src(
        r#"
        struct Foo {}
        extension Foo { fn one(self) {} }
        "#,
    );
    let r = TypeRegistry::from_items(&items).unwrap();
    assert_eq!(r.types["Foo"].methods[0].file_id, 0);
}
