use std::ops::Range;

use ariadne::{Color, Config, Label, Report, ReportKind};

pub struct MirrorLabel<S = Range<usize>> {
    pub span: S,
    _msg: Option<String>,
    _color: Option<Color>,
    _order: i32,
    _priority: i32,
}

impl<S> MirrorLabel<S> {
    pub fn from(r: &mut Label<S>) -> &mut Self {
        unsafe { std::mem::transmute(r) }
    }
}

pub struct MirrorReport<S: ariadne::Span = Range<usize>> {
    _kind: ReportKind,
    _code: Option<u32>,
    _msg: Option<String>,
    _note: Option<String>,
    pub location: (<S::SourceId as ToOwned>::Owned, usize),
    pub labels: Vec<Label<S>>,
    _config: Config,
}

impl<S: ariadne::Span> MirrorReport<S> {
    pub fn from(r: &mut Report<S>) -> &mut Self {
        unsafe { std::mem::transmute(r) }
    }
}
