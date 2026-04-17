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
        // The only "true" natives are the VM-level primitives: the
        // System register ops (there is literally no way to implement them
        // in user code) and Array, whose layout/addressing is under the
        // compiler's direct control. Operators (`+`, `-`, `==`, ...) live
        // in the stdlib — see `examples/new_syntax/std/Ops.ct` and their
        // impls in `std/U4.ct`.
        matches!(
            (type_name, method),
            ("System", "setRegister")
            | ("System", "getRegister")
            | ("System", "debug")
            | ("System", "debugType")
            | ("Array", "len")
            | ("Array", "set")
            | ("Array", "get")
            | ("Array", "setDyn")
            | ("Array", "getDyn")
        )
    }

    fn generate(&self, call: NativeCall<'_>, emitter: &mut NativeEmitter<'_>) -> Result<(), String> {
        match (call.type_name, call.method) {
            ("System", "setRegister") => emit_system_set_register(&call, emitter),
            ("System", "getRegister") => emit_system_get_register(&call, emitter),
            ("System", "debug") | ("System", "debugType") => Ok(()),

            ("Array", "len") => emit_array_len(&call, emitter),
            ("Array", "set") => emit_array_set_static(&call, emitter),
            ("Array", "get") => emit_array_get_static(&call, emitter),
            ("Array", "setDyn") => emit_array_set_dyn(&call, emitter),
            ("Array", "getDyn") => emit_array_get_dyn(&call, emitter),
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

// ---- Array ---------------------------------------------------------------

/// Extract `(element_size, array_size)` from the receiver's template args
/// `[T, Size, F]` using the registry for sizing of T.
fn array_geometry(call: &NativeCall<'_>) -> Result<(u32, u32), String> {
    if call.receiver_type_args.len() < 2 {
        return Err(format!(
            "Array::{}: need concrete Array<T, Size, F> on receiver",
            call.method
        ));
    }
    let t_name = match &call.receiver_type_args[0] {
        ConcreteTemplateArg::Type(t) => t.name.clone(),
        _ => return Err(format!("Array::{}: element type is not a type", call.method)),
    };
    let size = match &call.receiver_type_args[1] {
        ConcreteTemplateArg::Value(n) => *n as u32,
        _ => return Err(format!("Array::{}: array size is not a constant", call.method)),
    };
    let elem_size = size_of_type_name(&t_name, /* array_override */ None, call)?;
    Ok((elem_size, size))
}

fn size_of_type_name(
    name: &str,
    array_override: Option<u32>,
    call: &NativeCall<'_>,
) -> Result<u32, String> {
    if let Some(v) = array_override {
        return Ok(v);
    }
    let info = call
        .registry
        .types
        .get(name)
        .ok_or_else(|| format!("unknown type `{}`", name))?;
    match &info.kind {
        typer::TypeKind::Primitive { size } => Ok(*size),
        typer::TypeKind::Struct(typer::StructKind::Concrete(l)) => Ok(l.size),
        typer::TypeKind::Enum(typer::EnumKind::Concrete(l)) => Ok(l.total_size()),
        _ => Err(format!("type `{}` has no concrete size yet", name)),
    }
}

#[allow(dead_code)]
fn _ensure_emitter_used(_e: &NativeEmitter<'_>) {}

fn emit_array_len(call: &NativeCall<'_>, emitter: &mut NativeEmitter<'_>) -> Result<(), String> {
    // Array::len(): writes the array size into `_ret`, split nibble-style
    // across as many cells as F (the index type) uses.
    if call.receiver_type_args.len() < 3 {
        return Err("Array::len: receiver must be Array<T, Size, F>".to_string());
    }
    let size = match &call.receiver_type_args[1] {
        ConcreteTemplateArg::Value(n) => *n as u32,
        _ => return Err("Array::len: size is not a constant".to_string()),
    };
    // Write `size` in base-16 into the ret cells (low nibble first).
    let mut n = size;
    for slot in call.ret_slots {
        emitter.emit(HirOp::Set(*slot, (n % 16) as u8));
        n /= 16;
    }
    if n != 0 {
        return Err(format!(
            "Array::len: size {} doesn't fit in {} cells",
            size,
            call.ret_slots.len()
        ));
    }
    Ok(())
}

fn array_static_index(call: &NativeCall<'_>) -> Result<u32, String> {
    match call.template_args.first() {
        Some(ConcreteTemplateArg::Value(n)) => Ok(*n as u32),
        _ => Err(format!(
            "Array::{}: expected a single integer template argument",
            call.method
        )),
    }
}

fn emit_array_set_static(
    call: &NativeCall<'_>,
    emitter: &mut NativeEmitter<'_>,
) -> Result<(), String> {
    let (elem_size, max) = array_geometry(call)?;
    let index = array_static_index(call)?;
    if index >= max {
        return Err(format!(
            "Array::set<{}>: index out of bounds (size = {})",
            index, max
        ));
    }
    // Receiver slots: start at `arg_slots[0]`, extending `max * elem_size`.
    let self_start = call.arg_slots.first().copied().ok_or_else(|| {
        "Array::set: missing receiver".to_string()
    })?;
    let value_start = call
        .arg_slots
        .get(call.receiver_cell_count as usize)
        .copied()
        .ok_or_else(|| "Array::set: missing value arg".to_string())?;
    for i in 0..elem_size {
        emitter.emit(HirOp::Copy(
            SlotId(self_start.0 + index * elem_size + i),
            SlotId(value_start.0 + i),
        ));
    }
    Ok(())
}

fn emit_array_get_static(
    call: &NativeCall<'_>,
    emitter: &mut NativeEmitter<'_>,
) -> Result<(), String> {
    let (elem_size, max) = array_geometry(call)?;
    let index = array_static_index(call)?;
    if index >= max {
        return Err(format!(
            "Array::get<{}>: index out of bounds (size = {})",
            index, max
        ));
    }
    let self_start = call.arg_slots.first().copied().ok_or_else(|| {
        "Array::get: missing receiver".to_string()
    })?;
    let dst_start = call
        .ret_slots
        .first()
        .copied()
        .ok_or_else(|| "Array::get: missing return slot".to_string())?;
    for i in 0..elem_size {
        emitter.emit(HirOp::Copy(
            SlotId(dst_start.0 + i),
            SlotId(self_start.0 + index * elem_size + i),
        ));
    }
    Ok(())
}

fn emit_array_set_dyn(
    call: &NativeCall<'_>,
    emitter: &mut NativeEmitter<'_>,
) -> Result<(), String> {
    // setDyn(index, value): pattern-match on index_cell[0] for each position.
    //   For 1-cell index: single Match with `max` arms.
    //   For multi-cell index (rare in practice): fall back to nested If0 per cell.
    let (elem_size, max) = array_geometry(call)?;
    let idx_cells = index_type_size(call)?;
    let self_start = call.arg_slots.first().copied().ok_or_else(|| {
        "Array::setDyn: missing receiver".to_string()
    })?;
    let index_start = SlotId(call.arg_slots[call.receiver_cell_count as usize].0);
    let value_start = SlotId(
        call.arg_slots[call.receiver_cell_count as usize + idx_cells as usize].0,
    );

    if idx_cells == 1 {
        let mut arms: Vec<(HirBlock, Vec<u8>)> = Vec::with_capacity(max as usize);
        for i in 0..max {
            let mut block = HirBlock::new();
            for k in 0..elem_size {
                block.ops.push(HirOp::Copy(
                    SlotId(self_start.0 + i * elem_size + k),
                    SlotId(value_start.0 + k),
                ));
            }
            arms.push((block, vec![i as u8]));
        }
        emitter.emit(HirOp::Match(index_start, arms));
        return Ok(());
    }
    Err(format!(
        "Array::setDyn: multi-cell index types (got {} cells) not yet supported",
        idx_cells
    ))
}

fn emit_array_get_dyn(
    call: &NativeCall<'_>,
    emitter: &mut NativeEmitter<'_>,
) -> Result<(), String> {
    let (elem_size, max) = array_geometry(call)?;
    let idx_cells = index_type_size(call)?;
    let self_start = call.arg_slots.first().copied().ok_or_else(|| {
        "Array::getDyn: missing receiver".to_string()
    })?;
    let index_start = SlotId(call.arg_slots[call.receiver_cell_count as usize].0);
    let dst_start = call
        .ret_slots
        .first()
        .copied()
        .ok_or_else(|| "Array::getDyn: missing return slot".to_string())?;

    if idx_cells == 1 {
        let mut arms: Vec<(HirBlock, Vec<u8>)> = Vec::with_capacity(max as usize);
        for i in 0..max {
            let mut block = HirBlock::new();
            for k in 0..elem_size {
                block.ops.push(HirOp::Copy(
                    SlotId(dst_start.0 + k),
                    SlotId(self_start.0 + i * elem_size + k),
                ));
            }
            arms.push((block, vec![i as u8]));
        }
        emitter.emit(HirOp::Match(index_start, arms));
        return Ok(());
    }
    Err(format!(
        "Array::getDyn: multi-cell index types (got {} cells) not yet supported",
        idx_cells
    ))
}

fn index_type_size(call: &NativeCall<'_>) -> Result<u32, String> {
    let f = &call.receiver_type_args[2];
    match f {
        ConcreteTemplateArg::Type(t) => size_of_type_name(&t.name, None, call),
        ConcreteTemplateArg::Value(_) => Err("Array index-type slot must be a type".to_string()),
    }
}
