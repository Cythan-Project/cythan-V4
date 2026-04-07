use chumsky::prelude::*;

use crate::token::Token;
use crate::Span;

fn escape() -> impl Parser<char, char, Error = Simple<char>> {
    just('\\').ignore_then(choice((
        just('n').to('\n'),
        just('t').to('\t'),
        just('r').to('\r'),
        just('\\').to('\\'),
        just('"').to('"'),
        just('\'').to('\''),
    )))
}

pub fn lexer() -> impl Parser<char, Vec<(Token, Span)>, Error = Simple<char>> {
    let number = text::int(10).map(|s: String| Token::Number(s.parse().unwrap()));

    let string = just('"')
        .ignore_then(escape().or(none_of('"')).repeated())
        .then_ignore(just('"'))
        .collect::<String>()
        .map(Token::String);

    let char_lit = just('\'')
        .ignore_then(escape().or(none_of('\'')))
        .then_ignore(just('\''))
        .map(Token::Char);

    let ident_or_keyword = text::ident().map(|s: String| match s.as_str() {
        "if" => Token::If,
        "else" => Token::Else,
        "return" => Token::Return,
        "class" => Token::Class,
        "extends" => Token::Extends,
        "as" => Token::As,
        "loop" => Token::Loop,
        "continue" => Token::Continue,
        "break" => Token::Break,
        "true" => Token::True,
        "false" => Token::False,
        "self" => Token::Self_,
        "Self" => Token::SelfType,
        _ => {
            if s.starts_with(|c: char| c.is_uppercase()) {
                Token::TypeName(s)
            } else {
                Token::Ident(s)
            }
        }
    });

    let op = choice((
        just("&&").to(Token::And),
        just("||").to(Token::Or),
        just('.').to(Token::Dot),
        just(',').to(Token::Comma),
        just(':').to(Token::Colon),
        just(';').to(Token::Semicolon),
        just('=').to(Token::Eq),
        just('@').to(Token::At),
        just('(').to(Token::LParen),
        just(')').to(Token::RParen),
        just('{').to(Token::LBrace),
        just('}').to(Token::RBrace),
        just('<').to(Token::LAngle),
        just('>').to(Token::RAngle),
        just('[').to(Token::LBracket),
        just(']').to(Token::RBracket),
    ));

    let block_comment = just("/*")
        .then(take_until(just("*/")))
        .to(())
        .padded();

    let line_comment = just("//")
        .then(none_of('\n').repeated())
        .to(())
        .padded();

    let comment = block_comment.or(line_comment);

    let whitespace_or_comment = comment.or(filter(|c: &char| c.is_whitespace()).to(())).repeated();

    let token = choice((number, string, char_lit, ident_or_keyword, op))
        .map_with_span(|tok, span| (tok, span))
        .padded_by(whitespace_or_comment);

    token.repeated().then_ignore(end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(input: &str) -> Vec<Token> {
        lexer()
            .parse(input)
            .unwrap()
            .into_iter()
            .map(|(tok, _)| tok)
            .collect()
    }

    #[test]
    fn test_number() {
        assert_eq!(lex("42"), vec![Token::Number(42)]);
        assert_eq!(lex("0"), vec![Token::Number(0)]);
    }

    #[test]
    fn test_string() {
        assert_eq!(lex("\"hello\""), vec![Token::String("hello".into())]);
        assert_eq!(
            lex("\"a\\nb\""),
            vec![Token::String("a\nb".into())]
        );
    }

    #[test]
    fn test_char() {
        assert_eq!(lex("'a'"), vec![Token::Char('a')]);
        assert_eq!(lex("'\\n'"), vec![Token::Char('\n')]);
    }

    #[test]
    fn test_keywords() {
        assert_eq!(lex("if else return"), vec![Token::If, Token::Else, Token::Return]);
        assert_eq!(lex("class extends"), vec![Token::Class, Token::Extends]);
        assert_eq!(lex("loop break continue"), vec![Token::Loop, Token::Break, Token::Continue]);
        assert_eq!(lex("true false"), vec![Token::True, Token::False]);
        assert_eq!(lex("self Self"), vec![Token::Self_, Token::SelfType]);
    }

    #[test]
    fn test_identifiers() {
        assert_eq!(lex("foo"), vec![Token::Ident("foo".into())]);
        assert_eq!(lex("myVar"), vec![Token::Ident("myVar".into())]);
        assert_eq!(lex("Val"), vec![Token::TypeName("Val".into())]);
        assert_eq!(lex("Array"), vec![Token::TypeName("Array".into())]);
    }

    #[test]
    fn test_operators() {
        assert_eq!(lex("&& ||"), vec![Token::And, Token::Or]);
        assert_eq!(lex(". , ; = @"), vec![Token::Dot, Token::Comma, Token::Semicolon, Token::Eq, Token::At]);
    }

    #[test]
    fn test_delimiters() {
        assert_eq!(
            lex("( ) { } < > [ ]"),
            vec![Token::LParen, Token::RParen, Token::LBrace, Token::RBrace, Token::LAngle, Token::RAngle, Token::LBracket, Token::RBracket]
        );
    }

    #[test]
    fn test_comments() {
        assert_eq!(lex("42 /* comment */ 7"), vec![Token::Number(42), Token::Number(7)]);
        assert_eq!(lex("42 // comment\n7"), vec![Token::Number(42), Token::Number(7)]);
    }

    #[test]
    fn test_class_snippet() {
        let input = r#"class Val {
            Bool equalsZero(self) {
                return self as Bool;
            }
        }"#;
        let tokens = lex(input);
        assert_eq!(tokens[0], Token::Class);
        assert_eq!(tokens[1], Token::TypeName("Val".into()));
        assert_eq!(tokens[2], Token::LBrace);
    }

    #[test]
    fn test_lex_all_std_files() {
        // Verify we can tokenize every .ct file without error
        for dir in &["cythan/std", "cythan/examples", "cythan/games", "cythan/tests"] {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries {
                    let path = entry.unwrap().path();
                    if path.extension().map_or(false, |e| e == "ct") {
                        let src = std::fs::read_to_string(&path).unwrap();
                        let src = src.replace('\r', "");
                        let result = lexer().parse(src.clone());
                        assert!(
                            result.is_ok(),
                            "Failed to lex {}: {:?}",
                            path.display(),
                            result.unwrap_err()
                        );
                    }
                }
            }
        }
    }
}
