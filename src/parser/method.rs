use std::{
    collections::{HashMap, VecDeque},
    fmt::Debug,
    rc::Rc,
};

use either::Either;
use errors::{
    invalid_argument_type, invalid_type_template, Error, Span, SpannedObject, SpannedVector,
};
use mir::{need_block, remove_skips, Mir, MirCodeBlock};

use crate::{
    compiler::{
        compiler::compile_code_block,
        state::{
            code_manager::CodeManager, local_state::LocalState, output_data::OutputData,
            typed_definition::TypedMemory,
        },
    },
    parser::{
        expression::TokenProcessor,
        token_utils::{split_complex, SplitAction},
        ty::Type,
        ClosableType, Token, TokenExtracter, TokenParser,
    },
};

use super::{annotation::Annotation, class::TemplateFixer, ty::TemplateDefinition};
use crate::parser::expression::CodeBlock;

#[derive(Clone)]
pub struct Method {
    pub name: SpannedObject<String>,
    pub annotations: Vec<Annotation>,
    pub return_type: Option<Type>,
    pub arguments: Vec<(Type, String)>,
    pub template: Option<TemplateDefinition>,
    pub code: Either<CodeBlock, NativeMethod>,
}

impl Debug for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut k = f.debug_struct("Method");
        let k = k
            .field("name", &self.name)
            .field("annotations", &self.annotations)
            .field("return_type", &self.return_type)
            .field("arguments", &self.arguments)
            .field("template", &self.template);
        if let Either::Left(e) = &self.code {
            k.field("code", &e).finish()
        } else {
            k.finish()
        }
    }
}

pub struct MethodView {
    pub name: SpannedObject<String>,
    pub return_type: Option<Type>,
    pub arguments: Vec<(Type, String)>,
    pub template: Option<SpannedVector<Type>>,
    pub code: Either<CodeBlock, NativeMethod>,
}

pub type NativeMethod =
    Rc<Box<dyn Fn(&mut LocalState, &mut CodeManager, &MethodView) -> Result<OutputData, Error>>>;

impl MethodView {
    pub fn new(
        method: &Method,
        namerefspan: &Span,
        template: &Option<SpannedVector<Type>>,
    ) -> Result<Self, Error> {
        if method.template.as_ref().map(|x| x.0 .1.len()).unwrap_or(0)
            != template.as_ref().map(|x| x.1.len()).unwrap_or(0)
        {
            return Err(invalid_type_template(
                method
                    .template
                    .as_ref()
                    .map(|x| &x.0 .0)
                    .unwrap_or_else(|| &method.name.0),
                template.as_ref().map(|x| &x.0).unwrap_or(namerefspan),
            ));
        }
        let tmp_map = if let (Some(a), Some(b)) = (&method.template, template) {
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
            template: template.clone(),
            name: method.name.clone(),
            return_type: method.return_type.as_ref().map(|x| tmp_map.ty(x.clone())),
            arguments: method
                .arguments
                .iter()
                .map(|(x, y)| (tmp_map.ty(x.clone()), y.clone()))
                .collect(),
            code: match &method.code {
                Either::Left(a) => Either::Left(SpannedVector(
                    a.0.clone(),
                    a.1.iter().map(|x| tmp_map.expr(x.clone())).collect(),
                )),
                Either::Right(a) => Either::Right(a.clone()),
            },
        })
    }

    pub fn get_template(&self) -> Result<&SpannedVector<Type>, Error> {
        self.template
            .as_ref()
            .ok_or_else(|| invalid_type_template(&self.name.0, &self.name.0))
    }

    pub fn execute(
        &self,
        ls: &mut LocalState,
        cm: &mut CodeManager,
        arguments: Vec<TypedMemory>,
    ) -> Result<OutputData, Error> {
        let return_loc = match self.return_type.as_ref() {
            Some(x) => Some(TypedMemory::new(
                x.clone(),
                cm.alloc_type(x)?,
                self.name.0.clone(),
            )),
            None => None,
        };
        let mut ls = ls.shadow_method(return_loc.clone());
        for (x, y) in self.arguments.iter().zip(arguments.iter()) {
            if x.0 != y.ty {
                return Err(invalid_argument_type(
                    &y.span,
                    &format!("{:?}", x.0),
                    &format!("{:?}", y.ty),
                ));
            }
            ls.vars.insert(x.1.clone(), y.clone());
        }

        let (k, return_value) = match &self.code {
            Either::Left(a) => (compile_code_block(a, &mut ls, cm, a.0.clone())?, return_loc),
            Either::Right(a) => {
                let jk = a(&mut ls, cm, self)?;
                let lc = jk.return_value.clone();
                (jk, lc)
            }
        };
        let mir = if need_block(&k.mir.0) {
            vec![Mir::Block(k.mir)]
        } else {
            remove_skips(k.mir.0, false)
        };
        // Maybe later add tail auto return
        Ok(OutputData {
            return_value,
            span: self
                .return_type
                .as_ref()
                .map(|x| x.span.clone())
                .unwrap_or_else(|| self.name.0.clone()),
            mir: MirCodeBlock(mir),
        })
    }
}

impl TokenParser<Method> for VecDeque<Token> {
    fn parse(mut self, types: &Type) -> Result<Method, Error> {
        let annotations = self.extract(types)?;
        let tp = if matches!(self.front(), Some(Token::TypeName(_, _))) {
            Some(self.extract(types)?)
        } else {
            None
        };
        let name = if let Some(Token::Literal(span, name)) = self.get_token() {
            SpannedObject(span, name)
        } else {
            panic!("Expected method name");
        };
        let template = match self.get_token() {
            Some(Token::Block(_, ClosableType::Type, inside)) => Some(inside.parse(types)?),
            Some(e) => {
                self.push_front(e);
                None
            }
            None => None,
        };
        let arguments: Vec<(Type, String)> =
            if let Some(Token::Block(_, ClosableType::Parenthesis, inside)) = self.get_token() {
                split_complex(inside, |a| {
                    if matches!(a, Token::Comma(_)) {
                        SplitAction::SplitConsume
                    } else {
                        SplitAction::None
                    }
                })
                .into_iter()
                .map(|mut a| {
                    let ty = if matches!(a.front(), Some(&Token::Literal(..))) {
                        types.clone()
                    } else {
                        a.extract(types)?
                    };
                    let name = if let Some(Token::Literal(_, name)) = a.get_token() {
                        name
                    } else {
                        panic!("Expected argument name");
                    };
                    // TODO panic if more items are left in the vecdequeue
                    Ok((ty, name))
                })
                .collect::<Result<_, _>>()?
            } else {
                panic!("Expected brackets after method name");
            };
        let code = if let Some(Token::Block(span, ClosableType::Brace, inside)) = self.get_token() {
            Either::Left(SpannedVector(span, inside.parse(types)?))
        } else {
            panic!("Expected braces after method arguments");
        };
        Ok(Method {
            name,
            return_type: tp,
            arguments,
            template,
            code,
            annotations,
        })
    }
}
