pub mod ast;
pub mod lexer;
pub mod parser;
pub mod token;

pub type Span = std::ops::Range<usize>;
