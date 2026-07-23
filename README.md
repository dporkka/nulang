<p align="center">
  <img src="docs/src/assets/logo.svg" width="120" alt="Nulang logo">
</p>
<h1 align="center">Nulang</h1>
<p align="center">
  A durable computation language for long-lived, distributed, stateful software entities.
</p>
<p align="center">
  <a href="https://nulang.org">Website</a> •
  <a href="https://nulang.cloud">Nulang Cloud</a> •
  <a href="https://github.com/dporkka/nulang">GitHub</a>
</p>
<p align="center">
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/rust-2021%20Edition-orange.svg" alt="Rust 2021"></a>
  <a href="https://github.com/dporkka/nulang/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue.svg" alt="License Apache 2.0"></a>
  <a href="https://github.com/dporkka/nulang/actions"><img src="https://github.com/dporkka/nulang/workflows/CI/badge.svg" alt="CI"></a>
  <a href="https://codecov.io/github/dporkka/nulang"><img src="https://codecov.io/github/dporkka/nulang/graph/badge.svg" alt="Coverage"></a>
</p>

---

## Overview

**Nulang** is the language for building software that keeps running — across failures, restarts, and node boundaries — without constant human intervention.

If you are building AI agents that must remember state, durable workflows that survive crashes, distributed services that stay available under load, or any long-lived software entity that must outlast a single process, Nulang gives you a single, coherent foundation instead of a pile of bolted-on libraries.

### What you get

- **Fault tolerance by default** — supervision trees, links, and monitors turn crashes into recoverable events, not outages.
- **Distribution without rewiring** — actors and messages work the same whether they run on one node or a cluster.
- **Durable execution** — actors, workflows, and entities checkpoint state and resume after restarts with saga compensation for failures.
- **Composable capabilities** — AI, storage, networking, and external services are expressed through the same effect system and live in libraries, not the language core.
- **Memory safety without runtime pauses** — reference capabilities and per-actor ORCA GC keep you safe while actors stay responsive.
- **Compile once, run anywhere** — bytecode, native AOT, or WASM backends from the same source.

### Key Features

| Feature | Description |
|---------|-------------|
| **Actor Model** | Lightweight actors with cooperative scheduling, work-stealing queues, and supervision trees that isolate and recover from failures |
| **Algebraic Effects** | First-class effect system with `perform`/`handle`/`resume` semantics |
| **Capability System** | Fine-grained reference permissions (iso/trn/ref/val/box/tag/lineariso) for memory safety |
| **AI & External Capabilities** | LLMs, vector search, and external services composed through effects and Cloud SDK libraries, not language primitives |
| **Distributed Runtime** | Location-transparent actor messaging so you can scale from one node to a cluster without rewriting code |
| **ORCA GC** | Per-actor concurrent garbage collection with cycle detection |
| **CRDTs** | 8 conflict-free replicated data types for shared distributed state |
| **Register-Based VM** | High-performance bytecode VM with NaN-tagged value representation |
| **Cranelift JIT Backend** | Tiered execution: interpreter for cold code, JIT compilation for hot loops |
| **Native/AOT Backend** | Ahead-of-time compilation to native object code via Cranelift |
| **BEAM/OTP Primitives** | `link`/`monitor`/`exit`/`trap_exit`/registry via `perform Actor.*`, `spawn link`/`spawn monitor`, selective `receive` with `after` timeout, actor priority (`Actor.set_priority`), timers, process groups |
| **SIMD Vectorization** | Auto-vectorization of array loops via Cranelift SIMD (I64x2, F64x2, I32x4, F32x4) + WASM SIMD backend |
| **WASM Backend** | MIR→WASM compiler (`wasm-encoder`) + Wasmtime host runtime with guard pages, inlining, SIMD, and AOT compilation |
| **Python Interop** | Native Actor pattern: Python isolated to dedicated OS threads, marshal-only boundary |
| **Unbounded Mailboxes** | Lock-free MPSC queues (crossbeam::SegQueue) — BEAM-semantics, no message loss |
| **Stress Test Suite** | 30 chaos tests for supervision, scheduler fairness, GC, persistence, CRDTs, and JIT fallback under load |

### Current Status

Nulang is **Alpha** — but not a greenfield project. The compiler pipeline, VM and JIT, actor runtime, supervision, effects, capabilities, distribution, durability, and AI runtime all exist and are tested today:
- ✅ All 1392 tests pass with `cargo test` (1424 with `--features wasm-backend`)
- ✅ Builds with `cargo build`
- ✅ i64-tagged `Value` representation with distinct high-16 type tags (canonical constants in `src/value_layout.rs`) — immune to WASM NaN canonicalization
- ✅ 138-opcode bytecode ISA (arithmetic, control flow, closures, objects, effects, actors, FFI, Python, distribution)
- ✅ Hindley-Milner type inference with algebraic effects, user-declared variant types (construction + recursive pattern matching with guards), and row-polymorphic records (`fn(r) r.x + r.y` accepts any record with `x` and `y`; closed record annotations stay exact)
- ✅ Actor runtime: spawn, `spawn link`/`spawn monitor`, send, monitors, links, supervision, timers, registry, process groups, selective `receive` with `after`, actor priority
- ✅ ORCA-style per-actor GC with cycle detection
- ✅ AI runtime: `agent` declarations, LLM providers (OpenAI, Ollama), memory, pipelines, debates, supervisor teams
- ✅ Durable workflow runtime: `workflow` declarations with steps, timers, signals, saga compensation
- ✅ Format stability: frozen `.nbc` bytecode artifacts, NUL0 wire protocol versioning, language version `1.0.0-frozen` (RFC 0001/0002)
- ✅ `entity` declarations: durable-first actors (event-sourced by default) for long-lived domain objects

---

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (stable channel, 1.93+)
- Python 3 development headers, for the default build (see [Feature flags](#feature-flags) to skip this)
- Linux or macOS (Windows support planned)

### Building

```bash
git clone https://github.com/dporkka/nulang.git
cd nulang
cargo build --release
```

### Feature flags

Four optional subsystems are on by default so a plain `cargo build` behaves
as before this flag set existed. Build without them for a leaner binary and
fewer system dependencies:

| Feature | Enables | Off by default? |
|----------|---------|-----------------|
| `python` | PyO3 Python interop (`src/python/`) | No — on by default |
| `sqlite` | libsql/Turso persistence (`persistence.rs`) | No — on by default |
| `lsp` | tower-lsp language server (`src/lsp/`) | No — on by default |
| `ai-runtime` | AI runtime — LLM providers, pipelines, debates, supervisor teams (`src/ai/`) | No — on by default |
| `wasm-backend` | WASM compiler (`mir_wasm.rs`) + Wasmtime runtime (`wasm_runtime.rs`), `--backend wasm\|wasm-run\|wasm-aot` | Yes — requires `wasmtime` CLI for AOT |

```bash
# Build with WASM backend enabled:
cargo build --release --features wasm-backend
```
```bash
# Skip PyO3, libSQL, and the LSP server entirely:
cargo build --release --no-default-features

# Pick just what you need:
cargo build --release --no-default-features --features sqlite
```

### Running Tests

```bash
cargo test
```

### CLI Modes

```bash
# Compile and run a file (bytecode backend, default)
cargo run -- myprogram.nula

# Type-check only
cargo run -- --check myprogram.nula

# Evaluate a string
cargo run -- --eval 'perform IO.print("Hello")'

# Native/AOT backend: compile to native code via Cranelift
cargo run -- --backend native myprogram.nula

# WASM backend: compile to out.wasm
cargo run --features wasm-backend -- --backend wasm myprogram.nula

# WASM backend: compile and run via Wasmtime
cargo run --features wasm-backend -- --backend wasm-run myprogram.nula

# WASM backend: compile to .wasm + AOT .cwasm (requires wasmtime CLI)
cargo run --features wasm-backend -- --backend wasm-aot myprogram.nula

# Package manager (also: nula build-wasm for WASM AOT builds)
cargo run -- nula new my-app
```

### Examples

Runnable programs live in [`examples/`](examples/):

```bash
cargo run -- examples/fibonacci.nula       # closures + recursion
cargo run -- examples/effects.nula         # algebraic effect handlers
cargo run -- examples/counter_actor.nula   # actor declaration + spawn
cargo run -- examples/variant_option.nula  # user-declared variant types (Option)
```

---

## Language Tour

### Hello, World

`IO.print` is handled by the standalone VM's built-in effect (every snippet
below was verified with `cargo run`):

```nulang
perform IO.print("Hello, World!")
```

### Functions and Closures

From [`examples/fibonacci.nula`](examples/fibonacci.nula):

```nulang
let fib = fn(n) {
    if n <= 1 then n else fib(n - 1) + fib(n - 2)
} in fib(10)
```

### Actors

From [`examples/counter_actor.nula`](examples/counter_actor.nula):

```nulang
actor Counter {
    state count = 0
    behavior get() { self.count }
    behavior inc() { self.count + 1 }
}
spawn Counter { count = 0 }
```

### Effects

From [`examples/effects.nula`](examples/effects.nula):

```nulang
handle perform Math.getAnswer() {
    | Math.getAnswer() => 42
}
```

### Entities

An `entity` is a durable-first actor: it is persistent by default and its state defaults to `event_sourced`. This is the recommended surface for long-lived domain objects.

```nulang
entity BankAccount {
    state balance = 0             // event_sourced by default
    state local scratch = 0       // ephemeral, explicitly marked

    behavior deposit(amount) { self.balance = self.balance + amount }
    behavior withdraw(amount) { self.balance = self.balance - amount }
    behavior get_balance() { self.balance }
}

let account = spawn BankAccount {} as "savings:alice"
ask account deposit(100)
```

### AI Agents (Cloud SDK)

AI agents are ordinary actors that use effects from the Cloud SDK. In the current alpha this syntax is still language-level; the goal is to express the same idea with an `nlc.ai` library import instead of a dedicated `agent` keyword:

```nulang
agent Assistant = {
    model: "gpt-4o",
    system_prompt: "You are helpful.",
    memory: { max_turns: 10 }
}
let a = spawn Assistant {} in
ask a ask("What is an actor model?")
```

### Pattern Matching

```nulang
let s = "hello" in
match s {
    | "hello" => 1
    | _ => 0
}
```

Arms may carry a guard (`| pat if cond => body` — a failing guard falls
through to the next arm), and patterns nest recursively: `Some(Some(x))`,
tuple, and record sub-patterns each test the positions they name.

### Pipe Operator

```nulang
let inc = fn(x) { x + 1 } in
let dbl = fn(x) { x * 2 } in
1 |> inc |> dbl
```

---

## Architecture

```
                    +-------------------------+
                    |      Source Code        |
                    +-------------------------+
                              |
                              v
+----------+    +-------------------------+
|  Lexer   |--->|     Parser (AST)        |
+----------+    +-------------------------+
                              |
                              v
                    +-------------------------+
                    |  Type Checker (H-M)     |
                    |  Effect Checker         |
                    |  Capability Analyzer    |
                    +-------------------------+
                              |
                              v
                    +-------------------------+
                    |  HIR → MIR Lowering     |
                    +-------------------------+
                              |
             +----------------+----------------+
             |                |                 |
             v                v                 v
   +------------------+ +------------------+ +------------------+
   | Bytecode Backend | | Native/AOT       | | WASM Backend     |
   | (138 opcodes)    | | (Cranelift)      | | (wasm-encoder)   |
   +------------------+ +------------------+ +------------------+
             |                |                 |
             v                v                 v
   +------------------+ +------------------+ +------------------+
   | Register VM +    | | Native Binary    | | Wasmtime Runtime |
   | Cranelift JIT    | | (AOT compiled)   | | (WASM execution) |
   +------------------+ +------------------+ +------------------+
             |
             v
   +-------------------------+
   |   Actor Runtime         |
   | (Spawn/Send/Receive/    |
   |  Links/Monitors)        |
   +-------------------------+
             |
   +---------+-------------------------------+
   |         |                               |
   v         v                               v
+--------+ +--------+                  +-----------+
| Sched  | | ORCA   |                  |Distributed|
| (Work  | | GC     |                  | Runtime   |
| Steal) | |(Per-   |                  |(TCP,CRDT) |
+--------+ | Actor) |                  +-----------+
           +--------+
             |
   +---------+---------+
   |                   |
   v                   v
+----------+    +---------------+
|Supervisor|    | CRDT Manager  |
| (OTP)    |    | (8 CRDT types)|
+----------+    +---------------+
```

### Module Structure

| Module | Description | Lines |
|--------|-------------|-------|
| `lexer` | Hand-written state machine, indentation-based tokenization | ~1,320 |
| `parser` | Recursive descent with Pratt precedence climbing | ~4,540 |
| `ast` | Abstract syntax tree definitions (30+ expression types) | ~950 |
| `types` | Type system, capability lattice, effect rows, error types | ~1,150 |
| `typechecker` | Hindley-Milner Algorithm W with full inference | ~3,880 |
| `effect_checker` | Algebraic effect row checking + capability analysis | ~3,140 |
| `hir` / `hir_lower` | High-level IR and AST → HIR lowering | ~2,730 |
| `mir` / `mir_lower` | Mid-level IR and HIR → MIR lowering | ~3,660 |
| `mir_codegen` | MIR-to-bytecode compilation with register allocation | ~2,290 |
| `bytecode` | 138 opcodes, 32-bit fixed-width instructions | ~1,060 |
| `value_layout` | Canonical i64-tagged tag/mask constants (single source of truth) | ~300 |
| `vm` | Register-based virtual machine, effect handlers, JIT tiering hook | ~5,640 |
| `aot/mod` + `aot/codegen` | AOT native compiler: MIR → Cranelift object code | ~1,410 |
| `type_metadata` | Type metadata for typed JIT and AOT compilation | ~125 |
| `wasm_types` | WASM component model type definitions | ~100 |
| `wasm_component_runtime` | WASM component runtime (WIP) | ~95 |
| `mir_wasm` | MIR → WASM compiler via wasm-encoder (wasm-backend feature) | ~810 |
| `wasm_runtime` | Wasmtime host runtime for WASM modules | ~360 |
| `jit/mod` | JIT session manager, tiered execution, hot-counter tracking | ~610 |
| `jit/compiler` | Bytecode → Cranelift IR (50 opcodes) | ~1,080 |
| `jit/typed_compiler` | Type-directed JIT: direct CLIF when operand types are known | ~2,110 |
| `jit/simd_analyzer` / `jit/simd_compiler` | Vectorizable-loop detection + SIMD CLIF emission | ~3,190 |
| `jit/runtime` | NaN-tag-aware runtime helpers for JIT (31 extern C functions) | ~395 |
| `runtime/mod` | Runtime coordinator: actors, scheduling, GC, supervision, distribution | ~5,260 |
| `runtime/actor` | Actor struct, lifecycle, state management | ~520 |
| `runtime/scheduler` | Work-stealing queues + reduction-bounded cooperative scheduler | ~570 |
| `runtime/mailbox` | Unbounded lock-free MPSC via crossbeam SegQueue | ~450 |
| `runtime/timer` | Hierarchical timer wheel for send_after, exit_after, kill_after | ~505 |
| `runtime/registry` | Local actor name registry (register/whereis/registered) | ~210 |
| `runtime/process_groups` | Decentralized actor group membership (Erlang pg) | ~285 |
| `runtime/heap` | Per-actor bump allocator with ORCA object headers | ~1,690 |
| `runtime/gc` | ORCA reference counting (3-count protocol) | ~1,430 |
| `runtime/orca_cycle` | Intra-node cycle detector with weighted heuristic | ~1,680 |
| `runtime/supervisor` | Erlang/OTP-style supervision strategies | ~750 |
| `runtime/network` | TCP transport, NUL0 wire protocol | ~2,360 |
| `runtime/cluster` | Gossip-based cluster membership + failure detection | ~1,210 |
| `runtime/distributed` | Location-transparent actor addressing | ~2,060 |
| `runtime/crdt` | CRDT trait + GCounter, PNCounter, GSet, ORSet, AWORSet | ~1,365 |
| `runtime/crdt_reg` | LWWRegister, MVRegister, RGA sequence CRDT | ~875 |
| `runtime/crdt_manager` | CRDT factory, sync ops, inter-node merge | ~1,160 |
| `runtime/persistence` | Snapshot/journal stores (MemoryStore, JsonFileStore, LibsqlStore) | ~1,290 |
| `python/bridge` + `python/marshal` | PyO3 interpreter bridge + Value↔Python marshalling | ~1,290 |
| `ffi` | C-compatible FFI layer, native-library registry, embedder C API | ~1,440 |
| `ai` | LLM providers (OpenAI, Ollama), memory, pipelines, debates, supervisor teams | ~3,250 |
| `lsp` | tower-lsp language server (12 features incl. hover, inlay hints, completion) | ~2,000 |
| `package` | Nula package manager (manifest, lockfile, resolver, commands) | ~1,240 |
| `format` | Frozen artifact formats (`.nbc` bytecode, NUL0 wire protocol) + migration registry | ~590 |
| `docgen` | Documentation generator: .nula doc comments → docs/api.md | ~430 |
| `stdlib` | Standard-library inventory documenting built-in effects and functions | ~480 |
| `repl` | Interactive REPL with :type, :ast, :bytecode commands | ~800 |
| `main` | CLI entry point (run, repl, eval, check, lsp, backend selection) | ~940 |
| `integration_tests` / `stress_tests` / `runtime/tests` / `jit/tests` | End-to-end pipeline, chaos, runtime, and JIT test suites | ~13,600 |

**Total: ~93,900 lines of Rust across 104 source files with 1392 tests (1424 with `--features wasm-backend`).**

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

35+ Erlang/OTP primitives analyzed and the core set implemented. The primitives are
reachable from Nulang source as built-in effects — `perform Actor.link(t)`,
`Actor.unlink(t)`, `Actor.monitor(t)`, `Actor.demonitor(t)`, `Actor.trap_exit(flag)`,
`Actor.exit(reason)`, `Actor.register(name)`, `Actor.unregister(name)`,
`Actor.whereis(name)`, `Actor.set_priority(0|1|2)` — dispatched via
`ActorVmCallbacks::perform_builtin_effect` into `Runtime::perform_actor_builtin`
(nil no-op outside an actor). The legacy `monitor`/`demonitor`/`link`/`unlink`/`exit`/`yield`
**VM opcodes remain defined but unhandled** — superseded by the effect surface.

| Primitive | File | Description |
|-----------|------|-------------|
| `receive` | `parser.rs`, `vm.rs` | Selective receive: scans the mailbox in FIFO order for the first message matching any arm (`OpCode::ReceiveMatch` → `ActorVmCallbacks::try_receive_match`), binds payload values to arm params, non-matching messages stay queued; no-match falls back to pop-any (nil when empty). The timed form `receive { arms } after ms => body` (`OpCode::ReceiveWait` 0xA0) suspends the actor until a match arrives or the timeout fires. See `examples/receive.nula`. |
| `spawn link` / `spawn monitor` | `parser.rs` | Spawn-and-link/monitor in one step: parser desugars to `spawn` + `perform Actor.link`/`Actor.monitor` on the spawner |
| `monitor` | `runtime/mod.rs` | Watcher monitors target actor for exit (`Actor.monitor` builtin or Rust API) |
| `demonitor` | `runtime/mod.rs` | Remove a monitor (`Actor.demonitor` builtin or Rust API) |
| `link`/`unlink` | `runtime/mod.rs` | Bidirectional fault tolerance links (`Actor.link`/`Actor.unlink` builtins or Rust API) |
| `exit` | `runtime/mod.rs` | Typed actor exit with `ExitReason` enum (`Actor.exit` builtin or Rust API) |
| `trap_exit` | `runtime/actor.rs` | Convert exit signals to messages (`Actor.trap_exit` builtin) |
| `register`/`whereis` | `runtime/registry.rs` | Local actor name registry (`Actor.register`/`unregister`/`whereis` builtins) |
| `set_priority` | `runtime/actor.rs`, `runtime/scheduler.rs` | `Actor.set_priority(0|1|2)` → High/Normal/Low; scheduler drains High before Normal before Low (scheduling-only, mailbox stays FIFO) |
| `send_after` | `runtime/timer.rs` | Hierarchical timer wheel |
| `pg` process groups | `runtime/process_groups.rs` | Decentralized actor groups |
| `yield` | `runtime/actor.rs` | Cooperative scheduling yield via reduction quotas |

### v0.8 — Performance Improvements

Three high-ROI changes implemented in parallel:

| # | Proposal | Change | Impact |
|---|----------|--------|--------|
| 2.1 | Lock-free mailboxes | `crossbeam::SegQueue` | ABA-safe, cache-line optimized |
| 4.2 | Linear type moves | `Capability::LinearIso` + consumption tracking | Zero-cost `iso` sends |

### v0.9 — Cranelift JIT Backend

Tiered execution system with Cranelift 0.132:

| Component | Description |
|-----------|-------------|
| `JitSession` | Manages Cranelift JIT module, compiled function cache |
| `compiler.rs` | Bytecode → CLIF for 50 opcodes (arith, compare, control flow) |
| `runtime.rs` | 31 `extern "C"` NaN-tag-aware runtime helpers |
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

### v0.12 — Architectural Audit & Corrections

An external technical audit identified several architectural risks. The following corrections were applied:

**Reverted:** Dual-Region Heaps + Escape Analysis

| Audit Finding | Risk | Action |
|---------------|------|--------|
| Generational nursery + ORCA foreign-ref tracking | Cross-actor pointer rewriting during minor GC is a massive corruption vector | Reverted to pure ORCA per-actor heap |
| Escape analysis without generational GC | Vestigial — no runtime benefit without nursery | Removed from build |
| Bounded mailboxes (`ArrayQueue`) | Violates BEAM semantics — supervisor signals can be dropped | Switched to `crossbeam::SegQueue` (unbounded) |
| Centralized cycle detector (1,550 lines) | Distributed DFS over TCP misidentifies slow refs as dead cycles | Restricted to intra-node only |
| Deep Python integration (TAG_PYTHON in VM) | CPython global mutable state leaks into clean Rust runtime | Replaced with Native Actor pattern |

**Philosophy:** Optimize pure ORCA first. Layer generational GC only after ORCA is provably correct under chaotic conditions. Layer Python interop only via isolated native actors with marshal-only boundaries.

### v0.13 — Python Interop (Native Actor Pattern) + Stress Tests

**Python interop** is the critical path for AI adoption. After architectural audit, the design shifted from deep VM integration to the **Native Actor pattern** — Python runs only in dedicated OS threads with marshal-only data crossing.

```nulang
// Python interop via native actors (isolated, explicit marshal)
let result = perform Python.call("torch", ["Tensor"], [[1.0, 2.0, 3.0]])
perform IO.print(result)  // marshaled Float value: 6.0
```

| Component | Description |
|-----------|-------------|
| `python/bridge.rs` | PyO3 interpreter bridge with GIL management |
| `python/marshal.rs` | Bidirectional Nulang Value ↔ Python object conversion |
| Enforced isolation | No Python objects in Nulang VM — marshal at boundary |
| 8 Python opcodes | 0x94-0x9B (PyImport, PyCall, PyToNu, …) defined and dispatched by the VM |
| 21 tests | Registry, import, call, marshal round-trips |

**Stress test suite** (30 chaos tests in `src/stress_tests.rs`) deliberately breaks the runtime under load — actor-effect boundary, supervision, GC, persistence, CRDTs, and JIT fallback:

| Test | Scenario |
|------|----------|
| `stress_slow_worker_with_mailbox_flood` | Slow Worker + Mailbox Flood |
| `stress_actor_crash_during_scheduling` | Actor Crash During Scheduling |
| `stress_cascading_exit_under_load` | Cascading Exit Under Load |
| `stress_monitor_during_rapid_spawn_exit` | Monitor During Rapid Spawn/Exit |
| `stress_scheduler_with_mixed_workload` | Scheduler with Mixed Workload |
| `stress_mailbox_never_drops_system_messages` | Mailbox Never Drops System Messages |
| `stress_orphaned_actor_cleanup` | Orphaned Actor Cleanup |
| `stress_reduction_quota_fairness` | Reduction Quota Fairness |
| `stress_effect_resume_after_mailbox_pressure` | Effect Resume After Mailbox Pressure |
| `stress_supervisor_crash_during_recovery` | Supervisor Crash During Recovery |
| `stress_registry_high_churn` | Registry High Churn |
| `stress_process_groups_membership_churn` | Process Groups Membership Churn |
| `stress_timer_wheel_overload` | Timer Wheel Overload |
| `stress_persistent_actor_checkpoint_recovery` | Persistent Actor Checkpoint / Recovery |
| `stress_crdt_counter_merge_stress` | CRDT Counter Merge Stress |
| `stress_crdt_manager_sync_ops` | CRDT Manager Sync Ops |
| `stress_monitor_spawn_storm` | Monitor Spawn Storm |
| `stress_jit_hot_loop_then_cold_fallback` | JIT Hot Loop Matches Interpreter |
| `stress_remote_actor_cache_lru_eviction` | Remote Actor Cache LRU Eviction |
| `stress_supervisor_restart_intensity` | Supervisor Restart Intensity |
| `stress_gc_foreign_ref_churn` | GC Foreign Reference Churn |
| `stress_distribution_local_fallback_when_disabled` | Distribution Local Fallback When Disabled |
| `stress_reduction_yield_under_pressure` | Reduction Yield Under Pressure |
| `stress_actor_heap_allocation_pressure` | Actor Heap Allocation Pressure |
| `stress_cascading_supervisor_shutdown` | Cascading Supervisor Shutdown |
| `stress_persistence_journal_replay_ordering` | Persistence Journal Replay Ordering |
| `stress_cycle_detector_epoch_gating` | Cycle Detector Epoch Gating |
| `stress_mailbox_system_priority_preservation` | Mailbox System Priority Preservation |
| `stress_trap_exit_with_monitor_storm` | Trap Exit With Monitor Storm |
| `stress_gc_cycle_detector_under_foreign_ref_load` | GC Cycle Detector Under Foreign Reference Load |

**Design documents** (see [`docs/archive/`](docs/archive/)):
- `DESIGN_WORKFLOW_SDK.md` — workflow SDK design (partially implemented in v0.8)
- `DESIGN_PACKAGE_MANAGER.md` — package manager design (implemented as `nula` CLI)
- `DESIGN_WEB_FRAMEWORK.md`, `DESIGN_CLOUD.md` — design-only, not implemented

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
- [x] Compiler (AST → HIR → MIR → bytecode with register allocation)
- [x] Native AOT Compiler (MIR → Cranelift native object code)
- [x] WASM Compiler (MIR → WASM via wasm-encoder)
- [x] VM (register-based execution, arithmetic, comparisons, control flow)
- [x] REPL (parse-typecheck-compile-execute cycle with introspection)
- [x] Integration tests (264 end-to-end pipeline tests)
- [x] Actor runtime (spawn, send, links, monitors, selective receive)
- [x] Work-stealing scheduler (cooperative, reduction quotas)
- [x] ORCA garbage collector (per-actor heap, 3-count protocol, cycle detection)
- [x] Supervision trees (OneForOne, OneForAll, RestForOne restart strategies)
- [x] Fault tolerance tests (supervision, exit, link, monitor, trap_exit across runtime + integration suites)
- [x] Distributed runtime (TCP transport, cluster membership, location-transparent messaging)
- [x] CRDT subsystem (8 types: counters, sets, registers, sequences)
- [x] CRDT manager (factory, sync, inter-node merge)
- [x] BEAM/OTP primitives (`perform Actor.*`: monitor, link, exit, trap_exit, register, whereis, set_priority; send_after, pg, yield, selective receive with `after`; legacy VM opcodes for monitor/link/exit/yield are defined but unhandled)
- [x] `spawn link` / `spawn monitor` (parser desugar to spawn + `Actor.link`/`Actor.monitor`)
- [x] Unbounded mailboxes (crossbeam::SegQueue — BEAM semantics, no message loss)
- [x] Hierarchical timer wheel (send_after, exit_after, kill_after)
- [x] Actor registry (register/whereis/registered)
- [x] Process groups (decentralized actor group membership)
- [x] Linear type moves (compile-time `iso` consumption tracking)
- [x] Cranelift JIT backend (tiered execution, 50 opcodes, hot-counter threshold)
- [x] Type guard stripping (direct CLIF when types known, ~30% speedup in numeric loops)
- [x] LSP inlay hints (type/capability/effect annotations via tower-lsp)
- [x] SIMD vectorization (auto-vectorize array loops: I64x2, F64x2, I32x4, F32x4)
- [x] ~~Dual-region generational heap~~ **REVERTED** (audit: ORCA+nursery corruption risk)
- [x] ~~Escape analysis~~ **REVERTED** (audit: no benefit without nursery)
- [x] Python interop — Native Actor pattern (isolated OS threads, marshal-only boundary)
- [x] Stress test suite (30 chaos tests: supervision, scheduler fairness, GC, persistence, CRDTs, JIT fallback)
- [x] AI runtime (`agent` declarations, LLM providers (OpenAI, Ollama), memory, pipelines, debates, supervisor teams)
- [x] Durable workflow runtime (`workflow` declarations with steps, timers, signals, saga compensation)
- [x] Format stability layer (frozen `.nbc` bytecode artifacts, NUL0 wire protocol versioning, migration registry — RFC 0001/0002)
- [x] `entity` declarations (durable-first actors, event-sourced by default)
- [x] Web Framework design document (endpoints, controllers, channels, LiveView)
- [x] Package Manager (manifest, resolver, local registry, workspace, `nula` commands)
- [x] Cloud Platform design document (global deploy, auto-scaling, persistence)

### Roadmap

| Phase | Feature | Status |
|-------|---------|--------|
| v0.2 | Hindley-Milner type checker + effect checker | Completed |
| v0.3 | Actor scheduler + supervision trees | Completed |
| v0.4 | ORCA garbage collector | Completed |
| v0.5 | Multi-node distribution | Completed |
| v0.6 | CRDT integration | Completed |
| v0.7 | BEAM/OTP primitives (monitor, links, exit, trap_exit, registry, timers, process groups, selective receive with `after`, `spawn link`/`spawn monitor`, actor priority — `perform Actor.*` language surface) | Completed |
| v0.8 | Performance improvements (mimalloc, lock-free mailboxes, linear type moves) | Completed |
| v0.9 | Cranelift JIT backend | Completed |
| v0.10 | Type guard stripping + LSP inlay hints | Completed |
| v0.11 | SIMD vectorization (auto-vectorize array loops) | Completed |
| v0.13 | Python interop (Native Actor) + stress tests + AI ecosystem design foundation | Completed |
| v0.14 | Native AOT backend + WASM compilation and execution backends + Package Manager | Completed |
| — | Durable workflow runtime (steps, timers, signals, saga compensation) | Completed |
| — | AI runtime (`agent` keyword, LLM providers, memory, pipeline/debate/supervisor patterns) | Completed |
| — | Format stability (frozen `.nbc` artifacts, NUL0 wire protocol, language version `1.0.0-frozen` — RFC 0001/0002) | Completed |
| v1.0 | Production release — requires: chaos test suite passing ✅, scheduler profiled ✅, cycle detector intra-node only ✅ | Planned |

---

## Known Limitations

- **`receive` uses selective matching without payload patterns.** `receive { | Behavior(params) => expr }` scans the mailbox in FIFO order for the first message matching any arm (`src/mir_lower.rs` `lower_receive` → `OpCode::ReceiveMatch` → `ActorVmCallbacks::try_receive_match`), binds payload values to the arm's params (missing values bind to nil, extras ignored), and skips non-matching messages, which stay queued. In the plain form it never blocks: when nothing matches, a legacy fallback pops the next message and yields its first payload value (nil when the mailbox is empty or outside an actor context). The timed form `receive { arms } after ms => body` (`OpCode::ReceiveWait` 0xA0) does block — the actor suspends until a matching message arrives or the timeout fires, then runs the after body. Payload matching is by behavior name and arity-free — no guard expressions or payload patterns yet.
- Actor messaging that *is* fully wired goes through named behavior dispatch (`send actor behavior(args)`) — see `examples/counter_actor.nula`.

---

## Documentation

- **[nulang.org](https://nulang.org)** — Online documentation site (getting started, language reference, actor guide, stdlib, AI agents)
- **[SPEC2.md](SPEC2.md)** — Language specification covering syntax, semantics, type system, runtime, standard library, and format stability contract
- **[CHANGELOG.md](CHANGELOG.md)** — Changelog organized by stability tier (Frozen / Stable / Experimental), tracking the language version
- **[GOVERNANCE.md](GOVERNANCE.md)** — Stability tiers and RFC process
- **[RFC/](RFC/)** — Nulang RFC proposals (format stability, frozen core, deprecation cycles, roadmap items)
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — Implementation architecture notes
- **[docs/](docs/)** — Astro/Starlight documentation website source (deploys to nulang.org)
- **[docs/archive/](docs/archive/)** — Historical specs, roadmaps, design documents, and review reports

---

## License

Nulang is open source and licensed under the Apache License, Version 2.0. See the [LICENSE](https://github.com/dporkka/nulang/blob/main/LICENSE) file for the full license text.

Copyright 2026 © David Porkka

---

## Acknowledgments

- **Erlang/OTP** for the actor model and fault-tolerance philosophy
- **Pony** for the capability system and ORCA GC design
- **Koka** and **Eff** for algebraic effects
- **Rust** for ownership-based memory safety inspiration
- **Shapiro et al.** for CRDT theory and the state-based replication model

---

> *"Concurrency should be a language primitive, not a library afterthought."*
