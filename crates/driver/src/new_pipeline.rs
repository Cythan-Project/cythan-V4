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
use lir::CompilableInstruction;
use mir::{MemoryState, MirCodeBlock, MirState};

use crate::run_context::run_bin_with_limit;
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

/// Rich diagnostic form of `check`. Runs every pass it can and
/// collects structured `Diagnostic`s — errors AND warnings — instead
/// of bailing on the first string. Designed for the CLI and the
/// future LSP: both consume `Diagnostic` directly.
///
/// Returns `errors` and `warnings` separately so tools can surface
/// them with different severity handling.
pub fn diagnose(files: &[(&str, String)]) -> DiagnosticReport {
    let mut errors: Vec<errors::Diagnostic> = Vec::new();
    let mut warnings: Vec<errors::Diagnostic> = Vec::new();

    // Pass 1: parse every file. Parse errors today only carry a
    // terse message (no span), so promote them to a generic
    // diagnostic keyed to the file.
    let mut parsed: Vec<(String, Vec<new_parser::ast::Spanned<new_parser::ast::Item>>)> = Vec::new();
    for (name, src) in files {
        match new_parser::parse(src) {
            Ok(items) => parsed.push((name.to_string(), items)),
            Err(e) => {
                let diag = errors::Diagnostic::error(format!("parse error: {:?}", e))
                    .with_primary(errors::FileSpan::new(*name, 0..0), "");
                errors.push(diag);
            }
        }
    }
    if !errors.is_empty() {
        return DiagnosticReport { errors, warnings };
    }

    let as_refs: Vec<(&str, &[_])> = parsed
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_slice()))
        .collect();

    // Pass 2: typer. Figure out the file to attribute errors to by
    // the error's span offset if possible; otherwise fall back to the
    // first registered file.
    let reg = match typer::TypeRegistry::from_files(&as_refs) {
        Ok(r) => r,
        Err(errs) => {
            for e in errs {
                errors.push(e.into_diagnostic(default_file(files)));
            }
            return DiagnosticReport { errors, warnings };
        }
    };

    // Pass 3: function DB (flat sigs).
    let db = match typer::FunctionDB::from_registry(&reg) {
        Ok(db) => db,
        Err(errs) => {
            for e in errs {
                errors.push(e.into_diagnostic(default_file(files)));
            }
            return DiagnosticReport { errors, warnings };
        }
    };

    // Pass 4: HIR gen for every Simple function. Collect per-function
    // warnings. Collect errors per-function too so a single bad
    // function doesn't mask issues elsewhere.
    let natives = BuiltinNatives::new();
    for (k, f) in &db.functions {
        if let typer::Fn::Simple(s) = f {
            match gen_function_with_natives(k, s, &reg, &db, Some(&natives)) {
                Ok(hir) => {
                    warnings.extend(hir.warnings);
                }
                Err(e) => {
                    errors.push(e.into_diagnostic(default_file(files)));
                }
            }
        }
    }

    DiagnosticReport { errors, warnings }
}

fn default_file<'a>(files: &'a [(&'a str, String)]) -> &'a str {
    files.first().map(|(n, _)| *n).unwrap_or("<no-file>")
}

/// Bundle of `Diagnostic`s produced by `diagnose`. `errors` empty +
/// `warnings` empty means the program is clean.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticReport {
    pub errors: Vec<errors::Diagnostic>,
    pub warnings: Vec<errors::Diagnostic>,
}

impl DiagnosticReport {
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.warnings.is_empty()
    }
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// `check` toolchain entry point: run the new pipeline as far as HIR
/// gen and report the first error, or an `Ok` summary with counts.
/// Accumulates non-fatal warnings (unused variables, etc.) into the
/// summary so callers can surface them alongside the "ok" line.
pub fn check(files: &[(&str, String)]) -> Result<CheckSummary, String> {
    let built = build_hir(files)?;
    let mut warnings: Vec<errors::Diagnostic> = Vec::new();
    for hir in built.hir.values() {
        warnings.extend(hir.warnings.iter().cloned());
    }
    Ok(CheckSummary {
        types: built.reg.type_infos.len(),
        traits: built.reg.trait_infos.len(),
        functions: built.db.functions.len(),
        simple_fns: built.hir.len(),
        warnings,
    })
}

/// High-level report for a successful `check` — useful for CLI output.
#[derive(Debug, Clone)]
pub struct CheckSummary {
    pub types: usize,
    pub traits: usize,
    pub functions: usize,
    pub simple_fns: usize,
    /// Non-fatal diagnostics raised during HIR gen (unused variables,
    /// dead code, etc.). `is_empty` on a clean check.
    pub warnings: Vec<errors::Diagnostic>,
}

impl std::fmt::Display for CheckSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let warnings = if self.warnings.is_empty() {
            String::new()
        } else {
            format!(", {} warning{}", self.warnings.len(), plural(self.warnings.len()))
        };
        write!(
            f,
            "ok: {} type{}, {} trait{}, {} function{} ({} simple){}",
            self.types,
            plural(self.types),
            self.traits,
            plural(self.traits),
            self.functions,
            plural(self.functions),
            self.simple_fns,
            warnings,
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

/// Lower a MIR block to the LIR (flat `Vec<CompilableInstruction>`).
/// Runs the LIR optimizer (`opt_asm`) afterwards so callers see the
/// same form the bytecode compiler uses.
pub fn mir_to_lir(block: &MirCodeBlock) -> Vec<CompilableInstruction> {
    let mut state = MirState::default();
    block.to_asm(&mut state);
    state.opt_asm();
    state.instructions
}

/// Render LIR as text — one instruction per line, using
/// `CompilableInstruction`'s own `Display`.
pub fn lir_to_text(lir: &[CompilableInstruction]) -> String {
    lir.iter()
        .map(|ins| ins.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compile LIR to raw Cythan bytecode (`Vec<usize>`).
pub fn lir_to_bytecode(lir: Vec<CompilableInstruction>) -> Vec<usize> {
    CompilableInstruction::compile_to_binary(lir)
}

/// Render raw Cythan bytecode as a single whitespace-separated list
/// of decimal numbers — matches the `inspect` text format so users
/// can diff dumps from different compiler runs.
pub fn bytecode_to_text(bytecode: &[usize]) -> String {
    bytecode
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

// ---- Backend selection for `run` ---------------------------------------

/// Which runtime a `run` invocation targets.
///
/// * `Mir` — `mir::MemoryState` interpreter. Fastest, no lowering to
///   LIR/bytecode, easiest to step in a debugger.
/// * `Lir` — lowers through LIR and bytecode, then interprets the
///   bytecode on `cythan::InterruptedCythan`. Exercises the LIR
///   optimizer (`opt_asm`). Semantics-identical to `Cythan` — kept
///   as a distinct name so users can express "via the LIR path".
/// * `Cythan` — same as `Lir` today. Retained because the bytecode /
///   machine step-count is the metric most useful for perf checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Mir,
    Lir,
    Cythan,
}

impl std::str::FromStr for Backend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "mir" => Ok(Self::Mir),
            "lir" => Ok(Self::Lir),
            "cythan" | "vm" => Ok(Self::Cythan),
            other => Err(format!(
                "unknown backend `{}` (expected `mir`, `lir`, or `cythan`)",
                other
            )),
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Mir => "mir",
            Self::Lir => "lir",
            Self::Cythan => "cythan",
        })
    }
}

/// Run a compiled MIR block on the selected backend, capturing IO
/// the same way `run_mir_with_input` does. Returns a `CapturedRun`
/// whose `instr_count` is the backend's native unit (MIR ops for
/// `Mir`, VM steps for `Lir`/`Cythan`).
pub fn run_with_backend(
    block: &MirCodeBlock,
    backend: Backend,
    input: &str,
    mem_cells: usize,
    step_limit: usize,
) -> CapturedRun {
    match backend {
        Backend::Mir => run_mir_with_input_limited(block, input, mem_cells, step_limit),
        Backend::Lir | Backend::Cythan => run_bytecode_with_input(block, input, step_limit),
    }
}

fn run_bytecode_with_input(
    block: &MirCodeBlock,
    input: &str,
    step_limit: usize,
) -> CapturedRun {
    let lir = mir_to_lir(block);
    let bytecode = lir_to_bytecode(lir);
    let ctx = TestContext::new(input);
    let limit = if step_limit == 0 { 0 } else { step_limit };
    // `run_bin_with_limit` already panics on overshoot — catch it
    // with `catch_unwind` so we can turn the panic into an
    // `aborted_by_limit` signal instead of blowing up the caller.
    // `TestContext` is `Send` (inputs/prints are `String` / `VecDeque<u8>`),
    // but the closures in `run_bin_with_limit` aren't `UnwindSafe`, so we
    // mark the whole block as assert_unwind_safe.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_bin_with_limit(&bytecode, ctx, limit)
    }));
    match result {
        Ok((steps, ctx_mutex)) => {
            let ctx = ctx_mutex.lock().unwrap();
            CapturedRun {
                output: ctx.print.clone(),
                remaining_input: ctx.inputs.iter().map(|b| *b as char).collect(),
                instr_count: steps,
                aborted_by_limit: false,
            }
        }
        Err(panic_payload) => {
            // Only swallow the "step limit" panic — propagate anything else.
            let is_limit = panic_payload
                .downcast_ref::<String>()
                .map(|s| s.contains("step limit"))
                .unwrap_or_else(|| {
                    panic_payload
                        .downcast_ref::<&'static str>()
                        .map(|s| s.contains("step limit"))
                        .unwrap_or(false)
                });
            if is_limit {
                CapturedRun {
                    output: String::new(),
                    remaining_input: String::new(),
                    instr_count: step_limit,
                    aborted_by_limit: true,
                }
            } else {
                std::panic::resume_unwind(panic_payload);
            }
        }
    }
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
