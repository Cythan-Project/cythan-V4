use std::{
    collections::{HashMap, VecDeque},
    fmt::Debug,
    ops::Range,
    rc::Rc,
};

use ariadne::Report;
use either::Either;

use crate::{
    compiler::{
        compiler::compile_code_block,
        compiler_states::{CodeManager, LocalState, OutputData, TypedMemory},
        mir::{Mir, MirCodeBlock},
    },
    mir_utils::block_inliner::{need_block, remove_skips},
    parser::{
        expression::TokenProcessor,
        token_utils::{split_complex, SplitAction},
        ty::Type,
        ClosableType, Token, TokenExtracter, TokenParser,
    },
};

use super::{
    annotation::Annotation, class::TemplateFixer, expression::SpannedVector, ty::TemplateDefinition,
};
use crate::parser::expression::CodeBlock;

#[derive(Clone)]
pub struct Method {
    pub name: String,
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
    pub name: String,
    pub return_type: Option<Type>,
    pub arguments: Vec<(Type, String)>,
    pub template: Option<SpannedVector<Type>>,
    pub code: Either<CodeBlock, NativeMethod>,
}

pub type NativeMethod =
    Rc<Box<dyn Fn(&mut LocalState, &mut CodeManager, &MethodView) -> OutputData>>;

impl MethodView {
    pub fn new(method: &Method, template: &Option<SpannedVector<Type>>) -> Self {
        if method.template.as_ref().map(|x| x.0.len()).unwrap_or(0)
            != template.as_ref().map(|x| x.1.len()).unwrap_or(0)
        {
            panic!("Invalid type template for method {}", method.name);
        }
        let tmp_map = if let (Some(a), Some(b)) = (&method.template, template) {
            TemplateFixer::new(
                a.0.iter()
                    .zip(b.1.iter())
                    .map(|(x, y)| (x.clone(), y.clone()))
                    .collect::<HashMap<_, _>>(),
            )
        } else {
            TemplateFixer::new(HashMap::new())
        };

        Self {
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
        }
    }

    pub fn execute(
        &self,
        ls: &mut LocalState,
        cm: &mut CodeManager,
        arguments: Vec<TypedMemory>,
    ) -> Result<OutputData, Report<(String, Range<usize>)>> {
        let return_loc = self
            .return_type
            .as_ref()
            .map(|x| TypedMemory::new(x.clone(), cm.alloc_type(x).unwrap()));
        let mut ls = ls.shadow();
        ls.return_loc = return_loc.clone();
        self.arguments
            .iter()
            .zip(arguments.iter())
            .for_each(|(x, y)| {
                if x.0 != y.ty {
                    panic!(
                        "Invalid argument type for method {}: expected {:?}, got {:?}",
                        self.name, x.0, y.ty
                    );
                }
                ls.vars.insert(x.1.clone(), y.clone());
            });

        let (k, return_value) = match &self.code {
            Either::Left(a) => (compile_code_block(a, &mut ls, cm)?, return_loc),
            Either::Right(a) => {
                let jk = a(&mut ls, cm, self);
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
            mir: MirCodeBlock(mir),
        })
    }
}

impl TokenParser<Method> for VecDeque<Token> {
    fn parse(mut self) -> Result<Method, Report<(String, Range<usize>)>> {
        let annotations = self.extract()?;
        let tp = if matches!(self.front(), Some(Token::TypeName(_, _))) {
            Some(self.extract()?)
        } else {
            None
        };
        let name = if let Some(Token::Literal(_, name)) = self.get_token() {
            name
        } else {
            panic!("Expected method name");
        };
        let template = match self.get_token() {
            Some(Token::Block(_, ClosableType::Type, inside)) => Some(inside.parse()?),
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
                    let ty = a.extract()?;
                    let name = if let Some(Token::Literal(_, name)) = a.get_token() {
                        name
                    } else {
                        panic!("Expected argument name");
                    };
                    Ok((ty, name))
                })
                .collect::<Result<_, _>>()?
            } else {
                panic!("Expected brackets after method name");
            };
        let code = if let Some(Token::Block(span, ClosableType::Bracket, inside)) = self.get_token()
        {
            Either::Left(SpannedVector(span, inside.parse()?))
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
