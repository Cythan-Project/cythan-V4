//! Flat signature: the u4-cell slot layout of a concrete function.
//!
//! Phase 3 Step 3.3. Each parameter is expanded into one or more cells.
//! Returns become trailing output slots. Mutability is recorded per slot
//! (from `mut` annotations on params).

use std::collections::HashMap;

use new_parser::ast;

use crate::types::*;
use crate::TypeRegistry;

pub type SlotIndex = u32;

#[derive(Debug, Clone, PartialEq)]
pub struct FlatSig {
    pub slots: Vec<SlotInfo>,
    /// Number of input slots (params).
    pub input_count: SlotIndex,
    /// Number of output slots (return-value cells). Input comes first, output last.
    pub output_count: SlotIndex,
    /// For params whose type is a struct, the offsets of each field relative
    /// to the param's start slot. Keyed by param name. Enums and primitives
    /// don't populate this.
    pub field_offsets: HashMap<String, Vec<FieldSlot>>,
}

impl FlatSig {
    pub fn total_slots(&self) -> SlotIndex {
        self.input_count + self.output_count
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SlotInfo {
    /// Name of the source param (or `_ret` for return cells).
    pub name: String,
    /// Starting cell index of this slot.
    pub offset: SlotIndex,
    /// Number of cells this slot spans.
    pub size: CellCount,
    pub mutable: bool,
    /// Concrete type name this slot holds. Empty for synthesized slots.
    pub type_name: String,
    /// Template args of the slot's type as AST `TypeOrValue`s. Downstream
    /// consumers (HIR gen) convert these to `ConcreteTemplateArg` as
    /// needed. Empty for non-generic types.
    pub type_args: Vec<ast::TypeOrValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldSlot {
    pub name: String,
    /// Offset within the owning param (not the overall FlatSig).
    pub offset: SlotIndex,
    pub size: CellCount,
}

impl FlatSig {
    /// Flatten the signature of a *concrete* function.
    ///
    /// `self_type_name` is the name of the type the function belongs to (used
    /// to resolve `self` params). Pass `""` for free functions.
    ///
    /// Returns `Err` if any parameter or return type is generic / unresolved
    /// — those need monomorphization first.
    pub fn flatten(
        sig: &ast::FunctionSig,
        self_type_name: &str,
        reg: &TypeRegistry,
    ) -> Result<FlatSig, TyperError> {
        let mut slots: Vec<SlotInfo> = Vec::new();
        let mut field_offsets: HashMap<String, Vec<FieldSlot>> = HashMap::new();
        let mut cursor: SlotIndex = 0;

        for param in &sig.params {
            let (type_name, size) = resolve_param_type(param, self_type_name, reg)?;
            let type_args = param
                .ty
                .as_ref()
                .map(|(t, _)| t.templates.iter().map(|(tv, _)| tv.clone()).collect())
                .unwrap_or_default();

            // Record struct field offsets if applicable.
            if let Some(fields) = struct_field_offsets(reg, &type_name) {
                field_offsets.insert(param.name.0.clone(), fields);
            }

            slots.push(SlotInfo {
                name: param.name.0.clone(),
                offset: cursor,
                size,
                mutable: param.mutable,
                type_name,
                type_args,
            });
            cursor += size;
        }
        let input_count = cursor;

        // Return slots (always mutable — the body writes into them).
        let mut output_count: SlotIndex = 0;
        if let Some((ret_ty, sp)) = &sig.return_type {
            let resolved_name = resolve_self(&ret_ty.name.0, self_type_name);
            let size = size_of_named(reg, &resolved_name, ret_ty, sp)?;
            let type_args = ret_ty
                .templates
                .iter()
                .map(|(tv, _)| tv.clone())
                .collect();
            slots.push(SlotInfo {
                name: "_ret".into(),
                offset: cursor,
                size,
                mutable: true,
                type_name: resolved_name.clone(),
                type_args,
            });
            if let Some(fields) = struct_field_offsets(reg, &resolved_name) {
                field_offsets.insert("_ret".into(), fields);
            }
            output_count = size;
        }

        Ok(FlatSig {
            slots,
            input_count,
            output_count,
            field_offsets,
        })
    }
}

fn resolve_param_type(
    param: &ast::Param,
    self_type_name: &str,
    reg: &TypeRegistry,
) -> Result<(String, CellCount), TyperError> {
    if param.is_self {
        // `self` with no explicit type → type is the enclosing type.
        // `Self<...> self` with templates is not supported at this stage
        // (it's only used on generic-type methods, which are Templated).
        if let Some((ty, sp)) = &param.ty {
            let resolved = resolve_self(&ty.name.0, self_type_name);
            let size = size_of_named(reg, &resolved, ty, sp)?;
            return Ok((resolved, size));
        }
        if self_type_name.is_empty() {
            return Err(TyperError::at(
                "`self` used in a free function (no enclosing type)",
                param.name.1.clone(),
            ));
        }
        let size = resolve_named_size(reg, self_type_name, &param.name.1)?;
        return Ok((self_type_name.to_string(), size));
    }

    let (ty, sp) = param.ty.as_ref().expect("non-self param must have a type");
    let resolved = resolve_self(&ty.name.0, self_type_name);
    let size = size_of_named(reg, &resolved, ty, sp)?;
    Ok((resolved, size))
}

/// Map `Self` (and `Self::...`) to the enclosing type's name.
fn resolve_self(ty_name: &str, self_type_name: &str) -> String {
    if ty_name == "Self" {
        return self_type_name.to_string();
    }
    if let Some(rest) = ty_name.strip_prefix("Self::") {
        if self_type_name.is_empty() {
            return ty_name.to_string();
        }
        return format!("{}::{}", self_type_name, rest);
    }
    ty_name.to_string()
}

/// Compute size by the resolved type name, falling back to the original AST
/// type's `resolve_type_size` if the resolved name doesn't match (so generic
/// instantiation errors surface with the original span).
fn size_of_named(
    reg: &TypeRegistry,
    resolved_name: &str,
    original: &ast::Type,
    sp: &new_parser::Span,
) -> Result<CellCount, TyperError> {
    // If the type reference had template args, we still need to go through
    // resolve_type_size to get the "cannot compute size of generic" error.
    if !original.templates.is_empty() {
        return reg.resolve_type_size(original, sp);
    }
    // Otherwise, resolve by the (possibly substituted) name.
    resolve_named_size(reg, resolved_name, sp)
}

fn resolve_named_size(
    reg: &TypeRegistry,
    name: &str,
    sp: &new_parser::Span,
) -> Result<CellCount, TyperError> {
    let info = reg.types.get(name).ok_or_else(|| {
        TyperError::at(format!("unknown type `{}`", name), sp.clone())
    })?;
    match &info.kind {
        TypeKind::Primitive { size } => Ok(*size),
        TypeKind::Struct(StructKind::Concrete(l)) => Ok(l.size),
        TypeKind::Enum(EnumKind::Concrete(l)) => Ok(l.total_size()),
        _ => Err(TyperError::at(
            format!("type `{}` is generic; cannot flatten without monomorphization", name),
            sp.clone(),
        )),
    }
}

fn struct_field_offsets(reg: &TypeRegistry, type_name: &str) -> Option<Vec<FieldSlot>> {
    let info = reg.types.get(type_name)?;
    match &info.kind {
        TypeKind::Struct(StructKind::Concrete(l)) => Some(
            l.fields
                .iter()
                .map(|f| FieldSlot {
                    name: f.name.clone(),
                    offset: f.offset,
                    size: f.size,
                })
                .collect(),
        ),
        _ => None,
    }
}
