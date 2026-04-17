//! HIR data structures.
//!
//! HIR is a per-function, slot-indexed intermediate representation that
//! mirrors MIR op-for-op but keeps `Call` unresolved (target = `FnRef`).
//! Slots are local to a function; the inliner (Phase 6) remaps them to
//! the MIR's global `u32` addresses.

use either::Either;

/// Local slot identifier, unique within a single function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotId(pub u32);

impl std::fmt::Display for SlotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "s{}", self.0)
    }
}

/// One HIR operation. Mirrors `mir::Mir` plus the unresolved `Call`
/// variant. Unlike MIR, HIR has *no* `If0` — zero-versus-nonzero
/// branching is expressed as a two-arm `Match`. Rationale: keeps
/// control flow uniform so optimizer / interpreter / text-dump
/// have one pattern-match shape to handle. Use
/// [`HirOp::if_zero`] to construct the conventional shape.
#[derive(Debug, Clone, PartialEq)]
pub enum HirOp {
    Set(SlotId, u8),
    /// `Copy(dst, src)` — dst is written, src is read.
    Copy(SlotId, SlotId),
    Inc(SlotId),
    Dec(SlotId),
    Loop(HirBlock),
    Break,
    Continue,
    Stop,
    ReadRegister(SlotId, u8),
    WriteRegister(u8, Either<u8, SlotId>),
    Block(HirBlock),
    Skip,
    /// `Match(discr, arms)`. Each arm: (block, matching_discriminant_values).
    /// Arms are tried in order; the first arm whose values contain the
    /// runtime discriminant fires.
    Match(SlotId, Vec<(HirBlock, Vec<u8>)>),

    /// Unresolved call. `target` is a `FnRef`. `args` is the flat list of
    /// input slots (caller-owned). `ret` is the flat list of output slots
    /// (also caller-owned; the callee writes into them after monomorph+inline).
    Call {
        target: FnRef,
        args: Vec<SlotId>,
        ret: Vec<SlotId>,
    },
}

impl HirOp {
    /// Build a `Match` equivalent to the old `If0(slot, then, else)`:
    /// `then_block` fires when `slot == 0`, `else_block` when
    /// `slot ∈ 1..=15` (every non-zero value a single u4 cell can hold).
    pub fn if_zero(slot: SlotId, then_block: HirBlock, else_block: HirBlock) -> HirOp {
        HirOp::Match(
            slot,
            vec![
                (then_block, vec![0]),
                (else_block, (1..=15).collect()),
            ],
        )
    }
}

/// A block carries an optional `result_slot`, used for expression-based
/// semantics: the last expression in the block copies its value into
/// `result_slot`. For statement-only blocks, `result_slot` is `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct HirBlock {
    pub ops: Vec<HirOp>,
    pub result_slot: Option<SlotId>,
}

impl HirBlock {
    pub fn new() -> Self {
        Self {
            ops: Vec::new(),
            result_slot: None,
        }
    }

    pub fn with_result(result_slot: SlotId) -> Self {
        Self {
            ops: Vec::new(),
            result_slot: Some(result_slot),
        }
    }

    pub fn push(&mut self, op: HirOp) {
        self.ops.push(op);
    }
}

impl Default for HirBlock {
    fn default() -> Self {
        Self::new()
    }
}

/// A complete HIR function body.
#[derive(Debug, Clone, PartialEq)]
pub struct HirFunction {
    /// Flat signature (slot layout for inputs + outputs).
    pub sig: typer::FlatSig,
    pub body: HirBlock,
    /// Total number of slots used by this function (params + locals).
    pub slot_count: u32,
    /// Name of the owning type (matches the `FunctionDB` key).
    pub type_name: String,
    pub method_name: String,
    /// Non-fatal diagnostics raised while generating this function
    /// (unused variable, dead code, etc.). Callers aggregate these
    /// into the program-level warning list; an empty vec means the
    /// function compiled without any lints.
    pub warnings: Vec<errors::Diagnostic>,
}

/// Reference to a function — target of an unresolved `Call`. Template args
/// resolve at monomorphization time. `trait_name` is `None` when the call
/// routed to an inherent method, `Some(trait)` when routed through an
/// `impl Trait for Type` — so two traits with a `eq` method on the same
/// type produce two distinct `FnRef`s.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FnRef {
    pub type_name: String,
    pub method_name: String,
    pub template_args: Vec<ConcreteTemplateArg>,
    pub trait_name: Option<String>,
}

/// A template argument — either a concrete type name (possibly with nested
/// args) or an integer literal (for `Array<T, 9, U4>`-style arguments).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConcreteTemplateArg {
    Type(ConcreteType),
    Value(i64),
}

/// Concrete type after template substitution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConcreteType {
    pub name: String,
    pub args: Vec<ConcreteTemplateArg>,
}
