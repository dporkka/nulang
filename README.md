# Nulang

> A distributed actor-based programming language with AI agent support, built in Rust.

[![Rust](https://img.shields.io/badge/rust-2021%20Edition-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/github/license/dporkka/nulang.svg)](LICENSE)

---

## Overview

**Nulang** is a modern systems programming language that combines the fault-tolerant distributed computing model of Erlang with advanced type system features borrowed from Rust, Pony, and research languages. It is designed for building concurrent, distributed, and AI-agent-powered applications.

### Key Features

| Feature | Description |
|---------|-------------|
| **Actor Model** | Lightweight green-thread actors with M:N scheduling and work-stealing queues |
| **Algebraic Effects** | First-class effect system with `perform`/`handle`/`resume` semantics |
| **Capability System** | Fine-grained reference permissions (iso/trn/ref/val/box/tag) for memory safety |
| **AI Agent DSL** | Built-in syntax for defining LLM-powered agents with tool binding |
| **Distributed by Design** | Actor migration, CRDTs, and transparent cross-node messaging |
| **Register-Based VM** | High-performance bytecode VM with NaN-tagged value representation |
| **Zero Dependencies** | Self-contained Rust implementation with no external crates |

---

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (stable channel, 1.70+)
- Linux or macOS (Windows support planned)

### Building

```bash
git clone https://github.com/dporkka/nulang.git
cd nulang
cargo build --release
```

### Running Tests

```bash
cargo test
```

### REPL

```bash
cargo run
```

---

## Language Tour

### Hello, World

```nulang
fun main() =
  perform IO.print("Hello, World!")
```

### Actors

```nulang
actor Counter {
  state count: Int
  initial init

  behavior init() =
    receive
    | Tick =>
        self ! count(count + 1)
    | Get =>
        count
}
```

### Effects

```nulang
let result = handle compute() with
  | Log.msg(msg) =>
      perform IO.print(msg)
      resume ()
  | return(x) =>
      x
```

### AI Agents

```nulang
agent CodeReviewer {
  llm "gpt-4",
      "You are a senior code reviewer.",
      0.7

  tool analyze: SyntaxAnalysis,
       "Perform syntax analysis on the given code"

  behavior review(code: String) =
    let analysis = perform SyntaxAnalysis.analyze(code)
    let review = perform LLM.generate(analysis)
    review
}
```

### Pattern Matching

```nulang
match result with
| Ok(value) =>
    perform IO.print("Success: " <> value)
| Err(message) =>
    perform IO.print("Error: " <> message)
```

### Pipe Operator

```nulang
let processed =
  data
  |> transform
  |> filter(predicate)
  |> aggregate
```

---

## Architecture

```
                    +-------------------------+
                    |      Source Code        |
                    +-------------------------+
                              |
                              v
+----------+    +-------------------------+    +----------+
|  Lexer   |--->|     Parser (AST)        |--->| Compiler |
+----------+    +-------------------------+    +----------+
                              |                       |
                              v                       v
                    +-------------------------+  +-------------------+
                    |  Type Checker / Effects  |  | Bytecode Module   |
                    +-------------------------+  +-------------------+
                                                           |
                                                           v
                                              +-------------------------+
                                              |   Register-Based VM     |
                                              |  (Token-Threaded)       |
                                              +-------------------------+
                                                           |
                              +----------------------------+----------------------------+
                              |                            |                            |
                              v                            v                            v
                    +------------------+        +------------------+        +------------------+
                    | Actor Runtime    |        |   Scheduler      |        |   Mailbox/GC     |
                    | (Spawn/Send/    |        | (Work-Stealing   |        | (Bounded MPSC/   |
                    |  Receive)        |        |  M:N Threads)    |        |  Ref-Counting)   |
                    +------------------+        +------------------+        +------------------+
```

### Module Structure

| Module | Description |
|--------|-------------|
| `lexer` | Hand-written state machine, indentation-based tokenization |
| `parser` | Recursive descent with Pratt precedence climbing |
| `ast` | Abstract syntax tree definitions (25+ expression types) |
| `types` | Type system, capabilities, effects, NaN-tagged values |
| `bytecode` | 45 opcodes, 32-bit fixed-width instructions |
| `compiler` | AST-to-bytecode compilation with register allocation |
| `vm` | Register-based virtual machine with token-threaded dispatch |
| `effects` | Algebraic effect row subset and combine operations |
| `capabilities` | Permission lattice (join/meet) and access checking |
| `runtime` | Actor system, scheduler, mailbox, heap, GC, supervisor |
| `repl` | Interactive read-eval-print loop |

---

## Design Philosophy

1. **Fault Tolerance First** - Inspired by Erlang's "let it crash" philosophy with supervision trees
2. **Type Safety Without Ceremony** - Strong static typing with full inference
3. **Effects as Values** - Algebraic effects make computational context explicit
4. **Safe Sharing** - Capabilities control reference permissions at the type level
5. **AI-Native** - First-class support for LLM-powered agents as language primitives
6. **Zero-Cost Distribution** - Actors naturally span nodes; migration is transparent

---

## Project Status

This is an **MVP implementation** with the following components functional:

- [x] Lexer (full token set, indentation handling)
- [x] Parser (all expression types, declarations, actor/agent definitions)
- [x] AST (complete node types)
- [x] Bytecode (45 opcodes, constant pool, behavior table)
- [x] Compiler (AST to bytecode for all core constructs)
- [x] VM (register-based execution, arithmetic, comparisons, control flow)
- [x] Type system foundations (types, capabilities, effects)
- [x] Actor runtime model (spawn, send, receive interfaces)
- [x] REPL (parse-compile-execute cycle)

### Roadmap

| Phase | Feature | Status |
|-------|---------|--------|
| v0.2 | Full type checker (Hindley-Milner inference) | Planned |
| v0.3 | Actor scheduler with work-stealing | Planned |
| v0.4 | ORCA garbage collector | Planned |
| v0.5 | Multi-node distribution | Planned |
| v0.6 | CRDT integration | Planned |
| v0.7 | LLM agent runtime | Planned |
| v0.8 | Package manager | Planned |
| v0.9 | LSP server | Planned |
| v1.0 | Production release | Planned |

---

## Documentation

- **[SPEC.md](SPEC.md)** - Complete 60,000-word language specification covering syntax, semantics, type system, runtime, and standard library

---

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

## Acknowledgments

- **Erlang/OTP** for the actor model and fault-tolerance philosophy
- **Pony** for the capability system and ORCA GC design
- **Koka** and **Eff** for algebraic effects
- **Rust** for ownership-based memory safety inspiration

---

> *"Concurrency should be a language primitive, not a library afterthought."*
