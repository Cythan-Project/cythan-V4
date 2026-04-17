//! Phase 7 — native type providers.
//!
//! A `NativeProvider` lets the compiler inline built-in behavior for specific
//! type/method pairs at HIR generation time. Instead of emitting a
//! `HirOp::Call { target, ... }` (to be resolved during inlining), the HIR
//! generator asks the provider whether it "owns" the (type, method) pair,
//! and if so hands off emission. The provider returns the sequence of
//! `HirOp`s to splice in — and may allocate fresh scratch slots through the
//! supplied `NativeEmitter`.
//!
//! Shipping natives:
//!   - `U4::inc` / `U4::dec`                    (Step 7.2)
//!   - `System::setRegister<N>(value)`          (Step 7.3)
//!   - `System::getRegister<N>() -> U4`         (Step 7.3)
//!   - `System::debug<T>(T)` / `System::debugType<T>()` — no-ops at runtime.
//!   - `Array<T, Size, F>::set<N>` / `get<N>` / `setDyn` / `getDyn` / `len`
//!     (Step 7.4) — see module-level note below.
//!
//! ## Array note
//!
//! Array is the hairy one: it's generic over a type `T`, a compile-time
//! integer size, and an index-type `F`. Its layout (`Size * sizeof(T)`) can
//! only be known once the full concrete type has been resolved. In this
//! phase the Array natives are implemented assuming the receiver's concrete
//! template args are already known to the caller (e.g. supplied by the
//! monomorphizer from Phase 6). If the caller lacks the args, the generator
//! returns a clear error rather than silently doing something wrong.

use either::Either;

use crate::ir::*;

/// Emission context for a native. Supplies inputs/outputs, template args,
/// a reference to the type registry (for sizing), and a slot allocator for
/// any scratch the native needs. The native writes emitted ops into `ops`.
pub struct NativeEmitter<'a> {
    pub ops: &'a mut Vec<HirOp>,
    pub registry: &'a typer::TypeRegistry,
    /// Monotonic slot counter shared with the enclosing generator. Natives
    /// that need temporaries call `alloc` to reserve fresh slot IDs.
    pub next_slot: &'a mut u32,
}

impl<'a> NativeEmitter<'a> {
    pub fn emit(&mut self, op: HirOp) {
        self.ops.push(op);
    }

    /// Reserve `count` contiguous fresh slots and return the first one.
    pub fn alloc(&mut self, count: u32) -> SlotId {
        let base = *self.next_slot;
        *self.next_slot += count;
        SlotId(base)
    }
}

/// Everything the HIR generator hands to a native when it resolves a call.
pub struct NativeCall<'a> {
    pub type_name: &'a str,
    pub method: &'a str,
    pub template_args: &'a [ConcreteTemplateArg],
    /// Flat slot list of the receiver (for method calls) followed by args.
    /// Static calls have an empty receiver portion, so this just equals the
    /// args list.
    pub arg_slots: &'a [SlotId],
    pub ret_slots: &'a [SlotId],
    /// For method calls with generic receivers, the concrete template args
    /// on the receiver's type (e.g. `[Cell, 9, U4]` for an `Array<Cell, 9,
    /// U4>` method). Empty for non-generic types or when the caller can't
    /// supply them.
    pub receiver_type_args: &'a [ConcreteTemplateArg],
    /// Number of cells the receiver occupies (`0` for static calls). The
    /// remaining `arg_slots` after this prefix belong to the non-self args.
    pub receiver_cell_count: u32,
    /// Registry passed through for size lookups.
    pub registry: &'a typer::TypeRegistry,
}

/// Strategy interface: a provider knows whether it handles a (type, method)
/// pair and, if so, how to emit the corresponding HIR.
pub trait NativeProvider {
    fn has_method(&self, type_name: &str, method: &str) -> bool;
    fn generate(&self, call: NativeCall<'_>, emitter: &mut NativeEmitter<'_>) -> Result<(), String>;
}

// --------------------------------------------------------------------------
// Default built-in provider.
// --------------------------------------------------------------------------

pub struct BuiltinNatives;

impl BuiltinNatives {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BuiltinNatives {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeProvider for BuiltinNatives {
    fn has_method(&self, type_name: &str, method: &str) -> bool {
        // The only true natives are the System register ops — nothing else
        // can read / write VM registers. Array methods are synthesized by
        // the monomorphizer (`array_synth`); operators live in the stdlib.
        matches!(
            (type_name, method),
            ("System", "setRegister")
            | ("System", "getRegister")
            | ("System", "debug")
            | ("System", "debugType")
        )
    }

    fn generate(&self, call: NativeCall<'_>, emitter: &mut NativeEmitter<'_>) -> Result<(), String> {
        match (call.type_name, call.method) {
            ("System", "setRegister") => emit_system_set_register(&call, emitter),
            ("System", "getRegister") => emit_system_get_register(&call, emitter),
            ("System", "debug") | ("System", "debugType") => Ok(()),
            _ => Err(format!("native {}::{} not implemented", call.type_name, call.method)),
        }
    }
}

// ---- System --------------------------------------------------------------

fn reg_index_from_template(call: &NativeCall<'_>, label: &str) -> Result<u8, String> {
    match call.template_args.first() {
        Some(ConcreteTemplateArg::Value(n)) => {
            if *n < 0 || *n > 3 {
                return Err(format!("{}: register {} out of range [0, 3]", label, n));
            }
            Ok(*n as u8)
        }
        Some(_) | None => Err(format!(
            "{}: expected a single integer template argument",
            label
        )),
    }
}

fn emit_system_set_register(
    call: &NativeCall<'_>,
    emitter: &mut NativeEmitter<'_>,
) -> Result<(), String> {
    let reg = reg_index_from_template(call, "System::setRegister")?;
    // Static call: only an explicit value arg, no receiver cells.
    let value_slot = call
        .arg_slots
        .get(call.receiver_cell_count as usize)
        .copied()
        .ok_or_else(|| "System::setRegister: missing value arg".to_string())?;
    emitter.emit(HirOp::WriteRegister(reg, Either::Right(value_slot)));
    Ok(())
}

fn emit_system_get_register(
    call: &NativeCall<'_>,
    emitter: &mut NativeEmitter<'_>,
) -> Result<(), String> {
    let reg = reg_index_from_template(call, "System::getRegister")?;
    let dst = call
        .ret_slots
        .first()
        .copied()
        .ok_or_else(|| "System::getRegister: missing return slot".to_string())?;
    emitter.emit(HirOp::ReadRegister(dst, reg));
    Ok(())
}

