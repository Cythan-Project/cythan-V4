use std::fmt::Display;

use crate::number::Counter;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Label(pub usize, pub LabelType);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LabelType {
    LoopStart,
    LoopEnd,
    IfStart,
    IfEnd,
    BlockEnd,
    Match,
}

impl Display for LabelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                LabelType::LoopStart => 'A',
                LabelType::LoopEnd => 'B',
                LabelType::IfStart => 'D',
                LabelType::IfEnd => 'F',
                LabelType::BlockEnd => 'G',
                LabelType::Match => 'H',
            }
        )
    }
}

impl Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "'l{}{}", self.1, self.0)
    }
}

impl Label {
    pub fn alloc(state: &mut Counter, t: LabelType) -> Self {
        Self(state.count(), t)
    }
    pub fn derive(&self, t: LabelType) -> Self {
        Self(self.0, t)
    }
}
