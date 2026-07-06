# Nulang 50-Year Architecture Review — Runtime & Distributed Systems

> **Status:** Architecture review for the Nulang actor runtime, scheduler, GC, supervision, distribution, persistence, CRDTs, and cloud/workflow targets.  
> **Date:** 2026-07-06

---

## 1. Updated Runtime Architecture

### 1.1 Component narrative

Nulang’s runtime is currently a **single-threaded synchronous coordinator** (`Runtime` in `src/runtime/mod.rs:66`) that owns all actor state, scheduling, GC, networking, and persistence. The compiler pipeline is not the focus here; we treat the VM as the execution backend that the runtime drives through two object-safe callback traits (`ActorVmCallbacks`, `DistributedVmCallbacks`) defined in `src/vm.rs:44` and `src/vm.rs:87`.

At the center of the runtime is the `Runtime` god-object (`src/runtime/mod.rs:66`):

- `actors: HashMap<u64, Actor>` — all live actors indexed by a bare `u64` ID produced by `fresh_actor_id()` (`src/runtime/mod.rs:58`).
- `supervisors: HashMap<u64, Supervisor>` — supervision-tree metadata keyed by supervisor actor ID.
- `scheduler: Scheduler` — a Chase-Lev work-stealing structure with a global `Injector` and per-worker deques (`src/runtime/scheduler.rs:84`).
- `coordinator: OrcaCoordinator` — routes cross-actor reference-count operations.
- `cycle_detector: CycleDetector` — centralized, intra-node-only ORCA cycle detector.
- `transport`, `cluster`, `resolver`, `crdt_manager`, `timer_wheel`, `registry`, `process_groups`, `persistence` — distributed and auxiliary subsystems.

**Actor lifecycle (current):**

1. **Spawn** — `Runtime::spawn_actor_with_models` (`src/runtime/mod.rs:148`) calls `fresh_actor_id()`, constructs an `Actor` with a 64 KB `ActorHeap` and an `OrcaGc`, initializes `state_data`, and enqueues the actor.
2. **Schedule** — `Runtime::run_scheduler` (`src/runtime/mod.rs:337`) dequeues actor IDs and calls `step_actor`. `step_actor` pops one message, dispatches either a native Rust handler or a bytecode handler, journals the message if the actor is persistent, checkpoints durable fields, increments `reduction_count`, and requeues if the mailbox is non-empty and the actor has not yielded.
3. **Send** — `Runtime::send_message_by_id` (`src/runtime/mod.rs:215`) pushes a `Message` into the target mailbox and, for every pointer argument, calls `OrcaGc::send_ref_to` to increment `foreign_count` and submits a `ForeignRefOp` to the coordinator.
4. **GC** — `Runtime::process_gc_ops` (`src/runtime/mod.rs:265`) drains pending `ForeignRefOp`s, applies them to target actors’ GCs, removes edges from the cycle detector, and optionally runs an incremental cycle-detection pass.
5. **Fault** — `Runtime::handle_actor_exit` (`src/runtime/mod.rs:647`) unregisters names, leaves process groups, sends DOWN messages to monitors, propagates exit signals over links, and delegates to `Supervisor::handle_exit`.

**Important boundary:** the runtime is single-threaded. The `Scheduler` is built from `crossbeam::deque` and is *designed* for M:N threading, but `Runtime::run_scheduler` runs one `step_actor` at a time on the calling thread. The `Scheduler::run_worker` API exists (`src/runtime/scheduler.rs:257`) but is not wired into the runtime loop.

### 1.2 What the architecture hides

- **Capability tracking is compile-time only.** `CapChk`/`CapUp`/`CapDown`/`CapSend` opcodes in the VM are no-ops (`AGENTS.md:33`). The capability lattice is checked before bytecode generation; at runtime the VM simply copies values.
- **Effects are runtime-resolved.** The VM’s `handler_stack: Vec<HandlerFrame>` (`src/vm.rs:548`) implements `Handle`, `Perform`, `Resume`, and `Unwind`. Static effect rows are checked by `EffectChecker`; runtime only sees the four opcodes.
- **NaN-boxed values.** `Value` (`src/vm.rs:199`) uses quiet-NaN bits for type tags. Cross-actor pointer passing is ORCA-managed; non-pointer values are copied by value.
- **Persistent actors are MVP.** `Runtime::checkpoint_actor` (`src/runtime/mod.rs:467`) snapshots fields whose `StateModel` is `Durable`, `EventSourced`, or `Crdt`. `recover_actor` (`src/runtime/mod.rs:542`) loads the latest snapshot and replays journal entries with higher sequence numbers.

---

## 2. Updated Distributed Runtime

The current distributed runtime (`src/runtime/network.rs`, `cluster.rs`, `distributed.rs`, `crdt_manager.rs`) is a functional proof-of-concept: TCP transport, heartbeat/gossip membership, location-transparent addressing, and CRDT synchronization. For a 50-year-relevant architecture it must evolve from “messages can cross the network” into a **production distributed operating system** with placement, persistence, retries, observability, and autoscaling.

### 2.1 Actors across nodes

**Current state.** `ActorAddress` (`src/runtime/distributed.rs:81`) is either `Local { actor_id }` or `Remote { node_id, actor_id }`. `send_distributed` resolves the address and either calls `Runtime::send_message` or builds a `Packet::ActorMessage` and hands it to `NetworkTransport::send` (`src/runtime/distributed.rs:646`). A remote actor cache (`RemoteActorCache`, `src/runtime/distributed.rs:156`) stores up to 10,000 recently-contacted remote actors.

**What must change for production:**

1. **Actor identity must survive node changes.** Today actor IDs are bare `u64`s generated by a local `AtomicU64` (`src/runtime/mod.rs:55`). A production system needs globally-unique actor IDs (e.g., 128-bit composite IDs: node-id epoch + sequence) so that an actor migrated from node A to node B remains addressable.
2. **Mailbox semantics must be explicit.** The local mailbox is unbounded `crossbeam::SegQueue` (`src/runtime/mailbox.rs:43`). Across nodes, at-least-once delivery is the realistic default; exactly-once requires idempotency keys or durable journaling on both sides.
3. **Behavior resolution must not be a placeholder.** `send_distributed` currently sends `behavior_id = 0` for remote targets with the comment *“behavior_id placeholder — resolved on remote side”* (`src/runtime/distributed.rs:665`). The protocol should carry a stable behavior identifier (hash or index) and a fallback name.
4. **Backpressure and flow control.** The TCP sender thread enqueues on a bounded `mpsc::sync_channel(1024)` (`src/runtime/network.rs:587`) and silently drops on overflow (`src/runtime/network.rs:700-703`). Production needs explicit backpressure: NACKs, credit-based flow control, or bounded mailbox-aware routing.
5. **Serialization must be versioned.** The NUL0 wire protocol (`src/runtime/network.rs:17`) has a magic and a type discriminant but no schema/version field. Long-term evolution requires a versioned envelope and a canonical serialization format (e.g., Cap’n Proto, flatbuffers, or a self-describing binary schema).
6. **Security.** TCP is plaintext. Production needs TLS or WireGuard between nodes, plus authentication of node identity.

### 2.2 Supervision in a distributed system

**Current state.** Supervision is purely local. `Supervisor` (`src/runtime/supervisor.rs:108`) stores child specs and restart history; `handle_exit` restarts children on the same node.

**What must change:**

- **Supervisor groups must span nodes.** A supervisor should declare that its children live on a specific node, rack, or region. On child failure, the supervisor decides whether to restart locally, migrate, or spawn on a different node.
- **Restart counters must be cluster-aware.** `restart_history` is a `Vec<(String, Instant)>` in local memory (`src/runtime/supervisor.rs:120`). After a node crash and restart, history is lost. Persistent supervision state belongs in the event journal or a replicated metadata store.
- **Cascading shutdown must respect partitions.** `handle_supervisor_parent_exit` (`src/runtime/mod.rs:729`) recursively shuts down supervisors. In a partition, this can cause split-brain mass shutdown. Distributed supervision needs a failure-detector integration.

### 2.3 Durable workflows, retries, and timers

**Current state.** Timers are in-memory `TimerWheel` (`src/runtime/timer.rs:83`) backed by `BinaryHeap` and `RwLock`. They are not durable. Persistence has `MemoryStore`, `JsonFileStore`, and `SqliteStore` (`src/runtime/persistence.rs`), but only snapshots and message journals are implemented; there is no workflow engine.

**Target architecture:**

- **Workflow = actor + event journal.** A workflow actor writes every significant operation to an append-only journal. On recovery it replays events to reconstruct state.
- **Activities are separate worker tasks.** Long-running or external side effects happen in activities with their own retry policy, heartbeats, and timeouts. The workflow orchestrator itself never performs side effects.
- **Durable timers.** `Timer.sleep(24h)` must be persisted. The current `TimerWheel` must be backed by the event journal so that timers survive process restarts.
- **Sagas.** Multi-step transactions with compensating actions need to be first-class runtime constructs, not just library code.
- **Exactly-once processing.** Requires deduplication of journal entries (sequence numbers, deterministic IDs) and idempotent activity execution.

**Gap:** Nulang has the persistence trait and journal entries, but no workflow scheduler, activity worker pool, or durable timer service.

### 2.4 CRDTs, event sourcing, and durable workflows: how they coexist

Nulang currently conflates three distinct consistency models under the `StateModel` enum (`src/runtime/persistence.rs:16`):

| Model | Semantics | Best For | Current Implementation |
|-------|-----------|----------|------------------------|
| `Local` | Ephemeral | Caches, scratch state | In-memory only |
| `Durable` | Snapshot + journal | Single-actor state | Snapshot + message journal |
| `EventSourced` | Deterministic replay | Audit logs, workflows | MVP: increments integer counters on `emit_event` |
| `Crdt` | Merge across nodes | Shared distributed state | `CrdtManager` + `CrdtSync` packets |

**Recommended coexistence:**

- **CRDTs** are for *shared, eventually consistent* state: counters, sets, registers, collaborative sequences. They belong in a replicated data layer (`CrdtManager`) and should be exposed to actors as `crdt` state fields. The current 8 CRDTs (`GCounter`, `PNCounter`, `GSet`, `ORSet`, `AWORSet`, `LWWRegister`, `MVRegister`, `RGA`) are a solid foundation.
- **Event sourcing** is for *append-only, deterministic* history: workflow steps, audit trails, financial ledgers. It belongs in a journal store and should be queryable by sequence/time.
- **Durable actor state** is for *single-actor, crash-recoverable* mutable state: actor fields that must survive restart but do not need cross-node merge. It belongs in snapshot + journal.
- **Workflows** orchestrate activities, timers, and signals. They are event-sourced actors with a special lifecycle.

The runtime should make the choice explicit in the type system/AST and provide separate storage backends optimized for each.

### 2.5 Scheduling and placement

**Current state.** The scheduler is a `Scheduler` object with Chase-Lev deques but the runtime uses only the single-threaded `dequeue` path. There is no placement policy: all actors are spawned on the local node.

**Target architecture:**

- **Local scheduler:** true M:N with worker threads pinned to cores, actor affinity, and reduction quotas. `Scheduler::run_worker` should be wired into a thread pool.
- **Global scheduler (control plane):** decides which node hosts which actor based on load, locality, constraints, and cost.
- **Placement constraints:** actors should declare requirements (region, GPU, co-location with another actor, persistence).
- **Migration:** stateful actors must be migratable. This requires freezing the actor, serializing mailbox + heap + state, transferring, and resuming with the same actor ID. `VM::Migrate` opcode (`src/vm.rs:56`) currently just records a request in `pending_migrations`.
- **Hibernation:** idle persistent actors can be serialized to object storage and resumed on message arrival.

### 2.6 Node discovery and cluster membership

**Current state.** `ClusterState` (`src/runtime/cluster.rs:208`) uses heartbeats, suspicion timeouts, and gossip. `NodeId` is derived from the socket address hash (`src/runtime/cluster.rs:68`). Gossip fanout is 2, target selection is deterministic “first N healthy” (`src/runtime/cluster.rs:605`).

**What must change:**

- **Node IDs must be stable across restarts.** Address-derived IDs mean a restarted node at the same address gets the same ID, but the ID should also incorporate an incarnation to distinguish old and new processes.
- **Gossip needs randomization.** Deterministic first-N selection creates hot spots and poor convergence in large clusters.
- **Failure detection needs SWIM or similar.** The current heartbeat + suspicion timeout is acceptable for small clusters but does not scale. SWIM-style protocol with indirect pings and suspicion dissemination is the production baseline.
- **Metadata propagation.** `NodeInfo.metadata` (`src/runtime/cluster.rs:117`) exists but is not gossiped. Region, rack, version, and capacity must propagate.
- **Quorum / leader election.** For strongly consistent operations (e.g., actor ID allocation, global schema changes) the runtime needs a consensus primitive or integration with an existing one.

### 2.7 Distributed tracing and persistence

**Current state.** There is no distributed tracing in the runtime. Resolver stats (`ResolverStats`, `src/runtime/distributed.rs:284`) and scheduler stats (`SchedulerStats`, `src/runtime/scheduler.rs:27`) exist but are not exported. Persistence is actor-centric (`MemoryStore`, `JsonFileStore`, `SqliteStore`).

**Target architecture:**

- **Every cross-actor message carries a trace context.** `Packet::ActorMessage` (`src/runtime/network.rs:93`) should include `trace_id` and `parent_span_id`. The runtime should emit OpenTelemetry-compatible spans for send, schedule, execute, and persist.
- **Journal = audit log + replay source.** The journal should be written to a durable, replicated log (e.g., Kafka, NATS JetStream, or a raft-backed log) for workflows and event-sourced actors. Local `SqliteStore` is fine for single-node testing but not for production clusters.
- **Snapshots should be incremental and compressed.** The current snapshot is a full JSON dump of durable fields. Large actors need delta snapshots and garbage collection of old journal segments.
- **Object storage for cold state.** Hibernated actors and large CRDT backups should live in S3-compatible object storage.

### 2.8 Autoscaling

**Current state.** There is no autoscaling. `DESIGN_CLOUD.md` describes `min_instances`, `max_instances`, scaling metrics, cooldowns, and idle timeouts, but none are implemented.

**What must change:**

- **Metrics pipeline.** The runtime must export per-actor and per-node metrics: mailbox depth, reduction count, CPU time, memory, message rates, GC stats.
- **Control plane autoscaler.** A separate (or embedded) component reads metrics and decides to spawn/destroy actor instances. Stateless actors scale horizontally; stateful actors scale by partitioning.
- **Partitioning for stateful actors.** Stateful actors cannot be blindly replicated. The runtime needs consistent hashing or range partitioning so that messages for key `K` always route to the actor instance owning `K`.
- **Scale-to-zero and cold start.** Idle actors should hibernate. On first message, the control plane spawns a new instance and replays journal/snapshot. This requires fast snapshot loading and a warm pool for latency-sensitive actors.

---

## 3. Highest-Impact Runtime Improvements

These are the top five improvements ranked by production impact / architectural leverage.

### 3.1 True M:N scheduler with actor affinity

**What:** Replace the single-threaded `Runtime::run_scheduler` with a pool of worker threads each running `Scheduler::run_worker`, while preserving actor-isolation invariants.  
**Why it matters:** Today all actor execution is serialized on one thread. The Chase-Lev deque is already present but unused for true parallelism. Unlocking it is the single biggest throughput win.  
**Difficulty:** 3/5.  
**Maintenance cost:** 2/5.

### 3.2 Production-grade distributed messaging protocol

**What:** Replace the `behavior_id = 0` placeholder, silent drops, and unversioned NUL0 framing with a protocol carrying stable behavior IDs, delivery semantics, backpressure, and schema versioning.  
**Why it matters:** The current protocol loses messages silently when the outbound channel is full, cannot evolve, and cannot express delivery guarantees.  
**Difficulty:** 4/5.  
**Maintenance cost:** 3/5.

### 3.3 Durable workflow runtime

**What:** Build a workflow engine on top of persistent actors: durable timers, activity workers, saga compensation, and deterministic replay.  
**Why it matters:** Workflows turn “let it crash” into “resume exactly where you left off.”  
**Difficulty:** 5/5.  
**Maintenance cost:** 4/5.

### 3.4 Cross-node ownership model (forbid remote refs)

**What:** Forbid passing mutable references across nodes; only `iso` (linear) or `val` (immutable copy) values cross node boundaries.  
**Why it matters:** ORCA works well within a node, but the centralized `CycleDetector` is restricted to intra-node only after an audit found distributed DFS misidentifies slow remote refs as dead cycles. Cross-actor cycles spanning nodes will leak memory unless addressed.  
**Difficulty:** 4/5.  
**Maintenance cost:** 2/5.

### 3.5 Observability stack

**What:** Add OpenTelemetry-compatible tracing, Prometheus-style metrics, and structured logging throughout the runtime.  
**Why it matters:** There is almost no observability today. Scheduler stats, resolver stats, and GC stats exist but are not exported.  
**Difficulty:** 2/5.  
**Maintenance cost:** 1/5.

---

## 4. Specific Subsystem Analysis

### 4.1 ORCA GC: strengths and long-term risks

**Strengths:**

- **No global stop-the-world pauses.** Each actor collects its own heap independently.
- **Deterministic reclamation.** Reference counting frees objects as soon as the last reference drops.
- **Good fit for actor isolation.** Actors do not share mutable state by default, so per-actor heaps map cleanly to the programming model.
- **Cross-actor protocol is simple.** `send_ref_to` increments `foreign_count`; `process_foreign_op` decrements it (`src/runtime/gc.rs:417`).

**Risks:**

- **Fragmentation.** The 64 KB fixed initial heap (`src/runtime/actor.rs:66`) and bump allocator will fragment over time. There is no compaction because objects are never moved.
- **Cycle collection is centralized and single-threaded.** The `CycleDetector` runs on the coordinator thread and maintains a global `HashMap` of foreign edges (`src/runtime/orca_cycle.rs:268`). This will become a bottleneck.
- **Distributed cycles are not collected.** The detector is restricted to intra-node (`src/runtime/orca_cycle.rs:289`). Long-running distributed systems will leak memory if actors form cycles across nodes.
- **Header duplication.** `heap.rs` and `gc.rs` each define an `OrcaHeader` with overlapping but different layouts (`src/runtime/heap.rs:151` vs `src/runtime/gc.rs:83`).
- **Thread safety assumptions.** `ActorHeap` is `!Sync` and is only accessed while the actor runs. True M:N scheduling must preserve this invariant.

**Verdict:** Per-actor reference counting is the right permanent choice **for intra-node memory management**. For cross-node references, the architecture should forbid reference passing and require `iso`/`val` semantics, eliminating distributed GC. For cross-actor cycles within a node, the centralized detector should be made incremental and sharded by actor ID.

### 4.2 Scheduler: single-threaded coordinator vs. true M:N

**Current design.** `Scheduler` uses `crossbeam::deque::{Injector, Worker, Stealer}` (`src/runtime/scheduler.rs:17`). It supports local LIFO push/pop, global FIFO steal, and work stealing from other workers. However, `Runtime::run_scheduler` (`src/runtime/mod.rs:337`) calls `self.scheduler.dequeue()` and then `self.step_actor(actor_id)` sequentially on one thread.

**Work-stealing correctness.** The Chase-Lev implementation is correct for task queues. The issue is not the queue but the runtime:
- `Runtime` is not `Send`/`Sync` because it contains `HashMap<u64, Actor>` and many mutable fields.
- `step_actor` mutates actor state, the scheduler, the coordinator, and persistence without synchronization.

**Reduction semantics.** Each actor has `max_reductions: 1000` (`src/runtime/actor.rs:85`). After 1000 reductions the actor yields. This is a good cooperative preemption model.

**Path to M:N:**
1. Make actor lookup concurrent (sharded map or concurrent hash map).
2. Run each worker thread with its own `current_actor` and a reference to the actor table.
3. Move GC op draining and cycle detection to a background thread or per-worker batch.
4. Keep the reduction quota; it provides fairness without kernel scheduling.

### 4.3 CRDTs vs. event sourcing vs. durable workflows

**CRDTs** are best for *convergent shared state*. The current 8 CRDTs are solid, but:
- `CrdtManager` is not sharded; all CRDTs live in one `HashMap` (`src/runtime/crdt_manager.rs:114`).
- Synchronization is “send full replica to all healthy members” (`src/runtime/mod.rs:908-923`). This is O(n²) in replicas and O(size_of_state) per sync. Production needs delta-state CRDTs and anti-entropy with version vectors.
- `LamportTime`/`LamportClock` are defined twice (`src/runtime/crdt.rs:421` and `src/runtime/crdt_reg.rs:19`), a known hazard (`AGENTS.md:123`).

**Event sourcing** is best for *append-only audit/replay*. The current `emit_event` MVP increments integer counters (`src/runtime/mod.rs:182`). A real implementation needs domain event schema, journal compaction, snapshotting, and deterministic replay.

**Durable workflows** combine event sourcing with orchestration. They need a workflow state machine, activity workers, durable timers, signals/queries, and saga compensation.

**Coexistence model:** actors should declare `StateModel` per field. `Crdt` fields are merged across nodes. `EventSourced` fields are replayed from the journal. `Durable` fields are snapshotted. `Local` fields are reset. The runtime routes each to the appropriate subsystem.

### 4.4 Persistence: is the abstraction right?

**Current abstraction.** `PersistenceStore` (`src/runtime/persistence.rs:94`) has `save_snapshot`, `load_snapshot`, `append_journal`, `read_journal`, `latest_sequence`, and `clear`. Implementations: `MemoryStore`, `JsonFileStore`, `SqliteStore`.

**Assessment:**

- The trait is the right shape for actor-centric persistence.
- `PersistedValue` normalizes pointers and strings to `Nil` (`src/runtime/persistence.rs:47`), which is safe but means complex state cannot yet be persisted.
- `JsonFileStore` writes pretty-printed JSON on every snapshot — fine for debugging but too slow for high-throughput actors.
- `SqliteStore` uses one connection behind a `Mutex`. It will become a bottleneck under concurrent actor checkpoints.

**What is missing for production:**

- A replicated log backend for journal entries.
- Incremental/delta snapshots to reduce write amplification.
- Async checkpointing so persistence does not block the actor scheduler.
- Schema evolution for persisted actor state.
- Encryption at rest for sensitive snapshots.

### 4.5 Autoscaling and placement: missing pieces

**Missing entirely:**

- Metrics export and collection.
- Control plane autoscaler.
- Placement constraints and policies.
- Actor migration with state transfer.
- Partitioning for stateful actors.
- Scale-to-zero / hibernation.
- Warm pools for cold-start latency.

**Implementation path:**

1. Instrument the runtime.
2. Add placement metadata to actor spawn.
3. Build a global scheduler that assigns actors to nodes using consistent hashing or constraint satisfaction.
4. Implement migration: freeze → snapshot → transfer mailbox → resume.
5. Add autoscaler loops per actor type.

### 4.6 Observability: tracing, metrics, profiling gaps

**Current state:**

- `SchedulerStats` tracks local/global/steal tasks, empty polls (`src/runtime/scheduler.rs:27`).
- `ResolverStats` tracks local/remote resolves, cache hits/misses, failed resolves (`src/runtime/distributed.rs:284`).
- `GcStats` tracks allocations, frees, ref operations (`src/runtime/gc.rs:169`).
- None of these are exported or aggregated.

**Recommended stack:**

- OpenTelemetry for traces and logs.
- Prometheus for metrics.
- `tracing` crate for structured logging in Rust.
- A small Nulang-specific metrics exporter that converts scheduler/GC/resolver stats into Prometheus counters/gauges/histograms.

---

## 5. Summary and Recommended Roadmap for Runtime

| Priority | Item | Effort | Impact |
|----------|------|--------|--------|
| 1 | True M:N scheduler | Medium | Very High |
| 2 | Production distributed messaging protocol | High | Very High |
| 3 | Durable workflow runtime | Very High | Very High |
| 4 | Cross-node ownership model (forbid remote refs) | High | High |
| 5 | Observability stack | Low | High |
| 6 | Replicated persistence backend | High | High |
| 7 | Autoscaling / placement control plane | Very High | High |
| 8 | Delta-state CRDT synchronization | Medium | Medium |

The codebase has a strong conceptual foundation: actor isolation, ORCA, algebraic effects, capabilities, and a clear cloud/workflow vision. The next phase should focus on **concurrency** (M:N scheduler), **reliability** (production protocol + workflows), and **observability** before scaling out autoscaling and global placement.
