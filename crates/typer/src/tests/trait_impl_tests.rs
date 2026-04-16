//! Step 2.5: trait registration and `impl` validation.

use crate::tests::parse_src;
use crate::TypeRegistry;

#[test]
fn trait_is_registered() {
    let items = parse_src(
        r#"
        trait Eq {
            fn eq(self, Self other): Bool;
        }
        struct Bool { U4 value, }
        "#,
    );
    let r = TypeRegistry::from_items(&items).unwrap();
    let eq = r.traits.get("Eq").unwrap();
    assert_eq!(eq.name, "Eq");
    assert_eq!(eq.methods.len(), 1);
    assert_eq!(eq.methods[0].name.0, "eq");
}

#[test]
fn valid_impl_accepted_and_method_attached_to_target() {
    let items = parse_src(
        r#"
        struct Bool { U4 value, }
        enum Cell { Empty, O, X, }

        trait Eq {
            fn eq(self, Self other): Bool;
        }

        impl Eq for Cell {
            fn eq(self, Cell other): Bool { self as U4 == other as U4 }
        }
        "#,
    );
    let r = TypeRegistry::from_items(&items).unwrap();
    let cell = r.types.get("Cell").unwrap();
    let names: Vec<_> = cell.methods.iter().map(|m| m.function.sig.name.0.clone()).collect();
    assert!(names.contains(&"eq".to_string()));
    assert_eq!(r.impls.len(), 1);
    assert_eq!(r.impls[0].trait_name, "Eq");
    assert_eq!(r.impls[0].target_name, "Cell");
    // The attached method remembers where it came from.
    let eq = cell.methods.iter().find(|m| m.function.sig.name.0 == "eq").unwrap();
    assert_eq!(eq.from_trait.as_deref(), Some("Eq"));
}

#[test]
fn impl_missing_method_rejected() {
    let items = parse_src(
        r#"
        struct Bool { U4 value, }
        struct Foo {}
        trait Eq {
            fn eq(self, Self other): Bool;
        }
        impl Eq for Foo {}
        "#,
    );
    let err = TypeRegistry::from_items(&items).unwrap_err();
    assert!(
        err.iter().any(|e| e.message.contains("missing method `eq`")),
        "expected missing-method error, got: {:?}",
        err
    );
}

#[test]
fn impl_extra_method_rejected() {
    let items = parse_src(
        r#"
        struct Bool { U4 value, }
        struct Foo {}
        trait Eq {
            fn eq(self, Self other): Bool;
        }
        impl Eq for Foo {
            fn eq(self, Foo other): Bool { true }
            fn extra(self) {}
        }
        "#,
    );
    let err = TypeRegistry::from_items(&items).unwrap_err();
    assert!(
        err.iter().any(|e| e.message.contains("not declared by the trait")),
        "expected extra-method error, got: {:?}",
        err
    );
}

#[test]
fn impl_wrong_arity_rejected() {
    let items = parse_src(
        r#"
        struct Bool { U4 value, }
        struct Foo {}
        trait Eq {
            fn eq(self, Self other): Bool;
        }
        impl Eq for Foo {
            fn eq(self): Bool { true }
        }
        "#,
    );
    let err = TypeRegistry::from_items(&items).unwrap_err();
    assert!(
        err.iter().any(|e| e.message.contains("expected 2 parameters")),
        "expected arity error, got: {:?}",
        err
    );
}

#[test]
fn impl_unknown_trait_rejected() {
    let items = parse_src(
        r#"
        struct Foo {}
        impl Missing for Foo {}
        "#,
    );
    let err = TypeRegistry::from_items(&items).unwrap_err();
    assert!(
        err.iter().any(|e| e.message.contains("unknown trait")),
        "expected unknown-trait error, got: {:?}",
        err
    );
}

#[test]
fn impl_unknown_target_rejected() {
    let items = parse_src(
        r#"
        trait Eq {
            fn eq(self): U4;
        }
        impl Eq for Missing {}
        "#,
    );
    let err = TypeRegistry::from_items(&items).unwrap_err();
    assert!(
        err.iter().any(|e| e.message.contains("impl target")),
        "expected unknown-target error, got: {:?}",
        err
    );
}

#[test]
fn impl_associated_type_bindings_checked() {
    // Missing binding for `Other`.
    let missing = parse_src(
        r#"
        struct Foo {}
        struct Bool { U4 value, }
        trait Add {
            type Other;
            type Result;
            fn add(self, Self::Other other): Self::Result;
        }
        impl Add for Foo {
            type Other = Foo;
            fn add(self, Foo other): Foo { self }
        }
        "#,
    );
    let err = TypeRegistry::from_items(&missing).unwrap_err();
    assert!(
        err.iter().any(|e| e.message.contains("Result")),
        "expected missing-binding error for Result, got: {:?}",
        err
    );

    // Extraneous binding.
    let extra = parse_src(
        r#"
        struct Foo {}
        trait Add {
            type Other;
            fn add(self, Self::Other other): Self::Other;
        }
        impl Add for Foo {
            type Other = Foo;
            type Unexpected = Foo;
            fn add(self, Foo other): Foo { self }
        }
        "#,
    );
    let err = TypeRegistry::from_items(&extra).unwrap_err();
    assert!(
        err.iter().any(|e| e.message.contains("Unexpected")),
        "expected unknown-binding error, got: {:?}",
        err
    );
}
