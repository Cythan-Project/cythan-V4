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

use crate::ir::{HirBlock, HirOp, SlotId};

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
    specialize_to_fixpoint_with_domains(block, &[])
}

/// Run the pass to a fixpoint, seeding the initial context with
/// per-parameter domains. Used by `spec_monomorph` to fold the
/// freshly-minted specialized variants *before* the inliner splices
/// them in, so the inlined HIR post-inline is strictly smaller.
pub fn specialize_to_fixpoint_with_domains(
    mut block: HirBlock,
    arg_domains: &[Domain],
) -> HirBlock {
    for _ in 0..8 {
        let before = block.clone();
        let mut ctx = Ctx::default();
        for (i, d) in arg_domains.iter().enumerate() {
            ctx.put(SlotId(i as u32), *d);
        }
        block = specialize_block(block, &ctx);
        if block == before {
            break;
        }
    }
    block
}

fn specialize_block(block: HirBlock, inbound: &Ctx) -> HirBlock {
    let mut ctx = inbound.clone();
    let mut out: Vec<HirOp> = Vec::with_capacity(block.ops.len());
    for op in block.ops {
        specialize_op(op, &mut ctx, &mut out);
    }
    HirBlock {
        ops: out,
        result_slot: block.result_slot,
    }
}

fn specialize_op(op: HirOp, ctx: &mut Ctx, out: &mut Vec<HirOp>) {
    match op {
        HirOp::Set(slot, v) => {
            ctx.put(slot, Domain::singleton(v));
            out.push(HirOp::Set(slot, v));
        }
        HirOp::Copy(dst, src) => {
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
        HirOp::Inc(slot) => {
            let dom = ctx.get(slot).inc();
            ctx.put(slot, dom);
            out.push(HirOp::Inc(slot));
        }
        HirOp::Dec(slot) => {
            let dom = ctx.get(slot).dec();
            ctx.put(slot, dom);
            out.push(HirOp::Dec(slot));
        }
        HirOp::Match(scrutinee, arms) => {
            let scrut_dom = ctx.get(scrutinee);

            // Fast path: the scrutinee domain is fully contained in
            // one arm's value list — that arm always fires. Splice
            // its body in place.
            for (body, values) in &arms {
                let arm_dom = Domain::from_values(values);
                if scrut_dom.is_subset_of(arm_dom) {
                    let specialized = specialize_block(body.clone(), ctx);
                    let mutated = slots_mutated(&specialized);
                    out.extend(specialized.ops);
                    for s in mutated {
                        ctx.forget(s);
                    }
                    return;
                }
            }

            // General case: prune dead arms and recurse into the
            // survivors with the scrutinee narrowed to that arm's
            // values. Drop arms whose values don't overlap the
            // current scrutinee domain.
            let mut specialized_arms: Vec<(HirBlock, Vec<u8>)> = Vec::new();
            let mut mutated_across_arms = std::collections::HashSet::new();
            for (body, values) in arms {
                let arm_dom = Domain::from_values(&values);
                if scrut_dom.intersect(arm_dom).is_empty() {
                    continue; // dead arm
                }
                let mut arm_ctx = ctx.clone();
                arm_ctx.narrow(scrutinee, arm_dom);
                let specialized_body = specialize_block(body, &arm_ctx);
                for s in slots_mutated(&specialized_body) {
                    mutated_across_arms.insert(s);
                }
                specialized_arms.push((specialized_body, values));
            }
            // Arms may have mutated the scrutinee; post-match we
            // can't keep the old domain. The mutated set covers
            // this for free (the scrutinee is just another slot).
            for s in mutated_across_arms {
                ctx.forget(s);
            }
            out.push(HirOp::Match(scrutinee, specialized_arms));
        }
        HirOp::Loop(body) => {
            let mutated = slots_mutated(&body);
            let specialized = specialize_block(body, &Ctx::default());
            out.push(HirOp::Loop(specialized));
            for s in mutated {
                ctx.forget(s);
            }
        }
        HirOp::Block(body) => {
            let specialized = specialize_block(body, ctx);
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
            for r in &ret {
                ctx.forget(*r);
            }
            out.push(HirOp::Call { target, args, ret });
        }
    }
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
        HirOp::Set(s, _) | HirOp::Copy(s, _) | HirOp::Inc(s) | HirOp::Dec(s) => {
            out.insert(*s);
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
