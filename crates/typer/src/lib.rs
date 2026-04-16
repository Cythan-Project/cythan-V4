pub mod flat_sig;
pub mod function_db;
pub mod registry;
pub mod types;

#[cfg(test)]
mod tests;

pub use flat_sig::{FieldSlot, FlatSig, SlotIndex, SlotInfo};
pub use function_db::{Fn, FnSig, FunctionDB, SimpleFn, TemplatedFn};
pub use registry::TypeRegistry;
pub use registry::OPERATOR_TRAITS;
pub use types::{
    discriminant_size_for, CellCount, EnumKind, EnumLayout, EnumVariantLayout, FieldLayout,
    FileId, ImplInfo, MethodInfo, MethodResolution, StructKind, StructLayout, TemplatedVariant,
    TraitInfo, TypeInfo, TypeKind, TyperError, U4_NAME, U4_SIZE,
};
