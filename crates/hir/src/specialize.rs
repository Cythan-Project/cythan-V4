//! HIR specialization pass — flow-sensitive **domain tracking**.
//!
//! Runs on the inlined program body (after `inline_program_full`)
//! and before `hir_to_mir`.
//!
//! # What's "domain tracking"?
//!
//! For every u4 slot in scope we remember the *set of values* it
//! could possibly hold at this program point. Default is `Domain::ALL`
//! (all 16 u4 values — "we know nothing"). The set is a 16-bit
//! bitmask (`Domain(u16)`), so union / intersection / test for
//! containment are one-instruction.
//!
//! This is strictly richer than tracking single constants:
//!
//! * A `Set(slot, 7)` narrows the domain to the singleton `{7}`.
//!   Match folds the arm whose values contain `7`.
//! * A `Copy(dst, src)` copies the source's domain.
//! * `Inc` / `Dec` shift the whole bitmask (with wrap-around).
//! * **Entering a match arm narrows the scrutinee's domain to that
//!   arm's values** — even when the outer domain was `ALL`. This is
//!   what lets `if x == 0 { if x == 0 { … } }` collapse: inside
//!   the outer `then` arm, `x ∈ {0}`, so the nested `Match`'s
//!   `1..=15` arm is unreachable and its body is dropped.
//! * **Entering a match arm can also narrow by forbidden values**:
//!   inside the `else` arm of an `if_zero`, the scrutinee's domain
//!   becomes `{1..15}`. A nested `if_zero(s, …)` on that same slot
//!   proves the `0` arm dead and specializes to just the `else`
//!   body.
//! * Arms whose value sets **don't intersect** the scrutinee's
//!   domain are dead and get pruned from the `Match` entirely.
//!
//! # Scope
//!
//! * Loops / calls / inner blocks clear the context of slots they
//!   mutate (safe over-approximation).
//! * Arm join after a `Match` clears only the slots the arms
//!   actually mutated; domains of untouched slots survive.
//! * Iteration to fixpoint, bounded to 8 rounds.

use std::collections::HashMap;

use either::Either;

use crate::exit_domains::FnExitDomains;
use crate::ir::{FnRef, HirBlock, HirOp, SlotId};

/// Bitmask over the 16 possible u4 values. Bit `i` set iff value
/// `i` is possible at this program point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Domain(pub u16);

impl Domain {
    /// All 16 u4 values possible — the "we know nothing" state.
    pub const ALL: Domain = Domain(0xFFFF);
    /// No value possible — empty domain, indicates dead code.
    pub const EMPTY: Domain = Domain(0);

    /// `{v}` for the given value (masked to u4).
    pub fn singleton(v: u8) -> Self {
        Self(1u16 << (v & 0x0F))
    }

    /// Union of the listed values.
    pub fn from_values(values: &[u8]) -> Self {
        let mut m: u16 = 0;
        for &v in values {
            m |= 1u16 << (v & 0x0F);
        }
        Self(m)
    }

    pub fn contains(self, v: u8) -> bool {
        self.0 & (1u16 << (v & 0x0F)) != 0
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    /// If the domain has exactly one value, return it.
    pub fn as_singleton(self) -> Option<u8> {
        if self.0.count_ones() == 1 {
            Some(self.0.trailing_zeros() as u8)
        } else {
            None
        }
    }
    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    /// True when `self ⊆ other` — every possible value of `self`
    /// also appears in `other`.
    pub fn is_subset_of(self, other: Self) -> bool {
        (self.0 & other.0) == self.0
    }
    /// Domain after `Inc`: every bit shifts up by one, bit 15
    /// wraps to bit 0.
    pub fn inc(self) -> Self {
        let shifted = (self.0 << 1) & 0xFFFE;
        let wrap = (self.0 & 0x8000) >> 15;
        Self(shifted | wrap)
    }
    /// Domain after `Dec`: bits shift down, bit 0 wraps to bit 15.
    pub fn dec(self) -> Self {
        let shifted = (self.0 >> 1) & 0x7FFF;
        let wrap = (self.0 & 0x0001) << 15;
        Self(shifted | wrap)
    }
    /// Domain after `MapValue` with `table`: for every value `v` the
    /// input could be, the output could be `table[v]`. Forms the
    /// straight bit-set union.
    pub fn map(self, table: &[u8; 16]) -> Self {
        let mut out: u16 = 0;
        for v in 0u8..=15 {
            if self.0 & (1u16 << v) != 0 {
                out |= 1u16 << (table[v as usize] & 0x0F);
            }
        }
        Self(out)
    }
}

/// Per-slot domain context, threaded flow-sensitively.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Ctx {
    /// Missing key = `Domain::ALL`. Storing ALL explicitly is wasted
    /// space, so we keep the map sparse and treat absence as ALL.
    known: HashMap<SlotId, Domain>,
}

impl Ctx {
    fn get(&self, slot: SlotId) -> Domain {
        self.known.get(&slot).copied().unwrap_or(Domain::ALL)
    }
    fn put(&mut self, slot: SlotId, dom: Domain) {
        if dom == Domain::ALL {
            self.known.remove(&slot);
        } else {
            self.known.insert(slot, dom);
        }
    }
    fn forget(&mut self, slot: SlotId) {
        self.known.remove(&slot);
    }
    fn narrow(&mut self, slot: SlotId, dom: Domain) {
        let cur = self.get(slot);
        self.put(slot, cur.intersect(dom));
    }
}

/// Run the specialization pass to a fixpoint with no outer
/// knowledge — equivalent to `specialize_to_fixpoint_with_domains`
/// with an empty seed.
pub fn specialize_to_fixpoint(block: HirBlock) -> HirBlock {
    specialize_to_fixpoint_full(block, &[], None)
}

/// Run the pass to a fixpoint, seeding the initial context with
/// per-parameter domains. Used by `spec_monomorph` to fold the
/// freshly-minted specialized variants *before* the inliner splices
/// them in, so the inlined HIR post-inline is strictly smaller.
pub fn specialize_to_fixpoint_with_domains(
    block: HirBlock,
    arg_domains: &[Domain],
) -> HirBlock {
    specialize_to_fixpoint_full(block, arg_domains, None)
}

/// Full-featured entry point. Like `specialize_to_fixpoint_with_domains`
/// but also threads a map of per-function exit-domain summaries so
/// Call ops can propagate the callee's post-call Domain into the
/// caller's ctx instead of forgetting mutated slots.
pub fn specialize_to_fixpoint_full(
    mut block: HirBlock,
    arg_domains: &[Domain],
    summaries: Option<&HashMap<typer::FnSig, FnExitDomains>>,
) -> HirBlock {
    for _ in 0..8 {
        let before = block.clone();
        let mut ctx = Ctx::default();
        for (i, d) in arg_domains.iter().enumerate() {
            ctx.put(SlotId(i as u32), *d);
        }
        let (new_block, _final_ctx) = specialize_block(block, &ctx, summaries);
        block = new_block;
        if block == before {
            break;
        }
    }
    block
}

fn specialize_block(
    block: HirBlock,
    inbound: &Ctx,
    summaries: Option<&HashMap<typer::FnSig, FnExitDomains>>,
) -> (HirBlock, Ctx) {
    let mut ctx = inbound.clone();
    let mut out: Vec<HirOp> = Vec::with_capacity(block.ops.len());
    for op in block.ops {
        specialize_op(op, &mut ctx, &mut out, summaries);
    }
    (
        HirBlock {
            ops: out,
            result_slot: block.result_slot,
        },
        ctx,
    )
}

fn specialize_op(
    op: HirOp,
    ctx: &mut Ctx,
    out: &mut Vec<HirOp>,
    summaries: Option<&HashMap<typer::FnSig, FnExitDomains>>,
) {
    match op {
        HirOp::Set(slot, v) => {
            ctx.put(slot, Domain::singleton(v));
            out.push(HirOp::Set(slot, v));
        }
        HirOp::Copy(dst, src) => {
            // Identity copy: `Copy(x, x)` is a no-op. Drop it.
            if dst == src {
                return;
            }
            let src_dom = ctx.get(src);
            if let Some(v) = src_dom.as_singleton() {
                // Source is a known constant — collapse to Set.
                ctx.put(dst, src_dom);
                out.push(HirOp::Set(dst, v));
            } else {
                ctx.put(dst, src_dom);
                out.push(HirOp::Copy(dst, src));
            }
        }
        HirOp::MapValue(src, dst, table) => {
            let dom = ctx.get(src).map(&table);
            ctx.put(dst, dom);
            out.push(HirOp::MapValue(src, dst, table));
        }
        HirOp::Match(scrutinee, arms) => {
            let scrut_dom = ctx.get(scrutinee);

            // Fast path: the scrutinee domain is fully contained in
            // one arm's value list — that arm always fires. Splice
            // its body in place, inherit its final ctx.
            for (body, values) in &arms {
                let arm_dom = Domain::from_values(values);
                if scrut_dom.is_subset_of(arm_dom) {
                    let (specialized, arm_final_ctx) =
                        specialize_block(body.clone(), ctx, summaries);
                    out.extend(specialized.ops);
                    *ctx = arm_final_ctx;
                    return;
                }
            }

            // General case: prune dead arms and recurse into the
            // survivors with the scrutinee narrowed to that arm's
            // values. After the match, join domains across arms
            // that FALL THROUGH (don't break/continue/stop).
            let mut specialized_arms: Vec<(HirBlock, Vec<u8>)> = Vec::new();
            let mut falling_through_ctxs: Vec<Ctx> = Vec::new();
            let mut touched_by_any_arm: std::collections::HashSet<SlotId> =
                std::collections::HashSet::new();
            for (body, values) in arms {
                let arm_dom = Domain::from_values(&values);
                if scrut_dom.intersect(arm_dom).is_empty() {
                    continue; // dead arm
                }
                let mut arm_ctx = ctx.clone();
                arm_ctx.narrow(scrutinee, arm_dom);
                let (specialized_body, arm_final) =
                    specialize_block(body, &arm_ctx, summaries);
                for s in slots_mutated(&specialized_body) {
                    touched_by_any_arm.insert(s);
                }
                // Arms that break out (loop exit) or continue
                // don't contribute to the join — their final ctx
                // is for a path that never reaches the post-match
                // code.
                if !arm_exits_enclosing(&specialized_body) {
                    falling_through_ctxs.push(arm_final);
                }
                specialized_arms.push((specialized_body, values));
            }

            // Cross-arm join over arms that fall through. If no
            // arm falls through (all break/continue) then
            // post-match code is unreachable in theory; we still
            // keep the outer ctx (safe, nothing added).
            if !falling_through_ctxs.is_empty() {
                let mut touched_slots: std::collections::HashSet<SlotId> =
                    std::collections::HashSet::new();
                for c in &falling_through_ctxs {
                    for s in c.known.keys() {
                        touched_slots.insert(*s);
                    }
                }
                for s in ctx.known.keys().copied().collect::<Vec<_>>() {
                    touched_slots.insert(s);
                }
                for s in touched_slots {
                    let joined = falling_through_ctxs
                        .iter()
                        .map(|c| c.get(s))
                        .fold(Domain::EMPTY, |acc, d| acc.union(d));
                    ctx.put(s, joined);
                }
            } else {
                // At least forget slots we know were mutated —
                // we can't prove anything about their post-match
                // domain without a fall-through arm.
                for s in touched_by_any_arm {
                    ctx.forget(s);
                }
            }
            out.push(HirOp::Match(scrutinee, specialized_arms));
        }
        HirOp::Loop(body) => {
            // Only slots actually mutated inside the loop body
            // become unknown on each iteration. Read-only slots
            // keep the caller's domain — propagate it into the
            // body so folds that depend on outer constants still
            // fire inside the loop.
            let mutated = slots_mutated(&body);
            let mut loop_ctx = ctx.clone();
            for s in &mutated {
                loop_ctx.forget(*s);
            }
            let (specialized, _final) = specialize_block(body, &loop_ctx, summaries);
            out.push(HirOp::Loop(specialized));
            for s in mutated {
                ctx.forget(s);
            }
        }
        HirOp::Block(body) => {
            // Block bodies can `Skip` mid-way to exit early. We
            // can't tell without control-flow analysis whether
            // that happens, so: use the pre-block ctx with any
            // mutated slot forgotten — safe over-approximation.
            let (specialized, _final) = specialize_block(body, ctx, summaries);
            for s in slots_mutated(&specialized) {
                ctx.forget(s);
            }
            out.push(HirOp::Block(specialized));
        }
        HirOp::Break | HirOp::Continue | HirOp::Stop | HirOp::Skip => {
            out.push(op);
        }
        HirOp::ReadRegister(dst, r) => {
            // Fresh value from a register — could be any byte.
            ctx.forget(dst);
            out.push(HirOp::ReadRegister(dst, r));
        }
        HirOp::WriteRegister(r, src) => match src {
            Either::Right(slot) => {
                if let Some(v) = ctx.get(slot).as_singleton() {
                    out.push(HirOp::WriteRegister(r, Either::Left(v)));
                } else {
                    out.push(HirOp::WriteRegister(r, Either::Right(slot)));
                }
            }
            Either::Left(v) => {
                out.push(HirOp::WriteRegister(r, Either::Left(v)));
            }
        },
        HirOp::Call { target, args, ret } => {
            // When we have exit-domain summaries for the callee,
            // propagate guaranteed mutations to the caller's ctx:
            // mut arg cells pick up the callee's input_exit,
            // ret cells pick up the callee's output_exit. Without
            // summaries, fall back to the conservative "forget
            // ret + forget args that might be mut" behaviour.
            let target_sig = fn_ref_to_sig_local(&target);
            match summaries.and_then(|m| m.get(&target_sig)) {
                Some(summary) => {
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
                None => {
                    // No summary — the safe assumption is that any
                    // arg cell might be a mut param getting written.
                    for a in &args {
                        ctx.forget(*a);
                    }
                    for r in &ret {
                        ctx.forget(*r);
                    }
                }
            }
            out.push(HirOp::Call { target, args, ret });
        }
    }
}

fn fn_ref_to_sig_local(r: &FnRef) -> typer::FnSig {
    match &r.trait_name {
        None => typer::FnSig::new(r.type_name.clone(), r.method_name.clone()),
        Some(t) => typer::FnSig::new_trait(
            r.type_name.clone(),
            r.method_name.clone(),
            t.clone(),
        ),
    }
}

/// Does this arm always exit its enclosing control-flow (break,
/// continue, stop, or skip) regardless of which path it takes?
///
/// Conservative syntactic check: `true` iff any **top-level** op in
/// the block is `Break` / `Continue` / `Stop` / `Skip`, OR the last
/// op itself is a Match/Block whose every arm/body unconditionally
/// exits. Loops are NOT considered exiting (they may iterate 0
/// times and fall through).
///
/// Used by the cross-arm domain join: arms that exit don't
/// contribute to the post-match context because control never
/// reaches the post-match code via them.
fn arm_exits_enclosing(block: &HirBlock) -> bool {
    for op in &block.ops {
        match op {
            HirOp::Break | HirOp::Continue | HirOp::Stop | HirOp::Skip => return true,
            _ => {}
        }
    }
    // Also: if the final op is a Match whose every arm exits, or
    // a Block that exits, the whole arm exits. Pragmatic check:
    // only the immediate final op.
    if let Some(last) = block.ops.last() {
        match last {
            HirOp::Match(_, arms) if !arms.is_empty() => {
                return arms.iter().all(|(b, _)| arm_exits_enclosing(b));
            }
            HirOp::Block(b) => return arm_exits_enclosing(b),
            _ => {}
        }
    }
    false
}

/// Set of slots that some op in `block` writes to, recursively.
/// Used to decide what knowledge to drop when joining control
/// flow around a nested block (match arm, loop body, inner block).
fn slots_mutated(block: &HirBlock) -> std::collections::HashSet<SlotId> {
    let mut out = std::collections::HashSet::new();
    for op in &block.ops {
        collect_mutated(op, &mut out);
    }
    out
}

fn collect_mutated(op: &HirOp, out: &mut std::collections::HashSet<SlotId>) {
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
                for o in &b.ops {
                    collect_mutated(o, out);
                }
            }
        }
        HirOp::Loop(b) | HirOp::Block(b) => {
            for o in &b.ops {
                collect_mutated(o, out);
            }
        }
        HirOp::Call { ret, .. } => {
            for r in ret {
                out.insert(*r);
            }
        }
        _ => {}
    }
}

// ---- unit tests for the Domain helper -----------------------------------

#[cfg(test)]
mod domain_tests {
    use super::Domain;

    #[test]
    fn singleton_contains_only_itself() {
        let d = Domain::singleton(5);
        assert!(d.contains(5));
        assert!(!d.contains(0));
        assert!(!d.contains(15));
        assert_eq!(d.as_singleton(), Some(5));
    }

    #[test]
    fn from_values_is_union() {
        let d = Domain::from_values(&[1, 3, 5]);
        for v in [1, 3, 5] {
            assert!(d.contains(v), "missing {}", v);
        }
        for v in [0, 2, 4, 6] {
            assert!(!d.contains(v), "unexpectedly has {}", v);
        }
        assert_eq!(d.as_singleton(), None);
    }

    #[test]
    fn inc_wraps_at_16() {
        let d = Domain::singleton(15);
        let d2 = d.inc();
        assert_eq!(d2, Domain::singleton(0));
    }

    #[test]
    fn dec_wraps_at_zero() {
        let d = Domain::singleton(0);
        let d2 = d.dec();
        assert_eq!(d2, Domain::singleton(15));
    }

    #[test]
    fn intersect_and_subset() {
        let a = Domain::from_values(&[1, 2, 3]);
        let b = Domain::from_values(&[2, 3, 4]);
        assert_eq!(a.intersect(b), Domain::from_values(&[2, 3]));
        assert!(Domain::from_values(&[2]).is_subset_of(a));
        assert!(!Domain::from_values(&[1, 5]).is_subset_of(a));
    }

    #[test]
    fn all_and_empty() {
        assert!(!Domain::ALL.is_empty());
        assert!(Domain::EMPTY.is_empty());
        assert!(Domain::singleton(3).is_subset_of(Domain::ALL));
        assert_eq!(Domain::ALL.intersect(Domain::singleton(7)), Domain::singleton(7));
    }
}
