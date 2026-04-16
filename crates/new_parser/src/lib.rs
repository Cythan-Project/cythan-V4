pub mod ast;
pub mod lexer;
pub mod parser;
pub mod token;

#[cfg(test)]
mod tests;

pub type Span = std::ops::Range<usize>;

use chumsky::prelude::*;
use chumsky::Stream;

pub use ast::Item;

#[derive(Debug)]
pub enum ParseError {
    Lex(Vec<Simple<char>>),
    Parse(Vec<Simple<token::Token>>),
}

pub fn parse(src: &str) -> Result<Vec<ast::Spanned<Item>>, ParseError> {
    let tokens = lexer::lexer()
        .parse(src)
        .map_err(ParseError::Lex)?;
    let len = src.chars().count();
    let stream = Stream::from_iter(len..len + 1, tokens.into_iter());
    parser::program_parser()
        .parse(stream)
        .map_err(ParseError::Parse)
}
