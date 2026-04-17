//! Synthesize `HirFunction`s for a concrete `Array<T, N, F>`.
//!
//! The stdlib's `Array` methods have empty bodies; the compiler fills them
//! in per concrete instantiation. Each synthesized function uses only
//! `Set`/`Copy`/`Match` — no further `Call`s — so the inliner can splice it
//! in as a terminal unit.

use std::collections::HashMap;

use crate::ir::*;

pub type FnSigKey = typer::FnSig;

/// The geometry of a concrete `Array<T, N, F>`.
#[derive(Debug, Clone)]
pub struct ArraySpec {
    /// Element type (as a concrete type name, e.g. "U4", "Cell").
    pub element_type: String,
    /// Full concrete template args of the element type. Non-empty when
    /// the element is itself a generic instantiation like
    /// `ArrayList<U4, 3, U4>`. Kept separate from `element_type` so the
    /// mangled name can distinguish element-type instantiations of the
    /// same head (critical for nested Array<ArrayList<...>, ...>).
    pub element_args: Vec<ConcreteTemplateArg>,
    pub element_size: u32,
    /// Number of elements in the array.
    pub size: u32,
    /// Index type (e.g. "U4").
    pub index_type: String,
    pub index_size: u32,
}

fn render_concrete_arg(a: &ConcreteTemplateArg) -> String {
    match a {
        ConcreteTemplateArg::Value(n) => n.to_string(),
        ConcreteTemplateArg::Type(t) => render_concrete_type(t),
    }
}

fn render_concrete_type(t: &ConcreteType) -> String {
    if t.args.is_empty() {
        t.name.clone()
    } else {
        format!(
            "{}<{}>",
            t.name,
            t.args
                .iter()
                .map(render_concrete_arg)
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

impl ArraySpec {
    /// Total cell count: `size * element_size`.
    pub fn total_cells(&self) -> u32 {
        self.size * self.element_size
    }

    /// The mangled type name used to key monomorphs, e.g. `"Array<U4,9,U4>"`.
    /// Includes the full element-type instantiation so
    /// `Array<ArrayList<U4, 3, U4>, 2, U4>` and
    /// `Array<ArrayList<U4, 2, Bool>, 2, U4>` key distinctly.
    pub fn mangled_type_name(&self) -> String {
        let elem = if self.element_args.is_empty() {
            self.element_type.clone()
        } else {
            format!(
                "{}<{}>",
                self.element_type,
                self.element_args
                    .iter()
                    .map(render_concrete_arg)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        format!("Array<{},{},{}>", elem, self.size, self.index_type)
    }

    /// Resolve an `ArraySpec` from a three-element template-arg list
    /// `[T, N, F]`. The registry supplies sizes for T and F. Returns
    /// `None` on shape errors (wrong arity, non-concrete args, ...).
    pub fn from_template_args(
        args: &[ConcreteTemplateArg],
        reg: &typer::TypeRegistry,
    ) -> Option<ArraySpec> {
        if args.len() != 3 {
            return None;
        }
        let (element_type, element_args, element_size) = match &args[0] {
            ConcreteTemplateArg::Type(t) => {
                // Route through the typer so generic element types
                // (e.g. `ArrayList<U4, 3, U4>`) size correctly.
                let ast_ty = concrete_to_ast(t);
                let sz = reg.resolve_type_size(&ast_ty, &(0..0)).ok()?;
                (t.name.clone(), t.args.clone(), sz)
            }
            _ => return None,
        };
        let size = match &args[1] {
            ConcreteTemplateArg::Value(n) => (*n).max(0) as u32,
            _ => return None,
        };
        let (index_type, index_size) = match &args[2] {
            ConcreteTemplateArg::Type(t) => {
                let ast_ty = concrete_to_ast(t);
                let sz = reg.resolve_type_size(&ast_ty, &(0..0)).ok()?;
                (t.name.clone(), sz)
            }
            _ => return None,
        };
        Some(ArraySpec {
            element_type,
            element_args,
            element_size,
            size,
            index_type,
            index_size,
        })
    }
}

fn concrete_to_ast(t: &ConcreteType) -> new_parser::ast::Type {
    new_parser::ast::Type {
        name: (t.name.clone(), 0..0),
        templates: t
            .args
            .iter()
            .map(|a| (concrete_arg_to_ast(a), 0..0))
            .collect(),
        qself: None,
    }
}

fn concrete_arg_to_ast(a: &ConcreteTemplateArg) -> new_parser::ast::TypeOrValue {
    match a {
        ConcreteTemplateArg::Type(t) => new_parser::ast::TypeOrValue::Type(concrete_to_ast(t)),
        ConcreteTemplateArg::Value(n) => new_parser::ast::TypeOrValue::Value(*n),
    }
}

#[allow(dead_code)]
fn size_of(reg: &typer::TypeRegistry, name: &str) -> Option<u32> {
    let info = reg.get_type(name)?;
    match &info.kind {
        typer::TypeKind::Primitive { size } => Some(*size),
        typer::TypeKind::Struct(typer::StructKind::Concrete(l)) => Some(l.size),
        typer::TypeKind::Enum(typer::EnumKind::Concrete(l)) => Some(l.total_size()),
        _ => None,
    }
}

/// Names of the methods this module can synthesize.
pub const METHOD_NAMES: &[&str] = &["new", "get", "set", "len"];

/// Cache of already-synthesized monomorphs, keyed by the concrete
/// `Array<T, N, F>` mangled name + method.
#[derive(Default)]
pub struct ArrayMonomorphCache {
    entries: HashMap<FnSigKey, HirFunction>,
}

impl ArrayMonomorphCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up (or synthesize + insert) a monomorph for `Array<spec>::method`.
    pub fn get_or_synth(
        &mut self,
        spec: &ArraySpec,
        method: &str,
    ) -> Option<(FnSigKey, HirFunction)> {
        let key = FnSigKey::new(spec.mangled_type_name(), method);
        if let Some(f) = self.entries.get(&key) {
            return Some((key, f.clone()));
        }
        let f = match method {
            "new" => synth_new(spec, &key),
            "get" => synth_get(spec, &key),
            "set" => synth_set(spec, &key),
            "len" => synth_len(spec, &key),
            _ => return None,
        }?;
        self.entries.insert(key.clone(), f.clone());
        Some((key, f))
    }
}

// ---- individual synthesizers ---------------------------------------------

/// `fn new(): Self` — writes zeros to every cell of the return slot.
///
/// FlatSig: no inputs; `total_cells` output cells.
fn synth_new(spec: &ArraySpec, key: &FnSigKey) -> Option<HirFunction> {
    let total = spec.total_cells();
    let sig = typer::FlatSig {
        slots: if total > 0 {
            vec![typer::SlotInfo {
                name: "_ret".into(),
                offset: 0,
                size: total,
                mutable: true,
                type_name: spec.mangled_type_name(),
                type_args: Vec::new(),
            }]
        } else {
            Vec::new()
        },
        input_count: 0,
        output_count: total,
        field_offsets: std::collections::HashMap::new(),
    };
    let mut body = HirBlock::new();
    for i in 0..total {
        body.ops.push(HirOp::Set(SlotId(i), 0));
    }
    Some(HirFunction {
        sig,
        body,
        slot_count: total,
        type_name: key.type_name.clone(),
        method_name: key.method_name.clone(),
        warnings: Vec::new(),
    })
}

/// `fn get(self, F index): T` — match on the index cell, copy the i-th
/// element's cells into the return slot.
///
/// FlatSig layout:
///   [0 .. total_cells)                — self (immutable)
///   [total_cells .. +index_size)      — index (immutable)
///   [that .. +element_size)           — _ret (mutable)
fn synth_get(spec: &ArraySpec, key: &FnSigKey) -> Option<HirFunction> {
    if spec.index_size == 0 {
        return None;
    }
    let total = spec.total_cells();
    let input_count = total + spec.index_size;
    let output_count = spec.element_size;
    let self_start: u32 = 0;
    let index_start: u32 = total;
    let ret_start: u32 = input_count;

    let sig = typer::FlatSig {
        slots: vec![
            typer::SlotInfo {
                name: "self".into(),
                offset: self_start,
                size: total,
                mutable: false,
                type_name: spec.mangled_type_name(),
                type_args: Vec::new(),
            },
            typer::SlotInfo {
                name: "index".into(),
                offset: index_start,
                size: spec.index_size,
                mutable: false,
                type_name: spec.index_type.clone(),
                type_args: Vec::new(),
            },
            typer::SlotInfo {
                name: "_ret".into(),
                offset: ret_start,
                size: spec.element_size,
                mutable: true,
                type_name: spec.element_type.clone(),
                type_args: Vec::new(),
            },
        ],
        input_count,
        output_count,
        field_offsets: std::collections::HashMap::new(),
    };

    // Currently synthesize only for single-cell indices.
    if spec.index_size != 1 {
        return None;
    }
    let discr_slot = SlotId(index_start);
    let mut arms: Vec<(HirBlock, Vec<u8>)> = Vec::with_capacity(spec.size as usize);
    for i in 0..spec.size {
        let src_base = self_start + i * spec.element_size;
        let mut arm = HirBlock::new();
        for k in 0..spec.element_size {
            arm.ops.push(HirOp::Copy(
                SlotId(ret_start + k),
                SlotId(src_base + k),
            ));
        }
        arms.push((arm, vec![i as u8]));
    }
    let mut body = HirBlock::new();
    body.ops.push(HirOp::Match(discr_slot, arms));

    Some(HirFunction {
        sig,
        body,
        slot_count: input_count + output_count,
        type_name: key.type_name.clone(),
        method_name: key.method_name.clone(),
        warnings: Vec::new(),
    })
}

/// `fn set(mut self, F index, T value)` — match on index, copy value into
/// the indexed element's cells. Then copy the mutated self back out (the
/// inliner handles the "mut param propagation" via its own Copy-back pass).
fn synth_set(spec: &ArraySpec, key: &FnSigKey) -> Option<HirFunction> {
    if spec.index_size != 1 {
        return None;
    }
    let total = spec.total_cells();
    let input_count = total + spec.index_size + spec.element_size;
    let self_start: u32 = 0;
    let index_start: u32 = total;
    let value_start: u32 = total + spec.index_size;

    let sig = typer::FlatSig {
        slots: vec![
            typer::SlotInfo {
                name: "self".into(),
                offset: self_start,
                size: total,
                mutable: true,
                type_name: spec.mangled_type_name(),
                type_args: Vec::new(),
            },
            typer::SlotInfo {
                name: "index".into(),
                offset: index_start,
                size: spec.index_size,
                mutable: false,
                type_name: spec.index_type.clone(),
                type_args: Vec::new(),
            },
            typer::SlotInfo {
                name: "value".into(),
                offset: value_start,
                size: spec.element_size,
                mutable: false,
                type_name: spec.element_type.clone(),
                type_args: Vec::new(),
            },
        ],
        input_count,
        output_count: 0,
        field_offsets: std::collections::HashMap::new(),
    };

    let discr_slot = SlotId(index_start);
    let mut arms: Vec<(HirBlock, Vec<u8>)> = Vec::with_capacity(spec.size as usize);
    for i in 0..spec.size {
        let dst_base = self_start + i * spec.element_size;
        let mut arm = HirBlock::new();
        for k in 0..spec.element_size {
            arm.ops.push(HirOp::Copy(
                SlotId(dst_base + k),
                SlotId(value_start + k),
            ));
        }
        arms.push((arm, vec![i as u8]));
    }
    let mut body = HirBlock::new();
    body.ops.push(HirOp::Match(discr_slot, arms));

    Some(HirFunction {
        sig,
        body,
        slot_count: input_count,
        type_name: key.type_name.clone(),
        method_name: key.method_name.clone(),
        warnings: Vec::new(),
    })
}

/// `fn len(self): F` — write the compile-time constant `N` into the
/// return slot (nibble-split across index_size cells).
fn synth_len(spec: &ArraySpec, key: &FnSigKey) -> Option<HirFunction> {
    let total = spec.total_cells();
    let input_count = total;
    let output_count = spec.index_size;
    let ret_start = input_count;

    let sig = typer::FlatSig {
        slots: vec![
            typer::SlotInfo {
                name: "self".into(),
                offset: 0,
                size: total,
                mutable: false,
                type_name: spec.mangled_type_name(),
                type_args: Vec::new(),
            },
            typer::SlotInfo {
                name: "_ret".into(),
                offset: ret_start,
                size: output_count,
                mutable: true,
                type_name: spec.index_type.clone(),
                type_args: Vec::new(),
            },
        ],
        input_count,
        output_count,
        field_offsets: std::collections::HashMap::new(),
    };

    let mut body = HirBlock::new();
    let mut n = spec.size;
    for k in 0..output_count {
        body.ops
            .push(HirOp::Set(SlotId(ret_start + k), (n % 16) as u8));
        n /= 16;
    }

    Some(HirFunction {
        sig,
        body,
        slot_count: input_count + output_count,
        type_name: key.type_name.clone(),
        method_name: key.method_name.clone(),
        warnings: Vec::new(),
    })
}
