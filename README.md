# Nulang

> The language for autonomous distributed software systems — AI agents, durable workflows, and resilient services where distribution, durability, and capability security are the default model, built in Rust.

[![Rust](https://img.shields.io/badge/rust-2021%20Edition-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/github/license/dporkka/nulang.svg)](LICENSE)

---

## Overview

**Nulang** is the language for autonomous distributed software systems: AI agents, durable workflows, and resilient services where distribution, durability, capability security, and AI-assisted execution are the default model rather than bolted-on abstractions. It is not a general-purpose language competing with Rust or Go — it is purpose-built for software that must keep running across nodes, failures, and restarts without human intervention.

It combines the fault-tolerant actor model of Erlang with a Rust/Pony-inspired type system (Hindley-Milner inference, reference capabilities, row-polymorphic algebraic effects), a register-based VM with a Cranelift JIT, per-actor ORCA garbage collection, location-transparent distribution over TCP, CRDTs for shared state, durable workflows with saga compensation, and a first-class AI agent runtime — all in a single Rust implementation.

### Key Features

| Feature | Description |
|---------|-------------|
| **Actor Model** | Lightweight actors with cooperative reduction-bounded scheduling, work-stealing queues, and supervision trees |
| **Algebraic Effects** | First-class effect system with `perform`/`handle`/`resume` semantics |
| **Capability System** | Fine-grained reference permissions (iso/trn/ref/val/box/tag) for memory safety |
| **AI Agents** | First-class `agent` declarations with LLM clients (OpenAI, Ollama), episodic/semantic/procedural memory, pipelines, debates, and supervisor teams |
| **Distributed Runtime** | Location-transparent actor messaging across nodes with TCP transport |
| **ORCA GC** | Per-actor concurrent garbage collection with cycle detection |
| **CRDTs** | 8 conflict-free replicated data types for shared distributed state |
| **Register-Based VM** | High-performance bytecode VM with NaN-tagged value representation |
| **Cranelift JIT Backend** | Tiered execution: interpreter for cold code, native compilation for hot loops |
| **BEAM/OTP Primitives** | `spawn_link`, `monitor`, `link`, registry, timers, process groups, selective `receive` |
| **Linear Type Moves** | Zero-cost `iso` actor messaging via compile-time linearity tracking |
| **SIMD Vectorization** | Auto-vectorization of array loops via Cranelift SIMD (I64x2, F64x2, I32x4, F32x4) |
| **Python Interop** | Native Actor pattern: Python isolated to dedicated OS threads, marshal-only boundary |
| **Unbounded Mailboxes** | Lock-free MPSC queues (crossbeam::SegQueue) — BEAM-semantics, no message loss |
| **Stress Test Suite** | 30 chaos tests for supervision, scheduler fairness, GC, persistence, CRDTs, and JIT fallback under load |

### Current Status

Nulang is **Alpha** — but not a greenfield project. The compiler pipeline, VM and JIT, actor runtime, supervision, effects, capabilities, distribution, durability, and AI runtime all exist and are tested today:

- ✅ Builds with `cargo build`
- ✅ All 921 tests pass with `cargo test`
- ✅ NaN-boxed `Value` representation with distinct high-16 type tags (canonical constants in `src/value_layout.rs`)
- ✅ 141-opcode bytecode ISA (arithmetic, control flow, closures, objects, effects, actors, capabilities, FFI, Python, distribution)
- ✅ Hindley-Milner type inference with algebraic effects
- ✅ Actor runtime: spawn, send, monitors, links, supervision, timers, registry, process groups, selective `receive`
- ✅ ORCA-style per-actor GC with cycle detection
- ✅ AI runtime: `agent` declarations, LLM providers (OpenAI, Ollama), memory, pipelines, debates, supervisor teams
- ✅ Durable workflow runtime: `workflow` declarations with steps, timers, signals, saga compensation

---

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (stable channel, 1.95+)
- Python 3 development headers, for the default build (see [Feature flags](#feature-flags) to skip this)
- Linux or macOS (Windows support planned)

### Building

```bash
git clone https://github.com/dporkka/nulang.git
cd nulang
cargo build --release
```

### Feature flags

Three optional subsystems are on by default so a plain `cargo build` behaves
as before this flag set existed. Build without them for a leaner binary and
fewer system dependencies:

| Feature | Enables | Off by default? |
|---------|---------|------------------|
| `python` | Python interop (`perform Python.call(...)`) via PyO3 | No — needs Python 3 dev headers |
| `sqlite` | `SqliteStore` actor persistence backend | No — `MemoryStore`/`JsonFileStore` always available |
| `lsp` | Language Server Protocol server (`src/lsp/`) | No |

```bash
# Skip PyO3, SQLite, and the LSP server entirely:
cargo build --release --no-default-features

# Pick just what you need:
cargo build --release --no-default-features --features sqlite
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
cargo run -- myprogram.nula

# Type-check only
cargo run -- --check myprogram.nula

# Evaluate a string
cargo run -- --eval 'perform IO.print("Hello")'
```

### Examples

Runnable programs live in [`examples/`](examples/):

```bash
cargo run -- examples/fibonacci.nu       # closures + recursion
cargo run -- examples/effects.nu         # algebraic effect handlers
cargo run -- examples/counter_actor.nu   # actor declaration + spawn
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
                    |  Effect Checker         |  | (141 opcodes)     |
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
          |  Receive/Links)  |                    |  Coop Threads)   |        |  Ref Counting,   |
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
| `lexer` | Hand-written state machine, indentation-based tokenization | ~1,020 |
| `parser` | Recursive descent with Pratt precedence climbing | ~2,760 |
| `ast` | Abstract syntax tree definitions (30+ expression types) | ~600 |
| `types` | Type system, capability lattice, effect rows, error types | ~700 |
| `typechecker` | Hindley-Milner Algorithm W with full inference | ~3,125 |
| `effect_checker` | Algebraic effect row checking + capability analysis | ~1,825 |
| `hir` / `hir_lower` | High-level IR and AST → HIR lowering | ~2,030 |
| `mir` / `mir_lower` | Mid-level IR and HIR → MIR lowering | ~1,715 |
| `mir_codegen` | MIR-to-bytecode compilation with register allocation | ~1,230 |
| `bytecode` | 141 opcodes, 32-bit fixed-width instructions | ~880 |
| `value_layout` | Canonical NaN-boxing tag/mask constants (single source of truth) | ~140 |
| `vm` | Register-based virtual machine, effect handlers, JIT tiering hook | ~3,100 |
| `jit/mod` | JIT session manager, tiered execution, hot-counter tracking | ~375 |
| `jit/compiler` | Bytecode → Cranelift IR (41 opcodes) | ~400 |
| `jit/typed_compiler` | Type-directed JIT: direct CLIF when operand types are known | ~1,390 |
| `jit/simd_analyzer` / `jit/simd_compiler` | Vectorizable-loop detection + SIMD CLIF emission | ~2,960 |
| `jit/runtime` | NaN-tag-aware runtime helpers for JIT (31 extern C functions) | ~140 |
| `runtime/mod` | Runtime coordinator: actors, scheduling, GC, supervision, distribution | ~3,400 |
| `runtime/actor` | Actor struct, lifecycle, state management | ~300 |
| `runtime/scheduler` | Work-stealing queues + reduction-bounded cooperative scheduler | ~500 |
| `runtime/mailbox` | Unbounded lock-free MPSC via crossbeam SegQueue | ~270 |
| `runtime/timer` | Hierarchical timer wheel for send_after, exit_after, kill_after | ~345 |
| `runtime/registry` | Local actor name registry (register/whereis/registered) | ~215 |
| `runtime/process_groups` | Decentralized actor group membership (Erlang pg) | ~280 |
| `runtime/heap` | Per-actor bump allocator with ORCA object headers | ~990 |
| `runtime/gc` | ORCA reference counting (3-count protocol) | ~1,300 |
| `runtime/orca_cycle` | Intra-node cycle detector with weighted heuristic | ~1,660 |
| `runtime/supervisor` | Erlang/OTP-style supervision strategies | ~440 |
| `runtime/network` | TCP transport, NUL0 wire protocol | ~1,410 |
| `runtime/cluster` | Gossip-based cluster membership + failure detection | ~1,080 |
| `runtime/distributed` | Location-transparent actor addressing | ~1,140 |
| `runtime/crdt` | CRDT trait + GCounter, PNCounter, GSet, ORSet, AWORSet | ~860 |
| `runtime/crdt_reg` | LWWRegister, MVRegister, RGA sequence CRDT | ~565 |
| `runtime/crdt_manager` | CRDT factory, sync ops, inter-node merge | ~600 |
| `runtime/persistence` | Snapshot/journal stores (MemoryStore, JsonFileStore, SqliteStore) | ~785 |
| `python/bridge` + `python/marshal` | PyO3 interpreter bridge + Value↔Python marshalling | ~1,290 |
| `ffi` | C-compatible FFI layer, native-library registry, embedder C API | ~1,380 |
| `ai` | LLM providers (OpenAI, Ollama), memory, pipelines, debates, supervisor teams | ~2,590 |
| `lsp` | tower-lsp language server (12 features incl. hover, inlay hints, completion) | ~1,160 |
| `repl` | Interactive REPL with :type, :ast, :bytecode commands | ~570 |
| `main` | CLI entry point (run, repl, eval, check, lsp modes) | ~425 |
| `integration_tests` / `stress_tests` / `runtime/tests` | End-to-end pipeline, chaos, and runtime test suites | ~6,170 |

**Total: ~54,400 lines of Rust across 69 source files with 921 tests.**

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

35+ Erlang/OTP primitives analyzed and 12 core primitives implemented:

| Primitive | File | Description |
|-----------|------|-------------|
| `receive` | `parser.rs`, `vm.rs` | Selective receive: scans the mailbox in FIFO order for the first message matching any arm (`OpCode::ReceiveMatch` → `ActorVmCallbacks::try_receive_match`), binds payload values to arm params, non-matching messages stay queued; no-match falls back to pop-any (nil when empty). See `examples/receive.nu`. |
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
| 2.1 | Lock-free mailboxes | `crossbeam::ArrayQueue` | ABA-safe, cache-line optimized (later replaced by `SegQueue` in v0.12 — see below) |
| 4.2 | Linear type moves | `Capability::LinearIso` + consumption tracking | Zero-cost `iso` sends |

### v0.9 — Cranelift JIT Backend

Tiered execution system with Cranelift 0.132:

| Component | Description |
|-----------|-------------|
| `JitSession` | Manages Cranelift JIT module, compiled function cache |
| `compiler.rs` | Bytecode → CLIF for 41 opcodes (arith, compare, control flow) |
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
-- Python interop via native actors (isolated, explicit marshal)
let result = perform Python.call("torch", ["Tensor"], [[1.0, 2.0, 3.0]])
perform IO.print(result)  -- marshaled Float value: 6.0
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
- `DESIGN_AI_SDK.md` — AI SDK design (partially implemented in v0.9)
- `DESIGN_WORKFLOW_SDK.md` — workflow SDK design (partially implemented in v0.8)
- `DESIGN_WEB_FRAMEWORK.md`, `DESIGN_PACKAGE_MANAGER.md`, `DESIGN_CLOUD.md` — design-only, not implemented

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
- [x] Bytecode (141 opcodes, constant pool, behavior table)
- [x] Compiler (AST → HIR → MIR → bytecode with register allocation)
- [x] VM (register-based execution, arithmetic, comparisons, control flow)
- [x] REPL (parse-typecheck-compile-execute cycle with introspection)
- [x] Integration tests (122 end-to-end pipeline tests)
- [x] Actor runtime (spawn, send, links, monitors, selective receive)
- [x] Work-stealing scheduler (cooperative, reduction quotas)
- [x] ORCA garbage collector (per-actor heap, 3-count protocol, cycle detection)
- [x] Supervision trees (OneForOne, OneForAll, RestForOne restart strategies)
- [x] Fault tolerance tests (27 supervisor/exit/link/monitor tests)
- [x] Distributed runtime (TCP transport, cluster membership, location-transparent messaging)
- [x] CRDT subsystem (8 types: counters, sets, registers, sequences)
- [x] CRDT manager (factory, sync, inter-node merge)
- [x] BEAM/OTP primitives (spawn_link, monitor, link, exit, trap_exit, register, whereis, send_after, pg, yield, selective receive)
- [x] Unbounded mailboxes (crossbeam::SegQueue — BEAM semantics, no message loss)
- [x] Hierarchical timer wheel (send_after, exit_after, kill_after)
- [x] Actor registry (register/whereis/registered)
- [x] Process groups (decentralized actor group membership)
- [x] Linear type moves (compile-time `iso` consumption tracking)
- [x] Cranelift JIT backend (tiered execution, 41 opcodes, hot-counter threshold)
- [x] Type guard stripping (direct CLIF when types known, ~30% speedup in numeric loops)
- [x] LSP inlay hints (type/capability/effect annotations via tower-lsp)
- [x] SIMD vectorization (auto-vectorize array loops: I64x2, F64x2, I32x4, F32x4)
- [x] ~~Dual-region generational heap~~ **REVERTED** (audit: ORCA+nursery corruption risk)
- [x] ~~Escape analysis~~ **REVERTED** (audit: no benefit without nursery)
- [x] Python interop — Native Actor pattern (isolated OS threads, marshal-only boundary)
- [x] Stress test suite (30 chaos tests: supervision, scheduler fairness, GC, persistence, CRDTs, JIT fallback)
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
| v0.7 | BEAM/OTP primitives (spawn_link, monitor, links, registry, timers, process groups, selective receive) | Completed |
| v0.8 | Performance improvements (mimalloc, lock-free mailboxes, linear type moves) | Completed |
| v0.9 | Cranelift JIT backend | Completed |
| v0.10 | Type guard stripping + LSP inlay hints | Completed |
| v0.11 | SIMD vectorization (auto-vectorize array loops) | Completed |
| v0.12 | Architectural audit & corrections (reverted nursery, bounded mailbox, deep Python) | Completed |
| v0.13 | Python interop (Native Actor) + stress tests + AI ecosystem design foundation | Completed |
| — | Durable workflow runtime (steps, timers, signals, saga compensation) | Completed |
| — | AI runtime (`agent` keyword, LLM providers, memory, pipeline/debate/supervisor patterns) | Completed |
| v1.0 | Production release — requires: chaos test suite passing ✅, scheduler profiled ✅, cycle detector intra-node only ✅ | Planned |

---

## Known Limitations

- **`receive` uses non-blocking selective matching.** `receive { | Behavior(params) => expr }` scans the mailbox in FIFO order for the first message matching any arm (`src/mir_lower.rs` `lower_receive` → `OpCode::ReceiveMatch` → `ActorVmCallbacks::try_receive_match`), binds payload values to the arm's params (missing values bind to nil, extras ignored), and skips non-matching messages, which stay queued. Unlike Erlang's `receive`, it never blocks: when nothing matches, a legacy fallback pops the next message and yields its first payload value (nil when the mailbox is empty or outside an actor context). Payload matching is by behavior name and arity-free — no guard expressions or payload patterns yet.
- Actor messaging that *is* fully wired goes through named behavior dispatch (`send actor behavior(args)`) — see `examples/counter_actor.nu`.

---

## Documentation

- **[SPEC2.md](SPEC2.md)** — Language specification covering syntax, semantics, type system, runtime, and standard library
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — Implementation architecture notes
- **[docs/archive/](docs/archive/)** — Historical specs, roadmaps, design documents, and review reports

---

## License

Copyright 2026 © David Porkka

This project is licensed under the Apache License, Version 2.0 - see the [LICENSE](LICENSE) file for details.

---

## Acknowledgments

- **Erlang/OTP** for the actor model and fault-tolerance philosophy
- **Pony** for the capability system and ORCA GC design
- **Koka** and **Eff** for algebraic effects
- **Rust** for ownership-based memory safety inspiration
- **Shapiro et al.** for CRDT theory and the state-based replication model

---

> *"Concurrency should be a language primitive, not a library afterthought."*
