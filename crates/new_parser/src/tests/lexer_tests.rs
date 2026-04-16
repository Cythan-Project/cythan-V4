use chumsky::prelude::*;

use crate::lexer::lexer;
use crate::token::Token;

fn lex(input: &str) -> Vec<Token> {
    lexer()
        .parse(input)
        .unwrap()
        .into_iter()
        .map(|(t, _)| t)
        .collect()
}

#[test]
fn test_lex_numbers() {
    assert_eq!(lex("0"), vec![Token::Number(0)]);
    assert_eq!(lex("42"), vec![Token::Number(42)]);
    assert_eq!(lex("9"), vec![Token::Number(9)]);
}

#[test]
fn test_lex_strings() {
    assert_eq!(lex("\"hello\""), vec![Token::String("hello".into())]);
    assert_eq!(lex("\"a\\nb\""), vec![Token::String("a\nb".into())]);
    assert_eq!(lex("\"\""), vec![Token::String(String::new())]);
}

#[test]
fn test_lex_chars() {
    assert_eq!(lex("'a'"), vec![Token::Char('a')]);
    assert_eq!(lex("'\\n'"), vec![Token::Char('\n')]);
    assert_eq!(lex("'-'"), vec![Token::Char('-')]);
    assert_eq!(lex("'\\''"), vec![Token::Char('\'')]);
}

#[test]
fn test_lex_keywords() {
    assert_eq!(
        lex("struct enum extension impl trait type fn mut"),
        vec![
            Token::Struct,
            Token::Enum,
            Token::Extension,
            Token::Impl,
            Token::Trait,
            Token::Type,
            Token::Fn,
            Token::Mut,
        ]
    );
    assert_eq!(
        lex("if else loop break continue return match"),
        vec![
            Token::If,
            Token::Else,
            Token::Loop,
            Token::Break,
            Token::Continue,
            Token::Return,
            Token::Match,
        ]
    );
    assert_eq!(
        lex("true false const for in while as self _"),
        vec![
            Token::True,
            Token::False,
            Token::Const,
            Token::For,
            Token::In,
            Token::While,
            Token::As,
            Token::SelfValue,
            Token::Underscore,
        ]
    );
}

#[test]
fn test_lex_idents_vs_typenames() {
    assert_eq!(lex("foo"), vec![Token::Ident("foo".into())]);
    assert_eq!(lex("myVar"), vec![Token::Ident("myVar".into())]);
    assert_eq!(lex("U4"), vec![Token::TypeName("U4".into())]);
    assert_eq!(lex("Self"), vec![Token::TypeName("Self".into())]);
    assert_eq!(lex("Array"), vec![Token::TypeName("Array".into())]);
}

#[test]
fn test_lex_multi_char_ops() {
    assert_eq!(
        lex(":: => == != >= <= += -= && ||"),
        vec![
            Token::PathSep,
            Token::FatArrow,
            Token::EqEq,
            Token::NotEq,
            Token::GtEq,
            Token::LtEq,
            Token::PlusAssign,
            Token::MinusAssign,
            Token::AndAnd,
            Token::OrOr,
        ]
    );
}

#[test]
fn test_lex_single_char_ops() {
    assert_eq!(
        lex(". , : ; = + - ! < > ( ) { } [ ]"),
        vec![
            Token::Dot,
            Token::Comma,
            Token::Colon,
            Token::Semicolon,
            Token::Assign,
            Token::Plus,
            Token::Minus,
            Token::Bang,
            Token::Lt,
            Token::Gt,
            Token::LParen,
            Token::RParen,
            Token::LBrace,
            Token::RBrace,
            Token::LBracket,
            Token::RBracket,
        ]
    );
}

#[test]
fn test_lex_comments() {
    assert_eq!(
        lex("42 // this is a comment\n7"),
        vec![Token::Number(42), Token::Number(7)]
    );
    assert_eq!(
        lex("42 /* block\ncomment */ 7"),
        vec![Token::Number(42), Token::Number(7)]
    );
}

#[test]
fn test_lex_full_function() {
    let src = r#"fn zero(): Self { 0 }"#;
    assert_eq!(
        lex(src),
        vec![
            Token::Fn,
            Token::Ident("zero".into()),
            Token::LParen,
            Token::RParen,
            Token::Colon,
            Token::TypeName("Self".into()),
            Token::LBrace,
            Token::Number(0),
            Token::RBrace,
        ]
    );
}

#[test]
fn test_lex_spans() {
    let tokens = lexer().parse("fn  zero").unwrap();
    assert_eq!(tokens[0].0, Token::Fn);
    assert_eq!(tokens[0].1, 0..2);
    assert_eq!(tokens[1].0, Token::Ident("zero".into()));
    assert_eq!(tokens[1].1, 4..8);
}

#[test]
fn test_lex_all_new_syntax_files() {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/new_syntax");
    let mut count = 0;
    let mut stack = vec![base.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().map_or(false, |e| e == "ct") {
                let src = std::fs::read_to_string(&path).unwrap();
                let src = src.replace('\r', "");
                let result = lexer().parse(src);
                assert!(
                    result.is_ok(),
                    "Failed to lex {}: {:?}",
                    path.display(),
                    result.unwrap_err()
                );
                count += 1;
            }
        }
    }
    assert!(count >= 8, "expected at least 8 .ct files, found {}", count);
}
