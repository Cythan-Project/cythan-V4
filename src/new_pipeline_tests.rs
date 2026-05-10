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
fn toolchain_build_lir_produces_dump() {
    let mut files = new_syntax_stdlib();
    files.push((
        "LirDump.ct",
        "struct LirDump {} extension LirDump { fn main(): U4 { 7 } }".to_string(),
    ));
    let entry = typer::FnSig::new("LirDump", "main");
    let mir = cythan_driver::new_pipeline::compile(&files, &entry).expect("compile");
    let lir = cythan_driver::new_pipeline::mir_to_lir(&mir);
    let text = cythan_driver::new_pipeline::lir_to_text(&lir);
    assert!(!text.is_empty(), "LIR dump empty");
    assert!(
        !lir.is_empty(),
        "LIR should have at least one instruction"
    );
}

#[test]
fn toolchain_build_cythan_bytecode_round_trips_to_text() {
    let mut files = new_syntax_stdlib();
    files.push((
        "Cy.ct",
        "struct Cy {} extension Cy { fn main(): U4 { 1 } }".to_string(),
    ));
    let entry = typer::FnSig::new("Cy", "main");
    let mir = cythan_driver::new_pipeline::compile(&files, &entry).expect("compile");
    let lir = cythan_driver::new_pipeline::mir_to_lir(&mir);
    let bytecode = cythan_driver::new_pipeline::lir_to_bytecode(lir);
    assert!(
        !bytecode.is_empty(),
        "expected non-empty bytecode for trivial program"
    );
    let text = cythan_driver::new_pipeline::bytecode_to_text(&bytecode);
    // Format: space-separated decimals. Splitting should yield the
    // original word list byte-for-byte after parsing back.
    let parsed: Vec<usize> = text.split_whitespace().map(|s| s.parse().unwrap()).collect();
    assert_eq!(parsed, bytecode);
}

/// Regression for the LIR-optimizer bug that crashed Morpion on
/// the Cythan backend:
///
/// `opt_asm` collapses `Label A; Jump B` into `Jump B` and remaps
/// references to `A` via `remap()`. The pre-fix `remap` only
/// updated `Jump`, `Label`, and `If0` — `Match`'s 16-slot jump
/// table was left stale, so any if-zero-shaped match whose
/// then-branch body reduces to a single jump would point at a
/// label the emitter never declared. Cythan_compiler then panicked
/// with "Try to init your label at an index: 'lH…". The fix walks
/// the `Match` slots in `remap` too.
///
/// This test runs Morpion end-to-end on the Cythan VM, which
/// exercises many if-zero matches. A pass here proves the bug is
/// dead.
#[test]
fn morpion_runs_on_cythan_backend_after_lir_remap_fix() {
    let mut files = new_syntax_stdlib();
    files.push((
        "Morpion.ct",
        std::fs::read_to_string("examples/new_syntax/Morpion.ct")
            .expect("read Morpion")
            .replace('\r', ""),
    ));
    let entry = typer::FnSig::new("Morpion", "main");
    let mir = cythan_driver::new_pipeline::compile(&files, &entry).expect("compile");
    let run = cythan_driver::new_pipeline::run_with_backend(
        &mir,
        cythan_driver::new_pipeline::Backend::Cythan,
        "1234567",
        4096,
        0,
    );
    assert!(!run.aborted_by_limit);
    assert!(
        run.output.contains("O won!"),
        "expected 'O won!' in output; got (tail):\n{}",
        &run.output[run.output.len().saturating_sub(400)..]
    );
}

#[test]
fn run_with_cythan_backend_matches_mir_output() {
    // Trivial program — well within the cythan_compiler's comfort
    // zone (larger programs still hit a pre-existing limit in the
    // external bytecode compiler, tracked separately).
    let mut files = new_syntax_stdlib();
    files.push((
        "Greet.ct",
        r#"
            struct Greet {}
            extension Greet {
                fn main(): U4 {
                    'h'.print();
                    'i'.print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    let entry = typer::FnSig::new("Greet", "main");
    let mir = cythan_driver::new_pipeline::compile(&files, &entry).expect("compile");
    let via_mir = cythan_driver::new_pipeline::run_with_backend(
        &mir,
        cythan_driver::new_pipeline::Backend::Mir,
        "",
        512,
        cythan_driver::new_pipeline::DEFAULT_STEP_LIMIT,
    );
    let via_vm = cythan_driver::new_pipeline::run_with_backend(
        &mir,
        cythan_driver::new_pipeline::Backend::Cythan,
        "",
        512,
        cythan_driver::new_pipeline::DEFAULT_STEP_LIMIT,
    );
    assert!(!via_mir.aborted_by_limit);
    assert!(!via_vm.aborted_by_limit);
    assert_eq!(via_mir.output, "hi");
    assert_eq!(via_vm.output, "hi");
}

#[test]
fn pipeline_stats_are_populated_for_a_real_program() {
    // Compile Morpion end-to-end through `compile_with_stats` and
    // check every counter came out > 0 and that the LIR peephole
    // produced a real reduction.
    let mut files = new_syntax_stdlib();
    files.push((
        "Morpion.ct",
        std::fs::read_to_string("examples/new_syntax/Morpion.ct")
            .expect("read Morpion")
            .replace('\r', ""),
    ));
    let entry = typer::FnSig::new("Morpion", "main");
    let (_mir, stats) =
        cythan_driver::new_pipeline::compile_with_stats(&files, &entry).expect("compile");
    assert!(stats.hir_functions >= 40, "expected many functions, got {}", stats.hir_functions);
    assert!(stats.hir_ops_pre_inline > 0);
    assert!(
        stats.hir_ops_post_inline > stats.hir_ops_pre_inline,
        "inlining should expand the program: pre={} post={}",
        stats.hir_ops_pre_inline,
        stats.hir_ops_post_inline,
    );
    assert!(stats.mir_ops > 0);
    assert!(stats.lir_instructions_pre_opt > 0);
    assert!(
        stats.lir_instructions_post_opt <= stats.lir_instructions_pre_opt,
        "LIR opt should never grow the program: pre={} post={}",
        stats.lir_instructions_pre_opt,
        stats.lir_instructions_post_opt,
    );
    // Display emits a multi-line summary — check a representative line.
    let text = format!("{}", stats);
    assert!(text.contains("HIR:"), "Display should have HIR line");
    assert!(text.contains("MIR:"));
    assert!(text.contains("LIR:"));
}

#[test]
fn backend_from_str_parses_the_three_known_names() {
    use cythan_driver::new_pipeline::Backend;
    assert_eq!("mir".parse::<Backend>().unwrap(), Backend::Mir);
    assert_eq!("lir".parse::<Backend>().unwrap(), Backend::Lir);
    assert_eq!("cythan".parse::<Backend>().unwrap(), Backend::Cythan);
    assert_eq!("CYTHAN".parse::<Backend>().unwrap(), Backend::Cythan);
    assert_eq!("vm".parse::<Backend>().unwrap(), Backend::Cythan);
    assert!("bogus".parse::<Backend>().is_err());
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
    // Full French win message — now exact: byte-based print
    // (MIR `RunContext::print` takes `u8` instead of `char`) so the
    // UTF-8 `é` (`C3 A9`) survives the capture intact.
    assert!(
        result.output.contains("Vous avez gagné!"),
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

// -----------------------------------------------------------------------
// soir backend cross-checks (M4 rollout gate)
//
// For each of these cases the program is compiled via BOTH the classical
// `compile` path and the new `compile_via_soir` path, then run against the
// same scripted input. Outputs and remaining inputs must match. This is
// the "nothing broken" harness: as long as these pass, the soir backend
// is safe to stage behind a flag and eventually flip to default.
// -----------------------------------------------------------------------

use cythan_driver::new_pipeline::{
    compile_via_soir, compile_via_soir_lir, compile_via_soir_native_inline,
    run_bytecode_with_input_raw,
};

/// Cross-check helper for the soir → LIR path. Compiles the
/// program through both classical (HIR inline + MIR + LIR +
/// bytecode) and soir-to-LIR (soir inline + LIR + bytecode),
/// then runs both on the Cythan VM and asserts identical
/// output. Enforces the 30s ceiling.
fn cross_check_soir_lir(
    files: Vec<(&'static str, String)>,
    entry: typer::FnSig,
    input: &str,
    step_limit: usize,
) {
    use std::time::Instant;
    // Classical reference path.
    let classical = compile(&files, &entry).expect("classical compile");
    let r_class = cythan_driver::new_pipeline::run_with_backend(
        &classical,
        cythan_driver::new_pipeline::Backend::Cythan,
        input,
        4096,
        step_limit,
    );
    assert!(!r_class.aborted_by_limit, "classical hit step limit");

    // soir → LIR path.
    let t = Instant::now();
    let bytecode = compile_via_soir_lir(&files, &entry).expect("soir-lir compile");
    let elapsed = t.elapsed();
    assert!(
        elapsed.as_secs() < 30,
        "soir-lir compile exceeded 30s: {:?}",
        elapsed
    );
    let r_soir = run_bytecode_with_input_raw(&bytecode, input, step_limit);
    assert!(!r_soir.aborted_by_limit, "soir-lir hit step limit");

    assert_eq!(r_class.output, r_soir.output, "output mismatch");
    assert_eq!(
        r_class.remaining_input, r_soir.remaining_input,
        "input-consumption mismatch"
    );
}

/// Cross-check helper for the native-inline path. Asserts the
/// 30s compile ceiling so a regression into exponential walking
/// fails the test loudly instead of timing out silently.
fn cross_check_native(
    files: Vec<(&'static str, String)>,
    entry: typer::FnSig,
    input: &str,
    mem_cells: usize,
) {
    use std::time::Instant;
    let classical = compile(&files, &entry).expect("classical compile");
    let t = Instant::now();
    let via_soir = compile_via_soir_native_inline(&files, &entry)
        .expect("soir native-inline compile");
    let elapsed = t.elapsed();
    assert!(
        elapsed.as_secs() < 30,
        "soir native-inline compile exceeded 30s: {:?}",
        elapsed
    );
    let r_class = run_mir_with_input(&classical, input, mem_cells);
    let r_soir = run_mir_with_input(&via_soir, input, mem_cells);
    assert!(!r_class.aborted_by_limit);
    assert!(!r_soir.aborted_by_limit);
    assert_eq!(r_class.output, r_soir.output);
    assert_eq!(r_class.remaining_input, r_soir.remaining_input);
}

fn cross_check(files: Vec<(&'static str, String)>, entry: typer::FnSig, input: &str, mem_cells: usize) {
    let classical = compile(&files, &entry).expect("classical compile");
    let via_soir = compile_via_soir(&files, &entry).expect("soir compile");
    let r_class = run_mir_with_input(&classical, input, mem_cells);
    let r_soir = run_mir_with_input(&via_soir, input, mem_cells);
    assert!(!r_class.aborted_by_limit, "classical hit step limit");
    assert!(!r_soir.aborted_by_limit, "soir hit step limit");
    assert_eq!(r_class.output, r_soir.output, "output mismatch");
    assert_eq!(
        r_class.remaining_input, r_soir.remaining_input,
        "input-consumption mismatch"
    );
}

#[test]
fn soir_cross_echo() {
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
    cross_check(files, typer::FnSig::new("Echo", "main"), "", 512);
}

#[test]
fn soir_cross_input_echo() {
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
    cross_check(files, typer::FnSig::new("InputEcho", "main"), "AB", 512);
}

#[test]
#[ignore = "28k-node post-inline graph + deep nesting exhausts the \
    scheduler's step budget; M6 pattern rewrites should shrink this \
    enough to lift the #[ignore]"]
fn soir_morpion_phases_time() {
    // Isolate which phase of the soir pipeline is slow: HIR
    // pre-processing, soir translation, or soir scheduling. Runs
    // each sub-step independently so we know where to look.
    use cythan_driver::new_pipeline::{
        build_hir, compile as classical_compile,
    };
    use hir::{
        elide_redundant_mut, elide_unused_args, inline_program_full,
        specialize_monomorph_with_summaries, compute_exit_domains, BuiltinNatives,
        NativeProvider,
    };
    use std::time::Instant;
    let mut files = new_syntax_stdlib();
    files.push(("Morpion.ct", load_file("Morpion.ct")));
    let entry = typer::FnSig::new("Morpion", "main");
    let t0 = Instant::now();
    let built = build_hir(&files).expect("build_hir");
    eprintln!("build_hir: {}ms", t0.elapsed().as_millis());

    let t1 = Instant::now();
    let natives_for_summary = BuiltinNatives::new();
    let hir_map = built.hir.clone();
    let db_ref = &built.db;
    let summaries = compute_exit_domains(&hir_map, |sig| {
        natives_for_summary.has_method(&sig.type_name, &sig.method_name)
            || matches!(
                db_ref.get(sig),
                Some(typer::Fn::Simple(s)) if s.body.sig.params.iter().all(|p| !p.mutable)
            )
    });
    let spec = specialize_monomorph_with_summaries(hir_map, &entry, Some(&summaries));
    let natives = BuiltinNatives::new();
    let demut = elide_redundant_mut(spec.functions, |sig| {
        natives.has_method(&sig.type_name, &sig.method_name)
            || matches!(
                db_ref.get(sig),
                Some(typer::Fn::Simple(s)) if s.body.sig.params.iter().all(|p| !p.mutable)
            )
    });
    let trimmed = elide_unused_args(demut, &entry);
    let inlined = inline_program_full(&trimmed, &entry, Some(&built.reg), Some(&built.db))
        .expect("inline");
    eprintln!("hir pipeline → inlined: {}ms, {} ops", t1.elapsed().as_millis(),
              hir::count_ops(&inlined.body));

    let t2 = Instant::now();
    let graph = soir::translate_function(&inlined);
    eprintln!("soir translate: {}ms, {} live nodes", t2.elapsed().as_millis(),
              graph.live_len());

    let t3 = Instant::now();
    let mir = soir::schedule(&graph);
    eprintln!("soir schedule: {}ms, {} MIR ops", t3.elapsed().as_millis(),
              mir.0.len());

    // Sanity: also run classical to compare compile time.
    let t4 = Instant::now();
    let _ = classical_compile(&files, &entry).expect("classical");
    eprintln!("classical compile: {}ms", t4.elapsed().as_millis());
}

#[test]
#[ignore = "see soir_morpion_phases_time — blocked on M6"]
fn soir_morpion_compiles() {
    // Narrower than the full cross-check: just verify that the
    // soir backend produces MIR for Morpion in bounded time
    // (relies on the SCHEDULE_STEP_LIMIT + cycle guards). If
    // this times out, the scheduler is pathological; if it
    // passes but the cross-check times out, the issue is in
    // MIR execution (likely a soir-produced MIR that's correct
    // but un-optimised, exhausting the run-time step limit).
    use std::time::Instant;
    let mut files = new_syntax_stdlib();
    files.push(("Morpion.ct", load_file("Morpion.ct")));
    let entry = typer::FnSig::new("Morpion", "main");
    let start = Instant::now();
    let mir = compile_via_soir(&files, &entry).expect("soir compile");
    let elapsed = start.elapsed();
    eprintln!(
        "soir compile(Morpion): {}ms, {} MIR ops",
        elapsed.as_millis(),
        mir.0.len()
    );
    assert!(
        elapsed.as_secs() < 30,
        "soir compile exceeded the 30s ceiling: {:?}",
        elapsed
    );
}


#[test]
fn soir_lir_echo() {
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
    cross_check_soir_lir(files, typer::FnSig::new("Echo", "main"), "", 100_000);
}

#[test]
fn soir_lir_sizes() {
    use std::time::Instant;
    let mut files = new_syntax_stdlib();
    files.push(("Morpion.ct", load_file("Morpion.ct")));
    let entry = typer::FnSig::new("Morpion", "main");
    let t = Instant::now();
    let bc = compile_via_soir_lir(&files, &entry).expect("soir-lir");
    eprintln!("soir-lir Morpion: {} bytecode words in {}ms", bc.len(), t.elapsed().as_millis());
    let classical = compile(&files, &entry).expect("classical");
    let cl_lir = cythan_driver::new_pipeline::mir_to_lir(&classical);
    let cl_bc = cythan_driver::new_pipeline::lir_to_bytecode(cl_lir);
    eprintln!("classical Morpion: {} bytecode words", cl_bc.len());
}

#[test]
fn soir_lir_struct_mutation_in_loop() {
    // Cross-check struct mutation persisting across loop
    // iterations — the smallest version of the Morpion bug.
    // If this passes, the loop-phi for the struct's slots is
    // wired correctly. If it fails, that's the bug.
    let mut files = new_syntax_stdlib();
    files.push((
        "Box.ct",
        r#"
            struct Box { U4 v }
            extension Box {
                fn bump(mut self) {
                    self.v += 1;
                }
                fn main(): U4 {
                    mut Box b = Box { v: 0 };
                    mut U4 i = 0;
                    loop {
                        if i == 3 { break; }
                        b.bump();
                        i += 1;
                    }
                    b.v.print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    cross_check_soir_lir(files, typer::FnSig::new("Box", "main"), "", 100_000);
}

#[test]
fn debug_dump_array_graph() {
    use hir::{
        elide_redundant_mut, elide_unused_args, BuiltinNatives, NativeProvider,
    };
    let mut files = new_syntax_stdlib();
    files.push((
        "ArrTest.ct",
        r#"
            struct ArrTest { Array<U4, 4, U4> a }
            extension ArrTest {
                fn main(): U4 {
                    mut ArrTest t = ArrTest { a: Array<U4, 4, U4>::new() };
                    mut U4 i = 0;
                    loop {
                        if i == 4 { break; }
                        t.a.set(i, 5);
                        i += 1;
                    }
                    t.a.get(0).print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    let entry = typer::FnSig::new("ArrTest", "main");
    let built = cythan_driver::new_pipeline::build_hir(&files).unwrap();
    let cythan_driver::new_pipeline::BuiltHir { reg, db, hir } = built;
    let n = BuiltinNatives::new();
    let summaries = hir::compute_exit_domains(&hir, |s| {
        n.has_method(&s.type_name, &s.method_name)
            || matches!(db.get(s), Some(typer::Fn::Simple(s)) if s.body.sig.params.iter().all(|p| !p.mutable))
    });
    let spec = hir::specialize_monomorph_with_summaries(hir, &entry, Some(&summaries));
    let demut = elide_redundant_mut(spec.functions, |s| {
        n.has_method(&s.type_name, &s.method_name)
            || matches!(db.get(s), Some(typer::Fn::Simple(s)) if s.body.sig.params.iter().all(|p| !p.mutable))
    });
    let trimmed = elide_unused_args(demut, &entry);
    let program = soir::translate_program(&trimmed);
    let mut array_cache = hir::ArrayMonomorphCache::new();
    let mut resolve = |fn_ref: &hir::ir::FnRef| -> Option<soir::Graph> {
        if fn_ref.type_name != "Array" || !hir::array_synth::METHOD_NAMES.contains(&fn_ref.method_name.as_str()) {
            return None;
        }
        let spec = hir::array_synth::ArraySpec::from_template_args(&fn_ref.template_args, &reg)?;
        let (_, hir_fn) = array_cache.get_or_synth(&spec, &fn_ref.method_name)?;
        Some(soir::translate_function(&hir_fn))
    };
    let flat = soir::inline_program_with_resolver(&program, &entry, &mut resolve).unwrap();
    eprintln!("=== flat graph ===");
    eprintln!("{}", soir::dump_graph(&flat));
}

#[test]
fn debug_dump_array_lir() {
    use cythan_driver::new_pipeline::{
        build_hir, compile_via_soir_lir, lir_to_text,
    };
    use hir::{
        elide_redundant_mut, elide_unused_args, BuiltinNatives, NativeProvider,
    };
    let mut files = new_syntax_stdlib();
    files.push((
        "ArrTest.ct",
        r#"
            struct ArrTest { Array<U4, 4, U4> a }
            extension ArrTest {
                fn main(): U4 {
                    mut ArrTest t = ArrTest { a: Array<U4, 4, U4>::new() };
                    mut U4 i = 0;
                    loop {
                        if i == 4 { break; }
                        t.a.set(i, 5);
                        i += 1;
                    }
                    t.a.get(0).print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    let entry = typer::FnSig::new("ArrTest", "main");

    // Reproduce compile_via_soir_lir but stop before bytecode.
    let built = build_hir(&files).unwrap();
    let cythan_driver::new_pipeline::BuiltHir { reg, db, hir } = built;
    let n = BuiltinNatives::new();
    let summaries = hir::compute_exit_domains(&hir, |s| {
        n.has_method(&s.type_name, &s.method_name)
            || matches!(db.get(s), Some(typer::Fn::Simple(s)) if s.body.sig.params.iter().all(|p| !p.mutable))
    });
    let spec = hir::specialize_monomorph_with_summaries(hir, &entry, Some(&summaries));
    let demut = elide_redundant_mut(spec.functions, |s| {
        n.has_method(&s.type_name, &s.method_name)
            || matches!(db.get(s), Some(typer::Fn::Simple(s)) if s.body.sig.params.iter().all(|p| !p.mutable))
    });
    let trimmed = elide_unused_args(demut, &entry);
    let program = soir::translate_program(&trimmed);
    let mut array_cache = hir::ArrayMonomorphCache::new();
    let mut resolve = |fn_ref: &hir::ir::FnRef| -> Option<soir::Graph> {
        if fn_ref.type_name != "Array"
            || !hir::array_synth::METHOD_NAMES.contains(&fn_ref.method_name.as_str())
        {
            return None;
        }
        let spec = hir::array_synth::ArraySpec::from_template_args(&fn_ref.template_args, &reg)?;
        let (_, hir_fn) = array_cache.get_or_synth(&spec, &fn_ref.method_name)?;
        Some(soir::translate_function(&hir_fn))
    };
    let flat = soir::inline_program_with_resolver(&program, &entry, &mut resolve).unwrap();
    let lir = soir::schedule_lir(&flat);
    eprintln!("=== soir-lir output ({} ops) ===", lir.len());
    eprintln!("{}", lir_to_text(&lir));

    let _ = compile_via_soir_lir;
}

#[test]
fn soir_lir_array_mutation_in_loop() {
    let mut files = new_syntax_stdlib();
    files.push((
        "ArrTest.ct",
        r#"
            struct ArrTest { Array<U4, 4, U4> a }
            extension ArrTest {
                fn main(): U4 {
                    mut ArrTest t = ArrTest { a: Array<U4, 4, U4>::new() };
                    mut U4 i = 0;
                    loop {
                        if i == 4 { break; }
                        t.a.set(i, 5);
                        i += 1;
                    }
                    t.a.get(0).print();
                    t.a.get(1).print();
                    t.a.get(2).print();
                    t.a.get(3).print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    cross_check_soir_lir(files, typer::FnSig::new("ArrTest", "main"), "", 100_000);
}

#[test]
fn soir_lir_morpion() {
    // The milestone test: full Morpion game compiles via the
    // soir → LIR (CFG-style) backend AND its bytecode runs on
    // the Cythan VM matching the classical pipeline. If this
    // passes, we've solved the per-arm-duplication explosion
    // by emitting jumps to shared blocks the way LLVM /
    // Cranelift / GCC do at machine-code level.
    let mut files = new_syntax_stdlib();
    files.push(("Morpion.ct", load_file("Morpion.ct")));
    cross_check_soir_lir(
        files,
        typer::FnSig::new("Morpion", "main"),
        "123547698",
        4_000_000,
    );
}

#[test]
fn soir_lir_count_loop() {
    let mut files = new_syntax_stdlib();
    files.push((
        "CountLoop.ct",
        r#"
            struct CountLoop {}
            extension CountLoop {
                fn main(): U4 {
                    mut U4 i = 0;
                    loop { if i == 4 { break; } i += 1; }
                    i.print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    cross_check_soir_lir(
        files,
        typer::FnSig::new("CountLoop", "main"),
        "",
        100_000,
    );
}

#[test]
fn soir_native_inline_echo() {
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
    cross_check_native(files, typer::FnSig::new("Echo", "main"), "", 512);
}

#[test]
fn soir_native_inline_input_echo() {
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
    cross_check_native(
        files,
        typer::FnSig::new("InputEcho", "main"),
        "AB",
        512,
    );
}

#[test]
#[ignore = "scheduler hits the MIR-size ceiling on Morpion's post-inline \
    graph. Bounded walk_cache (256k ops) caps RAM, but each unique \
    (start, stop, scope-stack) triple still produces its own MIR, and \
    the scheduler's duplication exceeds 500k ops. Proper fix: structural \
    GVN on control subgraphs (merge duplicated match/region subtrees \
    across inlined copies) OR full GCM + dominator-based emission."]
fn soir_native_inline_morpion_phase_timing() {
    // Diagnostic: time each phase of compile_via_soir_native_inline
    // so we know if the 30s ceiling is breached during compile or
    // during MIR execution.
    use std::time::Instant;
    let mut files = new_syntax_stdlib();
    files.push(("Morpion.ct", load_file("Morpion.ct")));
    let entry = typer::FnSig::new("Morpion", "main");
    let t = Instant::now();
    let mir = compile_via_soir_native_inline(&files, &entry).expect("compile");
    let compile = t.elapsed();
    eprintln!(
        "native-inline compile: {}ms, {} MIR ops",
        compile.as_millis(),
        mir.0.len()
    );
    // Must complete under the ceiling.
    assert!(compile.as_secs() < 30, "compile too slow: {:?}", compile);
    // Don't run the MIR — this test is compile-phase only so a
    // broken soir→MIR scheduler can't mask compile regressions.
}

#[test]
#[ignore = "scheduler hits the MIR-size limit on Morpion's post-inline \
    graph. Array synth + per-function optimisation work, but the \
    scheduler duplicates downstream code into each arm of deeply \
    nested matches. Sharing-aware scheduling (Mir::Block + Mir::Skip) \
    is the next step."]
fn soir_native_inline_morpion() {
    // Full Morpion game: exercises `Array::new` / `get` / `set`
    // synthesis, deeply nested enum matches, arithmetic via the
    // stdlib's lockstep loops, and IO. If this passes under 30s
    // the per-function architecture works end-to-end.
    let mut files = new_syntax_stdlib();
    files.push(("Morpion.ct", load_file("Morpion.ct")));
    cross_check_native(
        files,
        typer::FnSig::new("Morpion", "main"),
        "123547698",
        4096,
    );
}

#[test]
fn soir_native_inline_count_loop() {
    // count_loop exercises the `U4::eq` + `U4::AddAssign` inlined
    // callees via the soir-native inliner. If the per-function
    // path resolves them correctly, the output matches classical.
    let mut files = new_syntax_stdlib();
    files.push((
        "CountLoop.ct",
        r#"
            struct CountLoop {}
            extension CountLoop {
                fn main(): U4 {
                    mut U4 i = 0;
                    loop {
                        if i == 4 { break; }
                        i += 1;
                    }
                    i.print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    cross_check_native(
        files,
        typer::FnSig::new("CountLoop", "main"),
        "",
        1024,
    );
}

#[test]
#[ignore = "blocked on M9b (true sharing-aware scheduler). Running \
    causes scheduler to walk exponentially many arm-duplicated paths; \
    SCHEDULE_MIR_SIZE_LIMIT (200k) panics cleanly, but the test \
    framework times out before even reaching that. DO NOT UN-IGNORE \
    without first implementing structural GVN or GCM — will burn RAM."]
fn soir_cross_morpion_equality() {
    // Full game cross-check: Morpion cat's-game scenario must
    // produce identical output through both backends. This is
    // the real "rollout gate" signal — if it passes, soir can
    // compile a non-trivial program (arrays, enums, structs,
    // traits, match dispatch, nested loops, IO) correctly.
    let mut files = new_syntax_stdlib();
    files.push(("Morpion.ct", load_file("Morpion.ct")));
    cross_check(
        files,
        typer::FnSig::new("Morpion", "main"),
        "123547698",
        4096,
    );
}

// Disabled until SoIR's `lower_match` correctly merges slot writes when the
// match arms write to the scrutinee slot itself. Affects every program that
// has a `+= 1` inside a loop after the inc/dec fast path was removed —
// `match-to-mapvalue` collapses the post-inline 16x16 lookup to a
// `MapValue(s, s, INC_TABLE)`, the SoIR builder lowers it via `lower_match`,
// and the post-arm phi for `s` reads back const(0) instead of the per-arm
// outputs. The classical (non-SoIR) pipeline is unaffected.
#[ignore = "SoIR lower_match: phi for write-to-scrut-slot reads const(0)"]
#[test]
fn soir_cross_count_loop() {
    let mut files = new_syntax_stdlib();
    files.push((
        "CountLoop.ct",
        r#"
            struct CountLoop {}
            extension CountLoop {
                fn main(): U4 {
                    mut U4 i = 0;
                    loop {
                        if i == 4 { break; }
                        i += 1;
                    }
                    i.print();
                    0
                }
            }
        "#
        .to_string(),
    ));
    cross_check(files, typer::FnSig::new("CountLoop", "main"), "", 1024);
}
