use std::collections::VecDeque;

pub mod annotation;
pub mod class;
pub mod expression;
pub mod field;
pub mod method;
pub mod token_utils;
pub mod ty;

pub trait TokenExtracter<T> {
    fn extract(&mut self) -> T;
}

pub trait TokenParser<T> {
    fn parse(self) -> T;
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum Token {
    Comma,
    Dot,
    Char(String),
    DoubleDot,
    Equals,
    At,
    SemiColon,
    Literal(String),
    Keyword(Keyword),
    Number(i32),
    TypeName(String),
    Block(ClosableType, VecDeque<Token>),
    Comment(String),
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum ClosableType {
    Parenthesis,
    Bracket,
    Type,
}

fn validate(current_token: Token) -> Token {
    match current_token {
        Token::Literal(e) => match e.as_str() {
            "if" => Token::Keyword(Keyword::If),
            "else" => Token::Keyword(Keyword::Else),
            "return" => Token::Keyword(Keyword::Return),
            "class" => Token::Keyword(Keyword::Class),
            "extends" => Token::Keyword(Keyword::Extends),
            "as" => Token::Keyword(Keyword::As),
            "loop" => Token::Keyword(Keyword::Loop),
            "in" => Token::Keyword(Keyword::In),
            "for" => Token::Keyword(Keyword::For),
            "while" => Token::Keyword(Keyword::While),
            _ => Token::Literal(e),
        },
        e => e,
    }
}

pub fn parse(token_map: &mut VecDeque<Token>, char: &mut VecDeque<char>) -> Option<ClosableType> {
    let mut current_token = None;
    while let Some(c) = char.pop_front() {
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
                    token_map.push_back(Token::Comment(comment));
                }
            }
            '\'' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                let mut literal = String::new();
                while let Some(c) = char.pop_front() {
                    if c == '\'' {
                        break;
                    }
                    literal.push(c);
                }
                token_map.push_back(Token::Char(literal));
            }
            '{' | '(' | '<' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                let mut vec = VecDeque::new();
                let closable_type = parse(&mut vec, char).unwrap();
                token_map.push_back(Token::Block(closable_type, vec));
            }
            '}' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                return Some(ClosableType::Bracket);
            }
            ')' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                return Some(ClosableType::Parenthesis);
            }
            '>' => {
                if let Some(e) = current_token.take() {
                    token_map.push_back(validate(e));
                }
                return Some(ClosableType::Type);
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
                    token_map.push_back(Token::Dot);
                }
                ':' => {
                    token_map.push_back(Token::DoubleDot);
                }
                ',' => {
                    token_map.push_back(Token::Comma);
                }
                ';' => {
                    token_map.push_back(Token::SemiColon);
                }
                '=' => {
                    token_map.push_back(Token::Equals);
                }
                '@' => {
                    token_map.push_back(Token::At);
                }
                'A'..='Z' => {
                    current_token = Some(Token::TypeName(c.to_string()));
                }
                'a'..='z' => {
                    current_token = Some(Token::Literal(c.to_string()));
                }
                '0'..='9' => {
                    current_token = Some(Token::Number(c.to_digit(10).unwrap() as i32));
                }
                _ => (),
            },
            Some(Token::Literal(mut a)) if matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_') => {
                a.push(c);
                current_token = Some(Token::Literal(a));
            }
            Some(Token::TypeName(mut a)) if matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_') => {
                a.push(c);
                current_token = Some(Token::TypeName(a));
            }
            Some(Token::Number(a)) if matches!(c, '0'..='9') => {
                current_token = Some(Token::Number(a * 10 + c.to_digit(10).unwrap() as i32));
            }
            Some(e) => {
                panic!("Invalid situation {:?} {:?}", e, c)
            }
        }
    }
    None
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
