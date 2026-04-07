use chumsky::prelude::*;
use chumsky::Stream;

use crate::ast::*;
use crate::token::Token;
use crate::Span;

type PErr = Simple<Token>;

/// Convert lexer output to a chumsky Stream for the token parser.
pub fn token_stream(
    tokens: Vec<(Token, Span)>,
    len: usize,
) -> Stream<'static, Token, Span, Box<dyn Iterator<Item = (Token, Span)>>> {
    Stream::from_iter(len..len + 1, Box::new(tokens.into_iter()) as Box<dyn Iterator<Item = _>>)
}

fn type_parser() -> impl Parser<Token, Type, Error = PErr> + Clone {
    recursive(|ty| {
        let name = select! {
            Token::TypeName(s) => s,
            Token::SelfType => "Self".to_string(),
            Token::Number(n) => n.to_string(),
        }
        .map_with_span(|s, span| (s, span));

        let template = just(Token::LAngle)
            .ignore_then(ty.separated_by(just(Token::Comma)).allow_trailing())
            .then_ignore(just(Token::RAngle));

        name.then(template.or_not()).map(|(name, template)| Type {
            name,
            template,
        })
    })
}

fn expr_parser() -> impl Parser<Token, Spanned<Expr>, Error = PErr> + Clone {
    recursive(|expr: Recursive<Token, Spanned<Expr>, PErr>| {
        let block = expr
            .clone()
            .separated_by(just(Token::Semicolon))
            .allow_trailing()
            .delimited_by(just(Token::LBrace), just(Token::RBrace));

        let number = select! { Token::Number(n) => Expr::Number(n) };
        let bool_lit = select! {
            Token::True => Expr::Bool(true),
            Token::False => Expr::Bool(false),
        };
        let variable = select! {
            Token::Ident(s) => Expr::Variable(s),
            Token::Self_ => Expr::Variable("self".to_string()),
        };
        let string_lit = select! { Token::String(s) => Expr::StringLit(s) };
        let char_lit = select! { Token::Char(c) => Expr::CharLit(c) };

        let ident = name_ident().map_with_span(|s, span| (s, span));
        let ty = type_parser();

        // if/else with recursive else-if chains
        let if_expr = recursive(|if_rec: Recursive<Token, Expr, PErr>| {
            just(Token::If)
                .ignore_then(expr.clone())
                .then(block.clone())
                .then(
                    just(Token::Else)
                        .ignore_then(
                            // else if ... (recursive)
                            if_rec
                                .map_with_span(|e, span| vec![(e, span)])
                                // else { ... }
                                .or(block.clone()),
                        )
                        .or_not(),
                )
                .map(|((cond, then), else_)| Expr::If {
                    cond: Box::new(cond),
                    then,
                    else_,
                })
        });

        // loop
        let loop_expr = just(Token::Loop)
            .ignore_then(block.clone())
            .map(Expr::Loop);

        // return
        let return_expr = just(Token::Return)
            .ignore_then(expr.clone().or_not())
            .map(|e| Expr::Return(e.map(Box::new)));

        // break / continue
        let break_expr = just(Token::Break).to(Expr::Break);
        let continue_expr = just(Token::Continue).to(Expr::Continue);

        // Type expression: new or declaration or just type
        let type_expr = ty
            .clone()
            .then(
                // New: Type { field = val, ... }
                ident
                    .clone()
                    .then_ignore(just(Token::Eq))
                    .then(expr.clone())
                    .separated_by(just(Token::Comma))
                    .allow_trailing()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace))
                    .map(TypeFollower::New)
                    // Declaration: Type name = expr  OR  Type name
                    .or(ident
                        .clone()
                        .then(just(Token::Eq).ignore_then(expr.clone()).or_not())
                        .map(|(name, val)| TypeFollower::Decl(name, val)))
                    .or_not(),
            )
            .map(|(ty, follower)| match follower {
                Some(TypeFollower::New(fields)) => Expr::New { ty, fields },
                Some(TypeFollower::Decl(name, Some(val))) => Expr::Declaration {
                    ty,
                    name,
                    value: Some(Box::new(val)),
                },
                Some(TypeFollower::Decl(name, None)) => Expr::Declaration {
                    ty,
                    name,
                    value: None,
                },
                None => Expr::TypeExpr(ty),
            });

        // Array literal [a, b, c]
        let array_lit = expr
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .delimited_by(just(Token::LBracket), just(Token::RBracket))
            .map(Expr::ArrayLit);

        // Parenthesized expression — unwrap the inner spanned to just the Expr
        let paren = expr
            .clone()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map(|(e, _)| e);

        // Atom: any primary expression
        // Group into batches with .boxed() to keep type complexity manageable
        let control_flow = choice((
            if_expr,
            loop_expr,
            return_expr,
            break_expr,
            continue_expr,
        ))
        .boxed();

        let literals = choice((bool_lit, number, string_lit, char_lit)).boxed();

        let compound = choice((array_lit, paren, type_expr, variable)).boxed();

        let atom = choice((control_flow, literals, compound))
            .map_with_span(|e, span: Span| (e, span))
            .boxed();

        // Template args for method call: <Type, Type>
        let template_args = just(Token::LAngle)
            .ignore_then(ty.clone().separated_by(just(Token::Comma)).allow_trailing())
            .then_ignore(just(Token::RAngle));

        // Method args: (expr, expr)
        let method_args = expr
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .delimited_by(just(Token::LParen), just(Token::RParen));

        // Dot chaining: .field or .method<T>(args)
        let dot_chain = just(Token::Dot)
            .ignore_then(ident.clone())
            .then(template_args.or_not())
            .then(method_args.or_not());

        // Apply dot chains
        let chained = atom.then(dot_chain.repeated()).foldl(|source, ((name, template), args)| {
            let span = source.1.start..name.1.end;
            let e = match args {
                Some(args) => {
                    let span = source.1.start..args.last().map_or(name.1.end, |(_, s)| s.end);
                    (Expr::MethodCall {
                        source: Box::new(source),
                        name,
                        template,
                        args,
                    }, span)
                }
                None => (Expr::Field {
                    source: Box::new(source),
                    name: name.clone(),
                }, span),
            };
            e
        });

        // Cast: expr as Type
        let cast = chained
            .clone()
            .then(just(Token::As).ignore_then(ty.clone()).or_not())
            .map_with_span(|(expr, cast_ty), span| match cast_ty {
                Some(ty) => (Expr::Cast { expr: Box::new(expr), ty }, span),
                None => expr,
            });

        // Boolean operators: && ||
        let and = cast
            .clone()
            .then(
                just(Token::And)
                    .ignore_then(cast.clone())
                    .repeated(),
            )
            .foldl(|lhs, rhs| {
                let span = lhs.1.start..rhs.1.end;
                (Expr::BinaryOp {
                    lhs: Box::new(lhs),
                    op: BinOp::And,
                    rhs: Box::new(rhs),
                }, span)
            });

        let or = and
            .clone()
            .then(
                just(Token::Or)
                    .ignore_then(and.clone())
                    .repeated(),
            )
            .foldl(|lhs, rhs| {
                let span = lhs.1.start..rhs.1.end;
                (Expr::BinaryOp {
                    lhs: Box::new(lhs),
                    op: BinOp::Or,
                    rhs: Box::new(rhs),
                }, span)
            });

        // Assignment: expr = expr (right-associative, lowest precedence)
        

        or
            .clone()
            .then(just(Token::Eq).ignore_then(expr.clone()).or_not())
            .map_with_span(|(target, value), span| match value {
                Some(value) => (Expr::Assign {
                    target: Box::new(target),
                    value: Box::new(value),
                }, span),
                None => target,
            })
    })
}

#[derive(Debug, Clone)]
enum TypeFollower {
    New(Vec<(Spanned<String>, Spanned<Expr>)>),
    Decl(Spanned<String>, Option<Spanned<Expr>>),
}

fn annotation_parser() -> impl Parser<Token, Annotation, Error = PErr> + Clone {
    let name = select! {
        Token::Ident(s) => s,
        Token::TypeName(s) => s,
    }
    .map_with_span(|s, span| (s, span));

    // Capture raw tokens inside parens — annotation args use non-standard syntax
    let raw_args = none_of(Token::RParen)
        .map_with_span(|tok, span| (tok, span))
        .repeated()
        .delimited_by(just(Token::LParen), just(Token::RParen));

    just(Token::At)
        .ignore_then(name)
        .then(raw_args.or_not())
        .map(|(name, raw_args)| Annotation { name, raw_args })
}

fn field_parser() -> impl Parser<Token, Field, Error = PErr> {
    annotation_parser()
        .repeated()
        .then(type_parser())
        .then(
            select! { Token::Ident(s) => s }.map_with_span(|s, span| (s, span)),
        )
        .map(|((annotations, ty), name)| Field {
            name,
            ty,
            annotations,
        })
}

/// An identifier that also accepts keywords used as names (e.g. `true`, `false` in Bool.ct, `self` in args)
fn name_ident() -> impl Parser<Token, String, Error = PErr> + Clone {
    select! {
        Token::Ident(s) => s,
        Token::True => "true".to_string(),
        Token::False => "false".to_string(),
        Token::Self_ => "self".to_string(),
    }
}

fn method_parser() -> impl Parser<Token, Method, Error = PErr> {
    let ident = name_ident().map_with_span(|s, span| (s, span));
    let ty = type_parser();

    let template_def = just(Token::LAngle)
        .ignore_then(
            select! {
                Token::TypeName(s) => s,
                Token::Number(n) => n.to_string(),
            }
            .map_with_span(|s, span| (s, span))
            .separated_by(just(Token::Comma))
            .allow_trailing(),
        )
        .then_ignore(just(Token::RAngle));

    let self_arg = just(Token::Self_)
        .map_with_span(|_, span: Span| {
            let ty = Type {
                name: ("self".to_string(), span.clone()),
                template: None,
            };
            (ty, ("self".to_string(), span))
        });

    let typed_arg = ty
        .clone()
        .then(ident.clone())
        .map(|(ty, name)| (ty, name));

    let untyped_arg = ident.clone().map(|name| {
        let ty = Type {
            name: name.clone(),
            template: None,
        };
        (ty, name)
    });

    let arg = self_arg.or(typed_arg).or(untyped_arg);

    let args = arg
        .separated_by(just(Token::Comma))
        .allow_trailing()
        .delimited_by(just(Token::LParen), just(Token::RParen));

    let body = expr_parser()
        .separated_by(just(Token::Semicolon))
        .allow_trailing()
        .delimited_by(just(Token::LBrace), just(Token::RBrace));

    annotation_parser()
        .repeated()
        .then(ty.or_not())
        .then(ident)
        .then(template_def.or_not())
        .then(args)
        .then(body)
        .map(
            |(((((annotations, return_type), name), template), args), body)| Method {
                name,
                annotations,
                return_type,
                template,
                args,
                body,
            },
        )
}

#[derive(Debug, Clone)]
enum ClassItem {
    Field(Field),
    Method(Method),
}

fn class_item_parser() -> impl Parser<Token, ClassItem, Error = PErr> {
    // Field: type name (no parens/braces after name)
    // Method: type? name template? (args) { body }
    // Key insight: a field is always "Type name" followed by ; or end.
    // A method always has (...) { ... } after the name.
    // We use the full method_parser and field_parser with .or(), relying on
    // chumsky's .or() backtracking behavior.
    method_parser()
        .map(ClassItem::Method)
        .or(field_parser().map(ClassItem::Field))
        .boxed()
}

pub fn class_parser() -> impl Parser<Token, Spanned<Class>, Error = PErr> {
    let ident_spanned =
        select! { Token::TypeName(s) => s }.map_with_span(|s, span| (s, span));

    let template_def = just(Token::LAngle)
        .ignore_then(
            select! {
                Token::TypeName(s) => s,
                Token::Number(n) => n.to_string(),
            }
            .map_with_span(|s, span| (s, span))
            .separated_by(just(Token::Comma))
            .allow_trailing(),
        )
        .then_ignore(just(Token::RAngle));

    let extends = just(Token::Extends).ignore_then(type_parser());

    // Methods end with }, fields end with ; — items are simply repeated
    let body = class_item_parser()
        .then_ignore(just(Token::Semicolon).or_not())
        .repeated()
        .delimited_by(just(Token::LBrace), just(Token::RBrace));

    annotation_parser()
        .repeated()
        .then_ignore(just(Token::Class))
        .then(ident_spanned)
        .then(template_def.or_not())
        .then(extends.or_not())
        .then(body)
        .map_with_span(|((((annotations, name), template), superclass), items), span| {
            let mut fields = Vec::new();
            let mut methods = Vec::new();
            for item in items {
                match item {
                    ClassItem::Field(f) => fields.push(f),
                    ClassItem::Method(m) => methods.push(m),
                }
            }
            (
                Class {
                    name,
                    annotations,
                    template,
                    superclass,
                    fields,
                    methods,
                },
                span,
            )
        })
}

pub fn program_parser() -> impl Parser<Token, Vec<Spanned<Class>>, Error = PErr> {
    class_parser().repeated().then_ignore(end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lexer;

    fn parse_expr(input: &str) -> Spanned<Expr> {
        let tokens = lexer().parse(input).unwrap();
        let len = input.len();
        expr_parser().parse(token_stream(tokens, len)).unwrap()
    }

    fn parse_class(input: &str) -> Class {
        let tokens = lexer().parse(input).unwrap();
        let len = input.len();
        let (class, _) = class_parser()
            .parse(token_stream(tokens, len))
            .unwrap();
        class
    }

    #[test]
    fn test_number() {
        let (expr, _) = parse_expr("42");
        assert_eq!(expr, Expr::Number(42));
    }

    #[test]
    fn test_variable() {
        let (expr, _) = parse_expr("foo");
        assert_eq!(expr, Expr::Variable("foo".into()));
    }

    #[test]
    fn test_string() {
        let (expr, _) = parse_expr("\"hello\"");
        assert_eq!(expr, Expr::StringLit("hello".into()));
    }

    #[test]
    fn test_bool() {
        let (expr, _) = parse_expr("true");
        assert_eq!(expr, Expr::Bool(true));
    }

    #[test]
    fn test_method_call() {
        let (expr, _) = parse_expr("a.foo(1, 2)");
        match expr {
            Expr::MethodCall { name, args, .. } => {
                assert_eq!(name.0, "foo");
                assert_eq!(args.len(), 2);
            }
            _ => panic!("Expected MethodCall, got {:?}", expr),
        }
    }

    #[test]
    fn test_field_access() {
        let (expr, _) = parse_expr("a.x");
        match expr {
            Expr::Field { name, .. } => assert_eq!(name.0, "x"),
            _ => panic!("Expected Field, got {:?}", expr),
        }
    }

    #[test]
    fn test_if_else() {
        let (expr, _) = parse_expr("if true { 1 } else { 2 }");
        match expr {
            Expr::If { else_, .. } => assert!(else_.is_some()),
            _ => panic!("Expected If, got {:?}", expr),
        }
    }

    #[test]
    fn test_cast() {
        let (expr, _) = parse_expr("x as Bool");
        match expr {
            Expr::Cast { ty, .. } => assert_eq!(ty.name.0, "Bool"),
            _ => panic!("Expected Cast, got {:?}", expr),
        }
    }

    #[test]
    fn test_and_or() {
        let (expr, _) = parse_expr("a && b || c");
        match expr {
            Expr::BinaryOp { op: BinOp::Or, .. } => {}
            _ => panic!("Expected Or at top level, got {:?}", expr),
        }
    }

    #[test]
    fn test_declaration() {
        let (expr, _) = parse_expr("Val x = 5");
        match expr {
            Expr::Declaration { ty, name, value } => {
                assert_eq!(ty.name.0, "Val");
                assert_eq!(name.0, "x");
                assert!(value.is_some());
            }
            _ => panic!("Expected Declaration, got {:?}", expr),
        }
    }

    #[test]
    fn test_new() {
        let (expr, _) = parse_expr("Self { x = 1, y = 2 }");
        match expr {
            Expr::New { ty, fields } => {
                assert_eq!(ty.name.0, "Self");
                assert_eq!(fields.len(), 2);
            }
            _ => panic!("Expected New, got {:?}", expr),
        }
    }

    #[test]
    fn test_simple_class() {
        let class = parse_class(
            "class Foo { Val x; Val bar(self) { return self.x; }; }",
        );
        assert_eq!(class.name.0, "Foo");
        assert_eq!(class.fields.len(), 1);
        assert_eq!(class.methods.len(), 1);
        assert_eq!(class.methods[0].name.0, "bar");
    }

    #[test]
    fn test_method_no_return() {
        let input = "class X { add(self, Self other) { return self; } }";
        let tokens = lexer().parse(input).unwrap();
        let len = input.len();
        let result = program_parser().parse(token_stream(tokens, len));
        assert!(result.is_ok(), "Failed: {:?}", result.unwrap_err());
        let class = &result.unwrap()[0].0;
        assert_eq!(class.methods.len(), 1);
        assert_eq!(class.methods[0].name.0, "add");
    }

    #[test]
    fn test_chess_ct() {
        let src = std::fs::read_to_string("../../cythan/games/Chess.ct")
            .or_else(|_| std::fs::read_to_string("cythan/games/Chess.ct"))
            .unwrap()
            .replace('\r', "");
        let tokens = lexer().parse(src.clone()).unwrap();
        let len = src.len();
        let result = program_parser().parse(token_stream(tokens, len));
        assert!(result.is_ok(), "Chess.ct parse failed: {:?}", result.unwrap_err());
    }

    #[test]
    fn test_bool_ct_directly() {
        let src = std::fs::read_to_string("../../cythan/std/Bool.ct")
            .or_else(|_| std::fs::read_to_string("cythan/std/Bool.ct"))
            .unwrap()
            .replace('\r', "");
        let tokens = lexer().parse(src.clone()).unwrap();
        let len = src.len();
        let result = program_parser().parse(token_stream(tokens, len));
        assert!(result.is_ok(), "Bool.ct parse failed: {:?}", result.unwrap_err());
        let classes = result.unwrap();
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].0.name.0, "Bool");
    }

    #[test]
    fn test_method_not() {
        let input = "Self not(self) { return self; }";
        let tokens = lexer().parse(input).unwrap();
        let len = input.len();
        let result = method_parser().parse(token_stream(tokens, len));
        assert!(result.is_ok(), "Method parse failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().name.0, "not");
    }

    #[test]
    fn test_method_parse() {
        let input = "Val bar(self) { return self.x; }";
        let tokens = lexer().parse(input).unwrap();
        let len = input.len();
        let result = method_parser().parse(token_stream(tokens, len));
        assert!(result.is_ok(), "Method parse failed: {:?}", result.unwrap_err());
        let method = result.unwrap();
        assert_eq!(method.name.0, "bar");
    }

    #[test]
    fn test_generic_class() {
        let class = parse_class("class Option<T> { Bool is_none; T t; }");
        assert_eq!(class.name.0, "Option");
        assert!(class.template.is_some());
        assert_eq!(class.template.unwrap().len(), 1);
        assert_eq!(class.fields.len(), 2);
    }

    #[test]
    fn test_parse_all_ct_files() {
        for dir in &[
            "cythan/std",
            "cythan/examples",
            "cythan/games",
            "cythan/tests",
        ] {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries {
                    let path = entry.unwrap().path();
                    if path.extension().map_or(false, |e| e == "ct") {
                        let src = std::fs::read_to_string(&path).unwrap().replace('\r', "");
                        let tokens = lexer().parse(src.clone());
                        assert!(tokens.is_ok(), "Lex failed for {}", path.display());
                        let tokens = tokens.unwrap();
                        let len = src.len();
                        let result = program_parser().parse(token_stream(tokens, len));
                        assert!(
                            result.is_ok(),
                            "Parse failed for {}: {:?}",
                            path.display(),
                            result.unwrap_err()
                        );
                    }
                }
            }
        }
    }
}
