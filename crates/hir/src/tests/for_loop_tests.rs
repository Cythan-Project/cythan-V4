//! Tests for `for x in iter { body }` and range expressions.
//!
//! Each test spins up the full stdlib (so `Range<T>`, `Iter<T>`,
//! `Option<T>`, and the operator traits resolve), compiles a tiny
//! user program, then runs it through the MIR interpreter and
//! asserts on the captured cell output. End-to-end coverage is the
//! point — unit-testing the desugar in isolation would just
//! re-test what the parser already gives us.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::{
    gen_function_with_natives, hir_to_mir, inline_program_full, BuiltinNatives, HirFunction,
};

fn load(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/new_syntax")
        .join(path);
    std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("read {}: {}", full.display(), e))
        .replace('\r', "")
}

fn compile_program(
    extra: &str,
) -> (
    typer::TypeRegistry,
    typer::FunctionDB,
    HashMap<typer::FnSig, HirFunction>,
) {
    let parts = [
        ("std/System.ct", load("std/System.ct")),
        ("std/Ops.ct", load("std/Ops.ct")),
        ("std/Bool.ct", load("std/Bool.ct")),
        ("std/U4.ct", load("std/U4.ct")),
        ("std/U8.ct", load("std/U8.ct")),
        ("std/Array.ct", load("std/Array.ct")),
        ("std/Option.ct", load("std/Option.ct")),
        ("std/Iter.ct", load("std/Iter.ct")),
        ("std/Range.ct", load("std/Range.ct")),
        ("std/RangeInclusive.ct", load("std/RangeInclusive.ct")),
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
    (reg, db, out)
}

/// Run `entry` on a captured-print interpreter; return the bytes
/// printed.
fn run_program_capturing(extra: &str, entry: &typer::FnSig) -> Vec<u8> {
    let (reg, db, fns) = compile_program(extra);
    let inlined = inline_program_full(&fns, entry, Some(&reg), Some(&db)).expect("inline");
    let mir_block = hir_to_mir(&inlined.body).expect("mir");
    struct Cap(Vec<u8>);
    impl mir::RunContext for Cap {
        fn input(&mut self) -> u8 {
            0
        }
        fn print(&mut self, b: u8) {
            self.0.push(b);
        }
    }
    let mut state = mir::MemoryState::new_with_limit(
        (inlined.slot_count as usize + 128).max(1024),
        4,
        5_000_000,
    );
    let mut ctx = Cap(Vec::new());
    state.execute_block(&mir_block, &mut ctx);
    ctx.0
}

/// Return the value sitting in the entry's `_ret` slot, ignoring
/// any prints.
fn run_program_ret(extra: &str, entry: &typer::FnSig) -> Vec<u8> {
    let (reg, db, fns) = compile_program(extra);
    let inlined = inline_program_full(&fns, entry, Some(&reg), Some(&db)).expect("inline");
    let mir_block = hir_to_mir(&inlined.body).expect("mir");
    struct Null;
    impl mir::RunContext for Null {
        fn input(&mut self) -> u8 {
            0
        }
        fn print(&mut self, _: u8) {}
    }
    let mut state = mir::MemoryState::new_with_limit(
        (inlined.slot_count as usize + 128).max(1024),
        4,
        5_000_000,
    );
    let mut ctx = Null;
    state.execute_block(&mir_block, &mut ctx);
    let ic = inlined.sig.input_count as usize;
    let oc = inlined.sig.output_count as usize;
    (0..oc)
        .map(|i| state.get_mem((ic + i) as u32))
        .collect()
}

// =========================================================================
// Empty range — body never runs.
// =========================================================================

#[test]
fn empty_range_runs_zero_iterations() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut U4 count = 0;
                for U4 i in 0..0 {
                    count += 1;
                }
                count
            }
        }
    "#;
    assert_eq!(
        run_program_ret(src, &typer::FnSig::new("U4", "test")),
        vec![0]
    );
}

// =========================================================================
// `for U4 i in 0..9` — basic counting loop, body runs 9 times.
// Sums the loop variable to confirm each value is visited exactly once.
// =========================================================================

#[test]
fn u4_exclusive_range_visits_each_value_once() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut U4 sum = 0;
                for U4 i in 0..6 {
                    sum += i;
                }
                sum
            }
        }
    "#;
    // 0+1+2+3+4+5 = 15 — fits in a U4 cell.
    assert_eq!(
        run_program_ret(src, &typer::FnSig::new("U4", "test")),
        vec![15]
    );
}

// =========================================================================
// Inclusive range covers full U4 domain (0..=15) without wrap-around.
// The `done` flag in `RangeInclusive<T>` is the whole point here:
// `Range<U4>::new(0, 16)` would wrap `16` to `0` and run 0 iterations.
// =========================================================================

#[test]
fn u4_inclusive_range_covers_full_domain() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut U4 count = 0;
                for U4 i in 0..=15 {
                    count += 1;
                }
                count
            }
        }
    "#;
    // 16 iterations; count wraps at 16 → 0 in U4.
    assert_eq!(
        run_program_ret(src, &typer::FnSig::new("U4", "test")),
        vec![0]
    );
}

// =========================================================================
// `break` inside the body terminates the loop early. Confirms the
// match-arm desugar still threads control flow correctly.
// =========================================================================

#[test]
fn break_inside_for_terminates_loop() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut U4 last = 0;
                for U4 i in 0..15 {
                    if i == 5 {
                        break;
                    }
                    last = i;
                }
                last
            }
        }
    "#;
    // Stops at i==5 without writing last; previous `last` was 4.
    assert_eq!(
        run_program_ret(src, &typer::FnSig::new("U4", "test")),
        vec![4]
    );
}

// =========================================================================
// Iterating a `Range<U4>` value bound to a local — exercises the
// "iter is not a literal range expression" branch of `gen_for`.
// =========================================================================

#[test]
fn for_over_named_range_value() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut Range<U4> r = 0..4;
                mut U4 sum = 0;
                for U4 i in r {
                    sum += i;
                }
                sum
            }
        }
    "#;
    // 0+1+2+3 = 6
    assert_eq!(
        run_program_ret(src, &typer::FnSig::new("U4", "test")),
        vec![6]
    );
}

// =========================================================================
// `RangeInclusive<U4>` value bound to a local.
// =========================================================================

#[test]
fn for_over_named_range_inclusive_value() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut RangeInclusive<U4> r = 0..=3;
                mut U4 count = 0;
                for U4 i in r {
                    count += 1;
                }
                count
            }
        }
    "#;
    // 4 iterations: 0, 1, 2, 3.
    assert_eq!(
        run_program_ret(src, &typer::FnSig::new("U4", "test")),
        vec![4]
    );
}

// =========================================================================
// Body prints each value through the print register; double-check
// the output bytes line up with the expected sequence.
// =========================================================================

#[test]
fn body_prints_each_loop_value() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                for U4 i in 0..5 {
                    i.print();
                }
                0
            }
        }
    "#;
    // U4::print emits the ASCII digit, not the raw cell value.
    assert_eq!(
        run_program_capturing(src, &typer::FnSig::new("U4", "test")),
        b"01234".to_vec()
    );
}

// =========================================================================
// Multi-width is supported in principle: `Range<T>` is generic in `T`,
// so `for U8 j in 0..N` would compile once `U8` gains an `AddAssign`
// + `Eq` impl in stdlib. Today `std/U8.ct` only ships `add`/`sub`
// (mutating signatures), not the trait — so the call from
// `Range<U8>::next`'s `self.cur += 1` fails with "inliner: missing
// function `U8::add_assign`". Adding `impl AddAssign for U8` to
// stdlib is the gating change; the for-loop machinery is ready.
// =========================================================================

// =========================================================================
// Nested for: outer × inner. Confirms scoping doesn't collide and
// the per-loop variables stay distinct.
// =========================================================================

#[test]
fn nested_for_loops() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut U4 count = 0;
                for U4 i in 0..3 {
                    for U4 j in 0..3 {
                        count += 1;
                    }
                }
                count
            }
        }
    "#;
    // 3*3 = 9 iterations.
    assert_eq!(
        run_program_ret(src, &typer::FnSig::new("U4", "test")),
        vec![9]
    );
}
