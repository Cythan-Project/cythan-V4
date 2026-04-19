//! Cursor-position utilities and identifier extraction.
//!
//! Small, self-contained helpers shared by every request
//! handler. No LSP-specific logic beyond the `Position` /
//! `Range` types; everything else is plain string walking.
//!
//! # Char vs byte offsets
//!
//! The Cythan parser stores spans as **character** offsets
//! (Unicode scalar values) — that's chumsky's default for `&str`
//! input. Everything in this module consumes / produces those
//! same char offsets. [`byte_range_to_lsp`] converts a char
//! range to an LSP `Range` by walking `chars()` and counting
//! UTF-16 code units per line (which is what clients expect by
//! default).

use lsp_types::{Position, Range};

/// What immediately precedes the identifier at the cursor.
/// Drives whether to interpret the identifier as a method
/// (`recv.foo`), an associated function (`Type::foo`), or a
/// free symbol.
pub(crate) enum IdContext {
    None,
    AfterDot,
    AfterColons,
}

/// Convert an LSP `Position` (line + UTF-16 char column) to a
/// character offset into `text`. Returns `None` if the position
/// falls outside the document.
pub(crate) fn position_to_char_offset(text: &str, pos: Position) -> Option<usize> {
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

/// Convert a **character-offset range** (as produced by the
/// Cythan parser) into an LSP `Range`. Counts UTF-16 code units
/// per line for the `character` field.
pub(crate) fn byte_range_to_lsp(text: &str, range: &std::ops::Range<usize>) -> Range {
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

/// Public alias so callers outside this module can convert a
/// char offset straight to an LSP `Position` without building
/// a one-element range first.
pub(crate) fn char_offset_to_lsp_position(text: &str, char_offset: usize) -> Position {
    char_offset_to_position(text, char_offset)
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

/// Extract the identifier the cursor is inside of (or adjacent
/// to). Returns the identifier text and a tag describing the
/// punctuation, if any, immediately before it.
pub(crate) fn identifier_at(text: &str, pos: Position) -> Option<(String, IdContext)> {
    let offset = position_to_char_offset(text, pos)?;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
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

/// Receiver identifier immediately before a `::` at `pos` —
/// the `T` in `T::method`. `None` when the receiver isn't a
/// simple identifier.
pub(crate) fn receiver_before(text: &str, pos: Position) -> Option<String> {
    let offset = position_to_char_offset(text, pos)?;
    let chars: Vec<char> = text.chars().collect();
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = offset.min(chars.len());
    while i > 0 && is_id(chars[i - 1]) {
        i -= 1;
    }
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

/// Pull the dot-separated identifier chain that ends at the `.`
/// at `dot_idx`. E.g. given `   self.r0.<dot>` returns
/// `["self", "r0"]`.
pub(crate) fn extract_dot_chain(chars: &[char], dot_idx: usize) -> Vec<String> {
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut out: Vec<String> = Vec::new();
    let mut end = dot_idx;
    loop {
        let mut start = end;
        while start > 0 && is_id(chars[start - 1]) {
            start -= 1;
        }
        if start == end {
            break;
        }
        let word: String = chars[start..end].iter().collect();
        out.push(word);
        if start == 0 || chars[start - 1] != '.' {
            break;
        }
        end = start - 1;
    }
    out.reverse();
    out
}

/// Every span where `word` appears as a standalone identifier
/// in `src`. Returns char-offset ranges suitable for
/// `byte_range_to_lsp`.
pub(crate) fn find_word_occurrences(src: &str, word: &str) -> Vec<std::ops::Range<usize>> {
    let chars: Vec<char> = src.chars().collect();
    let target: Vec<char> = word.chars().collect();
    if target.is_empty() {
        return vec![];
    }
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut i = 0;
    while i + target.len() <= chars.len() {
        if chars[i..i + target.len()] == target[..] {
            let prev_ok = i == 0 || !is_id(chars[i - 1]);
            let next_ok =
                i + target.len() == chars.len() || !is_id(chars[i + target.len()]);
            if prev_ok && next_ok {
                out.push(i..i + target.len());
            }
        }
        i += 1;
    }
    out
}
