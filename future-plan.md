# Future Cythan V4 Compiler Redesign

## 1. Language Syntax (New)

```rust
struct Main {
    // The first param is the type in the array.
    // The second is a number type param like const usize in Rust.
    // The third one is the index type. This can be any type supported
    // by the compiler Enumerable marker trait (u4 and u8 only for now).
    grid: Array<Val, 9, Val>
}

// ARE YOU SURE?
// This is pseudo-code showing the *compiler-internal* expansion of Array.
// It is NOT something the user writes. Having this as a language feature
// (compile-time for loops in struct definitions) would be extremely
// complex to implement and would require a const-eval system. Instead,
// the compiler should hardcode Array<T, N, Idx> as a built-in generic
// struct, just like it does today with native implementations.
// Recommendation: keep this as compiler-internal, don't expose the
// `for i in N { u4 cell_{i} }` syntax to users.
// ANSWER:
// Indeed this will stay as Rust-side only. Maybe in the future we will support macro like this but not for the first draft.
struct Array<T, size N, Idx: Enumerable> {
    for i in N {
        u4 cell_{i}
    }
}

// ARE YOU SURE?
// Same concern: the impl block uses `for i in N` and `Idx::get::<Id>()`
// which are compile-time metaprogramming features. This would require a
// full const-eval system. The current approach (native Rust functions that
// generate MIR directly in `crates/frontend/src/natives/array.rs`) is
// simpler and more maintainable. Unless you plan to make this a user-facing
// feature, keep array operations as compiler builtins.
// ANSWER:
// Same here
impl <T, size N, Idx: Enumerable> Array<T, N, Idx> {
    fn get(self, pos: Idx) -> Option<T> {
        match Idx {
            for i in N {
                Idx::get::<Id>() => self.cell_{i}()
            }
        }
    }
}

// ARE YOU SURE?
// This is a good design. `enum Option<T> { None, Some(T) }` maps cleanly
// to the Cythan memory model: 1 cell for discriminant (0 or 1) + N cells
// for T. Size = size(T) + 1. This replaces the current class-based
// `Option<T> { Bool is_none; T t; }` which wastes space when T is large
// and None is the common case. Discriminant-based enums are strictly better.
// Current Bool is 0=true, 1=false (inverted). This would need to change
// to 0=false, 1=true for enum discriminants to make sense.
// ANSWER:
// Note that enums are discriminant AND data whether they are None or Some. Just as in Rust meaning the side of the enum will always be static: Biggest variant size + Discr size
enum Option<T> {
    None,
    Some(T),
}

// ARE YOU SURE?
// This is a tagged-union enum backed by u4. Good design for small enums.
// The compiler can verify all variants fit in u4 (0-15). For enums with
// >16 variants, auto-promote to u8 (2 cells). This is sound.
// Implementation: each variant is a constant, pattern matching compiles
// to Mir::Match (already exists) or chained If0.
// Watch out: the discriminant value assignment (A=0, B=1...) means the
// enum IS its discriminant — no separate tag+data. This only works for
// "C-style" enums without payload. For data-carrying enums like Option<T>,
// you need tag+payload layout. Make sure the two enum styles are distinct.
// They are fundamentally the same just if people use a = then the discr value gets assigned to this specific value instead of auto-assigned any place where the = is missing it's auto-allocated on free values. You can then technically have a:
// enum TypeMap {
//     A(u4) = 1,
//     B = 2,
// }
// This will just make the size be u4x2 meaning you can do TypeMap as u4 anymore.
enum TypeMap {
    A = 0
    B = 1
    C = 2
    D = 3
    E = 4
    F = 5
    G = 6
    H = 7
}

extension Main {

    fn new(): Self {
        return Self {
            grid: Array::new()
        };
    }

    // ARE YOU SURE?
    // `mut self` is a good idea. Currently ALL self parameters are mutable
    // (pass-by-reference to the caller's memory). Making mutability explicit
    // catches bugs at compile time and enables optimizations (immutable
    // references can be safely copied/cached).
    // Implementation: the compiler checks that methods called on `self`
    // don't modify fields unless `mut self` is declared. In the MIR, this
    // means: for immutable self, allocate a COPY of the caller's memory
    // instead of using the original locations. Or simpler: just validate
    // at compile time that no Set/Copy targets the self locations.
    // ANSWER:
    fn set(mut self, u4 pos, u4 val) {
        self.grid.setDyn(pos, val);
    }

    // ARE YOU SURE?
    // Implicit return (last expression = return value) is a nice ergonomic
    // improvement. Implementation: check if the last expression in a method
    // body has a return type matching the declared return type. If so, emit
    // a Copy from the expression result to the return location. The current
    // compiler already handles `return expr;` — just add the implicit case.
    // ANSWER:
    // Note that the full language will be expression based so you can also do this in if-s, matches...
    fn getDyn(mut self, u4 pos): u4 {
        self.grid.getDyn(pos)
    }
}

// ARE YOU SURE?
// Traits are a major addition. In the current system, method dispatch is
// fully static (monomorphized). Traits don't change that — they add
// compile-time constraints on type parameters. This is sound because:
// 1. All types are known at compile time (no dynamic dispatch needed)
// 2. Trait bounds just validate that a type has the required methods
// 3. At monomorphization time, the concrete method is resolved
// Implementation: trait definitions store method signatures. When a type
// parameter has a trait bound, the compiler checks during ClassView
// creation that the concrete type implements all trait methods.
// This is mostly a TYPE-CHECKING feature, not a code generation feature.
// ANSWER:
// Define more how the trait resolver will work. The exact workings aren't well defined with each brick. How to decide which traits are on which method.
// Also note that the following code implies that operations get rendered as method calls and user-implementable (Eq, Add, Sub, Mul...)
trait Add {
    type Other;
    type Result;

    fn add(self, other: Self::Other): Self::Result;
}

impl Add for RandomItem {
    type Other = RandomItem;
    type Result = RandomItem;

    fn add(self, other: RandomItem): Self::Result {
        // TODO
    }
}
```

## 2. Key Language Changes vs Current Cythan

- **Rust-centric syntax.** Same model as Rust but everything is always borrowed. Mutability is now explicit (`mut self`). Composition-based type system with traits. No lifetimes since all memory is static.
- **Data is segregated from code.** `struct` for data, `extension` or `impl` for methods. Currently a single `class` keyword combines both.
- **Method signatures are flat and code-agnostic.** Just name, location, types in, types out and those types' sizes.
- **Enums exist.** Both C-style (`TypeMap`) and data-carrying (`Option<T>`).
- **Type matching.** `match` on enum variants.
- **No type inference.** All types explicit, but numbers are loosely defined until they encounter an Enumerable type.
- **No visibility modifiers.** Everything is public for now.

## 3. Full Compiler Pipeline (Proposed)

### Stage 1: Tokenization (DONE — `crates/parser/src/lexer.rs`)

```
Input:  Source text (&str)
Output: Vec<(Token, Span)>
```

Already implemented with chumsky 0.9. Handles strings, chars, numbers, idents, keywords, operators, comments.

**Changes needed for new syntax:**
- Add keywords: `struct`, `enum`, `extension`, `impl`, `trait`, `fn`, `mut`, `match`, `type`, `where`
- Add operator: `::` (path separator), `->` (return type), `=>` (match arm)
- Remove keywords: `class`, `extends`
- Keep: `if`, `else`, `loop`, `break`, `continue`, `return`, `as`, `self`, `Self`

```rust
// Example token additions:
enum Token {
    // ... existing ...
    Struct, Enum, Extension, Impl, Trait, Fn, Mut, Match,
    PathSep,    // ::
    Arrow,      // ->
    FatArrow,   // =>
    Colon,      // : (already exists)
}
```

### Stage 2: Parsing (DONE — `crates/parser/src/parser.rs`)

```
Input:  Vec<(Token, Span)>
Output: Vec<Spanned<Item>>  where Item = Struct | Enum | Extension | Impl | Trait
```

Currently parses `class` definitions. Needs to be extended for the new syntax.

**New AST types:**

```rust
// ARE YOU SURE?
// Splitting Class into Struct + Extension is clean separation of concerns.
// But it means the compiler must merge them before compilation: find all
// `extension Foo` blocks and combine their methods with `struct Foo`.
// This is a resolution step that doesn't exist currently.
// Implementation options:
//   A) Merge at parse time: single pass, collect all extensions per struct
//   B) Merge at ClassLoader time: load structs first, then extensions
//   C) Keep separate and resolve at method lookup time (lazy)
// Option B is simplest and matches the current ClassLoader pattern.

enum Item {
    Struct(Struct),
    Enum(Enum),
    Extension(Extension),
    Impl(Impl),
    Trait(Trait),
}

struct Struct {
    name: Spanned<String>,
    template: Option<Vec<TemplateParam>>,
    fields: Vec<Field>,
}

// ARE YOU SURE?
// TemplateParam needs to distinguish type params from const params.
// `Array<T, size N, Idx: Enumerable>` has:
//   T: type param (no bound)
//   N: const usize param (marked with `size`)
//   Idx: type param with trait bound `Enumerable`
// This is more complex than the current system which just has string names.
struct TemplateParam {
    name: Spanned<String>,
    kind: ParamKind,       // Type or Const
    bounds: Vec<String>,   // Trait bounds
}

enum ParamKind { Type, Const }

struct Enum {
    name: Spanned<String>,
    template: Option<Vec<TemplateParam>>,
    variants: Vec<EnumVariant>,
}

// ARE YOU SURE?
// Two kinds of enum variant:
//   Unit: `None` or `A = 0` (no data, optional explicit discriminant)
//   Tuple: `Some(T)` (carries data)
// The compiler needs to compute the memory layout differently:
//   C-style (all unit variants): just a discriminant cell
//   Data-carrying: discriminant + max(variant_sizes) cells
// This is standard tagged-union layout. Sound for static allocation.
enum EnumVariant {
    Unit { name: String, value: Option<i64> },
    Tuple { name: String, fields: Vec<Type> },
}

struct Extension {
    target: Type,             // The type being extended
    methods: Vec<Function>,
}

struct Impl {
    trait_name: Type,
    target: Type,
    assoc_types: Vec<(String, Type)>,   // type Other = Foo;
    methods: Vec<Function>,
}

struct Trait {
    name: Spanned<String>,
    template: Option<Vec<TemplateParam>>,
    assoc_types: Vec<String>,           // type Other;
    methods: Vec<FunctionSig>,          // signatures only
}

struct Function {
    sig: FunctionSig,
    body: Block,
}

struct FunctionSig {
    name: Spanned<String>,
    template: Option<Vec<TemplateParam>>,
    self_param: Option<SelfParam>,      // None = static, Some = instance
    params: Vec<(Type, Spanned<String>)>,
    return_type: Option<Type>,
}

enum SelfParam { Ref, MutRef }  // `self` or `mut self`
```

### Stage 3: Name Resolution & Type Registry

```
Input:  Vec<Item>
Output: TypeRegistry (all types, traits, impls registered)
```

**This stage is NEW.** Currently skipped — the old ClassLoader does name resolution lazily during compilation.

**What it does:**
1. Register all struct names and their field layouts
2. Register all enum names and their variant layouts
3. Register all trait definitions
4. Merge extensions into their target structs
5. Validate impl blocks match trait signatures
6. Compute type sizes

```rust
// ARE YOU SURE?
// A separate name resolution pass is a good design. It catches errors
// early (unknown types, duplicate names, missing trait impls) before
// any code compilation happens. The current system defers errors to
// compilation time, which gives confusing error messages.
// Implementation: HashMap<String, TypeInfo> where TypeInfo contains
// the struct/enum definition + all methods from extensions/impls.
// This replaces ClassLoader.classes and ClassLoader.constants.

struct TypeRegistry {
    types: HashMap<String, TypeInfo>,
    traits: HashMap<String, TraitInfo>,
    impls: Vec<ImplInfo>,  // trait impls to validate
}

struct TypeInfo {
    kind: TypeKind,  // Struct or Enum
    template: Option<Vec<TemplateParam>>,
    size: usize,     // in cells (u4 units)
    fields: Vec<(String, Type, usize)>,  // name, type, offset
    methods: Vec<Function>,
}
```

### Stage 4: Method Signature Flattening

```
Input:  TypeRegistry + Function
Output: FlatSig (just name, param sizes, return size)
```

**This stage is NEW.** Method signatures become flat: just a list of parameter sizes and a return size. No type information, just memory layout.

```rust
// ARE YOU SURE?
// Flattening signatures to sizes is correct for the Cythan memory model
// where everything is u4 cells. A Val is 1 cell, a Byte is 2 cells,
// an Array<Val, 8, Val> is 8 cells. The type information is only needed
// for type checking (stage 3), not for code generation.
// But you still need type info for FIELD ACCESS: knowing that field `x`
// is at offset 3 in a struct requires the type layout. So the flat sig
// should also include field offset maps for struct parameters.
// Implementation: during compilation, replace Type references with
// (offset, size) pairs. This is what CodeManager.location_and_type_of_field()
// already does, just made explicit.
// ANSWER:
// Let me add some context: The idea would be to have the function be a big list of u4.
// Like
// where Self: u8
// fn test(self, a: Option<u8>) -> (u4, u8) { // Some code }
// Becomes:
// fn test(self_b1, self_b2, arg_a_0, arg_a_1_0, arg_a_1_1, mut , ret_0, mut ret_1)
// Every arg is a list of u4.

struct FlatSig {
    name: String,
    params: Vec<ParamInfo>,     // each param: mutable?, size in cells
    return_size: Option<usize>, // None = void
}

struct ParamInfo {
    name: String,
    mutable: bool,
    size: usize,  // number of u4 cells
}
```

### Stage 5: HIR Generation (Function-Level)

```
Input:  Function body (AST expressions) + FlatSig
Output: HIR (function-local, no type info, just cell operations)
```

**This replaces the current compile() in compiler.rs.** The HIR is a new IR between AST and MIR.

```rust
// ARE YOU SURE?
// Adding an HIR between AST and MIR is a significant architectural change.
// Currently AST → MIR is done in one pass (compiler.rs). The proposed HIR
// would split this into AST → HIR → MIR. The benefit: HIR can be optimized
// per-function BEFORE inlining, which enables better optimization.
// The cost: another IR to maintain, another conversion pass.
// The current single-pass approach (AST → MIR) works but produces
// bloated MIR because monomorphization happens during compilation.
// A separate HIR pass would allow:
//   1. Compile each function once to HIR
//   2. Optimize HIR per-function (constant folding, dead code elimination)
//   3. Inline functions and specialize for concrete types → MIR
// This is the standard approach in production compilers (Rust MIR, LLVM).
// Recommendation: this is a good long-term design, but complex to
// implement. Consider doing it incrementally: first replace the current
// compiler.rs with a cleaner AST → MIR pass, then add HIR later.

// HIR: a function is a flat list of operations on numbered slots.
// Slot 0..N are parameters, N+1..M are locals.
// ANSWER:
// Let's have at first the HIR being basically a copy of the MIR but in functions so we don't redefine everything. This will not be a true HIR at first.
enum HirOp {
    Set(SlotId, u8),                          // slot = constant
    Copy(SlotId, SlotId),                     // slot = slot
    Inc(SlotId),                              // slot++ (mod 16)
    Dec(SlotId),                              // slot-- (mod 16)
    If0(SlotId, Vec<HirOp>, Vec<HirOp>),     // if slot==0 { } else { }
    Loop(Vec<HirOp>),                         // loop { }
    Break, Continue,
    Call(FnRef, Vec<SlotId>, Option<SlotId>), // call function with args, optional return slot
    Return,
    Stop,
}

type SlotId = u32;

struct HirFunction {
    name: String,
    params: Vec<SlotId>,        // parameter slots
    return_slot: Option<SlotId>,
    locals: u32,                // total slots used
    body: Vec<HirOp>,
}
```

### Stage 6: HIR Optimization (Per-Function)

```
Input:  HirFunction
Output: Optimized HirFunction
```

**Optimizations that can be done per-function:**
- Constant propagation
- Dead store elimination
- Copy propagation (replace `a = b; use(a)` with `use(b)`)
- Simplify `If0(const, then, else)` to the appropriate branch

```rust
// ARE YOU SURE?
// Per-function optimization is sound and beneficial. The current MIR
// optimizer (new_opt.rs) already does constant propagation and dead code
// elimination, but it operates on the ENTIRE program MIR (after inlining).
// Per-function optimization reduces the size of each function before
// inlining, which makes inlining cheaper and the post-inline optimization
// more effective. This is a clear win.
// Implementation: port the existing new_opt.rs passes to work on HirOp
// instead of Mir. The algorithms are the same.
```

### Stage 7: Inlining & Monomorphization → MIR

```
Input:  All HirFunctions + TypeRegistry + entry point (main)
Output: MirCodeBlock (single flat MIR for the whole program)
```

**This replaces the current MethodView.execute() call chain.**

Starting from `main()`, inline all function calls with concrete types:
1. For each `Call(fn, args)` in the HIR:
   - Resolve the function (by name + types)
   - Monomorphize: substitute template params with concrete types
   - Inline: replace the Call with the function's HIR body
   - Map parameter slots to argument slots
2. After full inlining, the result is a single flat instruction sequence.
3. Convert inlined HIR → MIR (trivial mapping since HirOp ≈ Mir).

```rust
// ARE YOU SURE?
// Inlining everything is the current approach and it works because:
// 1. All types are known at compile time (no virtual dispatch)
// 2. All function calls are monomorphized (no generics at runtime)
// 3. The Cythan VM has no function call mechanism (it's just copy ops)
// So full inlining is REQUIRED, not optional. The question is WHERE
// to inline: currently it happens during AST→MIR compilation (recursive
// MethodView.execute() calls). Moving it to a separate pass is cleaner.
// The risk: infinite inlining for recursive functions. The current
// system handles recursion via stack depth (STACK_SIZE = 1 GiB thread).
// A separate inlining pass would need cycle detection.
// Recommendation: start by detecting and rejecting direct recursion,
// since Cythan programs can't use recursion effectively anyway (no
// heap, no stack in the target VM).
// ANSWER:
// Cycles will be dectected on HIR inlining to MIR and rejected.
```

### Stage 8: MIR (unchanged from current)

```
Input:  Inlined MIR
Output: Optimized MIR
```

Same as current: `MirCodeBlock` containing `Vec<Mir>`.

Existing optimizations in `crates/mir/src/optimizer/new_opt.rs` apply here.

### Stage 9: MIR → LIR (unchanged)

```
Input:  Optimized MirCodeBlock
Output: Vec<CompilableInstruction>
```

Same as current: `Mir::to_asm()` in `crates/mir/src/mir.rs`.

### Stage 10: LIR → Bytecode (unchanged)

```
Input:  Vec<CompilableInstruction>
Output: Vec<usize> (bytecode) or direct MIR interpretation
```

Same as current: `CompilableInstruction::compile_to_string()` → `cythan_compiler::compile()`.

## 4. Enum Implementation Details

### C-style enums (no data)

```rust
enum TypeMap { A = 0, B = 1, C = 2 }
```

**Memory layout:** 1 cell (u4). Value IS the discriminant.

**Compilation:**
```
// TypeMap::A  →  Mir::Set(loc, 0)
// TypeMap::B  →  Mir::Set(loc, 1)

// match x { A => ..., B => ..., C => ... }
// →  Mir::Match(x_loc, [(block_A, [0]), (block_B, [1]), (block_C, [2])])
```

### Data-carrying enums

```rust
enum Option<T> { None, Some(T) }
```

**Memory layout:** 1 cell (discriminant) + max(0, size(T)) cells.
- `None` → discriminant = 0, data cells unused
- `Some(val)` → discriminant = 1, data cells hold val

```
// ARE YOU SURE?
// This layout wastes space for None (size(T) cells unused). Alternative:
// optimize Option<Val> to just 1 cell where 0 = None and 1-15 = Some(val-1).
// But this breaks if T can be 0. The safe approach (discriminant + data)
// is correct. The optimization can be added later for specific types.

// Option::None    →  Set(discriminant, 0)
// Option::Some(v) →  Set(discriminant, 1); Copy(data_cells, v_cells)

// match opt {
//     None => expr_a,
//     Some(x) => expr_b,  // x binds to data cells
// }
// →  If0(discriminant, block_none, block_some)
//    where block_some binds x_loc = data_cells
```
// ANSWER:
// We will enable packing later on enumerations that have type params that are all Enumarable and don't use all the space. But not for the first impl.

## 5. Trait Implementation Details

Traits are purely a compile-time constraint mechanism.

```rust
trait Add {
    type Other;
    type Result;
    fn add(self, other: Self::Other): Self::Result;
}
```

**Compilation:**
- No runtime representation
- At compile time: when a function has `T: Add`, check that the concrete type for T has `impl Add`
- At monomorphization: resolve `T::add(...)` to the concrete impl's method

```rust
// ARE YOU SURE?
// This works because all dispatch is static. Trait objects (dyn Trait)
// are NOT supported and don't need to be — the Cythan VM can't do
// indirect jumps. So traits are purely compile-time checked interfaces.
// This is sound and straightforward to implement: just a HashMap lookup
// during monomorphization.
// Implementation in TypeRegistry:
//   fn resolve_method(&self, type: &Type, method: &str) -> Option<&Function>
//   fn check_trait_bound(&self, type: &Type, trait: &str) -> bool
// ANSWER:
// Yep no dyn traits.
```

## 6. Mutability Checking

```rust
fn set(mut self, u4 pos, u4 val) { ... }
fn get(self, u4 pos): u4 { ... }  // self is immutable
```

**Implementation:**

```rust
// ARE YOU SURE?
// Two approaches to mutability checking:
//
// A) Compile-time validation only (recommended):
//    Track which slots are mutable. If a Set/Copy/Inc/Dec targets an
//    immutable slot, emit a compiler error. No runtime cost.
//    This is what Rust does (borrow checker is compile-time only).
//
// B) Copy-on-read for immutable params:
//    When `self` is immutable, copy all self cells to new locations
//    at method entry. The method operates on the copy. The original
//    is unchanged. This is safe but wastes memory.
//
// Option A is strictly better: zero overhead, catches bugs early.
// Implementation: add a `mutable: bool` field to each entry in
// LocalState.vars. In compile(), when generating Set/Copy/Inc/Dec,
// check if the target slot is mutable. If not, emit error.
// ANSWER:
// Everything is single threaded. The mut are propagated to the underlying raw values so unlike JS a const object can get it's field edited. Rust like on this. EVERYTHING is by reference no ownership concept. Just doing an assignement is COPY ONLY not by reference as we don't have pointers.
```

## 7. `struct` + `extension` Merging

```rust
struct Foo { x: u4 }
extension Foo { fn bar(self): u4 { self.x } }
extension Foo { fn baz(mut self, v: u4) { self.x = v; } }
```

**Resolution step (in TypeRegistry):**

```rust
// ARE YOU SURE?
// Multiple extension blocks for the same type is a good feature for
// code organization. Implementation: during TypeRegistry construction,
// collect all extension blocks, group by target type, and merge their
// methods into the type's method list. Duplicate method names → error.
// This is straightforward: HashMap<String, Vec<Function>> grouped by type.
// ANSWER:
// I think they can be kept under fileIDs so we can decide based on the imports if they are visible.

fn merge_extensions(types: &mut TypeRegistry, extensions: Vec<Extension>) {
    for ext in extensions {
        let type_name = ext.target.name;
        let info = types.get_mut(&type_name).expect("unknown type");
        for method in ext.methods {
            if info.methods.iter().any(|m| m.sig.name == method.sig.name) {
                panic!("duplicate method");
            }
            info.methods.push(method);
        }
    }
}
```

## 8. Migration Path from Current System

### Phase 1: New parser (DONE)
- chumsky lexer + parser producing clean AST ✓
- Bridge converting new AST → old compiler types ✓
- All 20 integration tests pass ✓

### Phase 2: New type system
- Add `struct`, `enum`, `extension` to parser
- Build TypeRegistry replacing ClassLoader
- Keep bridge for backward compatibility

### Phase 3: HIR
- New IR between AST and MIR
- Per-function compilation (no inlining yet)
- Per-function optimization

### Phase 4: Inliner
- Replace recursive MethodView.execute() with explicit inlining pass
- HIR → inlined MIR

### Phase 5: Traits + mutability
- Compile-time trait checking
- Mutability validation

### Phase 6: Remove old code
- Delete old parser (`crates/frontend/src/parser/`)
- Delete bridge (`crates/frontend/src/bridge.rs`)
- Delete ClassLoader/ClassView/MethodView/TemplateFixer
