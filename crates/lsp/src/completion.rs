//! `textDocument/completion` — context-aware completion.
//!
//! Completion contexts, in the order they're tried:
//!
//! * `recv.<cursor>` — every field + method of `recv`'s type
//!   (inherent + trait methods).
//! * `Type::<cursor>` — every method on `Type` plus its enum
//!   variants.
//! * `use <cursor>` — every type + trait in the workspace.
//! * **Bare identifier** — types, traits, enclosing method's
//!   params + locals, `self`, plus a handful of language
//!   keywords. Covers:
//!     - Declaration type positions: `<cursor> name = …;`
//!     - Return-type positions: `fn foo(): <cursor>`
//!     - Parameter-type positions: `fn foo(<cursor> x)`
//!     - Expression positions: any place a local / type is valid.
//!
//! When the open file doesn't parse (the user just typed a
//! trigger — that in itself breaks the parse), we fall back
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
    let Some(pos_off) = position_to_char_offset(text, pos) else {
        return vec![];
    };
    let chars: Vec<char> = text.chars().collect();

    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut id_start = pos_off.min(chars.len());
    while id_start > 0 && is_id(chars[id_start - 1]) {
        id_start -= 1;
    }

    // `.` and `::` triggers require a live registry to walk
    // field / method lookups. Without a registry, fall through
    // to the scope/import paths which only need the symbol index.
    if id_start > 0 && chars[id_start - 1] == '.' {
        if let Some(reg) = state.registry.as_ref() {
            let recv_chain = extract_dot_chain(&chars, id_start - 1);
            return complete_after_dot(state, uri, pos_off, reg, recv_chain);
        }
    }
    if id_start >= 2 && chars[id_start - 1] == ':' && chars[id_start - 2] == ':' {
        if let Some(reg) = state.registry.as_ref() {
            let mut j = id_start - 2;
            while j > 0 && is_id(chars[j - 1]) {
                j -= 1;
            }
            let recv: String = chars[j..id_start - 2].iter().collect();
            return complete_after_colons(reg, &recv);
        }
    }
    // `use <cursor>` — workspace imports.
    if after_keyword(&chars, id_start, "use") {
        return complete_imports(state);
    }
    // Bare identifier — scope-level completion. Uses only the
    // symbol index + AST cache, so it still works when the
    // registry failed to build (e.g. the file has a broken type
    // reference the user is still typing).
    complete_scope(state, uri, pos_off)
}

/// True iff the cursor sits immediately after the keyword
/// `kw` (optionally separated by whitespace, and optionally
/// followed by a partial identifier the user is in the middle
/// of typing).
fn after_keyword(chars: &[char], id_start: usize, kw: &str) -> bool {
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = id_start;
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    let kw_chars: Vec<char> = kw.chars().collect();
    if i < kw_chars.len() {
        return false;
    }
    if chars[i - kw_chars.len()..i] != kw_chars[..] {
        return false;
    }
    // Make sure it's the whole word — the char before `use`
    // must not be an identifier character.
    let before = i - kw_chars.len();
    before == 0 || !is_id(chars[before - 1])
}

/// After `use <cursor>` — every type and trait in the workspace
/// becomes a candidate. Each suggestion completes as `Name;` so
/// the statement terminates automatically.
fn complete_imports(state: &State) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = Vec::new();
    for name in state.symbols.types.keys() {
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::STRUCT),
            detail: Some("type".into()),
            insert_text: Some(format!("{};", name)),
            ..Default::default()
        });
    }
    for name in state.symbols.traits.keys() {
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::INTERFACE),
            detail: Some("trait".into()),
            insert_text: Some(format!("{};", name)),
            ..Default::default()
        });
    }
    items
}

/// Bare-identifier completion — every item a user could
/// plausibly be starting to type, aside from field/method
/// receivers (handled earlier). Produces:
///
/// * Every type name (struct / enum / primitive).
/// * Every trait name.
/// * Locals / params / `self` from the enclosing method's scope.
/// * A small set of language keywords that are valid at most
///   expression positions.
fn complete_scope(
    state: &State,
    uri: &Url,
    pos_off: usize,
) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = Vec::new();

    // Types from the symbol index (populated even when the
    // typer's registry build fails for unrelated files). When a
    // fresh registry happens to be available we refine the
    // `kind` + `detail` fields per type; otherwise everything
    // gets the generic STRUCT kind.
    let reg = state.registry.as_ref();
    for (name, _loc) in &state.symbols.types {
        let kind = reg
            .and_then(|r| r.get_type(name))
            .and_then(|info| match &info.kind {
                typer::TypeKind::Primitive { .. } => Some(CompletionItemKind::UNIT),
                typer::TypeKind::Struct(_) => Some(CompletionItemKind::STRUCT),
                typer::TypeKind::Enum(_) => Some(CompletionItemKind::ENUM),
            });
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(kind.unwrap_or(CompletionItemKind::STRUCT)),
            detail: reg.map(|r| type_detail(r, name)),
            ..Default::default()
        });
    }
    // Traits.
    for name in state.symbols.traits.keys() {
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::INTERFACE),
            detail: Some(format!("trait {}", name)),
            ..Default::default()
        });
    }
    // Locals / params / self from the enclosing method.
    let env = state
        .asts
        .get(uri)
        .and_then(|items| local_env_at(items, pos_off))
        .or_else(|| {
            let text = state.docs.get(uri)?;
            let items = parse_lenient(text, pos_off)?;
            local_env_at(&items, pos_off)
        });
    if let Some(env) = env {
        if let Some(t) = &env.self_ty {
            items.push(CompletionItem {
                label: "self".into(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some(format!("self: {}", t)),
                ..Default::default()
            });
        }
        for (name, ty) in &env.bindings {
            if name == "self" {
                continue; // already added explicitly
            }
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some(format!("{}: {}", name, ty)),
                ..Default::default()
            });
        }
    }
    // Keywords. Kept small — full grammar completion belongs in
    // a snippet extension, not here.
    for kw in [
        "if", "else", "match", "loop", "break", "continue", "return", "fn", "let",
        "mut", "struct", "enum", "trait", "impl", "extension", "use", "const",
        "true", "false",
    ] {
        items.push(CompletionItem {
            label: kw.into(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        });
    }
    items
}

fn type_detail(reg: &typer::TypeRegistry, name: &str) -> String {
    let Some(info) = reg.get_type(name) else {
        return format!("type {}", name);
    };
    let kind = match &info.kind {
        typer::TypeKind::Primitive { size } => {
            format!("primitive ({} cell{})", size, if *size == 1 { "" } else { "s" })
        }
        typer::TypeKind::Struct(_) => "struct".into(),
        typer::TypeKind::Enum(_) => "enum".into(),
    };
    if info.templates.is_empty() {
        format!("{} {}", kind, name)
    } else {
        format!("{} {}<{}>", kind, name, info.templates.join(", "))
    }
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
