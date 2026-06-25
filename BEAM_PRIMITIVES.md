# BEAM/OTP Primitives for Nulang: Adoption Analysis

## Overview

The BEAM (Bogdan/Bjorn's Erlang Abstract Machine) and OTP (Open Telecom Platform) define the gold standard for fault-tolerant distributed systems primitives, refined over 35 years of production use at Ericsson, WhatsApp, and thousands of other systems. This document maps the full BEAM/OTP primitive surface to Nulang's architecture, categorizing each primitive as **Adopt**, **Adapt**, **Replace**, or **Omit** based on Nulang's existing design.

---

## 1. Core Actor Lifecycle Primitives

### 1.1 Process/Actor Creation

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `spawn(Fun)` | **ADOPT** | `spawn ActorType` | Nulang already has `spawn`. Add `spawn` with initial arguments. |
| `spawn(Module, Fun, Args)` | **ADAPT** | `spawn ActorType(args)` | Nulang uses typed actor constructors rather than dynamic module references. |
| `spawn_link(Fun)` | **ADOPT** | `spawn_link ActorType` | Essential for supervision trees. Bidirectional fault propagation. |
| `spawn_monitor(Fun)` | **ADOPT** | `spawn_monitor ActorType` | Returns `{ActorRef, MonitorRef}`. Unilateral observation with down notifications. |
| `spawn_opt(Fun, Options)` | **ADAPT** | `spawn ActorType with options { ... }` | Nulang should support: `priority`, `scheduler_hint`, `max_heap_size`, `link`, `monitor`. |

**Design Note:** Nulang's typed actors eliminate the need for `apply/3` and dynamic function calls. The `spawn` family should return `Result[ActorRef, SpawnError]` rather than raw PIDs, enabling type-safe actor referencing.

### 1.2 Process/Actor Identity

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `self()` | **ADOPT** | `self` or `actor.id()` | Returns the current actor's `ActorId`. |
| `pid_to_list(Pid)` | **OMIT** | — | Not needed; Nulang `ActorId` has `to_string()`. |
| `list_to_pid(String)` | **OMIT** | — | Unsafe in typed system. |
| `is_process_alive(Pid)` | **ADOPT** | `actor.is_alive(actor_ref)` | Essential for liveness checks. |
| `process_info(Pid)` | **ADAPT** | `actor.info(actor_ref)` | Returns typed `ActorInfo` record: mailbox size, memory, reductions, links, monitors, current behavior. |
| `processes()` | **ADAPT** | `actor.list()` | Returns list of all actor refs on the node. |
| `register(Name, Pid)` | **ADOPT** | `actor.register(name, actor_ref)` | Local name registry. Returns `Result[Unit, RegisterError]`. |
| `unregister(Name)` | **ADOPT** | `actor.unregister(name)` | Remove from local registry. |
| `whereis(Name)` | **ADOPT** | `actor.whereis(name)` | Returns `Option[ActorRef]`. |
| `registered()` | **ADOPT** | `actor.registered()` | Returns list of registered names. |

**Design Note:** OTP's global registry (`global:register_name/2`) is subsumed by Nulang's virtual actor system ( Orleans-style identity-based addressing). Local registration (`register/2`) remains useful for well-known services on a node.

### 1.3 Termination and Signals

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `exit(Pid, Reason)` | **ADAPT** | `actor.exit(actor_ref, reason)` | Sends exit signal. Type: `exit : (ActorRef, ExitReason) -> [IO] Unit`. |
| `exit(Reason)` | **ADOPT** | `exit(reason)` | Exit the current actor. |
| `kill` (reason) | **ADOPT** | `ExitReason.Kill` | Untrappable kill signal. |
| `normal` (reason) | **ADOPT** | `ExitReason.Normal` | Normal termination, no supervisor notification. |
| `process_flag(trap_exit, true)` | **ADOPT** | `actor.trap_exit(true)` | Convert exit signals to messages. Essential for supervisors and generic servers. |
| `process_flag(priority, Level)` | **ADOPT** | `actor.set_priority(Level)` | `high`, `normal`, `low`. Bound to scheduler. |

**Design Note:** Nulang should model `ExitReason` as a variant type:

```nulang
type ExitReason =
  | Normal
  | Kill
  | Shutdown
  | Shutdown(Timeout)
  | Error(String)
  | Custom(val Tag)
```

---

## 2. Message Passing Primitives

### 2.1 Send Operations

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `Pid ! Message` | **ADOPTED** | `actor_ref <- message` | Nulang already uses `<-`. Non-blocking, asynchronous. |
| `Name ! Message` | **ADOPT** | `registered_name <- message` | Send to registered name. |
| `{Name, Node} ! Message` | **ADAPT** | `actor_ref <- message` (distributed) | Nulang's distributed runtime handles routing transparently. No need for explicit node tuple. |

### 2.2 Receive (Critical Addition)

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `receive ... end` | **ADOPT** | `receive { ... }` | **This is the single most important BEAM primitive Nulang currently lacks.** Pattern-matching mailbox receive with optional `after` timeout. |

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

### 2.3 Selective Receive Considerations

OTP's selective receive is both powerful and problematic — it can cause mailbox bloat when messages don't match any pattern. Nulang should:

1. **Support selective receive** (required for Erlang compatibility patterns)
2. **Provide mailbox inspection** (`actor.mailbox_size(self)`) for monitoring
3. **Warn at compile time** if a behavior has a `receive` with no catch-all pattern (potential mailbox leak)
4. **Support `receive` with `flush`** to clear non-matching messages after timeout

---

## 3. Linking and Monitoring

### 3.1 Links (Bidirectional)

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `link(Pid)` | **ADOPT** | `actor.link(other)` | Bidirectional fault propagation. If either dies, the other receives an exit signal. |
| `unlink(Pid)` | **ADOPT** | `actor.unlink(other)` | Remove link. |

### 3.2 Monitors (Unidirectional)

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `erlang:monitor(process, Pid)` | **ADOPT** | `actor.monitor(other)` | Returns `MonitorRef`. Receive `{'DOWN', Ref, process, Pid, Reason}` on death. |
| `erlang:demonitor(Ref)` | **ADOPT** | `actor.demonitor(monitor_ref)` | Remove monitor. |
| `erlang:demonitor(Ref, [flush])` | **ADOPT** | `actor.demonitor(monitor_ref, flush: true)` | Remove and flush pending DOWN message. |

**Design Note:** Nulang should model monitors as a typed effect:

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
| `gen_server:start_link/4` | **ADOPTED** | `spawn_link KeyValueStore` |
| `gen_server:call/2` | **ADOPTED** | `ask(store, get("key"))` |
| `gen_server:cast/2` | **ADOPTED** | `store <- put("key", "value")` |
| `gen_server:reply/2` | **OMIT** | Built into `ask`/behavior return |
| `gen_server:stop/1` | **ADOPT** | `actor.stop(store)` |
| `gen_server:abcast/2` | **ADAPT** | Distributed broadcast via `cluster.broadcast/2` |

### 4.2 gen_statem

**Status: ADAPT as `state_machine` behavior**

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
| `gen_statem:call/2` | **ADOPTED** | `ask(fsm, event)` |
| `gen_statem:cast/2` | **ADOPTED** | `fsm <- event` |
| Event actions | **ADAPT** | Declarative event handlers with state transitions |
| State enter/exit | **ADAPT** | `on_entry` / `on_exit` hooks |

### 4.3 gen_event

**Status: ADAPT as `event_bus` behavior**

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

**Status: ADOPTED** (Nulang already has supervision trees)

OTP supervisor primitives to ensure are complete:

| Supervisor Primitive | Nulang Status | Nulang Form |
|----------------------|---------------|-------------|
| `supervisor:start_link/2` | **ADOPTED** | `spawn_link Supervisor` with children spec |
| `supervisor:start_child/2` | **ADOPT** | `supervisor.add_child(supervisor, spec)` |
| `supervisor:terminate_child/2` | **ADOPT** | `supervisor.terminate_child(supervisor, child_id)` |
| `supervisor:restart_child/2` | **ADOPT** | `supervisor.restart_child(supervisor, child_id)` |
| `supervisor:delete_child/2` | **ADOPT** | `supervisor.remove_child(supervisor, child_id)` |
| `supervisor:which_children/1` | **ADOPT** | `supervisor.children(supervisor)` |
| Restart strategies | **ADOPTED** | `one_for_one`, `one_for_all`, `rest_for_one`, `simple_one_for_one` |

**Design Note:** Nulang should support dynamic supervision (adding children at runtime) and `simple_one_for_one` (template-based child creation), both critical for connection pools and worker pools.

---

## 5. In-Memory Storage

### 5.1 ETS (Erlang Term Storage)

**Status: ADAPT as `actor.local_table`**

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

**Status: ADOPT as `persistent_term`**

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

---

## 6. Distribution Primitives

### 6.1 Node Management

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `node()` | **ADOPT** | `cluster.this_node()` | Returns current `NodeId`. |
| `nodes()` | **ADOPT** | `cluster.nodes()` | Returns list of connected nodes. |
| `nodes(connected)` | **ADOPT** | `cluster.connected_nodes()` | Explicit connected filter. |
| `nodes(visible)` | **ADOPT** | `cluster.visible_nodes()` | Partitions visible but not directly connected. |
| `is_alive()` | **ADAPT** | `cluster.is_distributed()` | Whether distribution is enabled. |
| `net_kernel:connect_node/1` | **ADAPT** | `cluster.join(seed_nodes)` | Part of existing gossip-based cluster join. |
| `erlang:monitor_node/2` | **ADAPT** | `cluster.monitor_node(node_id)` | Receive `nodedown` / `nodeup` messages. |
| `erlang:set_cookie/2` | **OMIT** | — | Replaced by capability-based authentication. |

### 6.2 Remote Operations

| BEAM Primitive | Nulang Status | Nulang Form | Rationale |
|----------------|---------------|-------------|-----------|
| `{Name, Node} ! Message` | **ADAPTED** | `actor_ref <- message` | Nulang's distributed runtime handles transparent routing. |
| `rpc:call/4` | **ADAPT** | `cluster.call(node, Module, behavior, args)` | Type-safe RPC. |
| `rpc:multicall/4` | **ADAPT** | `cluster.multicall(nodes, behavior, args)` | Parallel RPC to multiple nodes. |
| `rpc:cast/4` | **ADAPT** | `cluster.cast(node, behavior, args)` | Fire-and-forget remote call. |
| `rpc:abcast/3` | **ADAPT** | `cluster.broadcast(behavior, args)` | Broadcast to all connected nodes. |
| `rpc:sbcast/3` | **ADAPT** | `cluster.broadcast_sync(behavior, args)` | Synchronous broadcast. |

### 6.3 Global Name Registration

**Status: REPLACE with virtual actors**

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
| `erlang:send_after/3` | **ADOPT** | `timer.send_after(delay, actor_ref, message)` | Critical for timeouts, retries, scheduled tasks. |
| `erlang:start_timer/3` | **ADOPT** | `timer.start(delay, actor_ref, message)` | Returns `TimerRef`. |
| `erlang:cancel_timer/1` | **ADOPT** | `timer.cancel(timer_ref)` | Returns `Option[RemainingTime]`. |
| `erlang:read_timer/1` | **ADOPT** | `timer.remaining(timer_ref)` | Returns `Option[Duration]`. |
| `timer:apply_after/4` | **OMIT** | — | Use `send_after` with behavior message. |
| `timer:exit_after/2` | **ADAPT** | `timer.exit_after(delay, reason)` | Kill actor after timeout. |
| `timer:kill_after/1` | **ADAPT** | `timer.kill_after(delay)` | Unconditional kill after timeout. |
| `timer:sleep/1` | **ADOPTED** | `time.sleep(duration)` | Nulang already has this via Time effect. |

### 7.2 Scheduling Hints

| BEAM Primitive | Nulang Status | Nulang Form |
|----------------|---------------|-------------|
| `erlang:yield/0` | **ADOPT** | `scheduler.yield()` | Yield to scheduler. |
| `erlang:hibernate/3` | **ADAPT** | `actor.hibernate()` | Minimize memory footprint until next message. |
| `erlang:garbage_collect/0,1` | **ADAPT** | `gc.collect()` / `gc.collect(actor_ref)` | Explicit GC trigger. |
| `erlang:system_monitor/2` | **ADAPT** | `system.set_monitor(callback)` | Long GC, large heap notifications. |

---

## 8. Code Loading and Hot Reloading

**Status: ADAPT for WASM module reloading**

Hot code reloading is one of Erlang's killer features. Nulang should support it at the WASM module level:

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

**Status: ADAPT for protocol parsing**

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

**Status: ADAPT as `actor.groups`**

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
| `pg:join/2,3` | **ADOPT** |
| `pg:leave/2,3` | **ADOPT** |
| `pg:get_members/1,2` | **ADOPT** |
| `pg:get_local_members/1,2` | **ADOPT** |
| `pg:which_groups/0,1` | **ADOPT** |

### 11.2 pg2 (Legacy)

**Status: OMIT** — Replaced by `pg` in modern Erlang. Nulang should only implement `pg`.

---

## 12. Application Behavior

**Status: ADAPT as `application` lifecycle**

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

**Status: ADAPT as `external.process`**

Ports let BEAM communicate with external OS processes. Nulang should support this for integrating with non-WASM code:

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

**Status: ADAPT as `external.wasm`**

NIFs let Erlang call C functions. Nulang's equivalent is WASM modules:

```nulang
-- Load a native WASM module
let crypto_lib = external.wasm.load("crypto.wasm")
let hash = crypto_lib.call("sha256", data)
```

---

## 14. Summary Table

| Category | Adopt | Adapt | Replace | Omit |
|----------|-------|-------|---------|------|
| **Actor Lifecycle** | spawn, spawn_link, spawn_monitor, self, exit, trap_exit, priority | spawn_opt, process_info | — | pid_to_list, list_to_pid |
| **Message Passing** | receive, after timeout | — | — | — |
| **Naming** | register, unregister, whereis, registered | — | — | — |
| **Links/Monitors** | link, unlink, monitor, demonitor | — | — | — |
| **OTP Behaviors** | supervisor strategies, gen_server patterns | gen_statem, gen_event | — | — |
| **Storage** | persistent_term | ETS (actor-local tables) | Mnesia | match_spec |
| **Distribution** | node(), nodes(), monitor_node | RPC calls | global registry | set_cookie |
| **Timers** | send_after, start_timer, cancel_timer, remaining | — | — | apply_after |
| **Hot Reloading** | — | code loading, sys operations | — | — |
| **Binary Syntax** | term_to_binary, binary_to_term | binary construction/matching | — | — |
| **Tracing** | — | trace, dbg | — | — |
| **Process Groups** | pg join/leave/members | — | pg2 | — |
| **Applications** | — | application lifecycle | — | — |
| **External** | — | ports, WASM modules | NIFs | — |

**Totals:** 35+ primitives adopted, 20+ adapted, 5 replaced, 10 omitted.

---

## 15. Priority Implementation Order

### Phase 1: Core Actor Model (Foundation)
1. `receive` / `receive after` — **The single most important missing primitive**
2. `spawn_link` / `spawn_monitor`
3. `link` / `unlink` / `monitor` / `demonitor`
4. `exit` signals and `trap_exit`
5. `process_flag` (priority, trap_exit)
6. `register` / `unregister` / `whereis`

### Phase 2: OTP Integration
7. `GenServer` behavior mixin
8. `GenStateM` behavior mixin
9. `EventBus` behavior mixin
10. Supervisor dynamic child management

### Phase 3: Operations
11. `timer.send_after` / `start_timer` / `cancel_timer`
12. ETS (actor-local tables)
13. `persistent_term`
14. Process groups (`pg`)

### Phase 4: Distribution
15. `cluster.call` / `multicast` / `broadcast`
16. `cluster.monitor_node`

### Phase 5: Advanced
17. Binary/bit syntax for protocol parsing
18. Hot code reloading at WASM level
19. Application lifecycle management
20. Tracing infrastructure
21. Port/external process interfaces

---

## 16. Design Principles for BEAM Primitives in Nulang

1. **Type safety first.** Every primitive returns `Result[T, Error]` where failure is possible. No raw PIDs — typed `ActorRef` everywhere.

2. **Capability-gated.** Actor lifecycle operations require `capability system`. Table operations require `capability table`. Timer operations require `capability time`.

3. **Effect-tracked.** `spawn` has effect `[IO]`. `receive` has no additional effect (it's actor-local). `exit` has effect `[IO]`.

4. **Virtual actor compatible.** All primitives work with both local and virtual (distributed) actors transparently.

5. **Mailbox-first.** `receive` is the primary message consumption mechanism. Behaviors compile to `receive` loops with automatic pattern matching.

6. **No `apply/3`.** Dynamic function application is intentionally omitted. Nulang's typed system uses behavior dispatch instead.

7. **Structured errors.** All BEAM-style "badarg", "badmatch", "noproc" errors become typed `Result` variants.
