//! Unused function argument elision.
//!
//! A parameter is "unused" when every cell in its slot span is
//! untouched by the function body (neither read nor written) and
//! the param is non-mutable. For such params we:
//!   1. Remove the slot from the callee's signature (`input_count`
//!      shrinks; the `SlotInfo` entry is dropped; offsets of
//!      kept slots + outputs shift left to fill the vacated space).
//!   2. Filter the caller-side `Call.args` for every call site
//!      that targets the callee, dropping the cell positions the
//!      callee no longer receives.
//!   3. Renumber slot ids in the callee's body so output + local
//!      slots shift into the space vacated by dropped inputs.
//!
//! Mutable params are always preserved — the inliner's mut-back
//! copy (`crates/hir/src/inline.rs`, step 3) makes callee-side
//! writes visible to the caller's original argument cell, so we
//! can't safely drop a mutable slot even if the body doesn't read
//! it.
//!
//! # Incremental-compilation friendliness
//!
//! The pass splits cleanly in two so future incremental-build
//! tooling has the right granularity to cache:
//!
//! * [`summarize`] inspects **one function's own body** and nothing
//!   else, producing a tiny [`FnSummary`]. The natural per-function
//!   cache unit.
//! * [`elide_unused_args`] takes a `(fns, summaries)` pair and
//!   performs a deterministic rewrite. An incremental scheduler
//!   recomputes a callee's summary when the callee body changes;
//!   re-runs the rewrite for a caller when any of its callees'
//!   summaries change.
//!
//! A fixpoint loop is needed because eliding an arg in callee F
//! may make a caller G's arg newly-dead (G only read it to pass
//! it to F; once F no longer accepts it, the read vanishes).

use std::collections::HashMap;

use either::Either;

use crate::ir::{FnRef, HirBlock, HirFunction, HirOp, SlotId};

/// Per-function usage summary. Entirely derived from the function
/// body — suitable for per-function caching.
#[derive(Debug, Clone)]
pub struct FnSummary {
    /// One entry per input parameter, in `sig.slots` order (entries
    /// for the `_ret` slot are excluded). `true` = keep this param.
    ///
    /// Kept unconditionally when the param is mutable; otherwise
    /// kept iff at least one of its cells is touched (read or
    /// written) somewhere in the body.
    pub keep_input_param: Vec<bool>,
}

/// Compute the usage summary for a function. Pure function of the
/// body — no dependency on callers or other callees.
pub fn summarize(f: &HirFunction) -> FnSummary {
    let ic = f.sig.input_count as usize;
    let mut touched = vec![false; ic];
    mark_touched_block(&f.body, &mut touched, f.sig.input_count);

    let mut keep = Vec::new();
    for slot in &f.sig.slots {
        if slot.name == "_ret" {
            break;
        }
        if slot.mutable {
            keep.push(true);
            continue;
        }
        let any_touched =
            (0..slot.size).any(|i| touched[(slot.offset + i) as usize]);
        keep.push(any_touched);
    }
    FnSummary {
        keep_input_param: keep,
    }
}

fn mark_touched_block(b: &HirBlock, out: &mut [bool], ic: u32) {
    for op in &b.ops {
        mark_touched_op(op, out, ic);
    }
}

fn mark_touched_op(op: &HirOp, out: &mut [bool], ic: u32) {
    let mark = |s: SlotId, out: &mut [bool]| {
        if s.0 < ic {
            out[s.0 as usize] = true;
        }
    };
    match op {
        HirOp::Set(s, _) => mark(*s, out),
        HirOp::Copy(dst, src) => {
            mark(*dst, out);
            mark(*src, out);
        }
        HirOp::MapValue(src, dst, _) => {
            mark(*src, out);
            mark(*dst, out);
        }
        HirOp::ReadRegister(dst, _) => mark(*dst, out),
        HirOp::WriteRegister(_, Either::Left(_)) => {}
        HirOp::WriteRegister(_, Either::Right(s)) => mark(*s, out),
        HirOp::Match(s, arms) => {
            mark(*s, out);
            for (b, _) in arms {
                mark_touched_block(b, out, ic);
            }
        }
        HirOp::Loop(b) | HirOp::Block(b) => mark_touched_block(b, out, ic),
        HirOp::Call { args, ret, .. } => {
            for a in args {
                mark(*a, out);
            }
            for r in ret {
                mark(*r, out);
            }
        }
        HirOp::Break | HirOp::Continue | HirOp::Stop | HirOp::Skip => {}
    }
}

/// How many cells were dropped (aggregated across all functions
/// and all fixpoint rounds) and how many functions lost at least
/// one parameter.
#[derive(Debug, Default, Clone, Copy)]
pub struct ElideStats {
    pub dropped_cells: u32,
    pub functions_trimmed: u32,
}

/// Rewrite `fns` so that unused parameters are dropped from callee
/// signatures and the corresponding cells are filtered out of
/// caller-side `Call.args`. Iterates to a fixpoint.
///
/// `entry` is preserved verbatim: its sig is the harness contract
/// and cannot be changed without updating the runner.
pub fn elide_unused_args(
    fns: HashMap<typer::FnSig, HirFunction>,
    entry: &typer::FnSig,
) -> HashMap<typer::FnSig, HirFunction> {
    elide_unused_args_with_stats(fns, entry).0
}

/// Like [`elide_unused_args`] but also returns aggregated stats.
pub fn elide_unused_args_with_stats(
    fns: HashMap<typer::FnSig, HirFunction>,
    entry: &typer::FnSig,
) -> (HashMap<typer::FnSig, HirFunction>, ElideStats) {
    let mut current = fns;
    let mut stats = ElideStats::default();
    loop {
        let summaries: HashMap<typer::FnSig, FnSummary> = current
            .iter()
            .map(|(k, f)| (k.clone(), summarize(f)))
            .collect();
        let remaps = build_remaps(&current, &summaries, entry);
        if remaps.is_empty() {
            return (current, stats);
        }
        for r in remaps.values() {
            let dropped = r.kept_input_cell.iter().filter(|&&b| !b).count() as u32;
            stats.dropped_cells += dropped;
            stats.functions_trimmed += 1;
        }
        current = apply_remaps(current, &remaps);
    }
}

/// Run a single round (no fixpoint iteration) given a summary
/// map. Exposed as its own entry point because an incremental
/// driver can pre-compute + cache summaries and call this directly.
pub fn apply_summaries(
    fns: HashMap<typer::FnSig, HirFunction>,
    summaries: &HashMap<typer::FnSig, FnSummary>,
    entry: &typer::FnSig,
) -> HashMap<typer::FnSig, HirFunction> {
    let remaps = build_remaps(&fns, summaries, entry);
    if remaps.is_empty() {
        return fns;
    }
    apply_remaps(fns, &remaps)
}

// ---- internals ------------------------------------------------------------

/// Per-function elision plan: which cells survive, and the old→new
/// slot id permutation that falls out.
struct SlotRemap {
    /// Length = old `sig.input_count`. `kept_input_cell[i]` == true
    /// means the caller must keep passing its arg cell at position
    /// `i`; false means drop.
    kept_input_cell: Vec<bool>,
    /// `map[old_slot_id as usize] = Some(new_slot_id)` when the
    /// slot survives, `None` when it was dropped. The body must
    /// never reference a `None` entry after rewrite.
    map: Vec<Option<SlotId>>,
    new_sig: typer::FlatSig,
    new_slot_count: u32,
}

fn build_remaps(
    fns: &HashMap<typer::FnSig, HirFunction>,
    summaries: &HashMap<typer::FnSig, FnSummary>,
    entry: &typer::FnSig,
) -> HashMap<typer::FnSig, SlotRemap> {
    let mut remaps = HashMap::new();
    for (sig, func) in fns {
        if sig == entry {
            continue;
        }
        let Some(summary) = summaries.get(sig) else { continue };
        if summary.keep_input_param.iter().any(|&b| !b) {
            remaps.insert(sig.clone(), build_remap(func, summary));
        }
    }
    remaps
}

fn build_remap(f: &HirFunction, summary: &FnSummary) -> SlotRemap {
    let old_ic = f.sig.input_count;
    let old_oc = f.sig.output_count;
    let old_slot_count = f.slot_count;

    // Per-cell kept mask derived from per-param kept flags.
    let mut kept_input_cell = vec![false; old_ic as usize];
    {
        let mut param_idx = 0;
        for slot in &f.sig.slots {
            if slot.name == "_ret" {
                break;
            }
            let keep = summary.keep_input_param[param_idx];
            for i in 0..slot.size {
                kept_input_cell[(slot.offset + i) as usize] = keep;
            }
            param_idx += 1;
        }
    }

    let new_ic: u32 = kept_input_cell.iter().filter(|&&b| b).count() as u32;
    let drop_total = old_ic - new_ic;

    // Old→new slot permutation: kept inputs get sequential new ids;
    // everything from `old_ic` up shifts left by `drop_total`
    // (inputs are contiguous at the start, so the shift is uniform
    // for outputs + locals).
    let mut map: Vec<Option<SlotId>> = Vec::with_capacity(old_slot_count as usize);
    {
        let mut next_new = 0u32;
        for i in 0..old_ic {
            if kept_input_cell[i as usize] {
                map.push(Some(SlotId(next_new)));
                next_new += 1;
            } else {
                map.push(None);
            }
        }
        for i in old_ic..old_slot_count {
            map.push(Some(SlotId(i - drop_total)));
        }
    }

    // New sig.slots: drop dropped params, recompute offsets for
    // kept slots + `_ret`. Field offsets for dropped params are
    // discarded; kept params' field offsets are *relative to
    // the param's start* so they're unaffected by the shift.
    let mut new_slots: Vec<typer::SlotInfo> = Vec::new();
    let mut new_field_offsets: HashMap<String, Vec<typer::FieldSlot>> =
        HashMap::new();
    {
        let mut cursor = 0u32;
        let mut param_idx = 0;
        for slot in &f.sig.slots {
            let keep = if slot.name == "_ret" {
                true
            } else {
                let k = summary.keep_input_param[param_idx];
                param_idx += 1;
                k
            };
            if !keep {
                continue;
            }
            let mut new_slot = slot.clone();
            new_slot.offset = cursor;
            if let Some(fo) = f.sig.field_offsets.get(&slot.name) {
                new_field_offsets.insert(slot.name.clone(), fo.clone());
            }
            cursor += slot.size;
            new_slots.push(new_slot);
        }
    }

    let new_sig = typer::FlatSig {
        slots: new_slots,
        input_count: new_ic,
        output_count: old_oc,
        field_offsets: new_field_offsets,
    };
    let new_slot_count = old_slot_count - drop_total;

    SlotRemap {
        kept_input_cell,
        map,
        new_sig,
        new_slot_count,
    }
}

fn apply_remaps(
    fns: HashMap<typer::FnSig, HirFunction>,
    remaps: &HashMap<typer::FnSig, SlotRemap>,
) -> HashMap<typer::FnSig, HirFunction> {
    let mut out = HashMap::with_capacity(fns.len());
    for (sig, func) in fns {
        let self_remap = remaps.get(&sig);
        let new_body = rewrite_block(func.body, self_remap, remaps);
        let (new_sig, new_slot_count) = match self_remap {
            Some(r) => (r.new_sig.clone(), r.new_slot_count),
            None => (func.sig, func.slot_count),
        };
        out.insert(
            sig,
            HirFunction {
                sig: new_sig,
                body: new_body,
                slot_count: new_slot_count,
                type_name: func.type_name,
                method_name: func.method_name,
                warnings: func.warnings,
            },
        );
    }
    out
}

fn rewrite_block(
    b: HirBlock,
    self_remap: Option<&SlotRemap>,
    remaps: &HashMap<typer::FnSig, SlotRemap>,
) -> HirBlock {
    let ops = b
        .ops
        .into_iter()
        .map(|op| rewrite_op(op, self_remap, remaps))
        .collect();
    let result_slot = b.result_slot.map(|s| map_slot(s, self_remap));
    HirBlock { ops, result_slot }
}

fn rewrite_op(
    op: HirOp,
    self_remap: Option<&SlotRemap>,
    remaps: &HashMap<typer::FnSig, SlotRemap>,
) -> HirOp {
    match op {
        HirOp::Set(s, v) => HirOp::Set(map_slot(s, self_remap), v),
        HirOp::Copy(dst, src) => {
            HirOp::Copy(map_slot(dst, self_remap), map_slot(src, self_remap))
        }
        HirOp::MapValue(src, dst, table) => HirOp::MapValue(
            map_slot(src, self_remap),
            map_slot(dst, self_remap),
            table,
        ),
        HirOp::ReadRegister(dst, r) => {
            HirOp::ReadRegister(map_slot(dst, self_remap), r)
        }
        HirOp::WriteRegister(r, Either::Left(v)) => {
            HirOp::WriteRegister(r, Either::Left(v))
        }
        HirOp::WriteRegister(r, Either::Right(s)) => {
            HirOp::WriteRegister(r, Either::Right(map_slot(s, self_remap)))
        }
        HirOp::Loop(b) => HirOp::Loop(rewrite_block(b, self_remap, remaps)),
        HirOp::Block(b) => HirOp::Block(rewrite_block(b, self_remap, remaps)),
        HirOp::Match(s, arms) => {
            let new_arms = arms
                .into_iter()
                .map(|(b, v)| (rewrite_block(b, self_remap, remaps), v))
                .collect();
            HirOp::Match(map_slot(s, self_remap), new_arms)
        }
        HirOp::Break => HirOp::Break,
        HirOp::Continue => HirOp::Continue,
        HirOp::Stop => HirOp::Stop,
        HirOp::Skip => HirOp::Skip,
        HirOp::Call { target, args, ret } => {
            let target_sig = fn_ref_to_sig(&target);
            // First: filter args by callee's kept-cell mask. This
            // takes the arg list from the *old* schema to the
            // *new* schema for the callee's inputs.
            let filtered_args: Vec<SlotId> =
                if let Some(r) = remaps.get(&target_sig) {
                    args.into_iter()
                        .zip(r.kept_input_cell.iter())
                        .filter_map(
                            |(a, keep)| if *keep { Some(a) } else { None },
                        )
                        .collect()
                } else {
                    args
                };
            // Then: remap slot ids through *caller's* own remap
            // (the surviving args are still expressed in the
            // caller's slot namespace).
            let mapped_args = filtered_args
                .into_iter()
                .map(|a| map_slot(a, self_remap))
                .collect();
            let mapped_ret = ret
                .into_iter()
                .map(|r| map_slot(r, self_remap))
                .collect();
            HirOp::Call {
                target,
                args: mapped_args,
                ret: mapped_ret,
            }
        }
    }
}

fn map_slot(s: SlotId, self_remap: Option<&SlotRemap>) -> SlotId {
    match self_remap {
        None => s,
        Some(r) => r.map[s.0 as usize].unwrap_or_else(|| {
            panic!(
                "arg_elide: body references dropped input slot {:?} \
                 (likely a non-mutable param write the summary missed)",
                s
            )
        }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;

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

    fn mk_func(
        sig: typer::FlatSig,
        body: HirBlock,
        slot_count: u32,
    ) -> HirFunction {
        HirFunction {
            sig,
            body,
            slot_count,
            type_name: "T".into(),
            method_name: "m".into(),
            warnings: vec![],
        }
    }

    #[test]
    fn summary_keeps_mutable_param_even_if_untouched() {
        let sig = mk_sig(
            vec![mk_slot("a", 0, 1, true), mk_slot("b", 1, 1, false)],
            0,
        );
        let body = HirBlock { ops: vec![], result_slot: None };
        let f = mk_func(sig, body, 2);
        let s = summarize(&f);
        assert_eq!(s.keep_input_param, vec![true, false]);
    }

    #[test]
    fn summary_drops_untouched_immutable_param() {
        let sig = mk_sig(
            vec![mk_slot("used", 0, 1, false), mk_slot("dead", 1, 1, false)],
            0,
        );
        let body = HirBlock {
            ops: vec![HirOp::MapValue(SlotId(0), SlotId(0), crate::ir::INC_TABLE)],
            result_slot: None,
        };
        let f = mk_func(sig, body, 2);
        let s = summarize(&f);
        assert_eq!(s.keep_input_param, vec![true, false]);
    }

    #[test]
    fn multi_cell_param_kept_if_any_cell_read() {
        let sig = mk_sig(
            vec![mk_slot("multi", 0, 3, false), mk_slot("dead", 3, 1, false)],
            0,
        );
        let body = HirBlock {
            ops: vec![HirOp::Copy(SlotId(10), SlotId(2))],
            result_slot: None,
        };
        let f = mk_func(sig, body, 11);
        let s = summarize(&f);
        assert_eq!(s.keep_input_param, vec![true, false]);
    }

    #[test]
    fn remap_renumbers_outputs_and_locals_after_dropped_inputs() {
        // 2-input param (both immutable, 1 cell each); 1 output; 1 local.
        // Drop input at index 0.
        let sig = mk_sig(
            vec![mk_slot("dead", 0, 1, false), mk_slot("used", 1, 1, false)],
            1,
        );
        // slot 0: dead input
        // slot 1: used input  (read by body)
        // slot 2: output  (_ret)
        // slot 3: local
        let body = HirBlock {
            ops: vec![
                HirOp::Copy(SlotId(3), SlotId(1)),
                HirOp::Copy(SlotId(2), SlotId(3)),
            ],
            result_slot: None,
        };
        let f = mk_func(sig, body, 4);
        let summary = FnSummary { keep_input_param: vec![false, true] };
        let remap = build_remap(&f, &summary);
        assert_eq!(remap.kept_input_cell, vec![false, true]);
        assert_eq!(remap.new_sig.input_count, 1);
        assert_eq!(remap.new_sig.output_count, 1);
        assert_eq!(remap.new_slot_count, 3);
        // slot 1 (used input) → slot 0; slot 2 (_ret) → slot 1; slot 3 (local) → slot 2.
        assert_eq!(remap.map[0], None);
        assert_eq!(remap.map[1], Some(SlotId(0)));
        assert_eq!(remap.map[2], Some(SlotId(1)));
        assert_eq!(remap.map[3], Some(SlotId(2)));
    }

    #[test]
    fn elide_filters_caller_args_and_remaps_body() {
        // Callee `cee(dead, used)` drops `dead`.
        let cee_sig = mk_sig(
            vec![mk_slot("dead", 0, 1, false), mk_slot("used", 1, 1, false)],
            1,
        );
        let cee_body = HirBlock {
            ops: vec![HirOp::Copy(SlotId(2), SlotId(1))],
            result_slot: None,
        };
        let cee = mk_func(cee_sig, cee_body, 3);

        // Caller: has local slots 0..3, calls cee with args [slot0, slot1], ret = [slot2].
        let caller_sig = mk_sig(vec![], 0);
        let caller_body = HirBlock {
            ops: vec![HirOp::Call {
                target: FnRef {
                    type_name: "T".into(),
                    method_name: "cee".into(),
                    template_args: vec![],
                    trait_name: None,
                },
                args: vec![SlotId(0), SlotId(1)],
                ret: vec![SlotId(2)],
            }],
            result_slot: None,
        };
        let caller = mk_func(caller_sig, caller_body, 3);

        let entry = typer::FnSig::new("T", "main");
        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "cee"), {
            let mut c = cee.clone();
            c.method_name = "cee".into();
            c
        });
        fns.insert(entry.clone(), {
            let mut c = caller.clone();
            c.method_name = "main".into();
            c
        });

        let out = elide_unused_args(fns, &entry);
        let new_caller = &out[&entry];
        let HirOp::Call { args, ret, .. } = &new_caller.body.ops[0] else {
            panic!("expected Call");
        };
        // The dead arg (slot 0 in the caller's arg list) is dropped;
        // slot 1 survives. Caller slot ids don't change (no elision
        // on caller itself).
        assert_eq!(args, &vec![SlotId(1)]);
        assert_eq!(ret, &vec![SlotId(2)]);

        // Callee sig + body got rewritten.
        let new_cee = &out[&typer::FnSig::new("T", "cee")];
        assert_eq!(new_cee.sig.input_count, 1);
        assert_eq!(new_cee.slot_count, 2);
        // Body: Copy(SlotId(1), SlotId(0)) — output was at 2, now at 1; used input was at 1, now at 0.
        assert_eq!(
            new_cee.body.ops,
            vec![HirOp::Copy(SlotId(1), SlotId(0))]
        );
    }

    #[test]
    fn elide_leaves_entry_unchanged() {
        // Entry with an unused immutable arg stays as-is.
        let entry_sig = mk_sig(vec![mk_slot("unused", 0, 1, false)], 0);
        let entry_body = HirBlock { ops: vec![], result_slot: None };
        let entry_fn = mk_func(entry_sig, entry_body, 1);
        let entry = typer::FnSig::new("T", "m");

        let mut fns = HashMap::new();
        fns.insert(entry.clone(), entry_fn.clone());

        let out = elide_unused_args(fns, &entry);
        assert_eq!(out[&entry].sig.input_count, 1);
        assert_eq!(out[&entry].slot_count, 1);
    }

    #[test]
    fn elide_reaches_fixpoint_cascading_across_callees() {
        // g(a) calls f(a); f(a) never reads a.
        // Round 1: f drops a → Call in g loses the arg.
        // Round 2: g's a is now untouched → g drops a.
        let f_sig = mk_sig(vec![mk_slot("a", 0, 1, false)], 0);
        let f_body = HirBlock { ops: vec![], result_slot: None };
        let f = mk_func(f_sig, f_body, 1);

        let g_sig = mk_sig(vec![mk_slot("a", 0, 1, false)], 0);
        let g_body = HirBlock {
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
        let g = mk_func(g_sig, g_body, 1);

        let entry = typer::FnSig::new("T", "main");
        let entry_body = HirBlock {
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
        let entry_sig = mk_sig(vec![mk_slot("x", 0, 1, false)], 0);
        let entry_fn = mk_func(entry_sig, entry_body, 1);

        let mut fns = HashMap::new();
        fns.insert(typer::FnSig::new("T", "f"), {
            let mut c = f;
            c.method_name = "f".into();
            c
        });
        fns.insert(typer::FnSig::new("T", "g"), {
            let mut c = g;
            c.method_name = "g".into();
            c
        });
        fns.insert(entry.clone(), {
            let mut c = entry_fn;
            c.method_name = "main".into();
            c
        });

        let out = elide_unused_args(fns, &entry);
        assert_eq!(out[&typer::FnSig::new("T", "f")].sig.input_count, 0);
        assert_eq!(out[&typer::FnSig::new("T", "g")].sig.input_count, 0);
        // Entry preserved.
        assert_eq!(out[&entry].sig.input_count, 1);
        // Entry's Call to g has zero args now.
        let HirOp::Call { args, .. } = &out[&entry].body.ops[0] else {
            panic!();
        };
        assert!(args.is_empty());
    }
}
