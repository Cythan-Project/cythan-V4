# Architectural Backlog

Weakest seams in the compiler, ranked by current pain and how load-bearing
they'll become. Items earlier in the list tend to unblock later ones.

## 1. Name-based type identity (biggest) — DONE

`TypeRegistry.types: HashMap<String, TypeInfo>` keyed by display name.
The cross-file collision work made this visible: `bare_aliases`,
`file_declarations`, `first_declarer`, `ambiguous_bare`, `type_aliases`,
`imports`, plus a ~60-line `canonicalize_type_name` fallback chain.

**Step 1 (done):** Introduced `TypeId(u32)` / `TraitId(u32)` opaque
handles. Storage moved to dense `Vec<TypeInfo>` / `Vec<TraitInfo>`;
names live in a side `HashMap<String, TypeId>`. Every lookup goes
through accessors (`get_type`, `has_type`, `type_id`, `type_by_id`, …).
Cross-file collision migration became a name-key rename — the
underlying `TypeInfo` never moves, so any held `TypeId` stays valid.

**Step 2 (done):** `MethodInfo.from_trait` and `BoundRef.trait_name`
now store `Option<TraitId>` / `TraitId`. Trait existence is checked at
registration (fail-fast on typos); method-dispatch comparisons are
integer equality. `FunctionDB` / `SimpleFn` / `TemplatedFn` still
surface names as strings at their public API — that's the boundary
where IDs cross back into user-facing form.

**Step 3 (done):** Collapsed the name-resolution side tables:
`bare_aliases` is gone entirely (FQ paths register directly in
`type_ids` / `trait_ids` alongside the bare form), and
`file_declarations` + `type_aliases` merged into `file_type_scope` /
`file_trait_scope` (per-file `name → TypeId`/`TraitId`). `use`
statements deferred-resolve through `pending_use_aliases` in a
sub-pass. New primary API: `resolve_type_id(name, file_id)` /
`resolve_trait_id(name, file_id)`; `canonicalize_type_name` is a
back-compat shim that delegates to the canonical-key table.

**Remaining (future):** `SlotInfo.type_name`, `ImplInfo.{trait_name,
target_name}`, and the various HIR paths still thread strings. Moving
those to IDs is a separate, larger refactor — touches `hir/gen.rs`
extensively. Not blocking; flagged as a follow-up.

## Games + new-pipeline harness — DONE

`cythan_driver::new_pipeline` (see `crates/driver/src/new_pipeline.rs`)
is the test harness for the new pipeline. It takes source files +
entry + scripted input, compiles via `new_parser` → `typer` → `hir`
→ `mir`, runs the MIR with a capturing `RunContext`, and returns
output plus remaining input.

**Ten integration tests** in `src/new_pipeline_tests.rs`:
- 3 harness smoke tests (inline programs for echo / input / compile-run composability).
- 3 Morpion scenarios (win / equality / invalid-input).
- 2 Pendu scenarios (win with `"gramire"` / lose with all wrong letters).
- 1 Game2048 (cycling 1234 input fills the board, triggers `Game over!`).
- 1 Chess (`"4158"` = d1→e8, captures Black king, White wins).

All four games from `cythan/` (old syntax) ported to
`examples/new_syntax/`. New stdlib additions needed during porting:
- `impl Eq for U8` in `std/U8.ct` — enables `==` on U8 for
  `DynArray::contains` and game logic comparisons.
- `DynArray` was using non-existent `Array::getDyn` / `Array::setDyn`;
  redirected to the existing `.get(i)` / `.set(i, v)` dynamic forms.

**Known fix during this work:** char literals (`'-'`, `'O'`, …) were
typed as `U4` (1 cell), truncating them to the low nibble so
`'X'.print()` emitted `8` instead of `X`. Re-typed as `U8`, emitting
high and low nibbles into two cells.

**UTF-8 corruption bug — FIXED.** `RunContext::print` (MIR) and
`IoContext::print` (HIR) used to take `char`. The MIR interpreter
cast each printed byte through `byte as char` — bytes ≥128 became
Latin-1 codepoints that re-encoded to two UTF-8 bytes in the
capture buffer, so Pendu's `gagné` (`C3 A9`) was captured as `Ã©`
(`C3 83 C2 A9`). Both traits now take `u8`; `TestContext.print` and
`CapturedIo.stdout` are `Vec<u8>` with lossy-decode accessors. The
Pendu test asserts on the exact `"Vous avez gagné!"` string.

## Rust-style diagnostics (LSP-ready) — DONE

`errors::Diagnostic` is the structured data model for every compiler
message: `severity`, optional stable `DiagCode` (`E0001` …, `W0001`
…), a `message`, `labels: Vec<Label>` (each `Primary` / `Secondary`,
each with a `FileSpan` + inline message), plus `notes` and `helps`.
All plain data, no terminal formatting — converting to
`lsp_types::Diagnostic` later is mechanical (primary label → range;
secondaries → `relatedInformation`; notes/helps concatenate into the
message; code maps through directly).

**Rendering is a separate concern.** `errors::render_diag` uses
ariadne for colored CLI output (Rust-style with underlines and
carets); `errors::render_plain` produces stable no-color text for
tests and LSP payloads; `errors::render_all` strings multiple
diagnostics together.

**Codes currently issued** (see `errors::codes`):
- `E0001` unknown type (with "did you mean?" via
  Damerau-Levenshtein suggestion)
- `E0003` duplicate type definition (secondary label points at the
  first definition)
- `E0004` duplicate trait definition (same shape)
- `E0011` mutability violation (`cannot assign to X — the binding
  is immutable`, with `consider mut X` help)
- `E0015` unknown field (with "did you mean?" pulled from the
  struct's actual field names)
- `W0001` unused variable (suppressed by the `_name` convention,
  with `prefix with an underscore` help)

**Infrastructure for scale:**
- `TypeInfo` / `TraitInfo` now carry `decl_span: Option<Span>` and
  `decl_file: Option<String>` so diagnostics can point at the
  original declaration on cross-file collisions.
- `TypeRegistry.file_names` keeps the raw filename per-`FileId` for
  diagnostic citation.
- `TyperError` and `HirError` gained an optional `diagnostic` field
  plus `from_diagnostic` / `into_diagnostic(file)` helpers. Legacy
  call sites keep emitting plain `message + span`; new sites upgrade
  by building a rich `Diagnostic`. `into_diagnostic` converts
  legacy errors into minimal diagnostics at the API boundary.
- `HirFunction` gained `warnings: Vec<Diagnostic>`; the HIR
  generator populates them during lowering.

**CLI surface.** `cargo run -- new check <file>` now drives the new
pipeline through a `diagnose()` entry point that returns structured
errors + warnings, and renders them with ariadne. Example:
```
Error: [E0001] cannot find type `Fooo` in this scope
   ╭─[Bad.ct:5:9]
 5 │         Fooo x = Foo { v: 1, };
   ·         ──┬─  
   ·           ╰─── not found in this scope
   · Help: a type with a similar name exists: `Foo`
```

**Tests.** `crates/hir/src/tests/diagnostic_tests.rs` covers every
error kind listed above plus both rendering modes — 9 tests.

## HIR specialization pass — DONE (domain tracking)

Flow-sensitive **domain** propagation + match folding on the inlined
HIR body. Runs in `hir::specialize::specialize_to_fixpoint` — wired
into `new_pipeline::compile` between `inline_program_full` and
`hir_to_mir`.

The pass tracks, per slot, the **set of possible u4 values** as a
16-bit bitmask (`Domain(u16)`). Default is `Domain::ALL` — we know
nothing. Bits get cleared as the pass moves through ops; arms
whose value lists don't intersect the scrutinee's domain are
pruned from the `Match` entirely; arms that fully contain the
scrutinee's domain get their body spliced in place.

**What it folds:**
1. **Constant propagation through `Copy`.** `Set(s0, 7);
   Copy(s1, s0)` → `Set(s0, 7); Set(s1, 7)` (Domain collapses to
   a singleton, then `Copy` → `Set`).
2. **`Inc` / `Dec` on known slots.** The whole bitmask shifts by
   one with wrap-around, so even non-singleton domains get
   refined.
3. **`Match` folding on contained scrutinee.** When the scrutinee's
   domain is a subset of some arm's values, that arm always fires
   and is spliced in; the rest of the `Match` is dropped.
4. **Dead-arm pruning.** Arms whose value set doesn't overlap the
   current scrutinee domain vanish from the `Match`. Example: in
   the `else` arm of an outer `if_zero(s0, …)`, `s0 ∈ {1..=15}`;
   a nested `Match(s0, [[0, 5], [10]])` drops the `[0]`-only arm
   and narrows `[0, 5]` effectively to `{5}`.
5. **Arm-local domain narrowing.** Entering an arm narrows the
   scrutinee domain to that arm's values. `if x == 0 { if x == 0 { … } }`
   collapses at the first fixpoint round — the inner match's
   domain is `{0}`, wholly in the `[0]` arm, so the body is
   spliced.
6. **`WriteRegister` on known source.** `Set(s0, 7);
   WriteRegister(1, slot=s0)` → `WriteRegister(1, literal=7)`.

**Loop / Call / Block boundaries** clear the context of every slot
mutated inside them — a safe over-approximation. Arm join after
a `Match` clears only slots some arm mutated; untouched slots'
domains survive. Joining arms' post-states into a union of
domains (instead of clearing) is a future extension.

**Impact on the games benchmark** (Cythan VM step count):

| scenario                 | pre-specialize | post-specialize |       Δ |
|--------------------------|---------------:|----------------:|--------:|
| chess fool_mate_4158     |        555,158 |         554,486 |    −672 |
| morpion cats_game        |        125,937 |         124,278 |  −1,659 |
| morpion diagonal_o_wins  |         98,250 |          97,053 |  −1,197 |
| pendu win_gramire        |        123,103 |         123,103 |       0 |
| pendu lose_hhhhhh        |         93,357 |          93,357 |       0 |

Bytecode size: Morpion 17,512 → 15,156 words (−13%), Chess 65,458
→ 61,714 words (−6%).

**Tests:** `crates/hir/src/tests/specialize_tests.rs` — 10 tests
covering constant propagation, match folding, arm-local
knowledge, `Inc`/`Dec` tracking, loop-boundary conservatism,
literal-WriteRegister collapse, nested-`if_zero`-in-else
folding, and unreachable-arm pruning.

Plus 6 unit tests on the `Domain` helper itself
(singleton / from_values / inc / dec / intersect / subset) in
`hir::specialize::domain_tests`.

## HIR `If0` → unified `Match` — DONE

`HirOp::If0` removed from the IR. All zero-versus-nonzero branching
now routes through a 2-arm `Match` constructed by the new helper
`HirOp::if_zero(slot, then, else)` (first arm = `[0]`, second arm =
`1..=15`). The HIR→MIR lowering drops its `If0` branch entirely —
everything lowers through `Mir::Match`. `text_dump` detects the
if-zero shape and pretty-prints it as `if s == 0 { … } else { … }`
for readability.

**Defense in depth added during this work:** the MIR interpreter and
the HIR interpreter gained step-limit fields (`MemoryState::step_limit`
/ `Interpreter::step_limit`). Tests use `MemoryState::new_with_limit`
with 5M ops; the HIR interpreter defaults to 2M. Runaway programs
now fail loudly via `InterpError::StepLimit` / `aborted_by_limit`
instead of hanging CI.

**Bug surfaced + fixed:** a silent-fall-through in the MIR `Match`
interpreter for the if-zero shape. `Set(slot, 42)` on a u4 cell
stored `42` literally, but the if-zero arms only covered `0` and
`1..=15`. Inside a `Loop` the `Match` fell through and the condition
never took a branch → infinite loop in `42 - 40` on `U4`. Fix: the
MIR interpreter now masks `Set` / `Copy` to 4-bit cell range. New
regression test `u4_literal_above_cell_range_still_terminates` in
`src/new_pipeline_tests.rs` guards it.

## New pipeline is the only pipeline — DONE

The legacy CLI (`run` / `build` / `inspect` / `precomp` / `exe`
subcommands), the `cythan-frontend` crate, the `crates/parser` crate,
the legacy stdlib under `cythan/`, `crates/driver/src/build_context.rs`,
and `src/tests/mod.rs` (legacy 20 tests) are all gone. The top-level
CLI now has exactly three subcommands — `check` / `build` / `run` —
and they all route through the new pipeline (`new_parser` → `typer`
→ `hir` → `mir` → `lir` → bytecode).

### `run --backend mir|lir|cythan`

- `mir`: run on the MIR interpreter (`mir::MemoryState`). Fastest,
  no bytecode lowering.
- `lir`: lower MIR → LIR → bytecode, run on `InterruptedCythan`. Same
  name kept to express "via the LIR path" — semantics identical
  to `cythan`.
- `cythan`: same bytecode + VM path as `lir`. Accepts `vm` as an
  alias.

### `build --hir/--mir/--lir/--cythan`

Each flag is optional; at least one must be supplied. `--hir` is
per-function (no entry needed); `--mir` / `--lir` / `--cythan` inline
from the entry point (`--entry-type`, `--entry-method`, defaults to
file stem / `main`). The cythan output is the raw bytecode as a
space-separated list of decimal words — same format the legacy
`inspect` command produced.

### `cythan_compiler` panic on LIR peephole — FIXED

**Symptom:** Morpion (and any other program with an if-zero match
whose then-branch body reduces to a single jump) panicked on the
LIR→bytecode step with "Try to init your label at an index: 'lH…".

**Root cause:** in `crates/lir/src/optimizer.rs`, `opt_asm` does a
peephole rewrite `Label A; Jump B` → `Jump B` and remaps references
to `A` via `remap()`. The pre-fix `remap` only walked `Jump`,
`Label`, and `If0` — `CompilableInstruction::Match`'s 16-slot jump
table was not updated. When an arm body reduced to `jump end`, its
entry label got eliminated but the match's slot still pointed at
it, and the external `cythan_compiler` couldn't resolve it.

**Fix:** extend `remap` to walk `Match`'s slot array in the same
pass. One-line conceptual change; diff is a handful of lines.

**Regression test:**
`src/new_pipeline_tests.rs::morpion_runs_on_cythan_backend_after_lir_remap_fix`
runs Morpion end-to-end through the Cythan VM with scripted input
and asserts on the win message.

## CLI toolchain — superseded by "New pipeline is the only pipeline"

See that section below — the CLI was flattened (`cythan check`,
`cythan build`, `cythan run` at the top level) and the legacy
subcommands were deleted. Kept for reference so future diffs land
on the current surface.

## 2. Two parsers in-tree — DONE

`crates/frontend` (old hand-written tokenizer+parser) and
`crates/parser` (older chumsky attempt) have been deleted. The
chumsky-based `crates/new_parser` (package `cythan-parser`) is the
only parser; every crate in the tree routes through it.

## 3. `registry.rs` is a god module (~2100 lines)

Holds registration, collision migration, canonicalization, extension
merging, impl validation, blanket-impl fixpoint attachment, bound
satisfaction, and size computation.

Blanket attachment iterates `O(blankets × types × rounds)` and
duplicates method entries onto every candidate.

**Rework:** split into `resolve::` / `register::` / `bounds::` /
`layout::` submodules. Make blanket impls lazy (resolve on method-lookup
instead of attach-on-register).

## 4. Methods duplicated across `TypeInfo.methods` and `FunctionDB`

Two sources of truth, synced by iteration order. Any new dispatch rule
needs both sides touched.

**Rework:** collapse to one. Removes most of the `type_name` juggling
the collision fix just had to thread through.

## 5. AST leakage through the IR

`SlotInfo.type_args: Vec<ast::TypeOrValue>`, blanket `trait_args`,
`BoundRef.trait_args` — all still carry AST with spans. There's no typed
IR between "parsed" and "HIR"; the typer decorates the AST in-place.

**Rework:** a proper `typed::Type` that decouples the stages and makes
substitution trivial.

## 6. Monomorphization via HIR inlining

Generics instantiate by inlining at HIR→MIR. Works, but duplicates
bodies per call site rather than per instantiation, and
`inline_program_full` runs to fixpoint.

**Rework:** a real monomorph pass that produces distinct `Fn` entries
keyed by `(FnSig, ConcreteArgs)`. Gives a stable call graph and makes
incremental compilation possible.

## 7. Native methods bypass the new pipeline — PARTIALLY FIXED

The legacy `crates/frontend/src/natives/` is gone (deleted with the
rest of the old frontend). Natives now live in
`crates/hir/src/natives.rs` — `NativeProvider` / `NativeEmitter`
plug into HIR gen via `gen_function_with_natives`, so every native
is seen by the typer pipeline.

The only "true" natives surviving are the register set/get
intrinsics on `System` (see `BuiltinNatives` in `hir::natives`);
every operator now lives in the stdlib as a trait impl. Remaining
future work: give native declarations a dedicated AST form so
signature-only stubs can be stated without the empty-body trick the
stdlib currently uses (e.g. `Array<T, E, F>::new()`).

## Minor but real

- ~~`0 = true`, `1 = false` — persistent footgun.~~ Fixed: the new
  pipeline has `true == 1`, `false == 0` (conventional). Guarded by
  the `gen_if` lowering (`HirOp::if_zero(cond, else, then)`) and the
  existing Bool tests.
- No `mod` keyword: paths derived from filenames
  (`std/Ops.ct` → `std::Ops` is convention, not syntax). Low priority
  while files stay one-type-each.
- No `dyn Trait`; all dispatch static. Deliberate — no heap in this
  VM means no vtables either.
- Error collection is now consistent: `from_files` /
  `from_registry` collect to `Vec<TyperError>`; the HIR gen and
  inliner still bail on first error per-function, which is usually
  the right shape. `diagnose()` in
  `cythan_driver::new_pipeline` wraps each pass's error set into a
  single `DiagnosticReport` for the CLI and future LSP.

## Suggested order for remaining architectural items (#3–#6)

Items #3 (registry.rs size), #4 (method list duplication), #5 (AST
leakage), #6 (monomorphization via inlining) are each big enough to
warrant a dedicated session. Item #1's Step 2 (propagate `TypeId` /
`TraitId` outward into `SlotInfo` / `ImplInfo`) would land #4 and #5
in one pass — start there.
