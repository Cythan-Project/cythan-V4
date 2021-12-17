use crate::{number::Number, var::Var};

#[derive(Debug, Clone, PartialEq, Hash)]
pub enum AsmValue {
    Var(Var),
    Number(Number),
}

#[allow(unused)]
impl AsmValue {
    pub fn number(&self) -> Option<Number> {
        if let Self::Number(a) = self {
            Some(a.clone())
        } else {
            None
        }
    }
    pub fn var(&self) -> Option<Var> {
        if let Self::Var(a) = self {
            Some(a.clone())
        } else {
            None
        }
    }
}

impl From<u8> for AsmValue {
    fn from(a: u8) -> Self {
        AsmValue::Number(Number(a))
    }
}

impl From<usize> for AsmValue {
    fn from(a: usize) -> Self {
        AsmValue::Var(Var(a))
    }
}
