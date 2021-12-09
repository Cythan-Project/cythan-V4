#![feature(box_syntax)]
#![feature(try_blocks)]

use std::ops::Range;

use ariadne::Report;

use crate::actions::{
    build_context::compile,
    run_context::{run, StdIoContext},
};

mod actions;
mod compiler;
mod errors;
mod mir_utils;
mod parser;
#[cfg(test)]
mod tests;

pub type Error = Report<(String, Range<usize>)>;

const STACK_SIZE: usize = 1024 * 1024 * 1024;

fn main() {
    let pg = compile("Counter".to_owned())/* .optimize() */;
    println!("Now running...");
    run(&pg, StdIoContext);
}

const MIR_MODE: bool = true;
