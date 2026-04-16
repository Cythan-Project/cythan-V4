pub mod error;
pub mod gen;
pub mod ir;

#[cfg(test)]
mod tests;

pub use error::HirError;
pub use gen::gen_function;
pub use ir::{ConcreteTemplateArg, ConcreteType, FnRef, HirBlock, HirFunction, HirOp, SlotId};
