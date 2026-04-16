//! Tests for the new parser.
//!
//! Layout:
//! - `fixtures.rs`   — hand-built AST values for small snippets, plus helpers.
//! - `lexer_tests.rs` — unit tests for the lexer.
//! - `parser_tests.rs` — unit tests covering each parser production against
//!                        fixture ASTs (the ground truth is written by hand,
//!                        then the parser is expected to reproduce it).
//! - `integration_tests.rs` — end-to-end tests that parse every file in
//!                             `examples/new_syntax/` and assert structural
//!                             properties of the resulting AST.

pub mod fixtures;
mod integration_tests;
mod lexer_tests;
mod parser_tests;
