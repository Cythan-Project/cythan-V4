//! Cythan Language Server.
//!
//! Minimal LSP that exposes the existing `cythan_driver::diagnose`
//! pipeline over stdio. Spawned by the VS Code extension (and any
//! other LSP-aware client). Capabilities:
//!
//! * Text-document sync (full document, on open / change / save).
//! * Push diagnostics — errors and warnings from parse / typer /
//!   HIR-gen, rendered with their `DiagCode` and labelled spans.
//!
//! Multi-file support is best-effort: when a workspace folder is
//! open, the server picks up any `.ct` files under
//! `<workspace>/<std-dir>` (default `examples/new_syntax/std`)
//! and includes them in each `diagnose` call so cross-file
//! symbols resolve. The client can override the std dir via the
//! `cythan.stdDir` initialization option.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_server::{Connection, Message, Notification};
use lsp_types::notification::{Notification as LspNotification, PublishDiagnostics};
use lsp_types::*;

use errors::{Diagnostic as ErrDiag, LabelKind, Severity};

fn main() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    eprintln!("cythan-lsp: starting");
    let (connection, io_threads) = Connection::stdio();

    let server_capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::FULL,
        )),
        definition_provider: Some(OneOf::Left(true)),
        ..Default::default()
    };
    let init_params = connection
        .initialize(serde_json::to_value(server_capabilities)?)?;

    let mut state = State::from_init(&init_params);
    main_loop(&connection, &mut state)?;
    io_threads.join()?;
    Ok(())
}

struct State {
    /// Buffer of currently-open documents (URI → full text).
    docs: HashMap<Url, String>,
    /// Standard library directory (resolved relative to the
    /// workspace root if there is one). When present, every
    /// `.ct` file inside is loaded alongside the open document
    /// so cross-file symbols resolve.
    std_dir: Option<PathBuf>,
    /// Last symbol index, refreshed on every diagnose call.
    /// Drives go-to-definition.
    symbols: SymbolIndex,
    /// Parsed AST per open document. Used by goto-definition to
    /// infer the type of a method-call receiver from local
    /// declarations / params / `self`.
    asts: HashMap<Url, Vec<new_parser::ast::Spanned<new_parser::ast::Item>>>,
    /// Last-built type registry. Required by receiver-type
    /// inference to look up struct fields and method return types.
    registry: Option<typer::TypeRegistry>,
}

/// Cross-file symbol index for go-to-definition. Stores the
/// declaration `Location` of every type and every method, keyed
/// by name. Cleared and rebuilt after each `diagnose` call so
/// the index reflects whatever the user has saved on disk.
#[derive(Default, Debug)]
struct SymbolIndex {
    /// Type name → declaration location.
    types: HashMap<String, Location>,
    /// Trait name → declaration location.
    traits: HashMap<String, Location>,
    /// (type_name, method_name) → declaration location of the
    /// method's `name` token (extension or impl).
    methods: HashMap<(String, String), Location>,
    /// Method name → list of (type_name, location). Used as a
    /// fallback when a method's receiver type can't be inferred
    /// at the cursor (e.g. plain `.method()` on something the
    /// LSP doesn't type-check). Returning every match lets the
    /// editor present a chooser.
    methods_by_name: HashMap<String, Vec<Location>>,
}

impl State {
    fn from_init(init: &serde_json::Value) -> Self {
        let workspace_root = init
            .get("rootUri")
            .and_then(|v| v.as_str())
            .and_then(|s| Url::parse(s).ok())
            .and_then(|u| u.to_file_path().ok())
            .or_else(|| {
                init.get("workspaceFolders")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(|f| f.get("uri"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| Url::parse(s).ok())
                    .and_then(|u| u.to_file_path().ok())
            });
        let configured_std = init
            .get("initializationOptions")
            .and_then(|v| v.get("stdDir"))
            .and_then(|v| v.as_str())
            .map(PathBuf::from);
        let std_dir = configured_std
            .filter(|p| p.is_dir())
            .or_else(|| workspace_root.as_ref().and_then(|r| find_std_dir(r)));
        eprintln!("cythan-lsp: std_dir = {:?}", std_dir);
        State {
            docs: HashMap::new(),
            std_dir,
            symbols: SymbolIndex::default(),
            asts: HashMap::new(),
            registry: None,
        }
    }
}

/// Locate the Cythan stdlib relative to a workspace folder. Tries
/// (in order): `<root>/std`, `<root>/examples/new_syntax/std`, then
/// walks up parent directories looking for either layout. Returns
/// the first directory found.
fn find_std_dir(start: &std::path::Path) -> Option<PathBuf> {
    let candidates = |dir: &std::path::Path| {
        vec![
            dir.join("std"),
            dir.join("examples").join("new_syntax").join("std"),
        ]
    };
    let mut dir = start.to_path_buf();
    loop {
        for c in candidates(&dir) {
            if c.is_dir() {
                return Some(c);
            }
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p.to_path_buf(),
            _ => return None,
        }
    }
}

fn main_loop(
    connection: &Connection,
    state: &mut State,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                handle_request(connection, state, req);
            }
            Message::Notification(not) => handle_notification(connection, state, not),
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn handle_request(connection: &Connection, state: &State, req: lsp_server::Request) {
    use lsp_types::request::{GotoDefinition, Request as LspRequest};
    let id = req.id.clone();
    if req.method == GotoDefinition::METHOD {
        let result = serde_json::from_value::<GotoDefinitionParams>(req.params)
            .ok()
            .and_then(|params| {
                resolve_definition(
                    state,
                    &params.text_document_position_params.text_document.uri,
                    params.text_document_position_params.position,
                )
            });
        let value = match result {
            Some(loc) => serde_json::to_value(GotoDefinitionResponse::Scalar(loc))
                .unwrap_or(serde_json::Value::Null),
            None => serde_json::Value::Null,
        };
        let _ = connection.sender.send(Message::Response(lsp_server::Response {
            id,
            result: Some(value),
            error: None,
        }));
    }
}

/// Resolve a `textDocument/definition` request: find the
/// identifier under the cursor in the open document, then look
/// it up in the symbol index. Tries (in order):
///   1. `Type::method` — exact `(Type, method)` lookup.
///   2. `recv.method` — infer `recv`'s type from the enclosing
///      method's params / locals / `self`, then exact lookup.
///      Falls back to "any method with this name" only when
///      inference doesn't give a concrete type.
///   3. Bare identifier — type, trait, then method-by-name.
fn resolve_definition(state: &State, uri: &Url, pos: Position) -> Option<Location> {
    let text = state.docs.get(uri)?;
    let (word, prev_marker) = identifier_at(text, pos)?;
    if word.is_empty() {
        return None;
    }

    if let IdContext::AfterColons = prev_marker {
        if let Some(receiver_token) = receiver_before(text, pos) {
            // Try exact `(Type, method)` match first.
            if let Some(loc) = state
                .symbols
                .methods
                .get(&(receiver_token.clone(), word.clone()))
            {
                return Some(loc.clone());
            }
            // `Self::method` when we know the enclosing extension's
            // target type.
            if receiver_token == "Self" {
                let pos_off = position_to_char_offset(text, pos)?;
                if let Some(items) = state.asts.get(uri) {
                    if let Some(self_ty) = enclosing_self_type(items, pos_off) {
                        if let Some(loc) = state
                            .symbols
                            .methods
                            .get(&(self_ty, word.clone()))
                        {
                            return Some(loc.clone());
                        }
                    }
                }
            }
        }
    }

    if let IdContext::AfterDot = prev_marker {
        // Find the MethodCall AST node containing the cursor and
        // infer the receiver expression's type. This handles
        // chained shapes like `self.r0.get(col)` whose receiver
        // is a Field, not a bare identifier.
        let pos_off = position_to_char_offset(text, pos)?;
        if let (Some(items), Some(reg)) =
            (state.asts.get(uri), state.registry.as_ref())
        {
            if let Some((env, recv)) = method_call_at(items, pos_off, &word) {
                if let Some(ty) = infer_expr_type(recv, &env, reg) {
                    if let Some(loc) = state
                        .symbols
                        .methods
                        .get(&(ty.clone(), word.clone()))
                    {
                        return Some(loc.clone());
                    }
                    eprintln!(
                        "cythan-lsp: inferred recv type `{}` for `.{}` but \
                         no method match",
                        ty, word
                    );
                }
            }
        }
        // Last resort — could not infer, fall back to any
        // matching method name. Only when there's a single
        // candidate, to avoid jumping to an arbitrary pick.
        if let Some(matches) = state.symbols.methods_by_name.get(&word) {
            if matches.len() == 1 {
                return Some(matches[0].clone());
            }
            return None;
        }
    }

    if let Some(loc) = state.symbols.types.get(&word) {
        return Some(loc.clone());
    }
    if let Some(loc) = state.symbols.traits.get(&word) {
        return Some(loc.clone());
    }
    None
}

/// What immediately precedes the identifier at the cursor —
/// helps decide whether to interpret it as a method, an
/// associated function, or a free symbol.
enum IdContext {
    None,
    AfterDot,
    AfterColons,
}

/// Extract the identifier the cursor is inside of (or adjacent
/// to). Returns the identifier text and a tag describing what
/// punctuation, if any, immediately precedes it.
fn identifier_at(text: &str, pos: Position) -> Option<(String, IdContext)> {
    let offset = position_to_char_offset(text, pos)?;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    // Find the start of the identifier the cursor sits on.
    let mut start = offset.min(chars.len());
    while start > 0 && is_id(chars[start - 1]) {
        start -= 1;
    }
    let mut end = offset.min(chars.len());
    while end < chars.len() && is_id(chars[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    let word: String = chars[start..end].iter().collect();
    // Look at the char(s) immediately before `start` to figure
    // out the surrounding punctuation context.
    let ctx = match start {
        0 => IdContext::None,
        1 => match chars[0] {
            '.' => IdContext::AfterDot,
            _ => IdContext::None,
        },
        _ => match (chars[start - 2], chars[start - 1]) {
            (':', ':') => IdContext::AfterColons,
            (_, '.') => IdContext::AfterDot,
            _ => IdContext::None,
        },
    };
    Some((word, ctx))
}

/// Identifier (or `self`) immediately before a `.` at `pos`.
/// Returns `None` if the receiver is a complex expression (we
/// only handle the common single-identifier case).
#[allow(dead_code)]
fn receiver_name_before_dot(text: &str, pos: Position) -> Option<String> {
    let offset = position_to_char_offset(text, pos)?;
    let chars: Vec<char> = text.chars().collect();
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    // Walk back through the identifier under cursor.
    let mut i = offset.min(chars.len());
    while i > 0 && is_id(chars[i - 1]) {
        i -= 1;
    }
    if i == 0 || chars[i - 1] != '.' {
        return None;
    }
    let mut j = i - 1;
    while j > 0 && is_id(chars[j - 1]) {
        j -= 1;
    }
    let recv: String = chars[j..i - 1].iter().collect();
    if recv.is_empty() {
        None
    } else {
        Some(recv)
    }
}

/// Snip out the receiver identifier directly before a `::` at
/// `pos` (inclusive of `pos`'s token's start). Used to map
/// `Type::method` to `methods[(Type, method)]`.
fn receiver_before(text: &str, pos: Position) -> Option<String> {
    let offset = position_to_char_offset(text, pos)?;
    let chars: Vec<char> = text.chars().collect();
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    // Walk back through the identifier under cursor.
    let mut i = offset.min(chars.len());
    while i > 0 && is_id(chars[i - 1]) {
        i -= 1;
    }
    // Expect `::` immediately before.
    if i < 2 || chars[i - 1] != ':' || chars[i - 2] != ':' {
        return None;
    }
    let mut j = i - 2;
    while j > 0 && is_id(chars[j - 1]) {
        j -= 1;
    }
    let recv: String = chars[j..i - 2].iter().collect();
    if recv.is_empty() {
        None
    } else {
        Some(recv)
    }
}

// ---- AST-driven receiver type inference ---------------------------------

/// Snapshot of the receiver-name → type bindings visible at a
/// particular cursor position. Built from the enclosing method's
/// `self` parameter, regular parameters, and `Declaration`
/// statements seen before the cursor.
struct LocalEnv {
    /// Type the enclosing method's `self` resolves to. `None` for
    /// free functions (which the language doesn't have today).
    self_ty: Option<String>,
    /// `name` → declared type name, lowest in the file wins
    /// (mirrors lexical scoping for shadowing).
    bindings: HashMap<String, String>,
}

/// Find the `MethodCall` AST node whose `name` token sits at
/// `pos_off` (matching `method_name`) and return its receiver
/// expression along with the local environment in scope at that
/// position. Returns `None` if the cursor isn't inside a method
/// of an extension/impl, or no matching MethodCall exists there.
fn method_call_at<'a>(
    items: &'a [new_parser::ast::Spanned<new_parser::ast::Item>],
    pos_off: usize,
    method_name: &str,
) -> Option<(LocalEnv, &'a new_parser::ast::Spanned<new_parser::ast::Expr>)> {
    use new_parser::ast::*;
    for item in items {
        let (target_ty, methods) = match &item.0 {
            Item::Extension(e) => (Some(&e.target.0), &e.methods),
            Item::Impl(i) => (Some(&i.target.0), &i.methods),
            _ => continue,
        };
        for m in methods {
            let body_sp = &m.0.body.1;
            if pos_off < body_sp.start || pos_off > body_sp.end {
                continue;
            }
            // Collect bindings visible up to pos_off.
            let mut env = LocalEnv {
                self_ty: target_ty.map(|t| t.name.0.clone()),
                bindings: HashMap::new(),
            };
            for p in &m.0.sig.params {
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
                    env.bindings.insert(pname, ty);
                }
            }
            walk_block(&m.0.body.0, &mut |sp| {
                if let Expr::Declaration { name, ty, .. } = &sp.0 {
                    if sp.1.start <= pos_off {
                        env.bindings.insert(name.0.clone(), ty.0.name.0.clone());
                    }
                }
            });

            // Find the MethodCall expression whose .name token
            // covers pos_off and whose name matches.
            let mc = find_method_call(&m.0.body.0, pos_off, method_name)?;
            if let Expr::MethodCall { receiver, .. } = &mc.0 {
                return Some((env, receiver));
            }
            return None;
        }
    }
    None
}

fn find_method_call<'a>(
    block: &'a new_parser::ast::Block,
    pos_off: usize,
    method_name: &str,
) -> Option<&'a new_parser::ast::Spanned<new_parser::ast::Expr>> {
    for stmt in &block.stmts {
        if let Some(found) = find_in_expr(stmt, pos_off, method_name) {
            return Some(found);
        }
    }
    None
}

fn find_in_expr<'a>(
    e: &'a new_parser::ast::Spanned<new_parser::ast::Expr>,
    pos_off: usize,
    method_name: &str,
) -> Option<&'a new_parser::ast::Spanned<new_parser::ast::Expr>> {
    use new_parser::ast::Expr;
    if let Expr::MethodCall { name, .. } = &e.0 {
        if name.0 == method_name
            && pos_off >= name.1.start
            && pos_off <= name.1.end
        {
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

/// Infer the bare type name of `expr`. Handles:
///   * `self` / `Self` → enclosing target.
///   * `Variable` → look up in env.
///   * `Field(recv, name)` → recv's type, then field lookup in registry.
///   * `MethodCall(recv, name, ...)` → recv's type, then method's
///     declared return type from registry.
///   * `StaticCall { ty, name }` → method's return type on `ty`.
///   * `StructLiteral { ty, .. }` → ty's name.
///   * `EnumVariant { ty, .. }` → ty's name.
///   * `Cast { ty, .. }` → ty's name.
fn infer_expr_type(
    expr: &new_parser::ast::Spanned<new_parser::ast::Expr>,
    env: &LocalEnv,
    reg: &typer::TypeRegistry,
) -> Option<String> {
    use new_parser::ast::Expr;
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
        Expr::StaticCall { ty, name, .. } => {
            method_return_type(reg, &ty.0.name.0, &name.0)
        }
        Expr::StructLiteral { ty, .. } => Some(ty.0.name.0.clone()),
        Expr::EnumVariant { ty, .. } => Some(ty.0.name.0.clone()),
        Expr::Cast { ty, .. } => Some(ty.0.name.0.clone()),
        _ => None,
    }
}

fn field_type(reg: &typer::TypeRegistry, ty_name: &str, field_name: &str) -> Option<String> {
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

fn method_return_type(
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

/// What is `Self` here? If the cursor is inside an extension or
/// impl method, returns that extension/impl's target type name.
fn enclosing_self_type(
    items: &[new_parser::ast::Spanned<new_parser::ast::Item>],
    pos_off: usize,
) -> Option<String> {
    use new_parser::ast::*;
    for item in items {
        let (target_ty, methods) = match &item.0 {
            Item::Extension(e) => (Some(&e.target.0), &e.methods),
            Item::Impl(i) => (Some(&i.target.0), &i.methods),
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

/// Walk every expression in `block` (recursing into nested
/// blocks / loops / matches / ifs) and call `f` on each
/// `Spanned<Expr>` node.
fn walk_block(
    block: &new_parser::ast::Block,
    f: &mut dyn FnMut(&new_parser::ast::Spanned<new_parser::ast::Expr>),
) {
    for stmt in &block.stmts {
        walk_expr(stmt, f);
    }
}

fn walk_expr(
    e: &new_parser::ast::Spanned<new_parser::ast::Expr>,
    f: &mut dyn FnMut(&new_parser::ast::Spanned<new_parser::ast::Expr>),
) {
    use new_parser::ast::Expr;
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

fn position_to_char_offset(text: &str, pos: Position) -> Option<usize> {
    let mut line: u32 = 0;
    let mut character: u32 = 0;
    for (i, c) in text.chars().enumerate() {
        if line == pos.line && character == pos.character {
            return Some(i);
        }
        if c == '\n' {
            line += 1;
            character = 0;
        } else {
            character += if (c as u32) > 0xFFFF { 2 } else { 1 };
        }
    }
    if line == pos.line && character == pos.character {
        Some(text.chars().count())
    } else {
        None
    }
}

fn handle_notification(connection: &Connection, state: &mut State, not: Notification) {
    match not.method.as_str() {
        "textDocument/didOpen" => {
            if let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(not.params)
            {
                state
                    .docs
                    .insert(params.text_document.uri.clone(), params.text_document.text.clone());
                publish_for(
                    connection,
                    state,
                    &params.text_document.uri,
                    &params.text_document.text,
                );
            }
        }
        "textDocument/didChange" => {
            if let Ok(mut params) =
                serde_json::from_value::<DidChangeTextDocumentParams>(not.params)
            {
                if let Some(change) = params.content_changes.pop() {
                    state
                        .docs
                        .insert(params.text_document.uri.clone(), change.text.clone());
                    publish_for(connection, state, &params.text_document.uri, &change.text);
                }
            }
        }
        "textDocument/didSave" => {
            if let Ok(params) = serde_json::from_value::<DidSaveTextDocumentParams>(not.params)
            {
                let uri = params.text_document.uri.clone();
                let text = params
                    .text
                    .clone()
                    .or_else(|| state.docs.get(&uri).cloned())
                    .unwrap_or_default();
                publish_for(connection, state, &uri, &text);
            }
        }
        "textDocument/didClose" => {
            if let Ok(params) = serde_json::from_value::<DidCloseTextDocumentParams>(not.params)
            {
                state.docs.remove(&params.text_document.uri);
                // Clear diagnostics on close so stale messages don't linger.
                send_diagnostics(connection, &params.text_document.uri, vec![]);
            }
        }
        _ => {}
    }
}

fn publish_for(connection: &Connection, state: &mut State, uri: &Url, text: &str) {
    let local_path = uri
        .to_file_path()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "<unsaved>".to_string());

    // Build the file set: the open document first, then any std
    // files we can locate. Skip the open file from the std set if
    // it lives there (so we don't double-include and confuse the
    // file-name attribution).
    // Match the CLI's gather_files ordering (std files first,
    // open file last). The typer's per-name attribution treats the
    // sequence as a flat set, so order shouldn't matter — but
    // mirroring the CLI keeps any incidental order-sensitive
    // behaviour consistent between the two surfaces.
    let mut files: Vec<(String, String)> = Vec::new();
    if let Some(std_dir) = &state.std_dir {
        match std::fs::read_dir(std_dir) {
            Ok(read) => {
                for entry in read.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|s| s.to_str()) != Some("ct") {
                        continue;
                    }
                    let same_as_open =
                        uri.to_file_path().map(|op| op == p).unwrap_or(false);
                    if same_as_open {
                        continue;
                    }
                    match std::fs::read_to_string(&p) {
                        Ok(content) => {
                            // Use `std/<name>` to match the CLI's naming
                            // convention so any file-based attribution
                            // matches between the two surfaces.
                            let name = format!(
                                "std/{}",
                                p.file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| p.display().to_string())
                            );
                            files.push((name, content));
                        }
                        Err(e) => eprintln!(
                            "cythan-lsp: failed to read {}: {}",
                            p.display(),
                            e
                        ),
                    }
                }
            }
            Err(e) => eprintln!(
                "cythan-lsp: read_dir({}) failed: {}",
                std_dir.display(),
                e
            ),
        }
    }
    // Track each file's name → absolute path so the symbol index
    // can build full file URIs in the resulting `Location`s.
    let mut name_to_path: HashMap<String, PathBuf> = HashMap::new();
    if let Some(std_dir) = &state.std_dir {
        for (name, _) in &files {
            if let Some(stripped) = name.strip_prefix("std/") {
                name_to_path.insert(name.to_string(), std_dir.join(stripped));
            }
        }
    }
    if let Some(open_path) = uri.to_file_path().ok() {
        name_to_path.insert(local_path.clone(), open_path);
    }

    files.push((local_path.clone(), text.to_string()));
    eprintln!(
        "cythan-lsp: diagnose with {} files (open = {})",
        files.len(),
        local_path
    );

    let as_refs: Vec<(&str, String)> = files
        .iter()
        .map(|(n, c)| (n.as_str(), c.clone()))
        .collect();
    let report = cythan_driver::new_pipeline::diagnose(&as_refs);

    // Refresh the symbol index + cached registry from the same
    // file set. Best-effort: parse / build failures yield empty
    // results without bringing the LSP down.
    let (idx, reg) = build_symbol_index(&files, &name_to_path);
    state.symbols = idx;
    state.registry = reg;

    // Cache the parsed AST of the open document so receiver-type
    // inference (used by goto-definition for `recv.method`) has
    // something to walk.
    if let Ok(items) = new_parser::parse(text) {
        state.asts.insert(uri.clone(), items);
    } else {
        state.asts.remove(uri);
    }

    // Filter to diagnostics that point at the open file. Other-file
    // diagnostics don't have a useful URI to attach to in this
    // single-file push model.
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for d in report.errors.iter().chain(report.warnings.iter()) {
        if let Some(lsp) = to_lsp_diagnostic(d, &local_path, text) {
            diagnostics.push(lsp);
        }
    }
    send_diagnostics(connection, uri, diagnostics);
}

/// Parse every file, build a `TypeRegistry`, and walk it to
/// produce a fresh `SymbolIndex`. Best-effort — any parse / build
/// failure yields a partial (or empty) index without bringing the
/// LSP down.
fn build_symbol_index(
    files: &[(String, String)],
    name_to_path: &HashMap<String, PathBuf>,
) -> (SymbolIndex, Option<typer::TypeRegistry>) {
    let mut idx = SymbolIndex::default();

    // Parse each file. On a parse error, skip the file (the user
    // sees the diagnostic via the diagnose call; goto-definition
    // simply doesn't index that file's symbols).
    type Parsed = (String, Vec<new_parser::ast::Spanned<new_parser::ast::Item>>);
    let mut parsed: Vec<Parsed> = Vec::new();
    for (name, src) in files {
        if let Ok(items) = new_parser::parse(src) {
            parsed.push((name.to_string(), items));
        }
    }
    let as_refs: Vec<(&str, &[_])> = parsed
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_slice()))
        .collect();

    let reg = match typer::TypeRegistry::from_files(&as_refs) {
        Ok(r) => r,
        Err(_) => return (idx, None),
    };

    let location_for = |file: &Option<String>,
                        span: &Option<new_parser::Span>|
     -> Option<Location> {
        let file_name = file.as_ref()?;
        let span = span.as_ref()?;
        let abs = name_to_path.get(file_name)?;
        let uri = Url::from_file_path(abs).ok()?;
        let src = files.iter().find(|(n, _)| n == file_name).map(|(_, s)| s)?;
        Some(Location {
            uri,
            range: byte_range_to_lsp(src, span),
        })
    };

    for (name, info) in reg.iter_types() {
        if let Some(loc) = location_for(&info.decl_file, &info.decl_span) {
            idx.types.insert(name.to_string(), loc.clone());
            // Methods attached to the type. The MethodInfo's
            // function carries a `name: Spanned<String>` whose
            // span IS the method-name token's span — perfect for
            // a goto-definition jump.
            for m in &info.methods {
                let m_name = &m.function.sig.name.0;
                let m_span = &m.function.sig.name.1;
                let m_file = reg.file_names.get(&m.file_id).cloned();
                if let Some(loc) = location_for(&m_file, &Some(m_span.clone())) {
                    idx.methods.insert((name.to_string(), m_name.to_string()), loc.clone());
                    idx.methods_by_name
                        .entry(m_name.to_string())
                        .or_default()
                        .push(loc);
                }
            }
        }
    }
    for (name, info) in reg.iter_traits() {
        if let Some(loc) = location_for(&info.decl_file, &info.decl_span) {
            idx.traits.insert(name.to_string(), loc);
        }
    }

    (idx, Some(reg))
}

fn send_diagnostics(connection: &Connection, uri: &Url, diagnostics: Vec<Diagnostic>) {
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: None,
    };
    let not = Notification {
        method: PublishDiagnostics::METHOD.to_string(),
        params: serde_json::to_value(params).unwrap_or(serde_json::Value::Null),
    };
    let _ = connection.sender.send(Message::Notification(not));
}

/// Convert one `errors::Diagnostic` to one LSP `Diagnostic`.
/// Returns `None` if the diagnostic doesn't reference `local_file`
/// (we only surface diagnostics for the open document).
fn to_lsp_diagnostic(d: &ErrDiag, local_file: &str, text: &str) -> Option<Diagnostic> {
    let primary = d
        .labels
        .iter()
        .find(|l| l.kind == LabelKind::Primary && l.span.file == local_file)
        .or_else(|| d.labels.iter().find(|l| l.span.file == local_file))?;

    let range = byte_range_to_lsp(text, &primary.span.range);

    let mut message = d.message.clone();
    if !primary.message.is_empty() {
        message.push('\n');
        message.push_str(&primary.message);
    }
    for n in &d.notes {
        message.push_str("\nnote: ");
        message.push_str(n);
    }
    for h in &d.helps {
        message.push_str("\nhelp: ");
        message.push_str(h);
    }

    let related: Vec<DiagnosticRelatedInformation> = d
        .labels
        .iter()
        .filter(|l| !std::ptr::eq(*l, primary))
        .filter_map(|l| {
            if l.span.file != local_file {
                return None;
            }
            let r = byte_range_to_lsp(text, &l.span.range);
            Some(DiagnosticRelatedInformation {
                location: Location {
                    uri: Url::from_file_path(&local_file).ok()?,
                    range: r,
                },
                message: l.message.clone(),
            })
        })
        .collect();

    Some(Diagnostic {
        range,
        severity: Some(severity_to_lsp(d.severity)),
        code: d.code.map(|c| NumberOrString::String(c.0.to_string())),
        code_description: None,
        source: Some("cythan".to_string()),
        message,
        related_information: if related.is_empty() {
            None
        } else {
            Some(related)
        },
        tags: None,
        data: None,
    })
}

fn severity_to_lsp(s: Severity) -> DiagnosticSeverity {
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Note | Severity::Help => DiagnosticSeverity::INFORMATION,
    }
}

/// Char-offset range → LSP `Range`.
///
/// The parser tracks spans as **character (Unicode scalar value)
/// offsets**, not byte offsets — chumsky's default for `&str`
/// input. A multi-byte UTF-8 char (e.g. an em-dash in a comment)
/// makes byte position `> char position` for everything after
/// it, so treating the range as bytes mis-points the diagnostic.
///
/// LSP wants line + UTF-16 code units for `character`, which we
/// compute by walking `chars()` and tracking the per-line UTF-16
/// width.
fn byte_range_to_lsp(text: &str, range: &std::ops::Range<usize>) -> Range {
    let start = char_offset_to_position(text, range.start);
    let end = char_offset_to_position(text, range.end);
    let end = if end == start {
        Position {
            line: start.line,
            character: start.character + 1,
        }
    } else {
        end
    };
    Range { start, end }
}

fn char_offset_to_position(text: &str, char_offset: usize) -> Position {
    let mut line: u32 = 0;
    let mut character: u32 = 0;
    let mut count: usize = 0;
    for c in text.chars() {
        if count >= char_offset {
            break;
        }
        if c == '\n' {
            line += 1;
            character = 0;
        } else {
            character += if (c as u32) > 0xFFFF { 2 } else { 1 };
        }
        count += 1;
    }
    Position { line, character }
}
