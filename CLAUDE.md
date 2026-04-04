# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Cythan V4 is an OOP-like compiler for the Cythan abstract machine. It compiles `.ct` source files (Java/Rust-like syntax) through multiple IR stages down to Cythan VM bytecode. The goal is to eventually reimplement the compiler using chumsky with a better-designed pipeline.

## Build & Run Commands

```bash
cargo build                          # Build the compiler
cargo test                           # Run all tests (morpion + pendu integration tests)
cargo test run_test_morpion          # Run a single test
cargo test run_test_pendu            # Run a single test
cargo run run <FileName>             # Compile and run a .ct file (looks in std/)
cargo run build <FileName> <out>     # Compile to binary (.cir)
cargo run build <FileName> <out> <mir_out>  # Compile + dump MIR
cargo run inspect <in> <out>         # Decode bytecode to readable format
```

Source files must be in the `std/` directory. The filename given to the compiler is without extension (e.g., `cargo run run Morpion`).

## Compilation Pipeline

```
Source (.ct) --> Tokenizer --> Parser --> AST (Classes/Methods/Expressions)
    --> Compiler (type checking + monomorphization) --> MIR
    --> MIR Optimizer --> LIR (labels + jumps) --> Cythan V3 Bytecode
```

## Workspace Crates

- **Root crate** (`src/`): Parser, compiler, test harness, CLI entry point
  - `src/parser/` - Tokenizer and parser producing AST (classes, methods, expressions, types)
  - `src/compiler/` - Type-checked compilation from AST to MIR, manages local state, memory allocation, class loading, and template monomorphization
  - `src/actions/` - Build context (loads std/ + natives), run context (VM execution), test context (mock IO)
  - `src/actions/natives/` - Native method implementations for `Val`, `System`, `Array`
- **mir** (`mir/`): Mid-level IR definition and optimizer. MIR ops: Set, Copy, Inc, Dec, If0, Loop, Break, Continue, Match, ReadRegister, WriteRegister. Optimizer does dead code elimination, block inlining, dependency analysis (~40-60% instruction reduction)
- **lir** (`lir/`): Low-level IR with flat instructions (Copy, Inc, Dec, Jump, Label, If0, Match, Stop, register IO). Compiles to Cythan V3 bytecode
- **errors** (`errors/`): Error reporting using ariadne
- **Cythan-V2** (`Cythan-V2/`): The Cythan virtual machine and bytecode format (encode/decode)

## Key Architecture Details

- **Monomorphization**: Each unique template instantiation (e.g., `Array<Val, 3>` vs `Array<Byte, 5>`) generates separate code. Templates can be types or integer sizes.
- **Memory model**: All variables are statically allocated to fixed memory slots (u32 addresses). No heap, no dynamic allocation.
- **Native methods**: `Val`, `System`, and `Array` have native implementations in Rust (`src/actions/natives/`) that emit MIR directly instead of being compiled from `.ct` source.
- **Standard library**: Written in Cythan itself under `std/` (Val.ct, Bool.ct, Byte.ct, Array.ct, Option.ct, DynArray.ct, etc.)
- **IO model**: Register-based. `System.setRegister<N>(value)` / `System.getRegister<N>()` with registers 0-3 controlling IO operations.
- **Bool semantics**: 0 is true, 1 is false (inverted from typical conventions).

## Test Structure

Tests are integration-level in `src/tests/mod.rs`. Each test compiles a `.ct` program, runs it with both optimized and unoptimized compilation, feeds mock input via `TestContext`, and asserts exact output strings. The two test programs are `Morpion` (tic-tac-toe) and `Pendu` (hangman), both in `std/`.

## File Formats

- `.ct` - Cythan source files (in `std/`)
- `.cir` - Compiled binary (Cythan V3 bytecode, varint-encoded)
- `.mir` - Human-readable MIR dump (debug output). `before.mir` and `after.mir` are written during compilation to show optimizer effect
- `MIR_MODE` constant in `main.rs` controls whether MIR interpreter or bytecode VM is used
