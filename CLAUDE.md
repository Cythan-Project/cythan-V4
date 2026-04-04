# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Cythan V4 is an OOP-like compiler for the Cythan abstract machine. It compiles `.ct` source files (Java/Rust-like syntax) through multiple IR stages down to Cythan VM bytecode. The goal is to eventually reimplement the compiler using chumsky with a better-designed pipeline.

## Build & Run Commands

```bash
cargo build                                    # Build everything
cargo test                                     # Run all 20 tests
cargo test test_val                            # Run a single test
cargo run -- run <File>                        # Compile and run a .ct file
cargo run -- run <File> -o                     # Run with MIR optimization
cargo run -- build <File> -o out.cir           # Compile to binary
cargo run -- build <File> -o out.cir -O        # Compile optimized
cargo run -- build <File> -o out.cir --dump-mir out.mir --dump-lir out.lir --dump-asm out.v3
cargo run -- inspect <in.cir> <out.txt>        # Decode bytecode
cargo run -- exe <in.cir>                      # Execute binary
```

Source files must be in the `std/` directory. The filename is without extension (e.g., `cargo run -- run Morpion`).

## Compilation Pipeline

```
Source (.ct) --> Tokenizer --> Parser --> AST (Classes/Methods/Expressions)
    --> Compiler (type checking + monomorphization) --> MIR
    --> MIR Optimizer --> LIR (labels + jumps) --> Cythan V3 Bytecode
```

## Workspace Crates

```
crates/
├── errors/      Error reporting (ariadne). Used by all crates.
├── vm/          Cythan VM and bytecode format (package: cythan). Standalone.
├── lir/         Low-level IR: flat instructions (Copy, Jump, If0, Match, Stop).
│                Compiles to V3 bytecode via cythan_compiler.
├── mir/         Mid-level IR: structured ops (Set, Copy, If0, Loop, Match...).
│                MIR optimizer + MIR interpreter. Depends on lir for to_asm().
├── frontend/    Parser + compiler + natives (package: cythan-frontend).
│                Tokenizes .ct source → AST → MIR. Depends on errors, mir.
└── driver/      Orchestration (package: cythan-driver). Compile + run + test.
                 Depends on frontend, mir, lir, cythan.
src/
├── main.rs      Thin CLI binary (clap). Depends on driver, mir, lir, cythan.
└── tests/       Integration tests (20 tests covering all language features).
std/             Cythan standard library + test/game programs (.ct files).
```

## Key Architecture Details

- **Monomorphization**: Each unique template instantiation generates separate code. Templates can be types or integer sizes.
- **Memory model**: All variables are statically allocated to fixed memory slots (u32 addresses). No heap.
- **Native methods**: `Val`, `System`, and `Array` have native implementations in `crates/frontend/src/natives/` that emit MIR directly.
- **Standard library**: Written in Cythan itself under `std/`.
- **IO model**: Register-based. `System.setRegister<N>(value)` / `System.getRegister<N>()` with registers 0-3.
- **Bool semantics**: 0 is true, 1 is false (inverted from typical conventions).
- **VM value encoding**: In the Cythan VM bytecode, value 0 is encoded as 16 (`base_as_pow`). The input handler in `crates/vm/src/implementations/interrupted.rs` converts 0-valued nibbles to 16 to match this convention.

## Test Structure

Tests are in `src/tests/mod.rs`. Each test compiles a `.ct` program via the MIR interpreter (not the bytecode VM), feeds mock input via `TestContext`, and asserts exact output. Test programs in `std/`: Morpion, Pendu, Game2048, Chess, plus 15 targeted feature tests (TestVal, TestBool, TestByte, etc.).

## File Formats

- `.ct` - Cythan source files (in `std/`)
- `.cir` - Compiled binary (Cythan V3 bytecode, varint-encoded)
- `.mir` - Human-readable MIR dump (opt-in via `--dump-mir`)
- `.lir` - Human-readable LIR dump (opt-in via `--dump-lir`)
- `.v3` - V3 assembly text (opt-in via `--dump-asm`)
