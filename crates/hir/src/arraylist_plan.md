# ArrayList implementation plan

## Goal

A fixed-capacity, growable list: `ArrayList<Index, N, T>` holds up to `N`
values of type `T`, indexed by type `Index`, and tracks its current length
in an `Index` cell. Backed by `Array<T, N, Index>`.

## Type definition (stdlib)

```cythan
struct ArrayList<Index, N, T> {
    Array<T, N, Index> backing,
    Index size,
}
```

Memory layout:
- `backing`  at offset 0, size `N * sizeof(T)` cells
- `size`     at offset `N * sizeof(T)`, size `sizeof(Index)` cells

Total cells: `N * sizeof(T) + sizeof(Index)`.

## Trait requirement

`Index` must implement `Ord` — specifically `gt`/`lt`/`ge`/`le`. The list
uses `capacity > size` to check fullness; that desugars to `Ord::gt`. The
compiler doesn't enforce trait bounds (no `where` clauses in the grammar),
so this is a convention the author must uphold. `impl Ord for U4` ships
with the stdlib.

## Public API

| Method | Signature | Notes |
|---|---|---|
| `new` | `() -> Self` | Empty list, size=0, backing zero-filled |
| `len` | `(self) -> Index` | Current element count |
| `capacity` | `(self) -> Index` | Constant N (from backing) |
| `is_empty` | `(self) -> Bool` | `size == 0` |
| `is_full` | `(self) -> Bool` | `size >= capacity` — uses `GreaterOp` |
| `push` | `(mut self, T) -> ()` | Silent no-op when full |
| `pop` | `(mut self) -> T` | Undefined when empty; decrements size |
| `get` | `(self, Index) -> T` | Delegates to `backing.get` |
| `set` | `(mut self, Index, T) -> ()` | Delegates to `backing.set` |

## Compiler work required

ArrayList is a user-defined generic struct with fields whose types reference
the struct's own template params (`T`, `N`, `Index`). The current compiler
only monomorphizes Array; generalizing requires three additions.

### 1. Typer: size user-defined generic struct instantiations

When `resolve_type_size` sees a concrete type reference like
`ArrayList<U4, 4, U4>`:

1. Look up the base type `ArrayList` — find `StructKind::Templated { fields }`.
2. Substitute template params (T→U4, N→4, Index→U4) in each field's AST
   type using the existing `monomorph::subst_type`.
3. Recursively size each substituted field.
4. Sum to get total cells.

Cache the resulting `StructLayout` under a mangled name so later field
offset lookups hit the same layout (e.g. `ArrayList<U4,4,U4>`). This
parallels the `Array<...>` mangling.

### 2. HIR gen: lookup fields on generic struct instances

`infer_expr_type` for `Expr::Field(list, "size")` on a
`ArrayList<U4, 4, U4>` must return `"U4"`, not `"<?>"`. Two fixes:

- `resolve_lvalue_base` (used by `gen_assign`, `gen_compound_assign`) needs
  to find the field layout for a generic instance, not just the bare name.
- `field_offset` / `reg_field_type` need to consult the cached concrete
  layout for the instantiation.

The natural hook: pipe through the same `resolve_struct_layout(ty)` helper
that the typer uses.

### 3. Inliner: monomorphize non-Array templated callees

The inliner already special-cases `Array::{new, get, set, len}` by calling
the synthesizer. For ArrayList methods, we invoke `monomorph::monomorphize`
instead — it takes the Templated AST body, substitutes templates from
`FnRef.template_args`, re-runs `gen_function`, and returns a fresh
`HirFunction`. Cache by mangled `FnSig`.

The wiring is a small extension of the existing Array branch: a generic
"if callee is Templated, monomorphize" fallback that runs when the
standard `functions` lookup misses AND the base FnSig has a Templated
entry in the `FunctionDB`.

## Test plan

Grouped by compiler capability they exercise. Ignore markers go away as the
matching compiler work lands.

- **Basic** (flat `ArrayList<U4, N, U4>`): construction, push, len, get,
  pop, is_empty, is_full, capacity.
- **Mutation**: push/set/get round-trip; pop after multiple pushes.
- **Full / empty boundaries**: push beyond capacity silently; pop from
  empty is UB.
- **Iteration**: classic `while i < list.len()` pattern summing elements.
- **Element type variety**: `ArrayList<U4, N, U4>`, `ArrayList<U4, N, U8>`
  (two-cell elements), `ArrayList<U4, N, Bool>`.
- **Nested**: `ArrayList<U4, 3, ArrayList<U4, 2, U4>>`. Requires
  user-defined-struct monomorphization to handle arbitrary element types.

## Scope call

Realistic plan: land steps 1–3 above, get flat ArrayList tests green,
mark nested tests `#[ignore]` initially. Nested works when the
monomorphizer's struct sizing is applied recursively — which it will be,
since `resolve_type_size` walks fields and sizes each. So nested may just
fall out. Will find out once it's wired.
