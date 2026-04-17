//! `TypeRegistry` and the registration/validation passes.

use std::collections::{HashMap, HashSet};

use new_parser::ast::{self, Spanned};

use crate::types::*;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TypeRegistry {
    pub types: HashMap<String, TypeInfo>,
    pub traits: HashMap<String, TraitInfo>,
    pub impls: Vec<ImplInfo>,
    /// Per-file import scope: for each `FileId`, the set of trait/type names
    /// explicitly brought into scope via `use Name;` in that file. Operator
    /// sugar (`+`, `-`, `==`, ...) bypasses this check — see `OPERATOR_TRAITS`.
    pub imports: HashMap<FileId, HashSet<String>>,
}

/// Traits that operator sugar desugars into. Calls routed through these
/// traits don't require an explicit `use` in the calling file.
pub const OPERATOR_TRAITS: &[&str] = &[
    "Add", "Sub", "Eq", "Ord", "PartialEq", "PartialOrd", "AddAssign", "SubAssign",
];

impl TypeRegistry {
    /// Create an empty registry, pre-populated with the hardcoded primitive
    /// `U4` (size = 1 cell, no fields, no methods).
    pub fn new() -> Self {
        let mut r = Self::default();
        r.types.insert(
            U4_NAME.to_string(),
            TypeInfo {
                name: U4_NAME.to_string(),
                templates: Vec::new(),
                kind: TypeKind::Primitive { size: U4_SIZE },
                methods: Vec::new(),
            },
        );
        r
    }

    // --- public registration entry points ---------------------------------

    /// Build a registry from parsed items coming from a single file.
    ///
    /// Errors are collected; partial registries are returned on error so
    /// callers can still inspect what succeeded. (This matches the plan's
    /// "collect all errors" requirement for Step 2.6.)
    pub fn from_items(
        items: &[Spanned<ast::Item>],
    ) -> Result<Self, Vec<TyperError>> {
        Self::from_files(&[("main", items)])
    }

    /// Build a registry from multiple files. Each file gets a distinct
    /// `FileId` (its index). Order is preserved; types declared later in
    /// the input win on conflicts only if explicitly supported (they're
    /// currently rejected).
    pub fn from_files(
        files: &[(&str, &[Spanned<ast::Item>])],
    ) -> Result<Self, Vec<TyperError>> {
        let mut r = Self::new();
        let mut errors: Vec<TyperError> = Vec::new();

        // Pass 1: collect structs/enums/traits (populates `types` and
        // `traits` with their top-level declarations) and per-file imports.
        for (file_ix, (_file, items)) in files.iter().enumerate() {
            let file_id = file_ix as FileId;
            // Ensure the file has an imports entry even if no `use` statements.
            r.imports.entry(file_id).or_default();
            for (item, _sp) in *items {
                let out = match item {
                    ast::Item::Struct(s) => r.register_struct(s),
                    ast::Item::Enum(e) => r.register_enum(e),
                    ast::Item::Trait(t) => r.register_trait(t),
                    ast::Item::Use(u) => {
                        r.imports
                            .entry(file_id)
                            .or_default()
                            .insert(u.name.0.clone());
                        Ok(())
                    }
                    _ => Ok(()),
                };
                if let Err(e) = out {
                    errors.push(e);
                }
            }
        }

        // Pass 2: merge extensions into their target types.
        for (file_ix, (_file, items)) in files.iter().enumerate() {
            let file_id = file_ix as FileId;
            for (item, _sp) in *items {
                if let ast::Item::Extension(ext) = item {
                    if let Err(e) = r.merge_extension(ext, file_id) {
                        errors.push(e);
                    }
                }
            }
        }

        // Pass 3: validate impls, register them, and attach their methods
        // to the target type.
        for (file_ix, (_file, items)) in files.iter().enumerate() {
            let file_id = file_ix as FileId;
            for (item, _sp) in *items {
                if let ast::Item::Impl(i) = item {
                    if let Err(e) = r.register_impl(i, file_id) {
                        errors.push(e);
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(r)
        } else {
            Err(errors)
        }
    }

    // --- individual registration passes (exposed for testing) -------------

    pub fn register_struct(&mut self, def: &ast::StructDef) -> Result<(), TyperError> {
        let name = def.name.0.clone();
        if self.types.contains_key(&name) && name != U4_NAME {
            return Err(TyperError::at(
                format!("duplicate type definition: {}", name),
                def.name.1.clone(),
            ));
        }
        let templates: Vec<String> = def.templates.iter().map(|t| t.0.clone()).collect();

        // Primitive U4 is special: its in-source declaration is an empty
        // struct, but the registry treats it as a 1-cell primitive.
        // (It is pre-populated in `new()`; a later `struct U4 {}` in source
        // is accepted as redundant.)
        if name == U4_NAME {
            if !def.fields.is_empty() {
                return Err(TyperError::at(
                    "primitive U4 must be declared with no fields",
                    def.name.1.clone(),
                ));
            }
            return Ok(());
        }

        let kind = if templates.is_empty() {
            // Concrete struct: need to resolve every field's size.
            // For now, only U4 and other already-registered concrete types
            // are valid field types.
            let layout = self.compute_struct_layout(def)?;
            TypeKind::Struct(StructKind::Concrete(layout))
        } else {
            TypeKind::Struct(StructKind::Templated {
                fields: def
                    .fields
                    .iter()
                    .map(|f| (f.name.0.clone(), f.ty.0.clone()))
                    .collect(),
            })
        };

        self.types.insert(
            name.clone(),
            TypeInfo {
                name,
                templates,
                kind,
                methods: Vec::new(),
            },
        );
        Ok(())
    }

    pub fn register_enum(&mut self, def: &ast::EnumDef) -> Result<(), TyperError> {
        let name = def.name.0.clone();
        if self.types.contains_key(&name) {
            return Err(TyperError::at(
                format!("duplicate type definition: {}", name),
                def.name.1.clone(),
            ));
        }
        let templates: Vec<String> = def.templates.iter().map(|t| t.0.clone()).collect();

        let kind = if templates.is_empty()
            && def.variants.iter().all(|v| {
                v.data
                    .as_ref()
                    .map_or(true, |t| !self.type_is_templated(&t.0))
            })
        {
            let layout = self.compute_enum_layout(def)?;
            TypeKind::Enum(EnumKind::Concrete(layout))
        } else {
            TypeKind::Enum(EnumKind::Templated {
                variants: def
                    .variants
                    .iter()
                    .map(|v| TemplatedVariant {
                        name: v.name.0.clone(),
                        data: v.data.as_ref().map(|(t, _)| t.clone()),
                        discriminant: v.discriminant.as_ref().map(|(n, _)| *n),
                    })
                    .collect(),
            })
        };

        self.types.insert(
            name.clone(),
            TypeInfo {
                name,
                templates,
                kind,
                methods: Vec::new(),
            },
        );
        Ok(())
    }

    pub fn register_trait(&mut self, def: &ast::TraitDef) -> Result<(), TyperError> {
        let name = def.name.0.clone();
        if self.traits.contains_key(&name) {
            return Err(TyperError::at(
                format!("duplicate trait definition: {}", name),
                def.name.1.clone(),
            ));
        }
        self.traits.insert(
            name.clone(),
            TraitInfo {
                name,
                templates: def.templates.iter().map(|t| t.0.clone()).collect(),
                associated_types: def.associated_types.iter().map(|a| a.0.clone()).collect(),
                methods: def.methods.iter().map(|m| m.0.clone()).collect(),
            },
        );
        Ok(())
    }

    pub fn merge_extension(
        &mut self,
        ext: &ast::ExtensionDef,
        file_id: FileId,
    ) -> Result<(), TyperError> {
        let target_name = ext.target.0.name.0.clone();
        let ty = self
            .types
            .get_mut(&target_name)
            .ok_or_else(|| TyperError::at(
                format!("extension target `{}` is not a known type", target_name),
                ext.target.1.clone(),
            ))?;

        for (method, _) in &ext.methods {
            let method_name = &method.sig.name.0;
            // Reject ONLY if an inherent method with the same name already
            // exists — two extensions defining the same inherent method is
            // a real error. A trait impl providing the same name alongside
            // is fine (resolver picks the extension via the "inherent wins"
            // rule).
            let collides_with_inherent = ty.methods.iter().any(|m| {
                m.function.sig.name.0 == *method_name && m.from_trait.is_none()
            });
            if collides_with_inherent {
                return Err(TyperError::at(
                    format!(
                        "duplicate method `{}::{}`",
                        target_name, method_name
                    ),
                    method.sig.name.1.clone(),
                ));
            }
            ty.methods.push(MethodInfo {
                function: method.clone(),
                file_id,
                from_trait: None,
            });
        }
        Ok(())
    }

    pub fn register_impl(
        &mut self,
        def: &ast::ImplDef,
        file_id: FileId,
    ) -> Result<(), TyperError> {
        let trait_name = def.trait_ty.0.name.0.clone();
        let target_name = def.target.0.name.0.clone();

        // Trait must exist.
        let trait_info = self
            .traits
            .get(&trait_name)
            .ok_or_else(|| TyperError::at(
                format!("unknown trait `{}` in impl", trait_name),
                def.trait_ty.1.clone(),
            ))?
            .clone();

        // Target type must exist.
        if !self.types.contains_key(&target_name) {
            return Err(TyperError::at(
                format!("impl target `{}` is not a known type", target_name),
                def.target.1.clone(),
            ));
        }

        // Every associated type of the trait must be bound, and no extras.
        let bound_names = def.associated_bindings_names();
        for expected in &trait_info.associated_types {
            if !bound_names.iter().any(|b| b == expected) {
                return Err(TyperError::at(
                    format!(
                        "impl of trait `{}` for `{}` is missing binding for associated type `{}`",
                        trait_name, target_name, expected
                    ),
                    def.trait_ty.1.clone(),
                ));
            }
        }
        for (bound_name, _) in &def.associated_types {
            if !trait_info.associated_types.iter().any(|e| e == &bound_name.0) {
                return Err(TyperError::at(
                    format!(
                        "impl binds unknown associated type `{}` (trait `{}`)",
                        bound_name.0, trait_name
                    ),
                    bound_name.1.clone(),
                ));
            }
        }

        // Every trait method must be implemented.
        for expected in &trait_info.methods {
            let name = &expected.name.0;
            if !def.methods.iter().any(|m| m.0.sig.name.0 == *name) {
                return Err(TyperError::at(
                    format!(
                        "impl of trait `{}` for `{}` is missing method `{}`",
                        trait_name, target_name, name
                    ),
                    def.trait_ty.1.clone(),
                ));
            }
        }
        // No extra methods beyond the trait's methods (impls are not for
        // adding free methods — use an extension for that).
        for (m, _) in &def.methods {
            if !trait_info.methods.iter().any(|e| e.name.0 == m.sig.name.0) {
                return Err(TyperError::at(
                    format!(
                        "impl of trait `{}` for `{}` has method `{}` not declared by the trait",
                        trait_name, target_name, m.sig.name.0
                    ),
                    m.sig.name.1.clone(),
                ));
            }
        }
        // Signature arity check (full structural match is deferred; at this
        // stage we verify param counts & return-presence, which is enough
        // to catch mistakes like adding an extra parameter.
        for expected in &trait_info.methods {
            let got = def
                .methods
                .iter()
                .find(|m| m.0.sig.name.0 == expected.name.0)
                .unwrap();
            check_sig_shape(expected, &got.0.sig)?;
        }

        // Attach impl methods to target type's methods list.
        //
        // Collisions are only an error when:
        //   - another impl of the SAME trait already defined this method
        //     (two impls of one trait for one type = UB), OR
        //   - inherent + trait with same name isn't what we want: we DO
        //     allow that (resolver picks inherent). So only same-trait
        //     duplication is rejected here.
        let ty = self.types.get_mut(&target_name).unwrap();
        for (method, _) in &def.methods {
            let collides_same_trait = ty.methods.iter().any(|m| {
                m.function.sig.name.0 == method.sig.name.0
                    && m.from_trait.as_deref() == Some(trait_name.as_str())
            });
            if collides_same_trait {
                return Err(TyperError::at(
                    format!(
                        "method `{}::{}` already provided by another `impl {} for {}`",
                        target_name, method.sig.name.0, trait_name, target_name
                    ),
                    method.sig.name.1.clone(),
                ));
            }
            ty.methods.push(MethodInfo {
                function: method.clone(),
                file_id,
                from_trait: Some(trait_name.clone()),
            });
        }

        self.impls.push(ImplInfo {
            trait_name,
            target_name,
            associated_bindings: def
                .associated_types
                .iter()
                .map(|(n, t)| (n.0.clone(), t.0.clone()))
                .collect(),
            methods: def.methods.iter().map(|m| m.0.clone()).collect(),
            file_id,
        });
        Ok(())
    }

    // --- method resolution (trait-aware) --------------------------------

    /// Resolve a method call `TypeName::method_name` made from `file_id`.
    ///
    /// Rules (Rust-flavoured):
    ///   1. If an **inherent** method (from an `extension` block) exists
    ///      with the given name, it always wins — regardless of imported
    ///      traits.
    ///   2. Else, collect every trait-impl method with the given name.
    ///      Filter to those whose trait is **in scope** in the calling
    ///      file (either via `use Trait;` or by virtue of the trait being
    ///      an operator trait — see `OPERATOR_TRAITS`).
    ///   3. If exactly one candidate remains, dispatch to it.
    ///   4. If multiple remain, return `Ambiguous`.
    ///   5. If zero remain but there were un-imported candidates, return
    ///      `TraitNotImported` (so the diagnostic can suggest a `use`).
    ///   6. Else, `NotFound`.
    ///
    /// Callers may force-select a specific trait (e.g. from syntax like
    /// `MyTrait::my_method(...)`) via `trait_hint`. When supplied, only
    /// candidates from that trait are considered, and the trait-in-scope
    /// check is skipped.
    pub fn resolve_method(
        &self,
        file_id: FileId,
        type_name: &str,
        method_name: &str,
        trait_hint: Option<&str>,
    ) -> MethodResolution {
        let Some(info) = self.types.get(type_name) else {
            return MethodResolution::NotFound {
                reason: format!("unknown type `{}`", type_name),
            };
        };

        // Explicit qualification: `MyTrait::my_method(args)`. Pick only
        // methods that came from `MyTrait`.
        if let Some(trait_name) = trait_hint {
            for m in &info.methods {
                if m.function.sig.name.0 == method_name
                    && m.from_trait.as_deref() == Some(trait_name)
                {
                    return MethodResolution::Trait {
                        trait_name: trait_name.to_string(),
                    };
                }
            }
            return MethodResolution::NotFound {
                reason: format!(
                    "`{}::{}` not implemented for `{}`",
                    trait_name, method_name, type_name
                ),
            };
        }

        // Step 1: inherent beats everything.
        for m in &info.methods {
            if m.function.sig.name.0 == method_name && m.from_trait.is_none() {
                return MethodResolution::Inherent;
            }
        }

        // Step 2: collect trait candidates.
        let empty = HashSet::new();
        let imports = self.imports.get(&file_id).unwrap_or(&empty);
        let mut in_scope: Vec<String> = Vec::new();
        let mut out_of_scope: Vec<String> = Vec::new();
        for m in &info.methods {
            if m.function.sig.name.0 != method_name {
                continue;
            }
            if let Some(t) = &m.from_trait {
                if imports.contains(t) || OPERATOR_TRAITS.iter().any(|op| op == t) {
                    in_scope.push(t.clone());
                } else {
                    out_of_scope.push(t.clone());
                }
            }
        }

        match in_scope.len() {
            1 => MethodResolution::Trait {
                trait_name: in_scope.into_iter().next().unwrap(),
            },
            0 => {
                if out_of_scope.is_empty() {
                    MethodResolution::NotFound {
                        reason: format!("no method `{}` on `{}`", method_name, type_name),
                    }
                } else {
                    MethodResolution::TraitNotImported { candidates: out_of_scope }
                }
            }
            _ => MethodResolution::Ambiguous {
                candidates: in_scope,
            },
        }
    }

    // --- helpers ----------------------------------------------------------

    fn compute_struct_layout(
        &self,
        def: &ast::StructDef,
    ) -> Result<StructLayout, TyperError> {
        let mut fields = Vec::new();
        let mut offset: CellCount = 0;
        for f in &def.fields {
            let size = self.resolve_type_size(&f.ty.0, &f.ty.1)?;
            fields.push(FieldLayout {
                name: f.name.0.clone(),
                offset,
                size,
                ast_type: f.ty.0.clone(),
            });
            offset += size;
        }
        Ok(StructLayout {
            fields,
            size: offset,
        })
    }

    fn compute_enum_layout(
        &self,
        def: &ast::EnumDef,
    ) -> Result<EnumLayout, TyperError> {
        let count = def.variants.len();
        if count == 0 {
            return Err(TyperError::at(
                format!("enum `{}` has no variants", def.name.0),
                def.name.1.clone(),
            ));
        }
        let discriminant_size = discriminant_size_for(count);

        // Resolve each variant's payload size.
        let mut resolved: Vec<(String, Option<i64>, CellCount)> =
            Vec::with_capacity(count);
        let mut data_size: CellCount = 0;
        for v in &def.variants {
            let size = match &v.data {
                Some((ty, sp)) => self.resolve_type_size(ty, sp)?,
                None => 0,
            };
            if size > data_size {
                data_size = size;
            }
            resolved.push((v.name.0.clone(), v.discriminant.as_ref().map(|(n, _)| *n), size));
        }

        // Assign discriminant values: explicit values taken first; gaps filled
        // with the smallest unused non-negative integer.
        let mut used: std::collections::BTreeSet<u32> = Default::default();
        for (_, discr, _) in &resolved {
            if let Some(d) = discr {
                if *d < 0 {
                    return Err(TyperError::new("negative enum discriminant"));
                }
                used.insert(*d as u32);
            }
        }
        let mut next_auto: u32 = 0;
        let mut variants = Vec::with_capacity(count);
        for (name, discr, size) in resolved {
            let d = match discr {
                Some(d) => d as u32,
                None => {
                    while used.contains(&next_auto) {
                        next_auto += 1;
                    }
                    let d = next_auto;
                    used.insert(d);
                    next_auto += 1;
                    d
                }
            };
            variants.push(EnumVariantLayout {
                name,
                discriminant: d,
                data_size: size,
            });
        }

        Ok(EnumLayout {
            variants,
            discriminant_size,
            data_size,
        })
    }

    /// Resolve a type reference to its size in cells.
    ///
    /// Handles:
    ///   - primitives (U4 = 1)
    ///   - non-generic structs / enums (use computed layout)
    ///   - `Array<T, N, F>` — compiler-known native type: `N * sizeof(T)`.
    ///
    /// Other generic instantiations (user-defined templated structs/enums
    /// with concrete args) are deferred until the monomorphizer regenerates
    /// them as concrete types.
    pub fn resolve_type_size(
        &self,
        ty: &ast::Type,
        sp: &new_parser::Span,
    ) -> Result<CellCount, TyperError> {
        // Native type: Array<T, N, F> has layout N * sizeof(T).
        if ty.name.0 == "Array" && ty.templates.len() == 3 {
            return self.array_layout_size(ty, sp);
        }

        if !ty.templates.is_empty() {
            return Err(TyperError::at(
                format!(
                    "cannot compute size of generic type reference `{}<...>` yet \
                     (monomorphization is Phase 6)",
                    ty.name.0
                ),
                sp.clone(),
            ));
        }
        let info = self.types.get(&ty.name.0).ok_or_else(|| {
            TyperError::at(
                format!("unknown type `{}`", ty.name.0),
                sp.clone(),
            )
        })?;
        match &info.kind {
            TypeKind::Primitive { size } => Ok(*size),
            TypeKind::Struct(StructKind::Concrete(layout)) => Ok(layout.size),
            TypeKind::Struct(StructKind::Templated { .. }) => Err(TyperError::at(
                format!(
                    "cannot take size of templated struct `{}` without template args",
                    ty.name.0
                ),
                sp.clone(),
            )),
            TypeKind::Enum(EnumKind::Concrete(layout)) => Ok(layout.total_size()),
            TypeKind::Enum(EnumKind::Templated { .. }) => Err(TyperError::at(
                format!(
                    "cannot take size of templated enum `{}` without template args",
                    ty.name.0
                ),
                sp.clone(),
            )),
        }
    }

    /// Compute the size of `Array<T, N, F>`: N cells per element, N fixed by
    /// the second template argument. T must be a concrete (sizeable) type;
    /// F (the index type) doesn't contribute to the array's own layout.
    fn array_layout_size(
        &self,
        ty: &ast::Type,
        sp: &new_parser::Span,
    ) -> Result<CellCount, TyperError> {
        let t_arg = &ty.templates[0];
        let n_arg = &ty.templates[1];
        let element_ty = match &t_arg.0 {
            ast::TypeOrValue::Type(t) => t,
            ast::TypeOrValue::Value(_) => {
                return Err(TyperError::at(
                    "Array's first template argument must be a type".to_string(),
                    t_arg.1.clone(),
                ));
            }
        };
        let n = match &n_arg.0 {
            ast::TypeOrValue::Value(v) => *v as CellCount,
            ast::TypeOrValue::Type(_) => {
                return Err(TyperError::at(
                    "Array's second template argument must be an integer literal".to_string(),
                    n_arg.1.clone(),
                ));
            }
        };
        let elem_size = self.resolve_type_size(element_ty, &t_arg.1)?;
        let _ = sp;
        Ok(elem_size * n)
    }

    /// Rough check: does this type reference a template parameter name, or
    /// does it instantiate a templated type? Used as a quick "can I compute
    /// this now?" gate.
    fn type_is_templated(&self, ty: &ast::Type) -> bool {
        if !ty.templates.is_empty() {
            return true;
        }
        match self.types.get(&ty.name.0) {
            Some(TypeInfo { kind: TypeKind::Struct(StructKind::Templated { .. }), .. }) => true,
            Some(TypeInfo { kind: TypeKind::Enum(EnumKind::Templated { .. }), .. }) => true,
            Some(_) => false,
            // Unknown name: could be a template param of the enclosing enum.
            None => true,
        }
    }
}

// --- signature-shape check -------------------------------------------------

fn check_sig_shape(
    expected: &ast::FunctionSig,
    got: &ast::FunctionSig,
) -> Result<(), TyperError> {
    if expected.params.len() != got.params.len() {
        return Err(TyperError::at(
            format!(
                "method `{}`: expected {} parameters, got {}",
                expected.name.0,
                expected.params.len(),
                got.params.len()
            ),
            got.name.1.clone(),
        ));
    }
    match (&expected.return_type, &got.return_type) {
        (Some(_), None) | (None, Some(_)) => {
            return Err(TyperError::at(
                format!(
                    "method `{}`: return type presence mismatches trait signature",
                    expected.name.0
                ),
                got.name.1.clone(),
            ));
        }
        _ => {}
    }
    Ok(())
}

// --- small conveniences on the AST ----------------------------------------

trait ImplDefExt {
    fn associated_bindings_names(&self) -> Vec<String>;
}
impl ImplDefExt for ast::ImplDef {
    fn associated_bindings_names(&self) -> Vec<String> {
        self.associated_types.iter().map(|(n, _)| n.0.clone()).collect()
    }
}

