//! Structured compiler diagnostics.
//!
//! A `Diagnostic` is the **data** — primary message, optional error
//! code, one or more labelled spans, plus free-form notes and helps.
//! Rendering (terminal, LSP, JSON) is a separate concern so the same
//! diagnostic can drive CLI output today and a language server later.
//!
//! # Shape
//!
//! ```text
//! error[E0003]: duplicate definition of type `Foo`
//!    ┌─ user.ct:8:8
//!  8 │ struct Foo {}
//!    │        ^^^ duplicate definition here
//!    ·
//!  3 │ struct Foo { U4 v, }
//!    │        --- first defined here
//!    = note: each type may only be defined once per scope
//!    = help: rename one of them, or use a module path
//! ```
//!
//! # LSP mapping
//!
//! All fields are plain data (no terminal escapes), so converting to
//! `lsp_types::Diagnostic` is mechanical: the primary label becomes the
//! diagnostic's range; secondary labels become `relatedInformation`;
//! notes + helps concatenate into the message. `DiagCode` is a stable
//! short string suitable for LSP's `code` field, and error codes can
//! be exposed from the CLI too so users can grep documentation.

use std::fmt;
use std::ops::Range;

use ariadne::{Color, Label as AriLabel, Report, ReportKind, Source};

/// Diagnostic kind. Tests and renderers can filter by severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Help,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Note => "note",
            Self::Help => "help",
        }
    }
}

/// Stable identifier for a diagnostic kind. Think of it as the entry
/// in a "E0xxx index" developers can look up. Tests assert on these
/// directly; CLI renders them as `error[E0003]`; LSPs surface them in
/// the `code` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagCode(pub &'static str);

impl fmt::Display for DiagCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Canonical error/warning codes. Keep these stable — tests, docs,
/// and any LSP client will reference them.
pub mod codes {
    use super::DiagCode;

    // ---- type/name resolution --------------------------------------
    pub const E_UNKNOWN_TYPE: DiagCode = DiagCode("E0001");
    pub const E_UNKNOWN_TRAIT: DiagCode = DiagCode("E0002");
    pub const E_DUPLICATE_TYPE: DiagCode = DiagCode("E0003");
    pub const E_DUPLICATE_TRAIT: DiagCode = DiagCode("E0004");
    pub const E_DUPLICATE_METHOD: DiagCode = DiagCode("E0005");

    // ---- type check ------------------------------------------------
    pub const E_TYPE_MISMATCH: DiagCode = DiagCode("E0010");
    pub const E_MUTABILITY: DiagCode = DiagCode("E0011");
    pub const E_MISSING_IMPL: DiagCode = DiagCode("E0012");
    pub const E_WRONG_ARG_COUNT: DiagCode = DiagCode("E0013");
    pub const E_UNKNOWN_METHOD: DiagCode = DiagCode("E0014");
    pub const E_UNKNOWN_FIELD: DiagCode = DiagCode("E0015");
    pub const E_UNKNOWN_VARIABLE: DiagCode = DiagCode("E0016");
    pub const E_UNKNOWN_VARIANT: DiagCode = DiagCode("E0017");
    pub const E_CONTROL_FLOW: DiagCode = DiagCode("E0018");

    // ---- warnings --------------------------------------------------
    pub const W_UNUSED_VARIABLE: DiagCode = DiagCode("W0001");
    pub const W_UNUSED_USE: DiagCode = DiagCode("W0002");
}

/// A span tied to a named source. The `file` is any opaque identifier
/// the renderer can look up (path string, LSP URI, etc.). `range` is a
/// byte-offset range into that file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileSpan {
    pub file: String,
    pub range: Range<usize>,
}

impl FileSpan {
    pub fn new(file: impl Into<String>, range: Range<usize>) -> Self {
        Self { file: file.into(), range }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LabelKind {
    /// The "main" span — what the diagnostic is about. Conventionally
    /// one per diagnostic, though the type allows more.
    Primary,
    /// An auxiliary span — "the other definition", "previous use",
    /// "expected here". Renderers typically underline with a thinner
    /// marker and put the label text next to it.
    Secondary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub span: FileSpan,
    pub message: String,
    pub kind: LabelKind,
}

impl Label {
    pub fn primary(span: FileSpan, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), kind: LabelKind::Primary }
    }
    pub fn secondary(span: FileSpan, message: impl Into<String>) -> Self {
        Self { span, message: message.into(), kind: LabelKind::Secondary }
    }
}

/// A single diagnostic. `notes` are expository (the compiler
/// explaining the rule), `helps` are actionable (the compiler
/// suggesting a fix). Both render after the labelled spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Option<DiagCode>,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub helps: Vec<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Self {
            severity,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            helps: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Severity::Error, message)
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, message)
    }

    pub fn with_code(mut self, code: DiagCode) -> Self {
        self.code = Some(code);
        self
    }

    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    pub fn with_primary(self, span: FileSpan, msg: impl Into<String>) -> Self {
        self.with_label(Label::primary(span, msg))
    }

    pub fn with_secondary(self, span: FileSpan, msg: impl Into<String>) -> Self {
        self.with_label(Label::secondary(span, msg))
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.helps.push(help.into());
        self
    }

    /// The first `Primary` label, if any. Renderers anchor the header
    /// line here; the LSP mapping uses this label's span as the
    /// `range` field.
    pub fn primary_label(&self) -> Option<&Label> {
        self.labels.iter().find(|l| l.kind == LabelKind::Primary)
    }
}

/// Trait for sources the renderer can read. In practice it's a
/// `HashMap<String, String>` or an IDE's live-buffer store. Kept
/// trait-shaped so tests can mock without touching the filesystem.
pub trait SourceMap {
    fn get(&self, file: &str) -> Option<&str>;
}

impl SourceMap for std::collections::HashMap<String, String> {
    fn get(&self, file: &str) -> Option<&str> {
        self.get(file).map(String::as_str)
    }
}

/// Render a diagnostic to a String using `ariadne` (pretty terminal
/// output with underlines, colors, margin numbers). Callers supply
/// a `SourceMap` so the renderer can pull the snippet text. When a
/// span's file isn't in the map the label is emitted without a
/// source snippet (just the byte range).
pub fn render(diag: &Diagnostic, sources: &dyn SourceMap) -> String {
    // Pick an anchor file/range — if the diagnostic has no labels at
    // all, fall back to a synthetic span in a "<no-location>" file.
    let anchor = diag
        .primary_label()
        .or_else(|| diag.labels.first())
        .map(|l| l.span.clone())
        .unwrap_or_else(|| FileSpan::new("<no-location>", 0..0));

    let kind = match diag.severity {
        Severity::Error => ReportKind::Error,
        Severity::Warning => ReportKind::Warning,
        Severity::Note | Severity::Help => ReportKind::Advice,
    };

    let header = match &diag.code {
        Some(c) => format!("[{}] {}", c, diag.message),
        None => diag.message.clone(),
    };

    let mut report = Report::build(kind, anchor.file.clone(), anchor.range.start)
        .with_message(header);

    for label in &diag.labels {
        let color = match label.kind {
            LabelKind::Primary => Color::Red,
            LabelKind::Secondary => Color::Cyan,
        };
        report = report.with_label(
            AriLabel::new((label.span.file.clone(), label.span.range.clone()))
                .with_message(&label.message)
                .with_color(color),
        );
    }
    for note in &diag.notes {
        report = report.with_note(note);
    }
    for help in &diag.helps {
        report = report.with_help(help);
    }

    // Collect all distinct files referenced so ariadne can resolve them.
    let mut used: Vec<String> = diag.labels.iter().map(|l| l.span.file.clone()).collect();
    used.push(anchor.file.clone());
    used.sort();
    used.dedup();

    let mut cache_entries: Vec<(String, Source)> = Vec::with_capacity(used.len());
    for file in used {
        let src = sources.get(&file).unwrap_or("").to_string();
        cache_entries.push((file, Source::from(src)));
    }
    let cache = FileCache(cache_entries);

    let mut out = Vec::new();
    let finished = report.finish();
    let _ = finished.write(cache, &mut out);
    String::from_utf8(out).unwrap_or_else(|_| "<non-utf8 diagnostic>".into())
}

/// Render several diagnostics, one after another with a blank line
/// between. Ergonomic for "show me everything that went wrong" flows.
pub fn render_all(diags: &[Diagnostic], sources: &dyn SourceMap) -> String {
    diags
        .iter()
        .map(|d| render(d, sources))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Plain-text dump (no colors, no underlines) for tests and LSPs
/// that want the raw message + code + labels. Stable, easy to match
/// against with `contains`.
pub fn render_plain(diag: &Diagnostic) -> String {
    let mut s = String::new();
    s.push_str(diag.severity.label());
    if let Some(code) = &diag.code {
        s.push('[');
        s.push_str(code.0);
        s.push(']');
    }
    s.push_str(": ");
    s.push_str(&diag.message);
    for label in &diag.labels {
        s.push('\n');
        s.push_str(match label.kind {
            LabelKind::Primary => "  --> ",
            LabelKind::Secondary => "   ^  ",
        });
        s.push_str(&label.span.file);
        s.push(':');
        s.push_str(&label.span.range.start.to_string());
        s.push_str("..");
        s.push_str(&label.span.range.end.to_string());
        if !label.message.is_empty() {
            s.push_str(": ");
            s.push_str(&label.message);
        }
    }
    for note in &diag.notes {
        s.push_str("\n  = note: ");
        s.push_str(note);
    }
    for help in &diag.helps {
        s.push_str("\n  = help: ");
        s.push_str(help);
    }
    s
}

struct FileCache(Vec<(String, Source)>);

impl ariadne::Cache<String> for FileCache {
    fn fetch(&mut self, key: &String) -> Result<&Source, Box<dyn fmt::Debug + '_>> {
        match self.0.iter().position(|(k, _)| k == key) {
            Some(i) => Ok(&self.0[i].1),
            None => Err(Box::new(format!("unknown file: {}", key))),
        }
    }
    fn display<'a>(&self, key: &'a String) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(key.clone()))
    }
}

// ---- name-suggestion helper ---------------------------------------------

/// Given a misspelled name and a set of candidates, return the
/// closest one (by Damerau-Levenshtein distance) if any candidate is
/// reasonably close. Threshold: `max(1, len/3)` edits — the same
/// "do you mean?" heuristic rustc uses. Returns `None` when nothing
/// qualifies, so callers can `.and_then` into `.with_help(...)`.
pub fn suggest_name<'a, I>(name: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let threshold = (name.len() / 3).max(1);
    candidates
        .into_iter()
        .filter_map(|c| {
            let d = strsim::damerau_levenshtein(name, c);
            if d == 0 || d > threshold {
                None
            } else {
                Some((d, c))
            }
        })
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}
