use std::{collections::HashMap, ops::Range};

use ariadne::Report;
use mir::MirCodeBlock;

use crate::{
    errors::{report_similar, Span},
    parser::{expression::SpannedObject, ty::Type},
    Error,
};

use super::{code_manager::CodeManager, typed_definition::TypedMemory};

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

    pub fn shadow_method(&mut self, return_loc: Option<TypedMemory>) -> LocalState {
        Self {
            vars: HashMap::new(),
            return_loc,
        }
    }

    pub fn get_var_native(
        &self,
        name: &str,
    ) -> Result<&TypedMemory, Report<(String, Range<usize>)>> {
        self.get_var(&SpannedObject(Span::default(), name.to_owned()))
    }

    pub fn get_var(
        &self,
        name: &SpannedObject<String>,
    ) -> Result<&TypedMemory, Report<(String, Range<usize>)>> {
        if let Some(e) = self.vars.get(&name.1) {
            Ok(e)
        } else {
            Err(report_similar(
                "variable",
                "variables",
                &name.0,
                &name.1,
                &self.vars.keys().cloned().collect::<Vec<_>>(),
                12,
            ))
        }
    }

    #[allow(dead_code)]
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
        span: Span,
    ) -> Result<TypedMemory, Error> {
        let data = cm.alloc_type(&ty)?;
        let tm = TypedMemory::new(ty, data, span);
        code.set_bulk(
            &tm.locations,
            &tm.locations.iter().map(|_| 0).collect::<Vec<_>>(),
        );
        self.vars.insert(name.to_string(), tm.clone());
        Ok(tm)
    }
}
