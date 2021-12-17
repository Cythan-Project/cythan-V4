use std::fmt::Display;

#[derive(Debug, Clone, PartialEq, Hash)]
pub struct Var(pub usize);

impl Display for Var {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "'v{}", self.0)
    }
}
impl From<usize> for Var {
    fn from(val: usize) -> Self {
        Self(val)
    }
}
