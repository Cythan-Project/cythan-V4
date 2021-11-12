use std::{collections::VecDeque, ops::Range};

use ariadne::{Color, ColorGenerator, Fmt, Label, Report, ReportKind};

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
        return None;
    }

    fn length(&self) -> usize {
        self.iter()
            .filter(|x| !matches!(x, &Token::Comment(_, _)))
            .count()
    }
}

use super::{ty::Type, TokenParser};

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    New {
        class: Type,
        fields: Vec<(String, Expr)>,
    },
    If {
        condition: Box<Expr>,
        then: CodeBlock,
        or_else: Option<CodeBlock>,
    },
    Cast {
        source: Box<Expr>,
        target: Type,
    },
    Number(i32),
    Variable(String),
    Type(Type),
    Field {
        source: Box<Expr>,
        name: String,
    },
    Method {
        source: Box<Expr>,
        name: String,
        arguments: Vec<Expr>,
        template: Option<Vec<Type>>,
    },
    NamedResource {
        vtype: Type,
        name: String,
    },
    Assignement {
        target: Box<Expr>,
        to: Box<Expr>,
    },
    Loop(CodeBlock),
    Break,
    Continue,
    Block(CodeBlock),
    Return(Option<Box<Expr>>),
    BooleanExpression(Box<Expr>, BooleanOperator, Box<Expr>),
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum BooleanOperator {
    And,
    Or,
}

fn chain_expression(
    tokens: &mut VecDeque<Token>,
    exp: Expr,
) -> Result<Expr, Report<(String, Range<usize>)>> {
    let k = match tokens.get_token() {
        None | Some(Token::SemiColon(_)) => return Ok(exp),
        Some(Token::Equals(_)) => Expr::Assignement {
            target: Box::new(exp),
            to: Box::new(tokens.drain(0..).collect::<VecDeque<_>>().parse()?),
        },
        Some(Token::Keyword(_, Keyword::As)) => {
            let t = tokens.extract()?;
            Expr::Cast {
                source: Box::new(exp),
                target: t,
            }
        }
        Some(Token::BooleanOperator(_, e)) => Expr::BooleanExpression(
            box exp,
            e,
            Box::new(tokens.drain(0..).collect::<VecDeque<_>>().parse()?),
        ),
        Some(Token::Dot(_)) => {
            if let Some(Token::Literal(_, name)) = tokens.get_token() {
                match tokens.get_token() {
                    Some(Token::Block(_, ClosableType::Parenthesis, arguments)) => Expr::Method {
                        source: box exp,
                        name,
                        arguments: split_complex(arguments, |a| {
                            if matches!(a, Token::Comma(_)) {
                                SplitAction::SplitConsume
                            } else {
                                SplitAction::None
                            }
                        })
                        .into_iter()
                        .map(|a| a.parse())
                        .collect::<Result<_, _>>()?,
                        template: None,
                    },
                    Some(Token::Block(_, ClosableType::Type, template)) => {
                        if let Some(Token::Block(_, ClosableType::Parenthesis, arguments)) =
                            tokens.get_token()
                        {
                            Expr::Method {
                                source: box exp,
                                name,
                                arguments: split_complex(arguments, |a| {
                                    if matches!(a, Token::Comma(_)) {
                                        SplitAction::SplitConsume
                                    } else {
                                        SplitAction::None
                                    }
                                })
                                .into_iter()
                                .map(|a| a.parse())
                                .collect::<Result<_, _>>()?,
                                template: Some(template.parse()?),
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
                        }
                    }
                }
            } else {
                panic!("Expected literal after dot")
            }
        }
        Some(e) => {
            let mut colors = ColorGenerator::new();
            let a = colors.next();
            let out = Color::Fixed(81);
            let span = e.span();
            return Err(Report::build(ReportKind::Error, span.file.to_owned(), 0)
                .with_code(1)
                .with_message(format!("Invalid token"))
                .with_label(
                    Label::new((span.file.to_owned(), 32..33))
                        .with_message(format!("This is a {} token", e.name().fg(a)))
                        .with_color(a),
                )
                .with_note(format!(
                    "Expected {}, {}, {}, {} or {}",
                    "BooleanOperator".fg(out),
                    "SemiColon".fg(out),
                    "Equals".fg(out),
                    "As".fg(out),
                    "Dot".fg(out)
                ))
                .finish());
        }
    };
    chain_expression(tokens, k)
}

fn parse_if(tokens: &mut VecDeque<Token>) -> Result<Expr, Report<(String, Range<usize>)>> {
    Ok(Expr::If {
        condition: box take_until(tokens, |e| {
            matches!(e, Token::Block(_, ClosableType::Bracket, _))
        })
        .parse()?,
        then: {
            if let Some(Token::Block(_, ClosableType::Bracket, e)) = tokens.get_token() {
                e.parse()?
            } else {
                panic!("Expected brackets after if")
            }
        },
        or_else: if matches!(tokens.front(), Some(Token::Keyword(_, Keyword::Else))) {
            tokens.remove(0);
            match tokens.get_token() {
                Some(Token::Block(_, ClosableType::Bracket, e)) => Some(e.parse()?),
                Some(Token::Keyword(_, Keyword::If)) => Some(vec![parse_if(tokens)?]),
                _ => panic!("Expected brackets after else"),
            }
        } else {
            None
        },
    })
}

impl TokenParser<Expr> for VecDeque<Token> {
    fn parse(mut self) -> Result<Expr, Report<(String, Range<usize>)>> {
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
            Token::Literal(_, a) => Expr::Variable(a),
            Token::Keyword(_, a) => match a {
                Keyword::Return => return Ok(Expr::Return(Some(box self.parse()?))),
                Keyword::If => parse_if(&mut self)?,
                Keyword::Else => panic!("Unexpected else"),
                Keyword::Class => panic!("Unexpected class"),
                Keyword::Extends => panic!("Unexpected extends"),
                Keyword::As => panic!("Unexpected as"),
                Keyword::Loop => Expr::Loop(
                    if let Some(Token::Block(_, ClosableType::Bracket, e)) = self.get_token() {
                        e.parse()?
                    } else {
                        panic!("Expected brackets after loop")
                    },
                ),
                Keyword::Continue => Expr::Continue,
                Keyword::Break => Expr::Break,
                Keyword::While => panic!("Not yet implemented"),
                Keyword::For => panic!("Not yet implemented"),
                Keyword::In => panic!("Unexpected in"),
            },
            Token::Number(_, a) => Expr::Number(a),
            Token::TypeName(_, a) => {
                let template = match self.get_token() {
                    Some(Token::Block(_, ClosableType::Type, e)) => Some(e.parse()?),
                    Some(e) => {
                        self.push_front(e);
                        None
                    }
                    None => None,
                };
                match self.get_token() {
                    Some(Token::Block(_, ClosableType::Bracket, inside)) => Expr::New {
                        class: Type { name: a, template },
                        fields: split_complex(inside, |a| {
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
                            let value = a.parse()?;
                            Ok((name, value))
                        })
                        .collect::<Result<_, _>>()?,
                    },
                    Some(Token::Literal(_, literal)) => Expr::NamedResource {
                        vtype: Type { name: a, template },
                        name: literal.to_owned(),
                    },
                    Some(e) => {
                        self.push_front(e);
                        Expr::Type(Type { name: a, template })
                    }
                    None => Expr::Type(Type { name: a, template }),
                }
            }
            Token::Block(_, ClosableType::Bracket, b) => Expr::Block(b.parse()?),
            Token::Block(_, ClosableType::Parenthesis, b) => b.parse()?,
            Token::Block(_, ClosableType::Type, _) => panic!("Unexpected template"),
            Token::Char(_, a) => {
                Expr::Number(a.chars().fold(0, |acc, c| acc * 255 + c as u8 as i32))
            }
            Token::Comment(_, _) => return self.parse(),
        };
        chain_expression(&mut self, j)
    }
}

pub type CodeBlock = Vec<Expr>;

impl TokenParser<Vec<Expr>> for VecDeque<Token> {
    fn parse(self) -> Result<Vec<Expr>, Report<(String, Range<usize>)>> {
        split_complex(self, |a| {
            if matches!(a, Token::SemiColon(_)) {
                SplitAction::SplitConsume
            } else {
                SplitAction::None
            }
        })
        .into_iter()
        .filter(|a| a.length() != 0)
        .map(|a| a.parse())
        .collect::<Result<_, _>>()
    }
}
