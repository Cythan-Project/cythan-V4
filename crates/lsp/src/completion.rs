//! `textDocument/completion` — `.` / `::` trigger-based field
//! and method suggestions.
//!
//! After `recv.` the list includes every field + method of
//! `recv`'s type, with methods coming from both `extension`
//! blocks AND `impl Trait for Type` (the typer records both in
//! the same `TypeInfo.methods`). After `Type::` the list
//! includes methods plus enum variants.
//!
//! When the open file doesn't parse (the user just typed the
//! trigger `.` — that in itself breaks the parse), we fall back
//! to a lenient re-parse with the partial chunk blanked out so
//! the local env is still usable.

use std::collections::HashMap;

use lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat, Position, Url};

use crate::ast::{
    field_type, local_env_at, method_param_string, parse_lenient, LocalEnv,
};
use crate::state::State;
use crate::text::{extract_dot_chain, position_to_char_offset};

pub(crate) fn resolve_completion(
    state: &State,
    uri: &Url,
    pos: Position,
) -> Vec<CompletionItem> {
    let Some(text) = state.docs.get(uri) else {
        return vec![];
    };
    let Some(reg) = state.registry.as_ref() else {
        return vec![];
    };
    let Some(pos_off) = position_to_char_offset(text, pos) else {
        return vec![];
    };
    let chars: Vec<char> = text.chars().collect();

    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut id_start = pos_off.min(chars.len());
    while id_start > 0 && is_id(chars[id_start - 1]) {
        id_start -= 1;
    }
    if id_start == 0 {
        return vec![];
    }

    let prev = chars[id_start - 1];
    if prev == '.' {
        let recv_chain = extract_dot_chain(&chars, id_start - 1);
        return complete_after_dot(state, uri, pos_off, reg, recv_chain);
    }
    if id_start >= 2 && chars[id_start - 1] == ':' && chars[id_start - 2] == ':' {
        let mut j = id_start - 2;
        while j > 0 && is_id(chars[j - 1]) {
            j -= 1;
        }
        let recv: String = chars[j..id_start - 2].iter().collect();
        return complete_after_colons(reg, &recv);
    }
    vec![]
}

fn complete_after_dot(
    state: &State,
    uri: &Url,
    pos_off: usize,
    reg: &typer::TypeRegistry,
    chain: Vec<String>,
) -> Vec<CompletionItem> {
    if chain.is_empty() {
        return vec![];
    }
    let env = state
        .asts
        .get(uri)
        .and_then(|items| local_env_at(items, pos_off))
        .or_else(|| {
            let text = state.docs.get(uri)?;
            let items = parse_lenient(text, pos_off)?;
            local_env_at(&items, pos_off)
        });
    let env = env.unwrap_or(LocalEnv {
        self_ty: None,
        bindings: HashMap::new(),
    });

    let mut ty: Option<String> = None;
    for (i, segment) in chain.iter().enumerate() {
        ty = if i == 0 {
            if segment == "self" {
                env.self_ty.clone()
            } else {
                env.bindings.get(segment).cloned()
            }
        } else {
            ty.as_ref().and_then(|t| field_type(reg, t, segment))
        };
        if ty.is_none() {
            return vec![];
        }
    }
    let Some(ty) = ty else {
        return vec![];
    };
    items_for_type(reg, &ty)
}

fn complete_after_colons(
    reg: &typer::TypeRegistry,
    ty_name: &str,
) -> Vec<CompletionItem> {
    let mut items = items_for_type(reg, ty_name);
    if let Some(info) = reg.get_type(ty_name) {
        if let typer::TypeKind::Enum(kind) = &info.kind {
            let variants: Vec<&str> = match kind {
                typer::EnumKind::Concrete(layout) => {
                    layout.variants.iter().map(|v| v.name.as_str()).collect()
                }
                typer::EnumKind::Templated { variants } => {
                    variants.iter().map(|v| v.name.as_str()).collect()
                }
            };
            for v in variants {
                items.push(CompletionItem {
                    label: v.to_string(),
                    kind: Some(CompletionItemKind::ENUM_MEMBER),
                    detail: Some(format!("variant of {}", ty_name)),
                    ..Default::default()
                });
            }
        }
    }
    items
}

/// Every field + method of `ty_name`, formatted as
/// `CompletionItem`s. Methods include those attached via
/// `extension` AND `impl Trait for ty_name`.
pub(crate) fn items_for_type(
    reg: &typer::TypeRegistry,
    ty_name: &str,
) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = Vec::new();
    let Some(info) = reg.get_type(ty_name) else {
        return items;
    };
    if let typer::TypeKind::Struct(kind) = &info.kind {
        match kind {
            typer::StructKind::Concrete(layout) => {
                for f in &layout.fields {
                    items.push(CompletionItem {
                        label: f.name.clone(),
                        kind: Some(CompletionItemKind::FIELD),
                        detail: Some(format!("{}: {}", f.name, f.ast_type.name.0)),
                        ..Default::default()
                    });
                }
            }
            typer::StructKind::Templated { fields } => {
                for (name, ty) in fields {
                    items.push(CompletionItem {
                        label: name.clone(),
                        kind: Some(CompletionItemKind::FIELD),
                        detail: Some(format!("{}: {}", name, ty.name.0)),
                        ..Default::default()
                    });
                }
            }
        }
    }
    let mut seen_methods: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for m in &info.methods {
        let name = m.function.sig.name.0.clone();
        if !seen_methods.insert(name.clone()) {
            continue;
        }
        let trait_tag = m
            .from_trait
            .map(|tid| reg.trait_canonical_keys.get(tid.0 as usize).cloned())
            .flatten()
            .map(|t| format!(" (from `{}`)", t))
            .unwrap_or_default();
        let ret = m
            .function
            .sig
            .return_type
            .as_ref()
            .map(|t| format!(": {}", t.0.name.0))
            .unwrap_or_default();
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(format!(
                "fn {}({}){}{}",
                name,
                method_param_string(&m.function.sig.params),
                ret,
                trait_tag
            )),
            insert_text: Some(format!("{}($0)", name)),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        });
    }
    items
}
