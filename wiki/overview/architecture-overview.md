---
updated: 2026-08-02
sources:
  - AGENTS.md
  - SPEC2.md
  - src/main.rs
  - src/lib.rs
tags: [overview, architecture]
---

# Architecture Overview

Nulang is a distributed, actor-based programming language written in Rust (edition 2021, single crate `nulang`). It fuses Erlang-style fault-tolerant actors with a Rust/Pony-inspired type system, algebraic effects, a register-based bytecode VM, a Cranelift JIT, WASM/AOT backends, and a v0.9 AI runtime.

## Subsystem map

| Subsystem | Path | One-liner |
|-----------|------|-----------|
| Lexer/Parser | `src/lexer.rs`, `src/parser.rs` | Source → tokens → AST. |
| Type system | `src/typechecker.rs`, `src/types.rs` | HM Algorithm W + row-polymorphic records + effects. |
| Capabilities | `src/effect_checker.rs` | Pony-inspired lattice (iso/trn/ref/val/box/tag/lineariso). |
| HIR/MIR | `src/hir_lower.rs`, `src/mir_lower.rs`, `src/mir_codegen.rs` | AST → HIR → MIR → bytecode (MIR-exclusive pipeline). |
| Bytecode VM | `src/vm.rs`, `src/bytecode.rs`, `src/value_layout.rs` | 256-register frames, i64-tagged values, 135 opcodes. |
| JIT | `src/jit/` | Cranelift-backed hot-region compilation, typed + SIMD tiers. |
| Actor runtime | `src/runtime/` | Work-stealing scheduler, ORCA GC, supervision, mailboxes. |
| Distribution | `src/runtime/network.rs`, `cluster.rs`, `distributed.rs` | NUL0 TCP wire protocol, gossip membership, remote spawn. |
| CRDTs | `src/runtime/crdt.rs`, `crdt_reg.rs`, `crdt_manager.rs` | 8 CRDT types with delta-state replication. |
| WASM backend | `src/mir_wasm.rs`, `src/wasm_runtime.rs` | MIR → WASM via `wasm-encoder`; Wasmtime host runtime. |
| AOT backend | `src/aot/` | MIR → Cranelift CLIF → native object code. |
| AI runtime | `src/ai/` | LLM providers, memory, pipelines, debates, supervisors. |
| LSP | `src/lsp/` | 12-feature `tower-lsp` server. |
| Python interop | `src/python/` | PyO3 abi3 bridge. |
| C FFI | `src/ffi/` | Stable C embedder API. |
| Package manager | `src/package/` | `nula` — manifest, lockfile, resolver, commands. |
| Format layer | `src/format/` | Frozen `.nbc` bytecode, NUL0 wire versioning, migration registry. |

## Two-backend model

The frontend (lexer → parser → typechecker → effect/capability checker → HIR → MIR) is shared. From MIR, three backends fan out:

1. **Bytecode VM** (default): MIR → bytecode → register VM (with Cranelift JIT tiering).
2. **AOT native** (`--backend native`): MIR → Cranelift CLIF → native object code (unboxed via compile-time type metadata).
3. **WASM** (`--backend wasm|wasm-run|wasm-aot`, requires `wasm-backend` feature): MIR → `.wasm` via `wasm-encoder`, executed by Wasmtime with guard pages, inlining, SIMD, and optional AOT compilation to `.cwasm`.

## Concurrency model

There is **no async/await in the VM or runtime.** Actor concurrency is cooperative reduction-yielding, built on `crossbeam` deques/queues + `std::sync` atomics/RwLock/mpsc + raw `unsafe` pointers for ORCA GC. The runtime is a multi-threaded work-stealing executor: `Runtime` is a shard (actor subset by `actor_id % shard_count`), each shard runs on one worker thread, and a Chase-Lev work-stealing scheduler distributes work across `worker_count` threads per shard. Cross-shard messaging uses `mpsc::SyncSender` channels; only value-type payloads cross shard boundaries.

The only async surfaces are `main.rs` (`#[tokio::main]`), the LSP server (`tower-lsp` over tokio stdin/stdout), and the AI LLM client (`async_trait`, exposed to sync callers via `complete_sync`).

## Actor lifecycle (short version)

Spawn → schedule (Chase-Lev deque, priority queues High/Normal/Low) → step (mailbox dequeue → handler dispatch → reduction budget → yield or continue) → GC (ORCA delta ops + incremental cycle detection) → fault (link/monitor propagation, supervisor restart strategies).

For the full protocol see [[../subsystems/actor-runtime]] _(to be created on next ingest of `src/runtime/`)_.

## What to read next

- Compiler stages: [[compiler-pipeline]].
- Language semantics: `SPEC2.md`.
- Architecture contract: `AGENTS.md` (authoritative, denser than this page).
- Stability tiers and RFC process: `GOVERNANCE.md`.

## Source citations

- Subsystem inventory: `AGENTS.md` (Key Directories section).
- Compiler pipeline: `AGENTS.md` (Architecture & Data Flow section).
- Concurrency model: `AGENTS.md` (project overview + runtime lifecycle sections).
- Backend selection: `src/main.rs` (`--backend` flag handling), `src/mir_codegen.rs`, `src/mir_wasm.rs`, `src/aot/codegen.rs`.
