# Cythan V4 Compiler Redesign — Implementation Plan

## Language Model

Everything is by reference, no ownership. Assignment is always COPY (no pointers). Mutability is explicit (`mut self`, `mut param`). Single-threaded. All memory is static. All dispatch is static (monomorphized). Expression-based (last expression = return value in blocks, ifs, matches). No type inference.

**Visibility:** No `pub`/`private` keywords — everything is public by default. Scoping comes from the import system: you must import files, traits, and structs to use them. Extensions carry file IDs for future import-based scoping.

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
  │                           - store file IDs on extensions for scoping
  │
  ├─ Stage 4: Function Registry ── TypeRegistry → FunctionDB
  │                           in crates/typer/
  │                           - global HashMap<FnSig, Fn> for all functions
  │                           - Fn = Templated(polymorph + monomorph cache)
  │                                | Simple(already concrete)
  │                           - trait methods inlined: Self + associated types
  │                             become template params on the function
  │                           - flatten sigs: params → flat u4 slot lists,
  │                             return → mut output slots appended to params
  │
  ├─ Stage 5: HIR Gen ─────── AST body + FlatSig → HirFunction
  │                           NEW crate: crates/hir/
  │                           - reuses Mir opcodes but scoped per-function
  │                           - has Call(fn_ref, slots) for unresolved calls
  │                           - locals are numbered slots, not global u32 addresses
  │                           - result_slot convention for expression-based blocks
  │
  ├─ Stage 6: HIR Opt ─────── HirFunction → optimized HirFunction
  │                           in crates/hir/
  │                           - port new_opt.rs passes to work on HirOp
  │                           - constant prop, dead store elim, copy prop
  │
  ├─ Stage 7: Inline ─────── HirFunctions + entry point → MirCodeBlock
  │                           in crates/hir/
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

### Global Function Registry

```rust
// All functions (from extensions, impls, standalone) are registered here.
// Trait methods are inlined: `Self` and associated types become template params.
//
// Example: trait Add { type Output; fn add(self, other: Self): Self::Output; }
//          impl Add for u8 { type Output = u8; fn add(self, other: u8): u8 { ... } }
//
// Gets registered as:
//   FnSig { type_name: "u8", method_name: "add", templates: [] }
//   → Simple(fn(u8 self, u8 other) -> u8 { ... })
//
// For a generic impl, the trait's Self and associated types remain as templates:
//   fn add<Self: Add, T>(Self self, T other) -> <Self as Add>::Output
//   → Templated { templates: [Self, T], polymorph: ..., monomorphs: {} }
//   The monomorph for (u8, u4) would be:
//   fn add(u8 self, u4 other) -> u8

struct FunctionDB {
    functions: HashMap<FnSig, Fn>,
}

struct FnSig {
    type_name: String,       // owning type (or "" for free functions)
    method_name: String,
}

enum Fn {
    Templated(TemplatedFn),
    Simple(SimpleFn),
}

struct TemplatedFn {
    templates: Vec<TemplateParam>,
    monomorphs: HashMap<Vec<ConcreteType>, SimpleFn>,  // cache
    polymorph: UnrenderedFn,                           // template body
}

struct SimpleFn {
    sig: FlatSig,
    body: HirFunction,
}
```

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

### HIR

```rust
enum HirOp {
    // Same as Mir:
    Set(SlotId, u8),
    Copy(SlotId, SlotId),
    Inc(SlotId),
    Dec(SlotId),
    If0(SlotId, HirBlock, HirBlock),
    Loop(HirBlock),
    Break,
    Continue,
    Stop,
    ReadRegister(SlotId, u8),
    WriteRegister(u8, Either<u8, SlotId>),
    Block(HirBlock),
    Skip,
    Match(SlotId, Vec<(HirBlock, Vec<u8>)>),

    // NEW: unresolved function call
    Call {
        target: FnRef,
        args: Vec<SlotId>,
        ret: Option<Vec<SlotId>>,
    },
}

// Expression-based blocks: each block has an optional result_slot.
// The last expression's result is copied into result_slot if set.
// This applies to if/else branches, match arms, and function bodies.
struct HirBlock {
    ops: Vec<HirOp>,
    result_slot: Option<SlotId>,
}

struct FnRef {
    type_name: String,
    method_name: String,
    template_args: Vec<ConcreteType>,
}
```

### Trait Resolution

```
// Traits are compile-time only. No runtime representation.
// Trait methods are "inlined" into concrete functions at registration time.
//
// trait Add { type Output; fn add(self, other: Self): Self::Output; }
// impl Add for u8 { type Output = u8; fn add(self, other: u8): u8 { ... } }
//
// The impl produces a concrete function registered in FunctionDB:
//   FnSig { type_name: "u8", method_name: "add" }
//   → Simple(fn(u8, u8) -> u8)
//
// Operators desugar to trait calls:
//   a + b → Add::add(a, b)  → resolved via FunctionDB lookup
//   a == b → Eq::eq(a, b)
//
// Builtin traits: Eq, Ord, Add, Sub, Enumerable
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
// Checked during HIR generation (stage 5).
// No runtime cost.
```

---

## Roadmap

### Phase 1: Parser Extensions

#### Step 1.1 — New tokens
**Crate:** `crates/parser/src/token.rs`, `crates/parser/src/lexer.rs`
**What:** Add token variants for the new syntax.
- Add to `Token` enum: `Struct`, `Enum`, `Extension`, `Impl`, `Trait`, `Fn`, `Mut`, `SelfType` (for `Self`), `SelfValue` (for `self`), `PathSep` (`::`)
- Update lexer to recognize these keywords
- **Test:** tokenize snippets containing the new keywords, verify token sequence

#### Step 1.2 — AST types for new items
**Crate:** `crates/parser/src/ast.rs`
**What:** Define the new top-level AST node types.
- `Item` enum: `Struct(StructDef)`, `Enum(EnumDef)`, `Extension(ExtensionDef)`, `Impl(ImplDef)`, `Trait(TraitDef)`
- `StructDef`: name, templates, fields
- `EnumDef`: name, templates, variants (each variant: name, optional data type, optional `= N`)
- `ExtensionDef`: target type name, file_id, methods
- `ImplDef`: trait name, target type, associated types, methods
- `TraitDef`: name, templates, associated types, method signatures
- `Function`: name, templates, params (with `mut` flag), return type, body
- Keep existing `Class`, `Method`, `Expr` types alongside — they remain in use during transition

#### Step 1.3 — Struct & enum parsers
**Crate:** `crates/parser/src/parser.rs`
**What:** Parse `struct` and `enum` declarations.
- `struct_parser()`: parses `struct Name<T> { field: Type, ... }`
- `enum_parser()`: parses `enum Name<T> { Variant, Variant(Type) = N, ... }`
- **Test:** parse struct with fields and templates, enum with mixed unit/data/explicit-discriminant variants

#### Step 1.4 — Extension & function parsers
**Crate:** `crates/parser/src/parser.rs`
**What:** Parse `extension` blocks and `fn` declarations.
- `function_parser()`: parses `fn name<T>(mut self, arg: Type): RetType { body }`
  - Handle `mut` on params, `self` as first param, `Self` type references
- `extension_parser()`: parses `extension TypeName { fn ... fn ... }`
- Reuse existing `expr_parser()` for function bodies
- **Test:** parse extension with multiple methods, methods with mut self

#### Step 1.5 — Trait & impl parsers
**Crate:** `crates/parser/src/parser.rs`
**What:** Parse `trait` and `impl` blocks.
- `trait_parser()`: parses `trait Name<T> { type Assoc; fn sig(...): RetType; }`
  - Method bodies are optional (signatures only in traits)
- `impl_parser()`: parses `impl TraitName for TypeName { type Assoc = ConcreteType; fn ... }`
- **Test:** parse trait with associated types and method sigs, impl block with concrete types and bodies

#### Step 1.6 — Top-level `program_parser()`
**Crate:** `crates/parser/src/parser.rs`
**What:** Combine all item parsers into a single top-level parser.
- `program_parser()` returns `Vec<Item>` — a file is a sequence of items
- Keep existing `class_parser()` working alongside for backward compatibility during transition
- **Test:** parse a complete `.ct` file with mixed structs, enums, extensions, traits, impls

---

### Phase 2: Type Registry (`crates/typer/`)

#### Step 2.1 — Create crate, define core types
**Crate:** NEW `crates/typer/`
**What:** Set up the crate and define the data structures.
- `TypeRegistry` struct: holds all registered types, traits, impls
- `TypeInfo`: name, templates, kind (struct fields / enum variants), size in cells
- `TraitInfo`: name, templates, associated type names, method signatures
- `EnumLayout`: discriminant size, data size, variant map (name → discriminant value + data type)
- `FieldLayout`: name, offset, size
- **Test:** construct TypeInfo/EnumLayout by hand, verify size computations

#### Step 2.2 — Struct registration & size computation
**Crate:** `crates/typer/`
**What:** Register struct definitions and compute their memory layout.
- Walk `Item::Struct` items, compute field offsets and total size
- Handle template structs: store unresolved, compute size on monomorphization
- Primitive types: `u4` = 1 cell, `u8` = 2 cells, `Bool` = 1 cell
- **Test:** `struct Pair { a: u4, b: u8 }` → size 3, offsets a=0, b=1

#### Step 2.3 — Enum registration & layout computation
**Crate:** `crates/typer/`
**What:** Register enum definitions and compute discriminant + data layout.
- Compute discriminant size from variant count (≤16 → u4, ≤256 → u8)
- Compute data size as max of variant data sizes
- Assign discriminant values: explicit `= N` or auto-assign from free values
- **Test:** `Option<u8>` → discr=1, data=2, total=3; all-unit enum → total=1

#### Step 2.4 — Extension merging
**Crate:** `crates/typer/`
**What:** Merge extension blocks into their target types.
- Group extensions by target type name
- Attach methods to the type's method list
- Store file ID on each extension for future import scoping
- Detect duplicate method names → error
- **Test:** two extensions on same type with non-overlapping methods → merged; overlapping → error

#### Step 2.5 — Trait registration & impl validation
**Crate:** `crates/typer/`
**What:** Register traits and validate impl blocks.
- Store trait definitions with their method signatures and associated types
- For each impl block: verify all trait methods are implemented, verify associated types are provided, verify method signatures match (after substituting associated types)
- **Test:** valid impl passes; missing method → error; wrong signature → error

#### Step 2.6 — `TypeRegistry::from_items()`
**Crate:** `crates/typer/`
**What:** Orchestrate the full type registration pipeline.
- `from_items(items: Vec<Item>) -> Result<TypeRegistry, Vec<Error>>`
- Calls steps 2.2–2.5 in order, collects all errors
- **Test:** end-to-end: parse a multi-item file → build TypeRegistry → verify contents

---

### Phase 3: Function Registry & Signature Flattening

#### Step 3.1 — FunctionDB structure
**Crate:** `crates/typer/`
**What:** Define the global function registry.
- `FunctionDB` with `HashMap<FnSig, Fn>`
- `Fn::Templated` holds template params, monomorph cache, and unrendered body
- `Fn::Simple` holds a concrete `FlatSig` + AST body (HIR body comes later)
- **Test:** construct FunctionDB, insert and retrieve functions

#### Step 3.2 — Populate FunctionDB from TypeRegistry
**Crate:** `crates/typer/`
**What:** Walk all types and their methods, register into FunctionDB.
- Extension methods → register under `FnSig { type_name, method_name }`
- Impl methods → inline trait's `Self` and associated types as resolved concrete types, register as Simple or Templated depending on remaining unresolved templates
- Free functions (if any) → register under `FnSig { type_name: "", method_name }`
- **Test:** trait impl methods are found by type+method lookup; templated functions store polymorph

#### Step 3.3 — Signature flattening
**Crate:** `crates/typer/`
**What:** For each function, compute the flat slot layout.
- Expand each parameter into u4 cells based on its type's size
- Mark `mut` params as mutable slots
- Append return type slots as mutable output slots
- Record field offset maps for struct params (for field access → slot offset lookup)
- `FlatSig`: list of `SlotInfo { name, offset, size, mutable }`, input count, output count
- **Test:** `fn test(self: u8, a: Option<u8>) -> u4` → 8 slots (2+3 input, 1+0+0 → wait, let me just verify the counts match)

---

### Phase 4: HIR (`crates/hir/`)

#### Step 4.1 — Create crate, define HirOp and HirBlock
**Crate:** NEW `crates/hir/`
**What:** Define the HIR data structures.
- `HirOp` enum: same variants as `Mir` + `Call { target: FnRef, args, ret }`
- `HirBlock`: `ops: Vec<HirOp>`, `result_slot: Option<SlotId>` (for expression-based semantics)
- `HirFunction`: `sig: FlatSig`, `body: HirBlock`, `slot_count: u32`
- `SlotId`: newtype wrapper around `u32` (local to function, not global)
- `FnRef`: `type_name`, `method_name`, `template_args`
- **Test:** construct HirOp values, verify Clone/Debug

#### Step 4.2 — HIR generation: literals, variables, field access
**Crate:** `crates/hir/`
**What:** Compile simple expressions to HIR.
- Port logic from `crates/frontend/src/compiler/compiler.rs`
- Number literal → `Set(slot, value)`
- Variable reference → `Copy(slot, var_slot)`
- Field access → `Copy(slot, base_slot + field_offset)` using FlatSig offset maps
- Variable declaration → allocate new local slot
- Assignment → `Copy(target_slot, source_slot)` with mutability check
- **Test:** compile `let x: u4 = 5; x` → Set + Copy sequence

#### Step 4.3 — HIR generation: control flow
**Crate:** `crates/hir/`
**What:** Compile if/else, loops, match to HIR.
- `if cond { a } else { b }` → `If0(cond_slot, then_block, else_block)` where both branches write to `result_slot`
- `loop { ... break }` → `Loop(block)` with `Break`/`Continue`
- `match expr { ... }` → `Match(discr_slot, arms)` with each arm writing to `result_slot`
- `return expr` → write to function's output slots
- Expression-based: last expression in block → copy result to `block.result_slot`
- **Test:** compile if/else → verify both branches target same result_slot

#### Step 4.4 — HIR generation: method calls
**Crate:** `crates/hir/`
**What:** Compile method calls to HIR Call ops.
- `obj.method(args)` → `Call { target: FnRef, args: [obj_slots..., arg_slots...], ret: [ret_slots...] }`
- Resolve method on type → look up in FunctionDB to get FnRef
- Allocate local slots for return values
- Handle chained calls: `a.f().g()` → Call for f, then Call for g using f's return slots
- **Test:** compile `x.inc()` → Call with correct slot mappings

#### Step 4.5 — HIR generation: struct construction & enum construction
**Crate:** `crates/hir/`
**What:** Compile `Self { field: val }` and enum variant construction.
- Struct construction → series of Copy ops to field slots
- Enum variant construction → Set discriminant + Copy data to data slots
- **Test:** construct `Option::Some(x)` → Set(discr, 1) + Copy(data, x)

#### Step 4.6 — Mutability checking
**Crate:** `crates/hir/`
**What:** Enforce mutability rules during HIR generation.
- Track mutability flag per slot (from FlatSig + local declarations)
- Any write (Set/Copy/Inc/Dec) to an immutable slot → compile error
- `mut` propagates: if `self` is immut, `self.field` is also immut
- **Test:** writing to immutable param → error; writing to mut param → ok

---

### Phase 5: HIR Optimization

#### Step 5.1 — Constant propagation
**Crate:** `crates/hir/`
**What:** Track known constant values and fold them.
- If `Set(slot, N)` and slot is only read (not re-written), replace uses with the constant
- Fold `If0(slot, ...)` when slot is a known constant → eliminate dead branch
- Fold `Match(slot, ...)` when slot is known → select matching arm
- Port logic from `crates/mir/src/new_opt.rs`
- **Test:** `Set(a, 0); If0(a, then, else)` → `then` block only

#### Step 5.2 — Dead store elimination & copy propagation
**Crate:** `crates/hir/`
**What:** Remove unnecessary writes and copies.
- Dead store: if a slot is written but never read before next write → remove first write
- Copy propagation: `Copy(a, b); use(a)` → `use(b)` when safe
- Port read/write analysis from `crates/mir/src/get_reads.rs` / `get_writes.rs`
- **Test:** redundant Set followed by overwrite → first Set removed

---

### Phase 6: Inlining (HIR → MIR)

#### Step 6.1 — Entry point resolution & call graph walk
**Crate:** `crates/hir/`
**What:** Start from `main()` and discover all reachable functions.
- Find `main()` in FunctionDB
- Walk the HIR, collect all `Call` targets
- Build call graph, detect cycles → error (recursion not supported)
- **Test:** simple call chain A→B→C detected; A→B→A → cycle error

#### Step 6.2 — Monomorphization
**Crate:** `crates/hir/`
**What:** Instantiate templated functions with concrete types.
- When a `Call` targets a `Templated` function, substitute template params with concrete types
- Check monomorph cache first; if miss, generate new `SimpleFn` and cache it
- Recursively monomorphize any calls inside the newly generated body
- **Test:** `Array<u4, 3, u4>::get()` generates concrete function with correct slot sizes

#### Step 6.3 — Inlining & global slot allocation
**Crate:** `crates/hir/`
**What:** Replace Call ops with inlined function bodies, assign global addresses.
- Maintain a global slot counter (u32), starting from 0
- For each function to inline: allocate fresh global slots for all its local slots
- Replace `Call` with: Copy args to callee param slots → inlined body → Copy callee return slots to caller ret slots
- After full inlining: no `Call` ops remain
- **Test:** inline `fn inc(mut self: u4) { self.inc() }` → produces flat MIR with global slots

#### Step 6.4 — HirOp → Mir conversion
**Crate:** `crates/hir/`
**What:** Convert the fully inlined HirOp tree to MIR.
- 1:1 mapping: `HirOp::Set(s,v)` → `Mir::Set(s,v)`, etc.
- `HirBlock` → `MirCodeBlock` (drop `result_slot`, it's already been handled)
- No `Call` ops should remain — assert this
- Output: `MirCodeBlock` ready for existing MIR optimizer
- **Test:** round-trip a simple program through full pipeline, compare MIR output

---

### Phase 7: Native Types

#### Step 7.1 — NativeProvider trait
**Crate:** `crates/hir/`
**What:** Define a clean interface for native type implementations.
- `trait NativeProvider { fn has_method(&self, type_name: &str, method: &str) -> bool; fn generate_hir(&self, type_name: &str, method: &str, slots: &[SlotId]) -> Vec<HirOp>; }`
- The compiler queries this during HIR generation: if a method call targets a native type, call `generate_hir()` instead of emitting a `Call`
- **Test:** mock NativeProvider returns HirOp for a known method

#### Step 7.2 — Val native
**Crate:** `crates/hir/` (or `crates/natives/`)
**What:** Implement Val's native methods as HIR.
- `val.inc()` → `Inc(slot)`
- `val.dec()` → `Dec(slot)`
- Port from `crates/frontend/src/natives/val.rs`
- **Test:** `Val.inc()` compiles to `Inc` op

#### Step 7.3 — System native
**Crate:** `crates/hir/`
**What:** Implement System's native methods as HIR.
- `System.getRegister<N>()` → `ReadRegister(slot, N)`
- `System.setRegister<N>(val)` → `WriteRegister(N, slot)`
- Port from `crates/frontend/src/natives/system.rs`
- **Test:** register read/write produces correct HIR ops

#### Step 7.4 — Array native
**Crate:** `crates/hir/`
**What:** Implement Array's native methods as HIR.
- `Array<T, Size, IndexType>` — statically allocated, size known at compile time
- `setDyn(index, value)` → match on index, copy value to correct offset
- `getDyn(index)` → match on index, copy from correct offset
- Port from `crates/frontend/src/natives/array.rs`
- **Test:** `Array<u4, 3, u4>::setDyn(1, x)` → Match with 3 arms

---

### Phase 8: Driver Integration

#### Step 8.1 — New compile path in driver
**Crate:** `crates/driver/`
**What:** Wire the new pipeline: lex → parse → TypeRegistry → FunctionDB → HIR → inline → MIR.
- Add `compile_v2()` function alongside existing `compile()`
- Load `.ct` files → parse as `Vec<Item>` → build TypeRegistry → build FunctionDB → generate HIR for all functions → optimize HIR → inline from main → MIR opt → LIR → bytecode
- Keep old `compile()` path for comparison
- **Test:** `compile_v2()` produces MIR for a simple program

#### Step 8.2 — Integration test: basic programs
**Crate:** `crates/driver/`
**What:** Get simple test programs working through the new pipeline.
- Start with TestVal, TestBool — smallest test programs
- Debug and fix issues until these pass
- **Test:** `cargo test test_val` passes with new pipeline

#### Step 8.3 — Integration test: all 20 tests pass
**Crate:** `crates/driver/`
**What:** Get all existing integration tests working.
- Work through failing tests one by one
- Fix edge cases in HIR gen, inlining, natives
- **Test:** `cargo test` — all 20 tests green

---

### Phase 9: New Standard Library & Cleanup

#### Step 9.1 — Rewrite std library in new syntax
**What:** Rewrite `.ct` files using struct/enum/extension/trait/impl syntax.
- Val.ct, Bool.ct, Byte.ct, Array.ct, Option.ct, DynArray.ct, System.ct
- Rewrite test programs (TestVal, TestBool, etc.)
- Rewrite game programs (Morpion, Pendu, Game2048, Chess)
- **Test:** all integration tests pass with new std library

#### Step 9.2 — Match exhaustiveness checking
**Crate:** `crates/hir/`
**What:** Warn/error on non-exhaustive enum matches.
- Collect matched discriminant values, compare with enum variant count
- Missing variants → error (or warning + default Stop arm)
- **Test:** match on `Option` with only `Some` arm → error

#### Step 9.3 — Remove old code
**What:** Delete the old frontend pipeline.
- Delete `crates/frontend/src/parser/` (old hand-rolled parser)
- Delete `crates/frontend/src/bridge.rs`
- Delete `crates/frontend/src/compiler/`
- Delete `crates/frontend/src/natives/`
- Remove `crates/frontend/` crate entirely — replaced by `crates/typer/` + `crates/hir/`
- Remove old `Class`/`Method` AST types from `crates/parser/` if no longer used
- **Test:** `cargo test` passes, `cargo clippy` clean

#### Step 9.4 — Error reporting cleanup
**Crate:** all
**What:** Unify span types across the pipeline.
- Define a single `Span` type (chumsky's `SimpleSpan` or custom `Range<usize>` + filename) in `crates/parser/`
- Use it in typer, hir, and error reporting
- Remove old span conversion code
- **Test:** error messages show correct file/line/column
