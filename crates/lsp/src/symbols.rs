//! Cross-file symbol index — what every LSP request looks up
//! before doing any AST walking.
//!
//! Populated after each `diagnose` call by re-parsing the file
//! set, building a `typer::TypeRegistry`, and walking it for
//! declaration spans. Holds:
//!   * type / trait names → declaration location,
//!   * `(type, method)` → location (with a by-name fallback for
//!     when we can't pin down the receiver type),
//!   * `(type, field)` → field-declaration location,
//!
//! Each request handler can treat the index as a read-only
//! snapshot — it's rebuilt on every document change.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_types::{Location, Url};

use crate::text::byte_range_to_lsp;

/// See the module doc.
#[derive(Default, Debug)]
pub(crate) struct SymbolIndex {
    pub(crate) types: HashMap<String, Location>,
    pub(crate) traits: HashMap<String, Location>,
    pub(crate) methods: HashMap<(String, String), Location>,
    pub(crate) methods_by_name: HashMap<String, Vec<Location>>,
    /// `(type_name, field_name)` → declaration location of the
    /// field's name token. Covers struct fields only (enums
    /// surface their variants through the type's name via
    /// completion / outline, which is handled separately).
    pub(crate) fields: HashMap<(String, String), Location>,
}

/// Parse every file, build a `TypeRegistry`, and walk it to
/// produce a fresh `SymbolIndex`. Best-effort — any parse /
/// build failure yields a partial (or empty) index without
/// bringing the LSP down.
pub(crate) fn build_symbol_index(
    files: &[(String, String)],
    name_to_path: &HashMap<String, PathBuf>,
) -> (SymbolIndex, Option<typer::TypeRegistry>) {
    let mut idx = SymbolIndex::default();

    type Parsed = (String, Vec<new_parser::ast::Spanned<new_parser::ast::Item>>);
    let mut parsed: Vec<Parsed> = Vec::new();
    for (name, src) in files {
        if let Ok(items) = new_parser::parse(src) {
            parsed.push((name.to_string(), items));
        }
    }
    // Keep a parsed-files view for field-span resolution (the
    // registry's FieldLayout stores only the ast::Type but not
    // the field-name span). Paired up via file + field-name.
    let as_refs: Vec<(&str, &[_])> = parsed
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_slice()))
        .collect();

    let reg = match typer::TypeRegistry::from_files(&as_refs) {
        Ok(r) => r,
        Err(_) => return (idx, None),
    };

    let location_for = |file: &Option<String>,
                        span: &Option<new_parser::Span>|
     -> Option<Location> {
        let file_name = file.as_ref()?;
        let span = span.as_ref()?;
        let abs = name_to_path.get(file_name)?;
        let uri = Url::from_file_path(abs).ok()?;
        let src = files.iter().find(|(n, _)| n == file_name).map(|(_, s)| s)?;
        Some(Location {
            uri,
            range: byte_range_to_lsp(src, span),
        })
    };

    // Helper: locate a struct-field declaration span by walking
    // the parsed AST for `type_name`'s declaration. The
    // registry's `TypeInfo.decl_file` tells us where to look.
    let find_field_span = |type_name: &str, field_name: &str| -> Option<Location> {
        for (file_name, items) in &parsed {
            for item in items {
                if let new_parser::ast::Item::Struct(d) = &item.0 {
                    if d.name.0 != type_name {
                        continue;
                    }
                    for f in &d.fields {
                        if f.name.0 != field_name {
                            continue;
                        }
                        let abs = name_to_path.get(file_name)?;
                        let uri = Url::from_file_path(abs).ok()?;
                        let src = files
                            .iter()
                            .find(|(n, _)| n == file_name)
                            .map(|(_, s)| s)?;
                        return Some(Location {
                            uri,
                            range: byte_range_to_lsp(src, &f.name.1),
                        });
                    }
                }
            }
        }
        None
    };

    for (name, info) in reg.iter_types() {
        if let Some(loc) = location_for(&info.decl_file, &info.decl_span) {
            idx.types.insert(name.to_string(), loc.clone());
            for m in &info.methods {
                let m_name = &m.function.sig.name.0;
                let m_span = &m.function.sig.name.1;
                let m_file = reg.file_names.get(&m.file_id).cloned();
                if let Some(loc) = location_for(&m_file, &Some(m_span.clone())) {
                    idx.methods
                        .insert((name.to_string(), m_name.to_string()), loc.clone());
                    idx.methods_by_name
                        .entry(m_name.to_string())
                        .or_default()
                        .push(loc);
                }
            }
            // Fields (struct types only). The registry stores
            // layouts but not field-name spans — dig back into
            // the parsed AST.
            if let typer::TypeKind::Struct(kind) = &info.kind {
                let names: Vec<String> = match kind {
                    typer::StructKind::Concrete(layout) => {
                        layout.fields.iter().map(|f| f.name.clone()).collect()
                    }
                    typer::StructKind::Templated { fields } => {
                        fields.iter().map(|(n, _)| n.clone()).collect()
                    }
                };
                for fname in names {
                    if let Some(loc) = find_field_span(name, &fname) {
                        idx.fields
                            .insert((name.to_string(), fname), loc);
                    }
                }
            }
        }
    }
    for (name, info) in reg.iter_traits() {
        if let Some(loc) = location_for(&info.decl_file, &info.decl_span) {
            idx.traits.insert(name.to_string(), loc);
        }
    }

    (idx, Some(reg))
}
