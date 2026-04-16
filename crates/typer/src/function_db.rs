//! Global function registry.
//!
//! Phase 3. All methods (from extensions and impls) get a single-source-of-
//! truth entry keyed by `FnSig { type_name, method_name }`. Methods split
//! into `Fn::Simple` (fully concrete, flat signature computed) and
//! `Fn::Templated` (has unresolved template params — body + templates stored
//! for later monomorphization).

use std::collections::HashMap;

use new_parser::ast;

use crate::flat_sig::FlatSig;
use crate::types::*;
use crate::TypeRegistry;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FunctionDB {
    pub functions: HashMap<FnSig, Fn>,
}

/// Key into `FunctionDB`. `type_name` is the owning type or `""` for free
/// functions (there are none in the current language, but the slot is kept
/// for future use).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct FnSig {
    pub type_name: String,
    pub method_name: String,
}

impl FnSig {
    pub fn new(type_name: impl Into<String>, method_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            method_name: method_name.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fn {
    Simple(SimpleFn),
    Templated(TemplatedFn),
}

/// A fully concrete function: enclosing type is non-generic AND the function
/// itself has no template parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct SimpleFn {
    pub body: ast::Function,
    pub sig: FlatSig,
    pub type_name: String,
    pub from_trait: Option<String>,
}

/// A function whose signature still has unresolved template params — either
/// type-level (enclosing generic type) or function-level (`fn f<N>(...)`),
/// or both. FlatSig is deferred to monomorphization.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplatedFn {
    pub body: ast::Function,
    pub type_name: String,
    /// Templates that must be filled in to monomorphize:
    /// first type-level templates, then function-level templates.
    pub templates: Vec<String>,
    pub from_trait: Option<String>,
}

impl FunctionDB {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, sig: FnSig, f: Fn) {
        self.functions.insert(sig, f);
    }

    pub fn get(&self, sig: &FnSig) -> Option<&Fn> {
        self.functions.get(sig)
    }

    /// Populate from a `TypeRegistry`. Walks every type's methods,
    /// categorizing each as `Simple` or `Templated` and computing the
    /// `FlatSig` for the simple ones.
    pub fn from_registry(reg: &TypeRegistry) -> Result<Self, Vec<TyperError>> {
        let mut db = Self::new();
        let mut errors: Vec<TyperError> = Vec::new();

        for info in reg.types.values() {
            let type_templates = info.templates.clone();
            for m in &info.methods {
                let method_templates: Vec<String> = m
                    .function
                    .sig
                    .templates
                    .iter()
                    .map(|t| t.0.clone())
                    .collect();

                let key = FnSig::new(info.name.clone(), m.function.sig.name.0.clone());

                let is_simple = type_templates.is_empty() && method_templates.is_empty();
                let f = if is_simple {
                    match FlatSig::flatten(&m.function.sig, &info.name, reg) {
                        Ok(flat) => Fn::Simple(SimpleFn {
                            body: m.function.clone(),
                            sig: flat,
                            type_name: info.name.clone(),
                            from_trait: m.from_trait.clone(),
                        }),
                        Err(e) => {
                            errors.push(e);
                            continue;
                        }
                    }
                } else {
                    let mut templates = type_templates.clone();
                    templates.extend(method_templates);
                    Fn::Templated(TemplatedFn {
                        body: m.function.clone(),
                        type_name: info.name.clone(),
                        templates,
                        from_trait: m.from_trait.clone(),
                    })
                };
                db.insert(key, f);
            }
        }

        if errors.is_empty() {
            Ok(db)
        } else {
            Err(errors)
        }
    }
}
