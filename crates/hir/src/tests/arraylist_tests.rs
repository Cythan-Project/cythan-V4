//! End-to-end tests for `ArrayList<Index, N, T>`. Same harness shape as
//! the array tests: compile stdlib + user source, inline from the test
//! function, run in the MIR interpreter, assert on output cells.
//!
//! Tests are grouped by the compiler capability they exercise. The flat
//! tests need user-defined-struct monomorphization (Phase: ArrayList
//! plan, step 1+2+3 in `arraylist_plan.md`). Nested tests additionally
//! need the monomorphizer to size a struct whose fields reference
//! *other* user-defined generic structs — that falls out of the recursive
//! sizing walk, but is called out separately so regressions are easy to
//! locate.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::{
    gen_function_with_natives, hir_to_mir, inline::inline_program_with_registry,
    BuiltinNatives, HirFunction,
};

fn load(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/new_syntax")
        .join(path);
    std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("read {}: {}", full.display(), e))
        .replace('\r', "")
}

/// Compile stdlib (System, Ops, Bool, U4, U8, Array, ArrayList) + the given
/// user source into a `(registry, hir_fns)` pair ready for inlining.
fn compile_program(extra: &str) -> (typer::TypeRegistry, HashMap<typer::FnSig, HirFunction>) {
    let parts = [
        ("std/System.ct", load("std/System.ct")),
        ("std/Ops.ct", load("std/Ops.ct")),
        ("std/Bool.ct", load("std/Bool.ct")),
        ("std/U4.ct", load("std/U4.ct")),
        ("std/U8.ct", load("std/U8.ct")),
        ("std/Array.ct", load("std/Array.ct")),
        ("std/ArrayList.ct", load("std/ArrayList.ct")),
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
    (reg, out)
}

fn run_program(extra: &str, entry: &typer::FnSig, args: &[u8]) -> Vec<u8> {
    let (reg, fns) = compile_program(extra);
    let inlined = inline_program_with_registry(&fns, entry, Some(&reg)).expect("inline");
    let mir_block = hir_to_mir(&inlined.body).expect("mir conv");

    struct Null;
    impl mir::RunContext for Null {
        fn input(&mut self) -> u8 { 0 }
        fn print(&mut self, _: char) {}
    }
    let mut state = mir::MemoryState::new((inlined.slot_count as usize + 64).max(256), 4);
    for (i, v) in args.iter().enumerate() {
        state.set_mem(i as u32, *v);
    }
    let mut ctx = Null;
    state.execute_block(&mir_block, &mut ctx);

    let input_count = inlined.sig.input_count as usize;
    let output_count = inlined.sig.output_count as usize;
    (0..output_count)
        .map(|i| state.get_mem((input_count + i) as u32))
        .collect()
}

// ---------- Construction & introspection ---------------------------------

#[test]
#[ignore = "pending user-struct monomorphization"]
fn new_list_is_empty() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, U4> list = ArrayList::new();
                if list.is_empty() { 1 } else { 0 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![1]);
}

#[test]
#[ignore = "pending user-struct monomorphization"]
fn new_list_has_zero_len() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, U4> list = ArrayList::new();
                list.len()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![0]);
}

#[test]
#[ignore = "pending user-struct monomorphization"]
fn new_list_capacity_equals_template_n() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 7, U4> list = ArrayList::new();
                list.capacity()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![7]);
}

#[test]
#[ignore = "pending user-struct monomorphization"]
fn new_list_is_not_full() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, U4> list = ArrayList::new();
                if list.is_full() { 1 } else { 0 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![0]);
}

// ---------- push / get round-trip -----------------------------------------

#[test]
#[ignore = "pending user-struct monomorphization"]
fn push_then_get_returns_value() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, U4> list = ArrayList::new();
                list.push(9);
                list.get(0)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![9]);
}

#[test]
#[ignore = "pending user-struct monomorphization"]
fn push_increments_len() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, U4> list = ArrayList::new();
                list.push(5);
                list.push(6);
                list.len()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![2]);
}

#[test]
#[ignore = "pending user-struct monomorphization"]
fn multiple_pushes_land_at_consecutive_indices() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, U4> list = ArrayList::new();
                list.push(1);
                list.push(2);
                list.push(4);
                list.push(8);
                list.get(0) + list.get(1) + list.get(2) + list.get(3)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![15]);
}

// ---------- push past capacity is a no-op --------------------------------

#[test]
#[ignore = "pending user-struct monomorphization"]
fn push_past_capacity_is_silent_noop() {
    // Capacity 2, push 3 times. The third push drops on the floor; len
    // stays at 2, and slots [0] and [1] keep their first two values.
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 2, U4> list = ArrayList::new();
                list.push(3);
                list.push(5);
                list.push(9);
                list.len() + list.get(0) + list.get(1)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 2 + 3 + 5 = 10.
    assert_eq!(run_program(src, &entry, &[]), vec![10]);
}

#[test]
#[ignore = "pending user-struct monomorphization"]
fn is_full_after_n_pushes() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 3, U4> list = ArrayList::new();
                list.push(1);
                list.push(2);
                list.push(3);
                if list.is_full() { 1 } else { 0 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![1]);
}

#[test]
#[ignore = "pending user-struct monomorphization"]
fn is_empty_false_after_push() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, U4> list = ArrayList::new();
                list.push(1);
                if list.is_empty() { 1 } else { 0 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![0]);
}

// ---------- pop -----------------------------------------------------------

#[test]
#[ignore = "pending user-struct monomorphization"]
fn pop_returns_last_pushed_value() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, U4> list = ArrayList::new();
                list.push(5);
                list.push(9);
                list.pop()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![9]);
}

#[test]
#[ignore = "pending user-struct monomorphization"]
fn pop_decrements_len() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, U4> list = ArrayList::new();
                list.push(1);
                list.push(2);
                list.pop();
                list.len()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![1]);
}

#[test]
#[ignore = "pending user-struct monomorphization"]
fn pop_after_fill_leaves_correct_state() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 3, U4> list = ArrayList::new();
                list.push(1);
                list.push(2);
                list.push(3);
                list.pop();
                if list.is_full() { 100 } else { list.len() }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![2]);
}

// ---------- set (mutate in place) -----------------------------------------

#[test]
#[ignore = "pending user-struct monomorphization"]
fn set_overwrites_element() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, U4> list = ArrayList::new();
                list.push(1);
                list.push(2);
                list.set(0, 9);
                list.get(0)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![9]);
}

// ---------- iteration pattern --------------------------------------------

#[test]
#[ignore = "pending user-struct monomorphization"]
fn iterate_and_sum() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 4, U4> list = ArrayList::new();
                list.push(1);
                list.push(2);
                list.push(3);
                list.push(4);
                mut U4 i = 0;
                mut U4 total = 0;
                loop {
                    if i == list.len() { break; }
                    total += list.get(i);
                    i += 1;
                }
                total
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 1+2+3+4 = 10
    assert_eq!(run_program(src, &entry, &[]), vec![10]);
}

// ---------- element-type variety -----------------------------------------

#[test]
#[ignore = "pending user-struct monomorphization"]
fn arraylist_of_bool() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 3, Bool> list = ArrayList::new();
                list.push(true);
                list.push(false);
                list.push(true);
                // count trues
                mut U4 i = 0;
                mut U4 count = 0;
                loop {
                    if i == list.len() { break; }
                    if list.get(i) { count += 1; }
                    i += 1;
                }
                count
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![2]);
}

#[test]
#[ignore = "pending user-struct monomorphization"]
fn arraylist_of_u8_round_trip() {
    // U8 is 2 cells per element — a stress test for element_size > 1.
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 3, U8> list = ArrayList::new();
                list.push(U8 { lower: 5, higher: 6, });
                list.push(U8 { lower: 1, higher: 2, });
                list.get(0).higher
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![6]);
}

// ---------- nested ArrayList<U4, M, ArrayList<U4, K, U4>> ---------------

#[test]
#[ignore = "nested — needs recursive user-struct sizing to handle element-of-ArrayList"]
fn nested_arraylist_push_to_inner() {
    // Outer capacity 2 lists of inner capacity 3.
    //
    // The real challenge: sizing `Array<ArrayList<U4, 3, U4>, 2, U4>` —
    // a native Array whose element is a user-defined generic struct. The
    // native Array sizer calls `resolve_type_size` on the element type,
    // which must recursively walk `ArrayList<U4, 3, U4>`'s fields. That
    // should "just work" once the user-struct monomorphization path is
    // wired (it reuses the same registry helper).
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 2, ArrayList<U4, 3, U4>> outer = ArrayList::new();
                mut ArrayList<U4, 3, U4> inner = ArrayList::new();
                inner.push(7);
                outer.push(inner);
                outer.get(0).get(0)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![7]);
}

#[test]
#[ignore = "nested — see above"]
fn nested_arraylist_multiple_inner_elements() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 2, ArrayList<U4, 3, U4>> outer = ArrayList::new();
                mut ArrayList<U4, 3, U4> inner_a = ArrayList::new();
                inner_a.push(1);
                inner_a.push(2);
                outer.push(inner_a);
                mut ArrayList<U4, 3, U4> inner_b = ArrayList::new();
                inner_b.push(4);
                outer.push(inner_b);
                outer.get(0).get(0) + outer.get(0).get(1) + outer.get(1).get(0)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 1 + 2 + 4 = 7
    assert_eq!(run_program(src, &entry, &[]), vec![7]);
}

#[test]
#[ignore = "nested — see above"]
fn nested_arraylist_inner_len_independent() {
    // Two inner lists with different current lengths — verifies that the
    // outer list's monomorph propagates the inner generic to `get`.
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut ArrayList<U4, 2, ArrayList<U4, 3, U4>> outer = ArrayList::new();
                mut ArrayList<U4, 3, U4> short = ArrayList::new();
                short.push(1);
                outer.push(short);
                mut ArrayList<U4, 3, U4> full = ArrayList::new();
                full.push(10);
                full.push(11);
                full.push(12);
                outer.push(full);
                outer.get(0).len() + outer.get(1).len()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![1 + 3]);
}
