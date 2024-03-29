use std::collections::{HashMap, VecDeque};

use either::Either;
use errors::{invalid_type_template, report_similar, Error, Span, SpannedObject, SpannedVector};

use crate::{
    compiler::class_loader::ClassLoader,
    parser::{
        expression::TokenProcessor,
        token_utils::{split_complex, SplitAction},
        ClosableType, Keyword, TokenExtracter,
    },
};

use super::{
    annotation::Annotation,
    expression::Expr,
    field::Field,
    method::{Method, MethodView},
    ty::{TemplateDefinition, Type},
    Token, TokenParser,
};

#[derive(Debug, Clone)]
pub struct Class {
    pub name: SpannedObject<String>,
    pub annotations: Vec<Annotation>,
    pub template: Option<TemplateDefinition>,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    pub superclass: Option<Type>,
}

impl Class {
    pub fn get_method_mut(&mut self, method: &str) -> &mut Method {
        self.methods
            .iter_mut()
            .find(|x| x.name.1 == method)
            .unwrap()
    }
}

pub struct ClassView {
    pub ty: Type,
    pub name: SpannedObject<String>,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    pub superclass: Option<Type>,
}

impl ClassView {
    pub fn new(class: &Class, classtype: &Type) -> Result<Self, Error> {
        if class.template.as_ref().map(|x| x.0 .1.len()).unwrap_or(0)
            != classtype.template.as_ref().map(|x| x.1.len()).unwrap_or(0)
        {
            return Err(invalid_type_template(
                class
                    .template
                    .as_ref()
                    .map(|x| &x.0 .0)
                    .unwrap_or_else(|| &class.name.0),
                classtype
                    .template
                    .as_ref()
                    .map(|x| &x.0)
                    .unwrap_or_else(|| &classtype.span),
            ));
        }
        let tmp_map = if let (Some(a), Some(b)) = (&class.template, &classtype.template) {
            TemplateFixer::new(
                a.0 .1
                    .iter()
                    .zip(b.1.iter())
                    .map(|(x, y)| (x.clone(), y.clone()))
                    .collect::<HashMap<_, _>>(),
            )
        } else {
            TemplateFixer::new(HashMap::new())
        };

        Ok(Self {
            ty: classtype.clone(),
            fields: class
                .fields
                .clone()
                .into_iter()
                .map(|x| tmp_map.field(x))
                .collect(),
            methods: class
                .methods
                .clone()
                .into_iter()
                .map(|x| tmp_map.method(x))
                .collect(),
            superclass: class.superclass.clone().map(|x| tmp_map.ty(x)),
            name: class.name.clone(),
        })
    }

    pub fn get_field_type(&self, field_name: &str, access_span: &Span) -> Result<Type, Error> {
        Ok(self
            .fields
            .iter()
            .find(|x| *x.name == field_name)
            .ok_or_else(|| {
                report_similar(
                    "field",
                    "fields",
                    access_span,
                    field_name,
                    &self
                        .fields
                        .iter()
                        .map(|x| x.name.1.clone())
                        .collect::<Vec<_>>(),
                    14,
                )
            })?
            .ty
            .clone())
    }

    pub fn method_view(
        &self,
        name: &SpannedObject<String>,
        template: &Option<SpannedVector<Type>>,
    ) -> Result<MethodView, Error> {
        if let Some(e) = self
            .methods
            .iter()
            .find(|x| x.name.1 == name.1)
            .map(|x| MethodView::new(x, &name.0, template))
        {
            e
        } else {
            Err(report_similar(
                "method",
                "methods",
                &name.0,
                name,
                &self
                    .methods
                    .iter()
                    .map(|x| x.name.1.clone())
                    .collect::<Vec<_>>(),
                13,
            ))
        }
    }

    pub fn size(&self, cl: &ClassLoader) -> Result<u32, Error> {
        if *self.name == "Val" {
            return Ok(1);
        }
        if *self.name == "Array" {
            return Ok({
                match self.ty.template.as_ref() {
                    Some(x) => {
                        let item_size = cl.view(&x.1[0])?.size(cl)?;
                        let number = x.1[1].name.1.parse::<u32>().unwrap();
                        item_size * number
                    }
                    None => 0,
                }
            });
        }
        Ok(self
            .fields
            .iter()
            .map(|x| ClassView::new(cl.get(&x.ty.name)?, &x.ty)?.size(cl))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .sum::<u32>())
    }
}

pub struct TemplateFixer {
    pub template: HashMap<String, Type>,
}

impl TemplateFixer {
    pub fn new(template: HashMap<String, Type>) -> Self {
        Self { template }
    }
    pub fn method(&self, m: Method) -> Method {
        Method {
            name: m.name.clone(),
            arguments: m
                .arguments
                .into_iter()
                .map(|x| (self.ty(x.0), x.1))
                .collect(),
            return_type: m.return_type.map(|x| self.ty(x)),
            annotations: m.annotations,
            template: m.template.clone(),
            code: match m.code {
                Either::Left(x) => Either::Left(SpannedVector(
                    x.0,
                    x.1.into_iter().map(|x| self.expr(x)).collect(),
                )),
                Either::Right(x) => Either::Right(x),
            },
        }
    }
    pub fn field(&self, f: Field) -> Field {
        Field {
            annotations: f.annotations,
            name: f.name,
            ty: self.ty(f.ty),
        }
    }
    pub fn ty(&self, ty: Type) -> Type {
        if let Some(e) = self.template.get(&ty.name.1) {
            self.ty(e.clone())
        } else {
            Type {
                span: ty.span,
                name: ty.name,
                template: ty
                    .template
                    .map(|x| SpannedVector(x.0, x.1.into_iter().map(|x| self.ty(x)).collect())),
            }
        }
    }
    pub fn expr(&self, expr: Expr) -> Expr {
        match expr {
            Expr::New {
                span,
                class,
                fields,
            } => Expr::New {
                span,
                class: self.ty(class),
                fields: SpannedVector(
                    fields.0,
                    fields
                        .1
                        .into_iter()
                        .map(|x| (x.0, self.expr(x.1)))
                        .collect(),
                ),
            },
            Expr::If {
                span,
                condition,
                then,
                or_else,
            } => Expr::If {
                span,
                condition: Box::new( self.expr(*condition)),
                then: SpannedVector(then.0, then.1.into_iter().map(|x| self.expr(x)).collect()),
                or_else: or_else
                    .map(|x| SpannedVector(x.0, x.1.into_iter().map(|x| self.expr(x)).collect())),
            },
            Expr::Number(span, a, t) => Expr::Number(span, a, t),
            Expr::Variable(span, a) => Expr::Variable(span, a),
            Expr::Type(span, a) => Expr::Type(span, self.ty(a)),
            Expr::Field { span, source, name } => Expr::Field {
                span,
                source: Box::new( self.expr(*source)),
                name,
            },
            Expr::Method {
                span,
                source,
                name,
                arguments,
                template,
            } => Expr::Method {
                span,
                source: Box::new( self.expr(*source)),
                name,
                arguments: SpannedVector(
                    arguments.0,
                    arguments.1.into_iter().map(|x| self.expr(x)).collect(),
                ),
                template: template
                    .map(|x| SpannedVector(x.0, x.1.into_iter().map(|x| self.ty(x)).collect())),
            },
            Expr::Block(span, a) => Expr::Block(
                span,
                SpannedVector(a.0, a.1.into_iter().map(|x| self.expr(x)).collect()),
            ),
            Expr::Return(span, a) => Expr::Return(span, a.map(|x| Box::new( self.expr(*x)))),
            Expr::NamedResource { span, vtype, name } => Expr::NamedResource {
                span,
                vtype: self.ty(vtype),
                name,
            },
            Expr::Assignement { span, target, to } => Expr::Assignement {
                span,
                target: Box::new( self.expr(*target)),
                to: Box::new( self.expr(*to)),
            },
            Expr::Cast {
                span,
                source,
                target,
            } => Expr::Cast {
                span,
                source: Box::new(self.expr(*source)),
                target: self.ty(target),
            },
            Expr::Loop(span, a) => Expr::Loop(
                span,
                SpannedVector(a.0, a.1.into_iter().map(|x| self.expr(x)).collect()),
            ),
            Expr::Break(span) => Expr::Break(span),
            Expr::Continue(span) => Expr::Continue(span),
            Expr::BooleanExpression(span, a, b, c) => {
                Expr::BooleanExpression(span, Box::new(self.expr(*a)), b, Box::new(self.expr(*c)))
            }
            Expr::ArrayDefinition(a, b) => Expr::ArrayDefinition(
                a,
                SpannedVector(b.0, b.1.into_iter().map(|x| self.expr(x)).collect()),
            ),
        }
    }
}

impl TokenParser<Class> for VecDeque<Token> {
    fn parse(mut self, native: &Type) -> Result<Class, Error> {
        let annotations = self.extract(native)?;
        if !matches!(self.get_token(), Some(Token::Keyword(_, Keyword::Class))) {
            panic!("Expected keyword class");
        }
        let name = if let Some(Token::TypeName(span, name)) = self.get_token() {
            SpannedObject(span, name)
        } else {
            panic!("Expected class name");
        };
        let template: Option<TemplateDefinition> = match self.get_token() {
            Some(Token::Block(_, ClosableType::Type, inside)) => Some(inside.parse(native)?),
            Some(e) => {
                self.push_front(e);
                None
            }
            None => None,
        };
        let types = Type::new(
            &name.1,
            template.as_ref().map(|x| {
                SpannedVector(
                    x.0 .0.clone(),
                    x.0.iter()
                        .map(|y| Type::simple(y, x.0 .0.clone()))
                        .collect(),
                )
            }),
            name.0.clone(),
        );
        let superclass = if matches!(self.front(), Some(Token::Keyword(_, Keyword::Extends))) {
            self.get_token();
            Some(self.extract(&types)?)
        } else {
            None
        };
        let mut class = Class {
            name,
            fields: Vec::new(),
            annotations,
            methods: Vec::new(),
            superclass,
            template,
        };
        if let Some(Token::Block(_, ClosableType::Brace, inside)) = self.get_token() {
            split_complex(inside, |t| {
                if matches!(t, &Token::SemiColon(_)) {
                    SplitAction::SplitConsume
                } else if matches!(t, Token::Block(_, ClosableType::Brace, _)) {
                    SplitAction::Split
                } else {
                    SplitAction::None
                }
            })
            .into_iter()
            .map(|mut x| {
                if matches!(x.back(), Some(Token::Block(_, ClosableType::Brace, _))) {
                    class.methods.push(x.parse(&types)?);
                } else {
                    let annotations: Vec<Annotation> = self.extract(&types)?;
                    let ty: Type = x.extract(&types)?;
                    let name = if let Some(Token::Literal(span, name)) = x.get_token() {
                        SpannedObject(span, name)
                    } else {
                        panic!("Expected field name");
                    };
                    class.fields.push(Field {
                        annotations,
                        name,
                        ty,
                    });
                }
                Ok(())
            })
            .collect::<Result<Vec<_>, _>>()?;
            Ok(class)
        } else {
            panic!("Expected braces after class name");
        }
    }
}
