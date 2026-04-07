//! Bridge between the new `cythan_parser` AST and the old parser types.

use std::collections::VecDeque;

use errors::{Span, SpannedObject, SpannedVector};

use crate::parser::{
    annotation::Annotation as OldAnnotation,
    class::Class as OldClass,
    expression::{BooleanOperator, Expr as OldExpr},
    field::Field as OldField,
    method::Method as OldMethod,
    ty::{TemplateDefinition, Type as OldType},
    NumberType, Token,
};

use cythan_parser::ast;

/// Conversion context — tracks class name for Self resolution.
struct Ctx<'a> {
    filename: &'a str,
    class_name: &'a str,
    class_template: &'a Option<Vec<ast::Spanned<String>>>,
}

fn span(s: &cythan_parser::Span, f: &str) -> Span {
    Span::new(f.to_owned(), s.start, s.end)
}

fn ty(t: &ast::Type, c: &Ctx) -> OldType {
    if t.name.0 == "Self" || t.name.0 == "self" {
        // Resolve Self → class name (matching old parser behavior)
        let tmpl = if let Some(user_tmpl) = &t.template {
            Some(SpannedVector(
                span(&t.name.1, c.filename),
                user_tmpl.iter().map(|x| ty(x, c)).collect(),
            ))
        } else { c.class_template.as_ref().map(|cls_tmpl| SpannedVector(
                span(&t.name.1, c.filename),
                cls_tmpl
                    .iter()
                    .map(|p| OldType {
                        span: span(&p.1, c.filename),
                        name: SpannedObject(span(&p.1, c.filename), p.0.clone()),
                        template: None,
                    })
                    .collect(),
            )) };
        OldType {
            span: span(&t.name.1, c.filename),
            name: SpannedObject(span(&t.name.1, c.filename), c.class_name.to_string()),
            template: tmpl,
        }
    } else {
        OldType {
            span: span(&t.name.1, c.filename),
            name: SpannedObject(span(&t.name.1, c.filename), t.name.0.clone()),
            template: t.template.as_ref().map(|ts| {
                SpannedVector(span(&t.name.1, c.filename), ts.iter().map(|x| ty(x, c)).collect())
            }),
        }
    }
}

fn expr(e: &ast::Spanned<ast::Expr>, c: &Ctx) -> OldExpr {
    let s = span(&e.1, c.filename);
    match &e.0 {
        ast::Expr::Number(n) => OldExpr::Number(s, *n as i32, NumberType::Auto),
        ast::Expr::Variable(v) => OldExpr::Variable(s, v.clone()),
        ast::Expr::StringLit(st) => OldExpr::ArrayDefinition(
            s.clone(),
            SpannedVector(
                s,
                st.chars()
                    .map(|ch| OldExpr::Number(span(&e.1, c.filename), ch as i32, NumberType::Byte))
                    .collect(),
            ),
        ),
        ast::Expr::CharLit(ch) => OldExpr::Number(s, *ch as i32, NumberType::Byte),
        ast::Expr::Bool(b) => {
            OldExpr::Variable(s, if *b { "true" } else { "false" }.to_string())
        }
        ast::Expr::Field { source, name } => OldExpr::Field {
            span: s,
            source: Box::new(expr(source, c)),
            name: name.0.clone(),
        },
        ast::Expr::MethodCall { source, name, template, args } => OldExpr::Method {
            span: s.clone(),
            source: Box::new(expr(source, c)),
            name: SpannedObject(span(&name.1, c.filename), name.0.clone()),
            arguments: SpannedVector(s, args.iter().map(|a| expr(a, c)).collect()),
            template: template.as_ref().map(|ts| {
                SpannedVector(span(&e.1, c.filename), ts.iter().map(|t| ty(t, c)).collect())
            }),
        },
        ast::Expr::BinaryOp { lhs, op, rhs } => OldExpr::BooleanExpression(
            s,
            Box::new(expr(lhs, c)),
            match op {
                ast::BinOp::And => BooleanOperator::And,
                ast::BinOp::Or => BooleanOperator::Or,
            },
            Box::new(expr(rhs, c)),
        ),
        ast::Expr::If { cond, then, else_ } => OldExpr::If {
            span: s.clone(),
            condition: Box::new(expr(cond, c)),
            then: SpannedVector(s.clone(), then.iter().map(|x| expr(x, c)).collect()),
            or_else: else_
                .as_ref()
                .map(|b| SpannedVector(s, b.iter().map(|x| expr(x, c)).collect())),
        },
        ast::Expr::Loop(body) => OldExpr::Loop(
            s.clone(),
            SpannedVector(s, body.iter().map(|x| expr(x, c)).collect()),
        ),
        ast::Expr::Break => OldExpr::Break(s),
        ast::Expr::Continue => OldExpr::Continue(s),
        ast::Expr::Return(val) => {
            OldExpr::Return(s, val.as_ref().map(|v| Box::new(expr(v, c))))
        }
        ast::Expr::Assign { target, value } => OldExpr::Assignement {
            span: s,
            target: Box::new(expr(target, c)),
            to: Box::new(expr(value, c)),
        },
        ast::Expr::Cast { expr: inner, ty: t } => OldExpr::Cast {
            span: s,
            source: Box::new(expr(inner, c)),
            target: ty(t, c),
        },
        ast::Expr::New { ty: t, fields } => OldExpr::New {
            span: s.clone(),
            class: ty(t, c),
            fields: SpannedVector(
                s,
                fields
                    .iter()
                    .map(|(name, val)| (name.0.clone(), expr(val, c)))
                    .collect(),
            ),
        },
        ast::Expr::Declaration { ty: t, name, value } => {
            let named = OldExpr::NamedResource {
                span: s.clone(),
                vtype: ty(t, c),
                name: SpannedObject(span(&name.1, c.filename), name.0.clone()),
            };
            match value {
                Some(val) => OldExpr::Assignement {
                    span: s,
                    target: Box::new(named),
                    to: Box::new(expr(val, c)),
                },
                None => named,
            }
        }
        ast::Expr::ArrayLit(items) => OldExpr::ArrayDefinition(
            s.clone(),
            SpannedVector(s, items.iter().map(|x| expr(x, c)).collect()),
        ),
        ast::Expr::TypeExpr(t) => OldExpr::Type(s, ty(t, c)),
    }
}

fn annotation(a: &ast::Annotation, c: &Ctx) -> OldAnnotation {
    let mut arguments = VecDeque::new();
    if let Some(raw_args) = &a.raw_args {
        for (tok, tok_span) in raw_args {
            let s = span(tok_span, c.filename);
            use cythan_parser::token::Token as NT;
            arguments.push_back(match tok {
                NT::Number(n) => Token::Number(s, *n as i32, NumberType::Auto),
                NT::Ident(v) => Token::Literal(s, v.clone()),
                NT::TypeName(v) => Token::TypeName(s, v.clone()),
                NT::String(v) => Token::String(s, v.clone()),
                NT::Char(ch) => Token::Char(s, ch.to_string()),
                NT::True => Token::Literal(s, "true".to_string()),
                NT::False => Token::Literal(s, "false".to_string()),
                NT::Self_ => Token::Literal(s, "self".to_string()),
                NT::SelfType => Token::TypeName(s, "Self".to_string()),
                NT::Dot => Token::Dot(s),
                NT::Comma => Token::Comma(s),
                NT::Semicolon => Token::SemiColon(s),
                NT::Eq => Token::Equals(s),
                NT::At => Token::At(s),
                _ => Token::Literal(s, format!("{}", tok)),
            });
        }
    }
    OldAnnotation {
        name: a.name.0.clone(),
        arguments,
    }
}

pub fn convert_class(class: &ast::Class, filename: &str) -> OldClass {
    let c = Ctx {
        filename,
        class_name: &class.name.0,
        class_template: &class.template,
    };
    OldClass {
        name: SpannedObject(span(&class.name.1, c.filename), class.name.0.clone()),
        annotations: class.annotations.iter().map(|a| annotation(a, &c)).collect(),
        template: class.template.as_ref().map(|ts| {
            TemplateDefinition(SpannedVector(
                span(&class.name.1, c.filename),
                ts.iter().map(|t| t.0.clone()).collect(),
            ))
        }),
        fields: class.fields.iter().map(|f| {
            OldField {
                annotations: f.annotations.iter().map(|a| annotation(a, &c)).collect(),
                name: SpannedObject(span(&f.name.1, c.filename), f.name.0.clone()),
                ty: ty(&f.ty, &c),
            }
        }).collect(),
        methods: class.methods.iter().map(|m| {
            let body_span = m.body.first()
                .map(|e| span(&e.1, c.filename))
                .unwrap_or_else(|| span(&m.name.1, c.filename));
            OldMethod {
                name: SpannedObject(span(&m.name.1, c.filename), m.name.0.clone()),
                annotations: m.annotations.iter().map(|a| annotation(a, &c)).collect(),
                return_type: m.return_type.as_ref().map(|t| ty(t, &c)),
                arguments: m.args.iter().map(|(t, n)| (ty(t, &c), n.0.clone())).collect(),
                template: m.template.as_ref().map(|ts| {
                    TemplateDefinition(SpannedVector(
                        span(&m.name.1, c.filename),
                        ts.iter().map(|t| t.0.clone()).collect(),
                    ))
                }),
                code: either::Left(SpannedVector(
                    body_span,
                    m.body.iter().map(|e| expr(e, &c)).collect(),
                )),
            }
        }).collect(),
        superclass: class.superclass.as_ref().map(|t| ty(t, &c)),
    }
}
