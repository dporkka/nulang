---
title: Algebraic Effects
description: Koka-inspired row-polymorphic algebraic effects — perform, handle, and resume.
---

## Overview

Algebraic effects let you separate _what_ a program does from _how_ it does it. An effect declares operations; handlers provide implementations. Effects compose freely without monad transformers.

## Defining Effects

Effects are declared with `effect` followed by a name and a block of operation signatures. Each operation is `name: (arg_types) -> return_type`:

```nulang
effect State {
    get: () -> Int
    put: (Int) -> Unit
}

effect Logger {
    log: (String) -> Unit
}
```

## Performing Effects

`perform` invokes an operation. The compiler statically tracks which effects each function may perform via row-polymorphic effect types:

```nulang
fn counter() -> Int ! {State} {
    let current = perform State.get()
    perform State.put(current + 1)
    current
}
```

## Handling Effects

`handle` intercepts effect operations and provides implementations. The handler arm's body expression is the value resumed into the continuation:

```nulang
// Handle State with a fixed initial value
handle perform State.get() {
    | State.get() => 42
}
```

When the handler needs to perform side effects before providing the resume value, use the `resume` keyword in the arm pattern. The arm body becomes the resumed value:

```nulang
handle perform Logger.log("hello") {
    | Logger.log(msg) resume => {
        perform IO.print("[LOG] " + msg)
    }
}
```

The `resume` keyword marks that execution should continue at the `perform` site with the arm body's value. Without `resume`, the handler arm's value is the handle expression's result (the handler does not resume the continuation).

## Effect Rows

Effect types use row polymorphism, like records. The effect row appears in a function's type signature after the return type, introduced by `!` or `throws`:

```nulang
// Pure function — no effects
fn add(x: Int, y: Int) -> Int ! {} = x + y

// This function performs IO and State effects
fn program() -> Unit ! {IO, State} {
    perform IO.print("Running...")
    let v = perform State.get()
    perform IO.print(perform Int.to_string(v))
}
```

`throws` is an alias for `!`:

```nulang
fn log(msg: String) -> Unit throws {IO} {
    perform IO.print(msg)
}
```

## Built-in Effects

Nulang ships with several built-in effects wired into the VM and runtime:

| Effect | Operations | Description |
|--------|-----------|-------------|
| `IO` | `print`, `println`, `read` | Console I/O |
| `Int` | `to_string` | Integer-to-string conversion |
| `Timer` | `sleep` | Durable workflow timers |
| `Signal` | `wait` | Workflow signal suspension |
| `LLM` | `ask` | AI language model queries (deprecated — use `Provider.ask`) |
| `Provider` | `ask` | General provider abstraction (replaces `LLM.ask`) |
| `Actor` | `link`, `monitor`, `exit`, ... | Actor lifecycle management |
| `Otp` | `create_supervisor`, ... | OTP supervision trees |

See the [Standard Library](/stdlib/overview/) for full documentation of each built-in effect.