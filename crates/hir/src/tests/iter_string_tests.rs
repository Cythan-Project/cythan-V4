//! End-to-end tests for the new stdlib: `Iter<Item>`, `IntoIter<Item,
//! I>`, `ArrayIter<T, N, F>`, and `String<N>` + `StringIter<N>`. Each
//! test covers one specific behaviour — construction, iteration,
//! exhaustion, interaction with the existing `Option` enum, and
//! dispatch through the generic `Iter` trait.

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
        ("std/ArrayList.ct", load("std/ArrayList.ct")),
        ("std/Option.ct", load("std/Option.ct")),
        ("std/Iter.ct", load("std/Iter.ct")),
        ("std/ArrayIter.ct", load("std/ArrayIter.ct")),
        ("std/String.ct", load("std/String.ct")),
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

fn run_program(extra: &str, entry: &typer::FnSig) -> Vec<u8> {
    let (reg, db, fns) = compile_program(extra);
    let inlined = inline_program_full(&fns, entry, Some(&reg), Some(&db)).expect("inline");
    let mir_block = hir_to_mir(&inlined.body).expect("mir");
    struct Null;
    impl mir::RunContext for Null {
        fn input(&mut self) -> u8 { 0 }
        fn print(&mut self, _: char) {}
    }
    let mut state = mir::MemoryState::new((inlined.slot_count as usize + 128).max(512), 4);
    let mut ctx = Null;
    state.execute_block(&mir_block, &mut ctx);
    let input_count = inlined.sig.input_count as usize;
    let output_count = inlined.sig.output_count as usize;
    (0..output_count)
        .map(|i| state.get_mem((input_count + i) as u32))
        .collect()
}

// =========================================================================
// ArrayIter basic: build an Array<U4, 4, U4>, iterate, sum all entries.
// Exercises Iter<U4> impl on ArrayIter.
// =========================================================================

#[test]
fn array_iter_sum_u4() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 4, U4> a = Array::new();
                a.set(0, 1);
                a.set(1, 2);
                a.set(2, 3);
                a.set(3, 4);
                mut ArrayIter<U4, 4, U4> it = ArrayIter::new(a);
                mut U4 total = 0;
                loop {
                    if let Option::Some(v) = it.next() {
                        total += v;
                    } else {
                        break;
                    }
                }
                total
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 1 + 2 + 3 + 4 = 10
    assert_eq!(run_program(src, &entry), vec![10]);
}

// =========================================================================
// ArrayIter on a partially-filled Array still walks all N slots (it
// doesn't know which are "live" — that's what ArrayList adds). Unset
// cells read as zero.
// =========================================================================

#[test]
fn array_iter_walks_full_capacity() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 5, U4> a = Array::new();
                a.set(2, 9);
                mut ArrayIter<U4, 5, U4> it = ArrayIter::new(a);
                mut U4 count = 0;
                mut U4 total = 0;
                loop {
                    if let Option::Some(v) = it.next() {
                        count += 1;
                        total += v;
                    } else {
                        break;
                    }
                }
                count + total
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // count=5 (all cells visited), total=9 → 14
    assert_eq!(run_program(src, &entry), vec![14]);
}

// =========================================================================
// Exhaustion: calling next() after the end returns None repeatedly.
// =========================================================================

#[test]
fn array_iter_returns_none_when_done() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 2, U4> a = Array::new();
                a.set(0, 7);
                a.set(1, 8);
                mut ArrayIter<U4, 2, U4> it = ArrayIter::new(a);
                mut U4 a1 = 0;
                mut U4 a2 = 0;
                mut U4 post = 0;
                if let Option::Some(v) = it.next() { a1 = v; }
                if let Option::Some(v) = it.next() { a2 = v; }
                // Third and fourth next() should produce None.
                if let Option::Some(_) = it.next() {
                    post = 1;
                }
                if let Option::Some(_) = it.next() {
                    post += 1;
                }
                a1 + a2 + post
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 7 + 8 + 0 = 15
    assert_eq!(run_program(src, &entry), vec![15]);
}

// =========================================================================
// String basic: new, push, len, get.
// =========================================================================

#[test]
fn string_push_and_get() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut String<4> s = String::new();
                s.push(U8 { lower: 1, higher: 4, });  // 'A' = 0x41
                s.push(U8 { lower: 2, higher: 4, });  // 'B' = 0x42
                s.push(U8 { lower: 3, higher: 4, });  // 'C' = 0x43
                // len() == 3
                U4 ln = s.len();
                // first byte's lower nibble = 1
                U4 first_lo = s.get(0).lower;
                ln + first_lo
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 3 + 1 = 4
    assert_eq!(run_program(src, &entry), vec![4]);
}

#[test]
fn string_is_empty_and_is_full() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut String<2> s = String::new();
                mut U4 out = 0;
                if s.is_empty() { out += 1; }
                s.push(U8 { lower: 1, higher: 0, });
                s.push(U8 { lower: 2, higher: 0, });
                if s.is_full() { out += 2; }
                if s.is_empty() { out += 4; }
                out
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 1 + 2 = 3 (empty initially, full after 2 pushes, not empty)
    assert_eq!(run_program(src, &entry), vec![3]);
}

// =========================================================================
// StringIter: iterate chars, sum their lower nibbles.
// =========================================================================

#[test]
fn probe_inspect_iter_data() {
    // Confirm that s.iter() copies String data properly by reading
    // the iter's `.data` field after the call.
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut String<2> s = String::new();
                s.push(U8 { lower: 5, higher: 0, });
                s.push(U8 { lower: 6, higher: 0, });
                mut StringIter<2> it = s.iter();
                // Read `it.data`'s size and its chars directly.
                it.data.size + it.data.get(0).lower + it.idx
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // size=2, get(0).lower=5, idx=0 → 7
    assert_eq!(run_program(src, &entry), vec![7]);
}

#[test]
fn probe_string_iter_method_returns_iter() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut String<2> s = String::new();
                s.push(U8 { lower: 5, higher: 0, });
                s.push(U8 { lower: 6, higher: 0, });
                // Call iter() and immediately peek at next.
                mut StringIter<2> it = s.iter();
                if let Option::Some(c) = it.next() { c.lower } else { 9 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![5]);
}

#[test]
fn probe_string_iter_construction() {
    // Can we construct a StringIter and call one next() on it?
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut String<2> s = String::new();
                s.push(U8 { lower: 5, higher: 0, });
                mut StringIter<2> it = StringIter::new(s);
                if let Option::Some(c) = it.next() { c.lower } else { 9 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![5]);
}

#[test]
fn string_iter_sums_lower_nibbles() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut String<4> s = String::new();
                s.push(U8 { lower: 1, higher: 0, });
                s.push(U8 { lower: 2, higher: 0, });
                s.push(U8 { lower: 4, higher: 0, });
                mut StringIter<4> it = s.iter();
                mut U4 total = 0;
                loop {
                    if let Option::Some(c) = it.next() {
                        total += c.lower;
                    } else {
                        break;
                    }
                }
                total
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 1+2+4 = 7, plus one more iteration for the 4th unused slot's
    // `lower` (0). Wait: string's len is 3, so iter stops after 3.
    // Actually the StringIter uses self.data.len() which is size (3).
    // 1 + 2 + 4 = 7.
    assert_eq!(run_program(src, &entry), vec![7]);
}

// =========================================================================
// StringIter stops at `len`, not capacity. Unused capacity slots
// aren't walked.
// =========================================================================

#[test]
fn string_iter_stops_at_size_not_capacity() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                // Capacity 8, only 2 chars pushed.
                mut String<8> s = String::new();
                s.push(U8 { lower: 3, higher: 0, });
                s.push(U8 { lower: 5, higher: 0, });
                mut StringIter<8> it = s.iter();
                mut U4 count = 0;
                loop {
                    if let Option::Some(_) = it.next() {
                        count += 1;
                    } else {
                        break;
                    }
                }
                count
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![2]);
}

// =========================================================================
// IntoIter impl: `s.into_iter()` returns a StringIter that produces the
// same sequence as `s.iter()`.
// =========================================================================

#[test]
fn string_into_iter_dispatch() {
    let src = r#"
        use Iter;
        use IntoIter;
        extension U4 {
            fn test(): U4 {
                mut String<3> s = String::new();
                s.push(U8 { lower: 4, higher: 0, });
                s.push(U8 { lower: 5, higher: 0, });
                mut StringIter<3> it = s.into_iter();
                mut U4 total = 0;
                loop {
                    if let Option::Some(c) = it.next() {
                        total += c.lower;
                    } else {
                        break;
                    }
                }
                total
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![9]);
}

// =========================================================================
// Two iterators over the same String each produce a fresh, independent
// stream (owned copies).
// =========================================================================

#[test]
fn two_string_iters_independent() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut String<3> s = String::new();
                s.push(U8 { lower: 2, higher: 0, });
                s.push(U8 { lower: 3, higher: 0, });
                s.push(U8 { lower: 4, higher: 0, });
                mut StringIter<3> a = s.iter();
                mut StringIter<3> b = s.iter();
                // Walk 'a' once.
                mut U4 av = 0;
                if let Option::Some(c) = a.next() { av = c.lower; }
                // Walk 'b' — should also start from the beginning.
                mut U4 bv = 0;
                if let Option::Some(c) = b.next() { bv = c.lower; }
                av + bv
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // Both read 2 → 4
    assert_eq!(run_program(src, &entry), vec![4]);
}

// =========================================================================
// Counting elements via direct method dispatch (not a blanket over Iter).
// Written as an extension on ArrayIter so the method is inherent — the
// more general `impl<T, I: Iter<T>> Countable for I` pattern requires a
// generic-target unification path that isn't implemented yet.
// =========================================================================

#[test]
fn inherent_count_on_array_iter() {
    let src = r#"
        use Iter;
        extension ArrayIter<T, N, F> {
            fn count(mut self): U4 {
                mut U4 n = 0;
                loop {
                    if let Option::Some(_) = self.next() {
                        n += 1;
                    } else {
                        break;
                    }
                }
                n
            }
        }
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 3, U4> a = Array::new();
                a.set(0, 1);
                a.set(1, 2);
                a.set(2, 3);
                mut ArrayIter<U4, 3, U4> it = ArrayIter::new(a);
                it.count()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![3]);
}

// =========================================================================
// Same pattern for StringIter: an inherent `count()` method.
// =========================================================================

#[test]
fn inherent_count_on_string_iter() {
    let src = r#"
        use Iter;
        extension StringIter<N> {
            fn count(mut self): U4 {
                mut U4 n = 0;
                loop {
                    if let Option::Some(_) = self.next() {
                        n += 1;
                    } else {
                        break;
                    }
                }
                n
            }
        }
        extension U4 {
            fn test(): U4 {
                mut String<4> s = String::new();
                s.push(U8 { lower: 1, higher: 0, });
                s.push(U8 { lower: 2, higher: 0, });
                s.push(U8 { lower: 3, higher: 0, });
                mut StringIter<4> it = s.iter();
                it.count()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![3]);
}

// =========================================================================
// Early termination in an inherent iter-walker.
// =========================================================================

#[test]
fn iter_inherent_with_early_break() {
    let src = r#"
        use Iter;
        extension ArrayIter<T, N, F> {
            fn take3_count(mut self): U4 {
                mut U4 n = 0;
                loop {
                    if n == 3 { break; }
                    if let Option::Some(_) = self.next() {
                        n += 1;
                    } else {
                        break;
                    }
                }
                n
            }
        }
        extension U4 {
            fn test(): U4 {
                mut Array<U4, 10, U4> a = Array::new();
                a.set(0, 1);
                mut ArrayIter<U4, 10, U4> it = ArrayIter::new(a);
                it.take3_count()
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![3]);
}

// =========================================================================
// Empty string: iter() produces no items.
// =========================================================================

#[test]
fn empty_string_iter_yields_none_immediately() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut String<4> s = String::new();
                mut StringIter<4> it = s.iter();
                if let Option::Some(_) = it.next() { 9 } else { 1 }
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    assert_eq!(run_program(src, &entry), vec![1]);
}

// =========================================================================
// String past capacity: push silently drops.
// =========================================================================

#[test]
fn string_push_past_capacity_drops() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut String<2> s = String::new();
                s.push(U8 { lower: 1, higher: 0, });
                s.push(U8 { lower: 2, higher: 0, });
                s.push(U8 { lower: 9, higher: 0, });  // dropped
                s.len() + s.get(0).lower + s.get(1).lower
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 2 + 1 + 2 = 5
    assert_eq!(run_program(src, &entry), vec![5]);
}

// =========================================================================
// String set overwrites a specific index.
// =========================================================================

#[test]
fn string_set_overwrites() {
    let src = r#"
        extension U4 {
            fn test(): U4 {
                mut String<3> s = String::new();
                s.push(U8 { lower: 1, higher: 0, });
                s.push(U8 { lower: 2, higher: 0, });
                s.set(0, U8 { lower: 8, higher: 0, });
                s.get(0).lower + s.get(1).lower
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // 8 + 2 = 10
    assert_eq!(run_program(src, &entry), vec![10]);
}

// =========================================================================
// Mix: accumulate into a String from another String's iterator.
// =========================================================================

#[test]
fn iter_one_string_into_another() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                mut String<3> src = String::new();
                src.push(U8 { lower: 4, higher: 0, });
                src.push(U8 { lower: 5, higher: 0, });
                src.push(U8 { lower: 6, higher: 0, });
                mut String<5> dst = String::new();
                mut StringIter<3> it = src.iter();
                loop {
                    if let Option::Some(c) = it.next() {
                        dst.push(c);
                    } else {
                        break;
                    }
                }
                dst.len() + dst.get(0).lower + dst.get(2).lower
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // len=3, first lower=4, third lower=6 → 13
    assert_eq!(run_program(src, &entry), vec![13]);
}

// =========================================================================
// Two ArrayIters over different-type arrays, each with its own
// concrete monomorph (ArrayIter<U4, N, U4> and ArrayIter<Bool, M, U4>).
// =========================================================================

#[test]
fn array_iter_monomorph_per_element_type() {
    let src = r#"
        use Iter;
        extension U4 {
            fn test(): U4 {
                // Path 1: ArrayIter<U4, 2, U4>
                mut Array<U4, 2, U4> a = Array::new();
                a.set(0, 2); a.set(1, 5);
                mut ArrayIter<U4, 2, U4> ia = ArrayIter::new(a);
                mut U4 sum_a = 0;
                loop {
                    if let Option::Some(v) = ia.next() { sum_a += v; } else { break; }
                }
                // Path 2: ArrayIter<Bool, 3, U4>
                mut Array<Bool, 3, U4> b = Array::new();
                b.set(0, true); b.set(1, false); b.set(2, true);
                mut ArrayIter<Bool, 3, U4> ib = ArrayIter::new(b);
                mut U4 count_b = 0;
                loop {
                    if let Option::Some(v) = ib.next() {
                        if v { count_b += 1; }
                    } else { break; }
                }
                sum_a + count_b
            }
        }
    "#;
    let entry = typer::FnSig::new("U4", "test");
    // sum_a=7, count_b=2 → 9
    assert_eq!(run_program(src, &entry), vec![9]);
}
