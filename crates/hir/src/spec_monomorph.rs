//! Pre-inline specialization monomorphizer.
//!
//! Classical specialization: create one `HirFunction` variant per
//! `(base_sig, argument_domains)` pair encountered at call sites.
//! Runs **before** inlining, so the number of distinct HIR bodies
//! grows — then the inliner splices the tighter bodies in place,
//! and the final MIR is smaller than it would be without.
//!
//! # Example
//!
//! `Morpion::winner(self, Cell tocheck)` is called three times from
//! `Morpion::play` with `tocheck` statically known — once with
//! `Cell::O` (domain `{1}`), once with `Cell::X` (domain `{2}`),
//! and one site where the value comes from a `mut` variable
//! (domain ALL). The pass emits:
//!
//! * `Morpion::winner$1` — body specialized with `tocheck == Cell::O`
//! * `Morpion::winner$2` — body specialized with `tocheck == Cell::X`
//! * the original `Morpion::winner` — used by the unspecialized
//!   third call site.
//!
//! Inside each variant, any match on `tocheck` collapses to the
//! matching arm. Each subsequent `Call` inside those variants is
//! itself re-walked with the tighter context, cascading.
//!
//! # Mangling
//!
//! Method name becomes `{orig}${dom0},{dom1},…` where each `dom`
//! is the u16 bitmask in hex.
//!
//! * `{1}` (Cell::O) → `2`  (bit 1 set)
//! * `{1..=15}`      → `fffe`
//! * `{0}`           → `1`
//!
//! ALL (0xFFFF) is omitted: if every argument is ALL there's
//! nothing to specialize and the call is left pointing at the
//! base function.
//!
//! # Cache + fixpoint
//!
//! A `SpecKey → FnSig` map de-duplicates identical `(base, domains)`
//! combos so the same specialization isn't re-emitted from every
//! caller. The pass processes a work-list: every newly-created
//! variant goes back on the queue so its own calls can cascade.
//! No recursion cycles are possible because the underlying call
//! graph is already validated as a DAG (`build_call_graph`).

use std::collections::{HashMap, HashSet};

use either::Either;

use crate::ir::{FnRef, HirBlock, HirFunction, HirOp, SlotId};
use crate::specialize::{specialize_to_fixpoint_with_domains, Domain};

/// Domain per input cell. `arg_domains[i]` is the domain of the
/// callee's input slot `i` (which corresponds to `Call.args[i]`
/// in the caller).
pub type ArgDomains = Vec<Domain>;

/// Cache key: original `FnSig` + its arg-domain fingerprint.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct SpecKey {
    base: typer::FnSig,
    domains: ArgDomains,
}

/// Output of the specialization pass.
pub struct SpecResult {
    /// The original map plus any specialized variants we created.
    pub functions: HashMap<typer::FnSig, HirFunction>,
    /// Count of specialized variants added to `functions`.
    pub specialized_count: usize,
}

/// Run the pass. Walks each callable function starting from
/// `entry`, computing flow-sensitive domain info and rewriting
/// `Call` ops to target specialized variants where at least one
/// argument is narrower than ALL.
pub fn run(
    mut fns: HashMap<typer::FnSig, HirFunction>,
    entry: &typer::FnSig,
) -> SpecResult {
    // Work-list items come in two shapes:
    //   * `(base_sig, None)` — walk the original function body
    //     in place with all-ALL initial context. Processed once
    //     per function.
    //   * `(variant_sig, Some(arg_domains))` — walk a specialized
    //     variant with its parameter domains as initial context.
    //     Processed once per unique `(base_sig, arg_domains)`.
    let mut base_queue: Vec<typer::FnSig> = fns.keys().cloned().collect();
    // Deterministic order — FnSig isn't Ord, so sort by a derived key.
    base_queue.sort_by(|a, b| {
        (a.type_name.as_str(), a.method_name.as_str(), a.trait_name.as_deref())
            .cmp(&(b.type_name.as_str(), b.method_name.as_str(), b.trait_name.as_deref()))
    });
    let mut variant_queue: Vec<(typer::FnSig, typer::FnSig, ArgDomains)> = Vec::new();

    let mut processed_base: HashSet<typer::FnSig> = HashSet::new();
    let mut processed_variants: HashSet<SpecKey> = HashSet::new();
    let mut cache: HashMap<SpecKey, typer::FnSig> = HashMap::new();
    let mut specialized_count = 0usize;

    let _ = entry; // reserved for future "only walk reachable" pruning

    loop {
        // Phase 1: walk every base function once with ALL context.
        while let Some(sig) = base_queue.pop() {
            if !processed_base.insert(sig.clone()) {
                continue;
            }
            let Some(func) = fns.get(&sig).cloned() else { continue };
            let input_count = func.sig.input_count as usize;
            let ctx_domains = vec![Domain::ALL; input_count];
            let (new_body, new_calls) = walk_function(
                func.body.clone(),
                &ctx_domains,
                &mut cache,
                &fns,
            );
            fns.insert(sig, HirFunction { body: new_body, ..func });
            // Queue the new variants this walk minted.
            for (variant_sig, variant_domains) in new_calls {
                if let Some((base_sig, _)) = cache
                    .iter()
                    .find(|(_, v)| **v == variant_sig)
                    .map(|(k, v)| (k.base.clone(), v.clone()))
                {
                    variant_queue.push((base_sig, variant_sig, variant_domains));
                }
            }
        }

        // Phase 2: materialize queued variants, walk them.
        let queued = std::mem::take(&mut variant_queue);
        if queued.is_empty() {
            break;
        }
        for (base_sig, variant_sig, variant_domains) in queued {
            let key = SpecKey {
                base: base_sig.clone(),
                domains: variant_domains.clone(),
            };
            if !processed_variants.insert(key) {
                continue;
            }
            // Materialize the variant if we haven't already. Fold
            // it with the specialization context right away so the
            // variant's body shrinks *before* the inliner splices
            // it in — that's what makes the post-inline program
            // strictly smaller than the unspecialized path.
            if !fns.contains_key(&variant_sig) {
                let Some(base_fn) = fns.get(&base_sig).cloned() else {
                    continue;
                };
                specialized_count += 1;
                let folded_body = specialize_to_fixpoint_with_domains(
                    base_fn.body.clone(),
                    &variant_domains,
                );
                let variant = HirFunction {
                    body: folded_body,
                    type_name: variant_sig.type_name.clone(),
                    method_name: variant_sig.method_name.clone(),
                    ..base_fn
                };
                fns.insert(variant_sig.clone(), variant);
            }
            // Walk with initial context from the specialization domains.
            let Some(variant_fn) = fns.get(&variant_sig).cloned() else {
                continue;
            };
            let (new_body, new_calls) = walk_function(
                variant_fn.body.clone(),
                &variant_domains,
                &mut cache,
                &fns,
            );
            fns.insert(
                variant_sig.clone(),
                HirFunction { body: new_body, ..variant_fn },
            );
            for (nested_variant_sig, nested_domains) in new_calls {
                if let Some((b, _)) = cache
                    .iter()
                    .find(|(_, v)| **v == nested_variant_sig)
                    .map(|(k, v)| (k.base.clone(), v.clone()))
                {
                    variant_queue.push((b, nested_variant_sig, nested_domains));
                }
            }
        }
    }

    SpecResult { functions: fns, specialized_count }
}

/// Walk a function body with the given per-parameter domain context
/// and return the rewritten body plus the list of new variants its
/// Call ops requested.
fn walk_function(
    body: HirBlock,
    arg_domains: &[Domain],
    cache: &mut HashMap<SpecKey, typer::FnSig>,
    fns: &HashMap<typer::FnSig, HirFunction>,
) -> (HirBlock, Vec<(typer::FnSig, ArgDomains)>) {
    let mut ctx = Ctx::default();
    for (i, d) in arg_domains.iter().enumerate() {
        ctx.put(SlotId(i as u32), *d);
    }
    let mut new_calls = Vec::new();
    let new_body = walk_block(body, &ctx, cache, fns, &mut new_calls);
    (new_body, new_calls)
}

fn is_all(domains: &[Domain]) -> bool {
    domains.iter().all(|d| *d == Domain::ALL)
}

/// Per-slot domain context — a thinned copy of the one in
/// `hir::specialize`, kept private so the two passes can diverge
/// without coupling.
#[derive(Debug, Clone, Default)]
struct Ctx {
    known: HashMap<SlotId, Domain>,
}
impl Ctx {
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

/// Walk a block threading `Ctx`, rewriting `Call` targets whose
/// caller-side arg domains are narrower than ALL. `new_calls`
/// collects the specializations this walk requested so the outer
/// driver can materialize + queue them.
fn walk_block(
    block: HirBlock,
    inbound: &Ctx,
    cache: &mut HashMap<SpecKey, typer::FnSig>,
    fns: &HashMap<typer::FnSig, HirFunction>,
    new_calls: &mut Vec<(typer::FnSig, ArgDomains)>,
) -> HirBlock {
    let mut ctx = inbound.clone();
    let mut out = Vec::with_capacity(block.ops.len());
    for op in block.ops {
        walk_op(op, &mut ctx, &mut out, cache, fns, new_calls);
    }
    HirBlock {
        ops: out,
        result_slot: block.result_slot,
    }
}

fn walk_op(
    op: HirOp,
    ctx: &mut Ctx,
    out: &mut Vec<HirOp>,
    cache: &mut HashMap<SpecKey, typer::FnSig>,
    fns: &HashMap<typer::FnSig, HirFunction>,
    new_calls: &mut Vec<(typer::FnSig, ArgDomains)>,
) {
    match op {
        HirOp::Set(s, v) => {
            ctx.put(s, Domain::singleton(v));
            out.push(HirOp::Set(s, v));
        }
        HirOp::Copy(dst, src) => {
            ctx.put(dst, ctx.get(src));
            out.push(HirOp::Copy(dst, src));
        }
        HirOp::Inc(s) => {
            let d = ctx.get(s).inc();
            ctx.put(s, d);
            out.push(HirOp::Inc(s));
        }
        HirOp::Dec(s) => {
            let d = ctx.get(s).dec();
            ctx.put(s, d);
            out.push(HirOp::Dec(s));
        }
        HirOp::Match(scrutinee, arms) => {
            let mut new_arms = Vec::with_capacity(arms.len());
            let mut mutated = HashSet::new();
            for (body, values) in arms {
                let mut arm_ctx = ctx.clone();
                let arm_dom = Domain::from_values(&values);
                arm_ctx.put(scrutinee, ctx.get(scrutinee).intersect(arm_dom));
                let new_body = walk_block(body, &arm_ctx, cache, fns, new_calls);
                for s in slots_mutated(&new_body) {
                    mutated.insert(s);
                }
                new_arms.push((new_body, values));
            }
            for s in mutated {
                ctx.forget(s);
            }
            out.push(HirOp::Match(scrutinee, new_arms));
        }
        HirOp::Loop(body) => {
            let mutated = slots_mutated(&body);
            let new_body = walk_block(body, &Ctx::default(), cache, fns, new_calls);
            for s in mutated {
                ctx.forget(s);
            }
            out.push(HirOp::Loop(new_body));
        }
        HirOp::Block(body) => {
            let new_body = walk_block(body, ctx, cache, fns, new_calls);
            for s in slots_mutated(&new_body) {
                ctx.forget(s);
            }
            out.push(HirOp::Block(new_body));
        }
        HirOp::Break | HirOp::Continue | HirOp::Stop | HirOp::Skip => {
            out.push(op);
        }
        HirOp::ReadRegister(dst, r) => {
            ctx.forget(dst);
            out.push(HirOp::ReadRegister(dst, r));
        }
        HirOp::WriteRegister(r, src) => {
            out.push(HirOp::WriteRegister(r, src));
        }
        HirOp::Call { target, args, ret } => {
            // Derive per-input-cell domains from the caller's ctx.
            let arg_domains: Vec<Domain> =
                args.iter().map(|s| ctx.get(*s)).collect();

            for r in &ret {
                ctx.forget(*r);
            }

            // If every arg is ALL — nothing gained by specializing.
            if is_all(&arg_domains) {
                out.push(HirOp::Call { target, args, ret });
                return;
            }

            // Look up (or mint) the specialized FnSig for this key.
            let base_sig = fn_ref_to_sig(&target);
            // Only specialize callables actually present in the
            // map. Templated or synthesized ones are skipped for
            // safety — their bodies aren't available here.
            if !fns.contains_key(&base_sig) {
                out.push(HirOp::Call { target, args, ret });
                return;
            }
            let key = SpecKey { base: base_sig.clone(), domains: arg_domains.clone() };
            let specialized_sig = match cache.get(&key) {
                Some(s) => s.clone(),
                None => {
                    let mangled_method = mangle_method_name(&target.method_name, &arg_domains);
                    let new_sig = match &target.trait_name {
                        None => typer::FnSig::new(target.type_name.clone(), mangled_method),
                        Some(t) => typer::FnSig::new_trait(
                            target.type_name.clone(),
                            mangled_method,
                            t.clone(),
                        ),
                    };
                    cache.insert(key.clone(), new_sig.clone());
                    new_calls.push((new_sig.clone(), arg_domains));
                    new_sig
                }
            };

            let new_target = FnRef {
                type_name: target.type_name,
                method_name: specialized_sig.method_name.clone(),
                template_args: target.template_args,
                trait_name: target.trait_name,
            };
            out.push(HirOp::Call {
                target: new_target,
                args,
                ret,
            });
        }
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

/// Compact, deterministic encoding of per-cell domains. Each cell
/// is the domain's `u16` mask in hex; cells are comma-separated.
/// Trailing ALLs are trimmed so variants with fewer constraints
/// don't grow unnecessarily long names.
fn mangle_method_name(base: &str, domains: &[Domain]) -> String {
    // Drop trailing ALL entries — they contribute nothing.
    let mut end = domains.len();
    while end > 0 && domains[end - 1] == Domain::ALL {
        end -= 1;
    }
    let chunks: Vec<String> = domains[..end]
        .iter()
        .map(|d| {
            if *d == Domain::ALL {
                "*".to_string()
            } else {
                format!("{:x}", d.0)
            }
        })
        .collect();
    format!("{}${}", base, chunks.join(","))
}

// ---- helpers (duplicated with `specialize.rs` — kept local to
// keep the two passes decoupled; fine for a <300-line module) ----

fn slots_mutated(block: &HirBlock) -> HashSet<SlotId> {
    let mut out = HashSet::new();
    for op in &block.ops {
        collect_mutated(op, &mut out);
    }
    out
}

fn collect_mutated(op: &HirOp, out: &mut HashSet<SlotId>) {
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

// Silence the unused `Either` import when no call sites need it.
// (Compile cleanliness only — no functional effect.)
#[allow(dead_code)]
fn _keep_either_used(_: Either<u8, SlotId>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mangle_encodes_domains_and_trims_trailing_all() {
        let s = mangle_method_name("f", &[Domain::singleton(0), Domain::ALL, Domain::ALL]);
        assert_eq!(s, "f$1");
        let s = mangle_method_name(
            "winner",
            &[Domain::ALL, Domain::singleton(1)],
        );
        assert_eq!(s, "winner$*,2");
        let s = mangle_method_name("g", &[Domain::from_values(&[1, 2, 3])]);
        assert_eq!(s, "g$e"); // bits 1|2|3 = 0b1110 = 0xe
    }

    #[test]
    fn mangle_empty_when_all_domains_are_all() {
        // All ALL → empty chunk list.
        let s = mangle_method_name("f", &[Domain::ALL, Domain::ALL]);
        assert_eq!(s, "f$");
    }
}
