//! `textDocument/codeAction` — lightbulb quick fixes.
//!
//! We read the helps attached to each diagnostic (encoded in the
//! message as `▸ help: …` lines by the diagnostic pusher) and
//! turn the actionable ones into `CodeAction` proposals the
//! editor surfaces via Ctrl+. / lightbulb.
//!
//! Patterns currently recognised:
//!
//! | diagnostic code | help pattern                                        | action |
//! |-----------------|-----------------------------------------------------|--------|
//! | E0001 / E0015   | `did you mean \`X\`?` / `… similar name exists: \`X\`` | rename to X |
//! | W0001           | `prefix with an underscore to silence: \`_x\``       | prefix with `_` |
//! | E0010           | `cast with \`as X\``                                 | append ` as X` |
//! | E0010           | `use \`U8\` (or a smaller literal)`                  | replace with `U8::from(…)` — skipped (needs typer info) |
//!
//! Helps we don't know how to apply are still shown as plain
//! text in the hover tooltip; this module just adds automatic
//! fixes on top of that.

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, Diagnostic,
    NumberOrString, OneOf, OptionalVersionedTextDocumentIdentifier, Position, Range,
    TextDocumentEdit, TextEdit, Url, WorkspaceEdit,
};

use crate::state::State;

pub(crate) fn resolve_code_actions(
    state: &State,
    params: CodeActionParams,
) -> Vec<CodeActionOrCommand> {
    let uri = params.text_document.uri;
    let mut out: Vec<CodeActionOrCommand> = Vec::new();
    for d in &params.context.diagnostics {
        // Help-derived fixes (rename to suggestion, prefix _, cast, …).
        for help in extract_helps(&d.message) {
            for action in actions_for_help(&uri, d, &help) {
                out.push(CodeActionOrCommand::CodeAction(action));
            }
        }
        // Import-derived fixes — look at the diagnostic's code +
        // the identifier from its message; if it's a known
        // workspace symbol, offer `use X;` at the file top.
        for action in actions_for_import(state, &uri, d) {
            out.push(CodeActionOrCommand::CodeAction(action));
        }
        // Missing-struct-field fix (E0015 only) — parse the
        // field names out of the help and splice placeholder
        // assignments in before the literal's closing `}`.
        for action in actions_for_missing_fields(state, &uri, d) {
            out.push(CodeActionOrCommand::CodeAction(action));
        }
    }
    out
}

/// Pull each `▸ help: …` line out of a diagnostic message.
/// Mirrors the format `crates/lsp/src/diagnostics.rs` writes.
fn extract_helps(message: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in message.lines() {
        if let Some(h) = line.trim_start().strip_prefix("▸ help: ") {
            out.push(h.trim().to_string());
        }
    }
    out
}

/// Build zero or more concrete `CodeAction`s from a single help
/// line, given the diagnostic that produced it. An action edits
/// the document so the compiler no longer emits the diagnostic.
fn actions_for_help(uri: &Url, diag: &Diagnostic, help: &str) -> Vec<CodeAction> {
    let mut out = Vec::new();

    // "did you mean `X`?" / "a type with a similar name exists: `X`"
    if let Some(sugg) = extract_suggestion(help) {
        out.push(edit_action(
            uri,
            diag,
            format!("Rename to `{}`", sugg),
            diag.range,
            sugg,
            /*preferred=*/ true,
        ));
    }

    // W0001: "prefix with an underscore to silence: `_x`"
    if help.starts_with("prefix with an underscore") {
        out.push(edit_action(
            uri,
            diag,
            "Prefix with `_` to silence unused-variable warning".into(),
            Range {
                start: diag.range.start,
                end: diag.range.start,
            },
            "_".into(),
            true,
        ));
    }

    // E0010: "cast with `as X` if the narrowing is intentional"
    if help.starts_with("cast with ") {
        if let Some(ty) = extract_cast_type(help) {
            // The diagnostic's range covers the arg expression;
            // append ` as Ty` after it.
            out.push(edit_action(
                uri,
                diag,
                format!("Cast with `as {}`", ty),
                Range {
                    start: diag.range.end,
                    end: diag.range.end,
                },
                format!(" as {}", ty),
                false,
            ));
        }
    }

    out
}

/// Extract the first backticked identifier from a "did you mean"
/// / "similar name" help.
fn extract_suggestion(help: &str) -> Option<String> {
    let patterns = [
        "did you mean `",
        "a type with a similar name exists: `",
        "a similar name exists: `",
    ];
    for p in patterns {
        if let Some(rest) = help.find(p) {
            let after = &help[rest + p.len()..];
            if let Some(end) = after.find('`') {
                return Some(after[..end].to_string());
            }
        }
    }
    None
}

fn extract_cast_type(help: &str) -> Option<String> {
    // Expected shape: "cast with `as X` ..."
    let p = "cast with `as ";
    let rest = help.strip_prefix(p)?;
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

/// Quick fix for E0015 missing-fields-in-struct-literal:
/// insert placeholder `field: 0` assignments before the
/// literal's closing `}`. The help text emitted by the HIR
/// generator lists the field names in backticks; we pull them
/// out and reconstruct the snippet.
fn actions_for_missing_fields(
    state: &State,
    uri: &Url,
    diag: &Diagnostic,
) -> Vec<CodeAction> {
    let Some(NumberOrString::String(code)) = &diag.code else {
        return vec![];
    };
    if code != "E0015" {
        return vec![];
    }
    // The `add `b: …`, `c: …` to this literal` help is the
    // only missing-fields shape we emit. Other E0015 shapes
    // (unknown field, duplicate) are handled by the rename /
    // dedup actions elsewhere.
    let helps = extract_helps(&diag.message);
    let Some(help) = helps.iter().find(|h| h.starts_with("add `")) else {
        return vec![];
    };
    let names = extract_field_names(help);
    if names.is_empty() {
        return vec![];
    }
    // Find the closing `}` of the struct literal by scanning
    // the source backwards from the diagnostic's primary range
    // end. The range includes the whole literal, so the last
    // non-whitespace char should be `}`.
    let Some(text) = state.docs.get(uri) else {
        return vec![];
    };
    let chars: Vec<char> = text.chars().collect();
    let end_off = crate::text::position_to_char_offset(text, diag.range.end)
        .unwrap_or(chars.len());
    // Scan back for the `}`.
    let mut brace_idx = None;
    for i in (0..end_off.min(chars.len())).rev() {
        if chars[i] == '}' {
            brace_idx = Some(i);
            break;
        }
    }
    let Some(brace) = brace_idx else {
        return vec![];
    };
    // Compute insert text. Figure out if the literal already
    // has any fields so we know whether to prepend a `,`.
    let between: String = chars[diag.range.start.line as usize..brace]
        .iter()
        .collect::<String>();
    let _ = between; // unused; simpler heuristic below.
    // Look for any comma or field before the `}` in the same
    // literal; if the char immediately before the brace
    // (skipping whitespace) is `{`, the literal is empty.
    let mut i = brace;
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    let literal_is_empty = i > 0 && chars[i - 1] == '{';
    let trailing_comma = i > 0 && chars[i - 1] == ',';
    let sep = if literal_is_empty || trailing_comma { "" } else { ", " };
    let body = names
        .iter()
        .map(|n| format!("{}: 0", n))
        .collect::<Vec<_>>()
        .join(", ");
    let insert = format!("{}{}", sep, body);

    let insert_pos = crate::text::char_offset_to_lsp_position(text, brace);
    let range = Range {
        start: insert_pos,
        end: insert_pos,
    };
    vec![edit_action(
        uri,
        diag,
        format!(
            "Add missing field{} ({})",
            if names.len() == 1 { "" } else { "s" },
            names
                .iter()
                .map(|n| format!("`{}`", n))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        range,
        insert,
        /*preferred=*/ true,
    )]
}

/// Extract every backticked identifier from a "add `a: …`, `b:
/// …` …" help.
fn extract_field_names(help: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = help.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'`' {
                j += 1;
            }
            if j > start && j < bytes.len() {
                let s = &help[start..j];
                // Accept only `name: …` shapes.
                if let Some(colon) = s.find(':') {
                    let name = s[..colon].trim();
                    if !name.is_empty()
                        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                    {
                        out.push(name.to_string());
                    }
                }
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// When an E0001/E0002 diagnostic names a symbol that IS known
/// to the workspace (just not imported in this file), offer a
/// one-click `use X;` insertion at the top of the document.
fn actions_for_import(state: &State, uri: &Url, diag: &Diagnostic) -> Vec<CodeAction> {
    let Some(NumberOrString::String(code)) = &diag.code else {
        return vec![];
    };
    // Only act on the unknown-type / unknown-trait codes.
    if code != "E0001" && code != "E0002" {
        return vec![];
    }
    let Some(name) = extract_backticked(&diag.message) else {
        return vec![];
    };
    // Symbol known to the index? If not, nothing to import.
    let is_known = state.symbols.types.contains_key(&name)
        || state.symbols.traits.contains_key(&name);
    if !is_known {
        return vec![];
    }
    // Don't offer the fix if the file already imports it. Cheap
    // textual scan — `use` in Cythan is a single-name statement
    // so looking for `use <name>;` is reliable.
    let Some(text) = state.docs.get(uri) else {
        return vec![];
    };
    let needle = format!("use {};", name);
    if text.contains(&needle) {
        return vec![];
    }
    let (range, insert) = compute_import_insertion(text, &name);
    vec![edit_action(
        uri,
        diag,
        format!("Import `{}`", name),
        range,
        insert,
        /*preferred=*/ true,
    )]
}

/// Compute where to insert `use Name;` in a file and the text to
/// insert. Prefers to append to an existing `use …;` block;
/// otherwise prepends to the first line.
fn compute_import_insertion(text: &str, name: &str) -> (Range, String) {
    // Line scan for the last `use …;` line.
    let mut last_use: Option<u32> = None;
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with("use ") && t.ends_with(';') {
            last_use = Some(i as u32);
        } else if last_use.is_some() && !t.is_empty() {
            // Past the imports block — stop looking.
            break;
        }
    }
    match last_use {
        Some(line) => {
            // Insert at the start of the next line.
            let pos = Position {
                line: line + 1,
                character: 0,
            };
            (
                Range { start: pos, end: pos },
                format!("use {};\n", name),
            )
        }
        None => {
            // Insert at the top of the file.
            let pos = Position { line: 0, character: 0 };
            (
                Range { start: pos, end: pos },
                format!("use {};\n", name),
            )
        }
    }
}

/// First backticked identifier in `s`. Used to pull the missing
/// name out of error messages like ``unknown type `Foo` ...``.
fn extract_backticked(s: &str) -> Option<String> {
    let start = s.find('`')?;
    let after = &s[start + 1..];
    let end = after.find('`')?;
    let name = &after[..end];
    if name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.is_empty() {
        Some(name.to_string())
    } else {
        None
    }
}

fn edit_action(
    uri: &Url,
    diag: &Diagnostic,
    title: String,
    range: Range,
    new_text: String,
    preferred: bool,
) -> CodeAction {
    let edit = TextEdit { range, new_text };
    let doc_edit = TextDocumentEdit {
        text_document: OptionalVersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: None,
        },
        edits: vec![OneOf::Left(edit)],
    };
    CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(lsp_types::DocumentChanges::Edits(vec![doc_edit])),
            change_annotations: None,
        }),
        command: None,
        is_preferred: Some(preferred),
        disabled: None,
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_help_lines() {
        let msg = "error: foo\n  • note: n1\n  ▸ help: do X\n  ▸ help: or Y";
        let helps = extract_helps(msg);
        assert_eq!(helps, vec!["do X".to_string(), "or Y".to_string()]);
    }

    #[test]
    fn suggestion_from_did_you_mean() {
        assert_eq!(
            extract_suggestion("did you mean `Foo`?"),
            Some("Foo".into())
        );
    }

    #[test]
    fn suggestion_from_similar_name() {
        assert_eq!(
            extract_suggestion("a type with a similar name exists: `Bar`"),
            Some("Bar".into())
        );
    }

    #[test]
    fn cast_help_extracts_type() {
        assert_eq!(
            extract_cast_type("cast with `as U8` if the narrowing is intentional"),
            Some("U8".into())
        );
    }
}
