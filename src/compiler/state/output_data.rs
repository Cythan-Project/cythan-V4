use crate::{compiler::mir::MirCodeBlock, errors::Span};

use super::typed_definition::TypedMemory;

pub struct OutputData {
    pub mir: MirCodeBlock,
    pub span: Span,
    pub return_value: Option<TypedMemory>,
}

impl OutputData {
    pub fn native(mir: MirCodeBlock, return_value: Option<TypedMemory>) -> Self {
        OutputData {
            mir,
            span: Span::default(),
            return_value,
        }
    }
    pub fn new(mir: MirCodeBlock, span: Span, return_value: Option<TypedMemory>) -> Self {
        OutputData {
            mir,
            span,
            return_value,
        }
    }
}
