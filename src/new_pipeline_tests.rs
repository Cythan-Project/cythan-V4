//! Integration tests for the new pipeline (`new_parser` → `typer` →
//! `hir` → `mir`). Each test compiles sources via
//! `cythan_driver::new_pipeline` and runs them with scripted input,
//! asserting on captured output. This is the harness that future
//! automated interaction tests should use.
//!
//! The new pipeline's stdlib lives under `examples/new_syntax/std/`;
//! programs under `examples/new_syntax/`. That's distinct from the
//! old pipeline's `cythan/std` + `cythan/examples` layout.
//!
//! # Writing a new interaction test
//!
//! 1. Put your `.ct` source under `examples/new_syntax/<Name>.ct`.
//! 2. Build a file list with `load_new_syntax_std()` + your program.
//! 3. Call `cythan_driver::new_pipeline::compile_and_run` with the
//!    entry `FnSig::new("<Name>", "main")`, scripted input, and a
//!    memory budget. Inspect `CapturedRun.output`.

use cythan_driver::new_pipeline::{compile, compile_and_run, run_mir_with_input, CapturedRun};
use std::path::PathBuf;

const NEW_SYNTAX_DIR: &str = "examples/new_syntax";

fn load_file(rel: &str) -> String {
    let path = PathBuf::from(NEW_SYNTAX_DIR).join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
        .replace('\r', "")
}

fn new_syntax_stdlib() -> Vec<(&'static str, String)> {
    vec![
        ("std/System.ct", load_file("std/System.ct")),
        ("std/Ops.ct", load_file("std/Ops.ct")),
        ("std/Bool.ct", load_file("std/Bool.ct")),
        ("std/U4.ct", load_file("std/U4.ct")),
        ("std/U8.ct", load_file("std/U8.ct")),
        ("std/Array.ct", load_file("std/Array.ct")),
        ("std/DynArray.ct", load_file("std/DynArray.ct")),
    ]
}

fn run_program(
    program_path: &str,
    entry_type: &str,
    entry_method: &str,
    input: &str,
    mem_cells: usize,
) -> CapturedRun {
    let mut files = new_syntax_stdlib();
    files.push((
        Box::leak(program_path.to_string().into_boxed_str()),
        load_file(program_path),
    ));
    let entry = typer::FnSig::new(entry_type, entry_method);
    let run = compile_and_run(&files, &entry, input, mem_cells)
        .unwrap_or_else(|e| panic!("harness failed on {}: {}", program_path, e));
    // A runaway MIR program must not silently masquerade as success —
    // any test that hits the step ceiling should fail loudly.
    assert!(
        !run.aborted_by_limit,
        "program `{}` ({}::{}): exceeded MIR step limit ({}), likely infinite loop. Partial output:\n{}",
        program_path, entry_type, entry_method, run.instr_count, run.output
    );
    run
}

// -----------------------------------------------------------------------
// Morpion — tic-tac-toe game. Exercises enum pattern matching, arrays
// of enums, input-driven loop, and trait impls (Eq for Cell).
// -----------------------------------------------------------------------

#[test]
fn morpion_o_wins_with_diagonal() {
    // Moves 1-7 alternate O and X; diagonal 1-5-9 is O, but here the
    // input triggers the first-win path the old pipeline also exercises.
    let result = run_program("Morpion.ct", "Morpion", "main", "1234567", 4096);
    assert!(
        result.output.contains("O won!"),
        "expected 'O won!' in output; got:\n{}",
        result.output
    );
}

#[test]
fn morpion_input_validation() {
    // Repeated moves to already-occupied cells exercise the
    // "Invalid input!" branch before eventually completing a game.
    let result = run_program(
        "Morpion.ct",
        "Morpion",
        "main",
        "956787821122189576321456987",
        4096,
    );
    assert!(
        result.output.contains("Invalid input!"),
        "expected 'Invalid input!' in output; got:\n{}",
        result.output
    );
}

// -----------------------------------------------------------------------
// Harness smoke tests — verify the compile/run helpers themselves work
// on a small inline program that echoes U8 input characters back.
// Don't rely on the full game stdlib; catch pipeline regressions fast.
// -----------------------------------------------------------------------

#[test]
fn harness_captures_output_from_inline_program() {
    let mut files = new_syntax_stdlib();
    files.push((
        "Echo.ct",
        r#"
            struct Echo {}
            extension Echo {
                fn main(): U4 {
                    'h'.print();
                    'i'.print();
                    '\n'.print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    let entry = typer::FnSig::new("Echo", "main");
    let result = compile_and_run(&files, &entry, "", 512).expect("compile+run");
    assert!(!result.aborted_by_limit, "Echo hit step limit — infinite loop?");
    assert_eq!(result.output, "hi\n");
    assert_eq!(result.remaining_input, "");
    assert!(result.instr_count > 0, "should have executed something");
}

#[test]
fn harness_feeds_scripted_input() {
    let mut files = new_syntax_stdlib();
    files.push((
        "InputEcho.ct",
        r#"
            struct InputEcho {}
            extension InputEcho {
                fn main(): U4 {
                    U8 a = U8::input();
                    a.print();
                    U8 b = U8::input();
                    b.print();
                    '\n'.print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    let entry = typer::FnSig::new("InputEcho", "main");
    let result = compile_and_run(&files, &entry, "AB", 512).expect("compile+run");
    assert!(!result.aborted_by_limit, "InputEcho hit step limit — infinite loop?");
    assert_eq!(result.output, "AB\n");
}

/// Regression: `42 - 40` on `U4` used to hang. `Set(slot, 42)` stored
/// `42` in a 4-bit cell, and the `if_zero` Match (`[0]` vs `1..=15`)
/// had no matching arm for that value, so the subtraction loop's
/// condition check fell through silently inside the `Loop` and
/// iterated forever. Fix: the interpreter masks `Set` / `Copy` to
/// the u4 cell range. This test guards the mask.
#[test]
fn u4_literal_above_cell_range_still_terminates() {
    let mut files = new_syntax_stdlib();
    files.push((
        "T.ct",
        r#"
            struct T {}
            extension T {
                fn main(): U4 { 42 - 40 }
            }
        "#
        .to_string(),
    ));
    let entry = typer::FnSig::new("T", "main");
    let r = compile_and_run(&files, &entry, "", 512).expect("run");
    assert!(!r.aborted_by_limit);
}

// -----------------------------------------------------------------------
// Toolchain tests — exercise the `check` / `build-hir` staging used by
// the `cythan new` subcommands without going through the CLI binary.
// -----------------------------------------------------------------------

#[test]
fn toolchain_check_reports_ok_summary() {
    let mut files = new_syntax_stdlib();
    files.push((
        "Trivial.ct",
        "struct Trivial {} extension Trivial { fn main(): U4 { 0 } }".to_string(),
    ));
    let summary = cythan_driver::new_pipeline::check(&files).expect("check");
    assert!(summary.types >= 1, "U4 at least should be present");
    assert!(summary.simple_fns >= 1, "Trivial::main is simple");
    let text = format!("{}", summary);
    assert!(text.starts_with("ok:"), "summary should start with 'ok:', got: {}", text);
}

#[test]
fn toolchain_check_surfaces_typer_errors() {
    let mut files = new_syntax_stdlib();
    files.push((
        "Bad.ct",
        // `Nope` is not a declared type — resolution fails.
        "struct Bad { Nope field, }".to_string(),
    ));
    let err = cythan_driver::new_pipeline::check(&files).unwrap_err();
    assert!(
        err.contains("error") || err.to_lowercase().contains("unknown"),
        "expected a typer error message, got: {}",
        err
    );
}

#[test]
fn toolchain_build_hir_produces_dump() {
    let mut files = new_syntax_stdlib();
    files.push((
        "Dump.ct",
        r#"
            struct Dump {}
            extension Dump {
                fn main(): U4 {
                    'x'.print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    let built = cythan_driver::new_pipeline::build_hir(&files).expect("build_hir");
    let text = cythan_driver::new_pipeline::hir_to_text(&built.hir);
    // The dump must cover Dump::main and its name should appear.
    assert!(text.contains("fn Dump::main"), "missing Dump::main entry; got:\n{}", text);
    // And reference some U8 print call.
    assert!(
        text.contains("U8::print") || text.contains("U4::print"),
        "expected a print call site; got:\n{}",
        text
    );
}

#[test]
fn toolchain_build_mir_produces_dump() {
    let mut files = new_syntax_stdlib();
    files.push((
        "MirDump.ct",
        r#"
            struct MirDump {}
            extension MirDump {
                fn main(): U4 { 7 }
            }
        "#
        .to_string(),
    ));
    let entry = typer::FnSig::new("MirDump", "main");
    let mir = cythan_driver::new_pipeline::compile(&files, &entry).expect("compile");
    let text = cythan_driver::new_pipeline::mir_to_text(&mir);
    assert!(!text.is_empty(), "MIR dump empty");
    // The literal 7 must appear as a Set value somewhere in the dump
    // — it's our entry function's only constant.
    assert!(text.contains('7'), "MIR dump doesn't reference the literal 7:\n{}", text);
    // Sanity: the dump shouldn't still carry HIR-only concepts.
    assert!(
        !text.contains("call "),
        "MIR dump should not contain HIR-style `call` (all calls inlined):\n{}",
        text
    );
}

#[test]
fn harness_compile_and_run_are_composable() {
    // The compile / run split lets callers reuse the compiled MIR
    // across multiple scripted inputs — useful for interaction
    // tests that replay a scenario with varying choices.
    let mut files = new_syntax_stdlib();
    files.push((
        "Greet.ct",
        r#"
            struct Greet {}
            extension Greet {
                fn main(): U4 {
                    U8 c = U8::input();
                    c.print();
                    '!'.print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    let entry = typer::FnSig::new("Greet", "main");
    let mir = compile(&files, &entry).expect("compile");
    let r1 = run_mir_with_input(&mir, "A", 512);
    let r2 = run_mir_with_input(&mir, "Z", 512);
    assert!(!r1.aborted_by_limit && !r2.aborted_by_limit, "hit step limit");
    assert_eq!(r1.output, "A!");
    assert_eq!(r2.output, "Z!");
}

// -----------------------------------------------------------------------
// Pendu — hangman. Target word "grammaire" (9 chars), 6 lives. Tests
// pull transcripts from the legacy pipeline to verify byte-exact
// behavior through the new pipeline.
// -----------------------------------------------------------------------

#[test]
fn pendu_wins_with_gramire() {
    // Letters 'g', 'r', 'a', 'm', 'i', 'r', 'e' reveal all chars.
    let result = run_program("Pendu.ct", "Pendu", "main", "gramire", 4096);
    // ASCII-only substring: the new pipeline double-encodes non-ASCII
    // bytes as chars via the MIR interpreter, so the French accent on
    // "gagné" round-trips as `Ã©`. Testing the ASCII prefix avoids
    // that re-encoding concern until string printing is settled.
    assert!(
        result.output.contains("Vous avez gagn"),
        "expected win message; got:\n{}",
        result.output
    );
    // The last displayed board before the win message should show 8
    // of 9 letters revealed ("grammair_" — 'e' is the final reveal).
    assert!(
        result.output.contains("grammair_"),
        "expected last-board state; got:\n{}",
        result.output
    );
}

// -----------------------------------------------------------------------
// 2048 — 4x4 merge puzzle. Input 1/2/3/4 = left/right/up/down.
// Deterministic: seed advances each turn, no RNG. We repeatedly cycle
// the four directions until the board fills and Game over! fires.
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// Chess — 8x8 board, four-digit input per turn (srcCol srcRow dstCol
// dstRow, all 1-based). No move validation; capture the opposing king
// to win. Test plays the minimal path to capture Black's king.
// -----------------------------------------------------------------------

#[test]
fn chess_white_captures_black_king() {
    // d1 → e8: col 4, row 1, to col 5, row 8. Black king sits at e8
    // in the starting position; no validation lets the white queen
    // teleport and capture immediately.
    let result = run_program("Chess.ct", "Chess", "main", "4158", 8192);
    assert!(
        result.output.contains("White wins!"),
        "expected White win; got (tail):\n{}",
        &result.output[result.output.len().saturating_sub(400)..]
    );
    // Initial board should show both back ranks.
    assert!(
        result.output.contains("R N B Q K B N R"),
        "expected white back rank; got:\n{}",
        &result.output[..result.output.len().min(400)]
    );
    assert!(
        result.output.contains("r n b q k b n r"),
        "expected black back rank; got:\n{}",
        &result.output[..result.output.len().min(400)]
    );
}

#[test]
fn game2048_fills_board_and_ends() {
    let input = "1234".repeat(50);
    let result = run_program("Game2048.ct", "Game2048", "main", &input, 8192);
    assert!(
        result.output.starts_with("2048"),
        "expected banner at start; got:\n{}",
        &result.output[..result.output.len().min(200)]
    );
    assert!(
        result.output.contains("Game over!") || result.output.contains("You won!"),
        "expected game to end; got (tail):\n{}",
        &result.output[result.output.len().saturating_sub(400)..]
    );
}

#[test]
fn pendu_loses_on_all_wrong_letters() {
    // 'h' never appears in "grammaire"; after 6 misses the player
    // loses with "GROSSE MERDE!".
    let result = run_program("Pendu.ct", "Pendu", "main", "hhhhhhhhhhhhhhhhhh", 4096);
    assert!(
        result.output.contains("GROSSE MERDE!"),
        "expected loss message; got:\n{}",
        result.output
    );
}

#[test]
fn morpion_equality() {
    // Classic cat's-game sequence — board fills with no winner.
    let result = run_program("Morpion.ct", "Morpion", "main", "123547698", 4096);
    assert!(
        result.output.contains("Equality!"),
        "expected 'Equality!' in output; got:\n{}",
        result.output
    );
}
