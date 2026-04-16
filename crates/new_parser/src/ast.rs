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
    pub trait_ty: Spanned<Type>,
    pub target: Spanned<Type>,
    pub associated_types: Vec<(Spanned<String>, Spanned<Type>)>,
    pub methods: Vec<Spanned<Function>>,
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
#[derive(Debug, Clone, PartialEq)]
pub struct Type {
    pub name: Spanned<String>,
    pub templates: Vec<Spanned<TypeOrValue>>,
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
    Break,
    Continue,
    Return(Option<Box<Spanned<Expr>>>),
    Match {
        scrutinee: Box<Spanned<Expr>>,
        arms: Vec<MatchArm>,
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
    Wildcard,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternBinding {
    Name(String),
    Wildcard,
}
