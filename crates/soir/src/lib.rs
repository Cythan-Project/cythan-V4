//! Sea-of-Nodes IR for the Cythan compiler.
//!
//! Replaces the HIR optimizer pipeline (inline + unroll +
//! specialize + optimize + LVA + hir_to_mir) with a graph-based
//! representation. Data, control, and effect are three distinct
//! kinds of edges. Pure ops have no control or effect input; they
//! float and are placed by a global scheduler (see crate `m3` when
//! it lands). Effectful ops consume and produce an effect token
//! threaded through a single linear chain.
//!
//! # Node shape
//!
//! Every node is one `NodeKind` variant. Multi-output nodes
//! (`Start`, `If`, `Match`, `ReadReg`, `WriteReg`, `Call`) expose
//! their outputs through `Proj` child nodes — one `Proj` per
//! selected output. This keeps each `NodeId` single-output from
//! the consumer's side.
//!
//! # Arena
//!
//! `Graph` is a flat `Vec<Option<Node>>` indexed by `NodeId`. IDs
//! are never reused; killed nodes become `None` so consumers never
//! observe a stale id pointing at someone else's data. A periodic
//! compaction pass can rebuild the arena later.

pub mod builder;
pub mod interp;
pub mod ir;
pub mod print;
pub mod schedule;

#[cfg(test)]
mod tests;

pub use builder::{translate_function, translate_program};
pub use interp::{run, run_with_limit, RunResult, DEFAULT_STEP_LIMIT};
pub use ir::{FnKey, Graph, Node, NodeId, NodeKind, Program, ProjKind};
pub use print::dump_graph;
pub use schedule::schedule;
