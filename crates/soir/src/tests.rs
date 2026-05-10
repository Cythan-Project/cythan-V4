//! Unit tests for M0 (arena + printer), M1 (HIR → SoN
//! translation), M2 (graph-walking interpreter), M3 (SoN → MIR
//! scheduler), M4-real (SoN inliner), and M5 (GVN / const-fold).

use crate::builder::translate_function;
use crate::inline::inline_program;
use crate::interp::{run_with_limit, RunResult};
use crate::ir::{Graph, NodeId, NodeKind, Program, ProjKind};
use crate::print::dump_graph;
use either::Either;
use hir::{FnRef, HirBlock, HirFunction, HirOp, SlotId};
use mir::RunContext;

fn empty_sig() -> typer::FlatSig {
    typer::FlatSig {
        slots: Vec::new(),
        input_count: 0,
        output_count: 0,
        field_offsets: std::collections::HashMap::new(),
    }
}

#[test]
fn start_is_preallocated() {
    let g = Graph::new(empty_sig());
    assert!(g.start().is_valid());
    assert_eq!(g.arena_len(), 1);
    assert_eq!(g.live_len(), 1);
    assert!(matches!(g.get(g.start()).kind, NodeKind::Start));
}

#[test]
fn alloc_registers_as_user() {
    let mut g = Graph::new(empty_sig());
    let c = g.alloc(NodeKind::Const(3));
    let inc = g.alloc(NodeKind::Inc(c));
    // `c` should now list `inc` as a user.
    assert_eq!(g.get(c).users, vec![inc]);
    // `inc` has no users yet.
    assert!(g.get(inc).users.is_empty());
}

#[test]
fn kill_removes_from_users() {
    let mut g = Graph::new(empty_sig());
    let c = g.alloc(NodeKind::Const(5));
    let inc = g.alloc(NodeKind::Inc(c));
    // Kill the lone user first (can't kill a node with live users).
    g.kill(inc);
    assert!(!g.is_live(inc));
    // `c` lost its only user.
    assert!(g.get(c).users.is_empty());
}

#[test]
#[should_panic(expected = "kill")]
fn kill_with_users_panics() {
    let mut g = Graph::new(empty_sig());
    let c = g.alloc(NodeKind::Const(2));
    let _inc = g.alloc(NodeKind::Inc(c));
    g.kill(c); // panics: inc still uses c
}

#[test]
fn replace_all_uses_rewires_edges() {
    let mut g = Graph::new(empty_sig());
    let a = g.alloc(NodeKind::Const(1));
    let b = g.alloc(NodeKind::Const(2));
    let inc_a = g.alloc(NodeKind::Inc(a));
    let dec_a = g.alloc(NodeKind::Dec(a));
    g.replace_all_uses(a, b);
    // Both consumers now point at `b`.
    assert!(matches!(g.get(inc_a).kind, NodeKind::Inc(n) if n == b));
    assert!(matches!(g.get(dec_a).kind, NodeKind::Dec(n) if n == b));
    // `a` is orphaned but still alive; caller kills it next.
    assert!(g.get(a).users.is_empty());
    // `b` picked up both users.
    assert_eq!(g.get(b).users.len(), 2);
    g.kill(a);
    assert!(!g.is_live(a));
}

#[test]
fn loop_backedge_fills_in() {
    let mut g = Graph::new(empty_sig());
    let start = g.start();
    let start_ctrl = g.alloc(NodeKind::Proj {
        of: start,
        kind: ProjKind::StartCtrl,
    });
    let lp = g.alloc(NodeKind::Loop {
        entry: start_ctrl,
        back: None,
    });
    // Body eventually loops back — use the loop itself as the
    // backedge source for this unit test (real body would be an
    // `IfTrue` projection or similar).
    g.set_loop_back(lp, lp);
    match &g.get(lp).kind {
        NodeKind::Loop { back, .. } => assert_eq!(*back, Some(lp)),
        _ => panic!("expected Loop"),
    }
    // Loop appears on its own users list via the backedge.
    assert!(g.get(lp).users.contains(&lp));
}

#[test]
fn dump_lists_every_live_node() {
    let mut g = Graph::new(empty_sig());
    let a = g.alloc(NodeKind::Const(7));
    let _ = g.alloc(NodeKind::Inc(a));
    let text = dump_graph(&g);
    assert!(text.contains("Start"));
    assert!(text.contains("Const 7"));
    assert!(text.contains("Inc n1")); // a is n1, inc is n2
    assert!(text.contains("in=0 out=0"));
}

#[test]
fn dump_skips_dead_nodes() {
    let mut g = Graph::new(empty_sig());
    let c = g.alloc(NodeKind::Const(9));
    let inc = g.alloc(NodeKind::Inc(c));
    g.kill(inc);
    let text = dump_graph(&g);
    // The dead slot's inc has been wiped — no Inc line remains.
    assert!(!text.contains("Inc "));
    // The Const survives.
    assert!(text.contains("Const 9"));
}

// ---------------------------------------------------------------
// M5 — GVN + const fold
// ---------------------------------------------------------------

#[test]
fn gvn_dedupes_same_constant() {
    let mut g = Graph::new(empty_sig());
    let a = g.alloc_const(5);
    let b = g.alloc_const(5);
    let c = g.alloc_const(7);
    assert_eq!(a, b, "Const(5) should dedup to one node");
    assert_ne!(a, c, "Const(5) and Const(7) must stay distinct");
}

#[test]
fn gvn_dedupes_same_inc() {
    let mut g = Graph::new(empty_sig());
    let p = g.alloc(NodeKind::Proj {
        of: g.start(),
        kind: ProjKind::Param(0),
    });
    let a = g.alloc_inc(p);
    let b = g.alloc_inc(p);
    assert_eq!(a, b, "Inc(p) must GVN");
}

#[test]
fn const_fold_inc_of_const() {
    let mut g = Graph::new(empty_sig());
    let c = g.alloc_const(3);
    let r = g.alloc_inc(c);
    // alloc_inc folds Inc(Const(3)) into Const(4).
    assert!(matches!(g.get(r).kind, NodeKind::Const(4)));
    // And Const(4) participates in the cache.
    let c4 = g.alloc_const(4);
    assert_eq!(r, c4);
}

#[test]
fn const_fold_dec_wraps() {
    let mut g = Graph::new(empty_sig());
    let c = g.alloc_const(0);
    let r = g.alloc_dec(c);
    // u4 wraparound: 0 - 1 = 15.
    assert!(matches!(g.get(r).kind, NodeKind::Const(15)));
}

#[test]
fn const_fold_add_sub_eq() {
    let mut g = Graph::new(empty_sig());
    let three = g.alloc_const(3);
    let four = g.alloc_const(4);
    let r_add = g.alloc_add(three, four);
    let r_sub = g.alloc_sub(four, three);
    let r_ne = g.alloc_eq(three, four);
    let r_eq = g.alloc_eq(three, three);
    assert!(matches!(g.get(r_add).kind, NodeKind::Const(7)));
    assert!(matches!(g.get(r_sub).kind, NodeKind::Const(1)));
    assert!(matches!(g.get(r_ne).kind, NodeKind::Const(0)));
    assert!(matches!(g.get(r_eq).kind, NodeKind::Const(1)));
}

#[test]
fn gvn_add_is_commutative() {
    let mut g = Graph::new(empty_sig());
    let p1 = g.alloc(NodeKind::Proj {
        of: g.start(),
        kind: ProjKind::Param(0),
    });
    let p2 = g.alloc(NodeKind::Proj {
        of: g.start(),
        kind: ProjKind::Param(1),
    });
    let a = g.alloc_add(p1, p2);
    let b = g.alloc_add(p2, p1);
    assert_eq!(a, b, "Add is commutative; both orders must share");
}

#[test]
fn gvn_shrinks_stdlib_translation() {
    // Measurement test: translating every stdlib function should
    // produce fewer live nodes after GVN than the arena length
    // would imply. Const(0) alone gets spammed across the whole
    // function via read_slot's zero-init path, so dedup there is
    // a big win.
    use std::path::Path;
    let std_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("examples/new_syntax/std"))
        .expect("workspace root");
    let files = load_std_files(&std_dir);
    let user = (
        "Main.ct".to_string(),
        "struct Main {}\nextension Main {\n  fn main() { 0.print(); }\n}\n".to_string(),
    );
    let mut all = files;
    all.push(user);
    let parsed: Vec<(String, Vec<new_parser::ast::Spanned<new_parser::ast::Item>>)> =
        all.iter()
            .map(|(n, s)| (n.clone(), new_parser::parse(s).expect("parse")))
            .collect();
    let as_refs: Vec<(&str, &[_])> = parsed
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_slice()))
        .collect();
    let reg = typer::TypeRegistry::from_files(&as_refs).expect("typer");
    let db = typer::FunctionDB::from_registry(&reg).expect("db");
    let natives = hir::BuiltinNatives::new();
    let mut total_const_zero_uses = 0u32;
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            let hir_fn =
                hir::gen_function_with_natives(k, s, &reg, &db, Some(&natives)).unwrap();
            let g = translate_function(&hir_fn);
            let zeros = g
                .iter()
                .filter(|(_, n)| matches!(n.kind, NodeKind::Const(0)))
                .count();
            // With GVN, each function has at most ONE Const(0) —
            // every zero-init read points at the same id.
            assert!(
                zeros <= 1,
                "{}::{}: expected <=1 Const(0) after GVN, got {}",
                k.type_name,
                k.method_name,
                zeros
            );
            total_const_zero_uses += zeros as u32;
        }
    }
    // At least some functions have a Const(0) — proves the test
    // wasn't vacuously true.
    assert!(total_const_zero_uses > 0);
}

#[test]
fn effectful_nodes_report_effectful() {
    let rr = NodeKind::ReadReg {
        ctrl: NodeId::INVALID,
        eff: NodeId::INVALID,
        reg: 2,
    };
    let pure = NodeKind::Const(1);
    assert!(rr.is_effectful());
    assert!(!pure.is_effectful());
}

// ---------------------------------------------------------------
// M1 — HIR → SoN translation
// ---------------------------------------------------------------

fn sig(input: u32, output: u32) -> typer::FlatSig {
    // Each cell gets a placeholder SlotInfo so total_slots() matches
    // what the builder expects.
    use typer::SlotInfo;
    let mut slots = Vec::new();
    for i in 0..input {
        slots.push(SlotInfo {
            name: format!("in{}", i),
            offset: i,
            size: 1,
            mutable: false,
            type_name: "U4".into(),
            type_args: Vec::new(),
        });
    }
    for i in 0..output {
        slots.push(SlotInfo {
            name: "_ret".into(),
            offset: input + i,
            size: 1,
            mutable: false,
            type_name: "U4".into(),
            type_args: Vec::new(),
        });
    }
    typer::FlatSig {
        slots,
        input_count: input,
        output_count: output,
        field_offsets: std::collections::HashMap::new(),
    }
}

fn func(sig: typer::FlatSig, body: HirBlock) -> HirFunction {
    HirFunction {
        sig,
        body,
        slot_count: 32,
        type_name: "T".into(),
        method_name: "f".into(),
        warnings: Vec::new(),
    }
}

fn count_kind(g: &Graph, mat: impl Fn(&NodeKind) -> bool) -> usize {
    g.iter().filter(|(_, n)| mat(&n.kind)).count()
}

#[test]
fn translates_empty_function() {
    // fn() -> () — just returns with no values.
    let f = func(sig(0, 0), HirBlock::new());
    let g = translate_function(&f);
    // Expect: Start, Proj(StartCtrl), Proj(StartEff), Return.
    assert_eq!(count_kind(&g, |k| matches!(k, NodeKind::Return { .. })), 1);
}

#[test]
fn translates_constant_return() {
    // fn() -> U4 { 7 }
    let mut body = HirBlock::new();
    body.push(HirOp::Set(SlotId(0), 7)); // slot 0 is the output cell (input_count=0)
    let f = func(sig(0, 1), body);
    let g = translate_function(&f);
    // Find the Return and confirm its `values[0]` points at a Const(7).
    let ret = g
        .iter()
        .find_map(|(_, n)| match &n.kind {
            NodeKind::Return { values, .. } => Some(values.clone()),
            _ => None,
        })
        .expect("Return exists");
    assert_eq!(ret.len(), 1);
    let v = &g.get(ret[0]).kind;
    assert!(matches!(v, NodeKind::Const(7)), "got {:?}", v);
}

#[test]
fn translates_identity() {
    // fn(x: U4) -> U4 { x }
    // Body: Copy(s1=output, s0=input)
    let mut body = HirBlock::new();
    body.push(HirOp::Copy(SlotId(1), SlotId(0)));
    let f = func(sig(1, 1), body);
    let g = translate_function(&f);
    let ret = g
        .iter()
        .find_map(|(_, n)| match &n.kind {
            NodeKind::Return { values, .. } => Some(values.clone()),
            _ => None,
        })
        .expect("Return");
    // The returned value should be Param(0).
    assert!(matches!(
        &g.get(ret[0]).kind,
        NodeKind::Proj { kind: ProjKind::Param(0), .. }
    ));
}

#[test]
fn translates_match_with_phi_merge() {
    // fn(c: U4) -> U4 { match c { [0] => 1, [1..15] => 2 } }
    // slot 0 = input c, slot 1 = output.
    let mut arm0 = HirBlock::new();
    arm0.push(HirOp::Set(SlotId(1), 1));
    let mut arm1 = HirBlock::new();
    arm1.push(HirOp::Set(SlotId(1), 2));
    let mut body = HirBlock::new();
    body.push(HirOp::Match(
        SlotId(0),
        vec![(arm0, vec![0]), (arm1, (1..=15).collect())],
    ));
    let f = func(sig(1, 1), body);
    let g = translate_function(&f);
    // Expect: one Match node, two Proj(MatchArm), one Region, one
    // Phi for slot 1 (the output).
    assert_eq!(count_kind(&g, |k| matches!(k, NodeKind::Match { .. })), 1);
    assert_eq!(
        count_kind(&g, |k| matches!(k, NodeKind::Proj { kind: ProjKind::MatchArm(_), .. })),
        2
    );
    assert_eq!(count_kind(&g, |k| matches!(k, NodeKind::Region { .. })), 1);
    // At least one Phi: the one merging slot 1's two defs.
    assert!(count_kind(&g, |k| matches!(k, NodeKind::Phi { .. })) >= 1);
}

#[test]
fn translates_loop_with_break() {
    // fn() -> U4 {
    //   mut s1 = 0;
    //   loop { if s1 == 3 { break; } s1 += 1; }
    //   s1
    // }
    // Lowered shape (skipping the eq): just loop { match s1 [3]=>break, rest=>inc }.
    // Here we use Match on slot 1 directly with arm values [3] → break,
    // rest → Inc(s1).
    let mut then_arm = HirBlock::new();
    then_arm.push(HirOp::Break);
    let mut else_arm = HirBlock::new();
    else_arm.push(HirOp::inc(SlotId(1)));
    let mut lbody = HirBlock::new();
    lbody.push(HirOp::Match(
        SlotId(1),
        vec![(then_arm, vec![3]), (else_arm, (0..=15).filter(|v| *v != 3).collect())],
    ));
    let mut body = HirBlock::new();
    body.push(HirOp::Set(SlotId(1), 0)); // init
    body.push(HirOp::Loop(lbody));
    // Note: slot 1 is output (signature: 0 inputs, 1 output).
    let f = func(sig(0, 1), body);
    let g = translate_function(&f);
    // Expect: at least one Loop, one Match (inside body), one
    // Region for exit merge (trivial here — only one Break), and
    // a Phi at the loop header for slot 1.
    assert_eq!(count_kind(&g, |k| matches!(k, NodeKind::Loop { .. })), 1);
    assert_eq!(count_kind(&g, |k| matches!(k, NodeKind::Match { .. })), 1);
    // Header phi for slot 1 exists.
    assert!(count_kind(&g, |k| matches!(k, NodeKind::Phi { .. })) >= 1);
    // Loop has a backedge resolved (not None).
    let lp = g
        .iter()
        .find_map(|(id, n)| match &n.kind {
            NodeKind::Loop { back, .. } => Some((id, *back)),
            _ => None,
        })
        .expect("Loop");
    assert!(
        lp.1.is_some(),
        "loop backedge should be resolved after body close"
    );
}

#[test]
fn translates_read_write_register() {
    // fn() -> U4 { s1 = ReadRegister<2>(); WriteRegister<0>(1); s1 }
    let mut body = HirBlock::new();
    body.push(HirOp::ReadRegister(SlotId(1), 2));
    body.push(HirOp::WriteRegister(0, Either::Left(1)));
    let f = func(sig(0, 1), body);
    let g = translate_function(&f);
    // Effect chain: Start -> Proj(StartEff) -> ReadReg -> Proj(Eff) -> WriteReg -> Proj(Eff) -> Return.eff
    assert_eq!(count_kind(&g, |k| matches!(k, NodeKind::ReadReg { .. })), 1);
    assert_eq!(
        count_kind(&g, |k| matches!(k, NodeKind::WriteReg { .. })),
        1
    );
    // The Return should reference the final effect Proj.
    let ret_eff = g
        .iter()
        .find_map(|(_, n)| match &n.kind {
            NodeKind::Return { eff, .. } => Some(*eff),
            _ => None,
        })
        .expect("Return");
    let eff_node = &g.get(ret_eff).kind;
    assert!(matches!(eff_node, NodeKind::Proj { kind: ProjKind::Eff, .. }));
}

// ---------------------------------------------------------------
// M2 — graph-walking interpreter
// ---------------------------------------------------------------

/// Minimal in-memory RunContext: captures prints, serves inputs.
struct TestCtx {
    input: std::collections::VecDeque<u8>,
    output: Vec<u8>,
}

impl TestCtx {
    fn new(input: &str) -> Self {
        Self {
            input: input.bytes().collect(),
            output: Vec::new(),
        }
    }
}

impl RunContext for TestCtx {
    fn input(&mut self) -> u8 {
        self.input.pop_front().unwrap_or(0)
    }
    fn print(&mut self, byte: u8) {
        self.output.push(byte);
    }
}

fn interp(f: &HirFunction, inputs: &[u8]) -> (RunResult, TestCtx) {
    let g = translate_function(f);
    let mut ctx = TestCtx::new("");
    let result = run_with_limit(&g, inputs, &mut ctx, 100_000);
    (result, ctx)
}

#[test]
fn interp_returns_constant() {
    // fn() -> U4 { 7 }
    let mut body = HirBlock::new();
    body.push(HirOp::Set(SlotId(0), 7));
    let f = func(sig(0, 1), body);
    let (r, _) = interp(&f, &[]);
    assert_eq!(r.values, vec![7]);
    assert!(!r.halted);
    assert!(!r.aborted_by_limit);
}

#[test]
fn interp_identity_function() {
    // fn(x: U4) -> U4 { x }
    let mut body = HirBlock::new();
    body.push(HirOp::Copy(SlotId(1), SlotId(0)));
    let f = func(sig(1, 1), body);
    let (r, _) = interp(&f, &[5]);
    assert_eq!(r.values, vec![5]);
    let (r, _) = interp(&f, &[12]);
    assert_eq!(r.values, vec![12]);
}

#[test]
fn interp_inc_dec() {
    // fn(x) -> U4 { x + 1 - 1 + 1 } => x + 1
    let mut body = HirBlock::new();
    body.push(HirOp::Copy(SlotId(1), SlotId(0)));
    body.push(HirOp::inc(SlotId(1)));
    body.push(HirOp::dec(SlotId(1)));
    body.push(HirOp::inc(SlotId(1)));
    let f = func(sig(1, 1), body);
    let (r, _) = interp(&f, &[3]);
    assert_eq!(r.values, vec![4]);
    // u4 wraparound
    let (r, _) = interp(&f, &[15]);
    assert_eq!(r.values, vec![0]);
}

#[test]
fn interp_match_branches_on_scrutinee() {
    // fn(c) -> U4 { match c { [0] => 1, [1..15] => 9 } }
    let mut arm0 = HirBlock::new();
    arm0.push(HirOp::Set(SlotId(1), 1));
    let mut arm1 = HirBlock::new();
    arm1.push(HirOp::Set(SlotId(1), 9));
    let mut body = HirBlock::new();
    body.push(HirOp::Match(
        SlotId(0),
        vec![(arm0, vec![0]), (arm1, (1..=15).collect())],
    ));
    let f = func(sig(1, 1), body);
    let (r0, _) = interp(&f, &[0]);
    assert_eq!(r0.values, vec![1]);
    let (r5, _) = interp(&f, &[5]);
    assert_eq!(r5.values, vec![9]);
}

#[test]
fn interp_loop_counts_and_breaks() {
    // fn() -> U4 {
    //   mut s1 = 0;
    //   loop {
    //     match s1 { [4] => break, rest => s1 += 1 }
    //   }
    //   s0 = s1;   // signature is (in=0, out=1) so output is SlotId(0)
    //   s0
    // }
    let mut break_arm = HirBlock::new();
    break_arm.push(HirOp::Break);
    let mut inc_arm = HirBlock::new();
    inc_arm.push(HirOp::inc(SlotId(1)));
    let mut lbody = HirBlock::new();
    lbody.push(HirOp::Match(
        SlotId(1),
        vec![
            (break_arm, vec![4]),
            (inc_arm, (0..=15).filter(|v| *v != 4).collect()),
        ],
    ));
    let mut body = HirBlock::new();
    body.push(HirOp::Set(SlotId(1), 0));
    body.push(HirOp::Loop(lbody));
    body.push(HirOp::Copy(SlotId(0), SlotId(1))); // return s1
    let f = func(sig(0, 1), body);
    let (r, _) = interp(&f, &[]);
    assert_eq!(r.values, vec![4]);
}

#[test]
fn interp_write_register_prints_bytes() {
    // Emit 'A' (0x41): set reg1 = 4, reg2 = 1, WriteRegister(0, 1).
    let mut body = HirBlock::new();
    body.push(HirOp::WriteRegister(1, Either::Left(4))); // high nibble
    body.push(HirOp::WriteRegister(2, Either::Left(1))); // low nibble
    body.push(HirOp::WriteRegister(0, Either::Left(1))); // trigger print
    let f = func(sig(0, 0), body);
    let g = translate_function(&f);
    let mut ctx = TestCtx::new("");
    let r = run_with_limit(&g, &[], &mut ctx, 1000);
    assert!(!r.aborted_by_limit);
    assert_eq!(ctx.output, b"A");
}

#[test]
fn interp_stop_returns() {
    // At the function level, `HirOp::Stop` represents `return`
    // in the source (HIR-gen lowers `return x` to `Set(ret, x);
    // Stop`). The builder captures Stops as return predecessors
    // and `finish()` routes them into the canonical `Return`,
    // so the interpreter observes a normal return rather than a
    // program halt.
    let mut body = HirBlock::new();
    body.push(HirOp::Stop);
    let f = func(sig(0, 0), body);
    let (r, _) = interp(&f, &[]);
    assert!(!r.halted);
    assert_eq!(r.values, Vec::<u8>::new());
}

#[test]
fn interp_match_inside_loop_counts_then_returns() {
    // Multi-iteration regression: slot must survive multiple
    // loop re-entries with phi at header, not get stuck.
    // fn(n) -> U4 { mut i = 0; loop { match i == n [?] break; else inc } ; i }
    // Simpler: loop 3 times, then break.
    let mut break_arm = HirBlock::new();
    break_arm.push(HirOp::Break);
    let mut cont_arm = HirBlock::new();
    cont_arm.push(HirOp::inc(SlotId(2))); // count++
    cont_arm.push(HirOp::inc(SlotId(1))); // temp++
    let mut lbody = HirBlock::new();
    lbody.push(HirOp::Match(
        SlotId(1),
        vec![
            (break_arm, vec![3]),
            (cont_arm, (0..=15).filter(|v| *v != 3).collect()),
        ],
    ));
    let mut body = HirBlock::new();
    body.push(HirOp::Set(SlotId(1), 0));
    body.push(HirOp::Set(SlotId(2), 0));
    body.push(HirOp::Loop(lbody));
    // Copy s2 → output slot (slot 0 since 0 input, 1 output).
    body.push(HirOp::Copy(SlotId(0), SlotId(2)));
    let f = func(sig(0, 1), body);
    let (r, _) = interp(&f, &[]);
    assert_eq!(r.values, vec![3]);
}

// ---------------------------------------------------------------
// M3 — scheduler (SoN → MIR)
// ---------------------------------------------------------------

fn schedule_and_run(f: &HirFunction, inputs: &[u8]) -> (Vec<u8>, Vec<u8>) {
    use mir::MemoryState;
    let g = crate::builder::translate_function(f);
    let mir_block = crate::schedule::schedule(&g);
    let sig = f.sig.clone();
    // Memory = per-function slots + scheduler temps.
    let memory_size = (sig.total_slots() + 64) as usize;
    let mut state = MemoryState::new_with_limit(memory_size, 8, 100_000);
    // Seed param slots.
    for (i, v) in inputs.iter().enumerate() {
        state.set_mem(i as u32, *v);
    }
    let mut ctx = TestCtx::new("");
    state.execute_block(&mir_block, &mut ctx);
    // Read output slots.
    let mut outs = Vec::new();
    for i in 0..sig.output_count {
        outs.push(state.get_mem(sig.input_count + i));
    }
    (outs, ctx.output)
}

#[test]
fn schedule_returns_constant() {
    let mut body = HirBlock::new();
    body.push(HirOp::Set(SlotId(0), 7));
    let f = func(sig(0, 1), body);
    let (outs, _) = schedule_and_run(&f, &[]);
    assert_eq!(outs, vec![7]);
}

#[test]
fn schedule_identity() {
    let mut body = HirBlock::new();
    body.push(HirOp::Copy(SlotId(1), SlotId(0)));
    let f = func(sig(1, 1), body);
    let (outs, _) = schedule_and_run(&f, &[3]);
    assert_eq!(outs, vec![3]);
    let (outs, _) = schedule_and_run(&f, &[11]);
    assert_eq!(outs, vec![11]);
}

#[test]
fn schedule_inc_dec() {
    let mut body = HirBlock::new();
    body.push(HirOp::Copy(SlotId(1), SlotId(0)));
    body.push(HirOp::inc(SlotId(1)));
    body.push(HirOp::inc(SlotId(1)));
    body.push(HirOp::dec(SlotId(1)));
    let f = func(sig(1, 1), body);
    let (outs, _) = schedule_and_run(&f, &[3]);
    assert_eq!(outs, vec![4]);
}

#[test]
fn schedule_match() {
    // fn(c) -> U4 { match c { [0] => 1, rest => 9 } }
    let mut arm0 = HirBlock::new();
    arm0.push(HirOp::Set(SlotId(1), 1));
    let mut arm1 = HirBlock::new();
    arm1.push(HirOp::Set(SlotId(1), 9));
    let mut body = HirBlock::new();
    body.push(HirOp::Match(
        SlotId(0),
        vec![(arm0, vec![0]), (arm1, (1..=15).collect())],
    ));
    let f = func(sig(1, 1), body);
    let (outs0, _) = schedule_and_run(&f, &[0]);
    assert_eq!(outs0, vec![1]);
    let (outs5, _) = schedule_and_run(&f, &[5]);
    assert_eq!(outs5, vec![9]);
}

#[test]
fn schedule_loop_counts_to_four() {
    // Same shape as the M2 test but runs through the MIR backend.
    let mut break_arm = HirBlock::new();
    break_arm.push(HirOp::Break);
    let mut inc_arm = HirBlock::new();
    inc_arm.push(HirOp::inc(SlotId(1)));
    let mut lbody = HirBlock::new();
    lbody.push(HirOp::Match(
        SlotId(1),
        vec![
            (break_arm, vec![4]),
            (inc_arm, (0..=15).filter(|v| *v != 4).collect()),
        ],
    ));
    let mut body = HirBlock::new();
    body.push(HirOp::Set(SlotId(1), 0));
    body.push(HirOp::Loop(lbody));
    body.push(HirOp::Copy(SlotId(0), SlotId(1)));
    let f = func(sig(0, 1), body);
    let (outs, _) = schedule_and_run(&f, &[]);
    assert_eq!(outs, vec![4]);
}

#[test]
fn schedule_write_register_prints() {
    let mut body = HirBlock::new();
    body.push(HirOp::WriteRegister(1, Either::Left(4)));
    body.push(HirOp::WriteRegister(2, Either::Left(1)));
    body.push(HirOp::WriteRegister(0, Either::Left(1)));
    let f = func(sig(0, 0), body);
    let (_, printed) = schedule_and_run(&f, &[]);
    assert_eq!(printed, b"A");
}

#[test]
fn schedule_agrees_with_interp_on_synthetic_cases() {
    // Cross-check: for the same synthetic HIR, the MIR-interp
    // (via schedule) and the soir interp should agree on both
    // output slots and printed bytes.
    //
    // Use a non-trivial fixture: match + loop + register write.
    let mut break_arm = HirBlock::new();
    break_arm.push(HirOp::Break);
    let mut inc_arm = HirBlock::new();
    inc_arm.push(HirOp::inc(SlotId(1)));
    let mut lbody = HirBlock::new();
    lbody.push(HirOp::Match(
        SlotId(1),
        vec![
            (break_arm, vec![3]),
            (inc_arm, (0..=15).filter(|v| *v != 3).collect()),
        ],
    ));
    let mut body = HirBlock::new();
    body.push(HirOp::Set(SlotId(1), 0));
    body.push(HirOp::Loop(lbody));
    body.push(HirOp::Copy(SlotId(0), SlotId(1)));
    let f = func(sig(0, 1), body);

    let g = translate_function(&f);
    let mut ctx1 = TestCtx::new("");
    let soir_result = crate::interp::run_with_limit(&g, &[], &mut ctx1, 100_000);
    let (mir_outs, mir_printed) = schedule_and_run(&f, &[]);

    assert_eq!(
        soir_result.values, mir_outs,
        "soir-interp vs MIR-scheduled-interp output slot mismatch"
    );
    assert_eq!(
        ctx1.output, mir_printed,
        "soir-interp vs MIR-scheduled-interp print buffer mismatch"
    );
    assert_eq!(mir_outs, vec![3]);
}

// ---------------------------------------------------------------
// SoN-level inliner (the "real M4")
// ---------------------------------------------------------------

fn fnref(type_name: &str, method: &str) -> FnRef {
    FnRef {
        type_name: type_name.into(),
        method_name: method.into(),
        template_args: Vec::new(),
        trait_name: None,
    }
}

fn fnkey(type_name: &str, method: &str) -> typer::FnSig {
    typer::FnSig::new(type_name, method)
}

#[test]
fn inliner_splices_identity_callee() {
    // Caller:  fn main() -> U4 { Helper::id(7) }
    // Callee:  fn id(x: U4) -> U4 { x }
    // After inline: main becomes just "return 7".
    let mut caller_body = HirBlock::new();
    caller_body.push(HirOp::Set(SlotId(0), 7));           // arg = 7 in slot 0
    caller_body.push(HirOp::Call {
        target: fnref("Helper", "id"),
        args: vec![SlotId(0)],
        ret: vec![SlotId(1)],                              // output at slot 1
    });
    // Output slot is SlotId(1) per signature (0 = input, 1 = output... wait
    // here we gave (0, 2) which doesn't match). Let me use an actually-
    // matching signature.
    let caller = func(sig(0, 1), {
        let mut b = HirBlock::new();
        b.push(HirOp::Set(SlotId(1), 7));                 // put 7 in local slot
        b.push(HirOp::Call {
            target: fnref("Helper", "id"),
            args: vec![SlotId(1)],
            ret: vec![SlotId(0)],                          // write to output
        });
        b
    });
    let mut callee_body = HirBlock::new();
    callee_body.push(HirOp::Copy(SlotId(1), SlotId(0)));   // ret = x
    let callee = func(sig(1, 1), callee_body);
    let _ = caller_body;

    let mut program = Program::new();
    program.insert(fnkey("Main", "main"), translate_function(&caller));
    program.insert(fnkey("Helper", "id"), translate_function(&callee));
    let flat = inline_program(&program, &fnkey("Main", "main"))
        .expect("inline ok");
    // Post-inline: no Call nodes remain.
    assert!(
        flat.iter().all(|(_, n)| !matches!(n.kind, NodeKind::Call { .. })),
        "call node survived inlining"
    );
    // Running the flat graph produces 7.
    let mut ctx = TestCtx::new("");
    let r = run_with_limit(&flat, &[], &mut ctx, 10_000);
    assert_eq!(r.values, vec![7]);
}

#[test]
fn inliner_errors_on_missing_callee() {
    let caller = func(sig(0, 1), {
        let mut b = HirBlock::new();
        b.push(HirOp::Call {
            target: fnref("Absent", "whatever"),
            args: vec![],
            ret: vec![SlotId(0)],
        });
        b
    });
    let mut program = Program::new();
    program.insert(fnkey("Main", "main"), translate_function(&caller));
    let err = inline_program(&program, &fnkey("Main", "main"));
    assert!(matches!(
        err,
        Err(crate::inline::InlineError::MissingCallee { .. })
    ));
}

#[test]
fn translates_every_stdlib_function() {
    // Integration-ish: parse the whole stdlib + a trivial user
    // program, run the typer, generate HIR per Simple function,
    // then translate every one to soir. Asserts: no panics, every
    // graph has a Start and a Return (or a Stop), live node count
    // is > 0.
    use std::path::Path;
    let std_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("examples/new_syntax/std"))
        .expect("workspace root");
    let files = load_std_files(&std_dir);
    // Add a minimal user file with a `main` so the typer is
    // happy about having something to build.
    let user = (
        "Main.ct".to_string(),
        "struct Main {}\nextension Main {\n  fn main() { 0.print(); }\n}\n".to_string(),
    );
    let mut all: Vec<(String, String)> = files;
    all.push(user);
    let parsed: Vec<(String, Vec<new_parser::ast::Spanned<new_parser::ast::Item>>)> = all
        .iter()
        .map(|(n, s)| {
            let items = new_parser::parse(s).expect("parse");
            (n.clone(), items)
        })
        .collect();
    let as_refs: Vec<(&str, &[_])> = parsed
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_slice()))
        .collect();
    let reg = typer::TypeRegistry::from_files(&as_refs).expect("typer registry");
    let db = typer::FunctionDB::from_registry(&reg).expect("function db");
    let natives = hir::BuiltinNatives::new();
    let mut translated = 0;
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            let hir_fn = hir::gen_function_with_natives(k, s, &reg, &db, Some(&natives))
                .unwrap_or_else(|e| panic!("hir {}::{}: {}", k.type_name, k.method_name, e));
            let g = translate_function(&hir_fn);
            // Every non-trivial function must have at least a
            // Start, a ctrl/eff pair, and either a Return or a
            // Stop somewhere in the live arena.
            let has_exit = g.iter().any(|(_, n)| {
                matches!(
                    &n.kind,
                    NodeKind::Return { .. } | NodeKind::Stop { .. }
                )
            });
            assert!(
                has_exit,
                "{}::{} produced no Return/Stop",
                k.type_name, k.method_name
            );
            assert!(g.live_len() >= 3);
            translated += 1;
        }
    }
    assert!(translated > 10, "expected to translate >10 std fns, got {}", translated);
}

fn load_std_files(std_dir: &std::path::Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(std_dir).expect("read std dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("ct") {
            let name = format!("std/{}", path.file_name().unwrap().to_string_lossy());
            let src = std::fs::read_to_string(&path).expect("read .ct");
            out.push((name, src.replace('\r', "")));
        }
    }
    out.sort();
    out
}

#[test]
fn dump_of_real_translation_is_readable() {
    // Sanity check: non-trivial body translates and dumps without
    // panics, and the dump mentions the expected shapes.
    let mut arm0 = HirBlock::new();
    arm0.push(HirOp::Set(SlotId(1), 1));
    let mut arm1 = HirBlock::new();
    arm1.push(HirOp::Set(SlotId(1), 0));
    let mut body = HirBlock::new();
    body.push(HirOp::Match(
        SlotId(0),
        vec![(arm0, vec![0]), (arm1, (1..=15).collect())],
    ));
    let f = func(sig(1, 1), body);
    let g = translate_function(&f);
    let text = dump_graph(&g);
    assert!(text.contains("Start"));
    assert!(text.contains("Return"));
    assert!(text.contains("Match"));
    assert!(text.contains("Phi"));
}

#[test]
fn match_arms_pretty_print_ranges() {
    let mut g = Graph::new(empty_sig());
    let start_ctrl = g.alloc(NodeKind::Proj {
        of: g.start(),
        kind: ProjKind::StartCtrl,
    });
    let scrut = g.alloc(NodeKind::Const(0));
    let _m = g.alloc(NodeKind::Match {
        ctrl: start_ctrl,
        scrut,
        arm_values: vec![vec![0], (1..=15).collect()],
    });
    let text = dump_graph(&g);
    assert!(text.contains("arms=[[0], [1..=15]]"), "got: {}", text);
}
