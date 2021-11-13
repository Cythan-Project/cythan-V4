use std::{collections::VecDeque, ops::Range};

use ariadne::Report;

use crate::parser::{
    class::{Class, ClassView},
    method::Method,
    parse,
    ty::Type,
    TokenParser,
};

pub mod asm;
pub mod asm_interpreter;
pub mod compiler;
pub mod compiler_states;
pub mod mir;
pub mod template;

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

    pub fn get(&self, name: &str) -> Option<&Class> {
        self.classes.iter().find(|c| c.name == name)
    }

    pub fn view(&self, ty: &Type) -> ClassView {
        ClassView::new(
            if let Some(e) = self.get(&ty.name.1) {
                e
            } else {
                panic!("Class not found: {}", ty.name.1)
            },
            ty,
        )
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
