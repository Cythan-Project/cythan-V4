//! Step 2.3: enum registration and layout computation.

use crate::tests::parse_src;
use crate::types::*;
use crate::TypeRegistry;

fn registry_with(src: &str) -> TypeRegistry {
    let items = parse_src(src);
    TypeRegistry::from_items(&items).unwrap_or_else(|e| panic!("typer errors: {:?}", e))
}

fn enum_layout(r: &TypeRegistry, name: &str) -> EnumLayout {
    match &r.types.get(name).expect("enum not found").kind {
        TypeKind::Enum(EnumKind::Concrete(l)) => l.clone(),
        other => panic!("{} is not a concrete enum: {:?}", name, other),
    }
}

#[test]
fn all_unit_enum_has_total_size_1() {
    let r = registry_with("enum Cell { Empty, O, X, }");
    let l = enum_layout(&r, "Cell");
    assert_eq!(l.discriminant_size, 1);
    assert_eq!(l.data_size, 0);
    assert_eq!(l.total_size(), 1);
    assert_eq!(l.variants.len(), 3);
    assert_eq!(l.variants[0].discriminant, 0);
    assert_eq!(l.variants[1].discriminant, 1);
    assert_eq!(l.variants[2].discriminant, 2);
}

#[test]
fn enum_with_concrete_data_computes_layout() {
    let r = registry_with(
        r#"
        struct U8 { U4 lower, U4 higher, }
        enum Mixed { A(U4), B(U8), C, }
        "#,
    );
    let l = enum_layout(&r, "Mixed");
    assert_eq!(l.discriminant_size, 1);
    assert_eq!(l.data_size, 2, "max(U4=1, U8=2, unit=0) = 2");
    assert_eq!(l.total_size(), 3);
    assert_eq!(l.variants[0].data_size, 1);
    assert_eq!(l.variants[1].data_size, 2);
    assert_eq!(l.variants[2].data_size, 0);
}

#[test]
fn explicit_discriminants_are_honored() {
    let r = registry_with("enum X { A = 0, B = 5, C = 2, }");
    let l = enum_layout(&r, "X");
    assert_eq!(l.variants[0].discriminant, 0);
    assert_eq!(l.variants[1].discriminant, 5);
    assert_eq!(l.variants[2].discriminant, 2);
}

#[test]
fn mixed_explicit_and_auto_discriminants() {
    // enum TypeMap { A = 0, B = 1, C(U4) = 2, D }
    //   D has no explicit discr; auto-assign smallest free value = 3.
    let r = registry_with("enum TypeMap { A = 0, B = 1, C(U4) = 2, D, }");
    let l = enum_layout(&r, "TypeMap");
    assert_eq!(l.variants[0].discriminant, 0);
    assert_eq!(l.variants[1].discriminant, 1);
    assert_eq!(l.variants[2].discriminant, 2);
    assert_eq!(l.variants[3].discriminant, 3);
    assert_eq!(l.data_size, 1);
}

#[test]
fn auto_discriminant_skips_used_values() {
    // A auto=0, B=3 explicit, C auto → skips 0 (taken) and 3 (taken) → 1.
    let r = registry_with("enum Y { A, B = 3, C, }");
    let l = enum_layout(&r, "Y");
    assert_eq!(l.variants[0].discriminant, 0);
    assert_eq!(l.variants[1].discriminant, 3);
    assert_eq!(l.variants[2].discriminant, 1);
}

#[test]
fn enum_with_17_variants_uses_2_cell_discriminant() {
    let variants = (0..17)
        .map(|i| format!("V{}", i))
        .collect::<Vec<_>>()
        .join(", ");
    let src = format!("enum Big {{ {}, }}", variants);
    let r = registry_with(&src);
    let l = enum_layout(&r, "Big");
    assert_eq!(l.discriminant_size, 2);
}

#[test]
fn templated_enum_is_stored_unresolved() {
    let r = registry_with("enum Option<T> { None, Some(T), }");
    let info = r.types.get("Option").unwrap();
    assert_eq!(info.templates, vec!["T"]);
    match &info.kind {
        TypeKind::Enum(EnumKind::Templated { variants }) => {
            assert_eq!(variants.len(), 2);
            assert_eq!(variants[0].name, "None");
            assert!(variants[0].data.is_none());
            assert_eq!(variants[1].name, "Some");
            assert!(variants[1].data.is_some());
        }
        other => panic!("expected templated enum, got {:?}", other),
    }
}

#[test]
fn duplicate_enum_rejected() {
    let items = parse_src("enum E { A, } enum E { B, }");
    let err = TypeRegistry::from_items(&items).unwrap_err();
    assert!(err.iter().any(|e| e.message.contains("duplicate")));
}
