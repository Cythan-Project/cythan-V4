//! Diagnostic push + `errors::Diagnostic` → LSP conversion.
//!
//! The pipeline's `diagnose` function is called from here on
//! every document change. Diagnostics that reference the open
//! file get mapped to `lsp_types::Diagnostic`; the caller's
//! existing file → symbol index + AST cache are refreshed as
//! side effects.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_server::{Connection, Message, Notification};
use lsp_types::notification::{Notification as LspNotification, PublishDiagnostics};
use lsp_types::*;

use errors::{Diagnostic as ErrDiag, LabelKind, Severity};

use crate::state::State;
use crate::symbols::build_symbol_index;
use crate::text::byte_range_to_lsp;

pub(crate) fn publish_for(
    connection: &Connection,
    state: &mut State,
    uri: &Url,
    text: &str,
) {
    let local_path = uri
        .to_file_path()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "<unsaved>".to_string());

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

    // Internal name → absolute path so Locations can carry a
    // proper file URI.
    let mut name_to_path: HashMap<String, PathBuf> = HashMap::new();
    if let Some(std_dir) = &state.std_dir {
        for (name, _) in &files {
            if let Some(stripped) = name.strip_prefix("std/") {
                name_to_path.insert(name.to_string(), std_dir.join(stripped));
            }
        }
    }
    if let Ok(open_path) = uri.to_file_path() {
        name_to_path.insert(local_path.clone(), open_path);
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

    let (idx, reg) = build_symbol_index(&files, &name_to_path);
    state.symbols = idx;
    state.registry = reg;

    if let Ok(items) = new_parser::parse(text) {
        state.asts.insert(uri.clone(), items);
    } else {
        state.asts.remove(uri);
    }

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for d in report.errors.iter().chain(report.warnings.iter()) {
        if let Some(lsp) = to_lsp_diagnostic(d, &local_path, text) {
            diagnostics.push(lsp);
        }
    }
    send_diagnostics(connection, uri, diagnostics);
}

pub(crate) fn send_diagnostics(
    connection: &Connection,
    uri: &Url,
    diagnostics: Vec<Diagnostic>,
) {
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
