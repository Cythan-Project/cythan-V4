//! Function **exit-domain** summaries.
//!
//! For every function the pass computes:
//!   * `input_exit[i]` — the Domain each `mut` input cell holds
//!     when the function returns, and
//!   * `output_exit[j]` — the Domain each output cell holds on
//!     return.
//!
//! Callers use these summaries at `Call` ops to turn what was
//! a "forget everything the callee might have touched" barrier
//! into a tight Domain propagation — the parent learns the
//! guaranteed Domain of the slot it just handed the callee,
//! and (for mut params) of the arg cells the callee may have
//! written back.
//!
//! # Analysis
//!
//! Straight abstract interpretation on each body, using the
//! previous round's summaries for nested Call effects. Start
//! with every input cell at `Domain::ALL` (conservative entry
//! state) and every summary at `Domain::ALL` (worst case).
//!
//! Iteration runs as a bounded fixpoint: each round re-analyses
//! every body, tightens the summaries, and terminates when no
//! summary changes (or a small iteration cap hits, so an
//! pathological body can't keep the pass running forever).
//!
//! For targets not in the summary map — natives, FunctionDB
//! templates that'll be monomorphized on the fly — the pass
//! asks the caller via `is_non_mutating_extern`. `true` means
//! "this extern won't write back to any caller slot" (natives,
//! or DB entries declared with zero `mut` params). `false`
//! means the conservative "forget everything" is used.
//!
//! # Incremental-compilation friendliness
//!
//! Summary of F depends on F's body + summaries of F's callees.
//! A future incremental scheduler caches summaries per function
//! and re-runs one round when a callee summary changes.

use std::collections::HashMap;

use either::Either;

use crate::ir::{FnRef, HirBlock, HirFunction, HirOp, SlotId};
use crate::specialize::Domain;

/// Per-function exit-Domain summary. See module docs.
#[derive(Debug, Clone)]
pub struct FnExitDomains {
    /// Length = `sig.input_count`. For cells whose owning slot
    /// is `mut`, the Domain of possible exit values. For non-mut
    /// cells, `Domain::ALL` (not meaningful — callee can't have
    /// written them).
    pub input_exit: Vec<Domain>,
    /// Length = `sig.input_count`. Mirrors `SlotInfo.mutable`,
    /// expanded to cell granularity. Callers gate their use of
    /// `input_exit[i]` on this flag.
    pub input_mut: Vec<bool>,
    /// Length = `sig.output_count`. Exit Domain of each output
    /// cell (i.e. the value the callee writes into it).
    pub output_exit: Vec<Domain>,
}

impl FnExitDomains {
    /// Conservative default: every exit is `ALL` (we learned
    /// nothing), mut mask matches the sig.
    pub fn all_unknown(sig: &typer::FlatSig) -> Self {
        let ic = sig.input_count as usize;
        let oc = sig.output_count as usize;
        let mut input_mut = vec![false; ic];
        for slot in &sig.slots {
            if slot.name == "_ret" {
                break;
            }
            if slot.mutable {
                for i in 0..slot.size {
                    input_mut[(slot.offset + i) as usize] = true;
                }
            }
        }
        Self {
            input_exit: vec![Domain::ALL; ic],
            input_mut,
            output_exit: vec![Domain::ALL; oc],
        }
    }
}

/// Bounded fixpoint iteration count. Each round re-analyses
/// every function body; summaries only ever narrow, so the
/// fixpoint converges in a small number of rounds in practice.
const MAX_ROUNDS: usize = 4;

/// Compute exit-domain summaries for every function in `fns`.
pub fn compute_exit_domains(
    fns: &HashMap<typer::FnSig, HirFunction>,
    is_non_mutating_extern: impl Fn(&typer::FnSig) -> bool,
) -> HashMap<typer::FnSig, FnExitDomains> {
    let mut summaries: HashMap<typer::FnSig, FnExitDomains> = fns
        .iter()
        .map(|(sig, f)| (sig.clone(), FnExitDomains::all_unknown(&f.sig)))
        .collect();

    for _ in 0..MAX_ROUNDS {
        let mut changed = false;
        let keys: Vec<typer::FnSig> = fns.keys().cloned().collect();
        for sig in keys {
            let f = &fns[&sig];
            let new_summary = analyze_function(f, &summaries, &is_non_mutating_extern);
            let old = &summaries[&sig];
            if !summaries_equal(&new_summary, old) {
                changed = true;
                summaries.insert(sig, new_summary);
            }
        }
        if !changed {
            break;
        }
    }

    summaries
}

fn summaries_equal(a: &FnExitDomains, b: &FnExitDomains) -> bool {
    a.input_exit == b.input_exit && a.output_exit == b.output_exit
}

fn analyze_function(
    f: &HirFunction,
    summaries: &HashMap<typer::FnSig, FnExitDomains>,
    is_non_mutating_extern: &impl Fn(&typer::FnSig) -> bool,
) -> FnExitDomains {
    let ic = f.sig.input_count;
    let oc = f.sig.output_count;
    // Entry: inputs at ALL (conservative — we don't track per-
    // caller context here). Everything else defaults to ALL via
    // the `get` fallback.
    let mut ctx = DomainCtx::default();
    analyze_block(&f.body, &mut ctx, summaries, is_non_mutating_extern);

    let input_exit: Vec<Domain> = (0..ic).map(|i| ctx.get(SlotId(i))).collect();
    let output_exit: Vec<Domain> = (0..oc)
        .map(|j| ctx.get(SlotId(ic + j)))
        .collect();
    let mut summary = FnExitDomains::all_unknown(&f.sig);
    summary.input_exit = input_exit;
    summary.output_exit = output_exit;
    summary
}

// ---- lightweight Domain ctx for the analysis ------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DomainCtx {
    known: HashMap<SlotId, Domain>,
}

impl DomainCtx {
    fn get(&self, s: SlotId) -> Domain {
        self.known.get(&s).copied().unwrap_or(Domain::ALL)
    }
    fn put(&mut self, s: SlotId, d: Domain) {
        if d == Domain::ALL {
            self.known.remove(&s);
        } else {
            self.known.insert(s, d);
        }
    }
    fn forget(&mut self, s: SlotId) {
        self.known.remove(&s);
    }
}

// ---- analysis -------------------------------------------------------------

fn analyze_block(
    b: &HirBlock,
    ctx: &mut DomainCtx,
    summaries: &HashMap<typer::FnSig, FnExitDomains>,
    is_non_mutating_extern: &impl Fn(&typer::FnSig) -> bool,
) {
    for op in &b.ops {
        analyze_op(op, ctx, summaries, is_non_mutating_extern);
    }
}

fn analyze_op(
    op: &HirOp,
    ctx: &mut DomainCtx,
    summaries: &HashMap<typer::FnSig, FnExitDomains>,
    is_non_mutating_extern: &impl Fn(&typer::FnSig) -> bool,
) {
    match op {
        HirOp::Set(s, v) => ctx.put(*s, Domain::singleton(*v)),
        HirOp::Copy(dst, src) => {
            let d = ctx.get(*src);
            ctx.put(*dst, d);
        }
        HirOp::MapValue(src, dst, table) => {
            let d = ctx.get(*src).map(table);
            ctx.put(*dst, d);
        }
        HirOp::ReadRegister(dst, _) => ctx.forget(*dst),
        HirOp::WriteRegister(_, _) => {}
        HirOp::Match(scrutinee, arms) => {
            let scrut_dom = ctx.get(*scrutinee);
            let mut joined: Option<DomainCtx> = None;
            for (body, values) in arms {
                let arm_dom = Domain::from_values(values);
                if scrut_dom.intersect(arm_dom).is_empty() {
                    continue;
                }
                let mut arm_ctx = ctx.clone();
                // Narrow scrutinee to the arm's values.
                arm_ctx.put(*scrutinee, scrut_dom.intersect(arm_dom));
                analyze_block(body, &mut arm_ctx, summaries, is_non_mutating_extern);
                joined = Some(match joined.take() {
                    None => arm_ctx,
                    Some(acc) => union_ctx(acc, arm_ctx),
                });
            }
            if let Some(j) = joined {
                *ctx = j;
            }
            // If no arm is reachable at all, leave ctx as-is
            // (the Match is dead code — its post-state is moot).
        }
        HirOp::Loop(body) => {
            // Can't cheaply fixpoint over loop iterations here —
            // conservatively forget everything the body writes.
            let mutated = slots_mutated(body);
            for s in mutated {
                ctx.forget(s);
            }
        }
        HirOp::Block(body) => {
            // Block body can `Skip` mid-way — conservatively
            // forget mutated slots (same approximation as
            // `specialize.rs`).
            analyze_block(body, ctx, summaries, is_non_mutating_extern);
            // Actually analyze_block already applied writes; we
            // don't need the extra forget.
        }
        HirOp::Break | HirOp::Continue | HirOp::Stop | HirOp::Skip => {}
        HirOp::Call { target, args, ret } => {
            let target_sig = fn_ref_to_sig(target);
            match summaries.get(&target_sig) {
                Some(summary) => {
                    // Update each arg's caller slot using the
                    // callee's input-exit domain (only when the
                    // callee param is declared mut — non-mut
                    // slots aren't written, so caller ctx is
                    // unchanged for those).
                    for (i, arg_slot) in args.iter().enumerate() {
                        if i < summary.input_mut.len() && summary.input_mut[i] {
                            let d = summary
                                .input_exit
                                .get(i)
                                .copied()
                                .unwrap_or(Domain::ALL);
                            ctx.put(*arg_slot, d);
                        }
                    }
                    for (j, ret_slot) in ret.iter().enumerate() {
                        let d = summary
                            .output_exit
                            .get(j)
                            .copied()
                            .unwrap_or(Domain::ALL);
                        ctx.put(*ret_slot, d);
                    }
                }
                None if is_non_mutating_extern(&target_sig) => {
                    // Extern doesn't mutate args; ret cells are
                    // freshly written so we must forget their
                    // caller-side prior domains.
                    for r in ret {
                        ctx.forget(*r);
                    }
                }
                None => {
                    // Unknown target — forget args (could be mut)
                    // and ret (definitely written).
                    for a in args {
                        ctx.forget(*a);
                    }
                    for r in ret {
                        ctx.forget(*r);
                    }
                }
            }
        }
    }
}

fn union_ctx(a: DomainCtx, b: DomainCtx) -> DomainCtx {
    let mut out = DomainCtx::default();
    let keys: std::collections::HashSet<SlotId> =
        a.known.keys().chain(b.known.keys()).copied().collect();
    for k in keys {
        out.put(k, a.get(k).union(b.get(k)));
    }
    out
}

fn fn_ref_to_sig(r: &FnRef) -> typer::FnSig {
    match &r.trait_name {
        None => typer::FnSig::new(r.type_name.clone(), r.method_name.clone()),
        Some(t) => typer::FnSig::new_trait(
            r.type_name.clone(),
            r.method_name.clone(),
            t.clone(),
        ),
    }
}

fn slots_mutated(block: &HirBlock) -> std::collections::HashSet<SlotId> {
    let mut out = std::collections::HashSet::new();
    collect_mut_block(block, &mut out);
    out
}

fn collect_mut_block(b: &HirBlock, out: &mut std::collections::HashSet<SlotId>) {
    for op in &b.ops {
        collect_mut_op(op, out);
    }
}

fn collect_mut_op(op: &HirOp, out: &mut std::collections::HashSet<SlotId>) {
    match op {
        HirOp::Set(s, _) | HirOp::Copy(s, _) => {
            out.insert(*s);
        }
        HirOp::MapValue(_, dst, _) => {
            out.insert(*dst);
        }
        HirOp::ReadRegister(s, _) => {
            out.insert(*s);
        }
        HirOp::Match(_, arms) => {
            for (b, _) in arms {
                collect_mut_block(b, out);
            }
        }
        HirOp::Loop(b) | HirOp::Block(b) => collect_mut_block(b, out),
        HirOp::Call { args, ret, .. } => {
            // Args may be mut-param writes; ret is always written.
            for s in args.iter().chain(ret.iter()) {
                out.insert(*s);
            }
        }
        HirOp::WriteRegister(_, Either::Right(_)) => {}
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;

    fn mk_sig(input_count: u32, output_count: u32, mut_slots: &[(u32, u32)]) -> typer::FlatSig {
        // mut_slots: list of (offset, size) for mutable input slots.
        let mut slots = Vec::new();
        let mut cursor = 0u32;
        let mut muts: std::collections::HashMap<u32, (u32, bool)> =
            std::collections::HashMap::new();
        for (off, size) in mut_slots {
            muts.insert(*off, (*size, true));
        }
        while cursor < input_count {
            let (size, mutable) = muts.get(&cursor).copied().unwrap_or((1, false));
            slots.push(typer::SlotInfo {
                name: format!("p{}", cursor),
                offset: cursor,
                size,
                mutable,
                type_name: "U4".into(),
                type_args: vec![],
            });
            cursor += size;
        }
        if output_count > 0 {
            slots.push(typer::SlotInfo {
                name: "_ret".into(),
                offset: input_count,
                size: output_count,
                mutable: true,
                type_name: "U4".into(),
                type_args: vec![],
            });
        }
        typer::FlatSig {
            slots,
            input_count,
            output_count,
            field_offsets: HashMap::new(),
        }
    }

    fn mk_func(sig: typer::FlatSig, body: HirBlock, slot_count: u32, name: &str) -> HirFunction {
        HirFunction {
            sig,
            body,
            slot_count,
            type_name: "T".into(),
            method_name: name.into(),
            warnings: vec![],
        }
    }

    #[test]
    fn output_set_to_constant_gets_singleton_exit() {
        let sig = mk_sig(0, 1, &[]);
        let body = HirBlock {
            ops: vec![HirOp::Set(SlotId(0), 7)],
            result_slot: None,
        };
        let f = mk_func(sig, body, 1, "f");
        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "f"), f);
        let summaries = compute_exit_domains(&fns, |_| false);
        let s = &summaries[&typer::FnSig::new("T", "f")];
        assert_eq!(s.output_exit, vec![Domain::singleton(7)]);
    }

    #[test]
    fn output_set_in_both_match_arms_unions() {
        let sig = mk_sig(1, 1, &[]);
        // if p0 == 0 { out = 3 } else { out = 5 }
        let body = HirBlock {
            ops: vec![HirOp::Match(
                SlotId(0),
                vec![
                    (
                        HirBlock {
                            ops: vec![HirOp::Set(SlotId(1), 3)],
                            result_slot: None,
                        },
                        vec![0],
                    ),
                    (
                        HirBlock {
                            ops: vec![HirOp::Set(SlotId(1), 5)],
                            result_slot: None,
                        },
                        (1..=15).collect(),
                    ),
                ],
            )],
            result_slot: None,
        };
        let f = mk_func(sig, body, 2, "f");
        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "f"), f);
        let summaries = compute_exit_domains(&fns, |_| false);
        let s = &summaries[&typer::FnSig::new("T", "f")];
        assert_eq!(s.output_exit, vec![Domain::from_values(&[3, 5])]);
    }

    #[test]
    fn mut_param_assigned_gets_exit_domain() {
        // fn f(mut self: U4): sets self = 9.
        let sig = mk_sig(1, 0, &[(0, 1)]);
        let body = HirBlock {
            ops: vec![HirOp::Set(SlotId(0), 9)],
            result_slot: None,
        };
        let f = mk_func(sig, body, 1, "f");
        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "f"), f);
        let summaries = compute_exit_domains(&fns, |_| false);
        let s = &summaries[&typer::FnSig::new("T", "f")];
        assert_eq!(s.input_mut, vec![true]);
        assert_eq!(s.input_exit, vec![Domain::singleton(9)]);
    }

    #[test]
    fn call_propagates_output_exit_to_caller() {
        // g(): returns 7.
        let g_sig = mk_sig(0, 1, &[]);
        let g_body = HirBlock {
            ops: vec![HirOp::Set(SlotId(0), 7)],
            result_slot: None,
        };
        let g = mk_func(g_sig, g_body, 1, "g");

        // f(): returns g().
        let f_sig = mk_sig(0, 1, &[]);
        let f_body = HirBlock {
            ops: vec![HirOp::Call {
                target: FnRef {
                    type_name: "T".into(),
                    method_name: "g".into(),
                    template_args: vec![],
                    trait_name: None,
                },
                args: vec![],
                ret: vec![SlotId(0)],
            }],
            result_slot: None,
        };
        let f = mk_func(f_sig, f_body, 1, "f");

        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "g"), g);
        fns.insert(typer::FnSig::new("T", "f"), f);
        let summaries = compute_exit_domains(&fns, |_| false);
        let f_exit = &summaries[&typer::FnSig::new("T", "f")];
        // f returns g's output, which is {7}.
        assert_eq!(f_exit.output_exit, vec![Domain::singleton(7)]);
    }
}
