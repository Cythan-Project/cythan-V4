use std::ops::Range;

use ariadne::{Color, ColorGenerator, Fmt, Label, Report, ReportKind};

use crate::{
    compiler::class_loader::ClassLoader,
    errors::Span,
    parser::{class::Class, ty::Type},
};

use super::output_data::OutputData;

#[derive(Debug, Clone)]
pub struct TypedMemory {
    pub locations: Vec<u32>,
    pub span: Span, // the origin of the value (Used for error reporting)
    pub ty: Type,
}

impl TypedMemory {
    pub fn new(ty: Type, locations: Vec<u32>, span: Span) -> TypedMemory {
        TypedMemory {
            locations,
            ty,
            span,
        }
    }
    pub fn native(ty: Type, locations: Vec<u32>) -> TypedMemory {
        Self::new(ty, locations, Span::default())
    }
}
impl CheckAgainst for TypedMemory {
    fn check_against(&self, other: &Type) -> Result<&[u32], Report<(String, Range<usize>)>> {
        if self.ty != *other {
            let mut colors = ColorGenerator::new();
            let a = colors.next();
            let b = colors.next();
            let span = &self.span;
            let er = Report::build(ReportKind::Error, span.file.to_owned(), span.start)
                .with_code(9)
                .with_message("Invalid type")
                .with_label(
                    Label::new(span.as_span())
                        .with_message(format!(
                            "Found {} expected {}",
                            format!("{:?}", self.ty).fg(b),
                            format!("{:?}", other).fg(a)
                        ))
                        .with_color(b),
                );
            let er = if other.span.file == "<internal>" || other.span.file == "<native>" {
                er
            } else {
                er.with_label(
                    Label::new(other.span.as_span())
                        .with_message(format!(
                            "Because of {} requirement here",
                            format!("{:?}", other).fg(b)
                        ))
                        .with_color(b),
                )
            };
            return Err(er.finish());
        }
        Ok(&self.locations)
    }
}

pub trait CheckAgainst {
    fn check_against(&self, other: &Type) -> Result<&[u32], Report<(String, Range<usize>)>>;
}

impl CheckAgainst for OutputData {
    fn check_against(&self, other: &Type) -> Result<&[u32], Report<(String, Range<usize>)>> {
        if let Some(e) = &self.return_value {
            e.check_against(other)
        } else {
            let mut colors = ColorGenerator::new();
            //let a = colors.next();
            let b = colors.next();
            let span = &self.span;
            let er = Report::build(ReportKind::Error, span.file.to_owned(), span.start)
                .with_code(8)
                .with_message("Invalid type")
                .with_label(
                    Label::new(span.as_span())
                        .with_message(format!("This expression doesn't output any value"))
                        .with_color(b),
                );
            let er = if other.span.file == "<internal>" || other.span.file == "<native>" {
                er
            } else {
                er.with_label(
                    Label::new(other.span.as_span())
                        .with_message(format!(
                            "Because of {} requirement here",
                            format!("{:?}", other).fg(b)
                        ))
                        .with_color(b),
                )
            };
            return Err(er.finish());
        }
    }
}
