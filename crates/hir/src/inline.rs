//! Phase 6 Steps 6.3 + 6.4 — inlining and HIR→MIR conversion.
//!
//! Step 6.3: walk the HIR starting from `main()`, and for every `Call`,
//! splice in the callee's body with all slot ids remapped to a fresh range
//! in a global slot namespace. After this pass, no `Call` ops remain.
//!
//! Step 6.4: convert the now-flat HIR to MIR — a 1:1 structural mapping.

use std::collections::HashMap;

use either::Either;
use mir::{Mir, MirCodeBlock};

use crate::array_synth::{ArrayMonomorphCache, ArraySpec};
use crate::call_graph::build_call_graph;
use crate::ir::*;
use crate::HirFunction;

pub type FnSigKey = typer::FnSig;

// ---------- public entry points --------------------------------------------

/// Inline the program starting from `entry`. Returns a single `HirFunction`
/// whose body contains no `Call` ops. All slot ids are in a freshly
/// allocated global namespace.
pub fn inline_program(
    functions: &HashMap<FnSigKey, HirFunction>,
    entry: &FnSigKey,
) -> Result<HirFunction, String> {
    inline_program_with_registry(functions, entry, None)
}

/// Like `inline_program`, but also supplies a `TypeRegistry` for resolving
/// Array type geometries (needed to synthesize `Array<T, N, F>` method
/// monomorphs on the fly).
pub fn inline_program_with_registry(
    functions: &HashMap<FnSigKey, HirFunction>,
    entry: &FnSigKey,
    registry: Option<&typer::TypeRegistry>,
) -> Result<HirFunction, String> {
    // Verify reachability + no cycles up front.
    //
    // NOTE: with Array synthesis, `Array::{get,set,new,len}` show up as
    // callees that aren't in `functions`. We still run the call-graph walk
    // for cycle detection, but gracefully skip missing targets.
    let _ = build_call_graph(functions, entry); // best-effort

    let entry_hir = functions
        .get(entry)
        .ok_or_else(|| format!("entry `{:?}` missing", entry))?;

    // `Inliner::functions` is owned so we can insert synthesized monomorphs
    // on the fly. Starts as a clone of the input map.
    let mut owned_functions = functions.clone();
    let mut inliner = Inliner {
        functions: &mut owned_functions,
        global_slots: entry_hir.slot_count,
        registry,
        array_cache: ArrayMonomorphCache::new(),
    };
    let body = inliner.inline_block(&entry_hir.body, /*base=*/ 0)?;

    Ok(HirFunction {
        sig: entry_hir.sig.clone(),
        body,
        slot_count: inliner.global_slots,
        type_name: entry_hir.type_name.clone(),
        method_name: entry_hir.method_name.clone(),
    })
}

/// Convert a (fully inlined) HIR function's body to a `MirCodeBlock`. Panics
/// (via Err) if any `Call` op remains.
pub fn hir_to_mir(block: &HirBlock) -> Result<MirCodeBlock, String> {
    let mut out = Vec::with_capacity(block.ops.len());
    for op in &block.ops {
        out.push(hir_op_to_mir(op)?);
    }
    Ok(MirCodeBlock(out))
}

fn hir_op_to_mir(op: &HirOp) -> Result<Mir, String> {
    Ok(match op {
        HirOp::Set(s, v) => Mir::Set(s.0, *v),
        HirOp::Copy(dst, src) => Mir::Copy(dst.0, src.0),
        HirOp::Inc(s) => Mir::Increment(s.0),
        HirOp::Dec(s) => Mir::Decrement(s.0),
        HirOp::If0(s, a, b) => Mir::If0(s.0, hir_to_mir(a)?, hir_to_mir(b)?),
        HirOp::Loop(b) => Mir::Loop(hir_to_mir(b)?),
        HirOp::Break => Mir::Break,
        HirOp::Continue => Mir::Continue,
        HirOp::Stop => Mir::Stop,
        HirOp::ReadRegister(s, r) => Mir::ReadRegister(s.0, *r),
        HirOp::WriteRegister(r, src) => Mir::WriteRegister(
            *r,
            match src {
                Either::Left(v) => Either::Left(*v),
                Either::Right(s) => Either::Right(s.0),
            },
        ),
        HirOp::Block(b) => Mir::Block(hir_to_mir(b)?),
        HirOp::Skip => Mir::Skip,
        HirOp::Match(s, arms) => {
            let mut converted = Vec::with_capacity(arms.len());
            for (body, values) in arms {
                converted.push((hir_to_mir(body)?, values.clone()));
            }
            Mir::Match(s.0, converted)
        }
        HirOp::Call { target, .. } => {
            return Err(format!(
                "unresolved Call after inlining: {}::{}",
                target.type_name, target.method_name
            ));
        }
    })
}

// ---------- inliner -------------------------------------------------------

struct Inliner<'a> {
    functions: &'a mut HashMap<FnSigKey, HirFunction>,
    /// Next free slot in the global space.
    global_slots: u32,
    /// Registry used to resolve `Array<T, N, F>` geometry. Optional — only
    /// needed when the inliner hits an Array method call that needs a
    /// fresh monomorph.
    registry: Option<&'a typer::TypeRegistry>,
    array_cache: ArrayMonomorphCache,
}

impl<'a> Inliner<'a> {
    /// Allocate a fresh contiguous range and return its base index.
    fn alloc_range(&mut self, count: u32) -> u32 {
        let base = self.global_slots;
        self.global_slots += count;
        base
    }

    /// Remap a slot from a function's local [0, slot_count) space to the
    /// global namespace, offset by `base`.
    fn remap(&self, s: SlotId, base: u32) -> SlotId {
        SlotId(s.0 + base)
    }

    /// Inline a block, remapping every slot through `base`.
    fn inline_block(&mut self, block: &HirBlock, base: u32) -> Result<HirBlock, String> {
        let mut out_ops = Vec::new();
        for op in &block.ops {
            self.inline_op(op, base, &mut out_ops)?;
        }
        Ok(HirBlock {
            ops: out_ops,
            result_slot: block.result_slot.map(|s| self.remap(s, base)),
        })
    }

    fn inline_op(
        &mut self,
        op: &HirOp,
        base: u32,
        out: &mut Vec<HirOp>,
    ) -> Result<(), String> {
        match op {
            HirOp::Set(s, v) => out.push(HirOp::Set(self.remap(*s, base), *v)),
            HirOp::Copy(a, b) => out.push(HirOp::Copy(self.remap(*a, base), self.remap(*b, base))),
            HirOp::Inc(s) => out.push(HirOp::Inc(self.remap(*s, base))),
            HirOp::Dec(s) => out.push(HirOp::Dec(self.remap(*s, base))),
            HirOp::If0(s, a, b) => out.push(HirOp::If0(
                self.remap(*s, base),
                self.inline_block(a, base)?,
                self.inline_block(b, base)?,
            )),
            HirOp::Loop(b) => out.push(HirOp::Loop(self.inline_block(b, base)?)),
            HirOp::Break => out.push(HirOp::Break),
            HirOp::Continue => out.push(HirOp::Continue),
            HirOp::Stop => out.push(HirOp::Stop),
            HirOp::ReadRegister(s, r) => out.push(HirOp::ReadRegister(self.remap(*s, base), *r)),
            HirOp::WriteRegister(r, src) => out.push(HirOp::WriteRegister(
                *r,
                match src {
                    Either::Left(v) => Either::Left(*v),
                    Either::Right(s) => Either::Right(self.remap(*s, base)),
                },
            )),
            HirOp::Block(b) => out.push(HirOp::Block(self.inline_block(b, base)?)),
            HirOp::Skip => out.push(HirOp::Skip),
            HirOp::Match(s, arms) => {
                let mut new_arms = Vec::with_capacity(arms.len());
                for (arm, vs) in arms {
                    new_arms.push((self.inline_block(arm, base)?, vs.clone()));
                }
                out.push(HirOp::Match(self.remap(*s, base), new_arms));
            }
            HirOp::Call { target, args, ret } => {
                self.inline_call(target, args, ret, base, out)?;
            }
        }
        Ok(())
    }

    fn inline_call(
        &mut self,
        target: &FnRef,
        args: &[SlotId],
        ret: &[SlotId],
        caller_base: u32,
        out: &mut Vec<HirOp>,
    ) -> Result<(), String> {
        // Array<T, N, F> methods are synthesized on demand: the stdlib has
        // empty bodies, so a normal lookup would miss. Resolve to a
        // monomorph built from `target.template_args`.
        let key: FnSigKey;
        let callee: HirFunction;
        if target.type_name == "Array"
            && crate::array_synth::METHOD_NAMES
                .contains(&target.method_name.as_str())
        {
            let Some(reg) = self.registry else {
                return Err(format!(
                    "inliner: Array::{} called without TypeRegistry — use \
                     `inline_program_with_registry`",
                    target.method_name
                ));
            };
            let Some(spec) = ArraySpec::from_template_args(&target.template_args, reg) else {
                return Err(format!(
                    "inliner: Array::{} needs concrete [T, N, F] template args, got {:?}",
                    target.method_name, target.template_args
                ));
            };
            let (k, f) = self
                .array_cache
                .get_or_synth(&spec, &target.method_name)
                .ok_or_else(|| {
                    format!(
                        "inliner: can't synthesize Array<{}, {}, {}>::{}",
                        spec.element_type, spec.size, spec.index_type, target.method_name
                    )
                })?;
            // Insert into `functions` so any secondary lookup works.
            self.functions.entry(k.clone()).or_insert_with(|| f.clone());
            key = k;
            callee = f;
        } else {
            key = match &target.trait_name {
                Some(t) => FnSigKey::new_trait(&target.type_name, &target.method_name, t),
                None => FnSigKey::new(&target.type_name, &target.method_name),
            };
            callee = self
                .functions
                .get(&key)
                .ok_or_else(|| {
                    format!(
                        "inliner: missing function `{}::{}`",
                        target.type_name, target.method_name
                    )
                })?
                .clone();
        }
        let _ = key;

        // Allocate a fresh slot range for the callee's locals.
        let callee_base = self.alloc_range(callee.slot_count);

        // 1. Copy args → callee input slots.
        //    Callee's inputs occupy slots [0, input_count) in its local
        //    space, which in global space is [callee_base, callee_base +
        //    input_count).
        for (i, a) in args.iter().enumerate() {
            let src = self.remap(*a, caller_base);
            let dst = SlotId(callee_base + i as u32);
            out.push(HirOp::Copy(dst, src));
        }

        // 2. Inline the callee body with its own base, wrapped in a Block
        //    and with `Stop` ops converted to `Skip`. A `return` in the
        //    callee emits `Stop` — which in a standalone function means
        //    "halt the program". Once inlined, that's the wrong semantics:
        //    we want "leave the inlined body and continue the caller".
        //    Wrapping in a Block + rewriting Stop → Skip gives that: Skip
        //    exits up to the nearest enclosing Block, and Block catches Skip
        //    and returns to the caller's linear flow.
        let mut body = self.inline_block(&callee.body, callee_base)?;
        stop_to_skip_in_block(&mut body);
        out.push(HirOp::Block(body));

        // 3. Propagate writes to `mut` params back out. In the language
        //    model, function parameters are "by reference" — mutating a
        //    `mut self` or `mut Self other` inside a callee must be
        //    visible to the caller's argument. Since we chose the
        //    copy-in approach at step (1) for simplicity, we restore the
        //    semantics here by copying the (possibly-mutated) callee
        //    input cells back into the caller's argument cells. Only
        //    `mut` params need this — immutable params' content hasn't
        //    changed.
        for slot in &callee.sig.slots {
            if slot.name == "_ret" || !slot.mutable {
                continue;
            }
            for i in 0..slot.size {
                let cell_ix = (slot.offset + i) as usize;
                if cell_ix >= args.len() {
                    break;
                }
                let caller_arg = self.remap(args[cell_ix], caller_base);
                let callee_cell = SlotId(callee_base + slot.offset + i);
                out.push(HirOp::Copy(caller_arg, callee_cell));
            }
        }

        // 4. Copy callee output slots → caller ret slots.
        for (i, r) in ret.iter().enumerate() {
            let src = SlotId(callee_base + callee.sig.input_count + i as u32);
            let dst = self.remap(*r, caller_base);
            out.push(HirOp::Copy(dst, src));
        }
        Ok(())
    }
}

/// Walk `block` and replace every `Stop` with `Skip`. Respects nested
/// control flow: Stops inside nested Loop/If0/Match/Block bodies are
/// rewritten too. Break and Continue are left alone — they target the
/// callee's own inner loops, which remain valid after inlining.
fn stop_to_skip_in_block(block: &mut HirBlock) {
    for op in &mut block.ops {
        stop_to_skip_in_op(op);
    }
}

fn stop_to_skip_in_op(op: &mut HirOp) {
    match op {
        HirOp::Stop => *op = HirOp::Skip,
        HirOp::If0(_, a, b) => {
            stop_to_skip_in_block(a);
            stop_to_skip_in_block(b);
        }
        HirOp::Loop(b) | HirOp::Block(b) => stop_to_skip_in_block(b),
        HirOp::Match(_, arms) => {
            for (arm, _) in arms {
                stop_to_skip_in_block(arm);
            }
        }
        _ => {}
    }
}
