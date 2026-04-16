use chumsky::prelude::*;

use crate::token::Token;
use crate::Span;

fn escape() -> impl Parser<char, char, Error = Simple<char>> {
    just('\\').ignore_then(choice((
        just('n').to('\n'),
        just('t').to('\t'),
        just('r').to('\r'),
        just('0').to('\0'),
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
        "struct" => Token::Struct,
        "enum" => Token::Enum,
        "extension" => Token::Extension,
        "impl" => Token::Impl,
        "trait" => Token::Trait,
        "type" => Token::Type,
        "fn" => Token::Fn,
        "mut" => Token::Mut,
        "if" => Token::If,
        "else" => Token::Else,
        "loop" => Token::Loop,
        "break" => Token::Break,
        "continue" => Token::Continue,
        "return" => Token::Return,
        "match" => Token::Match,
        "true" => Token::True,
        "false" => Token::False,
        "const" => Token::Const,
        "for" => Token::For,
        "in" => Token::In,
        "while" => Token::While,
        "as" => Token::As,
        "use" => Token::Use,
        "self" => Token::SelfValue,
        "_" => Token::Underscore,
        _ => {
            if s.starts_with(|c: char| c.is_uppercase()) {
                Token::TypeName(s)
            } else {
                Token::Ident(s)
            }
        }
    });

    let multi_op = choice((
        just("::").to(Token::PathSep),
        just("=>").to(Token::FatArrow),
        just("==").to(Token::EqEq),
        just("!=").to(Token::NotEq),
        just(">=").to(Token::GtEq),
        just("<=").to(Token::LtEq),
        just("+=").to(Token::PlusAssign),
        just("-=").to(Token::MinusAssign),
        just("&&").to(Token::AndAnd),
        just("||").to(Token::OrOr),
    ));
    let single_op = choice((
        just('.').to(Token::Dot),
        just(',').to(Token::Comma),
        just(':').to(Token::Colon),
        just(';').to(Token::Semicolon),
        just('=').to(Token::Assign),
        just('+').to(Token::Plus),
        just('-').to(Token::Minus),
        just('!').to(Token::Bang),
        just('<').to(Token::Lt),
        just('>').to(Token::Gt),
        just('(').to(Token::LParen),
        just(')').to(Token::RParen),
        just('{').to(Token::LBrace),
        just('}').to(Token::RBrace),
        just('[').to(Token::LBracket),
        just(']').to(Token::RBracket),
    ));
    let op = multi_op.or(single_op);

    let block_comment = just("/*").then(take_until(just("*/"))).to(()).padded();
    let line_comment = just("//").then(none_of('\n').repeated()).to(()).padded();
    let comment = block_comment.or(line_comment);
    let ws = comment.or(filter(|c: &char| c.is_whitespace()).to(())).repeated();

    let token = choice((number, string, char_lit, ident_or_keyword, op))
        .map_with_span(|tok, span| (tok, span))
        .padded_by(ws);

    token.repeated().then_ignore(end())
}
