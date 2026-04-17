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
/// for future use). `trait_name` is `None` for inherent methods (from
/// `extension`) and `Some(trait)` for methods attached via `impl Trait for`.
/// Two traits that both define `eq` on the same type produce distinct
/// entries because their `trait_name` differs.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct FnSig {
    pub type_name: String,
    pub method_name: String,
    pub trait_name: Option<String>,
}

impl FnSig {
    /// Construct a key for an inherent method (no trait).
    pub fn new(type_name: impl Into<String>, method_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            method_name: method_name.into(),
            trait_name: None,
        }
    }

    /// Construct a key for a method attached via `impl Trait for Type`.
    pub fn new_trait(
        type_name: impl Into<String>,
        method_name: impl Into<String>,
        trait_name: impl Into<String>,
    ) -> Self {
        Self {
            type_name: type_name.into(),
            method_name: method_name.into(),
            trait_name: Some(trait_name.into()),
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
    /// File in which this method was declared. Used by the HIR generator to
    /// look up which traits are in scope when resolving calls from its body.
    pub file_id: crate::types::FileId,
    /// When this `SimpleFn` is the output of monomorphizing a method on a
    /// generic type (e.g. `Pair<U4>::new` from `Pair<T>::new`), this holds
    /// the concrete type-level template args. Empty when the enclosing
    /// type is non-generic or when this is a direct non-monomorphized
    /// Simple. HIR gen reads this to fill in `Pair::new(...)` calls that
    /// implicitly mean `Self::new(...)`.
    pub type_template_args: Vec<ast::TypeOrValue>,
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
    pub file_id: crate::types::FileId,
    /// Blanket-impl attachment info, if this method came from a
    /// blanket. The monomorphizer uses it to resolve `Self` directly
    /// from the binding's `Target`/`TargetArg` sources rather than
    /// falling back to "first N template_args = Self's args" — which
    /// breaks down when the blanket's generic list doesn't align
    /// with the candidate's own template params.
    pub blanket: Option<crate::types::BlanketBinding>,
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

        // Iterate with the storage key so that — after cross-file
        // collision migration — FnSig keys align with the storage-key
        // form that type-name lookups resolve to. For unambiguous
        // types storage_key == info.name so this is a no-op.
        for (storage_key, info) in reg.iter_types() {
            let type_name = storage_key.to_string();
            let type_templates = info.templates.clone();
            for m in &info.methods {
                let method_templates: Vec<String> = m
                    .function
                    .sig
                    .templates
                    .iter()
                    .map(|t| t.0.clone())
                    .collect();

                // Resolve the `TraitId` back to its canonical name for
                // the string-keyed FnSig / SimpleFn payload. HIR gen
                // consumes these as strings.
                let from_trait_name: Option<String> = m
                    .from_trait
                    .map(|id| reg.trait_canonical_keys[id.0 as usize].clone());
                let key = match &from_trait_name {
                    None => FnSig::new(type_name.clone(), m.function.sig.name.0.clone()),
                    Some(t) => FnSig::new_trait(
                        type_name.clone(),
                        m.function.sig.name.0.clone(),
                        t.clone(),
                    ),
                };

                // Blanket-origin methods always need monomorphization —
                // the body is stored once on the blanket and reused across
                // every satisfying target type, with generic params
                // substituted at inline time. Register them as Templated
                // with the blanket's full ordered generic name list as
                // the synthetic leading templates.
                let is_blanket = m.blanket.is_some();
                let is_simple = !is_blanket
                    && type_templates.is_empty()
                    && method_templates.is_empty();
                let f = if is_simple {
                    match FlatSig::flatten(&m.function.sig, &type_name, reg) {
                        Ok(flat) => Fn::Simple(SimpleFn {
                            body: m.function.clone(),
                            sig: flat,
                            type_name: type_name.clone(),
                            from_trait: from_trait_name.clone(),
                            file_id: m.file_id,
                            type_template_args: Vec::new(),
                        }),
                        Err(e) => {
                            errors.push(e);
                            continue;
                        }
                    }
                } else {
                    let mut templates: Vec<String> = Vec::new();
                    if let Some(b) = &m.blanket {
                        // Blanket methods use the blanket's own generic
                        // list in place of the type's template params —
                        // for `impl<T> Trait for Container<T>`, the
                        // blanket's `T` already covers Container's single
                        // template slot. Including both would double-count.
                        templates.extend(b.generic_names.iter().cloned());
                    } else {
                        templates.extend(type_templates.clone());
                    }
                    templates.extend(method_templates);
                    Fn::Templated(TemplatedFn {
                        body: m.function.clone(),
                        type_name: type_name.clone(),
                        templates,
                        from_trait: from_trait_name.clone(),
                        file_id: m.file_id,
                        blanket: m.blanket.clone(),
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
