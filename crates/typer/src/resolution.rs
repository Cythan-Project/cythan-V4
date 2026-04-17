//! Shared utilities for trait / bound resolution.
//!
//! Two kinds of operations live here:
//!
//!   1. Structural equality that ignores `Span`s — needed because
//!      `ast::Type` derives `PartialEq` but spans are part of it.
//!
//!   2. Type-arg unification — binds free generic names in a bound
//!      against the template args of a candidate impl. Supports two
//!      binding shapes:
//!
//!         * `Concrete(tv)` — the free generic is bound to an actual
//!           type/value known at attach time (e.g. `T = U4`).
//!         * `CandidateArg(i)` — the free generic is bound to the
//!           candidate's `i`-th template arg, resolved *at call site*
//!           by the HIR generator. Used for blanket impls whose bound
//!           references another generic (e.g. `impl<T, I: Iter<T>>
//!           Countable for I` on `ArrayIter<X, N, F>` — `T` ties to
//!           ArrayIter's 0th arg, not to any compile-time concrete).

use std::collections::HashMap;

use new_parser::ast;

// ---------------------------------------------------------------------------
// Structural equality (span-ignoring)
// ---------------------------------------------------------------------------

/// True when two `TypeOrValue`s represent the same value, ignoring
/// `Span` positions.
pub fn tv_structural_eq(a: &ast::TypeOrValue, b: &ast::TypeOrValue) -> bool {
    match (a, b) {
        (ast::TypeOrValue::Value(x), ast::TypeOrValue::Value(y)) => x == y,
        (ast::TypeOrValue::Type(x), ast::TypeOrValue::Type(y)) => ty_structural_eq(x, y),
        _ => false,
    }
}

/// True when two `Type`s represent the same type, ignoring `Span`s.
/// Recurses through template args and the optional `qself` prefix.
pub fn ty_structural_eq(a: &ast::Type, b: &ast::Type) -> bool {
    if a.name.0 != b.name.0 {
        return false;
    }
    if a.templates.len() != b.templates.len() {
        return false;
    }
    for ((av, _), (bv, _)) in a.templates.iter().zip(b.templates.iter()) {
        if !tv_structural_eq(av, bv) {
            return false;
        }
    }
    match (&a.qself, &b.qself) {
        (None, None) => true,
        (Some(ax), Some(bx)) => {
            ty_structural_eq(&ax.self_ty.0, &bx.self_ty.0)
                && ty_structural_eq(&ax.trait_ty.0, &bx.trait_ty.0)
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Resolution result
// ---------------------------------------------------------------------------

/// A single binding produced by bound unification.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedArg {
    /// The bound unified the free generic with a fully concrete type
    /// or value known at attach time.
    Concrete(ast::TypeOrValue),
    /// The bound unified the free generic with one of the candidate
    /// type's own template-arg positions. HIR gen resolves this at
    /// each call site by reading the receiver's template args.
    CandidateArg(usize),
}

/// Extract a concrete `TypeOrValue` from a `ResolvedArg`, falling back
/// to a placeholder when the binding is positional. Callers that can't
/// defer to HIR gen (e.g. direct type-size queries during attachment)
/// use this to pick a reasonable stand-in.
pub fn resolved_arg_concrete(r: &ResolvedArg) -> Option<ast::TypeOrValue> {
    match r {
        ResolvedArg::Concrete(tv) => Some(tv.clone()),
        ResolvedArg::CandidateArg(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Unification
// ---------------------------------------------------------------------------

/// Unify two argument lists position-wise, extending `assignment` with
/// any new bindings discovered for free generics. `candidate_params`
/// names the template params of the candidate type — when the impl
/// side references one of these, the bound's free generic binds to
/// `CandidateArg(position)` rather than trying to unify a bare name
/// against another bare name.
pub fn unify_args(
    bound_args: &[ast::TypeOrValue],
    impl_args: &[ast::TypeOrValue],
    free: &[String],
    candidate_params: &[String],
    mut assignment: HashMap<String, ResolvedArg>,
) -> Option<HashMap<String, ResolvedArg>> {
    if bound_args.len() != impl_args.len() {
        return None;
    }
    for (b, i) in bound_args.iter().zip(impl_args.iter()) {
        assignment = unify_tv(b, i, free, candidate_params, assignment)?;
    }
    Some(assignment)
}

/// Unify one `TypeOrValue` pair. See `unify_args`.
pub fn unify_tv(
    bound: &ast::TypeOrValue,
    impl_arg: &ast::TypeOrValue,
    free: &[String],
    candidate_params: &[String],
    mut assignment: HashMap<String, ResolvedArg>,
) -> Option<HashMap<String, ResolvedArg>> {
    match (bound, impl_arg) {
        (ast::TypeOrValue::Value(a), ast::TypeOrValue::Value(b)) => {
            if a == b {
                Some(assignment)
            } else {
                None
            }
        }
        (ast::TypeOrValue::Type(bt), ast::TypeOrValue::Type(it)) => {
            // Bare-name free generic on the bound side → bind.
            if bt.templates.is_empty() && free.iter().any(|f| f == &bt.name.0) {
                // Decide whether the impl side is a bare reference to
                // a candidate template param (→ positional binding)
                // or a fully concrete type (→ concrete binding).
                let proposed = if it.templates.is_empty() && it.qself.is_none() {
                    match candidate_params.iter().position(|p| p == &it.name.0) {
                        Some(idx) => ResolvedArg::CandidateArg(idx),
                        None => ResolvedArg::Concrete(ast::TypeOrValue::Type(it.clone())),
                    }
                } else {
                    ResolvedArg::Concrete(ast::TypeOrValue::Type(it.clone()))
                };
                match assignment.get(&bt.name.0) {
                    Some(existing) if !resolved_args_equal(existing, &proposed) => return None,
                    Some(_) => {}
                    None => {
                        assignment.insert(bt.name.0.clone(), proposed);
                    }
                }
                return Some(assignment);
            }
            // Concrete: heads and args must match. (If the impl-side
            // head is a candidate_param, we still require head equality
            // — a bound `Iter<U4>` against an impl `Iter<T_a>` only
            // satisfies when T_a is the concrete U4, not when they're
            // both placeholders.)
            if bt.name.0 != it.name.0 {
                return None;
            }
            let b_args: Vec<ast::TypeOrValue> =
                bt.templates.iter().map(|(t, _)| t.clone()).collect();
            let i_args: Vec<ast::TypeOrValue> =
                it.templates.iter().map(|(t, _)| t.clone()).collect();
            unify_args(&b_args, &i_args, free, candidate_params, assignment)
        }
        _ => None,
    }
}

/// Structural equality for `ResolvedArg`s, ignoring spans inside any
/// concrete `ast::Type`.
pub fn resolved_args_equal(a: &ResolvedArg, b: &ResolvedArg) -> bool {
    match (a, b) {
        (ResolvedArg::Concrete(x), ResolvedArg::Concrete(y)) => tv_structural_eq(x, y),
        (ResolvedArg::CandidateArg(i), ResolvedArg::CandidateArg(j)) => i == j,
        _ => false,
    }
}
