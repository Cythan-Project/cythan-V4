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

**Known limitation:** the MIR interpreter casts each printed byte to
`char` via `byte as char`, which maps bytes ≥128 to Latin-1
codepoints that then re-encode to two UTF-8 bytes in the captured
`String`. Tests with non-ASCII output (e.g. French `é` in Pendu's
`"Vous avez gagné!"`) assert on ASCII-only substrings. A proper fix
would capture output as `Vec<u8>` instead of `String`, or push bytes
directly via `push(c)` that takes a raw byte.

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

## CLI toolchain for the new pipeline — DONE

`cythan new <command>` drives the new pipeline from the command line:

- `cythan new check <file>` — run the full new pipeline up through
  HIR gen; exits non-zero on any error, or prints a one-line summary
  `ok: <N> types, <N> traits, <N> functions (<N> simple)`.
- `cythan new build <file> [--hir <file.hir>] [--mir <file.mir>]`
  — compile and dump either or both IRs as human-readable text.
  `--hir` is per-function (sorted by `FnSig` for stable diffs);
  `--mir` inlines from an entry point (`--entry-type`, `--entry-method`;
  defaults: file stem, `main`) and dumps the flat `MirCodeBlock`
  using `Mir`'s own `Display`.
- `cythan new run <file>` — full compile + MIR interpret, wired to
  stdin/stdout. Accepts `--entry-type` / `--entry-method` (defaults:
  file stem / `main`) and `--mem-cells` (default 4096).

A `--new-std-dir` global flag selects the stdlib (defaults to
`examples/new_syntax/std`). Internal API lives in
`cythan_driver::new_pipeline::{check, build_hir, hir_to_text,
compile, compile_and_run, gather_files}`; the HIR text format is
implemented in `crates/hir/src/text_dump.rs`.

Three toolchain unit tests (`toolchain_*` in
`src/new_pipeline_tests.rs`) cover the OK/error paths of `check` and
the output shape of `build-hir`. All four migrated games pass
`cythan new check` cleanly.

## 2. Two parsers in-tree

`crates/frontend` has the old hand-written tokenizer+parser driving the
MIR pipeline and native lowering. `crates/new_parser` is chumsky-based
and feeds the typer/HIR path. Tests run both.

**Rework:** either finish the migration off the legacy crate or tombstone
it harder. The legacy side is currently the longest pole for anything
language-level.

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

## 7. Native methods bypass the new pipeline

`crates/frontend/src/natives/` emits MIR directly. Anything added there
is invisible to the typer.

**Rework:** a "native declaration" form that participates in typing
(signature-only, body = intrinsic tag) to unify the two paths.

## Minor but real

- `0 = true`, `1 = false` — persistent footgun.
- No `mod` keyword: paths derived from filenames
  (`std/Ops.ct` → `std::Ops` is convention, not syntax).
- No `dyn Trait`; all dispatch static.
- Error collection inconsistent: `from_files` collects,
  most everything else bails on first `?`.

## Suggested order

Start with **#1** (intern types → TypeId). It unblocks #3, #4, and #5
simultaneously — once identity stops being a string, the god-module
splits naturally along the type/function/dispatch axes.
