use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use cythan::format;
use lir::CompilableInstruction;
use mir::{MirState, StdIoContext};

use cythan_driver::run_context::{compute_max_bin, run, run_bin};

#[cfg(test)]
mod tests;

#[derive(Parser)]
#[command(name = "cythan", about = "Cythan V4 compiler and runtime")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Standard library directory (default: std/)
    #[arg(long, global = true, default_value = "std")]
    std_dir: PathBuf,
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
    }
}
