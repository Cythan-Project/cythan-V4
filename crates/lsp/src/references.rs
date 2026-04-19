//! `textDocument/references` — token-based cross-file search.
//!
//! Approximate by design: we match the identifier under the
//! cursor against every `.ct` file the server knows about
//! (std dir + currently-open buffers). Works well enough for
//! the editor-standard "Find All References" without needing
//! full resolution of shadowed locals.

use std::path::PathBuf;

use lsp_types::{Location, Position, Url};

use crate::state::State;
use crate::text::{byte_range_to_lsp, find_word_occurrences, identifier_at};

pub(crate) fn find_references(
    state: &State,
    uri: &Url,
    pos: Position,
    _include_declaration: bool,
) -> Vec<Location> {
    let Some(text) = state.docs.get(uri) else {
        return vec![];
    };
    let Some((word, _)) = identifier_at(text, pos) else {
        return vec![];
    };
    if word.is_empty() {
        return vec![];
    }

    let mut files: Vec<(PathBuf, String)> = Vec::new();
    if let Some(std_dir) = &state.std_dir {
        if let Ok(rd) = std::fs::read_dir(std_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("ct") {
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        files.push((p, s));
                    }
                }
            }
        }
    }
    for (open_uri, doc_text) in &state.docs {
        if let Ok(p) = open_uri.to_file_path() {
            if !files.iter().any(|(q, _)| q == &p) {
                files.push((p, doc_text.clone()));
            }
        }
    }

    let mut out: Vec<Location> = Vec::new();
    for (path, src) in &files {
        let Ok(uri) = Url::from_file_path(path) else {
            continue;
        };
        for occ in find_word_occurrences(src, &word) {
            out.push(Location {
                uri: uri.clone(),
                range: byte_range_to_lsp(src, &occ),
            });
        }
    }
    out
}
