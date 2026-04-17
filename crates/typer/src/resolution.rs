//! Shared utilities for trait / bound resolution.
//!
//! Type equality that ignores `Span`s, and the type-arg unifier that
//! backs blanket-impl bound satisfaction. Kept in one place so future
//! resolution paths (higher-order bounds, trait-template-arg
//! inference, etc.) can reuse the same primitives instead of
//! re-deriving them.

use std::collections::HashMap;

use new_parser::ast;

// ---------------------------------------------------------------------------
// Structural equality (span-ignoring)
//
// `ast::Type` derives `PartialEq` — but spans are part of the struct, so two
// types that represent the same thing but were constructed at different
// sites compare unequal. Resolution code needs "is the same type"
// semantics, regardless of where each copy came from.
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
// Unification
//
// Used during blanket-impl attachment to bind free generic names against
// the template args of candidate impls. The `free` slice lists names that
// may be bound during unification; anything else is treated as a concrete
// head that must match exactly.
// ---------------------------------------------------------------------------

/// Unify two argument lists position-wise, extending `assignment` with
/// any new bindings discovered for free generics. Returns `None` on any
/// contradiction (differing arity, mismatched concrete head, or
/// inconsistent rebinding of a free generic).
pub fn unify_args(
    bound_args: &[ast::TypeOrValue],
    impl_args: &[ast::TypeOrValue],
    free: &[String],
    mut assignment: HashMap<String, ast::TypeOrValue>,
) -> Option<HashMap<String, ast::TypeOrValue>> {
    if bound_args.len() != impl_args.len() {
        return None;
    }
    for (b, i) in bound_args.iter().zip(impl_args.iter()) {
        assignment = unify_tv(b, i, free, assignment)?;
    }
    Some(assignment)
}

/// Unify one `TypeOrValue` pair. See `unify_args`.
pub fn unify_tv(
    bound: &ast::TypeOrValue,
    impl_arg: &ast::TypeOrValue,
    free: &[String],
    mut assignment: HashMap<String, ast::TypeOrValue>,
) -> Option<HashMap<String, ast::TypeOrValue>> {
    match (bound, impl_arg) {
        (ast::TypeOrValue::Value(a), ast::TypeOrValue::Value(b)) => {
            if a == b {
                Some(assignment)
            } else {
                None
            }
        }
        (ast::TypeOrValue::Type(bt), ast::TypeOrValue::Type(it)) => {
            // Bare-name free generic on the bound side → bind/check.
            if bt.templates.is_empty() && free.iter().any(|f| f == &bt.name.0) {
                let proposed = ast::TypeOrValue::Type(it.clone());
                match assignment.get(&bt.name.0) {
                    Some(existing) if !tv_structural_eq(existing, &proposed) => return None,
                    Some(_) => {}
                    None => {
                        assignment.insert(bt.name.0.clone(), proposed);
                    }
                }
                return Some(assignment);
            }
            // Concrete: heads and args must match.
            if bt.name.0 != it.name.0 {
                return None;
            }
            let b_args: Vec<ast::TypeOrValue> =
                bt.templates.iter().map(|(t, _)| t.clone()).collect();
            let i_args: Vec<ast::TypeOrValue> =
                it.templates.iter().map(|(t, _)| t.clone()).collect();
            unify_args(&b_args, &i_args, free, assignment)
        }
        _ => None,
    }
}
