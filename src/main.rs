#![feature(box_syntax)]
#![feature(try_blocks)]

use std::ops::Range;

use ariadne::Report;
use mir::StdIoContext;

use crate::actions::{build_context::compile, run_context::run};

mod actions;
mod compiler;
mod errors;
mod parser;
#[cfg(test)]
mod tests;

pub type Error = Report<(String, Range<usize>)>;

const STACK_SIZE: usize = 1024 * 1024 * 1024;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|x| x.as_str()) {
        Some("run") => {
            let fname = args.get(2).expect("No file name given");
            let pg = compile(fname.to_owned()).optimize();
            println!("Compiled successfully!");
            println!("Now running...");
            run(&pg, StdIoContext);
        }
        Some("build") => {
            let fname = args.get(2).expect("No file name given");
            compile(fname.to_owned());
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
