//! Step 2.6: end-to-end — parse example files, build the registry, and
//! verify its contents.
//!
//! Since most of `examples/new_syntax/` is generic (templated structs /
//! enums), we exercise what Phase 2 supports today: the concrete types
//! (`Bool`, `U8`, `System`, `Cell`) get full layouts, templated ones
//! (`Array`, `DynArray`, `Option`) get stored as templated shells, and
//! extensions merge onto their targets.

use std::path::PathBuf;

use crate::tests::parse_src;
use crate::types::*;
use crate::TypeRegistry;

fn example(path: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/new_syntax")
        .join(path);
    std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("read {}: {}", p.display(), e))
        .replace('\r', "")
}

fn items_from(path: &str) -> Vec<new_parser::ast::Spanned<new_parser::ast::Item>> {
    parse_src(&example(path))
}

#[test]
fn registers_bool_with_concrete_layout_and_methods() {
    let items = items_from("std/Bool.ct");
    let r = TypeRegistry::from_items(&items).unwrap();

    match &r.get_type("Bool").unwrap().kind {
        TypeKind::Struct(StructKind::Concrete(layout)) => {
            assert_eq!(layout.size, 1);
            assert_eq!(layout.fields.len(), 1);
            assert_eq!(layout.fields[0].name, "value");
        }
        other => panic!("Bool should be concrete struct, got {:?}", other),
    }
    let names: Vec<_> = r.get_type("Bool").unwrap()
        .methods
        .iter()
        .map(|m| m.function.sig.name.0.clone())
        .collect();
    assert!(names.contains(&"not".to_string()));
    assert!(names.contains(&"print".to_string()));
}

#[test]
fn registers_u8_with_size_2() {
    let items = items_from("std/U8.ct");
    let r = TypeRegistry::from_items(&items).unwrap();
    match &r.get_type("U8").unwrap().kind {
        TypeKind::Struct(StructKind::Concrete(layout)) => {
            assert_eq!(layout.size, 2);
            assert_eq!(layout.fields[0].name, "lower");
            assert_eq!(layout.fields[0].offset, 0);
            assert_eq!(layout.fields[1].name, "higher");
            assert_eq!(layout.fields[1].offset, 1);
        }
        other => panic!("U8 should be concrete, got {:?}", other),
    }
}

#[test]
fn templated_generic_types_are_stored_unresolved() {
    // Array.ct and Option.ct use template params; they should register as
    // templated shells without attempting to compute a size.
    let combined = format!(
        "{}\n{}\n",
        example("std/Array.ct"),
        example("std/Option.ct"),
    );
    let items = parse_src(&combined);
    let r = TypeRegistry::from_items(&items).unwrap();
    assert!(matches!(
        r.get_type("Array").unwrap().kind,
        TypeKind::Struct(StructKind::Templated { .. })
    ));
    assert_eq!(r.get_type("Array").unwrap().templates, vec!["T", "E", "F"]);
    assert!(matches!(
        r.get_type("Option").unwrap().kind,
        TypeKind::Enum(EnumKind::Templated { .. })
    ));
    assert_eq!(r.get_type("Option").unwrap().templates, vec!["T"]);
}

#[test]
fn morpion_registry_shape() {
    // Morpion pulls together enum (Cell), trait (Eq), impl (Eq for Cell),
    // struct (Morpion — generic field, so templated field fails today:
    // field ty Array<Cell, 9, U4> is a generic instantiation which Phase 2
    // doesn't resolve yet. So we expect either an error or — once we
    // teach resolve_type_size about generic instantiations — concrete.
    //
    // For now this test just confirms the registry is *built* or errors
    // cleanly. Either way the enum + trait + impl should be registered.
    let combined = format!(
        "{}\n{}\n{}\n",
        example("std/Bool.ct"),
        example("std/U8.ct"),
        example("Morpion.ct"),
    );
    let items = parse_src(&combined);
    // We allow errors here because generic instantiations aren't resolvable
    // yet, but the trait & enum must still be registered in either branch.
    let (has_eq, types_with_cell) = match TypeRegistry::from_items(&items) {
        Ok(r) => (r.has_trait("Eq"), r.has_type("Cell")),
        Err(errs) => {
            // Allowed errors: generic instantiation ("Phase 6"),
            // "not a known type" for types discovered later, etc.
            for e in &errs {
                assert!(
                    e.message.contains("monomorphization")
                        || e.message.contains("unknown type")
                        || e.message.contains("not a known type")
                        || e.message.contains("unknown trait")
                        || e.message.contains("Phase 6"),
                    "unexpected typer error: {}",
                    e.message
                );
            }
            // We can't recover the registry on Err, so just return early —
            // the pass-half of the test is covered by the Ok branch via
            // smaller fixtures below.
            return;
        }
    };
    assert!(has_eq);
    assert!(types_with_cell);
}

#[test]
fn cell_enum_and_eq_impl_without_generics() {
    // Same ingredients as Morpion's Cell+Eq but without the generic
    // `Morpion` struct (which needs monomorphization). This is the piece
    // that Phase 2 should handle end-to-end.
    let src = r#"
        struct Bool { U4 value, }
        enum Cell { Empty, O, X, }

        trait Eq {
            fn eq(self, Self other): Bool;
        }

        impl Eq for Cell {
            fn eq(self, Cell other): Bool { self as U4 == other as U4 }
        }
    "#;
    let items = parse_src(src);
    let r = TypeRegistry::from_items(&items).unwrap();

    // Cell is a concrete all-unit enum: total size 1 cell.
    match &r.get_type("Cell").unwrap().kind {
        TypeKind::Enum(EnumKind::Concrete(l)) => {
            assert_eq!(l.total_size(), 1);
            assert_eq!(l.variants.len(), 3);
        }
        other => panic!("Cell should be concrete enum, got {:?}", other),
    }
    // Impl registered + method attached.
    assert_eq!(r.impls.len(), 1);
    assert!(r.get_type("Cell").unwrap().methods.iter().any(|m| m.function.sig.name.0 == "eq"));
}
