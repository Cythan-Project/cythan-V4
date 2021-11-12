use std::collections::VecDeque;

use super::{expression::TokenProcessor, ClosableType, Token, TokenExtracter};

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub name: String,
    pub arguments: VecDeque<Token>,
}

impl TokenExtracter<Vec<Annotation>> for VecDeque<Token> {
    fn extract(&mut self) -> Vec<Annotation> {
        let mut annotations = Vec::new();
        loop {
            match self.get_token() {
                Some(Token::At) => {
                    if let Some(Token::Literal(name) | Token::TypeName(name)) = self.get_token() {
                        if let Some(Token::Block(ClosableType::Parenthesis, inside)) =
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
                    return annotations;
                }
                None => return annotations,
            }
        }
    }
}
