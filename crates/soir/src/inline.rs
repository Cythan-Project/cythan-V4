//! SoN-level inliner.
//!
//! Splices a callee's graph into a caller's graph at each `Call`
//! node. Runs AFTER per-function SoN translation + optimization
//! so each callee's body has already been shrunk by M5/M6 rewrites
//! — inlining just duplicates the already-small version into
//! each call site instead of re-optimising per site.
//!
//! # Resolution
//!
//! `Call { target: FnRef, ... }` is mapped to a callee graph via
//! `FnRef → FnSig` (dropping template args — spec-monomorph has
//! already mangled variant names into keys by this point). Calls
//! whose callee isn't in the program (templated generics that
//! weren't spec-mono'd, Array::get synthesis, etc.) stay as-is
//! and cause the scheduler to panic — those paths still need to
//! route through the HIR inliner until SoN gains Array synth.
//!
//! # Splicing
//!
//! For a `Call { ctrl, eff, target, args, ret_count }` and its
//! `Proj::Eff` / `Proj::CallRet(i)` projections:
//!
//! 1. Walk every live callee node except `Start`; allocate a
//!    twin in the caller, recording the old→new id mapping. The
//!    twin's inputs are rewritten: references to the callee's
//!    `Proj::StartCtrl` / `Proj::StartEff` / `Proj::Param(i)`
//!    become `call.ctrl` / `call.eff` / `call.args[i]`.
//! 2. Find the callee's single `Return { ctrl, eff, values }`.
//!    Its ctrl becomes the callee's "output control". Rewire:
//!     * every caller node that had `call` as its ctrl → point
//!       at callee's remapped `return.ctrl` instead
//!     * uses of `Proj::Eff { of: call }` → callee's remapped
//!       `return.eff`
//!     * uses of `Proj::CallRet(i) { of: call }` → callee's
//!       remapped `return.values[i]`
//! 3. Kill the callee's copied Return, the Call, and its
//!    projections.
//!
//! The resulting graph has the callee's body spliced in place of
//! the Call, with effect + control threaded through correctly.

use std::collections::HashMap;

use hir::ir::FnRef;

use crate::ir::{FnKey, Graph, Node, NodeId, NodeKind, Program, ProjKind};

/// Fallback strategy when a `Call`'s callee isn't in the
/// `Program`. The driver can plug in an Array-method synthesizer
/// or any other on-demand graph generator by providing a
/// closure. Returning `None` falls back to the `MissingCallee`
/// error.
pub trait CalleeResolver {
    fn resolve(&mut self, fn_ref: &FnRef) -> Option<Graph>;
}

impl<F> CalleeResolver for F
where
    F: FnMut(&FnRef) -> Option<Graph>,
{
    fn resolve(&mut self, fn_ref: &FnRef) -> Option<Graph> {
        self(fn_ref)
    }
}

/// No-op resolver — always returns `None`, so the inliner
/// reports `MissingCallee` for any call not pre-loaded into the
/// program.
struct NoResolver;
impl CalleeResolver for NoResolver {
    fn resolve(&mut self, _: &FnRef) -> Option<Graph> {
        None
    }
}

/// Error surfacing from the inliner.
#[derive(Debug, Clone)]
pub enum InlineError {
    /// The Call's target couldn't be mapped to a callee graph.
    /// Usually a templated-generic or Array method that spec-
    /// mono didn't produce a concrete variant for.
    MissingCallee { type_name: String, method_name: String },
    /// Callee graph had no `Return` — malformed input.
    CalleeMissingReturn(FnKey),
}

impl std::fmt::Display for InlineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InlineError::MissingCallee { type_name, method_name } => write!(
                f,
                "soir inliner: no callee graph for `{}::{}`",
                type_name, method_name
            ),
            InlineError::CalleeMissingReturn(k) => {
                write!(f, "soir inliner: callee `{:?}` has no Return", k)
            }
        }
    }
}

/// Fully flatten the entry function by inlining every resolvable
/// Call. Returns the flattened graph, leaving the program's
/// other entries untouched. Equivalent to
/// `inline_program_with_resolver` with a no-op resolver.
pub fn inline_program(program: &Program, entry: &FnKey) -> Result<Graph, InlineError> {
    inline_program_with_resolver(program, entry, &mut NoResolver)
}

/// Same as `inline_program` but consults `resolver` whenever a
/// Call's callee can't be found in the program directly. The
/// driver uses this to synthesise `Array::new` / `get` / `set` /
/// `len` on demand via `hir::array_synth`, matching what the
/// classical HIR inliner does internally. Synthesised graphs are
/// cached inside `resolver`; each FnRef is resolved at most once
/// across the whole inlining fixpoint.
pub fn inline_program_with_resolver(
    program: &Program,
    entry: &FnKey,
    resolver: &mut dyn CalleeResolver,
) -> Result<Graph, InlineError> {
    let entry_graph = program
        .get(entry)
        .ok_or_else(|| InlineError::MissingCallee {
            type_name: entry.type_name.clone(),
            method_name: entry.method_name.clone(),
        })?
        .clone();
    let mut flat = entry_graph;
    let mut resolved_cache: HashMap<
        (String, String, Option<String>, Vec<hir::ConcreteTemplateArg>),
        Graph,
    > = HashMap::new();
    // Safety rails: any well-formed program's call graph should
    // fully flatten in O(number_of_calls) iterations. Hitting
    // these ceilings means the inliner is introducing work it
    // can't retire — a bug, not a huge-program case.
    const MAX_INLINE_STEPS: usize = 10_000;
    const MAX_ARENA_SIZE: usize = 500_000;
    let mut steps = 0usize;
    loop {
        steps += 1;
        if steps > MAX_INLINE_STEPS {
            return Err(InlineError::MissingCallee {
                type_name: format!(
                    "<inline fixpoint exceeded {} iterations — likely an \
                     inliner bug duplicating work>",
                    MAX_INLINE_STEPS
                ),
                method_name: format!("arena_size={}", flat.arena_len()),
            });
        }
        if flat.arena_len() > MAX_ARENA_SIZE {
            return Err(InlineError::MissingCallee {
                type_name: format!(
                    "<arena exceeded {} nodes during inlining — aborting \
                     before RAM runaway>",
                    MAX_ARENA_SIZE
                ),
                method_name: format!("steps={}", steps),
            });
        }
        let Some(call) = first_call(&flat) else {
            return Ok(flat);
        };
        let before = flat.arena_len();
        let call_target_dbg = match &flat.get(call).kind {
            NodeKind::Call { target, .. } => {
                format!("{}::{}", target.type_name, target.method_name)
            }
            _ => String::new(),
        };
        inline_one_call(&mut flat, call, program, resolver, &mut resolved_cache)?;
        let after = flat.arena_len();
        if std::env::var("CYTHAN_SOIR_INLINE_TRACE").is_ok() {
            eprintln!(
                "[soir inline step {}] {} arena {} → {} (+{})",
                steps, call_target_dbg, before, after, after - before
            );
        }
    }
}

/// First live `Call` node in the graph, in arena order.
fn first_call(g: &Graph) -> Option<NodeId> {
    for (id, n) in g.iter() {
        if matches!(n.kind, NodeKind::Call { .. }) {
            return Some(id);
        }
    }
    None
}

fn inline_one_call(
    caller: &mut Graph,
    call: NodeId,
    program: &Program,
    resolver: &mut dyn CalleeResolver,
    resolved_cache: &mut HashMap<
        (String, String, Option<String>, Vec<hir::ConcreteTemplateArg>),
        Graph,
    >,
) -> Result<(), InlineError> {
    // Snapshot the Call's inputs — we'll replace the call node
    // with the callee's body, so its own fields go away.
    let (call_ctrl, call_eff, call_target, call_args) = match &caller.get(call).kind {
        NodeKind::Call {
            ctrl, eff, target, args, ..
        } => (*ctrl, *eff, target.clone(), args.clone()),
        k => panic!("soir::inline: inline_one_call on non-Call {:?}", k),
    };

    let callee_key = FnKey {
        type_name: call_target.type_name.clone(),
        method_name: call_target.method_name.clone(),
        trait_name: call_target.trait_name.clone(),
    };
    // Try the program first; fall back to the resolver (e.g.
    // Array-method synthesis). Resolved graphs are cached per
    // FnRef so multiple call sites reuse one synth.
    let callee_owned: Graph = if let Some(g) = program.get(&callee_key) {
        g.clone()
    } else {
        let resolver_key = (
            call_target.type_name.clone(),
            call_target.method_name.clone(),
            call_target.trait_name.clone(),
            call_target.template_args.clone(),
        );
        if !resolved_cache.contains_key(&resolver_key) {
            let Some(g) = resolver.resolve(&call_target) else {
                return Err(InlineError::MissingCallee {
                    type_name: call_target.type_name.clone(),
                    method_name: call_target.method_name.clone(),
                });
            };
            resolved_cache.insert(resolver_key.clone(), g);
        }
        resolved_cache.get(&resolver_key).unwrap().clone()
    };
    let callee: &Graph = &callee_owned;

    // Allocate a `Block` scope wrapping the inlined callee body.
    // This is what lets the scheduler correctly emit `Mir::Skip`
    // for each arm that exits via the callee's `return` — without
    // the wrapping block, arms "escape" through the Return's
    // merge region and the scheduler duplicates the caller's
    // post-Call code into every callee arm.
    let block_node = caller.alloc(NodeKind::Block { ctrl: call_ctrl });

    // Build the old→new id map. Start and the three kinds of
    // start projection point directly at caller values: StartCtrl
    // redirects to the wrapping `Block` (so the body's first op
    // consumes Block as ctrl), StartEff to the caller's effect
    // token, Param(i) to the caller's call-site argument.
    let mut remap: HashMap<NodeId, NodeId> = HashMap::new();
    remap.insert(callee.start(), NodeId::INVALID);

    for (id, n) in callee.iter() {
        if let NodeKind::Proj { of, kind } = &n.kind {
            if *of == callee.start() {
                let target_in_caller = match kind {
                    ProjKind::StartCtrl => block_node,
                    ProjKind::StartEff => call_eff,
                    ProjKind::Param(i) => *call_args
                        .get(*i as usize)
                        .expect("soir::inline: Param index out of range"),
                    _ => continue,
                };
                remap.insert(id, target_in_caller);
            }
        }
    }

    // Copy each remaining live callee node into the caller.
    // Skip Start and the pre-mapped start projections — their
    // consumers already resolve to caller values via `remap`.
    //
    // Two-pass: first allocate fresh slots for every node so we
    // can rewrite inputs (including forward references via loop
    // backedges) without worrying about ordering; then patch
    // inputs in a second pass.
    let mut to_copy: Vec<NodeId> = Vec::new();
    for (id, _) in callee.iter() {
        if remap.contains_key(&id) {
            continue; // Start or pre-mapped proj
        }
        to_copy.push(id);
    }
    // Pass 1: allocate placeholder nodes with fresh ids so
    // `remap` covers every callee node before we start wiring.
    // Place a dummy `Const(0)`; we'll overwrite in pass 2.
    for id in &to_copy {
        let new_id = caller.alloc(NodeKind::Const(0));
        remap.insert(*id, new_id);
    }
    // Pass 2: overwrite each placeholder with the real kind,
    // with inputs remapped to caller ids.
    for id in &to_copy {
        let new_id = remap[id];
        let kind = callee.get(*id).kind.clone();
        let remapped = rewrite_kind_with_remap(kind, &remap);
        overwrite_kind(caller, new_id, remapped);
    }

    // Locate the callee's `Return` in caller-id space. Extract
    // its ctrl, eff, and values to feed into the wrapping
    // Block's exit.
    let ret_in_callee = callee
        .iter()
        .find_map(|(id, n)| match &n.kind {
            NodeKind::Return { .. } => Some(id),
            _ => None,
        })
        .ok_or_else(|| InlineError::CalleeMissingReturn(callee_key.clone()))?;
    let ret_in_caller = remap[&ret_in_callee];
    let (ret_ctrl, ret_eff, ret_values) = match &caller.get(ret_in_caller).kind {
        NodeKind::Return { ctrl, eff, values } => (*ctrl, *eff, values.clone()),
        _ => unreachable!(),
    };

    // Build a `BlockExit` merging every path that reaches the
    // callee's Return. If the callee had a single exit pred
    // (e.g. natural fall-through only), `ret_ctrl` is that pred
    // directly; otherwise it's a plain `Region` whose preds we
    // reuse.
    let exit_preds: Vec<NodeId> = match &caller.get(ret_ctrl).kind {
        NodeKind::Region { preds } => preds.clone(),
        _ => vec![ret_ctrl],
    };
    let block_exit = caller.alloc(NodeKind::BlockExit {
        preds: exit_preds.clone(),
        block: block_node,
    });

    // Re-point any `Phi { region: ret_ctrl_region }` to point at
    // `block_exit` so their pred order stays aligned with the new
    // merge. Same for `EffPhi`. We do this by rewriting the
    // region's uses — Phis that referenced it now reference the
    // BlockExit.
    if matches!(caller.get(ret_ctrl).kind, NodeKind::Region { .. }) {
        let users = caller.get(ret_ctrl).users.clone();
        for u in users {
            let kind_ref = caller.get(u).kind.clone();
            let is_phi_here = matches!(
                kind_ref,
                NodeKind::Phi { region, .. } if region == ret_ctrl
            ) || matches!(
                kind_ref,
                NodeKind::EffPhi { region, .. } if region == ret_ctrl
            );
            if is_phi_here {
                let new_kind = crate::ir::rewrite_kind_inputs_pub(
                    kind_ref, ret_ctrl, block_exit,
                );
                caller.get_mut(u).kind = new_kind;
                caller.get_mut(ret_ctrl).users.retain(|x| *x != u);
                caller.get_mut(block_exit).users.push(u);
            }
        }
    }

    // Rewire: anything that consumed the Call as ctrl now
    // consumes the wrapping Block's exit region. Includes
    // `Region` / `BlockExit` / `LoopExit` with the Call in
    // `preds` (they can legitimately appear when a previous
    // inline threaded control through this Call).
    replace_uses_predicate(caller, call, block_exit, |k, from| {
        matches!(k, NodeKind::Region { preds } if preds.contains(from))
            || matches!(k, NodeKind::BlockExit { preds, .. } if preds.contains(from))
            || matches!(k, NodeKind::LoopExit { preds, .. } if preds.contains(from))
            || has_ctrl_field(k, *from)
    });

    // Rewire Call's effect projection → callee's return.eff.
    if let Some(eff_proj) = find_proj(caller, call, &ProjKind::Eff) {
        caller.replace_all_uses(eff_proj, ret_eff);
        caller.kill(eff_proj);
    }
    // Rewire each CallRet(i) → return.values[i].
    if std::env::var("CYTHAN_SOIR_INLINE_DEBUG").is_ok() {
        eprintln!(
            "[inline] {}::{}: ret_values.len()={}, ret_values={:?}",
            call_target.type_name,
            call_target.method_name,
            ret_values.len(),
            ret_values,
        );
        for i in 0..ret_values.len() {
            let proj = find_proj(caller, call, &ProjKind::CallRet(i as u32));
            eprintln!(
                "[inline]   CallRet({}): proj={:?} → {:?}",
                i, proj, ret_values[i],
            );
        }
    }
    for i in 0..ret_values.len() {
        if let Some(ret_proj) =
            find_proj(caller, call, &ProjKind::CallRet(i as u32))
        {
            caller.replace_all_uses(ret_proj, ret_values[i]);
            caller.kill(ret_proj);
        }
    }

    // The callee's Return and its merge region are no longer
    // needed — BlockExit replaced the merge, the Block replaced
    // the scope entry. Kill in safe order.
    caller.kill(ret_in_caller);
    // Kill the callee's old merge region if it's now unused.
    if matches!(caller.get(ret_ctrl).kind, NodeKind::Region { .. })
        && caller.get(ret_ctrl).users.is_empty()
    {
        caller.kill(ret_ctrl);
    }
    // Diagnostic: any call user still remaining here is a soir-
    // inliner bug (we've already rewired ctrl consumers and
    // both Proj kinds). Dump them to make the panic actionable.
    if !caller.get(call).users.is_empty() {
        let users_detail: Vec<String> = caller
            .get(call)
            .users
            .iter()
            .map(|u| {
                let k = &caller.get(*u).kind;
                let preds_str = match k {
                    NodeKind::BlockExit { preds, block } => {
                        format!(" preds={:?} block={:?}", preds, block)
                    }
                    NodeKind::Region { preds } => format!(" preds={:?}", preds),
                    _ => String::new(),
                };
                format!("{}={}{}", u, k.tag(), preds_str)
            })
            .collect();
        panic!(
            "soir inliner: outer Call {} ({}::{}) still has users after rewiring: {:?}. \
             ret_ctrl={} ret_ctrl_kind={:?} block_node={} block_exit={}",
            call,
            call_target.type_name,
            call_target.method_name,
            users_detail,
            ret_ctrl,
            caller.get(ret_ctrl).kind.tag(),
            block_node,
            block_exit,
        );
    }
    caller.kill(call);
    Ok(())
}

/// Does `k` consume `from` as a *control* edge? Projections
/// use their `of` field for BOTH control projections (StartCtrl
/// / IfTrue / IfFalse / MatchArm) and data projections (Eff /
/// ReadRegVal / CallRet). Only the former should rewire here —
/// the latter (Proj::Eff / Proj::CallRet) go through
/// `replace_all_uses` after their target has been resolved.
fn has_ctrl_field(k: &NodeKind, from: NodeId) -> bool {
    match k {
        NodeKind::If { ctrl, .. }
        | NodeKind::Match { ctrl, .. }
        | NodeKind::Block { ctrl }
        | NodeKind::ReadReg { ctrl, .. }
        | NodeKind::WriteReg { ctrl, .. }
        | NodeKind::Call { ctrl, .. }
        | NodeKind::Stop { ctrl, .. }
        | NodeKind::Return { ctrl, .. } => *ctrl == from,
        NodeKind::Loop { entry, back } => *entry == from || *back == Some(from),
        NodeKind::Proj { of, kind } => {
            *of == from
                && matches!(
                    kind,
                    ProjKind::StartCtrl
                        | ProjKind::IfTrue
                        | ProjKind::IfFalse
                        | ProjKind::MatchArm(_)
                )
        }
        _ => false,
    }
}

/// Replace every use of `from` with `to` in nodes whose kind
/// matches `predicate(kind, &from)`. Manual replacement because
/// `Graph::replace_all_uses` handles all users uniformly; here
/// we need the filter.
fn replace_uses_predicate<F>(g: &mut Graph, from: NodeId, to: NodeId, predicate: F)
where
    F: Fn(&NodeKind, &NodeId) -> bool,
{
    let users: Vec<NodeId> = g.get(from).users.clone();
    for u in users {
        let kind = g.get(u).kind.clone();
        if predicate(&kind, &from) {
            let new_kind = crate::ir::rewrite_kind_inputs_pub(kind, from, to);
            // Update users lists.
            let n = g.get_mut(u);
            n.kind = new_kind;
            // Remove this user from `from`'s list.
            g.get_mut(from).users.retain(|x| *x != u);
            // Add to `to`'s list.
            g.get_mut(to).users.push(u);
        }
    }
}

fn rewrite_kind_with_remap(kind: NodeKind, remap: &HashMap<NodeId, NodeId>) -> NodeKind {
    let sub = |id: NodeId| remap.get(&id).copied().unwrap_or(id);
    match kind {
        NodeKind::Start | NodeKind::Const(_) | NodeKind::Dead => kind,
        NodeKind::Return { ctrl, eff, values } => NodeKind::Return {
            ctrl: sub(ctrl),
            eff: sub(eff),
            values: values.into_iter().map(sub).collect(),
        },
        NodeKind::Stop { ctrl, eff } => NodeKind::Stop {
            ctrl: sub(ctrl),
            eff: sub(eff),
        },
        NodeKind::Region { preds } => NodeKind::Region {
            preds: preds.into_iter().map(sub).collect(),
        },
        NodeKind::Loop { entry, back } => NodeKind::Loop {
            entry: sub(entry),
            back: back.map(sub),
        },
        NodeKind::Block { ctrl } => NodeKind::Block { ctrl: sub(ctrl) },
        NodeKind::BlockExit { preds, block } => NodeKind::BlockExit {
            preds: preds.into_iter().map(sub).collect(),
            block: sub(block),
        },
        NodeKind::LoopExit { preds, loop_node } => NodeKind::LoopExit {
            preds: preds.into_iter().map(sub).collect(),
            loop_node: sub(loop_node),
        },
        NodeKind::Phi { region, values } => NodeKind::Phi {
            region: sub(region),
            values: values.into_iter().map(|v| v.map(sub)).collect(),
        },
        NodeKind::EffPhi { region, effs } => NodeKind::EffPhi {
            region: sub(region),
            effs: effs.into_iter().map(|v| v.map(sub)).collect(),
        },
        NodeKind::If { ctrl, cond } => NodeKind::If {
            ctrl: sub(ctrl),
            cond: sub(cond),
        },
        NodeKind::Match {
            ctrl,
            scrut,
            arm_values,
        } => NodeKind::Match {
            ctrl: sub(ctrl),
            scrut: sub(scrut),
            arm_values,
        },
        NodeKind::Proj { of, kind } => NodeKind::Proj { of: sub(of), kind },
        NodeKind::Inc(a) => NodeKind::Inc(sub(a)),
        NodeKind::Dec(a) => NodeKind::Dec(sub(a)),
        NodeKind::Add(a, b) => NodeKind::Add(sub(a), sub(b)),
        NodeKind::Sub(a, b) => NodeKind::Sub(sub(a), sub(b)),
        NodeKind::Eq(a, b) => NodeKind::Eq(sub(a), sub(b)),
        NodeKind::ReadReg { ctrl, eff, reg } => NodeKind::ReadReg {
            ctrl: sub(ctrl),
            eff: sub(eff),
            reg,
        },
        NodeKind::WriteReg {
            ctrl,
            eff,
            reg,
            val,
        } => NodeKind::WriteReg {
            ctrl: sub(ctrl),
            eff: sub(eff),
            reg,
            val: sub(val),
        },
        NodeKind::Call {
            ctrl,
            eff,
            target,
            args,
            ret_count,
        } => NodeKind::Call {
            ctrl: sub(ctrl),
            eff: sub(eff),
            target,
            args: args.into_iter().map(sub).collect(),
            ret_count,
        },
    }
}

/// Overwrite a placeholder node's kind with the final kind, and
/// register it as a user on each input. The placeholder was
/// allocated as `Const(0)` with no inputs, so we don't need to
/// un-register any old users — just add the new ones.
fn overwrite_kind(g: &mut Graph, node: NodeId, new_kind: NodeKind) {
    let inputs = new_kind.inputs();
    g.get_mut(node).kind = new_kind;
    for input in inputs {
        if input.is_valid() && input != node {
            let slot = g.slot_mut(input);
            if let Some(n) = slot.as_mut() {
                n.users.push(node);
            }
        }
    }
}

fn find_proj(g: &Graph, source: NodeId, kind: &ProjKind) -> Option<NodeId> {
    for &u in &g.get(source).users {
        if let NodeKind::Proj { kind: k, .. } = &g.get(u).kind {
            if k == kind {
                return Some(u);
            }
        }
    }
    None
}

// Re-export for `inline::replace_uses_predicate` — keeps the
// builder's swap helper usable from sibling modules without
// making it fully public.
//
// (Implemented in `ir` as a non-public helper; re-exported here
// via the thin `rewrite_kind_inputs_pub` bridge.)
#[allow(dead_code)]
fn _reexport_anchor() -> fn(NodeKind, NodeId, NodeId) -> NodeKind {
    crate::ir::rewrite_kind_inputs_pub
}

// Silence the unused `Node` import warning if the compiler
// decides `Node` isn't reachable via any path in this file.
#[allow(dead_code)]
type _NodeAlias = Node;
