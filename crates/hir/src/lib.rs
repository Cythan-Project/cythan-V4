pub mod error;
pub mod gen;
pub mod ir;
pub mod opt;

#[cfg(test)]
mod tests;

pub use error::HirError;
pub use gen::gen_function;
pub use ir::{ConcreteTemplateArg, ConcreteType, FnRef, HirBlock, HirFunction, HirOp, SlotId};
pub use opt::{optimize_block, optimize_function};
