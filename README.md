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
| **Cranelift JIT Backend** | Tiered execution: interpreter for cold code, native compilation for hot loops |
| **BEAM/OTP Primitives** | `receive`, `spawn_link`, `monitor`, `link`, registry, timers, process groups |
| **Linear Type Moves** | Zero-cost `iso` actor messaging via compile-time linearity tracking |
| **SIMD Vectorization** | Auto-vectorization of array loops via Cranelift SIMD (I64x2, F64x2, I32x4, F32x4) |
| **Python Interop** | Import and call Python libraries (PyTorch, Transformers, NumPy) via PyO3 |
| **Dual-Region Heaps** | Generational GC with nursery + tenured spaces |
| **Escape Analysis** | Stack-allocate non-escaping objects, reducing GC pressure |

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
                                                           v
                                              +-------------------------+
                                              |   Cranelift JIT Tier    |
                                              |  (Tiered Execution)     |
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
| `jit/compiler` | Bytecode → Cranelift IR (30 opcodes, type-directed optimization) | ~400 |
| `jit/runtime` | NaN-tag-aware runtime helpers for JIT (30 extern C functions) | ~150 |
| `jit/mod` | JIT session manager, tiered execution, hot-counter tracking | ~200 |
| `runtime/actor` | Actor struct, lifecycle, state management | ~120 |
| `runtime/scheduler` | Work-stealing M:N scheduler with reductions | ~200 |
| `runtime/mailbox` | Lock-free MPSC via crossbeam ArrayQueue (ABA-safe) | ~200 |
| `runtime/timer` | Hierarchical timer wheel for send_after, exit_after, kill_after | ~220 |
| `runtime/registry` | Local actor name registry (register/whereis/registered) | ~230 |
| `runtime/process_groups` | Decentralized actor group membership (Erlang pg) | ~165 |
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

**Total: ~32,000 lines of Rust across 37 source files with 530+ tests.**

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

### v0.7 — BEAM/OTP Primitives

35+ Erlang/OTP primitives analyzed and 14 core primitives implemented:

| Primitive | File | Description |
|-----------|------|-------------|
| `receive` | `vm.rs` | Pattern-matching mailbox consume with timeout |
| `spawn_link` | `vm.rs` | Spawn actor with bidirectional fault link |
| `monitor` | `runtime/mod.rs` | Watcher monitors target actor for exit |
| `demonitor` | `runtime/mod.rs` | Remove a monitor |
| `link`/`unlink` | `runtime/mod.rs` | Bidirectional fault tolerance links |
| `exit` | `vm.rs` | Typed actor exit with `ExitReason` enum |
| `trap_exit` | `runtime/actor.rs` | Convert exit signals to messages |
| `register`/`whereis` | `runtime/registry.rs` | Local actor name registry |
| `send_after` | `runtime/timer.rs` | Hierarchical timer wheel |
| `pg` process groups | `runtime/process_groups.rs` | Decentralized actor groups |
| `yield` | `vm.rs` | Cooperative scheduling yield |

### v0.8 — Performance Improvements

Three high-ROI changes implemented in parallel:

| # | Proposal | Change | Impact |
|---|----------|--------|--------|
| 2.3 | mimalloc | `#[global_allocator]` → MiMalloc | 10-20% throughput |
| 2.1 | Lock-free mailboxes | `crossbeam::ArrayQueue` | ABA-safe, cache-line optimized |
| 4.2 | Linear type moves | `Capability::LinearIso` + consumption tracking | Zero-cost `iso` sends |

### v0.9 — Cranelift JIT Backend

Tiered execution system with Cranelift 0.132:

| Component | Description |
|-----------|-------------|
| `JitSession` | Manages Cranelift JIT module, compiled function cache |
| `compiler.rs` | Bytecode → CLIF for 30 opcodes (arith, compare, control flow) |
| `runtime.rs` | 30 `extern "C"` NaN-tag-aware runtime helpers |
| Tiered execution | Interpreter (cold) → JIT compile (hot threshold: 1,000) |
| Graceful fallback | Unsupported opcodes → continue interpreting |

### v0.10 — Type Guard Stripping + LSP Inlay Hints

**Type Guard Stripping** (proposal 1.2): When the typechecker knows a register holds `Int` or `Float`, the JIT emits direct CLIF instructions (`iadd`, `fadd`) instead of calling NaN-tag-aware runtime helpers. Eliminates ~30% of overhead in numeric loops.

| Component | Description |
|-----------|-------------|
| `typed_compiler.rs` | Type-directed JIT with `TypeMetadata` / `KnownType` |
| Inline sext48 | Sign-extend in ~5 CLIF instructions (was: runtime call) |
| Typed binops | Direct `iadd`/`fadd`/`imul` when operand types known |
| Fallback | Unknown types → same runtime helpers as v0.9 |

**LSP Inlay Hints** (proposal 6.1): Language Server Protocol support with inline type annotations.

| Component | Description |
|-----------|-------------|
| `lsp/mod.rs` | `tower-lsp` server with `textDocument/inlayHint` |
| Type inlays | `let x = 42` shows `: Int` after `x` |
| Capability inlays | `let y :iso String` highlights `:iso` |
| Effect inlays | `fun f() ! IO` shows `[IO]` |

### v0.11 — SIMD Vectorization

Auto-vectorization of element-wise array loops using Cranelift SIMD instructions:

| Component | Description |
|-----------|-------------|
| `simd_analyzer.rs` | Pattern detection for vectorizable loops (`c[i] = a[i] + b[i]`) |
| `simd_compiler.rs` | SIMD CLIF emission (I64x2, F64x2, I32x4, F32x4) |
| 8 vectorization checks | Uniform access, no loop-carried deps, no calls in loop, etc. |
| Scalar prefix/epilogue | Handles `trip_count % vector_width` elements individually |
| `is_simd_supported()` | Runtime CPU feature detection (SSE2/NEON) |
| Tiered integration | `CompiledSimdAndRan` action in tiered execution |
| ISA flag | `enable_simd = true` in Cranelift settings |

### v0.12 — Dual-Region Heaps + Escape Analysis

Generational heap partitioning with compile-time escape analysis to reduce GC pressure:

| Component | Description |
|-----------|-------------|
| `dual_heap.rs` | Nursery (256KB) + Tenured generational heap |
| `NurseryRegion` | Fast bump allocator for short-lived objects |
| `TenuredRegion` | Full size-class free lists for long-lived objects |
| Minor GC | Collect nursery only; promote survivors to tenured |
| Promotion threshold | Objects surviving 2 minor collections promoted |
| `escape_analysis.rs` | Bytecode-level flow analysis for object lifetimes |
| `EscapeStatus` | `NoEscape` / `ArgEscape` / `GlobalEscape` classification |
| Stack allocation | `NoEscape` objects bypass heap entirely |
| `OrcaHeap` trait | Dual heap is compatible with existing ORCA GC |

**Expected impact:** 30-50% reduction in GC pressure for typical workloads by:
- Avoiding heap allocation for temporary objects (escape analysis)
- Fast minor GC of nursery only (O(nursery) not O(all objects))
- Promoting long-lived objects out of the fast path

### v0.13 — Python Interop

Direct integration with the Python ecosystem via PyO3. This is the critical path for AI adoption — enabling Nulang to leverage PyTorch, Transformers, NumPy, and the entire Python ML stack.

```nulang
import python "torch" as torch
import python "transformers" as transformers

let tensor = torch.Tensor([1.0, 2.0, 3.0])
let result = tensor.sum()
perform IO.print(result)  -- 6.0
```

| Component | Description |
|-----------|-------------|
| `python/bridge.rs` | PyO3 interpreter bridge with GIL management |
| `python/marshal.rs` | Bidirectional Nulang Value ↔ Python object conversion |
| `python/mod.rs` | Module exports and public API |
| `PyBridge` | Import modules, get/set attributes, call functions |
| `PythonRegistry` | Thread-safe slab allocator for Python object handles |
| `TAG_PYTHON` | NaN tag (0x7FFE...) for Python object references in VM |
| 8 new opcodes | PyImport, PyGetAttr, PyCall, PyCallKw, PySetAttr, PyToNu, PyFromNu, PyRelease |
| Value constructors | `Value::python_object(id)`, `Value::as_python_object_id()` |
| VM dispatch | Full opcode implementation with error propagation |
| Compiler emission | `emit_py_import`, `emit_py_getattr`, `emit_py_call`, etc. |
| `NuError::PythonError` | Dedicated error variant for Python interop failures |
| 26 tests | Registry, import, getattr, call, marshal round-trips, VM dispatch |

**Design documents created:**
- [DESIGN_AI_SDK.md](DESIGN_AI_SDK.md) — OpenAI SDK equivalent (1,309 lines)
- [DESIGN_WORKFLOW_SDK.md](DESIGN_WORKFLOW_SDK.md) — Temporal SDK equivalent (1,774 lines)
- [DESIGN_WEB_FRAMEWORK.md](DESIGN_WEB_FRAMEWORK.md) — Phoenix Framework equivalent (1,978 lines)
- [DESIGN_PACKAGE_MANAGER.md](DESIGN_PACKAGE_MANAGER.md) — Cargo-equivalent package manager (1,627 lines)
- [DESIGN_CLOUD.md](DESIGN_CLOUD.md) — Nulang Cloud platform architecture (1,738 lines)

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
- [x] Bytecode (91 opcodes, constant pool, behavior table)
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
- [x] BEAM/OTP primitives (receive, spawn_link, monitor, link, exit, trap_exit, register, whereis, send_after, pg, yield)
- [x] Lock-free mailboxes (crossbeam ArrayQueue, ABA-safe)
- [x] Hierarchical timer wheel (send_after, exit_after, kill_after)
- [x] Actor registry (register/whereis/registered)
- [x] Process groups (decentralized actor group membership)
- [x] Linear type moves (compile-time `iso` consumption tracking)
- [x] Cranelift JIT backend (tiered execution, 30 opcodes, hot-counter threshold)
- [x] Type guard stripping (direct CLIF when types known, ~30% speedup in numeric loops)
- [x] LSP inlay hints (type/capability/effect annotations via tower-lsp)
- [x] SIMD vectorization (auto-vectorize array loops: I64x2, F64x2, I32x4, F32x4)
- [x] Dual-region generational heap (nursery + tenured, minor GC)
- [x] Escape analysis (stack-allocate non-escaping objects)
- [x] Python interop (PyO3 bridge, 8 opcodes, bidirectional marshalling)
- [x] AI SDK design document (agent DSL, tool binding, memory, multi-agent)
- [x] Workflow SDK design document (durable actors, sagas, timers, signals)
- [x] Web Framework design document (endpoints, controllers, channels, LiveView)
- [x] Package Manager design document (manifest, resolver, registry, workspace)
- [x] Cloud Platform design document (global deploy, auto-scaling, persistence)

### Roadmap

| Phase | Feature | Status |
|-------|---------|--------|
| v0.2 | Hindley-Milner type checker + effect checker | Completed |
| v0.3 | Actor scheduler + supervision trees | Completed |
| v0.4 | ORCA garbage collector | Completed |
| v0.5 | Multi-node distribution | Completed |
| v0.6 | CRDT integration | Completed |
| v0.7 | BEAM/OTP primitives (receive, spawn_link, monitor, links, registry, timers, process groups) | Completed |
| v0.8 | Performance improvements (mimalloc, lock-free mailboxes, linear type moves) | Completed |
| v0.9 | Cranelift JIT backend | Completed |
| v0.10 | Type guard stripping + LSP inlay hints | Completed |
| v0.11 | SIMD vectorization (auto-vectorize array loops) | Completed |
| v0.12 | Dual-region heaps + escape analysis | Completed |
| v0.13 | Python interop + AI ecosystem design foundation | Completed |
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