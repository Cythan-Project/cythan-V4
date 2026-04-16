//! Step 2.2: struct registration and size computation.

use crate::tests::parse_src;
use crate::types::*;
use crate::TypeRegistry;

fn registry_with(src: &str) -> TypeRegistry {
    let items = parse_src(src);
    TypeRegistry::from_items(&items).unwrap_or_else(|e| panic!("typer errors: {:?}", e))
}

fn layout_of(r: &TypeRegistry, name: &str) -> StructLayout {
    match &r.types.get(name).expect("type not found").kind {
        TypeKind::Struct(StructKind::Concrete(l)) => l.clone(),
        other => panic!("{} is not a concrete struct: {:?}", name, other),
    }
}

#[test]
fn empty_registry_contains_u4_primitive() {
    let r = TypeRegistry::new();
    let u4 = r.types.get("U4").expect("U4 pre-populated");
    assert_eq!(u4.kind, TypeKind::Primitive { size: 1 });
    assert!(u4.templates.is_empty());
    assert!(u4.methods.is_empty());
}

#[test]
fn struct_pair_offsets_and_size() {
    // struct Pair { U4 a, U8 b } → size 3, offsets 0, 1
    // (U8 provided via source so it gets registered too.)
    let r = registry_with(
        r#"
        struct U8 { U4 lower, U4 higher, }
        struct Pair { U4 a, U8 b, }
        "#,
    );
    let pair = layout_of(&r, "Pair");
    assert_eq!(pair.size, 3);
    assert_eq!(pair.fields.len(), 2);
    assert_eq!(pair.fields[0].name, "a");
    assert_eq!(pair.fields[0].offset, 0);
    assert_eq!(pair.fields[0].size, 1);
    assert_eq!(pair.fields[1].name, "b");
    assert_eq!(pair.fields[1].offset, 1);
    assert_eq!(pair.fields[1].size, 2);
}

#[test]
fn struct_u8_computes_size_2() {
    let r = registry_with("struct U8 { U4 lower, U4 higher, }");
    let u8 = layout_of(&r, "U8");
    assert_eq!(u8.size, 2);
    assert_eq!(u8.fields[0].offset, 0);
    assert_eq!(u8.fields[1].offset, 1);
}

#[test]
fn struct_bool_has_size_1() {
    let r = registry_with("struct Bool { U4 value, }");
    let b = layout_of(&r, "Bool");
    assert_eq!(b.size, 1);
}

#[test]
fn struct_u4_marker_is_primitive() {
    // Declaring `struct U4 {}` in source must keep U4 as a 1-cell primitive,
    // not a zero-sized struct.
    let r = registry_with("struct U4 {}");
    assert_eq!(r.types["U4"].kind, TypeKind::Primitive { size: 1 });
}

#[test]
fn struct_u4_with_fields_is_rejected() {
    let items = parse_src("struct U4 { U4 value, }");
    let err = TypeRegistry::from_items(&items).unwrap_err();
    assert!(
        err.iter().any(|e| e.message.contains("primitive U4")),
        "expected primitive U4 error, got: {:?}",
        err
    );
}

#[test]
fn templated_struct_stored_but_not_sized() {
    let r = registry_with("struct Array<T, E, F> {}");
    let info = r.types.get("Array").unwrap();
    assert_eq!(info.templates, vec!["T", "E", "F"]);
    match &info.kind {
        TypeKind::Struct(StructKind::Templated { fields }) => {
            assert!(fields.is_empty());
        }
        other => panic!("expected templated struct, got {:?}", other),
    }
}

#[test]
fn duplicate_struct_is_rejected() {
    let items = parse_src("struct Foo {} struct Foo {}");
    let err = TypeRegistry::from_items(&items).unwrap_err();
    assert!(err.iter().any(|e| e.message.contains("duplicate")));
}

#[test]
fn unknown_field_type_is_rejected() {
    let items = parse_src("struct X { Missing foo, }");
    let err = TypeRegistry::from_items(&items).unwrap_err();
    assert!(err.iter().any(|e| e.message.contains("unknown type")));
}
