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
    /// If this method came from a trait impl, carries the trait name.
    pub from_trait: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitInfo {
    pub name: String,
    pub templates: Vec<String>,
    pub associated_types: Vec<String>,
    pub methods: Vec<ast::FunctionSig>,
}

/// A concrete `impl Trait for Type { ... }` block.
#[derive(Debug, Clone, PartialEq)]
pub struct ImplInfo {
    pub trait_name: String,
    pub target_name: String,
    pub associated_bindings: Vec<(String, ast::Type)>,
    pub methods: Vec<ast::Function>,
    pub file_id: FileId,
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

#[derive(Debug, Clone, PartialEq)]
pub struct TyperError {
    pub message: String,
    pub span: Option<new_parser::Span>,
}

impl TyperError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: None,
        }
    }

    pub fn at(message: impl Into<String>, span: new_parser::Span) -> Self {
        Self {
            message: message.into(),
            span: Some(span),
        }
    }
}

impl std::fmt::Display for TyperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

