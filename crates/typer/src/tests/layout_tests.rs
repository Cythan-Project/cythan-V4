//! Step 2.1: construct `StructLayout` / `EnumLayout` by hand and verify
//! size computations (discriminant sizing, total_size, etc.).

use crate::types::*;

#[test]
fn discriminant_size_for_small_variant_counts() {
    assert_eq!(discriminant_size_for(1), 1);
    assert_eq!(discriminant_size_for(2), 1);
    assert_eq!(discriminant_size_for(16), 1);
    assert_eq!(discriminant_size_for(17), 2);
    assert_eq!(discriminant_size_for(100), 2);
    assert_eq!(discriminant_size_for(256), 2);
}

#[test]
fn struct_layout_sums_field_sizes() {
    fn dummy_ty(name: &str) -> new_parser::ast::Type {
        new_parser::ast::Type {
            name: (name.to_string(), 0..0),
            templates: vec![],
            qself: None,
        }
    }
    let layout = StructLayout {
        fields: vec![
            FieldLayout { name: "a".into(), offset: 0, size: 1, ast_type: dummy_ty("U4") },
            FieldLayout { name: "b".into(), offset: 1, size: 2, ast_type: dummy_ty("U8") },
        ],
        size: 3,
    };
    assert_eq!(layout.size, 3);
    let total: CellCount = layout.fields.iter().map(|f| f.size).sum();
    assert_eq!(total, layout.size);
}

#[test]
fn enum_layout_total_size_is_discr_plus_data() {
    // enum Option<u8>: 2 variants (discr=1), data size 2 (u8).
    let layout = EnumLayout {
        variants: vec![
            EnumVariantLayout { name: "None".into(), discriminant: 0, data_size: 0 },
            EnumVariantLayout { name: "Some".into(), discriminant: 1, data_size: 2 },
        ],
        discriminant_size: 1,
        data_size: 2,
    };
    assert_eq!(layout.total_size(), 3);
}

#[test]
fn enum_layout_all_unit_has_no_data() {
    // enum Cell { Empty, O, X } → 3 variants, all unit.
    let layout = EnumLayout {
        variants: vec![
            EnumVariantLayout { name: "Empty".into(), discriminant: 0, data_size: 0 },
            EnumVariantLayout { name: "O".into(),     discriminant: 1, data_size: 0 },
            EnumVariantLayout { name: "X".into(),     discriminant: 2, data_size: 0 },
        ],
        discriminant_size: 1,
        data_size: 0,
    };
    assert_eq!(layout.total_size(), 1);
}

#[test]
fn u4_primitive_constants() {
    assert_eq!(U4_SIZE, 1);
    assert_eq!(U4_NAME, "U4");
}
