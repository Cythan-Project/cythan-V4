use crate::Span;

pub type Spanned<T> = (T, Span);

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Struct(StructDef),
    Enum(EnumDef),
    Extension(ExtensionDef),
    Trait(TraitDef),
    Impl(ImplDef),
    Const(ConstDef),
    /// `use Name;` — brings a trait (or type) name into scope for the file.
    /// The name is the trait (or type) identifier; a full path form like
    /// `use foo::Bar;` isn't supported yet.
    Use(UseDef),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UseDef {
    pub name: Spanned<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    pub name: Spanned<String>,
    pub templates: Vec<Spanned<String>>,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub ty: Spanned<Type>,
    pub name: Spanned<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDef {
    pub name: Spanned<String>,
    pub templates: Vec<Spanned<String>>,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: Spanned<String>,
    pub data: Option<Spanned<Type>>,
    pub discriminant: Option<Spanned<i64>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionDef {
    pub target: Spanned<Type>,
    pub methods: Vec<Spanned<Function>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitDef {
    pub name: Spanned<String>,
    pub templates: Vec<Spanned<String>>,
    pub associated_types: Vec<Spanned<String>>,
    pub methods: Vec<Spanned<FunctionSig>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImplDef {
    /// Generic parameters on the impl header: `impl<T: A + B, U: C>`.
    /// Empty for a regular `impl Trait for Type`. When non-empty, this
    /// impl is a "blanket impl" — the compiler attaches its methods to
    /// every concrete type satisfying the bounds.
    pub generics: Vec<GenericParam>,
    pub trait_ty: Spanned<Type>,
    pub target: Spanned<Type>,
    pub associated_types: Vec<(Spanned<String>, Spanned<Type>)>,
    pub methods: Vec<Spanned<Function>>,
}

/// A generic parameter in an impl header: `T: Bound1 + Bound2`. `bounds`
/// is empty when no bounds are specified (`impl<T> ...`).
#[derive(Debug, Clone, PartialEq)]
pub struct GenericParam {
    pub name: Spanned<String>,
    pub bounds: Vec<Spanned<Type>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstDef {
    pub ty: Spanned<Type>,
    pub name: Spanned<String>,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSig {
    pub name: Spanned<String>,
    pub templates: Vec<Spanned<String>>,
    pub params: Vec<Param>,
    pub return_type: Option<Spanned<Type>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub sig: FunctionSig,
    pub body: Spanned<Block>,
}

/// A function parameter. Shapes:
/// - `self`                 → is_self=true, ty=None, mutable=false
/// - `mut self`             → is_self=true, ty=None, mutable=true
/// - `Self<...> self`       → is_self=true, ty=Some(Self<...>), mutable=false
/// - `Type name`            → is_self=false, ty=Some(Type), mutable=false
/// - `mut Type name`        → is_self=false, ty=Some(Type), mutable=true
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: Spanned<String>,
    pub ty: Option<Spanned<Type>>,
    pub mutable: bool,
    pub is_self: bool,
}

/// A type reference. `name` is the path (e.g. `"U4"`, `"Array"`, `"Self"`, `"Self::Output"`).
/// For associated-type references (`Self::Output`), the whole dotted path lives in `name`.
///
/// When the type is written with a qualified-path prefix
/// (`<SelfTy as Trait>::Ident`), `qself` carries the `SelfTy` and the trait
/// reference. The `Ident` tail ends up in `name` — e.g. for
/// `<U4 as Add>::Output`, `name.0 == "Output"` and `qself` is
/// `Some({ self_ty: U4, trait_ty: Add })`.
#[derive(Debug, Clone, PartialEq)]
pub struct Type {
    pub name: Spanned<String>,
    pub templates: Vec<Spanned<TypeOrValue>>,
    pub qself: Option<Box<QSelf>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QSelf {
    pub self_ty: Spanned<Type>,
    pub trait_ty: Spanned<Type>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeOrValue {
    Type(Type),
    Value(i64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Spanned<Expr>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Number(i64),
    String(String),
    Char(char),
    Bool(bool),
    SelfValue,
    Variable(String),

    Field(Box<Spanned<Expr>>, Spanned<String>),
    MethodCall {
        receiver: Box<Spanned<Expr>>,
        name: Spanned<String>,
        templates: Vec<Spanned<TypeOrValue>>,
        args: Vec<Spanned<Expr>>,
    },
    StaticCall {
        ty: Spanned<Type>,
        name: Spanned<String>,
        templates: Vec<Spanned<TypeOrValue>>,
        args: Vec<Spanned<Expr>>,
    },

    StructLiteral {
        ty: Spanned<Type>,
        fields: Vec<(Spanned<String>, Spanned<Expr>)>,
    },
    /// `Type::Variant` (unit) or `Type::Variant(expr)` (data, with no `(...)` distinction from
    /// StaticCall already handled by caller).
    EnumVariant {
        ty: Spanned<Type>,
        variant: Spanned<String>,
        data: Option<Box<Spanned<Expr>>>,
    },

    BinaryOp(BinOp, Box<Spanned<Expr>>, Box<Spanned<Expr>>),

    If {
        cond: Box<Spanned<Expr>>,
        then: Box<Spanned<Block>>,
        else_: Option<Box<Spanned<Expr>>>,
    },
    Loop(Box<Spanned<Block>>),
    /// `for TYPE NAME in ITER { BODY }`. Lowered in HIR-gen — the
    /// iter expression's type is needed to wire `Iter::next` calls,
    /// and that's a typer concern. The AST keeps the user-written
    /// shape verbatim.
    For {
        var_ty: Spanned<Type>,
        var_name: Spanned<String>,
        iter: Box<Spanned<Expr>>,
        body: Box<Spanned<Block>>,
    },
    Break,
    Continue,
    Return(Option<Box<Spanned<Expr>>>),
    Match {
        scrutinee: Box<Spanned<Expr>>,
        arms: Vec<MatchArm>,
    },
    /// `start..end` (exclusive) or `start..=end` (inclusive). HIR-gen
    /// lowers to `Range::new(start, end)` / `RangeInclusive::new(...)`
    /// once the element type is fixed by surrounding context.
    Range {
        start: Box<Spanned<Expr>>,
        end: Box<Spanned<Expr>>,
        inclusive: bool,
    },
    Block(Box<Spanned<Block>>),

    Declaration {
        mutable: bool,
        ty: Spanned<Type>,
        name: Spanned<String>,
        value: Box<Spanned<Expr>>,
    },
    Assign {
        target: Box<Spanned<Expr>>,
        value: Box<Spanned<Expr>>,
    },
    CompoundAssign {
        op: CompoundOp,
        target: Box<Spanned<Expr>>,
        value: Box<Spanned<Expr>>,
    },

    Cast {
        expr: Box<Spanned<Expr>>,
        ty: Spanned<Type>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    And,
    Or,
    EqEq,
    NotEq,
    Gt,
    Lt,
    GtEq,
    LtEq,
    Add,
    Sub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundOp {
    AddAssign,
    SubAssign,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Spanned<Pattern>,
    pub body: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// `Type::Variant` or `Type::Variant(binding)` or `Type::Variant(_)`
    Variant {
        ty: Spanned<Type>,
        variant: Spanned<String>,
        binding: Option<Spanned<PatternBinding>>,
    },
    /// `0`, `1`, ..., a literal numeric pattern. Used for matching on
    /// raw cell-sized values (U4, Bool) against constant discriminants.
    Integer(u8),
    /// `start..=end` (inclusive) numeric range.
    Range(u8, u8),
    /// `p1 | p2 | ...` — alternation. Inner patterns must be numeric
    /// (Integer / Range / nested Or); flattened in HIR-gen.
    Or(Vec<Spanned<Pattern>>),
    Wildcard,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternBinding {
    Name(String),
    Wildcard,
}
