# Repository Guidelines

> Practical orientation for AI assistants working in the Nulang codebase.
> All paths are relative to the repo root (`~/nulang`).

## Project Overview

**Nulang** is a distributed, actor-based programming language written in Rust (edition 2021, single crate `nulang`). It fuses Erlang-style fault-tolerant actors with a Rust/Pony-inspired type system (Hindley-Milner inference + reference capabilities + row-polymorphic algebraic effects), a register-based bytecode VM, a Cranelift JIT, BEAM/OTP primitives, CRDTs, location-transparent distribution, SQLite/JSON persistence, PyO3 Python interop, a C-compatible FFI layer, and a v0.9 AI runtime (`src/ai/` — LLM providers, memory, pipelines, debates, supervisor teams). Status: Alpha; 924 tests pass (`cargo test`). License: Apache-2.0.

## Architecture & Data Flow

The compiler pipeline is a straight line, wired in `src/main.rs` (`run_frontend` at `src/main.rs:183`, `run_source` at `src/main.rs:324`) and reused by the REPL (`src/repl.rs`) and LSP:

```
source &str
  -> Lexer::lex()                         -> Vec<Token>            src/lexer.rs
  -> Parser::parse_module()               -> AstModule             src/parser.rs
  -> TypeChecker::check_module()          -> Type                  src/typechecker.rs (HM Algorithm W)
  -> EffectChecker::infer_effects()       -> EffectRow             src/effect_checker.rs (per Function decl)
  -> CapabilityAnalyzer::infer_cap()      -> Capability            src/effect_checker.rs
  -> HIR lowering (hir_lower::lower_module) -> HIR Module          src/hir_lower.rs
  -> MIR lowering (mir_lower::lower_module) -> MIR Module          src/mir_lower.rs
  -> MIR codegen (mir_codegen::compile_mir)  -> CodeModule         src/mir_codegen.rs
  -> VM::load_module() + VM::run()        -> Value                 src/vm.rs (register VM + JIT tiering)
```

`--check` stops after capability analysis (no compile/run). The runtime (`src/runtime/`) is a **single-threaded synchronous coordinator** (`Runtime`) driving actors via reduction-bounded `step_actor`; it reaches the VM only through two object-safe callback traits (`ActorVmCallbacks`, `DistributedVmCallbacks`) to keep the dependency cycle-free. There is **no async/await in the runtime or VM** — concurrency is `crossbeam` deques/queues + `std::sync` atomics/RwLock + raw `unsafe` pointers for ORCA GC. The only async surfaces are `main.rs` (`#[tokio::main]`), the LSP server (`tower-lsp` over tokio stdin/stdout), and the `src/ai/` LLM client (`async_trait`, exposed to sync callers via `complete_sync`).

### Backend representation
- **Value** (`src/vm.rs`; tag constants canonical in `src/value_layout.rs`): NaN-boxed `u64` (`raw`), 48-bit payload, 16-bit type tag in the quiet-NaN bits. Tags: `TAG_NIL 0x7FF8`, `TAG_UNIT 0x7FF9`, `TAG_BOOL 0x7FFA`, `TAG_INT 0x7FFB`, `TAG_PTR 0x7FFC`, `TAG_ACTOR 0x7FFD`, `TAG_STRING 0x7FFE`, `TAG_CLOSURE 0x7FF7`.
- **Instruction** (`src/bytecode.rs`): 32-bit fixed-width `{opcode:u8, op1:u8, op2:u8, op3:u8}`; helpers `new0/1/2/3`, `imm16()`, `simm16()`, `offset16()`. 141 opcodes across 18 category ranges (Special, Stack & Locals, Int/Float Arithmetic, Comparison & Logic, Control Flow, Closures, Memory & Objects, Actor & Concurrency, Effects, Python Interop, Capabilities, FFI, Supervisor/Debate for the AI runtime, Distribution, String & IO, Debug & Meta).
- **Frames**: 256 registers each; flat `Vec<Frame>` with `caller_idx` links; closures carry `closure_env`.

### Effects & capabilities at runtime
Algebraic effects are runtime-resolved via `handler_stack: Vec<HandlerFrame>`: `Handle` pushes, `Perform` deep-clones a `Continuation` into the matching handler and jumps to its offset, `Resume` restores the continuation (overwrites frames/pc), `Unwind` pops. The static `EffectRow` is checked pre-compile; runtime only sees the four opcodes. Capabilities (`iso/trn/ref/val/box/tag/lineariso`) are **compile-time only** and erased at runtime — VM `CapChk`/`CapUp`/`CapDown`/`CapSend` are MVP no-ops (`CapChk` writes `true`, others copy).

### JIT tiering
`VM` holds `jit_session: Option<JitSession>`. Before each instruction (`vm.rs:1115-1140`) the VM snapshots frame regs into `[u64;256]` and calls `jit::tiered_execute_step_typed`. Cold code interprets; when a PC's hot counter hits `HOT_THRESHOLD=1000`, `find_compilable_region` (max 500 instrs, stops at unsupported op / `Ret`) is compiled. SIMD path: `simd_analyzer` detects element-wise binop/unary/cmp loops → `simd_compiler` emits prefix-scalar + SIMD-body (`I64x2`/`F64x2`/`I32x4`/`F32x4`) + epilogue CLIF; falls back to `compile_region_typed` (typed when metadata is provable, else scalar `compiler::compile_region`). Compiled fn ABI: `extern "C" fn(*mut u64 regs, *const u64 constants)`. Runtime helpers are `#[no_mangle] extern "C"` in `src/jit/runtime.rs` (NaN-tag-aware; div-by-zero → `nil`). `typed_compiler` strips NaN-tag guards using `TypeMetadata` recovered at tier-up time by `typed_compiler::infer_reg_types` (a conservative forward must-analysis over the enclosing function's bytecode — MIR pins each typed local to a fixed register, so types are recoverable from the instruction stream; unmodeled opcodes clobber all registers, effect opcodes yield empty metadata). The live tiering entry point is `jit::tiered_execute_step_typed` (`vm.rs` `step()`): hot regions compile through the guard-stripped path when register types are provable, falling back to scalar `compile_region` on absent/empty metadata or compile error. Typed `IDiv`/`IMod`/`FCmpEq` always emit runtime-helper calls (never raw `sdiv`/`srem`/`fcmp`) to match interpreter semantics exactly (div-by-zero → `nil`, epsilon float equality).

### Actor runtime lifecycle
1. **Spawn**: `Runtime::spawn_actor` → `fresh_actor_id()` (global `AtomicU64`) → `Actor::new` (64KB `ActorHeap` + `OrcaGc`) → enqueue in scheduler global `Injector`.
2. **Schedule**: `run_scheduler` loops `scheduler.dequeue()` (= global then any `Stealer` FIFO). `step_actor` sets `current_actor`, receives from mailbox, resolves `behavior_id` → `BehaviorEntry.handler_fn` (fn pointer) or bytecode handler (raw `*mut Runtime`), journals+checkpoints if persistent, increments `reduction_count` (monotonic lifetime metric; a separate `turn_reductions` tracks the per-turn budget); requeues while the mailbox is non-empty — when the per-turn budget (`max_reductions=1000` messages) is exhausted it resets `turn_reductions` and requeues at the back of the queue (yield); the turn budget also resets when the actor goes Waiting.
3. **Send**: `send_message_by_id` → `Message` → `mailbox.push` (always `Ok`, never drops) → ORCA `send_ref_to` bumps `foreign_count` → enqueue target.
4. **GC**: `process_gc_ops` drains `OrcaCoordinator` → per-actor `OrcaGc` applies deltas; `CycleDetector::incremental_detect` (epoch-gated) builds foreign-ref graph, suspects by weight, DFS, trial-decrement, reclaims.
5. **Fault**: `exit_actor` → `handle_actor_exit` → unregister + `leave_all`, DOWN to monitors, propagate to links (abnormal kills non-trapping; trapping gets System msg), `Supervisor.handle_exit` → `SupervisorAction` (`Restarted`/`Shutdown`/`Ignore`/`Escalate`) with cascading shutdown.

### Distribution
Custom TCP wire protocol (`src/runtime/network.rs`): length-prefixed frames, magic `NUL0`, 8-byte node-id handshake, `Packet` enum (`ActorMessage`/`Heartbeat`/`Ack`/`SpawnRequest`/`SpawnResponse`/`CrdtSync`/`CrdtDeltaSync`/`Gossip`) with hand-rolled big-endian serde. TCP links are fully duplex — reader threads run on both accepted inbound and dialled outbound connections (a joiner that only dials out still receives heartbeat replies). `AddressResolver` + `ActorAddress::{Local,Remote}` + LRU `RemoteActorCache` (10k) provide location transparency. Gossip membership in `cluster.rs` (`ClusterState::tick` → `ClusterAction`): `tick` heartbeats Joining + Healthy members; `SendGossip` is wired over the wire as `Packet::Gossip` (type 6, Vec<NodeGossip>: node id, address, status, incarnation per member), sent by `Runtime::process_network` and merged on receipt via `ClusterState::merge_membership` (higher incarnation wins; equal-incarnation gossip refreshes `last_heartbeat` as a liveness hint) — transitive propagation works, so a chain of pairwise seeds converges without a full mesh (heartbeat-based discovery — `process_network_packets` learns unknown heartbeat senders via `NetworkTransport::connection_addr` — remains the path by which a seed first learns about a joiner). Remote spawn: `Packet::SpawnRequest` is answered in `process_network_packets` — the receiver spawns the named behavior only if it was registered via `Runtime::register_spawnable_behavior` (unknown names get `SpawnResponse{success:false}`); the requester picks up the real actor id with `Runtime::take_spawn_response(request_id)` (the placeholder address from `spawn_on_node` carries the request id, not an actor id). Heartbeat packets carry the *sender's* node id. CRDTs: 8 types (`GCounter`, `PNCounter`, `GSet`, `ORSet`, `AWORSet` in `crdt.rs`; `LWWRegister`, `MVRegister`, `RGA` in `crdt_reg.rs`) behind the `Crdt` trait, owned by `CrdtManager`. Delta-state replication: each CRDT exposes `delta_since(base)` (minimal state that merges identically for any replica ≥ base; `None` = unchanged); `CrdtManager::generate_delta_sync_ops` ships first-seen entries full and changed entries as deltas over `Packet::CrdtDeltaSync` (type 7, `Vec<CrdtDeltaOp>`), applied via `apply_delta_op` (merge-only; deltas for unknown ids are ignored). `sync_crdts_delta` (`distributed.rs`) broadcasts deltas to healthy members; `Runtime::sync_crdts` ships deltas on most rounds and full state on round 1 and every `CRDT_FULL_SYNC_INTERVAL` (16) rounds thereafter — the sync base advances at delta generation, so these periodic full syncs are the repair mechanism for lost deltas. Persistence: `PersistenceStore` trait with `MemoryStore`, `JsonFileStore`, `SqliteStore` (rusqlite, two tables).

## Key Directories

- `src/` — language frontend + backend: `lexer.rs`, `parser.rs`, `ast.rs`, `typechecker.rs`, `types.rs`, `effect_checker.rs` (effects + capabilities), `bytecode.rs`, `value_layout.rs` (canonical NaN-boxing constants), `hir.rs`/`hir_lower.rs`, `mir.rs`/`mir_lower.rs`/`mir_codegen.rs`, `vm.rs`, `repl.rs`, `main.rs`, `lib.rs`, plus `integration_tests.rs` & `stress_tests.rs` (test-only). The legacy `compiler.rs` was removed — the pipeline is MIR-exclusive.
- `src/runtime/` — actor runtime: `mod.rs` (`Runtime` god-object), `actor.rs`, `scheduler.rs`, `mailbox.rs`, `heap.rs` (bump allocator + size-class free lists + large-object space for allocations over the 256-byte `Huge` threshold, exact-size free-list reuse, released on `reset()`/`Drop`), `gc.rs`, `orca_cycle.rs`, `supervisor.rs`, `registry.rs`, `process_groups.rs`, `timer.rs`, `cluster.rs`, `network.rs`, `distributed.rs`, `crdt.rs`/`crdt_reg.rs`/`crdt_manager.rs`, `persistence.rs`, `tests.rs`.
- `src/jit/` — Cranelift JIT: `mod.rs` (`JitSession`, `tiered_execute_step`, hot counters), `compiler.rs` (scalar CLIF), `typed_compiler.rs`, `simd_analyzer.rs`/`simd_compiler.rs`, `runtime.rs` (extern-C helpers), `tests.rs`.
- `src/lsp/` — `tower-lsp` language server (single `mod.rs`).
- `src/python/` — PyO3 interop: `bridge.rs` (GIL + `PythonRegistry`), `marshal.rs` (Value↔Py).
- `src/ai/` — v0.9 AI runtime: provider-agnostic LLM API (`client.rs`/`request.rs`/`response.rs`; async `LlmClient` + sync `complete_sync`), `providers/` (`ollama.rs`, `openai.rs`), memory (`memory.rs` episodic, `semantic_memory.rs`, `procedural_memory.rs`), `pipeline.rs` (sequential agent stages), `debate.rs`, `supervisor.rs` (worker teams), `schema.rs` (tool/JSON schema), `usage.rs` (token cost), `mock.rs`.
- `src/ffi/` — C-compatible FFI layer: `mod.rs` (module root + Rust registration API), `native.rs` (dynamic library registry), `marshal.rs` (Value↔C ABI), `c_api.rs` (stable C embedder API).
- `.cargo/` — `config.toml` (bfd linker + PyO3 abi3 env), `audit.toml` (one ignored advisory).
- `build.rs` — Fedora libpython symlink workaround for PyO3 linking.
- `.agents/` — orchestration scratch/handoff artifacts from a prior multi-agent analysis run; **not language source**.

## Development Commands

```bash
cargo build                      # dev build (opt-level 0, debug)
cargo build --release            # release (opt-level 3, LTO, codegen-units 1)
cargo test                       # run all 924 tests (test profile: no LTO, 16 codegen-units for speed)
cargo test --release             # run tests under the release profile
cargo run -- --repl              # interactive REPL (prompt `nulang>`)
cargo run -- --eval 'perform IO.print("Hello")'   # evaluate a string
cargo run -- --check myprogram.nula                 # type+effect+cap check only (no run)
cargo run -- myprogram.nula                          # compile and run a file
cargo run -- --lsp                                 # start the LSP server on stdin/stdout
cargo run -- -v myprogram.nula                       # verbose: print AST/bytecode/inferred type
python3 verify_implementation.py                  # gate: cargo test + forbidden-pattern scans + integration checks
python3 verify_report.py                          # gate: validates codebase_analysis_report.md structure
```

**Runtime requirements**: Rust stable 1.93+ (cranelift 0.132 requires 1.93), Linux or macOS. `.cargo/config.toml` forces the GNU `bfd` linker on `x86_64-unknown-linux-gnu` (for Cranelift/PyO3 native-symbol linking) and sets `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`. `build.rs` creates a `libpythonX.Y.so` symlink in `OUT_DIR` for Fedora-style systems missing the unversioned symlink (auto-detects Python; default 3.14). mimalloc is the `#[global_allocator]` (`src/main.rs`).

## Code Conventions & Common Patterns

- **Naming**: `snake_case` functions/methods/modules/files; `PascalCase` types/structs/enums and enum variants; `SCREAMING_SNAKE_CASE` consts (`HOT_THRESHOLD`, `TAG_INT`, `PAYLOAD_MASK`). `nulang_` prefix on `extern "C"` JIT runtime helpers. `__main` is the synthetic function wrapping a top-level expression (parser + HIR lowering).
- **Error model**: one project-wide `NuError` enum (`src/types.rs:463`) aliased `NuResult<T> = Result<T, NuError>`. Compile-time variants (`LexError`/`ParseError`/`TypeError`/`EffectError`/`CapError`/`LinearTypeError`) carry `{ msg: String, span: Span }`; runtime variants (`RuntimeError`/`VMError`/`PythonError`) carry `String`. `Display` formats spanned errors as `<Kind> at <line>:<col>: <msg>`. First error aborts; no error collection/recovery. `?` propagates. `EffectChecker`/`CapabilityAnalyzer` accumulate `diagnostics: Vec<String>` instead of failing fast. Runtime subsystems use per-domain enums (`RegisterError`, `PgError`) impl `std::error::Error`; persistence/network use `io::Result`/`Option`; JIT uses `CompileError`. No `anyhow`/`thiserror`.
- **Async**: only `main.rs` (`#[tokio::main]`), `src/lsp/`, and the `src/ai/` LLM client (`async_trait`) are async. VM, runtime, JIT, Python, REPL are all synchronous. Actor concurrency is cooperative reduction-yielding, not async tasks.
- **Non-blocking LLM calls**: `perform LLM.ask(...)` in scheduler-driven actor bytecode behaviors is non-blocking. The `LlmAsk` opcode calls `ActorVmCallbacks::llm_ask` (default delegates to blocking `complete_llm`); `BytecodeRuntimeCallbacks::llm_ask` builds the `LlmRequest` on the scheduler thread, spawns a `nulang-llm` worker thread (own current-thread tokio runtime + `block_on(client.complete(...))`, result sent over `Runtime.llm_tx`), and returns `Pending` — the VM then decrements the PC and raises the `"LlmAsk:suspend"` sentinel error, captured onto `actor.suspended_execution` in `run_bytecode_at_offset` exactly like `"SignalWait:suspend"` (helper `is_suspend_error`). `run_scheduler` pumps completions (`poll_llm_completions` → `store_llm_completion` → `resume_suspended_llm_step`, which re-installs per-actor callbacks before `vm.resume()` and re-captures on chained suspends) and keeps running while `llm_inflight_count > 0` (10ms `recv_timeout` wait when the queue is drained). Suspension is gated on `Runtime.llm_suspend_enabled`: `step_actor` enables it around the bytecode invocation; `ask_actor_sync` forces it off, so nested synchronous paths (pipelines, supervisors, debates, `Ask`, top-level VM) keep blocking behavior. Request build (`build_agent_llm_request`/`build_actor_llm_request`) happens pre-suspend; tool-call post-processing (`finish_tool_calls`) and agent memory/usage write-back (`finish_agent_llm`) happen on the scheduler thread at resume.
- **Unsafe / FFI**: raw `*mut` pointers with hand-written `unsafe Send/Sync` and `SAFETY` doc justifications (ORCA headers, foreign-ref ops, `BytecodeRuntimeCallbacks`). JIT function pointers obtained via `unsafe transmute` of `*const u8`; bytecode must not mutate during JIT execution. Python: GIL acquired via `Python::attach`; `PythonObjectId` is a non-owning `Copy` handle (real refcount in `PythonRegistry`); `get_object` acquires GIL **before** the registry `Mutex` to avoid lock-order deadlock.
- **Dependency injection / decoupling**: the VM talks to the runtime through two object-safe callback traits (`ActorVmCallbacks`, `DistributedVmCallbacks`) — default `StandaloneVmCallbacks` owns a private `ActorHeap`. `RuntimeVmCallbacks` (`Rc<RefCell<Runtime>>`) and `BytecodeRuntimeCallbacks` (raw `*mut Runtime`) bridge the other direction.
- **State**: actor state is a `Vec<(String, Value)>` linear scan (not a `HashMap`); each field has a `StateModel` (`Local`/`Durable`/`EventSourced`/`Crdt`). Actor identity is a bare `u64` (no `Pid` wrapper), from `fresh_actor_id()`.
- **Spans**: threaded into nearly every `Expr`/`Decl` variant and every compile-time error.
- **Type system**: HM Algorithm W (`Substitution = Vec<(TypeVar,Type)>`, `mgu` + occurs check, `generalize`/`instantiate` over `Type::Scheme`); Pony-inspired `Capability` lattice (`is_subtype_of` via `join`, `is_sendable`) with exactly-once `LinearIso` tracking in `TypeContext`; Koka-inspired row-polymorphic `EffectRow` (`Closed`/`Open` with `Region`).

## Important Files

- `src/main.rs` — CLI entry; hand-rolled arg parser; `run_source`/`check_source` pipeline; `#[tokio::main]`.
- `src/lib.rs` — crate root; declares all public modules.
- `src/hir.rs`, `src/mir.rs` — High-level and Mid-level IR type definitions.
- `src/hir_lower.rs`, `src/mir_lower.rs`, `src/mir_codegen.rs` — AST → HIR → MIR → bytecode pipeline.
- `src/vm.rs` — NaN-boxed `Value`, `Frame`, `VM`, `step`/`run`, effect handlers, JIT hook, callback traits.
- `src/runtime/mod.rs` — `Runtime` god-object; actors, scheduler, GC, supervision, distribution, persistence.
- `src/lsp/mod.rs` — Full-featured LSP server (12 features: hover, goto def, references, rename, signature help, inlay hints, completion, diagnostics, etc.).
- `src/types.rs` — `NuError`/`NuResult`, `Type`, `Capability`, `EffectRow`, `Span`.
- `src/bytecode.rs` — `OpCode` (141), `Instruction` (32-bit), `Constant`, `CodeModule`.

## Runtime/Tooling Preferences

- **Runtime**: Rust stable, edition 2021. Linux/macOS (Windows planned).
- **Linker**: GNU `bfd` forced on x86_64 Linux via `.cargo/config.toml` (not `lld`) for Cranelift/PyO3 compatibility.
- **Python**: PyO3 0.29 abi3 limited-API; `build.rs` symlinks `libpythonX.Y.so` for Fedora.
- **Allocator**: mimalloc (`#[global_allocator]` in `main.rs`).
- **Cargo features**: `default = ["python", "sqlite", "lsp"]` (PyO3 interop, rusqlite persistence, tower-lsp server) — all optional and on by default; `--no-default-features --features <subset>` builds a leaner binary.
- **No external test/criterion/proptest crates** — standard `#[test]` only.

## Testing & QA

- **Framework**: standard Rust `#[test]` + `#[cfg(test)]`. No proptest/quickcheck/criterion. No `#[ignore]`/`#[should_panic]`/async tests.
- **Organization**: two styles — (a) inline `mod tests` at file foot (`lexer.rs`, `parser.rs`, `typechecker.rs`, `effect_checker.rs`, `value_layout.rs`, `vm.rs`, most `runtime/*.rs`, `jit/*`, `python/*`, `ffi/*`, `ai/*`, `lsp/mod.rs`); (b) dedicated test files (`src/integration_tests.rs`, `src/stress_tests.rs`, `src/runtime/tests.rs`, `src/jit/tests.rs`).
- **Naming**: `test_<subject>` (unit/integration), `stress_<scenario>` (chaos).
- **Counts** (924 total, lib suite): `integration_tests.rs` 122 (end-to-end pipeline via `run_source`/`assert_int`/`run_source_with_runtime`, plus MIR-pipeline variants via `run_source_new`/`assert_int_new`/`run_source_new_with_runtime`, plus selective-receive, non-blocking LLM suspend/resume, and typed-JIT tiering tests), `stress_tests.rs` 30 (`stress_*` chaos: mailbox floods, crash/exit cascades, scheduler fairness, CRDT/persistence, GC/cycle-detector churn), `runtime/tests.rs` 84, `jit/tests.rs` 34; the remaining 654 are inline unit tests (`src/ai/` alone has 45). Doc-tests: 3 run, 8 ignored.
- **Helpers/fixtures**: `run_source`/`compile_source`/`assert_int` + `SharedMemoryStore` (`Arc<Mutex<MemoryStore>>` impl `PersistenceStore`) for restart simulation (`integration_tests.rs`); `TestContext {counters, log}` (`stress_tests.rs`); `make_jit()` (`jit/tests.rs`).
- **Run**: `cargo test` (test profile: LTO off, 16 codegen-units for fast parallel builds). `cargo test --release` for optimized runs.
- **Gate scripts**: `verify_implementation.py` (forbidden-pattern scans for known anti-patterns — Box'd frames, string leaks, `crdt_reg` temp Vec, timer BinaryHeap rebuild, check-then-unwrap — + asserts JIT integration, escape-analysis deadness, scheduler-stats and cycle-detector wiring, then runs `cargo test` and `cargo check --tests` against a zero-warning baseline) and `verify_report.py` (validates `codebase_analysis_report.md`: required sections, ≥5 code snippets, referenced `src/*.rs` paths exist). Each exits 0 only on full pass.
- **Audit**: `.cargo/audit.toml` ignores `RUSTSEC-2026-0186` (memmap2 unsound; vulnerable APIs unused; upgrade blocked on cranelift-jit).

## Known Hazards (for assistants)

- The `escape_analysis.rs` module was removed; its former tests and references have been cleaned up.
- NaN-tag constants now have a **single source of truth**: `src/value_layout.rs` (`TAG_MASK`, `PAYLOAD_MASK`, `SIGN_BIT`, all `TAG_*`, plus `sext48`/`tag_int`/`tag_bool`). `src/vm.rs`, `src/jit/{runtime,compiler,typed_compiler,simd_compiler}.rs`, and `src/python/marshal.rs` all import from it — do not reintroduce local copies. The one exception is `TAG_PYTHON` (`0x7FF6`), defined in `src/python/bridge.rs` and imported by `marshal.rs`; it was chosen not to collide with `TAG_CLOSURE` (`0x7FF7`) or `TAG_STRING` (`0x7FFE`).
- Remote actor messages carry the behavior **name** on the wire (`Packet::ActorMessage.behavior_name`); the receiving node resolves it via `Runtime::behavior_id_for` against the target actor's behavior table on delivery (`process_network_packets` in `src/runtime/distributed.rs`), falling back to behavior id 0 for unknown names — mirroring local `send_message`'s `unwrap_or(0)`.
- Cluster membership entries carry their gossip version in the `_incarnation` metadata key on `NodeInfo.metadata` (absent = baseline 1 in `gossip_payload`, 0 in merge-compare). `merge_membership` only applies status/address changes on a **strictly higher** incarnation; equal incarnation only refreshes `last_heartbeat`. `join_cluster` seeds start at incarnation 1 so gossip can't clobber the authoritative seed address; `handle_heartbeat` bumps the entry incarnation on status promotions so they propagate.
- `LamportTime`/`LamportClock` are defined in `crdt.rs` and imported by `crdt_reg.rs` — single definition, no duplication.
- LSP: **12 features** (per the capability table at the top of `src/lsp/mod.rs`) — diagnostics (full frontend), hover, goto definition, references, document symbols, rename (with prepareRename), signature help, formatting, semantic tokens, code actions, inlay hints (typechecker-backed for well-formed programs, regex fallback), completion. Zero compiler warnings (enforced by `verify_implementation.py`).
- The `receive` expression (`receive { | Behavior(params) => expr }`) is wired end-to-end with selective-receive dispatch. MIR lowering (`lower_receive` in `src/mir_lower.rs`) resolves arm behavior names to behavior-table indices (same suffix-match rule as `send`) and emits `mir::RValue::ReceiveMatch` (bytecode `OpCode::ReceiveMatch` 0x8F): a spec constant `"max_params:id1,id2,..."` carries the candidate behavior ids, the VM calls `ActorVmCallbacks::try_receive_match(&ids)` (mailbox scan in `Mailbox::receive_match`, FIFO order, non-matching messages requeued), writes the matched arm index to dst plus payload values into the following registers (missing → nil, extras ignored), and a MIR compare chain dispatches to the arm body with params bound. No-match falls through to the legacy pop-any `Receive` (nil when the mailbox is empty or outside an actor context) — non-blocking, no suspension.
- The compiler pipeline is MIR-exclusive (AST → HIR → MIR → bytecode). The legacy AST compiler (`src/compiler.rs`) has been removed.
