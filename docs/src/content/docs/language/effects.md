---
title: Algebraic Effects
description: Koka-inspired row-polymorphic algebraic effects — perform, handle, resume, and unwind.
---

## Overview

Algebraic effects let you separate _what_ a program does from _how_ it does it. An effect declares operations; handlers provide implementations. Effects compose freely without monad transformers.

## Defining Effects

```nulang
effect State[T] {
    op get() -> T
    op put(value: T) -> Unit
}

effect Logger {
    op log(msg: String) -> Unit
}
```

## Performing Effects

`perform` invokes an operation. The compiler statically tracks which effects each function may perform via row-polymorphic effect types:

```nulang
fn counter(): {State[Int]} Int {
    let current = perform State.get()
    perform State.put(current + 1)
    current
}
```

## Handling Effects

`handle` intercepts effect operations and provides implementations:

```nulang
// Handle State[Int] with a mutable cell
handle {
    counter();
    perform State.get()
} {
    | State.get() => {
        resume(42) // provide initial value via resume
    }
    | State.put(v) => {
        resume(()) // acknowledge the put
    }
}
```

## Resume and Continuations

`resume` passes a value back to the continuation — the code that follows the `perform`. The continuation is deep-cloned at the point of `perform`:

```nulang
effect Amb {
    op flip() -> Bool
}

// Handles flip by resuming twice — once for each branch
handle {
    let x = if perform Amb.flip() { 1 } else { 2 }
    let y = if perform Amb.flip() { 10 } else { 20 }
    x + y
} {
    | Amb.flip() => {
        resume(true);   // first branch
        resume(false)   // second branch
    }
}
```

## The Unwind Operation

`unwind` terminates an effect handler without resuming. Use it for early exit:

```nulang
effect Except {
    op raise(msg: String) -> Never
}

handle {
    perform Except.raise("oops")
} {
    | Except.raise(msg) => {
        perform IO.print("Caught: " + msg)
        // unwind: don't resume, just exit
        unwind(())
    }
}
```

## Effect Rows

Effect types use row polymorphism, like records. The effect row appears in a function's type signature after the return type:

```nulang
// This function performs both IO and State[Int] effects
fn program(): {IO, State[Int]} Unit {
    perform IO.print("Running...")
    let v = perform State.get()
    perform IO.print(Int.to_string(v))
}
```

## Built-in Effects

Nulang ships with several built-in effects wired into the VM and runtime:

| Effect | Operations | Description |
|--------|-----------|-------------|
| `IO` | `print`, `println`, `read` | Console I/O |
| `Timer` | `sleep` | Durable workflow timers |
| `Signal` | `wait` | Workflow signal suspension |
| `LLM` | `ask` | AI language model queries |
| `Actor` | `link`, `monitor`, `exit`, ... | Actor lifecycle management |
| `Otp` | `create_supervisor`, ... | OTP supervision trees |

See the [Standard Library](/stdlib/overview/) for full documentation of each built-in effect.
