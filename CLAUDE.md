# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Cythan V4 is a compiler for the Cythan abstract machine. It takes
`.ct` source files (Rust-flavoured syntax: `struct` / `enum` / `trait`
/ `extension` / `impl`) through a staged IR pipeline down to Cythan
VM bytecode.

## Build & Run Commands

```bash
cargo build                                 # Build everything
cargo test                                  # Run the full workspace test suite
cargo run -- check <file>                   # Type-check + HIR gen; prints rust-style errors
cargo run -- build <file> --hir out.hir     # Dump per-function HIR
cargo run -- build <file> --mir out.mir     # Dump inlined MIR
cargo run -- build <file> --lir out.lir     # Dump LIR (post-opt)
cargo run -- build <file> --cythan out.cy   # Dump raw Cythan bytecode
cargo run -- run   <file>                   # Run on MIR interpreter (default)
cargo run -- run   <file> --backend cythan  # Run on Cythan VM (full lowering)
```

`--std-dir` (default `examples/new_syntax/std`) points at the stdlib.
Every `build --*` flag is optional; at least one must be supplied.
Entry point defaults to `<FileStem>::main` and can be overridden with
`--entry-type` / `--entry-method`.

## Compilation Pipeline

```
Source (.ct)
  → new_parser         (chumsky, → AST)
  → typer              (TypeRegistry + FunctionDB, flat signatures, monomorph keys)
  → hir                (per-function HIR: Set/Copy/Match/Loop/Call/...)
  → inliner + monomorph (all Call ops resolved, generics instantiated)
  → mir                (flat ops, slot-addressed)
  → lir                (labelled asm, post-opt)
  → bytecode           (Cythan V3 via cythan_compiler, run on InterruptedCythan)
```

`HirOp::If0` was removed: zero-vs-nonzero branching lowers through a
2-arm `Match` (`[0]` then `1..=15`). `HirOp::if_zero(slot, then, else)`
builds that shape; the text dumper recognises and pretty-prints it as
`if s == 0 { … } else { … }`.

## Workspace Crates

```
crates/
├── errors/      Structured `Diagnostic` + ariadne renderer. Shared by all.
├── vm/          Cythan VM + bytecode format (package: cythan). Standalone.
├── lir/         Low-level IR: flat instructions (Copy, Jump, If0, Match, Stop).
│                Compiles to V3 bytecode via cythan_compiler.
├── mir/         Mid-level IR: Set/Copy/Inc/Dec/Match/Loop/... + interpreter.
│                Interpreter gained a `step_limit` so tests fail loudly on
│                runaway loops instead of hanging CI. Depends on lir.
├── new_parser/  Chumsky parser (package: cythan-parser). Source → AST.
├── typer/       Registry + FunctionDB + FlatSig. Resolves names via
│                `TypeId`/`TraitId` handles; dense storage + side lookup
│                maps. Carries decl_span/decl_file for rich diagnostics.
├── hir/         HIR generator + interpreter + inliner + monomorphizer +
│                text dumper. Unused-variable warnings fire during gen.
└── driver/      Orchestration. `new_pipeline::{check, build_hir,
                 compile, run_with_backend, diagnose, hir_to_text,
                 mir_to_text, lir_to_text, bytecode_to_text}`.
src/
├── main.rs              Thin CLI (clap). Three subcommands: check, build, run.
└── new_pipeline_tests.rs  Integration tests for the harness + games.
examples/new_syntax/     Stdlib (std/*.ct) + games (Morpion, Pendu, Chess,
                         Game2048) in the project's syntax.
```

## Key Architecture Details

- **Monomorphization**: per `(FnSig, ConcreteArgs)`; templates can be
  types or integer values.
- **Memory model**: all bindings statically allocated to fixed slot
  addresses. No heap.
- **Native methods**: `System::setRegister<N>` / `System::getRegister<N>`
  are the only true natives (`crates/hir/src/natives.rs`); operators
  (`Add`, `Sub`, `Eq`, `Ord`) live in the stdlib as trait impls.
- **Standard library**: written in Cythan itself under
  `examples/new_syntax/std/`.
- **IO model**: register-based. `setRegister<0>(1)` triggers a byte
  print, `setRegister<0>(2)` pulls a byte from input. The MIR
  interpreter's `RunContext::print` takes a `u8` so multi-byte UTF-8
  sequences survive capture intact.
- **Bool semantics**: `true` is `1`, `false` is `0`. `if cond { … }`
  fires the `then` branch when `cond != 0`.
- **Error reporting**: every diagnostic is an `errors::Diagnostic`
  (severity + optional `DiagCode` like `E0001`/`W0001` + labels +
  notes + helps). `cargo run -- check` renders with ariadne.

## Test Structure

Lib tests live in each crate's `tests/` module. Integration tests for
the full pipeline (harness + games + toolchain) live in
`src/new_pipeline_tests.rs`. Every test that drives a real compiled
program uses `MemoryState::new_with_limit` or `Interpreter::step_limit`
so a bad loop fails in bounded time.

## Per-commit benchmark

`src/bench_games_tests.rs::bench_games_report` runs the games on the
Cythan VM and records step counts to `benchmarks/games.txt`. Gated
behind `#[ignore]` (≈60s per run). Regenerate before a commit so the
diff shows how the change moved the numbers:

```bash
cargo test --bin cythan-v4 bench_games_report -- --ignored --nocapture
git add benchmarks/games.txt
```

Columns: `game`, `scenario`, `vm_steps` (Cythan VM instructions
executed), `out_bytes` (captured transcript length), `bytecode_words`
(size of the compiled program). Scenarios sort `(game, scenario)` for
line-level diffs.

## File Formats

- `.ct`  — Cythan source.
- `.hir` — per-function HIR text dump (from `build --hir`).
- `.mir` — flat MIR text dump (from `build --mir`).
- `.lir` — LIR text dump, post-`opt_asm` (from `build --lir`).
- `.cy`  — raw Cythan bytecode as space-separated decimals
           (from `build --cythan`).
