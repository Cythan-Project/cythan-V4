//! Unit tests for individual parser productions.
//!
//! Each test follows the same shape:
//! 1. build the expected AST by hand using `fixtures::*` helpers,
//! 2. parse the source,
//! 3. strip spans from both sides and assert structural equality.
//!
//! These tests were written *before* the parser was implemented; they define
//! the parser's contract.

use crate::ast::*;
use crate::parse;
use crate::tests::fixtures::*;

// ---------- helpers ---------------------------------------------------------

fn parse_ok(src: &str) -> Vec<Spanned<Item>> {
    match parse(src) {
        Ok(items) => items,
        Err(e) => panic!("parse failed: {:?}\nsource:\n{}", e, src),
    }
}

fn parse_single_item(src: &str) -> Item {
    let items = parse_ok(src);
    assert_eq!(items.len(), 1, "expected one item, got {}", items.len());
    items.into_iter().next().unwrap().0
}

fn assert_item_eq(src: &str, mut expected: Item) {
    let mut got = parse_single_item(src);
    got.strip_spans();
    expected.strip_spans();
    assert_eq!(got, expected, "\nparsed source:\n{}", src);
}

fn parse_expr_in_body(src: &str) -> Expr {
    // Wrap the expression in a tiny function to reuse the parser.
    let wrapped = format!("extension T {{ fn _f() {{ {} }} }}", src);
    let mut item = parse_single_item(&wrapped);
    let Item::Extension(ref mut ext) = item else {
        panic!("expected extension");
    };
    let m = ext.methods.remove(0).0;
    let mut stmts = m.body.0.stmts;
    assert_eq!(stmts.len(), 1, "expected exactly one expression in body");
    let mut e = stmts.remove(0).0;
    e.strip_spans();
    e
}

fn assert_expr_eq(src: &str, mut expected: Expr) {
    let got = parse_expr_in_body(src);
    expected.strip_spans();
    assert_eq!(got, expected, "\nparsed expression:\n{}", src);
}

// ---------- type parsing ----------------------------------------------------

#[test]
fn test_parse_type_simple() {
    // struct WrappedU4 { U4 value }
    let expected = Item::Struct(StructDef {
        name: ident_sp("WrappedU4"),
        templates: vec![],
        fields: vec![FieldDef {
            ty: ty("U4"),
            name: ident_sp("value"),
        }],
    });
    assert_item_eq("struct WrappedU4 { U4 value, }", expected);
}

#[test]
fn test_parse_type_generic() {
    // struct Box { Array<U4, 9, U4> grid }
    let expected = Item::Struct(StructDef {
        name: ident_sp("Box"),
        templates: vec![],
        fields: vec![FieldDef {
            ty: ty_g(
                "Array",
                vec![tv_t("U4"), tv_v(9), tv_t("U4")],
            ),
            name: ident_sp("grid"),
        }],
    });
    assert_item_eq("struct Box { Array<U4, 9, U4> grid, }", expected);
}

#[test]
fn test_parse_type_nested_generic() {
    // struct Outer { Array<Option<U8>, 4, U4> val }
    let expected = Item::Struct(StructDef {
        name: ident_sp("Outer"),
        templates: vec![],
        fields: vec![FieldDef {
            ty: ty_g(
                "Array",
                vec![
                    tv_g("Option", vec![tv_t("U8")]),
                    tv_v(4),
                    tv_t("U4"),
                ],
            ),
            name: ident_sp("val"),
        }],
    });
    assert_item_eq("struct Outer { Array<Option<U8>, 4, U4> val, }", expected);
}

// ---------- struct parsing --------------------------------------------------

#[test]
fn test_parse_struct_empty() {
    let expected = Item::Struct(StructDef {
        name: ident_sp("U4"),
        templates: vec![],
        fields: vec![],
    });
    assert_item_eq("struct U4 {}", expected);
}

#[test]
fn test_parse_struct_fields() {
    let expected = Item::Struct(StructDef {
        name: ident_sp("U8"),
        templates: vec![],
        fields: vec![
            FieldDef {
                ty: ty("U4"),
                name: ident_sp("lower"),
            },
            FieldDef {
                ty: ty("U4"),
                name: ident_sp("higher"),
            },
        ],
    });
    assert_item_eq("struct U8 { U4 lower, U4 higher, }", expected);
}

#[test]
fn test_parse_struct_templates() {
    let expected = Item::Struct(StructDef {
        name: ident_sp("Array"),
        templates: vec![ident_sp("T"), ident_sp("E"), ident_sp("F")],
        fields: vec![],
    });
    assert_item_eq("struct Array<T, E, F> {}", expected);
}

// ---------- enum parsing ----------------------------------------------------

#[test]
fn test_parse_enum_unit() {
    let expected = Item::Enum(EnumDef {
        name: ident_sp("Cell"),
        templates: vec![],
        variants: vec![
            EnumVariant {
                name: ident_sp("Empty"),
                data: None,
                discriminant: None,
            },
            EnumVariant {
                name: ident_sp("O"),
                data: None,
                discriminant: None,
            },
            EnumVariant {
                name: ident_sp("X"),
                data: None,
                discriminant: None,
            },
        ],
    });
    assert_item_eq("enum Cell { Empty, O, X, }", expected);
}

#[test]
fn test_parse_enum_mixed() {
    let expected = Item::Enum(EnumDef {
        name: ident_sp("Option"),
        templates: vec![ident_sp("T")],
        variants: vec![
            EnumVariant {
                name: ident_sp("None"),
                data: None,
                discriminant: None,
            },
            EnumVariant {
                name: ident_sp("Some"),
                data: Some(ty("T")),
                discriminant: None,
            },
        ],
    });
    assert_item_eq("enum Option<T> { None, Some(T), }", expected);
}

#[test]
fn test_parse_enum_explicit_discr() {
    let expected = Item::Enum(EnumDef {
        name: ident_sp("TypeMap"),
        templates: vec![],
        variants: vec![
            EnumVariant {
                name: ident_sp("A"),
                data: None,
                discriminant: Some(sp(0)),
            },
            EnumVariant {
                name: ident_sp("B"),
                data: None,
                discriminant: Some(sp(1)),
            },
            EnumVariant {
                name: ident_sp("C"),
                data: Some(ty("U4")),
                discriminant: Some(sp(2)),
            },
            EnumVariant {
                name: ident_sp("D"),
                data: None,
                discriminant: None,
            },
        ],
    });
    assert_item_eq(
        "enum TypeMap { A = 0, B = 1, C(U4) = 2, D, }",
        expected,
    );
}

// ---------- function parsing ------------------------------------------------

fn single_method(src: &str) -> Function {
    let item = parse_single_item(src);
    let Item::Extension(ext) = item else {
        panic!("expected extension");
    };
    assert_eq!(ext.methods.len(), 1);
    ext.methods.into_iter().next().unwrap().0
}

#[test]
fn test_parse_fn_no_params() {
    let m = single_method("extension U4 { fn zero(): Self { 0 } }");
    let mut sig = m.sig.clone();
    sig.strip_spans();
    assert_eq!(sig.name.0, "zero");
    assert_eq!(sig.params.len(), 0);
    let rt = sig.return_type.unwrap().0;
    assert_eq!(rt.name.0, "Self");
}

#[test]
fn test_parse_fn_self() {
    let m = single_method("extension Bool { fn not(self): Self {} }");
    assert_eq!(m.sig.params.len(), 1);
    let p = &m.sig.params[0];
    assert!(p.is_self);
    assert!(p.ty.is_none());
    assert!(!p.mutable);
    assert_eq!(p.name.0, "self");
}

#[test]
fn test_parse_fn_mut_self() {
    let m = single_method("extension Morpion { fn play(mut self) {} }");
    assert_eq!(m.sig.params.len(), 1);
    let p = &m.sig.params[0];
    assert!(p.is_self);
    assert!(p.mutable);
}

#[test]
fn test_parse_fn_params() {
    let m = single_method(
        "extension Morpion { fn set(mut self, U4 pos, Cell val) {} }",
    );
    let params = &m.sig.params;
    assert_eq!(params.len(), 3);
    assert!(params[0].is_self && params[0].mutable);
    assert_eq!(params[1].name.0, "pos");
    assert_eq!(params[1].ty.as_ref().unwrap().0.name.0, "U4");
    assert!(!params[1].mutable);
    assert_eq!(params[2].name.0, "val");
    assert_eq!(params[2].ty.as_ref().unwrap().0.name.0, "Cell");
}

#[test]
fn test_parse_fn_mut_param() {
    let m = single_method("extension U4 { fn sub(mut self, mut Self other) {} }");
    let params = &m.sig.params;
    assert_eq!(params.len(), 2);
    assert!(params[0].is_self && params[0].mutable);
    assert!(!params[1].is_self && params[1].mutable);
    assert_eq!(params[1].name.0, "other");
    assert_eq!(params[1].ty.as_ref().unwrap().0.name.0, "Self");
}

#[test]
fn test_parse_fn_template() {
    let m = single_method("extension Array<T, E, F> { fn get<N>(self): T {} }");
    let tpls: Vec<_> = m.sig.templates.iter().map(|t| t.0.clone()).collect();
    assert_eq!(tpls, vec!["N".to_string()]);
}

#[test]
fn test_parse_fn_self_specialized() {
    // Self<U8, E, F> self — Array.ct uses this shape
    let m = single_method(
        "extension Array<T, E, F> { fn print(Self<U8, E, F> self) {} }",
    );
    assert_eq!(m.sig.params.len(), 1);
    let p = &m.sig.params[0];
    assert!(p.is_self);
    let ty = p.ty.as_ref().unwrap();
    assert_eq!(ty.0.name.0, "Self");
    assert_eq!(ty.0.templates.len(), 3);
}

// ---------- extension / trait / impl ---------------------------------------

#[test]
fn test_parse_extension_simple() {
    let item = parse_single_item("extension U4 { fn zero(): Self { 0 } }");
    let Item::Extension(ext) = item else {
        panic!("expected extension");
    };
    assert_eq!(ext.target.0.name.0, "U4");
    assert_eq!(ext.target.0.templates.len(), 0);
    assert_eq!(ext.methods.len(), 1);
}

#[test]
fn test_parse_extension_generic() {
    let item = parse_single_item(
        "extension Array<T, E, F> { fn set<N>(mut self, T value) {} }",
    );
    let Item::Extension(ext) = item else {
        panic!("expected extension");
    };
    assert_eq!(ext.target.0.name.0, "Array");
    assert_eq!(ext.target.0.templates.len(), 3);
    assert_eq!(ext.methods.len(), 1);
}

#[test]
fn test_parse_trait() {
    let item = parse_single_item(
        "trait Eq { fn eq(self, Self other): Bool; }",
    );
    let Item::Trait(t) = item else {
        panic!("expected trait");
    };
    assert_eq!(t.name.0, "Eq");
    assert_eq!(t.methods.len(), 1);
    assert_eq!(t.methods[0].0.name.0, "eq");
}

#[test]
fn test_parse_trait_with_assoc_types() {
    let item = parse_single_item(
        "trait Add { type Other; type Result; fn add(self, Self::Other other): Self::Result; }",
    );
    let Item::Trait(t) = item else {
        panic!("expected trait");
    };
    let assoc: Vec<_> = t.associated_types.iter().map(|s| s.0.clone()).collect();
    assert_eq!(assoc, vec!["Other".to_string(), "Result".to_string()]);
}

#[test]
fn test_parse_impl() {
    let item = parse_single_item(
        "impl Eq for Cell { fn eq(self, Cell other): Bool { true } }",
    );
    let Item::Impl(i) = item else {
        panic!("expected impl");
    };
    assert_eq!(i.trait_ty.0.name.0, "Eq");
    assert_eq!(i.target.0.name.0, "Cell");
    assert_eq!(i.methods.len(), 1);
}

#[test]
fn test_parse_impl_with_assoc_types() {
    let item = parse_single_item(
        "impl Add for U8 { type Other = U8; type Result = U8; fn add(self, U8 other): U8 { self } }",
    );
    let Item::Impl(i) = item else {
        panic!("expected impl");
    };
    assert_eq!(i.associated_types.len(), 2);
    assert_eq!(i.associated_types[0].0 .0, "Other");
    assert_eq!(i.associated_types[0].1 .0.name.0, "U8");
}

#[test]
fn test_parse_const() {
    let item = parse_single_item("const U4 MAX = 15;");
    let Item::Const(c) = item else {
        panic!("expected const");
    };
    assert_eq!(c.name.0, "MAX");
    assert_eq!(c.ty.0.name.0, "U4");
    assert_eq!(c.value.0, Expr::Number(15));
}

// ---------- expressions -----------------------------------------------------

#[test]
fn test_parse_expr_number() {
    assert_expr_eq("42", Expr::Number(42));
}

#[test]
fn test_parse_expr_bool() {
    assert_expr_eq("true", Expr::Bool(true));
    assert_expr_eq("false", Expr::Bool(false));
}

#[test]
fn test_parse_expr_string_char() {
    assert_expr_eq("\"hi\"", Expr::String("hi".into()));
    assert_expr_eq("'a'", Expr::Char('a'));
}

#[test]
fn test_parse_expr_variable() {
    assert_expr_eq("count", Expr::Variable("count".into()));
}

#[test]
fn test_parse_expr_self() {
    assert_expr_eq("self", Expr::SelfValue);
}

#[test]
fn test_parse_expr_field() {
    let expected = Expr::Field(
        Box::new(sp(Expr::SelfValue)),
        ident_sp("grid"),
    );
    assert_expr_eq("self.grid", expected);
}

#[test]
fn test_parse_expr_method_call_no_args() {
    let expected = Expr::MethodCall {
        receiver: Box::new(sp(Expr::SelfValue)),
        name: ident_sp("display"),
        templates: vec![],
        args: vec![],
    };
    assert_expr_eq("self.display()", expected);
}

#[test]
fn test_parse_expr_method_call_chained() {
    // self.grid.getDyn(pos)
    let expected = Expr::MethodCall {
        receiver: Box::new(sp(Expr::Field(
            Box::new(sp(Expr::SelfValue)),
            ident_sp("grid"),
        ))),
        name: ident_sp("getDyn"),
        templates: vec![],
        args: vec![sp(Expr::Variable("pos".into()))],
    };
    assert_expr_eq("self.grid.getDyn(pos)", expected);
}

#[test]
fn test_parse_expr_static_call() {
    let expected = Expr::StaticCall {
        ty: ty("Self"),
        name: ident_sp("new"),
        templates: vec![],
        args: vec![],
    };
    assert_expr_eq("Self::new()", expected);
}

#[test]
fn test_parse_expr_template_call() {
    // self.get<0>()
    let expected = Expr::MethodCall {
        receiver: Box::new(sp(Expr::SelfValue)),
        name: ident_sp("get"),
        templates: vec![tv_v(0)],
        args: vec![],
    };
    assert_expr_eq("self.get<0>()", expected);
}

#[test]
fn test_parse_expr_static_template_call() {
    // System::setRegister<0>(2)
    let expected = Expr::StaticCall {
        ty: ty("System"),
        name: ident_sp("setRegister"),
        templates: vec![tv_v(0)],
        args: vec![sp(Expr::Number(2))],
    };
    assert_expr_eq("System::setRegister<0>(2)", expected);
}

#[test]
fn test_parse_expr_struct_literal() {
    let expected = Expr::StructLiteral {
        ty: ty("Self"),
        fields: vec![
            (ident_sp("lower"), sp(Expr::Variable("a".into()))),
            (ident_sp("higher"), sp(Expr::Number(0))),
        ],
    };
    assert_expr_eq("Self { lower: a, higher: 0, }", expected);
}

#[test]
fn test_parse_expr_enum_variant_unit() {
    let expected = Expr::EnumVariant {
        ty: ty("Self"),
        variant: ident_sp("None"),
        data: None,
    };
    assert_expr_eq("Self::None", expected);
}

#[test]
fn test_parse_expr_enum_variant_data() {
    let expected = Expr::EnumVariant {
        ty: ty("Self"),
        variant: ident_sp("Some"),
        data: Some(Box::new(sp(Expr::Variable("x".into())))),
    };
    assert_expr_eq("Self::Some(x)", expected);
}

#[test]
fn test_parse_expr_cast() {
    let expected = Expr::Cast {
        expr: Box::new(sp(Expr::SelfValue)),
        ty: ty("U4"),
    };
    assert_expr_eq("self as U4", expected);
}

#[test]
fn test_parse_expr_comparison() {
    let expected = Expr::BinaryOp(
        BinOp::EqEq,
        Box::new(sp(Expr::Variable("a".into()))),
        Box::new(sp(Expr::Variable("b".into()))),
    );
    assert_expr_eq("a == b", expected);
}

#[test]
fn test_parse_expr_arithmetic() {
    let expected = Expr::BinaryOp(
        BinOp::Add,
        Box::new(sp(Expr::Variable("a".into()))),
        Box::new(sp(Expr::Number(1))),
    );
    assert_expr_eq("a + 1", expected);
}

#[test]
fn test_parse_expr_bool_ops_precedence() {
    // a && b || c  --> (a && b) || c
    let expected = Expr::BinaryOp(
        BinOp::Or,
        Box::new(sp(Expr::BinaryOp(
            BinOp::And,
            Box::new(sp(Expr::Variable("a".into()))),
            Box::new(sp(Expr::Variable("b".into()))),
        ))),
        Box::new(sp(Expr::Variable("c".into()))),
    );
    assert_expr_eq("a && b || c", expected);
}

#[test]
fn test_parse_expr_string_method() {
    let expected = Expr::MethodCall {
        receiver: Box::new(sp(Expr::String("hello".into()))),
        name: ident_sp("print"),
        templates: vec![],
        args: vec![],
    };
    assert_expr_eq("\"hello\".print()", expected);
}

#[test]
fn test_parse_expr_char_method() {
    let expected = Expr::MethodCall {
        receiver: Box::new(sp(Expr::Char('\n'))),
        name: ident_sp("print"),
        templates: vec![],
        args: vec![],
    };
    assert_expr_eq("'\\n'.print()", expected);
}

// ---------- control flow ---------------------------------------------------

#[test]
fn test_parse_expr_if_else() {
    // if cond { a } else { b }
    let expected = Expr::If {
        cond: Box::new(sp(Expr::Variable("cond".into()))),
        then: Box::new(block(vec![sp(Expr::Variable("a".into()))])),
        else_: Some(Box::new(sp(Expr::Block(Box::new(block(vec![sp(
            Expr::Variable("b".into()),
        )])))))),
    };
    assert_expr_eq("if cond { a } else { b }", expected);
}

#[test]
fn test_parse_expr_if_no_else() {
    let expected = Expr::If {
        cond: Box::new(sp(Expr::Variable("cond".into()))),
        then: Box::new(block(vec![sp(Expr::Variable("a".into()))])),
        else_: None,
    };
    assert_expr_eq("if cond { a }", expected);
}

#[test]
fn test_parse_expr_if_else_if() {
    // if a { x } else if b { y } else { z }
    let expected = Expr::If {
        cond: Box::new(sp(Expr::Variable("a".into()))),
        then: Box::new(block(vec![sp(Expr::Variable("x".into()))])),
        else_: Some(Box::new(sp(Expr::If {
            cond: Box::new(sp(Expr::Variable("b".into()))),
            then: Box::new(block(vec![sp(Expr::Variable("y".into()))])),
            else_: Some(Box::new(sp(Expr::Block(Box::new(block(vec![sp(
                Expr::Variable("z".into()),
            )])))))),
        }))),
    };
    assert_expr_eq("if a { x } else if b { y } else { z }", expected);
}

#[test]
fn test_parse_expr_loop() {
    let expected = Expr::Loop(Box::new(block(vec![sp(Expr::Break)])));
    assert_expr_eq("loop { break; }", expected);
}

#[test]
fn test_parse_expr_return() {
    let expected = Expr::Return(Some(Box::new(sp(Expr::Bool(false)))));
    assert_expr_eq("return false;", expected);
}

#[test]
fn test_parse_expr_return_empty() {
    let expected = Expr::Return(None);
    assert_expr_eq("return;", expected);
}

#[test]
fn test_parse_expr_break_continue() {
    assert_expr_eq("break;", Expr::Break);
    assert_expr_eq("continue;", Expr::Continue);
}

#[test]
fn test_parse_expr_let() {
    let expected = Expr::Declaration {
        mutable: false,
        ty: ty("U4"),
        name: ident_sp("x"),
        value: Box::new(sp(Expr::Number(5))),
    };
    assert_expr_eq("U4 x = 5;", expected);
}

#[test]
fn test_parse_expr_let_mut() {
    let expected = Expr::Declaration {
        mutable: true,
        ty: ty("U4"),
        name: ident_sp("x"),
        value: Box::new(sp(Expr::Number(5))),
    };
    assert_expr_eq("mut U4 x = 5;", expected);
}

#[test]
fn test_parse_expr_assign() {
    let expected = Expr::Assign {
        target: Box::new(sp(Expr::Variable("x".into()))),
        value: Box::new(sp(Expr::Number(5))),
    };
    assert_expr_eq("x = 5;", expected);
}

#[test]
fn test_parse_expr_compound_assign_add() {
    let expected = Expr::CompoundAssign {
        op: CompoundOp::AddAssign,
        target: Box::new(sp(Expr::Variable("x".into()))),
        value: Box::new(sp(Expr::Number(1))),
    };
    assert_expr_eq("x += 1;", expected);
}

#[test]
fn test_parse_expr_compound_assign_sub() {
    let expected = Expr::CompoundAssign {
        op: CompoundOp::SubAssign,
        target: Box::new(sp(Expr::Variable("x".into()))),
        value: Box::new(sp(Expr::Number(10))),
    };
    assert_expr_eq("x -= 10;", expected);
}

#[test]
fn test_parse_expr_match_simple() {
    // match self { Self::None => true, Self::Some(_) => false, }
    let expected = Expr::Match {
        scrutinee: Box::new(sp(Expr::SelfValue)),
        arms: vec![
            MatchArm {
                pattern: sp(Pattern::Variant {
                    ty: ty("Self"),
                    variant: ident_sp("None"),
                    binding: None,
                }),
                body: sp(Expr::Bool(true)),
            },
            MatchArm {
                pattern: sp(Pattern::Variant {
                    ty: ty("Self"),
                    variant: ident_sp("Some"),
                    binding: Some(sp(PatternBinding::Wildcard)),
                }),
                body: sp(Expr::Bool(false)),
            },
        ],
    };
    assert_expr_eq(
        "match self { Self::None => true, Self::Some(_) => false, }",
        expected,
    );
}

#[test]
fn test_parse_expr_match_wildcard() {
    let expected = Expr::Match {
        scrutinee: Box::new(sp(Expr::SelfValue)),
        arms: vec![
            MatchArm {
                pattern: sp(Pattern::Variant {
                    ty: ty("Self"),
                    variant: ident_sp("Empty"),
                    binding: None,
                }),
                body: sp(Expr::Bool(true)),
            },
            MatchArm {
                pattern: sp(Pattern::Wildcard),
                body: sp(Expr::Bool(false)),
            },
        ],
    };
    assert_expr_eq(
        "match self { Self::Empty => true, _ => false, }",
        expected,
    );
}
