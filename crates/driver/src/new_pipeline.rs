//! End-to-end compile + run harness for the new pipeline
//! (`new_parser` → `typer` → `hir` → `mir`).
//!
//! Two audiences:
//!   1. Automated interaction tests — `compile_and_run(...)` takes
//!      sources + scripted input, returns captured output.
//!   2. The CLI toolchain (`cythan new check | build | run`) — uses
//!      `check(...)`, `build_hir(...)`, and `compile(...)` to stage
//!      the pipeline and inspect intermediate products.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hir::{
    gen_function_with_natives, hir_to_mir, inline_program_full, text_dump, BuiltinNatives,
    HirFunction,
};
use mir::{MemoryState, MirCodeBlock};

use crate::test_context::TestContext;

/// Parse every source file and build the typer registry + function
/// DB. Stops before HIR generation — this is what `check` relies on
/// for "does it compile?" without paying the HIR/inline cost.
pub fn check_registry(
    files: &[(&str, String)],
) -> Result<(typer::TypeRegistry, typer::FunctionDB), String> {
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
        .map_err(|errs| format_typer_errors(&errs))?;
    let db = typer::FunctionDB::from_registry(&reg)
        .map_err(|errs| format_typer_errors(&errs))?;
    Ok((reg, db))
}

/// Build every `Simple` function's HIR. Templated functions stay in
/// the DB for inlining later. Returns the HIR map plus the registry
/// so downstream stages (inliner, HIR text dump) can use both.
pub fn build_hir(
    files: &[(&str, String)],
) -> Result<BuiltHir, String> {
    let (reg, db) = check_registry(files)?;
    let natives = BuiltinNatives::new();
    let mut hir_fns: HashMap<typer::FnSig, HirFunction> = HashMap::new();
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            let hir = gen_function_with_natives(k, s, &reg, &db, Some(&natives))
                .map_err(|e| format!("hir {}::{}: {}", k.type_name, k.method_name, e))?;
            hir_fns.insert(k.clone(), hir);
        }
    }
    Ok(BuiltHir { reg, db, hir: hir_fns })
}

/// All artefacts from the HIR-build stage. Keeping them together
/// makes the `check` → `build-hir` → `run` progression explicit.
pub struct BuiltHir {
    pub reg: typer::TypeRegistry,
    pub db: typer::FunctionDB,
    pub hir: HashMap<typer::FnSig, HirFunction>,
}

/// `check` toolchain entry point: run the new pipeline as far as HIR
/// gen and report the first error, or an `Ok` summary with counts.
pub fn check(files: &[(&str, String)]) -> Result<CheckSummary, String> {
    let built = build_hir(files)?;
    Ok(CheckSummary {
        types: built.reg.type_infos.len(),
        traits: built.reg.trait_infos.len(),
        functions: built.db.functions.len(),
        simple_fns: built.hir.len(),
    })
}

/// High-level report for a successful `check` — useful for CLI output.
#[derive(Debug, Clone)]
pub struct CheckSummary {
    pub types: usize,
    pub traits: usize,
    pub functions: usize,
    pub simple_fns: usize,
}

impl std::fmt::Display for CheckSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ok: {} type{}, {} trait{}, {} function{} ({} simple)",
            self.types,
            plural(self.types),
            self.traits,
            plural(self.traits),
            self.functions,
            plural(self.functions),
            self.simple_fns,
        )
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Render the HIR produced by `build_hir` as text.
pub fn hir_to_text(hir: &HashMap<typer::FnSig, HirFunction>) -> String {
    text_dump::dump_program(hir)
}

/// Render a compiled, inlined MIR block as text. Uses `mir::Mir`'s
/// existing `Display` impl — one op per line, nested blocks indented.
pub fn mir_to_text(block: &MirCodeBlock) -> String {
    block
        .0
        .iter()
        .map(|op| op.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compile a program through the new pipeline, producing the fully
/// inlined MIR block ready to execute.
pub fn compile(
    files: &[(&str, String)],
    entry: &typer::FnSig,
) -> Result<MirCodeBlock, String> {
    let built = build_hir(files)?;
    let inlined = inline_program_full(&built.hir, entry, Some(&built.reg), Some(&built.db))
        .map_err(|e| format!("inline: {}", e))?;
    hir_to_mir(&inlined.body).map_err(|e| format!("mir: {}", e))
}

/// Default MIR-step ceiling for test harnesses. Every interaction
/// test runs with this cap so a runaway loop in the compiled program
/// fails the test promptly instead of hanging the suite.
pub const DEFAULT_STEP_LIMIT: usize = 5_000_000;

/// Run a compiled MIR block against a scripted input stream, returning
/// everything the program printed. `mem_cells` is the memory budget in
/// cells (4-bit slots). Uses [`DEFAULT_STEP_LIMIT`] as the MIR-step
/// ceiling — call [`run_mir_with_input_limited`] to pick another.
pub fn run_mir_with_input(
    mir: &MirCodeBlock,
    input: &str,
    mem_cells: usize,
) -> CapturedRun {
    run_mir_with_input_limited(mir, input, mem_cells, DEFAULT_STEP_LIMIT)
}

/// Run a compiled MIR block with an explicit step limit. `0` disables
/// the limit entirely — reserve that for interactive execution, never
/// for tests.
pub fn run_mir_with_input_limited(
    mir: &MirCodeBlock,
    input: &str,
    mem_cells: usize,
    step_limit: usize,
) -> CapturedRun {
    let mut ctx = TestContext::new(input);
    let mut state = MemoryState::new_with_limit(mem_cells, 8, step_limit);
    state.execute_block(mir, &mut ctx);
    CapturedRun {
        output: ctx.print,
        remaining_input: ctx.inputs.iter().map(|b| *b as char).collect(),
        instr_count: state.instr_count,
        aborted_by_limit: state.aborted_by_limit,
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
/// `aborted_by_limit` is `true` when execution hit the step ceiling
/// (usually = infinite loop).
#[derive(Debug, Clone)]
pub struct CapturedRun {
    pub output: String,
    pub remaining_input: String,
    pub instr_count: usize,
    pub aborted_by_limit: bool,
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
        ("std/DynArray.ct", read("std/DynArray.ct")),
    ]
}

/// Convenience: gather the standard file-set for the CLI toolchain.
/// Loads every `.ct` file directly under `std_dir`, then appends the
/// user's main file. File names are kept relative so module paths
/// derive correctly (`std/Foo.ct` → `std::Foo`).
pub fn gather_files(
    std_dir: &Path,
    main_file: &Path,
) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    if std_dir.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(std_dir)
            .map_err(|e| format!("read {}: {}", std_dir.display(), e))?
            .filter_map(|r| r.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "ct"))
            .collect();
        entries.sort();
        for path in entries {
            let name = format!(
                "std/{}",
                path.file_name().unwrap().to_string_lossy()
            );
            let src = std::fs::read_to_string(&path)
                .map_err(|e| format!("read {}: {}", path.display(), e))?
                .replace('\r', "");
            out.push((name, src));
        }
    }
    let main_name = main_file
        .file_name()
        .ok_or_else(|| format!("main path has no file name: {}", main_file.display()))?
        .to_string_lossy()
        .into_owned();
    let main_src = std::fs::read_to_string(main_file)
        .map_err(|e| format!("read {}: {}", main_file.display(), e))?
        .replace('\r', "");
    out.push((main_name, main_src));
    Ok(out)
}

fn format_typer_errors(errs: &[typer::TyperError]) -> String {
    let mut s = String::new();
    for (i, e) in errs.iter().enumerate() {
        if i > 0 {
            s.push('\n');
        }
        match &e.span {
            Some(sp) => s.push_str(&format!("error at {}..{}: {}", sp.start, sp.end, e.message)),
            None => s.push_str(&format!("error: {}", e.message)),
        }
    }
    s
}
