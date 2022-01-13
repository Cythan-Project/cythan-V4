#![feature(format_args_capture)]

mod instruction;
mod label;
mod number;
mod optimizer;
mod template;
mod value;
mod var;

pub use instruction::*;
pub use label::*;
pub use number::*;
pub use value::*;
pub use var::*;
