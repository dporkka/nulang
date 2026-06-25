# Nulang Performance & Architecture Deep-Dive Analysis
## 28 Proposals Across 6 Tracks

**Verdict:** 20 of 28 proposals are high-value and should be pursued. 8 deferred.

---

## Track 1: Native Compilation

### 1.1 Cranelift JIT Backend
| Criterion | Assessment |
|-----------|------------|
| **Impact** | Eliminates interpreter overhead (10-100x speedup for hot loops) |
| **Feasibility** | Cranelift is mature (wasmtime, rustc) |
| **Priority** | **P0 — Foundation for everything else** |
| **Effort** | 4-6 weeks |

**Recommendation:** **DO FIRST.** Highest-leverage single change. JIT can tier: interpreter for cold code, JIT for hot loops.

### 1.2 Static Type Guard Stripping
| Criterion | Assessment |
|-----------|------------|
| **Impact** | Eliminates ~30% of runtime overhead in numeric loops |
| **Priority** | **P0 — Bundle with 1.1** |
| **Effort** | 1-2 weeks |

**Recommendation:** **Bundle with 1.1.** Free once Cranelift backend exists.

### 1.3 Linear Scan Register Allocation
**Recommendation:** **DEFER.** Cranelift's regalloc2 is already excellent.

### 1.4 MLIR Dialect Pipeline
**Recommendation:** **DEFER to Year 3+.** Architecturally elegant but premature.

### 1.5 SIMD Auto-Vectorization
**Recommendation:** **Pursue after 1.1.** High-value for AI inference, text processing.

**Track 1 Summary: Do 1.1 + 1.2 first. Defer 1.3. Research 1.4. Do 1.5 after 1.1.**

---

## Track 2: Memory Management & Runtime Concurrency

### 2.1 Lock-Free CAS MPSC Actor Mailboxes
| Criterion | Assessment |
|-----------|------------|
| **Impact** | Eliminates mutex contention (up to 10x throughput improvement) |
| **Priority** | **P0 — Critical for multi-core scaling** |
| **Effort** | 2-3 weeks |

**Recommendation:** **DO IMMEDIATELY.** Highest-ROI change in Track 2. Use `crossbeam::queue::ArrayQueue`.

### 2.2 Dual-Region Actor Heaps (LOS Split)
**Recommendation:** **DO.** Natural evolution of existing `bumpalo`-based heap.

### 2.3 Global Memory Arena via mimalloc
**Recommendation:** **DO RIGHT NOW.** Literally a one-line change. Instant 10-20% win.

### 2.4 Static Escape Analysis for Stack Allocation
**Recommendation:** **DO after 1.1.** Key to making ORCA GC effectively disappear.

### 2.5 Actor Arenas (Allocation Pooling)
**Recommendation:** **DEFER.** Power-user feature; covered by 2.2 and 2.4.

### 2.6 Cache-Locality Aware Heterogeneous Scheduling
**Recommendation:** **Pursue after 2.1.** Important for Intel/ARM hybrid CPUs.

**Track 2 Summary: Do 2.1, 2.3 immediately. Do 2.2, 2.4 next. Defer 2.5. Do 2.6 after 2.1.**

---

## Track 3: Distributed Mesh & Consensus

### 3.1 Zero-Copy Serialization via rkyv
**Recommendation:** **DO.** Eliminates 50%+ of distributed message latency.

### 3.2 Delta-State & Op-Based CRDT Replication (CmRDT)
**Recommendation:** **DO.** 10-100x bandwidth reduction for large CRDTs.

### 3.3 Kernel-Bypass Networking (io_uring & RDMA)
**Recommendation:** **DEFER.** Platform-specific; significant complexity increase.

### 3.4 Native Raft Consensus Engine
**Recommendation:** **DEFER to Phase 4.** CRDTs cover 80% of distributed state needs.

### 3.5 Content-Addressable Bytecode Functions
**Recommendation:** **DEFER to Year 3+.** Unison-style paradigm shift; enormous complexity.

**Track 3 Summary: Do 3.1, 3.2. Defer 3.3, 3.4, 3.5.**

---

## Track 4: Type System Synergy

### 4.1 Evidence-Passing Style / Capability Inlining
**Recommendation:** **DO after 1.1.** Key optimization for algebraic effects performance.

### 4.2 Linear Type Move Semantics for iso Capabilities
**Recommendation:** **DO IMMEDIATELY.** Type system change for zero-cost actor messaging.

### 4.3 Typestate Analysis for Temporal Contracts
**Recommendation:** **Pursue after core type system stabilization.** Major differentiator.

### 4.4 Implicit Effect Return Fallbacks
**Recommendation:** **DO.** Trivial AST transformation; good ergonomics payoff.

**Track 4 Summary: Do 4.2 immediately. Do 4.1 after Cranelift. Do 4.3 after type system stabilization. Do 4.4 anytime.**

---

## Track 5: AI Agent Native Infrastructure

### 5.1 Complete Unification of Actor and Agent Primitives
**Recommendation:** **DO.** Aligns with v2.0 spec: agents become actors with `capability llm`.

### 5.2 Agent-Aware Supervision Trees
**Recommendation:** **DO.** Essential for production AI systems. LLM APIs are unreliable.

### 5.3 Sandboxed Tool Execution via Embedded Wasmtime
**Recommendation:** **DO IMMEDIATELY.** Security requirement, not a feature.

### 5.4 Agent Telemetry Monitors
**Recommendation:** **DO after core AI runtime.** Important for cost management.

**Track 5 Summary: Do 5.1, 5.3 immediately. Do 5.2 next. Do 5.4 after core AI runtime.**

---

## Track 6: Systems Observability & Verification

### 6.1 Real-Time LSP Inlay Hints
**Recommendation:** **DO IMMEDIATELY.** Most impactful developer experience feature.

### 6.2 Deterministic Simulation Testing (DST)
**Recommendation:** **DO.** How FoundationDB achieved legendary reliability.

### 6.3 Causal Profiling Infrastructure
**Recommendation:** **DEFER.** Research tool; profilers + DST cover most needs.

### 6.4 Visual Actor Topology Dashboard
**Recommendation:** **DO as a side project.** Low-risk, high-fun, great for demos.

**Track 6 Summary: Do 6.1 immediately. Do 6.2 next. Do 6.4 as side project. Defer 6.3.**

---

## Revised Implementation Matrix

### Phase 1: Native Speed Foundation (Weeks 1-8)
| # | Proposal | Effort | Depends On |
|---|----------|--------|------------|
| 2.3 | mimalloc global allocator | 1 day | — |
| 2.1 | Lock-free CAS MPSC mailboxes | 3 weeks | — |
| 1.1 | Cranelift JIT Backend | 6 weeks | — |
| 1.2 | Type guard stripping / unboxing | 2 weeks | 1.1 |
| 4.2 | Linear type moves for iso | 3 weeks | — |
| 6.1 | LSP Inlay Hints | 4 weeks | — |

### Phase 2: Memory & Concurrency (Weeks 9-16)
| # | Proposal | Effort | Depends On |
|---|----------|--------|------------|
| 2.2 | Dual-region actor heaps | 3 weeks | — |
| 2.4 | Static escape analysis | 4 weeks | 1.1 |
| 2.6 | Cache-locality scheduling | 5 weeks | 2.1 |
| 4.1 | Evidence-passing style | 5 weeks | 1.1 |
| 1.5 | SIMD auto-vectorization | 4 weeks | 1.1 |
| 6.2 | Deterministic Simulation Testing | 7 weeks | — |

### Phase 3: Distributed & AI (Weeks 17-28)
| # | Proposal | Effort | Depends On |
|---|----------|--------|------------|
| 3.1 | rkyv zero-copy serialization | 3 weeks | — |
| 3.2 | Delta-state CRDT replication | 5 weeks | 3.1 |
| 5.1 | Unify actor/agent primitives | 2 weeks | — |
| 5.3 | Wasmtime sandboxed execution | 4 weeks | — |
| 5.2 | Agent-aware supervision | 3 weeks | 5.1 |
| 4.3 | Typestate analysis | 7 weeks | 4.2 |
| 6.4 | TUI topology dashboard | 3 weeks | — |

### Phase 4: Advanced (Weeks 29-40+)
| # | Proposal | Effort | Depends On |
|---|----------|--------|------------|
| 3.4 | Native Raft consensus | 10 weeks | 3.2 |
| 4.4 | Implicit effect returns | 1 week | — |
| 5.4 | Agent telemetry monitors | 3 weeks | 5.2 |
| 1.3 | Linear scan regalloc (if needed) | 4 weeks | 1.1 |
| 2.5 | Actor arenas (if needed) | 4 weeks | 2.2 |

### Deferred (Research / Year 3+)
- 1.3: Linear scan regalloc (use Cranelift's default)
- 1.4: MLIR dialect pipeline
- 2.5: Actor arenas
- 3.3: io_uring / RDMA
- 3.5: Content-addressable bytecode
- 6.3: Causal profiling

---

## Critical Path Analysis

```
Cranelift JIT (1.1, 6w) → Type guard stripping (1.2, 2w) → Evidence-passing (4.1, 5w) → Escape analysis (2.4, 4w)
```

**Total: 17 weeks for full native-speed + zero-cost effects + stack allocation.**

Parallel tracks:
- **Type system:** Linear moves (4.2) can proceed in parallel with Cranelift (1.1)
- **Memory:** Lock-free mailboxes (2.1) + mimalloc (2.3) are independent
- **Distributed:** rkyv (3.1) + delta CRDTs (3.2) are independent of native compilation
- **Observability:** LSP (6.1) is independent; DST (6.2) needs stable runtime

**Total timeline to Phase 3 completion: ~28 weeks (7 months) with 2 engineers.**

---

## Risk Register

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Cranelift JIT has codegen bugs | Medium | High | Maintain interpreter as fallback |
| Lock-free mailbox ABA bug | Low | Critical | Use `crossbeam::epoch` |
| Escape analysis unsoundness | Medium | Critical | Conservative analysis; fall back to heap |
| rkyv endianness across platforms | Medium | High | Standardize little-endian wire format |
| LSP performance on large files | Medium | Medium | Incremental type checking; caching |
| Wasmtime sandbox escape | Low | Critical | Keep Wasmtime updated; minimal WASI |

---

## Final Verdict

**20 of 28 proposals should be pursued.** The deferred 8 are premature optimizations, research projects, or platform-specific. The 20 recommended proposals form a coherent, phased roadmap that transforms Nulang from an interpreted research language into a production-grade, native-speed distributed actor platform.

**Sequence matters: JIT first, then memory, then distributed, then AI, then advanced type system.**
