use std::time::Instant;

use crate::{actions::test_context::TestContext, compile, run};

// TODO: Create test using Annotations
/*
@Test("Test 1", "test,\ntest")
*/
fn execute(file: &str, input: &str, output: &str) {
    let (opt, ctx) = time("run_optimized", || {
        run(
            &{
                let a = time("compile_optimized", || compile(file.to_owned(), true));
                a
            },
            TestContext::new(input),
        )
    });
    let prt = ctx.lock().unwrap().print.clone();
    if prt != output {
        println!("Expected: {:?}", output);
        println!("Found: {:?}", prt);
        panic!("Test failed for optimized invalid output");
    }
    let (normal, ctx) = time("run_unoptimized", || {
        run(
            &time("compile_unoptimized", || {
                compile(file.to_owned(), false)
            }),
            TestContext::new(input),
        )
    });
    let prt = ctx.lock().unwrap().print.clone();
    if prt != output {
        println!("Expected: {:?}", output);
        println!("Found: {:?}", prt);
        panic!("Test failed for unoptimized invalid output");
    }
    println!(
        "opt: {}ops | uopt {}ops ({}% improvement)",
        get_format(opt),
        get_format(normal),
        normal * 100 / opt
    );
    println!("uopt {}ops", get_format(normal));
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

pub fn get_format(n: usize) -> String {
    if n > 1_000_000 {
        format!("{}M", (n / 100_000) as f64 / 10.0)
    } else if n > 1_000 {
        format!("{}K", (n / 100) as f64 / 10.0)
    } else {
        n.to_string()
    }
}
