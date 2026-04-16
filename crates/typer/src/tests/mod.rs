//! Tests for the type registry.
//!
//! Organized by plan step:
//! - `layout_tests`          (Step 2.1) hand-built layouts + size formulas
//! - `struct_tests`          (Step 2.2) struct registration + offsets
//! - `enum_tests`            (Step 2.3) enum discriminant / data layout
//! - `extension_tests`       (Step 2.4) extension merging
//! - `trait_impl_tests`      (Step 2.5) trait registration + impl validation
//! - `integration_tests`     (Step 2.6) end-to-end: parse → registry

mod enum_tests;
mod extension_tests;
mod flat_sig_tests;
mod function_db_tests;
mod integration_tests;
mod layout_tests;
mod struct_tests;
mod trait_impl_tests;

use new_parser::ast::{Item, Spanned};

/// Parse a source string and return the items, panicking if parsing fails.
pub fn parse_src(src: &str) -> Vec<Spanned<Item>> {
    match new_parser::parse(src) {
        Ok(items) => items,
        Err(e) => panic!("parse failed: {:?}\nsource:\n{}", e, src),
    }
}
