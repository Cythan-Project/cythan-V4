use std::{collections::VecDeque, fmt::Debug, ops::Range};

use ariadne::Report;

use super::{
    expression::{SpannedObject, SpannedVector, TokenProcessor},
    token_utils::split_complex,
    ClosableType, Token, TokenExtracter, TokenParser,
};
use crate::{errors::Span, parser::token_utils::SplitAction};

#[derive(Clone, Eq)]
pub struct Type {
    pub span: Span,
    pub name: SpannedObject<String>,
    pub template: Option<SpannedVector<Type>>,
}

impl PartialOrd for Type {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.name.partial_cmp(&other.name) {
            Some(core::cmp::Ordering::Equal) => {}
            ord => return ord,
        }
        self.template.partial_cmp(&other.template)
    }
}

impl PartialEq for Type {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.template == other.template
    }
}

impl Debug for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(a) = &self.template {
            write!(
                f,
                "{}<{}>",
                self.name.1,
                a.1.iter()
                    .map(|t| format!("{:?}", t))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        } else {
            write!(f, "{}", self.name.1)
        }
    }
}

impl Type {
    pub fn new(name: &str, template: Option<SpannedVector<Type>>, name_span: Span) -> Self {
        Self {
            name: SpannedObject(name_span.clone(), name.to_owned()),
            span: if let Some(template) = &template {
                name_span.merge(&template.0)
            } else {
                name_span
            },
            template,
        }
    }
    pub fn simple(name: &str, span: Span) -> Self {
        Self {
            name: SpannedObject(span.clone(), name.to_owned()),
            template: None,
            span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TemplateDefinition(pub Vec<String>);

impl TokenParser<TemplateDefinition> for VecDeque<Token> {
    fn parse(self) -> Result<TemplateDefinition, Report<(String, Range<usize>)>> {
        Ok(TemplateDefinition(
            split_complex(self, |a| {
                if matches!(a, Token::Comma(_)) {
                    SplitAction::SplitConsume
                } else {
                    SplitAction::None
                }
            })
            .into_iter()
            .map(|mut a| {
                if let Some(Token::TypeName(_, name)) = a.get_token() {
                    name
                } else {
                    panic!("Expected type name in type definition")
                }
            }) // TODO : Add error if The vec as more than 1 element.
            .collect(),
        ))
    }
}
impl TokenParser<Vec<Type>> for VecDeque<Token> {
    fn parse(self) -> Result<Vec<Type>, Report<(String, Range<usize>)>> {
        split_complex(self, |a| {
            if matches!(a, Token::Comma(_)) {
                SplitAction::SplitConsume
            } else {
                SplitAction::None
            }
        })
        .into_iter()
        .map(|mut a| a.extract())
        .collect()
    }
}
impl TokenExtracter<Type> for VecDeque<Token> {
    fn extract(&mut self) -> Result<Type, Report<(String, Range<usize>)>> {
        if let Some(Token::TypeName(name_span, name)) = self.get_token() {
            let template = match self.get_token() {
                Some(Token::Block(span, ClosableType::Type, inside)) => {
                    Some(SpannedVector(span, inside.parse()?))
                }
                Some(e) => {
                    self.push_front(e);
                    None
                }
                None => None,
            };
            Ok(Type::new(&name, template, name_span))
        } else {
            panic!("Expected type");
        }
    }
}
