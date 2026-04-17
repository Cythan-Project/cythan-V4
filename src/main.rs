use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use cythan::format;
use lir::CompilableInstruction;
use mir::{MirState, StdIoContext};

use cythan_driver::run_context::{compute_max_bin, run, run_bin};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod new_pipeline_tests;

#[derive(Parser)]
#[command(name = "cythan", about = "Cythan V4 compiler and runtime")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Standard library directory (legacy pipeline).
    #[arg(long, global = true, default_value = "cythan/std")]
    std_dir: PathBuf,
    /// Standard library directory for the new pipeline
    /// (`new_parser` → `typer` → `hir` → `mir`).
    #[arg(long, global = true, default_value = "examples/new_syntax/std")]
    new_std_dir: PathBuf,
}

#[derive(Subcommand)]
enum Command {
    /// Compile and run a Cythan program
    Run {
        /// Source file path (e.g. std/Morpion.ct)
        file: PathBuf,
        /// Enable MIR optimization
        #[arg(short, long)]
        optimize: bool,
        /// Dump MIR before optimization
        #[arg(long, value_name = "FILE")]
        dump_mir_before: Option<PathBuf>,
        /// Dump MIR after optimization
        #[arg(long, value_name = "FILE")]
        dump_mir_after: Option<PathBuf>,
    },
    /// Compile a Cythan program to binary
    Build {
        /// Source file path (e.g. std/Morpion.ct)
        file: PathBuf,
        /// Output binary file path
        #[arg(short, long)]
        output: PathBuf,
        /// Enable MIR optimization
        #[arg(short = 'O', long)]
        optimize: bool,
        /// Dump MIR (after optimization if enabled)
        #[arg(long, value_name = "FILE")]
        dump_mir: Option<PathBuf>,
        /// Dump MIR before optimization
        #[arg(long, value_name = "FILE")]
        dump_mir_before: Option<PathBuf>,
        /// Dump MIR after optimization
        #[arg(long, value_name = "FILE")]
        dump_mir_after: Option<PathBuf>,
        /// Dump LIR (low-level IR) instructions
        #[arg(long, value_name = "FILE")]
        dump_lir: Option<PathBuf>,
        /// Dump V3 assembly text
        #[arg(long, value_name = "FILE")]
        dump_asm: Option<PathBuf>,
    },
    /// Decode and inspect a compiled binary
    Inspect {
        /// Input binary file
        input: PathBuf,
        /// Output text file
        output: PathBuf,
    },
    /// Pre-compute machine state from a binary
    Precomp {
        /// Input binary file
        input: PathBuf,
        /// Output binary file
        output: PathBuf,
    },
    /// Execute a pre-compiled binary
    Exe {
        /// Input binary file
        input: PathBuf,
    },
    /// New-pipeline toolchain (`new_parser` → `typer` → `hir` → `mir`)
    New {
        #[command(subcommand)]
        command: NewCommand,
    },
}

#[derive(Subcommand)]
enum NewCommand {
    /// Type-check + HIR-gen a program; report errors or a success summary.
    /// Exits non-zero if any stage of the new pipeline fails.
    Check {
        /// Main source file (e.g. `examples/new_syntax/Morpion.ct`).
        file: PathBuf,
    },
    /// Compile to HIR and/or MIR and write human-readable text dumps.
    /// At least one of `--hir` / `--mir` must be supplied.
    Build {
        /// Main source file.
        file: PathBuf,
        /// Write HIR text dump here (per-function; no entry needed).
        #[arg(long, value_name = "FILE")]
        hir: Option<PathBuf>,
        /// Write MIR text dump here. Requires an entry point — inlining
        /// starts there and the resulting flat `MirCodeBlock` is dumped.
        #[arg(long, value_name = "FILE")]
        mir: Option<PathBuf>,
        /// Entry-point type name (for `--mir`). Defaults to the file's
        /// stem (e.g. `Morpion.ct` → `Morpion`).
        #[arg(long)]
        entry_type: Option<String>,
        /// Entry-point method (for `--mir`). Defaults to `main`.
        #[arg(long, default_value = "main")]
        entry_method: String,
    },
    /// Full compile + MIR-interpret against stdin/stdout.
    Run {
        /// Main source file.
        file: PathBuf,
        /// Entry-point type name. Defaults to the file's stem
        /// (e.g. `Morpion.ct` → `Morpion`).
        #[arg(long)]
        entry_type: Option<String>,
        /// Entry-point method. Defaults to `main`.
        #[arg(long, default_value = "main")]
        entry_method: String,
        /// Memory budget in cells (one cell = 4 bits).
        #[arg(long, default_value_t = 4096)]
        mem_cells: usize,
    },
}

fn compile_to_mir(file: &Path, std_dir: &Path, optimize: bool) -> mir::MirCodeBlock {
    cythan_driver::build_context::compile(file, std_dir, optimize)
}

fn dump_mir(mir: &mir::MirCodeBlock, path: &PathBuf) {
    std::fs::write(
        path,
        mir.0
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("Could not write MIR file");
}

fn main() {
    let cli = Cli::parse();

    let std_dir = cli.std_dir;
    match cli.command {
        Command::Run {
            file,
            optimize,
            dump_mir_before,
            dump_mir_after,
        } => {
            let raw_mir = compile_to_mir(&file, &std_dir, false);
            if let Some(path) = &dump_mir_before {
                dump_mir(&raw_mir, path);
            }
            let mir = if optimize {
                let count = raw_mir.instr_count();
                let optimized = raw_mir.optimize_code_new();
                let ncount = optimized.instr_count();
                eprintln!(
                    "Optimized from {} to {} ({:.02}%)",
                    count,
                    ncount,
                    (count - ncount) as f64 / count as f64 * 100.
                );
                optimized
            } else {
                raw_mir
            };
            if let Some(path) = &dump_mir_after {
                dump_mir(&mir, path);
            }
            eprintln!("Compiled successfully!");
            eprintln!("Now running...");
            run(&mir, StdIoContext);
        }
        Command::Build {
            file,
            output,
            optimize,
            dump_mir: dump_mir_path,
            dump_mir_before,
            dump_mir_after,
            dump_lir,
            dump_asm,
        } => {
            let raw_mir = compile_to_mir(&file, &std_dir, false);
            if let Some(path) = &dump_mir_before {
                dump_mir(&raw_mir, path);
            }
            let compiled = if optimize {
                let count = raw_mir.instr_count();
                let optimized = raw_mir.optimize_code_new();
                let ncount = optimized.instr_count();
                eprintln!(
                    "Optimized from {} to {} ({:.02}%)",
                    count,
                    ncount,
                    (count - ncount) as f64 / count as f64 * 100.
                );
                optimized
            } else {
                raw_mir
            };
            if let Some(path) = &dump_mir_after {
                dump_mir(&compiled, path);
            }
            if let Some(path) = &dump_mir_path {
                dump_mir(&compiled, path);
            }

            let mut mirstate = MirState::default();
            compiled.to_asm(&mut mirstate);
            mirstate.opt_asm();

            if let Some(path) = &dump_lir {
                std::fs::write(
                    path,
                    mirstate
                        .instructions
                        .iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
                .expect("Could not write LIR file");
            }

            let asm_text = CompilableInstruction::compile_to_string(mirstate.instructions.clone());
            if let Some(path) = &dump_asm {
                std::fs::write(path, &asm_text).expect("Could not write ASM file");
            }

            let k: Vec<u32> = CompilableInstruction::compile_to_binary(mirstate.instructions)
                .into_iter()
                .map(|x| x as u32)
                .collect();
            std::fs::write(
                &output,
                cythan::format::encode_to_bytes(cythan::format::HeaderData::default(), &k)
                    .expect("Could not create binary"),
            )
            .expect("Could not write binary file");
            eprintln!("Compiled successfully!");
        }
        Command::Inspect { input, output } => {
            let pg = format::decode_bytes(&std::fs::read(&input).unwrap())
                .unwrap()
                .1;
            std::fs::write(
                &output,
                pg.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
            )
            .unwrap();
            eprintln!("Decoded successfully!");
        }
        Command::Precomp { input, output } => {
            let pg = format::decode_bytes(&std::fs::read(&input).unwrap())
                .unwrap()
                .1;
            eprintln!("Now running...");
            let result = compute_max_bin(&pg.into_iter().map(|x| x as usize).collect::<Vec<_>>());
            eprintln!("Advanced machine by: {} steps", result.0);
            std::fs::write(
                &output,
                cythan::format::encode_to_bytes(
                    cythan::format::HeaderData::default(),
                    &result.1.iter().map(|x| *x as _).collect::<Vec<_>>(),
                )
                .expect("Could not create binary"),
            )
            .expect("Could not write file");
        }
        Command::Exe { input } => {
            let pg = format::decode_bytes(&std::fs::read(&input).unwrap())
                .unwrap()
                .1;
            eprintln!("Now running...");
            let (k, _) = run_bin(
                &pg.into_iter().map(|x| x as usize).collect::<Vec<_>>(),
                StdIoContext,
            );
            eprintln!("Took {} steps", k);
        }
        Command::New { command } => run_new_command(command, &cli.new_std_dir),
    }
}

fn run_new_command(command: NewCommand, new_std_dir: &Path) {
    use cythan_driver::new_pipeline;

    match command {
        NewCommand::Check { file } => {
            let files = gather_or_die(new_std_dir, &file);
            let refs: Vec<(&str, String)> = files
                .iter()
                .map(|(n, s)| (n.as_str(), s.clone()))
                .collect();
            match new_pipeline::check(&refs) {
                Ok(summary) => {
                    eprintln!("{}", summary);
                }
                Err(msg) => {
                    eprintln!("{}", msg);
                    std::process::exit(1);
                }
            }
        }
        NewCommand::Build {
            file,
            hir,
            mir,
            entry_type,
            entry_method,
        } => {
            if hir.is_none() && mir.is_none() {
                die("at least one of --hir / --mir must be supplied");
            }
            let files = gather_or_die(new_std_dir, &file);
            let refs: Vec<(&str, String)> = files
                .iter()
                .map(|(n, s)| (n.as_str(), s.clone()))
                .collect();

            if let Some(hir_path) = &hir {
                match new_pipeline::build_hir(&refs) {
                    Ok(built) => {
                        let text = new_pipeline::hir_to_text(&built.hir);
                        std::fs::write(hir_path, text).unwrap_or_else(|e| {
                            die(&format!("write {}: {}", hir_path.display(), e))
                        });
                        eprintln!(
                            "wrote HIR for {} function(s) to {}",
                            built.hir.len(),
                            hir_path.display()
                        );
                    }
                    Err(msg) => die(&msg),
                }
            }

            if let Some(mir_path) = &mir {
                let entry_type = entry_type.unwrap_or_else(|| {
                    file.file_stem()
                        .expect("main file has no stem")
                        .to_string_lossy()
                        .into_owned()
                });
                let entry = typer::FnSig::new(&entry_type, &entry_method);
                match new_pipeline::compile(&refs, &entry) {
                    Ok(mir_block) => {
                        let text = new_pipeline::mir_to_text(&mir_block);
                        std::fs::write(mir_path, text).unwrap_or_else(|e| {
                            die(&format!("write {}: {}", mir_path.display(), e))
                        });
                        eprintln!(
                            "wrote MIR ({} ops) for {}::{} to {}",
                            mir_block.0.len(),
                            entry_type,
                            entry_method,
                            mir_path.display()
                        );
                    }
                    Err(msg) => die(&msg),
                }
            }
        }
        NewCommand::Run {
            file,
            entry_type,
            entry_method,
            mem_cells,
        } => {
            let entry_type = entry_type.unwrap_or_else(|| {
                file.file_stem()
                    .expect("main file has no stem")
                    .to_string_lossy()
                    .into_owned()
            });
            let files = gather_or_die(new_std_dir, &file);
            let refs: Vec<(&str, String)> = files
                .iter()
                .map(|(n, s)| (n.as_str(), s.clone()))
                .collect();
            let entry = typer::FnSig::new(&entry_type, &entry_method);
            let mir = match new_pipeline::compile(&refs, &entry) {
                Ok(mir) => mir,
                Err(msg) => die(&msg),
            };
            eprintln!(
                "running {}::{} ({} cells)",
                entry_type, entry_method, mem_cells
            );
            let mut state = mir::MemoryState::new(mem_cells, 8);
            let mut ctx = mir::StdIoContext;
            state.execute_block(&mir, &mut ctx);
            eprintln!("done ({} MIR steps)", state.instr_count);
        }
    }
}

fn gather_or_die(std_dir: &Path, main: &Path) -> Vec<(String, String)> {
    match cythan_driver::new_pipeline::gather_files(std_dir, main) {
        Ok(v) => v,
        Err(msg) => die(&msg),
    }
}

fn die(msg: &str) -> ! {
    eprintln!("{}", msg);
    std::process::exit(1)
}
