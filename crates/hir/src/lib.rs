pub mod call_graph;
pub mod error;
pub mod gen;
pub mod inline;
pub mod interp;
pub mod ir;
pub mod monomorph;
pub mod opt;

#[cfg(test)]
mod tests;

pub use call_graph::{build_call_graph, CallGraph};
pub use error::HirError;
pub use gen::gen_function;
pub use inline::{hir_to_mir, inline_program};
pub use interp::{CapturedIo, InterpError, Interpreter, IoContext};
pub use ir::{ConcreteTemplateArg, ConcreteType, FnRef, HirBlock, HirFunction, HirOp, SlotId};
pub use monomorph::{monomorphize, MonomorphKey};
pub use opt::{optimize_block, optimize_function};

/// Alias used by the interpreter and inliner for dispatch keys.
pub type FnSigKey = typer::FnSig;
