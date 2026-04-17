//! Chumsky parser: tokens → AST.
//!
//! The grammar is recursive-descent with standard precedence for binary
//! operators. Declarations (`Type name = expr;`) and assignments
//! (`lvalue = expr;`) are top-level statement forms; they are represented as
//! `Expr` variants but only appear at statement position in the surface
//! grammar.

use chumsky::prelude::*;

use crate::ast::*;
use crate::token::Token;

type PErr = Simple<Token>;

// ------------- small helpers -----------------------------------------------

fn type_name_tok() -> impl Parser<Token, String, Error = PErr> + Clone {
    select! { Token::TypeName(s) => s }
}

fn ident_tok() -> impl Parser<Token, String, Error = PErr> + Clone {
    select! { Token::Ident(s) => s }
}

fn ident_spanned() -> impl Parser<Token, Spanned<String>, Error = PErr> + Clone {
    ident_tok().map_with_span(|s, sp| (s, sp))
}

fn type_name_spanned() -> impl Parser<Token, Spanned<String>, Error = PErr> + Clone {
    type_name_tok().map_with_span(|s, sp| (s, sp))
}

// ------------- types --------------------------------------------------------

/// Full type parser: `TypeName ( :: Ident | :: TypeName )* <templates>?`.
/// Used in type positions (field types, return types, param types,
/// associated-type references like `Self::Output`).
pub fn type_parser() -> impl Parser<Token, Spanned<Type>, Error = PErr> + Clone {
    recursive(|ty: Recursive<Token, Spanned<Type>, PErr>| {
        let path_segment = choice((type_name_tok(), ident_tok()));
        let path = type_name_tok()
            .then(just(Token::PathSep).ignore_then(path_segment).repeated())
            .map(|(head, tail)| {
                if tail.is_empty() {
                    head
                } else {
                    let mut out = head;
                    for part in tail {
                        out.push_str("::");
                        out.push_str(&part);
                    }
                    out
                }
            })
            .map_with_span(|s, sp| (s, sp));

        let type_or_value = choice((
            select! { Token::Number(n) => TypeOrValue::Value(n) }
                .map_with_span(|v, sp| (v, sp)),
            ty.clone().map(|(t, sp)| (TypeOrValue::Type(t), sp)),
        ));

        let templates = just(Token::Lt)
            .ignore_then(
                type_or_value
                    .separated_by(just(Token::Comma))
                    .allow_trailing(),
            )
            .then_ignore(just(Token::Gt));

        // Qualified path: `<SelfTy as Trait>::Ident`. Matches Rust's
        // disambiguating syntax for associated-type lookups. The `::Ident`
        // tail becomes the resulting Type's `name`; the `qself` carries
        // the SelfTy and the trait reference so the typer can resolve
        // "which Trait impl's Ident is this?".
        let qualified = just(Token::Lt)
            .ignore_then(ty.clone())
            .then_ignore(just(Token::As))
            .then(ty.clone())
            .then_ignore(just(Token::Gt))
            .then_ignore(just(Token::PathSep))
            .then(type_name_spanned())
            .map_with_span(|((self_ty, trait_ty), ident), sp| {
                (
                    Type {
                        name: ident,
                        templates: Vec::new(),
                        qself: Some(Box::new(QSelf {
                            self_ty,
                            trait_ty,
                        })),
                    },
                    sp,
                )
            });

        let plain = path.then(templates.or_not()).map_with_span(
            |(name, templates), sp| {
                (
                    Type {
                        name,
                        templates: templates.unwrap_or_default(),
                        qself: None,
                    },
                    sp,
                )
            },
        );

        choice((qualified, plain))
    })
}

/// A "bare" type: just `TypeName <templates>?` — does NOT consume `::Ident`.
/// This is the right shape when parsing an expression-level TypeName prefix,
/// where `::` is part of a variant / static-call suffix, not a type path.
fn bare_type_parser() -> impl Parser<Token, Spanned<Type>, Error = PErr> + Clone {
    recursive(|bty: Recursive<Token, Spanned<Type>, PErr>| {
        let type_or_value = choice((
            select! { Token::Number(n) => TypeOrValue::Value(n) }
                .map_with_span(|v, sp| (v, sp)),
            bty.map(|(t, sp)| (TypeOrValue::Type(t), sp)),
        ));
        let templates = just(Token::Lt)
            .ignore_then(
                type_or_value
                    .separated_by(just(Token::Comma))
                    .allow_trailing(),
            )
            .then_ignore(just(Token::Gt));
        type_name_spanned()
            .then(templates.or_not())
            .map_with_span(|(name, templates), sp| {
                (
                    Type {
                        name,
                        templates: templates.unwrap_or_default(),
                        qself: None,
                    },
                    sp,
                )
            })
    })
}

// ------------- expression parser ------------------------------------------

/// Template argument list for a method call, e.g. `<0>`, `<T, U>`, `<Ng>`.
///
/// Ambiguity with `<` as less-than is resolved by requiring the parse to
/// succeed as a bracketed `<...>` list. If parsing fails (e.g. because the
/// `<` belongs to a comparison), we return `vec![]`.
fn call_templates(
    ty: impl Parser<Token, Spanned<Type>, Error = PErr> + Clone,
) -> impl Parser<Token, Vec<Spanned<TypeOrValue>>, Error = PErr> + Clone {
    let type_or_value = choice((
        select! { Token::Number(n) => TypeOrValue::Value(n) }.map_with_span(|v, sp| (v, sp)),
        ty.map(|(t, sp)| (TypeOrValue::Type(t), sp)),
    ));
    just(Token::Lt)
        .ignore_then(
            type_or_value
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .at_least(1),
        )
        .then_ignore(just(Token::Gt))
}

pub fn expr_parser() -> impl Parser<Token, Spanned<Expr>, Error = PErr> + Clone {
    recursive(|expr: Recursive<Token, Spanned<Expr>, PErr>| {
        let ty = type_parser();

        // --- block: { stmt; stmt; tail? } -----------------------------------
        let block = recursive(|block: Recursive<Token, Spanned<Block>, PErr>| {
            // A "stmt" is an expression. If it's an *expression-statement*
            // it is followed by `;`. If it's a block-like control-flow
            // expression (if/loop/match/block), the `;` is optional. The
            // final expression in a block may omit `;` — it is then the
            // block's value.
            //
            // Implementation: we greedily parse expressions, separated by
            // semicolons, allowing trailing items to omit the final `;`.
            //
            // Because blocks/ifs/etc. don't need `;`, we treat `;` as a
            // permissive separator and also allow bare `;` to skip.
            let _ = block.clone(); // silence unused warning on `block` (for rec)
            stmt_list_parser(expr.clone())
                .delimited_by(just(Token::LBrace), just(Token::RBrace))
                .map_with_span(|stmts, sp| (Block { stmts }, sp))
        });

        // --- atoms ----------------------------------------------------------
        let number = select! { Token::Number(n) => Expr::Number(n) };
        let string = select! { Token::String(s) => Expr::String(s) };
        let char_lit = select! { Token::Char(c) => Expr::Char(c) };
        let bool_lit = select! {
            Token::True => Expr::Bool(true),
            Token::False => Expr::Bool(false),
        };
        let self_v = just(Token::SelfValue).to(Expr::SelfValue);
        let var = ident_tok().map(Expr::Variable);
        let break_kw = just(Token::Break).to(Expr::Break);
        let continue_kw = just(Token::Continue).to(Expr::Continue);

        let paren_expr = expr
            .clone()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map(|(e, _)| e);

        // TypeName prefix: `TypeName<...>?` followed by one of:
        //   `{ field: expr, ... }`          → struct literal
        //   `::Ident <...>? ( args )?`      → static call (method name is ident)
        //   `::TypeName ( arg )?`           → enum variant (optionally with data)
        //
        // Uses `bare_type_parser` (stops at generics) so the suffix `::` is
        // available for the branches below.
        let bare_ty = bare_type_parser();

        // After `::`, the name can be an Ident (static method, lowercase) or
        // a TypeName (enum variant, uppercase). We treat the case distinction
        // as the meaning distinction.
        let path_name = choice((
            type_name_tok().map(PathName::Variant),
            ident_tok().map(PathName::Method),
        ))
        .map_with_span(|n, sp| (n, sp));

        let path_tail = just(Token::PathSep)
            .ignore_then(path_name)
            .then(call_templates(ty.clone()).or_not())
            .then(
                expr.clone()
                    .separated_by(just(Token::Comma))
                    .allow_trailing()
                    .delimited_by(just(Token::LParen), just(Token::RParen))
                    .or_not(),
            )
            .map(|(((name, sp), tpl), args)| TyTail::Path {
                name: (
                    match &name {
                        PathName::Variant(s) | PathName::Method(s) => s.clone(),
                    },
                    sp,
                ),
                is_variant: matches!(name, PathName::Variant(_)),
                templates: tpl.unwrap_or_default(),
                args,
            });

        let struct_tail = ident_spanned()
            .then_ignore(just(Token::Colon))
            .then(expr.clone())
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .delimited_by(just(Token::LBrace), just(Token::RBrace))
            .map(TyTail::Struct);

        let ty_prefix_expr = bare_ty.then(choice((path_tail, struct_tail)));
        let ty_expr = ty_prefix_expr.map_with_span(|(ty, tail), sp| {
            let e = match tail {
                TyTail::Struct(fields) => Expr::StructLiteral { ty, fields },
                TyTail::Path {
                    name,
                    is_variant: true,
                    templates: _,
                    args,
                } => {
                    // Enum variant: optional single data payload (first arg only).
                    let data = args.and_then(|mut a| a.pop()).map(Box::new);
                    Expr::EnumVariant {
                        ty,
                        variant: name,
                        data,
                    }
                }
                TyTail::Path {
                    name,
                    is_variant: false,
                    templates,
                    args,
                } => Expr::StaticCall {
                    ty,
                    name,
                    templates,
                    args: args.unwrap_or_default(),
                },
            };
            (e, sp)
        });

        // match ::= `match` expr `{` arm,* `}`
        // arm   ::= pattern `=>` expr
        // pattern ::= `_`  |  Type `::` Ident ( `(` binding `)` )?
        // binding ::= `_` | ident
        let pattern_binding = choice((
            just(Token::Underscore)
                .map_with_span(|_, sp| (PatternBinding::Wildcard, sp)),
            ident_tok().map_with_span(|s, sp| (PatternBinding::Name(s), sp)),
        ));
        // A pattern's type prefix uses the *bare* type parser (stops at
        // generics), so the `::` that separates type from variant is left
        // for the next combinator to consume.
        let pat_variant_name =
            choice((type_name_tok(), ident_tok())).map_with_span(|s, sp| (s, sp));
        let pattern = choice((
            just(Token::Underscore)
                .map_with_span(|_, sp| (Pattern::Wildcard, sp)),
            bare_type_parser()
                .then_ignore(just(Token::PathSep))
                .then(pat_variant_name)
                .then(
                    pattern_binding
                        .clone()
                        .delimited_by(just(Token::LParen), just(Token::RParen))
                        .or_not(),
                )
                .map_with_span(|((ty, variant), binding), sp| {
                    (
                        Pattern::Variant {
                            ty,
                            variant,
                            binding,
                        },
                        sp,
                    )
                }),
        ));

        // Control flow expressions — parsed as atoms.
        //
        // Two shapes:
        //   `if COND { THEN } ( `else` ELSE )?`
        //   `if let PAT = SCRUT { THEN } ( `else` ELSE )?`
        //
        // The `if let` form desugars to a `match SCRUT { PAT => THEN, _ =>
        // ELSE }` at parse time — no separate AST node — so downstream
        // passes only ever deal with plain Match/If.
        let if_expr = recursive(|if_rec: Recursive<Token, Spanned<Expr>, PErr>| {
            let else_branch = just(Token::Else)
                .ignore_then(choice((
                    if_rec.clone(),
                    block.clone().map(|b| {
                        let sp = b.1.clone();
                        (Expr::Block(Box::new(b)), sp)
                    }),
                )))
                .or_not();

            // `if let PAT = EXPR { BLOCK } ( else BRANCH )?` → desugar to
            // `match EXPR { PAT => { BLOCK }, _ => BRANCH }`. The wildcard
            // arm is omitted (`None`) if there's no else — callers that
            // need a fallthrough should supply one explicitly.
            let if_let = just(Token::If)
                .ignore_then(just(Token::Let))
                .ignore_then(pattern.clone())
                .then_ignore(just(Token::Assign))
                .then(expr.clone())
                .then(block.clone())
                .then(else_branch.clone())
                .map_with_span(|(((pat, scrut), then_block), else_), sp| {
                    let then_sp = then_block.1.clone();
                    let then_expr = (Expr::Block(Box::new(then_block)), then_sp.clone());
                    let mut arms = vec![MatchArm {
                        pattern: pat,
                        body: then_expr,
                    }];
                    if let Some(else_expr) = else_ {
                        let else_sp = else_expr.1.clone();
                        arms.push(MatchArm {
                            pattern: (Pattern::Wildcard, else_sp.clone()),
                            body: else_expr,
                        });
                    }
                    (
                        Expr::Match {
                            scrutinee: Box::new(scrut),
                            arms,
                        },
                        sp,
                    )
                });

            let plain_if = just(Token::If)
                .ignore_then(expr.clone())
                .then(block.clone())
                .then(else_branch)
                .map_with_span(|((cond, then), else_), sp| {
                    (
                        Expr::If {
                            cond: Box::new(cond),
                            then: Box::new(then),
                            else_: else_.map(Box::new),
                        },
                        sp,
                    )
                });

            choice((if_let, plain_if))
        });

        let loop_expr = just(Token::Loop)
            .ignore_then(block.clone())
            .map_with_span(|b, sp| (Expr::Loop(Box::new(b)), sp));

        let return_expr = just(Token::Return)
            .ignore_then(expr.clone().or_not())
            .map_with_span(|e, sp| (Expr::Return(e.map(Box::new)), sp));

        let match_arm = pattern
            .then_ignore(just(Token::FatArrow))
            .then(expr.clone())
            .map(|(pattern, body)| MatchArm { pattern, body });

        let match_expr = just(Token::Match)
            .ignore_then(expr.clone())
            .then(
                match_arm
                    .separated_by(just(Token::Comma))
                    .allow_trailing()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with_span(|(scrutinee, arms), sp| {
                (
                    Expr::Match {
                        scrutinee: Box::new(scrutinee),
                        arms,
                    },
                    sp,
                )
            });

        let block_expr = block.clone().map(|b| {
            let sp = b.1.clone();
            (Expr::Block(Box::new(b)), sp)
        });

        // Atoms that don't need a span fix-up (either already spanned or we add one).
        let simple_atom = choice((
            number.map_with_span(|e, sp| (e, sp)),
            string.map_with_span(|e, sp| (e, sp)),
            char_lit.map_with_span(|e, sp| (e, sp)),
            bool_lit.map_with_span(|e, sp| (e, sp)),
            self_v.map_with_span(|e, sp| (e, sp)),
            break_kw.map_with_span(|e, sp| (e, sp)),
            continue_kw.map_with_span(|e, sp| (e, sp)),
            var.map_with_span(|e, sp| (e, sp)),
        ));

        let atom = choice((
            paren_expr.map_with_span(|e, sp| (e, sp)),
            if_expr,
            loop_expr,
            match_expr,
            return_expr,
            block_expr,
            ty_expr,
            simple_atom,
        ));

        // --- postfix: .field, .method<T>(args), as Type ---------------------
        #[derive(Clone)]
        enum Post {
            Field(Spanned<String>),
            Method(Spanned<String>, Vec<Spanned<TypeOrValue>>, Vec<Spanned<Expr>>),
            Cast(Spanned<Type>),
        }

        let method_tail = just(Token::Dot)
            .ignore_then(ident_spanned())
            .then(call_templates(ty.clone()).or_not())
            .then(
                expr.clone()
                    .separated_by(just(Token::Comma))
                    .allow_trailing()
                    .delimited_by(just(Token::LParen), just(Token::RParen))
                    .or_not(),
            )
            .map(|((name, tpl), args)| match args {
                Some(a) => Post::Method(name, tpl.unwrap_or_default(), a),
                None => Post::Field(name),
            });

        let cast_tail = just(Token::As)
            .ignore_then(ty.clone())
            .map(Post::Cast);

        let postfix = atom
            .then(method_tail.or(cast_tail).repeated())
            .foldl(|(acc, acc_sp), post| {
                let (new_expr, end_sp) = match post {
                    Post::Field(name) => {
                        let end = name.1.end;
                        let sp = acc_sp.start..end;
                        (
                            Expr::Field(Box::new((acc, acc_sp)), name),
                            sp,
                        )
                    }
                    Post::Method(name, templates, args) => {
                        let end = args
                            .last()
                            .map(|(_, s)| s.end)
                            .unwrap_or(name.1.end)
                            + 1;
                        let sp = acc_sp.start..end;
                        (
                            Expr::MethodCall {
                                receiver: Box::new((acc, acc_sp)),
                                name,
                                templates,
                                args,
                            },
                            sp,
                        )
                    }
                    Post::Cast(t) => {
                        let end = t.1.end;
                        let sp = acc_sp.start..end;
                        (
                            Expr::Cast {
                                expr: Box::new((acc, acc_sp)),
                                ty: t,
                            },
                            sp,
                        )
                    }
                };
                (new_expr, end_sp)
            });

        // --- binary operator precedence climb --------------------------------
        fn binop_fold(
            (lhs, lsp): Spanned<Expr>,
            (op, (rhs, rsp)): (BinOp, Spanned<Expr>),
        ) -> Spanned<Expr> {
            let sp = lsp.start..rsp.end;
            (
                Expr::BinaryOp(op, Box::new((lhs, lsp)), Box::new((rhs, rsp))),
                sp,
            )
        }

        // Add/Sub
        let add_op = just(Token::Plus).to(BinOp::Add).or(just(Token::Minus).to(BinOp::Sub));
        let add_expr = postfix
            .clone()
            .then(add_op.then(postfix).repeated())
            .foldl(binop_fold);

        // Comparisons
        let cmp_op = choice((
            just(Token::EqEq).to(BinOp::EqEq),
            just(Token::NotEq).to(BinOp::NotEq),
            just(Token::GtEq).to(BinOp::GtEq),
            just(Token::LtEq).to(BinOp::LtEq),
            just(Token::Gt).to(BinOp::Gt),
            just(Token::Lt).to(BinOp::Lt),
        ));
        let cmp_expr = add_expr
            .clone()
            .then(cmp_op.then(add_expr).repeated())
            .foldl(binop_fold);

        // &&
        let and_expr = cmp_expr
            .clone()
            .then(just(Token::AndAnd).to(BinOp::And).then(cmp_expr).repeated())
            .foldl(binop_fold);

        // ||
        and_expr
            .clone()
            .then(just(Token::OrOr).to(BinOp::Or).then(and_expr).repeated())
            .foldl(binop_fold)
    })
}

/// Internal helper enum used when branching on what follows a TypeName prefix.
enum TyTail {
    Struct(Vec<(Spanned<String>, Spanned<Expr>)>),
    Path {
        name: Spanned<String>,
        is_variant: bool,
        templates: Vec<Spanned<TypeOrValue>>,
        args: Option<Vec<Spanned<Expr>>>,
    },
}

enum PathName {
    Variant(String),
    Method(String),
}

// ------------- statements --------------------------------------------------

/// Parse a list of statements inside a `{ ... }` block.
///
/// Grammar (informal):
///   stmt_list ::= ( stmt )*
///   stmt      ::= decl `;`
///               | assignment `;`
///               | expr `;`?      (semicolon optional when expr is block-like
///                                 or when this is the final element)
///
/// We flatten everything into `Vec<Spanned<Expr>>` — both statements and
/// the optional tail expression live in the same vector.
fn stmt_list_parser(
    expr: Recursive<'_, Token, Spanned<Expr>, PErr>,
) -> impl Parser<Token, Vec<Spanned<Expr>>, Error = PErr> + Clone + '_ {
    let ty = type_parser();

    // Declaration: mut? Type ident = expr
    let decl = just(Token::Mut)
        .or_not()
        .then(ty.clone())
        .then(ident_spanned())
        .then_ignore(just(Token::Assign))
        .then(expr.clone())
        .map_with_span(|(((m, ty), name), value), sp| {
            (
                Expr::Declaration {
                    mutable: m.is_some(),
                    ty,
                    name,
                    value: Box::new(value),
                },
                sp,
            )
        });

    // A non-declaration expression-statement: expr, optionally with a trailing
    // `=`, `+=`, or `-=` to turn it into an assignment.
    let assign_like = expr
        .clone()
        .then(
            choice((
                just(Token::Assign).to(None),
                just(Token::PlusAssign).to(Some(CompoundOp::AddAssign)),
                just(Token::MinusAssign).to(Some(CompoundOp::SubAssign)),
            ))
            .then(expr.clone())
            .or_not(),
        )
        .map_with_span(|(lhs, rhs), sp| match rhs {
            None => lhs,
            Some((None, v)) => (
                Expr::Assign {
                    target: Box::new(lhs),
                    value: Box::new(v),
                },
                sp,
            ),
            Some((Some(op), v)) => (
                Expr::CompoundAssign {
                    op,
                    target: Box::new(lhs),
                    value: Box::new(v),
                },
                sp,
            ),
        });

    // Decl takes priority because it starts with `mut` (unambiguous) or with
    // `TypeName ident =` (distinguishable from any expression which after a
    // TypeName would have `::` or `{` or `<`). We order decl first.
    let stmt_core = decl.or(assign_like);

    // Each statement is followed by an optional `;`. Block-like expressions
    // don't require `;`.
    stmt_core
        .then_ignore(just(Token::Semicolon).repeated())
        .repeated()
}

// ------------- function / item-level parsers -------------------------------

fn param_parser() -> impl Parser<Token, Param, Error = PErr> + Clone {
    let ty = type_parser();

    // Forms:
    //   mut? self
    //   mut? Self<...> self
    //   mut? Type ident
    let self_only = just(Token::Mut)
        .or_not()
        .then_ignore(just(Token::SelfValue))
        .map_with_span(|m, sp| Param {
            name: ("self".to_string(), sp),
            ty: None,
            mutable: m.is_some(),
            is_self: true,
        });

    let typed_self = just(Token::Mut)
        .or_not()
        .then(ty.clone())
        .then_ignore(just(Token::SelfValue))
        .map_with_span(|(m, t), sp| Param {
            name: ("self".to_string(), sp),
            ty: Some(t),
            mutable: m.is_some(),
            is_self: true,
        });

    let typed_ident = just(Token::Mut)
        .or_not()
        .then(ty)
        .then(ident_spanned())
        .map(|((m, t), name)| Param {
            name,
            ty: Some(t),
            mutable: m.is_some(),
            is_self: false,
        });

    // Order matters: `typed_self` and `typed_ident` share a Type prefix; we
    // commit based on whether `self` or an ident follows. Since chumsky's
    // default `or` backtracks on failure-without-commitment and the Type
    // parser can consume multiple tokens, we instead parse the common prefix
    // once and branch on the next token.
    let typed_param = just(Token::Mut)
        .or_not()
        .then(type_parser())
        .then(choice((
            just(Token::SelfValue).to(None),
            ident_spanned().map(Some),
        )))
        .map_with_span(|((m, t), name_or_self), sp| match name_or_self {
            None => Param {
                name: ("self".to_string(), sp),
                ty: Some(t),
                mutable: m.is_some(),
                is_self: true,
            },
            Some(name) => Param {
                name,
                ty: Some(t),
                mutable: m.is_some(),
                is_self: false,
            },
        });

    // Prefer `self_only` (no type), then the combined typed form.
    let _ = typed_self;
    let _ = typed_ident;
    self_only.or(typed_param)
}

fn function_sig_parser() -> impl Parser<Token, FunctionSig, Error = PErr> + Clone {
    let ty = type_parser();
    let templates = just(Token::Lt)
        .ignore_then(
            type_name_spanned()
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .at_least(1),
        )
        .then_ignore(just(Token::Gt));

    just(Token::Fn)
        .ignore_then(ident_spanned())
        .then(templates.or_not())
        .then(
            param_parser()
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .delimited_by(just(Token::LParen), just(Token::RParen)),
        )
        .then(just(Token::Colon).ignore_then(ty).or_not())
        .map(|(((name, tpl), params), return_type)| FunctionSig {
            name,
            templates: tpl.unwrap_or_default(),
            params,
            return_type,
        })
}

fn block_only_parser() -> impl Parser<Token, Spanned<Block>, Error = PErr> + Clone {
    stmt_list_parser(recursive_expr())
        .delimited_by(just(Token::LBrace), just(Token::RBrace))
        .map_with_span(|stmts, sp| (Block { stmts }, sp))
}

// Hacky workaround: we need a block parser *outside* the recursive expr closure
// when parsing function bodies. Build a fresh expr parser each time.
fn recursive_expr() -> Recursive<'static, Token, Spanned<Expr>, PErr> {
    // We return a recursive box of the full expression parser. Chumsky's
    // `recursive` gives us a cloneable Recursive handle; we construct it and
    // link to `expr_parser()`.
    let mut r = Recursive::declare();
    let body = expr_parser();
    r.define(body);
    r
}

fn function_parser() -> impl Parser<Token, Spanned<Function>, Error = PErr> + Clone {
    function_sig_parser()
        .then(block_only_parser())
        .map_with_span(|(sig, body), sp| (Function { sig, body }, sp))
}

fn templates_decl_parser() -> impl Parser<Token, Vec<Spanned<String>>, Error = PErr> + Clone {
    just(Token::Lt)
        .ignore_then(
            type_name_spanned()
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .at_least(1),
        )
        .then_ignore(just(Token::Gt))
        .or_not()
        .map(|v| v.unwrap_or_default())
}

fn struct_parser() -> impl Parser<Token, Spanned<Item>, Error = PErr> + Clone {
    let ty = type_parser();
    let field = ty
        .then(ident_spanned())
        .map(|(ty, name)| FieldDef { ty, name });

    just(Token::Struct)
        .ignore_then(type_name_spanned())
        .then(templates_decl_parser())
        .then(
            field
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .map_with_span(|((name, templates), fields), sp| {
            (
                Item::Struct(StructDef {
                    name,
                    templates,
                    fields,
                }),
                sp,
            )
        })
}

fn enum_parser() -> impl Parser<Token, Spanned<Item>, Error = PErr> + Clone {
    let ty = type_parser();
    let variant = type_name_spanned()
        .then(
            ty.delimited_by(just(Token::LParen), just(Token::RParen))
                .or_not(),
        )
        .then(
            just(Token::Assign)
                .ignore_then(select! { Token::Number(n) => n }.map_with_span(|n, sp| (n, sp)))
                .or_not(),
        )
        .map(|((name, data), discriminant)| EnumVariant {
            name,
            data,
            discriminant,
        });

    just(Token::Enum)
        .ignore_then(type_name_spanned())
        .then(templates_decl_parser())
        .then(
            variant
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .map_with_span(|((name, templates), variants), sp| {
            (
                Item::Enum(EnumDef {
                    name,
                    templates,
                    variants,
                }),
                sp,
            )
        })
}

fn extension_parser() -> impl Parser<Token, Spanned<Item>, Error = PErr> + Clone {
    just(Token::Extension)
        .ignore_then(type_parser())
        .then(
            function_parser()
                .repeated()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .map_with_span(|(target, methods), sp| {
            (Item::Extension(ExtensionDef { target, methods }), sp)
        })
}

fn trait_parser() -> impl Parser<Token, Spanned<Item>, Error = PErr> + Clone {
    let assoc_type = just(Token::Type)
        .ignore_then(type_name_spanned())
        .then_ignore(just(Token::Semicolon));

    let method_sig = function_sig_parser()
        .then_ignore(just(Token::Semicolon))
        .map_with_span(|s, sp| (s, sp));

    enum TItem {
        Assoc(Spanned<String>),
        Method(Spanned<FunctionSig>),
    }

    let item = choice((assoc_type.map(TItem::Assoc), method_sig.map(TItem::Method)));

    just(Token::Trait)
        .ignore_then(type_name_spanned())
        .then(templates_decl_parser())
        .then(
            item.repeated()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .map_with_span(|((name, templates), items), sp| {
            let mut associated_types = Vec::new();
            let mut methods = Vec::new();
            for it in items {
                match it {
                    TItem::Assoc(s) => associated_types.push(s),
                    TItem::Method(m) => methods.push(m),
                }
            }
            (
                Item::Trait(TraitDef {
                    name,
                    templates,
                    associated_types,
                    methods,
                }),
                sp,
            )
        })
}

fn impl_parser() -> impl Parser<Token, Spanned<Item>, Error = PErr> + Clone {
    let ty = type_parser();
    let assoc_binding = just(Token::Type)
        .ignore_then(type_name_spanned())
        .then_ignore(just(Token::Assign))
        .then(ty.clone())
        .then_ignore(just(Token::Semicolon));

    enum IItem {
        Assoc(Spanned<String>, Spanned<Type>),
        Method(Spanned<Function>),
    }

    let item = choice((
        assoc_binding.map(|(n, t)| IItem::Assoc(n, t)),
        function_parser().map(IItem::Method),
    ));

    just(Token::Impl)
        .ignore_then(ty.clone())
        .then_ignore(just(Token::For))
        .then(ty)
        .then(
            item.repeated()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .map_with_span(|((trait_ty, target), items), sp| {
            let mut associated_types = Vec::new();
            let mut methods = Vec::new();
            for it in items {
                match it {
                    IItem::Assoc(n, t) => associated_types.push((n, t)),
                    IItem::Method(m) => methods.push(m),
                }
            }
            (
                Item::Impl(ImplDef {
                    trait_ty,
                    target,
                    associated_types,
                    methods,
                }),
                sp,
            )
        })
}

fn const_parser() -> impl Parser<Token, Spanned<Item>, Error = PErr> + Clone {
    // Constant names are traditionally all-uppercase, which the lexer
    // classifies as `TypeName`. Accept either `Ident` or `TypeName` here.
    let const_name = choice((ident_tok(), type_name_tok())).map_with_span(|s, sp| (s, sp));
    just(Token::Const)
        .ignore_then(type_parser())
        .then(const_name)
        .then_ignore(just(Token::Assign))
        .then(expr_parser())
        .then_ignore(just(Token::Semicolon))
        .map_with_span(|((ty, name), value), sp| {
            (Item::Const(ConstDef { ty, name, value }), sp)
        })
}

fn use_parser() -> impl Parser<Token, Spanned<Item>, Error = PErr> + Clone {
    // `use Name;` — `Name` must be a TypeName (traits/types are uppercase
    // by convention). Accepting Ident too is future-friendly but we keep it
    // strict for now.
    just(Token::Use)
        .ignore_then(type_name_spanned())
        .then_ignore(just(Token::Semicolon))
        .map_with_span(|name, sp| (Item::Use(UseDef { name }), sp))
}

pub fn program_parser() -> impl Parser<Token, Vec<Spanned<Item>>, Error = PErr> {
    let item = choice((
        use_parser(),
        struct_parser(),
        enum_parser(),
        extension_parser(),
        trait_parser(),
        impl_parser(),
        const_parser(),
    ));
    item.repeated().then_ignore(end())
}
