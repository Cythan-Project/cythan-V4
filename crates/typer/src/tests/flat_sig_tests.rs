//! Phase 3 Step 3.3: signature flattening to slot layouts.

use crate::tests::parse_src;
use crate::{FlatSig, Fn, FnSig, FunctionDB, TypeRegistry};

fn flat(src: &str, type_name: &str, method: &str) -> FlatSig {
    let items = parse_src(src);
    let reg = TypeRegistry::from_items(&items).unwrap();
    let db = FunctionDB::from_registry(&reg).unwrap();
    match db.get(&FnSig::new(type_name, method)).unwrap() {
        Fn::Simple(s) => s.sig.clone(),
        other => panic!("expected Simple, got {:?}", other),
    }
}

#[test]
fn no_params_no_return_is_empty() {
    let sig = flat(
        r#"
        struct Foo {}
        extension Foo { fn tick(self) {} }
        "#,
        "Foo",
        "tick",
    );
    // Only `self` input (size 0 for empty struct) — zero cells.
    assert_eq!(sig.total_slots(), 0);
    assert_eq!(sig.input_count, 0);
    assert_eq!(sig.output_count, 0);
    assert_eq!(sig.slots.len(), 1, "still records the self slot entry");
    assert_eq!(sig.slots[0].name, "self");
    assert_eq!(sig.slots[0].size, 0);
}

#[test]
fn self_u4_and_return_u4() {
    // `fn not(self): Self` on U4-wrapping Bool:
    //   input:  self (Bool = 1 cell)
    //   output: _ret (Bool = 1 cell)
    let sig = flat(
        r#"
        struct Bool { U4 value, }
        extension Bool { fn not(self): Self { self } }
        "#,
        "Bool",
        "not",
    );
    assert_eq!(sig.input_count, 1);
    assert_eq!(sig.output_count, 1);
    assert_eq!(sig.total_slots(), 2);
    assert_eq!(sig.slots[0].name, "self");
    assert_eq!(sig.slots[0].offset, 0);
    assert_eq!(sig.slots[0].size, 1);
    assert!(!sig.slots[0].mutable);
    assert_eq!(sig.slots[1].name, "_ret");
    assert_eq!(sig.slots[1].offset, 1);
    assert_eq!(sig.slots[1].size, 1);
    assert!(sig.slots[1].mutable, "return slots are mut");
}

#[test]
fn self_u8_and_u8_param_return_u4() {
    // `fn test(self, U8 a): U4` on U8:
    //   self = 2 cells [0,1]
    //   a    = 2 cells [2,3]
    //   _ret = 1 cell  [4]
    let sig = flat(
        r#"
        struct U8 { U4 lower, U4 higher, }
        extension U8 { fn test(self, U8 a): U4 { 0 } }
        "#,
        "U8",
        "test",
    );
    assert_eq!(sig.input_count, 4);
    assert_eq!(sig.output_count, 1);
    assert_eq!(sig.total_slots(), 5);
    let self_slot = &sig.slots[0];
    assert_eq!(self_slot.name, "self");
    assert_eq!((self_slot.offset, self_slot.size), (0, 2));
    assert!(!self_slot.mutable);
    let a = &sig.slots[1];
    assert_eq!(a.name, "a");
    assert_eq!((a.offset, a.size), (2, 2));
    let ret = &sig.slots[2];
    assert_eq!(ret.name, "_ret");
    assert_eq!((ret.offset, ret.size), (4, 1));
    assert!(ret.mutable);
}

#[test]
fn mut_propagates_to_slot() {
    let sig = flat(
        r#"
        struct U8 { U4 lower, U4 higher, }
        extension U8 {
            fn sub(mut self, mut U8 other) {}
        }
        "#,
        "U8",
        "sub",
    );
    assert_eq!(sig.slots.len(), 2);
    assert!(sig.slots[0].mutable, "mut self");
    assert!(sig.slots[1].mutable, "mut other");
}

#[test]
fn struct_param_records_field_offsets() {
    // For `self: U8`, `self.lower` is at offset 0, `self.higher` is at offset 1.
    let sig = flat(
        r#"
        struct U8 { U4 lower, U4 higher, }
        extension U8 { fn get_lower(self): U4 { self.lower } }
        "#,
        "U8",
        "get_lower",
    );
    let fields = sig
        .field_offsets
        .get("self")
        .expect("field_offsets for self");
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "lower");
    assert_eq!((fields[0].offset, fields[0].size), (0, 1));
    assert_eq!(fields[1].name, "higher");
    assert_eq!((fields[1].offset, fields[1].size), (1, 1));
}

#[test]
fn enum_return_has_discr_plus_data_cells() {
    // `fn make(): Cell` — Cell is a unit-only enum (total size 1).
    let sig = flat(
        r#"
        enum Cell { Empty, O, X, }
        extension Cell { fn make(): Self { Self::Empty } }
        "#,
        "Cell",
        "make",
    );
    // No inputs, return size = 1 (discr only, no data).
    assert_eq!(sig.input_count, 0);
    assert_eq!(sig.output_count, 1);
    let ret = sig.slots.iter().find(|s| s.name == "_ret").unwrap();
    assert_eq!(ret.size, 1);
    assert_eq!(ret.type_name, "Cell");
}

#[test]
fn return_type_name_self_is_resolved_to_enclosing() {
    // `fn new_zero(): Self` on U8 resolves `Self` → "U8" at flatten time
    // so downstream passes don't have to carry the enclosing context.
    let sig = flat(
        r#"
        struct U8 { U4 lower, U4 higher, }
        extension U8 { fn new_zero(): Self { Self { lower: 0, higher: 0, } } }
        "#,
        "U8",
        "new_zero",
    );
    let ret = sig.slots.iter().find(|s| s.name == "_ret").unwrap();
    assert_eq!(ret.type_name, "U8");
    assert_eq!(ret.size, 2);
    // Struct field offsets are populated for the return slot too.
    let fields = sig.field_offsets.get("_ret").expect("ret struct fields");
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "lower");
    assert_eq!(fields[1].name, "higher");
}

#[test]
fn no_input_all_output_flattens_correctly() {
    let sig = flat(
        r#"
        extension U4 { fn zero(): Self { 0 } }
        "#,
        "U4",
        "zero",
    );
    assert_eq!(sig.input_count, 0);
    assert_eq!(sig.output_count, 1);
    assert_eq!(sig.slots.len(), 1);
    assert_eq!(sig.slots[0].name, "_ret");
    assert_eq!(sig.slots[0].size, 1);
}
