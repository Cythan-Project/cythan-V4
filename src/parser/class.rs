use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
};

use ariadne::Report;
use either::Either;

use crate::{
    compiler::ClassLoader,
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
    pub name: String,
    pub annotations: Vec<Annotation>,
    pub template: Option<TemplateDefinition>,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    pub superclass: Option<Type>,
}

pub struct ClassView {
    pub ty: Type,
    pub name: String,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
    pub superclass: Option<Type>,
}

impl ClassView {
    pub fn new(class: &Class, classtype: &Type) -> Self {
        if class.template.as_ref().map(|x| x.0.len()).unwrap_or(0)
            != classtype.template.as_ref().map(|x| x.len()).unwrap_or(0)
        {
            panic!("Invalid type template for class {}", class.name);
        }
        let tmp_map = if let (Some(a), Some(b)) = (&class.template, &classtype.template) {
            TemplateFixer::new(
                a.0.iter()
                    .zip(b.iter())
                    .map(|(x, y)| (x.clone(), y.clone()))
                    .collect::<HashMap<_, _>>(),
            )
        } else {
            TemplateFixer::new(HashMap::new())
        };

        Self {
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
        }
    }

    pub fn method_view(&self, name: &str, template: &Option<Vec<Type>>) -> MethodView {
        if let Some(e) = self
            .methods
            .iter()
            .find(|x| x.name == name)
            .map(|x| MethodView::new(x, &template))
        {
            e
        } else {
            panic!("Method {} not found in class {}", name, self.name);
        }
    }

    pub fn size(&self, cl: &ClassLoader) -> u32 {
        if self.name == "Val" {
            return 1;
        }
        if self.name == "Array" {
            return self
                .ty
                .template
                .as_ref()
                .map(|x| {
                    let item_size = cl.view(&x[0]).size(cl);
                    let number = x[1].name[1..].parse::<u32>().unwrap();
                    item_size * number
                })
                .unwrap_or(0) as u32;
        }
        self.fields
            .iter()
            .map(|x| {
                if let Some(e) = cl.get(x.ty.name.as_str()) {
                    ClassView::new(e, &x.ty).size(cl)
                } else {
                    panic!("Unknown type {}", x.ty.name);
                }
            })
            .sum::<u32>()
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
                Either::Left(x) => Either::Left(x.into_iter().map(|x| self.expr(x)).collect()),
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
        if let Some(e) = self.template.get(&ty.name) {
            self.ty(e.clone())
        } else {
            Type {
                name: ty.name,
                template: ty
                    .template
                    .map(|x| x.into_iter().map(|x| self.ty(x)).collect()),
            }
        }
    }
    pub fn expr(&self, expr: Expr) -> Expr {
        match expr {
            Expr::New { class, fields } => Expr::New {
                class: self.ty(class),
                fields: fields.into_iter().map(|x| (x.0, self.expr(x.1))).collect(),
            },
            Expr::If {
                condition,
                then,
                or_else,
            } => Expr::If {
                condition: box self.expr(*condition),
                then: then.into_iter().map(|x| self.expr(x)).collect(),
                or_else: or_else.map(|x| x.into_iter().map(|x| self.expr(x)).collect()),
            },
            Expr::Number(a) => Expr::Number(a),
            Expr::Variable(a) => Expr::Variable(a),
            Expr::Type(a) => Expr::Type(self.ty(a)),
            Expr::Field { source, name } => Expr::Field {
                source: box self.expr(*source),
                name,
            },
            Expr::Method {
                source,
                name,
                arguments,
                template,
            } => Expr::Method {
                source: box self.expr(*source),
                name,
                arguments: arguments.into_iter().map(|x| self.expr(x)).collect(),
                template: template.map(|x| x.into_iter().map(|x| self.ty(x)).collect()),
            },
            Expr::Block(a) => Expr::Block(a.into_iter().map(|x| self.expr(x)).collect()),
            Expr::Return(a) => Expr::Return(a.map(|x| box self.expr(*x))),
            Expr::NamedResource { vtype, name } => Expr::NamedResource {
                vtype: self.ty(vtype),
                name,
            },
            Expr::Assignement { target, to } => Expr::Assignement {
                target: box self.expr(*target),
                to: box self.expr(*to),
            },
            Expr::Cast { source, target } => Expr::Cast {
                source: box self.expr(*source),
                target: self.ty(target),
            },
            Expr::Loop(a) => Expr::Loop(a.into_iter().map(|x| self.expr(x)).collect()),
            Expr::Break => Expr::Break,
            Expr::Continue => Expr::Continue,
            Expr::BooleanExpression(a, b, c) => {
                Expr::BooleanExpression(box self.expr(*a), b, box self.expr(*c))
            }
        }
    }
}

impl TokenParser<Class> for VecDeque<Token> {
    fn parse(mut self) -> Result<Class, Report<(String, Range<usize>)>> {
        let annotations = self.extract()?;
        if !matches!(self.get_token(), Some(Token::Keyword(_, Keyword::Class))) {
            panic!("Expected keyword class");
        }
        let name = if let Some(Token::TypeName(_, name)) = self.get_token() {
            name
        } else {
            panic!("Expected class name");
        };
        let template = match self.get_token() {
            Some(Token::Block(_, ClosableType::Type, inside)) => Some(inside.parse()?),
            Some(e) => {
                self.push_front(e);
                None
            }
            None => None,
        };
        let superclass = if matches!(self.front(), Some(Token::Keyword(_, Keyword::Extends))) {
            self.get_token();
            Some(self.extract()?)
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
        if let Some(Token::Block(_, ClosableType::Bracket, inside)) = self.get_token() {
            split_complex(inside, |t| {
                if matches!(t, &Token::SemiColon(_)) {
                    SplitAction::SplitConsume
                } else if matches!(t, Token::Block(_, ClosableType::Bracket, _)) {
                    SplitAction::Split
                } else {
                    SplitAction::None
                }
            })
            .into_iter()
            .map(|mut x| {
                if matches!(x.back(), Some(Token::Block(_, ClosableType::Bracket, _))) {
                    class.methods.push(x.parse()?);
                } else {
                    let annotations: Vec<Annotation> = self.extract()?;
                    let ty: Type = x.extract()?;
                    let name = if let Some(Token::Literal(_, name)) = x.get_token() {
                        name
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
