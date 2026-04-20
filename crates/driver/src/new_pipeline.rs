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

use hir::natives::NativeProvider;
use hir::{
    compute_exit_domains, elide_redundant_mut, elide_redundant_mut_with_stats, elide_unused_args,
    elide_unused_args_with_stats, eliminate_dead_writes, gen_function_with_natives, hir_to_mir,
    inline_program_full, merge_adjacent_matches, optimize_block,
    specialize_monomorph_with_summaries, specialize_to_fixpoint, text_dump,
    unroll_loops_with_stats, BuiltinNatives, HirFunction, SlotId, DEFAULT_UNROLL_FACTOR,
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

    // Pass 1: parse every file. Parse errors carry a proper
    // span and found-token from chumsky; render them as a
    // readable message anchored at the right location.
    let mut parsed: Vec<(String, Vec<new_parser::ast::Spanned<new_parser::ast::Item>>)> = Vec::new();
    for (name, src) in files {
        match new_parser::parse(src) {
            Ok(items) => parsed.push((name.to_string(), items)),
            Err(e) => {
                for d in render_parse_error(*name, src, &e) {
                    errors.push(d);
                }
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

/// Render a parser failure as one `Diagnostic` per Simple error.
/// Extracts the chumsky span and found-token so the diagnostic
/// anchors at the exact location of the mistake and the message
/// reads like `expected one of {…}, found ‘loop’` instead of the
/// raw `Parse([Simple { … }])` debug dump.
fn render_parse_error(
    file: &str,
    src: &str,
    err: &new_parser::ParseError,
) -> Vec<errors::Diagnostic> {
    use new_parser::ParseError;
    let mut out = Vec::new();
    match err {
        ParseError::Lex(errs) => {
            for e in errs {
                let span = e.span();
                let found = e
                    .found()
                    .map(|c| format!("`{}`", c))
                    .unwrap_or_else(|| "end of input".to_string());
                let mut msg = format!("unexpected {}", found);
                let expected: Vec<String> = e
                    .expected()
                    .flatten()
                    .map(|c| format!("`{}`", c))
                    .collect();
                if !expected.is_empty() {
                    msg.push_str(&format!(", expected {}", expected.join(" or ")));
                }
                let span = clamp_span(&span, src);
                out.push(
                    errors::Diagnostic::error(format!("parse error: {}", msg))
                        .with_primary(errors::FileSpan::new(file, span), ""),
                );
            }
        }
        ParseError::Parse(errs) => {
            for e in errs {
                let span = e.span();
                let found = e
                    .found()
                    .map(|t| format!("`{}`", t))
                    .unwrap_or_else(|| "end of input".to_string());
                let mut msg = format!("unexpected {}", found);
                let expected: Vec<String> = e
                    .expected()
                    .flatten()
                    .map(|t| format!("`{}`", t))
                    .collect();
                if !expected.is_empty() {
                    msg.push_str(&format!(", expected {}", expected.join(" or ")));
                }
                let span = clamp_span(&span, src);
                out.push(
                    errors::Diagnostic::error(format!("parse error: {}", msg))
                        .with_primary(errors::FileSpan::new(file, span), ""),
                );
            }
        }
    }
    if out.is_empty() {
        out.push(
            errors::Diagnostic::error("parse error")
                .with_primary(errors::FileSpan::new(file, 0..0), ""),
        );
    }
    out
}

fn clamp_span(span: &std::ops::Range<usize>, src: &str) -> std::ops::Range<usize> {
    let total = src.chars().count();
    let start = span.start.min(total);
    let end = span.end.min(total).max(start);
    start..end
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

/// Lower a MIR block to LIR *without* running the LIR optimizer.
/// Useful when you want to show pre-opt counts for pipeline stats;
/// production paths should use `mir_to_lir`.
pub fn mir_to_lir_raw(block: &MirCodeBlock) -> Vec<CompilableInstruction> {
    let mut state = MirState::default();
    block.to_asm(&mut state);
    state.instructions
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

// ---- pipeline-stage stats -------------------------------------------------

/// Per-stage instruction / word counts collected during `compile`.
/// Rendered by `Display` as a Rust-style multi-line summary the
/// `build` and `run` CLI commands print before the artefact reports.
#[derive(Debug, Clone, Default)]
pub struct PipelineStats {
    /// Number of HIR functions the generator produced.
    pub hir_functions: usize,
    /// Specialized variants minted by the pre-inline specialization
    /// monomorphizer. `functions + specialized_variants` is the
    /// total bodies the inliner saw.
    pub specialized_variants: usize,
    /// Sum of HIR ops across every generated function, pre-inline
    /// and pre-specialization.
    pub hir_ops_pre_inline: usize,
    /// Sum of HIR ops across every function *after* the pre-inline
    /// specialization monomorphizer (grows with new variants).
    pub hir_ops_post_spec_mono: usize,
    /// Number of param slots whose `mut` flag was flipped to
    /// non-mut by the mutability-elision pass
    /// (`crates/hir/src/mut_elide.rs`).
    pub mut_elide_params_flipped: u32,
    /// Number of input cells dropped by the unused-arg elision
    /// pass (`crates/hir/src/arg_elide.rs`). Aggregated across
    /// every function trimmed over all fixpoint rounds.
    pub arg_elide_dropped_cells: u32,
    /// Number of distinct functions that lost at least one param
    /// to elision (counted per fixpoint round — a function that
    /// shrinks in two rounds counts twice).
    pub arg_elide_functions_trimmed: u32,
    /// HIR ops in the flattened program after inlining + monomorph.
    pub hir_ops_post_inline: usize,
    /// Number of innermost `Loop` ops whose body got duplicated
    /// by the unroll pass (`crates/hir/src/unroll.rs`).
    pub loops_unrolled: u32,
    /// HIR ops added by unrolling (sum of `body_ops * (factor-1)`
    /// across every unrolled loop).
    pub unroll_ops_added: u32,
    /// HIR ops after the flow-sensitive specialization cleanup
    /// pass. `<=` `hir_ops_post_inline`.
    pub hir_ops_post_specialize: usize,
    /// MIR ops after `hir_to_mir` (no dedicated MIR opt yet).
    pub mir_ops: usize,
    /// LIR instructions straight out of `to_asm`, before `opt_asm`.
    pub lir_instructions_pre_opt: usize,
    /// LIR instructions after `opt_asm`.
    pub lir_instructions_post_opt: usize,
    /// Bytecode word count. `None` when the caller didn't compile
    /// all the way down (e.g. `check` stops at HIR gen).
    pub bytecode_words: Option<usize>,
}

impl std::fmt::Display for PipelineStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "HIR: {} fn — {} ops  →  spec-mono: +{} variant{}, {} ops  →  mut-elide: −{} mut  →  arg-elide: −{} cell{} ({} fn)  →  inlined: {} ops  →  unroll: {} loops (+{} ops)  →  cleanup: {} ops",
            self.hir_functions,
            self.hir_ops_pre_inline,
            self.specialized_variants,
            if self.specialized_variants == 1 { "" } else { "s" },
            self.hir_ops_post_spec_mono,
            self.mut_elide_params_flipped,
            self.arg_elide_dropped_cells,
            if self.arg_elide_dropped_cells == 1 { "" } else { "s" },
            self.arg_elide_functions_trimmed,
            self.hir_ops_post_inline,
            self.loops_unrolled,
            self.unroll_ops_added,
            self.hir_ops_post_specialize,
        )?;
        writeln!(f, "MIR: {} ops", self.mir_ops)?;
        let saved = self
            .lir_instructions_pre_opt
            .saturating_sub(self.lir_instructions_post_opt);
        writeln!(
            f,
            "LIR: {} → {} instructions  (opt_asm removed {})",
            self.lir_instructions_pre_opt, self.lir_instructions_post_opt, saved,
        )?;
        if let Some(words) = self.bytecode_words {
            writeln!(f, "Cythan bytecode: {} words", words)?;
        }
        Ok(())
    }
}

/// Compile-with-stats variant. Same as `compile` but also returns
/// counts from each pipeline stage (pre/post inline, MIR, pre/post
/// LIR opt). The CLI surfaces these so developers can eyeball how
/// much the LIR peephole buys them and how many HIR ops a program
/// actually expands to after inlining.
pub fn compile_with_stats(
    files: &[(&str, String)],
    entry: &typer::FnSig,
) -> Result<(MirCodeBlock, PipelineStats), String> {
    let built = build_hir(files)?;
    let hir_functions = built.hir.len();
    let hir_ops_pre_inline: usize = built
        .hir
        .values()
        .map(|f| hir::count_ops(&f.body))
        .sum();
    let BuiltHir { reg, db, hir } = built;

    // Per-function exit-Domain summaries feed both the variant
    // folder (below) and the speculative unroller, so calls
    // propagate guaranteed Domains instead of forgetting mutated
    // slots. Computed once on the initial HIR — the resulting
    // summaries are sound upper bounds for all variants that
    // spec-mono may later mint from this base.
    let natives_for_summary = BuiltinNatives::new();
    let summaries = compute_exit_domains(&hir, |sig| {
        is_target_known_non_mutating(sig, &natives_for_summary, &db)
    });

    let spec = specialize_monomorph_with_summaries(hir, entry, Some(&summaries));
    let specialized_variants = spec.specialized_count;
    let hir_ops_post_spec_mono: usize = spec
        .functions
        .values()
        .map(|f| hir::count_ops(&f.body))
        .sum();

    // Downgrade `mut` params that aren't effectively mutated
    // anywhere (see `crates/hir/src/mut_elide.rs`). Run before
    // arg-elide so newly-immutable-and-untouched params get
    // dropped in the same round.
    let natives = BuiltinNatives::new();
    let (demut, mut_elide_stats) =
        elide_redundant_mut_with_stats(spec.functions, |sig| {
            is_target_known_non_mutating(sig, &natives, &db)
        });
    // Drop unused function arguments across the program. Runs
    // after spec-monomorph so per-variant usage info (post
    // domain-propagation) drives elision. See
    // `crates/hir/src/arg_elide.rs`.
    let (trimmed, elide_stats) = elide_unused_args_with_stats(demut, entry);

    let inlined = inline_program_full(&trimmed, entry, Some(&reg), Some(&db))
        .map_err(|e| format!("inline: {}", e))?;
    let hir_ops_post_inline = hir::count_ops(&inlined.body);

    // Unroll innermost loops before the cleanup passes so that
    // domain-based specialization + constant folding + LVA all
    // get to see + fold across the duplicated bodies (see
    // `crates/hir/src/unroll.rs`). Post-inline body has no
    // `Call` ops left, so no exit summaries are threaded.
    let (unrolled_body, unroll_stats) =
        unroll_loops_with_stats(inlined.body.clone(), DEFAULT_UNROLL_FACTOR, None);
    let specialized_body = specialize_to_fixpoint(unrolled_body);
    let merged_body = merge_adjacent_matches(specialized_body);
    // Re-specialize: merging exposes single-value arms whose
    // bodies were previously locked behind nested matches, and
    // the specializer can now fold dead arms + constant-prop
    // through them.
    let respecialized_body = specialize_to_fixpoint(merged_body);
    let cleaned_body = optimize_block(respecialized_body);
    let live_at_exit = output_slots_of(&inlined.sig);
    let lva_body = eliminate_dead_writes(cleaned_body, &live_at_exit);
    let final_body = optimize_block(lva_body);
    let hir_ops_post_specialize = hir::count_ops(&final_body);

    let mir = hir_to_mir(&final_body).map_err(|e| format!("mir: {}", e))?;
    let mir_ops = mir.instr_count();

    let lir_pre = mir_to_lir_raw(&mir);
    let lir_instructions_pre_opt = lir_pre.len();
    let lir = mir_to_lir(&mir);
    let lir_instructions_post_opt = lir.len();

    let stats = PipelineStats {
        hir_functions,
        specialized_variants,
        hir_ops_pre_inline,
        hir_ops_post_spec_mono,
        mut_elide_params_flipped: mut_elide_stats.params_flipped,
        arg_elide_dropped_cells: elide_stats.dropped_cells,
        arg_elide_functions_trimmed: elide_stats.functions_trimmed,
        hir_ops_post_inline,
        loops_unrolled: unroll_stats.loops_unrolled,
        unroll_ops_added: unroll_stats.ops_added,
        hir_ops_post_specialize,
        mir_ops,
        lir_instructions_pre_opt,
        lir_instructions_post_opt,
        bytecode_words: None,
    };
    Ok((mir, stats))
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
                output: ctx.as_str().into_owned(),
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
    // Pre-inline specialization: emits named variants per
    // (base_sig, arg_domains). The fns map grows; the inliner
    // picks the tighter body at each call site.
    let BuiltHir { reg, db, hir } = built;
    // Precompute per-function exit-domain summaries so the
    // variant folder in `spec_monomorph` can propagate callee
    // mutations into caller ctx at `Call` ops instead of
    // forgetting them.
    let natives_for_summary = BuiltinNatives::new();
    let summaries = compute_exit_domains(&hir, |sig| {
        is_target_known_non_mutating(sig, &natives_for_summary, &db)
    });
    let spec = specialize_monomorph_with_summaries(hir, entry, Some(&summaries));
    // Downgrade `mut` params that aren't effectively mutated
    // anywhere in the program (see `crates/hir/src/mut_elide.rs`).
    // Run before arg-elide so newly-immutable-and-untouched
    // params get picked up there.
    let natives = BuiltinNatives::new();
    let demut = elide_redundant_mut(spec.functions, |sig| {
        is_target_known_non_mutating(sig, &natives, &db)
    });
    // Drop unused function arguments across the program (see
    // `crates/hir/src/arg_elide.rs`). Runs post-spec so
    // domain-propagation-driven constant folding has already had
    // its chance to reveal newly-unused params.
    let trimmed = elide_unused_args(demut, entry);
    let inlined = inline_program_full(&trimmed, entry, Some(&reg), Some(&db))
        .map_err(|e| format!("inline: {}", e))?;
    // Post-inline sweep. Note: after inline the program is one
    // flat body with no Call ops, so exit summaries aren't
    // useful here — pass `None`.
    let (unrolled, _) =
        unroll_loops_with_stats(inlined.body, DEFAULT_UNROLL_FACTOR, None);
    let specialized = specialize_to_fixpoint(unrolled);
    let merged = merge_adjacent_matches(specialized);
    let respecialized = specialize_to_fixpoint(merged);
    let cleaned = optimize_block(respecialized);
    let live_at_exit = output_slots_of(&inlined.sig);
    let lva_cleaned = eliminate_dead_writes(cleaned, &live_at_exit);
    let final_block = optimize_block(lva_cleaned);
    hir_to_mir(&final_block).map_err(|e| format!("mir: {}", e))
}

/// Compile a program via the soir (Sea-of-Nodes) backend.
///
/// Reuses the existing HIR pipeline up through inlining (so all
/// `Call` ops are already resolved), then translates the flat
/// post-inline function to soir and schedules it to MIR. The
/// intermediate soir rewrites (M5-M7) live inside the soir crate
/// and fire transparently between translation and scheduling.
///
/// Goal parity: a program compiled via soir must produce
/// semantically equivalent MIR to the classical `compile` path.
/// Bytecode-level equality isn't required — the schedulers lay
/// ops out differently — but every interaction test should yield
/// the same output + IO behaviour.
pub fn compile_via_soir(
    files: &[(&str, String)],
    entry: &typer::FnSig,
) -> Result<MirCodeBlock, String> {
    // Steps 1-4 match the classical pipeline verbatim: HIR gen,
    // spec-mono, mut-elide, arg-elide, inline_program_full.
    let built = build_hir(files)?;
    let BuiltHir { reg, db, hir } = built;
    let natives_for_summary = BuiltinNatives::new();
    let summaries = compute_exit_domains(&hir, |sig| {
        is_target_known_non_mutating(sig, &natives_for_summary, &db)
    });
    let spec = specialize_monomorph_with_summaries(hir, entry, Some(&summaries));
    let natives = BuiltinNatives::new();
    let demut = elide_redundant_mut(spec.functions, |sig| {
        is_target_known_non_mutating(sig, &natives, &db)
    });
    let trimmed = elide_unused_args(demut, entry);
    let inlined = inline_program_full(&trimmed, entry, Some(&reg), Some(&db))
        .map_err(|e| format!("inline: {}", e))?;

    // Steps 5+ swapped for the soir backend.
    let graph = soir::translate_function(&inlined);
    // M5-M7 rewrites will slot in here once landed — no-ops for M4.
    let mir = soir::schedule(&graph);
    Ok(mir)
}

/// Output slots of a function's flat signature: the cells reserved
/// for the `_ret` param, which the caller (or the top-level runner)
/// reads after the function returns.
/// Predicate for `mut_elide`: does this call target clearly not
/// mutate any caller slot? True for
///   * known natives (they only touch VM registers), and
///   * any function in the `FunctionDB` (Simple or Templated)
///     declared with zero `mut` params — the inliner's back-copy
///     loop is gated on the callee's `SlotInfo.mutable`, so such
///     a callee can't feed mutation back to the caller regardless
///     of its body.
///
/// Returning `false` is always safe — it just keeps the caller's
/// matching arg cell marked as mutated.
fn is_target_known_non_mutating(
    sig: &typer::FnSig,
    natives: &BuiltinNatives,
    db: &typer::FunctionDB,
) -> bool {
    if natives.has_method(&sig.type_name, &sig.method_name) {
        return true;
    }
    let params = match db.get(sig) {
        Some(typer::Fn::Simple(s)) => &s.body.sig.params,
        Some(typer::Fn::Templated(t)) => &t.body.sig.params,
        None => return false,
    };
    params.iter().all(|p| !p.mutable)
}

fn output_slots_of(sig: &typer::FlatSig) -> std::collections::HashSet<SlotId> {
    let mut out = std::collections::HashSet::new();
    for i in 0..sig.output_count {
        out.insert(SlotId(sig.input_count + i));
    }
    out
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
        output: ctx.as_str().into_owned(),
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
