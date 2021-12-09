use std::{collections::VecDeque, ops::Range};

use ariadne::{Color, ColorGenerator, Fmt, Label, Report, ReportKind};

use crate::errors::Span;

use self::expression::BooleanOperator;

pub mod annotation;
pub mod class;
pub mod expression;
pub mod field;
pub mod method;
pub mod token_utils;
pub mod ty;

pub trait TokenExtracter<T> {
    fn extract(&mut self) -> Result<T, Report<(String, Range<usize>)>>;
}

pub trait TokenParser<T> {
    fn parse(self) -> Result<T, Report<(String, Range<usize>)>>;
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Clone)]
pub enum Token {
    Comma(Span),
    String(Span, String),
    Dot(Span),
    Char(Span, String),
    DoubleDot(Span),
    Equals(Span),
    At(Span),
    SemiColon(Span),
    Literal(Span, String),
    Keyword(Span, Keyword),
    Number(Span, i32, NumberType),
    TypeName(Span, String),
    Block(Span, ClosableType, VecDeque<Token>),
    Comment(Span, String),
    BooleanOperator(Span, BooleanOperator),
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Clone)]
pub enum NumberType {
    Val,
    Byte,
    Auto,
    Short,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum ClosableType {
    Parenthesis,
    Brace,
    Type,
    Bracket,
}

fn validate(current_token: Token) -> Token {
    match current_token {
        Token::Literal(span, e) => match e.as_str() {
            "if" => Token::Keyword(span, Keyword::If),
            "else" => Token::Keyword(span, Keyword::Else),
            "return" => Token::Keyword(span, Keyword::Return),
            "class" => Token::Keyword(span, Keyword::Class),
            "extends" => Token::Keyword(span, Keyword::Extends),
            "as" => Token::Keyword(span, Keyword::As),
            "loop" => Token::Keyword(span, Keyword::Loop),
            "in" => Token::Keyword(span, Keyword::In),
            "for" => Token::Keyword(span, Keyword::For),
            "while" => Token::Keyword(span, Keyword::While),
            "continue" => Token::Keyword(span, Keyword::Continue),
            "break" => Token::Keyword(span, Keyword::Break),
            _ => Token::Literal(span, e),
        },
        e => e,
    }
}

pub fn parse(
    token_map: &mut VecDeque<Token>,
    char: &mut VecDeque<char>,
    initial_size: usize,
    file: &str,
) -> Result<Option<ClosableType>, Report<(String, std::ops::Range<usize>)>> {
    let mut current_token = None;
    while let Some(c) = char.pop_front() {
        let current = initial_size - char.len() - 1;
        match c {
            '/' => {
                if char.front() == Some(&'*') {
                    if let Some(e) = current_token.take() {
                        token_map.push_back(validate(e));
                    }
                    let mut comment = String::new();
                    while let Some(c) = char.pop_front() {
                        if c == '*' && char.front() == Some(&'/') {
                            break;
                        }
                        comment.push(c);
                    }
                    if let Some(e) = char.pop_front() {
                        comment.push(e);
                    }
                    token_map.push_back(Token::Comment(
                        Span::new(file.to_owned(), current, initial_size - char.len()),
                        comment,
                    ));
                }
            }
            '"' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                let mut literal = String::new();
                let mut was_backslash = false;
                while let Some(c) = char.pop_front() {
                    if was_backslash {
                        literal.push(match c {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '\\' => '\\',
                            '"' => '"',
                            _ => c,
                        });
                        was_backslash = false;
                        continue;
                    }
                    if c == '\\' {
                        was_backslash = true;
                        continue;
                    }
                    if c == '"' {
                        break;
                    }
                    literal.push(c);
                }
                token_map.push_back(Token::String(
                    Span::new(file.to_owned(), current, initial_size - char.len()),
                    literal,
                ));
            }
            '\'' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                let mut literal = String::new();
                let mut was_backslash = false;
                while let Some(c) = char.pop_front() {
                    if was_backslash {
                        literal.push(match c {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '\\' => '\\',
                            '\'' => '\'',
                            _ => c,
                        });
                        was_backslash = false;
                        continue;
                    }
                    if c == '\\' {
                        was_backslash = true;
                        continue;
                    }
                    if c == '\'' {
                        break;
                    }
                    literal.push(c);
                }
                token_map.push_back(Token::Char(
                    Span::new(file.to_owned(), current, initial_size - char.len()),
                    literal,
                ));
            }
            '{' | '(' | '<' | '[' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                let mut vec = VecDeque::new();
                let closable_type = parse(&mut vec, char, initial_size, file)?.unwrap();
                token_map.push_back(Token::Block(
                    Span::new(file.to_owned(), current, initial_size - char.len()),
                    closable_type,
                    vec,
                ));
            }
            '}' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                return Ok(Some(ClosableType::Brace));
            }
            ')' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                return Ok(Some(ClosableType::Parenthesis));
            }
            ']' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                return Ok(Some(ClosableType::Bracket));
            }
            '&' => {
                if char.front() == Some(&'&') {
                    if let Some(e) = current_token.take() {
                        token_map.push_back(validate(e));
                    }
                    char.remove(0);
                    token_map.push_back(Token::BooleanOperator(
                        Span::new(file.to_owned(), current, initial_size - char.len()),
                        BooleanOperator::And,
                    ));
                    continue;
                }
            }
            '|' => {
                if char.front() == Some(&'|') {
                    if let Some(e) = current_token.take() {
                        token_map.push_back(validate(e));
                    }
                    char.remove(0);
                    token_map.push_back(Token::BooleanOperator(
                        Span::new(file.to_owned(), current, initial_size - char.len()),
                        BooleanOperator::Or,
                    ));
                    continue;
                }
            }
            '>' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                return Ok(Some(ClosableType::Type));
            }
            '.' | ':' | ',' | ' ' | ';' | '=' | '\n' | '\r' | '@' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
            }
            _ => (),
        }
        match current_token.take() {
            None => match c {
                '.' => {
                    token_map.push_back(Token::Dot(Span::new(
                        file.to_owned(),
                        current,
                        initial_size - char.len(),
                    )));
                }
                ':' => {
                    token_map.push_back(Token::DoubleDot(Span::new(
                        file.to_owned(),
                        current,
                        initial_size - char.len(),
                    )));
                }
                ',' => {
                    token_map.push_back(Token::Comma(Span::new(
                        file.to_owned(),
                        current,
                        initial_size - char.len(),
                    )));
                }
                ';' => {
                    token_map.push_back(Token::SemiColon(Span::new(
                        file.to_owned(),
                        current,
                        initial_size - char.len(),
                    )));
                }
                '=' => {
                    token_map.push_back(Token::Equals(Span::new(
                        file.to_owned(),
                        current,
                        initial_size - char.len(),
                    )));
                }
                '@' => {
                    token_map.push_back(Token::At(Span::new(
                        file.to_owned(),
                        current,
                        initial_size - char.len(),
                    )));
                }
                'A'..='Z' => {
                    current_token = Some(Token::TypeName(
                        Span::new(file.to_owned(), current, initial_size - char.len()),
                        c.to_string(),
                    ));
                }
                'a'..='z' => {
                    current_token = Some(Token::Literal(
                        Span::new(file.to_owned(), current, initial_size - char.len()),
                        c.to_string(),
                    ));
                }
                '0'..='9' => {
                    current_token = Some(Token::Number(
                        Span::new(file.to_owned(), current, initial_size - char.len()),
                        c.to_digit(10).unwrap() as i32,
                        NumberType::Auto,
                    ));
                }
                _ => (),
            },
            Some(Token::Literal(span, mut a)) if matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_') =>
            {
                a.push(c);
                current_token = Some(Token::Literal(
                    Span::new(file.to_owned(), current, initial_size - char.len()).merge(&span),
                    a,
                ));
            }
            Some(Token::TypeName(span, mut a)) if matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_') =>
            {
                a.push(c);
                current_token = Some(Token::TypeName(
                    Span::new(file.to_owned(), current, initial_size - char.len()).merge(&span),
                    a,
                ));
            }
            Some(Token::Number(span, a, t)) if matches!(c, '0'..='9') => {
                current_token = Some(Token::Number(
                    Span::new(file.to_owned(), current, initial_size - char.len()).merge(&span),
                    a * 10 + c.to_digit(10).unwrap() as i32,
                    t,
                ));
            }
            Some(Token::Number(span, a, t)) if c == '_' => {
                current_token = Some(Token::Number(
                    Span::new(file.to_owned(), current, initial_size - char.len()).merge(&span),
                    a,
                    t,
                ));
            }
            Some(Token::Number(span, a, _t)) if c == 'v' => {
                current_token = Some(Token::Number(
                    Span::new(file.to_owned(), current, initial_size - char.len()).merge(&span),
                    a,
                    NumberType::Val,
                ));
            }
            Some(Token::Number(span, a, _t)) if c == 'b' => {
                current_token = Some(Token::Number(
                    Span::new(file.to_owned(), current, initial_size - char.len()).merge(&span),
                    a,
                    NumberType::Byte,
                ));
            }
            Some(Token::Number(span, a, _t)) if c == 'a' => {
                current_token = Some(Token::Number(
                    Span::new(file.to_owned(), current, initial_size - char.len()).merge(&span),
                    a,
                    NumberType::Auto,
                ));
            }
            Some(Token::Number(span, a, _t)) if c == 's' => {
                current_token = Some(Token::Number(
                    Span::new(file.to_owned(), current, initial_size - char.len()).merge(&span),
                    a,
                    NumberType::Short,
                ));
            }
            Some(e) => {
                let mut colors = ColorGenerator::new();
                let a = colors.next();
                let out = Color::Fixed(81);
                let span = e.span();
                return Err(Report::build(ReportKind::Error, span.file.to_owned(), 0)
                    .with_code(1)
                    .with_message("Invalid token")
                    .with_label(
                        Label::new(span.as_span())
                            .with_message(format!("This is a {} token", e.name().fg(a)))
                            .with_color(a),
                    )
                    .with_note(format!(
                        "Expected {}, {} or {}",
                        "Literal".fg(out),
                        "TypeName".fg(out),
                        "Number".fg(out)
                    ))
                    .finish());
            }
        }
    }
    Ok(None)
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum Keyword {
    Return,
    If,
    Extends,
    Else,
    Class,
    As,
    Loop,
    Continue,
    Break,
    While,
    For,
    In,
}
impl Token {
    pub fn span(&self) -> &Span {
        let (Self::At(span, ..)
        | Self::Block(span, ..)
        | Self::BooleanOperator(span, ..)
        | Self::Char(span, ..)
        | Self::Comma(span, ..)
        | Self::Comment(span, ..)
        | Self::Dot(span, ..)
        | Self::DoubleDot(span, ..)
        | Self::Equals(span, ..)
        | Self::String(span, ..)
        | Self::Keyword(span, ..)
        | Self::Literal(span, ..)
        | Self::Number(span, ..)
        | Self::SemiColon(span, ..)
        | Self::TypeName(span, ..)) = self;
        span
    }

    pub fn name(&self) -> String {
        match self {
            Token::Comma(_) => "Comma",
            Token::Dot(_) => "Dot",
            Token::Char(_, _) => "Char",
            Token::DoubleDot(_) => "DoubleDot",
            Token::Equals(_) => "Equals",
            Token::At(_) => "At",
            Token::SemiColon(_) => "SemiColon",
            Token::Literal(_, _) => "Literal",
            Token::Keyword(_, a) => return format!("{:?}", a),
            Token::Number(_, _, _) => "Number",
            Token::TypeName(_, _) => "TypeName",
            Token::Block(_, _, _) => "Block",
            Token::Comment(_, _) => "Comment",
            Token::String(_, _) => "String",
            Token::BooleanOperator(_, _) => "BooleanOperator",
        }
        .to_owned()
    }
}
