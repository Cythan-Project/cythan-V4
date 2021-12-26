use std::collections::VecDeque;

use errors::{invalid_token_after, Error, Span, SpannedObject, SpannedVector};

use crate::parser::{
    token_utils::{split_complex, take_until, SplitAction},
    ClosableType, Keyword, Token, TokenExtracter,
};

pub trait TokenProcessor {
    fn get_token(&mut self) -> Option<Token>;
    fn length(&self) -> usize;
}

impl TokenProcessor for VecDeque<Token> {
    fn get_token(&mut self) -> Option<Token> {
        while let Some(e) = self.pop_front() {
            if matches!(e, Token::Comment(_, _)) {
                continue;
            }
            return Some(e);
        }
        None
    }

    fn length(&self) -> usize {
        self.iter()
            .filter(|x| !matches!(x, &Token::Comment(_, _)))
            .count()
    }
}

use super::{ty::Type, NumberType, TokenParser};

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    New {
        span: Span,
        class: Type,
        fields: SpannedVector<(String, Expr)>,
    },
    If {
        span: Span,
        condition: Box<Expr>,
        then: CodeBlock,
        or_else: Option<CodeBlock>,
    },
    Cast {
        span: Span,
        source: Box<Expr>,
        target: Type,
    },
    Number(Span, i32, NumberType),
    Variable(Span, String),
    Type(Span, Type),
    ArrayDefinition(Span, SpannedVector<Expr>),
    Field {
        span: Span,
        source: Box<Expr>,
        name: String,
    },
    Method {
        span: Span,
        source: Box<Expr>,
        name: SpannedObject<String>,
        arguments: SpannedVector<Expr>,
        template: Option<SpannedVector<Type>>,
    },
    NamedResource {
        span: Span,
        vtype: Type,
        name: SpannedObject<String>,
    },
    Assignement {
        span: Span,
        target: Box<Expr>,
        to: Box<Expr>,
    },
    Loop(Span, CodeBlock),
    Break(Span),
    Continue(Span),
    Block(Span, CodeBlock),
    Return(Span, Option<Box<Expr>>),
    BooleanExpression(Span, Box<Expr>, BooleanOperator, Box<Expr>),
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum BooleanOperator {
    And,
    Or,
}

fn chain_expression(tokens: &mut VecDeque<Token>, exp: Expr, types: &Type) -> Result<Expr, Error> {
    let k = match tokens.get_token() {
        None | Some(Token::SemiColon(_)) => return Ok(exp),
        Some(Token::Equals(span)) => Expr::Assignement {
            span,
            target: Box::new(exp),
            to: Box::new(tokens.drain(0..).collect::<VecDeque<_>>().parse(types)?),
        },
        Some(Token::Keyword(span, Keyword::As)) => {
            let t = tokens.extract(&types)?;
            Expr::Cast {
                span,
                source: Box::new(exp),
                target: t,
            }
        }
        Some(Token::BooleanOperator(span, e)) => Expr::BooleanExpression(
            span,
            box exp,
            e,
            Box::new(tokens.drain(0..).collect::<VecDeque<_>>().parse(types)?),
        ),
        Some(Token::Dot(_)) => {
            if let Some(Token::Literal(name_span, name)) = tokens.get_token() {
                match tokens.get_token() {
                    Some(Token::Block(arguments_span, ClosableType::Parenthesis, arguments)) => {
                        Expr::Method {
                            name: SpannedObject(name_span, name),
                            span: exp.span().merge(&arguments_span),
                            source: box exp,
                            arguments: SpannedVector(
                                arguments_span,
                                split_complex(arguments, |a| {
                                    if matches!(a, Token::Comma(_)) {
                                        SplitAction::SplitConsume
                                    } else {
                                        SplitAction::None
                                    }
                                })
                                .into_iter()
                                .map(|a| a.parse(types))
                                .collect::<Result<_, _>>()?,
                            ),
                            template: None,
                        }
                    }
                    Some(Token::Block(template_span, ClosableType::Type, template)) => {
                        if let Some(Token::Block(
                            arguments_span,
                            ClosableType::Parenthesis,
                            arguments,
                        )) = tokens.get_token()
                        {
                            Expr::Method {
                                span: exp.span().merge(&arguments_span),
                                source: box exp,
                                name: SpannedObject(name_span, name),
                                arguments: SpannedVector(
                                    arguments_span,
                                    split_complex(arguments, |a| {
                                        if matches!(a, Token::Comma(_)) {
                                            SplitAction::SplitConsume
                                        } else {
                                            SplitAction::None
                                        }
                                    })
                                    .into_iter()
                                    .map(|a| a.parse(types))
                                    .collect::<Result<_, _>>()?,
                                ),
                                template: Some(SpannedVector(
                                    template_span,
                                    template.parse(types)?,
                                )),
                            }
                        } else {
                            panic!("Expected brackets after method call")
                        }
                    }
                    e => {
                        if let Some(e) = e {
                            tokens.push_front(e);
                        }
                        Expr::Field {
                            source: Box::new(exp),
                            name,
                            span: name_span,
                        }
                    }
                }
            } else {
                panic!("Expected literal after dot")
            }
        }
        Some(e) => {
            return Err(invalid_token_after(
                e.span(),
                exp.span(),
                "expression",
                &e.name(),
                &[";", "||", "&&", "=", ".", "as"],
                true,
            ));
        }
    };
    chain_expression(tokens, k, types)
}

fn parse_if(
    tokens: &mut VecDeque<Token>,
    if_token_span: Span,
    types: &Type,
) -> Result<Expr, Error> {
    let k = box take_until(tokens, |e| {
        matches!(e, Token::Block(_, ClosableType::Brace, _))
    })
    .parse(types)?;
    let if_b = match tokens.get_token() {
        Some(Token::Block(span, ClosableType::Brace, e)) => SpannedVector(span, e.parse(types)?),
        Some(e) => {
            return Err(invalid_token_after(
                e.span(),
                e.span(),
                "if",
                &e.name(),
                &["Literal", "TypeName", "Number"],
                false,
            ));
        }
        None => {
            return Err(invalid_token_after(
                &if_token_span,
                &if_token_span,
                "if",
                "",
                &["Literal", "TypeName", "Number"],
                false,
            ));
        }
    };
    let else_b = if matches!(tokens.front(), Some(Token::Keyword(_, Keyword::Else))) {
        tokens.remove(0);
        match tokens.get_token() {
            Some(Token::Block(span, ClosableType::Brace, e)) => {
                Some(SpannedVector(span, e.parse(types)?))
            }
            Some(Token::Keyword(span, Keyword::If)) => {
                let ifb = parse_if(tokens, span, types)?;
                Some(SpannedVector(ifb.span().clone(), vec![ifb]))
            }
            _ => panic!("Expected brackets after else"),
        }
    } else {
        None
    };
    Ok(Expr::If {
        condition: k,
        span: if_token_span.merge(else_b.as_ref().map(|e| &e.0).unwrap_or_else(|| &if_b.0)),
        then: if_b,
        or_else: else_b,
    })
}

impl TokenParser<Expr> for VecDeque<Token> {
    fn parse(mut self, types: &Type) -> Result<Expr, Error> {
        let tk = if let Some(e) = self.get_token() {
            e
        } else {
            panic!("Expected expression")
        };
        let j = match tk {
            Token::Comma(_) => panic!("Unexpected comma"),
            Token::At(_) => panic!("Unexpected Annotation"),
            Token::Dot(_) => panic!("Unexpected comma"),
            Token::DoubleDot(_) => panic!("Unexpected comma"),
            Token::SemiColon(_) => panic!("Unexpected comma"),
            Token::BooleanOperator(_, _) => panic!("Unexpected boolean operator"),
            Token::Equals(_) => {
                println!("{:?}", self);
                panic!("Unexpected equals")
            }
            Token::Literal(span, a) => Expr::Variable(span, a),
            Token::Keyword(span, a) => match a {
                Keyword::Return => {
                    let out: Expr = self.parse(types)?;
                    return Ok(Expr::Return(span.merge(out.span()), Some(box out)));
                }
                Keyword::If => parse_if(&mut self, span, types)?,
                Keyword::Else => panic!("Unexpected else"),
                Keyword::Class => panic!("Unexpected class"),
                Keyword::Extends => panic!("Unexpected extends"),
                Keyword::As => panic!("Unexpected as"),
                Keyword::Loop => {
                    let cb = if let Some(Token::Block(span, ClosableType::Brace, e)) =
                        self.get_token()
                    {
                        SpannedVector(span, e.parse(types)?)
                    } else {
                        panic!("Expected brackets after loop")
                    };
                    Expr::Loop(span.merge(&cb.0), cb)
                }
                Keyword::Continue => Expr::Continue(span),
                Keyword::Break => Expr::Break(span),
                Keyword::While => panic!("Not yet implemented"),
                Keyword::For => panic!("Not yet implemented"),
                Keyword::In => panic!("Unexpected in"),
            },
            Token::Number(span, a, t) => Expr::Number(span, a, t),
            Token::TypeName(span, a) => {
                let template: Option<SpannedVector<Type>> = match self.get_token() {
                    Some(Token::Block(tspan, ClosableType::Type, e)) => {
                        Some(SpannedVector(tspan, e.parse(types)?))
                    }
                    Some(e) => {
                        self.push_front(e);
                        None
                    }
                    None => None,
                };
                let (a, template) = if a == "Self" {
                    if template.is_none() {
                        (types.name.1.clone(), types.template.clone())
                    } else {
                        (types.name.1.clone(), template)
                    }
                } else {
                    (a, template)
                };
                match self.get_token() {
                    Some(Token::Block(span_block, ClosableType::Brace, inside)) => Expr::New {
                        span: span.merge(&span_block),
                        class: Type::new(&a, template, span),
                        fields: SpannedVector(
                            span_block,
                            split_complex(inside, |a| {
                                if matches!(a, Token::Comma(_)) {
                                    SplitAction::SplitConsume
                                } else {
                                    SplitAction::None
                                }
                            })
                            .into_iter()
                            .map(|mut a| {
                                let name = if let Some(Token::Literal(_, name)) = a.get_token() {
                                    name
                                } else {
                                    panic!("Expected argument name");
                                };
                                if !matches!(a.get_token(), Some(Token::Equals(_))) {
                                    panic!("Expected equals");
                                };
                                let value = a.parse(types)?;
                                Ok((name, value))
                            })
                            .collect::<Result<_, _>>()?,
                        ),
                    },
                    Some(Token::Literal(lspan, literal)) => Expr::NamedResource {
                        span: span.merge(&lspan),
                        vtype: Type::new(&a, template, span),
                        name: SpannedObject(lspan, literal),
                    },
                    Some(e) => {
                        self.push_front(e);
                        let ty = Type::new(&a, template, span);
                        Expr::Type(ty.span.clone(), ty)
                    }
                    None => {
                        let ty = Type::new(&a, template, span);
                        Expr::Type(ty.span.clone(), ty)
                    }
                }
            }
            Token::Block(span, ClosableType::Brace, b) => {
                Expr::Block(span.clone(), SpannedVector(span, b.parse(types)?))
            }
            Token::Block(_, ClosableType::Parenthesis, b) => b.parse(types)?,
            Token::Block(_, ClosableType::Type, _) => panic!("Unexpected template"),
            Token::Char(span, a) => Expr::Number(
                span,
                a.chars().fold(0, |acc, c| acc * 255 + c as u8 as i32),
                NumberType::Byte,
            ),
            Token::Comment(_, _) => return self.parse(types),
            Token::String(a, b) => Expr::ArrayDefinition(
                a.clone(),
                SpannedVector(
                    a.clone(),
                    b.chars()
                        .enumerate()
                        .map(|(i, x)| {
                            Expr::Number(
                                Span {
                                    file: a.file.clone(),
                                    start: a.start + i,
                                    end: a.start + i + 1,
                                },
                                x as i32,
                                NumberType::Byte,
                            )
                        })
                        .collect(),
                ),
            ),
            Token::Block(b, ClosableType::Bracket, inside) => Expr::ArrayDefinition(
                b.clone(),
                SpannedVector(
                    b,
                    split_complex(inside, |a| {
                        if matches!(a, Token::Comma(_)) {
                            SplitAction::SplitConsume
                        } else {
                            SplitAction::None
                        }
                    })
                    .into_iter()
                    .map(|x| x.parse(types))
                    .collect::<Result<_, _>>()?,
                ),
            ),
        };
        chain_expression(&mut self, j, types)
    }
}

pub type CodeBlock = SpannedVector<Expr>;

impl TokenParser<Vec<Expr>> for VecDeque<Token> {
    fn parse(self, types: &Type) -> Result<Vec<Expr>, Error> {
        split_complex(self, |a| {
            if matches!(a, Token::SemiColon(_)) {
                SplitAction::SplitConsume
            } else {
                SplitAction::None
            }
        })
        .into_iter()
        .filter(|a| a.length() != 0)
        .map(|a| a.parse(types))
        .collect::<Result<_, _>>()
    }
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Expr::New { span, .. }
            | Expr::If { span, .. }
            | Expr::Cast { span, .. }
            | Expr::Number(span, ..)
            | Expr::Variable(span, _)
            | Expr::Type(span, _)
            | Expr::Field { span, .. }
            | Expr::Method { span, .. }
            | Expr::NamedResource { span, .. }
            | Expr::Assignement { span, .. }
            | Expr::Loop(span, _)
            | Expr::Break(span)
            | Expr::Continue(span)
            | Expr::Block(span, _)
            | Expr::Return(span, _)
            | Expr::ArrayDefinition(span, _)
            | Expr::BooleanExpression(span, _, _, _) => span,
        }
    }
}
