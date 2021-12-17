pub enum SkipStatus {
    Stoped,
    Continue,
    Break,
    None,
    Skipped,
}

impl SkipStatus {
    pub fn lightest(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::None, _) => Self::None,
            (_, Self::None) => Self::None,
            (Self::Continue, _) => Self::Continue,
            (_, Self::Continue) => Self::Continue,
            (Self::Break, _) => Self::Break,
            (_, Self::Break) => Self::Break,
            (Self::Skipped, _) => Self::Skipped,
            (_, Self::Skipped) => Self::Skipped,
            _ => Self::Stoped,
        }
    }
}
