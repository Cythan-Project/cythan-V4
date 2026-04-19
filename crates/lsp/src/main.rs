//! Cythan Language Server.
//!
//! Minimal LSP that exposes the existing `cythan_driver::diagnose`
//! pipeline over stdio. Spawned by the VS Code extension (and any
//! other LSP-aware client). Capabilities:
//!
//! * Text-document sync (full document, on open / change / save).
//! * Push diagnostics — errors and warnings from parse / typer /
//!   HIR-gen, rendered with their `DiagCode` and labelled spans.
//!
//! Multi-file support is best-effort: when a workspace folder is
//! open, the server picks up any `.ct` files under
//! `<workspace>/<std-dir>` (default `examples/new_syntax/std`)
//! and includes them in each `diagnose` call so cross-file
//! symbols resolve. The client can override the std dir via the
//! `cythan.stdDir` initialization option.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_server::{Connection, Message, Notification};
use lsp_types::notification::{Notification as LspNotification, PublishDiagnostics};
use lsp_types::*;

use errors::{Diagnostic as ErrDiag, LabelKind, Severity};

fn main() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    eprintln!("cythan-lsp: starting");
    let (connection, io_threads) = Connection::stdio();

    let server_capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::FULL,
        )),
        ..Default::default()
    };
    let init_params = connection
        .initialize(serde_json::to_value(server_capabilities)?)?;

    let mut state = State::from_init(&init_params);
    main_loop(&connection, &mut state)?;
    io_threads.join()?;
    Ok(())
}

struct State {
    /// Buffer of currently-open documents (URI → full text).
    docs: HashMap<Url, String>,
    /// Standard library directory (resolved relative to the
    /// workspace root if there is one). When present, every
    /// `.ct` file inside is loaded alongside the open document
    /// so cross-file symbols resolve.
    std_dir: Option<PathBuf>,
}

impl State {
    fn from_init(init: &serde_json::Value) -> Self {
        let workspace_root = init
            .get("rootUri")
            .and_then(|v| v.as_str())
            .and_then(|s| Url::parse(s).ok())
            .and_then(|u| u.to_file_path().ok())
            .or_else(|| {
                init.get("workspaceFolders")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(|f| f.get("uri"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| Url::parse(s).ok())
                    .and_then(|u| u.to_file_path().ok())
            });
        let configured_std = init
            .get("initializationOptions")
            .and_then(|v| v.get("stdDir"))
            .and_then(|v| v.as_str())
            .map(PathBuf::from);
        let std_dir = configured_std
            .filter(|p| p.is_dir())
            .or_else(|| workspace_root.as_ref().and_then(|r| find_std_dir(r)));
        eprintln!("cythan-lsp: std_dir = {:?}", std_dir);
        State {
            docs: HashMap::new(),
            std_dir,
        }
    }
}

/// Locate the Cythan stdlib relative to a workspace folder. Tries
/// (in order): `<root>/std`, `<root>/examples/new_syntax/std`, then
/// walks up parent directories looking for either layout. Returns
/// the first directory found.
fn find_std_dir(start: &std::path::Path) -> Option<PathBuf> {
    let candidates = |dir: &std::path::Path| {
        vec![
            dir.join("std"),
            dir.join("examples").join("new_syntax").join("std"),
        ]
    };
    let mut dir = start.to_path_buf();
    loop {
        for c in candidates(&dir) {
            if c.is_dir() {
                return Some(c);
            }
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p.to_path_buf(),
            _ => return None,
        }
    }
}

fn main_loop(
    connection: &Connection,
    state: &mut State,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                // No requests answered yet (only diagnostics push).
            }
            Message::Notification(not) => handle_notification(connection, state, not),
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn handle_notification(connection: &Connection, state: &mut State, not: Notification) {
    match not.method.as_str() {
        "textDocument/didOpen" => {
            if let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(not.params)
            {
                state
                    .docs
                    .insert(params.text_document.uri.clone(), params.text_document.text.clone());
                publish_for(
                    connection,
                    state,
                    &params.text_document.uri,
                    &params.text_document.text,
                );
            }
        }
        "textDocument/didChange" => {
            if let Ok(mut params) =
                serde_json::from_value::<DidChangeTextDocumentParams>(not.params)
            {
                if let Some(change) = params.content_changes.pop() {
                    state
                        .docs
                        .insert(params.text_document.uri.clone(), change.text.clone());
                    publish_for(connection, state, &params.text_document.uri, &change.text);
                }
            }
        }
        "textDocument/didSave" => {
            if let Ok(params) = serde_json::from_value::<DidSaveTextDocumentParams>(not.params)
            {
                let uri = params.text_document.uri.clone();
                let text = params
                    .text
                    .clone()
                    .or_else(|| state.docs.get(&uri).cloned())
                    .unwrap_or_default();
                publish_for(connection, state, &uri, &text);
            }
        }
        "textDocument/didClose" => {
            if let Ok(params) = serde_json::from_value::<DidCloseTextDocumentParams>(not.params)
            {
                state.docs.remove(&params.text_document.uri);
                // Clear diagnostics on close so stale messages don't linger.
                send_diagnostics(connection, &params.text_document.uri, vec![]);
            }
        }
        _ => {}
    }
}

fn publish_for(connection: &Connection, state: &State, uri: &Url, text: &str) {
    let local_path = uri
        .to_file_path()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "<unsaved>".to_string());

    // Build the file set: the open document first, then any std
    // files we can locate. Skip the open file from the std set if
    // it lives there (so we don't double-include and confuse the
    // file-name attribution).
    // Match the CLI's gather_files ordering (std files first,
    // open file last). The typer's per-name attribution treats the
    // sequence as a flat set, so order shouldn't matter — but
    // mirroring the CLI keeps any incidental order-sensitive
    // behaviour consistent between the two surfaces.
    let mut files: Vec<(String, String)> = Vec::new();
    if let Some(std_dir) = &state.std_dir {
        match std::fs::read_dir(std_dir) {
            Ok(read) => {
                for entry in read.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|s| s.to_str()) != Some("ct") {
                        continue;
                    }
                    let same_as_open =
                        uri.to_file_path().map(|op| op == p).unwrap_or(false);
                    if same_as_open {
                        continue;
                    }
                    match std::fs::read_to_string(&p) {
                        Ok(content) => {
                            // Use `std/<name>` to match the CLI's naming
                            // convention so any file-based attribution
                            // matches between the two surfaces.
                            let name = format!(
                                "std/{}",
                                p.file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| p.display().to_string())
                            );
                            files.push((name, content));
                        }
                        Err(e) => eprintln!(
                            "cythan-lsp: failed to read {}: {}",
                            p.display(),
                            e
                        ),
                    }
                }
            }
            Err(e) => eprintln!(
                "cythan-lsp: read_dir({}) failed: {}",
                std_dir.display(),
                e
            ),
        }
    }
    files.push((local_path.clone(), text.to_string()));
    eprintln!(
        "cythan-lsp: diagnose with {} files (open = {})",
        files.len(),
        local_path
    );

    let as_refs: Vec<(&str, String)> = files
        .iter()
        .map(|(n, c)| (n.as_str(), c.clone()))
        .collect();
    let report = cythan_driver::new_pipeline::diagnose(&as_refs);

    // Filter to diagnostics that point at the open file. Other-file
    // diagnostics don't have a useful URI to attach to in this
    // single-file push model.
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for d in report.errors.iter().chain(report.warnings.iter()) {
        if let Some(lsp) = to_lsp_diagnostic(d, &local_path, text) {
            diagnostics.push(lsp);
        }
    }
    send_diagnostics(connection, uri, diagnostics);
    let _ = state; // keep parameter for future per-doc state tweaks
}

fn send_diagnostics(connection: &Connection, uri: &Url, diagnostics: Vec<Diagnostic>) {
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    };
    let not = Notification {
        method: PublishDiagnostics::METHOD.to_string(),
        params: serde_json::to_value(params).unwrap_or(serde_json::Value::Null),
    };
    let _ = connection.sender.send(Message::Notification(not));
}

/// Convert one `errors::Diagnostic` to one LSP `Diagnostic`.
/// Returns `None` if the diagnostic doesn't reference `local_file`
/// (we only surface diagnostics for the open document).
fn to_lsp_diagnostic(d: &ErrDiag, local_file: &str, text: &str) -> Option<Diagnostic> {
    let primary = d
        .labels
        .iter()
        .find(|l| l.kind == LabelKind::Primary && l.span.file == local_file)
        .or_else(|| d.labels.iter().find(|l| l.span.file == local_file))?;

    let range = byte_range_to_lsp(text, &primary.span.range);

    let mut message = d.message.clone();
    if !primary.message.is_empty() {
        message.push('\n');
        message.push_str(&primary.message);
    }
    for n in &d.notes {
        message.push_str("\nnote: ");
        message.push_str(n);
    }
    for h in &d.helps {
        message.push_str("\nhelp: ");
        message.push_str(h);
    }

    let related: Vec<DiagnosticRelatedInformation> = d
        .labels
        .iter()
        .filter(|l| !std::ptr::eq(*l, primary))
        .filter_map(|l| {
            if l.span.file != local_file {
                return None;
            }
            let r = byte_range_to_lsp(text, &l.span.range);
            Some(DiagnosticRelatedInformation {
                location: Location {
                    uri: Url::from_file_path(&local_file).ok()?,
                    range: r,
                },
                message: l.message.clone(),
            })
        })
        .collect();

    Some(Diagnostic {
        range,
        severity: Some(severity_to_lsp(d.severity)),
        code: d.code.map(|c| NumberOrString::String(c.0.to_string())),
        code_description: None,
        source: Some("cythan".to_string()),
        message,
        related_information: if related.is_empty() {
            None
        } else {
            Some(related)
        },
        tags: None,
        data: None,
    })
}

fn severity_to_lsp(s: Severity) -> DiagnosticSeverity {
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Note | Severity::Help => DiagnosticSeverity::INFORMATION,
    }
}

/// Char-offset range → LSP `Range`.
///
/// The parser tracks spans as **character (Unicode scalar value)
/// offsets**, not byte offsets — chumsky's default for `&str`
/// input. A multi-byte UTF-8 char (e.g. an em-dash in a comment)
/// makes byte position `> char position` for everything after
/// it, so treating the range as bytes mis-points the diagnostic.
///
/// LSP wants line + UTF-16 code units for `character`, which we
/// compute by walking `chars()` and tracking the per-line UTF-16
/// width.
fn byte_range_to_lsp(text: &str, range: &std::ops::Range<usize>) -> Range {
    let start = char_offset_to_position(text, range.start);
    let end = char_offset_to_position(text, range.end);
    let end = if end == start {
        Position {
            line: start.line,
            character: start.character + 1,
        }
    } else {
        end
    };
    Range { start, end }
}

fn char_offset_to_position(text: &str, char_offset: usize) -> Position {
    let mut line: u32 = 0;
    let mut character: u32 = 0;
    let mut count: usize = 0;
    for c in text.chars() {
        if count >= char_offset {
            break;
        }
        if c == '\n' {
            line += 1;
            character = 0;
        } else {
            character += if (c as u32) > 0xFFFF { 2 } else { 1 };
        }
        count += 1;
    }
    Position { line, character }
}
