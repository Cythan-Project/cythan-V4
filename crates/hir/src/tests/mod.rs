//! HIR generator tests.

mod data_tests;
mod expr_tests;
mod control_flow_tests;
mod call_tests;
mod construction_tests;
mod mutability_tests;
mod opt_tests;
pub mod interp_tests;
mod array_tests;
mod arraylist_tests;
mod game_tests;
mod lower_tests;
mod natives_tests;
mod operator_tests;
mod trait_dispatch_tests;

use new_parser::ast;

use crate::{gen_function, HirFunction};

/// Parse source, build registry + FunctionDB, and compile the named method
/// on the named type. Panics on any error — use `try_compile` to get the
/// error back.
pub fn compile(src: &str, type_name: &str, method: &str) -> HirFunction {
    try_compile(src, type_name, method).unwrap_or_else(|e| panic!("HIR: {:?}", e))
}

pub fn try_compile(
    src: &str,
    type_name: &str,
    method: &str,
) -> Result<HirFunction, String> {
    let items = match new_parser::parse(src) {
        Ok(i) => i,
        Err(e) => return Err(format!("parse: {:?}", e)),
    };
    let _ = items as Vec<ast::Spanned<ast::Item>>;
    let items = new_parser::parse(src).unwrap();
    let reg = typer::TypeRegistry::from_items(&items).map_err(|e| format!("typer: {:?}", e))?;
    let db = typer::FunctionDB::from_registry(&reg).map_err(|e| format!("fn_db: {:?}", e))?;
    let key = typer::FnSig::new(type_name, method);
    let f = db.get(&key).ok_or_else(|| format!("function not found: {:?}", key))?;
    let typer::Fn::Simple(s) = f else {
        return Err(format!("function {:?} is not Simple", key));
    };
    gen_function(&key, s, &reg, &db).map_err(|e| format!("hir: {}", e))
}
