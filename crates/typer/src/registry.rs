//! `TypeRegistry` and the registration/validation passes.

use std::collections::{HashMap, HashSet};

use new_parser::ast::{self, Spanned};

use crate::types::*;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TypeRegistry {
    /// Dense storage for every registered type. Indexed by `TypeId`.
    /// Entries never move; name-lookup tables get rewritten on
    /// cross-file collision migrations, not the infos themselves.
    pub type_infos: Vec<TypeInfo>,
    /// One canonical lookup key per `TypeId`, parallel to `type_infos`.
    /// For unambiguous types this is the bare name; for types that
    /// collided across files it's the fully-qualified `<module>::<bare>`
    /// form. Always guaranteed to be present as a key in `type_ids`,
    /// so callers can use it as a lookup string.
    pub type_canonical_keys: Vec<String>,
    /// Every globally-unambiguous lookup key → `TypeId`. Includes:
    ///   * the canonical bare name (unless ambiguous across files)
    ///   * every fully-qualified `<module>::<bare>` form
    /// Bare names that collide across files are removed here and kept
    /// only in per-file scope maps below.
    pub type_ids: HashMap<String, TypeId>,
    /// Dense storage for every registered trait. Indexed by `TraitId`.
    pub trait_infos: Vec<TraitInfo>,
    pub trait_canonical_keys: Vec<String>,
    pub trait_ids: HashMap<String, TraitId>,
    pub impls: Vec<ImplInfo>,
    /// Blanket impls: `impl<T: A + B> Trait for T { ... }`. Stored
    /// separately so the post-pass can iterate them to decide which
    /// concrete types satisfy their bounds. Direct (non-generic) impls
    /// remain in `impls`.
    pub blanket_impls: Vec<ImplInfo>,
    /// Per-file import scope: for each `FileId`, the set of trait
    /// names brought into scope via `use Name;`. Drives operator
    /// dispatch (trait-in-scope check).
    pub imports: HashMap<FileId, HashSet<String>>,
    /// Module path per file (e.g., `std::ArrayList` for
    /// `std/ArrayList.ct`). Used to derive fully-qualified names.
    pub file_module_paths: HashMap<FileId, String>,
    /// Original source filename per file. Kept separately from the
    /// module path so diagnostics can cite the actual file on disk
    /// (`examples/new_syntax/Morpion.ct`) rather than the derived
    /// module form.
    pub file_names: HashMap<FileId, String>,
    /// Per-file name → `TypeId` scope. Unified store for:
    ///   * the file's own declarations (bare → id, takes precedence
    ///     over the global ambiguous entry on collision)
    ///   * `use path::X;` aliases (bare leaf → id from the full path)
    /// Consulted BEFORE the global `type_ids` map.
    pub file_type_scope: HashMap<FileId, HashMap<String, TypeId>>,
    /// Same as `file_type_scope` but for traits.
    pub file_trait_scope: HashMap<FileId, HashMap<String, TraitId>>,
    /// Lazy resolution of `use` aliases: at registration time we may
    /// not yet know the referenced type's `TypeId`, so the statement
    /// is recorded here and resolved into the scope maps in a second
    /// pass after all types/traits are registered.
    pub pending_use_aliases: Vec<(FileId, String, String)>,
    /// Names declared in 2+ files; bare lookups from non-declaring
    /// files return `None`. Kept for diagnostics and to prevent
    /// permissive fallback from picking one arbitrarily.
    pub ambiguous_bare: HashSet<String>,
    /// First file to declare each bare name — used during registration
    /// to know which existing entry to migrate from bare → FQ when
    /// a second file declares the same name.
    pub first_declarer: HashMap<String, FileId>,
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
        r.insert_type(
            U4_NAME.to_string(),
            TypeInfo {
                name: U4_NAME.to_string(),
                templates: Vec::new(),
                kind: TypeKind::Primitive { size: U4_SIZE },
                methods: Vec::new(),
                decl_span: None,
                decl_file: None,
            },
        );
        r
    }

    // --- dense-storage accessors -----------------------------------------

    /// Append a new `TypeInfo`, register `name` → fresh `TypeId`.
    /// The given `name` also becomes this id's canonical key.
    pub fn insert_type(&mut self, name: String, info: TypeInfo) -> TypeId {
        let id = TypeId(self.type_infos.len() as u32);
        self.type_infos.push(info);
        self.type_canonical_keys.push(name.clone());
        self.type_ids.insert(name, id);
        id
    }

    pub fn insert_trait(&mut self, name: String, info: TraitInfo) -> TraitId {
        let id = TraitId(self.trait_infos.len() as u32);
        self.trait_infos.push(info);
        self.trait_canonical_keys.push(name.clone());
        self.trait_ids.insert(name, id);
        id
    }

    /// Swap the canonical key associated with `id`. Used when a
    /// cross-file collision forces the earlier entry from bare to FQ.
    fn set_type_canonical_key(&mut self, id: TypeId, new_key: String) {
        self.type_canonical_keys[id.0 as usize] = new_key;
    }

    fn set_trait_canonical_key(&mut self, id: TraitId, new_key: String) {
        self.trait_canonical_keys[id.0 as usize] = new_key;
    }

    /// Look up `name` → `TypeId`, following only the direct name index
    /// (no file-aware resolution). Callers that need path/alias
    /// resolution must go through `canonicalize_type_name` first.
    pub fn type_id(&self, name: &str) -> Option<TypeId> {
        self.type_ids.get(name).copied()
    }

    pub fn trait_id(&self, name: &str) -> Option<TraitId> {
        self.trait_ids.get(name).copied()
    }

    pub fn has_type(&self, name: &str) -> bool {
        self.type_ids.contains_key(name)
    }

    pub fn has_trait(&self, name: &str) -> bool {
        self.trait_ids.contains_key(name)
    }

    /// Look up a `TypeInfo` by its registered name (direct, no alias chain).
    pub fn get_type(&self, name: &str) -> Option<&TypeInfo> {
        let id = *self.type_ids.get(name)?;
        Some(&self.type_infos[id.0 as usize])
    }

    pub fn get_type_mut(&mut self, name: &str) -> Option<&mut TypeInfo> {
        let id = *self.type_ids.get(name)?;
        Some(&mut self.type_infos[id.0 as usize])
    }

    pub fn get_trait(&self, name: &str) -> Option<&TraitInfo> {
        let id = *self.trait_ids.get(name)?;
        Some(&self.trait_infos[id.0 as usize])
    }

    pub fn type_by_id(&self, id: TypeId) -> &TypeInfo {
        &self.type_infos[id.0 as usize]
    }

    pub fn type_by_id_mut(&mut self, id: TypeId) -> &mut TypeInfo {
        &mut self.type_infos[id.0 as usize]
    }

    pub fn trait_by_id(&self, id: TraitId) -> &TraitInfo {
        &self.trait_infos[id.0 as usize]
    }

    /// Iterate `(name, &TypeInfo)` pairs. Like the old `types.iter()`.
    /// Iterate `(canonical_key, &TypeInfo)` — one entry per unique
    /// `TypeId`, using each id's canonical lookup key. Aliases (FQ
    /// paths pointing at the same id) are NOT repeated. Use this for
    /// any "once per type" pass (e.g. FunctionDB construction).
    pub fn iter_types(&self) -> impl Iterator<Item = (&str, &TypeInfo)> {
        self.type_canonical_keys
            .iter()
            .zip(self.type_infos.iter())
            .map(|(k, info)| (k.as_str(), info))
    }

    pub fn iter_traits(&self) -> impl Iterator<Item = (&str, &TraitInfo)> {
        self.trait_canonical_keys
            .iter()
            .zip(self.trait_infos.iter())
            .map(|(k, info)| (k.as_str(), info))
    }

    /// Iterate `&TypeInfo` only. Like the old `types.values()`.
    pub fn all_types(&self) -> impl Iterator<Item = &TypeInfo> {
        self.type_infos.iter()
    }

    /// Derive a module path for a file — strip `.ct`, replace path
    /// separators with `::`. `std/ArrayList.ct` → `std::ArrayList`.
    fn derive_module_path(file_name: &str) -> String {
        let trimmed = file_name.strip_suffix(".ct").unwrap_or(file_name);
        trimmed.replace('/', "::").replace('\\', "::")
    }

    /// Join a module path and a leaf, producing `path::leaf` unless
    /// the path is empty (just `leaf`). Also returns just `leaf` for
    /// the synthetic "main" file of the single-file entry point.
    fn join_path(module: &str, leaf: &str) -> String {
        if module.is_empty() {
            leaf.to_string()
        } else {
            format!("{}::{}", module, leaf)
        }
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

        // Record per-file module paths + raw file names up front so
        // registration passes can canonicalize names as they go and
        // diagnostics have a file to cite.
        for (file_ix, (file_name, _)) in files.iter().enumerate() {
            let file_id = file_ix as FileId;
            r.file_module_paths
                .insert(file_id, Self::derive_module_path(file_name));
            r.file_names.insert(file_id, file_name.to_string());
        }

        // Pass 1: collect structs/enums/traits (populates `type_infos` /
        // `trait_infos`) and record per-file `use` aliases for later
        // resolution (they may reference types not yet registered).
        for (file_ix, (_file, items)) in files.iter().enumerate() {
            let file_id = file_ix as FileId;
            // Ensure the file has an imports entry even if no `use` statements.
            r.imports.entry(file_id).or_default();
            for (item, _sp) in *items {
                let out = match item {
                    ast::Item::Struct(s) => r.register_struct_with_file(s, file_id),
                    ast::Item::Enum(e) => r.register_enum_with_file(e, file_id),
                    ast::Item::Trait(t) => r.register_trait_with_file(t, file_id),
                    ast::Item::Use(u) => {
                        let full = u.name.0.clone();
                        let leaf = full.rsplit("::").next().unwrap_or(&full).to_string();
                        // Tracked in `imports` for trait-in-scope checks
                        // (operator dispatch).
                        r.imports.entry(file_id).or_default().insert(leaf.clone());
                        // Deferred: the target type/trait may not yet be
                        // registered. Pass 1.5 resolves these into the
                        // per-file scope maps below.
                        r.pending_use_aliases.push((file_id, leaf, full));
                        Ok(())
                    }
                    _ => Ok(()),
                };
                if let Err(e) = out {
                    errors.push(e);
                }
            }
        }

        // Pass 1.5: resolve `use` aliases now that all declarations are
        // registered. Unresolvable aliases stay pending; the lookup
        // path tolerates them falling through to None.
        for (file_id, leaf, full) in std::mem::take(&mut r.pending_use_aliases) {
            if let Some(id) = r.type_ids.get(&full).copied() {
                r.file_type_scope.entry(file_id).or_default().insert(leaf.clone(), id);
            }
            if let Some(id) = r.trait_ids.get(&full).copied() {
                r.file_trait_scope.entry(file_id).or_default().insert(leaf, id);
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

        // Pass 4: apply blanket impls. For every registered blanket
        // `impl<T: B1 + B2> Trait for T`, walk every concrete non-generic
        // type and — when all bounds are satisfied — attach the blanket
        // impl's methods to that type's method list. Errors (e.g. a
        // blanket method colliding with an inherent one already on the
        // type) become hard errors here.
        r.attach_blanket_impls(&mut errors);

        if errors.is_empty() {
            Ok(r)
        } else {
            Err(errors)
        }
    }

    /// Resolve `name` (bare, path-qualified, or `use`-aliased) in the
    /// given file's scope to a `TypeId`. Primary name-resolution API.
    ///
    /// Precedence: per-file scope (own decls and `use` aliases) wins
    /// over the global name map. Ambiguous bare names return `None`
    /// when queried without a file context that declares them.
    pub fn resolve_type_id(&self, name: &str, file_id: Option<FileId>) -> Option<TypeId> {
        if let Some(fid) = file_id {
            if let Some(id) = self.file_type_scope.get(&fid).and_then(|m| m.get(name)) {
                return Some(*id);
            }
        }
        if !name.contains("::") && self.ambiguous_bare.contains(name) {
            return None;
        }
        self.type_ids.get(name).copied()
    }

    /// Same as `resolve_type_id` but for traits.
    pub fn resolve_trait_id(&self, name: &str, file_id: Option<FileId>) -> Option<TraitId> {
        if let Some(fid) = file_id {
            if let Some(id) = self.file_trait_scope.get(&fid).and_then(|m| m.get(name)) {
                return Some(*id);
            }
        }
        if !name.contains("::") && self.ambiguous_bare.contains(name) {
            return None;
        }
        self.trait_ids.get(name).copied()
    }

    /// Back-compat helper that returns a storage-key string for `name`.
    /// The returned key is always present in `type_ids` / `trait_ids`,
    /// so callers can pass it straight to `get_type` / `get_trait`.
    ///
    /// Types win over traits when both share a name (matching the
    /// previous behavior — both were keyed into the same global map).
    pub fn canonicalize_type_name(
        &self,
        name: &str,
        file_id: Option<FileId>,
    ) -> Option<String> {
        if let Some(id) = self.resolve_type_id(name, file_id) {
            return Some(self.type_canonical_keys[id.0 as usize].clone());
        }
        if let Some(id) = self.resolve_trait_id(name, file_id) {
            return Some(self.trait_canonical_keys[id.0 as usize].clone());
        }
        // Permissive fallback for fully-qualified paths whose exact
        // key isn't registered but whose leaf is a globally-known,
        // unambiguous type. Preserves tests that make up a module
        // prefix (e.g. `nonsense::Foo` when only `Foo` is declared).
        if let Some(idx) = name.rfind("::") {
            let leaf = &name[idx + 2..];
            if !self.ambiguous_bare.contains(leaf) {
                if self.has_type(leaf) {
                    return Some(leaf.to_string());
                }
                if self.has_trait(leaf) {
                    return Some(leaf.to_string());
                }
            }
        }
        None
    }

    /// Convenience: resolve a bare/FQ name to a `TypeInfo` reference.
    pub fn lookup_type(&self, name: &str, file_id: Option<FileId>) -> Option<&TypeInfo> {
        self.canonicalize_type_name(name, file_id)
            .and_then(|cn| self.get_type(&cn))
    }

    // --- individual registration passes (exposed for testing) -------------

    /// Wrapper for legacy test call sites — registers with file_id=0,
    /// which uses empty module path ("") and names the type by its
    /// bare identifier only. Real compilation always goes through
    /// `register_struct_with_file`.
    pub fn register_struct(&mut self, def: &ast::StructDef) -> Result<(), TyperError> {
        self.register_struct_core(def)
    }

    pub fn register_enum(&mut self, def: &ast::EnumDef) -> Result<(), TyperError> {
        self.register_enum_core(def)
    }

    pub fn register_trait(&mut self, def: &ast::TraitDef) -> Result<(), TyperError> {
        self.register_trait_core(def)
    }

    /// File-aware struct registration. Storage key is the bare name
    /// when unambiguous; on collision with a type declared in another
    /// file, both entries get migrated to fully-qualified keys.
    pub fn register_struct_with_file(
        &mut self,
        def: &ast::StructDef,
        file_id: FileId,
    ) -> Result<(), TyperError> {
        let key = self.reserve_registration_key(&def.name.0, file_id);
        self.register_struct_core_at(def, &key, Some(file_id))?;
        self.alias_type_post_register(&def.name.0, &key, file_id);
        Ok(())
    }

    pub fn register_enum_with_file(
        &mut self,
        def: &ast::EnumDef,
        file_id: FileId,
    ) -> Result<(), TyperError> {
        let key = self.reserve_registration_key(&def.name.0, file_id);
        self.register_enum_core_at(def, &key, Some(file_id))?;
        self.alias_type_post_register(&def.name.0, &key, file_id);
        Ok(())
    }

    pub fn register_trait_with_file(
        &mut self,
        def: &ast::TraitDef,
        file_id: FileId,
    ) -> Result<(), TyperError> {
        let key = self.reserve_registration_key(&def.name.0, file_id);
        self.register_trait_core_at(def, &key, Some(file_id))?;
        self.alias_trait_post_register(&def.name.0, &key, file_id);
        Ok(())
    }

    /// Post-registration bookkeeping: expose the just-stored type
    /// under every valid lookup form and record its file-local scope.
    fn alias_type_post_register(&mut self, bare: &str, stored_key: &str, file_id: FileId) {
        let Some(id) = self.type_ids.get(stored_key).copied() else { return };
        // Always alias the FQ form so path references resolve globally.
        let module = self.file_module_paths.get(&file_id).cloned().unwrap_or_default();
        if !module.is_empty() {
            let fq = Self::join_path(&module, bare);
            self.type_ids.insert(fq, id);
        }
        // The declaring file sees the bare name via scope, winning over
        // any globally-ambiguous entry.
        self.file_type_scope.entry(file_id).or_default().insert(bare.to_string(), id);
    }

    fn alias_trait_post_register(&mut self, bare: &str, stored_key: &str, file_id: FileId) {
        let Some(id) = self.trait_ids.get(stored_key).copied() else { return };
        let module = self.file_module_paths.get(&file_id).cloned().unwrap_or_default();
        if !module.is_empty() {
            let fq = Self::join_path(&module, bare);
            self.trait_ids.insert(fq, id);
        }
        self.file_trait_scope.entry(file_id).or_default().insert(bare.to_string(), id);
    }

    /// Pick the storage key for a type/trait about to be registered.
    ///
    /// Returns the bare name on a first declaration (simplest case).
    /// On a cross-file collision, migrates the earlier entry to its
    /// fully-qualified key (`<module>::<bare>`) and returns the new
    /// declarer's own FQ key. Same-file re-declarations return the
    /// key that was previously assigned so the core's duplicate-check
    /// fires against the existing entry.
    fn reserve_registration_key(&mut self, bare: &str, file_id: FileId) -> String {
        let module = self
            .file_module_paths
            .get(&file_id)
            .cloned()
            .unwrap_or_default();
        let fq = Self::join_path(&module, bare);

        match self.first_declarer.get(bare).copied() {
            None => {
                // First declaration of this bare name anywhere. Register
                // under bare key. Post-registration we'll also alias the
                // FQ form → same id (see `register_*_with_file`).
                self.first_declarer.insert(bare.to_string(), file_id);
                bare.to_string()
            }
            Some(prev) if prev == file_id => {
                // Same file redeclares — return whichever key this file
                // originally registered under (bare or FQ after collision).
                self.file_type_scope
                    .get(&file_id)
                    .and_then(|m| m.get(bare))
                    .and_then(|id| Some(self.type_canonical_keys[id.0 as usize].clone()))
                    .or_else(|| {
                        self.file_trait_scope
                            .get(&file_id)
                            .and_then(|m| m.get(bare))
                            .map(|id| self.trait_canonical_keys[id.0 as usize].clone())
                    })
                    .unwrap_or_else(|| bare.to_string())
            }
            Some(prev_file_id) => {
                // Cross-file collision. The earlier entry is still keyed
                // at bare; migrate it to the previous declarer's FQ form,
                // drop the shared bare name from the global map, and
                // record both files' bare → id in their per-file scopes.
                if !self.ambiguous_bare.contains(bare) {
                    let prev_module = self
                        .file_module_paths
                        .get(&prev_file_id)
                        .cloned()
                        .unwrap_or_default();
                    let prev_fq = Self::join_path(&prev_module, bare);
                    if prev_fq != bare {
                        // Both types and traits can share a bare name,
                        // so handle each independently.
                        if let Some(id) = self.type_ids.remove(bare) {
                            self.type_ids.insert(prev_fq.clone(), id);
                            self.set_type_canonical_key(id, prev_fq.clone());
                            self.file_type_scope
                                .entry(prev_file_id)
                                .or_default()
                                .insert(bare.to_string(), id);
                        }
                        if let Some(id) = self.trait_ids.remove(bare) {
                            self.trait_ids.insert(prev_fq.clone(), id);
                            self.set_trait_canonical_key(id, prev_fq.clone());
                            self.file_trait_scope
                                .entry(prev_file_id)
                                .or_default()
                                .insert(bare.to_string(), id);
                        }
                    }
                    self.ambiguous_bare.insert(bare.to_string());
                }
                fq
            }
        }
    }

    fn register_struct_core(&mut self, def: &ast::StructDef) -> Result<(), TyperError> {
        self.register_struct_core_at(def, &def.name.0, None)
    }

    fn register_enum_core(&mut self, def: &ast::EnumDef) -> Result<(), TyperError> {
        self.register_enum_core_at(def, &def.name.0, None)
    }

    fn register_trait_core(&mut self, def: &ast::TraitDef) -> Result<(), TyperError> {
        self.register_trait_core_at(def, &def.name.0, None)
    }

    fn register_struct_core_at(
        &mut self,
        def: &ast::StructDef,
        storage_key: &str,
        file_id: Option<FileId>,
    ) -> Result<(), TyperError> {
        let name = def.name.0.clone();
        if self.has_type(storage_key) && name != U4_NAME {
            return Err(self.duplicate_type_diag(&name, &def.name.1, storage_key, file_id));
        }
        let templates: Vec<String> = def.templates.iter().map(|t| t.0.clone()).collect();

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

        self.insert_type(
            storage_key.to_string(),
            TypeInfo {
                name,
                templates,
                kind,
                methods: Vec::new(),
                decl_span: Some(def.name.1.clone()),
                decl_file: file_id.and_then(|f| self.file_names.get(&f).cloned()),
            },
        );
        Ok(())
    }

    fn register_enum_core_at(
        &mut self,
        def: &ast::EnumDef,
        storage_key: &str,
        file_id: Option<FileId>,
    ) -> Result<(), TyperError> {
        let name = def.name.0.clone();
        if self.has_type(storage_key) {
            return Err(self.duplicate_type_diag(&name, &def.name.1, storage_key, file_id));
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

        self.insert_type(
            storage_key.to_string(),
            TypeInfo {
                name,
                templates,
                kind,
                methods: Vec::new(),
                decl_span: Some(def.name.1.clone()),
                decl_file: file_id.and_then(|f| self.file_names.get(&f).cloned()),
            },
        );
        Ok(())
    }

    fn register_trait_core_at(
        &mut self,
        def: &ast::TraitDef,
        storage_key: &str,
        file_id: Option<FileId>,
    ) -> Result<(), TyperError> {
        let name = def.name.0.clone();
        if self.has_trait(storage_key) {
            return Err(self.duplicate_trait_diag(&name, &def.name.1, storage_key, file_id));
        }
        self.insert_trait(
            storage_key.to_string(),
            TraitInfo {
                name,
                templates: def.templates.iter().map(|t| t.0.clone()).collect(),
                associated_types: def.associated_types.iter().map(|a| a.0.clone()).collect(),
                methods: def.methods.iter().map(|m| m.0.clone()).collect(),
                decl_span: Some(def.name.1.clone()),
                decl_file: file_id.and_then(|f| self.file_names.get(&f).cloned()),
            },
        );
        Ok(())
    }

    /// Build a rich diagnostic for a duplicate type definition. Adds a
    /// secondary label pointing at the first declaration when we have
    /// its span on file.
    fn duplicate_type_diag(
        &self,
        name: &str,
        new_span: &new_parser::Span,
        existing_key: &str,
        file_id: Option<FileId>,
    ) -> TyperError {
        let file = file_id
            .and_then(|f| self.file_names.get(&f).cloned())
            .unwrap_or_default();
        let mut diag = errors::Diagnostic::error(format!("duplicate definition of type `{}`", name))
            .with_code(errors::codes::E_DUPLICATE_TYPE)
            .with_primary(
                errors::FileSpan::new(&file, new_span.clone()),
                "duplicate definition here",
            )
            .with_note("each type may only be defined once per scope")
            .with_help("rename one of them, or use a module path to disambiguate");
        if let Some(existing) = self.get_type(existing_key) {
            if let (Some(sp), Some(f)) = (&existing.decl_span, &existing.decl_file) {
                diag = diag.with_secondary(
                    errors::FileSpan::new(f, sp.clone()),
                    "first defined here",
                );
            }
        }
        TyperError::from_diagnostic(diag)
    }

    fn duplicate_trait_diag(
        &self,
        name: &str,
        new_span: &new_parser::Span,
        existing_key: &str,
        file_id: Option<FileId>,
    ) -> TyperError {
        let file = file_id
            .and_then(|f| self.file_names.get(&f).cloned())
            .unwrap_or_default();
        let mut diag = errors::Diagnostic::error(format!("duplicate definition of trait `{}`", name))
            .with_code(errors::codes::E_DUPLICATE_TRAIT)
            .with_primary(
                errors::FileSpan::new(&file, new_span.clone()),
                "duplicate definition here",
            )
            .with_note("each trait may only be defined once per scope");
        if let Some(existing) = self.get_trait(existing_key) {
            if let (Some(sp), Some(f)) = (&existing.decl_span, &existing.decl_file) {
                diag = diag.with_secondary(
                    errors::FileSpan::new(f, sp.clone()),
                    "first defined here",
                );
            }
        }
        TyperError::from_diagnostic(diag)
    }

    pub fn merge_extension(
        &mut self,
        ext: &ast::ExtensionDef,
        file_id: FileId,
    ) -> Result<(), TyperError> {
        let raw = ext.target.0.name.0.clone();
        let target_name = self
            .canonicalize_type_name(&raw, Some(file_id))
            .unwrap_or(raw);
        // Snapshot file name before we grab the mutable borrow
        // below — the duplicate-method diagnostic needs it and
        // borrow-checker rules forbid a second borrow.
        let file = self.file_names.get(&file_id).cloned().unwrap_or_default();
        let ty = self
            .get_type_mut(&target_name)
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
            let first = ty
                .methods
                .iter()
                .find(|m| {
                    m.function.sig.name.0 == *method_name && m.from_trait.is_none()
                })
                .map(|m| m.function.sig.name.1.clone());
            let collides_with_inherent = first.is_some();
            if collides_with_inherent {
                let mut diag = errors::Diagnostic::error(format!(
                    "duplicate method `{}::{}`",
                    target_name, method_name
                ))
                .with_code(errors::codes::E_DUPLICATE_METHOD)
                .with_primary(
                    errors::FileSpan::new(&file, method.sig.name.1.clone()),
                    format!("duplicate `{}` here", method_name),
                )
                .with_help(format!(
                    "rename one of the two `{}` methods, or merge their bodies",
                    method_name
                ));
                if let Some(first_span) = first {
                    diag = diag.with_secondary(
                        errors::FileSpan::new(&file, first_span),
                        "first defined here".to_string(),
                    );
                }
                return Err(TyperError::from_diagnostic(diag));
            }
            ty.methods.push(MethodInfo {
                function: method.clone(),
                file_id,
                from_trait: None,
                trait_template_args: Vec::new(),
                blanket: None,
            });
        }
        Ok(())
    }

    pub fn register_impl(
        &mut self,
        def: &ast::ImplDef,
        file_id: FileId,
    ) -> Result<(), TyperError> {
        // Canonicalize path-qualified references so `impl lib::M::Trait
        // for lib::T::Type` ends up keyed by the same bare name the
        // declaring file used. Falls through to the verbatim name if
        // the path doesn't resolve — later validation catches it.
        let trait_name_raw = def.trait_ty.0.name.0.clone();
        let target_name_raw = def.target.0.name.0.clone();
        let trait_id = self
            .resolve_trait_id(&trait_name_raw, Some(file_id))
            .ok_or_else(|| {
                let file = self.file_names.get(&file_id).cloned().unwrap_or_default();
                let diag = errors::Diagnostic::error(format!(
                    "unknown trait `{}` in impl",
                    trait_name_raw
                ))
                .with_code(errors::codes::E_UNKNOWN_TRAIT)
                .with_primary(
                    errors::FileSpan::new(&file, def.trait_ty.1.clone()),
                    format!("trait `{}` not found in scope", trait_name_raw),
                )
                .with_help(format!(
                    "declare it with `trait {} {{ … }}` or import it with `use {};`",
                    trait_name_raw, trait_name_raw
                ));
                TyperError::from_diagnostic(diag)
            })?;
        let trait_name = self.trait_canonical_keys[trait_id.0 as usize].clone();
        let target_name = self
            .canonicalize_type_name(&target_name_raw, Some(file_id))
            .unwrap_or(target_name_raw);

        let trait_info = self.trait_by_id(trait_id).clone();

        // A blanket impl is anything with a non-empty generic list on
        // the `impl` header. Two shapes are supported:
        //
        //   `impl<T: ...> Trait for T`              (bare target)
        //   `impl<T: ...> Trait for Container<T>`   (generic target)
        //
        // For the generic-target case, the target's template args must
        // each be a bare reference to one of the declared generics (so
        // we know how to source each generic from a call site).
        let generic_names: Vec<String> =
            def.generics.iter().map(|g| g.name.0.clone()).collect();
        let is_blanket = !def.generics.is_empty();
        if is_blanket {
            let target_is_bare_generic = generic_names.contains(&target_name)
                && def.target.0.templates.is_empty();
            let target_is_generic_instance = !def.target.0.templates.is_empty()
                && def.target.0.templates.iter().all(|(tv, _)| match tv {
                    ast::TypeOrValue::Type(t) => {
                        t.templates.is_empty() && generic_names.contains(&t.name.0)
                    }
                    _ => false,
                });
            if !target_is_bare_generic && !target_is_generic_instance {
                return Err(TyperError::at(
                    format!(
                        "generic impl target must be a bare generic or a \
                         generic instantiation whose args are all declared \
                         generics — `{}` does not match",
                        target_name
                    ),
                    def.target.1.clone(),
                ));
            }
        }
        // Resolve each bound's trait reference to a `TraitId` up
        // front so downstream comparisons are id-based. Unknown
        // traits error here; typos don't leak to the blanket pass.
        let mut generics: Vec<GenericParamInfo> = Vec::with_capacity(def.generics.len());
        for g in &def.generics {
            let mut bounds: Vec<BoundRef> = Vec::with_capacity(g.bounds.len());
            for (t, _) in &g.bounds {
                let trait_id = self
                    .resolve_trait_id(&t.name.0, Some(file_id))
                    .ok_or_else(|| {
                        let file = self
                            .file_names
                            .get(&file_id)
                            .cloned()
                            .unwrap_or_default();
                        let diag = errors::Diagnostic::error(format!(
                            "unknown trait `{}` in bound",
                            t.name.0
                        ))
                        .with_code(errors::codes::E_UNKNOWN_TRAIT)
                        .with_primary(
                            errors::FileSpan::new(&file, def.target.1.clone()),
                            format!("trait `{}` not found in scope", t.name.0),
                        )
                        .with_help(format!(
                            "declare the trait or bring it into scope with `use {};`",
                            t.name.0
                        ));
                        TyperError::from_diagnostic(diag)
                    })?;
                bounds.push(BoundRef {
                    trait_id,
                    trait_args: t.templates.iter().map(|(tv, _)| tv.clone()).collect(),
                });
            }
            generics.push(GenericParamInfo {
                name: g.name.0.clone(),
                bounds,
            });
        }

        // Blanket impls skip the direct target-exists check — T is a
        // placeholder, not a registered type.
        if !is_blanket && !self.has_type(&target_name) {
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

        // E0012: every trait method must be implemented. Batch
        // missing methods into one diagnostic so the user sees
        // the whole set at once instead of error-per-round-trip.
        let missing: Vec<&str> = trait_info
            .methods
            .iter()
            .filter(|e| !def.methods.iter().any(|m| m.0.sig.name.0 == e.name.0))
            .map(|e| e.name.0.as_str())
            .collect();
        if !missing.is_empty() {
            let file = self.file_names.get(&file_id).cloned().unwrap_or_default();
            let list = missing
                .iter()
                .map(|n| format!("`{}`", n))
                .collect::<Vec<_>>()
                .join(", ");
            let stubs = missing
                .iter()
                .map(|n| format!("fn {}(...)  {{ ... }}", n))
                .collect::<Vec<_>>()
                .join(", ");
            let diag = errors::Diagnostic::error(format!(
                "impl of trait `{}` for `{}` is missing method{}: {}",
                trait_name,
                target_name,
                if missing.len() == 1 { "" } else { "s" },
                list
            ))
            .with_code(errors::codes::E_MISSING_IMPL)
            .with_primary(
                errors::FileSpan::new(&file, def.trait_ty.1.clone()),
                format!(
                    "missing {} here",
                    if missing.len() == 1 { "method" } else { "methods" }
                ),
            )
            .with_help(format!(
                "add {} to the impl body",
                stubs
            ));
            return Err(TyperError::from_diagnostic(diag));
        }
        // No extra methods beyond the trait's methods (impls are not for
        // adding free methods — use an extension for that).
        for (m, _) in &def.methods {
            if !trait_info.methods.iter().any(|e| e.name.0 == m.sig.name.0) {
                let file = self.file_names.get(&file_id).cloned().unwrap_or_default();
                let diag = errors::Diagnostic::error(format!(
                    "impl of trait `{}` for `{}` has method `{}` not declared by the trait",
                    trait_name, target_name, m.sig.name.0
                ))
                .with_code(errors::codes::E_MISSING_IMPL)
                .with_primary(
                    errors::FileSpan::new(&file, m.sig.name.1.clone()),
                    format!("method `{}` is not part of trait `{}`", m.sig.name.0, trait_name),
                )
                .with_help(format!(
                    "move this method to an `extension {} {{ … }}` block, or add it to trait `{}`",
                    target_name, trait_name
                ));
                return Err(TyperError::from_diagnostic(diag));
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
        //   - another impl of the SAME trait WITH THE SAME TEMPLATE ARGS
        //     already defined this method (two identical impls for one
        //     type = UB), OR
        //   - inherent + trait with same name isn't what we want: we DO
        //     allow that (resolver picks inherent). So only same-trait-
        //     same-args duplication is rejected here — `impl Convert<U4>
        //     for U4` and `impl Convert<U8> for U4` coexist.
        let trait_args: Vec<ast::TypeOrValue> = def
            .trait_ty
            .0
            .templates
            .iter()
            .map(|(tv, _)| tv.clone())
            .collect();

        // Collect target template arg names for blanket impls. For
        // `impl<T> Trait for Container<T>`, this is `["T"]`. For bare
        // targets or non-blanket impls, this is empty.
        let target_template_args: Vec<String> = if is_blanket {
            def.target
                .0
                .templates
                .iter()
                .filter_map(|(tv, _)| match tv {
                    ast::TypeOrValue::Type(t) if t.templates.is_empty() => Some(t.name.0.clone()),
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };

        // Blanket impls are deferred: the post-pass (attach_blanket_impls)
        // walks them once all regular impls have been processed so we can
        // check each candidate type's bound satisfaction accurately.
        if is_blanket {
            self.blanket_impls.push(ImplInfo {
                trait_name,
                target_name,
                target_template_args,
                trait_template_args: trait_args.clone(),
                generics,
                associated_bindings: def
                    .associated_types
                    .iter()
                    .map(|(n, t)| (n.0.clone(), t.0.clone()))
                    .collect(),
                methods: def.methods.iter().map(|m| m.0.clone()).collect(),
                file_id,
            });
            return Ok(());
        }

        let ty = self.get_type_mut(&target_name).unwrap();
        for (method, _) in &def.methods {
            let collides_same_trait = ty.methods.iter().any(|m| {
                m.function.sig.name.0 == method.sig.name.0
                    && m.from_trait == Some(trait_id)
                    && m.trait_template_args == trait_args
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
                from_trait: Some(trait_id),
                trait_template_args: trait_args.clone(),
                blanket: None,
            });
        }

        self.impls.push(ImplInfo {
            trait_name,
            target_name,
            target_template_args: Vec::new(),
            trait_template_args: trait_args.clone(),
            generics,
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

    /// For each blanket impl, walk every registered non-generic concrete
    /// type and test whether it satisfies the blanket's target-generic
    /// bounds. Satisfaction is a unification problem: the bound
    /// `E: Wrap<T>` matches if the candidate E has an `impl Wrap<?> for
    /// E` for some concrete `?`, which gets recorded as a binding for
    /// the free generic `T`. Bindings must stay consistent across
    /// bounds; any mismatch skips that candidate.
    ///
    /// When a candidate satisfies all bounds with some assignment, the
    /// blanket's methods are attached to the candidate with a
    /// `BlanketBinding` describing the full ordered args list (the
    /// target slot is `None`; the others are `Some(concrete)`).
    fn attach_blanket_impls(&mut self, errors: &mut Vec<TyperError>) {
        // Snapshot the blanket list — we'll mutate `self.types` below.
        let blankets = self.blanket_impls.clone();

        // Iterate to fixpoint: a blanket might rely on another blanket's
        // attachment to satisfy its bounds (transitive satisfaction). An
        // upper bound on iterations = number of blankets × types; a
        // round that attaches nothing terminates the loop.
        let max_rounds = blankets.len().saturating_add(1).saturating_mul(
            self.type_infos.len().saturating_add(1),
        );
        for _round in 0..=max_rounds {
            let before = self.all_types().map(|i| i.methods.len()).sum::<usize>();
            self.attach_blanket_pass(&blankets);
            let after = self.all_types().map(|i| i.methods.len()).sum::<usize>();
            if before == after {
                break;
            }
        }
        let _ = errors;
    }

    /// One pass of blanket attachment. Iterates all blankets × types
    /// and attaches anything newly satisfiable. Idempotent — running
    /// twice with no new satisfactions is a no-op.
    fn attach_blanket_pass(&mut self, blankets: &[ImplInfo]) {
        for blanket in blankets {
            if blanket.target_template_args.is_empty() {
                // Bare target case: `impl<T: B> Trait for T`.
                self.attach_blanket_bare(blanket);
            } else {
                // Generic-target case: `impl<T: B> Trait for Container<T>`.
                self.attach_blanket_generic_target(blanket);
            }
        }
    }

    /// `impl<T: B> Trait for T` — attach to every concrete non-generic
    /// type that satisfies the blanket's bounds. The binding sources
    /// `T` from the receiver itself (`GenericSource::Target`) and any
    /// other free generics from the bound-resolution assignment.
    fn attach_blanket_bare(&mut self, blanket: &ImplInfo) {
        let target_index = match blanket
            .generics
            .iter()
            .position(|g| g.name == blanket.target_name)
        {
            Some(i) => i,
            None => return,
        };
        let target_generic = &blanket.generics[target_index];
        let free_names: Vec<String> = blanket
            .generics
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != target_index)
            .map(|(_, g)| g.name.clone())
            .collect();
        let generic_order: Vec<String> =
            blanket.generics.iter().map(|g| g.name.clone()).collect();

        let candidates: Vec<String> = self.type_canonical_keys.clone();
        for type_name in candidates {
            if generic_order.iter().any(|n| *n == type_name) {
                continue;
            }
            // Pass the candidate's own template-param names to unify
            // as `candidate_params`. For non-generic candidates this
            // is empty; for generic ones (ArrayIter with `[T, N, F]`)
            // it's the declared names, letting unify emit positional
            // `CandidateArg(i)` bindings instead of stalling on
            // placeholder-to-placeholder matches.
            let candidate_params: Vec<String> = self
                .get_type(&type_name)
                .map(|info| info.templates.clone())
                .unwrap_or_default();
            let Some(assignment) = self.satisfy_bounds(
                &type_name,
                &target_generic.name,
                &target_generic.bounds,
                &free_names,
                &candidate_params,
            ) else {
                continue;
            };
            let sources = Self::build_sources_for_bare(
                &blanket.generics,
                target_index,
                &assignment,
            );
            let Some(sources) = sources else { continue };
            let binding = BlanketBinding {
                generic_names: generic_order.clone(),
                sources,
            };
            self.attach_blanket_method_on(&type_name, blanket, binding);
        }
    }

    /// `impl<T: B> Trait for Container<T>` — attach to `Container`'s
    /// head (generic type). Each blanket generic is sourced from a
    /// position in the receiver's template args at call time. Bound
    /// satisfaction can't be fully checked here (specific X for
    /// `Container<X>` isn't known); the monomorph catches any
    /// violation at inline time.
    fn attach_blanket_generic_target(&mut self, blanket: &ImplInfo) {
        // Container must be a registered type.
        if !self.has_type(&blanket.target_name) {
            return;
        }
        let generic_order: Vec<String> =
            blanket.generics.iter().map(|g| g.name.clone()).collect();

        // Each blanket generic is sourced from a position in the
        // target's template args. A generic that doesn't appear in the
        // target is "free" — not supported for generic-target blankets
        // in this phase (we have no X to resolve against).
        let mut sources: Vec<GenericSource> = Vec::with_capacity(blanket.generics.len());
        for g in &blanket.generics {
            let pos = blanket.target_template_args.iter().position(|n| *n == g.name);
            match pos {
                Some(i) => sources.push(GenericSource::TargetArg(i)),
                None => return, // ill-formed: skip this blanket
            }
        }

        let binding = BlanketBinding {
            generic_names: generic_order,
            sources,
        };
        // Clone target_name before the mutable borrow.
        let target = blanket.target_name.clone();
        self.attach_blanket_method_on(&target, blanket, binding);
    }

    /// Build the per-generic `GenericSource` list for a bare-target
    /// blanket. The target slot becomes `GenericSource::Target`; other
    /// slots read from the already-resolved `assignment`. Returns
    /// `None` if any free generic is missing from the assignment
    /// (indicates an ill-formed impl).
    fn build_sources_for_bare(
        generics: &[GenericParamInfo],
        target_index: usize,
        assignment: &HashMap<String, crate::resolution::ResolvedArg>,
    ) -> Option<Vec<GenericSource>> {
        let mut out = Vec::with_capacity(generics.len());
        for (i, g) in generics.iter().enumerate() {
            if i == target_index {
                out.push(GenericSource::Target);
            } else {
                match assignment.get(&g.name) {
                    Some(crate::resolution::ResolvedArg::Concrete(v)) => {
                        out.push(GenericSource::Bound(v.clone()));
                    }
                    Some(crate::resolution::ResolvedArg::CandidateArg(idx)) => {
                        out.push(GenericSource::TargetArg(*idx));
                    }
                    None => return None,
                }
            }
        }
        Some(out)
    }

    /// Attach all of `blanket.methods` to `type_name` with the given
    /// binding, skipping any method that's already provided by the
    /// same trait.
    ///
    /// The `trait_template_args` stored on each attached method is the
    /// blanket impl's trait args, rewritten to reference the candidate
    /// type's own template-param names (for generic-target blankets)
    /// or the concrete candidate (for bare-target blankets). This lets
    /// another blanket's bound check unify its free generics against
    /// what this impl "exposes."
    fn attach_blanket_method_on(
        &mut self,
        type_name: &str,
        blanket: &ImplInfo,
        binding: BlanketBinding,
    ) {
        // Look up the impl's original trait_ty args (kept on the
        // blanket impl via its methods — they're the `trait_ty.0.templates`
        // we didn't yet store. As a stand-in, reconstruct from the
        // blanket's own generics reference if possible.) For phase 1
        // we take the blanket's generics names that appear in the
        // trait args — but that info isn't easily accessible here.
        // Instead we forge trait_template_args by walking the blanket's
        // target mapping: each blanket generic's position in the target
        // tells us the candidate's template arg index. If the trait
        // head args reference blanket generic names, we replace each
        // with the candidate's template name at that index.
        let candidate_params: Vec<String> = self
            .get_type(type_name)
            .map(|info| info.templates.clone())
            .unwrap_or_default();
        let attached_trait_args = translate_trait_args_for_candidate(
            blanket,
            type_name,
            &candidate_params,
        );

        // Blanket's trait stored by name for now (see ImplInfo); fetch
        // its id once so attached methods carry a `TraitId`.
        let trait_id = match self.trait_id(&blanket.trait_name) {
            Some(id) => id,
            None => return,
        };
        let ty = self.get_type_mut(type_name).unwrap();
        for method in &blanket.methods {
            let already = ty.methods.iter().any(|m| {
                m.function.sig.name.0 == method.sig.name.0
                    && m.from_trait == Some(trait_id)
            });
            if already {
                continue;
            }
            ty.methods.push(MethodInfo {
                function: method.clone(),
                file_id: blanket.file_id,
                from_trait: Some(trait_id),
                trait_template_args: attached_trait_args.clone(),
                blanket: Some(binding.clone()),
            });
        }
    }

    /// Try to satisfy every bound on a candidate type. Returns `Some`
    /// with the assignment of free generics → concrete types on
    /// success, `None` when some bound has no matching impl or when
    /// unification finds a contradiction across bounds.
    ///
    /// When a bound has multiple matching impls (e.g., `Wrap<U4>` AND
    /// `Wrap<U8>` both for `E`), we take the first that unifies — that
    /// picks a single attachment. A future extension could emit one
    /// attachment per distinct assignment.
    fn satisfy_bounds(
        &self,
        candidate: &str,
        target_name: &str,
        bounds: &[BoundRef],
        free_names: &[String],
        candidate_params: &[String],
    ) -> Option<HashMap<String, crate::resolution::ResolvedArg>> {
        use crate::resolution::{ResolvedArg, unify_args};
        let info = self.get_type(candidate)?;
        // Treat the target generic as an additional free name, pre-bound
        // to the candidate. Bounds that mention `T` (the target) then
        // check consistency instead of treating `T` as a concrete but
        // unknown head.
        let mut all_free: Vec<String> = free_names.to_vec();
        all_free.push(target_name.to_string());
        let mut assignment: HashMap<String, ResolvedArg> = HashMap::new();
        assignment.insert(
            target_name.to_string(),
            ResolvedArg::Concrete(ast::TypeOrValue::Type(ast::Type {
                name: (candidate.to_string(), 0..0),
                templates: Vec::new(),
                qself: None,
            })),
        );
        'each_bound: for bound in bounds {
            for m in &info.methods {
                if m.from_trait != Some(bound.trait_id) {
                    continue;
                }
                // Candidate impl: try to unify bound.trait_args against
                // the concrete impl's trait_template_args. Both direct
                // impls AND blanket-attached methods participate — a
                // blanket that already landed on this type counts as
                // "the type implements this trait" for bound purposes.
                let start = assignment.clone();
                if let Some(next) = unify_args(
                    &bound.trait_args,
                    &m.trait_template_args,
                    &all_free,
                    candidate_params,
                    start,
                ) {
                    assignment = next;
                    continue 'each_bound;
                }
            }
            // No impl matched this bound.
            return None;
        }
        // Strip the target's own binding from the final assignment —
        // callers only want the FREE generics' bindings.
        assignment.remove(target_name);
        Some(assignment)
    }

    /// Locate the first `MethodInfo` on `type_name` matching both the
    /// method name AND the given trait scope. `trait_name = None`
    /// selects inherent methods (extensions); `Some(name)` selects
    /// trait-impl methods, including blanket-attached ones.
    ///
    /// This is the shared entry point for every "peek at a method's
    /// metadata" caller (blanket info, operator resolution, etc.) —
    /// keeping the walk in one place prevents drift between the rules
    /// each caller applies.
    pub fn find_method_info(
        &self,
        type_name: &str,
        method_name: &str,
        trait_name: Option<&str>,
    ) -> Option<&MethodInfo> {
        let info = self.get_type(type_name)?;
        // Convert the trait-name hint to an id once so we can compare
        // against stored `from_trait: Option<TraitId>`. A hint that
        // doesn't resolve means no match — fall through the iter.
        let trait_hint_id = match trait_name {
            Some(n) => match self.trait_id(n) {
                Some(id) => Some(Some(id)),
                None => return None,
            },
            None => Some(None),
        };
        info.methods.iter().find(|m| {
            m.function.sig.name.0 == method_name && trait_hint_id == Some(m.from_trait)
        })
    }

    /// One-shot resolver: for a method call at `file_id` on
    /// `type_name::method_name` (optionally pinned to a trait via
    /// `trait_hint`), return the trait-scope plus any blanket
    /// attachment. This is the canonical accessor that HIR gen uses to
    /// build a FnRef — centralizes every "peek at resolution state"
    /// rule in one place.
    pub fn resolve_method_dispatch(
        &self,
        file_id: FileId,
        type_name: &str,
        method_name: &str,
        trait_hint: Option<&str>,
    ) -> MethodDispatch {
        let trait_name = match self.resolve_method(file_id, type_name, method_name, trait_hint) {
            MethodResolution::Trait { trait_name } => Some(trait_name),
            _ => None,
        };
        let blanket = self.method_blanket(type_name, method_name, trait_name.as_deref());
        MethodDispatch { trait_name, blanket }
    }

    /// If `(type_name, method_name, trait_name)` matches a method that
    /// was attached via a blanket impl, return its BlanketBinding. HIR
    /// gen uses this to thread receiver + pre-resolved bindings into
    /// the Call's template_args so the monomorphizer binds every
    /// blanket generic correctly.
    pub fn method_blanket(
        &self,
        type_name: &str,
        method_name: &str,
        trait_name: Option<&str>,
    ) -> Option<BlanketBinding> {
        self.find_method_info(type_name, method_name, trait_name)?
            .blanket
            .clone()
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
        let Some(info) = self.get_type(type_name) else {
            return MethodResolution::NotFound {
                reason: format!("unknown type `{}`", type_name),
            };
        };

        // Explicit qualification: `MyTrait::my_method(args)`. Pick only
        // methods that came from `MyTrait`.
        if let Some(trait_name) = trait_hint {
            let hint_id = self.trait_id(trait_name);
            if let Some(tid) = hint_id {
                for m in &info.methods {
                    if m.function.sig.name.0 == method_name
                        && m.from_trait == Some(tid)
                    {
                        return MethodResolution::Trait {
                            trait_name: trait_name.to_string(),
                        };
                    }
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
            if let Some(tid) = m.from_trait {
                let t = &self.trait_canonical_keys[tid.0 as usize];
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
        let mut resolved: Vec<(String, Option<i64>, CellCount, Option<ast::Type>)> =
            Vec::with_capacity(count);
        let mut data_size: CellCount = 0;
        for v in &def.variants {
            let (size, data_type) = match &v.data {
                Some((ty, sp)) => (self.resolve_type_size(ty, sp)?, Some(ty.clone())),
                None => (0, None),
            };
            if size > data_size {
                data_size = size;
            }
            resolved.push((
                v.name.0.clone(),
                v.discriminant.as_ref().map(|(n, _)| *n),
                size,
                data_type,
            ));
        }

        // Assign discriminant values: explicit values taken first; gaps filled
        // with the smallest unused non-negative integer.
        let mut used: std::collections::BTreeSet<u32> = Default::default();
        for (_, discr, _, _) in &resolved {
            if let Some(d) = discr {
                if *d < 0 {
                    return Err(TyperError::new("negative enum discriminant"));
                }
                used.insert(*d as u32);
            }
        }
        let mut next_auto: u32 = 0;
        let mut variants = Vec::with_capacity(count);
        for (name, discr, size, data_type) in resolved {
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
                data_type,
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
    ///   - user-defined generic struct instantiations — substitute template
    ///     params into each field's AST type and recursively size.
    ///
    /// Other generic instantiations (e.g. generic enums) are still
    /// deferred — the monomorphizer handles those on demand.
    pub fn resolve_type_size(
        &self,
        ty: &ast::Type,
        sp: &new_parser::Span,
    ) -> Result<CellCount, TyperError> {
        // Qualified path: `<SelfTy as Trait>::Ident`. Resolve to the
        // concrete type named by the impl's template arg or associated-
        // type binding, then size *that*.
        if ty.qself.is_some() {
            let resolved = self.resolve_qualified_path(ty, sp)?;
            return self.resolve_type_size(&resolved, sp);
        }

        // Native type: Array<T, N, F> has layout N * sizeof(T).
        if ty.name.0 == "Array" && ty.templates.len() == 3 {
            return self.array_layout_size(ty, sp);
        }

        // User-defined generic struct/enum instantiation like
        // `ArrayList<U4, 4, U4>` or `Option<U4>`. Path-qualified
        // references land here too; canonicalize to the bare storage
        // name before walking.
        if !ty.templates.is_empty() {
            let canonical = self
                .canonicalize_type_name(&ty.name.0, None)
                .unwrap_or_else(|| ty.name.0.clone());
            if let Some(info) = self.get_type(&canonical) {
                match &info.kind {
                    TypeKind::Struct(StructKind::Templated { .. }) => {
                        // Normalize the name for downstream sizing.
                        let mut ty2 = ty.clone();
                        ty2.name.0 = canonical;
                        return self.resolve_struct_layout(&ty2, sp).map(|l| l.size);
                    }
                    TypeKind::Enum(EnumKind::Templated { .. }) => {
                        let mut ty2 = ty.clone();
                        ty2.name.0 = canonical;
                        return self.resolve_enum_layout(&ty2, sp).map(|l| l.total_size());
                    }
                    _ => {}
                }
            }
            return Err(TyperError::at(
                format!(
                    "cannot compute size of generic type reference `{}<...>` yet \
                     (monomorphization is Phase 6)",
                    ty.name.0
                ),
                sp.clone(),
            ));
        }
        let canonical = self
            .canonicalize_type_name(&ty.name.0, None)
            .unwrap_or_else(|| ty.name.0.clone());
        let info = self.get_type(&canonical).ok_or_else(|| {
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

    /// Walk an AST type and resolve every qualified-path occurrence (at
    /// the top level and inside nested template args) to a concrete
    /// `ast::Type` with `qself: None`. Non-qself nodes are passed through
    /// with their children recursively resolved. Errors bubble up.
    pub fn resolve_qself_deep(
        &self,
        ty: &ast::Type,
        sp: &new_parser::Span,
        self_hint: &str,
    ) -> Result<ast::Type, TyperError> {
        let resolved_head = if ty.qself.is_some() {
            self.resolve_qualified_path_with_self(ty, sp, self_hint)?
        } else {
            ty.clone()
        };
        let mut out_templates: Vec<ast::Spanned<ast::TypeOrValue>> =
            Vec::with_capacity(resolved_head.templates.len());
        for (tv, tv_sp) in &resolved_head.templates {
            let new_tv = match tv {
                ast::TypeOrValue::Value(n) => ast::TypeOrValue::Value(*n),
                ast::TypeOrValue::Type(inner) => ast::TypeOrValue::Type(
                    self.resolve_qself_deep(inner, tv_sp, self_hint)?,
                ),
            };
            out_templates.push((new_tv, tv_sp.clone()));
        }
        Ok(ast::Type {
            name: resolved_head.name,
            templates: out_templates,
            qself: None,
        })
    }

    /// Resolve a `<SelfTy as Trait>::Ident` qualified path to a concrete
    /// type reference. The trait's template parameter list names positions;
    /// `Ident` must match one of them (e.g. `Output` at position 0 for
    /// `trait Add<Output>`). The resolver looks up the impl of `Trait` for
    /// `SelfTy` and returns that impl's template arg at the matching
    /// position. Returns a plain (`qself: None`) `ast::Type` so repeated
    /// resolution is safe.
    pub fn resolve_qualified_path(
        &self,
        ty: &ast::Type,
        sp: &new_parser::Span,
    ) -> Result<ast::Type, TyperError> {
        self.resolve_qualified_path_with_self(ty, sp, "")
    }

    /// Like `resolve_qualified_path` but with an optional enclosing-type
    /// hint. When the `self_ty` inside the qself is the bare `Self` head,
    /// `self_hint` substitutes it with the enclosing type's name so we
    /// can find the matching impl.
    pub fn resolve_qualified_path_with_self(
        &self,
        ty: &ast::Type,
        sp: &new_parser::Span,
        self_hint: &str,
    ) -> Result<ast::Type, TyperError> {
        let qself = ty.qself.as_ref().ok_or_else(|| {
            TyperError::at("not a qualified path", sp.clone())
        })?;
        let assoc_name = &ty.name.0;
        let mut self_ty: ast::Type = qself.self_ty.0.clone();
        if self_ty.name.0 == "Self" && !self_hint.is_empty() {
            self_ty.name.0 = self_hint.to_string();
        }
        let self_ty = &self_ty;
        let trait_head = &qself.trait_ty.0.name.0;

        let trait_info = self.get_trait(trait_head).ok_or_else(|| {
            TyperError::at(
                format!("unknown trait `{}`", trait_head),
                qself.trait_ty.1.clone(),
            )
        })?;

        // Position of Ident in the trait's template param list. If it's
        // not there, fall back to associated_types (for future-proofing).
        let pos = trait_info
            .templates
            .iter()
            .position(|n| n == assoc_name)
            .or_else(|| {
                let off = trait_info.templates.len();
                trait_info
                    .associated_types
                    .iter()
                    .position(|n| n == assoc_name)
                    .map(|i| off + i)
            })
            .ok_or_else(|| {
                TyperError::at(
                    format!(
                        "trait `{}` has no parameter or associated type `{}`",
                        trait_head, assoc_name
                    ),
                    sp.clone(),
                )
            })?;

        // Find an impl of `Trait` for `SelfTy`.
        let target = match self_ty.qself {
            Some(_) => self.resolve_qualified_path(self_ty, sp)?,
            None => self_ty.clone(),
        };
        let impl_ = self
            .impls
            .iter()
            .find(|i| i.trait_name == *trait_head && impl_target_matches(&i.target_name, &target))
            .ok_or_else(|| {
                TyperError::at(
                    format!(
                        "no `impl {} for {}` found — can't resolve `<{} as {}>::{}`",
                        trait_head, target.name.0, target.name.0, trait_head, assoc_name
                    ),
                    sp.clone(),
                )
            })?;

        // Template-arg position → real AST type. Trait-template args live
        // on the impl's `trait_ty` that it was declared against; associated-
        // type bindings live in `associated_bindings`.
        let n_trait_tpl = trait_info.templates.len();
        if pos < n_trait_tpl {
            // Look up in the impl's methods list — trait template args
            // aren't kept directly on ImplInfo, but they're on the methods'
            // `trait_template_args`. Grab from the first method.
            let info = self.get_type(&impl_.target_name).ok_or_else(|| {
                TyperError::at(
                    format!("impl target `{}` has no type info", impl_.target_name),
                    sp.clone(),
                )
            })?;
            let head_id = self.trait_id(trait_head);
            for m in &info.methods {
                if m.from_trait == head_id
                    && !m.trait_template_args.is_empty()
                {
                    if let Some(tv) = m.trait_template_args.get(pos) {
                        if let ast::TypeOrValue::Type(t) = tv {
                            return Ok(t.clone());
                        }
                    }
                }
            }
            return Err(TyperError::at(
                format!(
                    "`impl {} for {}` has no template arg at position {}",
                    trait_head, target.name.0, pos
                ),
                sp.clone(),
            ));
        }
        // Associated-type binding.
        let assoc_idx = pos - n_trait_tpl;
        impl_
            .associated_bindings
            .get(assoc_idx)
            .map(|(_, t)| t.clone())
            .ok_or_else(|| {
                TyperError::at(
                    format!(
                        "`impl {} for {}` is missing binding for associated type `{}`",
                        trait_head, target.name.0, assoc_name
                    ),
                    sp.clone(),
                )
            })
    }

    /// Resolve a (possibly-generic) struct type reference to a concrete
    /// `StructLayout`. For non-generic structs this is a direct lookup; for
    /// generic instantiations (`ArrayList<U4, 4, U4>`), this substitutes
    /// template params in each field's AST type and recursively sizes them.
    ///
    /// Results are computed on demand and NOT cached — callers that make
    /// this lookup often (HIR gen hits it per field access) should cache
    /// at their layer. Computation is cheap: one walk per call.
    pub fn resolve_struct_layout(
        &self,
        ty: &ast::Type,
        sp: &new_parser::Span,
    ) -> Result<StructLayout, TyperError> {
        let canonical = self
            .canonicalize_type_name(&ty.name.0, None)
            .unwrap_or_else(|| ty.name.0.clone());
        // Direct concrete lookup.
        if ty.templates.is_empty() {
            let info = self.get_type(&canonical).ok_or_else(|| {
                TyperError::at(format!("unknown type `{}`", ty.name.0), sp.clone())
            })?;
            return match &info.kind {
                TypeKind::Struct(StructKind::Concrete(l)) => Ok(l.clone()),
                _ => Err(TyperError::at(
                    format!("`{}` is not a concrete struct", ty.name.0),
                    sp.clone(),
                )),
            };
        }

        // Generic instantiation — substitute and compute.
        let info = self.get_type(&canonical).ok_or_else(|| {
            TyperError::at(format!("unknown type `{}`", ty.name.0), sp.clone())
        })?;
        let fields = match &info.kind {
            TypeKind::Struct(StructKind::Templated { fields }) => fields,
            _ => {
                return Err(TyperError::at(
                    format!("`{}` is not a generic struct", ty.name.0),
                    sp.clone(),
                ))
            }
        };
        if info.templates.len() != ty.templates.len() {
            return Err(TyperError::at(
                format!(
                    "`{}` expects {} template arguments, got {}",
                    ty.name.0,
                    info.templates.len(),
                    ty.templates.len()
                ),
                sp.clone(),
            ));
        }

        // Bind template param name → concrete arg.
        let bindings: std::collections::HashMap<String, &ast::TypeOrValue> = info
            .templates
            .iter()
            .zip(ty.templates.iter())
            .map(|(name, (arg, _))| (name.clone(), arg))
            .collect();

        let mut out_fields: Vec<FieldLayout> = Vec::with_capacity(fields.len());
        let mut offset: CellCount = 0;
        for (name, field_ty) in fields {
            let substituted = subst_ast_type(field_ty, &bindings);
            let size = self.resolve_type_size(&substituted, sp)?;
            out_fields.push(FieldLayout {
                name: name.clone(),
                offset,
                size,
                ast_type: substituted,
            });
            offset += size;
        }

        Ok(StructLayout {
            fields: out_fields,
            size: offset,
        })
    }

    /// Resolve a (possibly-generic) enum type reference to a concrete
    /// `EnumLayout`. Mirrors `resolve_struct_layout` but for enums: each
    /// variant's payload AST type is substituted with the concrete
    /// template args, then sized; the enum's data_size is the max of
    /// payload sizes.
    pub fn resolve_enum_layout(
        &self,
        ty: &ast::Type,
        sp: &new_parser::Span,
    ) -> Result<EnumLayout, TyperError> {
        let canonical = self
            .canonicalize_type_name(&ty.name.0, None)
            .unwrap_or_else(|| ty.name.0.clone());
        // Direct concrete lookup.
        if ty.templates.is_empty() {
            let info = self.get_type(&canonical).ok_or_else(|| {
                TyperError::at(format!("unknown type `{}`", ty.name.0), sp.clone())
            })?;
            return match &info.kind {
                TypeKind::Enum(EnumKind::Concrete(l)) => Ok(l.clone()),
                _ => Err(TyperError::at(
                    format!("`{}` is not a concrete enum", ty.name.0),
                    sp.clone(),
                )),
            };
        }

        // Generic instantiation — substitute and compute.
        let info = self.get_type(&canonical).ok_or_else(|| {
            TyperError::at(format!("unknown type `{}`", ty.name.0), sp.clone())
        })?;
        let variants = match &info.kind {
            TypeKind::Enum(EnumKind::Templated { variants }) => variants,
            _ => {
                return Err(TyperError::at(
                    format!("`{}` is not a generic enum", ty.name.0),
                    sp.clone(),
                ))
            }
        };
        if info.templates.len() != ty.templates.len() {
            return Err(TyperError::at(
                format!(
                    "`{}` expects {} template arguments, got {}",
                    ty.name.0,
                    info.templates.len(),
                    ty.templates.len()
                ),
                sp.clone(),
            ));
        }

        let bindings: std::collections::HashMap<String, &ast::TypeOrValue> = info
            .templates
            .iter()
            .zip(ty.templates.iter())
            .map(|(name, (arg, _))| (name.clone(), arg))
            .collect();

        let count = variants.len();
        let discriminant_size = discriminant_size_for(count);

        // Resolve each variant's payload size after substitution.
        let mut resolved: Vec<(String, Option<i64>, CellCount, Option<ast::Type>)> =
            Vec::with_capacity(count);
        let mut data_size: CellCount = 0;
        for v in variants {
            let (size, data_type) = match &v.data {
                Some(data_ty) => {
                    let substituted = subst_ast_type(data_ty, &bindings);
                    (self.resolve_type_size(&substituted, sp)?, Some(substituted))
                }
                None => (0, None),
            };
            if size > data_size {
                data_size = size;
            }
            resolved.push((v.name.clone(), v.discriminant, size, data_type));
        }

        // Assign discriminant values exactly like compute_enum_layout.
        let mut used: std::collections::BTreeSet<u32> = Default::default();
        for (_, discr, _, _) in &resolved {
            if let Some(d) = discr {
                if *d < 0 {
                    return Err(TyperError::new("negative enum discriminant"));
                }
                used.insert(*d as u32);
            }
        }
        let mut next_auto: u32 = 0;
        let mut out_variants = Vec::with_capacity(count);
        for (name, discr, size, data_type) in resolved {
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
            out_variants.push(EnumVariantLayout {
                name,
                discriminant: d,
                data_size: size,
                data_type,
            });
        }

        Ok(EnumLayout {
            variants: out_variants,
            discriminant_size,
            data_size,
        })
    }

    /// Rough check: does this type reference a template parameter name, or
    /// does it instantiate a templated type? Used as a quick "can I compute
    /// this now?" gate. Concrete generic instantiations (e.g. `Option<U4>`
    /// where U4 is a known primitive) are *not* considered templated —
    /// `resolve_type_size` will succeed on them.
    fn type_is_templated(&self, ty: &ast::Type) -> bool {
        // If we can size it now, it's effectively concrete.
        if self.resolve_type_size(ty, &ty.name.1).is_ok() {
            return false;
        }
        // Otherwise: any remaining template params or unknown heads count
        // as unresolved.
        if !ty.templates.is_empty() {
            // Recurse: a concrete generic instantiation whose args are
            // all concrete types is NOT templated.
            return ty.templates.iter().any(|(tv, _)| match tv {
                ast::TypeOrValue::Value(_) => false,
                ast::TypeOrValue::Type(inner) => self.type_is_templated(inner),
            }) || matches!(
                self.get_type(&ty.name.0),
                Some(TypeInfo {
                    kind: TypeKind::Struct(StructKind::Templated { .. }),
                    ..
                }) | Some(TypeInfo {
                    kind: TypeKind::Enum(EnumKind::Templated { .. }),
                    ..
                })
            );
        }
        match self.get_type(&ty.name.0) {
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

/// Match an impl's `target_name` (a bare type head) against a concrete
/// `target` AST type. Impls are registered keyed by the head-name only, so
/// `impl Add<U4> for Pair<U4>` lives under `target_name == "Pair"`. We
/// ignore the incoming target's template args for now — callers who need
/// precise multi-instantiation dispatch can narrow further.
fn impl_target_matches(impl_target_name: &str, target: &ast::Type) -> bool {
    impl_target_name == target.name.0
}

/// Translate the trait args of a blanket impl's header into the
/// candidate's template-name namespace.
///
/// Example: `impl<T, N, F> Iter<T> for ArrayIter<T, N, F>` attached to
/// `ArrayIter` (whose own template params are `[Elem, Size, Idx]`) —
/// the blanket's `T` at target position 0 corresponds to the
/// candidate's param at position 0 (`Elem`). So the translated trait
/// args become `[Type(Elem)]`, ready for a later blanket to unify
/// against.
///
/// For bare-target blankets (`impl<T: B> Trait for T`), the blanket
/// has no target_template_args, so nothing to translate; the trait
/// args are preserved but any bare generic name references are
/// replaced with the candidate's literal name (since the "candidate"
/// IS the blanket's T at attach time).
fn translate_trait_args_for_candidate(
    blanket: &ImplInfo,
    candidate_name: &str,
    candidate_params: &[String],
) -> Vec<ast::TypeOrValue> {
    blanket
        .trait_template_args
        .iter()
        .map(|tv| translate_tv(tv, blanket, candidate_name, candidate_params))
        .collect()
}

fn translate_tv(
    tv: &ast::TypeOrValue,
    blanket: &ImplInfo,
    candidate_name: &str,
    candidate_params: &[String],
) -> ast::TypeOrValue {
    match tv {
        ast::TypeOrValue::Value(n) => ast::TypeOrValue::Value(*n),
        ast::TypeOrValue::Type(ty) => {
            // Bare generic name reference: does it match one of the
            // blanket's declared generics? If yes, translate via the
            // target-template-args → candidate_params position mapping.
            if ty.templates.is_empty() && ty.qself.is_none() {
                // Bare-target case: if the name is the blanket's target
                // (e.g. T for `impl<T> ... for T`), use the candidate
                // itself as the substitution.
                if ty.name.0 == blanket.target_name
                    && blanket.target_template_args.is_empty()
                {
                    return ast::TypeOrValue::Type(ast::Type {
                        name: (candidate_name.to_string(), ty.name.1.clone()),
                        templates: Vec::new(),
                        qself: None,
                    });
                }
                // Generic-target case: the blanket generic appears at
                // target_template_args[i]; the candidate's template
                // param at position i is the translated name.
                if let Some(i) =
                    blanket.target_template_args.iter().position(|n| *n == ty.name.0)
                {
                    if let Some(cand_name) = candidate_params.get(i) {
                        return ast::TypeOrValue::Type(ast::Type {
                            name: (cand_name.clone(), ty.name.1.clone()),
                            templates: Vec::new(),
                            qself: None,
                        });
                    }
                }
            }
            // Concrete head — recurse into template args.
            ast::TypeOrValue::Type(ast::Type {
                name: ty.name.clone(),
                templates: ty
                    .templates
                    .iter()
                    .map(|(inner, sp)| {
                        (
                            translate_tv(inner, blanket, candidate_name, candidate_params),
                            sp.clone(),
                        )
                    })
                    .collect(),
                qself: ty.qself.clone(),
            })
        }
    }
}

// Unification / structural-eq helpers live in `typer::resolution`
// and are imported there at their call sites (satisfy_bounds etc.).
// No top-level re-exports — keeping the use lines local makes it
// obvious which helper each function reaches for.

/// Substitute template parameter references in an AST type with concrete
/// bindings. Used by `resolve_struct_layout` to specialize a generic
/// struct's field types to a concrete instantiation. The hir crate has a
/// sister helper (`hir::monomorph::subst_type`); this one is kept
/// self-contained so the typer doesn't depend on hir.
fn subst_ast_type(
    ty: &ast::Type,
    bindings: &std::collections::HashMap<String, &ast::TypeOrValue>,
) -> ast::Type {
    // If the type's head is a bound template param AND it's referenced
    // with no further template args of its own, replace wholesale.
    if ty.templates.is_empty() {
        if let Some(ast::TypeOrValue::Type(t)) = bindings.get(&ty.name.0) {
            return t.clone();
        }
    }
    // Otherwise recurse into template args.
    let templates = ty
        .templates
        .iter()
        .map(|(tv, sp)| {
            let new_tv = match tv {
                ast::TypeOrValue::Type(inner) => {
                    // Leaf template-param references can resolve to either
                    // Types or Values, so check the bindings directly
                    // before recursing.
                    if inner.templates.is_empty() {
                        if let Some(bound) = bindings.get(&inner.name.0) {
                            match bound {
                                ast::TypeOrValue::Type(t) => {
                                    return (ast::TypeOrValue::Type(t.clone()), sp.clone())
                                }
                                ast::TypeOrValue::Value(n) => {
                                    return (ast::TypeOrValue::Value(*n), sp.clone())
                                }
                            }
                        }
                    }
                    ast::TypeOrValue::Type(subst_ast_type(inner, bindings))
                }
                ast::TypeOrValue::Value(n) => ast::TypeOrValue::Value(*n),
            };
            (new_tv, sp.clone())
        })
        .collect();
    ast::Type {
        name: ty.name.clone(),
        templates,
        qself: ty.qself.clone(),
    }
}

