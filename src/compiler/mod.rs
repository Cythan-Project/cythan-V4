use std::{collections::VecDeque, ops::Range};

use ariadne::{Report, Source};

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

    pub fn load_string(&mut self, class: &str, filename: &str) {
        if let Err(e) = try {
            let mut vdc = VecDeque::new();
            parse(
                &mut vdc,
                &mut class.chars().collect(),
                class.chars().count(),
                filename,
            )?;
            self.load(vdc.parse()?);
        } {
            let e: Report<(String, Range<usize>)> = e;
            e.eprint((filename.to_owned(), Source::from(class)))
                .unwrap();
        };
    }

    pub fn load(&mut self, class: Class) {
        self.classes.push(class);
    }

    pub fn get(&self, name: &str) -> Option<&Class> {
        self.classes.iter().find(|c| c.name == name)
    }

    pub fn view(&self, ty: &Type) -> ClassView {
        ClassView::new(
            if let Some(e) = self.get(&ty.name) {
                e
            } else {
                panic!("Class not found: {}", ty.name)
            },
            ty,
        )
    }

    pub fn inject_method(&mut self, arg: &str, method: Method) {
        self.classes
            .iter_mut()
            .find(|c| c.name == arg)
            .unwrap()
            .methods
            .push(method);
    }
}
