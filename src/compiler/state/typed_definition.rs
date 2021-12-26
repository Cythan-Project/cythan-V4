use errors::{invalid_type, Error, Span};

use crate::parser::ty::Type;

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
    fn check_against(&self, other: &Type) -> Result<&[u32], Error> {
        if self.ty != *other {
            return Err(invalid_type(
                &self.span,
                &other.span,
                &format!("{:?}", self.ty),
                &format!("{:?}", other),
            ));
        }
        Ok(&self.locations)
    }
}

pub trait CheckAgainst {
    fn check_against(&self, other: &Type) -> Result<&[u32], Error>;
}

impl CheckAgainst for OutputData {
    fn check_against(&self, other: &Type) -> Result<&[u32], Error> {
        if let Some(e) = &self.return_value {
            e.check_against(other)
        } else {
            Err(invalid_type(
                &self.span,
                &other.span,
                &self
                    .return_value
                    .as_ref()
                    .map(|x| format!("{:?}", x.ty))
                    .unwrap_or_else(|| "Void".to_owned()),
                &format!("{:?}", other),
            ))
        }
    }
}
