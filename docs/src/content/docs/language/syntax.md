---
title: Syntax Basics
description: Core Nulang syntax — functions, bindings, control flow, records, variants, and expressions.
---

## Functions

Functions are defined with `fn` and use Hindley-Milner type inference:

```nulang
fn add(x: Int, y: Int) -> Int {
    x + y
}

// Type annotations are optional when inferable
fn double(x) {
    x * 2
}

// Single-expression functions
fn square(x: Int) -> Int = x * x
```

## Let Bindings

`let` introduces immutable bindings. The type is inferred unless annotated:

```nulang
let x = 42
let y: Int = x + 8
let pair = (x, y)  // Tuple: (Int, Int)
```

## Control Flow

### If Expressions

`if` is an expression — it returns a value:

```nulang
let status = if x > 0 { "positive" } else { "non-positive" }
```

### Match Expressions

Pattern matching on variants, records, and primitives:

```nulang
type Option[T] = Some(T) | None

fn unwrap_or(o: Option[Int], default: Int) -> Int {
    match o {
        Some(v) => v,
        None => default
    }
}
```

Match arms can have guards:

```nulang
match value {
    n if n > 0 => "positive",
    n if n < 0 => "negative",
    _ => "zero"
}
```

## Records

Records are row-polymorphic — a function can accept any record with the fields it needs:

```nulang
fn full_name(r: { first: String, last: String }) -> String {
    r.first + " " + r.last
}

let person = { first: "Alice", last: "Smith", age: 30 }
full_name(person)  // "Alice Smith" — extra fields are fine
```

Records are structurally typed and created with `{ field: value, ... }` syntax.

## Variants

Algebraic data types defined with `type ... = ... | ...`:

```nulang
type Result[T, E] = Ok(T) | Err(E)

type Tree[T] = Leaf(T) | Node(Tree[T], Tree[T])
```

Construct and match:

```nulang
let ok = Ok(42)

match ok {
    Ok(v) => "Got " + Int.to_string(v),
    Err(e) => "Error: " + e
}
```

## Arrays

```nulang
let nums = [1, 2, 3, 4, 5]
let first = nums[0]   // Index access
let len = length(nums) // Built-in length
```

## Comments

```nulang
// Single-line comment

//// Regular comment (not a doc comment)

/// Doc comment — attaches to the next declaration

//! Module-level doc comment
```
