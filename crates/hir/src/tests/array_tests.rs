//! End-to-end tests for `Array<T, N, F>` — the language's statically-sized
//! array. After the upcoming work, `Array` methods (`new`, `get`, `set`,
//! `len`) are synthesized on type monomorphization: the compiler generates
//! a dedicated `HirFunction` per concrete `Array<T, N, F>` whose body is a
//! `Match` on the index cell.
//!
//! Each test defines the expected end-to-end behavior. They're marked
//! `#[ignore]` until the implementation lands; unignore them as each stage
//! closes.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::{
    gen_function_with_natives, hir_to_mir, inline::inline_program_with_registry, BuiltinNatives,
    HirFunction,
};

fn load(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/new_syntax")
        .join(path);
    std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("read {}: {}", full.display(), e))
        .replace('\r', "")
}

/// Compile stdlib + extra user code. Returns `(registry, hir_functions)`.
fn compile_program(extra: &str) -> (typer::TypeRegistry, HashMap<typer::FnSig, HirFunction>) {
    let parts = [
        ("std/System.ct", load("std/System.ct")),
        ("std/Ops.ct", load("std/Ops.ct")),
        ("std/Bool.ct", load("std/Bool.ct")),
        ("std/U4.ct", load("std/U4.ct")),
        ("std/U8.ct", load("std/U8.ct")),
        ("std/Array.ct", load("std/Array.ct")),
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
    let as_refs: Vec<(&str, &[_])> = parsed.iter().map(|(n, v)| (n.as_str(), v.as_slice())).collect();
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

/// Run `entry` with the given initial input-slot bytes; return the output cells.
fn run_program(extra: &str, entry: &typer::FnSig, args: &[u8]) -> Vec<u8> {
    let (reg, fns) = compile_program(extra);
    let inlined = inline_program_with_registry(&fns, entry, Some(&reg)).expect("inline");
    let mir_block = hir_to_mir(&inlined.body).expect("mir conv");

    struct Null;
    impl mir::RunContext for Null {
        fn input(&mut self) -> u8 {
            0
        }
        fn print(&mut self, _: u8) {}
    }
    let mut state = mir::MemoryState::new_with_limit((inlined.slot_count as usize + 32).max(128), 4, 5_000_000);
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

// ---------- basic set/get on Array<U4, 4, U4> ------------------------------

#[test]
fn array_u4_set_then_get_returns_value() {
    // Build an Array<U4, 4, U4>, set index 2 to 7, read back.
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 4, U4> arr = Array::new();
                arr.set(2, 7);
                arr.get(2)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![7]);
}

#[test]

fn array_u4_get_unset_returns_zero() {
    // Array::new zeroes all cells; reading an unset index returns 0.
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 4, U4> arr = Array::new();
                arr.get(3)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![0]);
}

#[test]

fn array_u4_set_at_different_indices_reads_independent() {
    // Each index is an independent cell.
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 4, U4> arr = Array::new();
                arr.set(0, 1);
                arr.set(1, 2);
                arr.set(2, 4);
                arr.set(3, 8);
                arr.get(0) + arr.get(1) + arr.get(2) + arr.get(3)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![15]);
}

#[test]

fn array_set_then_overwrite() {
    // Setting the same index twice keeps the last value.
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 4, U4> arr = Array::new();
                arr.set(1, 5);
                arr.set(1, 9);
                arr.get(1)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![9]);
}

// ---------- dynamic index (runtime-computed) -------------------------------

#[test]

fn array_get_with_runtime_index_arg() {
    let src = r#"
        extension U4 {
            fn test(U4 i): U4 {
                mut Array<U4, 4, U4> arr = Array::new();
                arr.set(0, 10);
                arr.set(1, 11);
                arr.set(2, 12);
                arr.set(3, 13);
                arr.get(i)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[0]), vec![10]);
    assert_eq!(run_program(src, &entry, &[1]), vec![11]);
    assert_eq!(run_program(src, &entry, &[2]), vec![12]);
    assert_eq!(run_program(src, &entry, &[3]), vec![13]);
}

// ---------- element types larger than 1 cell ------------------------------

#[test]

fn array_of_u8_round_trip_two_cell_element() {
    // U8 is 2 cells — verifies that each element's cell range is copied
    // correctly, not just a single cell.
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut Array<U8, 3, U4> arr = Array::new();
                arr.set(1, U8 { lower: 5, higher: 6, });
                arr.get(1).lower
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![5]);
}

#[test]

fn array_of_u8_reads_higher_cell() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut Array<U8, 3, U4> arr = Array::new();
                arr.set(0, U8 { lower: 3, higher: 7, });
                arr.get(0).higher
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![7]);
}

// ---------- len -----------------------------------------------------------

#[test]

fn array_len_returns_compile_time_size() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 5, U4> arr = Array::new();
                arr.len()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![5]);
}

// ---------- iteration pattern ---------------------------------------------

#[test]

fn array_iterate_and_sum() {
    // Idiomatic loop: while i < len, add arr[i], increment i. Exercises
    // Array::get with every index in bounds and interaction with user's
    // control flow.
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 4, U4> arr = Array::new();
                arr.set(0, 1);
                arr.set(1, 2);
                arr.set(2, 3);
                arr.set(3, 4);
                mut U4 i = 0;
                mut U4 total = 0;
                loop {
                    if i == arr.len() {
                        break;
                    }
                    total += arr.get(i);
                    i += 1;
                }
                total
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![10]);
}

// ---------- array as a struct field ---------------------------------------

#[test]

fn array_as_struct_field_survives_get_set() {
    // Morpion-style: a struct wraps the array and exposes thin wrappers.
    let src = r#"
        struct Grid {
            Array<U4, 4, U4> cells,
        }
        extension Grid {
            fn new(): Self {
                Self { cells: Array::new(), }
            }
            fn put(mut self, U4 i, U4 v) {
                self.cells.set(i, v);
            }
            fn peek(self, U4 i): U4 {
                self.cells.get(i)
            }
        }
        extension U4 {
            fn test(): U4 {
                mut Grid g = Grid::new();
                g.put(2, 8);
                g.peek(2)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![8]);
}

// ---------- same Array<T, N, F> used twice → one monomorph cached ---------

#[test]

fn two_arrays_same_concrete_type_share_monomorph() {
    // Two independent Array<U4, 3, U4> variables. Each is its own memory
    // region, but the synthesized `get`/`set` functions should be shared
    // (the monomorph cache hits on the second one).
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 3, U4> a = Array::new();
                mut Array<U4, 3, U4> b = Array::new();
                a.set(0, 2);
                b.set(0, 5);
                a.get(0) + b.get(0)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry, &[]), vec![7]);
}

// ---------- different Array<T, N, F> instantiations are distinct ----------

#[test]

fn array_different_sizes_produce_distinct_monomorphs() {
    // Array<U4, 3, U4> and Array<U4, 5, U4> must both work in the same
    // program — their `get`/`set`/`len` bodies differ (3-arm match vs
    // 5-arm match, 3 cells vs 5 cells).
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 3, U4> small = Array::new();
                mut Array<U4, 5, U4> big = Array::new();
                small.set(2, 4);
                big.set(4, 8);
                small.len() + big.len() + small.get(2) + big.get(4)
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 3 + 5 + 4 + 8 = 20, which wraps mod 16 on U4 → 4.
    assert_eq!(run_program(src, &entry, &[]), vec![(3 + 5 + 4 + 8) % 16]);
}
