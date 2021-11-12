use std::{collections::VecDeque, fmt::Debug};

use super::{expression::TokenProcessor, ClosableType, Token, TokenExtracter, TokenParser};
use crate::parser::token_utils::split_simple;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Type {
    pub name: String,
    pub template: Option<Vec<Type>>,
}

impl Debug for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(a) = &self.template {
            write!(
                f,
                "{}<{}>",
                self.name,
                a.iter()
                    .map(|t| format!("{:?}", t))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        } else {
            write!(f, "{}", self.name)
        }
    }
}

impl Type {
    pub fn new(name: &str, template: Option<Vec<Type>>) -> Self {
        Self {
            name: name.to_owned(),
            template,
        }
    }
    pub fn simple(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            template: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TemplateDefinition(pub Vec<String>);

impl TokenParser<TemplateDefinition> for VecDeque<Token> {
    fn parse(self) -> TemplateDefinition {
        TemplateDefinition(
            split_simple(self, &Token::Comma)
                .into_iter()
                .map(|mut a| {
                    if let Some(Token::TypeName(name)) = a.get_token() {
                        name
                    } else {
                        panic!("Expected type name in type definition")
                    }
                }) // TODO : Add error if The vec as more than 1 element.
                .collect(),
        )
    }
}
impl TokenParser<Vec<Type>> for VecDeque<Token> {
    fn parse(self) -> Vec<Type> {
        split_simple(self, &Token::Comma)
            .into_iter()
            .map(|mut a| a.extract())
            .collect()
    }
}
impl TokenExtracter<Type> for VecDeque<Token> {
    fn extract(&mut self) -> Type {
        if let Some(Token::TypeName(name)) = self.get_token() {
            let template = match self.get_token() {
                Some(Token::Block(ClosableType::Type, inside)) => Some(inside.parse()),
                Some(e) => {
                    self.push_front(e);
                    None
                }
                None => None,
            };
            Type { name, template }
        } else {
            panic!("Expected type");
        }
    }
}
