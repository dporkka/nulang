---
updated: 2026-08-09
sources:
  - PERFORMANCE_ANALYSIS.md
  - src/vm.rs:1626-1712
  - src/vm.rs:3631-3711
  - src/jit/mod.rs:530-565
  - src/runtime/crdt_reg.rs:292-330
  - src/runtime/network.rs:431-1537
tags: [performance, jit, scheduler, distribution, assessment]
---

# Performance Proposal Assessment

> Assessed 2026-08-09 against the current tree. Sources: `PERFORMANCE_ANALYSIS.md` (28 proposals across 6 tracks, written 2026-06-25), source-level verification of additional techniques (threaded dispatch, OSR, NUMA, cache-line padding, non-temporal stores, Auto-SoA). Related: [[overview/architecture-overview]], [[overview/compiler-pipeline]].

## Summary

| Category | Count |
|----------|-------|
| Already shipped / fixed | 14 of 28 |
| Partial | 4 of 28 |
| Deferred (by design) | 7 of 28 |
| Actionable gap | 5 of 28 + 4 beyond-catalog |

**Bottom line:** Phase 1 (JIT, SIMD, mailboxes, heaps, mimalloc, CRDT deltas, LSP) is fully shipped. The remaining gaps are real but none are P0 correctness blockers — they're throughput/latency optimizations for production scale.

---

## Step 1: Ground-truth verification

### Claims verified as already present

| Claim | Source | Mechanism |
|-------|--------|-----------|
| Flat `Vec<Frame>` call stack | `src/vm.rs:1626` | `frames: Vec<Frame>` with `current_frame_idx: Option<usize>` — no `Box<Frame>` churn |
| No `.leak()` in VM | `src/vm.rs` (0 matches) | All string/heap allocations route through `ActorVmCallbacks` trait |
| RGA zero-allocation `insert_at`/`delete_at` | `src/runtime/crdt_reg.rs:292-329` | `.iter().filter().nth()` — no intermediate `Vec` |
| MVRegister in-place `retain` | `src/runtime/crdt_reg.rs:128` | `self.values.retain(|(_, t)| t.counter >= ts.counter)` |
| Criterion benchmark harness | `Cargo.toml:142-146`, `benches/` (7 groups) | `criterion = "0.5"`, 7 bench groups (vm, jit, actor, dist, gc, persist, bench_main) |
| MIR temp-fusion peephole | `src/mir_lower.rs:2418` | `fuse_single_use_temps` removes single-use temp/Load pairs |
| `TailCall` opcode | `src/vm.rs:3716` | Eliminates frame on tail-position calls |

### Claims verified as absent

| Technique | Checked | Conclusion |
|-----------|---------|------------|
| Direct threaded code (computed goto) | `src/vm.rs`, `src/` (0 matches for `computed goto`, `threaded code`, `label goto`) | Not present. Rust lacks computed-goto support; the interpreter uses `match instr.opcode` (`src/vm.rs:3711`) |
| JIT OSR / deoptimization | `src/jit/` (0 matches for `OSR`, `on.stack`, `deopt`) | Not present. `PERFORMANCE_ANALYSIS.md:144` confirms: "No deoptimization or on-stack replacement beyond re-entering the interpreter at region boundaries" |
| NUMA awareness | `src/runtime/` (0 matches for `numa`, `topology`, `sched_setaffinity`) | Not present |
| Cache-line padding | `src/runtime/` (0 matches for `align(64)`, `repr(align(64))`, `CACHELINE`) | Not present |
| Non-temporal stores / prefetch | `src/jit/` (0 matches for `MOVNTDQ`, `_mm_stream`, `prefetch`) | Not present |
| Auto-SoA transform | `src/mir_lower.rs` (0 matches for `SoA`, `AoS`, `transpose`) | Not present |
| rkyv zero-copy serialization | `Cargo.toml` (no `rkyv` dep) | Hand-rolled big-endian serde in `src/runtime/network.rs` |
| Evidence-passing for effects | `src/effect_checker.rs`, `src/hir_lower.rs` | Runtime handler-stack dispatch (`Handle`/`Perform`/`Resume`/`Unwind`), not compiled to CPS |
| DST harness | `src/` (no sim framework) | Not present |
| Profile-guided optimization | workspace (0 matches for `pgo`, `PGO`, `bolt`, `BOLT`, `autofdo`) | Not present |

### Performance Analysis claims cross-checked against `PERFORMANCE_ANALYSIS.md` status table

All 14 "Shipped" entries confirmed present in the tree. The 4 "Partial" entries (4.2 linear iso, 5.1 agent unification, 5.2 agent supervision, 5.4 agent telemetry) match the current source. The 7 "Deferred" entries (1.3, 1.4, 2.5, 3.3, 3.4, 3.5, 6.3) remain absent by design.

---

## Step 2-3: Categorization & ranking

### Actionable gaps ranked by impact/effort

#### 1. Interpreter dispatch throughput (HIGH impact, LOW effort)

**Problem:** `match instr.opcode` at `src/vm.rs:3711` compiles to a dense branch table, which is good but still 2-3 cycles per dispatch. The VM step overhead includes the JIT check, debug hook check, bounds checks, and PC increment before reaching the match.

**Measurable goal:** 15-25% instruction throughput improvement on interpreter-only workloads.

**Concrete entry point:** `src/vm.rs:3631-3711`. Options:
- **Token threading:** Replicate the dispatch at the tail of each hot opcode handler instead of returning to the top. Requires restructuring `step` into a flat loop with explicit `continue` at each opcode tail (Rust-friendly, no computed goto).
- **Macro-op fusion:** Fuse adjacent common pairs (e.g., `Load` + `IAdd` + `Store` → single fused handler) at MIR level — `fuse_single_use_temps` (`src/mir_lower.rs:2418`) is a precedent.
- **Benchmark-driven:** The 7-group criterion harness in `benches/` exists; add a pure-interpreter throughput bench to measure before/after.

**Synergy:** Token threading pairs with the existing JIT tiering — cold code gets faster interpretation until the JIT takes over.

#### 2. rkyv zero-copy serialization (HIGH impact, MEDIUM effort)

**Problem:** `src/runtime/network.rs` uses hand-rolled big-endian `Packet` serde. Every cross-node message serializes/deserializes byte-by-byte. rkyv would zero-copy deserialize directly from the wire buffer.

**Measurable goal:** 2-5x reduction in cross-node message latency.

**Concrete entry point:** `src/runtime/network.rs`. The `Packet` enum is the serialization boundary. rkyv's `Archive`/`Serialize`/`Deserialize` derive macros would replace the manual `encode`/`decode` methods. Wire format change requires version negotiation (already in `src/format/`).

**Risk:** rkyv's endianness handling and format stability need verification against the frozen-format contract.

#### 3. Cache-locality & NUMA awareness (MEDIUM impact, MEDIUM effort)

**Problem:** The scheduler is thread-per-core with Chase-Lev work-stealing but does not pin threads to cores or pad cache lines. `SchedulerStats` counters track work distribution but no topology is queried.

**Measurable goal:** 10-20% actor throughput on multi-socket machines.

**Concrete entry point:** `src/runtime/scheduler.rs`. Two changes:
1. `#[repr(align(64))]` on `Worker`, `Mailbox`, and `Scheduler` structs to prevent false sharing.
2. Core affinity via `core_affinity` crate or `libc::sched_setaffinity` in `Runtime::new_sharded`.

**Synergy:** The thread-per-core architecture already maps naturally to core pinning.

#### 4. Deterministic simulation testing (HIGH correctness impact, HIGH effort)

**Problem:** No DST harness. Distributed bugs (split-brain, message reordering, CRDT divergence) surface only in rare production conditions.

**Measurable goal:** Catch 90% of distributed-edge-case bugs in CI before they reach production.

**Concrete entry point:** New crate or module. FoundationDB's approach: single-threaded simulator that controls message ordering, faults, and time. Inject a `SimNetwork` impl of the network trait, a `SimClock`, and a `SimScheduler` that serializes all actor execution into a deterministic order. Run existing actor/dist tests under the sim.

#### 5. JIT on-stack replacement (MEDIUM impact, HIGH effort)

**Problem:** `PERFORMANCE_ANALYSIS.md:144` — "No deoptimization or on-stack replacement beyond re-entering the interpreter at region boundaries." A hot loop body compiles, but a function that spends its first 500 instructions in cold setup code and then enters a hot loop never tiers up within the loop.

**Concrete entry point:** `src/jit/mod.rs:550` (`find_compilable_region`). After region compilation, insert a back-edge counter that triggers deoptimization and re-entry into the JIT at the loop header. Cranelift supports stack maps for deopt, but this is ~4-6 weeks of work.

#### 6. Evidence-passing style for effects (MEDIUM impact, HIGH effort)

**Problem:** `Handle`/`Perform`/`Resume`/`Unwind` opcodes walk the runtime handler stack. Evidence-passing would compile effect handlers to continuation-passing style, eliminating the handler-stack walk.

**Concrete entry point:** `src/effect_checker.rs` + `src/hir_lower.rs`. Requires a compiler pass that inlines effect handlers at static call sites. Worth pursuing when the effect system stabilizes.

#### 7. Wasmtime sandboxed tool execution (security impact, MEDIUM effort)

**Problem:** Actor tools (AI tool-calling, FFI) run in-process. A buggy tool can corrupt the runtime.

**Concrete entry point:** Wire `wasmtime` (already a dep behind `wasm-backend` feature) to execute tool functions in isolated WASM modules. Make `wasm-backend` default-first, then gate tool execution behind it.

#### 8. Non-temporal stores in SIMD (LOW impact, LOW effort)

**Problem:** SIMD-compiled array operations use regular stores, polluting cache.

**Concrete entry point:** `src/jit/simd_compiler.rs`. Use Cranelift's `store` with `MemFlags::new().set_not_temporal()` for the SIMD body stores. Impact is small for typical actor workloads (small heaps), but measurable for array-heavy computation.

---

## Step 4: Prioritized recommendations

### Immediate (next 2-4 weeks, 1 engineer)

| # | Item | Effort | Impact |
|---|------|--------|--------|
| 1 | Add `#[repr(align(64))]` to hot scheduler/mailbox structs | 1 day | Modest |
| 2 | Thread-pin workers to cores in `Runtime::new_sharded` | 2 days | Modest |
| 3 | Token-threading for top-10 interpreter opcodes | 1 week | High |
| 4 | Add a pure-interpreter throughput criterion bench | 1 day | Enables measurement |

### Short-term (4-8 weeks)

| # | Item | Effort | Impact |
|---|------|--------|--------|
| 5 | rkyv zero-copy wire serialization | 3 weeks | High |
| 6 | Make `wasm-backend` a default feature; gate tool sandbox | 2 weeks | Security |

### Medium-term (8-16 weeks)

| # | Item | Effort | Impact |
|---|------|--------|--------|
| 7 | Deterministic simulation testing harness | 7 weeks | Correctness |
| 8 | Evidence-passing for effects | 5 weeks | Medium |

### Not recommended at this stage

- **Direct threaded code:** Rust has no computed-goto support. The `match`-based dispatch compiles to a dense jump table — the gap vs. threaded code is small (~5-10%) and not worth unsafe tricks.
- **NUMA-aware memory allocation:** Premature for a language that hasn't shipped v1.0. Core pinning + cache-line padding covers 80% of the benefit.
- **JIT OSR:** High complexity for modest gain given the existing region-based tiering already captures hot loops.
- **MLIR, Raft, content-addressable bytecode, io_uring:** Deferred per original analysis — still correct.

## Implementation status (2026-08-09)

Shipped from the "Immediate" tier in a follow-up session:

1. **Cache-line padding** — `#[repr(align(64))]` added to `Scheduler` (`src/runtime/scheduler.rs`), `SchedulerStatsInternal`, and `Mailbox` (`src/runtime/mailbox.rs`). Prevents false sharing across worker threads touching the lock-free deques/injectors and per-actor mailboxes.
2. **Core pinning (opt-in)** — `pin_current_thread_to_cpu` (`src/runtime/scheduler.rs`, Linux-only `sched_setaffinity`, no-op elsewhere) + `core_pinning_enabled()`. Wired into the shard spawn in `src/main.rs`: when `NULANG_PIN_CORES` is set, shard i pins to logical CPU i. Default off to avoid regressions on hybrid P+E topologies.
3. **JIT hot-counter hashing** — the per-instruction `HashMap.entry().or_insert()` in `record_and_check_hot` (a SipHash lookup + possible alloc on *every* interpreted dispatch with the JIT enabled) was the dominant interpreter cost. Swapped `compiled`, `hot_counters`, `tier2_counters`, `typed_regions` to `rustc_hash::{FxHashMap, FxHashSet}` (`src/jit/mod.rs`) — O(1) integer-key hashing, no per-instruction allocation.
4. **Interpreter throughput benchmark** — added `VM::new_without_jit()` (`src/vm.rs`, JIT session set to `None`) and an `interp` criterion group (`benches/interp_bench.rs`) that measures pure-interpreter dispatch cost. Registered in `benches/bench_main.rs`. This is the measurement baseline for any future dispatch optimization.

### Deferred

- **Full token-threading of the interpreter dispatch** (`src/vm.rs:3711` `match`): assessed and deferred. It is a ~1-week rewrite of the hottest, most safety-critical code (135 opcodes + JIT integration + effects/actors) with high regression risk. The hot-counter FxHashMap swap (#3) captured the largest safe slice of per-instruction interpreter overhead, and the `interp` bench now quantifies the remaining dispatch cost if/when it's pursued.

**Note on the test suite:** `fuzz::tests::fuzz_differential_quick` is *intermittently* flaky on a pre-existing AOT backend bug: `"hello" + 2 + 3` on a string is AOT-compiled as integer arithmetic, returning an ASLR-dependent garbage register. The fuzzer only flags it when that garbage value happens to carry an int-comparable tag, so it surfaces on some runs and not others (verified via `git stash` that the underlying bug reproduces at HEAD). Unrelated to these changes.

---

## Step 5: Verification

- Every "already present" claim cites an exact source line verified via `read` or `grep` in this session.
- Every "absent" claim cites the grep pattern used and the 0-match result.
- The `PERFORMANCE_ANALYSIS.md` status table was cross-checked against the current tree — no stale claims found.
- Benchmarks exist (`benches/`, 7 groups) but do not cover interpreter-only throughput — recommendation #1 adds that.
