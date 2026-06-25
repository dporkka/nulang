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

# Start LSP server
cargo run -- --lsp
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
actor Counter(start: Int) =
  behavior Count(n: Int) =>
    receive
      | Increment => become Count(n + 1)
      | GetValue(reply) =>
          perform reply(n)
          become Count(n)
    end
  become Count(start)
```

### AI Agents

```nulang
agent WeatherBot =
  model "gpt-4"
  tools [WeatherAPI.check, Calendar.schedule]
  memory episodic
  policy SafeToolUse
```

### Algebraic Effects

```nulang
effect FileSystem
  fun read(path: String) ! FileSystem: String
  fun write(path: String, content: String) ! FileSystem: Unit

fun readConfig() ! FileSystem: Config =
  let raw = perform FileSystem.read("app.conf")
  parseConfig(raw)
```

### Capabilities

```nulang
fun shareData(data: iso String, target: Address): Unit =
  -- `iso` guarantees unique reference; compiler tracks linear consumption
  perform send(target, DataMessage(consume data))
```

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Source Code (.nl)                         │
├─────────────────────────────────────────────────────────────────┤
│  Lexer → Parser → AST                                          │
├─────────────────────────────────────────────────────────────────┤
│  Type Checker (Hindley-Milner) → Effect Checker                │
│  Capability Analyzer → Linear Move Validator                    │
├─────────────────────────────────────────────────────────────────┤
│  Bytecode Compiler (register-based, 84 opcodes)                 │
├─────────────────────────────────────────────────────────────────┤
│  Tiered Execution:                                               │
│    ┌──────────────┐    ┌──────────────┐    ┌─────────────────┐ │
│    │  Interpreter │───→│ Type-special │───→│ SIMD Vectorized │ │
│    │   (cold)     │    │    JIT       │    │      JIT        │ │
│    └──────────────┘    │   (warm)     │    │    (hot)        │ │
│                        └──────────────┘    └─────────────────┘ │
├─────────────────────────────────────────────────────────────────┤
│  VM: Register file (256×u64), NaN-tagged values, actor heap     │
├─────────────────────────────────────────────────────────────────┤
│  Runtime: Actor scheduler (work-stealing), ORCA GC, mailbox     │
│  BEAM/OTP primitives, CRDT manager, distributed transport       │
└─────────────────────────────────────────────────────────────────┘
```

---

## Implementation Status

### Modules (34 source files, ~35,000 lines, 490+ tests)

| Module | File | Description | Tests |
|--------|------|-------------|-------|
| Lexer | `src/lexer.rs` | Tokenization, indentation handling | 42 |
| Parser | `src/parser.rs` | Recursive descent, all expression types | 38 |
| AST | `src/ast.rs` | Complete AST node types | - |
| Type Checker | `src/typechecker.rs` | Hindley-Milner Algorithm W | 45 |
| Effect Checker | `src/effect_checker.rs` | Effect row compatibility | 28 |
| Capabilities | `src/capabilities.rs` | Reference capability analysis | 15 |
| Bytecode | `src/bytecode.rs` | 84 opcodes, instruction encoding | 12 |
| Compiler | `src/compiler.rs` | AST-to-bytecode with reg alloc | 24 |
| VM | `src/vm.rs` | Register-based execution engine | 36 |
| REPL | `src/repl.rs` | Interactive loop | 8 |
| JIT Core | `src/jit/compiler.rs` | Bytecode → CLIF (30 opcodes) | 17 |
| JIT Typed | `src/jit/typed_compiler.rs` | Type guard stripping | 8 |
| JIT SIMD Analyzer | `src/jit/simd_analyzer.rs` | Vectorization pattern detection | 14 |
| JIT SIMD Compiler | `src/jit/simd_compiler.rs` | SIMD CLIF emission | 10 |
| JIT Runtime | `src/jit/runtime.rs` | 30 NaN-tag runtime helpers | - |
| Scheduler | `src/scheduler.rs` | Work-stealing M:N threading | 22 |
| Actor | `src/actor.rs` | Spawn, send, receive, behavior | 31 |
| Mailbox | `src/mailbox.rs` | Lock-free message queues | 18 |
| Supervisor | `src/supervisor.rs` | Supervision trees | 18 |
| GC | `src/gc.rs` | ORCA per-actor collector | 24 |
| GC Cycle | `src/gc_cycle.rs` | Cycle detection | 12 |
| Distributed | `src/distributed.rs` | TCP transport, clustering | 16 |
| CRDT | `src/crdt.rs` | 8 CRDT types | 20 |
| CRDT Manager | `src/crdt_manager.rs` | Factory, sync, merge | 14 |
| BEAM Primitives | `src/beam_primitives.rs` | 14 core OTP functions | 8 |
| Process Groups | `src/process_groups.rs` | Decentralized group membership | 6 |
| Registry | `src/registry.rs` | Actor register/whereis | 10 |
| Timer | `src/timer.rs` | Hierarchical timer wheel | 8 |
| Exit Reason | `src/exit_reason.rs` | Exit signal types | - |
| LSP Server | `src/lsp/mod.rs` | Language server with inlay hints | 9 |
| Runtime | `src/runtime.rs` | Standard library runtime | 15 |
| Integration | `src/integration_tests.rs` | End-to-end pipeline tests | 16 |
| Types | `src/types.rs` | Type definitions, error types | 6 |

### Version History

### v0.7 — BEAM/OTP Primitives

| Primitive | Description |
|-----------|-------------|
| `receive`/`after` | Selective message receive with timeout |
| `spawn_link` | Spawn actor with bidirectional link |
| `monitor`/`demonitor` | One-way actor monitoring |
| `link`/`unlink` | Dynamic link management |
| `exit` | Signal-based actor termination |
| `trap_exit` | Convert exit signals to messages |
| `register`/`whereis` | Named actor registry |
| `send_after` | Delayed message delivery |
| `pg` | Process groups (decentralized) |
| `yield` | Cooperative scheduling |

### v0.8 — Performance Improvements

| Feature | Implementation | Impact |
|---------|---------------|--------|
| mimalloc | Global allocator replacement | ~15% faster allocation, reduced fragmentation |
| Lock-free mailboxes | `crossbeam::queue::ArrayQueue` | Eliminated mutex contention under high concurrency |
| LinearIso capability | `Capability::LinearIso` + `TypeContext::consume()` | Zero-copy actor message passing |

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
- [x] Bytecode (84 opcodes, constant pool, behavior table)
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
| v0.12 | Dual-region heaps + escape analysis | Planned |
| v1.0 | Production release | Planned |

---

## Documentation

- **[SPEC.md](SPEC.md)** — Complete 60,000-word language specification covering syntax, semantics, type system, runtime, and standard library

---

## License

MIT License - see [LICENSE](LICENSE) for details.
