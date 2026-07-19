---
title: Standard Library Overview
description: Built-in effects and operations wired into the Nulang VM and runtime.
---

## Built-in Effects

Nulang ships with a set of built-in effects wired directly into the VM and runtime. These effects provide core functionality — I/O, timing, signals, LLM integration, actor management, and OTP supervision.

Each effect groups related operations accessed via `perform Effect.operation(...)`:

| Effect | Operations | Implements |
|--------|-----------|------------|
| [IO](/stdlib/io/) | `print`, `println`, `read` | Console input/output |
| [Int](/stdlib/int/) | `to_string` | Integer conversions |
| [Timer](/stdlib/timer/) | `sleep` | Durable workflow timers |
| [Signal](/stdlib/signal/) | `wait` | Workflow signal suspension |
| [LLM](/stdlib/llm/) | `ask` | AI language model queries |
| [Actor](/stdlib/actor/) | `link`, `unlink`, `monitor`, `demonitor`, `trap_exit`, `exit`, `register`, `unregister`, `whereis`, `set_priority` | Actor lifecycle management |
| [Otp](/stdlib/otp/) | `create_supervisor`, `supervise_child`, `set_template`, `start_child`, `terminate_child`, `child_count` | OTP supervision trees |

## Implementation Sites

Built-in operations are implemented in one of two places:

- **Standalone VM** (`ImplSite::StandaloneVm`) — Operations handled by the VM directly, available in actor-free scripts (e.g., REPL, one-shot `--eval`).
- **Runtime Host** (`ImplSite::RuntimeHost`) — Operations that require the actor runtime, reached through the `ActorVmCallbacks` trait. These are nil no-ops outside an actor context.

## Using Built-in Effects

```nulang
// IO effects work everywhere
perform IO.print("Hello, World!")
perform IO.println("With a newline")
let input = perform IO.read()

// Actor effects require the runtime
perform Actor.register("my_service")
perform Actor.link(some_actor)

// OTP effects for supervision
let sup = perform Otp.create_supervisor("my_sup", 0)
```

## Adding New Built-in Effects

New built-in operations are registered in the `StdLib` registry (`src/stdlib.rs`). The registry is static — it mirrors dispatch sites in `vm.rs` and `runtime/mod.rs` and is updated by hand when a new built-in is wired.

Each entry requires:

```rust
BuiltinOp {
    name: "Effect.operation",      // Fully-qualified name
    effect: "Effect",              // Effect namespace
    op: "operation",               // Operation within the effect
    signature: "op(args) -> Ret",  // Human-readable signature
    implemented_in: ImplSite::..., // Where it's dispatched
    description: "...",            // One-line behavior description
}
```

The auto-generated per-effect reference pages are produced from this registry.
