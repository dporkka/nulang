# BEAM/OTP Primitives for Nulang: Adoption Analysis

## Overview

The BEAM (Bogdan/Bjorn's Erlang Abstract Machine) and OTP (Open Telecom Platform) define the gold standard for fault-tolerant distributed systems primitives, refined over 35 years of production use at Ericsson, WhatsApp, and thousands of other systems. This document maps the full BEAM/OTP primitive surface to Nulang's architecture, categorizing each primitive as **Adopt**, **Adapt**, **Replace**, or **Omit** based on Nulang's existing design.

> **Document status.** Sections 1–16 are the original *design* map: **ADOPT**/**ADAPT**/**REPLACE**/**OMIT** are design decisions, not implementation claims, and several rows (originally marked **ADOPTED**) overstated what exists. Those rows have been corrected inline. For the verified, source-checked state of the runtime — what is implemented, what is stubbed, and what does not exist — see **Section 17: Implementation Status (Ground Truth)**.

---

## 1. Core Actor Lifecycle Primitives

### 1.1 Process/Actor Creation

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `spawn(Fun)` | **IMPLEMENTED** | `spawn ActorType { field = value }` | Parser + `Spawn` opcode (0x80) → `ActorVmCallbacks::spawn_actor`. Field-init record, not a function. |
| `spawn(Module, Fun, Args)` | **ADAPT** | `spawn ActorType(args)` | Nulang uses typed actor constructors rather than dynamic module references. Positional-arg spawn is not implemented (only field init). |
| `spawn_link(Fun)` | **ADOPT — NOT IMPLEMENTED** | `spawn_link ActorType` | Essential for supervision trees. Bidirectional fault propagation. No `spawn_link` syntax or opcode exists; linking is a separate `Runtime::link_actors` call. |
| `spawn_monitor(Fun)` | **ADOPT — NOT IMPLEMENTED** | `spawn_monitor ActorType` | Returns `{ActorRef, MonitorRef}`. Unilateral observation with down notifications. No `spawn_monitor` exists; monitoring is a separate `Runtime::monitor` call. |
| `spawn_opt(Fun, Options)` | **ADAPT — NOT IMPLEMENTED** | `spawn ActorType with options { ... }` | Nulang should support: `priority`, `scheduler_hint`, `max_heap_size`, `link`, `monitor`. None of these options exist today. |

**Design Note:** Nulang's typed actors eliminate the need for `apply/3` and dynamic function calls. As implemented, `spawn` returns an `ActorRef` directly (a NaN-boxed actor id), not `Result[ActorRef, SpawnError]` — spawn cannot fail in the current runtime.

### 1.2 Process/Actor Identity

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `self()` | **IMPLEMENTED** | `self` keyword → `SelfOp` opcode (0x83) | Returns the current actor's `ActorId` as an `ActorRef`. |
| `pid_to_list(Pid)` | **OMIT** | — | Not needed; actor ids are plain integers. |
| `list_to_pid(String)` | **OMIT** | — | Unsafe in typed system. |
| `is_process_alive(Pid)` | **ADOPT — NOT IMPLEMENTED** | `actor.is_alive(actor_ref)` | Essential for liveness checks. No such builtin exists. |
| `process_info(Pid)` | **ADAPT — NOT IMPLEMENTED** | `actor.info(actor_ref)` | Returns typed `ActorInfo` record: mailbox size, memory, reductions, links, monitors, current behavior. Fields exist on the Rust `Actor` struct; no language-level accessor. |
| `processes()` | **ADAPT — NOT IMPLEMENTED** | `actor.list()` | Returns list of all actor refs on the node. (`Runtime.actors` keys in Rust.) |
| `register(Name, Pid)` | **IMPLEMENTED (runtime API)** | `Runtime::registry.register(name, id)` | Local name registry with name validation. Returns `Result[Unit, RegisterError]`. No Nulang-level `actor.register` builtin. |
| `unregister(Name)` | **IMPLEMENTED (runtime API)** | `Runtime::registry.unregister(name)` | Remove from local registry. Also auto-removed on actor exit (`unregister_by_actor`). |
| `whereis(Name)` | **IMPLEMENTED (runtime API)** | `Runtime::registry.whereis(name)` | Returns `Option<u64>`. |
| `registered()` | **IMPLEMENTED (runtime API)** | `Runtime::registry.registered()` | Returns list of registered names. |

**Design Note:** OTP's global registry (`global:register_name/2`) is planned to be subsumed by Nulang's virtual actor system (Orleans-style identity-based addressing), which is **not yet implemented** — see §6.3. Local registration (`register/2`) exists today as the `ActorRegistry` (`src/runtime/registry.rs`) and is exercised by the test suite, but it is not reachable from Nulang source code.

### 1.3 Termination and Signals

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `exit(Pid, Reason)` | **IMPLEMENTED (runtime API)** | `Runtime::exit_actor(id, reason)` / `Runtime::kill_actor(id)` | Terminates the actor and runs exit handling (DOWN messages, link propagation, supervisor notification). No language-level `actor.exit` builtin; the `Exit` opcode (0x89) is defined but unhandled by the VM. |
| `exit(Reason)` | **ADOPT — NOT IMPLEMENTED** | `exit(reason)` | Exit the current actor from Nulang source. |
| `kill` (reason) | **IMPLEMENTED (with caveat)** | `ExitReason::Kill` via `Runtime::kill_actor` | Documented as untrappable in `src/types.rs`, but `handle_actor_exit` treats it like any abnormal reason — a `trap_exits` actor currently converts it to a message. The `Killed` variant is defined but never constructed. |
| `normal` (reason) | **IMPLEMENTED** | `ExitReason::Normal` | Normal termination, no link propagation, no supervisor restart for `Transient` children. |
| `process_flag(trap_exit, true)` | **IMPLEMENTED (runtime API)** | `Actor.trap_exits` field | Convert exit signals to `System`-priority messages. Honored in `Runtime::handle_actor_exit`; settable only from Rust (no language builtin). |
| `process_flag(priority, Level)` | **ADOPT — NOT IMPLEMENTED** | `actor.set_priority(Level)` | `high`, `normal`, `low`. Bound to scheduler. No actor priority field exists; `MessagePriority` (System/Normal/Bulk) is message-level only and does not affect scheduling. |

**Design Note:** `ExitReason` is implemented in `src/types.rs` as:

```nulang
type ExitReason =
  | Normal
  | Kill                    -- untrappable
  | Killed                  -- a Kill that propagated to this actor
  | Shutdown(Option[Duration])
  | Error(String)
  | Custom(String)
```

`Kill` maps to DOWN reason code 2, `Killed` to 3 (see §17 for the DOWN message encoding).

---

## 2. Message Passing Primitives

### 2.1 Send Operations

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `Pid ! Message` | **IMPLEMENTED** | `send actor_ref behavior(args)` | Non-blocking, asynchronous → `Send` opcode (0x81) → `Runtime::send_message_by_id`. **Note:** the `<-` operator shown in earlier drafts is not Nulang syntax (the lexer tokenizes `<-` but the parser never uses it); `!` is unary-not. |
| `Name ! Message` | **ADAPT — NOT IMPLEMENTED** | `registered_name <- message` | Send to registered name. Requires an explicit `whereis` lookup today (registry is Rust-only). |
| `{Name, Node} ! Message` | **IMPLEMENTED (runtime API)** | `Runtime::send_distributed(ActorAddress, behavior, args)` | `ActorAddress::Local`/`Remote` give location-transparent routing. Remote sends carry the behavior **name** on the wire; the receiver resolves it to a behavior id on delivery (unknown names fall back to behavior 0, mirroring local sends). |

### 2.2 Receive (Critical Addition)

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `receive ... end` | **IMPLEMENTED (non-blocking)** | `receive { \| Behavior(params) => expr }` | Selective receive is wired end-to-end: MIR lowering (`lower_receive` in `src/mir_lower.rs`) resolves arm names to behavior-table indices and emits `OpCode::ReceiveMatch` (0x8F); the VM scans the mailbox in FIFO order via `ActorVmCallbacks::try_receive_match` (`Mailbox::receive_match`), skips non-matching messages (left queued), binds payload values to arm params (missing → nil, extras ignored), and dispatches to the arm body. Non-blocking: no-match falls back to pop-any (nil when empty). No `after` timeout yet. |

**Nulang `receive` Design:**

```nulang
-- Basic receive with pattern matching
let msg = receive {
  | TemperatureReading(temp) => temp
  | ErrorSignal(reason) => {
      perform io.println("Error: " ++ reason)
      0.0
    }
}

-- Receive with timeout (after)
let result = receive {
  | Response(data) => Ok(data)
  | Error(reason) => Error(reason)
} after Duration.milliseconds(5000) {
  Error("Timeout waiting for response")
}

-- Nested/repeated receive (server loop)
behavior server_loop(state: ServerState) {
  receive {
    | Get(key, reply_to) => {
        let value = Map.get(state.store, key)
        reply_to <- Reply(value)
        server_loop(state)
      }
    | Put(key, value) => {
        let new_state = { state .. store = Map.insert(state.store, key, value) }
        server_loop(new_state)
      }
    | Stop => {
        perform io.println("Server stopping")
        exit(Normal)
      }
  }
}
```

**Design Note:** Unlike Erlang's `receive` (which scans the entire mailbox), Nulang's `receive` should compile to efficient mailbox matching. Messages that don't match are left in the mailbox for future receives. The `after` clause provides timeout semantics for request-response patterns.

**Reality check:** the basic-receive example above is implemented today (behavior-name arms, payload binding, selective mailbox scan via `OpCode::ReceiveMatch` / `Mailbox::receive_match`). Two differences from Erlang remain: `receive` never blocks — when no queued message matches, a legacy fallback pops the next message and yields its first payload value (nil when empty) — and the `after` clause is not in the grammar. Message delivery to running actors still primarily happens through behavior dispatch in `Runtime::step_actor`; the legacy pop-any `Receive` opcode (0x84) remains as the no-match fallback path.

### 2.3 Selective Receive Considerations

OTP's selective receive is both powerful and problematic — it can cause mailbox bloat when messages don't match any pattern. Nulang should:

1. **Support selective receive** (required for Erlang compatibility patterns) — **implemented** (mailbox-order scan, non-matching messages stay queued; non-blocking)
2. **Provide mailbox inspection** (`actor.mailbox_size(self)`) for monitoring — exists as `Mailbox::len()` in Rust; no language builtin
3. **Warn at compile time** if a behavior has a `receive` with no catch-all pattern (potential mailbox leak) — **not implemented**
4. **Support `receive` with `flush`** to clear non-matching messages after timeout — **not implemented**

---

## 3. Linking and Monitoring

### 3.1 Links (Bidirectional)

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `link(Pid)` | **IMPLEMENTED (runtime API)** | `Runtime::link_actors(a, b)` | Bidirectional fault propagation. Abnormal exit of either side terminates the other (or delivers a `System` message if it traps exits). `Link` opcode (0x87) is defined but unhandled by the VM; no language builtin. |
| `unlink(Pid)` | **IMPLEMENTED (runtime API)** | `Runtime::unlink_actors(a, b)` | Remove link. |

### 3.2 Monitors (Unidirectional)

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `erlang:monitor(process, Pid)` | **IMPLEMENTED (runtime API)** | `Runtime::monitor(watcher, target)` | Monitoring a dead/unknown target immediately delivers DOWN with reason `Error("noproc")`. `Monitor` opcode (0x85) is defined but unhandled by the VM; no language builtin. |
| `erlang:demonitor(Ref)` | **IMPLEMENTED (runtime API)** | `Runtime::demonitor(watcher, target)` | Remove monitor. Identified by `(watcher, target)` actor-id pair — there is no `MonitorRef` type. |
| `erlang:demonitor(Ref, [flush])` | **ADOPT — NOT IMPLEMENTED** | `actor.demonitor(monitor_ref, flush: true)` | Remove and flush pending DOWN message. The implemented `demonitor` has no flush variant. |

**Design Note:** Monitors are stored as watcher-id lists on the target actor (`Actor.monitors`). On death, `Runtime::handle_actor_exit` sends each watcher a `System`-priority DOWN message with `behavior_id = 0` and payload `[target_id, watcher_id, reason_code]` where reason codes are `Normal=0, Error=1, Kill=2, Killed=3, Shutdown=4, Custom=5`. The typed `MonitorMessage`/`MonitorRef` surface below remains the design target:

```nulang
type MonitorMessage =
  | Down { monitor_ref: MonitorRef, actor_ref: ActorRef, reason: ExitReason }

-- Monitoring pattern
let monitor_ref = actor.monitor(worker)
receive {
  | Down { monitor_ref = m, reason, .. } if m == monitor_ref => {
      perform io.println("Worker died: " ++ reason.to_string())
      -- Restart worker
      let new_worker = spawn_link Worker
      server_loop(new_worker)
    }
}
```

---

## 4. OTP Behaviors (Generic Patterns)

### 4.1 gen_server

**Status: ADAPT as `behavior` patterns**

Erlang's `gen_server` callback module pattern should be available as a Nulang behavior mixin:

```nulang
actor KeyValueStore {
  use GenServer  -- OTP gen_server pattern

  state durable store: Map[String, String] = Map.empty()

  -- handle_call: synchronous request-response
  behavior get(key: String): Option[String] {
    Map.get(store, key)
  }

  -- handle_cast: asynchronous fire-and-forget
  behavior put(key: String, value: String) {
    store = Map.insert(store, key, value)
  }

  -- handle_info: catch-all for non-behavior messages
  on_info message {
    match message {
      | SystemMessage(ReloadConfig) => reload_config()
      | _ => perform io.println("Unknown message: " ++ message.to_string())
    }
  }
}
```

Key `gen_server` primitives:

| gen_server Function | Nulang Status | Nulang Form |
|---------------------|---------------|-------------|
| `gen_server:start_link/4` | **NOT IMPLEMENTED** | `spawn_link` does not exist; use `Runtime::link_actors` after `spawn` |
| `gen_server:call/2` | **IMPLEMENTED** | `ask store get("key")` — `Ask` opcode (0x82) → `Runtime::ask_actor_sync` (synchronous, single-threaded runtime) |
| `gen_server:cast/2` | **IMPLEMENTED** | `send store put("key", "value")` (not `<-`) |
| `gen_server:reply/2` | **OMIT** | Built into `ask`/behavior return |
| `gen_server:stop/1` | **IMPLEMENTED (runtime API)** | `Runtime::exit_actor(id, ExitReason::Normal)`; no `actor.stop` builtin |
| `gen_server:abcast/2` | **ADAPT — NOT IMPLEMENTED** | Distributed broadcast via `cluster.broadcast/2` — no broadcast API exists |

The `use GenServer` mixin, `state durable` sugar, and `on_info` catch-all shown in the example above are design sketches — none of them parse today. Actor state is declared per-field and given a `StateModel` (`Local`/`Durable`/`EventSourced`/`Crdt`) through the runtime API.

### 4.2 gen_statem

**Status: ADAPT as `state_machine` behavior — NOT IMPLEMENTED** (no `state_machine` keyword exists; the example below is aspirational syntax)

State machines are critical for protocol handling, workflow engines, and AI agent state management:

```nulang
state_machine TcpConnection {
  state Closed

  event connect(address): Connecting
  event connection_established: Connected
  event disconnect: Closed
  event data_received(bytes): handle_data
  event timeout: handle_timeout

  -- State entry/exit actions
  on_entry Connected {
    perform io.println("Connection established")
  }

  on_exit Connected {
    perform io.println("Connection closing")
  }
}
```

Key `gen_statem` primitives:

| gen_statem Function | Nulang Status | Nulang Form |
|---------------------|---------------|-------------|
| `gen_statem:call/2` | **NOT IMPLEMENTED** | `ask fsm event` works for any actor, but no state-machine semantics |
| `gen_statem:cast/2` | **NOT IMPLEMENTED** | `send fsm event` works for any actor, but no state-machine semantics |
| Event actions | **ADAPT — NOT IMPLEMENTED** | Declarative event handlers with state transitions |
| State enter/exit | **ADAPT — NOT IMPLEMENTED** | `on_entry` / `on_exit` hooks |

### 4.3 gen_event

**Status: ADAPT as `event_bus` behavior — NOT IMPLEMENTED** (`use EventBus` does not parse)

Event buses are essential for pub/sub patterns, logging pipelines, and metrics collection:

```nulang
actor MetricsBus {
  use EventBus

  behavior add_handler(handler: ActorRef) {
    add_event_handler(handler)
  }

  behavior report(metric: Metric) {
    notify(metric)
  }
}
```

### 4.4 supervisor

**Status: IMPLEMENTED (runtime API)** — supervision trees exist in `src/runtime/supervisor.rs` and are exercised by unit and stress tests. There is no Nulang-level supervisor DSL yet.

OTP supervisor primitives to ensure are complete:

| Supervisor Primitive | Nulang Status | Nulang Form |
|----------------------|---------------|-------------|
| `supervisor:start_link/2` | **IMPLEMENTED (runtime API)** | `Runtime::create_supervisor(name, strategy)` + `Runtime::supervise_child(sup, spec, child)` |
| `supervisor:start_child/2` | **IMPLEMENTED (runtime API)** | `Supervisor::add_child(spec, actor_id)` (via `supervise_child`) |
| `supervisor:terminate_child/2` | **NOT IMPLEMENTED** | No dedicated terminate; `exit_actor` on the child routes through the supervisor's restart policy |
| `supervisor:restart_child/2` | **IMPLEMENTED (runtime API)** | `Supervisor::restart_child(actor_id, runtime)` |
| `supervisor:delete_child/2` | **IMPLEMENTED (internal)** | `Supervisor::remove_child` (private; invoked on `Temporary`/normal-`Transient` exits) |
| `supervisor:which_children/1` | **IMPLEMENTED (runtime API)** | `Supervisor::child_count()` / `children` field |
| Restart strategies | **IMPLEMENTED: 3 of 4** | `one_for_one`, `one_for_all`, `rest_for_one`. **`simple_one_for_one` does not exist.** |

Restart semantics: three restart policies (`Permanent`, `Temporary`, `Transient`), per-child rate limiting (`max_restarts = 5` within `restart_window_secs = 60` by default, tracked per child-spec id), and escalation — exceeding the limit returns `SupervisorAction::Shutdown`, which cascades to the parent supervisor. Note that restarts recreate a fresh actor with a new id; bytecode/behavior restoration for restarted children is future work (the recreated child is a bare `Actor` today).

**Design Note:** Nulang should support dynamic supervision (adding children at runtime — partially present via `add_child`) and `simple_one_for_one` (template-based child creation), both critical for connection pools and worker pools.

---

## 5. In-Memory Storage

### 5.1 ETS (Erlang Term Storage)

**Status: ADAPT as `actor.local_table` — NOT IMPLEMENTED** (no `Table` type, no `capability table`; the example below is aspirational)

ETS is critical for fast in-memory key-value access within a node. Nulang should provide ETS-like tables as a capability-gated feature:

```nulang
actor CacheService {
  capability table  -- Grants access to local tables

  state local cache: Table[String, CachedItem] = Table.new(
    type = Set,           -- Set, OrderedSet, Bag, DuplicateBag
    keypos = 1,           -- Position of key element
    read_concurrency = true,
    write_concurrency = true
  )

  behavior get(key: String): Option[CachedItem] {
    Table.lookup(cache, key)
  }

  behavior put(key: String, value: CachedItem) {
    Table.insert(cache, (key, value))
  }
}
```

Key ETS primitives:

| ETS Function | Nulang Status | Nulang Form |
|-------------|---------------|-------------|
| `ets:new/2` | **ADAPT** | `Table.new(options)` |
| `ets:insert/2` | **ADAPT** | `Table.insert(table, row)` |
| `ets:lookup/2` | **ADAPT** | `Table.lookup(table, key)` |
| `ets:delete/1,2` | **ADAPT** | `Table.delete(table)` / `Table.delete(table, key)` |
| `ets:match/1,2,3` | **OMIT** | Use `Table.filter()` with Nulang lambdas |
| `ets:foldl/3` | **ADAPT** | `Table.fold(table, init, fn)` |
| `ets:tab2list/1` | **ADAPT** | `Table.to_list(table)` |
| `ets:info/1` | **ADAPT** | `Table.info(table)` |
| `ets:select/1,2,3` | **OMIT** | Replaced by typed `Table.filter()` and `Table.query()` |

**Design Note:** ETS's `match_spec` (a mini-query language) is powerful but untyped. Nulang should replace it with typed filter/query functions. ETS tables should be **actor-local** (not globally accessible) to maintain capability safety.

### 5.2 Persistent Term

**Status: ADOPT as `persistent_term` — NOT IMPLEMENTED**

`persistent_term` (OTP 21.2+) provides zero-copy global immutable terms, perfect for configuration and compiled patterns:

```nulang
-- Store: O(1), no copying
persistent_term.put(http_config, config)

-- Read: O(1), no copying, no GC impact
let config = persistent_term.get(http_config)
```

| persistent_term Primitive | Nulang Status |
|---------------------------|---------------|
| `persistent_term:put/2` | **ADOPT** |
| `persistent_term:get/1,2` | **ADOPT** |
| `persistent_term:erase/1` | **ADOPT** |

### 5.3 Mnesia

**Status: REPLACE with persistent actors + CRDTs**

Mnesia (Erlang's distributed database) is largely subsumed by Nulang's design:

| Mnesia Feature | Nulang Replacement |
|----------------|-------------------|
| In-memory tables | Persistent actors with `local` state |
| Disk-backed tables | Persistent actors with `durable` state |
| Distributed replication | Persistent actors with `crdt` state |
| Transactions | Workflow `step` with compensation |
| Schema management | Actor type definitions |
| `dirty_read/write` | Direct behavior calls |
| `qlc` queries | Nulang's `List`/`Array` operations |

Mnesia's complex transaction semantics and schema evolution are pain points. Nulang's actor-centric persistence is simpler and more robust.

**Implementation status of the replacement:** the persistent-actor layer is real and tested — `PersistenceStore` (`src/runtime/persistence.rs`) with `MemoryStore`, `JsonFileStore`, and `SqliteStore` (rusqlite) backends; per-field `StateModel` (`Local`/`Durable`/`EventSourced`/`Crdt`); journal + snapshot checkpointing; and an 8-variant `WorkflowEvent` journal for event-sourced workflow actors with deterministic replay on recovery. The CRDT row is also implemented — 8 CRDT types (`GCounter`, `PNCounter`, `GSet`, `ORSet`, `AWORSet`, `LWWRegister`, `MVRegister`, `RGA`) behind `CrdtManager`, synced over `CrdtSync` packets. The `workflow step with compensation` row is implemented as saga compensation for workflow steps.

---

## 6. Distribution Primitives

### 6.1 Node Management

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `node()` | **IMPLEMENTED (runtime API)** | `NodeId` opcode (0xD0); `Runtime::node_id` | Returns current `NodeId`. No `cluster.this_node()` builtin. |
| `nodes()` | **IMPLEMENTED (runtime API)** | `ClusterState::all_members()` | Returns list of known nodes. |
| `nodes(connected)` | **IMPLEMENTED (runtime API)** | `ClusterState::healthy_members()` | Explicit connected filter. |
| `nodes(visible)` | **ADOPT — NOT IMPLEMENTED** | `cluster.visible_nodes()` | No visible/hidden node distinction; `NodeStatus` is `Joining`/`Healthy`/`Suspicious`/`Failed`/`Leaving`. |
| `is_alive()` | **IMPLEMENTED (runtime API)** | `Runtime::distributed_enabled` | Whether distribution is enabled (`enable_distribution` binds the transport). |
| `net_kernel:connect_node/1` | **IMPLEMENTED (runtime API)** | `Runtime::join_cluster(seed_addr)` | Gossip-based cluster join (`ClusterState::join_cluster`). |
| `erlang:monitor_node/2` | **ADAPT — NOT IMPLEMENTED** | `cluster.monitor_node(node_id)` | Receive `nodedown` / `nodeup` messages. Node failure produces `ClusterAction::NodeFailed` internally but is not delivered to actors as messages. |
| `erlang:set_cookie/2` | **OMIT** | — | Replaced by capability-based authentication (planned; no auth on the wire today). |

### 6.2 Remote Operations

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `{Name, Node} ! Message` | **IMPLEMENTED (runtime API)** | `Runtime::send_distributed(ActorAddress, behavior, args)` | Location-transparent routing via `AddressResolver` + LRU `RemoteActorCache` (10,000 entries). Remote `ActorMessage` packets carry the behavior **name**; the receiver resolves it via `Runtime::behavior_id_for` on delivery (unknown names fall back to behavior 0, mirroring local sends). The `RSend` opcode (0xD2) is a no-op in the VM. |
| `rpc:call/4` | **IMPLEMENTED (partial)** | `RAsk` opcode (0xD3) → `DistributedVmCallbacks::remote_ask(target, behavior, args, 5000ms)` | Type-safe RPC. Only through the VM callback; returns `nil` when no distributed runtime is attached. |
| `rpc:multicall/4` | **ADAPT — NOT IMPLEMENTED** | `cluster.multicall(nodes, behavior, args)` | Parallel RPC to multiple nodes. |
| `rpc:cast/4` | **ADAPT — NOT IMPLEMENTED** | `cluster.cast(node, behavior, args)` | Fire-and-forget remote call. |
| `rpc:abcast/3` | **ADAPT — NOT IMPLEMENTED** | `cluster.broadcast(behavior, args)` | Broadcast to all connected nodes. |
| `rpc:sbcast/3` | **ADAPT — NOT IMPLEMENTED** | `cluster.broadcast_sync(behavior, args)` | Synchronous broadcast. |
| `spawn(Node, ...)` | **STUB** | `distributed::spawn_on_node` sends `Packet::SpawnRequest` | The receiver never handles `SpawnRequest` (dropped in `process_network_packets`); `RSpawn` opcode (0xD4) returns `actor_ref(0)`. Remote spawn does not work end-to-end. |

### 6.3 Global Name Registration

**Status: REPLACE with virtual actors — NOT IMPLEMENTED** (no `virtual` keyword; the example below is aspirational)

Erlang's `global` module provides cluster-wide name registration. Nulang's **virtual actors** (identity-based, location-transparent) provide the same capability more elegantly:

```nulang
-- Instead of global:register_name(KeyValueService, Pid)
-- Nulang uses virtual actor identity:
let store = virtual KeyValueStore("user-cart-123")
store <- add_item(item)  -- Routes to whichever node hosts it
```

| global Primitive | Nulang Replacement |
|------------------|-------------------|
| `global:register_name/2` | `virtual Actor("identity")` |
| `global:unregister_name/1` | Actor lifecycle management |
| `global:whereis_name/1` | Transparent routing (no explicit lookup needed) |
| `global:re_register_name/2` | Virtual actor reactivation on new node |
| `global:sync/0` | CRDT-based state convergence |

---

## 7. Timer and Scheduling Primitives

### 7.1 Timers

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `erlang:send_after/3` | **IMPLEMENTED (runtime API)** | `TimerWheel::send_after(delay, target, behavior_id, payload)` → `TimerId` | Critical for timeouts, retries, scheduled tasks. Min-heap wheel, driven by `Runtime::tick_timers` on every scheduler loop iteration. No `timer.*` language builtins. |
| `erlang:start_timer/3` | **IMPLEMENTED (runtime API)** | `TimerWheel::send_after(...)` returns `TimerId` | Same as `send_after` in Nulang's model. |
| `erlang:cancel_timer/1` | **IMPLEMENTED (runtime API)** | `TimerWheel::cancel(TimerId)` | Lazy cancellation (flag checked at fire time); returns `bool`, not remaining time. |
| `erlang:read_timer/1` | **IMPLEMENTED (runtime API)** | `TimerWheel::remaining(TimerId)` | Returns `Option[Duration]`. |
| `timer:apply_after/4` | **OMIT** | — | Use `send_after` with behavior message. |
| `timer:exit_after/2` | **IMPLEMENTED (runtime API)** | `TimerWheel::exit_after(delay, target, reason)` | Exits actor with `ExitReason::Error(reason)` after timeout. |
| `timer:kill_after/1` | **IMPLEMENTED (runtime API)** | `TimerWheel::kill_after(delay, target)` | Unconditional kill (`ExitReason::Kill`) after timeout. |
| `timer:sleep/1` | **IMPLEMENTED (workflow-scoped)** | `perform Timer.sleep(name, duration_ms)` inside a `workflow` step | A **durable** workflow timer: journaled (`TimerSet`/`TimerFired` events) and re-armed on recovery. There is no general blocking `time.sleep`; the `Time` effect exists for tracking only. |

### 7.2 Scheduling Hints

| BEAM Primitive | Nulang Status | Nulang Form |
|----------------|---------------|-------------|
| `erlang:yield/0` | **REPLACE** | Automatic: the scheduler preempts an actor after `max_reductions = 1000` reductions (`Actor::should_yield`) and re-enqueues it if the mailbox is non-empty. The `Yield` opcode (0x8A) is defined but never emitted or handled; no `scheduler.yield()` builtin. |
| `erlang:hibernate/3` | **ADAPT — NOT IMPLEMENTED** | `actor.hibernate()` | Minimize memory footprint until next message. |
| `erlang:garbage_collect/0,1` | **ADAPT — NOT IMPLEMENTED** | `gc.collect()` / `gc.collect(actor_ref)` | Explicit GC trigger. ORCA deferred frees are pumped automatically every 256 scheduler ticks and on run-queue drain. |
| `erlang:system_monitor/2` | **ADAPT — NOT IMPLEMENTED** | `system.set_monitor(callback)` | Long GC, large heap notifications. (`Runtime::gc_stats()` / `scheduler_stats()` expose counters in Rust.) |

---

## 8. Code Loading and Hot Reloading

**Status: ADAPT for module reloading — NOT IMPLEMENTED.** Nulang compiles to its own register bytecode (interpreted + Cranelift JIT), not to WASM, so the WASM-shaped examples below are design sketches. Persistent actors do survive *runtime* restarts via snapshot/journal recovery (`Runtime::recover_actor` + `register_recovery_module`), which is the only "code reload" adjacency that exists today.

Hot code reloading is one of Erlang's killer features. Nulang should support it at the module level:

```nulang
-- Load new version of actor module
perform code.load("MyActor", wasm_bytes)

-- Upgrade running actors (OTP gen_server code_change pattern)
behavior code_change(old_version: String, state: State): State {
  -- Migrate state from old version to new version
  migrate_state(state, from: old_version, to: CURRENT_VERSION)
}
```

| Code Loading Primitive | Nulang Status | Nulang Form |
|------------------------|---------------|-------------|
| `code:load_file/1` | **ADAPT** | `code.load(module_name, wasm_module)` |
| `code:purge/1` | **ADAPT** | `code.purge(module_name)` |
| `code:soft_purge/1` | **ADAPT** | `code.soft_purge(module_name)` |
| `code:delete/1` | **ADAPT** | `code.unload(module_name)` |
| `code:which/1` | **ADAPT** | `code.which(module_name)` |
| `code:get_path/0` | **ADAPT** | `code.load_path()` |
| `code:add_path/1` | **ADAPT** | `code.add_load_path(path)` |
| `erlang:check_old_code/1` | **ADAPT** | `code.has_old_version(module_name)` |
| `erlang:check_process_code/2` | **ADAPT** | `code.actor_running_old_version(actor_ref, module)` |
| `sys:suspend/1` | **ADAPT** | `system.suspend(actor_ref)` |
| `sys:resume/1` | **ADAPT** | `system.resume(actor_ref)` |
| `sys:replace_state/2` | **ADAPT** | `system.replace_state(actor_ref, new_state)` |
| `sys:get_status/1` | **ADAPT** | `system.status(actor_ref)` |
| `sys:get_state/1` | **ADAPT** | `system.state(actor_ref)` |
| `sys:change_code/4` | **ADAPT** | `system.upgrade(actor_ref, old_vsn, new_vsn, extra)` |

**Design Note:** Hot reloading is harder with WASM than with BEAM bytecode, but feasible with dynamic module linking. Nulang should support "rolling upgrade" patterns where new actor instances use the new code while old instances drain their mailboxes.

---

## 9. Binary and Bit Syntax

**Status: ADAPT for protocol parsing — NOT IMPLEMENTED** (no `binary`/`<< >>` syntax; `term_to_binary` equivalents do not exist). Nulang's own distribution wire protocol is hand-rolled big-endian serde in Rust (`Packet::to_bytes`/`from_bytes`, `src/runtime/network.rs`) — see §17 for the format.

Binary pattern matching is one of Erlang's most powerful features. Nulang should support it for wire protocol implementation:

```nulang
-- Binary construction
let packet = binary {
  version: 1 as u8,
  flags: 0x03 as u16_be,
  payload_length: String.length(data) as u32_be,
  payload: data as bytes
}

-- Binary pattern matching
match packet {
  | <<version: u8, flags: u16_be, length: u32_be, payload: bytes(length)>> => {
      parse_payload(version, flags, payload)
    }
  | <<0xFF, rest: bytes>> => handle_legacy(rest)
  | _ => Error("Invalid packet")
}
```

| Binary Primitive | Nulang Status | Nulang Form |
|------------------|---------------|-------------|
| `<<Data>>` | **ADAPT** | `binary { ... }` / `<< ... >>` |
| Integer segments | **ADAPT** | `value as u8`, `value as u16_be` |
| Binary segments | **ADAPT** | `data as bytes` |
| Bit-size segments | **ADAPT** | `value as bits(5)` |
| UTF-8 segments | **ADAPT** | `string as utf8` |
| `binary:match/2` | **OMIT** | Use pattern matching |
| `binary:part/3` | **ADAPT** | `Binary.slice(data, start, length)` |
| `binary:copy/1` | **ADAPT** | `Binary.copy(data)` |
| `binary:list_to_bin/1` | **ADAPT** | `Binary.from_list(list)` |
| `binary:bin_to_list/1` | **ADAPT** | `Binary.to_list(data)` |
| `term_to_binary/1` | **ADOPT** | `Binary.serialize(term)` |
| `binary_to_term/1` | **ADOPT** | `Binary.deserialize(data)` |

---

## 10. Tracing and Debugging

**Status: NOT IMPLEMENTED.** No tracing infrastructure exists beyond debug opcodes (`DbgBreak` 0xF0, `DbgPrint` 0xF1, `DbgStack` 0xF2, `MetaType` 0xF3, `MetaCap` 0xF4) and Rust-side counters (`SchedulerStats`, `GcStats`). The `trace.*`/`debug.*` APIs below are the design target.

### 10.1 Tracing

| BEAM Primitive | Nulang Status | Nulang Form |
|----------------|---------------|-------------|
| `erlang:trace/3` | **ADAPT** | `trace.enable(actor_ref, flags)` |
| `erlang:trace_pattern/2` | **ADAPT** | `trace.set_pattern(module, behavior, match_spec)` |
| `dbg:tracer/0` | **ADAPT** | `trace.start_tracer()` |
| `dbg:p/2` | **ADAPT** | `trace.set_actor(actor_ref, flags)` |
| `dbg:tp/2` | **ADAPT** | `trace.set_breakpoint(module, behavior)` |
| `dbg:ctp/2` | **ADAPT** | `trace.clear_breakpoint(module, behavior)` |
| `dbg:stop/0` | **ADAPT** | `trace.stop()` |

### 10.2 Debugging

| BEAM Primitive | Nulang Status | Nulang Form |
|----------------|---------------|-------------|
| `sys:suspend/1` | **ADAPT** | `debug.suspend(actor_ref)` |
| `sys:resume/1` | **ADAPT** | `debug.resume(actor_ref)` |
| `sys:get_state/1` | **ADAPT** | `debug.get_state(actor_ref)` |
| `sys:get_status/1` | **ADAPT** | `debug.get_status(actor_ref)` |
| `sys:replace_state/2` | **ADAPT** | `debug.replace_state(actor_ref, fn)` |
| `sys:statistics/2` | **ADAPT** | `debug.statistics(actor_ref, flags)` |
| `sys:log/2` | **ADAPT** | `debug.log(actor_ref, options)` |

---

## 11. Process Groups

### 11.1 pg (Process Groups, OTP 23+)

**Status: IMPLEMENTED (runtime API)** — `ProcessGroups` (`src/runtime/process_groups.rs`) is a single-node, `RwLock<HashMap<String, HashSet<u64>>>` implementation. The `actor.groups.*` syntax below does not parse; membership is managed from Rust, and actors are auto-removed from all groups on exit (`leave_all` in `handle_actor_exit`).

Process groups provide decentralized, conflict-free process group membership:

```nulang
-- Join a process group
actor.groups.join("http_workers", self)

-- Get all members of a group
let workers = actor.groups.members("http_workers")

-- Send to all members in a group
actor.groups.broadcast("http_workers", ReloadConfig)

-- Leave a group
actor.groups.leave("http_workers", self)
```

| pg Primitive | Nulang Status |
|-------------|---------------|
| `pg:join/2,3` | **IMPLEMENTED (runtime API)** — `ProcessGroups::join(group, id)` (idempotent, validated names) |
| `pg:leave/2,3` | **IMPLEMENTED (runtime API)** — `leave(group, id)`; empty groups are pruned |
| `pg:get_members/1,2` | **IMPLEMENTED (runtime API)** — `members(group)` |
| `pg:get_local_members/1,2` | **IMPLEMENTED (runtime API)** — all members are local (single-node) |
| `pg:which_groups/0,1` | **IMPLEMENTED (runtime API)** — `which_groups()` |
| broadcast to group | **NOT IMPLEMENTED** — no `broadcast`/`actor.groups.broadcast`; senders must iterate `members()` |

### 11.2 pg2 (Legacy)

**Status: OMIT** — Replaced by `pg` in modern Erlang. Nulang should only implement `pg`.

---

## 12. Application Behavior

**Status: ADAPT as `application` lifecycle — NOT IMPLEMENTED** (no `application` block syntax)

OTP applications provide structured lifecycle management. Nulang should support application trees:

```nulang
application MyService {
  version = "1.0.0"

  on_start {
    let store = spawn_link KeyValueStore
    let api = spawn_link ApiServer(store)
    let metrics = spawn_link MetricsCollector
    Ok({ store, api, metrics })
  }

  on_stop(state) {
    actor.stop(state.api)
    actor.stop(state.store)
    actor.stop(state.metrics)
    Ok(())
  }
}
```

| Application Primitive | Nulang Status | Nulang Form |
|----------------------|---------------|-------------|
| `application:start/1` | **ADAPT** | `application.start(MyService)` |
| `application:stop/1` | **ADAPT** | `application.stop(MyService)` |
| `application:loaded_applications/0` | **ADAPT** | `application.list_loaded()` |
| `application:which_applications/0` | **ADAPT** | `application.list_running()` |
| `application:get_env/2` | **ADAPT** | `application.get_env(MyService, key)` |
| `application:set_env/3` | **ADAPT** | `application.set_env(MyService, key, value)` |
| Application callback module | **ADAPT** | `application` block with `on_start`, `on_stop` |

---

## 13. External Interfaces

### 13.1 Ports

**Status: ADAPT as `external.process` — NOT IMPLEMENTED.** Nulang's actual external interfaces today are the PyO3 Python bridge (`src/python/`, `perform Python.call(...)`) and the C FFI (`src/ffi/`, `FFICall` opcode 0xB0) — neither is a BEAM-style port with message-passing to an OS process.

Ports let BEAM communicate with external OS processes. Nulang should support this for integrating with external code:

```nulang
let port = external.process.spawn("/usr/bin/python3", ["script.py"])
port <- { command: "classify", data: image_bytes }
receive {
  | PortData(result) => result
  | PortClosed => Error("Python process crashed")
} after Duration.seconds(30) {
  Error("Classification timeout")
}
```

### 13.2 NIFs (Native Implemented Functions)

**Status: REPLACE — Nulang already has the C FFI** (`src/ffi/`: native library registry, `Value`↔C marshaling, stable C embedder API) instead of a WASM-module story. `external.wasm` does not exist.

NIFs let Erlang call C functions. Nulang's equivalent is its FFI layer; a WASM-module variant remains a design option:

```nulang
-- Load a native WASM module
let crypto_lib = external.wasm.load("crypto.wasm")
let hash = crypto_lib.call("sha256", data)
```

---

## 14. Summary Table

| Category | Adopt | Adapt | Replace | Omit |
|----------|-------|-------|---------|------|
| **Actor Lifecycle** | spawn ✅, self ✅, exit ✅(API), trap_exit ✅(API) | spawn_opt, process_info, priority | — | pid_to_list, list_to_pid, spawn_link ❌, spawn_monitor ❌ |
| **Message Passing** | send ✅, ask ✅, receive ⚠️ (syntax only) | — | — | — |
| **Naming** | register, unregister, whereis, registered ✅(API) | — | — | — |
| **Links/Monitors** | link, unlink, monitor, demonitor ✅(API) | demonitor flush ❌ | — | — |
| **OTP Behaviors** | supervisor strategies ✅ (3 of 4), gen_server call/cast ✅ | gen_statem ❌, gen_event ❌, simple_one_for_one ❌ | — | — |
| **Storage** | persistent_term ❌ | ETS (actor-local tables) ❌ | Mnesia ✅ (persistent actors + CRDTs implemented) | match_spec |
| **Distribution** | node(), nodes() ✅(API), monitor_node ❌ | RPC calls ⚠️ (RAsk partial; send stub) | global registry ❌ (no virtual actors) | set_cookie |
| **Timers** | send_after, start_timer, cancel, remaining, exit_after, kill_after ✅(API) | — | sleep (workflow-only `Timer.sleep`) | apply_after |
| **Hot Reloading** | — | code loading, sys operations ❌ | — | — |
| **Binary Syntax** | term_to_binary, binary_to_term ❌ | binary construction/matching ❌ | — | — |
| **Tracing** | — | trace, dbg ❌ | — | — |
| **Process Groups** | pg join/leave/members ✅(API) | group broadcast ❌ | pg2 | — |
| **Applications** | — | application lifecycle ❌ | — | — |
| **External** | — | ports ❌, WASM modules ❌ | NIFs → C FFI ✅ | — |

**Legend:** ✅ implemented (API = Rust runtime API only, no Nulang-language builtin) · ⚠️ partial/stub · ❌ not implemented. Design tallies ("35+ adopted, 20+ adapted, 5 replaced, 10 omitted") were aspirational; the verified counts are in §17.

---

## 15. Priority Implementation Order

### Phase 1: Core Actor Model (Foundation)
1. `receive` / `receive after` — **The single most important missing primitive** — *syntax only; needs MIR lowering, pattern dispatch, and `after`*
2. `spawn_link` / `spawn_monitor` — *not started (no syntax, no opcodes)*
3. `link` / `unlink` / `monitor` / `demonitor` — *runtime API done; needs VM opcode handling + language builtins*
4. `exit` signals and `trap_exit` — *runtime done; needs language surface*
5. `process_flag` (priority, trap_exit) — *not started (no actor priority)*
6. `register` / `unregister` / `whereis` — *runtime API done; needs language surface*

### Phase 2: OTP Integration
7. `GenServer` behavior mixin — *not started*
8. `GenStateM` behavior mixin — *not started*
9. `EventBus` behavior mixin — *not started*
10. Supervisor dynamic child management — *runtime API done (`add_child`); `simple_one_for_one` missing*

### Phase 3: Operations
11. `timer.send_after` / `start_timer` / `cancel_timer` — *runtime API done (`TimerWheel`); needs language surface*
12. ETS (actor-local tables) — *not started*
13. `persistent_term` — *not started*
14. Process groups (`pg`) — *runtime API done; group broadcast missing*

### Phase 4: Distribution
15. `cluster.call` / `multicast` / `broadcast` — *not started; remote-send behavior-name resolution is done (§17.5 item 1), remote `SpawnRequest` delivery is done as an MVP (§17.5 item 2), gossip has a wire packet (`Packet::Gossip`)*
16. `cluster.monitor_node` — *not started*

### Phase 5: Advanced
17. Binary/bit syntax for protocol parsing — *not started*
18. Hot code reloading — *not started (and not WASM-based; Nulang targets native bytecode)*
19. Application lifecycle management — *not started*
20. Tracing infrastructure — *not started*
21. Port/external process interfaces — *not started; C FFI + Python bridge already cover part of the integration story*

---

## 16. Design Principles for BEAM Primitives in Nulang

1. **Type safety first.** *(Target.)* As implemented, `spawn`/`send`/`ask` do not return `Result` — spawn is infallible and actor identity is a bare `u64` carried as a NaN-boxed `ActorRef`. Remote sends to unresolvable targets are silently dropped (`runtime/distributed.rs:677`). The `Result[T, Error]` surface remains the design goal.

2. **Capability-gated.** *(Target.)* No capability checks gate actor operations today; capability opcodes (`CapChk`/`CapUp`/`CapDown`/`CapSend`) are MVP no-ops in the VM.

3. **Effect-tracked.** Implemented with dedicated effects: `spawn` adds `Spawn`, `send` adds `Send`, `receive` adds `Receive`, `ask` adds `Send + Receive` (`src/effect_checker.rs`) — not `[IO]` as earlier drafts stated.

4. **Virtual actor compatible.** *(Target.)* All primitives are planned to work with local and virtual actors; only `ActorAddress::Local`/`Remote` routing exists today.

5. **Mailbox-first.** *(Target.)* Behaviors currently run as message handlers dispatched by `Runtime::step_actor`; `receive` is not yet a real consumption mechanism (§2.2).

6. **No `apply/3`.** Dynamic function application is intentionally omitted. Nulang's typed system uses behavior dispatch instead. (Holds today.)

7. **Structured errors.** *(Partial.)* `RegisterError` and `PgError` are typed; VM/runtime failures surface as `NuError::VMError`/`RuntimeError` strings rather than typed `badarg`/`badmatch`/`noproc` variants. The `noproc` case exists only as the DOWN reason `Error("noproc")` when monitoring a dead actor.

---

## 17. Implementation Status (Ground Truth)

Verified by reading `src/runtime/`, `src/bytecode.rs`, and `src/vm.rs` (post-v0.9 tree). "Runtime API" means implemented in Rust and covered by tests but **not** reachable from Nulang source code. File references point at the defining code.

### 17.1 What actually exists

| Area | Implementation | Where |
|------|----------------|-------|
| Actor lifecycle | `Runtime::spawn_actor` / `spawn_persistent_actor` / `spawn_workflow_actor`; ids from a global `AtomicU64` (`fresh_actor_id`); state machine `Created → Running → Waiting → Suspended → Terminated` | `runtime/mod.rs:175`, `runtime/actor.rs:11` |
| Language surface | `spawn Actor { field = v }`, `send a b(args)`, `ask a b(args)`, `self`, `receive { \| B(p) => e }` (syntax only), `emit`, `migrate a to node` | `parser.rs:1363`, `lexer.rs:699` |
| VM actor opcodes (handled) | `Spawn` 0x80, `Send` 0x81, `Ask` 0x82, `SelfOp` 0x83, `Receive` 0x84 (unreachable — §17.5 item 4), `StateGet` 0x8B, `StateSet` 0x8C, `Emit` 0x8D, `SignalWait` 0x8E | `bytecode.rs:108`, `vm.rs:1297` |
| VM actor opcodes (defined, **unhandled** — fall to "unimplemented opcode") | `Monitor` 0x85, `Demon` 0x86, `Link` 0x87, `Unlink` 0x88, `Exit` 0x89, `Yield` 0x8A | `vm.rs:2222` |
| Mailbox | Unbounded lock-free MPSC via `crossbeam::queue::SegQueue`; push never fails, never drops; epoch-based reclamation; `Message { behavior_id: u16, payload: Vec<Value>, sender: u64, priority }` with `MessagePriority::{System=0, Normal=1, Bulk=2}` (stored, not scheduling-affecting) | `runtime/mailbox.rs` |
| Scheduler | Work-stealing: Chase-Lev `Worker` deque per worker (LIFO local, FIFO steal) + global `Injector`; `Runtime::new` configures **4 workers**; idle backoff (3 empty polls → 100 µs sleep); profiling counters (`SchedulerStats`) | `runtime/scheduler.rs` |
| Preemption | Reduction counting: +1 per message processed; yield at `max_reductions = 1000`; actor re-enqueued only while mailbox non-empty | `runtime/actor.rs:110`, `runtime/mod.rs:1644` |
| GC | Per-actor ORCA: 64 KiB bump-allocator heaps (5 size classes, free lists), `local_count`/`foreign_count` per object; cross-actor sends bump `foreign_count` via `OrcaCoordinator`; deferred frees pumped every **256 scheduler ticks** and on run-queue drain | `runtime/heap.rs`, `runtime/gc.rs`, `runtime/mod.rs:1320` |
| Cycle detection | Incremental `CycleDetector`: per-actor pinned sentinel node, foreign-ref edge graph with ref counts, full scan every **10 epochs**, suspect marking, DFS, trial decrements, reclamation | `runtime/orca_cycle.rs` |
| Links/monitors/exit | `link_actors`/`unlink_actors`/`monitor`/`demonitor`/`exit_actor`/`kill_actor`; abnormal exit cascades to non-trapping links; trapping actors get a `System` message `[dead_id, linked_id]`; monitors get DOWN `[target_id, watcher_id, reason_code]` (codes: Normal 0, Error 1, Kill 2, Killed 3, Shutdown 4, Custom 5), all with `behavior_id = 0`; monitoring a dead actor → immediate DOWN `Error("noproc")` | `runtime/mod.rs:2461` |
| Supervision | 3 strategies (`OneForOne`, `OneForAll`, `RestForOne`), 3 policies (`Permanent`, `Temporary`, `Transient`), per-spec rate limits (default 5 restarts / 60 s), escalation with cascading supervisor shutdown | `runtime/supervisor.rs` |
| Registry | `ActorRegistry`: register/unregister/whereis/registered + name validation + auto-cleanup on exit | `runtime/registry.rs` |
| Process groups | `ProcessGroups`: join/leave/leave_all/members/is_member/member_count/which_groups; empty-group pruning; auto-leave on exit | `runtime/process_groups.rs` |
| Timers | `TimerWheel` (min-heap, lazy cancel): `send_after`, `send_after_with_context`, `exit_after`, `kill_after`, `cancel`, `remaining`, `tick`; driven every scheduler iteration; durable workflow timers via `perform Timer.sleep(name, ms)` (journaled, re-armed on recovery) | `runtime/timer.rs`, `runtime/mod.rs:1925` |
| Persistence | `PersistenceStore` trait + `MemoryStore`, `JsonFileStore`, `SqliteStore` (rusqlite); per-field `StateModel` (`Local`/`Durable`/`EventSourced`/`Crdt`); journal (`JournalEntry`) + snapshot (`ActorSnapshot`, incl. `waiting_signal`); 8-variant `WorkflowEvent` journal; `recover_actor` replays journal + restores bytecode via `register_recovery_module`; pointers/strings normalize to `Nil` across restarts | `runtime/persistence.rs`, `runtime/mod.rs:1974` |
| Event sourcing | `emit` opcode → `Runtime::emit_event` appends to `Actor.event_log`; saga compensation for failed workflow steps; workflow signals (`SignalWait` suspend/resume) | `runtime/mod.rs:785`, `vm.rs:1371` |

### 17.2 Distribution wire protocol (implemented, previously undocumented here)

Custom TCP protocol in `src/runtime/network.rs`. Every frame:

```text
[0..4]   message length (u32, big-endian, includes this header)
[4..8]   magic: "NUL0"
[8]      packet type discriminant
[9..17]  sequence number (u64, big-endian)
[17..]   type-specific payload
```

An **8-byte node-id handshake** (big-endian `u64`) is exchanged immediately after TCP connect, before any framed packets; a mismatch aborts the connection. Limits: `MAX_PACKET_LEN` 16 MiB, per-connection I/O timeout 30 s, internal channel capacity 1024.

Packet types: `ActorMessage` = 0, `Heartbeat` = 1, `Ack` = 2 (serde-complete but unused in delivery paths), `SpawnRequest` = 3, `SpawnResponse` = 4, `CrdtSync` = 5, `Gossip` = 6. All serde is hand-rolled big-endian. `Value` payloads serialize under five tags — int / float / bool / string-id (u32) / unit; anything else (nil, actor refs, pointers) is written as raw-bit float and does **not** round-trip on read (see §17.5 item 12).

Cluster membership (`src/runtime/cluster.rs`) is gossip/SWIM-style: heartbeat every **500 ms**, heartbeat timeout **2 s**, suspicion **5 s**, failed-node retention **60 s**, gossip fanout **2**. `NodeStatus`: `Joining`, `Healthy`, `Suspicious`, `Failed`, `Leaving`. `ClusterState::tick` returns `ClusterAction::{SendHeartbeat, NodeJoined, NodeFailed, NodeLeft, SendGossip}` which `Runtime::process_network` executes. `SendGossip` is wired: `Packet::Gossip` carries the sender's compact membership view (`Vec<NodeGossip>` — node id, address, status, incarnation per member; address = family byte + octets + port), merged on receipt by `ClusterState::merge_membership` (higher incarnation wins; equal incarnation refreshes `last_heartbeat` as a liveness hint, which keeps relay-only nodes from being suspected). Transitive propagation works — a chain of pairwise seeds (B joins A, C joins B) converges without a full mesh; see `test_three_node_gossip_converges_chain_seeded`.

Location transparency (`src/runtime/distributed.rs`): `ActorAddress::{Local, Remote}`, `AddressResolver` (checks cluster health before resolving), and an LRU `RemoteActorCache` capped at **10,000** entries. `NodeId::LOCAL = 0`. `Migrate` opcode (0xD1) records `(actor, node)` in `VM::pending_migrations` and forwards to the distributed callback; actual cross-node state transfer is not implemented.

### 17.3 CRDT inventory (implemented, previously undocumented here)

8 types behind the `Crdt` trait, owned by `CrdtManager` (created with `create_*` constructors, synced via `CrdtSync` packets to all healthy members):

| CRDT | File |
|------|------|
| `GCounter`, `PNCounter`, `GSet`, `ORSet`, `AWORSet` (+ `LamportTime`/`LamportClock` helpers) | `runtime/crdt.rs` |
| `LWWRegister`, `MVRegister`, `RGA` | `runtime/crdt_reg.rs` |

`CrdtOp` wire format: `crdt_id` u64 BE · `crdt_type` u8 · `payload_len` u32 BE · payload. Entries created from remote payloads have their local node identity rewritten so new operations tag the local node.

### 17.4 Verified constants

| Constant | Value | Where |
|----------|-------|-------|
| `max_reductions` (preemption) | 1000 | `runtime/actor.rs:110` |
| Scheduler workers (`Runtime::new`) | 4 | `runtime/mod.rs:145` |
| GC deferred-free pump interval | 256 scheduler ticks | `runtime/mod.rs:1320` |
| Cycle-detection full-scan interval | 10 epochs | `runtime/orca_cycle.rs:348` |
| Initial actor heap | 64 KiB | `runtime/actor.rs:88` |
| Mailbox capacity | unbounded (`SegQueue`; constructor arg ignored) | `runtime/mailbox.rs:49` |
| Remote actor cache | 10,000 entries (LRU) | `runtime/distributed.rs:56` |
| Supervisor restart defaults | 5 restarts / 60 s window | `runtime/supervisor.rs:82` |
| Heartbeat interval / timeout / suspicion / retention | 500 ms / 2 s / 5 s / 60 s | `runtime/cluster.rs:38` |
| Gossip fanout | 2 | `runtime/cluster.rs:50` |
| `remote_ask` timeout | 5000 ms | `vm.rs:2015` |

### 17.5 Stubs and known gaps (flag for fixing)

1. ~~**Remote send drops the behavior name.**~~ **FIXED.** `Packet::ActorMessage` now carries `behavior_name: String` on the wire (length-prefixed UTF-8, replacing the `behavior_id: u16` field); `process_network_packets` resolves it via `Runtime::behavior_id_for` against the target actor's behavior table, falling back to behavior 0 for unknown names — mirroring local `send_message`'s `unwrap_or(0)` (`runtime/distributed.rs`, `runtime/network.rs`).
2. ~~**Remote spawn is send-only.**~~ **FIXED (MVP).** `process_network_packets` now handles `Packet::SpawnRequest`: the receiving node spawns a fresh actor with the request's `initial_state` and registers the requested behavior — but only if that behavior was explicitly offered via `Runtime::register_spawnable_behavior` (MVP scope: remote spawn supports native behaviors the receiver opted into, not arbitrary or bytecode behaviors). Unknown names are answered with `SpawnResponse{success:false}` and no actor is created — the no-crash counterpart of the local unknown-behavior fallback. The reply carries the real actor id; the requester collects it via `Runtime::take_spawn_response(request_id)` (recorded by the `SpawnResponse` arm of `process_network_packets`; the address returned by `spawn_on_node` is still a placeholder whose `actor_id` is the request id). `RSpawn` (0xD4) still returns `actor_ref(0)` (`vm.rs:1392`); `DistributedRuntimeImpl::spawn_on_node` still returns placeholder addresses.
3. **`RSend` (0xD2) is a no-op** in the VM (`vm.rs:1389`).
4. **`receive` has no semantics.** MIR lowering discards arms and yields `nil` (`mir_lower.rs:959`); the VM's `Receive` handler (`vm.rs:2213`, pops next message, returns first payload or `nil`) is never emitted by the compiler. No `after` timeout in the grammar.
5. **Fault-tolerance opcodes unhandled.** `Monitor`/`Demon`/`Link`/`Unlink`/`Exit`/`Yield` hit the VM's "unimplemented opcode" catch-all (`vm.rs:2222`); the functionality exists only as Rust runtime API.
6. **`trap_exits` is Rust-only** (public `Actor` field, no setter/builtin); same for registry, process groups, and `TimerWheel`.
7. **No actor scheduling priority.** `MessagePriority` is stored on messages but never consulted by the scheduler or mailbox (FIFO segmented queue).
8. **Unresolvable remote sends are silently dropped** (`ResolveResult::Unresolvable` → ignored, `runtime/distributed.rs:677`).
9. **`Ack` packets** serialize/deserialize and are tested, but nothing sends or consumes them.
10. **Supervisor restarts recreate bare actors**: `Supervisor::restart_child` builds a fresh `Actor` with no behavior table or bytecode; restarted children cannot process messages until behavior restoration is wired up.
11. **`Kill` is trappable in practice.** `handle_actor_exit` special-cases nothing for `ExitReason::Kill` — linked actors with `trap_exits` receive it as a message instead of dying, contradicting the "cannot be trapped" doc comment (`runtime/mod.rs:2533`). `ExitReason::Killed` is never constructed; link cascades use `Error("linked actor ... exited with ...")` instead.
12. **Wire `Value` serde lossy.** Only int/float/bool/string-id/unit round-trip; nil, actor refs, and pointers serialize as raw-bit `VAL_FLOAT` and read back as floats (`runtime/network.rs:401`, `:426`).
13. No: `spawn_link`/`spawn_monitor`, `is_alive`/`process_info`/`processes` builtins, actor hibernation, explicit GC triggers, group broadcast, `monitor_node`, cluster RPC family (`call`/`multicall`/`cast`/`broadcast`), ETS tables, `persistent_term`, `simple_one_for_one`, virtual actors, application lifecycle, tracing, ports, binary/bit syntax, hot code loading.

### 17.6 Implemented but previously undocumented in this file

- The full distribution wire protocol, handshake, packet inventory, and cluster timing constants (§17.2).
- The 8-type CRDT inventory and `CrdtOp` sync format (§17.3).
- The three persistence backends, the journal/snapshot model, and the 8-variant workflow event journal with recovery replay (§17.1).
- The DOWN-message and trap-exit-message wire shapes (`behavior_id = 0`, reason codes) (§17.1).
- ORCA per-actor GC with deferred frees + epoch-driven cycle detection, and the 256-tick GC pump (§17.1, §17.4).
- Scheduler profiling (`SchedulerStats`) and GC counters (`GcStats`) — the closest thing to `system_monitor` today.
- Debug opcodes `DbgBreak`/`DbgPrint`/`DbgStack`/`MetaType`/`MetaCap` (0xF0–0xF4).
- The v0.9 AI runtime (`src/ai/`: LLM providers, semantic/procedural memory, pipelines, debates, supervisor teams) is wired into `Runtime` (`pipeline_*`, `debate_*`, `supervisor_*`, `complete_llm`) with dedicated opcodes (`LlmAsk` 0x9C, `PipelineNew/Stage/Run` 0x9D–0x9F, `SupervisorNew/Worker/Run` 0xC0–0xC2, `DebateNew/Participant/Run` 0xC3–0xC5) — out of BEAM scope but resident in the same runtime.
