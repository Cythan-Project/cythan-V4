use errors::{report_similar, Error, SpannedObject};

use crate::{
    compiler::class_loader::ClassLoader,
    parser::{class::ClassView, ty::Type},
};

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
    pub fn alloc_type(&mut self, ty: &Type) -> Result<Vec<u32>, Error> {
        Ok(self.alloc_block(self.cl.view(ty)?.size(&self.cl)? as usize))
    }

    pub fn location_and_type_of_field(
        &self,
        locations: &[u32],
        view: ClassView,
        name: &SpannedObject<String>,
    ) -> Result<(Type, Vec<u32>), Error> {
        let mut offset = 0;
        for f in &view.fields {
            if f.name.1 == name.1 {
                return Ok((
                    f.ty.clone(),
                    locations
                        .iter()
                        .skip(offset)
                        .take(self.cl.view(&f.ty)?.size(&self.cl)? as usize)
                        .copied()
                        .collect::<Vec<u32>>(),
                ));
            }
            offset += self.cl.view(&f.ty)?.size(&self.cl)? as usize;
        }
        return Err(report_similar(
            "field",
            "fields",
            &name.0,
            &name.1,
            &view
                .fields
                .iter()
                .map(|x| x.name.1.clone())
                .collect::<Vec<_>>(),
            14,
        ));
    }
}
