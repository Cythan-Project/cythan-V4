#![feature(box_syntax)]
#![feature(try_blocks)]

use cythan::format;
use lir::CompilableInstruction;
use mir::{MirState, StdIoContext};

use crate::actions::{
    build_context::compile,
    run_context::{run, run_bin},
};

mod actions;
mod compiler;
mod parser;
#[cfg(test)]
mod tests;

const STACK_SIZE: usize = 1024 * 1024 * 1024;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|x| x.as_str()) {
        Some("run") => {
            let fname = args.get(2).expect("No file name given");
            let pg = compile(fname.to_owned());
            println!("Compiled successfully!");
            println!("Now running...");
            run(&pg, StdIoContext);
        }
        Some("exe") => {
            let fname = args.get(2).expect("No file name given");
            let pg = format::decode_bytes(&std::fs::read(fname).unwrap())
                .unwrap()
                .1;
            println!("Now running...");
            run_bin(
                &pg.into_iter().map(|x| x as usize).collect::<Vec<_>>(),
                StdIoContext,
            );
        }
        Some("build") => {
            let fname = args.get(2).expect("No file name given");
            let oname = args.get(3).expect("No file name given");
            let compiled = compile(fname.to_owned());
            if let Some(e) = args.get(4) {
                std::fs::write(
                    e,
                    compiled
                        .0
                        .iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
                .expect("Could not write file");
            }
            let mut mirstate = MirState::default();
            compiled.to_asm(&mut mirstate);
            mirstate.opt_asm();
            let k: Vec<u32> = CompilableInstruction::compile_to_binary(mirstate.instructions)
                .into_iter()
                .map(|x| x as u32)
                .collect();
            std::fs::write(
                oname,
                cythan::format::encode_to_bytes(cythan::format::HeaderData::default(), &k)
                    .expect("Could not create binary"),
            )
            .expect("Could not write file");
            println!("Compiled successfully!")
        }
        Some("test") => {
            unimplemented!()
        }
        _ => {
            println!("Invalid command expected run, test or build");
        }
    }
}

const MIR_MODE: bool = false;
