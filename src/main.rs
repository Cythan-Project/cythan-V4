#![feature(box_syntax)]
#![feature(try_blocks)]

use mir::StdIoContext;

use crate::actions::{build_context::compile, run_context::run};

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
        Some("build") => {
            let fname = args.get(2).expect("No file name given");
            let compiled = compile(fname.to_owned());
            if let Some(e) = args.get(3) {
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

const MIR_MODE: bool = true;
