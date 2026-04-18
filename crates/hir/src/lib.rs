pub mod arg_elide;
pub mod array_synth;
pub mod call_graph;
pub mod error;
pub mod gen;
pub mod inline;
pub mod interp;
pub mod ir;
pub mod monomorph;
pub mod mut_elide;
pub mod natives;
pub mod opt;
pub mod spec_monomorph;
pub mod specialize;
pub mod text_dump;
pub mod unroll;

#[cfg(test)]
mod tests;

pub use arg_elide::{
    elide_unused_args, elide_unused_args_with_stats, summarize as summarize_arg_usage, ElideStats,
    FnSummary,
};
pub use mut_elide::{
    compute_effective_mutation, elide_redundant_mut, elide_redundant_mut_with_stats,
    summarize_mutation, FnMutationSummary, MutationElideStats,
};
pub use unroll::{unroll_loops, unroll_loops_with_stats, UnrollStats, DEFAULT_UNROLL_FACTOR};
pub use array_synth::{ArrayMonomorphCache, ArraySpec};
pub use call_graph::{build_call_graph, CallGraph};
pub use error::HirError;
pub use gen::{gen_function, gen_function_with_natives};
pub use inline::{hir_to_mir, inline_program, inline_program_full};
pub use interp::{CapturedIo, InterpError, Interpreter, IoContext};
pub use ir::{count_ops, ConcreteTemplateArg, ConcreteType, FnRef, HirBlock, HirFunction, HirOp, SlotId};
pub use monomorph::{monomorphize, MonomorphKey};
pub use natives::{BuiltinNatives, NativeCall, NativeEmitter, NativeProvider};
pub use opt::{eliminate_dead_writes, optimize_block, optimize_function};
pub use specialize::{specialize_to_fixpoint, specialize_to_fixpoint_with_domains};
pub use spec_monomorph::{run as specialize_monomorph, SpecResult};

/// Alias used by the interpreter and inliner for dispatch keys.
pub type FnSigKey = typer::FnSig;
