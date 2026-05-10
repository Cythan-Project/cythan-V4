//! HIR → SoN translation.
//!
//! Consumes a `hir::HirFunction` and produces a `Graph`. Uses a
//! flavour of Braun et al.-style on-the-fly SSA: per-slot
//! "current definition" tracked as the builder walks ops. At
//! structured merge points (end of Match, Loop header, Loop exit),
//! phis are inserted for every slot whose definition differs
//! across the merging paths (or every currently-known slot at a
//! Loop header — redundant phis get cleaned up by later GVN/DCE).
//!
//! # Slot semantics
//!
//! HIR slots are mutable cells; SoN values are single-assignment.
//! `slot_def: SlotId -> NodeId` maps each HIR slot to its most
//! recent SSA definition on the currently-open control path.
//! Reads pull through this map; writes update it.
//!
//! # Control paths and termination
//!
//! `ctrl` / `eff` are `Option<NodeId>` so the builder can express
//! "this path has ended" (after `Break` / `Continue` / `Stop`).
//! Once `ctrl` is `None`, subsequent ops in the current block are
//! skipped as dead code.
//!
//! # Loops
//!
//! At loop entry, a `Loop` header is allocated with the current
//! control as its `entry` pred and `None` for the backedge. A phi
//! is placed at the header for every currently-known slot (and one
//! EffPhi for the effect token). The body builds with `ctrl` set
//! to the Loop header and phis as the new slot defs. Each path
//! that eventually re-enters the loop (natural fall-through,
//! `Continue`) contributes to the backedge list; each `Break`
//! contributes to the exit list. After the body, backedge preds
//! merge at a synthetic `Region` that becomes the Loop's `back`;
//! exit preds merge at the post-loop `Region`.

use std::collections::{HashMap, HashSet};

use either::Either;
use hir::ir::FnRef;
use hir::{HirBlock, HirFunction, HirOp, SlotId};

use crate::ir::{Graph, NodeId, NodeKind, Program, ProjKind};

/// Per-callee "which of my input cells do I mutate" info. A
/// callee with `mut self` spanning cells `0..3` returns
/// `vec![0, 1, 2]`. Used to thread mut-param writes through
/// `Call` / `Return` so the caller's view of those cells picks
/// up the callee's writes after inlining.
///
/// Without this, `mut self` methods like `Array::set` lose
/// their writes — HIR-gen leaves the mutated cells out of
/// `Call.ret`, relying on the HIR inliner's back-copy pass.
/// soir tracks values per-function and needs the mutation
/// made explicit as extra `Proj::CallRet` / `Return.values`
/// entries.
pub trait MutCellsResolver {
    fn resolve(&self, fn_ref: &FnRef) -> Vec<u32>;
}

impl<F> MutCellsResolver for F
where
    F: Fn(&FnRef) -> Vec<u32>,
{
    fn resolve(&self, fn_ref: &FnRef) -> Vec<u32> {
        self(fn_ref)
    }
}

/// No-op resolver: treats every call as having zero mut cells.
/// Safe for leaf graphs (no Call ops) or for testing/toy uses
/// where mut-param threading doesn't matter.
struct EmptyResolver;
impl MutCellsResolver for EmptyResolver {
    fn resolve(&self, _: &FnRef) -> Vec<u32> {
        Vec::new()
    }
}

/// Extract the sorted cell indices for every mut-flagged input
/// slot of a flat sig. Skips `_ret` slots (those are outputs,
/// never mutable inputs).
pub fn mut_cells_of_sig(sig: &typer::FlatSig) -> Vec<u32> {
    let mut out = Vec::new();
    let input_count = sig.input_count;
    for slot in &sig.slots {
        if slot.name == "_ret" {
            continue;
        }
        if !slot.mutable {
            continue;
        }
        // Skip slots that overlap output cells (shouldn't happen
        // in practice but guard against a weird sig).
        if slot.offset >= input_count {
            continue;
        }
        for i in 0..slot.size {
            out.push(slot.offset + i);
        }
    }
    out
}

/// Translate every function in `hir_fns` into a `Program` of
/// soir graphs. Uses a default resolver that looks up each
/// `Call` target in `hir_fns` by `FnSig` (stripping
/// `template_args` from the `FnRef`). Calls to functions not
/// in `hir_fns` (notably on-demand `Array` synthesis) get
/// zero mut cells — the driver should use
/// [`translate_program_with_resolver`] and supply an Array-aware
/// resolver for those.
pub fn translate_program(
    hir_fns: &HashMap<typer::FnSig, HirFunction>,
) -> Program {
    let default_resolver = DefaultResolver { hir_fns };
    translate_program_with_resolver(hir_fns, &default_resolver)
}

/// Like `translate_program` but consults `resolver` to resolve
/// each callee's mut cells. The driver uses this to add Array-
/// method handling that computes mut cells from
/// `hir::array_synth::ArraySpec`.
pub fn translate_program_with_resolver(
    hir_fns: &HashMap<typer::FnSig, HirFunction>,
    resolver: &dyn MutCellsResolver,
) -> Program {
    let mut program = Program::new();
    for (sig, func) in hir_fns {
        let graph = translate_function_with_resolver(func, resolver);
        program.insert(sig.clone(), graph);
    }
    program
}

struct DefaultResolver<'a> {
    hir_fns: &'a HashMap<typer::FnSig, HirFunction>,
}

impl<'a> MutCellsResolver for DefaultResolver<'a> {
    fn resolve(&self, fn_ref: &FnRef) -> Vec<u32> {
        let key = typer::FnSig {
            type_name: fn_ref.type_name.clone(),
            method_name: fn_ref.method_name.clone(),
            trait_name: fn_ref.trait_name.clone(),
        };
        match self.hir_fns.get(&key) {
            Some(f) => mut_cells_of_sig(&f.sig),
            None => Vec::new(),
        }
    }
}

/// Translate one HIR function into a soir `Graph`. The result
/// ends with exactly one `Return` node whose `values` list has:
///
///   * the flat signature's output slots in order, followed by
///   * one entry per cell of each `mut`-flagged input slot
///     (in `mut_cells_of_sig` order).
///
/// Both `return expr` (lowered to `Stop` by HIR-gen) and
/// natural fall-through contribute to the merged exit. Each
/// top-level `Stop` captures a snapshot of `(ctrl, eff,
/// slot_def)` and `finish` builds one canonical `Return`.
///
/// This variant uses an empty mut-cells resolver — safe only
/// when the function has no `Call` ops, or when mut-param
/// writes out of callees aren't needed (e.g. the caller doesn't
/// care about the callee's side effects). For the general case
/// use [`translate_function_with_resolver`].
pub fn translate_function(func: &HirFunction) -> Graph {
    translate_function_with_resolver(func, &EmptyResolver)
}

/// Like `translate_function` but consults `resolver` to resolve
/// each callee's mut cells. See [`MutCellsResolver`].
pub fn translate_function_with_resolver(
    func: &HirFunction,
    resolver: &dyn MutCellsResolver,
) -> Graph {
    let mut g = Graph::new(func.sig.clone());
    let mut builder = Builder::new(&mut g, resolver);
    builder.seed_params();
    builder.lower_block(&func.body);
    builder.finish();
    g
}

/// State kept while building one graph. Lives for the duration
/// of one `translate_function` call.
struct Builder<'g> {
    g: &'g mut Graph,
    /// Current control edge. `None` means the path has ended
    /// (dead code region after Break/Continue/Stop/Skip).
    ctrl: Option<NodeId>,
    /// Current effect token. `None` iff `ctrl` is `None`.
    eff: Option<NodeId>,
    /// Live definition per HIR slot on the currently-open path.
    slot_def: HashMap<SlotId, NodeId>,
    /// Stack of enclosing loops. Topmost entry is the innermost
    /// loop; `Break`/`Continue` target it.
    loops: Vec<LoopCtx>,
    /// Stack of enclosing `Block` scopes. Topmost entry is the
    /// innermost Block; `Skip` targets it (exits to the post-
    /// block merge). The HIR inliner produces these by wrapping
    /// each inlined callee in a `Block(... Skip ...)` so early
    /// returns become local exits instead of program halts.
    blocks: Vec<BlockCtx>,
    /// Paths that exited the function body via `Stop` (HIR's
    /// lowering of `return value`). `finish` merges these with
    /// any natural fall-through to build a single `Return`.
    return_preds: Vec<PathSnapshot>,
    /// Resolver for callees' mut cells: supplied by the caller
    /// of `translate_function_with_resolver`. Consulted at each
    /// `HirOp::Call` to place extra `Proj::CallRet` projections
    /// for the callee's mut-param cells.
    mut_resolver: &'g dyn MutCellsResolver,
    /// This function's own mut-param cell list — appended to
    /// `Return.values` at `finish` so the caller can pick up the
    /// mutations via the corresponding `Proj::CallRet` entries.
    own_mut_cells: Vec<u32>,
}

/// Per-loop bookkeeping. One entry is pushed on loop entry and
/// popped after the body + exit merge are wired up.
struct LoopCtx {
    /// The `Loop` header node.
    header: NodeId,
    /// Phis placed at the header, one per slot tracked when the
    /// loop opened (plus more if newly-defined slots show up). The
    /// builder updates the backedge value on the phi when a path
    /// ends at the backedge (fall-through or `Continue`).
    header_phis: HashMap<SlotId, NodeId>,
    /// The EffPhi at the header.
    header_eff_phi: NodeId,
    /// Slots known to the builder at loop entry — used to
    /// initialise new header phis lazily when a fresh slot is
    /// introduced inside the loop body.
    known_at_entry: HashSet<SlotId>,
    /// One entry per path that should feed the loop's backedge:
    /// natural body fall-through + every `Continue`.
    back_preds: Vec<PathSnapshot>,
    /// One entry per `Break` inside this loop.
    break_preds: Vec<PathSnapshot>,
}

/// Captured state of a control path at a merge point: its
/// control + effect tokens plus every slot's current def.
struct PathSnapshot {
    ctrl: NodeId,
    eff: NodeId,
    slot_def: HashMap<SlotId, NodeId>,
}

/// Per-`Block` bookkeeping. Pushed on Block entry, popped after
/// the body + Skip merge are wired up.
struct BlockCtx {
    /// Slots known at Block entry — used so the post-block merge
    /// can emit phis for every slot that might be written on
    /// some path.
    known_at_entry: HashSet<SlotId>,
    /// Paths that exited the Block early via `Skip`. Merged into
    /// the post-block `BlockExit` along with the natural
    /// fall-through.
    skip_preds: Vec<PathSnapshot>,
    /// The `Block` scope marker node. The scheduler uses it
    /// together with the paired `BlockExit` to emit `Mir::Block`.
    #[allow(dead_code)]
    block_node: NodeId,
}

impl<'g> Builder<'g> {
    fn new(g: &'g mut Graph, mut_resolver: &'g dyn MutCellsResolver) -> Self {
        let own_mut_cells = mut_cells_of_sig(g.sig());
        Self {
            g,
            ctrl: None,
            eff: None,
            slot_def: HashMap::new(),
            loops: Vec::new(),
            blocks: Vec::new(),
            return_preds: Vec::new(),
            mut_resolver,
            own_mut_cells,
        }
    }

    /// Create the `Proj::StartCtrl` / `Proj::StartEff` / `Param(i)`
    /// nodes and seed `slot_def` with the per-input projections.
    fn seed_params(&mut self) {
        let start = self.g.start();
        let start_ctrl = self.g.alloc(NodeKind::Proj {
            of: start,
            kind: ProjKind::StartCtrl,
        });
        let start_eff = self.g.alloc(NodeKind::Proj {
            of: start,
            kind: ProjKind::StartEff,
        });
        self.ctrl = Some(start_ctrl);
        self.eff = Some(start_eff);
        // One Param proj per input cell. Flat signature: cells
        // [0..input_count) are inputs, so SlotId(i) for i < input_count
        // is defined by Param(i).
        let sig = self.g.sig().clone();
        for i in 0..sig.input_count {
            let p = self.g.alloc(NodeKind::Proj {
                of: start,
                kind: ProjKind::Param(i),
            });
            self.slot_def.insert(SlotId(i), p);
        }
    }

    /// After lowering the function body, emit a `Return`
    /// consuming the output slots in order. Merges:
    ///   * the natural fall-through (if any — `self.ctrl` still
    ///     live at function-body end), and
    ///   * every path captured in `return_preds` (from
    ///     `HirOp::Stop`s encountered during lowering).
    ///
    /// With both contributions funneled through one `Return`,
    /// every translated function exposes a single, canonical
    /// exit — the soir inliner's splicing logic can assume it.
    fn finish(&mut self) {
        // Fold natural fall-through (if any) into return_preds.
        if let (Some(c), Some(e)) = (self.ctrl.take(), self.eff.take()) {
            let defs = std::mem::take(&mut self.slot_def);
            self.return_preds.push(PathSnapshot {
                ctrl: c,
                eff: e,
                slot_def: defs,
            });
        }
        if self.return_preds.is_empty() {
            // No path reaches the function's exit at all — the
            // body loops forever. A `Return` is impossible; the
            // graph has no canonical exit, and inlining will
            // reject it.
            return;
        }
        let preds = std::mem::take(&mut self.return_preds);
        let entry_defs: HashMap<SlotId, NodeId> = HashMap::new();
        let (ctrl, eff, merged_defs) = self.merge_paths(preds, &entry_defs);
        let sig = self.g.sig().clone();
        let own_mut_cells = self.own_mut_cells.clone();
        let mut values =
            Vec::with_capacity(sig.output_count as usize + own_mut_cells.len());
        for i in 0..sig.output_count {
            let slot = SlotId(sig.input_count + i);
            let v = match merged_defs.get(&slot) {
                Some(v) => *v,
                None => self.g.alloc_const(0),
            };
            values.push(v);
        }
        // Append one entry per mut-input cell so the caller can
        // pick up the mutation via `Proj::CallRet(output_count + k)`.
        // Convention must match the builder's `HirOp::Call`
        // handling, which places caller-side CallRet projs at
        // `ret.len() + k` for each callee mut cell `k`.
        for cell in &own_mut_cells {
            let slot = SlotId(*cell);
            let v = match merged_defs.get(&slot) {
                Some(v) => *v,
                None => self.g.alloc_const(0),
            };
            values.push(v);
        }
        let _ret = self.g.alloc(NodeKind::Return { ctrl, eff, values });
    }

    /// Read the current definition of `slot`. If this slot has
    /// never been defined, create a `Const(0)` — matches the
    /// interpreter's zero-initialisation semantics.
    fn read_slot(&mut self, slot: SlotId) -> NodeId {
        if let Some(&def) = self.slot_def.get(&slot) {
            return def;
        }
        let zero = self.g.alloc_const(0);
        self.slot_def.insert(slot, zero);
        zero
    }

    /// Write a new definition for `slot`.
    fn write_slot(&mut self, slot: SlotId, val: NodeId) {
        self.slot_def.insert(slot, val);
    }

    fn lower_block(&mut self, block: &HirBlock) {
        for op in &block.ops {
            if self.ctrl.is_none() {
                // Dead code after Break/Continue/Stop: skip
                // cleanly rather than erroring. Downstream
                // analysers will never observe these ops.
                break;
            }
            self.lower_op(op);
        }
    }

    fn lower_op(&mut self, op: &HirOp) {
        match op {
            HirOp::Set(dst, v) => {
                let c = self.g.alloc_const(*v);
                self.write_slot(*dst, c);
            }
            HirOp::Copy(dst, src) => {
                let v = self.read_slot(*src);
                self.write_slot(*dst, v);
            }
            HirOp::MapValue(src, dst, table) => {
                let v = self.read_slot(*src);
                // Recognize the canonical inc/dec tables to keep the
                // SoIR cache effective; arbitrary tables fall through
                // (TODO: a NodeKind::Map(NodeId, [u8;16]) for full
                // generality once the SoIR pipeline needs it).
                let new = if *table == hir::ir::INC_TABLE {
                    self.g.alloc_inc(v)
                } else if *table == hir::ir::DEC_TABLE {
                    self.g.alloc_dec(v)
                } else {
                    // Fall back: emit a 16-arm Match-of-Set, then read.
                    // For now, panic — SoIR doesn't yet support general
                    // MapValue tables, and only inc/dec reach this path
                    // through the normal HIR-gen pipeline.
                    panic!(
                        "soir: general MapValue table not yet supported \
                         (src={src:?}, dst={dst:?})"
                    );
                };
                self.write_slot(*dst, new);
            }
            HirOp::Skip => self.lower_skip(),
            HirOp::Block(b) => self.lower_block_scope(b),
            HirOp::Stop => {
                // HIR's `Stop` represents `return` when lowered
                // from `return expr`. Semantically: halt the
                // function and hand control back to the caller
                // with whatever the output slots currently hold.
                //
                // At the soir level this becomes a return
                // predecessor — a (ctrl, eff, slot_def) snapshot
                // that `finish()` will merge into the function's
                // single `Return` node. Without this, callees
                // that return via `return false` / `return x`
                // would have no `Return` at all, and the soir
                // inliner couldn't extract their output values.
                let ctrl = self.ctrl.take().expect("live ctrl before Stop");
                let eff = self.eff.take().expect("live eff before Stop");
                let defs = std::mem::take(&mut self.slot_def);
                self.return_preds.push(PathSnapshot {
                    ctrl,
                    eff,
                    slot_def: defs,
                });
            }
            HirOp::Break => self.lower_break(),
            HirOp::Continue => self.lower_continue(),
            HirOp::ReadRegister(dst, reg) => {
                let ctrl = self.ctrl.expect("live ctrl before ReadRegister");
                let eff = self.eff.expect("live eff before ReadRegister");
                let rr = self.g.alloc(NodeKind::ReadReg {
                    ctrl,
                    eff,
                    reg: *reg,
                });
                let new_eff = self.g.alloc(NodeKind::Proj {
                    of: rr,
                    kind: ProjKind::Eff,
                });
                let val = self.g.alloc(NodeKind::Proj {
                    of: rr,
                    kind: ProjKind::ReadRegVal,
                });
                // Effectful nodes are control-pinned: the next
                // op's control input is the effectful node itself,
                // so the interpreter / scheduler knows it runs in
                // sequence with side-effects.
                self.ctrl = Some(rr);
                self.eff = Some(new_eff);
                self.write_slot(*dst, val);
            }
            HirOp::WriteRegister(reg, src) => {
                let ctrl = self.ctrl.expect("live ctrl before WriteRegister");
                let eff = self.eff.expect("live eff before WriteRegister");
                let val = match src {
                    Either::Left(imm) => self.g.alloc_const(*imm),
                    Either::Right(slot) => self.read_slot(*slot),
                };
                let wr = self.g.alloc(NodeKind::WriteReg {
                    ctrl,
                    eff,
                    reg: *reg,
                    val,
                });
                let new_eff = self.g.alloc(NodeKind::Proj {
                    of: wr,
                    kind: ProjKind::Eff,
                });
                self.ctrl = Some(wr);
                self.eff = Some(new_eff);
            }
            HirOp::Call { target, args, ret } => {
                let ctrl = self.ctrl.expect("live ctrl before Call");
                let eff = self.eff.expect("live eff before Call");
                let arg_vals: Vec<NodeId> =
                    args.iter().map(|a| self.read_slot(*a)).collect();
                // Resolve the callee's mut-cell list so we can
                // emit extra CallRet projs for each mutated input
                // cell. Missing callees (e.g. templated generics
                // not yet monomorphised) get an empty list; that
                // matches the classical pre-mut-threading
                // behaviour and lets the inliner report a crisp
                // `MissingCallee` later.
                let mut_cells: Vec<u32> = self.mut_resolver.resolve(target);
                let call = self.g.alloc(NodeKind::Call {
                    ctrl,
                    eff,
                    target: target.clone(),
                    args: arg_vals,
                    ret_count: (ret.len() + mut_cells.len()) as u32,
                });
                let new_eff = self.g.alloc(NodeKind::Proj {
                    of: call,
                    kind: ProjKind::Eff,
                });
                self.ctrl = Some(call);
                self.eff = Some(new_eff);
                for (i, ret_slot) in ret.iter().enumerate() {
                    let r = self.g.alloc(NodeKind::Proj {
                        of: call,
                        kind: ProjKind::CallRet(i as u32),
                    });
                    self.write_slot(*ret_slot, r);
                }
                // Place one extra CallRet per callee mut cell.
                // Each one writes back to the caller's slot that
                // matches the callee's cell position — `args[c]`
                // is exactly the caller-side SlotId whose value
                // was passed in for callee-cell `c`. The ordering
                // here must match the callee's `finish()`
                // appending mut cells to `Return.values`.
                for (k, cell) in mut_cells.iter().enumerate() {
                    let caller_slot = match args.get(*cell as usize) {
                        Some(s) => *s,
                        None => continue,
                    };
                    let proj_idx = (ret.len() + k) as u32;
                    let r = self.g.alloc(NodeKind::Proj {
                        of: call,
                        kind: ProjKind::CallRet(proj_idx),
                    });
                    self.write_slot(caller_slot, r);
                }
            }
            HirOp::Match(scrut, arms) => self.lower_match(*scrut, arms),
            HirOp::Loop(body) => self.lower_loop(body),
        }
    }

    /// `Skip` exits to the enclosing `Block`'s post-merge region.
    /// Structurally identical to `Break` but targets the
    /// innermost `Block` scope rather than the innermost `Loop`.
    fn lower_skip(&mut self) {
        let ctrl = self.ctrl.take().expect("live ctrl before Skip");
        let eff = self.eff.take().expect("live eff before Skip");
        let defs = std::mem::take(&mut self.slot_def);
        let blk = self
            .blocks
            .last_mut()
            .expect("Skip outside any Block — HIR malformed");
        blk.skip_preds.push(PathSnapshot {
            ctrl,
            eff,
            slot_def: defs,
        });
    }

    /// Lower a `HirOp::Block(body)`. Emits a `Block` scope node
    /// wrapping the body and a `BlockExit` merge that all `Skip`
    /// paths and the natural fall-through feed into. The scope
    /// is essential for MIR — `Mir::Skip` only unwinds to the
    /// nearest `Mir::Block`.
    fn lower_block_scope(&mut self, body: &HirBlock) {
        let entry_ctrl = self.ctrl.expect("live ctrl before Block");
        let entry_eff = self.eff.expect("live eff before Block");
        let entry_defs = self.slot_def.clone();
        let block_node = self.g.alloc(NodeKind::Block { ctrl: entry_ctrl });
        self.blocks.push(BlockCtx {
            known_at_entry: entry_defs.keys().copied().collect(),
            skip_preds: Vec::new(),
            block_node,
        });
        self.ctrl = Some(block_node);
        self.eff = Some(entry_eff);
        self.lower_block(body);

        let mut preds: Vec<PathSnapshot> = Vec::new();
        if let (Some(c), Some(e)) = (self.ctrl.take(), self.eff.take()) {
            let defs = std::mem::take(&mut self.slot_def);
            preds.push(PathSnapshot {
                ctrl: c,
                eff: e,
                slot_def: defs,
            });
        }
        let ctx = self.blocks.pop().expect("block scope stack");
        let _ = ctx.known_at_entry;
        preds.extend(ctx.skip_preds);
        if preds.is_empty() {
            self.ctrl = None;
            self.eff = None;
            self.slot_def.clear();
            return;
        }
        // Build a BlockExit that references the owning Block so
        // the scheduler can pair them up.
        let pred_ctrls: Vec<NodeId> = preds.iter().map(|p| p.ctrl).collect();
        let exit_node = self.g.alloc(NodeKind::BlockExit {
            preds: pred_ctrls,
            block: block_node,
        });
        // EffPhi for effect, Phi per slot — same shape as
        // merge_paths but with a fixed region we've already
        // allocated.
        let exit_eff = if preds.len() == 1 {
            preds[0].eff
        } else {
            self.g.alloc(NodeKind::EffPhi {
                region: exit_node,
                effs: preds.iter().map(|p| Some(p.eff)).collect(),
            })
        };
        let mut all_slots: HashSet<SlotId> = HashSet::new();
        for p in &preds {
            all_slots.extend(p.slot_def.keys().copied());
        }
        let mut merged: HashMap<SlotId, NodeId> = HashMap::new();
        for slot in all_slots {
            let defs: Vec<NodeId> = preds
                .iter()
                .map(|p| {
                    p.slot_def
                        .get(&slot)
                        .copied()
                        .unwrap_or_else(|| self.const_zero())
                })
                .collect();
            let v = if defs.iter().all(|v| *v == defs[0]) {
                defs[0]
            } else {
                self.g.alloc(NodeKind::Phi {
                    region: exit_node,
                    values: defs.into_iter().map(Some).collect(),
                })
            };
            merged.insert(slot, v);
        }
        self.ctrl = Some(exit_node);
        self.eff = Some(exit_eff);
        self.slot_def = merged;
    }

    fn lower_break(&mut self) {
        let ctrl = self.ctrl.take().expect("live ctrl before Break");
        let eff = self.eff.take().expect("live eff before Break");
        let defs = std::mem::take(&mut self.slot_def);
        let lp = self.loops.last_mut().expect("Break outside any loop");
        lp.break_preds.push(PathSnapshot {
            ctrl,
            eff,
            slot_def: defs,
        });
    }

    fn lower_continue(&mut self) {
        let ctrl = self.ctrl.take().expect("live ctrl before Continue");
        let eff = self.eff.take().expect("live eff before Continue");
        let defs = std::mem::take(&mut self.slot_def);
        let lp = self.loops.last_mut().expect("Continue outside any loop");
        lp.back_preds.push(PathSnapshot {
            ctrl,
            eff,
            slot_def: defs,
        });
    }

    fn lower_match(&mut self, scrut: SlotId, arms: &[(HirBlock, Vec<u8>)]) {
        let entry_ctrl = self.ctrl.expect("live ctrl before Match");
        let entry_eff = self.eff.expect("live eff before Match");
        let scrut_val = self.read_slot(scrut);
        let match_node = self.g.alloc(NodeKind::Match {
            ctrl: entry_ctrl,
            scrut: scrut_val,
            arm_values: arms.iter().map(|(_, v)| v.clone()).collect(),
        });
        // Build each arm in its own forked state.
        let entry_defs = self.slot_def.clone();
        let mut falling_through: Vec<PathSnapshot> = Vec::new();
        for (i, (body, _vals)) in arms.iter().enumerate() {
            let arm_ctrl = self.g.alloc(NodeKind::Proj {
                of: match_node,
                kind: ProjKind::MatchArm(i as u32),
            });
            self.ctrl = Some(arm_ctrl);
            self.eff = Some(entry_eff);
            self.slot_def = entry_defs.clone();
            self.lower_block(body);
            // If the arm still has a live path, it falls through
            // into the post-match region.
            if let (Some(c), Some(e)) = (self.ctrl.take(), self.eff.take()) {
                falling_through.push(PathSnapshot {
                    ctrl: c,
                    eff: e,
                    slot_def: std::mem::take(&mut self.slot_def),
                });
            } else {
                // Arm exited via Break/Continue/Stop — no
                // contribution to the post-match merge.
                self.slot_def.clear();
            }
        }
        // Merge fall-through arms at a Region.
        if falling_through.is_empty() {
            // Every arm terminated (break/continue/stop). Post-
            // match is unreachable.
            self.ctrl = None;
            self.eff = None;
            self.slot_def.clear();
            return;
        }
        let (region_ctrl, merged_eff, merged_defs) =
            self.merge_paths(falling_through, &entry_defs);
        self.ctrl = Some(region_ctrl);
        self.eff = Some(merged_eff);
        self.slot_def = merged_defs;
    }

    fn lower_loop(&mut self, body: &HirBlock) {
        let entry_ctrl = self.ctrl.expect("live ctrl before Loop");
        let entry_eff = self.eff.expect("live eff before Loop");
        let header = self.g.alloc(NodeKind::Loop {
            entry: entry_ctrl,
            back: None,
        });
        // Seed a phi at the header for every currently-known
        // slot. Second input left `None`; filled when we resolve
        // the backedge Region below.
        let known_at_entry: HashSet<SlotId> = self.slot_def.keys().copied().collect();
        let mut header_phis: HashMap<SlotId, NodeId> = HashMap::new();
        for (slot, def) in &self.slot_def {
            let phi = self.g.alloc(NodeKind::Phi {
                region: header,
                values: vec![Some(*def), None],
            });
            header_phis.insert(*slot, phi);
        }
        let header_eff_phi = self.g.alloc(NodeKind::EffPhi {
            region: header,
            effs: vec![Some(entry_eff), None],
        });
        // Build body with the header as ctrl + phis as slot defs.
        let body_defs: HashMap<SlotId, NodeId> =
            header_phis.iter().map(|(s, p)| (*s, *p)).collect();
        self.ctrl = Some(header);
        self.eff = Some(header_eff_phi);
        self.slot_def = body_defs;
        let ctx = LoopCtx {
            header,
            header_phis,
            header_eff_phi,
            known_at_entry,
            back_preds: Vec::new(),
            break_preds: Vec::new(),
        };
        self.loops.push(ctx);
        self.lower_block(body);
        // Natural fall-through = another backedge pred.
        if let (Some(c), Some(e)) = (self.ctrl.take(), self.eff.take()) {
            let defs = std::mem::take(&mut self.slot_def);
            self.loops
                .last_mut()
                .unwrap()
                .back_preds
                .push(PathSnapshot {
                    ctrl: c,
                    eff: e,
                    slot_def: defs,
                });
        }
        self.close_loop();
    }

    /// Wire up the loop's backedge and exit regions, then restore
    /// the outer control path.
    fn close_loop(&mut self) {
        let ctx = self.loops.pop().expect("close_loop without matching open");
        let LoopCtx {
            header,
            header_phis,
            header_eff_phi,
            known_at_entry,
            back_preds,
            break_preds,
        } = ctx;

        // --- backedge ------------------------------------------------
        if back_preds.is_empty() {
            // No path re-enters the loop. Body runs once then
            // exits via breaks / stop. Leave Loop.back unset;
            // the scheduler is free to treat this as straight-
            // line code.
        } else {
            // Merge the preds and set Loop.back to the merge.
            // For every slot tracked at the header, resolve its
            // phi's backedge input.
            let back_ctrl = self.merge_ctrl(back_preds.iter().map(|p| p.ctrl));
            let back_eff =
                self.merge_eff(back_ctrl, back_preds.iter().map(|p| p.eff));
            self.g.set_loop_back(header, back_ctrl);
            // Resolve header phis.
            for (slot, phi_id) in &header_phis {
                // Each back-pred contributes its current def for
                // this slot (falling back to entry-time def if
                // unchanged).
                let def_per_pred: Vec<NodeId> = back_preds
                    .iter()
                    .map(|p| {
                        p.slot_def
                            .get(slot)
                            .copied()
                            .unwrap_or_else(|| self.const_zero())
                    })
                    .collect();
                let back_def = self.merge_values(back_ctrl, def_per_pred);
                self.set_phi_back(*phi_id, back_def);
            }
            self.set_eff_phi_back(header_eff_phi, back_eff);
            let _ = known_at_entry; // reserved for future fixups

            // Redundant-phi elision: a header phi whose backedge
            // value equals its entry value is a no-op — the slot
            // is loop-invariant. Rewrite uses of the phi to point
            // at the entry value and kill the phi. This removes
            // hundreds of phis from loops that only mutate a few
            // slots but inherit many live ones at entry.
            let phi_ids: Vec<NodeId> = header_phis.values().copied().collect();
            for phi in phi_ids {
                let (entry_val, back_val) = match &self.g.get(phi).kind {
                    NodeKind::Phi { values, .. } if values.len() == 2 => {
                        (values[0], values[1])
                    }
                    _ => continue,
                };
                if entry_val == back_val && entry_val.is_some() {
                    let entry = entry_val.unwrap();
                    self.g.replace_all_uses(phi, entry);
                    self.g.kill(phi);
                    // Also update outer slot_def entries that may
                    // have cached this phi id.
                    for v in self.slot_def.values_mut() {
                        if *v == phi {
                            *v = entry;
                        }
                    }
                }
            }
        }

        // --- exit ----------------------------------------------------
        if break_preds.is_empty() {
            // Infinite loop. No code runs after it.
            self.ctrl = None;
            self.eff = None;
            self.slot_def.clear();
            return;
        }
        // Merge Break paths at a dedicated `LoopExit`. Using a
        // distinct NodeKind (rather than plain `Region`) lets the
        // scheduler identify Break targets reliably without
        // heuristically scanning the control subgraph.
        let pred_ctrls: Vec<NodeId> = break_preds.iter().map(|p| p.ctrl).collect();
        let exit_region = self.g.alloc(NodeKind::LoopExit {
            preds: pred_ctrls,
            loop_node: header,
        });
        // EffPhi for the exit: one effect per break pred.
        let exit_eff = if break_preds.len() == 1 {
            break_preds[0].eff
        } else {
            self.g.alloc(NodeKind::EffPhi {
                region: exit_region,
                effs: break_preds.iter().map(|p| Some(p.eff)).collect(),
            })
        };
        // Per-slot exit defs: one Phi per slot that differs
        // across break preds.
        let mut all_slots: HashSet<SlotId> = HashSet::new();
        for p in &break_preds {
            all_slots.extend(p.slot_def.keys().copied());
        }
        let mut merged: HashMap<SlotId, NodeId> = HashMap::new();
        for slot in all_slots {
            let defs: Vec<NodeId> = break_preds
                .iter()
                .map(|p| {
                    p.slot_def
                        .get(&slot)
                        .copied()
                        .unwrap_or_else(|| self.const_zero())
                })
                .collect();
            let v = if defs.iter().all(|v| *v == defs[0]) {
                defs[0]
            } else {
                self.g.alloc(NodeKind::Phi {
                    region: exit_region,
                    values: defs.into_iter().map(Some).collect(),
                })
            };
            merged.insert(slot, v);
        }
        self.ctrl = Some(exit_region);
        self.eff = Some(exit_eff);
        self.slot_def = merged;
    }

    /// Merge a list of control preds at a fresh `Region`, unless
    /// there's exactly one pred (pass through).
    fn merge_ctrl<I: Iterator<Item = NodeId>>(&mut self, preds: I) -> NodeId {
        let preds: Vec<NodeId> = preds.collect();
        if preds.len() == 1 {
            return preds[0];
        }
        self.g.alloc(NodeKind::Region { preds })
    }

    /// Build an EffPhi at `region` with one effect per pred.
    fn merge_eff<I: Iterator<Item = NodeId>>(
        &mut self,
        region: NodeId,
        effs: I,
    ) -> NodeId {
        let effs: Vec<Option<NodeId>> = effs.map(Some).collect();
        if effs.len() == 1 {
            return effs[0].unwrap();
        }
        self.g.alloc(NodeKind::EffPhi { region, effs })
    }

    /// Build a Phi at `region` with one value per pred (or pass
    /// through when all preds agree / there's only one).
    fn merge_values(&mut self, region: NodeId, values: Vec<NodeId>) -> NodeId {
        if values.is_empty() {
            return self.const_zero();
        }
        if values.len() == 1 {
            return values[0];
        }
        // Short-cut: if every pred provides the same def, the
        // phi is redundant.
        if values.iter().all(|v| *v == values[0]) {
            return values[0];
        }
        self.g.alloc(NodeKind::Phi {
            region,
            values: values.into_iter().map(Some).collect(),
        })
    }

    /// Merge multiple fall-through arms of a `Match` into a single
    /// post-match state. Returns (region_ctrl, eff, slot_defs).
    fn merge_paths(
        &mut self,
        paths: Vec<PathSnapshot>,
        _entry_defs: &HashMap<SlotId, NodeId>,
    ) -> (NodeId, NodeId, HashMap<SlotId, NodeId>) {
        let region = self.merge_ctrl(paths.iter().map(|p| p.ctrl));
        let eff = self.merge_eff(region, paths.iter().map(|p| p.eff));
        let mut all_slots: HashSet<SlotId> = HashSet::new();
        for p in &paths {
            all_slots.extend(p.slot_def.keys().copied());
        }
        let mut merged: HashMap<SlotId, NodeId> = HashMap::new();
        for slot in all_slots {
            let defs: Vec<NodeId> = paths
                .iter()
                .map(|p| {
                    p.slot_def
                        .get(&slot)
                        .copied()
                        .unwrap_or_else(|| self.const_zero())
                })
                .collect();
            let v = self.merge_values(region, defs);
            merged.insert(slot, v);
        }
        (region, eff, merged)
    }

    fn const_zero(&mut self) -> NodeId {
        self.g.alloc_const(0)
    }

    /// Set the backedge input of a header `Phi`.
    fn set_phi_back(&mut self, phi: NodeId, back: NodeId) {
        match &mut self.g.get_mut(phi).kind {
            NodeKind::Phi { values, .. } => {
                assert_eq!(values.len(), 2, "header phi expected 2 inputs");
                assert!(values[1].is_none(), "backedge already set");
                values[1] = Some(back);
            }
            k => panic!("set_phi_back on non-Phi: {:?}", k),
        }
        if let Some(Some(n)) = self.g_node_slot(back) {
            n.users.push(phi);
        }
    }

    fn set_eff_phi_back(&mut self, phi: NodeId, back: NodeId) {
        match &mut self.g.get_mut(phi).kind {
            NodeKind::EffPhi { effs, .. } => {
                assert_eq!(effs.len(), 2);
                assert!(effs[1].is_none());
                effs[1] = Some(back);
            }
            k => panic!("set_eff_phi_back on non-EffPhi: {:?}", k),
        }
        if let Some(Some(n)) = self.g_node_slot(back) {
            n.users.push(phi);
        }
    }

    /// Direct mutable access to an arena slot. Used when updating
    /// `users` lists from inside the builder without going through
    /// the public `get_mut` (which would re-borrow the graph).
    fn g_node_slot(&mut self, id: NodeId) -> Option<&mut Option<crate::ir::Node>> {
        // Implemented via a small escape hatch on Graph; see
        // `Graph::slot_mut`.
        Some(self.g.slot_mut(id))
    }
}
