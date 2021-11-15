use std::{cmp::Ordering, ops::Range};

use ariadne::{Color, ColorGenerator, Fmt, Label, Report, ReportKind};

use crate::Error;

#[derive(Debug, Clone, Eq)]
pub struct Span {
    pub file: String,
    pub start: usize,
    pub end: usize,
}

impl Default for Span {
    fn default() -> Self {
        Span {
            file: "<native>".to_owned(),
            start: 0,
            end: 0,
        }
    }
}

impl PartialOrd for Span {
    fn partial_cmp(&self, _: &Self) -> Option<std::cmp::Ordering> {
        Some(Ordering::Equal)
    }
}

impl PartialEq for Span {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Span {
    pub fn as_range(&self) -> Range<usize> {
        Range {
            start: self.start,
            end: self.end,
        }
    }

    pub fn as_span(&self) -> (String, Range<usize>) {
        (self.file.to_owned(), self.as_range())
    }
    pub fn new(file: String, start: usize, end: usize) -> Self {
        Self { file, start, end }
    }

    pub fn merge(&self, other: &Span) -> Self {
        let start = self.start.min(other.start);
        let end = self.end.max(other.end);
        Self::new(self.file.clone(), start, end)
    }
}

pub fn report_similar(
    singular: &str,
    plural: &str,
    span: &Span,
    current: &str,
    strings: &[String],
    error_id: u32,
) -> Error {
    let mut colors = ColorGenerator::new();
    let b = colors.next();
    let similar = strings
        .iter()
        .filter(|k| {
            k.len() - 1 > 0 && strsim::damerau_levenshtein(current, k) <= (k.len() - 1).min(2)
        })
        .take(5)
        .collect::<Vec<_>>();
    let er = Report::build(ReportKind::Error, span.file.to_owned(), span.start)
        .with_code(error_id)
        .with_message(format!("Invalid {}", singular))
        .with_label(
            Label::new(span.as_span())
                .with_message(format!(
                    "{}{} {} wasn't found",
                    singular[0..1].to_uppercase(),
                    &singular[1..],
                    current.fg(Color::Blue)
                ))
                .with_color(b),
        );
    let er = if similar.is_empty() {
        er
    } else if similar.len() == 1 {
        er.with_note(format!(
            "Another {} in scope has a similar name {}",
            singular,
            similar
                .iter()
                .map(|x| format!("{}", x.fg(Color::Blue)))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    } else {
        er.with_note(format!(
            "Other {} in scope have similar names {}",
            plural,
            similar
                .iter()
                .map(|x| format!("{}", x.fg(Color::Blue)))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    };
    er.finish()
}
