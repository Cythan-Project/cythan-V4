//! `textDocument/definition` — Ctrl-click / Go to Definition.
//!
//! Resolves in order:
//! 1. `Type::method` — exact `(Type, method)` match.
//! 2. `recv.method` — infer `recv`'s type from the AST, then
//!    exact `(Type, method)` match. Falls back to by-name lookup
//!    only when there's a single candidate.
//! 3. `recv.field` — infer `recv`'s type, then look up the
//!    field span.
//! 4. Bare identifier — type, trait, then any method name.

use lsp_types::{Location, Position, Url};

use crate::ast::{
    enclosing_self_type, find_at_cursor, infer_expr_type, method_call_at, CursorNode,
};
use crate::state::State;
use crate::text::{identifier_at, position_to_char_offset, receiver_before, IdContext};

pub(crate) fn resolve_definition(
    state: &State,
    uri: &Url,
    pos: Position,
) -> Option<Location> {
    let text = state.docs.get(uri)?;
    let (word, prev_marker) = identifier_at(text, pos)?;
    if word.is_empty() {
        return None;
    }

    if let IdContext::AfterColons = prev_marker {
        if let Some(receiver_token) = receiver_before(text, pos) {
            if let Some(loc) = state
                .symbols
                .methods
                .get(&(receiver_token.clone(), word.clone()))
            {
                return Some(loc.clone());
            }
            if receiver_token == "Self" {
                let pos_off = position_to_char_offset(text, pos)?;
                if let Some(items) = state.asts.get(uri) {
                    if let Some(self_ty) = enclosing_self_type(items, pos_off) {
                        if let Some(loc) = state
                            .symbols
                            .methods
                            .get(&(self_ty, word.clone()))
                        {
                            return Some(loc.clone());
                        }
                    }
                }
            }
        }
    }

    if let IdContext::AfterDot = prev_marker {
        let pos_off = position_to_char_offset(text, pos)?;
        if let (Some(items), Some(reg)) =
            (state.asts.get(uri), state.registry.as_ref())
        {
            // Method? Look for a MethodCall whose name token
            // covers the cursor, infer the receiver type, match.
            if let Some((env, recv)) = method_call_at(items, pos_off, &word) {
                if let Some(ty) = infer_expr_type(recv, &env, reg) {
                    if let Some(loc) = state
                        .symbols
                        .methods
                        .get(&(ty.clone(), word.clone()))
                    {
                        return Some(loc.clone());
                    }
                }
            }
            // Field? Look for a FieldAccess at the cursor.
            if let Some(CursorNode::FieldAccess(recv, _)) = find_at_cursor(items, pos_off) {
                if let Some((env, _f)) = crate::ast::enclosing_method_env(items, pos_off) {
                    if let Some(ty) = infer_expr_type(recv, &env, reg) {
                        if let Some(loc) = state
                            .symbols
                            .fields
                            .get(&(ty, word.clone()))
                        {
                            return Some(loc.clone());
                        }
                    }
                }
            }
        }
        // By-name fallback for methods only (fields aren't
        // unique enough to be a reasonable fallback).
        if let Some(matches) = state.symbols.methods_by_name.get(&word) {
            if matches.len() == 1 {
                return Some(matches[0].clone());
            }
            return None;
        }
    }

    if let Some(loc) = state.symbols.types.get(&word) {
        return Some(loc.clone());
    }
    if let Some(loc) = state.symbols.traits.get(&word) {
        return Some(loc.clone());
    }
    None
}
