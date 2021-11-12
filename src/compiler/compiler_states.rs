use std::collections::HashMap;

use crate::parser::{class::ClassView, ty::Type};

use super::{
    mir::{Mir, MirCodeBlock},
    ClassLoader,
};

pub struct OutputData {
    pub mir: MirCodeBlock,
    pub return_value: Option<TypedMemory>,
}

impl OutputData {
    pub fn new(mir: MirCodeBlock, return_value: Option<TypedMemory>) -> Self {
        OutputData { mir, return_value }
    }
}

/* pub struct MirCodeBlock {
    pub mir: Vec<Mir>,
} */

impl MirCodeBlock {
    pub fn from(mir: Vec<Mir>) -> Self {
        Self(mir)
    }
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn add(&mut self, mut mir: MirCodeBlock) -> &mut Self {
        self.0.append(&mut mir.0);
        self
    }

    pub fn add_mir(&mut self, mir: Mir) -> &mut Self {
        self.0.push(mir);
        self
    }

    pub fn copy(&mut self, to: u32, from: u32) -> &mut Self {
        self.0.push(Mir::Copy(to, from));
        self
    }

    pub fn copy_bulk(&mut self, to: &[u32], from: &[u32]) -> &mut Self {
        if to.len() != from.len() {
            panic!("Invalid copy operation");
        }
        to.iter().zip(from.iter()).for_each(|(to, from)| {
            self.0.push(Mir::Copy(*to, *from));
        });
        self
    }

    pub fn set_bulk(&mut self, to: &[u32], from: &[u8]) -> &mut Self {
        if to.len() != from.len() {
            panic!("Invalid set operation");
        }
        to.iter().zip(from.iter()).for_each(|(to, from)| {
            self.0.push(Mir::Set(*to, *from));
        });
        self
    }

    pub fn set(&mut self, to: u32, value: u8) -> &mut Self {
        self.0.push(Mir::Set(to, value));
        self
    }
}

pub struct LocalState {
    pub vars: HashMap<String, TypedMemory>,
    pub return_loc: Option<TypedMemory>,
}

impl LocalState {
    pub fn new() -> LocalState {
        LocalState {
            vars: HashMap::new(),
            return_loc: None,
        }
    }

    pub fn shadow(&mut self) -> LocalState {
        Self {
            vars: self.vars.clone(),
            return_loc: self.return_loc.clone(),
        }
    }

    pub fn get_var(&mut self, name: &str) -> Option<&TypedMemory> {
        self.vars.get(name)
    }

    pub fn set_var(&mut self, name: &str, value: TypedMemory, code: &mut MirCodeBlock) {
        if let Some(e) = self.vars.get(name) {
            code.copy_bulk(&e.locations, &value.locations);
        }
    }

    pub fn new_var(
        &mut self,
        cm: &mut CodeManager,
        name: &str,
        ty: Type,
        code: &mut MirCodeBlock,
    ) -> TypedMemory {
        let data = cm.alloc_type(&ty).expect("Can't alloc");
        let tm = TypedMemory::new(ty, data);
        code.set_bulk(
            &tm.locations,
            &tm.locations.iter().map(|_| 0).collect::<Vec<_>>(),
        );
        self.vars.insert(name.to_string(), tm.clone());
        tm
    }
}

#[derive(Debug, Clone)]
pub struct TypedMemory {
    pub locations: Vec<u32>,
    pub ty: Type,
}

impl TypedMemory {
    pub fn new(ty: Type, locations: Vec<u32>) -> TypedMemory {
        TypedMemory { locations, ty }
    }
}

pub struct CodeManager {
    pub cl: ClassLoader,
    calloc: u32,
}

impl CodeManager {
    pub fn new(cl: ClassLoader) -> CodeManager {
        CodeManager { cl, calloc: 0 }
    }

    pub fn alloc(&mut self) -> u32 {
        self.calloc += 1;
        self.calloc
    }
    pub fn alloc_block(&mut self, size: usize) -> Vec<u32> {
        (0..size).map(|_| self.alloc()).collect()
    }
    pub fn alloc_type(&mut self, ty: &Type) -> Option<Vec<u32>> {
        Some(self.alloc_block(self.cl.view(ty).size(&self.cl) as usize))
    }

    pub fn location_and_type_of_field(
        &self,
        locations: &[u32],
        view: ClassView,
        name: &str,
    ) -> (Type, Vec<u32>) {
        let mut offset = 0;
        for f in &view.fields {
            if f.name == name {
                return (
                    f.ty.clone(),
                    locations
                        .iter()
                        .skip(offset)
                        .take(self.cl.view(&f.ty).size(&self.cl) as usize)
                        .copied()
                        .collect::<Vec<u32>>(),
                );
            }
            offset += self.cl.view(&f.ty).size(&self.cl) as usize;
        }
        panic!("Field {} not found", name);
    }
}
