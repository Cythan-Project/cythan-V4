//! Core data structures for the type registry.
//!
//! Everything is sized in "cells" (4-bit slots). The primitive `U4` is 1 cell;
//! composite types are the sum/max of their components.

use new_parser::ast::{self};

/// Size measured in u4 cells.
pub type CellCount = u32;

/// File identifier for future import scoping. For now, just a monotonically
/// increasing number; Phase 2 only needs to carry it through.
pub type FileId = u32;

/// Opaque handle for a registered type. Index into `TypeRegistry.type_infos`.
///
/// Created via `TypeRegistry::insert_type` / resolved via `type_id(name)`.
/// Handles are stable for the registry's lifetime — migrating a type's name
/// on cross-file collision rewrites the name map, NOT the `TypeInfo`'s slot,
/// so any `TypeId` held elsewhere keeps pointing at the right data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeId(pub u32);

/// Opaque handle for a registered trait. Index into
/// `TypeRegistry.trait_infos`. Same stability guarantees as `TypeId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TraitId(pub u32);

/// The only hard-coded primitive.
///
/// `U4` is declared in source as `struct U4 {}` (zero fields) but is special-
/// cased here as a 1-cell primitive. Every other composite type gets its size
/// from its fields / variants.
pub const U4_SIZE: CellCount = 1;

/// Name of the primitive type.
pub const U4_NAME: &str = "U4";

/// Everything the registry knows about a single named type (struct or enum).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeInfo {
    pub name: String,
    /// Template parameter names, e.g. `["T", "E", "F"]`. Empty if concrete.
    pub templates: Vec<String>,
    pub kind: TypeKind,
    /// Methods attached via `extension` blocks. Populated by Step 2.4.
    pub methods: Vec<MethodInfo>,
    /// Source span of the type's declared name (the `Foo` in
    /// `struct Foo {}`). Used by diagnostics to point at the original
    /// declaration when a duplicate is rejected. `None` for the
    /// pre-seeded `U4` primitive.
    pub decl_span: Option<new_parser::Span>,
    /// File in which the type was declared. Paired with `decl_span`
    /// when emitting cross-file "first defined here" labels.
    pub decl_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    /// Hardcoded primitive (currently only `U4`).
    Primitive { size: CellCount },
    Struct(StructKind),
    Enum(EnumKind),
}

/// A struct is either fully concrete (known size, field offsets) or
/// "templated" — the fields reference template params and the layout depends
/// on the monomorphization. Templated structs store raw field types; concrete
/// structs store a computed `StructLayout`.
#[derive(Debug, Clone, PartialEq)]
pub enum StructKind {
    Concrete(StructLayout),
    Templated {
        /// Raw fields `(name, ast::Type)` in declaration order.
        fields: Vec<(String, ast::Type)>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructLayout {
    pub fields: Vec<FieldLayout>,
    pub size: CellCount,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldLayout {
    pub name: String,
    pub offset: CellCount,
    pub size: CellCount,
    /// Original AST type of the field. Kept so downstream passes (HIR gen,
    /// the Array monomorphizer) can recover concrete template args like
    /// `Array<Cell, 9, U4>` from a field reference.
    pub ast_type: ast::Type,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnumKind {
    Concrete(EnumLayout),
    Templated {
        /// Raw variants in declaration order.
        variants: Vec<TemplatedVariant>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplatedVariant {
    pub name: String,
    pub data: Option<ast::Type>,
    pub discriminant: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumLayout {
    pub variants: Vec<EnumVariantLayout>,
    /// Size in cells of the discriminant field (1 or 2).
    pub discriminant_size: CellCount,
    /// Size in cells reserved for variant payload; 0 if all variants are unit.
    pub data_size: CellCount,
}

impl EnumLayout {
    pub fn total_size(&self) -> CellCount {
        self.discriminant_size + self.data_size
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariantLayout {
    pub name: String,
    /// Resolved discriminant value (either explicit or auto-assigned).
    pub discriminant: u32,
    /// Size in cells of this variant's payload (0 for unit variants).
    pub data_size: CellCount,
    /// Original AST type of the payload, if any. Kept so downstream
    /// passes (HIR match lowering) can recover a concrete type name for
    /// pattern bindings — `data_size` alone can't distinguish `U8` (2)
    /// from a `Pair<U4>` (also 2).
    pub data_type: Option<ast::Type>,
}

/// Smallest discriminant size in cells that can fit `variant_count` distinct
/// values. ≤16 → 1 cell (u4), ≤256 → 2 cells (u8).
pub fn discriminant_size_for(variant_count: usize) -> CellCount {
    if variant_count <= 16 {
        1
    } else if variant_count <= 256 {
        2
    } else {
        // Beyond 256 variants is out of language scope for now.
        panic!("enum has {} variants — max 256 supported", variant_count);
    }
}

/// A method attached to a type via an extension (or impl) block.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodInfo {
    pub function: ast::Function,
    pub file_id: FileId,
    /// If this method came from a trait impl, carries the trait id.
    /// Resolved at registration time; `None` for inherent methods.
    pub from_trait: Option<TraitId>,
    /// Concrete template args on the trait head of the impl — e.g. for
    /// `impl Convert<U4> for U4`, this is `[U4]`. Empty for inherent
    /// extensions and for impls of a non-generic trait. Used to
    /// distinguish multiple `impl Convert<T>` for the same target type.
    pub trait_template_args: Vec<ast::TypeOrValue>,
    /// If this method was attached via a blanket `impl<T, E: Wrap<T>>
    /// MyTrait for E { ... }`, carries the per-instantiation bindings
    /// that the post-pass resolved. HIR gen uses it to thread the
    /// receiver's concrete type into the target slot at each call site.
    pub blanket: Option<BlanketBinding>,
}

/// Per-attachment data for a blanket impl method. Produced by the typer
/// post-pass when a concrete type satisfies the blanket's target bounds.
///
/// One entry per declared blanket generic, in order. Each entry describes
/// where the binding should come from at call time. Unifies the two
/// structural cases:
///   1. `impl<T: Foo> Bar for T` — target is a bare generic; `T` binds
///      to the receiver's full concrete type.
///   2. `impl<T: Foo> Bar for Container<T>` — target is a generic
///      instantiation; `T` binds to one of the receiver type's
///      template args.
/// plus:
///   3. `impl<T, E: Wrap<T>> Bar for E` — `T` is "free"; resolved from
///      the target's bound impls at attachment time and pre-bound.
#[derive(Debug, Clone, PartialEq)]
pub struct BlanketBinding {
    /// Names of the blanket's generics, mirroring `sources`. Stored for
    /// diagnostics and name-based lookups.
    pub generic_names: Vec<String>,
    /// Where each generic binding comes from at dispatch time.
    pub sources: Vec<GenericSource>,
}

/// Source of a single blanket generic's value at call time.
#[derive(Debug, Clone, PartialEq)]
pub enum GenericSource {
    /// The receiver's full concrete type — for `impl<T> Trait for T`,
    /// `T` at the call site IS the receiver.
    Target,
    /// The receiver's i-th template arg — for `impl<T> Trait for
    /// Container<T>`, `T` comes from `receiver_args[i]`.
    TargetArg(usize),
    /// Pre-bound at attachment time from satisfying the blanket's
    /// bounds (a "free" generic appearing in bounds but not the target).
    Bound(ast::TypeOrValue),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitInfo {
    pub name: String,
    pub templates: Vec<String>,
    pub associated_types: Vec<String>,
    pub methods: Vec<ast::FunctionSig>,
    pub decl_span: Option<new_parser::Span>,
    pub decl_file: Option<String>,
}

/// A concrete `impl Trait for Type { ... }` block.
#[derive(Debug, Clone, PartialEq)]
pub struct ImplInfo {
    pub trait_name: String,
    pub target_name: String,
    /// Template args on the target type. For `impl<T> Trait for Container<T>`
    /// this is `[T]`; for a bare target `impl Trait for Foo` or
    /// `impl<T> Trait for T` this is empty. Each entry is a generic
    /// param name — validated by `register_impl`.
    pub target_template_args: Vec<String>,
    /// Raw template args on the trait reference as written in the
    /// impl header — preserved in AST form so blanket attachments can
    /// translate them to candidate-template names (for downstream
    /// bound-unification by other blankets).
    pub trait_template_args: Vec<ast::TypeOrValue>,
    /// Generic parameters declared on the impl header. Non-empty only
    /// for blanket impls (`impl<T: A + B> Trait for T`), which are also
    /// stored separately in `TypeRegistry.blanket_impls`.
    pub generics: Vec<GenericParamInfo>,
    pub associated_bindings: Vec<(String, ast::Type)>,
    pub methods: Vec<ast::Function>,
    pub file_id: FileId,
}

/// Template parameter on an impl header with its trait bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericParamInfo {
    pub name: String,
    /// Bounds: each one a trait reference possibly carrying template
    /// args that reference other generic params. For `T: IndexedGet +
    /// Length`, this is `[{IndexedGet, []}, {Length, []}]`. For
    /// `E: Wrap<T>`, this is `[{Wrap, [T]}]` where `T` may resolve to a
    /// free generic declared earlier on the same impl header.
    pub bounds: Vec<BoundRef>,
}

/// A single trait bound on a generic parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundRef {
    /// Resolved id of the bound trait. Validated at registration —
    /// unknown traits error there instead of at use sites.
    pub trait_id: TraitId,
    /// Template args on the bound's trait head — may reference free
    /// generics by name (e.g. `T` in `Wrap<T>`).
    pub trait_args: Vec<ast::TypeOrValue>,
}

/// Unified result of resolving a method call — the trait it dispatches
/// through (if any) plus, for blanket-attached methods, the per-generic
/// `BlanketBinding` the HIR generator needs to build the callee's
/// template args.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodDispatch {
    pub trait_name: Option<String>,
    pub blanket: Option<BlanketBinding>,
}

// ---- error type -----------------------------------------------------------

/// Result of resolving a method call against a type's method list, taking
/// the calling file's trait-import scope into account.
#[derive(Debug, Clone, PartialEq)]
pub enum MethodResolution {
    /// The call dispatches to an inherent method defined in an `extension`.
    Inherent,
    /// The call dispatches to a trait method. `trait_name` identifies the
    /// specific trait impl selected — the key bit of info the monomorphizer
    /// will need to keep two traits with the same method name distinct.
    Trait { trait_name: String },
    /// More than one in-scope trait provides this method.
    Ambiguous { candidates: Vec<String> },
    /// The method exists on the type, but only via unimported traits. The
    /// diagnostic can suggest `use <candidate>;`.
    TraitNotImported { candidates: Vec<String> },
    /// No such method anywhere.
    NotFound { reason: String },
}

/// Typer-pass error.
///
/// `message` and `span` are the quick-and-dirty form kept for
/// back-compat with the many call sites that emit terse errors.
/// `diagnostic`, when present, carries the richer form — error code,
/// multiple labels, notes/helps — that renderers and LSPs use. New
/// error sites should populate `diagnostic`; `.into_diagnostic()`
/// upgrades old-form errors into a structured one on demand.
#[derive(Debug, Clone, PartialEq)]
pub struct TyperError {
    pub message: String,
    pub span: Option<new_parser::Span>,
    pub diagnostic: Option<errors::Diagnostic>,
}

impl TyperError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: None,
            diagnostic: None,
        }
    }

    pub fn at(message: impl Into<String>, span: new_parser::Span) -> Self {
        Self {
            message: message.into(),
            span: Some(span),
            diagnostic: None,
        }
    }

    /// Build a typer error from a structured `Diagnostic`. The plain
    /// `message`/`span` fields mirror the diagnostic's header and
    /// primary label so legacy callers keep working.
    pub fn from_diagnostic(diag: errors::Diagnostic) -> Self {
        let span = diag.primary_label().map(|l| l.span.range.clone());
        let message = diag.message.clone();
        Self { message, span, diagnostic: Some(diag) }
    }

    /// Return a structured `Diagnostic` — if the error was built from
    /// one, return it directly; otherwise synthesize a minimal
    /// diagnostic from `message` and `span`. `file` is attached to
    /// the synthesized span so renderers can place it in a source.
    pub fn into_diagnostic(self, file: &str) -> errors::Diagnostic {
        if let Some(d) = self.diagnostic {
            return d;
        }
        let mut d = errors::Diagnostic::error(self.message);
        if let Some(range) = self.span {
            d = d.with_primary(errors::FileSpan::new(file, range), "");
        }
        d
    }
}

impl std::fmt::Display for TyperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

