//! Phase 6 Step 6.2 — monomorphization.
//!
//! When the HIR generator emits a `Call` targeting a templated function, we
//! must instantiate a concrete Simple version with template params replaced
//! by concrete types. This module does the AST substitution + regenerates
//! HIR for the concrete form.
//!
//! Scope: function-level templates (e.g. `fn debug<T>(T a) {}`) work via
//! AST-type substitution. Type-level templates on structs/enums (e.g.
//! `struct Array<T, E, F>`) require native-type providers — see Phase 7.

use std::collections::HashMap;

use new_parser::ast;

use crate::gen_function;
use crate::ir::*;
use crate::HirFunction;

pub type FnSigKey = typer::FnSig;

/// A fully concretized monomorph: the original `FnSig` plus the specific
/// template args used to instantiate it. Two calls with the same name but
/// different template args produce different monomorphs.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MonomorphKey {
    pub sig: FnSigKey,
    pub template_args: Vec<ConcreteTemplateArg>,
}

impl MonomorphKey {
    /// Render a distinct symbol name for this monomorph, useful for
    /// debugging and for re-inserting into a `HashMap<FnSig, HirFunction>`
    /// where the top-level `FnSig` must be unique.
    pub fn mangled(&self) -> FnSigKey {
        if self.template_args.is_empty() {
            return self.sig.clone();
        }
        let args = self
            .template_args
            .iter()
            .map(render_arg)
            .collect::<Vec<_>>()
            .join(",");
        FnSigKey::new(
            format!("{}<{}>", self.sig.type_name, args),
            self.sig.method_name.clone(),
        )
    }
}

fn render_arg(a: &ConcreteTemplateArg) -> String {
    match a {
        ConcreteTemplateArg::Value(n) => n.to_string(),
        ConcreteTemplateArg::Type(t) => {
            if t.args.is_empty() {
                t.name.clone()
            } else {
                format!(
                    "{}<{}>",
                    t.name,
                    t.args.iter().map(render_arg).collect::<Vec<_>>().join(",")
                )
            }
        }
    }
}

// ---------- substitution ---------------------------------------------------

/// Substitute template param names with concrete args in an AST type.
fn subst_type(ty: &ast::Type, bindings: &HashMap<String, ConcreteTemplateArg>) -> ast::Type {
    // Case 1: the type name itself is a bound template param (e.g. `T`).
    if let Some(ConcreteTemplateArg::Type(ct)) = bindings.get(&ty.name.0) {
        // Convert the concrete type back to an AST type. Nested args are
        // converted recursively. The span is kept from the original.
        return concrete_type_to_ast(ct, &ty.name.1).0;
    }
    // Case 2: recurse into template args.
    let templates = ty
        .templates
        .iter()
        .map(|(t, sp)| (subst_tv(t, bindings), sp.clone()))
        .collect();
    ast::Type {
        name: ty.name.clone(),
        templates,
    }
}

fn subst_tv(
    t: &ast::TypeOrValue,
    bindings: &HashMap<String, ConcreteTemplateArg>,
) -> ast::TypeOrValue {
    match t {
        ast::TypeOrValue::Type(ty) => {
            // If this type is just a bound template param, it may instead
            // resolve to an integer value (e.g. `E` → 9 in Array<T, E, F>).
            if ty.templates.is_empty() {
                if let Some(bound) = bindings.get(&ty.name.0) {
                    return match bound {
                        ConcreteTemplateArg::Value(n) => ast::TypeOrValue::Value(*n),
                        ConcreteTemplateArg::Type(ct) => {
                            ast::TypeOrValue::Type(concrete_type_to_ast(ct, &ty.name.1).0)
                        }
                    };
                }
            }
            ast::TypeOrValue::Type(subst_type(ty, bindings))
        }
        ast::TypeOrValue::Value(n) => ast::TypeOrValue::Value(*n),
    }
}

fn concrete_type_to_ast(ct: &ConcreteType, span: &new_parser::Span) -> ast::Spanned<ast::Type> {
    let templates = ct
        .args
        .iter()
        .map(|arg| {
            let tv = match arg {
                ConcreteTemplateArg::Value(n) => ast::TypeOrValue::Value(*n),
                ConcreteTemplateArg::Type(t) => {
                    ast::TypeOrValue::Type(concrete_type_to_ast(t, span).0)
                }
            };
            (tv, span.clone())
        })
        .collect();
    (
        ast::Type {
            name: (ct.name.clone(), span.clone()),
            templates,
        },
        span.clone(),
    )
}

/// Substitute template params inside a function's param types and return type.
fn subst_sig(
    sig: &ast::FunctionSig,
    bindings: &HashMap<String, ConcreteTemplateArg>,
) -> ast::FunctionSig {
    let params = sig
        .params
        .iter()
        .map(|p| ast::Param {
            name: p.name.clone(),
            ty: p
                .ty
                .as_ref()
                .map(|(t, sp)| (subst_type(t, bindings), sp.clone())),
            mutable: p.mutable,
            is_self: p.is_self,
        })
        .collect();
    let return_type = sig
        .return_type
        .as_ref()
        .map(|(t, sp)| (subst_type(t, bindings), sp.clone()));
    ast::FunctionSig {
        name: sig.name.clone(),
        templates: Vec::new(), // concretized
        params,
        return_type,
    }
}

/// Substitute template params inside an expression tree.
fn subst_expr(expr: &ast::Expr, bindings: &HashMap<String, ConcreteTemplateArg>) -> ast::Expr {
    use ast::Expr::*;
    match expr {
        Number(_) | String(_) | Char(_) | Bool(_) | SelfValue | Variable(_) | Break
        | Continue => expr.clone(),
        Field(r, f) => Field(Box::new((subst_expr(&r.0, bindings), r.1.clone())), f.clone()),
        MethodCall {
            receiver,
            name,
            templates,
            args,
        } => MethodCall {
            receiver: Box::new((subst_expr(&receiver.0, bindings), receiver.1.clone())),
            name: name.clone(),
            templates: templates
                .iter()
                .map(|(t, sp)| (subst_tv(t, bindings), sp.clone()))
                .collect(),
            args: args
                .iter()
                .map(|(e, sp)| (subst_expr(e, bindings), sp.clone()))
                .collect(),
        },
        StaticCall {
            ty,
            name,
            templates,
            args,
        } => StaticCall {
            ty: (subst_type(&ty.0, bindings), ty.1.clone()),
            name: name.clone(),
            templates: templates
                .iter()
                .map(|(t, sp)| (subst_tv(t, bindings), sp.clone()))
                .collect(),
            args: args
                .iter()
                .map(|(e, sp)| (subst_expr(e, bindings), sp.clone()))
                .collect(),
        },
        StructLiteral { ty, fields } => StructLiteral {
            ty: (subst_type(&ty.0, bindings), ty.1.clone()),
            fields: fields
                .iter()
                .map(|(n, e)| (n.clone(), (subst_expr(&e.0, bindings), e.1.clone())))
                .collect(),
        },
        EnumVariant { ty, variant, data } => EnumVariant {
            ty: (subst_type(&ty.0, bindings), ty.1.clone()),
            variant: variant.clone(),
            data: data
                .as_ref()
                .map(|d| Box::new((subst_expr(&d.0, bindings), d.1.clone()))),
        },
        BinaryOp(op, l, r) => BinaryOp(
            *op,
            Box::new((subst_expr(&l.0, bindings), l.1.clone())),
            Box::new((subst_expr(&r.0, bindings), r.1.clone())),
        ),
        If { cond, then, else_ } => If {
            cond: Box::new((subst_expr(&cond.0, bindings), cond.1.clone())),
            then: Box::new((subst_block(&then.0, bindings), then.1.clone())),
            else_: else_
                .as_ref()
                .map(|e| Box::new((subst_expr(&e.0, bindings), e.1.clone()))),
        },
        Loop(b) => Loop(Box::new((subst_block(&b.0, bindings), b.1.clone()))),
        Return(e) => Return(
            e.as_ref()
                .map(|v| Box::new((subst_expr(&v.0, bindings), v.1.clone()))),
        ),
        Match { scrutinee, arms } => Match {
            scrutinee: Box::new((subst_expr(&scrutinee.0, bindings), scrutinee.1.clone())),
            arms: arms
                .iter()
                .map(|arm| ast::MatchArm {
                    pattern: (subst_pattern(&arm.pattern.0, bindings), arm.pattern.1.clone()),
                    body: (subst_expr(&arm.body.0, bindings), arm.body.1.clone()),
                })
                .collect(),
        },
        Block(b) => Block(Box::new((subst_block(&b.0, bindings), b.1.clone()))),
        Declaration {
            mutable,
            ty,
            name,
            value,
        } => Declaration {
            mutable: *mutable,
            ty: (subst_type(&ty.0, bindings), ty.1.clone()),
            name: name.clone(),
            value: Box::new((subst_expr(&value.0, bindings), value.1.clone())),
        },
        Assign { target, value } => Assign {
            target: Box::new((subst_expr(&target.0, bindings), target.1.clone())),
            value: Box::new((subst_expr(&value.0, bindings), value.1.clone())),
        },
        CompoundAssign { op, target, value } => CompoundAssign {
            op: *op,
            target: Box::new((subst_expr(&target.0, bindings), target.1.clone())),
            value: Box::new((subst_expr(&value.0, bindings), value.1.clone())),
        },
        Cast { expr, ty } => Cast {
            expr: Box::new((subst_expr(&expr.0, bindings), expr.1.clone())),
            ty: (subst_type(&ty.0, bindings), ty.1.clone()),
        },
    }
}

fn subst_block(
    b: &ast::Block,
    bindings: &HashMap<String, ConcreteTemplateArg>,
) -> ast::Block {
    ast::Block {
        stmts: b
            .stmts
            .iter()
            .map(|(e, sp)| (subst_expr(e, bindings), sp.clone()))
            .collect(),
    }
}

fn subst_pattern(
    p: &ast::Pattern,
    bindings: &HashMap<String, ConcreteTemplateArg>,
) -> ast::Pattern {
    match p {
        ast::Pattern::Variant {
            ty,
            variant,
            binding,
        } => ast::Pattern::Variant {
            ty: (subst_type(&ty.0, bindings), ty.1.clone()),
            variant: variant.clone(),
            binding: binding.clone(),
        },
        ast::Pattern::Wildcard => ast::Pattern::Wildcard,
    }
}

// ---------- main entry point -----------------------------------------------

/// Given a templated function and the template args it's being called with,
/// build a concrete `SimpleFn` and compile it to HIR.
///
/// `self_type_override` is the enclosing type's name if the method's type
/// itself is generic (so the inner `Self` references resolve to the
/// concrete type, e.g. `Array<Cell, 9, U4>` → name "Array" stays but the
/// registry doesn't know this exact instantiation yet).
pub fn monomorphize(
    key: &MonomorphKey,
    templated: &typer::TemplatedFn,
    reg: &typer::TypeRegistry,
    db: &typer::FunctionDB,
) -> Result<HirFunction, String> {
    if templated.templates.len() != key.template_args.len() {
        return Err(format!(
            "template arg mismatch: {} expected, {} given",
            templated.templates.len(),
            key.template_args.len(),
        ));
    }
    let mut bindings: HashMap<String, ConcreteTemplateArg> = HashMap::new();
    for (name, arg) in templated.templates.iter().zip(key.template_args.iter()) {
        bindings.insert(name.clone(), arg.clone());
    }

    // Substitute into the function body.
    let new_sig = subst_sig(&templated.body.sig, &bindings);
    let new_body_block = subst_block(&templated.body.body.0, &bindings);
    let new_fn = ast::Function {
        sig: new_sig.clone(),
        body: (new_body_block, templated.body.body.1.clone()),
    };

    // Build a concrete FlatSig against the registry.
    let flat = typer::FlatSig::flatten(&new_sig, &templated.type_name, reg).map_err(
        |e| format!("flatten for {:?}: {}", key, e.message),
    )?;

    let simple = typer::SimpleFn {
        body: new_fn,
        sig: flat,
        type_name: templated.type_name.clone(),
        from_trait: templated.from_trait.clone(),
    };

    // Compile to HIR via the regular generator.
    let mangled = key.mangled();
    gen_function(&mangled, &simple, reg, db).map_err(|e| e.to_string())
}
