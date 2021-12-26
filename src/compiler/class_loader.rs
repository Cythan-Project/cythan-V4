use std::{collections::VecDeque, rc::Rc};

use either::Either;
use errors::{report_similar, Error, SpannedObject};

use crate::parser::{
    class::{Class, ClassView},
    method::{Method, MethodView},
    parse,
    ty::Type,
    TokenParser,
};

use super::state::{code_manager::CodeManager, local_state::LocalState, output_data::OutputData};

#[derive(Debug)]
pub struct ClassLoader {
    classes: Vec<Class>,
}

impl ClassLoader {
    pub fn new() -> ClassLoader {
        ClassLoader {
            classes: Vec::new(),
        }
    }

    pub fn implement_native(
        &mut self,
        class_name: &str,
        name: &str,
        native: impl Fn(&mut LocalState, &mut CodeManager, &MethodView) -> Result<OutputData, Error>
            + 'static,
    ) {
        self.get_class_mut(class_name).get_method_mut(name).code =
            Either::Right(Rc::new(Box::new(native)));
    }

    pub fn load_string(&mut self, class: &str, filename: &str) -> Result<(), Error> {
        let mut vdc = VecDeque::new();
        let mut k: VecDeque<char> = class.chars().filter(|x| *x != '\r').collect();
        let kl = k.len();
        parse(&mut vdc, &mut k, kl, filename)?;
        /* for i in &vdc {
            display(i, class);
        } */
        self.load(vdc.parse(&Type::native_simple("Self provider"))?);
        Ok(())
    }

    pub fn load(&mut self, class: Class) {
        self.classes.push(class);
    }

    pub fn get(&self, name: &SpannedObject<String>) -> Result<&Class, Error> {
        self.classes
            .iter()
            .find(|c| c.name.1 == name.1)
            .ok_or_else(|| {
                report_similar(
                    "class",
                    "classes",
                    &name.0,
                    &name.1,
                    &self
                        .classes
                        .iter()
                        .map(|c| c.name.1.clone())
                        .collect::<Vec<_>>(),
                    11,
                )
            })
    }

    pub fn view(&self, ty: &Type) -> Result<ClassView, Error> {
        ClassView::new(self.get(&ty.name)?, ty)
    }

    #[allow(dead_code)]
    pub fn inject_method(&mut self, arg: &str, method: Method) {
        self.classes
            .iter_mut()
            .find(|c| c.name.1 == arg)
            .unwrap()
            .methods
            .push(method);
    }

    pub fn get_class_mut(&mut self, arg: &str) -> &mut Class {
        self.classes.iter_mut().find(|x| x.name.1 == arg).unwrap()
    }
}
