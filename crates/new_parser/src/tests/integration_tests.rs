//! Integration tests: parse every file in `examples/new_syntax/` and assert
//! structural properties of the resulting AST.
//!
//! The acceptance criteria for Phase 1 is that all of these tests pass.

use std::path::PathBuf;

use crate::ast::*;
use crate::parse;

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/new_syntax")
}

fn load(path: &str) -> String {
    let full = example_dir().join(path);
    std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e))
        .replace('\r', "")
}

fn parse_file(path: &str) -> Vec<Spanned<Item>> {
    let src = load(path);
    match parse(&src) {
        Ok(items) => items,
        Err(e) => panic!("parse of {} failed:\n{:?}", path, e),
    }
}

fn find_struct<'a>(items: &'a [Spanned<Item>], name: &str) -> &'a StructDef {
    items
        .iter()
        .find_map(|(it, _)| match it {
            Item::Struct(s) if s.name.0 == name => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no struct named {}", name))
}

fn find_enum<'a>(items: &'a [Spanned<Item>], name: &str) -> &'a EnumDef {
    items
        .iter()
        .find_map(|(it, _)| match it {
            Item::Enum(e) if e.name.0 == name => Some(e),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no enum named {}", name))
}

fn find_extension<'a>(items: &'a [Spanned<Item>], target: &str) -> &'a ExtensionDef {
    items
        .iter()
        .find_map(|(it, _)| match it {
            Item::Extension(e) if e.target.0.name.0 == target => Some(e),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no extension targeting {}", target))
}

fn method_names(ext: &ExtensionDef) -> Vec<String> {
    ext.methods.iter().map(|m| m.0.sig.name.0.clone()).collect()
}

#[test]
fn test_parse_u4() {
    let items = parse_file("std/U4.ct");
    let s = find_struct(&items, "U4");
    assert_eq!(s.fields.len(), 0);
    assert_eq!(s.templates.len(), 0);

    let ext = find_extension(&items, "U4");
    let names = method_names(ext);
    for expected in ["zero", "input", "equals", "greater", "sub", "printDec", "print"] {
        assert!(names.iter().any(|n| n == expected), "missing method {}", expected);
    }
}

#[test]
fn test_parse_bool() {
    let items = parse_file("std/Bool.ct");
    let s = find_struct(&items, "Bool");
    assert_eq!(s.fields.len(), 1);
    assert_eq!(s.fields[0].name.0, "value");
    assert_eq!(s.fields[0].ty.0.name.0, "U4");

    let ext = find_extension(&items, "Bool");
    let names = method_names(ext);
    assert!(names.contains(&"not".to_string()));
    assert!(names.contains(&"print".to_string()));
}

#[test]
fn test_parse_u8() {
    let items = parse_file("std/U8.ct");
    let s = find_struct(&items, "U8");
    assert_eq!(s.fields.len(), 2);
    let field_names: Vec<_> = s.fields.iter().map(|f| f.name.0.clone()).collect();
    assert_eq!(field_names, vec!["lower", "higher"]);

    let ext = find_extension(&items, "U8");
    let names = method_names(ext);
    for expected in [
        "new", "zero", "inc", "dec", "add", "sub", "fromU4", "fromU4AsNumber",
        "input", "print", "equals", "equalsZero", "printDec", "debug",
    ] {
        assert!(names.iter().any(|n| n == expected), "missing {}", expected);
    }
}

#[test]
fn test_parse_system() {
    let items = parse_file("std/System.ct");
    let _ = find_struct(&items, "System");

    let ext = find_extension(&items, "System");
    let names = method_names(ext);
    for expected in ["setRegister", "getRegister", "debug", "debugType", "debugInterupt"] {
        assert!(names.iter().any(|n| n == expected), "missing {}", expected);
    }

    // setRegister<N>(U4) — templated
    let set_reg = ext.methods.iter().find(|m| m.0.sig.name.0 == "setRegister").unwrap();
    assert_eq!(set_reg.0.sig.templates.len(), 1);
    assert_eq!(set_reg.0.sig.templates[0].0, "N");
}

#[test]
fn test_parse_array() {
    let items = parse_file("std/Array.ct");
    let s = find_struct(&items, "Array");
    assert_eq!(s.fields.len(), 0);
    let tpls: Vec<_> = s.templates.iter().map(|t| t.0.clone()).collect();
    assert_eq!(tpls, vec!["T", "E", "F"]);

    let ext = find_extension(&items, "Array");
    // Extension target has templates
    assert_eq!(ext.target.0.templates.len(), 3);
    let names = method_names(ext);
    for expected in ["set", "setDyn", "get", "getDyn", "len", "print", "println", "contains"] {
        assert!(names.iter().any(|n| n == expected), "missing {}", expected);
    }
}

#[test]
fn test_parse_option() {
    let items = parse_file("std/Option.ct");
    let e = find_enum(&items, "Option");
    assert_eq!(e.templates.len(), 1);
    assert_eq!(e.variants.len(), 2);
    assert_eq!(e.variants[0].name.0, "None");
    assert!(e.variants[0].data.is_none());
    assert_eq!(e.variants[1].name.0, "Some");
    assert!(e.variants[1].data.is_some());

    let ext = find_extension(&items, "Option");
    let names = method_names(ext);
    for expected in ["none", "some", "is_none"] {
        assert!(names.iter().any(|n| n == expected), "missing {}", expected);
    }

    // Verify is_none contains a Match expression
    let is_none = ext.methods.iter().find(|m| m.0.sig.name.0 == "is_none").unwrap();
    let body = &is_none.0.body.0.stmts;
    assert!(matches!(body[0].0, Expr::Match { .. }));
}

#[test]
fn test_parse_dynarray() {
    let items = parse_file("std/DynArray.ct");
    let s = find_struct(&items, "DynArray");
    assert_eq!(s.fields.len(), 2);
    assert_eq!(s.fields[0].name.0, "array");
    assert_eq!(s.fields[0].ty.0.name.0, "Array");
    assert_eq!(s.fields[1].name.0, "length");

    let ext = find_extension(&items, "DynArray");
    let names = method_names(ext);
    for expected in [
        "new", "from", "add", "addAll", "pop", "len", "capacity",
        "getDyn", "setDyn", "get", "set", "last", "println", "contains",
    ] {
        assert!(names.iter().any(|n| n == expected), "missing {}", expected);
    }
}

#[test]
fn test_parse_morpion() {
    let items = parse_file("Morpion.ct");

    let cell = find_enum(&items, "Cell");
    assert_eq!(cell.variants.len(), 3);
    let cell_names: Vec<_> = cell.variants.iter().map(|v| v.name.0.clone()).collect();
    assert_eq!(cell_names, vec!["Empty", "O", "X"]);

    let _ = find_extension(&items, "Cell");

    // trait Eq
    let has_eq_trait = items.iter().any(|(it, _)| matches!(it, Item::Trait(t) if t.name.0 == "Eq"));
    assert!(has_eq_trait, "expected trait Eq");

    // impl Eq for Cell
    let eq_impl = items
        .iter()
        .find_map(|(it, _)| match it {
            Item::Impl(i) if i.trait_ty.0.name.0 == "Eq" && i.target.0.name.0 == "Cell" => Some(i),
            _ => None,
        })
        .expect("expected impl Eq for Cell");
    assert_eq!(eq_impl.methods.len(), 1);

    let morpion = find_struct(&items, "Morpion");
    assert_eq!(morpion.fields.len(), 1);
    assert_eq!(morpion.fields[0].name.0, "grid");

    let ext = find_extension(&items, "Morpion");
    let names = method_names(ext);
    for expected in ["new", "set", "getDyn", "get", "display", "play", "winner", "main"] {
        assert!(names.iter().any(|n| n == expected), "missing {}", expected);
    }

    let main = ext.methods.iter().find(|m| m.0.sig.name.0 == "main").unwrap();
    assert_eq!(main.0.sig.return_type.as_ref().unwrap().0.name.0, "U4");
}

#[test]
fn test_parse_all_new_syntax_files_no_errors() {
    let mut count = 0;
    let mut stack = vec![example_dir()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().map_or(false, |e| e == "ct") {
                let src = std::fs::read_to_string(&path).unwrap().replace('\r', "");
                let rel = path.strip_prefix(example_dir()).unwrap().display().to_string();
                match parse(&src) {
                    Ok(items) => assert!(!items.is_empty(), "no items in {}", rel),
                    Err(e) => panic!("parse of {} failed: {:?}", rel, e),
                }
                count += 1;
            }
        }
    }
    assert!(count >= 8, "expected at least 8 files, got {}", count);
}
