use std::collections::VecDeque;

use errors::Error;

use crate::parser::ty::Type;

use super::{expression::TokenProcessor, ClosableType, Token, TokenExtracter};

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub name: String,
    pub arguments: VecDeque<Token>,
}

impl TokenExtracter<Vec<Annotation>> for VecDeque<Token> {
    fn extract(&mut self, types: &Type) -> Result<Vec<Annotation>, Error> {
        let mut annotations = Vec::new();
        loop {
            match self.get_token() {
                Some(Token::At(_)) => {
                    if let Some(Token::Literal(_, name) | Token::TypeName(_, name)) =
                        self.get_token()
                    {
                        if let Some(Token::Block(_, ClosableType::Parenthesis, inside)) =
                            self.get_token()
                        {
                            annotations.push(Annotation {
                                name,
                                arguments: inside,
                            });
                        } else {
                            annotations.push(Annotation {
                                name,
                                arguments: VecDeque::new(),
                            });
                        }
                    } else {
                        panic!("Expected annotation name");
                    }
                }
                Some(e) => {
                    self.push_front(e);
                    return Ok(annotations);
                }
                None => return Ok(annotations),
            }
        }
    }
}
