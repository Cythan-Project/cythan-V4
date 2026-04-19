//! Server-wide state kept between requests.
//!
//! `State` is constructed once in `main` from the
//! `initialize` params and threaded through every handler. It
//! owns the open-document buffers, the cached AST + type
//! registry, the symbol index, and the std-dir config.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_types::Url;

use crate::symbols::SymbolIndex;

pub(crate) struct State {
    /// Buffer of currently-open documents (URI → full text).
    pub(crate) docs: HashMap<Url, String>,
    /// Standard library directory; every `.ct` file inside is
    /// loaded alongside the active document so cross-file
    /// symbols resolve.
    pub(crate) std_dir: Option<PathBuf>,
    /// Last symbol index, refreshed on every `diagnose` call.
    pub(crate) symbols: SymbolIndex,
    /// Parsed AST per open document.
    pub(crate) asts: HashMap<Url, Vec<new_parser::ast::Spanned<new_parser::ast::Item>>>,
    /// Last-built type registry.
    pub(crate) registry: Option<typer::TypeRegistry>,
}

impl State {
    pub(crate) fn from_init(init: &serde_json::Value) -> Self {
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

/// Locate the Cythan stdlib relative to a workspace folder.
/// Tries (in order): `<root>/std`, `<root>/examples/new_syntax/std`,
/// then walks up parent directories looking for either layout.
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
