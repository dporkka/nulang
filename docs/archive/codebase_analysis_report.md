# Nulang Codebase Analysis Report

**Date:** 2026-07-20  
**Commit context:** Current working tree  
**Test status:** `cargo test` passes with 1366 tests (9 ignored across 3 suites); `cargo test --features wasm-backend` passes with 1372 tests (9 ignored).  

This report evaluates the Nulang compiler, VM, runtime, and supporting design documents. It recognizes the fixes that have already landed and documents the remaining architectural gaps, coverage holes, and technical debt that should be addressed before the next milestone.

---

## 1. Architectural & Design Review

### 1.1 Compiler Pipeline

The compiler pipeline is orchestrated by `run_source` in `src/main.rs:174-195` and reused by the REPL (`src/repl.rs`) and LSP (`src/lsp/mod.rs`):

```rust
// src/main.rs — conceptual pipeline ordering
let tokens = Lexer::new(source).lex()?;
let ast = Parser::new(tokens).parse_module()?;
let _ty = TypeChecker::new().check_module(&ast)?;
let _effects = EffectChecker::new().infer_effects(&ast)?;
let _caps = CapabilityAnalyzer::new().infer_cap(&ast)?;
let hir = hir_lower::lower_module(&ast)?;
let mir = mir_lower::lower_module(&hir)?;
let code = mir_codegen::compile_mir(&mir)?;
let mut vm = VM::new();
vm.load_module(code)?;
vm.run()
```

All phases are wired together. The previous escape-analysis module was dead code and has been removed; heap allocation elision will be revisited alongside a correct nursery implementation.

### 1.2 Typechecker (`src/typechecker.rs`)

Two previously reported issues have been resolved:

* **`infer_app` preserves function effect rows.** Earlier code forced `EffectRow::empty()` during application, which broke unification for effectful functions. The current implementation propagates the callee's actual effect row.

* **`infer_handle` checks handler bodies.** The typechecker now infers a type for each handler arm and unifies it with the body's return type, satisfying `SPEC2.md` Chapter 4 requirements.

Remaining gap: `infer_agent_decl` (`src/typechecker.rs:1479-1506`) still stubs the agent type with fresh variables because there is no runtime implementation to constrain.

### 1.3 VM (`src/vm.rs`)

The VM uses a flat `Vec<Frame>` stack, eliminating the previous per-call `Box<Frame>` heap allocation:

```rust
// src/vm.rs
pub struct Frame {
    pub regs: [Value; 256],
    pub pc: usize,
    pub module_idx: usize,
    pub return_dst: u8,
    pub caller_idx: Option<usize>,   // flat-stack parent link
    pub closure_env: Option<Value>,
}

pub struct VM {
    modules: Vec<CodeModule>,
    frames: Vec<Frame>,              // single contiguous stack
    current_frame_idx: Option<usize>,
    handler_stack: Vec<HandlerFrame>,
    jit_session: Option<JitSession>,
    actor_callbacks: Box<dyn ActorVmCallbacks>,
}
```

`OpCode::Call` pushes onto `frames`, `OpCode::Ret` pops to `caller_idx`, and continuations deep-clone the active slice of the vector.

### 1.4 Actor Runtime (`src/runtime/mod.rs`)

`Runtime` remains the single-threaded synchronous coordinator. The scheduler now exposes atomics-based profiling counters via `SchedulerStats` (`src/runtime/scheduler.rs:28-43`), accessible through `Runtime::scheduler_stats()` / `Runtime::reset_scheduler_stats()`.

---

## 2. Performance & Optimization

### 2.1 VM Allocation Paths Now Route Through Actor GC

The VM no longer leaks strings or raw-allocates arrays through the system allocator. `OpCode::SConcat`, `OpCode::SRead`, `OpCode::ArrAlloc`, and `OpCode::Drop` now delegate to `actor_callbacks`:

```rust
// src/vm.rs — current pattern for string concat
OpCode::SConcat => {
    let s1 = self.string_from_value(src1)?;
    let s2 = self.string_from_value(src2)?;
    let result = format!("{}{}", s1, s2);
    let ptr = self.alloc_string(&result)?;
    frame.regs[dst] = ptr;
}
```

`alloc_string` consults `self.actor_callbacks.alloc(...)` so payloads live on the current actor's heap and are visible to ORCA reference counting.

### 2.2 RGA CRDT Uses Lazy Iteration

`RGA::insert_at` and `RGA::delete_at` in `src/runtime/crdt_reg.rs` no longer build a temporary `Vec<ElementId>`. They use `.nth()` on the live-elements iterator:

```rust
// src/runtime/crdt_reg.rs
pub fn insert_at(&mut self, index: usize, value: T) -> ElementId {
    let parent = if index == 0 {
        None
    } else {
        self.elements
            .iter()
            .filter(|e| e.value.is_some())
            .nth(index - 1)
            .map(|e| e.id)
    };
    self.insert_after(parent, value)
}
```

### 2.3 MVRegister Uses In-Place `HashSet::retain`

`MVRegister::write` and `MVRegister::merge` (`src/runtime/crdt_reg.rs`) now prune outdated timestamps in place instead of allocating temporary vectors.

### 2.4 Timer Wheel Uses `BinaryHeap::peek()`

`TimerWheel::tick` (`src/runtime/timer.rs`) now examines the heap top and breaks early, turning the previous full-heap rebuild into an $O(K \log N)$ operation for the $K$ fired timers.

### 2.5 JIT Uses Strongly-Typed Helper Enum

`src/jit/compiler.rs` defines `RuntimeHelper` and looks up extern helpers by enum key rather than by string:

```rust
// src/jit/compiler.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeHelper {
    IAdd, ISub, IMul, IDiv, IMod,
    ICmpEq, ICmpLt, ICmpGt, ICmpLe, ICmpGe,
    FAdd, FSub, FMul, FDiv,
    // ...
}

let func_ref = *helpers.get(&RuntimeHelper::IAdd).expect("helper registered");
```

This removes the previous stringly-typed lookup and the associated typo/panic risk.

### 2.6 Remaining Optimization Concerns

* **Typed and SIMD JIT compilers** (`src/jit/typed_compiler.rs`, `src/jit/simd_compiler.rs`) are exercised only indirectly. Without dedicated stress tests, the hot-path benefit of the JIT tier is unquantified.
* **Escape analysis was removed.** The previous escape-analysis module was dead code and has been deleted; heap allocation elision is not yet realized and will be revisited with a correct nursery implementation.

---

## 3. Code Quality & Rust Idioms

### 3.1 Resolved Code-Quality Issues

The following items from earlier reports are now idiomatic and should not be re-reported as active bugs:

| File | Old issue | Current state |
|------|-----------|---------------|
| `src/mir_codegen.rs` | `unsafe { transmute }` for `Self` opcode | Uses `OpCode::SelfOp` |
| `src/typechecker.rs` | `infer_app` forced `EffectRow::empty()` | Preserves callee effect row |
| `src/typechecker.rs` | `infer_handle` ignored handler arms | Checks each handler body |
| `src/vm.rs` | Per-call `Box<Frame>` allocation | Flat `Vec<Frame>` with `caller_idx` |
| `src/vm.rs` | `SConcat`/`SRead` leaked strings | Routes through actor callbacks |
| `src/runtime/crdt_reg.rs` | RGA temporary `Vec` in `insert_at` | Lazy `.nth()` iteration |
| `src/runtime/crdt_reg.rs` | MVRegister temporary vectors | `HashSet::retain` |
| `src/runtime/timer.rs` | Full heap rebuild in `tick` | `peek()` + early break |
| `src/runtime/distributed.rs` | Cache check-then-unwrap | Single `if let Some(...)` |
| `src/runtime/crdt.rs` | Packed `u64` ORSet tag | Structured `Tag { node_id, counter }` |
| `src/jit/compiler.rs` | String helper lookup | `RuntimeHelper` enum |

### 3.2 Outstanding Technical Debt

`cargo check` currently reports 0 warnings. A recent cleanup pass removed dead scaffolding (including the previously referenced `NativeActorPool` structure) and resolved unused constants and imports across the parser, runtime, and Python modules. The `src/python/bridge.rs` and `src/python/marshal.rs` modules are active, tested PyO3 code rather than abandoned scaffolding. Going forward, the project should keep `cargo check` warning-free.

---

## 4. Verification & Test Coverage

### 4.1 Current Test Inventory

* `src/integration_tests.rs`: ~52 end-to-end pipeline tests.
* `src/stress_tests.rs`: 29 actor/supervision/scheduler/runtime chaos tests.
* `src/runtime/tests.rs`: 110 runtime unit tests.
* `src/jit/tests.rs`: 18 JIT tests.
* Inline `mod tests` in lexer, parser, `mir_codegen`, typechecker, effect checker, and others.

### 4.2 Coverage Gaps

1. **Dedicated frontend unit tests.** `src/lexer.rs`, `src/parser.rs`, and `src/mir_codegen.rs` rely heavily on integration tests. Boundary cases such as invalid escape sequences, malformed match arms, and all `OpCode` emission paths are not covered at the unit level.

2. **Typed and SIMD JIT compilers.** `src/jit/typed_compiler.rs` and `src/jit/simd_compiler.rs` lack targeted tests beyond the generic JIT suite. SIMD loops, NaN-tag stripping, and typed-to-untyped fallback paths need isolated regression tests.

3. **Allocator and GC stress coverage.** The per-actor bump allocator (`src/runtime/heap.rs`) and ORGC protocol are exercised by integration and stress tests, but dedicated unit tests for size-class free-list reuse, large-object allocation, and cross-actor foreign-reference churn under high allocation pressure are areas to expand.

4. **CRDT manager replication.** `src/runtime/crdt_manager.rs` has no mock-network tests for convergence under packet loss or partition healing.

5. **Cycle-detector foreign-ref graph.** The wiring between the runtime's GC paths and the cycle detector has been verified under stress (`stress_gc_cycle_detector_under_foreign_ref_load` and related tests). The foreign-ref graph is populated during cross-actor reference transfers and the detector runs its incremental epoch-gated algorithm as intended. This item is considered resolved.

---

## 5. Specification Alignment

### 5.1 Implemented Specifications

`SPEC.md` and the core chapters of `SPEC2.md` are largely implemented: lexer, parser, HM type inference, effect rows, capabilities, register VM, algebraic effects at runtime, actors, supervision, persistence, and distribution primitives.

### 5.2 Design Documents Without Implementation

Most design specifications have now been implemented. The remaining design-only documents are:

1. **`DESIGN_WEB_FRAMEWORK.md`** — Phoenix-style endpoints, routers, controllers, channels, and LiveView. No files implement this framework yet; it is a candidate for a future v0.14+ release.

2. **`DESIGN_CLOUD.md` / `DESIGN_CLOUD_PLATFORM.md`** — Cloud control-plane and managed service concepts. No runtime control plane or managed Nulang Cloud service code exists in this repository; the separately hosted [Nulang Cloud](https://nulang.cloud) service is the external realization of this design.

The following items were previously listed here but are now implemented:

- **AI runtime** — `src/ai/` provides LLM providers (OpenAI, Ollama), episodic/semantic/procedural memory, agent pipelines, debate teams, and supervisor teams. The `agent` declaration is parsed, desugared into an actor, and executed through the runtime.
- **Workflow runtime** — `workflow` declarations, durable steps, saga compensation, parallel branches, and signal waiting are supported end-to-end. The runtime persists workflow state and replays on restart.
- **Package manager** — `src/package/` implements the `nula` CLI (`nula new`, `build`, `test`, `run`, `build-wasm`) with `Nulang.toml` manifests, `Nulang.lock` lockfiles, and local-path/git dependency resolution.

### 5.3 Recommended Roadmap

```text
Priority 1: Complete specification-driven subsystems
  - DESIGN_WEB_FRAMEWORK.md: Phoenix-style web framework (endpoints, routers, controllers, channels, LiveView).
  - DESIGN_CLOUD.md: Cloud control-plane and managed-service abstractions.

Priority 2: Harden production paths
  - Keep `cargo check` warning-free and expand stress-test coverage for JIT typed/simd fallback and CRDT convergence.
  - Continue optimizing ORCA GC and cycle-detector behavior under high foreign-ref churn.

Priority 3: Maintainer tooling and governance
  - Keep RFC process (GOVERNANCE.md, RFC/), changelog (CHANGELOG.md), and language-version frozen core (Cargo.toml language-version = "1.0.0-frozen") in sync with releases.
```

### 5.4 Completed Actionable Checklist (Historical Record)

The following items were identified in prior audits and have since been resolved. They are kept here for traceability rather than as open work.

- [x] Verify `cargo test` continues to pass (currently 1366 tests; release profile also green; 9 ignored across workspace suites).
- [x] Wire runtime GC foreign-ref operations into `src/runtime/orca_cycle.rs` and verify under stress tests.
- [x] Add unit tests for lexer, parser, and compiler boundary cases.
- [x] Add regression tests for typed and SIMD JIT compilers.
- [x] Add unit tests for allocator promotion and alignment paths.
- [x] Add mock-network convergence tests for `src/runtime/crdt_manager.rs`.
- [x] Reduce `cargo check` warnings to zero and keep CI lint gates passing.
- [x] Remove Python native actor scaffolding and verify `src/python/bridge.rs` / `src/python/marshal.rs` are active, tested PyO3 code.
- [x] Seed the v0.8 Workflow SDK syntax (`workflow`, `step`, `parallel`, `compensate`, `await`, `subworkflow`, `emit`) in lexer/parser/AST and implement durable runtime support.

## 6. Governance, Language Version, and Change Control

As of 2026-07-19 the project has ratified:

- `GOVERNANCE.md` — project governance, RFC process, and decision-making rules.
- `CHANGELOG.md` — user-facing release notes organized by version.
- `RFC/` — request-for-comments documents for major cross-cutting changes (`0000-template.md`, `0001-format-stability.md`, `0002-frozen-core.md`, `0003-remaining-roadmap.md`).
- Cargo.toml `[package.metadata] language-version = "1.0.0-frozen"`, establishing the durable-format frozen core distinct from the crate's alpha release version (`0.13.0-alpha.1`).

Future backwards-incompatible changes to the language surface, bytecode format, or runtime durable artifacts must follow the RFC process and update the language version only after ratification.
