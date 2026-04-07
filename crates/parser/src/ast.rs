use crate::Span;

pub type Spanned<T> = (T, Span);

#[derive(Debug, Clone)]
pub struct Class {
    pub name: Spanned<String>,
    pub annotations: Vec<Annotation>,
    pub template: Option<Vec<Spanned<String>>>,
    pub superclass: Option<Type>,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
}

#[derive(Debug, Clone)]
pub struct Method {
    pub name: Spanned<String>,
    pub annotations: Vec<Annotation>,
    pub return_type: Option<Type>,
    pub template: Option<Vec<Spanned<String>>>,
    pub args: Vec<(Type, Spanned<String>)>,
    pub body: Block,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: Spanned<String>,
    pub ty: Type,
    pub annotations: Vec<Annotation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Type {
    pub name: Spanned<String>,
    pub template: Option<Vec<Type>>,
}

#[derive(Debug, Clone)]
pub struct Annotation {
    pub name: Spanned<String>,
    /// Raw tokens inside the annotation parentheses.
    /// Stored as raw tokens because annotation syntax may differ from expression syntax.
    pub raw_args: Option<Vec<Spanned<crate::token::Token>>>,
}

pub type Block = Vec<Spanned<Expr>>;

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Number(i64),
    Variable(String),
    StringLit(String),
    CharLit(char),
    Bool(bool),

    Field {
        source: Box<Spanned<Expr>>,
        name: Spanned<String>,
    },
    MethodCall {
        source: Box<Spanned<Expr>>,
        name: Spanned<String>,
        template: Option<Vec<Type>>,
        args: Vec<Spanned<Expr>>,
    },
    BinaryOp {
        lhs: Box<Spanned<Expr>>,
        op: BinOp,
        rhs: Box<Spanned<Expr>>,
    },

    If {
        cond: Box<Spanned<Expr>>,
        then: Block,
        else_: Option<Block>,
    },
    Loop(Block),
    Break,
    Continue,
    Return(Option<Box<Spanned<Expr>>>),

    Assign {
        target: Box<Spanned<Expr>>,
        value: Box<Spanned<Expr>>,
    },
    Cast {
        expr: Box<Spanned<Expr>>,
        ty: Type,
    },
    New {
        ty: Type,
        fields: Vec<(Spanned<String>, Spanned<Expr>)>,
    },
    Declaration {
        ty: Type,
        name: Spanned<String>,
        value: Option<Box<Spanned<Expr>>>,
    },
    ArrayLit(Vec<Spanned<Expr>>),
    TypeExpr(Type),
}
