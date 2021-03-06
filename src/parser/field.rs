use errors::SpannedObject;

use super::{annotation::Annotation, ty::Type};

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub annotations: Vec<Annotation>,
    pub name: SpannedObject<String>,
    pub ty: Type,
}
