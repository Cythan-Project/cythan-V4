//! AST-driven type inference + scope walking.
//!
//! The typer already gives us a `TypeRegistry` for cross-file
//! symbol resolution, but to answer "what type is this
//! expression under the cursor?" we also need to walk the open
//! file's AST. This module holds:
//!
//! * [`LocalEnv`] — the `self` type + local name → type bindings
//!   visible at a cursor position.
//! * [`infer_expr_type`] — recursive type inference over
//!   expressions, handling `self`, variables, fields, method
//!   calls, static calls, struct / enum literals, and casts.
//! * [`method_call_at`] / [`find_at_cursor`] — locate the AST
//!   node(s) the cursor is sitting on so we can ask questions
//!   about them.
//! * [`parse_lenient`] — retry-on-failure parser that blanks out
//!   the partial expression at the cursor so completion works
//!   mid-edit.

use std::collections::HashMap;

use new_parser::ast;

/// Snapshot of the receiver-name → type bindings visible at a
/// particular cursor position.
pub(crate) struct LocalEnv {
    /// Type the enclosing method's `self` resolves to. `None` for
    /// free functions (which the language doesn't have today).
    pub(crate) self_ty: Option<String>,
    /// `name` → declared type name, lowest in the file wins
    /// (mirrors lexical scoping for shadowing).
    pub(crate) bindings: HashMap<String, String>,
}

/// Describes what AST node the cursor is sitting on. Some
/// fields are unused by current callers but retained so new
/// handlers can reach them without a second AST walk.
#[allow(dead_code)]
pub(crate) enum CursorNode<'a> {
    /// Cursor is on the name-token of a method call; the
    /// receiver expression is attached.
    MethodCall(&'a ast::Spanned<ast::Expr>, &'a ast::Spanned<String>),
    /// Cursor is on the field-name of a `Field` expression; the
    /// receiver is attached.
    FieldAccess(&'a ast::Spanned<ast::Expr>, &'a ast::Spanned<String>),
    /// Cursor is on the name-token of a static call (`Type::name(…)`).
    StaticCall(&'a ast::Spanned<ast::Type>, &'a ast::Spanned<String>),
}

/// Find the enclosing extension/impl method whose body contains
/// `pos_off`, and return (env, the method's AST) for that
/// position. Useful for any feature that needs the local scope.
pub(crate) fn enclosing_method_env<'a>(
    items: &'a [ast::Spanned<ast::Item>],
    pos_off: usize,
) -> Option<(LocalEnv, &'a ast::Function)> {
    for item in items {
        let (target_ty, methods) = match &item.0 {
            ast::Item::Extension(e) => (Some(&e.target.0), &e.methods),
            ast::Item::Impl(i) => (Some(&i.target.0), &i.methods),
            _ => continue,
        };
        for m in methods {
            let body_sp = &m.0.body.1;
            if pos_off < body_sp.start || pos_off > body_sp.end {
                continue;
            }
            let mut env = LocalEnv {
                self_ty: target_ty.map(|t| t.name.0.clone()),
                bindings: HashMap::new(),
            };
            populate_env_from_method(&mut env, &m.0, pos_off);
            return Some((env, &m.0));
        }
    }
    None
}

/// Build a `LocalEnv` at `pos_off` without an anchor on any
/// specific method name. Used by completion, where the user
/// hasn't typed the method yet.
pub(crate) fn local_env_at(
    items: &[ast::Spanned<ast::Item>],
    pos_off: usize,
) -> Option<LocalEnv> {
    enclosing_method_env(items, pos_off).map(|(env, _)| env)
}

/// Populate `env.bindings` with `(name → type)` for every param
/// and every `Declaration` before `pos_off`. Rewrites a declared
/// `Self` type to the enclosing target type's name so downstream
/// lookups don't have to.
pub(crate) fn populate_env_from_method(
    env: &mut LocalEnv,
    f: &ast::Function,
    pos_off: usize,
) {
    use ast::Expr;
    let resolve_self_alias = |env: &LocalEnv, t: String| -> String {
        if t == "Self" {
            env.self_ty.clone().unwrap_or(t)
        } else {
            t
        }
    };
    for p in &f.sig.params {
        let pname = p.name.0.clone();
        let pty = p
            .ty
            .as_ref()
            .map(|t| t.0.name.0.clone())
            .or_else(|| {
                if p.is_self {
                    env.self_ty.clone()
                } else {
                    None
                }
            });
        if let Some(ty) = pty {
            let resolved = resolve_self_alias(env, ty);
            env.bindings.insert(pname, resolved);
        }
    }
    let self_ty_snapshot = env.self_ty.clone();
    walk_block(&f.body.0, &mut |sp| {
        if let Expr::Declaration { name, ty, .. } = &sp.0 {
            if sp.1.start <= pos_off {
                let raw = ty.0.name.0.clone();
                let resolved = if raw == "Self" {
                    self_ty_snapshot.clone().unwrap_or(raw)
                } else {
                    raw
                };
                env.bindings.insert(name.0.clone(), resolved);
            }
        }
    });
}

/// `(env, receiver)` for the MethodCall whose `name` token
/// covers `pos_off` and matches `method_name`.
pub(crate) fn method_call_at<'a>(
    items: &'a [ast::Spanned<ast::Item>],
    pos_off: usize,
    method_name: &str,
) -> Option<(LocalEnv, &'a ast::Spanned<ast::Expr>)> {
    let (env, f) = enclosing_method_env(items, pos_off)?;
    let mc = find_method_call(&f.body.0, pos_off, method_name)?;
    if let ast::Expr::MethodCall { receiver, .. } = &mc.0 {
        return Some((env, receiver));
    }
    None
}

/// Locate the AST node whose name-token covers `pos_off`.
/// Returns the node shape so callers can act on it — MethodCall
/// / FieldAccess / StaticCall. `None` if the cursor isn't on a
/// recognisable name token.
pub(crate) fn find_at_cursor<'a>(
    items: &'a [ast::Spanned<ast::Item>],
    pos_off: usize,
) -> Option<CursorNode<'a>> {
    let (_env, f) = enclosing_method_env(items, pos_off)?;
    find_cursor_in_block(&f.body.0, pos_off)
}

fn find_cursor_in_block<'a>(
    block: &'a ast::Block,
    pos_off: usize,
) -> Option<CursorNode<'a>> {
    for stmt in &block.stmts {
        if let Some(n) = find_cursor_in_expr(stmt, pos_off) {
            return Some(n);
        }
    }
    None
}

fn find_cursor_in_expr<'a>(
    e: &'a ast::Spanned<ast::Expr>,
    pos_off: usize,
) -> Option<CursorNode<'a>> {
    use ast::Expr;
    // Check this node first — if the cursor sits on its name
    // token, we're done. Otherwise recurse into children.
    match &e.0 {
        Expr::MethodCall { receiver: _, name, .. } => {
            if pos_off >= name.1.start && pos_off <= name.1.end {
                if let Expr::MethodCall { receiver, .. } = &e.0 {
                    return Some(CursorNode::MethodCall(receiver, name));
                }
            }
        }
        Expr::Field(recv, field) => {
            if pos_off >= field.1.start && pos_off <= field.1.end {
                return Some(CursorNode::FieldAccess(recv, field));
            }
            if let Some(n) = find_cursor_in_expr(recv, pos_off) {
                return Some(n);
            }
        }
        Expr::StaticCall { ty, name, .. } => {
            if pos_off >= name.1.start && pos_off <= name.1.end {
                return Some(CursorNode::StaticCall(ty, name));
            }
        }
        _ => {}
    }
    // Recurse into children for the remaining shapes.
    match &e.0 {
        Expr::MethodCall { receiver, args, .. } => find_cursor_in_expr(receiver, pos_off)
            .or_else(|| args.iter().find_map(|a| find_cursor_in_expr(a, pos_off))),
        Expr::Field(_, _) => None, // already handled above
        Expr::StaticCall { args, .. } => {
            args.iter().find_map(|a| find_cursor_in_expr(a, pos_off))
        }
        Expr::StructLiteral { fields, .. } => fields
            .iter()
            .find_map(|(_, v)| find_cursor_in_expr(v, pos_off)),
        Expr::EnumVariant { data: Some(d), .. } => find_cursor_in_expr(d, pos_off),
        Expr::BinaryOp(_, l, r) => find_cursor_in_expr(l, pos_off)
            .or_else(|| find_cursor_in_expr(r, pos_off)),
        Expr::If { cond, then, else_ } => find_cursor_in_expr(cond, pos_off)
            .or_else(|| find_cursor_in_block(&then.0, pos_off))
            .or_else(|| else_.as_ref().and_then(|e| find_cursor_in_expr(e, pos_off))),
        Expr::Loop(b) | Expr::Block(b) => find_cursor_in_block(&b.0, pos_off),
        Expr::Match { scrutinee, arms } => find_cursor_in_expr(scrutinee, pos_off)
            .or_else(|| arms.iter().find_map(|a| find_cursor_in_expr(&a.body, pos_off))),
        Expr::Return(Some(x)) => find_cursor_in_expr(x, pos_off),
        Expr::Declaration { value, .. } => find_cursor_in_expr(value, pos_off),
        Expr::Assign { target, value } => find_cursor_in_expr(target, pos_off)
            .or_else(|| find_cursor_in_expr(value, pos_off)),
        Expr::CompoundAssign { target, value, .. } => find_cursor_in_expr(target, pos_off)
            .or_else(|| find_cursor_in_expr(value, pos_off)),
        Expr::Cast { expr, .. } => find_cursor_in_expr(expr, pos_off),
        _ => None,
    }
}

fn find_method_call<'a>(
    block: &'a ast::Block,
    pos_off: usize,
    method_name: &str,
) -> Option<&'a ast::Spanned<ast::Expr>> {
    for stmt in &block.stmts {
        if let Some(found) = find_in_expr(stmt, pos_off, method_name) {
            return Some(found);
        }
    }
    None
}

fn find_in_expr<'a>(
    e: &'a ast::Spanned<ast::Expr>,
    pos_off: usize,
    method_name: &str,
) -> Option<&'a ast::Spanned<ast::Expr>> {
    use ast::Expr;
    if let Expr::MethodCall { name, .. } = &e.0 {
        if name.0 == method_name && pos_off >= name.1.start && pos_off <= name.1.end {
            return Some(e);
        }
    }
    match &e.0 {
        Expr::Field(recv, _) => find_in_expr(recv, pos_off, method_name),
        Expr::MethodCall { receiver, args, .. } => find_in_expr(receiver, pos_off, method_name)
            .or_else(|| args.iter().find_map(|a| find_in_expr(a, pos_off, method_name))),
        Expr::StaticCall { args, .. } => {
            args.iter().find_map(|a| find_in_expr(a, pos_off, method_name))
        }
        Expr::StructLiteral { fields, .. } => fields
            .iter()
            .find_map(|(_, v)| find_in_expr(v, pos_off, method_name)),
        Expr::EnumVariant { data: Some(d), .. } => find_in_expr(d, pos_off, method_name),
        Expr::BinaryOp(_, l, r) => find_in_expr(l, pos_off, method_name)
            .or_else(|| find_in_expr(r, pos_off, method_name)),
        Expr::If { cond, then, else_ } => find_in_expr(cond, pos_off, method_name)
            .or_else(|| find_method_call(&then.0, pos_off, method_name))
            .or_else(|| {
                else_
                    .as_ref()
                    .and_then(|e| find_in_expr(e, pos_off, method_name))
            }),
        Expr::Loop(b) | Expr::Block(b) => find_method_call(&b.0, pos_off, method_name),
        Expr::Match { scrutinee, arms } => find_in_expr(scrutinee, pos_off, method_name)
            .or_else(|| arms.iter().find_map(|a| find_in_expr(&a.body, pos_off, method_name))),
        Expr::Return(Some(x)) => find_in_expr(x, pos_off, method_name),
        Expr::Declaration { value, .. } => find_in_expr(value, pos_off, method_name),
        Expr::Assign { target, value } => find_in_expr(target, pos_off, method_name)
            .or_else(|| find_in_expr(value, pos_off, method_name)),
        Expr::CompoundAssign { target, value, .. } => find_in_expr(target, pos_off, method_name)
            .or_else(|| find_in_expr(value, pos_off, method_name)),
        Expr::Cast { expr, .. } => find_in_expr(expr, pos_off, method_name),
        _ => None,
    }
}

/// What is `Self` at this cursor position? Returns the enclosing
/// extension/impl target type's name.
pub(crate) fn enclosing_self_type(
    items: &[ast::Spanned<ast::Item>],
    pos_off: usize,
) -> Option<String> {
    for item in items {
        let (target_ty, methods) = match &item.0 {
            ast::Item::Extension(e) => (Some(&e.target.0), &e.methods),
            ast::Item::Impl(i) => (Some(&i.target.0), &i.methods),
            _ => continue,
        };
        for m in methods {
            let body_sp = &m.0.body.1;
            if pos_off >= body_sp.start && pos_off <= body_sp.end {
                return target_ty.map(|t| t.name.0.clone());
            }
        }
    }
    None
}

/// Infer the bare type name of `expr`.
pub(crate) fn infer_expr_type(
    expr: &ast::Spanned<ast::Expr>,
    env: &LocalEnv,
    reg: &typer::TypeRegistry,
) -> Option<String> {
    use ast::Expr;
    match &expr.0 {
        Expr::SelfValue => env.self_ty.clone(),
        Expr::Variable(name) => env.bindings.get(name).cloned(),
        Expr::Field(recv, field) => {
            let recv_ty = infer_expr_type(recv, env, reg)?;
            field_type(reg, &recv_ty, &field.0)
        }
        Expr::MethodCall { receiver, name, .. } => {
            let recv_ty = infer_expr_type(receiver, env, reg)?;
            method_return_type(reg, &recv_ty, &name.0)
        }
        Expr::StaticCall { ty, name, .. } => method_return_type(reg, &ty.0.name.0, &name.0),
        Expr::StructLiteral { ty, .. } => Some(ty.0.name.0.clone()),
        Expr::EnumVariant { ty, .. } => Some(ty.0.name.0.clone()),
        Expr::Cast { ty, .. } => Some(ty.0.name.0.clone()),
        _ => None,
    }
}

/// The declared type of `field_name` on `ty_name` — bare type
/// name, no template args.
pub(crate) fn field_type(
    reg: &typer::TypeRegistry,
    ty_name: &str,
    field_name: &str,
) -> Option<String> {
    let info = reg.get_type(ty_name)?;
    let typer::TypeKind::Struct(kind) = &info.kind else {
        return None;
    };
    match kind {
        typer::StructKind::Concrete(layout) => layout
            .fields
            .iter()
            .find(|f| f.name == field_name)
            .map(|f| f.ast_type.name.0.clone()),
        typer::StructKind::Templated { fields } => fields
            .iter()
            .find(|(n, _)| n == field_name)
            .map(|(_, t)| t.name.0.clone()),
    }
}

/// Declared return type of `method_name` on `ty_name`.
pub(crate) fn method_return_type(
    reg: &typer::TypeRegistry,
    ty_name: &str,
    method_name: &str,
) -> Option<String> {
    let info = reg.get_type(ty_name)?;
    for m in &info.methods {
        if m.function.sig.name.0 == method_name {
            return m.function.sig.return_type.as_ref().map(|t| t.0.name.0.clone());
        }
    }
    None
}

/// Walk every expression in `block` recursively.
pub(crate) fn walk_block(
    block: &ast::Block,
    f: &mut dyn FnMut(&ast::Spanned<ast::Expr>),
) {
    for stmt in &block.stmts {
        walk_expr(stmt, f);
    }
}

fn walk_expr(
    e: &ast::Spanned<ast::Expr>,
    f: &mut dyn FnMut(&ast::Spanned<ast::Expr>),
) {
    use ast::Expr;
    f(e);
    match &e.0 {
        Expr::Field(recv, _) => walk_expr(recv, f),
        Expr::MethodCall { receiver, args, .. } => {
            walk_expr(receiver, f);
            for a in args {
                walk_expr(a, f);
            }
        }
        Expr::StaticCall { args, .. } => {
            for a in args {
                walk_expr(a, f);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, v) in fields {
                walk_expr(v, f);
            }
        }
        Expr::EnumVariant { data: Some(d), .. } => walk_expr(d, f),
        Expr::BinaryOp(_, l, r) => {
            walk_expr(l, f);
            walk_expr(r, f);
        }
        Expr::If { cond, then, else_ } => {
            walk_expr(cond, f);
            walk_block(&then.0, f);
            if let Some(e) = else_ {
                walk_expr(e, f);
            }
        }
        Expr::Loop(b) => walk_block(&b.0, f),
        Expr::Block(b) => walk_block(&b.0, f),
        Expr::Match { scrutinee, arms } => {
            walk_expr(scrutinee, f);
            for arm in arms {
                walk_expr(&arm.body, f);
            }
        }
        Expr::Return(Some(x)) => walk_expr(x, f),
        Expr::Declaration { value, .. } => walk_expr(value, f),
        Expr::Assign { target, value } => {
            walk_expr(target, f);
            walk_expr(value, f);
        }
        Expr::CompoundAssign { target, value, .. } => {
            walk_expr(target, f);
            walk_expr(value, f);
        }
        Expr::Cast { expr, .. } => walk_expr(expr, f),
        _ => {}
    }
}

/// Parse the file, retrying with the partial expression at the
/// cursor blanked out if the literal text doesn't parse.
pub(crate) fn parse_lenient(
    text: &str,
    pos_off: usize,
) -> Option<Vec<ast::Spanned<ast::Item>>> {
    if let Ok(items) = new_parser::parse(text) {
        return Some(items);
    }
    let chars: Vec<char> = text.chars().collect();
    let dot = (0..pos_off.min(chars.len())).rev().find(|&i| chars[i] == '.')?;
    let mut end = pos_off.min(chars.len());
    while end < chars.len() && !matches!(chars[end], ';' | '}' | '\n' | ',' | ')') {
        end += 1;
    }
    let mut sanitized = String::with_capacity(text.len());
    for (i, c) in chars.iter().enumerate() {
        if i >= dot && i < end {
            sanitized.push(' ');
        } else {
            sanitized.push(*c);
        }
    }
    new_parser::parse(&sanitized).ok()
}

/// Pretty-print a method parameter list for hover / completion
/// detail text.
pub(crate) fn method_param_string(params: &[ast::Param]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for p in params {
        if p.is_self {
            parts.push(if p.mutable { "mut self".into() } else { "self".into() });
            continue;
        }
        let ty = p
            .ty
            .as_ref()
            .map(|t| t.0.name.0.clone())
            .unwrap_or_default();
        parts.push(if p.mutable {
            format!("mut {} {}", ty, p.name.0)
        } else {
            format!("{} {}", ty, p.name.0)
        });
    }
    parts.join(", ")
}
