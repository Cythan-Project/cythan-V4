use std::{collections::VecDeque, fmt::Debug};

use errors::{
    expected_number_as_type, invalid_type_template, Error, Span, SpannedObject, SpannedVector,
};

use super::{
    expression::TokenProcessor, token_utils::split_complex, ClosableType, Token, TokenExtracter,
    TokenParser,
};
use crate::parser::token_utils::SplitAction;

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
    pub fn apply_expected(&self, expected: &Option<Type>) -> Self {
        if let Some(e) = expected {
            if matches!(self.template, None) {
                Type {
                    span: self.span.clone(),
                    name: self.name.clone(),
                    template: e.template.clone(),
                }
            } else {
                self.clone()
            }
        } else {
            self.clone()
        }
    }
    #[allow(dead_code)]
    pub fn native(name: &str, template: Option<SpannedVector<Type>>) -> Self {
        Self::new(name, template, Span::default())
    }
    pub fn native_simple(name: &str) -> Self {
        Self::simple(name, Span::default())
    }
    pub fn simple(name: &str, span: Span) -> Self {
        Self::new(name, None, span)
    }
    pub fn as_number(&self) -> Result<u32, Error> {
        self.name
            .1
            .parse()
            .map_err(|_| expected_number_as_type(&self.name.0))
    }

    pub fn get_template(&self) -> Result<&SpannedVector<Type>, Error> {
        self.template
            .as_ref()
            .ok_or_else(|| invalid_type_template(&self.name.0, &self.name.0))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateDefinition(pub SpannedVector<String>);

impl TokenParser<TemplateDefinition> for VecDeque<Token> {
    fn parse(self, _: &Type) -> Result<TemplateDefinition, Error> {
        Ok(TemplateDefinition(SpannedVector(
            self.iter()
                .fold(None, |a: Option<Span>, b| {
                    Some(
                        a.map(|x| x.merge(b.span()))
                            .unwrap_or_else(|| b.span().clone()),
                    )
                })
                .unwrap(),
            split_complex(self, |a| {
                if matches!(a, Token::Comma(_)) {
                    SplitAction::SplitConsume
                } else {
                    SplitAction::None
                }
            })
            .into_iter()
            .map(|mut a| match a.get_token() {
                Some(Token::TypeName(_, name)) => name,
                Some(Token::Number(_, number, _)) => number.to_string(),
                _ => panic!("Expected type name in type definition"),
            }) // TODO : Add error if The vec as more than 1 element.
            .collect(),
        )))
    }
}
impl TokenParser<Vec<Type>> for VecDeque<Token> {
    fn parse(self, types: &Type) -> Result<Vec<Type>, Error> {
        split_complex(self, |a| {
            if matches!(a, Token::Comma(_)) {
                SplitAction::SplitConsume
            } else {
                SplitAction::None
            }
        })
        .into_iter()
        .map(|mut a| a.extract(types))
        .collect()
    }
}
impl TokenExtracter<Type> for VecDeque<Token> {
    fn extract(&mut self, types: &Type) -> Result<Type, Error> {
        match self.get_token() {
            Some(Token::TypeName(name_span, name)) => {
                let template = match self.get_token() {
                    Some(Token::Block(span, ClosableType::Type, inside)) => {
                        Some(SpannedVector(span, inside.parse(types)?))
                    }
                    Some(e) => {
                        self.push_front(e);
                        None
                    }
                    None => None,
                };
                if name == "Self" {
                    Ok(Type::new(
                        &types.name.1,
                        if let Some(e) = template {
                            Some(e)
                        } else {
                            types.template.clone()
                        },
                        name_span,
                    ))
                } else {
                    Ok(Type::new(&name, template, name_span))
                }
            }
            Some(Token::Number(name_span, number, _)) => {
                let template = match self.get_token() {
                    Some(Token::Block(span, ClosableType::Type, inside)) => {
                        Some(SpannedVector(span, inside.parse(types)?))
                    }
                    Some(e) => {
                        self.push_front(e);
                        None
                    }
                    None => None,
                };
                Ok(Type::new(&number.to_string(), template, name_span))
            }
            _ => {
                panic!("Expected type");
            }
        }
    }
}
