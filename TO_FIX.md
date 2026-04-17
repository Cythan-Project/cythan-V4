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

## Games + new-pipeline harness — PARTIAL

`cythan_driver::new_pipeline` (see `crates/driver/src/new_pipeline.rs`)
is the test harness for the new pipeline. It takes source files +
entry + scripted input, compiles via `new_parser` → `typer` → `hir`
→ `mir`, runs the MIR with a capturing `RunContext`, and returns
output plus remaining input. Six integration tests in
`src/new_pipeline_tests.rs` cover the harness itself and run Morpion
end-to-end with three distinct input scripts (win / equality /
invalid-input).

**Known fix during this work:** char literals (`'-'`, `'O'`, …) were
typed as `U4` (1 cell), which truncated them to the low nibble and
made `'X'.print()` emit `8` instead of `X`. Re-typed as `U8`,
emitting high and low nibbles into two cells.

**Remaining:** Pendu / Chess / Game2048 live only under `cythan/` in
the OLD syntax — they need porting to `examples/new_syntax/` to go
through the new pipeline. The harness will run them unchanged once
the source files exist.

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
