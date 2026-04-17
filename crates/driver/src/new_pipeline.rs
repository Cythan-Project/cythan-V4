//! End-to-end compile + run harness for the new pipeline
//! (`new_parser` → `typer` → `hir` → `mir`). Built for automated
//! interaction tests: caller supplies the source files and canned
//! keyboard input, harness returns the captured output.
//!
//! The pipeline itself is driven by the hir crate; this module just
//! stitches the passes together behind one ergonomic entry point.

use std::collections::HashMap;

use hir::{gen_function_with_natives, hir_to_mir, inline_program_full, BuiltinNatives};
use mir::{MemoryState, MirCodeBlock};

use crate::test_context::TestContext;

/// Compile a program through the new pipeline, producing the fully
/// inlined MIR block ready to execute.
pub fn compile(
    files: &[(&str, String)],
    entry: &typer::FnSig,
) -> Result<MirCodeBlock, String> {
    let parsed: Vec<(String, Vec<new_parser::ast::Spanned<new_parser::ast::Item>>)> = files
        .iter()
        .map(|(name, src)| {
            let items = new_parser::parse(src)
                .map_err(|e| format!("parse `{}`: {:?}", name, e))?;
            Ok::<_, String>((name.to_string(), items))
        })
        .collect::<Result<_, _>>()?;

    let as_refs: Vec<(&str, &[_])> = parsed
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_slice()))
        .collect();
    let reg = typer::TypeRegistry::from_files(&as_refs)
        .map_err(|errs| format!("typer: {:?}", errs))?;
    let db = typer::FunctionDB::from_registry(&reg)
        .map_err(|errs| format!("fn_db: {:?}", errs))?;

    let natives = BuiltinNatives::new();
    let mut hir_fns = HashMap::new();
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            let hir = gen_function_with_natives(k, s, &reg, &db, Some(&natives))
                .map_err(|e| format!("hir {}::{}: {}", k.type_name, k.method_name, e))?;
            hir_fns.insert(k.clone(), hir);
        }
    }

    let inlined = inline_program_full(&hir_fns, entry, Some(&reg), Some(&db))
        .map_err(|e| format!("inline: {}", e))?;
    hir_to_mir(&inlined.body).map_err(|e| format!("mir: {}", e))
}

/// Run a compiled MIR block against a scripted input stream, returning
/// everything the program printed. `mem_cells` is the memory budget in
/// cells (4-bit slots).
pub fn run_mir_with_input(
    mir: &MirCodeBlock,
    input: &str,
    mem_cells: usize,
) -> CapturedRun {
    let mut ctx = TestContext::new(input);
    let mut state = MemoryState::new(mem_cells, 8);
    state.execute_block(mir, &mut ctx);
    CapturedRun {
        output: ctx.print,
        remaining_input: ctx.inputs.iter().map(|b| *b as char).collect(),
        instr_count: state.instr_count,
    }
}

/// Combined compile + run. The common case for interaction tests.
pub fn compile_and_run(
    files: &[(&str, String)],
    entry: &typer::FnSig,
    input: &str,
    mem_cells: usize,
) -> Result<CapturedRun, String> {
    let mir = compile(files, entry)?;
    Ok(run_mir_with_input(&mir, input, mem_cells))
}

/// Outcome of a scripted run: what the program printed, anything that
/// remained in the input queue (useful to confirm the program stopped
/// when we expected), and how many MIR instructions it consumed.
#[derive(Debug, Clone)]
pub struct CapturedRun {
    pub output: String,
    pub remaining_input: String,
    pub instr_count: usize,
}

/// Build the conventional stdlib file-set for new-pipeline tests.
/// Caller passes the path prefix to resolve `std/*.ct` against — this
/// keeps the harness crate-root agnostic.
pub fn load_std(base: &std::path::Path) -> Vec<(&'static str, String)> {
    let read = |rel: &str| {
        std::fs::read_to_string(base.join(rel))
            .unwrap_or_else(|e| panic!("read {}: {}", base.join(rel).display(), e))
            .replace('\r', "")
    };
    vec![
        ("std/System.ct", read("std/System.ct")),
        ("std/Ops.ct", read("std/Ops.ct")),
        ("std/Bool.ct", read("std/Bool.ct")),
        ("std/U4.ct", read("std/U4.ct")),
        ("std/U8.ct", read("std/U8.ct")),
        ("std/Array.ct", read("std/Array.ct")),
    ]
}
