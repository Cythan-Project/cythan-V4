use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use cythan_driver::new_pipeline::{self, Backend};

#[cfg(test)]
mod new_pipeline_tests;

#[derive(Parser)]
#[command(
    name = "cythan",
    about = "Cythan V4 compiler and runtime — new_parser → typer → hir → mir → lir → bytecode."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Standard library directory.
    #[arg(long, global = true, default_value = "examples/new_syntax/std")]
    std_dir: PathBuf,
}

#[derive(Subcommand)]
enum Command {
    /// Type-check + HIR-gen a program; report errors or a success
    /// summary. Exits non-zero on any pipeline failure.
    Check {
        /// Main source file (e.g. `examples/new_syntax/Morpion.ct`).
        file: PathBuf,
    },
    /// Compile the program and dump any combination of the
    /// intermediate representations as human-readable text.
    /// `--hir` / `--mir` / `--lir` / `--cythan` are all optional; at
    /// least one must be supplied. `--hir` is per-function and
    /// doesn't need an entry point. `--mir` / `--lir` / `--cythan`
    /// inline from an entry (`--entry-type`, `--entry-method`).
    Build {
        /// Main source file.
        file: PathBuf,
        /// Write per-function HIR dump.
        #[arg(long, value_name = "FILE")]
        hir: Option<PathBuf>,
        /// Write flat MIR (post-inlining) dump.
        #[arg(long, value_name = "FILE")]
        mir: Option<PathBuf>,
        /// Write LIR (labelled assembly, post-`opt_asm`) dump.
        #[arg(long, value_name = "FILE")]
        lir: Option<PathBuf>,
        /// Write raw Cythan bytecode as a space-separated list of
        /// decimals. The same format `cythan inspect` accepted.
        #[arg(long, value_name = "FILE")]
        cythan: Option<PathBuf>,
        /// Entry-point type name. Defaults to the file's stem.
        #[arg(long)]
        entry_type: Option<String>,
        /// Entry-point method. Defaults to `main`.
        #[arg(long, default_value = "main")]
        entry_method: String,
    },
    /// Full compile + execute. Backend chooses the runtime: the MIR
    /// interpreter (`mir`), the bytecode path via LIR (`lir`), or
    /// the Cythan VM (`cythan`). Input comes from stdin, output
    /// goes to stdout.
    Run {
        /// Main source file.
        file: PathBuf,
        /// Runtime to execute on.
        #[arg(long, default_value = "mir", value_parser = parse_backend)]
        backend: Backend,
        /// Entry-point type name. Defaults to file stem.
        #[arg(long)]
        entry_type: Option<String>,
        /// Entry-point method. Defaults to `main`.
        #[arg(long, default_value = "main")]
        entry_method: String,
        /// Memory budget in u4 cells (`mir` backend only).
        #[arg(long, default_value_t = 4096)]
        mem_cells: usize,
    },
}

fn parse_backend(s: &str) -> Result<Backend, String> {
    s.parse::<Backend>()
}

fn main() {
    let cli = Cli::parse();
    let std_dir = cli.std_dir;
    match cli.command {
        Command::Check { file } => run_check(&file, &std_dir),
        Command::Build {
            file,
            hir,
            mir,
            lir,
            cythan,
            entry_type,
            entry_method,
        } => run_build(
            &file,
            &std_dir,
            hir.as_deref(),
            mir.as_deref(),
            lir.as_deref(),
            cythan.as_deref(),
            entry_type.as_deref(),
            &entry_method,
        ),
        Command::Run {
            file,
            backend,
            entry_type,
            entry_method,
            mem_cells,
        } => run_program(&file, &std_dir, backend, entry_type.as_deref(), &entry_method, mem_cells),
    }
}

// ---- check ---------------------------------------------------------------

fn run_check(file: &Path, std_dir: &Path) {
    let files = gather_or_die(std_dir, file);
    let refs: Vec<(&str, String)> = files.iter().map(|(n, s)| (n.as_str(), s.clone())).collect();
    let report = new_pipeline::diagnose(&refs);
    let sources: std::collections::HashMap<String, String> = files
        .iter()
        .map(|(n, s)| (n.clone(), s.clone()))
        .collect();
    for diag in report.errors.iter().chain(report.warnings.iter()) {
        eprint!("{}", errors::render_diag(diag, &sources));
    }
    if report.has_errors() {
        std::process::exit(1);
    }
    eprintln!(
        "ok ({} warning{})",
        report.warnings.len(),
        if report.warnings.len() == 1 { "" } else { "s" }
    );
}

// ---- build ---------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_build(
    file: &Path,
    std_dir: &Path,
    hir_out: Option<&Path>,
    mir_out: Option<&Path>,
    lir_out: Option<&Path>,
    cythan_out: Option<&Path>,
    entry_type_override: Option<&str>,
    entry_method: &str,
) {
    if hir_out.is_none() && mir_out.is_none() && lir_out.is_none() && cythan_out.is_none() {
        die("at least one of --hir / --mir / --lir / --cythan must be supplied");
    }
    let files = gather_or_die(std_dir, file);
    let refs: Vec<(&str, String)> = files.iter().map(|(n, s)| (n.as_str(), s.clone())).collect();

    if let Some(path) = hir_out {
        let built = new_pipeline::build_hir(&refs).unwrap_or_else(|e| die(&e));
        std::fs::write(path, new_pipeline::hir_to_text(&built.hir))
            .unwrap_or_else(|e| die(&format!("write {}: {}", path.display(), e)));
        eprintln!(
            "wrote HIR for {} function(s) to {}",
            built.hir.len(),
            path.display()
        );
    }

    // If any of mir/lir/cythan is requested, we need the compiled MIR
    // with a concrete entry point.
    let needs_mir = mir_out.is_some() || lir_out.is_some() || cythan_out.is_some();
    if !needs_mir {
        return;
    }
    let entry_type = entry_type_override
        .map(String::from)
        .unwrap_or_else(|| file_stem(file));
    let entry = typer::FnSig::new(&entry_type, entry_method);
    let mir = new_pipeline::compile(&refs, &entry).unwrap_or_else(|e| die(&e));

    if let Some(path) = mir_out {
        std::fs::write(path, new_pipeline::mir_to_text(&mir))
            .unwrap_or_else(|e| die(&format!("write {}: {}", path.display(), e)));
        eprintln!(
            "wrote MIR ({} ops) for {}::{} to {}",
            mir.0.len(),
            entry_type,
            entry_method,
            path.display()
        );
    }

    // LIR + Cythan share the same lowering; do it once if either
    // output is requested.
    if lir_out.is_some() || cythan_out.is_some() {
        let lir = new_pipeline::mir_to_lir(&mir);
        if let Some(path) = lir_out {
            std::fs::write(path, new_pipeline::lir_to_text(&lir))
                .unwrap_or_else(|e| die(&format!("write {}: {}", path.display(), e)));
            eprintln!(
                "wrote LIR ({} instructions) for {}::{} to {}",
                lir.len(),
                entry_type,
                entry_method,
                path.display()
            );
        }
        if let Some(path) = cythan_out {
            let bytecode = new_pipeline::lir_to_bytecode(lir);
            std::fs::write(path, new_pipeline::bytecode_to_text(&bytecode))
                .unwrap_or_else(|e| die(&format!("write {}: {}", path.display(), e)));
            eprintln!(
                "wrote Cythan bytecode ({} words) for {}::{} to {}",
                bytecode.len(),
                entry_type,
                entry_method,
                path.display()
            );
        }
    }
}

// ---- run -----------------------------------------------------------------

fn run_program(
    file: &Path,
    std_dir: &Path,
    backend: Backend,
    entry_type_override: Option<&str>,
    entry_method: &str,
    mem_cells: usize,
) {
    let entry_type = entry_type_override
        .map(String::from)
        .unwrap_or_else(|| file_stem(file));
    let files = gather_or_die(std_dir, file);
    let refs: Vec<(&str, String)> = files.iter().map(|(n, s)| (n.as_str(), s.clone())).collect();
    let entry = typer::FnSig::new(&entry_type, entry_method);
    let mir = new_pipeline::compile(&refs, &entry).unwrap_or_else(|e| die(&e));

    eprintln!(
        "running {}::{} on backend `{}`",
        entry_type, entry_method, backend
    );
    match backend {
        Backend::Mir => {
            // MIR interpreter: wire to stdin/stdout directly.
            let mut state = mir::MemoryState::new(mem_cells, 8);
            let mut ctx = mir::StdIoContext;
            state.execute_block(&mir, &mut ctx);
            eprintln!("done ({} MIR steps)", state.instr_count);
        }
        Backend::Lir | Backend::Cythan => {
            let lir = new_pipeline::mir_to_lir(&mir);
            let bytecode = new_pipeline::lir_to_bytecode(lir);
            let (steps, _) = cythan_driver::run_context::run_bin(&bytecode, mir::StdIoContext);
            eprintln!("done ({} VM steps)", steps);
        }
    }
}

// ---- helpers -------------------------------------------------------------

fn gather_or_die(std_dir: &Path, main: &Path) -> Vec<(String, String)> {
    match new_pipeline::gather_files(std_dir, main) {
        Ok(v) => v,
        Err(msg) => die(&msg),
    }
}

fn file_stem(p: &Path) -> String {
    p.file_stem()
        .expect("source file has no stem")
        .to_string_lossy()
        .into_owned()
}

fn die(msg: &str) -> ! {
    eprintln!("{}", msg);
    std::process::exit(1)
}
