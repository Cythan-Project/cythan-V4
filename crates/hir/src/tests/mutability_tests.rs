//! Step 4.6: mutability checking during HIR generation.

use crate::tests::try_compile;

#[test]
fn write_to_immutable_self_rejected() {
    // `fn bad(self) { self = 5; }` — self is immutable.
    let err = try_compile(
        r#"
        extension U4 {
            fn bad(self) { self = 5; }
        }
        "#,
        "U4",
        "bad",
    )
    .unwrap_err();
    assert!(err.contains("immutable") || err.contains("cannot write"));
}

#[test]
fn write_to_mut_self_ok() {
    let hir = try_compile(
        r#"
        extension U4 {
            fn good(mut self) { self = 5; }
        }
        "#,
        "U4",
        "good",
    );
    assert!(hir.is_ok(), "unexpected: {:?}", hir);
}

#[test]
fn write_to_immutable_local_rejected() {
    let err = try_compile(
        r#"
        extension U4 {
            fn bad(): U4 {
                U4 x = 1;
                x = 2;
                x
            }
        }
        "#,
        "U4",
        "bad",
    )
    .unwrap_err();
    assert!(err.contains("immutable"));
}

#[test]
fn write_to_mut_local_ok() {
    let hir = try_compile(
        r#"
        extension U4 {
            fn good(): U4 {
                mut U4 x = 1;
                x = 2;
                x
            }
        }
        "#,
        "U4",
        "good",
    );
    assert!(hir.is_ok(), "unexpected: {:?}", hir);
}

#[test]
fn compound_assign_on_immutable_rejected() {
    let err = try_compile(
        r#"
        extension U4 {
            fn bad(): U4 {
                U4 x = 0;
                x += 1;
                x
            }
        }
        "#,
        "U4",
        "bad",
    )
    .unwrap_err();
    assert!(err.contains("immutable"));
}

#[test]
fn compound_assign_on_mut_local_emits_inc() {
    use crate::tests::compile;
    use crate::ir::*;
    let hir = compile(
        r#"
        extension U4 {
            fn run(): U4 {
                mut U4 x = 0;
                x += 1;
                x
            }
        }
        "#,
        "U4",
        "run",
    );
    assert!(
        hir.body.ops.iter().any(|op| matches!(op, HirOp::Inc(_))),
        "expected Inc for `x += 1`"
    );
}
