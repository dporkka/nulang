# Nulang Strategic Redesign: The Erlang of the AI Era

**Document Version:** 1.0
**Date:** January 2025
**Audience:** Core team, contributors, and senior engineers evaluating Nulang for production systems
**Status:** Draft for implementation alignment

---

## Table of Contents

1. [Language Philosophy](#1-language-philosophy)
2. [Five-Layer Architecture](#2-five-layer-architecture)
3. [Durable Execution Subsystem](#3-durable-execution-subsystem)
4. [Workflow Subsystem](#4-workflow-subsystem)
5. [AI Runtime Design](#5-ai-runtime-design)
6. [WASM Interoperability](#6-wasm-interoperability)
7. [Capability Networking Model](#7-capability-networking-model)
8. [Cloud Deployment Architecture](#8-cloud-deployment-architecture)
9. [Developer Tooling Roadmap](#9-developer-tooling-roadmap)
10. [Migration Plan](#10-migration-plan)

---

## 1. Language Philosophy

### The Actor as Universal Abstraction

Nulang is built on a single, powerful insight: **the actor model subsumes every other model of computation.** A service is an actor. An agent is an actor. A workflow step is an actor. A durable object is an actor. Even a human in a business process is modeled as an actor -- one with slow response times and unpredictable outputs, but an actor nonetheless.

Every Nulang program is a collection of actors. There are no standalone functions, no global state, no ambient authority. An actor is the only first-class abstraction that combines all three essential properties of real-world computation: state encapsulation (it has internal state), concurrency (it processes messages), and identity (it has a stable address that outlives any single message).

This is not an academic stance. We chose the actor model because it is the **only** programming model that naturally handles the four horsemen of distributed systems: failure, latency, concurrency, and partial knowledge. Functions can't handle failure without callers managing retries. Threads share mutable state by default. RPC abstracts away failure modes that inevitably leak. Only actors acknowledge reality: messages are sent, may be lost, may be delayed, and the actor itself may crash before processing them.

### Five Design Principles

#### 1. Durability by Default

Every actor in Nulang is durable unless explicitly marked otherwise. This is the opposite of every other programming language, where persistence is an afterthought added through annotations, external databases, or fragile serialization code.

The default state model is `durable`: the runtime automatically checkpoints actor state to persistent storage on each message boundary. When a node crashes and restarts, the actor resumes from its last checkpoint with exactly the state it had before failure. No manual serialization. No ORM. No "what if the process dies mid-request."

This principle is borrowed directly from Durable Functions and Orleans, both of which proved that developers forget to make things durable. We make the safe choice the easy choice.

#### 2. Effects for Side Effects

Nulang uses an algebraic effect system to track and control side effects. Effects are not exceptions -- they are resumable, typed operations that separate the *what* from the *how*. When an actor performs IO, accesses the network, or calls an LLM, it uses an effect. The runtime handles the effect, enabling it to log, intercept, retry, or replace the handler.

Effects serve three purposes in Nulang:
- **Observability**: Every effect invocation is automatically traced
- **Testability**: Effect handlers can be swapped for mocks in tests
- **Determinism**: For event-sourced actors, effect results are captured in the event journal, enabling deterministic replay

This is not Haskell's monadic IO. Effects in Nulang are designed for distributed systems: they can be serialized, forwarded across the network, and resumed on another node.

#### 3. Capabilities for Security

Nulang adopts capability-based security as its fundamental security model. There is no ambient authority. An actor cannot open a file, make a network request, or invoke an LLM unless it holds a capability for that operation. Capabilities are first-class values that can be passed between actors, delegated with restrictions, and revoked.

This maps cleanly to the WASM Component Model's WASI worlds. A Nulang capability compiles to a WIT interface import. The runtime verifies capabilities at message boundaries. The result is security that is: (a) composable -- capabilities compose like functions, (b) auditable -- every capability grant is logged, and (c) zero-overhead -- verification happens at compile time via the type system and at deploy time via WASI linking.

#### 4. WASM for Interop

Nulang compiles to WebAssembly components (wasm32-wasip2). Every actor is a WASM component. Every actor protocol is a WIT interface. This is non-negotiable -- it is the foundational interop decision.

WASM gives Nulang three properties no native binary format can match:
- **Sandboxed execution**: Each actor runs in its own WASM sandbox with no ambient authority
- **Cross-language composition**: Rust, Go, C, and other WASM-targeting languages can implement Nulang actor protocols via WIT
- **Portable deployment**: The same WASM binary runs on bare metal, Kubernetes, edge workers, and serverless platforms

The WASM Component Model is production-ready. The binary format has been stable for over a year. Tooling exists for a dozen languages. Betting against WASM at this point is like betting against TCP/IP in 1990.

#### 5. Virtual Actors for Simplicity

Nulang adopts the Orleans virtual actor model: actors always "exist" logically. You address them by identity, not by location. The runtime manages activation, deactivation, placement, and migration. Creating an actor is as simple as sending it a message -- there is no explicit `spawn`, `new`, or `create` operation that returns a handle.

This is a deliberate rejection of Erlang's explicit process spawning and Akka's actor-of-actor factories. Both models force developers to manage actor lifetimes, which introduces coupling between the creator and the created. In Nulang, actor A can send a message to actor B without knowing whether B is running, where it is running, or whether it has ever received a message before. The runtime handles all of this.

Virtual actors also enable automatic scaling: if 1000 actors with the same behavior type each receive one message per second, the runtime might colocate them on a single node. If one of those actors suddenly receives 10,000 messages per second, the runtime can migrate just that actor to its own node.

### What Nulang Is NOT

**Nulang is not a research language.** We do not invent new type theories. Hindley-Milner with row-polymorphic effects is well-understood. The WASM Component Model is a shipping standard. We combine proven ideas, we do not invent unproven ones.

**Nulang is not a new syntax for existing concepts.** We do not wrap Kubernetes in a new syntax and call it a language. Nulang's syntax is a means to express actor semantics that have no equivalent in general-purpose languages. If you can write it in Rust with a library, Nulang is not the right tool.

**Nulang is not an academic exercise.** Every feature justifies itself through developer productivity or operational value. If a feature requires a PhD to use correctly, it does not ship. If a feature saves 10 lines of boilerplate at the cost of making the runtime 10x harder to debug, it does not ship.

**Nulang is not only for AI agents.** The AI runtime is Layer 5 of 5. Nulang is a distributed systems language that happens to be excellent for AI agents because AI agents are distributed systems: they have state, communicate asynchronously, fail unpredictably, and need durability.

### Target Audience

Nulang is designed for three archetypes:

1. **The Backend Engineer building distributed systems.** They have been burned by microservices that call each other in cascading RPC chains. They want deterministic, testable, fault-tolerant services without the operational complexity of Kubernetes + Istio + Kafka + Postgres + Redis + Temporal. Nulang replaces that stack.

2. **The AI Engineer building agent workflows.** They started with LangChain, hit its single-process ceiling, tried LangGraph, and now need something that actually runs across machines, survives crashes, and handles human-in-the-loop approvals. Nulang replaces LangGraph + a job queue + a database + custom infrastructure.

3. **The Platform Engineer building cloud infrastructure.** They want to offer internal developers a platform where services are just actors, scaling is automatic, and security is capability-based rather than network-policy-based. Nulang replaces Akka + a service mesh + a secrets manager + custom authz.

---

## 2. Five-Layer Architecture

Nulang is organized into five layers, each building on the one below. The layer model ensures that each subsystem has clear boundaries and can be understood, tested, and evolved independently.

### Architecture Overview

```
+------------------------------------------------------------------+
|                        LAYER 5: AI RUNTIME                        |
|  Models | Tools | Memory (Vector Store) | Planning | Evaluation  |
+------------------------------------------------------------------+
|                        LAYER 4: DISTRIBUTED PLATFORM               |
|  Clustering | CRDTs | Message Routing | Service Discovery         |
|  Consistent Hashing | Multi-Region Replication | Gossip Protocol  |
+------------------------------------------------------------------+
|                        LAYER 3: DURABLE EXECUTION                  |
|  Persistence | Snapshots | Event Replay | Crash Recovery          |
|  Event Journal | State Checkpointing | Deterministic Replay       |
+------------------------------------------------------------------+
|                        LAYER 2: ACTOR RUNTIME                      |
|  Virtual Actors | Supervision Trees | Scheduler (Work-Stealing)  |
|  Message Passing | Location Transparency | Mailbox Management     |
|  Process Isolation (WASM Sandboxes) | Hot Code Reloading         |
+------------------------------------------------------------------+
|                        LAYER 1: LANGUAGE                           |
|  Syntax | Hindley-Milner Type System | Algebraic Effects          |
|  Capability Types | WIT Interface Compilation | Pattern Matching   |
+------------------------------------------------------------------+
```

### Layer Interactions

Each layer communicates only with adjacent layers. Layer 3 (Durable Execution) does not call Layer 5 (AI Runtime) directly -- it goes through Layer 4 (Distributed Platform) and Layer 2 (Actor Runtime). This constraint ensures that each layer can be tested, mocked, and replaced independently.

```
+------------------------------------------------------------------+
| LAYER 5: AI RUNTIME                                               |
| Calls: Layer 4 (for distributed AI agents), Layer 2 (for actors) |
| Provides: LLM completion, tool calling, vector memory, planning  |
+------------------------------------------------------------------+
   |
   v
+------------------------------------------------------------------+
| LAYER 4: DISTRIBUTED PLATFORM                                     |
| Calls: Layer 3 (for state replication), Layer 2 (for placement)  |
| Provides: Clustering, routing, CRDT sync, multi-region replication|
+------------------------------------------------------------------+
   |
   v
+------------------------------------------------------------------+
| LAYER 3: DURABLE EXECUTION                                        |
| Calls: Layer 2 (for actor state snapshots)                       |
| Provides: Persistence, checkpoints, event replay, recovery       |
+------------------------------------------------------------------+
   |
   v
+------------------------------------------------------------------+
| LAYER 2: ACTOR RUNTIME                                            |
| Calls: Layer 1 (for compiled WASM components)                    |
| Provides: Scheduling, messaging, supervision, sandbox execution  |
+------------------------------------------------------------------+
   |
   v
+------------------------------------------------------------------+
| LAYER 1: LANGUAGE                                                 |
| Calls: None (pure compiler)                                      |
| Provides: Parser, type checker, WIT generator, WASM compiler     |
+------------------------------------------------------------------+
```

### Layer 1: The Language

Layer 1 defines what Nulang code looks like, how it is type-checked, and how it compiles. This layer has no runtime dependencies -- it is a pure compiler from Nulang source to WASM components.

**Key components:**

- **Parser and AST**: Nulang uses a Rust-like syntax with actor-specific keywords (`actor`, `behavior`, `protocol`, `effect`, `capability`, `persistent`, `workflow`). The parser produces a typed AST.

- **Type checker**: Hindley-Milner with row-polymorphic effects. Every function signature includes its effect row: `fn handle_order(order: Order) : [IO, Log, DB] -> Result`. Effect rows are open by default (they can have additional effects) and are unified at call sites.

- **Capability type system**: Capabilities are types, not runtime checks. `capability DatabaseRead` is a type like `Int` or `String`. Functions declare capability requirements in their types. The type checker verifies that every capability used is either held by the actor or passed as an argument.

- **WIT compiler**: The Nulang compiler generates WIT interface definitions from actor protocols and compiles actor implementations to WASM components targeting wasm32-wasip2. Each actor becomes a WASM component with its protocol as the exported interface and its required capabilities as imported interfaces.

**Design decisions:**

- **No type inference across actor boundaries**: When actor A calls actor B, the protocol type is explicit. This is a deliberate tradeoff: slightly more annotation in exchange for the ability to change B's implementation without recompiling A.
- **Generics, but no higher-kinded types**: Generics are essential (a `Mailbox<T>` protocol is generic over message type). HKTs would complicate the WASM compilation target without clear distributed systems benefits.
- **No async/await syntax**: Actors process messages sequentially within a single behavior handler. Concurrency is via actor spawning, not async tasks. This avoids the "colored functions" problem entirely.

### Layer 2: Actor Runtime

Layer 2 is the in-memory execution engine. It manages the lifecycle of actors: creating them, scheduling them, passing messages between them, and restarting them when they fail.

**Key components:**

- **Virtual actor manager**: Implements the Orleans-style virtual actor abstraction. When a message arrives for actor ID `order-42`, the virtual actor manager either routes it to the currently activated instance or activates a new one (possibly on another node). Activation is transparent to the sender.

- **Supervision trees**: Every actor has a supervisor. If an actor crashes processing a message, the supervisor decides the recovery policy: restart the actor, restart all children, escalate to its own supervisor, or stop. This is adapted directly from Erlang/OTP's supervision model, with the key difference that supervisors are also actors and can be supervised themselves.

- **Work-stealing scheduler**: Nulang uses a scheduler inspired by Go's runtime and Erlang's BEAM VM. Each OS thread runs a scheduler that maintains a local run queue of ready actors. When a queue is empty, the scheduler steals work from other queues. Actors yield cooperatively at message boundaries (there are no long-running computations within a behavior handler), so preemption is via work-based yielding, not time-slicing.

- **WASM sandbox pool**: Each actor runs inside a WASM sandbox (Wasmtime instance). Sandboxes are pooled and reused across actor activations to minimize cold-start latency. Actor state lives in the sandbox's linear memory, which the runtime snapshots for durability.

- **Message router**: Routes messages by actor ID. The router consults a location directory (distributed via gossip) to find which node hosts an actor. If the actor is not activated anywhere, the router picks a node and requests activation.

**The scheduler in detail**:

```
+----------------------------------------------------------+
|                   OS Thread Pool                         |
|  +--------+  +--------+  +--------+  +--------+         |
|  |Sched 1 |  |Sched 2 |  |Sched 3 |  |Sched N |         |
|  |Queue:  |  |Queue:  |  |Queue:  |  |Queue:  |         |
|  |[A1,A3] |  |[A2]   |  |[A5,A6]|  |[A4,A7] |         |
|  +--------+  +--------+  +--------+  +--------+         |
|       |           |           |           |              |
|       | steal     | steal     |           | steal        |
|       +---------->+---------->+<----------+              |
|                                                   |
+----------------------------------------------------------+
```

Each scheduler maintains a local run queue (LIFO for spawned actors, FIFO for incoming messages). When a queue is empty, the scheduler attempts to steal half the queue from another scheduler. This is Go's work-stealing algorithm, proven to minimize idle time and maximize cache locality.

An actor yields at every message boundary. If a behavior handler needs to perform a long computation, it must either: (a) break it into multiple messages to itself, or (b) spawn a child actor. The scheduler detects runaway handlers via fuel metering in the WASM runtime and forcibly yields actors that exceed their fuel allocation.

**Hot code reloading**: Nulang supports hot code reloading at the actor level. When a new version of an actor's code is deployed, existing activations finish their current message, checkpoint, and are replaced by new activations. This is adapted from Erlang's code_change mechanism but uses WASM module swapping instead of BEAM code loading. Hot reloading requires that the new code can handle the old state format (via a migration function) or that the state is re-initialized.

**Design decisions:**

- **One mailbox per actor**: Following Erlang, not Akka (which allows multiple mailbox implementations). This simplifies reasoning: messages are processed FIFO, one at a time. Parallelism comes from having many actors, not from concurrent message processing within one actor.
- **Messages are immutable and copy-on-write**: When actor A sends a message to actor B, the message is serialized to a shared format (Cap'n Proto) and ownership transfers. The sender cannot mutate the message after sending. This enables zero-copy message passing within a node while maintaining safety.
- **Maximum message size: 1MB**: Messages larger than 1MB should reference external storage (the built-in blob store capability). This prevents unbounded memory growth in mailboxes.
- **Fair scheduling**: Each actor gets a turn every scheduling quantum. No actor can starve others, even if it receives a continuous stream of high-priority messages. Priority levels (high, normal, low) exist but are advisory -- the scheduler guarantees fairness within each priority level.

### Layer 3: Durable Execution

Layer 3 persists actor state so that actors survive node crashes, planned maintenance, and restarts. Without Layer 3, Nulang would be an in-memory actor framework like Akka Classic. With Layer 3, Nulang becomes a durable execution platform like Temporal or Durable Functions.

See [Section 3: Durable Execution Subsystem](#3-durable-execution-subsystem) for the full design.

**Key components:**

- **State persistence engine**: Stores actor state snapshots to persistent storage. Storage backend is pluggable: local SQLite (development), PostgreSQL (production single-node), S3-compatible object store (production multi-node).

- **Event journal**: An append-only log of all events for event-sourced actors. Each actor has its own journal (physically sharded by actor ID). Journals support reading from arbitrary positions for replay debugging.

- **Checkpoint manager**: Triggers state snapshots based on configurable policies: every N messages, every T seconds, or when memory exceeds a threshold. Coordinates with the scheduler to pause an actor briefly during snapshot.

- **Recovery orchestrator**: When a node fails, detects the failure (via heartbeat timeout), finds all actors that were activated on that node, and replays their event journals or restores their state snapshots on surviving nodes.

### Layer 4: Distributed Platform

Layer 4 enables multiple Nulang runtime nodes to form a cluster. It handles node discovery, actor placement, state replication across regions, and network partitions.

**Key components:**

- **Cluster manager**: Nodes join clusters via a gossip protocol (SWIM-based failure detection). New nodes announce themselves, existing nodes share their actor directory partitions, and failed nodes are detected within seconds.

- **CRDT engine**: For `crdt` state model actors (see Section 3), Layer 4 provides conflict-free replicated data types that sync across nodes without coordination. CRDTs are essential for low-latency geographically distributed state.

- **Consistent hash ring**: Actor placement uses consistent hashing on actor ID. This ensures that the same actor ID always maps to the same node (within replication factor), enabling efficient caching of location lookups.

- **Multi-region replicator**: Replicates event journals and CRDT state across geographic regions. Uses async replication with configurable consistency: `eventual` (default), `causal`, or `strong` (with latency cost).

- **Service discovery**: Actor protocols are registered in a distributed registry. Clients discover actors by protocol type, not by hardcoded addresses. This enables dynamic scaling and load balancing.

**Design decisions:**

- **CAP theorem stance**: Nulang defaults to AP (available + partition-tolerant) with CRDTs and eventual consistency. Strong consistency is available as a per-actor option but requires cross-region coordination. This matches the reality that most distributed systems prefer availability over strong consistency.
- **No distributed transactions**: Distributed transactions across multiple actors are not supported as a primitive. Use sagas (see Section 4: Workflow Subsystem) instead. This is a deliberate simplification: distributed transactions are slow, complex, and fail in ways that are harder to debug than sagas.

### Layer 5: AI Runtime

Layer 5 is the AI-specific subsystem that sits atop the distributed actor platform. It provides LLM integration, tool calling, memory management, and agent planning.

See [Section 5: AI Runtime Design](#5-ai-runtime-design) for the full design.

**Key design principle**: There is NO separate AI Agent DSL. An AI agent in Nulang is just an actor that holds the `LLM` capability. Everything agents do -- calling tools, remembering context, planning multi-step workflows -- is built from the same primitives as any other actor.

**Key components:**

- **Model provider abstraction**: Pluggable backends for OpenAI, Anthropic, local models (Ollama, vLLM), and custom model servers. Each provider implements the same WIT interface, so actors are portable across providers.

- **Tool registry**: Typed tools that actors can register and invoke. Tools are just actor behaviors exposed via a standard protocol. The AI runtime handles tool schema generation (OpenAI function calling format), invocation, and result parsing.

- **Memory manager**: Three-tier memory for AI actors: short-term (conversation context within a behavior handler), long-term (vector store for RAG), and event memory (the actor's event journal as structured knowledge).

- **Planner**: Composes workflows from natural language descriptions by delegating to the workflow subsystem. The planner is itself an actor.

- **Evaluation framework**: Tracing and observability for AI actor execution, including LLM call latency, token usage, tool invocation success rates, and human feedback integration.

---

## 3. Durable Execution Subsystem

The durable execution subsystem is Nulang's defining feature. It is what separates Nulang from actor frameworks like Akka (in-memory only) and what makes Nulang a credible replacement for Temporal, Durable Functions, and self-hosted LangGraph.

### Core Design: Four State Models

Every actor in Nulang declares a state model. The state model determines how the actor's state is persisted, recovered, and replicated. There are four state models:

| State Model | Persistence | Recovery | Consistency | Use Case | Overhead |
|-------------|-------------|----------|-------------|----------|----------|
| `local` | None | None | Immediate | Stateless services, pure computation | Zero |
| `durable` | Automatic checkpointing | State restore | Strong (single node) | Most business logic, workflows | Low (~1ms per checkpoint) |
| `event_sourced` | Append-only event journal | Event replay | Strong (single node) | Financial transactions, audit trails | Medium (journal append) |
| `crdt` | CRDT merge + replication | State sync | Eventual (distributed) | Collaborative state, geo-replicated | Low (merge on sync) |

An actor declares its state model explicitly:

```nulang
// Stateless computation - no persistence
actor Calculator { ... }

// Automatic checkpointing on message boundaries
persistent actor OrderWorkflow { ... }

// Full event sourcing with replay
persistent event_sourced actor BankAccount { ... }

// Geo-replicated CRDT - no coordination needed
persistent crdt actor ShoppingCart { ... }
```

State model is part of the actor's type. A `durable` actor cannot be used where an `event_sourced` actor is required, and vice versa. This is not a runtime flag -- it is a semantic property that affects the actor's observable behavior, particularly around recovery and consistency.

### The `durable` State Model

`durable` is the default state model for `persistent` actors. It works as follows:

1. **Message arrival**: Actor receives message M. The scheduler assigns it a message ID (monotonically increasing per actor).

2. **State loading**: If the actor is being activated (not already in memory), its state is loaded from the most recent checkpoint. The actor is now at state S_n.

3. **Message processing**: The behavior handler runs to completion, processing message M. The actor may: (a) mutate internal state, (b) send messages to other actors, (c) perform effects, (d) return a result.

4. **Checkpoint**: After the handler completes successfully, the runtime captures the actor's entire linear memory and serializes it to the persistence backend. This is the new checkpoint S_{n+1}.

5. **Acknowledgment**: The checkpoint is acknowledged. Messages sent during the handler are now eligible for delivery to their recipients. If the actor returned a result to a caller, the result is sent.

This is exactly the model used by Durable Functions. Key insight: checkpointing happens at message boundaries, not mid-computation. A behavior handler is a synchronous, deterministic function from (current_state, incoming_message) to (new_state, outgoing_messages, effect_results). It runs to completion or not at all.

**Checkpoint policy**: Checkpoints are triggered by one of three conditions:
- **Count**: Every N messages (default: 1, meaning checkpoint after every message)
- **Time**: Every T seconds (default: none, count-only)
- **Size**: When actor memory exceeds M MB (default: 100MB)

Checkpoint-on-every-message is the safe default. For high-throughput, low-value actors, users can relax this to every N messages or time-based. The runtime batches checkpoints across actors to amortize storage write costs.

**Crash recovery**: When a node crashes, the recovery orchestrator:
1. Detects the crash via heartbeat timeout (default: 5 seconds)
2. Reads the cluster's actor placement map to find all actors that were on the crashed node
3. For each `durable` actor: loads the most recent checkpoint on a new node, activates the actor
4. The actor resumes processing from the next unacknowledged message in its mailbox

Messages sent but not yet acknowledged are replayed from the sender's journal (for `event_sourced` senders) or retried by the caller (for at-least-once delivery guarantees).

### The `event_sourced` State Model

`event_sourced` is for actors that require full audit history and deterministic replay. It works as follows:

1. **Event sourcing invariant**: An event-sourced actor's state is computed by folding a pure function over its event journal. The journal is the source of truth; checkpoints are optimizations.

2. **Command processing**: When a message arrives (treated as a command), the actor's behavior handler does not mutate state directly. Instead, it emits one or more events: `emit OrderCreated { id: "42", items: [...] }`.

3. **Event append**: Events are appended to the actor's event journal before any state is updated. The append is atomic and durable.

4. **State projection**: After events are durably appended, the actor's state is updated by applying the events through a projection function. This function is pure and deterministic.

5. **Snapshots**: Periodically (every N events or T seconds), the runtime takes a snapshot of the projected state and stores it alongside the journal. Recovery can start from the latest snapshot and replay only events after it.

**Deterministic replay**: For `event_sourced` actors, all effects must be deterministic or their results must be captured in events. If an actor calls a database, the query result must be part of the event. If an actor calls an LLM, the LLM response must be captured. This is the Temporal model: non-deterministic operations are recorded as events, and replay uses the recorded results.

Nulang enforces this at compile time through the effect system. Effect handlers for `event_sourced` actors automatically capture effect results into events. The developer does not write replay logic -- the runtime does.

```nulang
persistent event_sourced actor BankAccount {
  state {
    balance: Decimal
  }

  behavior deposit(amount: Decimal) {
    emit Deposited { amount: amount }
  }

  behavior withdraw(amount: Decimal) {
    if state.balance >= amount {
      emit Withdrawn { amount: amount }
    } else {
      emit WithdrawalDeclined { amount: amount, reason: InsufficientFunds }
    }
  }

  projection Deposited(e) {
    state.balance = state.balance + e.amount
  }

  projection Withdrawn(e) {
    state.balance = state.balance - e.amount
  }
}
```

**The event journal**: Each event-sourced actor has a dedicated append-only log, physically implemented as a sharded write-ahead log. Key properties:
- **Ordering**: Events within an actor's journal are totally ordered (per-actor serializability)
- **Addressing**: Events are addressed by (actor_id, sequence_number)
- **Retention**: Configurable. Options: keep forever, snapshot + retain last N events, time-based retention
- **Compaction**: Events before the latest snapshot can be archived (moved to cold storage) but are not deleted unless explicitly configured
- **Querying**: The journal supports reading from arbitrary positions for debugging, auditing, and building read models

**Journal storage implementation**: The event journal is stored as a sequence of immutable segment files. Each segment contains a fixed number of events (default: 10,000). When a segment fills, a new segment is created. Segments are written sequentially and never modified after close. This append-only segment design enables efficient replication (only the latest segment needs to be synced) and cheap snapshots (a snapshot is a reference to a segment + sequence number).

**Compaction strategy**: Over time, journals grow. Nulang uses a tiered compaction strategy:
- **Hot events** (last 10,000): Kept in memory + on disk
- **Warm events** (10,000 to 1,000,000): Kept on local SSD
- **Cold events** (beyond 1,000,000): Archived to object storage (S3-compatible)
- **Snapshots**: Taken every N events (default: 1,000). A snapshot captures the projected state, allowing recovery without replaying from event 0.

For actors with very high event volumes (e.g., IoT sensors emitting 1000 events/second), a time-based retention policy is recommended: keep 30 days of events with daily snapshots. This caps storage while preserving the ability to debug recent issues via replay.

**Deterministic replay for debugging**: A key operational advantage of event sourcing is the ability to replay an actor's exact history. Nulang provides a replay CLI:

```bash
nulang replay --actor bank-account-42 --from 0 --to 10000 --speed 100x
```

This replays the first 10,000 events of `bank-account-42` at 100x speed, producing the exact state the actor had at that point in time. Developers can set breakpoints, inspect state at any event boundary, and even fork the replay to test "what if" scenarios (what if event 5000 had a different outcome).

### The `crdt` State Model

`crdt` is for actors that need to be active on multiple nodes simultaneously with no coordination. Shopping carts, presence indicators, collaborative cursors -- any state where availability matters more than immediate consistency.

1. **CRDT state**: The actor's state is a CRDT (conflict-free replicated data type). Nulang supports: Grow-Only Counter (G-Counter), PN-Counter, Grow-Only Set (G-Set), 2P-Set, OR-Set, LWW-Register, MV-Register, OR-Map, and Delta-State CRDTs for efficient synchronization.

2. **Local updates**: Updates are applied locally and immediately. No coordination with other nodes.

3. **Async sync**: The CRDT engine periodically syncs state with other replicas using anti-entropy protocols. Sync can be: continuous (every update is broadcast), periodic (batched sync every N seconds), or on-demand (pull-based).

4. **Merge**: When sync occurs, CRDT merge functions guarantee convergence. Conflicting updates are resolved by the CRDT semantics (last-writer-wins, add-wins, remove-wins, etc.).

The CRDT model is the only state model that supports active-active multi-region deployment without a single point of coordination. A `crdt` actor can be active in us-east, eu-west, and ap-south simultaneously, with updates converging asynchronously.

### The `local` State Model

`local` actors have no persistence. They are in-memory only and do not survive node crashes. This is appropriate for:
- Pure computation (transforming data)
- Ephemeral workers (image processing, data validation)
- Stateless protocol adapters (HTTP request handlers)
- Caches that can be rebuilt

Local actors are the only actor type that can use unrestricted mutability within their behavior handlers. Since they have no persistence, determinism constraints do not apply.

### Deterministic Replay

Deterministic replay is required for `event_sourced` actors and optional (but recommended) for `durable` actors.

**How it works**: When a `durable` or `event_sourced` actor performs an effect, the runtime captures the effect's result and stores it alongside the checkpoint or event. On recovery, the runtime restores the actor's state and, for effects that were already executed, returns the stored result instead of re-executing the effect.

**Example**:
```nulang
persistent actor OrderProcessor {
  behavior process(order: Order) {
    // This HTTP call's result is captured in the checkpoint
    let payment = effect HttpPost(payment_gateway_url, order.payment_info)
    if payment.success {
      effect EmailSend(order.customer_email, "Your order is confirmed")
    }
    emit OrderProcessed { order_id: order.id, payment_ref: payment.ref }
  }
}
```

If this actor crashes after the HTTP call but before the checkpoint, recovery replays the behavior from the last checkpoint. The HTTP effect handler checks: was this call already made? If yes, returns the stored response. If no, makes the call. This ensures idempotency without requiring the developer to write idempotency keys manually.

**Non-deterministic effects**: Time, randomness, and external IO are the primary sources of non-determinism. Nulang handles each:
- **Time**: `effect Now` returns the current time, which is captured. On replay, the same timestamp is returned.
- **Randomness**: `effect Random` is a capability-gated effect. Results are captured.
- **External IO**: All IO is effect-based, so all IO results are captured.

### Workflow Resumption

Workflows (see Section 4) are a special case of durable actors. When a workflow actor reaches a suspension point -- `await approval`, `sleep duration`, or a call to an external service -- it checkpoints and deactivates. The scheduler removes it from the run queue.

When the awaited event occurs (human clicks "approve", timer fires, external callback arrives), the recovery orchestrator:
1. Loads the workflow's last checkpoint
2. Restores its state, including the suspended operation and its context
3. Resumes execution from the point after the suspension
4. The awaited value is injected as the result of the suspend effect

This is how Temporal handles workflow resumption and how Nulang handles all durable actor resumption. The mechanism is unified: workflows are actors, suspension is an effect, and resumption is recovery.

---

## 4. Workflow Subsystem

Workflows in Nulang are not a separate subsystem. A workflow is a `durable` or `event_sourced` actor that follows a specific pattern: its behavior is a graph of steps, and execution flows through the graph according to control-flow constructs.

### Workflows as Actor Graphs

Every workflow definition compiles to an actor. Every step in a workflow compiles to a behavior handler or a child actor. The workflow actor maintains the execution state (which steps have completed, which are running, what data has been produced).

```nulang
workflow PurchaseOrder {
  input {
    order: Order
    customer: Customer
  }

  step Validate {
    input = order
    behavior validate_order
  }

  // Conditional branch based on order value
  if order.total > 10_000 {
    step Approve {
      input = { order, approver: "manager" }
      behavior request_approval
      // Human-in-the-loop: workflow suspends here
      await approval from manager
    }
  }

  // Parallel execution
  parallel {
    step ChargePayment {
      input = order.payment_info
      behavior process_payment
    }
    step ReserveInventory {
      input = order.items
      behavior reserve_stock
    }
  }

  step Confirm {
    input = { order, payment_result, inventory_result }
    behavior send_confirmation
  }

  // Compensation on failure
  compensate {
    step Refund on PaymentFailed
    step ReleaseInventory on InventoryFailed
    step NotifyFailure on AnyFailure
  }

  output {
    confirmation: Confirm.result
    tracking_id: ChargePayment.result.transaction_id
  }
}
```

### Compilation Model

The workflow above compiles to:

```
+-----------------------------------------------------------+
|                     PurchaseOrder Actor                     |
|  (durable actor maintaining workflow execution state)      |
+-----------------------------------------------------------+
|                                                            |
|  State:                                                    |
|  - steps_completed: Set<StepId>                            |
|  - step_results: Map<StepId, Result>                       |
|  - current_step: StepId | Terminal                         |
|  - compensation_stack: List<CompensationAction>            |
|                                                            |
|  Behaviors:                                                |
|  - StartWorkflow (triggered by first message)              |
|  - StepCompleted { step, result }                          |
|  - HumanApproved { step, approver, decision }              |
|  - TimerFired { step }                                     |
|  - Compensate { failed_step, error }                       |
|                                                            |
|  Child Actors (activated per workflow instance):           |
|  - Validate actor     [local, ephemeral]                   |
|  - Approve actor      [durable, human-interactive]         |
|  - ChargePayment actor [durable, idempotent]               |
|  - ReserveInventory actor [durable, idempotent]            |
|  - Confirm actor      [local, notification]                |
+-----------------------------------------------------------+
```

Each `step` in the workflow definition becomes a message sent to a child actor. The workflow actor itself is a state machine that tracks which steps are complete and which can be started next.

### Control Flow

**Sequential execution**: Steps execute in order unless control flow changes it. A step's output becomes available as input to subsequent steps.

**Conditional branching**: `if` and `match` expressions in workflow definitions compile to conditional message sends. The condition is evaluated by the workflow actor (which has access to all completed step results), and the appropriate branch is taken.

```nulang
if order.total > 10_000 {
  step Approve { ... }
} else {
  step AutoApprove { ... }
}
```

**Parallel execution**: The `parallel` block sends all contained step messages simultaneously. The workflow actor waits for ALL to complete before continuing. If any parallel step fails, the others are cancelled (via supervision) and compensation begins.

```nulang
parallel {
  step A { ... }
  step B { ... }
  step C { ... }
}
// Execution continues here only when A, B, and C all complete
```

Parallelism is bounded by the `parallel_limit` configuration (default: 10 steps). Beyond that, steps are batched.

**Loops**: `foreach` and `while` loops are supported. A `foreach` loop over N items creates N step activations (potentially in parallel with a configurable fan-out). A `while` loop re-evaluates its condition after each iteration.

```nulang
foreach item in order.items {
  step ProcessItem {
    input = item
    behavior process_item
  }
}
```

**Error handling**: Workflow steps can specify retry policies:

```nulang
step ChargePayment {
  input = order.payment_info
  behavior process_payment
  retry {
    max_attempts: 3
    backoff: exponential { initial: 1s, max: 60s, multiplier: 2 }
    retry_on: [PaymentDeclined, NetworkError]
    fail_on:  [InvalidCard, FraudDetected]
  }
}
```

Retries are implemented by the workflow actor sending the same message again after the backoff period. The retry state (attempt count, next retry time) is part of the workflow actor's durable state, so retries survive node crashes.

**Workflow composition**: Workflows can call other workflows as sub-workflows:

```nulang
workflow PurchaseOrder {
  step Validate { ... }

  // Embed the Fulfillment workflow as a sub-workflow
  subworkflow Fulfillment {
    input = { order, warehouse: "us-east" }
  }

  step Confirm { ... }
}
```

The sub-workflow runs as a separate child actor but is monitored by the parent workflow. If the sub-workflow fails, the parent's compensation chain runs.

### Compensation and Sagas

The `compensate` block defines saga compensation actions. When a step fails, compensations for all previously completed steps are executed in reverse order.

Compensation rules:
- `on StepFailed`: Triggered when the named step fails
- `on AnyFailure`: Triggered when any step fails
- Compensations are themselves steps with behavior handlers
- A compensation failure triggers its own compensation (nested compensation)
- After all compensations complete, the workflow fails with the original error

This is the saga pattern as described in the original sagas paper and implemented in Temporal's saga abstraction. Nulang makes it declarative.

### Human-in-the-Loop

Human-in-the-loop is a first-class workflow primitive, not an afterthought:

```nulang
step Approve {
  behavior request_approval
  await approval from manager within 2.days
  timeout action escalate_to_director
}
```

When execution reaches `await`, the workflow actor:
1. Persists its state (checkpoint)
2. Sends a notification to the specified approver(s) via the configured channel (email, Slack, in-app)
3. Deactivates itself (removes from scheduler)

When the human responds, the approval service sends a message to the workflow actor, which reactivates and resumes. If the timeout expires before approval, the timeout action runs (escalation, auto-rejection, etc.).

### Time-Based Operations

```nulang
sleep 1.day                    // Suspend for duration
timeout 5.minutes { ... }      // Timeout a block
schedule "0 9 * * *" { ... }   // Cron-like scheduling
```

Time operations are effects that suspend the workflow. The runtime schedules a timer message. When the timer fires, the workflow resumes. Timer messages are durable: if the timer node fails, timers are recovered from the journal and re-scheduled.

### Observability

Every workflow execution produces a trace that is a first-class view into the event journal:
- Step start/end events
- Message sends between steps
- Compensation invocations
- Human approval requests and responses
- Timer schedules and firings
- Errors and recoveries

Workflows can be visualized in real-time (see Section 9: Workflow Visualizer) and debugged by replaying from any point in the event journal.

---

## 5. AI Runtime Design

Nulang's AI runtime is designed around a single principle: **there is no separate AI agent DSL**. AI agents are actors with LLM capabilities. Everything an agent does -- reasoning, tool use, memory, planning -- is built from the same primitives available to all actors.

This is a deliberate rejection of LangChain, LangGraph, and other AI frameworks that create a parallel programming model for agents. In Nulang, agents participate in supervision trees, have durable state, communicate via messages, and are secured by capabilities -- just like every other actor.

### Model Provider Abstraction

LLM providers implement a standard WIT interface:

```wit
interface llm-provider {
  variant model { gpt-4o, claude-sonnet, llama-3-1, custom(string) }

  record message {
    role: string,
    content: string,
  }

  record completion-request {
    model: model,
    messages: list<message>,
    tools: option<list<tool-definition>>,
    temperature: option<float64>,
  }

  record completion-response {
    content: string,
    tool-calls: list<tool-call>,
    usage: token-usage,
  }

  complete: func(req: completion-request) -> result<completion-response, error>
}
```

Actors request the `LLM` capability, and the runtime injects the configured provider. Providers are configured at deployment time:

```nulang
// Actor declares it needs LLM access
capability LLM

actor Researcher {
  behavior research(topic: String) {
    let prompt = "Research the following topic: {topic}"
    let response = effect LLMComplete({
      model: GPT4O,
      messages: [{ role: "user", content: prompt }]
    })
    return response.content
  }
}
```

**Provider backends**: OpenAI, Anthropic, Azure OpenAI, Ollama (local), vLLM (self-hosted), and custom (any HTTP endpoint conforming to the interface). New providers are added by implementing the WIT interface.

**Routing**: The runtime can route different actors to different providers based on configuration. A production deployment might use GPT-4o for complex reasoning tasks and a local Llama model for simple classification -- all within the same application.

### Tool System

Tools in Nulang are typed actor behaviors exposed to LLMs. There is no separate tool definition format -- if an actor has a behavior, it can be a tool.

```nulang
actor WeatherService {
  // This behavior can be called as a tool by any LLM actor
  behavior get_forecast(city: String, days: u8) -> Forecast {
    effect HttpGet("https://api.weather.com/v1/forecast", { city, days })
  }
}

// In the agent actor
actor TravelAgent {
  capability LLM

  behavior plan_trip(destination: String) {
    // The LLM can call WeatherService.get_forecast as a tool
    let response = effect LLMComplete({
      model: GPT4O,
      messages: [...],
      tools: [WeatherService.get_forecast, HotelService.search]
    })
    return response.content
  }
}
```

The AI runtime automatically generates tool schemas (OpenAI function calling format) from actor behavior types. When the LLM requests a tool call, the runtime:
1. Parses the tool call parameters
2. Sends a message to the target actor
3. Awaits the response
4. Injects the response into the conversation as a tool result message

Tool calls are effects, so they are traced, can be mocked in tests, and are captured for deterministic replay in event-sourced agents.

### Actor Memory

AI agents need memory. Nulang provides three tiers:

| Tier | Scope | API | Backend | Persistence |
|------|-------|-----|---------|-------------|
| Short-term | Single behavior handler | `conversation` | In-memory buffer | None (session) |
| Long-term | Actor instance | `memory.remember`, `memory.recall` | Vector store (Qdrant, pgvector) | `durable` |
| Event memory | All messages ever received | `journal.read` | Event journal | `event_sourced` |

**Short-term memory**: The `conversation` buffer holds the current LLM conversation. It is truncated to fit the model's context window. Automatic summarization when the buffer exceeds limits.

**Long-term memory**: A vector store for RAG. The actor can `remember` facts and `recall` them by semantic similarity.

```nulang
actor PersonalAssistant {
  capability LLM
  capability VectorStore

  behavior learn_fact(fact: String) {
    effect VectorStoreUpsert({ id: generate_id(), text: fact })
  }

  behavior answer_question(question: String) {
    let relevant_facts = effect VectorStoreQuery(question, limit: 5)
    let context = relevant_facts.map(|f| f.text).join("\n")
    let response = effect LLMComplete({
      model: GPT4O,
      messages: [
        { role: "system", content: "Context: {context}" },
        { role: "user", content: question }
      ]
    })
    return response.content
  }
}
```

**Event memory**: For event-sourced actors, the entire message history is available via the journal. This is "perfect memory" -- every interaction the actor has ever had is recorded and replayable.

### Planning

Planning is workflow composition. When an agent needs to perform a multi-step task, it delegates to the workflow subsystem:

```nulang
actor Planner {
  capability LLM

  behavior plan_task(goal: String) {
    // The LLM generates a workflow definition
    let workflow_def = effect LLMComplete({
      model: GPT4O,
      messages: [{ role: "user", content: "Create a workflow to: {goal}" }],
      tools: [WorkflowCompiler.validate]  // Validate generated workflows
    })

    // Compile and start the workflow
    let workflow = compile_workflow(workflow_def)
    effect StartWorkflow(workflow)
  }
}
```

The planner actor itself is a workflow orchestrator. It can:
- Generate workflows from natural language
- Execute pre-defined workflows with LLM-filled parameters
- Delegate to sub-agents (other actors) for parallel work
- Handle failures by replanning

### Evaluation and Observability

Every LLM call is automatically traced:
- Input (prompt, with PII redaction)
- Output (completion, with PII redaction)
- Token usage (input/output counts)
- Latency (time to first token, total time)
- Tool calls made
- Model used

Traces are sent to the configured observability backend (OpenTelemetry). This enables:
- Cost tracking per actor, per workflow, per deployment
- Performance monitoring (latency percentiles, error rates)
- Debugging (full conversation history for failed requests)
- A/B testing (compare model performance across providers)

**No separate AI observability framework.** The same tracing system used for all actors captures LLM calls. There is no LangSmith equivalent needed because the event journal IS the trace.

---

## 6. WASM Interoperability

Nulang's WASM strategy is simple: **compile everything to WASM components, run everything in Wasmtime.** This is not a compilation target of convenience -- it is the core architectural decision that enables security, portability, and cross-language interop.

### Compilation Target: wasm32-wasip2

Every Nulang actor compiles to a WASM component conforming to the WebAssembly Component Model specification. The compilation process:

1. Nulang source is parsed and type-checked
2. Actor protocols are extracted and converted to WIT interfaces
3. Actor implementations are compiled to WASM core modules
4. The WIT interface + WASM module are packaged as a WASM component

```
+----------------------------------+
|        Nulang Source File         |
|  actor MyActor { ... }            |
+----------------------------------+
           |
           v
+----------------------------------+
|        Nulang Compiler            |
|  - Parse                          |
|  - Type check (HM + effects)      |
|  - Extract WIT interfaces         |
|  - Generate WASM core module      |
+----------------------------------+
           |
           v
+----------------------------------+
|        WASM Component             |
|  +-----------------------------+  |
|  | WIT Interface (protocol)    |  |
|  | - behaviors                 |  |
|  | - types                     |  |
|  | - imports (capabilities)    |  |
|  +-----------------------------+  |
|  +-----------------------------+  |
|  | WASM Core Module (impl)     |  |
|  | - linear memory             |  |
|  | - functions                 |  |
|  +-----------------------------+  |
+----------------------------------+
```

### Actor Protocols as WIT Interfaces

A Nulang actor protocol:

```nulang
protocol OrderService {
  behavior place_order(order: Order) -> Result<OrderId, Error>
  behavior get_order(id: OrderId) -> Option<Order>
  behavior cancel_order(id: OrderId) -> Result<(), Error>
}
```

Compiles to:

```wit
interface order-service {
  record order { ... }
  record order-id { ... }
  variant error { ... }

  place-order: func(order: order) -> result<order-id, error>
  get-order: func(id: order-id) -> option<order>
  cancel-order: func(id: order-id) -> result<_, error>
}
```

### Cross-Language Composition

Because actor protocols are WIT interfaces, any language that compiles to WASM components can implement a Nulang actor protocol:

- **Rust**: Use `cargo-component` to generate bindings from WIT, implement the interface
- **Go**: Use TinyGo with Component Model support
- **C/C++**: Use `wit-bindgen` for C
- **Nulang**: Native -- WIT generation is built into the compiler

This enables a Nulang system to gradually incorporate components written in other languages. The boundary is always the WIT interface -- there is no O(N^2) FFI problem.

### WASI Worlds for Capability Sandboxing

Nulang capabilities map to WASI world imports. A Nulang actor that requires `FileSystemRead`, `HttpClient`, and `DatabaseQuery` compiles to a WASM component that imports those interfaces from its host world.

```wit
world actor-world {
  import wasi:filesystem/filesystem@0.2.0
  import wasi:http/outgoing-handler@0.2.0
  import nulang:database/query@1.0.0

  export order-service
}
```

The runtime (Wasmtime) provides implementations of these imports. If an actor does not import `FileSystemWrite`, it structurally cannot write files. This is capability-based security enforced by the WASM sandbox, not by runtime checks.

### Building Block Pattern

Nulang adopts Dapr's building block pattern, adapted for WASM:

| Building Block | WIT Interface | Default Backend | Pluggable Backends |
|---------------|---------------|----------------|-------------------|
| State store | `nulang:state/store` | SQLite (local) | PostgreSQL, Redis, DynamoDB |
| Pub/sub | `nulang:messaging/pubsub` | In-memory | NATS, Kafka, SQS |
| Key-value | `nulang:state/kv` | SQLite | Redis, DynamoDB, Consul |
| Blob store | `nulang:blob/store` | Filesystem | S3, GCS, Azure Blob |
| Secrets | `nulang:secrets/manager` | Environment vars | HashiCorp Vault, AWS Secrets Manager |
| Configuration | `nulang:config/store` | TOML files | etcd, Consul, AWS Parameter Store |
| Lock | `nulang:distributed/lock` | In-memory (single node) | Redis Redlock, etcd |
| LLM | `nulang:ai/llm` | OpenAI | Anthropic, Ollama, Azure |

Each building block is a WIT interface. The runtime provides the implementation. Actors declare which building blocks they need via capability imports. At deployment, the operator configures which backend each building block uses.

This gives infrastructure portability: the same WASM binary runs with local SQLite in development and PostgreSQL in production, with no code changes.

### Wasmtime as Embedded Runtime

Nulang embeds Wasmtime as its WASM runtime. Key configuration:
- **Fuel metering**: Limit compute per actor activation (prevention of runaway computation)
- **Memory limits**: Configurable max memory per actor (default: 100MB)
- **Async support**: Wasmtime's async support enables cooperative yielding
- **Component Model**: Full support for the Component Model specification

Actor activation reuse: After an actor finishes processing a message, its WASM instance is returned to a pool rather than destroyed. This minimizes cold-start latency to microsecond-level for subsequent messages.

---

## 7. Capability Networking Model

Capabilities in Nulang are not just a type-system concept. They are a distributed security mechanism that controls what actors can do across the network.

### Capabilities as First-Class Values

A capability is a value that combines:
1. **Authority**: Permission to perform an operation
2. **Identity**: Which resource the operation applies to
3. **Constraints**: Optional restrictions (read-only, time-limited, rate-limited)

```nulang
// A capability to read from a specific database table
capability DatabaseRead { table: "orders" }

// A capability to send email, rate-limited to 100/hour
capability EmailSend { rate_limit: "100/hour" }

// A capability to call an LLM, cost-limited to $100/day
capability LLM { budget: "$100/day" }
```

Capabilities are types in the Nulang type system. A function that requires a capability declares it:

```nulang
fn process_orders(db: capability DatabaseRead) -> Result {
  // Can read from the database, but not write
  let orders = effect DBQuery(db, "SELECT * FROM orders")
  ...
}
```

### Delegation

Capabilities can be passed between actors. When actor A holds a capability and sends a message to actor B, it can include the capability in the message. Actor B can then use that capability (subject to any constraints) but cannot escalate it.

```nulang
actor OrderService {
  capability DatabaseRead { table: "orders" }

  behavior get_order(id: OrderId) {
    // Passes a narrower capability to the child actor
    let child = activate OrderValidator
    child.validate(id, capability DatabaseRead { table: "orders", read_only: true })
  }
}
```

Key rule: capabilities can only be narrowed when delegated, never broadened. If A has `DatabaseRead { table: "orders" }`, it can delegate `DatabaseRead { table: "orders", columns: ["id", "status"] }` but not `DatabaseWrite` or `DatabaseRead { table: "customers" }`.

### Revocation

Capabilities can be revoked. When an actor is deactivated or a session ends, all capabilities it holds are revoked. Revocation is lazy: the capability token is added to a revocation list, and subsequent uses are rejected.

For time-limited capabilities, expiration is checked at each use. No network call is needed -- the token contains an expiry timestamp signed by the issuer.

### Distributed Authority

In a distributed cluster, capability verification works as follows:

```
Actor A (Node 1)        Node 1 Runtime        Node 2 Runtime        Actor B (Node 2)
     |                         |                      |                    |
     | send(msg, cap)          |                      |                    |
     |------------------------>|                      |                    |
     |                         | sign msg+cap         |                    |
     |                         | with node key        |                    |
     |                         |--------------------->|                    |
     |                         |                      | verify signature   |
     |                         |                      |------------------->|
     |                         |                      |                    |
     |                         |                      |<-------------------|
     |                         |                      | (cap valid/invalid)|
     |                         |<---------------------|                    |
     |                         | (delivery/failure)                        |
```

Each node has an Ed25519 key pair. Messages between nodes are signed. Capabilities contain the issuer's public key and a signature chain. Verification checks:
1. The capability signature chain is valid
2. The capability has not expired
3. The capability has not been revoked (checked against local revocation cache)
4. The delegator had authority to delegate this capability

### Mapping to WASI

Nulang capabilities compile to WASI world imports:

| Nulang Capability | WASI Import | Notes |
|------------------|-------------|-------|
| `FileSystemRead` | `wasi:filesystem/filesystem` | Read-only, path-restricted |
| `FileSystemWrite` | `wasi:filesystem/filesystem` | Write access, path-restricted |
| `HttpClient` | `wasi:http/outgoing-handler` | Outgoing requests only |
| `DatabaseQuery` | `nulang:database/query` | Custom Nulang interface |
| `LLM` | `nulang:ai/llm` | Custom Nulang interface |

At runtime, the Nulang runtime provides implementations of these imports that perform capability checks before forwarding to the actual backend.

### Network-Level Capability Tokens

Messages between actors carry capability tokens in their headers. A token is a compact, signed JWT-like structure:

```json
{
  "iss": "node-1.nulang.cluster",
  "sub": "actor-order-service-42",
  "cap": [{
    "type": "DatabaseRead",
    "resource": "orders",
    "constraints": { "columns": ["id", "status"] }
  }],
  "iat": 1704067200,
  "exp": 1706659200,
  "delegation": [
    { "from": "node-admin", "ts": 1704067200 }
  ]
}
```

Tokens are verified by the receiving node's runtime before the message is delivered to the target actor. Invalid or expired tokens result in message rejection with a capability error.

---

## 8. Cloud Deployment Architecture

Nulang Cloud is the deployment target for Nulang applications. The vision: `git push` → build → deploy → scale, with zero infrastructure configuration for the common case.

### The Deployment Unit: Realm

A **realm** is a collection of actors deployed together with shared routing and scaling policies. Realms are the unit of deployment, not individual actors.

```nulang
realm ECommerce {
  // Actor definitions or references
  actors {
    OrderService,
    PaymentService,
    InventoryService,
    NotificationService
  }

  // Routing: how messages find actors
  routing {
    consistent_hash  // default: consistent hashing on actor ID
  }

  // Scaling policy
  scaling {
    min_nodes: 2
    max_nodes: 20
    metric: mailbox_depth    // scale based on queue depth
    target_value: 100        // target: 100 messages per mailbox
  }

  // Regions for geo-replication
  regions: ["us-east-1", "eu-west-1", "ap-south-1"]

  // State model defaults
  default_state_model: durable

  // Persistence backend
  persistence {
    backend: postgresql
    connection: env("DATABASE_URL")
  }
}
```

### Deployment Flow

```
Developer Machine              CI/CD                Nulang Cloud
     |                           |                       |
     | git push                  |                       |
     |------------------------->|                       |
     |                           | nulang build          |
     |                           |---------------------->|
     |                           |                       |
     |                           |<----------------------|
     |                           | (WASM components)     |
     |                           | nulang deploy         |
     |                           |---------------------->|
     |                           |                       |
     |<--------------------------------------------------|
     |                           |       (deployed)      |
```

### Multi-Region Replication

Nulang supports three replication strategies:

| Strategy | State Models | Consistency | Latency | Use Case |
|----------|-------------|-------------|---------|----------|
| Single region | All | Strong | Low | Development, compliance |
| Active-passive | `durable`, `event_sourced` | Strong (primary) | Cross-region failover | Most production workloads |
| Active-active | `crdt` only | Eventual | Low everywhere | Geo-distributed apps |

**Active-passive**: One region is primary for each actor. Writes go to the primary. Reads can be served from replicas with configurable staleness. On primary failure, a replica is promoted.

**Active-active**: CRDT actors are active in all regions simultaneously. Updates are applied locally and sync via the CRDT engine. No primary, no failover.

### Auto-Scaling

Nulang auto-scales at two levels:

**Node scaling**: The number of runtime nodes in a realm scales based on:
- Mailbox depth (primary signal): if average mailbox depth > target, add nodes
- CPU utilization: if average CPU > 70%, add nodes
- Memory utilization: if average memory > 80%, add nodes

**Actor placement**: Within nodes, actors are placed using consistent hashing. Hot actors (high message volume) can be:
- **Migrated**: Moved to a less loaded node
- **Replicated**: For `crdt` actors, add replicas in other regions
- **Sharded**: For actors with large state, partition by sub-key

### Zero-Downtime Updates

Updates are deployed using blue-green deployment at the actor level:

1. New version of actor code is deployed alongside the old version
2. New messages are routed to the new version
3. Old actors continue processing their current message, then checkpoint
4. After checkpoint, old actors are deactivated
5. When all old actors are gone, old code is removed

This means:
- No dropped messages
- No interruption to long-running workflows
- Rollback is instant: route messages back to the old version
- Database schema changes are handled via migrations specified in the deployment config

### Runtime Targets

Nulang runs on multiple runtime targets, from a single binary on a laptop to a globally distributed cluster:

| Target | Use Case | WASM Runtime | Orchestration |
|--------|---------|--------------|---------------|
| **Local dev** | Development, testing | Wasmtime (embedded) | None (single process) |
| **Single node** | Small production, edge | Wasmtime | systemd / Docker |
| **Kubernetes** | Medium production | Wasmtime | Custom controller + operator |
| **Edge** | CDN edge workers | WasmEdge | Cloudflare Workers, Fastly Compute |
| **Serverless** | Event-driven, pay-per-use | Wasmtime | Nulang Cloud (managed) |

The same WASM binary runs on all targets. Target-specific configuration (which persistence backend, which message queue) is specified at deploy time, not in code.

**Local development**: The local target runs the entire Nulang stack in a single process. Persistence uses SQLite, messaging uses in-memory queues, and the LLM backend can be a local Ollama instance. This enables a developer to run a complete distributed system on their laptop with zero external dependencies.

```bash
nulang run                    # Start local runtime
nulang run --hot-reload       # Auto-restart on file changes
nulang run --debug            # Enable distributed debugger
```

**Kubernetes deployment**: Nulang provides a Kubernetes operator that manages Nulang runtime nodes as a StatefulSet. The operator handles:
- Node provisioning and lifecycle management
- Actor placement and rebalancing
- Persistent volume claims for event journals
- Service mesh integration (optional -- Nulang's own routing can replace Istio for inter-actor communication)
- Horizontal pod autoscaling based on mailbox depth metrics

```yaml
# nulang-realm.yaml
apiVersion: nulang.io/v1
kind: Realm
metadata:
  name: e-commerce
spec:
  image: registry.nulang.io/ecommerce:v1.2.3
  replicas: 5
  regions:
    - us-east-1
    - eu-west-1
  persistence:
    backend: postgresql
    secretRef: db-credentials
  scaling:
    minReplicas: 3
    maxReplicas: 50
    metric: mailbox_depth
```

**Edge deployment**: Nulang actors can run on edge WASM runtimes (WasmEdge, Cloudflare Workers, Fastly Compute). Edge actors are `local` state only (no durability at the edge) but can communicate with durable actors in central regions. This enables latency-sensitive operations (A/B testing, personalization, request validation) to run at the edge while stateful operations run in the cloud.

### Configuration

Deployment configuration is declarative:

```nulang
// deployment.nul
realm MyApp {
  regions: ["us-east-1", "eu-west-1"]

  persistence {
    backend: postgresql
    url: env("DATABASE_URL")
  }

  messaging {
    backend: nats
    url: env("NATS_URL")
  }

  llm {
    provider: openai
    api_key: secret("openai-api-key")
  }

  scaling {
    min_nodes: 3
    max_nodes: 50
    metric: mailbox_depth
  }
}
```

Environment variables and secrets are resolved at deploy time, not build time. Secrets are stored in the configured secrets backend and injected as capabilities.

```toml
# nulang.toml (project manifest)
[package]
name = "ecommerce-backend"
version = "1.2.3"
edition = "2025"

[dependencies]
nulang-http = "1.2.0"
nulang-postgres = "2.1.0"
nulang-openai = "3.0.1"

[dev-dependencies]
nulang-test = "2.0.0"

[capabilities]
database-read = { tables = ["orders", "customers"] }
llm = { provider = "openai", model = "gpt-4o" }
http-client = { allow_hosts = ["api.stripe.com", "api.sendgrid.com"] }

[deployment]
realm = "ecommerce"
regions = ["us-east-1", "eu-west-1"]
persistence = "postgresql"
scaling = { min_nodes = 3, max_nodes = 50 }
```

---

## 9. Developer Tooling Roadmap

Developer tooling is not a nice-to-have. It is the primary determinant of adoption. Nulang's tooling strategy prioritizes the tools that unlock the fastest developer feedback loops.

### Priority 1: Language Server Protocol (LSP)

**Status: Required for v0.1**

The LSP server provides:
- Type checking on every keystroke
- Auto-completion for actor behaviors, effects, and capabilities
- Go-to-definition across actor boundaries
- Real-time error reporting (type errors, capability violations, protocol mismatches)
- Rename refactoring across the codebase

The LSP reuses the Nulang compiler's type checker directly. There is no separate analysis engine -- the LSP IS the compiler running in watch mode.

### Priority 2: Formatter

**Status: Required for v0.1**

A deterministic formatter (like `gofmt` or `rustfmt`) that all code must pass. No configuration options. This eliminates style debates and makes all Nulang code look the same.

### Priority 3: Package Manager

**Status: Required for v0.2**

Packages are WASM components distributed via a registry. A package can contain:
- Nulang actors (compiled to WASM)
- WIT interfaces
- Building block implementations
- Configuration templates

Dependencies are resolved at the WIT interface level. If package A depends on interface `orderservice@1.0.0`, any package providing that interface satisfies the dependency.

The package manager handles versioning, deduplication, and WIT interface compatibility checking. Packages are published to a registry (public or private) and downloaded on first build.

```bash
nulang init                     # Initialize a new project
nulang add nulang/http@1.2.0   # Add a dependency
nulang add --dev nulang/test@2.0.0  # Add a dev dependency
nulang publish                  # Publish to registry
nulang update                   # Update all dependencies
```

Package structure:
```
my-package/
  nulang.toml           # Package manifest
  src/
    actors.nul          # Source files
  wit/
    interfaces.wit      # Exported WIT interfaces
  tests/
    integration.nul     # Test files
```

The `nulang.toml` manifest specifies name, version, dependencies, exported capabilities, and building block requirements. The lock file (`nulang.lock`) pins exact versions and WASM content hashes for reproducible builds.

### Priority 4: Documentation Generator

**Status: Required for v0.2**

Generates documentation from actor protocols, WIT interfaces, and doc comments. Produces browsable HTML with:
- Actor behavior signatures with types
- Protocol diagrams (which actors implement which protocols)
- Effect and capability listings
- Cross-references between actors

### Priority 5: Testing Framework

**Status: Required for v0.2**

Nulang's testing framework includes:

- **Unit tests**: Test individual behavior handlers with mock effects
- **Actor tests**: Test an actor's message protocol in isolation
- **Integration tests**: Test actor interactions within a local cluster
- **Replay tests**: Record an actor's message stream and replay it to verify deterministic behavior
- **Property tests**: Generate random message sequences to test actor invariants

Replay testing is unique to Nulang: since all actor state is deterministic given its message history, you can record a production actor's messages, replay them in a test, and assert that the final state matches. This catches non-determinism bugs and regression errors.

```nulang
test "order processing replay" {
  let recording = load_recording("order-42-production.json")
  let actor = spawn OrderProcessor with state OrderProcessor.default_state
  replay(recording.messages, actor)
  assert actor.state == recording.final_state
}
```

**Testing philosophy**: Nulang tests are actor-centric, not function-centric. A unit test sends messages to an actor and asserts on its responses and state changes. An integration test starts a local cluster, deploys multiple actors, and verifies their interactions. This matches how Nulang applications are actually built -- as collections of communicating actors -- rather than forcing a function-oriented testing model onto an actor-oriented language.

### Priority 6: VS Code Extension

**Status: Required for v0.3**

Bundles the LSP, formatter, and a workflow visualizer. Provides:
- Syntax highlighting
- In-editor type errors
- "Run test" and "Debug actor" buttons
- Workflow graph visualization
- Actor topology viewer (which actors exist, their state, message counts)

### Priority 7: Debugger

**Status: Required for v0.4**

A distributed, actor-aware debugger:
- Step through behavior handlers
- Inspect actor state (including across nodes)
- Set breakpoints on specific message types
- Trace message paths through the system
- Time-travel debugging for event-sourced actors (step backward through events)

The debugger connects to a running cluster via a WebSocket protocol. It can attach to any actor, anywhere in the cluster, without stopping the system.

### Priority 8: Cloud Deployment CLI

**Status: Required for v0.4**

```bash
nulang login                    # Authenticate with Nulang Cloud
nulang deploy                   # Deploy current directory
nulang logs --actor OrderService # Stream logs
nulang scale --realm MyApp --nodes 10  # Manual scaling
nulang rollback --version 42    # Rollback to previous version
```

### Priority 9: Workflow Visualizer

**Status: Required for v0.5**

A web-based tool that shows:
- Workflow definitions as graphs
- Active workflow instances with real-time progress
- Step execution times and success/failure rates
- Human approval pending states
- Compensation chains

### Priority 10: Actor Inspector

**Status: Required for v0.5**

A web-based distributed systems dashboard:
- Actor topology: which actors exist, where they run, their state
- Message flow: visualizing message paths between actors
- State inspection: viewing actor state (with permission)
- Event journal viewer: browsing event-sourced actor histories
- Performance metrics: latency, throughput, error rates per actor

---

## 10. Migration Plan

This redesign represents a significant evolution from the current Nulang implementation. The migration is organized into five phases, each delivering independent value.

### Phase 1: Remove AI Agent DSL (Month 1-2)

**Goal**: Eliminate the separate AI agent DSL. Make agents regular actors.

**What changes**:
- Remove `agent`, `tool`, `prompt`, and `memory` keywords from the language
- Convert all agent definitions to `actor` definitions with `capability LLM`
- Convert tool definitions to `behavior` definitions on regular actors
- Migrate `agent` memory to actor state + vector store capability

**What stays**: Actor model, supervision, effects, capabilities

**Migration for users**:
```nulang
// Before
agent Researcher {
  tool search(query: String) -> Results
  prompt "You are a research assistant"
}

// After
actor Researcher {
  capability LLM

  behavior search(query: String) -> Results {
    effect LLMComplete({ model: GPT4O, messages: [...] })
  }
}
```

This is a mechanical transformation. The migration tool can auto-convert 90% of agent definitions.

**Risk mitigation**: Phase 1 changes the surface syntax but not the runtime semantics. Existing applications continue to run. The migration is source-code-only. A compatibility shim allows old `.agent` files to be imported as actors during the transition period.

### Phase 2: Add Persistent Actor Keyword + Durable Execution (Month 3-5)

**Goal**: Add the `persistent` keyword, state models, and durable execution.

**What changes**:
- Add `persistent`, `event_sourced`, and `crdt` keywords
- Implement the state persistence engine (SQLite, PostgreSQL backends)
- Implement the event journal (append-only log)
- Implement checkpoint manager and recovery orchestrator
- Add the `durable` and `event_sourced` state models

**What stays**: All existing non-persistent actors continue to work (they are `local` by default)

**New capabilities**:
- Any actor can become durable by adding the `persistent` keyword
- Event-sourced actors get full audit trails
- Automatic crash recovery

**Testing strategy**: Phase 2 introduces the most critical runtime component: durability. The testing plan includes:
- **Chaos testing**: Randomly kill runtime nodes and verify all actors recover correctly
- **Property testing**: Generate random message sequences, crash at random points, verify no state is lost
- **Performance testing**: Measure checkpoint latency under load, target <5ms p99
- **Replay testing**: Record production traffic, replay in staging, verify identical outputs

**Risk mitigation**: Phase 2 is backward-compatible. Non-persistent actors work exactly as before. Persistent actors are opt-in. A feature flag enables durability on a per-actor basis, allowing gradual rollout.

### Phase 3: Add Workflow Syntax (Month 6-8)

**Goal**: Add the `workflow` keyword and compile workflows to actor graphs.

**What changes**:
- Add `workflow`, `step`, `parallel`, `compensate`, `await` keywords
- Build the workflow compiler (workflow AST → actor graph)
- Implement child actor spawning for workflow steps
- Implement compensation chains
- Implement human-in-the-loop suspension/resumption

**What stays**: All existing actors work unchanged

**New capabilities**:
- Declare workflows that compile to durable actor graphs
- Built-in saga compensation
- Human-in-the-loop approvals
- Parallel step execution
- Time-based operations (sleep, timeout, schedule)

**Implementation approach**: The workflow compiler is a Nulang-to-Nulang transformation. It takes a `workflow` definition and generates an `actor` definition plus child `actor` definitions for each step. This means the workflow subsystem reuses all existing infrastructure: the actor runtime for execution, the durable execution layer for persistence, and the message router for distribution. There is no separate workflow engine.

**Risk mitigation**: Workflows are purely additive -- no existing syntax changes. The workflow compiler is tested by comparing its output against hand-written actor equivalents for a suite of workflow patterns.

### Phase 4: WASM Component Compilation (Month 9-11)

**Goal**: Compile Nulang to WASM components instead of native binaries.

**What changes**:
- Replace the native code generator with a WASM component generator
- Implement WIT interface extraction from actor protocols
- Integrate Wasmtime as the embedded runtime
- Implement capability-to-WASI mapping
- Build the building block abstraction (pluggable backends)

**What stays**: All source code (Nulang syntax is unchanged)

**New capabilities**:
- Cross-language composition (Rust, Go, C actors)
- Sandboxed execution (security isolation per actor)
- Portable deployment (same binary on all targets)
- WASM-based hot code reloading

**Implementation approach**: The WASM compiler is built alongside the existing native compiler during Phase 4. Both compilation targets are supported, with WASM becoming the default for new projects. The native compiler remains for debugging and platforms where WASM is not yet supported.

**Risk mitigation**: The WASM compiler is validated by compiling the entire Nulang test suite to WASM and running it in Wasmtime. Performance parity with the native compiler is a requirement -- no more than 20% overhead on message processing latency.

### Phase 5: Cloud Deployment Tooling (Month 12-14)

**Goal**: Build the cloud deployment platform.

**What changes**:
- Build the realm abstraction and deployment engine
- Implement auto-scaling (mailbox depth, CPU, memory signals)
- Implement multi-region replication (active-passive and active-active)
- Build the zero-downtime deployment system
- Build the CLI and web dashboard

**What stays**: All actor code (deployment is infrastructure, not application code)

**New capabilities**:
- `git push` deployment
- Automatic scaling
- Multi-region deployment
- Managed persistence and messaging backends

**Risk mitigation**: Cloud deployment is entirely opt-in. Self-hosted Nulang (single binary on a VM) continues to work. The cloud platform is a value-add, not a requirement. All deployment tooling is open source, so users can run their own deployment infrastructure if desired.

**Timeline summary**:

| Phase | Duration | Key Deliverable | Backward Compatible |
|-------|----------|----------------|-------------------|
| Phase 1: Remove Agent DSL | Month 1-2 | Actors as unified abstraction | Yes |
| Phase 2: Durable Execution | Month 3-5 | `persistent` keyword, checkpointing | Yes |
| Phase 3: Workflow Syntax | Month 6-8 | `workflow` keyword, saga support | Yes |
| Phase 4: WASM Compilation | Month 9-11 | WASM component output | Yes |
| Phase 5: Cloud Deployment | Month 12-14 | Managed platform, CLI | Yes |

### What Stays from Current Nulang

The following are foundational and do not change:

| Feature | Status | Rationale |
|---------|--------|-----------|
| Actor model | Core | The fundamental abstraction |
| Supervision trees | Core | Fault tolerance mechanism |
| Hindley-Milner types | Core | Type system foundation |
| Algebraic effects | Core | Side effect tracking and control |
| Capabilities | Core | Security model |
| CRDTs | Core | Distributed state without coordination |
| Distributed runtime | Core | Cluster formation, message routing |
| ORCA GC | Core | Actor-local garbage collection |

### What Changes

| Feature | Change | Rationale |
|---------|--------|-----------|
| AI Agent DSL | **Removed** | Agents are actors with LLM capability |
| Actor addressing | **Unified** | Virtual actors by default (Orleans model) |
| State models | **Explicit** | `local` / `durable` / `event_sourced` / `crdt` |
| Compilation target | **WASM** | wasm32-wasip2 components |
| Persistence | **Built-in** | Not bolted-on, integrated into runtime |
| Workflows | **New** | Declarative workflow syntax |
| Cloud deployment | **New** | Managed platform with auto-scaling |

---

## Appendix: Comparison with Existing Systems

### Nulang vs. Temporal

| Aspect | Temporal | Nulang |
|--------|----------|--------|
| Programming model | Async/await in existing languages | Native actor language |
| Durability | Event sourcing + replay | Multiple state models |
| Distribution | Separate service cluster | Built into language runtime |
| AI agents | Not a focus | First-class AI runtime |
| Type safety | Language-level | End-to-end (WIT interfaces) |
| Security | Role-based | Capability-based |

**Verdict**: Temporal is excellent for workflow durability in existing codebases. Nulang replaces Temporal when workflows are part of a larger distributed system that also needs actors, AI agents, and edge deployment.

### Nulang vs. Akka

| Aspect | Akka | Nulang |
|--------|------|--------|
| Actor model | Explicit creation | Virtual actors |
| Persistence | Optional plugin | Built-in by default |
| Type safety | Akka Typed (optional) | Required from day one |
| Effects | Untracked | Tracked in type system |
| Security | No built-in model | Capability-based |
| License | BSL | Apache 2.0 |

**Verdict**: Akka is a Java/Scala library. Nulang is a language. If you need actors within a larger JVM application, Akka is appropriate. If you are building a distributed system from scratch, Nulang provides an integrated stack that Akka cannot match.

### Nulang vs. LangGraph

| Aspect | LangGraph | Nulang |
|--------|-----------|--------|
| Execution | Single Python process | Distributed cluster |
| Durability | Checkpoints (optional) | Built-in with multiple models |
| Workflows | Graph-based state machines | Actor graphs |
| Human-in-the-loop | Interrupt mechanism | First-class await primitive |
| Scalability | Vertical only | Horizontal + vertical |
| AI integration | Core focus | Layer 5 of 5 |

**Verdict**: LangGraph is excellent for single-process AI agent prototyping. Nulang is for production AI agents that need durability, distribution, and integration with non-AI services.

### Nulang vs. Erlang/OTP

| Aspect | Erlang/OTP | Nulang |
|--------|-----------|--------|
| VM | BEAM | WASM (Wasmtime) |
| Process model | Lightweight processes | Virtual actors |
| Fault tolerance | "Let it crash" | Supervision + durable recovery |
| Type system | Dynamic | Static (HM + effects) |
| Hot code reloading | Yes | Yes (WASM module swap) |
| Persistence | None built-in | Core feature |
| AI support | None | Native runtime |

**Verdict**: Erlang/OTP remains unmatched for telecom-style soft real-time systems. Nulang brings Erlang's fault tolerance philosophy to modern cloud-native and AI-driven applications with static types, WASM sandboxing, and built-in durability.

---

## Conclusion

Nulang is the durable actor language for building AI agents, workflows, and distributed systems. It combines:

- **Virtual actors** (from Orleans) for simple, identity-based addressing
- **Durable execution** (from Temporal and Durable Functions) for crash-proof state
- **Effect tracking** (from algebraic effects research) for controlled side effects
- **Capability security** (from object-capability theory and WASI) for zero-trust security
- **WASM components** (from the WebAssembly Component Model) for sandboxed portability
- **CRDTs** (from distributed systems research) for coordination-free replicated state
- **Supervision trees** (from Erlang/OTP) for fault tolerance

Ten years from now, building a distributed system should be as simple as defining actors and their protocols. The runtime handles durability, distribution, scaling, and recovery. The type system prevents entire classes of bugs. The capability system makes security composable. And AI agents are just actors with an LLM capability -- no separate framework, no special case.

This document is the blueprint. The implementation starts now.

---

*"The world is concurrent. Things in the world don't share data. Things communicate with messages. Things fail."* -- Joe Armstrong, creator of Erlang

*Nulang takes this observation seriously and builds a complete platform around it.*
