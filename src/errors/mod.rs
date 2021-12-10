use std::{cmp::Ordering, ops::Range};

use ariadne::{Color, ColorGenerator, Fmt, Label, Report, ReportKind};

use crate::Error;

pub mod mirrors;
pub mod reporting;

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

pub fn invalid_type_template(template_def: &Span, span: &Span) -> Error {
    let mut colors = ColorGenerator::new();
    let b = colors.next();
    let er = Report::build(ReportKind::Error, span.file.to_owned(), span.start)
        .with_code(15)
        .with_message("Invalid template")
        .with_label(
            Label::new(span.as_span())
                .with_message("This doesn't match the type template")
                .with_color(b),
        )
        .with_label(
            Label::new(template_def.as_span())
                .with_message("Should match this template")
                .with_color(b),
        );
    er.finish()
}

pub fn index_out_of_bounds(len: usize, alen: usize, access: &Span, listdef: &Span) -> Error {
    let mut colors = ColorGenerator::new();
    let b = colors.next();
    let er = Report::build(ReportKind::Error, access.file.to_owned(), access.start)
        .with_code(18)
        .with_message("Index out of bounds")
        .with_label(
            Label::new(access.as_span())
                .with_message(format!("The accessed index is {}", len))
                .with_color(b),
        )
        .with_label(
            Label::new(listdef.as_span())
                .with_message(format!("But the list length is {}", alen))
                .with_color(colors.next()),
        );
    let er = if len == alen {
        er.with_note(format!(
            "Did you correctly shift the index? In a {} size list the maximum index is {}",
            len,
            len - 1
        ))
    } else {
        er.with_note(format!(
            "Maybe you should change the list from {} length to {} length",
            alen,
            len + 1
        ))
    };
    er.finish()
}

pub fn expected_number_as_type(span: &Span) -> Error {
    let mut colors = ColorGenerator::new();
    let b = colors.next();
    let er = Report::build(ReportKind::Error, span.file.to_owned(), span.start)
        .with_code(17)
        .with_message("Expected number as type")
        .with_label(
            Label::new(span.as_span())
                .with_message("Should be a number")
                .with_color(b),
        );
    er.finish()
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
