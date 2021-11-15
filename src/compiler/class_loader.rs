use std::{collections::VecDeque, ops::Range};

use ariadne::{Color, ColorGenerator, Fmt, Label, Report, ReportKind};

use crate::{
    errors::report_similar,
    parser::{
        class::{Class, ClassView},
        expression::SpannedObject,
        method::Method,
        parse,
        ty::Type,
        TokenParser,
    },
    Error,
};

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

    pub fn load_string(
        &mut self,
        class: &str,
        filename: &str,
    ) -> Result<(), Report<(String, Range<usize>)>> {
        let mut vdc = VecDeque::new();
        let mut k: VecDeque<char> = class.chars().filter(|x| *x != '\r').collect();
        let kl = k.len();
        parse(&mut vdc, &mut k, kl, filename)?;
        /* for i in &vdc {
            display(i, class);
        } */
        self.load(vdc.parse()?);
        Ok(())
    }

    pub fn load(&mut self, class: Class) {
        self.classes.push(class);
    }

    pub fn get(&self, name: &SpannedObject<String>) -> Result<&Class, Error> {
        self.classes
            .iter()
            .find(|c| c.name == name.1)
            .ok_or_else(|| {
                report_similar(
                    "class",
                    "classes",
                    &name.0,
                    &name.1,
                    &self
                        .classes
                        .iter()
                        .map(|c| c.name.clone())
                        .collect::<Vec<_>>(),
                    11,
                )
            })
    }

    pub fn view(&self, ty: &Type) -> Result<ClassView, Error> {
        Ok(ClassView::new(self.get(&ty.name)?, ty))
    }

    #[allow(dead_code)]
    pub fn inject_method(&mut self, arg: &str, method: Method) {
        self.classes
            .iter_mut()
            .find(|c| c.name == arg)
            .unwrap()
            .methods
            .push(method);
    }

    pub fn get_class_mut(&mut self, arg: &str) -> &mut Class {
        self.classes.iter_mut().find(|x| x.name == arg).unwrap()
    }
}
