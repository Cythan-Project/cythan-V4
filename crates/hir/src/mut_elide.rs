//! Mutability elision.
//!
//! `mut` on a parameter is a caller contract: when the callee
//! mutates the param, the inliner emits back-copy ops to make
//! those writes visible on the caller's side (see
//! `crates/hir/src/inline.rs` step 3). A `mut` param that isn't
//! actually mutated anywhere costs:
//!   * back-copy ops at every inlined call site (wasted MIR), and
//!   * a slot that `arg_elide` can't reclaim (since it
//!     conservatively preserves mutable params).
//!
//! A param is *effectively mutated* if any execution path reaches
//! a write to one of its cells. Writes happen either:
//!   1. **Locally** — the function's own body writes the cell
//!      (Set / Copy-dst / Inc / Dec / ReadRegister-dst, or a
//!      `Call.ret` landing on the cell), or
//!   2. **Transitively** — the cell is passed as `Call.args[i]`
//!      to a callee whose input cell `i` is itself effectively
//!      mutated (the inliner's back-copy propagates the write).
//!
//! We compute this across the program via a classic call-graph
//! fixpoint, then flip `SlotInfo.mutable = false` for any input
//! param whose cells are none of them effectively mutated.
//! `arg_elide` (run immediately after) then reclaims the freshly-
//! immutable-and-untouched ones.
//!
//! # Incremental-compilation friendliness
//!
//! The same two-phase split as `arg_elide`:
//!
//! * [`summarize_mutation`] inspects **one function's body** and
//!   produces an [`FnMutationSummary`] — the cacheable unit. It
//!   records:
//!   - which of the function's own input cells are locally
//!     written, and
//!   - the list of outgoing Calls, each with the caller→callee
//!     cell wiring needed to propagate callee mutation back.
//! * [`compute_effective_mutation`] iterates a monotone fixpoint
//!   over the summary map. An incremental scheduler can cache
//!   per-function summaries; re-run the fixpoint when any
//!   summary or callee's effective set changes.
//!
//! The apply step (flipping `mut` → `!mut`) is a deterministic
//! function of the fixpoint output + the input sig.

use std::collections::HashMap;

use crate::ir::{FnRef, HirBlock, HirFunction, HirOp, SlotId};

/// Per-function summary for the mutation analysis.
#[derive(Debug, Clone)]
pub struct FnMutationSummary {
    /// Length = `sig.input_count`. `local_writes[i]` is `true`
    /// iff the body itself writes to input cell `i` (not counting
    /// transitive writes through callees — those come from
    /// [`compute_effective_mutation`]).
    pub local_writes: Vec<bool>,
    /// Every `Call` in the body, captured for fixpoint
    /// propagation.
    pub calls: Vec<CallLink>,
}

/// One outgoing `Call` described as its caller→callee cell wiring.
#[derive(Debug, Clone)]
pub struct CallLink {
    pub target: typer::FnSig,
    /// Length = caller's `Call.args` length. `args[i] = Some(c)`
    /// when the argument at callee-side input cell `i` is the
    /// caller's own input cell `c`. `None` means the argument is
    /// a caller-local (or output/`_ret` cell) — no feedback to
    /// caller's inputs.
    pub args: Vec<Option<u32>>,
}

/// Compute the per-function summary. Reads only `f`'s body.
pub fn summarize_mutation(f: &HirFunction) -> FnMutationSummary {
    let ic = f.sig.input_count;
    let mut local_writes = vec![false; ic as usize];
    let mut calls = Vec::new();
    collect_block(&f.body, &mut local_writes, &mut calls, ic);
    FnMutationSummary {
        local_writes,
        calls,
    }
}

fn collect_block(
    b: &HirBlock,
    lw: &mut [bool],
    calls: &mut Vec<CallLink>,
    ic: u32,
) {
    for op in &b.ops {
        collect_op(op, lw, calls, ic);
    }
}

fn collect_op(
    op: &HirOp,
    lw: &mut [bool],
    calls: &mut Vec<CallLink>,
    ic: u32,
) {
    let mark = |s: SlotId, lw: &mut [bool]| {
        if s.0 < ic {
            lw[s.0 as usize] = true;
        }
    };
    match op {
        HirOp::Set(s, _) => mark(*s, lw),
        HirOp::Copy(dst, _) => mark(*dst, lw),
        HirOp::MapValue(_, dst, _) => mark(*dst, lw),
        HirOp::ReadRegister(dst, _) => mark(*dst, lw),
        HirOp::WriteRegister(..) => {}
        HirOp::Match(_, arms) => {
            for (b, _) in arms {
                collect_block(b, lw, calls, ic);
            }
        }
        HirOp::Loop(b) | HirOp::Block(b) => collect_block(b, lw, calls, ic),
        HirOp::Call { target, args, ret } => {
            // Call-ret cells that land on our inputs count as
            // local writes (the caller's input is written by the
            // Call's output-copy).
            for r in ret {
                mark(*r, lw);
            }
            let args_in: Vec<Option<u32>> = args
                .iter()
                .map(|a| if a.0 < ic { Some(a.0) } else { None })
                .collect();
            calls.push(CallLink {
                target: fn_ref_to_sig(target),
                args: args_in,
            });
        }
        HirOp::Break | HirOp::Continue | HirOp::Stop | HirOp::Skip => {}
    }
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

/// Given per-function summaries, iterate a monotone fixpoint and
/// return the effective mutation set for each function.
///
/// `effective[sig][i]` is `true` iff input cell `i` of function
/// `sig` is written on at least one execution path (locally or
/// transitively through callees). Never flips back to `false`.
///
/// **Soundness for unknown callees**: a `Call` whose target sig
/// isn't in `summaries` is treated as mutating every one of its
/// argument cells *unless* `is_non_mutating_extern` returns
/// `true` for that target. Callers that know their externs don't
/// mutate caller slots (e.g. `System::setRegister`, which only
/// writes to VM registers) should recognise them in that
/// predicate — otherwise the analysis conservatively keeps every
/// arg to an unknown target marked as mutated.
pub fn compute_effective_mutation<F>(
    summaries: &HashMap<typer::FnSig, FnMutationSummary>,
    is_non_mutating_extern: F,
) -> HashMap<typer::FnSig, Vec<bool>>
where
    F: Fn(&typer::FnSig) -> bool,
{
    let mut effective: HashMap<typer::FnSig, Vec<bool>> = summaries
        .iter()
        .map(|(k, s)| (k.clone(), s.local_writes.clone()))
        .collect();

    loop {
        let mut changed = false;
        for (sig, summary) in summaries {
            // Gather marks contributed by this function's Calls
            // into its own input cells, then apply in a second
            // pass (keeps the borrow checker happy when sig ==
            // link.target, i.e. a self-recursive function).
            let mut new_marks: Vec<u32> = Vec::new();
            for link in &summary.calls {
                match effective.get(&link.target) {
                    Some(target_eff) => {
                        for (i, slot) in link.args.iter().enumerate() {
                            if i >= target_eff.len() {
                                continue;
                            }
                            if !target_eff[i] {
                                continue;
                            }
                            if let Some(caller_cell) = slot {
                                new_marks.push(*caller_cell);
                            }
                        }
                    }
                    None if is_non_mutating_extern(&link.target) => {
                        // Whitelisted extern — known not to mutate
                        // caller slots.
                    }
                    None => {
                        // Unknown target: be conservative, mark
                        // every caller-input cell it could touch.
                        for slot in &link.args {
                            if let Some(caller_cell) = slot {
                                new_marks.push(*caller_cell);
                            }
                        }
                    }
                }
            }
            let Some(e) = effective.get_mut(sig) else {
                continue;
            };
            for c in new_marks {
                let idx = c as usize;
                if idx < e.len() && !e[idx] {
                    e[idx] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    effective
}

/// Stats for the apply step.
#[derive(Debug, Default, Clone, Copy)]
pub struct MutationElideStats {
    /// Number of param slots whose `mutable` flag was flipped
    /// from `true` to `false`.
    pub params_flipped: u32,
}

/// Flip `SlotInfo.mutable = false` on any input param whose cells
/// are none of them effectively mutated. Returns the rewritten
/// map plus stats.
///
/// See [`compute_effective_mutation`] for the role of
/// `is_non_mutating_extern`.
pub fn elide_redundant_mut_with_stats<F>(
    fns: HashMap<typer::FnSig, HirFunction>,
    is_non_mutating_extern: F,
) -> (HashMap<typer::FnSig, HirFunction>, MutationElideStats)
where
    F: Fn(&typer::FnSig) -> bool,
{
    let summaries: HashMap<typer::FnSig, FnMutationSummary> = fns
        .iter()
        .map(|(k, f)| (k.clone(), summarize_mutation(f)))
        .collect();
    let effective = compute_effective_mutation(&summaries, is_non_mutating_extern);
    apply_effective_mutation(fns, &effective)
}

/// Deterministic apply step, exposed so an incremental driver
/// can cache summaries + the fixpoint result and skip straight
/// to the rewrite.
pub fn apply_effective_mutation(
    fns: HashMap<typer::FnSig, HirFunction>,
    effective: &HashMap<typer::FnSig, Vec<bool>>,
) -> (HashMap<typer::FnSig, HirFunction>, MutationElideStats) {
    let mut stats = MutationElideStats::default();
    let mut out = HashMap::with_capacity(fns.len());
    for (sig, mut func) in fns {
        let Some(eff) = effective.get(&sig) else {
            out.insert(sig, func);
            continue;
        };
        for slot in func.sig.slots.iter_mut() {
            if slot.name == "_ret" {
                continue;
            }
            if !slot.mutable {
                continue;
            }
            let any_written =
                (0..slot.size).any(|i| eff[(slot.offset + i) as usize]);
            if !any_written {
                slot.mutable = false;
                stats.params_flipped += 1;
            }
        }
        out.insert(sig, func);
    }
    (out, stats)
}

/// Convenience one-shot.
pub fn elide_redundant_mut<F>(
    fns: HashMap<typer::FnSig, HirFunction>,
    is_non_mutating_extern: F,
) -> HashMap<typer::FnSig, HirFunction>
where
    F: Fn(&typer::FnSig) -> bool,
{
    elide_redundant_mut_with_stats(fns, is_non_mutating_extern).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;
    use either::Either;

    fn mk_slot(name: &str, offset: u32, size: u32, mutable: bool) -> typer::SlotInfo {
        typer::SlotInfo {
            name: name.into(),
            offset,
            size,
            mutable,
            type_name: "U4".into(),
            type_args: vec![],
        }
    }

    fn mk_sig(input_slots: Vec<typer::SlotInfo>, output_size: u32) -> typer::FlatSig {
        let input_count: u32 = input_slots.iter().map(|s| s.size).sum();
        let mut slots = input_slots;
        if output_size > 0 {
            slots.push(mk_slot("_ret", input_count, output_size, true));
        }
        typer::FlatSig {
            slots,
            input_count,
            output_count: output_size,
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
    fn flips_mut_when_body_never_writes() {
        let sig = mk_sig(vec![mk_slot("a", 0, 1, true)], 0);
        let body = HirBlock {
            ops: vec![HirOp::WriteRegister(3, Either::Right(SlotId(0)))],
            result_slot: None,
        };
        let f = mk_func(sig, body, 1, "f");
        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "f"), f);

        let (out, stats) = elide_redundant_mut_with_stats(fns, |_| false);
        assert_eq!(stats.params_flipped, 1);
        assert_eq!(out[&typer::FnSig::new("T", "f")].sig.slots[0].mutable, false);
    }

    #[test]
    fn keeps_mut_when_body_writes() {
        let sig = mk_sig(vec![mk_slot("a", 0, 1, true)], 0);
        let body = HirBlock {
            ops: vec![HirOp::MapValue(SlotId(0), SlotId(0), INC_TABLE)],
            result_slot: None,
        };
        let f = mk_func(sig, body, 1, "f");
        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "f"), f);

        let (out, stats) = elide_redundant_mut_with_stats(fns, |_| false);
        assert_eq!(stats.params_flipped, 0);
        assert_eq!(out[&typer::FnSig::new("T", "f")].sig.slots[0].mutable, true);
    }

    #[test]
    fn propagates_through_call_then_flips() {
        // g(a: &mut U4) { /* never writes a */ }
        // f(b: &mut U4) { g(b) }
        // Expect: both g and f flip.
        let g_sig = mk_sig(vec![mk_slot("a", 0, 1, true)], 0);
        let g_body = HirBlock { ops: vec![], result_slot: None };
        let g = mk_func(g_sig, g_body, 1, "g");

        let f_sig = mk_sig(vec![mk_slot("b", 0, 1, true)], 0);
        let f_body = HirBlock {
            ops: vec![HirOp::Call {
                target: FnRef {
                    type_name: "T".into(),
                    method_name: "g".into(),
                    template_args: vec![],
                    trait_name: None,
                },
                args: vec![SlotId(0)],
                ret: vec![],
            }],
            result_slot: None,
        };
        let f = mk_func(f_sig, f_body, 1, "f");

        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "g"), g);
        fns.insert(typer::FnSig::new("T", "f"), f);

        let (out, stats) = elide_redundant_mut_with_stats(fns, |_| false);
        assert_eq!(stats.params_flipped, 2);
        assert_eq!(out[&typer::FnSig::new("T", "g")].sig.slots[0].mutable, false);
        assert_eq!(out[&typer::FnSig::new("T", "f")].sig.slots[0].mutable, false);
    }

    #[test]
    fn propagation_keeps_mut_when_callee_actually_mutates() {
        // g(a: &mut U4) { a += 1 }
        // f(b: &mut U4) { g(b) }  — b is transitively mutated.
        let g_sig = mk_sig(vec![mk_slot("a", 0, 1, true)], 0);
        let g_body = HirBlock {
            ops: vec![HirOp::MapValue(SlotId(0), SlotId(0), INC_TABLE)],
            result_slot: None,
        };
        let g = mk_func(g_sig, g_body, 1, "g");

        let f_sig = mk_sig(vec![mk_slot("b", 0, 1, true)], 0);
        let f_body = HirBlock {
            ops: vec![HirOp::Call {
                target: FnRef {
                    type_name: "T".into(),
                    method_name: "g".into(),
                    template_args: vec![],
                    trait_name: None,
                },
                args: vec![SlotId(0)],
                ret: vec![],
            }],
            result_slot: None,
        };
        let f = mk_func(f_sig, f_body, 1, "f");

        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "g"), g);
        fns.insert(typer::FnSig::new("T", "f"), f);

        let (out, stats) = elide_redundant_mut_with_stats(fns, |_| false);
        assert_eq!(stats.params_flipped, 0);
        assert_eq!(out[&typer::FnSig::new("T", "g")].sig.slots[0].mutable, true);
        assert_eq!(out[&typer::FnSig::new("T", "f")].sig.slots[0].mutable, true);
    }

    #[test]
    fn recursive_function_without_writes_flips() {
        // f(a: &mut U4) { f(a) }  — no writes anywhere, should flip.
        let f_sig = mk_sig(vec![mk_slot("a", 0, 1, true)], 0);
        let f_body = HirBlock {
            ops: vec![HirOp::Call {
                target: FnRef {
                    type_name: "T".into(),
                    method_name: "f".into(),
                    template_args: vec![],
                    trait_name: None,
                },
                args: vec![SlotId(0)],
                ret: vec![],
            }],
            result_slot: None,
        };
        let f = mk_func(f_sig, f_body, 1, "f");

        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "f"), f);

        let (out, stats) = elide_redundant_mut_with_stats(fns, |_| false);
        assert_eq!(stats.params_flipped, 1);
        assert_eq!(out[&typer::FnSig::new("T", "f")].sig.slots[0].mutable, false);
    }

    #[test]
    fn unknown_callee_is_treated_as_mutating_all_args() {
        // f(a: &mut U4) passes `a` to `Unknown::thing`. Unknown
        // isn't in the summary map → must conservatively keep
        // f's `a` as mut to preserve the inliner's back-copy
        // when the target is resolved later.
        let f_sig = mk_sig(vec![mk_slot("a", 0, 1, true)], 0);
        let f_body = HirBlock {
            ops: vec![HirOp::Call {
                target: FnRef {
                    type_name: "Unknown".into(),
                    method_name: "thing".into(),
                    template_args: vec![],
                    trait_name: None,
                },
                args: vec![SlotId(0)],
                ret: vec![],
            }],
            result_slot: None,
        };
        let f = mk_func(f_sig, f_body, 1, "f");
        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "f"), f);

        let (out, stats) = elide_redundant_mut_with_stats(fns, |_| false);
        assert_eq!(stats.params_flipped, 0);
        assert_eq!(out[&typer::FnSig::new("T", "f")].sig.slots[0].mutable, true);
    }

    #[test]
    fn multi_cell_param_flips_only_when_no_cell_written() {
        let sig = mk_sig(vec![mk_slot("s", 0, 3, true)], 0);
        // Body writes cell 1 of the param → keep mut.
        let body = HirBlock {
            ops: vec![HirOp::MapValue(SlotId(1), SlotId(1), INC_TABLE)],
            result_slot: None,
        };
        let f = mk_func(sig, body, 3, "f");
        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "f"), f);

        let (out, stats) = elide_redundant_mut_with_stats(fns, |_| false);
        assert_eq!(stats.params_flipped, 0);
        assert_eq!(out[&typer::FnSig::new("T", "f")].sig.slots[0].mutable, true);
    }
}
