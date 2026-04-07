use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    // Literals
    Number(i64),
    Ident(String),
    TypeName(String),
    String(String),
    Char(char),

    // Keywords
    If,
    Else,
    Return,
    Class,
    Extends,
    As,
    Loop,
    Continue,
    Break,
    True,
    False,
    Self_,
    SelfType,

    // Operators & Punctuation
    Dot,
    Comma,
    Colon,
    Semicolon,
    Eq,
    At,
    And,
    Or,

    // Delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LAngle,
    RAngle,
    LBracket,
    RBracket,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Token::Number(n) => write!(f, "{}", n),
            Token::Ident(s) => write!(f, "{}", s),
            Token::TypeName(s) => write!(f, "{}", s),
            Token::String(s) => write!(f, "\"{}\"", s),
            Token::Char(c) => write!(f, "'{}'", c),
            Token::If => write!(f, "if"),
            Token::Else => write!(f, "else"),
            Token::Return => write!(f, "return"),
            Token::Class => write!(f, "class"),
            Token::Extends => write!(f, "extends"),
            Token::As => write!(f, "as"),
            Token::Loop => write!(f, "loop"),
            Token::Continue => write!(f, "continue"),
            Token::Break => write!(f, "break"),
            Token::True => write!(f, "true"),
            Token::False => write!(f, "false"),
            Token::Self_ => write!(f, "self"),
            Token::SelfType => write!(f, "Self"),
            Token::Dot => write!(f, "."),
            Token::Comma => write!(f, ","),
            Token::Colon => write!(f, ":"),
            Token::Semicolon => write!(f, ";"),
            Token::Eq => write!(f, "="),
            Token::At => write!(f, "@"),
            Token::And => write!(f, "&&"),
            Token::Or => write!(f, "||"),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::LAngle => write!(f, "<"),
            Token::RAngle => write!(f, ">"),
            Token::LBracket => write!(f, "["),
            Token::RBracket => write!(f, "]"),
        }
    }
}
