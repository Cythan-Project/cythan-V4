use std::time::Instant;

use crate::{actions::test_context::TestContext, compile, actions::run_context::run_with_limit};

// TODO: Create test using Annotations
/*
@Test("Test 1", "test,\ntest")
*/
const MAX_STEPS: usize = 100_000_000;

fn execute(file: &str, input: &str, output: &str) {
    let (normal, ctx) = time("run_unoptimized", || {
        run_with_limit(
            &time("compile_unoptimized", || {
                compile(file.to_owned(), false)
            }),
            TestContext::new(input),
            MAX_STEPS,
        )
    });
    let prt = ctx.lock().unwrap().print.clone();
    if prt != output {
        println!("Expected: {:?}", output);
        println!("Found: {:?}", prt);
        panic!("Test failed for unoptimized invalid output");
    }
    println!("{}ops", get_format(normal));
}
#[test]
pub fn run_test_morpion() {
    execute("Morpion","1234567", "---\n---\n---\nO--\n---\n---\nOX-\n---\n---\nOXO\n---\n---\nOXO\nX--\n---\nOXO\nXO-\n---\nOXO\nXOX\n---\nOXO\nXOX\nO--\nO won!\n");
    execute("Morpion","956787821122189576321456987", "---\n---\n---\n---\n---\n--O\n---\n-X-\n--O\n---\n-XO\n--O\n---\n-XO\nX-O\n---\n-XO\nXOO\nInvalid input!\nInvalid input!\n-X-\n-XO\nXOO\nOX-\n-XO\nXOO\nInvalid input!\nInvalid input!\nInvalid input!\nInvalid input!\nInvalid input!\nInvalid input!\nInvalid input!\nInvalid input!\nInvalid input!\nOXX\n-XO\nXOO\nX won!\n");
    execute("Morpion", "123546789", "---\n---\n---\nO--\n---\n---\nOX-\n---\n---\nOXO\n---\n---\nOXO\n-X-\n---\nOXO\nOX-\n---\nOXO\nOXX\n---\nOXO\nOXX\nO--\nO won!\n");
    execute("Morpion", "123547698", "---\n---\n---\nO--\n---\n---\nOX-\n---\n---\nOXO\n---\n---\nOXO\n-X-\n---\nOXO\nOX-\n---\nOXO\nOX-\nX--\nOXO\nOXO\nX--\nOXO\nOXO\nX-X\nOXO\nOXO\nXOX\nEquality!\n");
}

#[test]
pub fn run_test_pendu() {
    execute("Pendu","gramire", "\n\n\n\n------\n\n\n_________\n\n\n\n\n\n------\n\n\ng________\n\n\n\n\n\n------\n\n\ngr_____r_\n\n\n\n\n\n------\n\n\ngra__a_r_\n\n\n\n\n\n------\n\n\ngramma_r_\n\n\n\n\n\n------\n\n\ngrammair_\n\n\n\n\n\n------\n\n\ngrammair_\n\nVous avez gagné!\n");
    execute("Pendu","migrare", "\n\n\n\n------\n\n\n_________\n\n\n\n\n\n------\n\n\n___mm____\n\n\n\n\n\n------\n\n\n___mm_i__\n\n\n\n\n\n------\n\n\ng__mm_i__\n\n\n\n\n\n------\n\n\ngr_mm_ir_\n\n\n\n\n\n------\n\n\ngrammair_\n\n\n\n\n\n------\n\n\ngrammair_\n\nVous avez gagné!\n");
    execute("Pendu", "hhhhhhhhhhhhhhhhhh", "\n\n\n\n------\n\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |\n |\n |\n |\n------\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |--\n |\n |\n |\n------\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n |  |\n |\n------\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n | /|\n |\n------\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n | /|\\\n |\n------\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n | /|\\\n | / \\\n------\n\n_________\n\nGROSSE MERDE!\n");
    execute("Pendu", "graghhmirei", "\n\n\n\n------\n\n\n_________\n\n\n\n\n\n------\n\n\ng________\n\n\n\n\n\n------\n\n\ngr_____r_\n\n\n\n\n\n------\n\n\ngra__a_r_\n\n\n\n\n\n------\n\n\ngra__a_r_\n\nTu n'as pas trouvé de lettre -1 vie\n |\n |\n |\n |\n------\n\ngra__a_r_\n\nTu n'as pas trouvé de lettre -1 vie\n |--\n |\n |\n |\n------\n\ngra__a_r_\n\n |--\n |\n |\n |\n------\n\ngramma_r_\n\n |--\n |\n |\n |\n------\n\ngrammair_\n\n |--\n |\n |\n |\n------\n\ngrammair_\n\nVous avez gagné!\n");
    execute("Pendu", "gramihjkkkjkjkhjkhjkre", "\n\n\n\n------\n\n\n_________\n\n\n\n\n\n------\n\n\ng________\n\n\n\n\n\n------\n\n\ngr_____r_\n\n\n\n\n\n------\n\n\ngra__a_r_\n\n\n\n\n\n------\n\n\ngramma_r_\n\n\n\n\n\n------\n\n\ngrammair_\n\nTu n'as pas trouvé de lettre -1 vie\n |\n |\n |\n |\n------\n\ngrammair_\n\nTu n'as pas trouvé de lettre -1 vie\n |--\n |\n |\n |\n------\n\ngrammair_\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n |  |\n |\n------\n\ngrammair_\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n | /|\n |\n------\n\ngrammair_\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n | /|\\\n |\n------\n\ngrammair_\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n | /|\\\n | / \\\n------\n\ngrammair_\n\nGROSSE MERDE!\n");
}

pub fn time<T>(legend: &str, f: impl FnOnce() -> T) -> T {
    let instant = Instant::now();
    let t = f();
    println!("{} done in {:?}", legend, instant.elapsed());
    t
}

// === Targeted feature tests ===

#[test]
pub fn test_string() {
    execute("TestString", "", "hello\nworld\nA\ntest 123\n");
}

#[test]
pub fn test_val() {
    execute("TestVal", "", "05\n65\nyes\nno\neq5\nneq3\ngt3\nngt5\nngt8\n12\n");
}

#[test]
pub fn test_bool() {
    execute("TestBool", "", "true\nfalse\nfalse\ntrue\na_true\nb_false\ntt\nntf\nor_ba\nnor_bb\n");
}

#[test]
pub fn test_byte() {
    execute("TestByte", "", "zero_ok\ninc_ok\ndec_ok\nnz_ok\nadd_ok\nsub_ok\n5\n");
}

#[test]
pub fn test_cast() {
    execute("TestCast", "", "0_is_true\n1_is_false\n0\n1\n");
}

#[test]
pub fn test_loop() {
    execute("TestLoop", "", "01234\n124578\n00 01 02 10 11 12 20 21 22 \n");
}

#[test]
pub fn test_expr() {
    execute("TestExpr", "", "5\n9\n7\n4\n");
}

#[test]
pub fn test_shadow() {
    execute("TestShadow", "", "37\n\x02\n95\n");
}

#[test]
pub fn test_class() {
    execute("TestClass", "", "3,5\n8\n9\n1,2\n");
}

#[test]
pub fn test_array() {
    execute("TestArray", "", "159\n3\n8\nhas5\nno6\n");
}

#[test]
pub fn test_option() {
    execute("TestOption", "", "none_ok\nsome_ok\n7\nbool_true\n");
}

#[test]
pub fn test_template() {
    execute("TestTemplate", "", "3\nbtrue\ncnone\n2\narr_true\n");
}

#[test]
pub fn test_dyn_array() {
    execute("TestDynArray", "", "371\nlen3\n1\nlen2\nhas7\nno9\n");
}

#[test]
pub fn test_nested() {
    execute("TestNested", "", "ok\n46\nempty_ok\n");
}

#[test]
pub fn test_io() {
    execute("TestIO", "AB", "enter:\n1\n2\n");
}

pub fn get_format(n: usize) -> String {
    if n > 1_000_000 {
        format!("{}M", (n / 100_000) as f64 / 10.0)
    } else if n > 1_000 {
        format!("{}K", (n / 100) as f64 / 10.0)
    } else {
        n.to_string()
    }
}
