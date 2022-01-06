use std::{cmp::Ordering, ops::Range};

use ariadne::{Color, ColorGenerator, Fmt, Label, Report, ReportBuilder, ReportKind};

pub type Error = ReportBuilder<(String, Range<usize>)>;

mod mirrors;
mod reporting;
mod wrappers;

pub use wrappers::*;

pub use reporting::report;

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

pub fn in_method(template_def: &Span, er: Error) -> Error {
    let mut colors = ColorGenerator::new();
    let b = colors.next();
    er.with_label(
        Label::new(template_def.as_span())
            .with_color(b)
            .with_message("Originated from here"),
    )
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
    er
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
    er
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
    er
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
    er
}

pub fn invalid_token(
    token_name: &str,
    expected_tokens: &[&str],
    span: &Span,
    error_id: u32,
) -> Error {
    let mut colors = ColorGenerator::new();
    let a = colors.next();
    let out = Color::Fixed(81);
    Report::build(ReportKind::Error, span.file.to_owned(), 0)
        .with_code(error_id)
        .with_message("Invalid token")
        .with_label(
            Label::new(span.as_span())
                .with_message(format!("This is a {} token", token_name.fg(a)))
                .with_color(a),
        )
        .with_note(match expected_tokens.len() {
            0 => "No token expected".to_owned(),
            1 => format!("Expected {}", expected_tokens[0].fg(out)),
            _ => format!(
                "Expected {} or {}",
                expected_tokens
                    .iter()
                    .skip(1)
                    .map(|x| x.fg(out).to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                expected_tokens[0].fg(out)
            ),
        })
}

pub fn invalid_argument_type(span: &Span, expected_type: &str, found_type: &str) -> Error {
    Report::build(ReportKind::Error, span.file.to_owned(), span.start)
        .with_code(16)
        .with_message("Invalid argument type")
        .with_label(
            Label::new(span.as_span())
                .with_message(format!(
                    "Expected {} found {}",
                    expected_type.fg(Color::Green),
                    found_type.fg(Color::Green),
                ))
                .with_color(Color::Green),
        )
}

pub fn invalid_token_after(
    span: &Span,
    previous_span: &Span,
    previous_name: &str,
    token_name: &str,
    expected_tokens: &[&str],
    show_semicolon_suggestion: bool,
) -> Error {
    let mut colors = ColorGenerator::new();
    let a = colors.next();
    let b = colors.next();
    let out = Color::Fixed(81);

    let k = Report::build(ReportKind::Error, span.file.to_owned(), span.start)
        .with_code(2)
        .with_message(if token_name.is_empty() {
            format!("Expected token after {}", previous_name)
        } else {
            format!("Invalid token after {}", previous_name)
        });
    if show_semicolon_suggestion {
        k.with_label(
            Label::new(previous_span.as_span())
                .with_message(format!("Did you forget a {} at the end?", ";".fg(b)))
                .with_color(b),
        )
    } else {
        k
    }
    .with_label(
        Label::new(span.as_span())
            .with_message(if token_name.is_empty() {
                format!("Token expected after {}", previous_name,)
            } else {
                format!(
                    "This {} token wasn't expected after {}",
                    token_name.fg(a),
                    previous_name,
                )
            })
            .with_color(a),
    )
    .with_note(match expected_tokens.len() {
        0 => "No token expected".to_owned(),
        1 => format!("Expected {}", expected_tokens[0].fg(out)),
        _ => format!(
            "Expected {} or {}",
            expected_tokens
                .iter()
                .skip(1)
                .map(|x| x.fg(out).to_string())
                .collect::<Vec<_>>()
                .join(", "),
            expected_tokens[0].fg(out)
        ),
    })
}

pub fn method_return_type_invalid(
    span: &Span,
    type_def: &Span,
    found_type: &str,
    expected_type: &str,
) -> Error {
    let mut colors = ColorGenerator::new();
    let a = colors.next();
    let b = colors.next();
    let _out = Color::Fixed(81);

    Report::build(ReportKind::Error, span.file.to_owned(), span.start)
        .with_code(5)
        .with_message("Return type doesn't match method return type")
        .with_label(
            Label::new(type_def.as_span())
                .with_message("Type defined here")
                .with_color(b),
        )
        .with_label(
            Label::new(span.as_span())
                .with_message(format!(
                    "This expression has type {} expected {}.",
                    found_type.fg(a),
                    expected_type.fg(a)
                ))
                .with_color(a),
        )
}

pub fn invalid_length_asm(this: &Span, length1: u32, length2: u32) -> Error {
    let mut colors = ColorGenerator::new();
    let a = colors.next();
    let b = colors.next();
    Report::build(ReportKind::Error, this.file.to_owned(), this.start)
        .with_code(666)
        .with_message("Invalid length in MIR-ASM copy operation")
        .with_label(
            Label::new(this.as_span())
                .with_message(format!(
                    "Found {} expected {}",
                    length1.to_string().fg(b),
                    length2.to_string().fg(a)
                ))
                .with_color(b),
        )
}

pub fn invalid_type(this: &Span, other: &Span, this_type: &str, other_type: &str) -> Error {
    let mut colors = ColorGenerator::new();
    let a = colors.next();
    let b = colors.next();
    let er = Report::build(ReportKind::Error, this.file.to_owned(), this.start)
        .with_code(9)
        .with_message("Invalid type")
        .with_label(
            Label::new(this.as_span())
                .with_message(format!(
                    "Found {} expected {}",
                    this_type.fg(b),
                    other_type.fg(a)
                ))
                .with_color(b),
        );
    if other.file == "<internal>" || other.file == "<native>" {
        er
    } else {
        er.with_label(
            Label::new(other.as_span())
                .with_message(format!("Because of {} requirement here", other_type.fg(b)))
                .with_color(b),
        )
    }
}
