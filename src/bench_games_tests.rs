//! Perf baseline for the four games on the Cythan VM.
//!
//! Runs each `(game, scenario)` pair through the full pipeline and
//! records the VM step count. Output goes to `benchmarks/games.txt`
//! at the repo root — commit the file alongside a change to see how
//! the numbers move.
//!
//! # Reading the report
//!
//! ```
//! cythan-V4 benchmark report
//!
//! game         scenario                      vm_steps    out_bytes  compile_bytecode
//! chess        fool_mate_4158                 ...          ...              ...
//! game2048     cycle_1234_small              ...          ...              ...
//! morpion      cats_game_123547698           ...          ...              ...
//! morpion      diagonal_o_wins_1234567       ...          ...              ...
//! ...
//! ```
//!
//! Numbers sort by `(game, scenario)` so diffs are clean and
//! line-level — a regression shows up as a changed column on one
//! row, not as reordered lines.
//!
//! # Single-test design
//!
//! Everything runs inside one `#[test]` (`bench_games_report`) so
//! the report file is written once per `cargo test` invocation and
//! output ordering is deterministic. Individual scenarios can still
//! fail the test — we collect successes and failures into the
//! table, then fail the test at the end if *anything* broke.

use std::path::PathBuf;

use cythan_driver::new_pipeline::{compile_with_stats, mir_to_lir, lir_to_bytecode};
use cythan_driver::test_context::TestContext;

/// One scenario. `input` is the scripted keyboard input fed byte-
/// for-byte to the game; `expect_contains` is an ASCII substring
/// the captured transcript must contain (e.g. `"O won!"` for a
/// Morpion scenario where O should win).
struct Scenario {
    game: &'static str,
    /// Path under `examples/new_syntax/`, e.g. `"Morpion.ct"`.
    file: &'static str,
    /// Entry-point `type_name`. Matches the file's struct name.
    entry_type: &'static str,
    /// Short, stable scenario identifier — goes into the report.
    name: &'static str,
    input: String,
    expect_contains: &'static str,
    /// Upper bound on VM steps. Acts as a hard stop so a runaway
    /// game can't hang the suite. Picked generously — the real
    /// baseline lives in the committed report.
    step_limit: usize,
}

fn scenarios() -> Vec<Scenario> {
    vec![
        // ---------- Morpion ----------
        Scenario {
            game: "morpion",
            file: "Morpion.ct",
            entry_type: "Morpion",
            name: "diagonal_o_wins_1234567",
            input: "1234567".to_string(),
            expect_contains: "O won!",
            step_limit: 1_000_000,
        },
        Scenario {
            game: "morpion",
            file: "Morpion.ct",
            entry_type: "Morpion",
            name: "cats_game_123547698",
            input: "123547698".to_string(),
            expect_contains: "Equality!",
            step_limit: 1_000_000,
        },
        // ---------- Pendu ----------
        Scenario {
            game: "pendu",
            file: "Pendu.ct",
            entry_type: "Pendu",
            name: "win_gramire",
            input: "gramire".to_string(),
            expect_contains: "Vous avez gagné!",
            step_limit: 2_000_000,
        },
        Scenario {
            game: "pendu",
            file: "Pendu.ct",
            entry_type: "Pendu",
            name: "lose_hhhhhh",
            input: "hhhhhhhhhhhhhhhhhh".to_string(),
            expect_contains: "GROSSE MERDE!",
            step_limit: 2_000_000,
        },
        // ---------- Chess ----------
        Scenario {
            game: "chess",
            file: "Chess.ct",
            entry_type: "Chess",
            name: "fool_mate_4158",
            input: "4158".to_string(),
            expect_contains: "White wins!",
            step_limit: 5_000_000,
        },
        // ---------- Game2048 ----------
        //
        // Game2048 is deliberately not benched on the Cythan VM.
        // Its board display (16 cells × full-byte prints each turn)
        // and slide/merge loops (every row walked twice per move)
        // cost hundreds of millions of VM steps for any
        // end-to-end scenario — too slow for a per-commit baseline.
        // The MIR-backend coverage for 2048 lives in
        // `new_pipeline_tests::game2048_fills_board_and_ends`; if
        // / when the VM gets fast enough, a scenario can be added
        // back here.
    ]
}

struct RunResult {
    scenario: Scenario,
    /// `Ok(...)` on a clean run, `Err(reason)` on compile failure,
    /// step-limit overshoot, or failed assertion.
    outcome: Result<Metrics, String>,
}

struct Metrics {
    vm_steps: usize,
    output_bytes: usize,
    bytecode_words: usize,
}

fn new_syntax_file(rel: &str) -> String {
    let p = PathBuf::from("examples/new_syntax").join(rel);
    std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("read {}: {}", p.display(), e))
        .replace('\r', "")
}

fn stdlib_files() -> Vec<(String, String)> {
    [
        "std/System.ct",
        "std/Ops.ct",
        "std/Bool.ct",
        "std/U4.ct",
        "std/U8.ct",
        "std/Array.ct",
        "std/DynArray.ct",
    ]
    .iter()
    .map(|rel| (rel.to_string(), new_syntax_file(rel)))
    .collect()
}

fn run_scenario(sc: &Scenario) -> Result<Metrics, String> {
    let mut files: Vec<(String, String)> = stdlib_files();
    files.push((sc.file.to_string(), new_syntax_file(sc.file)));
    let refs: Vec<(&str, String)> = files.iter().map(|(n, s)| (n.as_str(), s.clone())).collect();
    let entry = typer::FnSig::new(sc.entry_type, "main");

    let (mir, _stats) = compile_with_stats(&refs, &entry)?;
    let lir = mir_to_lir(&mir);
    let bytecode = lir_to_bytecode(lir);

    // Run on the Cythan VM with a step limit so a runaway game
    // fails the scenario rather than hanging the report.
    let ctx = TestContext::new(&sc.input);
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cythan_driver::run_context::run_bin_with_limit(&bytecode, ctx, sc.step_limit)
    }));
    let (vm_steps, ctx_mutex) = match run {
        Ok(r) => r,
        Err(_) => return Err(format!("VM exceeded step limit of {}", sc.step_limit)),
    };
    let captured = ctx_mutex.lock().unwrap();
    let output = captured.as_str().into_owned();
    if !output.contains(sc.expect_contains) {
        let tail = &output[output.len().saturating_sub(400)..];
        return Err(format!(
            "expected {:?} in output; tail:\n{}",
            sc.expect_contains, tail
        ));
    }
    Ok(Metrics {
        vm_steps,
        output_bytes: captured.print.len(),
        bytecode_words: bytecode.len(),
    })
}

fn format_report(results: &[RunResult]) -> String {
    let mut out = String::new();
    out.push_str("cythan-V4 benchmark report — Cythan VM backend\n");
    out.push_str("\n");
    out.push_str(
        "game       scenario                       vm_steps  out_bytes  bytecode_words\n",
    );
    out.push_str(
        "----       --------                       --------  ---------  --------------\n",
    );
    for r in results {
        match &r.outcome {
            Ok(m) => {
                out.push_str(&format!(
                    "{:<10} {:<30} {:>8}  {:>9}  {:>14}\n",
                    r.scenario.game,
                    r.scenario.name,
                    m.vm_steps,
                    m.output_bytes,
                    m.bytecode_words,
                ));
            }
            Err(reason) => {
                out.push_str(&format!(
                    "{:<10} {:<30}  FAILED: {}\n",
                    r.scenario.game, r.scenario.name, reason
                ));
            }
        }
    }
    out
}

/// Run every scenario, record metrics, write `benchmarks/games.txt`,
/// and fail the test if any scenario errored.
///
/// Report is sorted `(game, scenario)` so line-level diffs across
/// commits are clean.
///
/// Gated behind `#[ignore]` because the Cythan VM runs for tens of
/// seconds (Chess alone is 550K steps). Regenerate the committed
/// report with:
///
/// ```bash
/// cargo test --bin cythan-v4 bench_games_report -- --ignored --nocapture
/// ```
#[test]
#[ignore = "long-running: regenerates benchmarks/games.txt via the Cythan VM"]
fn bench_games_report() {
    let mut scs = scenarios();
    scs.sort_by_key(|s| (s.game, s.name));

    let results: Vec<RunResult> = scs
        .into_iter()
        .map(|sc| {
            let outcome = run_scenario(&sc);
            RunResult { scenario: sc, outcome }
        })
        .collect();

    let report = format_report(&results);
    // eprintln so `cargo test -- --nocapture` shows the report too.
    eprintln!("{}", report);
    let path = PathBuf::from("benchmarks").join("games.txt");
    std::fs::create_dir_all(path.parent().unwrap()).expect("create benchmarks dir");
    std::fs::write(&path, &report).expect("write benchmarks file");
    eprintln!("wrote {}", path.display());

    // Now fail the test if anything failed — but only after the
    // report has been written, so the user can read the current
    // state even when a scenario regresses.
    let failures: Vec<_> = results
        .iter()
        .filter_map(|r| match &r.outcome {
            Err(reason) => Some(format!("{}/{}: {}", r.scenario.game, r.scenario.name, reason)),
            Ok(_) => None,
        })
        .collect();
    if !failures.is_empty() {
        panic!("{} scenario failure(s):\n{}", failures.len(), failures.join("\n"));
    }
}
