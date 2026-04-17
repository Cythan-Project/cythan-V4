pub mod array_synth;
pub mod call_graph;
pub mod error;
pub mod gen;
pub mod inline;
pub mod interp;
pub mod ir;
pub mod monomorph;
pub mod natives;
pub mod opt;

#[cfg(test)]
mod tests;

pub use array_synth::{ArrayMonomorphCache, ArraySpec};
pub use call_graph::{build_call_graph, CallGraph};
pub use error::HirError;
pub use gen::{gen_function, gen_function_with_natives};
pub use inline::{hir_to_mir, inline_program, inline_program_full};
pub use interp::{CapturedIo, InterpError, Interpreter, IoContext};
pub use ir::{ConcreteTemplateArg, ConcreteType, FnRef, HirBlock, HirFunction, HirOp, SlotId};
pub use monomorph::{monomorphize, MonomorphKey};
pub use natives::{BuiltinNatives, NativeCall, NativeEmitter, NativeProvider};
pub use opt::{optimize_block, optimize_function};

/// Alias used by the interpreter and inliner for dispatch keys.
pub type FnSigKey = typer::FnSig;
