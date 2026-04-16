pub mod registry;
pub mod types;

#[cfg(test)]
mod tests;

pub use registry::TypeRegistry;
pub use types::{
    discriminant_size_for, CellCount, EnumKind, EnumLayout, EnumVariantLayout, FieldLayout,
    FileId, ImplInfo, MethodInfo, StructKind, StructLayout, TemplatedVariant, TraitInfo,
    TypeInfo, TypeKind, TyperError, U4_NAME, U4_SIZE,
};
