# Array support — design doc

`Array<T, N, F>` is the language's statically-sized array. Its layout is
`N * sizeof(T)` contiguous cells. After this work:

- `fn new(): Self` — zero-initializes N*sizeof(T) cells.
- `fn get(self, F index): T` — returns the element at `index` (runtime).
- `fn set(mut self, F index, T value)` — writes the element at `index`.
- `fn len(self): F` — returns the compile-time constant `N`.

No static-index variants. `get<N>()` / `set<N>()` are removed — a runtime
`get(i)` lowered to a jump-table `Match` is just as efficient for N ≤ 16.

## Why monomorph-generated, not native-handler

The Phase-7 native handlers worked, but they needed `receiver_type_args`
to be threaded through every call site. Two problems:

1. Threading concrete type args through `infer_expr_type`, `LocalBinding`,
   and `gen_method_call` is a larger refactor than it looks (type
   information touches every expression kind).
2. Even when threaded, the native emits ops *inline at every call site*,
   producing N copies of the same Match table. A per-monomorph
   `HirFunction` is DRY and benefits from the standard inlining path.

So: at monomorph-request time, synthesize a `HirFunction` per concrete
`Array<T, N, F>::{new, get, set, len}` and let the existing inliner splice
it in like any other callee.

## Pipeline

```
HIR gen emits Call { type: "Array", method: "get", template_args: [], ... }
  (for `arr.get(i)` where `arr: Array<T, N, F>`)

           │   ↓ currently target lacks type args
           │
           ▼

HIR gen knows receiver's concrete type args   ← the threading gap (task #50)
  → Call { template_args: [T, N, F] }

           │
           ▼

Inliner encounters Call to Array::get
  → extracts template args
  → asks `ArrayMonomorphs::get_or_synthesize(t, n, f)`
     which returns a concrete HirFunction
  → splices it in (standard callee inlining path)
```

## Key design calls

1. **`Array::new()` returns `Self`** — but `Self` means `Array<T, N, F>`.
   Synthesis: emit N*sizeof(T) `Set(ret_slot, 0)` ops. Done.

2. **`get(self, F index): T`** body:
   ```
   Match(index_slot, arms)
     where arm i: Copy(ret_slot[0..elem_size], self_slot[i*elem_size ..
                        (i+1)*elem_size])
   ```
   One arm per position, matching discriminant `i` (1 byte for U4-index,
   need higher-arity match for U8-index).

3. **`set(mut self, F index, T value)`** body: symmetric — Match, each arm
   copies from value slots into the i-th cell range of self.

4. **`len(self): F`** body: `Set(ret_slot[0], N % 16); Set(ret_slot[1],
   (N/16) % 16); ...` spreading N across however many cells F occupies.

5. **FlatSig for a synthesized `get`**:
   - self: N*sizeof(T) cells (immutable)
   - index: sizeof(F) cells (immutable)
   - _ret: sizeof(T) cells (mutable)

6. **Cache** by `(T, N, F)` keyed `MonomorphKey` — two `Array<U4, 3, U4>`
   variables share one monomorph. Different `N` values instantiate
   different functions.

## Receiver-type-args threading (task #50) — the prerequisite

Without this, the inliner never knows which `Array<T, N, F>` is being
called. Minimal surgery:

- Extend `LocalBinding` with `template_args: Vec<ConcreteTemplateArg>`.
- Populate from declarations: `mut Array<Cell, 9, U4> grid = ...` records
  `[Cell, 9, U4]`.
- For `self` inside a method on a generic type — look up the enclosing
  type's template parameters, but since we're inside a *concrete* context
  (the enclosing method has already been monomorphized with those args,
  OR was never generic), we know them directly from `SimpleFn.type_name`
  after mangling. For now, only inherent methods of concrete types
  flow through; Array monomorphs populate `self`'s template_args
  automatically because they're synthesized with concrete values.
- For struct-field reads (`self.grid`), the field's declared type
  contains the template args — the struct layout needs to remember the
  AST types, not just cell sizes. This is a small addition to
  `StructLayout.fields` (add `type_args: Vec<ConcreteTemplateArg>`).
- `infer_expr_type` starts returning `ConcreteType` instead of a bare
  `String`. Or simpler — keep the `String` return for the name but add a
  parallel helper `infer_expr_type_args` for when template args matter.
- `gen_method_call` reads the receiver's concrete template args and
  populates the `FnRef.template_args` list.

I'll take the "parallel helper" route because `infer_expr_type` is used
widely; reshaping its return type would be invasive. `infer_expr_type_args`
returns `Option<Vec<ConcreteTemplateArg>>` — None when we can't tell.

## Synthesis (task #51)

New module `hir/src/array_synth.rs`:

```rust
pub struct ArraySpec {
    pub element: ConcreteType,
    pub size: u32,
    pub index: ConcreteType,
}

pub fn synth_new(spec: &ArraySpec, reg: &TypeRegistry) -> HirFunction;
pub fn synth_get(spec: &ArraySpec, reg: &TypeRegistry) -> HirFunction;
pub fn synth_set(spec: &ArraySpec, reg: &TypeRegistry) -> HirFunction;
pub fn synth_len(spec: &ArraySpec, reg: &TypeRegistry) -> HirFunction;
```

Each builds FlatSig + HirBlock directly, no AST involved.

## Inliner dispatch (task #52)

When `inline_call` encounters a target whose `type_name == "Array"` and
`method_name` is one of {new, get, set, len}:

1. Read `FnRef.template_args` — must be `[T, N, F]` (panic otherwise:
   means the threading step dropped them).
2. Build an `ArraySpec`.
3. Check a per-inliner cache `HashMap<(ArraySpec, method), HirFunction>`.
4. On miss: synth the function, store, insert into `self.functions`
   under the mangled key, continue with the normal dispatch path.
5. On hit: proceed with the cached entry.

Mangle monomorph keys as
`FnSig { type_name: "Array<Cell,9,U4>", method_name: "get", trait_name: None }`
so they don't collide with the generic `FnSig::new("Array", "get")` that
sits unused in the DB as `Fn::Templated`.

## Stdlib shape after this work

`std/Array.ct` becomes a stub of signatures (no bodies — `{}`):

```cythan
struct Array<T, E, F> {}

extension Array<T, E, F> {
    fn new(): Self {}
    fn get(self, F index): T {}
    fn set(mut self, F index, T value) {}
    fn len(self): F {}
}
```

No `getDyn`/`setDyn`/`get<N>`/`set<N>`. Users write `arr.get(i)` and the
compiler synthesizes behavior per concrete `Array<T, N, F>`.

## What this doesn't do

- Arrays of generic structs whose layout isn't yet known (monomorphization
  of user-defined generic structs is still needed in the general case).
  For Morpion, `Array<Cell, 9, U4>` is fully concrete (Cell is a non-
  generic enum), so this works.
- Out-of-bounds handling. `arr.get(5)` on an `Array<T, 4, F>` silently
  runs the first arm whose discriminant covers 5 — usually no arm, so
  the Match falls through and the ret slot keeps its previous value
  (zero for a fresh local). That matches "out-of-bounds is UB" in the
  VM's cell model.

## Rollout order

1. ✅ Rename `getDyn`→`get`, `setDyn`→`set` in stdlib.
2. ✅ Strip `self.get<N>()` from Morpion.
3. Struct layout carries concrete template args per field.
4. `infer_expr_type_args` helper.
5. `gen_method_call` populates `FnRef.template_args`.
6. `array_synth` module.
7. Inliner Array-dispatch path.
8. Unignore Array tests, watch them go green.
9. Unignore `morpion_inline_end_to_end`, fix remaining gaps.
