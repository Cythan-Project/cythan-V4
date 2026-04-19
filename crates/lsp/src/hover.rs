//! `textDocument/hover` — type / signature tooltip.
//!
//! Expanded coverage relative to the older minimal version:
//! * `recv.method(…)` → `fn Type::method(params): ret (from Trait)`.
//! * `recv.field`     → `field: FieldType` and the owning
//!                      struct name.
//! * `Type::method`   → same as the method form above.
//! * `Type::new(…)`   → the static call's signature.
//! * Local variable / param / `self` → `name: Type`.
//! * Type / trait name → the kind (`struct Foo`, `trait Bar`,
//!                      `enum E { variants… }`).
//!
//! All results are a single line of `MarkedString::String` —
//! simple, readable, copy-pasteable into docs.

use lsp_types::{Hover, HoverContents, MarkedString, Position, Url};

use crate::ast::{
    enclosing_method_env, find_at_cursor, infer_expr_type, local_env_at,
    method_call_at, method_return_type, CursorNode,
};
use crate::state::State;
use crate::text::{identifier_at, position_to_char_offset, receiver_before, IdContext};

pub(crate) fn resolve_hover(state: &State, uri: &Url, pos: Position) -> Option<Hover> {
    let text = state.docs.get(uri)?;
    let reg = state.registry.as_ref()?;
    let (word, ctx) = identifier_at(text, pos)?;
    let pos_off = position_to_char_offset(text, pos)?;
    let items = state.asts.get(uri);

    let mut detail: Option<String> = None;

    // Method call under cursor.
    if matches!(ctx, IdContext::AfterDot) {
        if let Some(items) = items {
            if let Some((env, recv)) = method_call_at(items, pos_off, &word) {
                if let Some(ty) = infer_expr_type(recv, &env, reg) {
                    detail = method_signature(reg, &ty, &word);
                }
            }
        }
    }

    // Field access under cursor (AfterDot but no MethodCall match).
    if detail.is_none() && matches!(ctx, IdContext::AfterDot) {
        if let Some(items) = items {
            if let Some(CursorNode::FieldAccess(recv, _)) = find_at_cursor(items, pos_off) {
                if let Some((env, _)) = enclosing_method_env(items, pos_off) {
                    if let Some(ty) = infer_expr_type(recv, &env, reg) {
                        detail = field_hover(reg, &ty, &word);
                    }
                }
            }
        }
    }

    // `Type::method` — same flow as AfterDot.
    if detail.is_none() && matches!(ctx, IdContext::AfterColons) {
        if let Some(receiver) = receiver_before(text, pos) {
            detail = method_signature(reg, &receiver, &word);
            // Could be a static call returning some type (e.g.
            // `Array::new()` returns `Array`) — include the
            // return type in the tooltip.
            if detail.is_none() {
                if let Some(ret) = method_return_type(reg, &receiver, &word) {
                    detail = Some(format!("fn {}::{}  → {}", receiver, word, ret));
                }
            }
        }
    }

    // Type / trait fallback.
    if detail.is_none() {
        detail = type_or_trait_hover(reg, &word);
    }

    // Local variable / self fallback.
    if detail.is_none() {
        if let Some(items) = items {
            if let Some(env) = local_env_at(items, pos_off) {
                if word == "self" {
                    if let Some(t) = &env.self_ty {
                        detail = Some(format!("self: {}", t));
                    }
                } else if let Some(ty) = env.bindings.get(&word) {
                    detail = Some(format!("{}: {}", word, ty));
                }
            }
        }
    }

    detail.map(|d| Hover {
        contents: HoverContents::Scalar(MarkedString::String(d)),
        range: None,
    })
}

fn method_signature(
    reg: &typer::TypeRegistry,
    ty_name: &str,
    method: &str,
) -> Option<String> {
    let info = reg.get_type(ty_name)?;
    let m = info.methods.iter().find(|m| m.function.sig.name.0 == method)?;
    let trait_tag = m
        .from_trait
        .map(|tid| reg.trait_canonical_keys.get(tid.0 as usize).cloned())
        .flatten()
        .map(|t| format!("  (from `{}`)", t))
        .unwrap_or_default();
    let ret = m
        .function
        .sig
        .return_type
        .as_ref()
        .map(|t| format!(": {}", t.0.name.0))
        .unwrap_or_default();
    Some(format!(
        "fn {}::{}({}){}{}",
        ty_name,
        method,
        crate::ast::method_param_string(&m.function.sig.params),
        ret,
        trait_tag
    ))
}

fn field_hover(
    reg: &typer::TypeRegistry,
    ty_name: &str,
    field_name: &str,
) -> Option<String> {
    let info = reg.get_type(ty_name)?;
    let typer::TypeKind::Struct(kind) = &info.kind else {
        return None;
    };
    let field_ty = match kind {
        typer::StructKind::Concrete(layout) => layout
            .fields
            .iter()
            .find(|f| f.name == field_name)
            .map(|f| f.ast_type.name.0.clone()),
        typer::StructKind::Templated { fields } => fields
            .iter()
            .find(|(n, _)| n == field_name)
            .map(|(_, t)| t.name.0.clone()),
    }?;
    Some(format!("{}::{}: {}", ty_name, field_name, field_ty))
}

fn type_or_trait_hover(reg: &typer::TypeRegistry, name: &str) -> Option<String> {
    if let Some(info) = reg.get_type(name) {
        let kind = match &info.kind {
            typer::TypeKind::Primitive { size } => format!("primitive ({} cell{})", size, if *size == 1 { "" } else { "s" }),
            typer::TypeKind::Struct(_) => "struct".into(),
            typer::TypeKind::Enum(_) => "enum".into(),
        };
        let tparams = if info.templates.is_empty() {
            String::new()
        } else {
            format!("<{}>", info.templates.join(", "))
        };
        return Some(format!("{} {}{}", kind, name, tparams));
    }
    for (trait_name, _info) in reg.iter_traits() {
        if trait_name == name {
            return Some(format!("trait {}", name));
        }
    }
    None
}
