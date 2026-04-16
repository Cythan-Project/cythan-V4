//! HIR interpreter tests.
//!
//! Each test parses source → builds registry/FnDB → compiles every `Simple`
//! function in the DB to HIR → runs the named entry function with the given
//! inputs and asserts on the outputs.

use std::collections::HashMap;

use crate::{gen_function, CapturedIo, HirFunction, Interpreter};

/// Compile every `Simple` function in the source's `FunctionDB` into HIR
/// and return the map.
pub fn compile_all(src: &str) -> HashMap<typer::FnSig, HirFunction> {
    let items = new_parser::parse(src).expect("parse");
    let reg = typer::TypeRegistry::from_items(&items).expect("typer");
    let db = typer::FunctionDB::from_registry(&reg).expect("fn_db");
    let mut out = HashMap::new();
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            let hir = gen_function(k, s, &reg, &db).expect("hir gen");
            out.insert(k.clone(), hir);
        }
    }
    out
}

fn run_noio(
    src: &str,
    type_name: &str,
    method: &str,
    args: &[u8],
) -> Vec<u8> {
    let fns = compile_all(src);
    let mut io = CapturedIo::new();
    let mut interp = Interpreter::new(fns, &mut io);
    interp
        .run(&typer::FnSig::new(type_name, method), args)
        .expect("interp")
}

fn run_with_io(
    src: &str,
    type_name: &str,
    method: &str,
    args: &[u8],
    stdin: Vec<u8>,
) -> (Vec<u8>, CapturedIo) {
    let fns = compile_all(src);
    let mut io = CapturedIo::with_input(stdin);
    let out = {
        let mut interp = Interpreter::new(fns, &mut io);
        interp
            .run(&typer::FnSig::new(type_name, method), args)
            .expect("interp")
    };
    (out, io)
}

// ---------- pure-value tests ------------------------------------------------

#[test]
fn returns_literal_number() {
    let src = "extension U4 { fn forty_two(): Self { 15 } }";
    let out = run_noio(src, "U4", "forty_two", &[]);
    assert_eq!(out, vec![15]);
}

#[test]
fn identity_copies_input_to_output() {
    let src = "extension U4 { fn id(U4 x): U4 { x } }";
    let out = run_noio(src, "U4", "id", &[7]);
    assert_eq!(out, vec![7]);
}

#[test]
fn bool_true_false_literals() {
    let src = r#"
        struct Bool { U4 value, }
        extension Bool {
            fn t(): Self { true }
            fn f(): Self { false }
        }
    "#;
    assert_eq!(run_noio(src, "Bool", "t", &[]), vec![1]);
    assert_eq!(run_noio(src, "Bool", "f", &[]), vec![0]);
}

#[test]
fn inc_dec_via_compound_assign() {
    // `mut U4 x; x += 1; x += 1; x -= 1` on initial 3 → 4
    let src = r#"
        extension U4 {
            fn bump(mut U4 x): U4 {
                x += 1;
                x += 1;
                x -= 1;
                x
            }
        }
    "#;
    let out = run_noio(src, "U4", "bump", &[3]);
    assert_eq!(out, vec![4]);
}

#[test]
fn inc_wraps_at_16() {
    // U4 is 4 bits → 15 + 1 wraps to 0.
    let src = "extension U4 { fn wrap(mut U4 x): U4 { x += 1; x } }";
    let out = run_noio(src, "U4", "wrap", &[15]);
    assert_eq!(out, vec![0]);
}

// ---------- control flow ----------------------------------------------------

#[test]
fn if_branches_on_bool_input() {
    let src = r#"
        struct Bool { U4 value, }
        extension Bool {
            fn pick(self): U4 {
                if self { 3 } else { 7 }
            }
        }
    "#;
    assert_eq!(run_noio(src, "Bool", "pick", &[1]), vec![3]);
    assert_eq!(run_noio(src, "Bool", "pick", &[0]), vec![7]);
}

#[test]
fn match_selects_arm_by_discriminant() {
    let src = r#"
        enum Cell { Empty, O, X, }
        extension Cell {
            fn label(self): U4 {
                match self {
                    Self::Empty => 0,
                    Self::O => 8,
                    _ => 15,
                }
            }
        }
    "#;
    assert_eq!(run_noio(src, "Cell", "label", &[0]), vec![0]);
    assert_eq!(run_noio(src, "Cell", "label", &[1]), vec![8]);
    assert_eq!(run_noio(src, "Cell", "label", &[2]), vec![15]);
}

#[test]
fn loop_with_break_using_if_on_variable() {
    // `mut U4 i = 0; loop { i += 1; if i { break; } } i`
    // Here `if i` evaluates the cond directly; on U4 the cond is copied to
    // a 1-cell Bool slot. The loop breaks on the first non-zero i (= 1).
    let src = r#"
        extension U4 {
            fn first_nonzero(): U4 {
                mut U4 i = 0;
                loop {
                    i += 1;
                    if i { break; }
                }
                i
            }
        }
    "#;
    let out = run_noio(src, "U4", "first_nonzero", &[]);
    assert_eq!(out, vec![1]);
}

#[test]
fn return_exits_early_with_value() {
    let src = r#"
        extension U4 {
            fn five(): U4 {
                return 5;
                0
            }
        }
    "#;
    let out = run_noio(src, "U4", "five", &[]);
    assert_eq!(out, vec![5]);
}

// ---------- struct / enum construction --------------------------------------

#[test]
fn struct_literal_builds_cells_at_offsets() {
    // U8 { lower: 3, higher: 7 } → cells = [3, 7]
    let src = r#"
        struct U8 { U4 lower, U4 higher, }
        extension U8 {
            fn new(U4 a, U4 b): Self {
                Self { lower: a, higher: b, }
            }
        }
    "#;
    let out = run_noio(src, "U8", "new", &[3, 7]);
    assert_eq!(out, vec![3, 7]);
}

#[test]
fn enum_unit_variant_returns_discriminant() {
    let src = r#"
        enum Cell { Empty, O, X, }
        extension Cell {
            fn o(): Self { Self::O }
        }
    "#;
    let out = run_noio(src, "Cell", "o", &[]);
    assert_eq!(out, vec![1]);
}

#[test]
fn field_read_picks_correct_cell() {
    let src = r#"
        struct U8 { U4 lower, U4 higher, }
        extension U8 {
            fn get_higher(self): U4 { self.higher }
            fn get_lower(self): U4 { self.lower }
        }
    "#;
    assert_eq!(run_noio(src, "U8", "get_lower", &[3, 7]), vec![3]);
    assert_eq!(run_noio(src, "U8", "get_higher", &[3, 7]), vec![7]);
}

// ---------- cross-function calls --------------------------------------------

#[test]
fn static_call_dispatches_to_callee_and_returns() {
    let src = r#"
        extension U4 {
            fn echo(U4 x): Self { x }
            fn caller(): U4 { U4::echo(9) }
        }
    "#;
    let out = run_noio(src, "U4", "caller", &[]);
    assert_eq!(out, vec![9]);
}

#[test]
fn method_call_on_self_propagates_input() {
    let src = r#"
        extension U4 {
            fn double_ret(self): Self { self }
            fn caller(U4 x): U4 { x.double_ret() }
        }
    "#;
    let out = run_noio(src, "U4", "caller", &[6]);
    assert_eq!(out, vec![6]);
}

#[test]
fn nested_call_chain() {
    let src = r#"
        extension U4 {
            fn a(U4 x): Self { x }
            fn b(U4 x): Self { U4::a(x) }
            fn c(U4 x): Self { U4::b(x) }
        }
    "#;
    let out = run_noio(src, "U4", "c", &[11]);
    assert_eq!(out, vec![11]);
}

#[test]
fn call_with_two_args_passes_them_correctly() {
    // echo_second returns its second arg — proves arg ordering works.
    let src = r#"
        extension U4 {
            fn echo_second(U4 _a, U4 b): Self { b }
            fn caller(): U4 { U4::echo_second(1, 14) }
        }
    "#;
    let out = run_noio(src, "U4", "caller", &[]);
    assert_eq!(out, vec![14]);
}

// ---------- I/O via register protocol --------------------------------------

#[test]
fn direct_write_register_prints_char() {
    // Hand-built HIR that prints 'A' (byte 0x41 → high-nibble=4, low-nibble=1):
    //   WriteRegister(1, imm 4);  // high nibble
    //   WriteRegister(2, imm 1);  // low nibble
    //   WriteRegister(0, imm 1);  // command = 1 (print)
    use crate::ir::{HirBlock, HirFunction, HirOp};
    use either::Either;

    let body = HirBlock {
        ops: vec![
            HirOp::WriteRegister(1, Either::Left(4)),
            HirOp::WriteRegister(2, Either::Left(1)),
            HirOp::WriteRegister(0, Either::Left(1)),
        ],
        result_slot: None,
    };
    let sig = typer::FlatSig {
        slots: Vec::new(),
        input_count: 0,
        output_count: 0,
        field_offsets: std::collections::HashMap::new(),
    };
    let f = HirFunction {
        sig,
        body,
        slot_count: 0,
        type_name: "Test".into(),
        method_name: "print_a".into(),
    };
    let mut fns = std::collections::HashMap::new();
    fns.insert(typer::FnSig::new("Test", "print_a"), f);

    let mut io = CapturedIo::new();
    {
        let mut interp = Interpreter::new(fns, &mut io);
        interp
            .run(&typer::FnSig::new("Test", "print_a"), &[])
            .expect("interp");
    }
    assert_eq!(io.stdout, "A");
}

#[test]
fn direct_read_register_echoes_input() {
    // Hand-built: trigger input (WriteRegister(0, 2)) then read registers
    // [1] and [2] into slots and copy them to output.
    //   slot 0 = _ret_high, slot 1 = _ret_low
    //   ops: WriteRegister(0, imm 2) — pulls a byte from stdin, splits
    //        nibbles into registers[1] (high), registers[2] (low)
    //        ReadRegister(s0, 1); ReadRegister(s1, 2)
    use crate::ir::{HirBlock, HirFunction, HirOp, SlotId};
    use either::Either;

    let body = HirBlock {
        ops: vec![
            HirOp::WriteRegister(0, Either::Left(2)),
            HirOp::ReadRegister(SlotId(0), 1),
            HirOp::ReadRegister(SlotId(1), 2),
        ],
        result_slot: None,
    };
    let sig = typer::FlatSig {
        slots: Vec::new(),
        input_count: 0,
        output_count: 2,
        field_offsets: std::collections::HashMap::new(),
    };
    let f = HirFunction {
        sig,
        body,
        slot_count: 2,
        type_name: "Test".into(),
        method_name: "read".into(),
    };
    let mut fns = std::collections::HashMap::new();
    fns.insert(typer::FnSig::new("Test", "read"), f);

    let mut io = CapturedIo::with_input(vec![0x5A]); // nibbles: high=5, low=0xA=10
    let out = {
        let mut interp = Interpreter::new(fns, &mut io);
        interp
            .run(&typer::FnSig::new("Test", "read"), &[])
            .expect("interp")
    };
    assert_eq!(out, vec![5, 10]);
}
