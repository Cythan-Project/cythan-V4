We have a first Tokenization layer.
Then a parser to build an AST.

Types / method definitions get rendered but not their body so function body compilation can be para.

The synthax of the language is the following:
```
struct Main {

    // The first param is the type in the array.
    // The second is a number type param like const usize in Rust.
    // The third one is the index type. This can be any type supported by the compiler Enumerable market trait (u4 and u8 only for now ).
    grid: Array<Val, 9, Val>
}

// In the compiler note that this is pseudo code and has to be impl Rust side.
struct Array<T, size N, Idx: Enumerable> {
    for i in N {
        u4 cell_{i}
    }
}

impl <T, size N, Idx: Enumerable> Array<T, N, Idx> {
    fn get(self, pos: Idx) -> Option<T> {
        match Idx {
            for i in N {
                Idx::get::<Id>() => self.cell_{i}()
            }
        }
    }
}

// We get back to real Cythan code.
// The size of this type will logically be [size T + 1] as the Some/None will be 1 or 0
enum Option<T> {
    None,
    Some(T),
}

// This is another type of enum possible
// It is a mapping over a u4 type note that bigger types can be defined and will automatically switch to u8, u12, u16...
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

    // If no type is set then the method is return type ()
    fn set(mut self, u4 pos, u4 val) {
        self.grid.setDyn(pos, val);
    }

    fn getDyn(mut self, u4 pos): u4 {
        // straight up: self.grid.getDyn(pos) should work without the return just like rust.
        return self.grid.getDyn(pos);
    }
}

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

Compared to old cythan:
 - Very rust centric approach. Basically the same model as Rust but everything is always borrowed. Now mutability is explicit. Still the asignment copy pattern as we can have pointers. The type system is composition based with traits. There is no lifetimes since all memory is static anyway. 
 - Data is segregated from code.
 - Method signatures are flat and code agnostic. Just name location, types in, types out and those types size.
 - A new HIR function based with no type info. A function is a big array of params that are mutable or not. The return type is just mut references after the arguments as arguments.
 - The HIR is optimized per function and can then be inlined and that gives birth to the actual MIR. 
 - Enum now exist and the language supports type matching. 
 - There is still no type inference. They are all explicit but number are loosely defined until they encounter a type Enumerable and take this type. 
 - There is no visibility concept for now.
 - The method get compiled as.