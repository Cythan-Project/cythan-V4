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

use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Url};

use crate::ast::{
    enclosing_method_env, find_at_cursor, infer_expr_type, local_env_at,
    method_call_at, method_return_type, CursorNode,
};
use crate::state::State;
use crate::text::{identifier_at, position_to_char_offset, receiver_before, IdContext};

/// Render a hover payload as a markdown code fence tagged with
/// the `cythan` language id. VS Code (and any other
/// LSP-compliant client) then runs the contents through the
/// extension's TextMate grammar, giving us the same colored
/// tooltips rust-analyzer surfaces. `extra` (optional) is
/// appended below the fence as regular markdown — useful for
/// "(from `Trait`)" annotations that aren't themselves Cythan
/// code.
fn code_fence(signature: &str, extra: Option<&str>) -> HoverContents {
    let mut value = format!("```cythan\n{}\n```", signature);
    if let Some(e) = extra {
        if !e.is_empty() {
            value.push('\n');
            value.push_str(e);
        }
    }
    HoverContents::Markup(MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    })
}

/// A hover payload split into two pieces so the signature can
/// go into a code-fenced (syntax-highlighted) block and the
/// documentation into plain markdown below.
struct HoverParts {
    signature: String,
    extra: Option<String>,
}

pub(crate) fn resolve_hover(state: &State, uri: &Url, pos: Position) -> Option<Hover> {
    let text = state.docs.get(uri)?;
    let reg = state.registry.as_ref()?;
    let (word, ctx) = identifier_at(text, pos)?;
    let pos_off = position_to_char_offset(text, pos)?;
    let items = state.asts.get(uri);

    let mut parts: Option<HoverParts> = None;

    if matches!(ctx, IdContext::AfterDot) {
        if let Some(items) = items {
            if let Some((env, recv)) = method_call_at(items, pos_off, &word) {
                if let Some(ty) = infer_expr_type(recv, &env, reg) {
                    parts = method_hover(reg, &ty, &word);
                }
            }
        }
    }

    if parts.is_none() && matches!(ctx, IdContext::AfterDot) {
        if let Some(items) = items {
            if let Some(CursorNode::FieldAccess(recv, _)) = find_at_cursor(items, pos_off)
            {
                if let Some((env, _)) = enclosing_method_env(items, pos_off) {
                    if let Some(ty) = infer_expr_type(recv, &env, reg) {
                        parts = field_hover(reg, &ty, &word);
                    }
                }
            }
        }
    }

    if parts.is_none() && matches!(ctx, IdContext::AfterColons) {
        if let Some(receiver) = receiver_before(text, pos) {
            parts = method_hover(reg, &receiver, &word);
            if parts.is_none() {
                if let Some(ret) = method_return_type(reg, &receiver, &word) {
                    parts = Some(HoverParts {
                        signature: format!("fn {}::{} -> {}", receiver, word, ret),
                        extra: None,
                    });
                }
            }
        }
    }

    if parts.is_none() {
        parts = type_or_trait_hover(reg, &word);
    }

    if parts.is_none() {
        if let Some(items) = items {
            if let Some(env) = local_env_at(items, pos_off) {
                if word == "self" {
                    if let Some(t) = &env.self_ty {
                        parts = Some(HoverParts {
                            signature: format!("self: {}", t),
                            extra: None,
                        });
                    }
                } else if let Some(ty) = env.bindings.get(&word) {
                    parts = Some(HoverParts {
                        signature: format!("{}: {}", word, ty),
                        extra: None,
                    });
                }
            }
        }
    }

    parts.map(|p| Hover {
        contents: code_fence(&p.signature, p.extra.as_deref()),
        range: None,
    })
}

fn method_hover(
    reg: &typer::TypeRegistry,
    ty_name: &str,
    method: &str,
) -> Option<HoverParts> {
    let info = reg.get_type(ty_name)?;
    let m = info.methods.iter().find(|m| m.function.sig.name.0 == method)?;
    let ret = m
        .function
        .sig
        .return_type
        .as_ref()
        .map(|t| format!(": {}", t.0.name.0))
        .unwrap_or_default();
    let signature = format!(
        "fn {}::{}({}){}",
        ty_name,
        method,
        crate::ast::method_param_string(&m.function.sig.params),
        ret,
    );
    let extra = m
        .from_trait
        .and_then(|tid| reg.trait_canonical_keys.get(tid.0 as usize).cloned())
        .map(|t| format!("*from trait `{}`*", t));
    Some(HoverParts { signature, extra })
}

fn field_hover(
    reg: &typer::TypeRegistry,
    ty_name: &str,
    field_name: &str,
) -> Option<HoverParts> {
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
    Some(HoverParts {
        signature: format!("{}::{}: {}", ty_name, field_name, field_ty),
        extra: None,
    })
}

fn type_or_trait_hover(reg: &typer::TypeRegistry, name: &str) -> Option<HoverParts> {
    if let Some(info) = reg.get_type(name) {
        let kind = match &info.kind {
            typer::TypeKind::Primitive { size } => {
                format!("// primitive ({} cell{})", size, if *size == 1 { "" } else { "s" })
            }
            typer::TypeKind::Struct(_) => "struct".into(),
            typer::TypeKind::Enum(_) => "enum".into(),
        };
        let tparams = if info.templates.is_empty() {
            String::new()
        } else {
            format!("<{}>", info.templates.join(", "))
        };
        let signature = if kind.starts_with("//") {
            // Primitive — the `kind` string is the comment line;
            // keep the type name on its own.
            format!("{}\ntype {}", kind, name)
        } else {
            format!("{} {}{}", kind, name, tparams)
        };
        return Some(HoverParts { signature, extra: None });
    }
    for (trait_name, _info) in reg.iter_traits() {
        if trait_name == name {
            return Some(HoverParts {
                signature: format!("trait {}", name),
                extra: None,
            });
        }
    }
    None
}
