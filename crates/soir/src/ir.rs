//! Node types, arena, and program container for the Sea-of-Nodes IR.
//!
//! # Edge categories
//!
//! Edges fall into three kinds, distinguished by the field name on
//! the producing/consuming node:
//!
//! * **Control** (`ctrl`, `entry`, `back`, `preds`, `region`) —
//!   orders operations. Every node that runs has one control input
//!   reaching it from `Start`; pure nodes are an exception (they
//!   float).
//! * **Data** (`cond`, `scrut`, `val`, `values`, `args`, function
//!   positional args on pure ops) — a u4 value flowing between ops.
//!   Produced by `Const`, `Param`, arithmetic, `Phi`, and the
//!   `ReadRegVal` projection of `ReadReg`.
//! * **Effect** (`eff`) — a token threaded through every effectful
//!   op (`ReadReg`, `WriteReg`, `Call`, `Stop`, `Return`). Enforces
//!   a total order on side effects. Produced by `Start` (initial
//!   token), by the `Eff` projection of each effectful op, and by
//!   `EffPhi` at merges.
//!
//! # Multi-output nodes
//!
//! `Start`, `If`, `Match`, `ReadReg`, `WriteReg`, `Call` produce
//! more than one output. Consumers reach a specific output via a
//! `Proj { of, kind }` node. `Proj` itself is single-output — from
//! the consumer's perspective everything downstream is uniform.
//!
//! # Death
//!
//! `kill` replaces a slot with `None`. `NodeId`s never get reused,
//! so dangling edges are always detectable.

use hir::ir::FnRef;
use std::collections::HashMap;
use std::fmt;

/// Arena index. `u32` is plenty; largest Cythan programs today have
/// ~2000 HIR ops, so graphs stay well under 2^32 nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    pub const INVALID: NodeId = NodeId(u32::MAX);

    pub fn is_valid(self) -> bool {
        self.0 != u32::MAX
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_valid() {
            write!(f, "n{}", self.0)
        } else {
            f.write_str("n?")
        }
    }
}

/// Per-function identity inside a `Program`. Mirrors typer's
/// `FnSig` so lookup + monomorph tables line up with the rest of
/// the pipeline.
pub type FnKey = typer::FnSig;

/// Which output of a multi-output producer a `Proj` selects.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProjKind {
    /// Control exit from `Start`.
    StartCtrl,
    /// Initial effect token from `Start`.
    StartEff,
    /// Parameter `index` value from `Start`. Flattened over the
    /// function's input slots — so a struct param spanning 3 cells
    /// reaches the body via 3 consecutive `Param` projections.
    Param(u32),
    /// True branch of an `If`.
    IfTrue,
    /// False branch of an `If`.
    IfFalse,
    /// Arm `index` of a `Match`.
    MatchArm(u32),
    /// Effect token produced by `ReadReg`, `WriteReg`, or `Call`.
    Eff,
    /// Value produced by `ReadReg`.
    ReadRegVal,
    /// Return value at position `index` from a `Call`.
    CallRet(u32),
}

impl fmt::Display for ProjKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjKind::StartCtrl => f.write_str("start.ctrl"),
            ProjKind::StartEff => f.write_str("start.eff"),
            ProjKind::Param(i) => write!(f, "param[{}]", i),
            ProjKind::IfTrue => f.write_str("if.true"),
            ProjKind::IfFalse => f.write_str("if.false"),
            ProjKind::MatchArm(i) => write!(f, "match.arm[{}]", i),
            ProjKind::Eff => f.write_str("eff"),
            ProjKind::ReadRegVal => f.write_str("read.val"),
            ProjKind::CallRet(i) => write!(f, "call.ret[{}]", i),
        }
    }
}

/// What a node does. Field names carry edge-category meaning
/// (see the module doc).
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    // ---- entry / exit ----
    /// Graph entry. Exposes `StartCtrl`, `StartEff`, and one
    /// `Param(i)` per input slot through `Proj`. No inputs.
    Start,

    /// Function return. Consumes control, effect, and one data
    /// input per output slot. Every graph ends at exactly one
    /// `Return` (multi-path returns merge at a `Region` first).
    Return {
        ctrl: NodeId,
        eff: NodeId,
        values: Vec<NodeId>,
    },

    /// VM `stop`. Halts execution; no further effect chain.
    Stop {
        ctrl: NodeId,
        eff: NodeId,
    },

    // ---- control merges ----
    /// Plain control merge. `preds[i]` flows into `phi.values[i]`
    /// and `eff_phi.effs[i]` at referenced phis.
    Region {
        preds: Vec<NodeId>,
    },

    /// Loop header. Exactly two preds: `entry` from outside and
    /// `back` from the body's backedge. `back` is `None` while the
    /// builder still has the body open; set before closing.
    Loop {
        entry: NodeId,
        back: Option<NodeId>,
    },

    /// `Block` scope marker. Control enters via `ctrl` and flows
    /// into the block's body (whose first op takes this node as
    /// its ctrl input). Every `HirOp::Skip` inside the body — and
    /// the natural fall-through — merges at a `BlockExit` node.
    ///
    /// Required for correct MIR lowering: `Mir::Skip` only
    /// unwinds to the nearest enclosing `Mir::Block`. The HIR
    /// inliner rewrites each inlined callee's early returns as
    /// `Skip` inside a `Block`, so losing block scope here would
    /// change `Skip` semantics to "halt the function".
    Block {
        ctrl: NodeId,
    },

    /// Merge point for every path that exits a `Block` scope —
    /// both natural fall-through and `Skip`. Shape is the same as
    /// `Region`; the distinct variant lets the scheduler emit
    /// `Mir::Block(...)` around the body and `Mir::Skip` on any
    /// interior path that reaches here.
    BlockExit {
        preds: Vec<NodeId>,
        /// The owning `Block` node — scheduler uses this to map
        /// `BlockExit` back to its scope marker.
        block: NodeId,
    },

    /// Merge point for every `Break` inside a `Loop`. Same shape
    /// as `Region`, with a back-reference to the owning loop so
    /// the scheduler can identify Break targets reliably without
    /// heuristics on the control subgraph.
    LoopExit {
        preds: Vec<NodeId>,
        loop_node: NodeId,
    },

    /// Data phi. `region` points at the controlling `Region` or
    /// `Loop`; `values[i]` is the data flowing in from that region's
    /// `preds[i]`. `None` means "undefined on this pred" — used
    /// during building; fixed up before the graph is considered
    /// complete.
    Phi {
        region: NodeId,
        values: Vec<Option<NodeId>>,
    },

    /// Effect phi. Same shape as `Phi` but for effect tokens.
    EffPhi {
        region: NodeId,
        effs: Vec<Option<NodeId>>,
    },

    // ---- branches ----
    /// Two-way branch. Expose true/false via `IfTrue`/`IfFalse`
    /// projections.
    If {
        ctrl: NodeId,
        cond: NodeId,
    },

    /// N-way branch on a u4 scrutinee. `arm_values[i]` is the set
    /// of scrutinee values routing to `MatchArm(i)`. Arm sets are
    /// disjoint; together they cover 0..=15.
    Match {
        ctrl: NodeId,
        scrut: NodeId,
        arm_values: Vec<Vec<u8>>,
    },

    /// Projection. Single-output view of a specific slot on a
    /// multi-output producer.
    Proj {
        of: NodeId,
        kind: ProjKind,
    },

    // ---- pure data ----
    /// Literal u4 constant. Pure, no inputs.
    Const(u8),

    /// u4 increment (mod 16). Pure.
    Inc(NodeId),
    /// u4 decrement (mod 16). Pure.
    Dec(NodeId),
    /// u4 addition (mod 16). Pure. Introduced by later rewrites;
    /// HIR-gen never produces this directly.
    Add(NodeId, NodeId),
    /// u4 subtraction (mod 16). Pure.
    Sub(NodeId, NodeId),
    /// u4 equality. Produces `0` (false) or `1` (true). Pure.
    /// Introduced by loop-pattern rewrites in M6.
    Eq(NodeId, NodeId),

    // ---- effectful leaves ----
    /// Read VM register. Outputs effect + value via `Proj::Eff`
    /// and `Proj::ReadRegVal`.
    ReadReg {
        ctrl: NodeId,
        eff: NodeId,
        reg: u8,
    },

    /// Write VM register. Outputs new effect via `Proj::Eff`.
    WriteReg {
        ctrl: NodeId,
        eff: NodeId,
        reg: u8,
        val: NodeId,
    },

    /// Unresolved function call. Resolved to a concrete graph
    /// during inlining (M4). Outputs effect + each return value.
    Call {
        ctrl: NodeId,
        eff: NodeId,
        target: FnRef,
        args: Vec<NodeId>,
        ret_count: u32,
    },

    /// Placeholder for a killed slot. Kept so `NodeId`s stay
    /// stable and dangling edges become obvious.
    Dead,
}

impl NodeKind {
    /// Collect every input `NodeId` (data + control + effect) in
    /// a stable order. Allocates; fine for small graphs and
    /// occasional passes, rewrite this to an iterator once we hit
    /// a profile spike.
    pub fn inputs(&self) -> Vec<NodeId> {
        let mut v = Vec::new();
        match self {
            NodeKind::Start | NodeKind::Const(_) | NodeKind::Dead => {}
            NodeKind::Return { ctrl, eff, values } => {
                v.push(*ctrl);
                v.push(*eff);
                v.extend(values);
            }
            NodeKind::Stop { ctrl, eff } => {
                v.push(*ctrl);
                v.push(*eff);
            }
            NodeKind::Region { preds } => {
                v.extend(preds);
            }
            NodeKind::Loop { entry, back } => {
                v.push(*entry);
                if let Some(b) = back {
                    v.push(*b);
                }
            }
            NodeKind::Block { ctrl } => {
                v.push(*ctrl);
            }
            NodeKind::BlockExit { preds, block } => {
                v.extend(preds);
                v.push(*block);
            }
            NodeKind::LoopExit { preds, loop_node } => {
                v.extend(preds);
                v.push(*loop_node);
            }
            NodeKind::Phi { region, values } => {
                v.push(*region);
                v.extend(values.iter().filter_map(|x| *x));
            }
            NodeKind::EffPhi { region, effs } => {
                v.push(*region);
                v.extend(effs.iter().filter_map(|x| *x));
            }
            NodeKind::If { ctrl, cond } => {
                v.push(*ctrl);
                v.push(*cond);
            }
            NodeKind::Match { ctrl, scrut, .. } => {
                v.push(*ctrl);
                v.push(*scrut);
            }
            NodeKind::Proj { of, .. } => {
                v.push(*of);
            }
            NodeKind::Inc(a) | NodeKind::Dec(a) => {
                v.push(*a);
            }
            NodeKind::Add(a, b) | NodeKind::Sub(a, b) | NodeKind::Eq(a, b) => {
                v.push(*a);
                v.push(*b);
            }
            NodeKind::ReadReg { ctrl, eff, .. } => {
                v.push(*ctrl);
                v.push(*eff);
            }
            NodeKind::WriteReg { ctrl, eff, val, .. } => {
                v.push(*ctrl);
                v.push(*eff);
                v.push(*val);
            }
            NodeKind::Call { ctrl, eff, args, .. } => {
                v.push(*ctrl);
                v.push(*eff);
                v.extend(args);
            }
        }
        v
    }

    /// Short human-readable tag for printing.
    pub fn tag(&self) -> &'static str {
        match self {
            NodeKind::Start => "Start",
            NodeKind::Return { .. } => "Return",
            NodeKind::Stop { .. } => "Stop",
            NodeKind::Region { .. } => "Region",
            NodeKind::Loop { .. } => "Loop",
            NodeKind::Block { .. } => "Block",
            NodeKind::BlockExit { .. } => "BlockExit",
            NodeKind::LoopExit { .. } => "LoopExit",
            NodeKind::Phi { .. } => "Phi",
            NodeKind::EffPhi { .. } => "EffPhi",
            NodeKind::If { .. } => "If",
            NodeKind::Match { .. } => "Match",
            NodeKind::Proj { .. } => "Proj",
            NodeKind::Const(_) => "Const",
            NodeKind::Inc(_) => "Inc",
            NodeKind::Dec(_) => "Dec",
            NodeKind::Add(..) => "Add",
            NodeKind::Sub(..) => "Sub",
            NodeKind::Eq(..) => "Eq",
            NodeKind::ReadReg { .. } => "ReadReg",
            NodeKind::WriteReg { .. } => "WriteReg",
            NodeKind::Call { .. } => "Call",
            NodeKind::Dead => "Dead",
        }
    }

    /// Does this node produce an effect token (directly or through
    /// a `Proj::Eff`)? Effectful nodes can't be reordered or deduped
    /// by pure-value rewrites.
    pub fn is_effectful(&self) -> bool {
        matches!(
            self,
            NodeKind::ReadReg { .. }
                | NodeKind::WriteReg { .. }
                | NodeKind::Call { .. }
                | NodeKind::Stop { .. }
                | NodeKind::Return { .. }
        )
    }

    /// Does this node participate in the control subgraph?
    pub fn is_control(&self) -> bool {
        matches!(
            self,
            NodeKind::Start
                | NodeKind::Region { .. }
                | NodeKind::Loop { .. }
                | NodeKind::Block { .. }
                | NodeKind::BlockExit { .. }
                | NodeKind::LoopExit { .. }
                | NodeKind::If { .. }
                | NodeKind::Match { .. }
                | NodeKind::Stop { .. }
                | NodeKind::Return { .. }
                | NodeKind::ReadReg { .. }
                | NodeKind::WriteReg { .. }
                | NodeKind::Call { .. }
        )
    }
}

/// One node in the arena: its kind plus a reverse-edge list of
/// consumers that helps rewrites find who needs rewiring when a
/// node is replaced.
#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub users: Vec<NodeId>,
}

/// Structural key for a pure-data node, used by GVN. Two nodes
/// with identical keys are guaranteed to compute the same value,
/// so the second one is redundant — the cache hands back the id
/// of the first. Keys only cover pure ops; effectful /
/// control-flow nodes are always allocated fresh because they
/// have position (e.g. two `WriteReg` writes aren't the same even
/// with the same reg+val).
///
/// Commutative ops (`Add`, `Eq`) canonicalise their operand order
/// so `Add(a, b)` and `Add(b, a)` share a slot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PureKey {
    Const(u8),
    Inc(NodeId),
    Dec(NodeId),
    Add(NodeId, NodeId),
    Sub(NodeId, NodeId),
    Eq(NodeId, NodeId),
}

/// Per-function graph: an arena of nodes plus the `Start` id.
#[derive(Debug, Clone)]
pub struct Graph {
    nodes: Vec<Option<Node>>,
    start: NodeId,
    sig: typer::FlatSig,
    /// GVN cache for pure nodes. Keyed by the op's shape; value
    /// is the id of the (first) node that realised that shape.
    /// Populated by `alloc_const` / `alloc_inc` / etc; plain
    /// `alloc` bypasses it (for effectful and control nodes).
    pure_cache: HashMap<PureKey, NodeId>,
}

impl Graph {
    /// Fresh graph pre-populated with a `Start` node.
    pub fn new(sig: typer::FlatSig) -> Self {
        let mut g = Self {
            nodes: Vec::new(),
            start: NodeId::INVALID,
            sig,
            pure_cache: HashMap::new(),
        };
        let start = g.alloc(NodeKind::Start);
        g.start = start;
        g
    }

    /// GVN + const-fold allocation for a u4 constant.
    /// Same-valued constants share a single node across the
    /// whole function — every `Const(0)` for zero-init reads
    /// collapses to one id.
    pub fn alloc_const(&mut self, v: u8) -> NodeId {
        let v = v & 0x0F;
        let key = PureKey::Const(v);
        if let Some(&id) = self.pure_cache.get(&key) {
            return id;
        }
        let id = self.alloc(NodeKind::Const(v));
        self.pure_cache.insert(key, id);
        id
    }

    /// GVN + const-fold for u4 increment. Folds `Inc(Const(n))`
    /// to `Const((n+1) & 0xF)`.
    pub fn alloc_inc(&mut self, a: NodeId) -> NodeId {
        if let Some(v) = self.as_const(a) {
            return self.alloc_const((v + 1) & 0x0F);
        }
        let key = PureKey::Inc(a);
        if let Some(&id) = self.pure_cache.get(&key) {
            return id;
        }
        let id = self.alloc(NodeKind::Inc(a));
        self.pure_cache.insert(key, id);
        id
    }

    /// GVN + const-fold for u4 decrement.
    pub fn alloc_dec(&mut self, a: NodeId) -> NodeId {
        if let Some(v) = self.as_const(a) {
            return self.alloc_const(v.wrapping_sub(1) & 0x0F);
        }
        let key = PureKey::Dec(a);
        if let Some(&id) = self.pure_cache.get(&key) {
            return id;
        }
        let id = self.alloc(NodeKind::Dec(a));
        self.pure_cache.insert(key, id);
        id
    }

    /// GVN + const-fold for u4 add. Commutative: canonicalise
    /// operand order so `Add(a, b)` and `Add(b, a)` share.
    pub fn alloc_add(&mut self, a: NodeId, b: NodeId) -> NodeId {
        if let (Some(va), Some(vb)) = (self.as_const(a), self.as_const(b)) {
            return self.alloc_const((va + vb) & 0x0F);
        }
        let (a, b) = if a.0 <= b.0 { (a, b) } else { (b, a) };
        let key = PureKey::Add(a, b);
        if let Some(&id) = self.pure_cache.get(&key) {
            return id;
        }
        let id = self.alloc(NodeKind::Add(a, b));
        self.pure_cache.insert(key, id);
        id
    }

    /// GVN + const-fold for u4 sub. Not commutative.
    pub fn alloc_sub(&mut self, a: NodeId, b: NodeId) -> NodeId {
        if let (Some(va), Some(vb)) = (self.as_const(a), self.as_const(b)) {
            return self.alloc_const(va.wrapping_sub(vb) & 0x0F);
        }
        let key = PureKey::Sub(a, b);
        if let Some(&id) = self.pure_cache.get(&key) {
            return id;
        }
        let id = self.alloc(NodeKind::Sub(a, b));
        self.pure_cache.insert(key, id);
        id
    }

    /// GVN + const-fold for equality. Commutative.
    pub fn alloc_eq(&mut self, a: NodeId, b: NodeId) -> NodeId {
        if let (Some(va), Some(vb)) = (self.as_const(a), self.as_const(b)) {
            return self.alloc_const(if va == vb { 1 } else { 0 });
        }
        // x == x folds to `true`.
        if a == b {
            return self.alloc_const(1);
        }
        let (a, b) = if a.0 <= b.0 { (a, b) } else { (b, a) };
        let key = PureKey::Eq(a, b);
        if let Some(&id) = self.pure_cache.get(&key) {
            return id;
        }
        let id = self.alloc(NodeKind::Eq(a, b));
        self.pure_cache.insert(key, id);
        id
    }

    /// If `id` is a `Const(v)`, return `v`.
    pub fn as_const(&self, id: NodeId) -> Option<u8> {
        match self.nodes.get(id.index()).and_then(|s| s.as_ref()) {
            Some(Node {
                kind: NodeKind::Const(v),
                ..
            }) => Some(*v),
            _ => None,
        }
    }

    /// Read-only accessor.
    pub fn sig(&self) -> &typer::FlatSig {
        &self.sig
    }

    /// The `Start` node id. Never mutates.
    pub fn start(&self) -> NodeId {
        self.start
    }

    /// Number of arena slots, including dead ones. Use `live_len`
    /// for the count of still-present nodes.
    pub fn arena_len(&self) -> usize {
        self.nodes.len()
    }

    /// Count of live (non-None, non-Dead) nodes.
    pub fn live_len(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| matches!(n, Some(Node { kind, .. }) if !matches!(kind, NodeKind::Dead)))
            .count()
    }

    /// Iterate every live `(NodeId, &Node)` pair in arena order.
    pub fn iter(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter().enumerate().filter_map(|(i, slot)| {
            let node = slot.as_ref()?;
            if matches!(node.kind, NodeKind::Dead) {
                return None;
            }
            Some((NodeId(i as u32), node))
        })
    }

    /// Look up a node. Panics on invalid / killed ids — that's a
    /// bug in the caller, not a user-visible error.
    pub fn get(&self, id: NodeId) -> &Node {
        self.nodes
            .get(id.index())
            .and_then(|s| s.as_ref())
            .unwrap_or_else(|| panic!("soir: get({}) on empty slot", id))
    }

    /// Direct mutable access to the arena slot — escape hatch for
    /// rewrites that need to update `users` on a node after edit
    /// without immediately re-borrowing through `get_mut`. Prefer
    /// `get_mut` for normal work.
    pub fn slot_mut(&mut self, id: NodeId) -> &mut Option<Node> {
        &mut self.nodes[id.index()]
    }

    /// Look up a node mutably. Same panic policy.
    pub fn get_mut(&mut self, id: NodeId) -> &mut Node {
        self.nodes
            .get_mut(id.index())
            .and_then(|s| s.as_mut())
            .unwrap_or_else(|| panic!("soir: get_mut({}) on empty slot", id))
    }

    /// Is this id pointing at a live, non-`Dead` node?
    pub fn is_live(&self, id: NodeId) -> bool {
        if !id.is_valid() {
            return false;
        }
        matches!(
            self.nodes.get(id.index()),
            Some(Some(Node { kind, .. })) if !matches!(kind, NodeKind::Dead)
        )
    }

    /// Add a new node. Records the new id on every input's `users`
    /// list so downstream rewrites can find consumers cheaply.
    pub fn alloc(&mut self, kind: NodeKind) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        let inputs = kind.inputs();
        self.nodes.push(Some(Node {
            kind,
            users: Vec::new(),
        }));
        for input in inputs {
            if input.is_valid() {
                if let Some(Some(n)) = self.nodes.get_mut(input.index()) {
                    n.users.push(id);
                }
            }
        }
        id
    }

    /// Turn a node into `Dead`. Removes this node from its inputs'
    /// `users`. Does not touch `users` of this node — the caller is
    /// responsible for ensuring nobody still points at it.
    ///
    /// Panics if anyone still consumes this node (use `replace_all_uses`
    /// first).
    pub fn kill(&mut self, id: NodeId) {
        let old_inputs = self.get(id).kind.inputs();
        assert!(
            self.get(id).users.is_empty(),
            "soir: kill({}) while users remain: {:?}",
            id,
            self.get(id).users
        );
        for inp in old_inputs {
            if let Some(Some(n)) = self.nodes.get_mut(inp.index()) {
                n.users.retain(|u| *u != id);
            }
        }
        if let Some(slot) = self.nodes.get_mut(id.index()) {
            if let Some(n) = slot.as_mut() {
                n.kind = NodeKind::Dead;
            }
        }
    }

    /// Rewrite every use of `from` to point at `to`. Leaves `from`
    /// alive with an empty user list; the caller typically kills it
    /// afterwards. Used by rewrites (M5+).
    pub fn replace_all_uses(&mut self, from: NodeId, to: NodeId) {
        if from == to {
            return;
        }
        let users = std::mem::take(&mut self.get_mut(from).users);
        for u in &users {
            self.rewrite_input(*u, from, to);
            if let Some(Some(n)) = self.nodes.get_mut(to.index()) {
                n.users.push(*u);
            }
        }
    }

    /// Replace every occurrence of `from` with `to` in one node's
    /// input edges. Does NOT update user lists.
    fn rewrite_input(&mut self, node: NodeId, from: NodeId, to: NodeId) {
        let kind = std::mem::replace(&mut self.get_mut(node).kind, NodeKind::Dead);
        let new_kind = rewrite_kind_inputs(kind, from, to);
        self.get_mut(node).kind = new_kind;
    }

    /// Set the backedge on a `Loop`. Called by the builder when it
    /// finishes emitting the loop body. Panics if the loop already
    /// has a backedge or if `loop_id` isn't a `Loop`.
    pub fn set_loop_back(&mut self, loop_id: NodeId, back: NodeId) {
        let n = self.get_mut(loop_id);
        match &mut n.kind {
            NodeKind::Loop { back: slot, .. } => {
                assert!(slot.is_none(), "soir: loop {} already has a backedge", loop_id);
                *slot = Some(back);
            }
            other => panic!("soir: set_loop_back on non-Loop {}: {:?}", loop_id, other),
        }
        if let Some(Some(n)) = self.nodes.get_mut(back.index()) {
            n.users.push(loop_id);
        }
    }
}

/// Swap every occurrence of `from` with `to` inside a `NodeKind`.
fn rewrite_kind_inputs(kind: NodeKind, from: NodeId, to: NodeId) -> NodeKind {
    let swap = |id: NodeId| if id == from { to } else { id };
    match kind {
        NodeKind::Start | NodeKind::Const(_) | NodeKind::Dead => kind,
        NodeKind::Return { ctrl, eff, values } => NodeKind::Return {
            ctrl: swap(ctrl),
            eff: swap(eff),
            values: values.into_iter().map(swap).collect(),
        },
        NodeKind::Stop { ctrl, eff } => NodeKind::Stop {
            ctrl: swap(ctrl),
            eff: swap(eff),
        },
        NodeKind::Region { preds } => NodeKind::Region {
            preds: preds.into_iter().map(swap).collect(),
        },
        NodeKind::Loop { entry, back } => NodeKind::Loop {
            entry: swap(entry),
            back: back.map(swap),
        },
        NodeKind::Block { ctrl } => NodeKind::Block { ctrl: swap(ctrl) },
        NodeKind::BlockExit { preds, block } => NodeKind::BlockExit {
            preds: preds.into_iter().map(swap).collect(),
            block: swap(block),
        },
        NodeKind::LoopExit { preds, loop_node } => NodeKind::LoopExit {
            preds: preds.into_iter().map(swap).collect(),
            loop_node: swap(loop_node),
        },
        NodeKind::Phi { region, values } => NodeKind::Phi {
            region: swap(region),
            values: values.into_iter().map(|v| v.map(swap)).collect(),
        },
        NodeKind::EffPhi { region, effs } => NodeKind::EffPhi {
            region: swap(region),
            effs: effs.into_iter().map(|v| v.map(swap)).collect(),
        },
        NodeKind::If { ctrl, cond } => NodeKind::If {
            ctrl: swap(ctrl),
            cond: swap(cond),
        },
        NodeKind::Match {
            ctrl,
            scrut,
            arm_values,
        } => NodeKind::Match {
            ctrl: swap(ctrl),
            scrut: swap(scrut),
            arm_values,
        },
        NodeKind::Proj { of, kind } => NodeKind::Proj { of: swap(of), kind },
        NodeKind::Inc(a) => NodeKind::Inc(swap(a)),
        NodeKind::Dec(a) => NodeKind::Dec(swap(a)),
        NodeKind::Add(a, b) => NodeKind::Add(swap(a), swap(b)),
        NodeKind::Sub(a, b) => NodeKind::Sub(swap(a), swap(b)),
        NodeKind::Eq(a, b) => NodeKind::Eq(swap(a), swap(b)),
        NodeKind::ReadReg { ctrl, eff, reg } => NodeKind::ReadReg {
            ctrl: swap(ctrl),
            eff: swap(eff),
            reg,
        },
        NodeKind::WriteReg {
            ctrl,
            eff,
            reg,
            val,
        } => NodeKind::WriteReg {
            ctrl: swap(ctrl),
            eff: swap(eff),
            reg,
            val: swap(val),
        },
        NodeKind::Call {
            ctrl,
            eff,
            target,
            args,
            ret_count,
        } => NodeKind::Call {
            ctrl: swap(ctrl),
            eff: swap(eff),
            target,
            args: args.into_iter().map(swap).collect(),
            ret_count,
        },
    }
}

/// Program: a map from function signature to its graph. Mirrors
/// `hir::HirFunction` keyed by `typer::FnSig` so driver-level code
/// can swap the two representations with minimal plumbing.
#[derive(Debug, Clone, Default)]
pub struct Program {
    pub graphs: HashMap<FnKey, Graph>,
}

impl Program {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key: FnKey, graph: Graph) {
        self.graphs.insert(key, graph);
    }

    pub fn get(&self, key: &FnKey) -> Option<&Graph> {
        self.graphs.get(key)
    }

    pub fn len(&self) -> usize {
        self.graphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.graphs.is_empty()
    }
}
