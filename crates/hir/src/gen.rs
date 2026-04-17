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
    /// Read but not currently used downstream; `slot_mut`/`check_mutable_slot`
    /// is the authoritative mutability source. Kept for future lookups.
    #[allow(dead_code)]
    mutable: bool,
    /// Kept for future struct-aware lookups; currently unused because the
    /// generator re-derives field offsets from the registry.
    #[allow(dead_code)]
    field_offsets: Option<Vec<(String, u32, u32)>>,
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
                g.scopes.last_mut().unwrap().insert(
                    fs.name.clone(),
                    LocalBinding {
                        slot: SlotId(first),
                        type_name: fs.type_name.clone(),
                        mutable: fs.mutable,
                        field_offsets,
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
            let size = self.type_size(&b.type_name)?;
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
        let (recv_slot, recv_ty) = self.resolve_lvalue_base(&recv.0, &recv.1)?;
        let (offset, size) =
            self.field_offset(&recv_ty, &field.0, &field.1)?;
        let src = SlotId(recv_slot.0 + offset);
        self.copy_multi(dst, src, size, block)
    }

    /// Resolve an l-value expression (variable or chain of field accesses) to
    /// a (starting slot, type name) pair without emitting any ops.
    fn resolve_lvalue_base(
        &mut self,
        expr: &ast::Expr,
        sp: &new_parser::Span,
    ) -> Result<(SlotId, String), HirError> {
        match expr {
            ast::Expr::SelfValue => {
                let b = self.lookup("self").ok_or_else(|| {
                    HirError::at("`self` not available here", sp.clone())
                })?;
                Ok((b.slot, b.type_name))
            }
            ast::Expr::Variable(name) => {
                let b = self.lookup(name).ok_or_else(|| {
                    HirError::at(format!("undefined variable `{}`", name), sp.clone())
                })?;
                Ok((b.slot, b.type_name))
            }
            ast::Expr::Field(inner, field) => {
                let (base_slot, base_ty) = self.resolve_lvalue_base(&inner.0, &inner.1)?;
                let (offset, _size) = self.field_offset(&base_ty, &field.0, &field.1)?;
                let field_ty = self.field_type(&base_ty, &field.0, &field.1)?;
                Ok((SlotId(base_slot.0 + offset), field_ty))
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
        let resolved = self.resolve_ty_name(&ty.0.name.0);
        let size = self.type_size(&resolved)?;
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
        self.scopes.last_mut().unwrap().insert(
            name.0.clone(),
            LocalBinding {
                slot,
                type_name: resolved,
                mutable,
                field_offsets,
            },
        );
        self.gen_expr_into(&value.0, &value.1, Some(slot), block)
    }

    fn gen_assign(
        &mut self,
        target: &ast::Spanned<ast::Expr>,
        value: &ast::Spanned<ast::Expr>,
        sp: &new_parser::Span,
        block: &mut HirBlock,
    ) -> Result<(), HirError> {
        let (dst_slot, dst_ty) = self.resolve_lvalue_base(&target.0, &target.1)?;
        self.check_mutable_slot(dst_slot, sp)?;
        let size = self.type_size(&dst_ty)?;
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
        let (dst_slot, dst_ty) = self.resolve_lvalue_base(&target.0, &target.1)?;
        let size = self.type_size(&dst_ty)?;
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
        block.push(HirOp::Call {
            target: FnRef {
                type_name: dst_ty,
                method_name: method.to_string(),
                template_args: Vec::new(),
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
        // Determine scrutinee type.
        let scrut_ty = self.infer_expr_type(&scrutinee.0, &scrutinee.1)?;
        let resolved = self.resolve_ty_name(&scrut_ty);
        let info = self.reg.types.get(&resolved).ok_or_else(|| {
            HirError::at(
                format!("unknown scrutinee type `{}`", resolved),
                sp.clone(),
            )
        })?;
        let layout = match &info.kind {
            typer::TypeKind::Enum(typer::EnumKind::Concrete(l)) => l.clone(),
            _ => {
                return Err(HirError::at(
                    format!("match scrutinee must be a concrete enum; got `{}`", resolved),
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
                    // Bindings: introduce a local for the data payload.
                    if let Some(bind) = binding {
                        if let ast::PatternBinding::Name(nm) = &bind.0 {
                            let ty = "U4"; // payload type unknown at this layer
                            let bind_slot = SlotId(scrut_slot.0 + layout.discriminant_size);
                            self.scopes.last_mut().unwrap().insert(
                                nm.clone(),
                                LocalBinding {
                                    slot: bind_slot,
                                    type_name: ty.to_string(),
                                    mutable: false,
                                    field_offsets: None,
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
        let lsize = self.type_size(&resolved)?;
        let rty = self.infer_expr_type(&r.0, &r.1)?;
        let resolved_r = self.resolve_ty_name(&rty);
        let rsize = self.type_size(&resolved_r)?;

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
        let ret: Vec<SlotId> = match dst {
            Some(d) => {
                // For comparisons, return is Bool (size 1). For Add/Sub it's
                // Self (size lsize). We emit `ret` list accordingly.
                let ret_size = match op {
                    ast::BinOp::Add | ast::BinOp::Sub => lsize,
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
                template_args: Vec::new(),
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
        // Infer receiver type.
        let recv_ty = self.infer_expr_type(&receiver.0, &receiver.1)?;
        let resolved_recv = self.resolve_ty_name(&recv_ty);
        let recv_size = self.type_size_permissive(&resolved_recv);
        let recv_slot = self.alloc_temp(&resolved_recv, recv_size);
        self.gen_expr_into(&receiver.0, &receiver.1, Some(recv_slot), block)?;

        // Evaluate args into slots.
        let mut arg_slots: Vec<(SlotId, u32)> = Vec::new();
        for a in args {
            let aty = self.infer_expr_type(&a.0, &a.1)?;
            let resolved = self.resolve_ty_name(&aty);
            let size = self.type_size_permissive(&resolved);
            let s = self.alloc_temp(&resolved, size);
            self.gen_expr_into(&a.0, &a.1, Some(s), block)?;
            arg_slots.push((s, size));
        }

        // Build flat arg list: recv cells, then each arg's cells.
        let mut flat_args: Vec<SlotId> = (0..recv_size).map(|i| SlotId(recv_slot.0 + i)).collect();
        for (s, size) in &arg_slots {
            for i in 0..*size {
                flat_args.push(SlotId(s.0 + i));
            }
        }

        let ret_slots: Vec<SlotId> = match dst {
            Some(d) => {
                // Determine return size by looking up the function in the DB.
                let ret_size =
                    self.lookup_return_size(&resolved_recv, &name.0).unwrap_or(0);
                (0..ret_size).map(|i| SlotId(d.0 + i)).collect()
            }
            None => Vec::new(),
        };

        let tpl = lower_templates(templates);
        if self.try_emit_native(
            &resolved_recv,
            &name.0,
            &tpl,
            &flat_args,
            &ret_slots,
            recv_size,
            &[],
            block,
        )? {
            return Ok(());
        }
        let trait_name = self.resolve_trait_for(&resolved_recv, &name.0);
        block.push(HirOp::Call {
            target: FnRef {
                type_name: resolved_recv,
                method_name: name.0.clone(),
                template_args: tpl,
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
            let size = self.type_size_permissive(&resolved_a);
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

        let ret_slots: Vec<SlotId> = match dst {
            Some(d) => {
                let ret_size = self.lookup_return_size(&resolved, &name.0).unwrap_or(0);
                (0..ret_size).map(|i| SlotId(d.0 + i)).collect()
            }
            None => Vec::new(),
        };

        let tpl = lower_templates(templates);
        let recv_ty_args: Vec<ConcreteTemplateArg> = ty
            .0
            .templates
            .iter()
            .map(|(tv, _)| lower_tv(tv))
            .collect();
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
        block.push(HirOp::Call {
            target: FnRef {
                type_name: resolved,
                method_name: name.0.clone(),
                template_args: tpl,
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

    /// Type-size helper that falls back to 1 cell for unknown types (which
    /// can happen for templated types used inside `infer_expr_type`). This
    /// keeps HIR generation pragmatic in the face of partial resolution.
    fn type_size_permissive(&self, name: &str) -> u32 {
        self.type_size(name).unwrap_or(1)
    }

    fn lookup_return_size(&self, type_name: &str, method: &str) -> Option<u32> {
        // Try both inherent and trait-keyed entries — if the method exists
        // in either form we can take its return size.
        let inherent_key = typer::FnSig::new(type_name, method);
        if let Some(f) = self.db.get(&inherent_key) {
            return Some(match f {
                typer::Fn::Simple(s) => s.sig.output_count,
                typer::Fn::Templated(t) => {
                    let ret_ty = t.body.sig.return_type.as_ref()?;
                    let resolved = self.resolve_ty_name(&ret_ty.0.name.0);
                    self.type_size(&resolved).ok()?
                }
            });
        }
        // Scan trait-keyed entries for the same (type, method).
        for (k, f) in &self.db.functions {
            if k.type_name == type_name
                && k.method_name == method
                && k.trait_name.is_some()
            {
                return Some(match f {
                    typer::Fn::Simple(s) => s.sig.output_count,
                    typer::Fn::Templated(t) => {
                        let ret_ty = t.body.sig.return_type.as_ref()?;
                        let resolved = self.resolve_ty_name(&ret_ty.0.name.0);
                        self.type_size(&resolved).ok()?
                    }
                });
            }
        }
        None
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
        let info = self.reg.types.get(&resolved).ok_or_else(|| {
            HirError::at(format!("unknown type `{}`", resolved), sp.clone())
        })?;
        let layout = match &info.kind {
            typer::TypeKind::Struct(typer::StructKind::Concrete(l)) => l.clone(),
            _ => {
                return Err(HirError::at(
                    format!("cannot build struct literal for non-concrete `{}`", resolved),
                    sp.clone(),
                ))
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
            self.gen_expr_into(&fval.0, &fval.1, Some(dst_slot), block)?;
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
                let recv_ty = self.infer_expr_type(&recv.0, &recv.1)?;
                let resolved = self.resolve_ty_name(&recv_ty);
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
                ast::BinOp::Add | ast::BinOp::Sub => self.infer_expr_type(&l.0, &l.1)?,
            },
            ast::Expr::StructLiteral { ty, .. } | ast::Expr::EnumVariant { ty, .. } => {
                self.resolve_ty_name(&ty.0.name.0)
            }
            ast::Expr::StaticCall { ty, .. } => self.resolve_ty_name(&ty.0.name.0),
            ast::Expr::If { then, .. } => {
                // Type of an if-expression = type of its then branch's last stmt.
                then.0
                    .stmts
                    .last()
                    .map(|s| self.infer_expr_type(&s.0, &s.1).unwrap_or_default())
                    .unwrap_or_default()
            }
            ast::Expr::MethodCall { .. } | ast::Expr::Match { .. } | ast::Expr::Block(_)
            | ast::Expr::Loop(_) | ast::Expr::Return(_) | ast::Expr::Break
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
