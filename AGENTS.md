# Repository Guidelines

> Practical orientation for AI assistants working in the Nulang codebase.
> All paths are relative to the repo root (`~/nulang`).

## Project Overview

**Nulang** is a distributed, actor-based programming language written in Rust (edition 2021, single crate `nulang`). It fuses Erlang-style fault-tolerant actors with a Rust/Pony-inspired type system (Hindley-Milner inference + reference capabilities + row-polymorphic algebraic effects), a register-based bytecode VM, a Cranelift JIT, BEAM/OTP primitives, CRDTs, location-transparent distribution, SQLite/JSON persistence, PyO3 Python interop, and a C-compatible FFI layer. Status: Alpha; ~789 tests pass (`cargo test`). License: Apache-2.0.

## Architecture & Data Flow

The compiler pipeline is a straight line, wired exactly in `src/main.rs:174-195` (`run_source`) and reused by the REPL (`src/repl.rs`) and LSP:

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

`--check` stops after capability analysis (no compile/run). The runtime (`src/runtime/`) is a **single-threaded synchronous coordinator** (`Runtime`) driving actors via reduction-bounded `step_actor`; it reaches the VM only through two object-safe callback traits (`ActorVmCallbacks`, `DistributedVmCallbacks`) to keep the dependency cycle-free. There is **no async/await in the runtime or VM** — concurrency is `crossbeam` deques/queues + `std::sync` atomics/RwLock + raw `unsafe` pointers for ORCA GC. The only async surface is `main.rs` (`#[tokio::main]`) and the LSP server (`tower-lsp` over tokio stdin/stdout).

### Backend representation
- **Value** (`src/vm.rs`): NaN-boxed `u64` (`raw`), 48-bit payload, 16-bit type tag in the quiet-NaN bits. Tags: `TAG_NIL 0x7FF8`, `TAG_UNIT 0x7FF9`, `TAG_BOOL 0x7FFA`, `TAG_INT 0x7FFB`, `TAG_PTR 0x7FFC`, `TAG_ACTOR 0x7FFD`, `TAG_STRING 0x7FFE`, `TAG_CLOSURE 0x7FF7`.
- **Instruction** (`src/bytecode.rs`): 32-bit fixed-width `{opcode:u8, op1:u8, op2:u8, op3:u8}`; helpers `new0/1/2/3`, `imm16()`, `simm16()`, `offset16()`. ~92 opcodes across 11 categories (Special, Stack/Locals, IntArith, FloatArith, Compare/Logic, Control, Closures, Memory/Objects, Actor, Effects, Python, Capabilities, FFI, Distribution, String/IO, Debug).
- **Frames**: 256 registers each; flat `Vec<Frame>` with `caller_idx` links; closures carry `closure_env`.

### Effects & capabilities at runtime
Algebraic effects are runtime-resolved via `handler_stack: Vec<HandlerFrame>`: `Handle` pushes, `Perform` deep-clones a `Continuation` into the matching handler and jumps to its offset, `Resume` restores the continuation (overwrites frames/pc), `Unwind` pops. The static `EffectRow` is checked pre-compile; runtime only sees the four opcodes. Capabilities (`iso/trn/ref/val/box/tag/lineariso`) are **compile-time only** and erased at runtime — VM `CapChk`/`CapUp`/`CapDown`/`CapSend` are MVP no-ops (`CapChk` writes `true`, others copy).

### JIT tiering
`VM` holds `jit_session: Option<JitSession>`. Before each instruction (`vm.rs:745-771`) the VM snapshots frame regs into `[u64;256]` and calls `jit::tiered_execute_step`. Cold code interprets; when a PC's hot counter hits `HOT_THRESHOLD=1000`, `find_compilable_region` (max 500 instrs, stops at unsupported op / `Ret`) is compiled. SIMD path: `simd_analyzer` detects element-wise binop/unary/cmp loops → `simd_compiler` emits prefix-scalar + SIMD-body (`I64x2`/`F64x2`/`I32x4`/`F32x4`) + epilogue CLIF; falls back to scalar `compiler::compile_region`. Compiled fn ABI: `extern "C" fn(*mut u64 regs, *const u64 constants)`. Runtime helpers are `#[no_mangle] extern "C"` in `src/jit/runtime.rs` (NaN-tag-aware; div-by-zero → `nil`). `typed_compiler` strips NaN-tag guard…

### Actor runtime lifecycle
1. **Spawn**: `Runtime::spawn_actor` → `fresh_actor_id()` (global `AtomicU64`) → `Actor::new` (64KB `ActorHeap` + `OrcaGc`) → enqueue in scheduler global `Injector`.
2. **Schedule**: `run_scheduler` loops `scheduler.dequeue()` (= global then any `Stealer` FIFO). `step_actor` sets `current_actor`, receives from mailbox, resolves `behavior_id` → `BehaviorEntry.handler_fn` (fn pointer) or bytecode handler (raw `*mut Runtime`), journals+checkpoints if persistent, increments `reduction_count`; requeues if mailbox non-empty && `!should_yield` (`max_reductions=1000`).
3. **Send**: `send_message_by_id` → `Message` → `mailbox.push` (always `Ok`, never drops) → ORCA `send_ref_to` bumps `foreign_count` → enqueue target.
4. **GC**: `process_gc_ops` drains `OrcaCoordinator` → per-actor `OrcaGc` applies deltas; `CycleDetector::incremental_detect` (epoch-gated) builds foreign-ref graph, suspects by weight, DFS, trial-decrement, reclaims.
5. **Fault**: `exit_actor` → `handle_actor_exit` → unregister + `leave_all`, DOWN to monitors, propagate to links (abnormal kills non-trapping; trapping gets System msg), `Supervisor.handle_exit` → `SupervisorAction` (`Restarted`/`Shutdown`/`Ignore`/`Escalate`) with cascading shutdown.

### Distribution
Custom TCP wire protocol (`src/runtime/network.rs`): length-prefixed frames, magic `NUL0`, 8-byte node-id handshake, `Packet` enum (`ActorMessage`/`Heartbeat`/`Ack`/`SpawnRequest`/`SpawnResponse`/`CrdtSync`) with hand-rolled big-endian serde. `AddressResolver` + `ActorAddress::{Local,Remote}` + LRU `RemoteActorCache` (10k) provide location transparency. Gossip membership in `cluster.rs` (`ClusterState::tick` → `ClusterAction`). CRDTs: 8 types (`GCounter`, `PNCounter`, `GSet`, `ORSet`, `AWORSet` in `crdt.rs`; `LWWRegister`, `MVRegister`, `RGA` in `crdt_reg.rs`) behind the `Crdt` trait, owned by `CrdtManager`. Persistence: `PersistenceStore` trait with `MemoryStore`, `JsonFileStore`, `SqliteStore` (rusqlite, two tables).

## Key Directories

- `src/` — language frontend + backend: `lexer.rs`, `parser.rs`, `ast.rs`, `typechecker.rs`, `types.rs`, `effect_checker.rs`, `effects.rs`, `capabilities.rs`, `compiler.rs`, `bytecode.rs`, `vm.rs`, `repl.rs`, `main.rs`, `lib.rs`, plus `integration_tests.rs` & `stress_tests.rs` (test-only).
- `src/runtime/` — actor runtime: `mod.rs` (`Runtime` god-object), `actor.rs`, `scheduler.rs`, `mailbox.rs`, `heap.rs`, `dual_heap.rs`, `gc.rs`, `orca_cycle.rs`, `supervisor.rs`, `registry.rs`, `process_groups.rs`, `timer.rs`, `cluster.rs`, `network.rs`, `distributed.rs`, `crdt.rs`/`crdt_reg.rs`/`crdt_manager.rs`, `persistence.rs`, `tests.rs`.
- `src/jit/` — Cranelift JIT: `mod.rs` (`JitSession`, `tiered_execute_step`, hot counters), `compiler.rs` (scalar CLIF), `typed_compiler.rs`, `simd_analyzer.rs`/`simd_compiler.rs`, `runtime.rs` (extern-C helpers), `tests.rs`.
- `src/lsp/` — `tower-lsp` language server (single `mod.rs`).
- `src/python/` — PyO3 interop: `bridge.rs` (GIL + `PythonRegistry`), `marshal.rs` (Value↔Py).
- `src/ffi/` — C-compatible FFI layer: `mod.rs` (module root + Rust registration API), `native.rs` (dynamic library registry), `marshal.rs` (Value↔C ABI), `c_api.rs` (stable C embedder API).
- `.cargo/` — `config.toml` (bfd linker + PyO3 abi3 env), `audit.toml` (one ignored advisory).
- `build.rs` — Fedora libpython symlink workaround for PyO3 linking.
- `.agents/` — orchestration scratch/handoff artifacts from a prior multi-agent analysis run; **not language source**.

## Development Commands

```bash
cargo build                      # dev build (opt-level 0, debug)
cargo build --release            # release (opt-level 3, LTO, codegen-units 1)
cargo test                       # run all ~590+ tests (test profile: no LTO, 16 codegen-units for speed)
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

**Runtime requirements**: Rust stable 1.70+, Linux or macOS. `.cargo/config.toml` forces the GNU `bfd` linker on `x86_64-unknown-linux-gnu` (for Cranelift/PyO3 native-symbol linking) and sets `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1`. `build.rs` creates a `libpythonX.Y.so` symlink in `OUT_DIR` for Fedora-style systems missing the unversioned symlink (auto-detects Python; default 3.14). mimalloc is the `#[global_allocator]` (`src/main.rs`).

## Code Conventions & Common Patterns

- **Naming**: `snake_case` functions/methods/modules/files; `PascalCase` types/structs/enums and enum variants; `SCREAMING_SNAKE_CASE` consts (`HOT_THRESHOLD`, `TAG_INT`, `PAYLOAD_MASK`). `nulang_` prefix on `extern "C"` JIT runtime helpers. `__main` is the synthetic function wrapping a top-level expression (parser + HIR lowering).
- **Error model**: one project-wide `NuError` enum (`src/types.rs:523`) aliased `NuResult<T> = Result<T, NuError>`. Compile-time variants (`LexError`/`ParseError`/`TypeError`/`EffectError`/`CapError`/`LinearTypeError`) carry `{ msg: String, span: Span }`; runtime variants (`RuntimeError`/`VMError`/`PythonError`) carry `String`. `Display` formats spanned errors as `<Kind> at <line>:<col>: <msg>`. First error aborts; no error collection/recovery. `?` propagates. `EffectChecker`/`CapabilityAnalyzer` accumulate `diagnostics: Vec<String>` instead of failing fast. Runtime subsystems use per-domain enums (`RegisterError`, `PgError`) impl `std::error::Error`; persistence/network use `io::Result`/`Option`; JIT uses `CompileError`. No `anyhow`/`thiserror`.
- **Async**: only `main.rs` (`#[tokio::main]`) and `src/lsp/` are async. VM, runtime, JIT, Python, REPL are all synchronous. Actor concurrency is cooperative reduction-yielding, not async tasks.
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
- `src/lsp/mod.rs` — Full-featured LSP server (11 features: hover, goto def, references, rename, signature help, etc.).
- `src/types.rs` — `NuError`/`NuResult`, `Type`, `Capability`, `EffectRow`, `Span`.
- `src/bytecode.rs` — `OpCode` (~91), `Instruction` (32-bit), `Constant`, `CodeModule`.

## Runtime/Tooling Preferences

- **Runtime**: Rust stable, edition 2021. Linux/macOS (Windows planned).
- **Linker**: GNU `bfd` forced on x86_64 Linux via `.cargo/config.toml` (not `lld`) for Cranelift/PyO3 compatibility.
- **Python**: PyO3 0.29 abi3 limited-API; `build.rs` symlinks `libpythonX.Y.so` for Fedora.
- **Allocator**: mimalloc (`#[global_allocator]` in `main.rs`).
- **No external test/criterion/proptest crates** — standard `#[test]` only.

## Testing & QA

- **Framework**: standard Rust `#[test]` + `#[cfg(test)]`. No proptest/quickcheck/criterion. No `#[ignore]`/`#[should_panic]`/async tests.
- **Organization**: two styles — (a) inline `mod tests` at file foot (`compiler.rs`, `lexer.rs`, `parser.rs`, `typechecker.rs`, `effect_checker.rs`, most `runtime/*.rs`, `jit/simd_*`, `python/*`, `lsp/mod.rs`); (b) dedicated test files (`src/integration_tests.rs`, `src/stress_tests.rs`, `src/runtime/tests.rs`, `src/jit/tests.rs`).
- **Naming**: `test_<subject>` (unit/integration), `stress_<scenario>` (chaos).
- **Counts**: `integration_tests.rs` ~106 (end-to-end pipeline via `run_source`/`assert_int`/`run_source_with_runtime`, plus ~20 MIR-pipeline tests via `run_source_new`/`assert_int_new`/`run_source_new_with_runtime`), `stress_tests.rs` exactly 10 (actor lifecycle/supervision/scheduler fairness under load), `runtime/tests.rs` 110 (83 pre-v0.7 + 24 v0.7 BEAM primitives + 3 v1.0 scheduler/cycle-detector checks), `jit/tests.rs` 18.
- **Helpers/fixtures**: `run_source`/`compile_source`/`assert_int` + `SharedMemoryStore` (`Arc<Mutex<MemoryStore>>` impl `PersistenceStore`) for restart simulation (`integration_tests.rs`); `TestContext {counters, log}` (`stress_tests.rs`); `make_jit()` (`jit/tests.rs`).
- **Run**: `cargo test` (test profile: LTO off, 16 codegen-units for fast parallel builds). `cargo test --release` for optimized runs.
- **Gate scripts**: `verify_implementation.py` (runs `cargo test` + forbidden-pattern scans for known anti-patterns + asserts JIT integration) and `verify_report.py` (validates `codebase_analysis_report.md`). Exits 0 only on full pass.
- **Audit**: `.cargo/audit.toml` ignores `RUSTSEC-2026-0186` (memmap2 unsound; vulnerable APIs unused; upgrade blocked on cranelift-jit).

## Known Hazards (for assistants)

- The `escape_analysis.rs` module was removed; its former tests and references have been cleaned up.
- NaN-tag constants are **duplicated** between `src/vm.rs`, `src/jit/runtime.rs`, `src/jit/typed_compiler.rs`, and `src/python/marshal.rs`. `marshal.rs` imports `TAG_PYTHON` from `bridge.rs` (`0x7FF6`) so it does not collide with `TAG_CLOSURE` (`0x7FF7`) or `TAG_STRING` (`0x7FFE`).
- Remote actor messages send `behavior_id=0` as a placeholder (remote side resolves the name) — a known stub in the distributed trait API.
- `LamportTime`/`LamportClock` are defined in `crdt.rs` and imported by `crdt_reg.rs` — single definition, no duplication.
- LSP: **13 features** — hover, goto definition, document symbols, references, prepare rename, rename, signature help, code actions, formatting, semantic tokens, inlay hints (typechecker-backed for well-formed programs, regex fallback), completion, diagnostics (full frontend). Zero compiler warnings.
- The `receive` expression (`receive { | Behavior(params) => expr }`) is fully implemented through lexer→parser→HIR→MIR→VM. The VM handler calls `ActorVmCallbacks::try_receive()` which pops the next message from the actor's mailbox and returns its first payload value. Pattern-matching dispatch across arms is future work (currently evaluates all arms and returns the first message value).
- The compiler pipeline is MIR-exclusive (AST → HIR → MIR → bytecode). The legacy AST compiler (`src/compiler.rs`) has been removed.
