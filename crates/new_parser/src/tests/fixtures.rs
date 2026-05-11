//! Helpers for building AST values by hand in tests, and fixture snippets.
//!
//! Span equality is waived here: parsing the same text twice should produce
//! the same *shape* of AST, but the specific span ranges depend on how the
//! fixture source is embedded. We use `strip_spans` to zero out every span so
//! the comparison only checks structure.

#![allow(dead_code)]

use crate::ast::*;
use crate::Span;

pub const ZERO: Span = 0..0;

pub fn sp<T>(t: T) -> Spanned<T> {
    (t, ZERO)
}

pub fn ty(name: &str) -> Spanned<Type> {
    sp(Type {
        name: sp(name.to_string()),
        templates: Vec::new(),
        qself: None,
    })
}

pub fn ty_g(name: &str, templates: Vec<Spanned<TypeOrValue>>) -> Spanned<Type> {
    sp(Type {
        name: sp(name.to_string()),
        templates,
        qself: None,
    })
}

pub fn tv_t(name: &str) -> Spanned<TypeOrValue> {
    sp(TypeOrValue::Type(ty(name).0))
}

pub fn tv_g(name: &str, templates: Vec<Spanned<TypeOrValue>>) -> Spanned<TypeOrValue> {
    sp(TypeOrValue::Type(ty_g(name, templates).0))
}

pub fn tv_v(v: i64) -> Spanned<TypeOrValue> {
    sp(TypeOrValue::Value(v))
}

pub fn ident_sp(s: &str) -> Spanned<String> {
    sp(s.to_string())
}

pub fn block(stmts: Vec<Spanned<Expr>>) -> Spanned<Block> {
    sp(Block { stmts })
}

pub fn empty_block() -> Spanned<Block> {
    block(Vec::new())
}

/// Recursively zeroes every span in the tree so structural comparisons work.
pub trait StripSpans {
    fn strip_spans(&mut self);
}

impl StripSpans for Span {
    fn strip_spans(&mut self) {
        *self = ZERO;
    }
}

impl<T: StripSpans> StripSpans for (T, Span) {
    fn strip_spans(&mut self) {
        self.0.strip_spans();
        self.1 = ZERO;
    }
}

impl<T: StripSpans> StripSpans for Vec<T> {
    fn strip_spans(&mut self) {
        for x in self.iter_mut() {
            x.strip_spans();
        }
    }
}

impl<T: StripSpans> StripSpans for Box<T> {
    fn strip_spans(&mut self) {
        (**self).strip_spans();
    }
}

impl<T: StripSpans> StripSpans for Option<T> {
    fn strip_spans(&mut self) {
        if let Some(x) = self.as_mut() {
            x.strip_spans();
        }
    }
}

impl StripSpans for String {
    fn strip_spans(&mut self) {}
}

impl StripSpans for i64 {
    fn strip_spans(&mut self) {}
}

impl StripSpans for bool {
    fn strip_spans(&mut self) {}
}

impl StripSpans for char {
    fn strip_spans(&mut self) {}
}

impl StripSpans for BinOp {
    fn strip_spans(&mut self) {}
}

impl StripSpans for CompoundOp {
    fn strip_spans(&mut self) {}
}

impl StripSpans for Item {
    fn strip_spans(&mut self) {
        match self {
            Item::Struct(s) => s.strip_spans(),
            Item::Enum(e) => e.strip_spans(),
            Item::Extension(e) => e.strip_spans(),
            Item::Trait(t) => t.strip_spans(),
            Item::Impl(i) => i.strip_spans(),
            Item::Const(c) => c.strip_spans(),
            Item::Use(u) => u.name.strip_spans(),
        }
    }
}

impl StripSpans for StructDef {
    fn strip_spans(&mut self) {
        self.name.strip_spans();
        self.templates.strip_spans();
        self.fields.strip_spans();
    }
}

impl StripSpans for FieldDef {
    fn strip_spans(&mut self) {
        self.ty.strip_spans();
        self.name.strip_spans();
    }
}

impl StripSpans for EnumDef {
    fn strip_spans(&mut self) {
        self.name.strip_spans();
        self.templates.strip_spans();
        self.variants.strip_spans();
    }
}

impl StripSpans for EnumVariant {
    fn strip_spans(&mut self) {
        self.name.strip_spans();
        self.data.strip_spans();
        self.discriminant.strip_spans();
    }
}

impl StripSpans for ExtensionDef {
    fn strip_spans(&mut self) {
        self.target.strip_spans();
        self.methods.strip_spans();
    }
}

impl StripSpans for TraitDef {
    fn strip_spans(&mut self) {
        self.name.strip_spans();
        self.templates.strip_spans();
        self.associated_types.strip_spans();
        self.methods.strip_spans();
    }
}

impl StripSpans for ImplDef {
    fn strip_spans(&mut self) {
        self.trait_ty.strip_spans();
        self.target.strip_spans();
        for (n, t) in self.associated_types.iter_mut() {
            n.strip_spans();
            t.strip_spans();
        }
        self.methods.strip_spans();
    }
}

impl StripSpans for ConstDef {
    fn strip_spans(&mut self) {
        self.ty.strip_spans();
        self.name.strip_spans();
        self.value.strip_spans();
    }
}

impl StripSpans for FunctionSig {
    fn strip_spans(&mut self) {
        self.name.strip_spans();
        self.templates.strip_spans();
        self.params.strip_spans();
        self.return_type.strip_spans();
    }
}

impl StripSpans for Function {
    fn strip_spans(&mut self) {
        self.sig.strip_spans();
        self.body.strip_spans();
    }
}

impl StripSpans for Param {
    fn strip_spans(&mut self) {
        self.name.strip_spans();
        self.ty.strip_spans();
    }
}

impl StripSpans for Type {
    fn strip_spans(&mut self) {
        self.name.strip_spans();
        self.templates.strip_spans();
    }
}

impl StripSpans for TypeOrValue {
    fn strip_spans(&mut self) {
        match self {
            TypeOrValue::Type(t) => t.strip_spans(),
            TypeOrValue::Value(_) => {}
        }
    }
}

impl StripSpans for Block {
    fn strip_spans(&mut self) {
        self.stmts.strip_spans();
    }
}

impl StripSpans for Expr {
    fn strip_spans(&mut self) {
        match self {
            Expr::Number(_)
            | Expr::String(_)
            | Expr::Char(_)
            | Expr::Bool(_)
            | Expr::SelfValue
            | Expr::Variable(_)
            | Expr::Break
            | Expr::Continue => {}
            Expr::Field(e, n) => {
                e.strip_spans();
                n.strip_spans();
            }
            Expr::MethodCall {
                receiver,
                name,
                templates,
                args,
            } => {
                receiver.strip_spans();
                name.strip_spans();
                templates.strip_spans();
                args.strip_spans();
            }
            Expr::StaticCall {
                ty,
                name,
                templates,
                args,
            } => {
                ty.strip_spans();
                name.strip_spans();
                templates.strip_spans();
                args.strip_spans();
            }
            Expr::StructLiteral { ty, fields } => {
                ty.strip_spans();
                for (n, v) in fields.iter_mut() {
                    n.strip_spans();
                    v.strip_spans();
                }
            }
            Expr::EnumVariant { ty, variant, data } => {
                ty.strip_spans();
                variant.strip_spans();
                data.strip_spans();
            }
            Expr::BinaryOp(_, l, r) => {
                l.strip_spans();
                r.strip_spans();
            }
            Expr::If { cond, then, else_ } => {
                cond.strip_spans();
                then.strip_spans();
                else_.strip_spans();
            }
            Expr::Loop(b) => b.strip_spans(),
            Expr::For {
                var_ty,
                var_name,
                iter,
                body,
            } => {
                var_ty.strip_spans();
                var_name.strip_spans();
                iter.strip_spans();
                body.strip_spans();
            }
            Expr::Range { start, end, .. } => {
                start.strip_spans();
                end.strip_spans();
            }
            Expr::Return(e) => e.strip_spans(),
            Expr::Match { scrutinee, arms } => {
                scrutinee.strip_spans();
                arms.strip_spans();
            }
            Expr::Block(b) => b.strip_spans(),
            Expr::Declaration {
                ty, name, value, ..
            } => {
                ty.strip_spans();
                name.strip_spans();
                value.strip_spans();
            }
            Expr::Assign { target, value } => {
                target.strip_spans();
                value.strip_spans();
            }
            Expr::CompoundAssign { target, value, .. } => {
                target.strip_spans();
                value.strip_spans();
            }
            Expr::Cast { expr, ty } => {
                expr.strip_spans();
                ty.strip_spans();
            }
        }
    }
}

impl StripSpans for MatchArm {
    fn strip_spans(&mut self) {
        self.pattern.strip_spans();
        self.body.strip_spans();
    }
}

impl StripSpans for Pattern {
    fn strip_spans(&mut self) {
        match self {
            Pattern::Variant {
                ty,
                variant,
                binding,
            } => {
                ty.strip_spans();
                variant.strip_spans();
                binding.strip_spans();
            }
            Pattern::Integer(_) => {}
            Pattern::Range(_, _) => {}
            Pattern::Or(ps) => ps.strip_spans(),
            Pattern::Wildcard => {}
        }
    }
}

impl StripSpans for PatternBinding {
    fn strip_spans(&mut self) {}
}
