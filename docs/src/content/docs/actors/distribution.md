---
title: Distribution & Clustering
description: Location-transparent actors across networked nodes with gossip membership and CRDT state replication.
---

## Location Transparency

Nulang actors are location-transparent: you `send` to an actor without knowing (or caring) which node it lives on. The runtime resolves actor addresses — local or remote — transparently.

```nulang
// Same code works for local AND remote actors
send some_actor inc()
```

## Actor Addresses

An `ActorAddress` is either:
- **Local** — a direct 64-bit actor id on the current node
- **Remote** — a `(node_id, actor_id)` pair on a different node

The `AddressResolver` maintains an LRU cache (10k entries) mapping remote addresses.

## Cluster Membership

Nodes discover each other via a **gossip protocol**:

1. **Seeds** — A node joins a cluster by connecting to one or more seed nodes
2. **Heartbeats** — Nodes periodically heartbeat all known members
3. **Gossip** — Membership state propagates transitively through connected peers

### Joining a Cluster

```nulang
// Start a node and join a cluster
// Each node needs a unique node_id
let node = spawn ClusterNode {
    state node_id: Int,
    state seeds: [String]  // e.g. ["192.168.1.10:9000"]
} in { ... }
```

### Member States

| State | Description |
|-------|-------------|
| `Joining` | Initial state, awaiting first gossip round |
| `Healthy` | Active member, heartbeating and receiving messages |
| `Suspect` | Missed heartbeats, awaiting confirmation |
| `Left` | Gracefully departed |
| `Removed` | Pruned from membership after timeout |

Membership state carries an **incarnation number** — higher incarnations win in merge conflicts, preventing split-brain regressions.

## Remote Spawn

Spawn an actor on a remote node:

```nulang
// Register a spawnable behavior on the remote node first
perform Actor.register("worker")

// Spawn on a remote node
let remote_actor = spawn_on_node("10.0.0.5:9000") Worker {
    state task: String = "process"
} in {
    remote_actor
}
```

The remote node must have `Worker` registered via `Runtime::register_spawnable_behavior`.

## Wire Protocol

The distribution layer uses a custom TCP protocol:

- **Magic**: `NUL0` (4 bytes)
- **Handshake**: 8-byte node id exchange
- **Frames**: Length-prefixed, big-endian encoded
- **Packet types**: `ActorMessage`, `Heartbeat`, `Ack`, `SpawnRequest`, `SpawnResponse`, `CrdtSync`, `CrdtDeltaSync`, `Gossip`

String values travel **by content** — the sender populates a string table and the receiver interns strings into its module pool. Heap pointers, closures, actor refs, and nil are rejected at send time.

## CRDT Replication

Nulang supports 8 Conflict-free Replicated Data Types for eventually-consistent state:

| CRDT | Description |
|------|-------------|
| `GCounter` | Grow-only counter |
| `PNCounter` | Positive-negative counter (increment/decrement) |
| `GSet` | Grow-only set |
| `ORSet` | Observed-remove set |
| `AWORSet` | Add-wins observed-remove set |
| `LWWRegister` | Last-writer-wins register |
| `MVRegister` | Multi-value register |
| `RGA` | Replicated growable array |

CRDTs use **delta-state replication**: only changed state (deltas) is shipped over the wire, with periodic full syncs (every 16 rounds) as a repair mechanism.

### Using CRDTs

```nulang
actor SharedCounter {
    state counter = Crdt.new_gcounter("my_counter")

    behavior inc() {
        Crdt.increment(self.counter, 1)
    }

    behavior get(sender: Actor) {
        let value = Crdt.value(self.counter)
        send sender reply(value)
    }
}
```

## Persistence

Actors can be persisted for durability:

| Store | Description |
|-------|-------------|
| `MemoryStore` | In-memory (default, ephemeral) |
| `JsonFileStore` | JSON file on disk |
| `SqliteStore` | SQLite database (via rusqlite) |

Persistent actors support journaling and checkpointing for crash recovery.

```nulang
// Actor state model annotations
actor DurableWorker {
    state tasks: Durable [String] = []

    behavior add_task(task: String) {
        self.tasks = self.tasks + [task]
    }
}
```
