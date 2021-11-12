use super::{annotation::Annotation, ty::Type};

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub annotations: Vec<Annotation>,
    pub name: String,
    pub ty: Type,
}
