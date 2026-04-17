//! HIR generation from AST.
//!
//! Takes a `typer::SimpleFn` (which has a computed FlatSig) and lowers its
//! AST body to a `HirFunction`.
//!
//! Expression-based semantics: every expression may produce a value, which
//! is copied into a caller-provided `result_slot` (or allocated fresh if the
//! value is ignored). Blocks have an optional `result_slot` carrying the
//! value of their final expression.

use std::collections::HashMap;

use either::Either;
use new_parser::ast;

use crate::error::HirError;
use crate::ir::*;
use crate::natives::NativeProvider;

/// Compile a `SimpleFn` into a `HirFunction` without consulting any native
/// provider. Call-sites that target built-ins (System, Array, ...) are
/// emitted as plain `Call` ops that the inliner will need to resolve.
pub fn gen_function(
    fnsig: &typer::FnSig,
    simple: &typer::SimpleFn,
    reg: &typer::TypeRegistry,
    db: &typer::FunctionDB,
) -> Result<HirFunction, HirError> {
    gen_function_with_natives::<NoNatives>(fnsig, simple, reg, db, None)
}

/// Compile a `SimpleFn`, using `natives` (if supplied) to short-circuit
/// method/static calls that target built-in types.
pub fn gen_function_with_natives<P: NativeProvider>(
    fnsig: &typer::FnSig,
    simple: &typer::SimpleFn,
    reg: &typer::TypeRegistry,
    db: &typer::FunctionDB,
    natives: Option<&P>,
) -> Result<HirFunction, HirError> {
    let mut g = Generator::new(reg, db, simple, natives.map(|n| n as &dyn NativeProvider));

    let result_slot = if simple.sig.output_count > 0 {
        Some(g.first_output_slot())
    } else {
        None
    };
    let body = g.gen_block(&simple.body.body.0.stmts, result_slot)?;

    Ok(HirFunction {
        sig: simple.sig.clone(),
        body,
        slot_count: g.next_slot,
        type_name: fnsig.type_name.clone(),
        method_name: fnsig.method_name.clone(),
    })
}

/// A zero-sized placeholder used purely to satisfy the generic bound of
/// `gen_function_with_natives` when no provider is supplied.
pub struct NoNatives;
impl NativeProvider for NoNatives {
    fn has_method(&self, _t: &str, _m: &str) -> bool {
        false
    }
    fn generate(
        &self,
        _call: crate::natives::NativeCall<'_>,
        _emitter: &mut crate::natives::NativeEmitter<'_>,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Per-function generator state.
struct Generator<'a> {
    reg: &'a typer::TypeRegistry,
    db: &'a typer::FunctionDB,
    natives: Option<&'a dyn NativeProvider>,
    simple: &'a typer::SimpleFn,
    /// Next available slot index for fresh allocation.
    next_slot: u32,
    /// Symbol table: variable name → (slot index, type name, mutable).
    scopes: Vec<HashMap<String, LocalBinding>>,
    /// Slot-level mutability flags. Indexed by slot id.
    slot_mut: Vec<bool>,
    /// Slot-level type names. Indexed by slot id.
    slot_type: Vec<String>,
    /// Slot-level "name tags" — for nicer diagnostics; unused otherwise.
    slot_name: Vec<String>,
}

#[derive(Clone)]
struct LocalBinding {
    slot: SlotId,
    type_name: String,
    /// Cell count of the binding's type. Captured at declaration time so
    /// later reads (`gen_variable`) don't have to re-size the type —
    /// important for Array<T, N, F>-style generics that aren't looked-up-
    /// able by plain name.
    size: u32,
    /// Read but not currently used downstream; `slot_mut`/`check_mutable_slot`
    /// is the authoritative mutability source. Kept for future lookups.
    #[allow(dead_code)]
    mutable: bool,
    /// Kept for future struct-aware lookups; currently unused because the
    /// generator re-derives field offsets from the registry.
    #[allow(dead_code)]
    field_offsets: Option<Vec<(String, u32, u32)>>,
    /// Concrete template args of the binding's type. Populated when the
    /// declaration (or param) uses a generic instantiation like
    /// `Array<Cell, 9, U4>`. Empty for non-generic types.
    template_args: Vec<ConcreteTemplateArg>,
}

impl<'a> Generator<'a> {
    fn new(
        reg: &'a typer::TypeRegistry,
        db: &'a typer::FunctionDB,
        simple: &'a typer::SimpleFn,
        natives: Option<&'a dyn NativeProvider>,
    ) -> Self {
        let mut g = Self {
            reg,
            db,
            natives,
            simple,
            next_slot: 0,
            scopes: vec![HashMap::new()],
            slot_mut: Vec::new(),
            slot_type: Vec::new(),
            slot_name: Vec::new(),
        };

        // Populate slots from FlatSig. Each FlatSig slot may span multiple
        // cells; we allocate one SlotId per cell so HIR can set them
        // individually. The symbol table entry points to the first cell.
        for fs in &simple.sig.slots {
            let first = g.next_slot;
            for _ in 0..fs.size {
                let sid = g.alloc_slot(&fs.name, &fs.type_name, fs.mutable);
                let _ = sid;
            }
            let field_offsets = simple
                .sig
                .field_offsets
                .get(&fs.name)
                .map(|v| {
                    v.iter()
                        .map(|f| (f.name.clone(), f.offset, f.size))
                        .collect()
                });
            // Only record the param/ret binding in the local scope if it
            // has a source-visible name. Return slots ("_ret") stay out.
            if fs.name != "_ret" {
                let template_args = lower_ast_tv_list(&fs.type_args);
                g.scopes.last_mut().unwrap().insert(
                    fs.name.clone(),
                    LocalBinding {
                        slot: SlotId(first),
                        type_name: fs.type_name.clone(),
                        size: fs.size,
                        mutable: fs.mutable,
                        field_offsets,
                        template_args,
                    },
                );
            }
        }

        g
    }

    fn alloc_slot(&mut self, name: &str, ty: &str, mutable: bool) -> SlotId {
        let id = SlotId(self.next_slot);
        self.next_slot += 1;
        self.slot_mut.push(mutable);
        self.slot_type.push(ty.to_string());
        self.slot_name.push(name.to_string());
        id
    }

    fn alloc_temp(&mut self, ty: &str, size: u32) -> SlotId {
        let first = SlotId(self.next_slot);
        for _ in 0..size {
            let _ = self.alloc_slot("_tmp", ty, true);
        }
        first
    }

    fn first_output_slot(&self) -> SlotId {
        SlotId(self.simple.sig.input_count)
    }

    fn lookup(&self, name: &str) -> Option<LocalBinding> {
        for scope in self.scopes.iter().rev() {
            if let Some(b) = scope.get(name) {
                return Some(b.clone());
            }
        }
        None
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn type_size(&self, type_name: &str) -> Result<u32, HirError> {
        let info = self
            .reg
            .types
            .get(type_name)
            .ok_or_else(|| HirError::new(format!("unknown type `{}`", type_name)))?;
        match &info.kind {
            typer::TypeKind::Primitive { size } => Ok(*size),
            typer::TypeKind::Struct(typer::StructKind::Concrete(l)) => Ok(l.size),
            typer::TypeKind::Enum(typer::EnumKind::Concrete(l)) => Ok(l.total_size()),
            _ => Err(HirError::new(format!(
                "type `{}` is generic — cannot size in HIR",
                type_name
            ))),
        }
    }

    /// Resolve `Self` → enclosing type name.
    fn resolve_ty_name(&self, name: &str) -> String {
        if name == "Self" {
            return self.simple.type_name.clone();
        }
        if let Some(rest) = name.strip_prefix("Self::") {
            return format!("{}::{}", self.simple.type_name, rest);
        }
        name.to_string()
    }

    // ---- block lowering --------------------------------------------------

    /// Compile a sequence of statement-expressions into a `HirBlock`.
    /// If `result_slot` is Some, the block's *final* expression's value is
    /// copied into it; otherwise the final value is discarded.
    fn gen_block(
        &mut self,
        stmts: &[ast::Spanned<ast::Expr>],
        result_slot: Option<SlotId>,
    ) -> Result<HirBlock, HirError> {
        self.push_scope();
        let mut block = match result_slot {
            Some(s) => HirBlock::with_result(s),
            None => HirBlock::new(),
        };

        if stmts.is_empty() {
            self.pop_scope();
            return Ok(block);
        }

        let last_ix = stmts.len() - 1;
        for (i, (expr, sp)) in stmts.iter().enumerate() {
            let is_last = i == last_ix;
            let dst = if is_last { result_slot } else { None };
            self.gen_expr_into(expr, sp, dst, &mut block)?;
        }

        self.pop_scope();
        Ok(block)
    }

    // ---- expression lowering --------------------------------------------

    /// Compile an expression into `block.ops`, writing its value (if any)
    /// into `dst`. If `dst` is `None`, the value is computed into a scratch
    /// slot (for expression-statements with side effects).
    fn gen_expr_into(
        &mut self,
        expr: &ast::Expr,
        sp: &new_parser::Span,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        match expr {
            // ---- literals -------------------------------------------------
            ast::Expr::Number(n) => {
                if let Some(dst) = dst {
                    block.push(HirOp::Set(dst, clamp_u8(*n, sp)?));
                }
                Ok(())
            }
            ast::Expr::Bool(b) => {
                if let Some(dst) = dst {
                    block.push(HirOp::Set(dst, if *b { 1 } else { 0 }));
                }
                Ok(())
            }
            ast::Expr::Char(c) => {
                if let Some(dst) = dst {
                    let byte = (*c as u32).min(255) as u8;
                    block.push(HirOp::Set(dst, byte));
                }
                Ok(())
            }
            ast::Expr::String(_) => {
                // Strings desugar into method chains; a bare String literal
                // with no `.print()` isn't meaningful. For now, reject.
                Err(HirError::at(
                    "string literals can only be used with method calls (e.g. \"hi\".print())",
                    sp.clone(),
                ))
            }
            ast::Expr::SelfValue => self.gen_variable("self", sp, dst, block),
            ast::Expr::Variable(name) => self.gen_variable(name, sp, dst, block),

            // ---- field access & method calls -----------------------------
            ast::Expr::Field(recv, field) => {
                self.gen_field_read(recv, field, dst, block)
            }
            ast::Expr::MethodCall {
                receiver,
                name,
                templates,
                args,
            } => self.gen_method_call(receiver, name, templates, args, sp, dst, block),
            ast::Expr::StaticCall {
                ty,
                name,
                templates,
                args,
            } => self.gen_static_call(ty, name, templates, args, sp, dst, block),

            // ---- construction --------------------------------------------
            ast::Expr::StructLiteral { ty, fields } => {
                self.gen_struct_literal(ty, fields, sp, dst, block)
            }
            ast::Expr::EnumVariant { ty, variant, data } => {
                self.gen_enum_variant(ty, variant, data.as_deref(), sp, dst, block)
            }

            // ---- binops ---------------------------------------------------
            ast::Expr::BinaryOp(op, l, r) => {
                self.gen_binop(*op, l, r, sp, dst, block)
            }

            // ---- control flow --------------------------------------------
            ast::Expr::If { cond, then, else_ } => {
                self.gen_if(cond, then, else_.as_deref(), sp, dst, block)
            }
            ast::Expr::Loop(body) => {
                let inner = self.gen_block(&body.0.stmts, None)?;
                block.push(HirOp::Loop(inner));
                Ok(())
            }
            ast::Expr::Break => {
                block.push(HirOp::Break);
                Ok(())
            }
            ast::Expr::Continue => {
                block.push(HirOp::Continue);
                Ok(())
            }
            ast::Expr::Return(inner) => {
                if let Some(e) = inner {
                    if self.simple.sig.output_count == 0 {
                        return Err(HirError::at(
                            "return expression in a function with no return type",
                            sp.clone(),
                        ));
                    }
                    let ret_slot = self.first_output_slot();
                    self.gen_expr_into(&e.0, &e.1, Some(ret_slot), block)?;
                }
                block.push(HirOp::Stop);
                Ok(())
            }
            ast::Expr::Match { scrutinee, arms } => {
                self.gen_match(scrutinee, arms, sp, dst, block)
            }
            ast::Expr::Block(inner) => {
                // Inline the block's statements directly instead of wrapping
                // them in `HirOp::Block`. Scoping is already handled at the
                // generator level (gen_block pushes/pops a scope); we don't
                // need a runtime `Block` here and must NOT emit one because
                // `Block` catches `Skip` — which the inliner uses to
                // represent function-return out of an inlined callee. An
                // inner `Block` from a source `{ ... }` would swallow the
                // Skip prematurely, trapping `return` inside the wrong
                // frame.
                let sub = self.gen_block(&inner.0.stmts, dst)?;
                for op in sub.ops {
                    block.push(op);
                }
                Ok(())
            }

            // ---- bindings & assignment -----------------------------------
            ast::Expr::Declaration {
                mutable,
                ty,
                name,
                value,
            } => self.gen_declaration(*mutable, ty, name, value, block),
            ast::Expr::Assign { target, value } => {
                self.gen_assign(target, value, sp, block)
            }
            ast::Expr::CompoundAssign { op, target, value } => {
                self.gen_compound_assign(*op, target, value, sp, block)
            }

            // ---- cast -----------------------------------------------------
            ast::Expr::Cast { expr, ty: _ } => {
                // Casts are erasures in this language (e.g. Cell as U4) —
                // both sides have the same cell layout. Compile as the
                // inner expression's value.
                self.gen_expr_into(&expr.0, &expr.1, dst, block)
            }
        }
    }

    fn gen_variable(
        &mut self,
        name: &str,
        sp: &new_parser::Span,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        let b = self
            .lookup(name)
            .ok_or_else(|| HirError::at(format!("undefined variable `{}`", name), sp.clone()))?;
        if let Some(dst) = dst {
            // Prefer the binding's captured size (covers generic-typed
            // locals like `Array<U4, 4, U4>` whose bare name isn't
            // sizeable). Fall back to type-name lookup when size is 0
            // (synthetic bindings like match pattern captures).
            let size = if b.size > 0 {
                b.size
            } else {
                self.type_size(&b.type_name)?
            };
            self.copy_multi(dst, b.slot, size, block)?;
        }
        Ok(())
    }

    fn gen_field_read(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        field: &ast::Spanned<String>,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        let Some(dst) = dst else {
            return Ok(());
        };
        // L-value receivers (variables / field chains) address the
        // existing storage directly. Generic-instance layouts go through
        // `struct_layout_for_expr`, falling back to the bare-name path.
        if let Ok((recv_slot, recv_ty)) = self.resolve_lvalue_base(&recv.0, &recv.1) {
            if let Some(layout) = self.struct_layout_for_expr(&recv.0) {
                if let Some(f) = layout.fields.iter().find(|f| f.name == field.0) {
                    let src = SlotId(recv_slot.0 + f.offset);
                    return self.copy_multi(dst, src, f.size, block);
                }
            }
            let (offset, size) = self.field_offset(&recv_ty, &field.0, &field.1)?;
            let src = SlotId(recv_slot.0 + offset);
            return self.copy_multi(dst, src, size, block);
        }
        // Non-l-value (method call, struct literal, etc.): evaluate the
        // receiver into a temp, then extract the field. Prefer the
        // generic-aware `struct_layout_for_expr` so receivers whose type
        // is a user-generic instantiation (e.g. `list.get(1)` returning
        // `Pair<U4>`) resolve their field offsets through the registry's
        // substituted layout.
        let recv_ty = self.infer_expr_type(&recv.0, &recv.1)?;
        let resolved = self.resolve_ty_name(&recv_ty);
        if let Some(layout) = self.struct_layout_for_expr(&recv.0) {
            if let Some(f) = layout.fields.iter().find(|f| f.name == field.0) {
                let tmp = self.alloc_temp(&resolved, layout.size);
                self.gen_expr_into(&recv.0, &recv.1, Some(tmp), block)?;
                let src = SlotId(tmp.0 + f.offset);
                return self.copy_multi(dst, src, f.size, block);
            }
        }
        let recv_size = self.type_size_permissive(&resolved);
        let tmp = self.alloc_temp(&resolved, recv_size);
        self.gen_expr_into(&recv.0, &recv.1, Some(tmp), block)?;
        let (offset, size) = self.field_offset(&resolved, &field.0, &field.1)?;
        let src = SlotId(tmp.0 + offset);
        self.copy_multi(dst, src, size, block)
    }

    /// Resolve an l-value expression (variable or chain of field accesses) to
    /// a (starting slot, type name) pair without emitting any ops.
    fn resolve_lvalue_base(
        &mut self,
        expr: &ast::Expr,
        sp: &new_parser::Span,
    ) -> Result<(SlotId, String), HirError> {
        self.resolve_lvalue_base_sized(expr, sp).map(|(s, t, _)| (s, t))
    }

    /// Like `resolve_lvalue_base` but also returns the lvalue's cell size.
    /// Needed for operator desugar on generic structs where the bare type
    /// name (`Pair`) can't be sized without its template args.
    fn resolve_lvalue_base_sized(
        &mut self,
        expr: &ast::Expr,
        sp: &new_parser::Span,
    ) -> Result<(SlotId, String, u32), HirError> {
        match expr {
            ast::Expr::SelfValue => {
                let b = self.lookup("self").ok_or_else(|| {
                    HirError::at("`self` not available here", sp.clone())
                })?;
                Ok((b.slot, b.type_name, b.size))
            }
            ast::Expr::Variable(name) => {
                let b = self.lookup(name).ok_or_else(|| {
                    HirError::at(format!("undefined variable `{}`", name), sp.clone())
                })?;
                Ok((b.slot, b.type_name, b.size))
            }
            ast::Expr::Field(inner, field) => {
                let (base_slot, base_ty, _) = self.resolve_lvalue_base_sized(&inner.0, &inner.1)?;
                // Try generic-aware layout first.
                if let Some(layout) = self.struct_layout_for_expr(&inner.0) {
                    if let Some(f) = layout.fields.iter().find(|f| f.name == field.0) {
                        return Ok((
                            SlotId(base_slot.0 + f.offset),
                            f.ast_type.name.0.clone(),
                            f.size,
                        ));
                    }
                }
                let (offset, size) = self.field_offset(&base_ty, &field.0, &field.1)?;
                let field_ty = self.field_type(&base_ty, &field.0, &field.1)?;
                Ok((SlotId(base_slot.0 + offset), field_ty, size))
            }
            _ => Err(HirError::at(
                "not an l-value (expected variable or field chain)",
                sp.clone(),
            )),
        }
    }

    fn field_offset(
        &self,
        type_name: &str,
        field: &str,
        sp: &new_parser::Span,
    ) -> Result<(u32, u32), HirError> {
        let info = self.reg.types.get(type_name).ok_or_else(|| {
            HirError::at(format!("unknown type `{}`", type_name), sp.clone())
        })?;
        match &info.kind {
            typer::TypeKind::Struct(typer::StructKind::Concrete(layout)) => {
                for f in &layout.fields {
                    if f.name == field {
                        return Ok((f.offset, f.size));
                    }
                }
                Err(HirError::at(
                    format!("type `{}` has no field `{}`", type_name, field),
                    sp.clone(),
                ))
            }
            _ => Err(HirError::at(
                format!("type `{}` is not a concrete struct; cannot take `.{}`", type_name, field),
                sp.clone(),
            )),
        }
    }

    fn field_type(
        &self,
        type_name: &str,
        field: &str,
        sp: &new_parser::Span,
    ) -> Result<String, HirError> {
        let info = self.reg.types.get(type_name).ok_or_else(|| {
            HirError::at(format!("unknown type `{}`", type_name), sp.clone())
        })?;
        let typer::TypeKind::Struct(typer::StructKind::Concrete(_)) = &info.kind else {
            return Err(HirError::at(
                format!("type `{}` is not a concrete struct", type_name),
                sp.clone(),
            ));
        };
        // Fall back to the original declaration for the field type name.
        // We don't have a direct "field name → type" mapping on StructLayout,
        // so look it up via the original `StructDef` isn't possible without
        // more context. As a shortcut, reconstruct from the layout's field
        // name + walk the AST... we don't have the AST here.
        //
        // Pragmatic: keep a side table on the registry for field types too.
        // We'll use the FlatSig field_offsets when available (for params /
        // return) or fall back to typer by looking up the struct's AST.
        //
        // For now, use `reg.field_type` helper added below.
        self.reg_field_type(type_name, field, sp)
    }

    fn reg_field_type(
        &self,
        type_name: &str,
        field: &str,
        sp: &new_parser::Span,
    ) -> Result<String, HirError> {
        let info = self.reg.types.get(type_name).unwrap();
        // Re-read the struct's field layout; since layout only has names +
        // sizes, we reconstruct the AST type name by matching the cell size
        // to the primitive U4 / or a same-size struct. This is a pragmatic
        // simplification: for Phase 4's test scope (U4, Bool, U8), a size-1
        // field is U4 or Bool and size-2 is U8. We conservatively return
        // `U4` for size 1; callers of `field_type` are paths that won't
        // actually need precise type info during HIR generation (they use
        // the size to emit Copy ops, not type-specific logic).
        match &info.kind {
            typer::TypeKind::Struct(typer::StructKind::Concrete(layout)) => {
                for f in &layout.fields {
                    if f.name == field {
                        // Heuristic: we can't recover the exact field type
                        // from layout alone. Return a size-based guess.
                        return Ok(match f.size {
                            1 => "U4".to_string(),
                            2 => "U8".to_string(),
                            _ => "<?>".to_string(),
                        });
                    }
                }
                Err(HirError::at(
                    format!("no field `{}` on `{}`", field, type_name),
                    sp.clone(),
                ))
            }
            _ => Err(HirError::at(
                format!("`{}` is not a struct", type_name),
                sp.clone(),
            )),
        }
    }

    // ---- declarations, assignment, compound assignment -------------------

    fn gen_declaration(
        &mut self,
        mutable: bool,
        ty: &ast::Spanned<ast::Type>,
        name: &ast::Spanned<String>,
        value: &ast::Spanned<ast::Expr>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        // Resolve qself at the top level AND inside any nested template
        // args (e.g. `Array<<U4 as Add>::Output, 3, U4>`) so the binding
        // and the downstream Call sees concrete types everywhere.
        let resolved_ast_ty = self
            .reg
            .resolve_qself_deep(&ty.0, &ty.1, &self.simple.type_name)
            .map_err(|e| HirError::at(e.message, ty.1.clone()))?;
        let resolved = self.resolve_ty_name(&resolved_ast_ty.name.0);
        // Use the typer's Array-aware sizing when we have an AST type with
        // template args (falls back to the plain lookup otherwise). This
        // is what lets `mut Array<U4, 4, U4> arr = ...` compute size = 4.
        let size = if !resolved_ast_ty.templates.is_empty() {
            self.reg
                .resolve_type_size(&resolved_ast_ty, &ty.1)
                .map_err(|e| HirError::at(e.message, ty.1.clone()))?
        } else {
            self.type_size(&resolved)?
        };
        let slot = self.alloc_temp(&resolved, size);
        // Mark slot mutability for each cell.
        for i in 0..size {
            let ix = (slot.0 + i) as usize;
            self.slot_mut[ix] = mutable;
            self.slot_name[ix] = name.0.clone();
        }
        let field_offsets = self
            .reg
            .types
            .get(&resolved)
            .and_then(|info| match &info.kind {
                typer::TypeKind::Struct(typer::StructKind::Concrete(l)) => Some(
                    l.fields
                        .iter()
                        .map(|f| (f.name.clone(), f.offset, f.size))
                        .collect(),
                ),
                _ => None,
            });
        let template_args: Vec<ConcreteTemplateArg> = resolved_ast_ty
            .templates
            .iter()
            .map(|(tv, _)| lower_tv(tv))
            .collect();
        self.scopes.last_mut().unwrap().insert(
            name.0.clone(),
            LocalBinding {
                slot,
                type_name: resolved,
                size,
                mutable,
                field_offsets,
                template_args,
            },
        );

        // Type inference shortcut: `mut Array<U4, 4, U4> arr = Array::new();`
        // — the call's target type is bare `Array` with no template args,
        // but the declaration makes the concrete shape obvious. Copy the
        // declared template args into the call so the monomorphizer sees
        // them. Narrow (only static calls matching the declared type
        // name), but it covers the only idiom Cythan programs need.
        let patched_value;
        let value_ref = match &value.0 {
            ast::Expr::StaticCall {
                ty: call_ty,
                name: call_name,
                templates: call_tpl,
                args: call_args,
            } if call_tpl.is_empty()
                && call_ty.0.name.0 == resolved_ast_ty.name.0
                && !resolved_ast_ty.templates.is_empty() =>
            {
                let mut new_ty = call_ty.clone();
                new_ty.0.templates = resolved_ast_ty.templates.clone();
                patched_value = (
                    ast::Expr::StaticCall {
                        ty: new_ty,
                        name: call_name.clone(),
                        templates: call_tpl.clone(),
                        args: call_args.clone(),
                    },
                    value.1.clone(),
                );
                &patched_value
            }
            _ => value,
        };
        self.gen_expr_into(&value_ref.0, &value_ref.1, Some(slot), block)
    }

    fn gen_assign(
        &mut self,
        target: &ast::Spanned<ast::Expr>,
        value: &ast::Spanned<ast::Expr>,
        sp: &new_parser::Span,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        let (dst_slot, _dst_ty, size) =
            self.resolve_lvalue_base_sized(&target.0, &target.1)?;
        self.check_mutable_slot(dst_slot, sp)?;
        // Mutability check for the entire span.
        for i in 0..size {
            self.check_mutable_slot(SlotId(dst_slot.0 + i), sp)?;
        }
        self.gen_expr_into(&value.0, &value.1, Some(dst_slot), block)
    }

    fn gen_compound_assign(
        &mut self,
        op: ast::CompoundOp,
        target: &ast::Spanned<ast::Expr>,
        value: &ast::Spanned<ast::Expr>,
        sp: &new_parser::Span,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        let (dst_slot, dst_ty, size) =
            self.resolve_lvalue_base_sized(&target.0, &target.1)?;
        self.check_mutable_slot(dst_slot, sp)?;
        for i in 0..size {
            self.check_mutable_slot(SlotId(dst_slot.0 + i), sp)?;
        }

        // Native U4 fast path: `+= 1` / `-= 1` are Inc/Dec.
        if dst_ty == "U4" {
            if let ast::Expr::Number(1) = value.0 {
                let op_hir = match op {
                    ast::CompoundOp::AddAssign => HirOp::Inc(dst_slot),
                    ast::CompoundOp::SubAssign => HirOp::Dec(dst_slot),
                };
                block.push(op_hir);
                return Ok(());
            }
            // Generic form for U4: loop N times emit Inc/Dec. For small
            // integer literals this is OK; for non-literals we synthesize
            // `while other { target += 1; other -= 1; }`-style lowering
            // via a Call to the trait method. But traits aren't wired to
            // ops yet; fall back to a Call of AddAssign::add_assign.
        }

        // General form: desugar to `target = target OP value` via a Call to
        // the appropriate AddAssign/SubAssign method. Phase 4 doesn't have
        // operator trait resolution yet, so emit a `Call` with a synthetic
        // FnRef — the inliner in Phase 6 will resolve it.
        let method = match op {
            ast::CompoundOp::AddAssign => "add_assign",
            ast::CompoundOp::SubAssign => "sub_assign",
        };
        // Evaluate the value into a temp slot.
        let val_size = size;
        let val_slot = self.alloc_temp(&dst_ty, val_size);
        self.gen_expr_into(&value.0, &value.1, Some(val_slot), block)?;

        let mut args: Vec<SlotId> = (0..size).map(|i| SlotId(dst_slot.0 + i)).collect();
        for i in 0..val_size {
            args.push(SlotId(val_slot.0 + i));
        }
        let trait_name = self.resolve_trait_for(&dst_ty, method);
        // Thread target's concrete template args so `impl AddAssign for
        // Pair<U4>` dispatches correctly.
        let recv_args = self.infer_receiver_type_args(&target.0);
        block.push(HirOp::Call {
            target: FnRef {
                type_name: dst_ty,
                method_name: method.to_string(),
                template_args: recv_args,
                trait_name,
            },
            args,
            ret: Vec::new(),
        });
        Ok(())
    }

    fn check_mutable_slot(&self, slot: SlotId, sp: &new_parser::Span) -> Result<(), HirError> {
        if let Some(mutable) = self.slot_mut.get(slot.0 as usize) {
            if !*mutable {
                let name = self
                    .slot_name
                    .get(slot.0 as usize)
                    .cloned()
                    .unwrap_or_default();
                return Err(HirError::at(
                    format!("cannot write to immutable slot `{}`", name),
                    sp.clone(),
                ));
            }
        }
        Ok(())
    }

    // ---- if / match ------------------------------------------------------

    fn gen_if(
        &mut self,
        cond: &ast::Spanned<ast::Expr>,
        then: &ast::Spanned<ast::Block>,
        else_: Option<&ast::Spanned<ast::Expr>>,
        _sp: &new_parser::Span,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        // Evaluate cond into a 1-cell temp (Bool = 1 cell).
        let cond_slot = self.alloc_temp("Bool", 1);
        self.gen_expr_into(&cond.0, &cond.1, Some(cond_slot), block)?;

        // If0 branches on == 0; in our semantics 0=false, 1=true. So the
        // "true" branch (`then`) goes into the `else-side` of If0, and the
        // "false" branch (`else_`) goes into the If0 "then" side. We pass
        // them flipped in the HirOp call.
        let else_block = match else_ {
            Some(e) => {
                let mut b = HirBlock::new();
                b.result_slot = dst;
                self.gen_expr_into(&e.0, &e.1, dst, &mut b)?;
                b
            }
            None => HirBlock::new(),
        };
        let then_block = self.gen_block(&then.0.stmts, dst)?;

        // HirOp::If0(cond, when_zero, when_nonzero)
        // when_zero == else branch; when_nonzero == then branch.
        block.push(HirOp::If0(cond_slot, else_block, then_block));
        Ok(())
    }

    fn gen_match(
        &mut self,
        scrutinee: &ast::Spanned<ast::Expr>,
        arms: &[ast::MatchArm],
        sp: &new_parser::Span,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        // Determine scrutinee type (name + concrete template args).
        let scrut_ty = self.infer_expr_type(&scrutinee.0, &scrutinee.1)?;
        let resolved = self.resolve_ty_name(&scrut_ty);
        let info = self.reg.types.get(&resolved).ok_or_else(|| {
            HirError::at(
                format!("unknown scrutinee type `{}`", resolved),
                sp.clone(),
            )
        })?;
        // For a generic enum, resolve the instantiated layout; fetch the
        // scrutinee's template args from its binding (via `concrete_type_of`).
        // Also derive the variants' payload AST types so pattern bindings
        // get the right type/size — `EnumVariantLayout` only carries sizes,
        // so we build a parallel Vec of per-variant payload types.
        let (layout, payload_ast): (typer::EnumLayout, Vec<Option<ast::Type>>) =
            match &info.kind {
                typer::TypeKind::Enum(typer::EnumKind::Concrete(l)) => {
                    // Concrete enums now keep each variant's payload AST
                    // type on the layout — pull it out so pattern bindings
                    // get the right type (e.g. `U8` rather than a defaulted
                    // `U4`).
                    let payload: Vec<Option<ast::Type>> = l
                        .variants
                        .iter()
                        .map(|v| v.data_type.clone())
                        .collect();
                    (l.clone(), payload)
                }
                typer::TypeKind::Enum(typer::EnumKind::Templated { variants }) => {
                    let (_, recv_args) =
                        self.concrete_type_of(&scrutinee.0).unwrap_or_else(|| {
                            (resolved.clone(), Vec::new())
                        });
                    let ast_ty = ast::Type {
                        name: (resolved.clone(), scrutinee.1.clone()),
                        templates: recv_args
                            .iter()
                            .map(|a| (lower_concrete_to_ast_tv(a), 0..0))
                            .collect(),
                        qself: None,
                    };
                    let l = self
                        .reg
                        .resolve_enum_layout(&ast_ty, &scrutinee.1)
                        .map_err(|e| HirError::at(e.message, sp.clone()))?;
                    // Build substitution for variant payloads.
                    let bindings: std::collections::HashMap<String, ConcreteTemplateArg> =
                        info.templates
                            .iter()
                            .zip(recv_args.iter())
                            .map(|(n, a)| (n.clone(), a.clone()))
                            .collect();
                    let payload: Vec<Option<ast::Type>> = variants
                        .iter()
                        .map(|v| {
                            v.data.as_ref().map(|t| {
                                let arg = subst_concrete_arg(
                                    &ast::TypeOrValue::Type(t.clone()),
                                    &bindings,
                                );
                                match arg {
                                    ConcreteTemplateArg::Type(ct) => ast::Type {
                                        name: (ct.name, 0..0),
                                        templates: ct
                                            .args
                                            .iter()
                                            .map(|a| (lower_concrete_to_ast_tv(a), 0..0))
                                            .collect(),
                                        qself: None,
                                    },
                                    _ => t.clone(),
                                }
                            })
                        })
                        .collect();
                    (l, payload)
                }
                _ => {
                    return Err(HirError::at(
                        format!("match scrutinee must be an enum; got `{}`", resolved),
                        sp.clone(),
                    ));
                }
            };

        // Compile scrutinee into a slot (full size).
        let total_size = layout.total_size();
        let scrut_slot = self.alloc_temp(&resolved, total_size);
        self.gen_expr_into(&scrutinee.0, &scrutinee.1, Some(scrut_slot), block)?;

        // Compile each arm. A Match op takes (discriminant_slot, Vec<(block, Vec<u8>)>).
        // Each arm's vec collects the discriminant values it matches.
        let discr_slot = scrut_slot; // first cell(s) = discriminant
        let mut out_arms: Vec<(HirBlock, Vec<u8>)> = Vec::new();

        // Pre-compute default-arm values = all discriminants not matched explicitly.
        let explicit: std::collections::HashSet<u32> = arms
            .iter()
            .filter_map(|a| match &a.pattern.0 {
                ast::Pattern::Variant { variant, .. } => layout
                    .variants
                    .iter()
                    .find(|v| v.name == variant.0)
                    .map(|v| v.discriminant),
                _ => None,
            })
            .collect();

        for arm in arms {
            let mut ab = HirBlock::new();
            ab.result_slot = dst;
            let discr_values: Vec<u8> = match &arm.pattern.0 {
                ast::Pattern::Variant { variant, binding, .. } => {
                    let layv = layout
                        .variants
                        .iter()
                        .find(|v| v.name == variant.0)
                        .ok_or_else(|| {
                            HirError::at(
                                format!("variant `{}` not on enum `{}`", variant.0, resolved),
                            arm.pattern.1.clone(),
                            )
                        })?;
                    // Bindings: introduce a local for the data payload. For
                    // generic enums we know the payload's concrete AST type
                    // via `payload_ast`; for concrete enums we fall back to
                    // the layout's size (type name: unknown → "U4" hint).
                    if let Some(bind) = binding {
                        if let ast::PatternBinding::Name(nm) = &bind.0 {
                            let bind_slot = SlotId(scrut_slot.0 + layout.discriminant_size);
                            let vix = layout
                                .variants
                                .iter()
                                .position(|v| v.name == variant.0)
                                .unwrap_or(0);
                            let payload_ty = payload_ast.get(vix).and_then(|o| o.as_ref());
                            let (ty_name, size, template_args): (String, u32, Vec<ConcreteTemplateArg>) =
                                if let Some(pt) = payload_ty {
                                    let nm = pt.name.0.clone();
                                    let sz = self
                                        .reg
                                        .resolve_type_size(pt, &(0..0))
                                        .unwrap_or(layv.data_size);
                                    let args = pt
                                        .templates
                                        .iter()
                                        .map(|(tv, _)| lower_tv(tv))
                                        .collect();
                                    (nm, sz, args)
                                } else {
                                    ("U4".to_string(), layv.data_size, Vec::new())
                                };
                            self.scopes.last_mut().unwrap().insert(
                                nm.clone(),
                                LocalBinding {
                                    slot: bind_slot,
                                    type_name: ty_name,
                                    size,
                                    mutable: false,
                                    field_offsets: None,
                                    template_args,
                                },
                            );
                        }
                    }
                    vec![layv.discriminant as u8]
                }
                ast::Pattern::Wildcard => {
                    // Match everything not covered by explicit arms.
                    (0u32..16u32)
                        .filter(|d| !explicit.contains(d))
                        .map(|d| d as u8)
                        .collect()
                }
            };
            self.gen_expr_into(&arm.body.0, &arm.body.1, dst, &mut ab)?;
            out_arms.push((ab, discr_values));
        }

        block.push(HirOp::Match(discr_slot, out_arms));
        Ok(())
    }

    // ---- binops ---------------------------------------------------------

    fn gen_binop(
        &mut self,
        op: ast::BinOp,
        l: &ast::Spanned<ast::Expr>,
        r: &ast::Spanned<ast::Expr>,
        sp: &new_parser::Span,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        // && and || have short-circuit semantics; desugar to If0.
        if matches!(op, ast::BinOp::And) {
            return self.gen_short_circuit_and(l, r, sp, dst, block);
        }
        if matches!(op, ast::BinOp::Or) {
            return self.gen_short_circuit_or(l, r, sp, dst, block);
        }

        // For comparisons / arithmetic, desugar to a Call to the appropriate
        // trait method. Phase 4 emits an unresolved Call; the inliner takes
        // care of resolving it via FunctionDB.
        let method = match op {
            ast::BinOp::EqEq => "eq",
            ast::BinOp::NotEq => "ne",
            ast::BinOp::Gt => "gt",
            ast::BinOp::Lt => "lt",
            ast::BinOp::GtEq => "ge",
            ast::BinOp::LtEq => "le",
            ast::BinOp::Add => "add",
            ast::BinOp::Sub => "sub",
            ast::BinOp::And | ast::BinOp::Or => unreachable!(),
        };
        let lty = self.infer_expr_type(&l.0, &l.1)?;
        let resolved = self.resolve_ty_name(&lty);
        // `Pair<U4>` etc: the bare name alone isn't sizeable; use the
        // expression's concrete binding to recover the cell count.
        let lsize = self
            .receiver_cell_size(&l.0)
            .or_else(|| self.type_size(&resolved).ok())
            .ok_or_else(|| HirError::at(
                format!("cannot size operand of type `{}`", resolved),
                sp.clone(),
            ))?;
        let rty = self.infer_expr_type(&r.0, &r.1)?;
        let resolved_r = self.resolve_ty_name(&rty);
        let rsize = self
            .receiver_cell_size(&r.0)
            .or_else(|| self.type_size(&resolved_r).ok())
            .unwrap_or(lsize);

        let lslot = self.alloc_temp(&resolved, lsize);
        self.gen_expr_into(&l.0, &l.1, Some(lslot), block)?;
        let rslot = self.alloc_temp(&resolved_r, rsize);
        self.gen_expr_into(&r.0, &r.1, Some(rslot), block)?;

        let mut args: Vec<SlotId> = Vec::new();
        for i in 0..lsize {
            args.push(SlotId(lslot.0 + i));
        }
        for i in 0..rsize {
            args.push(SlotId(rslot.0 + i));
        }
        // Recover the receiver's concrete template args (for dispatching
        // through the right monomorph of `impl Add for Pair<U4>`), then
        // use them to size the return. For `Add<Output>` / `Sub<Output>`
        // this is the Output type — which can differ from `lsize`.
        let recv_args = self.infer_receiver_type_args(&l.0);
        let ret: Vec<SlotId> = match dst {
            Some(d) => {
                let ret_size = match op {
                    ast::BinOp::Add | ast::BinOp::Sub | ast::BinOp::EqEq
                    | ast::BinOp::NotEq | ast::BinOp::Gt | ast::BinOp::Lt
                    | ast::BinOp::GtEq | ast::BinOp::LtEq => self
                        .lookup_return_size_full(&resolved, method, &recv_args, &[])
                        .unwrap_or_else(|| {
                            // Fallback: arithmetic ⇒ lhs size, comparisons ⇒ 1.
                            match op {
                                ast::BinOp::Add | ast::BinOp::Sub => lsize,
                                _ => 1,
                            }
                        }),
                    _ => 1,
                };
                (0..ret_size).map(|i| SlotId(d.0 + i)).collect()
            }
            None => Vec::new(),
        };
        let trait_name = self.resolve_trait_for(&resolved, method);
        block.push(HirOp::Call {
            target: FnRef {
                type_name: resolved,
                method_name: method.to_string(),
                template_args: recv_args,
                trait_name,
            },
            args,
            ret,
        });
        Ok(())
    }

    fn gen_short_circuit_and(
        &mut self,
        l: &ast::Spanned<ast::Expr>,
        r: &ast::Spanned<ast::Expr>,
        _sp: &new_parser::Span,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        // l && r  ≡  if l { r } else { false }
        let cond_slot = self.alloc_temp("Bool", 1);
        self.gen_expr_into(&l.0, &l.1, Some(cond_slot), block)?;
        let mut when_nonzero = HirBlock::new();
        when_nonzero.result_slot = dst;
        self.gen_expr_into(&r.0, &r.1, dst, &mut when_nonzero)?;
        let mut when_zero = HirBlock::new();
        when_zero.result_slot = dst;
        if let Some(dst) = dst {
            when_zero.push(HirOp::Set(dst, 0));
        }
        block.push(HirOp::If0(cond_slot, when_zero, when_nonzero));
        Ok(())
    }

    fn gen_short_circuit_or(
        &mut self,
        l: &ast::Spanned<ast::Expr>,
        r: &ast::Spanned<ast::Expr>,
        _sp: &new_parser::Span,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        // l || r  ≡  if l { true } else { r }
        let cond_slot = self.alloc_temp("Bool", 1);
        self.gen_expr_into(&l.0, &l.1, Some(cond_slot), block)?;
        let mut when_zero = HirBlock::new();
        when_zero.result_slot = dst;
        self.gen_expr_into(&r.0, &r.1, dst, &mut when_zero)?;
        let mut when_nonzero = HirBlock::new();
        when_nonzero.result_slot = dst;
        if let Some(dst) = dst {
            when_nonzero.push(HirOp::Set(dst, 1));
        }
        block.push(HirOp::If0(cond_slot, when_zero, when_nonzero));
        Ok(())
    }

    // ---- calls -----------------------------------------------------------

    fn gen_method_call(
        &mut self,
        receiver: &ast::Spanned<ast::Expr>,
        name: &ast::Spanned<String>,
        templates: &[ast::Spanned<ast::TypeOrValue>],
        args: &[ast::Spanned<ast::Expr>],
        _sp: &new_parser::Span,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        // String-literal receiver shortcut: `"abc".print()` / `.println()`
        // have no `String` type in the type registry. We lower them directly
        // to per-byte register writes — the only primitive "print" path the
        // VM supports. Everything else with strings still errors.
        if let ast::Expr::String(s) = &receiver.0 {
            let method = name.0.as_str();
            if method == "print" || method == "println" {
                if !args.is_empty() {
                    return Err(HirError::at(
                        format!("`\"...\".{}` takes no arguments", method),
                        name.1.clone(),
                    ));
                }
                let _ = templates;
                self.emit_string_print(s, method == "println", block);
                return Ok(());
            }
            return Err(HirError::at(
                format!("string literals only support `.print()` / `.println()` — got `.{}`", method),
                name.1.clone(),
            ));
        }

        // Infer receiver type.
        let recv_ty = self.infer_expr_type(&receiver.0, &receiver.1)?;
        let resolved_recv = self.resolve_ty_name(&recv_ty);
        let recv_size = self
            .receiver_cell_size(&receiver.0)
            .unwrap_or_else(|| self.type_size_permissive(&resolved_recv));

        // If the receiver is a direct l-value (variable / self / field
        // chain), use its storage slot AS the receiver slot — don't alloc a
        // temp. That way the callee's `mut self` writes land back in the
        // caller's variable, restoring the "params are by reference" rule
        // the language spec calls for. (When the receiver is a literal or
        // computed expression, falling through to alloc_temp preserves the
        // old behavior.)
        let recv_slot = match self.lvalue_slot(&receiver.0) {
            Some(s) => s,
            None => {
                let s = self.alloc_temp(&resolved_recv, recv_size);
                self.gen_expr_into(&receiver.0, &receiver.1, Some(s), block)?;
                s
            }
        };

        // Evaluate args into slots. For l-value args we reuse the
        // caller's storage so the inliner's mut-param write-back lands
        // on the real source variable (mirrors the `mut self` lvalue-
        // passthrough). Non-lvalue args copy into a fresh temp as before.
        let mut arg_slots: Vec<(SlotId, u32)> = Vec::new();
        for a in args {
            let aty = self.infer_expr_type(&a.0, &a.1)?;
            let resolved = self.resolve_ty_name(&aty);
            let size = self
                .receiver_cell_size(&a.0)
                .unwrap_or_else(|| self.type_size_permissive(&resolved));
            let s = match self.lvalue_slot(&a.0) {
                Some(slot) => slot,
                None => {
                    let t = self.alloc_temp(&resolved, size);
                    self.gen_expr_into(&a.0, &a.1, Some(t), block)?;
                    t
                }
            };
            arg_slots.push((s, size));
        }

        // Build flat arg list: recv cells, then each arg's cells.
        let mut flat_args: Vec<SlotId> = (0..recv_size).map(|i| SlotId(recv_slot.0 + i)).collect();
        for (s, size) in &arg_slots {
            for i in 0..*size {
                flat_args.push(SlotId(s.0 + i));
            }
        }

        let tpl = lower_templates(templates);
        let recv_ty_args = self.infer_receiver_type_args(&receiver.0);
        // Detect blanket-impl dispatch up front so both the return-size
        // lookup and the emitted Call include the blanket's full binding.
        let early_trait = self.resolve_trait_for(&resolved_recv, &name.0);
        let blanket_info = self
            .reg
            .method_blanket(&resolved_recv, &name.0, early_trait.as_deref());
        // `method_args_for_size` mirrors `combined` below — it's the
        // ordered concrete args list that the callee's Templated templates
        // consume. For blankets, fill the target slot with the receiver
        // and the rest from pre-resolved bindings.
        let method_args_for_size: Vec<ConcreteTemplateArg> = if let Some(b) = &blanket_info {
            let mut v = Vec::with_capacity(b.generic_args.len() + tpl.len());
            for (i, slot) in b.generic_args.iter().enumerate() {
                if i == b.target_index {
                    v.push(ConcreteTemplateArg::Type(ConcreteType {
                        name: resolved_recv.clone(),
                        args: recv_ty_args.clone(),
                    }));
                } else if let Some(tv) = slot {
                    v.push(lower_tv(tv));
                } else {
                    // Shouldn't happen — post-pass only attaches with
                    // fully resolved non-target slots.
                    v.push(ConcreteTemplateArg::Type(ConcreteType {
                        name: "<?>".to_string(),
                        args: Vec::new(),
                    }));
                }
            }
            v.extend(tpl.clone());
            v
        } else {
            tpl.clone()
        };
        let ret_slots: Vec<SlotId> = match dst {
            Some(d) => {
                let ret_size = array_method_return_size(&resolved_recv, &name.0, &recv_ty_args)
                    .or_else(|| {
                        self.lookup_return_size_full(
                            &resolved_recv,
                            &name.0,
                            &recv_ty_args,
                            &method_args_for_size,
                        )
                    })
                    .unwrap_or(0);
                (0..ret_size).map(|i| SlotId(d.0 + i)).collect()
            }
            None => Vec::new(),
        };
        if self.try_emit_native(
            &resolved_recv,
            &name.0,
            &tpl,
            &flat_args,
            &ret_slots,
            recv_size,
            &recv_ty_args,
            block,
        )? {
            return Ok(());
        }
        let trait_name = early_trait;
        // For calls on generic-receiver types (like Array<Cell, 9, U4>),
        // embed the concrete receiver type args into the FnRef's
        // template_args so the inliner / monomorphizer can dispatch. If
        // the method itself also has template args (from `self.m<N>()`),
        // we append them after the receiver args.
        //
        // Blanket-impl dispatch (`impl<T: ...> Trait for T { ... }`) also
        // needs to bind the blanket's `T` to the receiver's concrete
        // type. That binding lives at position 0 of the callee's
        // Templated template list, so we prepend the receiver type here.
        let mut combined: Vec<ConcreteTemplateArg> = Vec::new();
        if let Some(b) = &blanket_info {
            // Emit the blanket's full args in declaration order; the
            // target slot carries the concrete receiver type.
            for (i, slot) in b.generic_args.iter().enumerate() {
                if i == b.target_index {
                    combined.push(ConcreteTemplateArg::Type(ConcreteType {
                        name: resolved_recv.clone(),
                        args: recv_ty_args.clone(),
                    }));
                } else if let Some(tv) = slot {
                    combined.push(lower_tv(tv));
                }
            }
        } else {
            combined.extend(recv_ty_args);
        }
        combined.extend(tpl);
        block.push(HirOp::Call {
            target: FnRef {
                type_name: resolved_recv,
                method_name: name.0.clone(),
                template_args: combined,
                trait_name,
            },
            args: flat_args,
            ret: ret_slots,
        });
        Ok(())
    }

    fn gen_static_call(
        &mut self,
        ty: &ast::Spanned<ast::Type>,
        name: &ast::Spanned<String>,
        templates: &[ast::Spanned<ast::TypeOrValue>],
        args: &[ast::Spanned<ast::Expr>],
        _sp: &new_parser::Span,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        let resolved = self.resolve_ty_name(&ty.0.name.0);

        let mut arg_slots: Vec<(SlotId, u32)> = Vec::new();
        for a in args {
            let aty = self.infer_expr_type(&a.0, &a.1)?;
            let resolved_a = self.resolve_ty_name(&aty);
            let size = self
                .receiver_cell_size(&a.0)
                .unwrap_or_else(|| self.type_size_permissive(&resolved_a));
            let s = self.alloc_temp(&resolved_a, size);
            self.gen_expr_into(&a.0, &a.1, Some(s), block)?;
            arg_slots.push((s, size));
        }

        let mut flat_args: Vec<SlotId> = Vec::new();
        for (s, size) in &arg_slots {
            for i in 0..*size {
                flat_args.push(SlotId(s.0 + i));
            }
        }

        let tpl = lower_templates(templates);
        // Determine the receiver's concrete template args via (in order):
        //   1. Explicit `<...>` on the call site.
        //   2. Inherited from the enclosing monomorph when the bare head
        //      matches (e.g. `Pair::new(...)` inside `impl for Pair<U4>`).
        //   3. Inferred from argument types by unification with the
        //      callee's param types (e.g. `Pair::new(3, 5)` ⇒ T=U4).
        let recv_ty_args: Vec<ConcreteTemplateArg> = if !ty.0.templates.is_empty() {
            ty.0.templates.iter().map(|(tv, _)| lower_tv(tv)).collect()
        } else if resolved == self.simple.type_name
            && !self.simple.type_template_args.is_empty()
        {
            self.simple
                .type_template_args
                .iter()
                .map(|tv| lower_tv(tv))
                .collect()
        } else {
            self.infer_recv_template_args_from_args(&resolved, &name.0, args)
                .unwrap_or_default()
        };
        let ret_slots: Vec<SlotId> = match dst {
            Some(d) => {
                let ret_size = array_method_return_size(&resolved, &name.0, &recv_ty_args)
                    .or_else(|| {
                        self.lookup_return_size_full(
                            &resolved,
                            &name.0,
                            &recv_ty_args,
                            &tpl,
                        )
                    })
                    .unwrap_or(0);
                (0..ret_size).map(|i| SlotId(d.0 + i)).collect()
            }
            None => Vec::new(),
        };
        if self.try_emit_native(
            &resolved,
            &name.0,
            &tpl,
            &flat_args,
            &ret_slots,
            /*receiver_cell_count*/ 0,
            &recv_ty_args,
            block,
        )? {
            return Ok(());
        }
        let trait_name = self.resolve_trait_for(&resolved, &name.0);
        // As in gen_method_call: fold receiver-type template args (the
        // `<Cell, 9, U4>` on `Array<Cell, 9, U4>::new()`) into the FnRef
        // so the inliner / monomorphizer sees them.
        let mut combined = recv_ty_args;
        combined.extend(tpl);
        block.push(HirOp::Call {
            target: FnRef {
                type_name: resolved,
                method_name: name.0.clone(),
                template_args: combined,
                trait_name,
            },
            args: flat_args,
            ret: ret_slots,
        });
        Ok(())
    }

    fn try_emit_native(
        &mut self,
        type_name: &str,
        method: &str,
        template_args: &[ConcreteTemplateArg],
        arg_slots: &[SlotId],
        ret_slots: &[SlotId],
        receiver_cell_count: u32,
        receiver_type_args: &[ConcreteTemplateArg],
        block: &mut HirBlock,
    ) -> Result<bool, HirError> {
        let Some(natives) = self.natives else {
            return Ok(false);
        };
        if !natives.has_method(type_name, method) {
            return Ok(false);
        }
        let mut ops = Vec::new();
        {
            let mut emitter = crate::natives::NativeEmitter {
                ops: &mut ops,
                registry: self.reg,
                next_slot: &mut self.next_slot,
            };
            let call = crate::natives::NativeCall {
                type_name,
                method,
                template_args,
                arg_slots,
                ret_slots,
                receiver_type_args,
                receiver_cell_count,
                registry: self.reg,
            };
            natives
                .generate(call, &mut emitter)
                .map_err(|e| HirError::new(e))?;
        }
        for o in ops {
            block.push(o);
        }
        Ok(true)
    }

    /// Emit HIR for `"str".print()` / `.println()`: for each byte, write
    /// its high nibble to register 1, low nibble to register 2, then write
    /// 1 (the "print char" command) to register 0. Matches the VM's
    /// register protocol — same path U4::print uses for single chars.
    fn emit_string_print(&mut self, s: &str, newline: bool, block: &mut HirBlock) {
        for ch in s.chars() {
            let mut byte_buf = [0u8; 4];
            let bytes = ch.encode_utf8(&mut byte_buf).as_bytes().to_vec();
            for byte in bytes {
                let high = (byte >> 4) & 0xF;
                let low = byte & 0xF;
                block.push(HirOp::WriteRegister(1, Either::Left(high)));
                block.push(HirOp::WriteRegister(2, Either::Left(low)));
                block.push(HirOp::WriteRegister(0, Either::Left(1)));
            }
        }
        if newline {
            let high = (b'\n' >> 4) & 0xF;
            let low = b'\n' & 0xF;
            block.push(HirOp::WriteRegister(1, Either::Left(high)));
            block.push(HirOp::WriteRegister(2, Either::Left(low)));
            block.push(HirOp::WriteRegister(0, Either::Left(1)));
        }
    }

    /// Type-size helper that falls back to 1 cell for unknown types (which
    /// can happen for templated types used inside `infer_expr_type`). This
    /// keeps HIR generation pragmatic in the face of partial resolution.
    fn type_size_permissive(&self, name: &str) -> u32 {
        self.type_size(name).unwrap_or(1)
    }

    fn lookup_return_size(&self, type_name: &str, method: &str) -> Option<u32> {
        self.lookup_return_size_with_args(type_name, method, &[])
    }

    /// Determine the cell count of a method call's return value, given the
    /// receiver's concrete template args. Substitutes template params in
    /// the declared return type and then asks the typer for its size —
    /// handles both plain template-param returns (`fn capacity(self):
    /// Index`) and generic-instantiation returns (`fn get(self): T`
    /// where T is itself a `ArrayList<…>`).
    fn lookup_return_size_with_args(
        &self,
        type_name: &str,
        method: &str,
        recv_args: &[ConcreteTemplateArg],
    ) -> Option<u32> {
        self.lookup_return_size_full(type_name, method, recv_args, &[])
    }

    /// Same as `lookup_return_size_with_args` but also accepts the
    /// method-level template args so return types like `fn id<T>(T): T`
    /// can be sized. `recv_args` bind the enclosing type's templates (in
    /// declaration order); `method_args` bind the method's own templates
    /// (also in declaration order).
    fn lookup_return_size_full(
        &self,
        type_name: &str,
        method: &str,
        recv_args: &[ConcreteTemplateArg],
        method_args: &[ConcreteTemplateArg],
    ) -> Option<u32> {
        let size_of_return = |f: &typer::Fn| -> Option<u32> {
            let ret_ty = match f {
                typer::Fn::Simple(s) => return Some(s.sig.output_count),
                typer::Fn::Templated(t) => t.body.sig.return_type.as_ref()?,
            };
            // Build param-name → concrete-arg bindings across both scopes.
            // Also bind `Self` to the concrete instantiation so that
            // return types like `fn new(): Self` resolve correctly.
            let info = self.reg.types.get(type_name)?;
            if info.templates.len() != recv_args.len() {
                return None;
            }
            let mut bindings: std::collections::HashMap<String, ConcreteTemplateArg> = info
                .templates
                .iter()
                .zip(recv_args.iter())
                .map(|(n, a)| (n.clone(), a.clone()))
                .collect();
            // Method-level template params. Layout: templated.templates =
            // type-params ++ method-params, so the method-level names are
            // whatever the Templated fn declares.
            if let typer::Fn::Templated(t) = f {
                let n_type = info.templates.len();
                let method_names: Vec<String> = t.templates[n_type..].to_vec();
                if method_names.len() == method_args.len() {
                    for (n, a) in method_names.iter().zip(method_args.iter()) {
                        bindings.insert(n.clone(), a.clone());
                    }
                }
            }
            bindings.insert(
                "Self".to_string(),
                ConcreteTemplateArg::Type(ConcreteType {
                    name: type_name.to_string(),
                    args: recv_args.to_vec(),
                }),
            );
            // Substitute into the return type. If the return type uses a
            // qualified path (e.g. `<T as Add>::Output`), first substitute
            // the qself's `self_ty`/`trait_ty` through the bindings, then
            // let the registry resolve the qualified path and size it.
            let substituted = subst_ast_type_with_qself(&ret_ty.0, &bindings);
            let concrete = self
                .reg
                .resolve_qself_deep(&substituted, &(0..0), type_name)
                .ok()?;
            self.reg.resolve_type_size(&concrete, &(0..0)).ok()
        };
        let inherent_key = typer::FnSig::new(type_name, method);
        if let Some(f) = self.db.get(&inherent_key) {
            if let Some(s) = size_of_return(f) {
                return Some(s);
            }
        }
        for (k, f) in &self.db.functions {
            if k.type_name == type_name
                && k.method_name == method
                && k.trait_name.is_some()
            {
                if let Some(s) = size_of_return(f) {
                    return Some(s);
                }
            }
        }
        None
    }

    /// If `raw_name` is a template param of `enclosing_type` and the
    /// caller provided `recv_args`, substitute the corresponding concrete
    /// type's bare name. Otherwise return `raw_name` unchanged.
    fn subst_template_ref(
        &self,
        raw_name: &str,
        enclosing_type: &str,
        recv_args: &[ConcreteTemplateArg],
    ) -> String {
        let Some(info) = self.reg.types.get(enclosing_type) else {
            return raw_name.to_string();
        };
        if info.templates.len() != recv_args.len() {
            return raw_name.to_string();
        }
        for (i, t_name) in info.templates.iter().enumerate() {
            if t_name == raw_name {
                return match &recv_args[i] {
                    ConcreteTemplateArg::Type(ct) => ct.name.clone(),
                    _ => raw_name.to_string(),
                };
            }
        }
        raw_name.to_string()
    }

    /// Look up the return-type head name for a (type, method) pair,
    /// substituting template params from the receiver's concrete args.
    /// Used by operator-sugar return-type inference — knowing the *size*
    /// isn't enough; callers need the head name to chain further member
    /// lookups. Returns `None` when we can't figure out a concrete head
    /// (template-param binding missing, unknown method, ...).
    fn lookup_return_type_name(
        &self,
        type_name: &str,
        method: &str,
        recv_args: &[ConcreteTemplateArg],
    ) -> Option<String> {
        let info = self.reg.types.get(type_name)?;
        let lookup = |f: &typer::Fn| -> Option<String> {
            let ret_ty = match f {
                typer::Fn::Simple(s) => s.body.sig.return_type.as_ref()?,
                typer::Fn::Templated(t) => t.body.sig.return_type.as_ref()?,
            };
            let mut bindings: std::collections::HashMap<String, ConcreteTemplateArg> =
                std::collections::HashMap::new();
            if info.templates.len() == recv_args.len() {
                for (n, a) in info.templates.iter().zip(recv_args.iter()) {
                    bindings.insert(n.clone(), a.clone());
                }
            }
            bindings.insert(
                "Self".to_string(),
                ConcreteTemplateArg::Type(ConcreteType {
                    name: type_name.to_string(),
                    args: recv_args.to_vec(),
                }),
            );
            let arg = subst_concrete_arg(
                &ast::TypeOrValue::Type(ret_ty.0.clone()),
                &bindings,
            );
            match arg {
                ConcreteTemplateArg::Type(ct) => Some(ct.name),
                _ => None,
            }
        };
        // Inherent first.
        if let Some(f) = self.db.get(&typer::FnSig::new(type_name, method)) {
            if let Some(n) = lookup(f) {
                return Some(n);
            }
        }
        // Then any trait-keyed match.
        for (k, f) in &self.db.functions {
            if k.type_name == type_name && k.method_name == method {
                if let Some(n) = lookup(f) {
                    return Some(n);
                }
            }
        }
        None
    }

    /// Try to infer the enclosing type's template args for a
    /// static-call like `Pair::new(3, 5)` by unifying each call-site
    /// argument's concrete type against the corresponding param AST type
    /// of the callee. Returns bindings ordered to match the type's
    /// declared template param list.
    fn infer_recv_template_args_from_args(
        &self,
        type_name: &str,
        method: &str,
        args: &[ast::Spanned<ast::Expr>],
    ) -> Option<Vec<ConcreteTemplateArg>> {
        let info = self.reg.types.get(type_name)?;
        if info.templates.is_empty() {
            return None;
        }
        let f = self.db.get(&typer::FnSig::new(type_name, method))?;
        let typer::Fn::Templated(t) = f else {
            return None;
        };
        // Only bind the type-level slice of `templated.templates`.
        let n_type = info.templates.len();
        let type_param_names: std::collections::HashSet<String> =
            info.templates.iter().cloned().collect();

        let mut bindings: std::collections::HashMap<String, ConcreteTemplateArg> =
            std::collections::HashMap::new();

        // Callee's non-self params, pair with actual args.
        let callee_params: Vec<&ast::Param> = t
            .body
            .sig
            .params
            .iter()
            .filter(|p| !p.is_self)
            .collect();
        let pairs = callee_params.iter().zip(args.iter());
        for (param, arg_sp) in pairs {
            let (param_ty, _) = param.ty.as_ref()?;
            let (arg_name, arg_args) = self
                .concrete_type_of(&arg_sp.0)
                .or_else(|| {
                    // Fallback: use infer_expr_type for bare-name types.
                    let n = self.infer_expr_type(&arg_sp.0, &arg_sp.1).ok()?;
                    Some((self.resolve_ty_name(&n), Vec::new()))
                })?;
            let arg_concrete = ConcreteType {
                name: arg_name,
                args: arg_args,
            };
            unify_type_against_concrete(
                param_ty,
                &arg_concrete,
                &type_param_names,
                &mut bindings,
            );
        }

        // Assemble ordered list matching `info.templates` — only if every
        // slot got bound.
        let mut out = Vec::with_capacity(n_type);
        for tp in &info.templates {
            out.push(bindings.remove(tp)?);
        }
        Some(out)
    }

    /// Get a concrete `StructLayout` for the binding/expression's type.
    /// Combines the bare type name with any known template args — either
    /// from a `LocalBinding`, a struct field, or an explicit AST type
    /// reference — and asks the typer to substitute and size. Returns
    /// `None` when we can't figure it out.
    fn struct_layout_for_expr(&self, expr: &ast::Expr) -> Option<typer::StructLayout> {
        let (name, args) = self.concrete_type_of(expr)?;
        let ty = ast::Type {
            name: (name, 0..0),
            templates: args
                .into_iter()
                .map(|arg| (lower_concrete_to_ast_tv(&arg), 0..0))
                .collect(),
            qself: None,
        };
        self.reg.resolve_struct_layout(&ty, &(0..0)).ok()
    }

    /// (type_name, template_args) for an expression, when we can figure
    /// them out. Mirrors `infer_receiver_type_args` but returns the name
    /// too so callers can rebuild a full AST type reference.
    fn concrete_type_of(
        &self,
        expr: &ast::Expr,
    ) -> Option<(String, Vec<ConcreteTemplateArg>)> {
        match expr {
            ast::Expr::Variable(name) => {
                let b = self.lookup(name)?;
                Some((b.type_name, b.template_args))
            }
            ast::Expr::SelfValue => {
                let b = self.lookup("self")?;
                Some((b.type_name, b.template_args))
            }
            ast::Expr::Field(recv, field) => {
                let (recv_name, _) = self.concrete_type_of(&recv.0)?;
                let resolved = self.resolve_ty_name(&recv_name);
                let info = self.reg.types.get(&resolved)?;
                let typer::TypeKind::Struct(typer::StructKind::Concrete(layout)) = &info.kind
                else {
                    // Try via the receiver's template args
                    let (_, recv_args) = self.concrete_type_of(&recv.0)?;
                    let ty = ast::Type {
                        name: (resolved, 0..0),
                        templates: recv_args
                            .into_iter()
                            .map(|a| (lower_concrete_to_ast_tv(&a), 0..0))
                            .collect(),
                        qself: None,
                    };
                    let l = self.reg.resolve_struct_layout(&ty, &(0..0)).ok()?;
                    let f = l.fields.iter().find(|f| f.name == field.0)?;
                    let args = f
                        .ast_type
                        .templates
                        .iter()
                        .map(|(tv, _)| lower_tv(tv))
                        .collect();
                    return Some((f.ast_type.name.0.clone(), args));
                };
                let f = layout.fields.iter().find(|f| f.name == field.0)?;
                let args = f
                    .ast_type
                    .templates
                    .iter()
                    .map(|(tv, _)| lower_tv(tv))
                    .collect();
                Some((f.ast_type.name.0.clone(), args))
            }
            ast::Expr::BinaryOp(op, l, _) => {
                // For Add/Sub, the result is the lhs type's `<Output>`
                // per the operator trait. Resolve the lhs's concrete
                // type + args, then consult the add/sub method's return
                // type with substitution. Comparisons and short-circuit
                // operators always produce a Bool (no template args).
                match op {
                    ast::BinOp::Add | ast::BinOp::Sub => {
                        let method = if matches!(op, ast::BinOp::Add) {
                            "add"
                        } else {
                            "sub"
                        };
                        let (lhs_name, lhs_args) = self.concrete_type_of(&l.0)?;
                        let resolved = self.resolve_ty_name(&lhs_name);
                        let info = self.reg.types.get(&resolved)?;
                        let key = typer::FnSig::new(&resolved, method);
                        let mut found: Option<&typer::Fn> = self.db.get(&key);
                        if found.is_none() {
                            for (k, f) in &self.db.functions {
                                if k.type_name == resolved && k.method_name == method {
                                    found = Some(f);
                                    break;
                                }
                            }
                        }
                        let f = found?;
                        let ret_ty = match f {
                            typer::Fn::Simple(s) => s.body.sig.return_type.as_ref()?,
                            typer::Fn::Templated(t) => t.body.sig.return_type.as_ref()?,
                        };
                        let mut bindings: std::collections::HashMap<
                            String,
                            ConcreteTemplateArg,
                        > = std::collections::HashMap::new();
                        if info.templates.len() == lhs_args.len() {
                            for (n, a) in info.templates.iter().zip(lhs_args.iter()) {
                                bindings.insert(n.clone(), a.clone());
                            }
                        }
                        bindings.insert(
                            "Self".to_string(),
                            ConcreteTemplateArg::Type(ConcreteType {
                                name: resolved.clone(),
                                args: lhs_args.clone(),
                            }),
                        );
                        let arg = subst_concrete_arg(
                            &ast::TypeOrValue::Type(ret_ty.0.clone()),
                            &bindings,
                        );
                        match arg {
                            ConcreteTemplateArg::Type(ct) => {
                                Some((ct.name, ct.args))
                            }
                            _ => None,
                        }
                    }
                    _ => Some(("Bool".to_string(), Vec::new())),
                }
            }
            ast::Expr::StaticCall { ty, name, templates, args } => {
                // Determine the receiver's concrete template args the same
                // way `gen_static_call` does: explicit first, then inherit
                // from the enclosing monomorph, then unify with arg types.
                let resolved_recv = self.resolve_ty_name(&ty.0.name.0);
                let recv_args: Vec<ConcreteTemplateArg> = if !ty.0.templates.is_empty() {
                    ty.0.templates.iter().map(|(tv, _)| lower_tv(tv)).collect()
                } else if resolved_recv == self.simple.type_name
                    && !self.simple.type_template_args.is_empty()
                {
                    self.simple
                        .type_template_args
                        .iter()
                        .map(|tv| lower_tv(tv))
                        .collect()
                } else {
                    self.infer_recv_template_args_from_args(&resolved_recv, &name.0, args)
                        .unwrap_or_default()
                };
                let method_args: Vec<ConcreteTemplateArg> = templates
                    .iter()
                    .map(|(tv, _)| lower_tv(tv))
                    .collect();
                // Look up the method's declared return type and substitute.
                let info = self.reg.types.get(&resolved_recv)?;
                let (f, fk) = self
                    .db
                    .get(&typer::FnSig::new(&resolved_recv, &name.0))
                    .map(|f| (f, typer::FnSig::new(&resolved_recv, &name.0)))
                    .or_else(|| {
                        for (k, f) in &self.db.functions {
                            if k.type_name == resolved_recv && k.method_name == name.0 {
                                return Some((f, k.clone()));
                            }
                        }
                        None
                    })?;
                let _ = fk;
                let ret_ty = match f {
                    typer::Fn::Simple(s) => s.body.sig.return_type.as_ref()?,
                    typer::Fn::Templated(t) => t.body.sig.return_type.as_ref()?,
                }
                .0
                .clone();
                let mut bindings: std::collections::HashMap<String, ConcreteTemplateArg> =
                    std::collections::HashMap::new();
                if info.templates.len() == recv_args.len() {
                    for (n, a) in info.templates.iter().zip(recv_args.iter()) {
                        bindings.insert(n.clone(), a.clone());
                    }
                }
                if let typer::Fn::Templated(t) = f {
                    let n_type = info.templates.len();
                    if t.templates.len() >= n_type {
                        let method_names: Vec<String> = t.templates[n_type..].to_vec();
                        if method_names.len() == method_args.len() {
                            for (n, a) in method_names.iter().zip(method_args.iter()) {
                                bindings.insert(n.clone(), a.clone());
                            }
                        }
                    }
                }
                bindings.insert(
                    "Self".to_string(),
                    ConcreteTemplateArg::Type(ConcreteType {
                        name: resolved_recv.clone(),
                        args: recv_args.clone(),
                    }),
                );
                let arg = subst_concrete_arg(
                    &ast::TypeOrValue::Type(ret_ty),
                    &bindings,
                );
                match arg {
                    ConcreteTemplateArg::Type(ct) => Some((ct.name.clone(), ct.args.clone())),
                    _ => None,
                }
            }
            ast::Expr::MethodCall { receiver, name, .. } => {
                // Method call that returns a generic instantiation — e.g.
                // `outer.get(0)` where outer is `ArrayList<U4, 2,
                // ArrayList<U4, 3, U4>>` returns the inner ArrayList. We
                // look up the method's declared return type, then
                // substitute the receiver's template args (so return `T`
                // becomes the concrete `ArrayList<U4, 3, U4>`).
                let (recv_name, recv_args) = self.concrete_type_of(&receiver.0)?;
                let resolved = self.resolve_ty_name(&recv_name);
                let info = self.reg.types.get(&resolved)?;
                // Find the Templated function's declared return type.
                let mut ret_ty: Option<ast::Type> = None;
                if let Some(f) = self.db.get(&typer::FnSig::new(&resolved, &name.0)) {
                    let rt = match f {
                        typer::Fn::Simple(s) => s.body.sig.return_type.as_ref(),
                        typer::Fn::Templated(t) => t.body.sig.return_type.as_ref(),
                    };
                    if let Some((rt, _)) = rt {
                        ret_ty = Some(rt.clone());
                    }
                }
                let ret_ty = ret_ty?;

                // Substitute the type params of the enclosing type in the
                // return type. If ret_ty's head is a bare template param
                // (like `T`), return the corresponding concrete arg
                // outright (name + its own args).
                if ret_ty.templates.is_empty() {
                    if info.templates.len() == recv_args.len() {
                        for (i, tp_name) in info.templates.iter().enumerate() {
                            if tp_name == &ret_ty.name.0 {
                                if let ConcreteTemplateArg::Type(ct) = &recv_args[i] {
                                    let nested_args = ct
                                        .args
                                        .iter()
                                        .cloned()
                                        .collect::<Vec<_>>();
                                    return Some((ct.name.clone(), nested_args));
                                }
                            }
                        }
                    }
                    // Plain concrete return type (e.g. Bool).
                    return Some((ret_ty.name.0.clone(), Vec::new()));
                }
                // ret_ty is a generic instance referencing type params —
                // substitute each arg.
                let mut bindings = std::collections::HashMap::new();
                if info.templates.len() == recv_args.len() {
                    for (name, arg) in info.templates.iter().zip(recv_args.iter()) {
                        bindings.insert(name.clone(), arg.clone());
                    }
                }
                let args = ret_ty
                    .templates
                    .iter()
                    .map(|(tv, _)| subst_concrete_arg(tv, &bindings))
                    .collect();
                Some((ret_ty.name.0.clone(), args))
            }
            _ => None,
        }
    }

    /// If `expr` is a direct l-value (variable, self, or a field chain),
    /// return the slot id that stores it. Used by `gen_method_call` to
    /// route the receiver at the caller's existing storage rather than
    /// allocating a fresh temp — preserving "params are by reference"
    /// semantics for mutable receivers.
    fn lvalue_slot(&self, expr: &ast::Expr) -> Option<SlotId> {
        match expr {
            ast::Expr::Variable(name) => self.lookup(name).map(|b| b.slot),
            ast::Expr::SelfValue => self.lookup("self").map(|b| b.slot),
            ast::Expr::Field(recv, field) => {
                let base = self.lvalue_slot(&recv.0)?;
                // Try the generic-aware path first (handles `list.size`
                // on an ArrayList<U4, 4, U4>).
                if let Some(layout) = self.struct_layout_for_expr(&recv.0) {
                    let fl = layout.fields.iter().find(|f| f.name == field.0)?;
                    return Some(SlotId(base.0 + fl.offset));
                }
                let recv_ty = self.infer_expr_type(&recv.0, &recv.1).ok()?;
                let resolved = self.resolve_ty_name(&recv_ty);
                let info = self.reg.types.get(&resolved)?;
                let typer::TypeKind::Struct(typer::StructKind::Concrete(layout)) = &info.kind
                else {
                    return None;
                };
                let fl = layout.fields.iter().find(|f| f.name == field.0)?;
                Some(SlotId(base.0 + fl.offset))
            }
            _ => None,
        }
    }

    /// Size an expression's value in cells. Handles variables (binding's
    /// size), self, field chains (via struct layout), and method calls
    /// (compute from the receiver's concrete type args + return type).
    fn receiver_cell_size(&self, expr: &ast::Expr) -> Option<u32> {
        match expr {
            ast::Expr::Variable(name) => self.lookup(name).map(|b| b.size).filter(|s| *s > 0),
            ast::Expr::SelfValue => self.lookup("self").map(|b| b.size).filter(|s| *s > 0),
            ast::Expr::Field(_, field) => {
                let parent_layout = self.struct_layout_for_expr(
                    match expr {
                        ast::Expr::Field(inner, _) => &inner.0,
                        _ => unreachable!(),
                    },
                )?;
                parent_layout
                    .fields
                    .iter()
                    .find(|f| f.name == field.0)
                    .map(|f| f.size)
                    .filter(|s| *s > 0)
            }
            _ => {
                // General path: infer the concrete type (including template
                // args) and ask the typer for its cell count.
                let (name, args) = self.concrete_type_of(expr)?;
                let ast_ty = ast::Type {
                    name: (name, 0..0),
                    templates: args
                        .into_iter()
                        .map(|a| (lower_concrete_to_ast_tv(&a), 0..0))
                        .collect(),
                    qself: None,
                };
                self.reg.resolve_type_size(&ast_ty, &(0..0)).ok().filter(|s| *s > 0)
            }
        }
    }

    /// Infer the concrete template args of an expression's type. Delegates
    /// to `concrete_type_of` which handles generic struct instances (e.g.
    /// a field of `Array<U4, 4, U4>` on an `ArrayList<U4, 4, U4>`).
    fn infer_receiver_type_args(&self, expr: &ast::Expr) -> Vec<ConcreteTemplateArg> {
        self.concrete_type_of(expr).map(|(_, args)| args).unwrap_or_default()
    }

    /// Consult the typer's scoped method-resolution to discover whether
    /// this (type, method) dispatches via a trait. Returns `Some(trait)` if
    /// it does, `None` for inherent dispatch, ambiguous, or not-found cases
    /// (in which case the caller emits a Call that the inliner may yet
    /// fail on — preserving the current behaviour where unresolvable calls
    /// surface as late errors rather than stopping HIR gen).
    fn resolve_trait_for(&self, type_name: &str, method: &str) -> Option<String> {
        use typer::MethodResolution;
        let res = self.reg.resolve_method(
            self.simple.file_id,
            type_name,
            method,
            /*trait_hint=*/ None,
        );
        match res {
            MethodResolution::Trait { trait_name } => Some(trait_name),
            _ => None,
        }
    }

    // ---- struct / enum construction -------------------------------------

    fn gen_struct_literal(
        &mut self,
        ty: &ast::Spanned<ast::Type>,
        fields: &[(ast::Spanned<String>, ast::Spanned<ast::Expr>)],
        sp: &new_parser::Span,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        let Some(dst) = dst else {
            // Still need to evaluate fields for side effects.
            for (_, v) in fields {
                self.gen_expr_into(&v.0, &v.1, None, block)?;
            }
            return Ok(());
        };
        let resolved = self.resolve_ty_name(&ty.0.name.0);
        // For generic instantiations (e.g. `Self { … }` in a monomorphized
        // ArrayList method, where Self is `ArrayList<U4, 4, U4>`), the
        // bare registry lookup finds only the Templated shell. Ask the
        // typer for a substituted concrete layout instead.
        let layout = if !ty.0.templates.is_empty() {
            self.reg
                .resolve_struct_layout(&ty.0, &ty.1)
                .map_err(|e| HirError::at(e.message, ty.1.clone()))?
        } else {
            let info = self.reg.types.get(&resolved).ok_or_else(|| {
                HirError::at(format!("unknown type `{}`", resolved), sp.clone())
            })?;
            match &info.kind {
                typer::TypeKind::Struct(typer::StructKind::Concrete(l)) => l.clone(),
                _ => {
                    return Err(HirError::at(
                        format!("cannot build struct literal for non-concrete `{}`", resolved),
                        sp.clone(),
                    ))
                }
            }
        };
        for (fname, fval) in fields {
            let field = layout
                .fields
                .iter()
                .find(|f| f.name == fname.0)
                .ok_or_else(|| {
                    HirError::at(
                        format!("no field `{}` on `{}`", fname.0, resolved),
                        fname.1.clone(),
                    )
                })?;
            let dst_slot = SlotId(dst.0 + field.offset);
            // Same type-inference shortcut as gen_declaration: if the
            // field's declared type is a generic like `Array<U4, 4, U4>`
            // and the value is a bare `Array::new()` call, substitute the
            // field's template args into the call's type.
            let patched: Option<ast::Spanned<ast::Expr>>;
            let value_ref = match &fval.0 {
                ast::Expr::StaticCall {
                    ty: call_ty,
                    name: call_name,
                    templates: call_tpl,
                    args: call_args,
                } if call_tpl.is_empty()
                    && call_ty.0.name.0 == field.ast_type.name.0
                    && !field.ast_type.templates.is_empty() =>
                {
                    let mut new_ty = call_ty.clone();
                    new_ty.0.templates = field.ast_type.templates.clone();
                    patched = Some((
                        ast::Expr::StaticCall {
                            ty: new_ty,
                            name: call_name.clone(),
                            templates: call_tpl.clone(),
                            args: call_args.clone(),
                        },
                        fval.1.clone(),
                    ));
                    patched.as_ref().unwrap()
                }
                _ => {
                    patched = None;
                    fval
                }
            };
            self.gen_expr_into(&value_ref.0, &value_ref.1, Some(dst_slot), block)?;
        }
        Ok(())
    }

    fn gen_enum_variant(
        &mut self,
        ty: &ast::Spanned<ast::Type>,
        variant: &ast::Spanned<String>,
        data: Option<&ast::Spanned<ast::Expr>>,
        sp: &new_parser::Span,
        dst: Option<SlotId>,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        let Some(dst) = dst else {
            if let Some(d) = data {
                self.gen_expr_into(&d.0, &d.1, None, block)?;
            }
            return Ok(());
        };
        let resolved = self.resolve_ty_name(&ty.0.name.0);
        let info = self.reg.types.get(&resolved).ok_or_else(|| {
            HirError::at(format!("unknown type `{}`", resolved), sp.clone())
        })?;
        let layout = match &info.kind {
            typer::TypeKind::Enum(typer::EnumKind::Concrete(l)) => l.clone(),
            typer::TypeKind::Enum(typer::EnumKind::Templated { .. }) => {
                // Generic enum instantiation — require template args on the
                // AST type reference (or infer from context elsewhere). Use
                // the registry's layout resolver to compute the concrete
                // layout with substituted payload sizes.
                let ast_ty = ast::Type {
                    name: (resolved.clone(), ty.0.name.1.clone()),
                    templates: ty.0.templates.clone(),
                    qself: None,
                };
                self.reg
                    .resolve_enum_layout(&ast_ty, &ty.1)
                    .map_err(|e| HirError::at(e.message, sp.clone()))?
            }
            _ => {
                return Err(HirError::at(
                    format!("cannot construct variant on non-concrete enum `{}`", resolved),
                    sp.clone(),
                ))
            }
        };
        let v = layout
            .variants
            .iter()
            .find(|v| v.name == variant.0)
            .ok_or_else(|| {
                HirError::at(
                    format!("variant `{}` not on `{}`", variant.0, resolved),
                    variant.1.clone(),
                )
            })?;

        // Set discriminant cells. For a 1-cell discriminant, just Set dst.
        // For 2-cell, split high/low.
        match layout.discriminant_size {
            1 => block.push(HirOp::Set(dst, v.discriminant as u8)),
            2 => {
                block.push(HirOp::Set(dst, (v.discriminant & 0xF) as u8));
                block.push(HirOp::Set(SlotId(dst.0 + 1), ((v.discriminant >> 4) & 0xF) as u8));
            }
            n => {
                return Err(HirError::at(
                    format!("unsupported discriminant size {}", n),
                    sp.clone(),
                ))
            }
        }

        if let Some(d) = data {
            let data_start = SlotId(dst.0 + layout.discriminant_size);
            self.gen_expr_into(&d.0, &d.1, Some(data_start), block)?;
        }
        Ok(())
    }

    // ---- small utilities ------------------------------------------------

    fn copy_multi(
        &mut self,
        dst: SlotId,
        src: SlotId,
        size: u32,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        for i in 0..size {
            block.push(HirOp::Copy(SlotId(dst.0 + i), SlotId(src.0 + i)));
        }
        Ok(())
    }

    /// Infer an expression's static type name. Used to size temps and pick
    /// method-call return sizes.
    fn infer_expr_type(
        &self,
        expr: &ast::Expr,
        _sp: &new_parser::Span,
    ) -> Result<String, HirError> {
        Ok(match expr {
            ast::Expr::Number(_) => "U4".into(),
            ast::Expr::Bool(_) => "Bool".into(),
            ast::Expr::Char(_) => "U4".into(),
            ast::Expr::String(_) => "<str>".into(),
            ast::Expr::SelfValue => {
                self.lookup("self")
                    .map(|b| b.type_name)
                    .unwrap_or_else(|| self.simple.type_name.clone())
            }
            ast::Expr::Variable(n) => self
                .lookup(n)
                .map(|b| b.type_name)
                .unwrap_or_else(|| "<?>".into()),
            ast::Expr::Field(recv, field) => {
                // Prefer the expr-based resolution (handles generic
                // instances via resolve_struct_layout). Fall back to bare
                // registry lookup for simple concrete structs.
                if let Some(layout) = self.struct_layout_for_expr(&recv.0) {
                    if let Some(f) = layout.fields.iter().find(|f| f.name == field.0) {
                        return Ok(f.ast_type.name.0.clone());
                    }
                }
                let recv_ty = self.infer_expr_type(&recv.0, &recv.1)?;
                let resolved = self.resolve_ty_name(&recv_ty);
                if let Some(info) = self.reg.types.get(&resolved) {
                    if let typer::TypeKind::Struct(typer::StructKind::Concrete(layout)) =
                        &info.kind
                    {
                        if let Some(f) =
                            layout.fields.iter().find(|f| f.name == field.0)
                        {
                            return Ok(f.ast_type.name.0.clone());
                        }
                    }
                }
                if let Ok(s) = self.reg_field_type(&resolved, &field.0, &field.1) {
                    s
                } else {
                    "<?>".into()
                }
            }
            ast::Expr::Cast { ty, .. } => self.resolve_ty_name(&ty.0.name.0),
            ast::Expr::BinaryOp(op, l, _) => match op {
                ast::BinOp::EqEq
                | ast::BinOp::NotEq
                | ast::BinOp::Gt
                | ast::BinOp::Lt
                | ast::BinOp::GtEq
                | ast::BinOp::LtEq
                | ast::BinOp::And
                | ast::BinOp::Or => "Bool".into(),
                ast::BinOp::Add | ast::BinOp::Sub => {
                    // With `Add<Output>` / `Sub<Output>`, the result type
                    // can differ from the lhs (widening, etc.). Look it
                    // up via the operator trait's impl on the lhs type.
                    let method = match op {
                        ast::BinOp::Add => "add",
                        ast::BinOp::Sub => "sub",
                        _ => unreachable!(),
                    };
                    let lhs_name = self.infer_expr_type(&l.0, &l.1)?;
                    let resolved = self.resolve_ty_name(&lhs_name);
                    // Look up the return type's head via the trait method.
                    let recv_args = self.infer_receiver_type_args(&l.0);
                    self.lookup_return_type_name(&resolved, method, &recv_args)
                        .unwrap_or(resolved)
                }
            },
            ast::Expr::StructLiteral { ty, .. } | ast::Expr::EnumVariant { ty, .. } => {
                self.resolve_ty_name(&ty.0.name.0)
            }
            ast::Expr::StaticCall { ty, name, templates, .. } => {
                // Look up the method's declared return type and substitute
                // both the receiver's type-level templates and the
                // method-level templates. For `U4::id<U8>(...)`, the
                // declared return `T` becomes `U8`; without this the
                // caller thinks the expression's type is the bare head
                // `U4`, breaking later field / sizing lookups.
                let resolved_recv = self.resolve_ty_name(&ty.0.name.0);
                let recv_args: Vec<ConcreteTemplateArg> =
                    ty.0.templates.iter().map(|(tv, _)| lower_tv(tv)).collect();
                let method_args: Vec<ConcreteTemplateArg> = templates
                    .iter()
                    .map(|(tv, _)| lower_tv(tv))
                    .collect();
                let fallback = || resolved_recv.clone();
                let Some(info) = self.reg.types.get(&resolved_recv) else {
                    return Ok(fallback());
                };
                let lookup = |f: &typer::Fn| -> Option<String> {
                    let ret_ty = match f {
                        typer::Fn::Simple(s) => s.body.sig.return_type.as_ref()?,
                        typer::Fn::Templated(t) => t.body.sig.return_type.as_ref()?,
                    };
                    let mut bindings: std::collections::HashMap<String, ConcreteTemplateArg> =
                        std::collections::HashMap::new();
                    if info.templates.len() == recv_args.len() {
                        for (n, a) in info.templates.iter().zip(recv_args.iter()) {
                            bindings.insert(n.clone(), a.clone());
                        }
                    }
                    if let typer::Fn::Templated(t) = f {
                        let n_type = info.templates.len();
                        if t.templates.len() >= n_type {
                            let method_names: Vec<String> =
                                t.templates[n_type..].to_vec();
                            if method_names.len() == method_args.len() {
                                for (n, a) in method_names.iter().zip(method_args.iter()) {
                                    bindings.insert(n.clone(), a.clone());
                                }
                            }
                        }
                    }
                    bindings.insert(
                        "Self".to_string(),
                        ConcreteTemplateArg::Type(ConcreteType {
                            name: resolved_recv.clone(),
                            args: recv_args.clone(),
                        }),
                    );
                    let arg = subst_concrete_arg(
                        &ast::TypeOrValue::Type(ret_ty.0.clone()),
                        &bindings,
                    );
                    match arg {
                        ConcreteTemplateArg::Type(ct) => Some(ct.name),
                        _ => None,
                    }
                };
                let inherent_key = typer::FnSig::new(&resolved_recv, &name.0);
                if let Some(f) = self.db.get(&inherent_key) {
                    if let Some(n) = lookup(f) {
                        return Ok(n);
                    }
                }
                for (k, f) in &self.db.functions {
                    if k.type_name == resolved_recv && k.method_name == name.0 {
                        if let Some(n) = lookup(f) {
                            return Ok(n);
                        }
                    }
                }
                fallback()
            }
            ast::Expr::If { then, .. } => {
                // Type of an if-expression = type of its then branch's last stmt.
                then.0
                    .stmts
                    .last()
                    .map(|s| self.infer_expr_type(&s.0, &s.1).unwrap_or_default())
                    .unwrap_or_default()
            }
            ast::Expr::MethodCall { receiver, name, .. } => {
                let recv_ty = self.infer_expr_type(&receiver.0, &receiver.1)?;
                let resolved = self.resolve_ty_name(&recv_ty);

                // Special-case: Array<T, N, F> synthesized methods. Their
                // return types reference the (yet-unsubstituted) `T`/`F`
                // template params — which the general lookup below can't
                // handle because `T`/`F` aren't registered types.
                if resolved == "Array" {
                    let recv_args = self.infer_receiver_type_args(&receiver.0);
                    if recv_args.len() == 3 {
                        let arg_name = |i: usize| match &recv_args[i] {
                            ConcreteTemplateArg::Type(t) => Some(t.name.clone()),
                            _ => None,
                        };
                        match name.0.as_str() {
                            "get" => {
                                if let Some(n) = arg_name(0) {
                                    return Ok(n);
                                }
                            }
                            "len" => {
                                if let Some(n) = arg_name(2) {
                                    return Ok(n);
                                }
                            }
                            "new" => return Ok("Array".to_string()),
                            _ => {}
                        }
                    }
                }

                // General path: look up the method's return type in the DB
                // and substitute type-level template params with the
                // receiver's concrete args. Without this, `list.size` on an
                // `ArrayList<U4, 4, U4>` returns the bare "Index" which no
                // downstream pass can size.
                let recv_args = self.infer_receiver_type_args(&receiver.0);
                let substitute_template_ref = |raw_name: &str| -> String {
                    // Find the enclosing type's template params; if
                    // `raw_name` is one of them, map it to the concrete arg.
                    let Some(info) = self.reg.types.get(&resolved) else {
                        return raw_name.to_string();
                    };
                    if info.templates.len() != recv_args.len() {
                        return raw_name.to_string();
                    }
                    for (i, t_name) in info.templates.iter().enumerate() {
                        if t_name == raw_name {
                            return match &recv_args[i] {
                                ConcreteTemplateArg::Type(ct) => ct.name.clone(),
                                _ => raw_name.to_string(),
                            };
                        }
                    }
                    raw_name.to_string()
                };
                let return_type_of = |f: &typer::Fn| -> Option<String> {
                    let ret = match f {
                        typer::Fn::Simple(s) => s.body.sig.return_type.as_ref()?,
                        typer::Fn::Templated(t) => t.body.sig.return_type.as_ref()?,
                    };
                    let raw = self.resolve_ty_name(&ret.0.name.0);
                    Some(substitute_template_ref(&raw))
                };
                if let Some(f) = self.db.get(&typer::FnSig::new(&resolved, &name.0)) {
                    if let Some(t) = return_type_of(f) {
                        return Ok(t);
                    }
                }
                for (k, f) in &self.db.functions {
                    if k.type_name == resolved && k.method_name == name.0 {
                        if let Some(t) = return_type_of(f) {
                            return Ok(t);
                        }
                    }
                }
                "<?>".into()
            }
            ast::Expr::Block(b) => {
                // Type of a block = type of its last statement (if any).
                b.0
                    .stmts
                    .last()
                    .map(|s| self.infer_expr_type(&s.0, &s.1).unwrap_or_default())
                    .unwrap_or_default()
            }
            ast::Expr::Match { arms, .. } => arms
                .first()
                .map(|arm| self.infer_expr_type(&arm.body.0, &arm.body.1).unwrap_or_default())
                .unwrap_or_default(),
            ast::Expr::Loop(_) | ast::Expr::Return(_) | ast::Expr::Break
            | ast::Expr::Continue | ast::Expr::Declaration { .. } | ast::Expr::Assign { .. }
            | ast::Expr::CompoundAssign { .. } => "<?>".into(),
        })
    }
}

// ---- free helpers ---------------------------------------------------------

fn clamp_u8(n: i64, sp: &new_parser::Span) -> Result<u8, HirError> {
    if !(0..=255).contains(&n) {
        return Err(HirError::at(
            format!("integer literal {} doesn't fit in a byte", n),
            sp.clone(),
        ));
    }
    Ok(n as u8)
}

fn lower_templates(templates: &[ast::Spanned<ast::TypeOrValue>]) -> Vec<ConcreteTemplateArg> {
    templates
        .iter()
        .map(|(t, _)| lower_tv(t))
        .collect()
}

/// Return size of synthesized `Array<T, N, F>::method(...)` calls. Uses the
/// receiver's concrete template args — which is how we know `T`'s size
/// without monomorphizing first. Returns `None` for non-Array calls or
/// when args aren't complete enough to decide.
fn array_method_return_size(
    type_name: &str,
    method: &str,
    recv_args: &[ConcreteTemplateArg],
) -> Option<u32> {
    if type_name != "Array" || recv_args.len() < 3 {
        return None;
    }
    let t_size: u32 = match &recv_args[0] {
        ConcreteTemplateArg::Type(t) => match t.name.as_str() {
            "U4" | "Bool" => 1,
            "U8" => 2,
            _ => return None,
        },
        _ => return None,
    };
    let n: u32 = match &recv_args[1] {
        ConcreteTemplateArg::Value(v) => (*v).max(0) as u32,
        _ => return None,
    };
    let f_size: u32 = match &recv_args[2] {
        ConcreteTemplateArg::Type(t) => match t.name.as_str() {
            "U4" | "Bool" => 1,
            "U8" => 2,
            _ => return None,
        },
        _ => return None,
    };
    match method {
        "new" => Some(t_size * n),
        "get" => Some(t_size),
        "len" => Some(f_size),
        "set" => Some(0),
        _ => None,
    }
}

/// Lower a raw (unspanned) list of AST `TypeOrValue`s to concrete template
/// args. Used to lift cached `SlotInfo.type_args` / `FieldLayout.ast_type`
/// template lists.
pub(crate) fn lower_ast_tv_list(tvs: &[ast::TypeOrValue]) -> Vec<ConcreteTemplateArg> {
    tvs.iter().map(lower_tv).collect()
}

/// Substitute a template-param reference in an AST `TypeOrValue` using a
/// binding table (param name → concrete arg). Used when inferring the
/// concrete type of a method call's return — e.g. `self.backing: Array<T,
/// N, Index>` becomes `Array<Cell, 9, U4>` once T/N/Index are bound.
/// Unify a callee-param AST type against the arg's concrete type, filling
/// in any `type_params` that appear in the param type with bindings drawn
/// from the arg. Succeeds silently — this is a best-effort inference that
/// contributes partial information; the caller checks completeness.
fn unify_type_against_concrete(
    param_ty: &ast::Type,
    arg: &ConcreteType,
    type_params: &std::collections::HashSet<String>,
    bindings: &mut std::collections::HashMap<String, ConcreteTemplateArg>,
) {
    // Leaf case: param is a bare template-param reference.
    if param_ty.templates.is_empty() && type_params.contains(&param_ty.name.0) {
        bindings
            .entry(param_ty.name.0.clone())
            .or_insert_with(|| ConcreteTemplateArg::Type(arg.clone()));
        return;
    }
    // Heads don't match — no further info.
    if param_ty.name.0 != arg.name {
        return;
    }
    // Recurse into template args.
    for (p_tv, a_arg) in param_ty.templates.iter().zip(arg.args.iter()) {
        if let (ast::TypeOrValue::Type(p_ty), ConcreteTemplateArg::Type(a_ct)) = (&p_tv.0, a_arg) {
            unify_type_against_concrete(p_ty, a_ct, type_params, bindings);
        } else if let (ast::TypeOrValue::Type(p_ty), ConcreteTemplateArg::Value(_)) = (&p_tv.0, a_arg)
        {
            // Param expects a type but arg is a value — could be a bare
            // name that's a value template param. Bind that.
            if p_ty.templates.is_empty() && type_params.contains(&p_ty.name.0) {
                bindings
                    .entry(p_ty.name.0.clone())
                    .or_insert_with(|| a_arg.clone());
            }
        }
    }
}

/// Substitute template-param bindings through an AST type tree. Unlike
/// `subst_concrete_arg`, this preserves `qself` prefixes so a later
/// `resolve_qself_deep` pass can resolve them against the bound
/// `self_ty`. Used by return-size lookups when the callee's declared
/// return type is a qualified path like `<T as Add>::Output`.
pub(crate) fn subst_ast_type_with_qself(
    ty: &ast::Type,
    bindings: &std::collections::HashMap<String, ConcreteTemplateArg>,
) -> ast::Type {
    // Case 1: the type head itself is a bound template param AND has no
    // further template args (e.g. bare `T`). Replace wholesale.
    if ty.templates.is_empty() && ty.qself.is_none() {
        if let Some(ConcreteTemplateArg::Type(ct)) = bindings.get(&ty.name.0) {
            return ast::Type {
                name: (ct.name.clone(), 0..0),
                templates: ct
                    .args
                    .iter()
                    .map(|a| (lower_concrete_to_ast_tv(a), 0..0))
                    .collect(),
                qself: None,
            };
        }
    }
    // Case 2: recurse into template args.
    let templates = ty
        .templates
        .iter()
        .map(|(tv, sp)| {
            let new_tv = match tv {
                ast::TypeOrValue::Value(n) => ast::TypeOrValue::Value(*n),
                ast::TypeOrValue::Type(inner) => ast::TypeOrValue::Type(
                    subst_ast_type_with_qself(inner, bindings),
                ),
            };
            (new_tv, sp.clone())
        })
        .collect();
    ast::Type {
        name: ty.name.clone(),
        templates,
        qself: ty.qself.as_ref().map(|q| {
            Box::new(ast::QSelf {
                self_ty: (
                    subst_ast_type_with_qself(&q.self_ty.0, bindings),
                    q.self_ty.1.clone(),
                ),
                trait_ty: (
                    subst_ast_type_with_qself(&q.trait_ty.0, bindings),
                    q.trait_ty.1.clone(),
                ),
            })
        }),
    }
}

pub(crate) fn subst_concrete_arg(
    tv: &ast::TypeOrValue,
    bindings: &std::collections::HashMap<String, ConcreteTemplateArg>,
) -> ConcreteTemplateArg {
    match tv {
        ast::TypeOrValue::Value(n) => ConcreteTemplateArg::Value(*n),
        ast::TypeOrValue::Type(t) => {
            // Leaf template-param reference → direct substitution.
            if t.templates.is_empty() {
                if let Some(bound) = bindings.get(&t.name.0) {
                    return bound.clone();
                }
            }
            ConcreteTemplateArg::Type(ConcreteType {
                name: t.name.0.clone(),
                args: t
                    .templates
                    .iter()
                    .map(|(inner, _)| subst_concrete_arg(inner, bindings))
                    .collect(),
            })
        }
    }
}

/// Inverse of `lower_tv` — convert a `ConcreteTemplateArg` back to an AST
/// `TypeOrValue` so we can rebuild a full `ast::Type` to feed through
/// `resolve_struct_layout`.
pub(crate) fn lower_concrete_to_ast_tv(arg: &ConcreteTemplateArg) -> ast::TypeOrValue {
    match arg {
        ConcreteTemplateArg::Value(n) => ast::TypeOrValue::Value(*n),
        ConcreteTemplateArg::Type(ct) => ast::TypeOrValue::Type(ast::Type {
            name: (ct.name.clone(), 0..0),
            templates: ct
                .args
                .iter()
                .map(|a| (lower_concrete_to_ast_tv(a), 0..0))
                .collect(),
            qself: None,
        }),
    }
}

fn lower_tv(t: &ast::TypeOrValue) -> ConcreteTemplateArg {
    match t {
        ast::TypeOrValue::Value(n) => ConcreteTemplateArg::Value(*n),
        ast::TypeOrValue::Type(ty) => ConcreteTemplateArg::Type(ConcreteType {
            name: ty.name.0.clone(),
            args: ty
                .templates
                .iter()
                .map(|(tv, _)| lower_tv(tv))
                .collect(),
        }),
    }
}

/// Silence unused-import warning: `Either` used only if WriteRegister is emitted.
#[allow(dead_code)]
fn _ensure_either_use(_e: Either<u8, SlotId>) {}
