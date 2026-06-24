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
| **Actor Model** | Lightweight green-thread actors with M:N scheduling, work-stealing queues, and supervision trees |
| **Algebraic Effects** | First-class effect system with `perform`/`handle`/`resume` semantics |
| **Capability System** | Fine-grained reference permissions (iso/trn/ref/val/box/tag) for memory safety |
| **AI Agent DSL** | Built-in syntax for defining LLM-powered agents with tool binding |
| **Distributed Runtime** | Location-transparent actor messaging across nodes with TCP transport |
| **ORCA GC** | Per-actor concurrent garbage collection with cycle detection |
| **CRDTs** | 8 conflict-free replicated data types for shared distributed state |
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
cargo run -- --repl
```

### CLI Modes

```bash
# Compile and run a file
cargo run -- myprogram.nl

# Type-check only
cargo run -- --check myprogram.nl

# Evaluate a string
cargo run -- --eval 'perform IO.print("Hello")'
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
                    |  Type Checker (H-M)     |  | Bytecode Module   |
                    |  Effect Checker         |  | (64 opcodes)      |
                    +-------------------------+  +-------------------+
                                                           |
                                                           v
                                              +-------------------------+
                                              |   Register-Based VM     |
                                              |  (Token-Threaded)       |
                                              +-------------------------+
                                                           |
                    +--------------------------------------+---------------------------+
                    |                                      |                           |
                    v                                      v                           v
          +------------------+                    +------------------+        +------------------+
          | Actor Runtime    |                    |   Scheduler      |        |   ORCA GC        |
          | (Spawn/Send/     |                    | (Work-Stealing   |        | (Per-Actor Heap, |
          |  Receive/Links)  |                    |  M:N Threads)    |        |  Ref Counting,   |
          +--------+---------+                    +------------------+        |  Cycle Detect)   |
                   |                                                          +------------------+
                   |
         +---------+------------------------------------------+
         |                                                    |
         v                                                    v
+------------------+                            +-------------------------+
| Supervision      |                            | Distributed Runtime     |
| (OneForOne/All/  |                            | (TCP Transport, Cluster |
|  RestForOne)     |                            |  Membership, CRDT Sync) |
+------------------+                            +-------------------------+
                                                           |
                                                           v
                                               +-------------------------+
                                               | CRDT Manager            |
                                               | (GCounter, PNCounter,   |
                                               |  GSet, ORSet, AWORSet,  |
                                               |  LWWReg, MVReg, RGA)    |
                                               +-------------------------+
```

### Module Structure

| Module | Description | Lines |
|--------|-------------|-------|
| `lexer` | Hand-written state machine, indentation-based tokenization | ~550 |
| `parser` | Recursive descent with Pratt precedence climbing | ~1,400 |
| `ast` | Abstract syntax tree definitions (30+ expression types) | ~400 |
| `types` | Type system, capabilities, effects, NaN-tagged values | ~600 |
| `typechecker` | Hindley-Milner Algorithm W with full inference | ~2,500 |
| `effect_checker` | Algebraic effect row checking + capability analysis | ~1,750 |
| `bytecode` | 64 opcodes, 32-bit fixed-width instructions | ~300 |
| `compiler` | AST-to-bytecode compilation with register allocation | ~1,000 |
| `vm` | Register-based virtual machine with token-threaded dispatch | ~1,200 |
| `effects` | Algebraic effect row subset and combine operations | ~200 |
| `capabilities` | Permission lattice (join/meet) and access checking | ~150 |
| `runtime/actor` | Actor struct, lifecycle, state management | ~120 |
| `runtime/scheduler` | Work-stealing M:N scheduler with reductions | ~200 |
| `runtime/mailbox` | Bounded MPSC mailbox with priority queues | ~180 |
| `runtime/heap` | Per-actor bump allocator with ORCA object headers | ~1,030 |
| `runtime/gc` | ORCA reference counting (3-count protocol) | ~1,400 |
| `runtime/orca_cycle` | Centralized cycle detector with weighted heuristic | ~1,550 |
| `runtime/supervisor` | Erlang/OTP-style supervision strategies | ~465 |
| `runtime/network` | TCP transport, NUL0 wire protocol | ~1,390 |
| `runtime/cluster` | Gossip-based cluster membership + failure detection | ~1,080 |
| `runtime/distributed` | Location-transparent actor addressing | ~1,140 |
| `runtime/crdt` | CRDT trait + GCounter, PNCounter, GSet, ORSet, AWORSet | ~1,680 |
| `runtime/crdt_reg` | LWWRegister, MVRegister, RGA sequence CRDT | ~1,170 |
| `runtime/crdt_manager` | CRDT factory, sync ops, inter-node merge | ~450 |
| `runtime/tests` | Integration tests (fault tolerance, GC, distributed, CRDTs) | ~2,050 |
| `repl` | Interactive REPL with :type, :ast, :bytecode commands | ~490 |
| `main` | CLI entry point (run, repl, eval, check modes) | ~450 |

**Total: ~25,000 lines of Rust across 28 source files with 340+ tests.**

---

## Implemented Subsystems

### v0.4 — ORCA Garbage Collector

Nulang uses the **ORCA (Optimized Reference Counting Architecture)** protocol from Pony for memory management. Each actor has its own heap, and garbage is collected without global stop-the-world pauses.

| Component | Description |
|-----------|-------------|
| `ActorHeap` | Bump allocator with 5 size-class free lists and live-object tracking |
| `OrcaGc` | Three-count reference counting (local/foreign/sticky) |
| `OrcaCoordinator` | Routes cross-actor reference operations between nodes |
| `CycleDetector` | Weighted-heuristic DFS cycle detection with trial decrements |

### v0.5 — Multi-Node Distribution

Actors can communicate across machine boundaries with location-transparent messaging.

| Component | Description |
|-----------|-------------|
| `NetworkTransport` | TCP-based transport with NUL0 binary wire protocol |
| `ClusterState` | Gossip-based membership with heartbeat failure detection |
| `ActorAddress` | Location-transparent addressing (local or remote) |
| `AddressResolver` | Resolves addresses to local actors or network routes |

**API:**
```rust
rt.enable_distribution("0.0.0.0:7878".parse()?)?;
rt.join_cluster("192.168.1.100:7878".parse()?);
rt.send_distributed(ActorAddress::remote(node_id, actor_id), "hello", &[]);
rt.process_network();  // handle incoming packets
```

### v0.6 — CRDT Integration

Eight conflict-free replicated data types enable actors to share mutable state across nodes without coordination.

| CRDT | Type | Operations | Use Case |
|------|------|-----------|----------|
| `GCounter` | Counter | increment | Page views, likes |
| `PNCounter` | Counter | increment, decrement | Inventory, voting |
| `GSet` | Set | insert | Tags, followers |
| `ORSet` | Set | add, remove (add-wins) | Shopping cart |
| `AWORSet` | Set | add, remove (timestamp) | Collaborative todo |
| `LWWRegister` | Register | write | Profile name, config |
| `MVRegister` | Register | write (multi-value) | Conflict detection |
| `RGA` | Sequence | insert, delete | Collaborative text |

**API:**
```rust
let (id, _) = rt.crdt_manager.as_mut().unwrap().create_gcounter();
rt.crdt_manager.as_mut().unwrap().get_gcounter_mut(id).unwrap().increment_by(5);
rt.sync_crdts();  // broadcast to all connected nodes
```

---

## Design Philosophy

1. **Fault Tolerance First** — Inspired by Erlang's "let it crash" philosophy with supervision trees
2. **Type Safety Without Ceremony** — Strong static typing with Hindley-Milner full inference
3. **Effects as Values** — Algebraic effects make computational context explicit
4. **Safe Sharing** — Capabilities control reference permissions at the type level
5. **AI-Native** — First-class support for LLM-powered agents as language primitives
6. **Zero-Cost Distribution** — Actors naturally span nodes; CRDTs share state without coordination

---

## Project Status

This is an active implementation with the following components functional:

- [x] Lexer (full token set, indentation handling)
- [x] Parser (all expression types, declarations, actor/agent definitions)
- [x] AST (complete node types)
- [x] Hindley-Milner type checker (Algorithm W with full inference)
- [x] Algebraic effect checker (effect row compatibility, capability analysis)
- [x] Bytecode (64 opcodes, constant pool, behavior table)
- [x] Compiler (AST-to-bytecode with register allocation)
- [x] VM (register-based execution, arithmetic, comparisons, control flow)
- [x] REPL (parse-typecheck-compile-execute cycle with introspection)
- [x] Integration tests (16 end-to-end pipeline tests)
- [x] Actor runtime (spawn, send, receive, links, monitors)
- [x] Work-stealing scheduler (M:N threading with reduction quotas)
- [x] ORCA garbage collector (per-actor heap, 3-count protocol, cycle detection)
- [x] Supervision trees (OneForOne, OneForAll, RestForOne restart strategies)
- [x] Fault tolerance tests (18 supervisor/exit/link/monitor tests)
- [x] Distributed runtime (TCP transport, cluster membership, location-transparent messaging)
- [x] CRDT subsystem (8 types: counters, sets, registers, sequences)
- [x] CRDT manager (factory, sync, inter-node merge)

### Roadmap

| Phase | Feature | Status |
|-------|---------|--------|
| v0.2 | Hindley-Milner type checker + effect checker | Completed |
| v0.3 | Actor scheduler + supervision trees | Completed |
| v0.4 | ORCA garbage collector | Completed |
| v0.5 | Multi-node distribution | Completed |
| v0.6 | CRDT integration | Completed |
| v0.7 | LLM agent runtime | Planned |
| v0.8 | Package manager | Planned |
| v0.9 | LSP server | Planned |
| v1.0 | Production release | Planned |

---

## Documentation

- **[SPEC.md](SPEC.md)** — Complete 60,000-word language specification covering syntax, semantics, type system, runtime, and standard library

---

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

## Acknowledgments

- **Erlang/OTP** for the actor model and fault-tolerance philosophy
- **Pony** for the capability system and ORCA GC design
- **Koka** and **Eff** for algebraic effects
- **Rust** for ownership-based memory safety inspiration
- **Shapiro et al.** for CRDT theory and the state-based replication model

---

> *"Concurrency should be a language primitive, not a library afterthought."*
