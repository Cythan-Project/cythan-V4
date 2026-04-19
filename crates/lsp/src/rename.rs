//! `textDocument/rename` and `textDocument/prepareRename`.
//!
//! Cythan has one flat identifier namespace per scope and does
//! not overload names across categories, so a textual match is
//! a reasonable approximation of semantic occurrence:
//!
//! * **Types / traits** — occurrences across every known `.ct`
//!   file get rewritten.
//! * **Methods / fields** — same, scoped to textual matches.
//!   Two types sharing a method name see both renamed; that's
//!   the common case (trait methods named consistently across
//!   implementors).
//! * **Local variables / params** — occurrences limited to the
//!   enclosing method body, found by walking the AST for
//!   `Variable(name)` expressions plus parameter names. Keeps
//!   rename from bleeding into unrelated functions that reuse
//!   the same identifier.
//!
//! `prepareRename` just validates the cursor sits on an
//! identifier and returns its text range.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_types::{
    Position, PrepareRenameResponse, Range, RenameParams, TextDocumentPositionParams,
    TextEdit, Url, WorkspaceEdit,
};

use crate::ast::{enclosing_method_env, walk_block};
use crate::state::State;
use crate::text::{
    byte_range_to_lsp, find_word_occurrences, identifier_at, position_to_char_offset,
    IdContext,
};

pub(crate) fn prepare_rename(
    state: &State,
    params: TextDocumentPositionParams,
) -> Option<PrepareRenameResponse> {
    let uri = &params.text_document.uri;
    let text = state.docs.get(uri)?;
    let (word, _) = identifier_at(text, params.position)?;
    if word.is_empty() {
        return None;
    }
    // Return the identifier's range so the client pre-selects it.
    let range = identifier_range_at(text, params.position)?;
    Some(PrepareRenameResponse::RangeWithPlaceholder { range, placeholder: word })
}

pub(crate) fn rename(state: &State, params: RenameParams) -> Option<WorkspaceEdit> {
    let uri = params.text_document_position.text_document.uri.clone();
    let pos = params.text_document_position.position;
    let new_name = params.new_name;
    let text = state.docs.get(&uri)?;
    let (old_name, ctx) = identifier_at(text, pos)?;
    if old_name.is_empty() || new_name.is_empty() || new_name == old_name {
        return None;
    }
    // Reject invalid identifiers early so the editor shows a useful error.
    if !is_valid_identifier(&new_name) {
        return None;
    }

    // Detect what kind of identifier the cursor is on, then
    // choose a search scope.
    let kind = classify(state, &uri, pos, &old_name, &ctx);
    let mut edits: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    match kind {
        RenameKind::Local => {
            // Scope to the enclosing method's body.
            if let Some(method_edits) = rename_local(state, &uri, text, pos, &old_name, &new_name) {
                edits.insert(uri.clone(), method_edits);
            }
        }
        RenameKind::WorkspaceIdentifier => {
            // Types / traits / methods / fields — rewrite every
            // whole-word occurrence in every known `.ct` file.
            let files = collect_workspace_files(state);
            for (path, src) in &files {
                let Ok(file_uri) = Url::from_file_path(path) else { continue; };
                let occs = find_word_occurrences(src, &old_name);
                if occs.is_empty() {
                    continue;
                }
                let text_edits: Vec<TextEdit> = occs
                    .into_iter()
                    .map(|r| TextEdit {
                        range: byte_range_to_lsp(src, &r),
                        new_text: new_name.clone(),
                    })
                    .collect();
                edits.insert(file_uri, text_edits);
            }
        }
    }

    if edits.is_empty() {
        None
    } else {
        Some(WorkspaceEdit {
            changes: Some(edits),
            document_changes: None,
            change_annotations: None,
        })
    }
}

enum RenameKind {
    /// A local variable or parameter — restrict to the
    /// enclosing method's body so shadowed names elsewhere stay
    /// untouched.
    Local,
    /// A type / trait / method / field. Rename every textual
    /// occurrence workspace-wide.
    WorkspaceIdentifier,
}

fn classify(
    state: &State,
    uri: &Url,
    pos: Position,
    name: &str,
    ctx: &IdContext,
) -> RenameKind {
    // Method calls (`.name`) and static calls (`Type::name`)
    // are global identifiers.
    if matches!(ctx, IdContext::AfterDot | IdContext::AfterColons) {
        return RenameKind::WorkspaceIdentifier;
    }
    // Known type / trait / method name → workspace.
    if state.symbols.types.contains_key(name)
        || state.symbols.traits.contains_key(name)
    {
        return RenameKind::WorkspaceIdentifier;
    }
    if state.symbols.methods_by_name.contains_key(name) {
        return RenameKind::WorkspaceIdentifier;
    }
    // Field? Fields live in `(Type, field)` — a bare name match
    // suffices.
    if state
        .symbols
        .fields
        .keys()
        .any(|(_, f)| f == name)
    {
        return RenameKind::WorkspaceIdentifier;
    }
    // Otherwise assume it's a local. Verify by checking the
    // enclosing method env.
    if let (Some(text), Some(items)) = (state.docs.get(uri), state.asts.get(uri)) {
        if let Some(off) = position_to_char_offset(text, pos) {
            if let Some((env, _f)) = enclosing_method_env(items, off) {
                if env.bindings.contains_key(name) || name == "self" {
                    return RenameKind::Local;
                }
            }
        }
    }
    // Default to local if we couldn't classify — safer than
    // rewriting the whole workspace on a name we don't recognise.
    RenameKind::Local
}

/// Rename a local variable / parameter: walk the enclosing
/// method's AST, collect every `Variable(name)` expression + the
/// param's own name token (if it's a parameter), and emit edits
/// for each occurrence.
fn rename_local(
    state: &State,
    uri: &Url,
    text: &str,
    pos: Position,
    old_name: &str,
    new_name: &str,
) -> Option<Vec<TextEdit>> {
    use new_parser::ast::Expr;
    let items = state.asts.get(uri)?;
    let off = position_to_char_offset(text, pos)?;
    let (_env, f) = enclosing_method_env(items, off)?;
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();

    // Parameter name / `Declaration` names within the body.
    for p in &f.sig.params {
        if p.name.0 == old_name {
            ranges.push(p.name.1.clone());
        }
    }
    walk_block(&f.body.0, &mut |sp| match &sp.0 {
        Expr::Variable(n) if n == old_name => {
            // AST `Expr::Variable` spans the whole expression;
            // that's exactly the identifier token.
            ranges.push(sp.1.clone());
        }
        Expr::Declaration { name, .. } if name.0 == old_name => {
            ranges.push(name.1.clone());
        }
        _ => {}
    });

    if ranges.is_empty() {
        return None;
    }
    Some(
        ranges
            .into_iter()
            .map(|r| TextEdit {
                range: byte_range_to_lsp(text, &r),
                new_text: new_name.to_string(),
            })
            .collect(),
    )
}

/// Collect every `.ct` file we know about: every open
/// document plus every `.ct` file inside `state.std_dir`.
fn collect_workspace_files(state: &State) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    for (uri, text) in &state.docs {
        if let Ok(p) = uri.to_file_path() {
            out.push((p, text.clone()));
        }
    }
    if let Some(std_dir) = &state.std_dir {
        if let Ok(rd) = std::fs::read_dir(std_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("ct")
                    && !out.iter().any(|(q, _)| q == &p)
                {
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        out.push((p, s));
                    }
                }
            }
        }
    }
    out
}

fn identifier_range_at(text: &str, pos: Position) -> Option<Range> {
    let offset = position_to_char_offset(text, pos)?;
    let chars: Vec<char> = text.chars().collect();
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = offset.min(chars.len());
    while start > 0 && is_id(chars[start - 1]) {
        start -= 1;
    }
    let mut end = offset.min(chars.len());
    while end < chars.len() && is_id(chars[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    Some(byte_range_to_lsp(text, &(start..end)))
}

fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first.is_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_identifier_rules() {
        assert!(is_valid_identifier("x"));
        assert!(is_valid_identifier("Foo"));
        assert!(is_valid_identifier("_foo"));
        assert!(is_valid_identifier("foo_bar_42"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("42x"));
        assert!(!is_valid_identifier("x-y"));
        assert!(!is_valid_identifier("x y"));
    }
}
