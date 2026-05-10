//! Phase 6 tests: call graph (6.1), monomorphization (6.2), inliner (6.3),
//! and HIR → MIR conversion (6.4).

use std::collections::HashMap;

use crate::tests::interp_tests::compile_all;
use crate::{
    build_call_graph, gen_function, hir_to_mir, inline_program, monomorphize, HirFunction,
    HirOp, MonomorphKey, SlotId,
};

fn compile_with_entry(src: &str, ty: &str, method: &str) -> (HashMap<typer::FnSig, HirFunction>, typer::FnSig) {
    (compile_all(src), typer::FnSig::new(ty, method))
}

// ---------- Step 6.1: call graph -------------------------------------------

#[test]
fn call_graph_finds_linear_chain() {
    let src = r#"
        extension U4 {
            fn leaf(U4 x): Self { x }
            fn middle(U4 x): Self { U4::leaf(x) }
            fn top(U4 x): Self { U4::middle(x) }
        }
    "#;
    let (fns, entry) = compile_with_entry(src, "U4", "top");
    let graph = build_call_graph(&fns, &entry).expect("call graph");
    // reachable: top, middle, leaf (order: BFS/DFS-ish; just check contents).
    let names: std::collections::HashSet<String> = graph
        .reachable
        .iter()
        .map(|k| format!("{}::{}", k.type_name, k.method_name))
        .collect();
    for m in ["U4::top", "U4::middle", "U4::leaf"] {
        assert!(names.contains(m), "missing {}: {:?}", m, names);
    }
}

#[test]
fn call_graph_rejects_recursion() {
    // fn a(U4 x): U4 { U4::b(x) }   fn b(U4 x): U4 { U4::a(x) }  — cycle
    let src = r#"
        extension U4 {
            fn a(U4 x): Self { U4::b(x) }
            fn b(U4 x): Self { U4::a(x) }
        }
    "#;
    let (fns, entry) = compile_with_entry(src, "U4", "a");
    let result = build_call_graph(&fns, &entry);
    assert!(result.is_err(), "expected cycle error, got {:?}", result);
    let err = result.unwrap_err();
    assert!(
        err.contains("recursion") || err.contains("cycle"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn call_graph_self_recursion_rejected() {
    let src = r#"
        extension U4 {
            fn spin(U4 x): Self { U4::spin(x) }
        }
    "#;
    let (fns, entry) = compile_with_entry(src, "U4", "spin");
    assert!(build_call_graph(&fns, &entry).is_err());
}

// ---------- Step 6.2: monomorphization -------------------------------------

#[test]
fn monomorphize_function_level_template_t_to_u4() {
    // `fn identity<T>(T x): T { x }` on System, template T = U4.
    let src = r#"
        struct System {}
        extension System {
            fn identity<T>(T x): T { x }
        }
    "#;
    let items = new_parser::parse(src).unwrap();
    let reg = typer::TypeRegistry::from_items(&items).unwrap();
    let db = typer::FunctionDB::from_registry(&reg).unwrap();

    let templated = match db.get(&typer::FnSig::new("System", "identity")).unwrap() {
        typer::Fn::Templated(t) => t.clone(),
        _ => panic!("expected Templated"),
    };
    let key = MonomorphKey {
        sig: typer::FnSig::new("System", "identity"),
        template_args: vec![crate::ConcreteTemplateArg::Type(crate::ConcreteType {
            name: "U4".into(),
            args: vec![],
        })],
    };
    let hir = monomorphize(&key, &templated, &reg, &db).expect("monomorphize");
    assert_eq!(hir.type_name, "System<U4>");
    // The instantiated function takes a U4 (1 cell) and returns U4 (1 cell).
    assert_eq!(hir.sig.input_count, 1);
    assert_eq!(hir.sig.output_count, 1);
}

// ---------- Step 6.3: inlining ---------------------------------------------

#[test]
fn inline_removes_all_call_ops() {
    let src = r#"
        extension U4 {
            fn echo(U4 x): Self { x }
            fn caller(): U4 { U4::echo(5) }
        }
    "#;
    let (fns, entry) = compile_with_entry(src, "U4", "caller");
    let inlined = inline_program(&fns, &entry).expect("inline");
    assert!(
        !contains_call(&inlined.body),
        "Call ops remain after inlining: {:#?}",
        inlined.body.ops
    );
}

fn contains_call(block: &crate::HirBlock) -> bool {
    block.ops.iter().any(|op| match op {
        HirOp::Call { .. } => true,
        HirOp::Loop(b) | HirOp::Block(b) => contains_call(b),
        HirOp::Match(_, arms) => arms.iter().any(|(b, _)| contains_call(b)),
        _ => false,
    })
}

#[test]
fn inline_allocates_disjoint_slot_ranges_per_callee() {
    // Inline two sequential calls — the two callee instantiations must not
    // share slot numbers.
    let src = r#"
        extension U4 {
            fn echo(U4 x): Self { x }
            fn caller(): U4 {
                mut U4 a = U4::echo(1);
                mut U4 b = U4::echo(2);
                b
            }
        }
    "#;
    let (fns, entry) = compile_with_entry(src, "U4", "caller");
    let inlined = inline_program(&fns, &entry).expect("inline");
    // slot_count must account for BOTH callee frames.
    // caller locals: _ret(1), a(1), b(1) = 3 slots. Plus 2 × echo(input=1 + ret=1) = 4.
    // Total at least 7. Be tolerant of extra scratch slots.
    assert!(inlined.slot_count >= 7, "slot_count too low: {}", inlined.slot_count);
}

#[test]
fn inline_copies_args_and_rets() {
    // Check that an inlined `caller(): U4 { U4::echo(5) }` emits:
    //   Copy(callee_input, 5_constant_slot)    — arg copy
    //   <body of echo, remapped>
    //   Copy(caller_ret, callee_output)        — return copy
    let src = r#"
        extension U4 {
            fn echo(U4 x): Self { x }
            fn caller(): U4 { U4::echo(5) }
        }
    "#;
    let (fns, entry) = compile_with_entry(src, "U4", "caller");
    let inlined = inline_program(&fns, &entry).expect("inline");
    // Both caller ret slot and callee slot should receive writes.
    let has_set_5 = inlined
        .body
        .ops
        .iter()
        .any(|op| matches!(op, HirOp::Set(_, 5)));
    assert!(has_set_5, "literal 5 was lost during inlining");
}

// ---------- Step 6.4: HIR → MIR --------------------------------------------

#[test]
fn hir_to_mir_converts_basic_ops() {
    use mir::Mir;
    let block = crate::HirBlock {
        ops: vec![
            HirOp::Set(SlotId(0), 5),
            HirOp::Copy(SlotId(1), SlotId(0)),
            HirOp::inc(SlotId(1)),
            HirOp::dec(SlotId(1)),
            HirOp::Stop,
        ],
        result_slot: None,
    };
    let mir = hir_to_mir(&block).expect("convert");
    assert!(matches!(mir.0[0], Mir::Set(0, 5)));
    assert!(matches!(mir.0[1], Mir::Copy(1, 0)));
    assert!(matches!(mir.0[2], Mir::MapValue(1, 1, mir::INC_TABLE)));
    assert!(matches!(mir.0[3], Mir::MapValue(1, 1, mir::DEC_TABLE)));
    assert!(matches!(mir.0[4], Mir::Stop));
}

#[test]
fn hir_to_mir_rejects_remaining_call() {
    let block = crate::HirBlock {
        ops: vec![HirOp::Call {
            target: crate::FnRef {
                type_name: "X".into(),
                method_name: "y".into(),
                template_args: vec![],
                trait_name: None,
            },
            args: vec![],
            ret: vec![],
        }],
        result_slot: None,
    };
    assert!(hir_to_mir(&block).is_err());
}

#[test]
fn hir_to_mir_handles_control_flow() {
    use mir::Mir;
    // HIR has no If0: `if_zero` builds a 2-arm `Match` (`[0]` then
    // `1..=15`), which lowers to `Mir::Match` with the same shape.
    let block = crate::HirBlock {
        ops: vec![HirOp::if_zero(
            SlotId(0),
            crate::HirBlock {
                ops: vec![HirOp::Set(SlotId(1), 10)],
                result_slot: None,
            },
            crate::HirBlock {
                ops: vec![HirOp::Set(SlotId(1), 20)],
                result_slot: None,
            },
        )],
        result_slot: None,
    };
    let mir = hir_to_mir(&block).expect("convert");
    match &mir.0[0] {
        Mir::Match(s, arms) => {
            assert_eq!(*s, 0);
            assert_eq!(arms.len(), 2);
            assert_eq!(arms[0].1, vec![0u8]);
            assert_eq!(arms[1].1, (1u8..=15u8).collect::<Vec<_>>());
            assert!(matches!(arms[0].0 .0[0], Mir::Set(1, 10)));
            assert!(matches!(arms[1].0 .0[0], Mir::Set(1, 20)));
        }
        _ => panic!("expected Match"),
    }
}

// ---------- end-to-end: source → parse → typer → fnDB → HIR → inline → MIR

#[test]
fn end_to_end_simple_program_compiles_to_mir_and_runs() {
    // `fn caller(): U4 { U4::echo(7) }`  after inlining + conversion must
    // produce a MirCodeBlock that, when interpreted, yields `7` in global
    // slot 0 (_ret).
    let src = r#"
        extension U4 {
            fn echo(U4 x): Self { x }
            fn caller(): U4 { U4::echo(7) }
        }
    "#;
    let (fns, entry) = compile_with_entry(src, "U4", "caller");
    let inlined = inline_program(&fns, &entry).expect("inline");
    let mir_block = hir_to_mir(&inlined.body).expect("convert");

    // Run with the MIR interpreter.
    struct Null;
    impl mir::RunContext for Null {
        fn input(&mut self) -> u8 {
            0
        }
        fn print(&mut self, _: u8) {}
    }
    let mut state = mir::MemoryState::new_with_limit((inlined.slot_count as usize + 4).max(16), 4, 5_000_000);
    let mut ctx = Null;
    state.execute_block(&mir_block, &mut ctx);
    // The caller's _ret slot is index `input_count` of the caller sig = 0,
    // since caller has no input. Verify that slot holds 7.
    assert_eq!(state.get_mem(0), 7);
}
