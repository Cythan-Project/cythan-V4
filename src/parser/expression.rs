use std::collections::VecDeque;

use crate::parser::{
    token_utils::{split_simple, take_until},
    ClosableType, Keyword, Token, TokenExtracter,
};

use super::{
    ty::{TemplateDefinition, Type},
    TokenParser,
};

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
}

fn chain_expression(tokens: &mut VecDeque<Token>, exp: Expr) -> Expr {
    let k = match tokens.pop_front() {
        None | Some(Token::SemiColon) => return exp,
        Some(Token::Equals) => Expr::Assignement {
            target: Box::new(exp),
            to: Box::new(tokens.drain(0..).collect::<VecDeque<_>>().parse()),
        },
        Some(Token::Keyword(Keyword::As)) => {
            let t = tokens.extract();
            Expr::Cast {
                source: Box::new(exp),
                target: t,
            }
        }
        Some(Token::Dot) => {
            if let Some(Token::Literal(name)) = tokens.pop_front() {
                match tokens.pop_front() {
                    Some(Token::Block(ClosableType::Parenthesis, arguments)) => Expr::Method {
                        source: box exp,
                        name,
                        arguments: split_simple(arguments, &Token::Comma)
                            .into_iter()
                            .map(|a| a.parse())
                            .collect(),
                        template: None,
                    },
                    Some(Token::Block(ClosableType::Type, template)) => {
                        if let Some(Token::Block(ClosableType::Parenthesis, arguments)) =
                            tokens.pop_front()
                        {
                            Expr::Method {
                                source: box exp,
                                name,
                                arguments: split_simple(arguments, &Token::Comma)
                                    .into_iter()
                                    .map(|a| a.parse())
                                    .collect(),
                                template: Some(template.parse()),
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
        Some(e) => panic!("Unexpected token after literal {:?}", e),
    };
    chain_expression(tokens, k)
}

impl TokenParser<Expr> for VecDeque<Token> {
    fn parse(mut self) -> Expr {
        let tk = if let Some(e) = self.pop_front() {
            e
        } else {
            panic!("Expected expression")
        };
        let j = match tk {
            Token::Comma => panic!("Unexpected comma"),
            Token::At => panic!("Unexpected Annotation"),
            Token::Dot => panic!("Unexpected comma"),
            Token::DoubleDot => panic!("Unexpected comma"),
            Token::SemiColon => panic!("Unexpected comma"),
            Token::Equals => {
                println!("{:?}", self);
                panic!("Unexpected equals")
            }
            Token::Literal(a) => Expr::Variable(a),
            Token::Keyword(a) => match a {
                Keyword::Return => return Expr::Return(Some(box self.parse())),
                Keyword::If => Expr::If {
                    condition: box take_until(&mut self, |e| {
                        matches!(e, Token::Block(ClosableType::Bracket, _))
                    })
                    .parse(),
                    then: {
                        if let Some(Token::Block(ClosableType::Bracket, e)) = self.pop_front() {
                            e.parse()
                        } else {
                            panic!("Expected brackets after if")
                        }
                    },
                    or_else: if matches!(self.front(), Some(Token::Keyword(Keyword::Else))) {
                        self.remove(0);
                        if let Some(Token::Block(ClosableType::Bracket, e)) = self.pop_front() {
                            Some(e.parse())
                        } else {
                            panic!("Expected brackets after if")
                        }
                    } else {
                        None
                    },
                },
                Keyword::Else => panic!("Unexpected else"),
                Keyword::Class => panic!("Unexpected class"),
                Keyword::Extends => panic!("Unexpected extends"),
                Keyword::As => panic!("Unexpected as"),
                Keyword::Loop => Expr::Loop(
                    if let Some(Token::Block(ClosableType::Bracket, e)) = self.pop_front() {
                        e.parse()
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
            Token::Number(a) => Expr::Number(a),
            Token::TypeName(a) => {
                let template = match self.pop_front() {
                    Some(Token::Block(ClosableType::Type, e)) => Some(e.parse()),
                    Some(e) => {
                        self.push_front(e);
                        None
                    }
                    None => None,
                };
                match self.pop_front() {
                    Some(Token::Block(ClosableType::Bracket, inside)) => Expr::New {
                        class: Type { name: a, template },
                        fields: split_simple(inside, &Token::Comma)
                            .into_iter()
                            .map(|mut a| {
                                let name = if let Some(Token::Literal(name)) = a.pop_front() {
                                    name
                                } else {
                                    panic!("Expected argument name");
                                };
                                if !matches!(a.pop_front(), Some(Token::Equals)) {
                                    panic!("Expected equals");
                                };
                                let value = a.parse();
                                (name, value)
                            })
                            .collect(),
                    },
                    Some(Token::Literal(literal)) => Expr::NamedResource {
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
            Token::Block(ClosableType::Bracket, b) => Expr::Block(b.parse()),
            Token::Block(ClosableType::Parenthesis, b) => b.parse(),
            Token::Block(ClosableType::Type, _) => panic!("Unexpected template"),
            Token::Char(a) => Expr::Number(a.chars().fold(0, |acc, c| acc * 255 + c as u8 as i32)),
            Token::Comment(_) => return self.parse(),
        };
        chain_expression(&mut self, j)
    }
}

pub type CodeBlock = Vec<Expr>;

impl TokenParser<Vec<Expr>> for VecDeque<Token> {
    fn parse(self) -> Vec<Expr> {
        split_simple(self, &Token::SemiColon)
            .into_iter()
            .filter(|a| !a.is_empty())
            .map(|a| a.parse())
            .collect()
    }
}
