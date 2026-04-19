//! Document outline + workspace symbol search.
//!
//! `textDocument/documentSymbol` walks the open file's AST and
//! produces a nested `DocumentSymbol` tree (types / traits /
//! impl blocks, with their fields / variants / methods as
//! children). `workspace/symbol` does a substring match across
//! the whole symbol index for the Ctrl+T "go to symbol"
//! command.

use lsp_types::{DocumentSymbol, Location, SymbolInformation, SymbolKind, Url};

use crate::ast::method_param_string;
use crate::state::State;
use crate::text::byte_range_to_lsp;

pub(crate) fn document_symbols(state: &State, uri: &Url) -> Vec<DocumentSymbol> {
    let Some(text) = state.docs.get(uri) else {
        return vec![];
    };
    let Some(items) = state.asts.get(uri) else {
        return vec![];
    };
    use new_parser::ast::*;
    let mut out: Vec<DocumentSymbol> = Vec::new();
    for item in items {
        let sym = match &item.0 {
            Item::Struct(d) => Some(struct_symbol(text, d, &item.1)),
            Item::Enum(d) => Some(enum_symbol(text, d, &item.1)),
            Item::Trait(d) => Some(trait_symbol(text, d, &item.1)),
            Item::Extension(d) => Some(extension_symbol(text, d, &item.1)),
            Item::Impl(d) => Some(impl_symbol(text, d, &item.1)),
            Item::Const(d) => Some(const_symbol(text, d, &item.1)),
            Item::Use(_) => None,
        };
        if let Some(s) = sym {
            out.push(s);
        }
    }
    out
}

pub(crate) fn workspace_symbols(state: &State, query: &str) -> Vec<SymbolInformation> {
    let q = query.to_lowercase();
    let mut out: Vec<SymbolInformation> = Vec::new();
    let push = |out: &mut Vec<SymbolInformation>,
                name: &str,
                kind: SymbolKind,
                loc: &Location,
                container: Option<String>| {
        #[allow(deprecated)]
        out.push(SymbolInformation {
            name: name.to_string(),
            kind,
            tags: None,
            deprecated: None,
            location: loc.clone(),
            container_name: container,
        });
    };
    for (name, loc) in &state.symbols.types {
        if name.to_lowercase().contains(&q) {
            push(&mut out, name, SymbolKind::STRUCT, loc, None);
        }
    }
    for (name, loc) in &state.symbols.traits {
        if name.to_lowercase().contains(&q) {
            push(&mut out, name, SymbolKind::INTERFACE, loc, None);
        }
    }
    for ((ty, m), loc) in &state.symbols.methods {
        if m.to_lowercase().contains(&q) {
            push(&mut out, m, SymbolKind::METHOD, loc, Some(ty.clone()));
        }
    }
    for ((ty, f), loc) in &state.symbols.fields {
        if f.to_lowercase().contains(&q) {
            push(&mut out, f, SymbolKind::FIELD, loc, Some(ty.clone()));
        }
    }
    out.truncate(200);
    out
}

fn struct_symbol(
    text: &str,
    d: &new_parser::ast::StructDef,
    item_sp: &new_parser::Span,
) -> DocumentSymbol {
    let fields = d
        .fields
        .iter()
        .map(|f| {
            #[allow(deprecated)]
            DocumentSymbol {
                name: f.name.0.clone(),
                detail: Some(f.ty.0.name.0.clone()),
                kind: SymbolKind::FIELD,
                tags: None,
                deprecated: None,
                range: byte_range_to_lsp(text, &f.name.1),
                selection_range: byte_range_to_lsp(text, &f.name.1),
                children: None,
            }
        })
        .collect();
    #[allow(deprecated)]
    DocumentSymbol {
        name: d.name.0.clone(),
        detail: Some("struct".into()),
        kind: SymbolKind::STRUCT,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(text, item_sp),
        selection_range: byte_range_to_lsp(text, &d.name.1),
        children: Some(fields),
    }
}

fn enum_symbol(
    text: &str,
    d: &new_parser::ast::EnumDef,
    item_sp: &new_parser::Span,
) -> DocumentSymbol {
    let variants = d
        .variants
        .iter()
        .map(|v| {
            #[allow(deprecated)]
            DocumentSymbol {
                name: v.name.0.clone(),
                detail: None,
                kind: SymbolKind::ENUM_MEMBER,
                tags: None,
                deprecated: None,
                range: byte_range_to_lsp(text, &v.name.1),
                selection_range: byte_range_to_lsp(text, &v.name.1),
                children: None,
            }
        })
        .collect();
    #[allow(deprecated)]
    DocumentSymbol {
        name: d.name.0.clone(),
        detail: Some("enum".into()),
        kind: SymbolKind::ENUM,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(text, item_sp),
        selection_range: byte_range_to_lsp(text, &d.name.1),
        children: Some(variants),
    }
}

fn trait_symbol(
    text: &str,
    d: &new_parser::ast::TraitDef,
    item_sp: &new_parser::Span,
) -> DocumentSymbol {
    let methods = d
        .methods
        .iter()
        .map(|m| {
            #[allow(deprecated)]
            DocumentSymbol {
                name: m.0.name.0.clone(),
                detail: Some(format!("fn {}", m.0.name.0)),
                kind: SymbolKind::METHOD,
                tags: None,
                deprecated: None,
                range: byte_range_to_lsp(text, &m.1),
                selection_range: byte_range_to_lsp(text, &m.0.name.1),
                children: None,
            }
        })
        .collect();
    #[allow(deprecated)]
    DocumentSymbol {
        name: d.name.0.clone(),
        detail: Some("trait".into()),
        kind: SymbolKind::INTERFACE,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(text, item_sp),
        selection_range: byte_range_to_lsp(text, &d.name.1),
        children: Some(methods),
    }
}

fn extension_symbol(
    text: &str,
    d: &new_parser::ast::ExtensionDef,
    item_sp: &new_parser::Span,
) -> DocumentSymbol {
    let methods = d
        .methods
        .iter()
        .map(|m| function_symbol(text, &m.0, &m.1))
        .collect();
    let target_name = &d.target.0.name.0;
    #[allow(deprecated)]
    DocumentSymbol {
        name: format!("extension {}", target_name),
        detail: Some("extension".into()),
        kind: SymbolKind::NAMESPACE,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(text, item_sp),
        selection_range: byte_range_to_lsp(text, &d.target.0.name.1),
        children: Some(methods),
    }
}

fn impl_symbol(
    text: &str,
    d: &new_parser::ast::ImplDef,
    item_sp: &new_parser::Span,
) -> DocumentSymbol {
    let methods = d
        .methods
        .iter()
        .map(|m| function_symbol(text, &m.0, &m.1))
        .collect();
    #[allow(deprecated)]
    DocumentSymbol {
        name: format!(
            "impl {} for {}",
            d.trait_ty.0.name.0, d.target.0.name.0
        ),
        detail: Some("impl".into()),
        kind: SymbolKind::NAMESPACE,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(text, item_sp),
        selection_range: byte_range_to_lsp(text, &d.trait_ty.0.name.1),
        children: Some(methods),
    }
}

fn const_symbol(
    text: &str,
    d: &new_parser::ast::ConstDef,
    item_sp: &new_parser::Span,
) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: d.name.0.clone(),
        detail: Some(d.ty.0.name.0.clone()),
        kind: SymbolKind::CONSTANT,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(text, item_sp),
        selection_range: byte_range_to_lsp(text, &d.name.1),
        children: None,
    }
}

fn function_symbol(
    text: &str,
    f: &new_parser::ast::Function,
    fn_sp: &new_parser::Span,
) -> DocumentSymbol {
    let detail = format!(
        "fn({}){}",
        method_param_string(&f.sig.params),
        f.sig
            .return_type
            .as_ref()
            .map(|t| format!(": {}", t.0.name.0))
            .unwrap_or_default()
    );
    #[allow(deprecated)]
    DocumentSymbol {
        name: f.sig.name.0.clone(),
        detail: Some(detail),
        kind: SymbolKind::METHOD,
        tags: None,
        deprecated: None,
        range: byte_range_to_lsp(text, fn_sp),
        selection_range: byte_range_to_lsp(text, &f.sig.name.1),
        children: None,
    }
}
