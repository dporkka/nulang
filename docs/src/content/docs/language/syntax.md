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

## Pipe Operator

The `|>` operator pipes a value left-to-right into a function:

```nulang
let inc = fn(x) { x + 1 } in
let dbl = fn(x) { x * 2 } in
1 |> inc |> dbl   // 4
```

`x |> f` is equivalent to `f(x)`. Chaining `a |> f |> g |> h` applies `f`, then `g`, then `h` in order.

## Send Operators

There are two syntaxes for sending a message to an actor:

```nulang
// Keyword form
send counter inc()

// Operator form (equivalent)
counter ! inc()
```

Both parse to the same AST. The `!` form is more concise; the `send` form is more readable for complex arguments:

```nulang
w ! watch(v)
send counter inc_by(5)
send counter get(self)
```

## Ask Operator

`ask` is a synchronous request/reply call to an agent or actor behavior. The caller blocks until the target responds:

```nulang
let a = spawn Assistant {} in
ask a ask("What is an actor model?")
```

See [AI Agents](/ai/overview/) for agent declarations and tool binding.

## Ternary Expressions

`if` can be used inline with the `then` keyword:

```nulang
let fib = fn(n) {
    if n <= 1 then n else fib(n - 1) + fib(n - 2)
} in fib(10)
```

`if cond then a else b` returns `a` when `cond` is truthy, `b` otherwise. The block form (`if cond { a } else { b }`) is equivalent.

## Effect Annotations

Function signatures declare their effects with `!` or `throws` followed by an effect row:

```nulang
// Pure function — no effects
fn add(x: Int, y: Int) -> Int = x + y

// Effectful — performs IO
fn greet(): ! IO Unit {
    perform IO.print("Hello")
}

// throws is an alias for !
fn log(msg: String): throws IO Unit {
    perform IO.print(msg)
}
```

See [Algebraic Effects](/language/effects/) for the full effect system.

## Comments

```nulang
// Single-line comment

//// Regular comment (not a doc comment)

/// Doc comment — attaches to the next declaration

//! Module-level doc comment
```
