#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: String,
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(file: String, start: usize, end: usize) -> Self {
        Self { file, start, end }
    }

    pub fn merge(&self, other: &Span) -> Self {
        let start = self.start.min(other.start);
        let end = self.end.max(other.end);
        Self::new(self.file.clone(), start, end)
    }
}
