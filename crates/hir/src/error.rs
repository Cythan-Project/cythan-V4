//! HIR-generation errors.
//!
//! Shares the same structure as `typer::TyperError`: a terse
//! `message + span` pair for legacy call sites plus an optional
//! full `Diagnostic` for rich error reports. New sites should go
//! through `HirError::from_diagnostic`.

#[derive(Debug, Clone, PartialEq)]
pub struct HirError {
    pub message: String,
    pub span: Option<new_parser::Span>,
    pub diagnostic: Option<errors::Diagnostic>,
}

impl HirError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: None,
            diagnostic: None,
        }
    }

    pub fn at(message: impl Into<String>, span: new_parser::Span) -> Self {
        Self {
            message: message.into(),
            span: Some(span),
            diagnostic: None,
        }
    }

    pub fn from_diagnostic(diag: errors::Diagnostic) -> Self {
        let span = diag.primary_label().map(|l| l.span.range.clone());
        let message = diag.message.clone();
        Self { message, span, diagnostic: Some(diag) }
    }

    pub fn into_diagnostic(self, file: &str) -> errors::Diagnostic {
        if let Some(d) = self.diagnostic {
            return d;
        }
        let mut d = errors::Diagnostic::error(self.message);
        if let Some(range) = self.span {
            d = d.with_primary(errors::FileSpan::new(file, range), "");
        }
        d
    }
}

impl std::fmt::Display for HirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
