# Cythan V4 Compiler Redesign — Detailed Design & Implementation Plan

## Language Model

Everything is by reference, no ownership. Assignment is always COPY (no pointers). Mutability is explicit (`mut self`, `mut param`). Single-threaded. All memory is static. All dispatch is static (monomorphized). Expression-based (last expression = return value in blocks, ifs, matches). No type inference. No visibility modifiers (yet — extensions will carry file IDs for future import-based visibility).
REFINE: No visibility modifiers (yet — extensions will carry file IDs for future import-based visibility).
ACTUALLY: There are no visibility modifier in the sense that you don't have to add pub or private everything is public by default but you should still need to import files and trait, structs... So you don't polule the scope.

## Syntax Reference

```rust
struct Main {
    grid: Array<Val, 9, Val>       // Array<ElementType, const_size, IndexType>
}

enum Option<T> {
    None,                           // discriminant = 0, data unused
    Some(T),                        // discriminant = 1, data = T
}

// Unified enum model: variants can mix unit + data + explicit discriminants.
// Missing `= N` means auto-assigned from free values.
// Size = discr_size + max(variant_data_sizes)
// If all variants are unit → size = discr_size only, can cast to u4/u8.
enum TypeMap {
    A = 0,
    B = 1,
    C(u4) = 2,     // data-carrying with explicit discriminant
    D,              // auto-assigned = 3
}

extension Main {
    fn new(): Self {
        Self { grid: Array::new() }     // implicit return (expression-based)
    }

    fn set(mut self, pos: u4, val: u4) {
        self.grid.setDyn(pos, val);     // mut self required to modify fields
    }

    fn getDyn(self, pos: u4): u4 {
        self.grid.getDyn(pos)           // implicit return
    }
}

trait Add {
    type Other;
    type Result;
    fn add(self, other: Self::Other): Self::Result;
}

impl Add for MyType {
    type Other = MyType;
    type Result = MyType;
    fn add(self, other: MyType): MyType { /* ... */ }
}
```

## Full Pipeline

```
Source (.ct)
  │
  ├─ Stage 1: Lexer ──────── chars → Vec<(Token, Span)>
  │                           crates/parser/src/lexer.rs
  │
  ├─ Stage 2: Parser ─────── tokens → Vec<Item>
  │                           crates/parser/src/parser.rs
  │                           Item = Struct | Enum | Extension | Impl | Trait
  │
  ├─ Stage 3: Type Registry ─ items → TypeRegistry
  │                           NEW crate: crates/typer/
  │                           - register structs/enums, compute sizes
  │                           - merge extensions into target types
  │                           - validate impl blocks vs trait sigs
  │                           - store file IDs on extensions for future imports
  │
  ├─ Stage 4: Flatten Sigs ── TypeRegistry → FlatFunctionDB
  │                           in crates/typer/
  │                           - each function sig → flat list of u4 slots
  │                           - return values become mut output slots appended to params
  │                           - field access → (offset, size) pairs
  │
  ├─ Stage 5: HIR Gen ─────── AST body + FlatSig → HirFunction
  │                           NEW crate: crates/hir/
  │                           - reuses Mir opcodes but scoped per-function
  │                           - has Call(fn_ref, slots) for unresolved calls
  │                           - locals are numbered slots, not global u32 addresses
  │
  ├─ Stage 6: HIR Opt ─────── HirFunction → optimized HirFunction
  │                           in crates/hir/
  │                           - port new_opt.rs passes to work on HirOp
  │                           - constant prop, dead store elim, copy prop
  │
  ├─ Stage 7: Inline ─────── HirFunctions + entry point → MirCodeBlock
  │                           NEW crate: crates/inliner/ (or in crates/hir/)
  │                           - walk from main(), inline all Call ops
  │                           - monomorphize: substitute template params
  │                           - map local slots → global u32 addresses
  │                           - detect cycles → reject recursion
  │                           - output: single flat MirCodeBlock (no Call ops)
  │
  ├─ Stage 8: MIR Opt ─────── MirCodeBlock → optimized MirCodeBlock
  │                           crates/mir/ (UNCHANGED)
  │
  ├─ Stage 9: MIR → LIR ──── MirCodeBlock → Vec<CompilableInstruction>
  │                           crates/mir/ (UNCHANGED)
  │
  ├─ Stage 10: LIR → Bytecode ── Vec<CI> → Vec<usize>
  │                               crates/lir/ (UNCHANGED)
  │
  └─ Stage 11: Execute ──── bytecode → output
                             crates/vm/ (UNCHANGED)
```

## Key Data Structures

### Enum Memory Layout

```
// Discriminant size = smallest Enumerable fitting variant_count.
//   ≤16 variants → u4 (1 cell)
//   ≤256 variants → u8 (2 cells)
//
// Data size = max(variant_data_sizes). 0 if all unit variants.
// Total size = discr_size + data_size.
//
// Example: enum Option<u8> { None, Some(u8) }
//   discr_size = 1 cell (2 variants fit in u4)
//   data_size = 2 cells (u8 = 2 cells)
//   total = 3 cells: [discr][data_lo][data_hi]
//
// Example: enum TypeMap { A=0, B=1, ..., H=7 }
//   discr_size = 1 cell
//   data_size = 0
//   total = 1 cell
//
// Example: enum Mixed { A(u4)=1, B=2 }
//   discr_size = 1 cell
//   data_size = 1 cell (from A's u4 payload)
//   total = 2 cells: [discr][data]
//   Cannot cast to u4 because data_size > 0.
//
// Future optimization: enum packing for Enumerable types where
// discr + all-variant data fits in a single cell. Not in first impl.
```

### Signature Flattening

```
// fn test(self, a: Option<u8>) -> (u4, u8)
// where Self is a u8 (2 cells)
//
// Flattened:
//   slot 0: self_b0      (immut, from caller)
//   slot 1: self_b1      (immut, from caller)
//   slot 2: arg_a_discr  (immut)
//   slot 3: arg_a_d0     (immut)
//   slot 4: arg_a_d1     (immut)
//   slot 5: ret_0        (mut, output)   ← return u4
//   slot 6: ret_1_lo     (mut, output)   ← return u8 low
//   slot 7: ret_1_hi     (mut, output)   ← return u8 high
//
// Total: 8 slots. Params 0-4 are inputs, 5-7 are outputs.
// The function body writes to slots 5-7 to "return" values.
```

### HIR (= MIR but per-function with Call)

```rust
// Initially, HIR reuses the exact same opcodes as MIR plus Call.
// This avoids defining a new IR from scratch. Later, HIR can diverge
// (e.g., add SSA form, phi nodes) without affecting MIR.

enum HirOp {
    // Same as Mir:
    Set(SlotId, u8),
    Copy(SlotId, SlotId),
    Inc(SlotId),
    Dec(SlotId),
    If0(SlotId, Vec<HirOp>, Vec<HirOp>),
    Loop(Vec<HirOp>),
    Break,
    Continue,
    Stop,
    ReadRegister(SlotId, u8),
    WriteRegister(u8, Either<u8, SlotId>),
    Block(Vec<HirOp>),
    Skip,
    Match(SlotId, Vec<(Vec<HirOp>, Vec<u8>)>),

    // NEW: unresolved function call (resolved during inlining)
    Call {
        target: FnRef,              // which function
        args: Vec<SlotId>,          // input slots
        ret: Option<Vec<SlotId>>,   // output slots (mut params at end)
    },
}

struct FnRef {
    type_name: String,
    method_name: String,
    template_args: Vec<ConcreteType>,  // already resolved
}
```

### Trait Resolution

```
// Traits are compile-time only. No runtime representation.
//
// TypeRegistry stores:
//   traits: HashMap<TraitName, TraitDef>
//   impls: Vec<(TraitName, ConcreteType, ImplBlock)>
//
// Resolution at monomorphization:
//   fn resolve(type: &Type, trait: &str, method: &str) -> &Function
//   1. Find impl block: impls.iter().find(|(t, ty, _)| t == trait && ty == type)
//   2. Find method in impl block
//   3. Return function body
//
// Operators (==, +, -, *, <, >) desugar to trait method calls:
//   a + b → Add::add(a, b)
//   a == b → Eq::eq(a, b)
//
// Builtin traits provided by compiler: Eq, Ord, Add, Sub, Enumerable
// Users can implement them for custom types.
```

### Mutability Model

```
// Everything is by reference. No ownership.
// Assignment is COPY: `a = b` copies b's cells into a's cells.
// Mutability is Rust-like: mut propagates to fields.
//   `mut self` → can modify self.field
//   `self` → cannot modify self.field (compile-time error)
//
// Implementation: each slot in HIR has a `mutable: bool` flag.
// Set/Copy/Inc/Dec targeting an immutable slot → compiler error.
// This is checked during HIR generation (stage 5).
//
// No runtime cost. No copies for immutable params.
```

## Things to refine

Have all functions/methods be added to a global HashMap<FnSig, Fn> with FnSig being the static arguments or the function with template types not yet being rendered.

It then should go to a:
enum Fn {
    Templated(TemplatedFn),
    SimpleFn(SimpleFn),
}

struct TemplatedFn {
    templates: Vec<TemplateParam>,
    monomorphs: Map<TemplateSig, SimpleFn>,
    polymorph: NonRenderedTemplateFn,
}

Another thing to refine is trait handling / template on types.

They should get inline into the methods like so.

trait A: B {
    type Output;

    fn f<T>(self, a: u4, t: T): Self::Output;
}

with impl on a struct S and Output = S1

should get inlined to this:
fn a<Self: A, t: T>(Self self, a: u4, t: T) -> <Self as A>::Output;

and the monomorph for S, u4 should be, the monomorph should have all templates rendered.
fn a(S self, u4 a, u4 t) -> S1

Let's have HIR as it's own crate like the 1. argument.
For arg 2. what would be the other approaches.
For 3., 4., 5., 6. 100% in.


## What Could Be Improved

1. **HIR should be its own crate from day one** — even if it reuses MIR opcodes, having `crates/hir/` with its own types makes the dependency graph clean: `parser → typer → hir → mir → lir → vm`. The "just reuse MIR" shortcut will create import cycles if HIR lives inside MIR or frontend.

2. **Expression-based semantics need a clear "value slot" convention** — if `if/else` and `match` are expressions that return values, the HIR must handle "the result of this block is in slot X". Define a convention: every block has an optional `result_slot: Option<SlotId>`. The last expression's result is copied to `result_slot` if set. This applies to if/else branches, match arms, and function bodies.

3. **Native types (Array, System) need a clean plugin interface** — currently they're hardcoded Rust closures in `natives/`. The new system should have a `NativeProvider` trait that the compiler queries: "does this type have a native implementation? If so, generate HIR for method X with these slot mappings." This keeps native types out of the core compiler.

4. **Error reporting should use the new Span throughout** — currently the bridge converts chumsky spans to ariadne spans. Once the old parser is removed, all stages should use a single Span type (chumsky's `SimpleSpan` or a custom `Range<usize>` + filename). Define it once in `crates/parser/` and use it everywhere.

5. **The `extension` file ID tracking** (for future import visibility) should be designed now, even if not enforced yet. Each `Item` should carry a `FileId` so the TypeRegistry knows which file defined which extension. This is cheap to add and avoids a painful retrofit later.

6. **Match exhaustiveness checking** — enum pattern matching should check at compile time that all variants are covered. This is straightforward: collect matched discriminant values, compare with enum variant count. Missing variants → warning or error. Emit a default `Stop` arm for safety.

## Actionable Implementation Steps

### Step 1: New tokens + parser for struct/enum/extension/trait/impl
**Crate:** `crates/parser/`
**Effort:** Medium
- Add tokens: `Struct, Enum, Extension, Impl, Trait, Fn, Mut, Match, PathSep, Arrow, FatArrow`
- Add AST types: `Item, Struct, Enum, EnumVariant, Extension, Impl, Trait, Function, FunctionSig, TemplateParam`
- Add parsers: `struct_parser(), enum_parser(), extension_parser(), impl_parser(), trait_parser()`
- Keep `class_parser()` working (backward compat during transition)
- **Test:** parse new syntax `.ct` files alongside existing ones
- **Deliverable:** `program_parser()` returns `Vec<Item>` where Item includes both old `Class` and new types

### Step 2: Create `crates/typer/` — TypeRegistry
**Crate:** NEW `crates/typer/`
**Effort:** Large
- Define `TypeRegistry`, `TypeInfo`, `TraitInfo`, `EnumLayout`
- Implement: register structs (compute field offsets, sizes), register enums (compute discriminant size, data size, variant layout), register traits (store signatures), merge extensions (group by target type, check duplicates, attach file IDs), validate impls (match trait signatures, resolve associated types)
- **Test:** unit tests for size computation, extension merging, trait validation
- **Deliverable:** `TypeRegistry::from_items(Vec<Item>) -> Result<TypeRegistry, Vec<Error>>`

### Step 3: Signature flattening
**Crate:** `crates/typer/`
**Effort:** Small
- For each function in TypeRegistry, compute `FlatSig`: expand all params to u4 slots, append return slots as mut outputs, record field offset maps for struct params
- **Test:** verify slot counts match expected sizes for known types
- **Deliverable:** `TypeRegistry::flatten_sig(&self, fn: &Function) -> FlatSig`

### Step 4: Create `crates/hir/` — HIR generation
**Crate:** NEW `crates/hir/`
**Effort:** Large
- Define `HirOp` (= Mir + Call), `HirFunction`, `SlotId`
- Port `compiler.rs` compile() logic to produce HIR instead of MIR: Number → Set, Variable → Copy from named slot, Field access → Copy from offset, Method call → Call(FnRef, slots), If → If0 with result_slot convention, Loop/Break/Continue → same, Assignment → Copy to target slot, Declaration → allocate local slot, Match on enum → Match on discriminant slot with variant data binding
- Mutability checking: track slot mutability, error on immutable write
- **Test:** compile individual functions to HIR, verify slot counts and op sequences
- **Deliverable:** `compile_function(fn: &Function, sig: &FlatSig, registry: &TypeRegistry) -> HirFunction`

### Step 5: HIR optimization
**Crate:** `crates/hir/`
**Effort:** Medium
- Port `new_opt.rs` passes to work on `Vec<HirOp>`: constant propagation (track Set values, fold Into If0/Match), dead store elimination (if slot never read after write, remove write), copy propagation (a=b; use(a) → use(b))
- **Test:** verify optimization reduces op count on known functions
- **Deliverable:** `optimize(fn: &mut HirFunction)`

### Step 6: Inliner — HIR → MIR
**Crate:** `crates/hir/` or NEW `crates/inliner/`
**Effort:** Large
- Start from `main()`, walk the HIR
- For each `Call`: resolve FnRef → HirFunction (monomorphize templates), detect cycles (maintain call stack, error on revisit), inline: allocate fresh global u32 addresses for the callee's slots, map caller's arg slots → callee's param slots (Copy ops), replace Call with inlined body, map callee's return slots → caller's ret slots
- After full inlining: no Call ops remain, all slots are global u32 addresses
- Convert HirOp → Mir (trivial 1:1 mapping since they share opcodes, just drop Call)
- **Test:** inline a simple call chain, verify output MIR matches expected
- **Deliverable:** `inline_program(entry: &str, fns: &HashMap<FnRef, HirFunction>) -> MirCodeBlock`

### Step 7: Wire new pipeline into driver
**Crate:** `crates/driver/`
**Effort:** Medium
- New compile path: `lex → parse → TypeRegistry → flatten → HIR gen → HIR opt → inline → MIR opt → LIR → bytecode`
- Keep old path behind a flag for comparison
- Native types: implement Array/System/Val as `NativeProvider` returning HIR ops
- **Test:** all 20 integration tests pass with new pipeline
- **Deliverable:** `compile_v2(file: &Path, std_dir: &Path) -> MirCodeBlock`

### Step 8: Write new std library in new syntax
**Crate:** N/A (`.ct` files)
**Effort:** Medium
- Rewrite `cythan/std/Val.ct, Bool.ct, Byte.ct, Array.ct, Option.ct, DynArray.ct, System.ct` using struct/enum/extension/trait syntax
- Rewrite test programs and games
- **Test:** all integration tests pass with new std library

### Step 9: Remove old code
**Effort:** Small
- Delete `crates/frontend/src/parser/` (old hand-rolled parser)
- Delete `crates/frontend/src/bridge.rs`
- Delete `crates/frontend/src/compiler/` (old ClassLoader/ClassView/MethodView/TemplateFixer/compiler.rs)
- The `crates/frontend/` crate can be removed entirely — its role is replaced by `crates/typer/` + `crates/hir/`
- **Test:** `cargo test` still passes, `cargo clippy` clean
