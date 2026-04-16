# Cythan V4 Compiler Redesign — Implementation Plan

## Language Model

Everything is by reference, no ownership. Assignment is always COPY (no pointers). Mutability is explicit (`mut self`, `mut param`). Single-threaded. All memory is static. All dispatch is static (monomorphized). Expression-based (last expression = return value in blocks, ifs, matches). No type inference.

**Type renames:** `Val` is renamed to `U4` and `Byte` is renamed to `U8`. These are the standard names for the 4-bit and 8-bit types. All std library files, examples, and compiler references use the new names.

**Bool semantics:** 1 is true, 0 is false (standard convention). The old VM convention (0 = true) is inverted at the compiler boundary — the language uses normal truthiness. `if expr { ... }` branches when expr is nonzero (true = 1). Bool is a struct wrapping a U4; `true` and `false` are built-in constants (`true` = `Bool { value: 1 }`, `false` = `Bool { value: 0 }`).

**Mutability:** Everything is const by default. Variables need `mut` prefix to be mutable (e.g. `mut U4 x = 5`). Parameters need `mut` prefix to be mutable. `mut` propagates to fields: `mut self` means `self.field` is mutable too. Writing to an immutable slot is a compile-time error.

**Operators:** The language supports comparison and arithmetic operators that desugar to trait method calls:
- `==` `!=` → `Eq::eq()` / negated `Eq::eq()`
- `>` `<` `>=` `<=` → `Ord::gt()`, `Ord::lt()`, etc.
- `+` `-` → `Add::add()`, `Sub::sub()` — return new values, do NOT modify operands
- `+=` `-=` → `AddAssign::add_assign()`, `SubAssign::sub_assign()` — modify left operand in place (left operand must be `mut`)
- `&&` `||` → short-circuit boolean operators (built-in, not trait-based)

Key distinction: `Sub::sub(self, other): Self` returns a new value without modifying self. `SubAssign::sub_assign(mut self, other)` modifies self in place. Use `-=` when you want in-place modification, `-` when you want a new value.

**Semicolons:** Semicolons terminate expression-statements (`x += 1;`, `return true;`). Block expressions (`if`, `loop`, `match`) do NOT require a trailing `;` when used as statements. Semicolons after closing `}` are optional and not idiomatic.

**Constants:** `const Type name = expr;` declares a compile-time constant. `true` and `false` are built-in constants of type Bool — they are NOT factory methods. Bool is a struct wrapping a U4; the compiler knows `true` = `Bool { value: 1 }` and `false` = `Bool { value: 0 }`.

**Increment/Decrement:** There are no `.inc()` / `.dec()` methods. Use `+= 1` and `-= 1` instead. For U4, these are native operations (compiler emits Inc/Dec ops). For composite types like U8, they call the AddAssign/SubAssign trait implementation.

**Visibility:** No `pub`/`private` keywords — everything is public by default. Scoping comes from the import system: you must import files, traits, and structs to use them. Extensions carry file IDs for future import-based scoping.

## Syntax Reference

```rust
struct Main {
    Array<U4, 9, U4> grid,       // Array<ElementType, const_size, IndexType>
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

    fn set(mut self, u4 pos, u4 val) {
        self.grid.setDyn(pos, val);     // mut self required to modify fields
    }

    fn getDyn(self, u4 pos): u4 {
        self.grid.getDyn(pos)           // implicit return
    }
}

trait Add {
    type Other;
    type Result;
    fn add(self, Self::Other other): Self::Result;
}

impl Add for MyType {
    type Other = MyType;
    type Result = MyType;
    fn add(self, MyType other): MyType { /* ... */ }
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
  │                           Item = Struct | Enum | Extension | Impl | Trait | Const
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
// Example: trait Add { type Output; fn add(self, Self other): Self::Output; }
//          impl Add for u8 { type Output = u8; fn add(self, u8 other): u8 { ... } }
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
// fn test(self, Option<u8> a) -> (u4, u8)
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
// trait Add { type Output; fn add(self, Self other): Self::Output; }
// impl Add for u8 { type Output = u8; fn add(self, u8 other): u8 { ... } }
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

### Phase 1: New Parser (from scratch)

The new parser is a standalone crate `crates/new_parser/` built from scratch — no modifications to the existing `crates/parser/`. The existing parser remains untouched and continues to work for the old pipeline. The acceptance criteria is: **all example `.ct` files in the new syntax parse successfully and produce the expected AST**.

The steps are ordered: write examples first, review them manually, create the crate structure, build a test harness with expected outputs, then implement the parser piece by piece.

---

#### Step 1.1 — Write example standard library in new syntax
**Output:** `examples/new_syntax/std/` directory with `.ct` files
**What:** Translate the existing standard library (`cythan/std/`) into the new struct/enum/extension/trait/impl syntax. These files define the language's surface syntax and serve as the parser's acceptance test inputs.

Files to create:

**`U4.ct`** — primitive 4-bit value type:
```rust
// U4 is a native type (no fields). Its size is 1 cell (4 bits).
// += 1 and -= 1 are native operations (compiler emits Inc/Dec ops).
struct U4 {}

extension U4 {
    fn zero(): Self {
        0
    }

    fn input(): Self {
        System::setRegister<0>(2);
        System::getRegister<2>()
    }

    fn equals(self, Self other): Bool {
        mut Self copy = self;
        mut Self copy1 = other;
        loop {
            if copy == 0 {
                return copy1 == 0;
            }
            copy -= 1;
            copy1 -= 1;
        }
    }

    fn greater(self, Self other): Bool {
        mut Self copy = self;
        mut Self copy1 = other;
        loop {
            if copy == 0 {
                return false;
            } else if copy1 == 0 {
                return true;
            }
            copy -= 1;
            copy1 -= 1;
        }
    }

    fn sub(mut self, mut Self other) {
        loop {
            if other == 0 {
                break;
            }
            self -= 1;
            other -= 1;
        }
    }

    fn printDec(self) {
        if self > 9 {
            '1'.print();
            mut Self k = self;
            k -= 10;
            k.print();
        } else {
            self.print();
        }
    }

    fn print(self) {
        System::setRegister<1>(3);
        System::setRegister<2>(self);
        System::setRegister<0>(1);
    }
}
```

**`Bool.ct`** — boolean type (1 = true, 0 = false):
```rust
struct Bool {
    U4 value,
}

extension Bool {
    fn not(self): Self {
        if self {
            false
        } else {
            true
        }
    }

    fn print(self) {
        if self {
            "true".print();
        } else {
            "false".print();
        }
    }
}
```

**`U8.ct`** — 8-bit type (two U4 cells):
```rust
struct U8 {
    U4 lower,
    U4 higher,
}

extension U8 {
    fn new(Self a): Self {
        Self { lower: a.lower, higher: a.higher }
    }

    fn zero(): Self {
        Self { lower: 0, higher: 0 }
    }

    fn inc(mut self) {
        self.lower += 1;
        if self.lower == 0 {
            self.higher += 1;
        }
    }

    fn dec(mut self) {
        if self.lower == 0 {
            self.higher -= 1;
        }
        self.lower -= 1;
    }

    fn add(mut self, Self other) {
        mut Self u = other;
        loop {
            if u == 0 {
                break;
            }
            self.inc();
            u.dec();
        }
    }

    fn sub(mut self, Self other) {
        mut Self u = other;
        loop {
            if u == 0 {
                break;
            }
            self.dec();
            u.dec();
        }
    }

    fn fromU4(U4 a): Self {
        Self { lower: a, higher: 0 }
    }

    fn fromU4AsNumber(U4 a): Self {
        Self { lower: a, higher: 3 }
    }

    fn input(): Self {
        System::setRegister<0>(2);
        Self {
            lower: System::getRegister<2>(),
            higher: System::getRegister<1>(),
        }
    }

    fn print(self) {
        System::setRegister<2>(self.lower);
        System::setRegister<1>(self.higher);
        System::setRegister<0>(1);
    }

    fn equals(self, Self other): Bool {
        self.lower == other.lower && self.higher == other.higher
    }

    fn equalsZero(self): Bool {
        self.lower == 0 && self.higher == 0
    }

    fn printDec(self) {
        mut U4 lower = self.lower;
        mut U4 higher = self.higher;
        if lower > 9 {
            lower -= 10;
            lower.printDec();
            higher += 1;
        } else {
            lower.printDec();
        }
        if higher > 9 {
            higher -= 10;
            higher.printDec();
            '1'.print();
        } else {
            higher.printDec();
        }
    }

    fn debug(self) {
        System::debugInterupt(self.lower);
        System::debugInterupt(self.higher);
    }
}
```

**`System.ct`** — system I/O (all methods are native):
```rust
struct System {}

extension System {
    fn setRegister<N>(U4 value) {}
    fn getRegister<N>(): U4 {}
    fn debug<T>(T a) {}
    fn debugType<T>() {}

    fn debugInterupt(U4 a) {
        Self::setRegister<1>(a);
        Self::setRegister<0>(3);
    }
}
```

**`Array.ct`** — fixed-size array (native setDyn/getDyn/len):
```rust
struct Array<T, E, F> {}

extension Array<T, E, F> {
    fn set<N>(mut self, T value) {}
    fn setDyn(mut self, F index, T value) {}
    fn get<N>(self): T {}
    fn getDyn(self, F index): T {}
    fn len(self): F {}

    fn print(Self<U8, E, F> self) {
        mut F index = F::zero();
        loop {
            if self.len() > index {
                self.getDyn(index).print();
                index += 1;
            } else {
                break;
            }
        }
    }

    fn println(Self<U8, E, F> self) {
        mut F index = F::zero();
        loop {
            if self.len() == index {
                break;
            } else {
                self.getDyn(index).print();
                index += 1;
            }
        }
        '\n'.print();
    }

    fn contains(self, T t): Bool {
        mut F size = self.len();
        loop {
            if size == 0 {
                return false;
            }
            size -= 1;
            if self.getDyn(size) == t {
                return true;
            }
        }
    }
}
```

**`Option.ct`** — generic option type (as enum):
```rust
enum Option<T> {
    None,
    Some(T),
}

extension Option<T> {
    fn none(): Self {
        Self::None
    }

    fn some(T t): Self {
        Self::Some(t)
    }

    fn is_none(self): Bool {
        match self {
            Self::None => true,
            Self::Some(_) => false,
        }
    }
}
```

**`DynArray.ct`** — dynamic-length array backed by a fixed-size array:
```rust
struct DynArray<T, N, F> {
    Array<T, N, F> array,
    F length,
}

extension DynArray<T, N, F> {
    fn new(): Self {
        Self { array: Array::new(), length: F::zero() }
    }

    fn from<Number>(Array<T, Number, F> input): Self {
        mut Self dyn = Self::new();
        dyn.addAll<Number>(input);
        dyn
    }

    fn add(mut self, T t) {
        self.array.setDyn(self.length, t);
        self.length += 1;
    }

    fn addAll<Ng>(mut self, Array<T, Ng, F> arr) {
        mut F l = arr.len();
        mut F c = F::zero();
        loop {
            if l == 0 {
                break;
            }
            self.add(arr.getDyn(c));
            c += 1;
            l -= 1;
        }
    }

    fn pop(mut self): T {
        self.length -= 1;
        self.getDyn(self.length)
    }

    fn len(self): F {
        self.length
    }

    fn capacity(self): F {
        self.array.len()
    }

    fn getDyn(self, F pos): T {
        self.array.getDyn(pos)
    }

    fn setDyn(mut self, F pos, T t) {
        self.array.setDyn(pos, t);
    }

    fn get<Number>(self): T {
        self.array.get<Number>()
    }

    fn set<Number>(mut self, T t) {
        self.array.set<Number>(t);
    }

    fn last(self): T {
        mut F k = self.len();
        k -= 1;
        self.getDyn(k)
    }

    fn println(Self<U8, N, F> self) {
        mut F index = F::zero();
        loop {
            if self.len() == index {
                break;
            } else {
                self.getDyn(index).print();
                index += 1;
            }
        }
        '\n'.print();
    }

    fn contains(self, T t): Bool {
        mut F size = self.len();
        loop {
            if size == 0 {
                return false;
            }
            size -= 1;
            if self.getDyn(size) == t {
                return true;
            }
        }
    }
}
```

#### Step 1.2 — Write example Morpion game in new syntax
**Output:** `examples/new_syntax/Morpion.ct`
**What:** Translate the Morpion (tic-tac-toe) game to the new syntax. This is the primary end-to-end example: it exercises enums (Cell state), traits (Eq), structs, extensions, match expressions, method calls, control flow, expressions-as-values, and templates.

```rust
enum Cell {
    Empty,
    O,
    X,
}

extension Cell {
    fn print(self) {
        match self {
            Self::Empty => '-'.print(),
            Self::O => 'O'.print(),
            Self::X => 'X'.print(),
        }
    }

    fn isEmpty(self): Bool {
        match self {
            Self::Empty => true,
            _ => false,
        }
    }
}

trait Eq {
    fn eq(self, Self other): Bool;
}

impl Eq for Cell {
    fn eq(self, Cell other): Bool {
        self as U4 == other as U4
    }
}

struct Morpion {
    Array<Cell, 9, U4> grid,
}

extension Morpion {
    fn new(): Self {
        Self { grid: Array::new() }
    }

    fn set(mut self, U4 pos, Cell val) {
        self.grid.setDyn(pos, val);
    }

    fn getDyn(self, U4 pos): Cell {
        self.grid.getDyn(pos)
    }

    fn get<TE>(self): Cell {
        self.grid.get<TE>()
    }

    fn display(self) {
        mut U4 count = 0;
        loop {
            if count == 9 {
                '\n'.print();
                break;
            }
            if count == 3 || count == 6 {
                '\n'.print();
            }
            self.getDyn(count).print();
            count += 1;
        }
    }

    fn play(mut self) {
        mut Cell currentPlayer = Cell::O;
        mut U4 count = 9;
        self.display();
        loop {
            mut U4 pos = U4::input();
            pos -= 1;
            if pos > 8 {
                continue;
            }
            if self.getDyn(pos).isEmpty() {
                self.set(pos, currentPlayer);
                count -= 1;
                self.display();
                if self.winner(currentPlayer) {
                    currentPlayer.print();
                    " won!".println();
                    break;
                }
                if count == 0 {
                    "Equality!".println();
                    break;
                }
                currentPlayer = match currentPlayer {
                    Cell::O => Cell::X,
                    _ => Cell::O,
                };
            } else {
                "Invalid input!".println();
            }
        }
    }

    fn winner(self, Cell tocheck): Bool {
        if self.get<0>() == tocheck {
            if self.get<1>() == tocheck && self.get<2>() == tocheck {
                return true;
            }
            if self.get<3>() == tocheck && self.get<6>() == tocheck {
                return true;
            }
            if self.get<4>() == tocheck && self.get<8>() == tocheck {
                return true;
            }
        }
        if self.get<1>() == tocheck && self.get<4>() == tocheck && self.get<7>() == tocheck {
            return true;
        }
        if self.get<2>() == tocheck && self.get<4>() == tocheck && self.get<6>() == tocheck {
            return true;
        }
        if self.get<3>() == tocheck && self.get<4>() == tocheck && self.get<5>() == tocheck {
            return true;
        }
        if self.get<6>() == tocheck && self.get<7>() == tocheck && self.get<8>() == tocheck {
            return true;
        }
        false
    }

    fn main(): U4 {
        mut Self morpion = Self::new();
        morpion.play();
        0
    }
}
```

#### Step 1.3 — Manual review of example files
**What:** Review all example files for syntax consistency before building the parser.
Checklist:
- [ ] All struct fields use `Type name` syntax (type before name)
- [ ] All struct construction uses `Self { field: value }` (colon, not `=`)
- [ ] All `fn` declarations use `fn name(params): ReturnType { body }` format
- [ ] Mutating methods use `mut self` or `mut` on parameters
- [ ] Static methods (no `self`) use `Self::method()` or `TypeName::method()` call syntax
- [ ] Template parameters use `<T>` on declarations, `<ConcreteType>` on calls
- [ ] Extensions on generic types use `extension TypeName<T, U>` syntax
- [ ] Variable declarations use `mut Type name = expr;` (no `let` keyword)
- [ ] Enum variants use `Variant`, `Variant(Type)`, `Variant = N` syntax
- [ ] Enum construction uses `Self::Variant` or `Self::Variant(value)`
- [ ] Match arms use `Self::Variant => expr` with `_` for wildcard
- [ ] Native methods have empty bodies `{}`
- [ ] Expression-based returns: last expression in block (no semicolon) = return value
- [ ] Explicit `return expr;` allowed for early returns
- [ ] `self as Type` cast syntax preserved
- [ ] String literals (`"..."`) and char literals (`'...'`) have `.print()` and `.println()` methods
- [ ] `&&` and `||` operators work as before
- [ ] Semicolons terminate statements; missing semicolon on last expression = return value
- [ ] No `.inc()` / `.dec()` calls — use `+= 1` / `-= 1` instead
- [ ] `true` and `false` are used directly, not as `Bool::true()` / `Bool::false()`
- [ ] Semicolons after block expressions (`if { }`, `loop { }`, `match { }`) are not required
- [ ] Constants use `const Type name = expr;`

**Deliverable:** Finalized example files, checked into the repo under `examples/new_syntax/`.

#### Step 1.4 — Create `crates/new_parser/` crate structure
**What:** Set up the new parser crate with module stubs and dependencies.
```
crates/new_parser/
├── Cargo.toml          # deps: chumsky 0.9, ariadne
├── src/
│   ├── lib.rs          # pub mod lexer, token, ast, parser; pub fn parse(src) -> Result<Vec<Item>>
│   ├── token.rs        # Token enum
│   ├── lexer.rs        # fn lexer() -> impl Parser<char, Vec<(Token, Span)>>
│   ├── ast.rs          # AST node types (Item, StructDef, EnumDef, etc.)
│   ├── parser.rs       # fn parser() -> impl Parser<Token, Vec<Item>>
│   └── tests/          # test module
│       ├── mod.rs
│       ├── lexer_tests.rs
│       ├── parser_tests.rs
│       └── integration_tests.rs
```
- Add `crates/new_parser` to workspace `Cargo.toml`
- All source files start as stubs (empty types, `todo!()` parsers)
- **Deliverable:** `cargo check -p new_parser` compiles (with `todo!()` bodies)

#### Step 1.5 — Test harness: expected AST for each example file
**What:** Write tests that parse each example file and assert the resulting AST structure.

Tests in `crates/new_parser/src/tests/integration_tests.rs`:
- `test_parse_val()` — parses `examples/new_syntax/std/U4.ct`, asserts: 1 struct item (U4, no fields), 1 extension item on U4 with N methods (equalsZero, zero, input, equals, greater, sub, printDec, print)
- `test_parse_bool()` — parses `Bool.ct`, asserts: 1 struct (Bool, 1 field `U4 value`), 1 extension with 2 methods (not, print)
- `test_parse_const()` — parses `const U4 MAX = 15;` → Item::Const with type U4, name "MAX", value 15
- `test_parse_expr_true_false()` — `true` → Expr::Bool(true), `false` → Expr::Bool(false)
- `test_parse_byte()` — parses `U8.ct`, asserts: 1 struct (U8, 2 fields), 1 extension with N methods
- `test_parse_system()` — parses `System.ct`, asserts: 1 struct, 1 extension, methods have template params
- `test_parse_array()` — parses `Array.ct`, asserts: 1 struct with 3 template params, 1 extension with template params on the extension itself
- `test_parse_option()` — parses `Option.ct`, asserts: 1 enum with 2 variants (None unit, Some with data), 1 extension with match expression in a method body
- `test_parse_dynarray()` — parses `DynArray.ct`, asserts: 1 struct with 2 fields (one generic), 1 extension
- `test_parse_morpion()` — parses `Morpion.ct`, asserts: 1 struct, 1 extension, 7 methods including `main() -> U4`

Tests in `crates/new_parser/src/tests/lexer_tests.rs`:
- `test_lex_keywords()` — tokenize `struct enum extension impl trait fn mut match` → correct token variants
- `test_lex_symbols()` — tokenize `:: => { } ( ) < > , : ; .` → correct tokens
- `test_lex_string_char()` — tokenize `"hello" '\n'` → String and Char tokens
- `test_lex_full_function()` — tokenize a complete `fn` declaration, verify token sequence

Tests in `crates/new_parser/src/tests/parser_tests.rs` (unit tests for individual parsers):
- `test_parse_type_simple()` — `U4` → Type { name: "U4", templates: [] }
- `test_parse_type_generic()` — `Array<U4, 9, U4>` → Type with 3 template args
- `test_parse_struct_empty()` — `struct U4 {}` → StructDef with no fields
- `test_parse_struct_fields()` — `struct U8 { U4 lower, U4 higher }` → 2 fields
- `test_parse_enum_unit()` — `enum Color { Red, Green, Blue }` → 3 unit variants
- `test_parse_enum_mixed()` — `enum Option<T> { None, Some(T) }` → unit + data variant
- `test_parse_enum_explicit_discr()` — `enum X { A = 0, B(u4) = 2, C }` → explicit + auto discriminants
- `test_parse_fn_no_params()` — `fn zero(): Self { 0 }` → no params, return type Self, body = number literal
- `test_parse_fn_self()` — `fn not(self): Self {}` → self param, return type Self, empty body
- `test_parse_fn_params()` — `fn set(mut self, U4 pos, U4 val) { ... }` → 3 params with mut
- `test_parse_fn_template()` — `fn get<N>(self): T {}` → template param N
- `test_parse_extension_simple()` — `extension U4 { fn zero(): Self { 0 } }` → 1 method
- `test_parse_extension_generic()` — `extension Array<T, E, F> { ... }` → extension with type params
- `test_parse_trait()` — `trait Add { type Other; fn add(self, Self::Other other): Self::Result; }` → associated types + method sigs
- `test_parse_impl()` — `impl Add for MyType { type Other = MyType; fn add(...) { ... } }` → concrete types + body
- `test_parse_expr_number()` — `42` → Expr::Number(42)
- `test_parse_expr_variable()` — `count` → Expr::Variable("count")
- `test_parse_expr_field()` — `self.grid` → Expr::Field(self, "grid")
- `test_parse_expr_method_call()` — `self.grid.getDyn(pos)` → chained method call
- `test_parse_expr_static_call()` — `Self::new()` → Expr::StaticCall
- `test_parse_expr_template_call()` — `self.get<0>()` → method call with template arg
- `test_parse_expr_if_else()` — `if cond { a } else { b }` → Expr::If
- `test_parse_expr_if_else_chain()` — `if a { } else if b { } else { }` → nested If
- `test_parse_expr_if_as_expr()` — `U4 x = if cond { 1 } else { 2 };` → if used as expression
- `test_parse_expr_loop()` — `loop { break; }` → Expr::Loop
- `test_parse_expr_return()` — `return false;` → Expr::Return
- `test_parse_expr_let()` — `U4 x = 5;` → Expr::Declaration (mutable = false)
- `test_parse_expr_let_mut()` — `mut U4 x = 5;` → Expr::Declaration (mutable = true)
- `test_parse_expr_assign()` — `x = 5;` → Expr::Assign
- `test_parse_expr_compound_assign()` — `x -= 10;` → Expr::CompoundAssign(SubAssign, ...)
- `test_parse_expr_plus_assign()` — `x += 1;` → Expr::CompoundAssign(AddAssign, ...)
- `test_parse_expr_comparison()` — `a == b` → Expr::BinaryOp(EqEq, ...), `a > 9` → BinaryOp(Gt, ...)
- `test_parse_expr_arithmetic()` — `a + b` → Expr::BinaryOp(Add, ...), `a - b` → BinaryOp(Sub, ...)
- `test_parse_expr_bool_ops()` — `a && b || c` → Expr::BinaryOp chain
- `test_parse_expr_struct_literal()` — `Self { lower: a, higher: b }` → Expr::StructLiteral
- `test_parse_expr_enum_variant()` — `Self::None` → Expr::EnumVariant
- `test_parse_expr_enum_variant_data()` — `Self::Some(x)` → Expr::EnumVariant with data
- `test_parse_expr_match()` — `match self { Self::None => true, Self::Some(_) => false }` → Expr::Match
- `test_parse_expr_cast()` — `self as Bool` → Expr::Cast
- `test_parse_expr_string_method()` — `"hello".print()` → method call on string literal
- `test_parse_expr_char_method()` — `'\n'.print()` → method call on char literal
- `test_parse_expr_chained_method_on_if()` — `if a { '-' } else { 'X' }.print()` → method call on if-expression result

**Deliverable:** All tests written (they will fail/panic since parsers are stubs). `cargo test -p new_parser` compiles but tests fail.

#### Step 1.6 — Token enum
**File:** `crates/new_parser/src/token.rs`
**What:** Define the complete Token enum.

```rust
enum Token {
    // Literals
    Number(i64),
    Ident(String),          // lowercase identifiers: variable names, field names
    TypeName(String),       // uppercase identifiers: type names (U4, Bool, Self)
    String(String),         // "hello"
    Char(char),             // '\n'

    // Keywords
    Struct,
    Enum,
    Extension,
    Impl,
    Trait,
    Fn,
    Mut,
    If,
    Else,
    Loop,
    Break,
    Continue,
    Return,
    Match,
    True,
    False,
    Const,                  // const
    For,                    // reserved
    As,
    SelfValue,              // `self` (the value)

    // Symbols
    Dot,                    // .
    Comma,                  // ,
    Colon,                  // :
    Semicolon,              // ;
    Assign,                 // =
    EqEq,                   // ==
    NotEq,                  // !=
    PathSep,                // ::
    FatArrow,               // =>
    Underscore,             // _
    And,                    // &&
    Or,                     // ||
    Plus,                   // +
    Minus,                  // -
    PlusAssign,             // +=
    MinusAssign,            // -=
    Bang,                   // !
    LParen, RParen,         // ( )
    LBrace, RBrace,         // { }
    LAngle, RAngle,         // < >
    LBracket, RBracket,     // [ ]
}
```

Note: `Self` is parsed as `TypeName("Self")`, not a separate keyword. This keeps type parsing uniform.

#### Step 1.7 — Lexer
**File:** `crates/new_parser/src/lexer.rs`
**What:** Implement the chumsky lexer: `fn lexer() -> impl Parser<char, Vec<(Token, Span)>, Error = Simple<char>>`

Lexing rules (in priority order):
1. Whitespace and `//` line comments → skip
2. Multi-char operators (must come before single-char):
   - `::` → PathSep
   - `=>` → FatArrow
   - `==` → EqEq
   - `!=` → NotEq
   - `+=` → PlusAssign
   - `-=` → MinusAssign
   - `&&` → And
   - `||` → Or
3. Single-char symbols: `.` `,` `:` `;` `=` `+` `-` `!` `(` `)` `{` `}` `<` `>` `[` `]` `_`
4. String literals: `"..."` with `\n`, `\t`, `\\`, `\"` escapes
5. Char literals: `'...'` with same escapes
6. Integer literals: `[0-9]+` → Number
7. Identifiers/keywords: `[a-zA-Z_][a-zA-Z0-9_]*`
    - Keyword check: `struct`, `enum`, `extension`, `impl`, `trait`, `fn`, `mut`, `if`, `else`, `loop`, `break`, `continue`, `return`, `match`, `true`, `false`, `for`, `as`, `self`, `const`
    - If starts with uppercase and not a keyword → TypeName
    - Otherwise → Ident

**Test:** all `test_lex_*` tests pass.

#### Step 1.8 — AST types
**File:** `crates/new_parser/src/ast.rs`
**What:** Define all AST node types.

```rust
// Spans are attached to every node for error reporting.
type Span = Range<usize>;
type Spanned<T> = (T, Span);

// Top-level items
enum Item {
    Struct(StructDef),
    Enum(EnumDef),
    Extension(ExtensionDef),
    Trait(TraitDef),
    Impl(ImplDef),
    Const(ConstDef),
}

struct ConstDef {
    ty: Spanned<Type>,
    name: Spanned<String>,
    value: Spanned<Expr>,
}

struct StructDef {
    name: Spanned<String>,
    templates: Vec<Spanned<String>>,
    fields: Vec<FieldDef>,
}

struct FieldDef {
    name: Spanned<String>,
    ty: Spanned<Type>,
}

struct EnumDef {
    name: Spanned<String>,
    templates: Vec<Spanned<String>>,
    variants: Vec<EnumVariant>,
}

struct EnumVariant {
    name: Spanned<String>,
    data: Option<Spanned<Type>>,       // None = unit variant
    discriminant: Option<Spanned<i64>>, // None = auto-assigned
}

struct ExtensionDef {
    target: Spanned<Type>,             // e.g. U4 or Array<T, E, F>
    methods: Vec<Spanned<Function>>,
}

struct TraitDef {
    name: Spanned<String>,
    templates: Vec<Spanned<String>>,
    associated_types: Vec<Spanned<String>>,
    methods: Vec<Spanned<FunctionSig>>, // signatures only, no body
}

struct ImplDef {
    trait_name: Spanned<Type>,         // e.g. Add or Add<T>
    target: Spanned<Type>,             // e.g. MyType
    associated_types: Vec<(Spanned<String>, Spanned<Type>)>, // name = ConcreteType
    methods: Vec<Spanned<Function>>,
}

struct FunctionSig {
    name: Spanned<String>,
    templates: Vec<Spanned<String>>,
    params: Vec<Param>,
    return_type: Option<Spanned<Type>>,
}

struct Function {
    sig: FunctionSig,
    body: Spanned<Block>,
}

struct Param {
    name: Spanned<String>,
    ty: Option<Spanned<Type>>,         // None for `self` (type = Self)
    mutable: bool,
}

struct Type {
    name: String,                       // "U4", "Array", "Self", "Self::Output"
    templates: Vec<Spanned<TypeOrValue>>,
}

// Template arguments can be types or integer constants
enum TypeOrValue {
    Type(Type),
    Value(i64),                         // e.g. 9 in Array<U4, 9, U4>
}

// Expression block
struct Block {
    stmts: Vec<Spanned<Expr>>,
}

enum Expr {
    // Literals
    Number(i64),
    String(String),
    Char(char),
    Bool(bool),
    Variable(String),

    // Access
    Field(Box<Spanned<Expr>>, Spanned<String>),                        // expr.field
    MethodCall(Box<Spanned<Expr>>, Spanned<String>, Vec<Spanned<Type>>, Vec<Spanned<Expr>>),
                                                                       // expr.method<T>(args)
    StaticCall(Spanned<Type>, Spanned<String>, Vec<Spanned<Type>>, Vec<Spanned<Expr>>),
                                                                       // Type::method<T>(args)

    // Construction
    StructLiteral(Spanned<Type>, Vec<(Spanned<String>, Spanned<Expr>)>), // Type { f: v, ... }
    EnumVariant(Spanned<Type>, Spanned<String>, Option<Box<Spanned<Expr>>>),
                                                                       // Type::Variant or Type::Variant(expr)

    // Operators
    BinaryOp(BinOp, Box<Spanned<Expr>>, Box<Spanned<Expr>>),

    // Control flow
    If(Box<Spanned<Expr>>, Box<Spanned<Block>>, Option<Box<Spanned<Expr>>>),
                                                                       // if cond { block } else { expr_or_if }
    Loop(Box<Spanned<Block>>),
    Break,
    Continue,
    Return(Option<Box<Spanned<Expr>>>),
    Match(Box<Spanned<Expr>>, Vec<MatchArm>),

    // Binding
    Declaration(bool, Spanned<String>, Spanned<Type>, Box<Spanned<Expr>>), // [mut] Type name = expr
                                                                       // bool = mutable
    Assign(Box<Spanned<Expr>>, Box<Spanned<Expr>>),                    // lvalue = expr
    CompoundAssign(CompoundOp, Box<Spanned<Expr>>, Box<Spanned<Expr>>), // lvalue += expr, lvalue -= expr

    // Cast
    Cast(Box<Spanned<Expr>>, Spanned<Type>),                           // expr as Type
}

enum BinOp { And, Or, EqEq, NotEq, Gt, Lt, GtEq, LtEq, Add, Sub }
enum CompoundOp { AddAssign, SubAssign }

struct MatchArm {
    pattern: Spanned<Pattern>,
    body: Spanned<Expr>,
}

enum Pattern {
    Variant(Spanned<Type>, Spanned<String>, Option<Spanned<String>>),  // Type::Variant(binding)
    Wildcard,                                                          // _
}
```

#### Step 1.9 — Parser: types and struct/enum
**File:** `crates/new_parser/src/parser.rs`
**What:** Implement parsers for types, struct definitions, and enum definitions.

Parsers to implement:
- `type_parser()` — parses `U4`, `Array<T, 9, U4>`, `Self`, `Self::Output`
  - TypeName, optionally followed by `<` type_or_value_list `>`
  - `Self::Ident` for associated type references
- `struct_parser()` — parses `struct Name<T> { Type field, ... }`
  - `struct` keyword, TypeName, optional `<` template list `>`, `{` fields `}`
  - Fields: `Type name` separated by `,` (trailing comma optional)
  - Empty struct: `struct U4 {}`
- `enum_parser()` — parses `enum Name<T> { Variant, Variant(Type) = N, ... }`
  - Variants separated by `,` (trailing comma optional)
  - Each variant: name, optional `(Type)`, optional `= Number`

**Test:** `test_parse_type_*`, `test_parse_struct_*`, `test_parse_enum_*` tests pass.

#### Step 1.10 — Parser: expressions (core)
**File:** `crates/new_parser/src/parser.rs`
**What:** Implement expression parser for literals, variables, field access, method calls, and operators.

Parsers to implement:
- `expr_parser()` — recursive descent, handles:
  - **Atoms:** Number, String, Char, `true`, `false`, `self`, Variable
  - **Struct literal:** `TypeName { field: expr, ... }`
  - **Enum variant:** `TypeName::VariantName` or `TypeName::VariantName(expr)`
  - **Static call:** `TypeName::method<T>(args)` — distinguished from enum variant by presence of `(`
  - **Parenthesized:** `(expr)`
- `postfix_parser()` — left-to-right chaining after an atom:
  - `.field` → Field access
  - `.method<T>(args)` → MethodCall
  - `as Type` → Cast
- `binop_parser()` — binary operators with precedence (highest to lowest):
  1. `+` `-` (arithmetic)
  2. `==` `!=` `>` `<` `>=` `<=` (comparison)
  3. `&&` (logical and)
  4. `||` (logical or)
  - All left-to-right associative
- `block_parser()` — `{ stmt; stmt; expr }` where last expr without `;` is the block's value

**Test:** `test_parse_expr_number`, `test_parse_expr_variable`, `test_parse_expr_field`, `test_parse_expr_method_call`, `test_parse_expr_static_call`, `test_parse_expr_template_call`, `test_parse_expr_bool_ops`, `test_parse_expr_comparison`, `test_parse_expr_arithmetic`, `test_parse_expr_struct_literal`, `test_parse_expr_enum_variant*`, `test_parse_expr_cast`, `test_parse_expr_string_method`, `test_parse_expr_char_method` pass.

#### Step 1.11 — Parser: expressions (control flow)
**File:** `crates/new_parser/src/parser.rs`
**What:** Add control flow expressions to the expression parser.

Parsers to implement:
- `if_parser()` — `if expr { block } else if expr { block } else { block }`
  - `else` branch is optional; `else if` chains
  - If-expression as value: `U4 x = if a { 1 } else { 2 };`
  - Chained method call on if-expression: `if a { '-' } else { 'X' }.print()`
- `loop_parser()` — `loop { block }`
- `match_parser()` — `match expr { Pattern => expr, ... }`
  - Patterns: `Type::Variant`, `Type::Variant(binding)`, `_`
  - Arms separated by `,` (trailing comma optional)
- `return_parser()` — `return expr;` or `return;`
- `break` and `continue` as expression keywords
- `let_parser()` — `Type name = expr;` or `mut Type name = expr;`
- Assignment: `lvalue = expr;` where lvalue is Variable or Field chain
- Compound assignment: `lvalue += expr;`, `lvalue -= expr;` → CompoundAssign node

**Test:** `test_parse_expr_if_else`, `test_parse_expr_if_else_chain`, `test_parse_expr_if_as_expr`, `test_parse_expr_loop`, `test_parse_expr_return`, `test_parse_expr_let`, `test_parse_expr_let_mut`, `test_parse_expr_assign`, `test_parse_expr_compound_assign`, `test_parse_expr_match`, `test_parse_expr_chained_method_on_if` pass.

#### Step 1.12 — Parser: functions, extensions, traits, impls
**File:** `crates/new_parser/src/parser.rs`
**What:** Implement parsers for function declarations and top-level item blocks.

Parsers to implement:
- `param_parser()` — `mut? self` or `mut? Type name`
  - `self` has no explicit type annotation (type is `Self`)
  - `Self<U8, E, F> self` for specialized self types (as seen in Array.print)
- `function_sig_parser()` — `fn name<T>(params): ReturnType`
  - Template params optional, return type optional
- `function_parser()` — `fn_sig { block }`
- `extension_parser()` — `extension Type<T> { fn ... fn ... }`
- `trait_parser()` — `trait Name<T> { type Assoc; fn sig(); ... }`
  - Methods are signatures only (no body): `fn name(params): RetType;`
  - Associated types: `type Name;`
- `impl_parser()` — `impl Trait for Type { type Name = Type; fn ... }`
- `const_parser()` — `const Type name = expr;`
- `program_parser()` — `Vec<Item>` = sequence of struct/enum/extension/trait/impl/const at top level

**Test:** `test_parse_fn_*`, `test_parse_extension_*`, `test_parse_trait`, `test_parse_impl` pass.

#### Step 1.13 — Integration tests: parse all example files
**File:** `crates/new_parser/src/tests/integration_tests.rs`
**What:** Run all integration tests that parse the example `.ct` files.
- All `test_parse_val`, `test_parse_bool`, ..., `test_parse_morpion` tests pass
- Each test reads the `.ct` file, calls `parse()`, asserts success, and checks the AST structure (item counts, method counts, field names, template params)
- **Acceptance criteria:** every example file parses without errors and produces the expected AST

**Deliverable:** `cargo test -p new_parser` — all tests green.

#### Step 1.14 — Documentation
**File:** `crates/new_parser/README.md`
**What:** Document the new parser crate.
- Language syntax reference with EBNF-like grammar
- Token list
- AST structure overview
- How to use: `new_parser::parse(source: &str) -> Result<Vec<Item>, Vec<Error>>`
- Differences from old syntax (class → struct + extension, old-style declarations → let, etc.)

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
- **Test:** `struct Pair { u4 a, u8 b }` → size 3, offsets a=0, b=1

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
- **Test:** `fn test(self, Option<u8> a): u4` where Self is u8 → 8 slots (2+3 input, 1+0+0 → wait, let me just verify the counts match)

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
- **Test:** compile `u4 x = 5; x` → Set + Copy sequence

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
- **Test:** inline `fn inc(mut self) { self.inc() }` on u4 → produces flat MIR with global slots

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
