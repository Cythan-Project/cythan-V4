use std::time::Instant;

use mir::MemoryState;
use crate::{actions::test_context::TestContext, compile};

// TODO: Create test using Annotations
/*
@Test("Test 1", "test,\ntest")
*/
fn execute(file: &str, input: &str, output: &str) {
    let mir = time("compile", || compile(file.to_owned(), false));
    let mut ctx = TestContext::new(input);
    let mut ms = MemoryState::new(2048, 8);
    time("run_mir", || ms.execute_block(&mir, &mut ctx));
    if ctx.print != output {
        println!("Expected: {:?}", output);
        println!("Found: {:?}", ctx.print);
        panic!("Test failed: output mismatch");
    }
    println!("{}ops", get_format(ms.instr_count));
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

#[test]
pub fn test_pendu_with_p() {
    // Reproducer: entering 'p' (not in "grammaire") should behave like any other wrong letter
    execute("Pendu", "pppppp", "\n\n\n\n------\n\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |\n |\n |\n |\n------\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |--\n |\n |\n |\n------\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n |  |\n |\n------\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n | /|\n |\n------\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n | /|\\\n |\n------\n\n_________\n\nTu n'as pas trouvé de lettre -1 vie\n |--|\n |  O\n | /|\\\n | / \\\n------\n\n_________\n\nGROSSE MERDE!\n");
}

#[test]
pub fn test_game2048() {
    // Play a full game with deterministic input (cycling 1234 = left/right/up/down)
    let mir = compile("Game2048".to_owned(), false);
    let input = "1234".repeat(50);
    let mut ctx = TestContext::new(&input);
    let mut ms = MemoryState::new(4096, 8);
    ms.execute_block(&mir, &mut ctx);
    assert!(ctx.print.starts_with("2048"), "Should start with banner");
    assert!(ctx.print.ends_with("Game over!\n"), "Game should end with Game over!");
    println!("{}ops", get_format(ms.instr_count));
}

#[test]
pub fn test_chess() {
    // Input: srcCol srcRow dstCol dstRow (1-based, 'a'=1...'h'=8)
    // Scholar's mate-style: White moves e-pawn, bishop, queen, then queen takes f7
    // Move format: col(a-h=1-8) row(1-8) col row
    // e2e4 e7e5 f1c4 b8c6 d1h5 a7a6 h5f7 = queen captures pawn next to king
    // But f7 pawn isn't the king. Let's just play until a king is captured.
    // Simplified: move pieces until one side captures the other's king.
    // White: e2→e4, Black: f7→f5, White: d1→h5 (queen), Black: g7→g6, White: h5→e8 (capture king!)
    // Encoded: e=5,2=2 → "5254" then f=6,7=7,f=6,5=5 → "6765" etc.
    // Actually Val.input returns lower nibble of char. '5'=0x35→5, '2'=0x32→2
    let input = concat!(
        "5254",  // White: e2→e4
        "6765",  // Black: f7→f5
        "4185",  // White: d1→h5 (queen to h5)
        "7776",  // Black: g7→g6
        "8558",  // White: h5→e8 (queen captures next to king... actually e8 has nothing)
    );
    // Actually this is hard to predict without seeing the board. Let me use a direct
    // king capture: remove blocking pieces then take the king.
    // Simpler: White e2e4, Black e7e5, White d1f3, Black a7a6, White f3f7 (takes pawn near king),
    // Black a6a5, White f7e8 (not king). Hmm.
    // Let me just do a very direct test: move white queen diagonally to capture black king.
    // With no move validation, we can cheat:
    // White: e1→e8 (king jumps to rank 8?). No, we want to capture black king at e8.
    // Black king is at e8 = col 5, row 8 (index col=4, row=7 in 0-based).
    // White queen at d1 = col 4, row 1 (0-based: col=3, row=0).
    // Move white queen d1→e8: input "4158" (d=4,1,e=5,8)
    // Then black needs to move: say a7→a6: "1716"
    // Then white queen takes black king at e8: wait, queen is already there after first move.
    // No: first move "4158" moves queen from d1 to e8 which has the black king. King captured!
    let mir = compile("Chess".to_owned(), false);
    let mut ctx = TestContext::new("4158");
    let mut ms = MemoryState::new(4096, 8);
    ms.execute_block(&mir, &mut ctx);
    assert!(ctx.print.contains("White wins!"), "White should win by capturing black king");
    // Verify the initial board display is present
    assert!(ctx.print.contains("R N B Q K B N R"), "Initial board should show white back rank");
    assert!(ctx.print.contains("r n b q k b n r"), "Initial board should show black back rank");
    println!("{}ops", get_format(ms.instr_count));
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
