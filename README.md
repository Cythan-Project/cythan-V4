# Cythan V4

An OOP -like Cythan programming language

# Introduction

This language has a synthax between Java and Rust and is the officially recomemnded language for Cythan projects.
The main goal of this language is to hide the complexity of the Cythan machine and make it easy to use providing high level concepts.

# Features

- Optimizing compiler
- Object oriented programming (Classes)
- Monomorphisation (Templates that create a version of every function for every type)
- Variable shadowing
- Predictable memory footprint and operation count
- All the STD written in Cythan V4

# Begginer guide

## Getting started

Download `cargo`.
Run an example for instance `cargo run run Pendu`.

## Cythan basics

Cythan compiler will compile and run the main function of the given file
All files should be in the STD folder
Run with `cargo run run <YOUR NAMEFILE>`
Build with `cargo run build <YOUR NAMEFILE>`

**Note: In cythan files should only contain one class named the same as the file (Just like in java)**

### Hello world function

```java
class MyFileName {

    Val main() {
        "Hello world!".println();
        return 0;
    }
}
```

### Basic types in cythan

#### Primitive types

- `Val`: [0; 15]
- `Byte`: [0; 255]
- `Bool`: [0; 1] (Note that 1 is false and 0 is true)

#### Data structures

 - `Array<T, N>`: Avec T le type et N la taille du tableau (Les tableaux ne peuvent pas changer de taille en cours d'execution)
 - `DynArray<T,N>`: Avec T le type du tableau et N le nombre maximum d'éléments dans le tableau (A noter que la mémoire donnée sera utilisée même si le tableau n'est pas rempli)

#### Type aliases
 - `Self` references to the current instance type
 - `self` references to the current instance and so has `Self` type

#### Type convertions
To convert from one type to the other use the `as` keyword.
Note that if the types don't have the same size you won't be able to cast them.
Note that those type convertions aren't checked and can break things.

Example:
```rust
0 as Bool
```
### Conditions, variables and expressions

In cythan everything is an expression this means **if** statement should end with a **;**.
All variables in cythan can be shadowed and modified.
Note that self variables are references to the object itself and if you use = will edit the instance.

```java
class MyFileName {

    Val main() {
        if 1.equals(2) {
            "Stupid".println();
        } else {
            "Logic".println();
        };
        Val v = if 0.isZero() {
            1;
        } else {
            2;
        };
        v.print();
        Byte v = 20;
        v.print();
        return 0;
    }
}
```

### Classes
Classes in cythan are similar to java classes.
All methods are static by default but they can be called with the `.` operator. In this case the instance will be place in the first argument.
Classes can have templates which can either be types or sizes.
```java
class Option<T> {
    Bool is_none;
    T t;

    Self none<T>() {
        return Self {
            is_none = true,
            t = T t
        };
    }

    print(self) {
        if self.is_none {
            "None".println();
        } else {
            t.print();
        };
    }

    Self some<T>(T t) {
        return Self {
            is_none = false,
            t = t
        };
    }
}
```