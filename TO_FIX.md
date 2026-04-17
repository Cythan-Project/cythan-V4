# Architectural Backlog

Weakest seams in the compiler, ranked by current pain and how load-bearing
they'll become. Items earlier in the list tend to unblock later ones.

## 1. Name-based type identity (biggest) — PARTIALLY DONE

`TypeRegistry.types: HashMap<String, TypeInfo>` keyed by display name. The
cross-file collision work made this visible: `bare_aliases`,
`file_declarations`, `first_declarer`, `ambiguous_bare`, `type_aliases`,
`imports`, plus a ~60-line `canonicalize_type_name` fallback chain — all
just to decide "which type?".

**Step 1 (done):** Introduced `TypeId(u32)` / `TraitId(u32)` opaque
handles. Storage moved to dense `Vec<TypeInfo>` / `Vec<TraitInfo>`;
names live in a side `HashMap<String, TypeId>`. Every lookup now goes
through accessors (`get_type`, `has_type`, `type_id`, `type_by_id`, …).
Cross-file collision migration became a pure name-key rename — the
underlying `TypeInfo` never moves, so any held `TypeId` stays valid.

**Step 2 (pending):** Propagate `TypeId` outward. Today external
callers (HIR gen, FlatSig, monomorph) still pass strings through the
accessors; they should hold `TypeId` directly and stop re-resolving by
name on every lookup. That requires `SlotInfo`, `MethodInfo.from_trait`,
`BoundRef.trait_name`, `ImplInfo.{trait_name, target_name}`, etc. to
carry IDs.

**Step 3 (pending):** Collapse the name-resolution side tables
(`bare_aliases`, `file_declarations`, `first_declarer`,
`ambiguous_bare`, `type_aliases`) into a single file-aware
`resolve(name, file_id) -> Option<TypeId>` entry point.

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
