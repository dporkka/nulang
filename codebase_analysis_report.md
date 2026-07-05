# Nulang Codebase Analysis Report

**Date:** 2026-07-05  
**Commit context:** Current working tree  
**Test status:** `cargo test` passes with 589 tests  

This report evaluates the Nulang compiler, VM, runtime, and supporting design documents. It recognizes the fixes that have already landed and documents the remaining architectural gaps, coverage holes, and technical debt that should be addressed before the next milestone.

---

## 1. Architectural & Design Review

### 1.1 Compiler Pipeline

The compiler pipeline is orchestrated by `run_source` in `src/main.rs:174-195` and reused by the REPL (`src/repl.rs`) and LSP (`src/lsp/mod.rs`):

```rust
// src/main.rs — conceptual pipeline ordering
let tokens = Lexer::new(source).lex()?;
let ast = Parser::new(tokens).parse_module()?;
let ty = TypeChecker::new().check_module(&ast)?;
let effects = EffectChecker::new().infer_effects(&ast)?;
let caps = CapabilityAnalyzer::new().infer_cap(&ast)?;
let code = Compiler::new("main").compile_module(&ast)?;
let mut vm = VM::new();
vm.load_module(code)?;
vm.run()
```

All phases are wired together. `src/escape_analysis.rs` exists and has unit tests, but it is intentionally not part of the production pipeline; it should only be re-integrated alongside a correct nursery implementation.

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
* **`src/escape_analysis.rs` is dead code.** Keeping it outside the pipeline means heap allocation elision is not yet realized.

---

## 3. Code Quality & Rust Idioms

### 3.1 Resolved Code-Quality Issues

The following items from earlier reports are now idiomatic and should not be re-reported as active bugs:

| File | Old issue | Current state |
|------|-----------|---------------|
| `src/compiler.rs` | `unsafe { transmute }` for `Self` opcode | Uses `OpCode::SelfOp` |
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

`cargo check` currently reports approximately 79 warnings. The majority are:

* Unused constants in `src/python/marshal.rs` and `src/python/native_actor.rs`.
* Dead fields such as `NativeActorPool.size` and `NativeActorPool.interpreters`.
* Unused imports and helper functions across the parser and runtime.

These warnings do not break the build, but they obscure new warnings and indicate partially abandoned scaffolding (especially in `src/python/` after the v0.14 Python removal). A cleanup pass is recommended:

```diff
- pub struct NativeActorPool {
-     size: usize,
-     interpreters: Mutex<Vec<()>>, // Placeholder for Python interpreters
- }
+ // NativeActorPool removed — Python interop was dropped in v0.14.
```

---

## 4. Verification & Test Coverage

### 4.1 Current Test Inventory

* `src/integration_tests.rs`: ~52 end-to-end pipeline tests.
* `src/stress_tests.rs`: 10 actor/supervision/scheduler chaos tests.
* `src/runtime/tests.rs`: 110 runtime unit tests.
* `src/jit/tests.rs`: 18 JIT tests.
* Inline `mod tests` in lexer, parser, compiler, typechecker, effect checker, escape analysis, and others.

### 4.2 Coverage Gaps

1. **Dedicated frontend unit tests.** `src/lexer.rs`, `src/parser.rs`, and `src/compiler.rs` rely heavily on integration tests. Boundary cases such as invalid escape sequences, malformed match arms, and all `OpCode` emission paths are not covered at the unit level.

2. **Typed and SIMD JIT compilers.** `src/jit/typed_compiler.rs` and `src/jit/simd_compiler.rs` lack targeted tests beyond the generic JIT suite. SIMD loops, NaN-tag stripping, and typed-to-untyped fallback paths need isolated regression tests.

3. **Dual-heap allocator.** `src/runtime/dual_heap.rs` nursery promotion, pointer alignment, and collection triggers are not exercised by dedicated unit tests.

4. **CRDT manager replication.** `src/runtime/crdt_manager.rs` has no mock-network tests for convergence under packet loss or partition healing.

5. **Cycle-detector foreign-ref graph.** Although `src/runtime/orca_cycle.rs` supports `register_foreign_ref` / `remove_foreign_ref` and `process_gc_ops` calls `set_local_actors`, the runtime's GC paths (`OrcaGc::send_ref_to`, `OrcaGc::receive_ref`, `OrcaCoordinator::deliver_pending_ops`) do **not** populate the detector's foreign-ref graph. The cycle detector therefore operates on an empty graph in production and only sees edges in unit tests.

Recommended additions:

```rust
// src/runtime/tests.rs — example cycle-detector wiring test
#[test]
fn test_gc_updates_cycle_detector_graph() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(behavior_a(), vec![]);
    let b = rt.spawn_actor(behavior_b(), vec![]);
    // Simulate a message carrying a reference from a to b.
    rt.send_message_by_id(a, b, Message { behavior_id: 0, payload: Value::nil() });
    rt.process_gc_ops();
    assert!(!rt.cycle_detector.foreign_ref_graph.is_empty());
}
```

---

## 5. Specification Alignment

### 5.1 Implemented Specifications

`SPEC.md` and the core chapters of `SPEC2.md` are largely implemented: lexer, parser, HM type inference, effect rows, capabilities, register VM, algebraic effects at runtime, actors, supervision, persistence, and distribution primitives.

### 5.2 Design Documents Without Implementation

Four design specifications describe subsystems that are not yet present in `src/`:

1. **`DESIGN_AI_SDK.md`** — Agents, tools, memory, and multi-agent orchestration. The parser accepts an `agent` keyword, and `src/typechecker.rs` stubs `Type::Agent` with fresh variables, but there is no interpreter support for LLM calls, tool binding, or agent lifecycle management.

2. **`DESIGN_WEB_FRAMEWORK.md`** — Phoenix-style endpoints, routers, controllers, channels, and LiveView. No files implement this framework.

3. **`DESIGN_WORKFLOW_SDK.md`** — Durable actors, sagas, timers-as-signals, and workflow replay. Not implemented.

4. **`DESIGN_PACKAGE_MANAGER.md` and `DESIGN_CLOUD_PLATFORM.md`** — Design documents exist, but no package manager or cloud control-plane code is present.

### 5.3 Recommended Roadmap

```text
Priority 1: Finish production wiring for the cycle detector
  - src/runtime/gc.rs: call cycle_detector.register_foreign_ref / remove_foreign_ref
  - src/runtime/orca_cycle.rs: verify incremental_detect runs with real edges

Priority 2: Increase unit-test coverage
  - src/lexer.rs, src/parser.rs, src/compiler.rs: dedicated boundary tests
  - src/jit/typed_compiler.rs, src/jit/simd_compiler.rs: isolated regression tests
  - src/runtime/dual_heap.rs: nursery/tenured promotion tests
  - src/runtime/crdt_manager.rs: mock-network convergence tests

Priority 3: Reduce compiler warnings
  - Remove or use dead Python-interop scaffolding in src/python/
  - Address unused constants and dead fields flagged by cargo

Priority 4: Begin specification-driven subsystems
  - DESIGN_AI_SDK.md
  - DESIGN_WEB_FRAMEWORK.md
  - DESIGN_WORKFLOW_SDK.md
  - DESIGN_PACKAGE_MANAGER.md / DESIGN_CLOUD_PLATFORM.md
```

### 5.4 Actionable Checklist

- [ ] Verify `cargo test` continues to pass (currently 589 tests).
- [ ] Wire runtime GC foreign-ref operations into `src/runtime/orca_cycle.rs`.
- [ ] Add unit tests for `src/lexer.rs`, `src/parser.rs`, and `src/compiler.rs`.
- [ ] Add regression tests for `src/jit/typed_compiler.rs` and `src/jit/simd_compiler.rs`.
- [ ] Add unit tests for `src/runtime/dual_heap.rs` promotion and alignment.
- [ ] Add mock-network convergence tests for `src/runtime/crdt_manager.rs`.
- [ ] Reduce `cargo check` warnings from ~79 toward zero.
- [ ] Remove or complete `src/python/native_actor.rs` scaffolding.
- [ ] Decide whether to implement or defer each design-document subsystem.
