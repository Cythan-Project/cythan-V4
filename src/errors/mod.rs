use std::{cmp::Ordering, ops::Range};

#[derive(Debug, Clone, Ord, Eq, Hash)]
pub struct Span {
    pub file: String,
    pub start: usize,
    pub end: usize,
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
