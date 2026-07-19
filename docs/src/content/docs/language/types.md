---
title: Type System
description: Hindley-Milner type inference, row polymorphism, reference capabilities, and algebraic data types.
---

## Hindley-Milner Inference

Nulang uses Algorithm W (Hindley-Milner) for global type inference. You rarely need to write type annotations — the compiler infers them:

```nulang
// The compiler infers: fn compose[A,B,C](f: B -> C, g: A -> B) -> A -> C
fn compose(f, g) {
    fn(x) { f(g(x)) }
}
```

Explicit annotations are supported and encouraged for public APIs:

```nulang
fn compose[A, B, C](f: B -> C, g: A -> B) -> A -> C {
    fn(x: A) -> C { f(g(x)) }
}
```

## Primitive Types

| Type | Description | Example |
|------|-------------|---------|
| `Int` | 48-bit signed integer (NaN-tagged) | `42` |
| `Float` | 64-bit IEEE 754 float | `3.14` |
| `Bool` | Boolean | `true`, `false` |
| `String` | UTF-8 string | `"hello"` |
| `Unit` | Unit value (like `void`) | `()` |
| `Nil` | Nil/null | `nil` |
| `Never` | Uninhabited type (bottom) | — |

## Row-Polymorphic Records

Records are structurally typed with row polymorphism. An open record carries a row tail variable encoded as `..`:

```nulang
// This function accepts ANY record with 'x' and 'y' fields of type Int
fn distance(point: { x: Int, y: Int }) -> Float {
    sqrt(to_float(point.x * point.x + point.y * point.y))
}

// Works with extra fields
let p3d = { x: 1, y: 2, z: 3 }
distance(p3d)  // OK
```

Closed record annotations require exact field counts:

```nulang
fn origin(): { x: Int, y: Int } = { x: 0, y: 0 }
```

## Reference Capabilities

Inspired by Pony, Nulang uses reference capabilities for data-race freedom:

| Capability | Deny Read | Deny Write | Sendable | Description |
|------------|-----------|------------|----------|-------------|
| `iso` | Yes | Yes | Yes | Isolated, unique reference |
| `trn` | No | Yes | No | Transitional, write-unique |
| `ref` | No | No | No | Mutable, shared-nothing |
| `val` | No | Yes | Yes | Immutable, shareable |
| `box` | Yes | No | No | Read-only |
| `tag` | Yes | Yes | Yes | Opaque, identity-only |
| `lineariso` | Yes | Yes | Yes | Linear isolated (at-most-once) |

Capabilities are **compile-time only** and erased at runtime. There are no runtime capability checks.

```nulang
// iso reference: can be sent between actors
let data: &iso { x: Int } = { x: 42 }

// val reference: immutable and shareable
let shared: &val String = "hello"
```

## Algebraic Data Types

Sum types via `type` declarations:

```nulang
type Option[T] = Some(T) | None
type Result[T, E] = Ok(T) | Err(E)
type List[T] = Cons(T, List[T]) | Nil
```

Generic type parameters use `[T, U, ...]` syntax and are inferred at call sites.

## Effect Types

Every function carries an effect row. Pure functions have an empty effect row. Effectful functions declare their effects:

```nulang
// Pure: no effects
fn add(x: Int, y: Int) -> Int = x + y

// Effectful: performs IO
fn greet(): {IO} Unit {
    perform IO.print("Hello")
}
```

See [Algebraic Effects](/language/effects/) for the full effect system.
