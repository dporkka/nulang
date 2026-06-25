# Nulang Architecture Reference

**Document Version:** 1.0
**Date:** January 2025
**Audience:** Core implementers, runtime engineers, language designers
**Companion Document:** STRATEGY.md (strategic rationale and migration plan)

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Layer 1: Language](#2-layer-1-language)
3. [Layer 2: Actor Runtime](#3-layer-2-actor-runtime)
4. [Layer 3: Durable Execution](#4-layer-3-durable-execution)
5. [Layer 4: Distributed Platform](#5-layer-4-distributed-platform)
6. [Layer 5: AI Runtime](#6-layer-5-ai-runtime)
7. [Data Flow Diagrams](#7-data-flow-diagrams)
8. [Component Interaction Map](#8-component-interaction-map)
9. [Architecture Decision Records](#9-architecture-decision-records)
10. [Performance Targets](#10-performance-targets)
11. [References](#11-references)

---

## 1. System Overview

Nulang is organized into five strictly layered subsystems. Each layer communicates only with adjacent layers. This constraint ensures independent testability, replaceability, and evolution.

```
+==========================================================================+
|                           LAYER 5: AI RUNTIME                             |
|  Model Providers | Tool Registry | Memory (3-tier) | Planner | Traces   |
+==========================================================================+
                              |
+==========================================================================+
|                       LAYER 4: DISTRIBUTED PLATFORM                       |
|  Gossip Membership | CRDT Engine | Message Router | Service Discovery   |
|  Consistent Hash Ring | Multi-Region Replicator | NUL0 Transport        |
+==========================================================================+
                              |
+==========================================================================+
|                       LAYER 3: DURABLE EXECUTION                          |
|  State Models (4) | Checkpoint Manager | Event Journal | Recovery        |
|  Snapshot Engine | Replay Orchestrator | Determinism Enforcer           |
+==========================================================================+
                              |
+==========================================================================+
|                       LAYER 2: ACTOR RUNTIME                              |
|  Virtual Actor Manager | Work-Stealing Scheduler | Bounded Mailboxes    |
|  Supervision Trees | WASM Sandbox Pool | Location Directory | ORCA GC   |
+==========================================================================+
                              |
+==========================================================================+
|                       LAYER 1: LANGUAGE                                   |
|  Parser | HM Type Checker | Effect System | Capability Types | WIT Gen   |
+==========================================================================+
```

**Layer boundary rules:**
- Layer N may call only Layer N-1 and Layer N+1
- Cross-layer calls are forbidden (no Layer 5 → Layer 3 shortcuts)
- Data passes across boundaries as plain structs; no shared mutable state
- Each layer can be tested with mocked adjacent layers

---

## 2. Layer 1: Language

The Language layer is a pure compiler: Nulang source files in, WASM components out. It has zero runtime dependencies and can run entirely at compile time.

### 2.1 Syntax Design

Nulang uses an indentation-sensitive syntax. This is not a stylistic choice — it is a semantic choice that eliminates an entire class of bracket-matching bugs and forces a visual hierarchy that mirrors the actor hierarchy.

**Design decisions:**

| Decision | Rationale | Rejected Alternative |
|----------|-----------|---------------------|
| Indentation-based | Visual hierarchy matches actor nesting; no bracket noise | Braces (C-style) — visual clutter |
| Expression-oriented | Every construct returns a value; no `return` keyword needed | Statement-oriented — requires explicit returns |
| Actor as top-level | The actor is the only module-level construct | Module system separate from actors — extra layer of indirection |
| No semicolons | Newlines separate statements; semicolons allow multi-line | Mandatory semicolons — visual noise |

**Indentation rules:**
- 2 spaces per indentation level (not configurable — see ADR-001)
- Actor bodies are indented under the `actor` keyword
- Behavior bodies are indented under the `behavior` keyword
- Continuation lines indent 4 spaces past the start of the expression

```nulang
actor BankAccount:
  state:
    balance: Decimal

  behavior deposit(amount: Decimal):
    state.balance = state.balance + amount
    effect Log("Deposited {amount}, new balance: {state.balance}")

  behavior withdraw(amount: Decimal) -> Result<Unit, Error>:
    if state.balance >= amount:
      state.balance = state.balance - amount
      Ok(Unit)
    else:
      Err(InsufficientFunds)
```

**Expression orientation:** There is no `return` keyword. The last expression in a block is its value. This extends to behavior handlers, `if` branches, and `match` arms:

```nulang
behavior get_balance() -> Decimal:
  state.balance                    -- last expression = return value

behavior describe() -> String:
  if state.balance > 0:
    " solvent"                    -- if branch returns String
  else:
    " overdrawn"                  -- else branch returns String
```

**Pattern matching:** Nulang uses exhaustively-checked pattern matching for all conditional destructuring. There is no `switch` statement, no `if-else` chains on enums:

```nulang
match payment_result:
  case Success(tx_id):
    effect Log("Payment {tx_id} succeeded")
    confirm_order(order)
  case Failure(NetworkError):
    retry_after(Duration.seconds(5))
  case Failure(Declined(reason)):
    cancel_order(order, reason)
  case Failure(FraudSuspected):
    escalate_to_security(order)
```

Non-exhaustive matches are compile-time errors. The compiler suggests missing cases.

### 2.2 HM Type Inference with Extensions

Nulang uses Hindley-Milner type inference with four extensions: parameterized types, row-polymorphic effects, capability types, and constrained types.

**Core algorithm:** Standard HM with let-generalization. Type variables are unified via Robinson's algorithm. The type checker is a constraint generator + solver, producing a typed AST.

**Extension 1: Parameterized types (ML-style generics).**

```nulang
-- Generic mailbox protocol
protocol Mailbox<T>:
  behavior send(msg: T)
  behavior receive() -> T

-- Usage: Mailbox<Order> is a distinct type from Mailbox<Invoice>
```

Generics are monomorphized at compile time for WASM generation. There is no runtime boxing of generic values.

**Extension 2: Row-polymorphic effects.**

Every function type carries an effect row: an unordered set of effects the function may perform. Effect rows are open by default (prefixed with `..`), enabling composition:

```nulang
-- Effect row is open: the function requires at least IO and Log
fn process_file(path: String) : [IO, Log, ..] -> String

-- A caller with [IO, Log, DB] can call process_file (DB is extra, allowed)
-- A caller with [IO] cannot (Log is missing)
```

Effect rows are represented internally as sorted, duplicate-free lists. Unification of effect rows uses row unification: `{a, b | r1}` unifies with `{b, c | r2}` by producing `{a, b, c | r3}` where `r3` is the union of the remainders.

**Extension 3: Capability types.**

Capabilities are types that represent authority. They are distinct from regular types — a `capability DatabaseRead` is not a struct, an enum, or a function. Capabilities appear in effect rows and function signatures:

```nulang
-- Function requires the DatabaseRead capability
fn query_orders(db: capability DatabaseRead) -> List<Order>

-- Actor holds the capability; behaviors implicitly have access
actor OrderService:
  capability DatabaseRead

  behavior list_orders():
    -- No need to pass 'db' explicitly; the actor holds it
    effect DBQuery("SELECT * FROM orders")
```

**Extension 4: Constrained types.**

Type constraints restrict the set of valid types for a parameter:

```nulang
-- T must be serializable (for message passing)
fn send_message<T: Serializable>(mailbox: Mailbox<T>, msg: T)

-- T must be comparable (for CRDT merge)
fn merge_values<T: Comparable>(a: T, b: T) -> T
```

Built-in type classes: `Serializable`, `Comparable`, `Hashable`, `Default`, `Clone`.

**Type inference across actor boundaries:** There is NONE. Actor protocols are fully explicit — every behavior parameter and return type must be annotated. This is deliberate: it enables separate compilation, protocol evolution, and cross-language implementation. Within an actor's behavior handlers, local functions are fully inferred.

### 2.3 Effect System Integration with the Actor Model

Nulang's effect system is the bridge between actor code and the runtime. Effects are not just types — they are the mechanism by which actors interact with the world.

**Effect lifecycle:**

```
Actor code calls `effect Foo(arg)`
         |
         v
Runtime intercepts the effect (WASM host function)
         |
         v
Effect handler executes (may be async, may access external systems)
         |
         v
Result returned to actor; optionally captured in checkpoint
```

**Effect categories:**

| Category | Examples | Handler Location | Captured in Journal? |
|----------|----------|-----------------|---------------------|
| IO | `HttpGet`, `HttpPost` | Runtime IO thread | Yes |
| Time | `Now`, `Sleep` | Runtime clock | Yes |
| Random | `Random`, `RandomInt` | Runtime entropy | Yes |
| Storage | `DBQuery`, `KVGet` | Building block backend | Yes |
| Messaging | `Send`, `Ask` | Actor runtime | Yes (send recorded) |
| AI | `LLMComplete` | AI runtime | Yes |
| Logging | `Log`, `Metrics` | Observability subsystem | No (non-deterministic) |
| Capability | `Require`, `Delegate` | Capability manager | No |

**Effect handlers are pluggable.** The runtime maintains an effect handler registry. Each effect name maps to a handler function. For testing, handlers can be swapped:

```nulang
test "order processing with mock payment":
  let mock_handler = MockHttp({
    "POST /charge": Ok({"status": "approved", "ref": "tx-123"})
  })
  with_handler(HttpPost, mock_handler):
    let result = order_processor.process(test_order)
    assert result == Ok(OrderConfirmed("tx-123"))
```

**Effects and determinism:** For `durable` and `event_sourced` actors, effect results are captured. On replay, the stored result is returned without re-invoking the handler. This is how Nulang implements deterministic replay — not by recording all IO at the OS level (like Temporal records gRPC calls), but by capturing at the effect boundary.

### 2.4 Capability Syntax and Type Integration

Capabilities are first-class types and first-class values. They can be held, passed, narrowed, and revoked.

**Capability syntax:**

```nulang
-- Actor declares it holds a capability with optional constraints
capability DatabaseRead { tables: ["orders", "customers"] }
capability HttpClient { hosts: ["api.stripe.com"] }
capability LLM { model: GPT4O, budget: "$100/day" }

-- Capabilities can be passed as parameters
behavior delegate_reader(consumer: ActorRef, read_cap: capability DatabaseRead):
  consumer.accept(read_cap)  -- consumer can now use this capability

-- Capabilities can be narrowed at delegation
behavior narrow_and_delegate():
  let narrow_cap = capability DatabaseRead {
    tables: ["orders"],       -- subset of original tables
    columns: ["id", "status"]  -- further restriction
  }
  child_actor.work(narrow_cap)
```

**Capability narrowing rules:** A capability can only be narrowed when delegated. Narrowing means reducing the set of allowed operations, adding constraints, or reducing scope. A broader capability cannot be created from a narrower one.

| Original | Delegated | Valid? |
|----------|-----------|--------|
| `DatabaseRead { tables: ["orders"] }` | `DatabaseRead { tables: ["orders"] }` | Yes (exact) |
| `DatabaseRead { tables: ["orders", "customers"] }` | `DatabaseRead { tables: ["orders"] }` | Yes (subset) |
| `DatabaseRead { tables: ["orders"] }` | `DatabaseRead { tables: ["orders", "customers"] }` | No (superset) |
| `HttpClient { hosts: ["api.stripe.com"] }` | `HttpClient { hosts: ["api.stripe.com"], rate_limit: "100/h" }` | Yes (added constraint) |
| `DatabaseRead { tables: ["orders"] }` | `DatabaseWrite { tables: ["orders"] }` | No (different capability) |

**Capability type checking:** The type checker verifies that every `effect` invocation is covered by a capability held by the actor or passed as a parameter. Violations are compile-time errors, not runtime failures.

```nulang
actor UnsafeActor:
  -- No HttpClient capability declared

  behavior fetch_data():
    effect HttpGet("https://example.com")  -- COMPILE ERROR: missing capability HttpClient
```

**Capability revocation:** Capabilities are revoked in three scenarios:
1. **Actor deactivation:** When an actor is deactivated, all its capabilities are revoked
2. **Explicit revocation:** An actor can revoke a capability it previously delegated
3. **Timeout:** Time-limited capabilities expire automatically

Revocation is lazy: the capability token is added to a revocation list. The next use of the token is rejected. There is no active "kill switch" that interrupts in-progress operations.

### 2.5 Compilation Pipeline

```
+-------------------------------------------------------------------+
|  Nulang Source (.nul files)                                        |
|  actor MyActor { ... }                                            |
+-------------------------------------------------------------------+
                            |
                            v
+-------------------------------------------------------------------+
|  1. Lexer + Parser                                                 |
|     - Indentation-sensitive token stream                           |
|     - Produces untyped AST                                         |
|     - Error recovery: continues past syntax errors                 |
+-------------------------------------------------------------------+
                            |
                            v
+-------------------------------------------------------------------+
|  2. Type Checker (HM + Effects + Capabilities)                    |
|     - Constraint generation: HM unification + effect row unification|
|     - Capability checking: every effect must be covered            |
|     - Exhaustiveness: all pattern matches must be complete         |
|     - Produces typed AST                                           |
+-------------------------------------------------------------------+
                            |
                            v
+-------------------------------------------------------------------+
|  3. WIT Interface Extractor                                        |
|     - Actor protocols -> WIT interface definitions                 |
|     - Actor capabilities -> WIT world imports                      |
|     - Produces .wit files                                          |
+-------------------------------------------------------------------+
                            |
                            v
+-------------------------------------------------------------------+
|  4. WASM Code Generator                                            |
|     - Typed AST -> WASM core module (wasm32-wasip2)                |
|     - Actor state -> linear memory layout                          |
|     - Effect calls -> host function imports                        |
|     - Generic monomorphization                                     |
+-------------------------------------------------------------------+
                            |
                            v
+-------------------------------------------------------------------+
|  5. Component Linker                                               |
|     - WASM core module + WIT interface -> WASM component           |
|     - Exports actor protocol, imports capabilities                 |
+-------------------------------------------------------------------+
```

---

## 3. Layer 2: Actor Runtime

The Actor Runtime is the in-memory execution engine. It schedules actors, routes messages, manages lifecycles, and enforces isolation. All code runs inside WASM sandboxes provided by Wasmtime.

### 3.1 Virtual Actor Model

Nulang implements the Orleans virtual actor model. An actor exists whenever it has an identity. There is no explicit creation or destruction.

**Actor identity:** Every actor has a unique identity string, typically `<type>:<key>`. Examples: `OrderActor:order-42`, `UserSession:session-abc123`, `PaymentGateway:stripe`. Identity is the only thing you need to send a message — no PID, no handle, no reference.

**Addressing:**

```
+------------------+     +------------------+     +------------------+
|  Sender Actor    |     |  Runtime         |     |  Target Actor    |
|                  |     |                  |     |                  |
|  send(msg, to=   | --> |  1. Parse target | --> |  (may not exist  |
|  "Order:ord-42") |     |     identity     |     |   in memory yet) |
+------------------+     |  2. Look up in   |     +------------------+
                         |     directory    |              ^
                         |  3. If inactive: |              |
                         |     activate     |--------------+
                         |  4. Deliver to   |
                         |     mailbox      |
                         +------------------+
```

**Activation lifecycle:**

1. **Activation trigger:** A message arrives for actor identity `I`
2. **Directory lookup:** The location directory maps `I` to a node (via consistent hash)
3. **Local check:** Is `I` already activated on this node?
   - Yes: route to mailbox
   - No: proceed to activation
4. **Activation:**
   - Load actor code (WASM component)
   - Restore state from latest checkpoint (if `durable` or `event_sourced`)
   - Initialize actor state (constructor runs)
   - Register in local activation table
   - Deliver message to mailbox
5. **Deactivation** (triggered by inactivity timeout or memory pressure):
   - Finish processing current message
   - Checkpoint state (if persistent)
   - Release WASM instance back to pool
   - Remove from activation table
   - Directory entry marked "inactive"

**Activation table** (per node):

```
+------------------+----------+----------+------------------+----------+
| Actor Identity   | WASM     | Mailbox  | State            | Last     |
|                  | Instance | Ref      | (running/        | Active   |
|                  | Ref      |          |  suspended)      | (ms ago) |
+------------------+----------+----------+------------------+----------+
| Order:ord-42     | wasm-17  | mbox-33  | running          | 12       |
| User:sess-abc    | wasm-4   | mbox-12  | suspended        | 4500     |
| Cart:cart-99     | wasm-17  | mbox-45  | running          | 3        |
+------------------+----------+----------+------------------+----------+
```

WASM instances are shared across actors of the same type via copy-on-write. The instance pool keeps recently-used instances warm.

**Placement strategy:** Actors are placed via consistent hashing on their identity string. This means:
- The same actor identity always maps to the same node (within replication factor)
- Node additions cause only 1/N actors to migrate (where N = node count)
- No central coordinator needed for placement decisions
- Hot actors can be manually migrated via the management API

### 3.2 Scheduler: Work-Stealing M:N with Reduction Counting

The scheduler maps M actors onto N OS threads. It is the execution heart of the runtime.

**Architecture:**

```
+------------------------------------------------------------------------+
|                          OS Thread Pool (N threads)                     |
|  Typically N = number of CPU cores                                     |
+------------------------------------------------------------------------+
|                                                                         |
|  +----------+  +----------+  +----------+         +----------+         |
|  | Sched 0  |  | Sched 1  |  | Sched 2  |  ...    | Sched N-1|         |
|  |          |  |          |  |          |         |          |         |
|  | Run Queue|  | Run Queue|  | Run Queue|         | Run Queue|         |
|  | [A1, A3] |  | [A7]     |  | [A2, A5] |         | [A4, A6] |         |
|  |          |  |          |  |          |         |          |         |
|  | LIFO for |  | LIFO for |  | LIFO for |         | LIFO for |         |
|  | spawns   |  | spawns   |  | spawns   |         | spawns   |         |
|  | FIFO for |  | FIFO for |  | FIFO for |         | FIFO for |         |
|  | messages |  | messages |  | messages |         | messages |         |
|  +----------+  +----------+  +----------+         +----------+         |
|       |             |             |                      |              |
|       | steal <-----+----- steal +----- steal -----------+              |
|       |    (when empty, steal half from random neighbor)                |
+------------------------------------------------------------------------+
```

**Per-scheduler data structures:**

| Structure | Purpose | Access Pattern |
|-----------|---------|----------------|
| Local run queue | Ready actors | LIFO (spawns) + FIFO (messages), producer-consumer |
| Steal deque | Work available for other schedulers | Lock-free Chase-Lev deque |
| Current actor | Actor currently executing | Single owner |
| Reduction counter | Remaining reductions for current actor | Decremented per operation |

**Reduction counting:** Each actor is assigned a reduction budget (default: 2000 reductions) when scheduled. One reduction ≈ one WASM instruction or one host function call. When the counter reaches zero:
1. The actor yields (WASM execution paused via asyncify)
2. Actor returns to the run queue
3. Scheduler picks the next actor

Reduction counting serves two purposes: fairness (no actor monopolizes a thread) and checkpointing (yield points are natural checkpoint boundaries).

**Work-stealing algorithm** (Chase-Lev, lock-free):

```
When scheduler S has no work:
  1. Pick a random victim scheduler V
  2. Attempt to steal half of V's deque (from the bottom)
  3. If steal succeeds: process stolen work
  4. If steal fails: try another victim (up to 3 attempts)
  5. If all fail: park the OS thread (futex wait)
  6. Wake when new work arrives (futex wake)
```

**Message processing loop:**

```
for each scheduled actor:
  1. Dequeue next message from actor's mailbox
  2. If mailbox empty: deactivate actor (if idle timeout exceeded)
  3. Load reduction counter (default 2000)
  4. Enter WASM sandbox, call behavior handler with message
  5. Handler runs until completion OR reduction counter hits 0
  6. If counter hit 0: save WASM stack (asyncify), requeue actor
  7. If handler completed: check for outgoing messages, deliver them
  8. If persistent actor: trigger checkpoint (async, non-blocking)
  9. If more messages in mailbox: requeue actor
  10. If mailbox empty: mark idle, start idle timer
```

### 3.3 Mailbox Design: Bounded MPSC with Backpressure

Every actor has exactly one mailbox. The mailbox is a bounded multi-producer, single-consumer queue.

**Mailbox structure:**

```
+------------------------------------------------------------------+
|                         Actor Mailbox                             |
|                                                                   |
|  +----------------------------------------------------------+    |
|  |  Message Queue (ring buffer, bounded)                     |    |
|  |  +------+------+------+------+------+------+------+      |    |
|  |  | Msg1 | Msg2 | Msg3 |      |      | Msg7 | Msg8 |      |    |
|  |  +------+------+------+------+------+------+------+      |    |
|  |     ^                       ^                             |    |
|  |     | head (consumer)       | tail (producers)            |    |
|  +----------------------------------------------------------+    |
|                                                                   |
|  Capacity: 10,000 (default)                                       |
|  Backpressure strategy: drop oldest / block sender (configurable) |
|                                                                   |
|  Priority bands:                                                  |
|  +----------------------------------------------------------+    |
|  | [HIGH: system messages] [NORMAL: user messages] [LOW:   |    |
|  |  batch work]                                             |    |
|  +----------------------------------------------------------+    |
+------------------------------------------------------------------+
```

**Mailbox properties:**

| Property | Value | Rationale |
|----------|-------|-----------|
| Default capacity | 10,000 messages | Prevents unbounded memory growth |
| Max message size | 1MB | Larger data uses blob store references |
| Queue type | Lock-free ring buffer | MPSC, cache-friendly |
| Priority levels | 3 (high, normal, low) | System vs user vs batch |
| Overflow policy | Configurable: `block`, `drop_oldest`, `drop_newest` | `block` for correctness; `drop` for availability |
| Fairness | Round-robin across senders | Prevents single sender from starving others |

**Backpressure:** When a mailbox is full, the sender is blocked (for local sends) or receives a `MailboxFull` error (for remote sends). The sender can retry with exponential backoff or route to a dead-letter actor. This is the only backpressure mechanism in Nulang — there are no rate limiters, no flow control protocols, just bounded mailboxes.

**Message format:**

```
+--------+--------+--------+---------+---------+----------+
|  Meta  | Priority| Sender | Target  | Payload | Cap     |
|  (16B) | (1B)    | ID     | ID      | (var)   | Tokens  |
+--------+--------+--------+---------+---------+----------+

Meta: timestamp (8B), correlation ID (4B), flags (4B)
Payload: Cap'n Proto serialized message
Cap Tokens: list of capability tokens (signed JWTs)
```

Messages are serialized to Cap'n Proto for zero-copy deserialization within the WASM sandbox. Cap'n Proto was chosen over Protocol Buffers because it requires no parsing step — the serialized bytes are the in-memory layout.

### 3.4 Supervision Trees

Every actor has a supervisor. Supervisors are themselves actors. Supervision creates a tree of accountability.

**Supervision strategies** (from Erlang/OTP):

| Strategy | Behavior | Use Case |
|----------|----------|----------|
| `OneForOne` | Restart only the failed child | Independent workers |
| `OneForAll` | Restart all children when one fails | Tightly coupled group |
| `RestForOne` | Restart failed child + all children started after it | Dependency chain |

**Restart policies:**

```nulang
actor OrderSupervisor:
  supervise:
    strategy: OneForOne
    max_restarts: 5        -- max 5 restarts in 10 seconds
    max_seconds: 10
    children:
      - OrderProcessor { restart: permanent }   -- always restart
      - PaymentGateway { restart: transient }   -- restart only on crash
      - AuditLogger    { restart: temporary }   -- never restart
```

**Escalation:** If restart limits are exceeded, the supervisor itself fails, and its own supervisor handles it. This creates a fault propagation tree:

```
Root Supervisor (realm level)
    |
    +-- Service Supervisor (one per service type)
    |       |
    |       +-- OrderActor:order-42 (worker)
    |       +-- OrderActor:order-43 (worker)
    |       +-- OrderActor:order-44 (worker)
    |
    +-- System Supervisor (runtime services)
            |
            +-- Mailbox Manager
            +-- Checkpoint Manager
            +-- Location Directory
```

**Restart sequence:**
1. Child actor crashes (WASM trap, panic, or explicit `fail`)
2. Supervisor receives `EXIT` signal with reason
3. Supervisor checks restart policy:
   - `normal` exit: no restart (unless `permanent`)
   - `crash` exit: restart (if under limit)
   - `killed` exit: restart (if under limit)
4. If under restart limit: stop child, reload WASM, restore state, start child
5. If over limit: supervisor exits with `shutdown`, escalates to parent

**Process isolation:** Each actor runs in its own WASM sandbox instance. Memory is isolated — actor A cannot read actor B's linear memory. If actor A crashes (segmentation fault, infinite loop, stack overflow), only actor A is affected. The runtime detects the crash via Wasmtime's trap handling and restarts the actor via its supervisor.

### 3.5 Per-Actor Heaps and ORCA GC

Nulang uses ORCA (Ownership and Reference Counting based on Cycles in Actors) garbage collection. ORCA is a concurrent, per-actor garbage collector designed specifically for actor systems.

**Why ORCA:** Traditional GC (stop-the-world, generational) is a poor fit for actor systems because:
- Stop-the-world pauses break real-time message processing guarantees
- Cross-actor references are hard to trace (actors are distributed)
- Per-actor collection is naturally parallel (each actor collects independently)

**ORCA overview:** Each actor manages its own heap. Memory is reclaimed via:
1. **Reference counting:** Most objects are freed immediately when their refcount hits zero
2. **Cycle detection:** Reference cycles are detected via a lightweight local cycle collector
3. **Cross-actor references:** When actor A holds a reference to an object in actor B, actor B tracks it as a "foreign reference." When the foreign reference is dropped, a decrement message is sent to actor B.

**Heap layout per actor:**

```
+------------------------------------------------------------------+
|                    Actor Linear Memory (WASM)                     |
|  +----------------------+-------------------------------------+   |
|  | Immutable Heap       | Mutable Heap                        |   |
|  | (actor code, static  | (state, message buffers, temp       |   |
|  |  data)               |  allocations)                       |   |
|  +----------------------+-------------------------------------+   |
|  | GC-managed objects   | ORCA allocator: bump + free list    |   |
|  +----------------------+-------------------------------------+   |
|  Max size: configurable (default 100MB, hard max 1GB)           |
+------------------------------------------------------------------+
```

**GC guarantees:**
- Pause time: < 1ms per collection (per-actor, no global pause)
- Collection frequency: triggered when heap crosses threshold (default: 75% of max)
- Cross-actor decrements: batched and sent asynchronously (not immediate)
- Memory limit enforcement: hard limit enforced by WASM memory bounds

### 3.6 Location Directory

The location directory is a distributed hash table mapping actor identities to their hosting nodes.

```
+------------------------------------------------------------------+
|                    Location Directory (per node)                  |
|                                                                   |
|  Actor Identity          ->  Node ID         | Status            |
|  -------------------------   --------------   | --------          |
|  Order:ord-42              ->  node-7        | active            |
|  User:sess-abc             ->  node-3        | active            |
|  Cart:cart-99              ->  node-7        | active            |
|  Payment:pay-12            ->  node-1        | deactivated       |
+------------------------------------------------------------------+
```

The directory is sharded by actor identity across all nodes via consistent hashing. Each node is responsible for a portion of the directory. Directory entries are cached locally with a TTL (default: 30 seconds). Stale cache entries are refreshed on message send failure.

---

## 4. Layer 3: Durable Execution

Durable execution is the persistence layer. It ensures that actor state survives crashes, restarts, and node failures. Without this layer, Nulang is an in-memory actor framework. With it, Nulang is a distributed durable execution platform.

### 4.1 Four State Models with Decision Flowchart

Every actor has a state model. The state model determines persistence, recovery, and consistency semantics. It is part of the actor's type — it cannot be changed after deployment.

```
                              START
                                |
                    +-----------+-----------+
                    |                       |
              Stateless?              Stateful?
                    |                       |
                    v                       v
              +---------+          +--------+--------+
              | `local` |          |  Needs audit?   |
              +---------+          +--------+--------+
                                         |
                              +----------+----------+
                              |                     |
                            No audit            Full audit
                              |                     |
                              v                     v
                       +-----------+       +----------------+
                       | `durable` |       | `event_sourced`|
                       +-----------+       +----------------+
                              |
                    Geo-replicated?
                              |
                    +---------+---------+
                    |                   |
                  Single           Multi-region
                  region               |
                    |                   v
                    v            +------------+
              (done)             |  `crdt`    |
                                 +------------+
```

**State model comparison:**

| Property | `local` | `durable` | `event_sourced` | `crdt` |
|----------|---------|-----------|-----------------|--------|
| Persistence | None | Automatic checkpoint | Event journal | CRDT merge |
| Recovery | None | State restore | Event replay | State sync |
| Consistency | N/A | Strong (single node) | Strong (single node) | Eventual |
| Multi-region | N/A | Active-passive | Active-passive | Active-active |
| Audit trail | No | No (checkpoints only) | Yes (full history) | Yes (ops only) |
| Overhead | Zero | ~1ms/checkpoint | Journal append | Merge on sync |
| Determinism | Not required | Recommended | Required | Required |

**Decision guide:**
- Use `local` for stateless computation, caches, protocol adapters
- Use `durable` as default for business logic (simplest, safest)
- Use `event_sourced` when you need audit history or deterministic replay
- Use `crdt` for geo-replicated collaborative state

### 4.2 Checkpointing Algorithm

**When to checkpoint:**

| Trigger | Default | Configurable | Rationale |
|---------|---------|--------------|-----------|
| Every message | Default for `durable` | Yes | Maximum safety |
| Every N messages | N=10 (configurable) | N | Amortize write cost |
| Every T seconds | Disabled by default | T | Time-based for batch |
| Memory threshold | 100MB | M | Prevent OOM |
| Before deactivation | Always | No | Ensure clean shutdown |

**What to checkpoint:**

```
Checkpoint payload:
+------------------+---------+-----------------------------------------+
| Field            | Size    | Description                             |
+------------------+---------+-----------------------------------------+
| Actor identity   | var     | Type + key                              |
| Sequence number  | 8B      | Monotonic message counter               |
| State blob       | var     | Serialized linear memory (LZ4 compressed)|
| Effect results   | var     | Map of (effect_id -> result) since last |
|                  |         | checkpoint                              |
| Mailbox snapshot | var     | Unprocessed messages (for exactly-once) |
| Timestamp        | 8B      | Wall clock (for debugging)              |
| Checksum         | 8B      | xxHash64 of entire payload              |
+------------------+---------+-----------------------------------------+
```

**How to checkpoint (async, non-blocking):**

```
1. Actor finishes message M_n
2. Scheduler marks actor as "checkpointing" (still in activation table)
3. Fork-on-write: actor's memory pages marked copy-on-write
4. Actor continues processing next message M_{n+1}
5. Background thread serializes checkpoint from COW snapshot
6. Serialized checkpoint written to storage backend (append-only)
7. On success: checkpoint metadata updated, COW pages released
8. On failure: actor marked for restart, supervisor handles recovery
```

The copy-on-write technique ensures that checkpointing does not block message processing. The actor incurs at most one page fault per written page during checkpointing. This is the same technique used by Erlang's `erlang:hibernate/3` and by Linux's fork() system call.

**Storage backends:**

| Backend | Use Case | Latency | Throughput |
|---------|----------|---------|------------|
| SQLite (local file) | Development | <1ms | 10K writes/sec |
| PostgreSQL | Single-node production | 1-5ms | 50K writes/sec |
| S3-compatible | Multi-node production | 10-50ms | Unlimited |
| Local NVMe (ephemeral) | High-throughput temp | <0.1ms | 100K writes/sec |

### 4.3 Event Journal Format and Storage

Event-sourced actors use an append-only event journal as their source of truth.

**Journal entry format:**

```
+--------+----------+----------+-------+----------+---------+----------+
| Header | Sequence | Timestamp| Type  | Payload  | Effect  | Checksum |
| (16B)  | (8B)     | (8B)     | (var) | (var)    | Results | (8B)     |
|        |          |          |       |          | (var)   |          |
+--------+----------+----------+-------+----------+---------+----------+

Header: magic (4B), version (2B), flags (2B), payload_len (4B), type_len (4B)
Type: event type name (e.g., "OrderCreated", "PaymentProcessed")
Payload: Cap'n Proto serialized event data
Effect Results: optional map of effect_id -> captured result
```

**Journal storage layout:**

```
journals/
  <actor_type>/
    <shard>/
      <actor_identity>/
        journal-00000001.seg    (10,000 events)
        journal-00000002.seg    (10,000 events)
        journal-00000003.seg    (5,000 events, current)
        snapshot-000020000.bin  (state at sequence 20,000)
        snapshot-000030000.bin  (state at sequence 30,000)
        meta.json               (journal metadata)
```

Segments are immutable once closed. Only the current (latest) segment is written to. Segments are written sequentially and never modified after close, enabling efficient replication (only the latest segment needs sync).

**Segment file format:**

```
+----------+----------+----------+----------+-----+----------+
| Index    | Entry 1  | Entry 2  | Entry 3  | ... | Trailer  |
| (offset  |          |          |          |     | (max     |
|  table)  |          |          |          |     |  seq)    |
+----------+----------+----------+----------+-----+----------+

Index: array of (sequence_number -> file_offset) for random access
Trailer: max sequence number, checksum, timestamp
```

### 4.4 Replay Mechanism and Determinism Enforcement

Replay is the ability to reconstruct an actor's exact state by replaying its event journal. It is the foundation of crash recovery and debugging.

**Replay algorithm:**

```
function replay(actor_identity, from_seq=0, to_seq=INF):
  1. Load latest snapshot at or before from_seq
  2. state = snapshot.state
  3. For each event in journal from snapshot.seq+1 to to_seq:
     a. Load event + captured effect results
     b. Apply projection function (pure, deterministic)
     c. For effects: return captured results, do not re-invoke handlers
     d. Update state
  4. Return state
```

**Determinism enforcement:** Nulang enforces determinism at compile time and runtime:

| Source of non-determinism | Compile-time enforcement | Runtime capture |
|---------------------------|-------------------------|-----------------|
| Time (`Now`) | Effect, not primitive | Captured timestamp |
| Randomness (`Random`) | Effect, not primitive | Captured value |
| External IO | Must use effects | Captured result |
| Message ordering | FIFO mailbox guarantee | Inherent |
| Internal iteration | Ordered data structures | Inherent |

**What happens on non-determinism violation:** If an event-sourced actor produces different output during replay (detected by hash mismatch of the state blob), the runtime:
1. Logs the divergence with full context
2. Halts replay for that actor
3. Notifies the supervisor
4. Supervisor restarts the actor from the last known-good snapshot
5. Human operator is alerted

This is a safety mechanism. Divergence should never happen in correct code. If it does, it indicates a bug (e.g., using `DateTime.now()` instead of `effect Now`).

### 4.5 Snapshotting Strategy

Snapshots are checkpoints of an event-sourced actor's projected state. They enable fast recovery without replaying from event 0.

**Snapshot triggers:**

| Trigger | Default | Description |
|---------|---------|-------------|
| Event count | Every 1,000 events | Regular snapshots for bounded replay |
| Time | Every 60 seconds | Time-based for low-volume actors |
| Size | When state > 10MB | Prevent oversized snapshots |
| Manual | On demand | Pre-deployment, debugging |

**Snapshot format:**

```
+----------+----------+----------+----------+----------+
| Sequence | Version  | State    | Index    | Checksum |
| Number   | (schema) | Blob     | (for     |          |
| (8B)     | (4B)     | (var)    |  partial | (8B)     |
|          |          |          |  load)   |          |
+----------+----------+----------+----------+----------+
```

**Tiered storage:**

```
+--------------------------------------------------------------+
|  Hot snapshots (last 10)     -> Local NVMe SSD               |
|  Warm snapshots (10-100)     -> Network-attached SSD         |
|  Cold snapshots (archived)   -> S3-compatible object store   |
+--------------------------------------------------------------+
```

**Partial loading:** For large state snapshots ( > 100MB), Nulang supports partial loading. The snapshot index maps state keys to file offsets. Only accessed keys are loaded into memory. This is essential for actors with large state (e.g., a shopping cart actor holding 10,000 items).

---

## 5. Layer 4: Distributed Platform

The Distributed Platform enables multiple Nulang runtime nodes to form a cluster. It handles node discovery, actor placement, cross-node messaging, and state replication.

### 5.1 Cluster Membership: Gossip Protocol with SWIM Failure Detection

Nodes discover each other and maintain a consistent view of cluster membership using a gossip protocol.

**SWIM protocol (Scalable Weakly-consistent Infection-style Process Group Membership):**

```
+--------------------------------------------------------------+
|  Node A                        Node B          Node C         |
|                                                              |
|  1. A picks random member (B)                               |
|     sends PING to B                                         |
|                                                              |
|  2. B receives PING, responds ACK                           |
|                                                              |
|  3. If A doesn't receive ACK within timeout:                |
|     A asks K random members (C, D) to PING B indirectly     |
|                                                              |
|  4. If no indirect ACK: B is marked failed                  |
|     Failure is gossiped to all members                      |
+--------------------------------------------------------------+
```

**Gossip dissemination:** In addition to failure detection, the gossip protocol disseminates:
- Node join/leave events
- Actor placement changes
- Configuration updates
- CRDT state deltas (piggybacked on gossip messages)

**Gossip message format:**

```
+----------+----------+----------+----------+----------+
| Sender   | Sequence | Updates  | Piggyback| Checksum |
| Node ID  | Number   | (array)  | (CRDT    |          |
|          |          |          |  deltas) |          |
+----------+----------+----------+----------+----------+

Updates: array of (type, payload) where type is:
  - NodeJoined { node_id, address, capabilities }
  - NodeFailed { node_id, timestamp }
  - ActorPlaced { actor_id, node_id }
  - ActorMigrated { actor_id, from_node, to_node }
  - ConfigChanged { key, value, version }
```

**Failure detection parameters:**

| Parameter | Default | Description |
|-----------|---------|-------------|
| Probe interval | 1 second | Time between direct probes |
| Probe timeout | 3 seconds | Time to wait for direct ACK |
| Indirect probes | 3 | Number of indirect probes on direct failure |
| Indirect timeout | 5 seconds | Time to wait for indirect ACK |
| Suspicion threshold | 3 | Number of missed probes before declaring failure |
| Sync interval | 200ms | Gossip sync frequency |

**False positive handling:** SWIM can falsely detect failure under high packet loss. Nulang mitigates this:
1. A node marked failed enters "suspected" state first
2. If suspected node responds to any probe, it is reinstated
3. If no response after suspicion threshold, node is declared failed
4. Actor placements on failed node are rebalanced to surviving nodes
5. If the "failed" node was actually alive (network partition), it detects the partition and shuts down (split-brain prevention)

### 5.2 Message Routing: Local vs Remote, Caching

Messages are routed by actor identity. The routing process determines whether the target is local or remote and delivers accordingly.

**Routing algorithm:**

```
function route_message(msg, target_identity):
  1. Hash target_identity -> consistent_hash_ring_position
  2. Look up responsible node in consistent hash ring
  3. If responsible node == this node:
       a. Look up target_identity in local activation table
       b. If active: push to mailbox
       c. If inactive: activate, then push to mailbox
  4. If responsible node == other node:
       a. Check local cache for target location
       b. If cache hit and fresh: forward to cached node
       c. If cache miss or stale: forward to responsible node
  5. If responsible node is failed:
       a. Find next replica in hash ring
       b. Forward to replica
       c. Trigger rebalancing for failed node's actors
```

**Consistent hash ring:**

```
Consistent Hash Ring (0 to 2^160 - 1, SHA-1 space)
+------------------------------------------------------------------+
|                                                                   |
|  node-1        node-2        node-3        node-1(replica)       |
|     |            |            |               |                  |
|     v            v            v               v                  |
|  +------+    +------+    +------+        +------+               |
|  |######|    |      |    |######|        |      |               |
|  |######|    |      |    |######|        |      |               |
|  +------+    +------+    +------+        +------+               |
|  0x10..    0x50..    0x90..         0xD0..                      |
|                                                                   |
|  Actor "Order:42" -> hash("Order:42") = 0x67 -> node-2          |
|  Actor "Cart:99"  -> hash("Cart:99")  = 0xB3 -> node-3          |
+------------------------------------------------------------------+
```

Each node is placed at multiple points on the ring (virtual nodes, default: 150 per physical node). This ensures uniform distribution even with non-uniform actor identity hashing.

**Location cache:** Each node maintains an LRU cache of actor identity -> node mappings. Cache entries have a TTL (default: 30 seconds). A cache hit avoids a consistent hash computation and a directory lookup. On cache miss or stale entry, the router falls back to the directory.

### 5.3 CRDT Replication: Anti-Entropy Protocol

CRDT actors replicate state across nodes without coordination. The replication uses an anti-entropy protocol.

**Anti-entropy process:**

```
Node A (CRDT replica)              Node B (CRDT replica)
       |                                    |
       | 1. A initiates sync                |
       |    sends sync request with         |
       |    vector clock + digest           |
       |----------------------------------->|
       |                                    |
       | 2. B compares digest with          |
       |    its own state                   |
       |    identifies missing deltas       |
       |<-----------------------------------|
       |    sends delta batch               |
       |                                    |
       | 3. A applies deltas, sends         |
       |    ack + any deltas B is missing   |
       |----------------------------------->|
       |                                    |
       | 4. B applies A's deltas            |
       |<-----------------------------------|
       |    final ack                       |
```

**CRDT types supported:**

| Type | Operations | Merge Semantics | Use Case |
|------|-----------|-----------------|----------|
| G-Counter | increment | max of replicas | Vote counting, page views |
| PN-Counter | increment, decrement | per-replica G-Counters | Like/dislike counts |
| G-Set | add | set union | Tag collections |
| 2P-Set | add, remove | add-wins over remove | Shopping cart items |
| OR-Set | add, remove | add-wins, unique tags | Collaborative lists |
| LWW-Register | set | last-writer-wins | User preferences |
| MV-Register | set | multi-value on conflict | Concurrent edits |
| OR-Map | put, remove, nested CRDTs | recursive merge | Document stores |
| Delta-State | all above | delta-based sync | Efficient replication |

**Sync modes:**

| Mode | Trigger | Latency | Bandwidth |
|------|---------|---------|-----------|
| Continuous | Every update | Low | High |
| Periodic | Every 5 seconds | Medium | Low |
| On-demand | Pull-based | High | Lowest |
| Gossip piggyback | On gossip messages | Medium | Very low |

The default is gossip piggyback for low-throughput CRDTs and periodic (1s) for high-throughput CRDTs.

### 5.4 Service Discovery

Actor protocols are registered in a distributed service registry. Clients discover actors by protocol type, not by hardcoded identity.

**Registry structure:**

```
+------------------------------------------------------------------+
|                     Service Registry                              |
|                                                                   |
|  Protocol Type       ->  Actor Type(s)       | Instances         |
|  -----------------      -----------------     | --------          |
|  OrderService        ->  OrderActor          | [ord-42, ord-43]  |
|  PaymentGateway      ->  StripeGateway       | [stripe]          |
|                       ->  PayPalGateway      | [paypal]          |
|  UserSession         ->  SessionActor        | [sess-abc, ...]   |
+------------------------------------------------------------------+
```

**Discovery patterns:**

| Pattern | API | Use Case |
|---------|-----|----------|
| By identity | `actor("Order:ord-42")` | Known entity (specific order) |
| By type | `actor_of_type(OrderService)` | Any instance (round-robin) |
| By key range | `actors_in_range("User:", start, end)` | Shard scanning |
| By capability | `actors_with(LLM)` | Find all LLM-enabled actors |

The registry is itself a CRDT, replicated across all nodes via gossip. Registration is implicit: when an actor type is deployed, it is automatically registered. Deregistration happens on deployment removal.

### 5.5 Network Transport: NUL0 Protocol

NUL0 is Nulang's inter-node network protocol. It is a binary protocol over TCP (with optional QUIC for cross-region).

**NUL0 frame format:**

```
+--------+--------+--------+--------+--------+----------+----------+
| Magic  | Version| Type   | Flags  | Length | Payload  | MAC      |
| (4B)   | (1B)   | (1B)   | (2B)   | (4B)   | (var)    | (16B)    |
+--------+--------+--------+--------+--------+----------+----------+

Magic: 0x4E554C30 ("NUL0" in ASCII)
Version: 0x01 (current)
Type: Message, Ack, Nack, Heartbeat, Gossip, CRDTSync, Control
Flags: encrypted, compressed, priority
MAC: Poly1305 message authentication code
```

**Connection management:**

```
+------------------------------------------------------------------+
|                    Connection Pool (per node pair)                |
|                                                                   |
|  Node A maintains persistent TCP connections to all other nodes   |
|  Connection count per pair: min(4, CPU cores)                     |
|                                                                   |
|  +----------+    +----------+    +----------+    +----------+   |
|  | Conn 1   |    | Conn 2   |    | Conn 3   |    | Conn 4   |   |
|  | (msgs)   |    | (gossip) |    | (crdt)   |    | (ctrl)   |   |
|  +----------+    +----------+    +----------+    +----------+   |
|                                                                   |
|  Separation: different traffic types use different connections    |
|  to prevent head-of-line blocking                                 |
+------------------------------------------------------------------+
```

**Security:**
- All inter-node traffic is encrypted (TLS 1.3 over TCP, or native QUIC encryption)
- Messages are authenticated with Poly1305 MACs
- Capability tokens are embedded in message headers and verified at the receiving node
- Node identities are verified via Ed25519 signatures exchanged during handshake

---

## 6. Layer 5: AI Runtime

The AI Runtime is the top layer. It provides LLM integration, tool calling, memory management, planning, and observability. Every feature in this layer is built from the same primitives as all other layers — there is no special-case "AI subsystem."

### 6.1 Model Provider Abstraction

All LLM providers implement a standard WIT interface. Actors request the `LLM` capability, and the runtime injects the configured provider.

**WIT interface:**

```wit
interface llm-provider {
  variant model { gpt-4o, claude-sonnet, llama-3-1, custom(string) }

  record message {
    role: string,
    content: string,
  }

  record tool-definition {
    name: string,
    description: string,
    parameters: schema,
  }

  record tool-call {
    id: string,
    name: string,
    arguments: string,
  }

  record completion-request {
    model: model,
    messages: list<message>,
    tools: option<list<tool-definition>>,
    temperature: option<float64>,
    max-tokens: option<u32>,
  }

  record completion-response {
    content: string,
    tool-calls: list<tool-call>,
    usage: token-usage,
    latency: duration,
  }

  complete: func(req: completion-request) -> result<completion-response, error>
}
```

**Provider backends:**

| Provider | Connection | Streaming | Tool Calling | Cost Tracking |
|----------|-----------|-----------|--------------|---------------|
| OpenAI | HTTP/2 | Yes | Native | Per-token |
| Anthropic | HTTP/2 | Yes | Native | Per-token |
| Azure OpenAI | HTTP/2 | Yes | Native | Per-token + quota |
| Ollama | HTTP/1.1 | Yes | Via prompt | None |
| vLLM | HTTP/2 | Yes | Native | Per-token |
| Custom | Configurable | Optional | Via schema | Custom |

**Multi-provider routing:** The runtime can route different actors to different providers:

```nulang
-- Actor-level provider selection
actor Researcher:
  capability LLM { provider: openai, model: gpt-4o }

-- Runtime-level routing rules
llm_routing:
  - match: { actor_type: "Researcher", behavior: "deep_research" }
    provider: openai/gpt-4o
  - match: { actor_type: "Researcher", behavior: "classify" }
    provider: ollama/llama-3.1
  - match: { cost_today: "> $100" }
    provider: ollama/llama-3.1  -- fallback on budget exceed
```

### 6.2 Tool Registration and Execution

Tools are actor behaviors exposed to LLMs. There is no separate tool definition format.

**Tool discovery:**

```
1. Actor A calls LLMComplete with `tools: [B.behavior1, C.behavior2]`
2. AI runtime generates tool schemas from behavior types
3. Schemas sent to LLM provider (OpenAI function calling format)
4. LLM may request tool call in its response
5. AI runtime parses tool call, sends message to target actor
6. Tool result injected back into conversation
```

**Tool schema generation:**

```nulang
actor WeatherService:
  -- This behavior becomes a tool with auto-generated schema:
  -- {
  --   "name": "WeatherService_get_forecast",
  --   "description": "Get weather forecast for a city",
  --   "parameters": {
  --     "type": "object",
  --     "properties": {
  --       "city": { "type": "string", "description": "..." },
  --       "days": { "type": "integer", "description": "..." }
  --     },
  --     "required": ["city", "days"]
  --   }
  -- }
  behavior get_forecast(city: String, days: U8) -> Forecast:
    effect HttpGet("https://api.weather.com/forecast", { city, days })
```

**Tool execution flow:**

```
LLM requests tool call: get_forecast({"city": "Paris", "days": 3})
         |
         v
AI Runtime parses arguments (JSON -> Nulang values)
         |
         v
Sends message to WeatherService actor
         |
         v
Waits for response (with timeout, default 30s)
         |
         v
Injects result into conversation as tool result message
         |
         v
Re-sends conversation to LLM for final response
```

**Tool safety:**
- Tool calls are effects, so they are traced and captured for replay
- Tool arguments are validated against the behavior's parameter types
- Tool execution timeouts prevent hung tool calls from blocking forever
- Tool results are truncated to fit the model's context window

### 6.3 Memory Subsystem (3 Tiers)

AI agents need memory. Nulang provides three tiers, each with different scope, persistence, and access patterns.

**Tier 1: Short-term (conversation buffer):**

| Property | Value |
|----------|-------|
| Scope | Single behavior handler |
| API | `conversation` (implicit) |
| Backend | In-memory buffer |
| Persistence | None (session-scoped) |
| Limit | Model context window (auto-truncation) |

The conversation buffer holds the current LLM conversation. It is automatically managed:
- Messages are appended as the conversation progresses
- When the buffer exceeds the model's context window, old messages are summarized
- The summary replaces the oldest messages, preserving context within limits

```nulang
behavior chat(user_message: String):
  -- conversation is implicitly available
  -- effect LLMComplete uses conversation as messages
  let response = effect LLMComplete({ model: GPT4O })
  -- response is automatically appended to conversation
  return response.content
```

**Tier 2: Long-term (vector store):**

| Property | Value |
|----------|-------|
| Scope | Actor instance |
| API | `memory.remember(key, text)`, `memory.recall(query, limit)` |
| Backend | Qdrant, pgvector, or in-memory HNSW |
| Persistence | `durable` (checkpointed with actor) |
| Embedding model | Configurable (default: text-embedding-3-small) |

```nulang
actor PersonalAssistant:
  capability LLM
  capability VectorStore

  behavior learn_fact(fact: String):
    effect VectorStoreUpsert({ id: generate_id(), text: fact })

  behavior answer_question(question: String):
    let relevant = effect VectorStoreQuery(question, limit: 5)
    let context = relevant.map(|f| f.text).join("\n")
    effect LLMComplete({
      model: GPT4O,
      messages: [
        { role: "system", content: "Context: {context}" },
        { role: "user", content: question }
      ]
    })
```

**Tier 3: Event memory (the journal):**

| Property | Value |
|----------|-------|
| Scope | All messages ever received |
| API | `journal.read(from, to)`, `journal.search(query)` |
| Backend | Event journal (Layer 3) |
| Persistence | `event_sourced` (permanent) |
| Query | Full-text search on message payloads |

For event-sourced actors, the entire message history is the memory. This is "perfect memory" — every interaction is recorded and replayable. The journal supports full-text search via inverted indices built asynchronously.

### 6.4 Planning and Delegation

Planning is workflow composition driven by an LLM. The planner actor generates workflows from natural language goals.

**Planner architecture:**

```
+------------------------------------------------------------------+
|                        Planner Actor                              |
|                                                                   |
|  Input: natural language goal ("Process all pending orders")     |
|         + available tools (actor behavior registry)               |
|         + constraints (max cost, max time, allowed actors)        |
|                                                                   |
|  LLM generates workflow definition (JSON/YAML)                    |
|       |                                                           |
|       v                                                           |
|  Workflow Validator (compile-time check)                          |
|       |                                                           |
|       v                                                           |
|  Workflow Compiler -> Actor graph (Layer 3)                       |
|       |                                                           |
|       v                                                           |
|  Submitted to runtime for execution                               |
+------------------------------------------------------------------+
```

**Planner capabilities:**
- Generate workflows from natural language descriptions
- Execute pre-defined workflow templates with LLM-filled parameters
- Delegate to sub-agents (other actors) for parallel work
- Handle failures by replanning (retry with modified workflow)
- Respect capability constraints (only use tools the caller has access to)

**Delegation pattern:**

```nulang
actor Manager:
  capability LLM

  behavior delegate_task(task: String):
    -- Planner generates a workflow
    let plan = effect LLMComplete({
      model: GPT4O,
      messages: [
        { role: "system", content: planner_prompt },
        { role: "user", content: task }
      ],
      tools: [WorkflowCompiler.validate, ToolRegistry.list]
    })

    -- Compile and execute
    let workflow = compile_workflow(plan.tool_calls[0].arguments)
    effect StartWorkflow(workflow)
```

### 6.5 Observability: Tracing Every LLM Call

Every LLM call is automatically traced. No manual instrumentation required.

**Traced fields:**

| Field | Description | PII Handling |
|-------|-------------|--------------|
| Input prompt | Full conversation | Redacted by default (hash + length) |
| Output completion | Full response | Redacted by default |
| Model name | e.g., "gpt-4o" | None |
| Token usage | Input/output counts | None |
| Latency | Time to first token, total time | None |
| Tool calls requested | Names and arguments | Redacted |
| Tool call results | Return values | Redacted |
| Cost | Estimated USD | None |
| Actor identity | Which actor made the call | None |
| Behavior name | Which behavior | None |
| Timestamp | Wall clock | None |

**Trace output:** All traces are emitted as OpenTelemetry spans. They can be sent to:
- Jaeger (distributed tracing)
- Prometheus (metrics: latency histograms, token counters)
- Custom backends (via OTLP)

**Cost tracking:** The AI runtime maintains per-actor, per-workflow, and per-deployment cost counters. These are exposed as metrics and can be used for:
- Budget alerts ("deployment X has spent $500 today")
- Auto-routing ("switch to local model when budget exceeded")
- Chargeback ("team Y spent $Z on LLM calls this month")

---

## 7. Data Flow Diagrams

### 7.1 Local Message Flow

```
+--------+     +---------+     +----------+     +----------+     +-------+
| Sender |     | Message |     | Scheduler|     | Behavior |     |Mailbox|
| Actor  |     | Router  |     | (work    |     | Handler  |     | (next)|
|        |     |         |     |  queue)  |     | (WASM)   |     |       |
+---+----+     +----+----+     +----+-----+     +----+-----+     +---+---+"
    |               |                |                |                |
    | 1. send(msg)  |                |                |                |
    |-------------->|                |                |                |
    |               | 2. route to    |                |                |
    |               |    scheduler   |                |                |
    |               |--------------->|                |                |
    |               |                | 3. enqueue     |                |
    |               |                |    in run queue|                |
    |               |                |                |                |
    |               |                | 4. schedule    |                |
    |               |                |    actor       |                |
    |               |                |--------------->|                |
    |               |                |                | 5. dequeue msg |
    |               |                |                |                |
    |               |                |                | 6. run handler |
    |               |                |                |    in WASM     |
    |               |                |                |                |
    |               |                |                | 7. handler     |
    |               |                |                |    completes   |
    |               |                |                |                |
    |               |                | 8. deliver     |                |
    |               |                |    outgoing    |                |
    |               |                |    messages    |                |
    |               |                |                |                |
    |               |                | 9. if reply    |                |
    |<------------------------------------expected:   |                |
    |               |                |    send reply  |                |
    |               |                |                |                |
    |               |                | 10. requeue    |                |
    |               |                |    if more msgs|---------------->|
    +               +                +                +                +
```

### 7.2 Durable Execution Flow

```
+---------+     +----------+     +-----------+     +----------+     +--------+
|Behavior |     |Checkpoint|     |  Storage  |     | Recovery |     |Replay  |
|Handler  |     | Manager  |     |  Backend  |     |Orchestr. |     |Engine  |
+----+----+     +----+-----+     +-----+-----+     +----+-----+     +---+----+
     |               |                  |                |               |
     | 1. handler    |                  |                |               |
     |    completes  |                  |                |               |
     |-------------->|                  |                |               |
     |               | 2. COW snapshot  |                |               |
     |               |    of state      |                |               |
     |    (actor     |                  |                |               |
     |     continues)|                  |                |               |
     |               | 3. serialize +   |                |               |
     |               |    compress      |                |               |
     |               |----------------->|                |               |
     |               |                  | 4. append to   |                |
     |               |                  |    journal     |                |
     |               |<-----------------|                |               |
     |               | 5. ack           |                |               |
     |               |                  |                |               |
     |               |                  |                |               |
     |               |                  |                | 6. node fails  |
     |               |                  |                |               |
     |               |                  |                | 7. detect      |
     |               |                  |                |    failure     |
     |               |                  |<---------------|               |
     |               |                  | 8. load latest |                |
     |               |                  |    checkpoint  |                |
     |               |                  |                |               |
     |               |                  |                | 9. replay      |
     |               |                  |                |    events from |
     |               |                  |                |    checkpoint  |
     |               |<-----------------------------------|                |
     |               | 10. restore state                  |                |
     |<--------------|                                    |                |
     | 11. resume    |                                    |                |
     |    processing |                                    |                |
     +               +                  +                 +                +

For event_sourced actors, step 8 loads the snapshot and steps 9-10 replay
events after the snapshot. Effect results from the journal are used instead
of re-invoking handlers.
```

### 7.3 Distributed Message Flow

```
+--------+     +----------+     +----------+     +----------+     +--------+
|Local   |     |Location  |     | NUL0     |     |Location  |     |Remote  |
|Actor   |     |Directory |     |Transport |     |Directory |     |Actor   |
+---+----+     +----+-----+     +-----+----+     +-----+----+     +---+----+
    |               |                  |                |               |
    | 1. send(msg,  |                  |                |               |
    |    "Order:42")|                  |                |               |
    |-------------->|                  |                |               |
    |               | 2. hash("Order:42")|                |               |
    |               |    -> node-3       |                |               |
    |               |                  |                |               |
    |               | 3. if local:     |                |               |
    |               |    deliver       |                |               |
    |               |    if remote:    |                |               |
    |               |    forward       |                |               |
    |               |----------------->|                |               |
    |               |                  | 4. serialize   |                |
    |               |                  |    + encrypt   |                |
    |               |                  |                |               |
    |               |                  | 5. TCP send    |                |
    |               |                  |--------------->|               |
    |               |                  |                | 6. receive     |
    |               |                  |                |    + decrypt   |
    |               |                  |                |               |
    |               |                  |                | 7. look up     |
    |               |                  |                |    activation  |
    |               |                  |                |               |
    |               |                  |                | 8. deliver to  |
    |               |                  |                |    mailbox     |
    |               |                  |                |-------------->|
    |               |                  |                | 9. send ACK   |
    |               |                  |<---------------|               |
    |               |                  | 10. ACK        |               |
    |               |<-----------------|                |               |
    | 11. delivery   |                  |                |               |
    |     confirmed |                  |                |               |
    +               +                  +                +               +
```

### 7.4 CRDT Sync Flow

```
+--------+     +---------+     +----------+     +---------+     +--------+
|Local   |     |CRDT     |     | Gossip / |     |CRDT     |     |Remote  |
|Modify  |     |Engine    |     | NUL0     |     |Engine   |     |Merge   |
+---+----+     +----+----+     +-----+----+     +----+----+     +---+----+
    |               |                  |                |               |
    | 1. apply(op)  |                  |                |               |
    |-------------->|                  |                |               |
    |               | 2. update local  |                |               |
    |               |    state         |                |               |
    |               |                  |                |               |
    | 3. return OK  |                  |                |               |
    |<--------------|                  |                |               |
    |               |                  |                |               |
    |               | 4. delta =       |                |               |
    |               |    state.delta() |                |               |
    |               |                  |                |               |
    |               | 5. encode delta  |                |               |
    |               |----------------->|                |               |
    |               |                  | 6. send via    |                |
    |               |                  |    gossip/NUL0 |                |
    |               |                  |--------------->|                |
    |               |                  |                | 7. receive     |
    |               |                  |                |    delta       |
    |               |                  |                |               |
    |               |                  |                | 8. merge(delta)|
    |               |                  |                |    (converged) |
    |               |                  |                |               |
    |               |                  |                | 9. if local    |
    |               |                  |                |    state       |
    |               |                  |                |    changed:    |
    |               |                  |                |    notify      |
    |               |                  |                |    actor       |
    |               |                  |                +------->|       |
    +               +                  +                +        +       +

No coordination, no locks, no consensus. Both replicas apply the operation
locally and converge via the CRDT merge function.
```

### 7.5 AI Workflow Flow

```
+--------+     +--------+     +----------+     +--------+     +--------+
| User   |     | Agent  |     | AI       |     | Tool   |     | LLM    |
| Message|     | Actor  |     | Runtime  |     | Exec   |     |Provider|
+---+----+     +---+----+     +-----+----+     +---+----+     +---+----+
    |              |                |                |               |
    | 1. message   |                |                |               |
    |------------->|                |                |               |
    |              | 2. build       |                |               |
    |              |    conversation|                |               |
    |              |    (short-term  |                |               |
    |              |     memory)     |                |               |
    |              |                |                |               |
    |              | 3. recall      |                |               |
    |              |    long-term    |                |               |
    |              |    memory       |                |               |
    |              |                |                |               |
    |              | 4. effect      |                |               |
    |              |    LLMComplete  |                |               |
    |              |--------------->|                |               |
    |              |                | 5. send to     |                |
    |              |                |    LLM provider|                |
    |              |                |--------------->|                |
    |              |                |                | 6. LLM decides |
    |              |                |                |    tool call   |
    |              |                |<---------------|                |
    |              |                | 7. parse tool  |                |
    |              |                |    call        |                |
    |              |                |                |               |
    |              |                | 8. send to     |                |
    |              |                |    tool actor  |                |
    |              |                |--------------->|                |
    |              |                |                | 9. execute     |
    |              |                |                |    behavior    |
    |              |                |                |               |
    |              |                | 10. tool result|                |
    |              |                |<---------------|                |
    |              |                |                |               |
    |              |                | 11. re-send to |                |
    |              |                |    LLM with    |                |
    |              |                |    tool result |                |
    |              |                |--------------->|                |
    |              |                |                | 12. final      |
    |              |                |                |    response    |
    |              |                |<---------------|                |
    |              |                |                |               |
    |              | 13. emit trace |                |               |
    |              |    (async)     |                |               |
    |              |                |                |               |
    | 14. response |                |                |               |
    |<-------------|                |                |               |
    +              +                +                +               +
```

---

## 8. Component Interaction Map

| Component | Talks To | Protocol | Purpose |
|-----------|----------|----------|---------|
| **Scheduler** | Actor (via mailbox) | Mailbox push (lock-free) | Work distribution |
| **Scheduler** | Location Directory | Read (local cache) | Determine if actor is local |
| **Scheduler** | Checkpoint Manager | Async callback | Trigger post-message checkpoint |
| **Virtual Actor Manager** | Location Directory | Read/Write | Track activations |
| **Virtual Actor Manager** | WASM Sandbox Pool | Acquire/Release | Get WASM instance for activation |
| **Mailbox** | Scheduler | Dequeue (consumer) | Deliver messages to scheduled actor |
| **Mailbox** | Message Router | Enqueue (producer) | Accept incoming messages |
| **Message Router** | Location Directory | Lookup | Resolve actor identity to node |
| **Message Router** | NUL0 Transport | Send frame | Forward remote messages |
| **Message Router** | Local Scheduler | Enqueue | Deliver local messages |
| **Supervisor** | Child actors | EXIT signals, restart commands | Fault recovery |
| **Supervisor** | Parent supervisor | Escalation | Propagate unrecoverable failures |
| **WASM Sandbox Pool** | Wasmtime | Instantiate/Call host | Run actor code |
| **Checkpoint Manager** | Storage Backend | Append | Write checkpoint blobs |
| **Checkpoint Manager** | Actor (COW) | Read memory | Capture actor state |
| **Event Journal** | Storage Backend | Append segment | Persist event log |
| **Event Journal** | Replay Engine | Read segment | Replay events for recovery |
| **Replay Engine** | Actor (WASM) | Inject effect results | Deterministic replay |
| **Recovery Orchestrator** | Event Journal | Read | Load actor state after crash |
| **Recovery Orchestrator** | Scheduler | Activate | Resume recovered actors |
| **Gossip Protocol** | All nodes | UDP multicast / TCP | Membership, failure detection |
| **Gossip Protocol** | Location Directory | Push updates | Propagate placement changes |
| **Gossip Protocol** | CRDT Engine | Piggyback deltas | Efficient CRDT sync |
| **CRDT Engine** | Actor (local) | Apply ops | Local CRDT updates |
| **CRDT Engine** | NUL0 Transport | Send deltas | Cross-node CRDT sync |
| **CRDT Engine** | Storage Backend | Read/Write | Persist CRDT state |
| **Location Directory** | Consistent Hash Ring | Compute | Map identity to node |
| **Location Directory** | Service Registry | Read/Write | Register actor types |
| **Service Registry** | Gossip Protocol | Replicate | Cross-node registry sync |
| **NUL0 Transport** | All nodes | TCP / QUIC | Encrypted inter-node messaging |
| **AI Runtime** | LLM Provider | HTTP/2 (OpenAI, Anthropic) | LLM completions |
| **AI Runtime** | Tool Registry | Read | Discover available tools |
| **AI Runtime** | Actor (via effect) | Effect return | Deliver LLM responses |
| **AI Runtime** | Vector Store | Query/Upsert | Long-term memory |
| **AI Runtime** | Event Journal | Read | Event memory queries |
| **AI Runtime** | Observability | Emit spans | LLM call traces |
| **Tool Registry** | Service Registry | Query | Map tool names to actors |
| **Tool Registry** | Scheduler | Send/Receive | Execute tool calls |
| **Capability Manager** | Actor (compile-time) | Type check | Verify capability usage |
| **Capability Manager** | Message Router | Verify tokens | Validate cross-node caps |
| **Capability Manager** | Revocation List | Append/Check | Lazy capability revocation |
| **ORCA GC** | Actor (per-actor) | Collect | Reclaim actor-local memory |
| **ORCA GC** | Cross-actor | Send decrements | Reclaim cross-actor references |

---

## 9. Architecture Decision Records

### ADR-001: Virtual Actors by Default

**Status:** Accepted
**Context:** Actor lifecycle management is complex and error-prone. Erlang requires explicit spawn/link/monitor. Akka requires actor-of-actor factories. Both force developers to manage lifetimes, introducing coupling between creator and created.
**Decision:** All actors in Nulang are virtual — addressed by identity, runtime manages activation/deactivation/ placement/migration. Creating an actor is as simple as sending it a message.
**Consequences:** (+) Simpler programming model, location transparency, automatic scaling. (-) Requires distributed directory service, potential "cold start" latency for first message.
**References:** Orleans (Microsoft Research), "Orleans: Distributed Virtual Actors for Programmability and Scalability" (Bernstein et al., 2014)

### ADR-002: WASM as Compilation Target

**Status:** Accepted
**Context:** Nulang needs sandboxed execution, cross-language interop, and portable deployment. Options: native code (fast but unsafe), JVM (heavyweight, Oracle licensing), BEAM (Erlang-specific), WASM (sandboxed, portable, standard).
**Decision:** Nulang compiles to WASM components (wasm32-wasip2). Every actor is a WASM component running in Wasmtime.
**Consequences:** (+) Sandboxed execution, cross-language composition, portable deployment, capability-based security via WASI. (-) ~10-20% performance overhead vs native, dependency on Wasmtime project health.
**References:** WebAssembly Component Model W3C specification, Wasmtime runtime

### ADR-003: Four State Models

**Status:** Accepted
**Context:** Different use cases need different persistence/consistency tradeoffs. A single model (e.g., always event-sourced) is too expensive for simple stateless services. No persistence makes Nulang just another actor framework.
**Decision:** Nulang provides four state models: `local` (no persistence), `durable` (automatic checkpointing), `event_sourced` (full audit journal), `crdt` (geo-replicated). The state model is part of the actor's type.
**Consequences:** (+) Optimal cost/performance for each use case, type-safe persistence. (-) More complexity in the runtime, migration between state models requires code changes.
**References:** Orleans grain persistence, Durable Functions stateful entities, Akka Persistence, CRDT research (Shapiro et al.)

### ADR-004: No Separate AI DSL

**Status:** Accepted
**Context:** AI frameworks (LangChain, LangGraph) create parallel programming models for agents. This forces developers to learn two systems and prevents agents from using general distributed systems features.
**Decision:** There is no separate AI DSL in Nulang. An AI agent is an actor that holds the `LLM` capability. Tools are actor behaviors. Memory is actor state. Planning delegates to the workflow subsystem.
**Consequences:** (+) Unified programming model, agents get durability/distribution/supervision for free, no framework lock-in. (-) AI-specific ergonomics (prompt templates, conversation management) must be provided as libraries, not language features.
**References:** LangGraph, AutoGen, "Actors Are All You Need" (design principle)

### ADR-005: Workflows as Actor Graphs

**Status:** Accepted
**Context:** Workflow engines (Temporal, Camunda) are separate systems that integrate poorly with actor models. Workflow steps are not actors — they cannot send/receive messages, hold state, or be supervised individually.
**Decision:** Workflow definitions compile to actor graphs. Each step is a child actor. The workflow actor is a durable state machine tracking step completion. Control flow (parallel, conditional, compensation) compiles to message patterns.
**Consequences:** (+) Workflows inherit all actor properties (durability, distribution, supervision), no separate workflow engine to operate, workflow steps can be any actor. (-) Workflow compilation is complex, workflow visualization requires mapping actor graphs back to workflow steps.
**References:** Temporal (workflow as deterministic program), Saga pattern, Erlang gen_fsm

### ADR-006: ORCA Garbage Collection

**Status:** Accepted
**Context:** Stop-the-world GC is incompatible with actor systems where each actor must process messages with predictable latency. Per-actor heaps need a collection strategy that doesn't pause other actors.
**Decision:** Nulang uses ORCA (Ownership and Reference Counting based on Cycles in Actors) for per-actor garbage collection. Each actor collects independently. Cross-actor references use asynchronous decrement messages.
**Consequences:** (+) <1ms pause per actor, no global pause, naturally parallel. (-) Reference counting overhead (~5-10%), cycle detection requires periodic local tracing, cross-actor decrements are eventual (not immediate).
**References:** "ORCA: A Causal Delta-Order for Concurrent Reclamation" (Clebsch et al., 2015)

### ADR-007: CRDTs Over Consensus for Distributed State

**Status:** Accepted
**Context:** Strongly consistent replicated state requires consensus (Paxos/Raft), which has high latency and is unavailable during partitions. Many distributed use cases (shopping carts, presence, counters) can tolerate eventual consistency.
**Decision:** Nulang uses CRDTs (Conflict-Free Replicated Data Types) as the default for geo-replicated state. Strong consistency is available but opt-in and requires consensus.
**Consequences:** (+) Low latency everywhere, available during partitions, no coordination overhead. (-) Eventual consistency (stale reads possible), limited CRDT types (not all data structures have CRDT equivalents), larger state (need to track tombstones).
**References:** "Conflict-Free Replicated Data Types" (Shapiro et al., 2011), "A Comprehensive Study of Convergent and Commutative Replicated Data Types"

### ADR-008: Algebraic Effects for Side Effects

**Status:** Accepted
**Context:** Side effects (IO, time, randomness) must be tracked for deterministic replay and capability-based security. Options: monadic IO (Haskell — too invasive), unchecked effects (most languages — unsafe), effect handlers (Koka, Eff — right abstraction).
**Decision:** Nulang uses row-polymorphic algebraic effects. Effects are typed, resumable, and tracked in function signatures. Effect results are captured for deterministic replay.
**Consequences:** (+) Clean separation of what from how, testable via handler swapping, automatic tracing, deterministic replay. (-) Effect row syntax adds noise to types, effect handlers are a new concept for many developers.
**References:** Koka (Leijen), Eff (Bauer & Pretnar), "Algebraic Effects for Functional Programming" (Plotkin & Power, 2002)

### ADR-009: Capabilities Map to WASI

**Status:** Accepted
**Context:** Nulang's capability security model needs an implementation mechanism. Inventing a custom capability system would require custom tooling and verification.
**Decision:** Nulang capabilities compile to WASI world imports. The WASM Component Model's import system IS the capability system. If an actor doesn't import `wasi:http/outgoing-handler`, it cannot make HTTP requests — enforced by the WASM sandbox.
**Consequences:** (+) Zero-overhead capability enforcement (sandbox-level, not runtime checks), standard tooling, composable via WIT. (-) Tied to WASI/WASM ecosystem evolution, some capabilities need custom WIT interfaces.
**References:** WASI (WebAssembly System Interface), WASM Component Model, "Capability-Based Security and the WASM Component Model"

### ADR-010: Gossip-Based Cluster Membership

**Status:** Accepted
**Context:** Cluster membership and failure detection need to be decentralized, scalable, and tolerant of network partitions. Options: centralized coordinator (single point of failure), consensus-based (too slow), gossip-based (eventual, scalable).
**Decision:** Nulang uses the SWIM gossip protocol for cluster membership and failure detection. Node state is disseminated via gossip. Actor placement changes propagate via gossip piggybacking.
**Consequences:** (+) Decentralized, scalable to 1000+ nodes, sub-second failure detection, minimal network overhead. (-) Eventual consistency of membership view, false positives under high packet loss, no global consistency guarantee.
**References:** SWIM (Scalable Weakly-consistent Infection-style Process Group Membership Protocol) (Das et al., 2002), HashiCorp Memberlist

### ADR-011: Indentation-Sensitive Syntax

**Status:** Accepted
**Context:** Syntax style debates waste time. Braces (C-style) are redundant with proper indentation. Python proved indentation-based syntax works at scale.
**Decision:** Nulang uses mandatory 2-space indentation. No braces, no semicolons, no configuration. A deterministic formatter is required (like gofmt).
**Consequences:** (+) Visual hierarchy matches semantic hierarchy, no style debates, smaller code, enforced consistency. (-) Whitespace sensitivity can surprise newcomers, copy-paste can break indentation, hard to generate programmatically.
**References:** Python, Haskell, F#, gofmt

### ADR-012: No Async/Await Syntax

**Status:** Accepted
**Context:** Async/await creates "colored functions" — sync functions can't call async functions, leading to contagious `async` annotations throughout codebases. In an actor model, all concurrency is via message passing, not async tasks.
**Decision:** Nulang has no async/await syntax. Actors process messages sequentially within a behavior handler. Concurrency comes from having many actors. Effect handlers may be async internally, but this is invisible to actor code.
**Consequences:** (+) No colored functions, simpler mental model, behavior handlers are atomic from the actor's perspective. (-) Cannot express intra-actor concurrency, long operations must be split into multiple messages.
**References:** "What Color is Your Function?" (Nathaniel Smith, 2015), Erlang, original actor model (Hewitt et al., 1973)

---

## 10. Performance Targets

These targets define the performance envelope for a production Nulang deployment on commodity hardware (AMD EPYC / Intel Xeon, NVMe SSD, 10Gbps network).

### 10.1 Actor Lifecycle

| Metric | Target | Measurement | Notes |
|--------|--------|-------------|-------|
| Actor creation (activation) | < 1 microsecond | Time from first message to behavior handler start | Warm WASM instance from pool |
| Actor creation (cold start) | < 10 milliseconds | Time when WASM instance must be compiled | JIT compilation cost |
| Actor deactivation | < 1 millisecond | Time to checkpoint and release resources | Excluding storage write latency |
| Actor migration (same DC) | < 5 milliseconds | Time to stop on source, start on target | State transfer via shared storage |

### 10.2 Messaging

| Metric | Target | Measurement | Notes |
|--------|--------|-------------|-------|
| Message send (local) | < 100 nanoseconds | Time to enqueue in mailbox | Lock-free ring buffer |
| Message send (remote, same DC) | < 1 millisecond | End-to-end including serialization | TLS + TCP, under 1ms at p99 |
| Message send (cross-region) | < 50 milliseconds | Transatlantic / transpacific | Depends on geography |
| Message processing throughput | > 1M messages/second/node | Sustained, `local` actors | 64-byte messages, batching enabled |
| Mailbox capacity | 10,000 messages | Default, configurable | Bounded to prevent OOM |

### 10.3 Durability

| Metric | Target | Measurement | Notes |
|--------|--------|-------------|-------|
| Checkpoint latency | < 10 milliseconds | COW snapshot + serialization | Excluding storage backend write |
| Storage write (SQLite) | < 1 millisecond | Append to local SQLite | NVMe SSD |
| Storage write (PostgreSQL) | < 5 milliseconds | Network roundtrip to DB | Same datacenter |
| Storage write (S3) | < 50 milliseconds | HTTP PUT to object store | Regional endpoint |
| Event journal append | < 1 millisecond | Local segment file append | Sequential write, fsync optional |
| Replay throughput | > 100K events/second | Events replayed per second | From SSD, single-threaded |
| Recovery time | < 5 seconds | Node failure to all actors resumed | 100K actors, NVMe storage |

### 10.4 Scheduling

| Metric | Target | Measurement | Notes |
|--------|--------|-------------|-------|
| Scheduler latency | < 10 microseconds | Time from message arrival to actor scheduled | Work-stealing efficiency |
| Context switch (actor) | < 50 nanoseconds | WASM instance swap | Instance pool hot path |
| Fairness guarantee | Max 2x ideal share | No actor starves others | Reduction counting |
| Max actors per node | > 1,000,000 | Active + deactivated (memory-bound) | 1KB average per actor |
| Max active actors | > 100,000 | Actually scheduled | CPU-bound |

### 10.5 Garbage Collection

| Metric | Target | Measurement | Notes |
|--------|--------|-------------|-------|
| GC pause (per-actor) | < 1 millisecond | Stop time for collection | ORCA cycle detection |
| GC overhead | < 5% | CPU time spent in GC | Reference counting dominant |
| Memory per actor | < 1KB baseline | Empty actor overhead | WASM instance shared |
| Heap limit per actor | 100MB default, 1GB max | Configurable | Enforced by WASM memory bounds |

### 10.6 Distributed

| Metric | Target | Measurement | Notes |
|--------|--------|-------------|-------|
| Cluster size | > 100 nodes | Tested, supported | Gossip protocol scales sub-linearly |
| Failure detection | < 3 seconds | Time to detect node failure | SWIM with suspicion mechanism |
| Gossip propagation | < 500ms | Time for update to reach all nodes | 100-node cluster |
| CRDT sync latency | < 100ms eventual | Time to converge after update | Same region, periodic sync |
| CRDT sync (cross-region) | < 5 seconds | Time to converge globally | Periodic sync mode |
| Consistent hash lookup | < 1 microsecond | Identity to node resolution | In-memory ring |

### 10.7 AI Runtime

| Metric | Target | Measurement | Notes |
|--------|--------|-------------|-------|
| LLM call latency (local) | < 100ms | End-to-end, cached tools | Excluding model inference time |
| Tool registration | < 1ms | Schema generation + registry | Per tool, at deploy time |
| Tool execution | Same as message send | Tool is just an actor message | + LLM roundtrip |
| Vector query | < 10ms | Semantic similarity search | HNSW index, local Qdrant |
| Vector upsert | < 5ms | Add to vector store | Batched async |
| Trace emission | < 1ms | Fire-and-forget to collector | Async, buffered |

### 10.8 Workflow Throughput

| Metric | Target | Measurement | Notes |
|--------|--------|-------------|-------|
| Workflow start | < 5ms | Time from request to first step | Includes compilation for dynamic workflows |
| Step throughput | > 10K steps/second/node | Completed workflow steps | `durable` actors, local storage |
| Compensation execution | < 50ms per step | Time to execute compensation | Sequential, reverse order |
| Human-in-the-loop resume | < 2 seconds | Time from human action to workflow resume | Includes activation + replay |
| Parallel step fan-out | 10 steps default, 100 max | Concurrent parallel steps | Configurable |

---

## 11. References

### Academic Papers

1. Hewitt, C., Bishop, P., & Steiger, R. (1973). "A Universal Modular ACTOR Formalism for Artificial Intelligence." *IJCAI*.
2. Agha, G. (1986). *Actors: A Model of Concurrent Computation in Distributed Systems.* MIT Press.
3. Bernstein, P. A., et al. (2014). "Orleans: Distributed Virtual Actors for Programmability and Scalability." *Microsoft Research Technical Report*.
4. Shapiro, M., Preguica, N., Baquero, C., & Zawirski, M. (2011). "Conflict-Free Replicated Data Types." *SSS*.
5. Clebsch, S., Drossopoulou, S., Blessing, S., & McNeil, A. (2015). "Deny Capabilities for Safe, Fast Actors." *AGERE!*.
6. Das, A., Gupta, I., & Motivala, A. (2002). "SWIM: Scalable Weakly-consistent Infection-style Process Group Membership Protocol." *ICDCS*.
7. Leijen, D. (2017). "Type Directed Compilation of Row-Typed Algebraic Effects." *POPL*.
8. Plotkin, G., & Power, J. (2002). "Notions of Computation Determine Monads." *FOSSACS*.

### Industry Systems

1. **Erlang/OTP** — Ericsson. The original actor language and runtime.
2. **Orleans** — Microsoft. Virtual actors for .NET.
3. **Akka** — Lightbend. Actor toolkit for JVM (now Apache Pekko).
4. **Temporal** — Temporal Technologies. Durable execution platform.
5. **Dapr** — Microsoft. Distributed application runtime with building blocks.
6. **Wasmtime** — Bytecode Alliance. WASM runtime for the Component Model.
7. **Cloudflare Workers** — Edge WASM runtime (inspiration for deployment targets).

### Specifications

1. **WebAssembly Core Specification 2.0** — W3C.
2. **WebAssembly Component Model** — W3C Community Group.
3. **WASI Preview 2** — WebAssembly System Interface.
4. **OpenTelemetry** — CNCF observability standard.
5. **Cap'n Proto Serialization** — Cap'n Proto project.

---

*End of Architecture Reference*
