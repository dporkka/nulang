# RFC 0014: Durable-Actor Re-Spawn on Node Failure

- **Status:** Draft
- **Tier:** Stable (extends Stable-tier actor/supervision surface)
- **Author:** AI assistant
- **Created:** 2026-08-09
- **Language-version at effect:** 1.0.0-frozen
- **Supersedes:** none
- **Depends on:** RFC 0011 (split-brain resolver), RFC 0012 (cross-node
  supervision), RFC 0013 (authenticated transport, for the wire additions'
  TLS path only — no dependency on its version-negotiation mechanics)

## Summary

When a node is confirmed gone, durable actors that lived on it must be
re-spawned on a healthy node from their last durable snapshot — under an
explicit supervisor policy, and never in a way that produces two live
copies of the same durable-id actor. This RFC is PLAN.md Phase 5
deliverable 7 part (c). Parts (a)+(b) — cache invalidation and
`DOWN`-with-`noconnection` to local watchers — landed 2026-08-09
(`handle_node_failed`); this RFC supplies the missing recovery half and
the safety gate that (a)+(b) deliberately left open.

## Motivation

Today a durable actor on a dead node is silently orphaned:

- `handle_node_failed` (parts a+b) invalidates the dead node's
  `RemoteActorCache` entries and delivers `DOWN`-with-`noconnection` to
  local watchers, but nothing re-spawns the durable actor. A `persistent
  actor` / `entity` / `workflow` that held the only copy of durable state
  is gone with the node.
- `receive_migrated_actor` + `Packet::MigrateActor` (deliverable 14)
  already ship a full snapshot + NBC module cross-node and restore it —
  but migration is operator/supervisor-**explicit**. Node failure is
  reactive: the source node is dead, so the snapshot must come from a
  **replica**, and the decision to re-spawn must come from a **policy**.
- The failure detector's `Failed` status is not a confirmation of death:
  a partitioned-but-alive node re-joins via the probe path (RFC 0011 §3),
  and a self-downed node keeps running its local actors with a shut
  transport (RFC 0011 §3). Naive "respawn everything that was on a
  `Failed` node" produces exactly the two-live-copies hazard the PLAN
  warns about.

The safety gate is modeled on Kubernetes StatefulSet pod rescheduling:
the old pod must be **confirmed terminated** before the replacement
starts. This RFC defines what "confirmed" means for a node and how
re-spawn proceeds only after that confirmation.

## Design

### 1. Confirmed-gone determination: the `Removed` membership state

`Failed` (RFC 0011) means "unreachable for the suspicion window" — it
does **not** mean dead. A partition that heals re-promotes `Failed`
members via the probe path; a self-downed node cannot be reached but its
actors are still running locally. Neither case may trigger re-spawn.

Re-spawn triggers only on a new membership state, `Removed`, entered
exactly one of two ways:

1. **Positive goodbye** (safe, immediate): a node that is shutting down
   — graceful `Actor.exit`/process shutdown, or the split-brain resolver
   downing the local node — sends `Packet::NodeGoodbye` **before** it
   shuts its transport, carrying its durable-actor manifest:
   `Vec<(actor_id, epoch)>`. Receivers mark the sender `Removed`
   immediately: the goodbye is the sender's own confirmation that its
   durable actors have been checkpointed and terminated. This is the
   StatefulSet "old pod confirmed terminated" signal, made explicit.
   A `Removed` node never re-joins with the same identity; if it restarts
   it must re-join under a fresh incarnation (see §5).

2. **Failure-confirmation timeout** (delayed, majority-gated): a node
   that went `Failed` and never sent a goodbye is promoted to `Removed`
   by each quorate survivor after a configurable
   `removal_confirmation_timeout` (default 60s, matching the existing
   `FAILED_NODE_RETENTION` purge — the point at which the cluster
   already forgets the node). The promotion happens only when the local
   node's resolver decision is `StayUp` (i.e. the local node is on the
   majority side and retains quorum). A `Failed` node that was merely
   partitioned re-joins via probe within this window and is never
   promoted; a truly dead node is. This is the bounded risk window: up
   to `removal_confirmation_timeout` of degraded availability for
   actors on the dead node, in exchange for never re-spawning while the
   old node might still be alive.

`Removed` is a new `NodeStatus` value (or an explicit `removed: bool`
flag on `NodeInfo` — see §6) that is **not** gossip-replicated as
Healthy/Joining; it is a local survivor-side determination, like the
`Failed` status today. `ClusterAction` gains `NodeRemoved { node }`,
which the runtime routes to the re-spawn driver (§4).

### 2. Durable-actor location directory

To re-spawn the right actors on the right node, the cluster needs to
know, for each durable actor, where it lives and at what epoch. This is
a new gossip-replicated directory:

```
DurableDirectoryEntry { actor_id: u64, node_id: NodeId, epoch: u64 }
```

- The **home node** of every durable actor that is opted into re-spawn
  (§3) announces its entry piggybacked on the existing `Packet::Gossip`
  round (one `Vec<DurableDirectoryEntry>` per gossip payload, additive
  field — old peers ignore it).
- Every node stores the directory as `HashMap<actor_id, (NodeId, u64)>`,
  updated by **highest-epoch-wins** merge (same rule as incarnation in
  `merge_membership`).
- On `NodeRemoved`, a survivor with quorum collects all directory entries
  whose `node_id == removed` and feeds them to the re-spawn driver.
- The directory is also how a resurrected node detects it has been
  replaced (§5): on re-join, if the directory holds a higher epoch for
  one of its own durable actors, it self-demotes that actor.

The `RemoteActorCache` is explicitly **not** this directory: it is a
TTL cache of *contacted* remote actors and loses entries; the directory
is authoritative, gossip-replicated, and only covers re-spawn-opted
actors (a small, bounded set).

### 3. Snapshot replication: the shadow copy

The dead node's local persistence store is unreachable, so "from the
last durable snapshot" requires the snapshot to exist elsewhere. Each
re-spawn-opted durable actor replicates its snapshot to a **shadow
node** — the deterministic next healthy member after its home node in
cluster-id order (the same deterministic rule the resolver's
`static-quorum` expected-size config uses; no leader election needed):

- The replication hook is `workflow::checkpoint_actor`, the single
  choke point through which every durable snapshot already flows. After
  `rt.persistence.save_snapshot(snapshot)`, if the actor is
  re-spawn-opted and a shadow is known, serialize `snapshot` + the
  actor's NBC module (already obtainable via `module.to_nbc(None)`) into
  the existing `Packet::MigrateActor` shape and send it to the shadow.
  The shadow stores it in its own persistence store under the same
  `actor_id` (actor ids are globally unique via `fresh_actor_id`, so no
  collision) plus an `epoch` marker.
- The shadow node re-ships the snapshot at re-spawn time: the
  re-spawning node either is the shadow itself or fetches from it via
  the existing `Packet::MigrateActor` (reuse; no new packet).
- CRDT state needs no special handling: `crdt_snapshot` is already part
  of `ActorSnapshot` and converges cluster-wide via `CrdtManager`
  replication regardless of which node hosts the actor. Re-spawn merges
  the carried `crdt_snapshot` on the target node exactly as
  `receive_migrated_actor` does today.
- If the shadow is unreachable at checkpoint time (single-point
  concern), the checkpoint is written locally as today and the next
  checkpoint retries; a node is only re-spawn-opted once a shadow has
  acknowledged at least one replica (the directory entry's presence
  implies an acknowledged first replica). Actors whose replica was never
  acknowledged are **not** re-spawned — they degrade to the
  part-(a)+(b) behavior (DOWN notification), never a partial re-spawn
  with a stale snapshot.

### 4. Re-spawn driver and policy surface

The trigger is `ClusterAction::NodeRemoved { node }` (§1). The driver
runs on every quorate survivor, but only the node that holds the
**shadow replica** actually re-spawns each actor (the directory + shadow
determinism ensures exactly one node holds the replica for a given
actor).

Policy is per-child, on the supervisor that owns the durable actor —
extending the existing `ChildSpec`/`RestartPolicy` seam:

```rust
pub enum RestartPolicy {
    Permanent,   // existing: always restart
    Temporary,   // existing: never restart
    Transient,   // existing: restart on abnormal exit
    RespawnOnNodeLoss, // NEW: re-spawn on the shadow node when the
                       // home node is confirmed Removed, from the last
                       // replicated snapshot
}
```

`RespawnOnNodeLoss` implies `Permanent` semantics for the exit
protocol (a `Removed` home node's actors are dead; the child restarts),
and additionally:

1. **Opt-in**: the supervisor registers the child with the directory
   (§2) and enables snapshot replication (§3) at `supervise_child` time
   when the policy is `RespawnOnNodeLoss`.
2. **Re-spawn**: on `NodeRemoved`, the shadow node restores the actor
   from its replica via `receive_migrated_actor` (reusing its
   `RestartTemplate`-equivalent metadata), re-parents it under the same
   supervisor if that supervisor also re-spawned, otherwise under a
   fresh supervisor of the same strategy, and bumps the epoch in the
   directory.
3. **No silent re-spawn**: `RespawnOnNodeLoss` is an explicit policy on
   an explicit supervision edge. Default supervision (`Permanent` etc.)
   behaves exactly as today — the PLAN's "Do NOT implement silent
   automatic migration on node failure" is preserved.

### 5. Two-live-copies resolution: epoch + self-demote

The residual hazard is a node that was promoted `Removed` by timeout
(§1 path 2) but was actually alive and later heals. The epoch rule
makes this safe:

- The directory entry carries the actor's activation epoch; every
  re-spawn bumps it.
- When a node re-joins the cluster (probe or fresh join), it consults
  the directory for its own durable actors. If the directory holds a
  **higher epoch** for an actor it hosts, the node **self-demotes** that
  actor: it checkpoints (discarded — the replica is newer), terminates
  the local copy, and keeps only the directory entry. It does not
  resume writing to durable state, so no two nodes ever write the same
  durable-id actor.
- A node that sent `NodeGoodbye` (§1 path 1) re-joins only under a
  fresh incarnation and must re-announce; the goodbye made its old
  actors dead by its own declaration.

This mirrors how Orleans resolves activation races (versioned
activations) and how the existing `migrated_actors` forwarding entry
(TTL 60s, `MIGRATED_ACTOR_TTL`) already prevents double delivery for
explicit migration — re-spawn reuses the same forwarding mechanism so
in-flight messages during the handoff window are routed to the new
location.

### 6. Wire changes (additive; NUL0 v1-compatible)

New packet variants, all with unused type bytes (15+; the 0-14 range is
allocated in `network.rs`):

- `Packet::NodeGoodbye { node_id: NodeId, durable: Vec<(u64, u64)> }`
  (`TYPE_NODE_GOODBYE = 15`) — §1 path 1. Carries `(actor_id, epoch)`
  pairs for the sender's re-spawn-opted durable actors.
- `Packet::MigrateActor` (existing, `TYPE_MIGRATE_ACTOR = 14`) is
  reused unchanged for both explicit migration (D14) and snapshot
  replication (§3) — the shadow store is a normal `PersistenceStore`
  entry keyed by `actor_id`. No new snapshot packet needed.
- Directory entries ride inside the existing `Packet::Gossip` payload
  as an additive `Vec<DurableDirectoryEntry>` field (§2).

Per the established additive-type precedent (`TYPE_LINK`/`TYPE_MONITOR`/
`TYPE_DOWN`/`TYPE_CRDT_OP`/`TYPE_MIGRATE_ACTOR` were added post-v1 and
old peers drop unknown types), none of these require a NUL0 version
bump. RFC 0013's authenticated-transport v2 work is orthogonal: all new
packets flow over whatever transport the cluster already negotiated.

### 7. Configuration

Additive `ClusterConfig` fields, defaults preserving current behavior:

```rust
pub struct ClusterConfig {
    // ... existing (split_brain, probe_interval) ...
    pub removal_confirmation_timeout: Duration, // default 60s
    pub directory_gossip: bool,                  // default true
}
```

`removal_confirmation_timeout = 0` disables timeout-based promotion
(only `NodeGoodbye` triggers re-spawn — the strictest configuration;
`Duration::MAX` effectively disables D7c entirely, matching today).

## Backwards Compatibility

- All new surface is additive: new `RestartPolicy` variant (existing
  policies unchanged), new packet types (old peers drop them), new
  gossip field (old peers ignore it), new config fields (defaults
  preserve behavior).
- No existing supervision semantics change. Actors not opted into
  `RespawnOnNodeLoss` behave exactly as today (DOWN notification only).
- `Removed` is a survivor-side status derived from `Failed` + timeout
  (or goodbye); it does not change the wire meaning of `Failed`.
- `receive_migrated_actor` and `Packet::MigrateActor` are reused
  unchanged — the re-spawn path is a caller of existing machinery, not
  a fork.

## Verification Requirements

Landing the implementation requires, in the same change set:

1. **Directory unit tests**: highest-epoch-wins merge, entry
   announcement piggyback on gossip, removal of a node's entries on
   `NodeRemoved`.
2. **Shadow replication tests**: checkpoint → shadow replica stored;
   unreachable shadow → no re-spawn opt-in; re-spawn from shadow
   replica restores identical durable state (round-trip through
   `receive_migrated_actor`).
3. **Confirmed-gone tests**: `NodeGoodbye` → immediate `Removed` →
   re-spawn; `Failed` without goodbye → no re-spawn before
   `removal_confirmation_timeout`, re-spawn after; partition that heals
   within the window → probe re-join, no re-spawn, no epoch bump.
4. **Two-live-copies test**: a `Removed`-by-timeout node re-joins with
   a live actor; the directory's higher epoch triggers self-demote;
   exactly one live copy remains and the durable store is written by
   exactly one node.
5. **DST scenarios** in `src/runtime/cluster_sim.rs`: kill a node
   carrying a `RespawnOnNodeLoss` actor; assert the actor is re-spawned
   on the shadow with state equal to the last checkpoint, messages sent
   to the old location are forwarded (via `migrated_actors`), and no
   duplicate actor exists on any node.
6. **End-to-end `.nula` test**: `supervisor` + `RespawnOnNodeLoss`
   child + node kill, asserting the child's durable field survives on
   the new node.

## Implementation Roadmap

1. `NodeStatus::Removed`/flag + `ClusterAction::NodeRemoved` +
   `removal_confirmation_timeout` promotion in `cluster.rs` (pure
   membership, unit-testable).
2. `Packet::NodeGoodbye` + send-on-shutdown/self-down + receive-side
   promotion (network.rs, distribution.rs).
3. Directory: `DurableDirectoryEntry`, gossip piggyback, highest-epoch
   merge (cluster.rs, distributed.rs).
4. Shadow replication in `workflow::checkpoint_actor` + shadow store +
   re-spawn driver on `NodeRemoved` (workflow.rs, mod.rs).
5. `RestartPolicy::RespawnOnNodeLoss` + `supervise_child` opt-in +
   re-parenting (supervisor.rs).
6. Epoch self-demote on re-join (distributed.rs, cluster.rs).
7. Tests per §Verification; RFC 0012's cross-node supervision
   (deliverable 8's remaining send-side wiring) is a prerequisite for
   watchers of re-spawned actors to keep receiving `DOWN` — coordinate.

## Open Questions

- **Shadow determinism under partial-view membership**: the "next
  healthy member" rule needs a stable total order across views. RFC
  0011 deferred `keep-majority`/`keep-oldest` partly on this basis.
  Fallback: shadow = explicitly configured node per actor type
  (`@shadow "node-name"` annotation) — deterministic and
  operator-controlled, at the cost of a single-point shadow. Decide
  when implementing §3.
- **`Removed` retention**: should a `Removed` node be forgotten on a
  shorter timer than `FAILED_NODE_RETENTION`? The directory entries it
  leaves behind must persist at least until re-spawn completes.
- **Supervisor re-parenting**: when the removed node also hosted the
  child's supervisor, the shadow node creates a new supervisor of the
  same strategy. `SupervisorAction::Escalate` semantics across the
  re-parent boundary need a decision (escalate to the shadow's own
  parent? or to a synthetic root?).
- **Gossip size**: the directory grows with re-spawn-opted actors. The
  gossip payload already caps at `GOSSIP_PAYLOAD_MAX_ENTRIES`; the
  directory needs its own cap + full-state repair cadence (mirroring
  the CRDT full-sync pattern) or it becomes a scalability ceiling.

## Alternatives Considered

- **Respawn everything on `Failed`** (no confirmation): rejected — the
  two-live-copies hazard the PLAN explicitly forbids; a partitioned
  node's actors would be re-spawned while still alive.
- **Lease-based activation** (Orleans-style): each activation holds a
  lease renewed via heartbeat; expiry = presumed dead. Rejected for v1:
  leases add a renewal protocol and a clock-skew hazard, and a
  self-downed node (transport shut) cannot renew anyway — the same
  ambiguity the goodbye solves more cheaply. Epoch + self-demote covers
  the race leases would.
- **Raft-backed durable-store replication**: out of scope per
  `PERFORMANCE_ANALYSIS.md` row 3.4's standing deferral; this RFC is
  explicitly not consensus, exactly like RFC 0011's resolver.
- **CRDT-only resurrection** (re-spawn durable actors from CRDT fields,
  drop Durable/EventSourced fields): rejected — violates "from the last
  durable snapshot" for the very state models whose whole point is
  non-CRDT durability.
