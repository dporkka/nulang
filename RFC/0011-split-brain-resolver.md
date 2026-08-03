# RFC 0011: Split-Brain Resolver for Cluster Membership

- **Status:** Draft
- **Tier:** Experimental (formally tiers the previously-untiered
  cluster-membership surface; see Tier Classification)
- **Author:** AI assistant review
- **Created:** 2026-08-03
- **Resolved:** (pending)
- **Language-version at effect:** none (no Frozen or Stable surface touched;
  NUL0 wire protocol v1 unchanged)
- **Supersedes:** none
- **Superseded by:** none

## Summary

Add a pluggable split-brain resolver to the cluster-membership runtime so a
partitioned cluster converges to one surviving side instead of running two
disconnected partitions indefinitely, and fix the related non-self-healing
bug: once two sides of a clean partition each mark the other `Failed`,
neither side ever redials the other, so the split persists until an external
rejoin. The first shipped strategy is `static-quorum` (configured expected
cluster size, no live member count). The resolver is opt-in and defaults to
disabled, so existing embedders see zero behavior change.

## Motivation

`ClusterState` (`src/runtime/cluster.rs`) has no quorum, leader-election, or
majority logic anywhere — there are zero `quorum`/`leader`/`elect` hits in
`src/runtime/` (the only substring matches are inside "selective"). A node's
`tick()` sends heartbeats and gossip only to `Healthy`/`Joining` members, so
when a partition splits a cluster and both sides time each other out to
`Failed` (the failure detector already works: 2s heartbeat timeout →
`Suspicious`, +5s → `Failed`, 60s retention), neither side redials the other.
The split does not self-heal; it requires an external rejoin. The existing
multi-node test coverage explicitly disclaims this: `tests.rs` states that
split-brain (two healthy sub-clusters that can't see each other) and
asymmetric partition are "NOT covered" and are follow-up.

This matters because Nulang's distributed-actor story is a core differentiator
and the current runtime cannot survive an adversarial operational review: a
partitioned cluster keeps both halves serving the same durable actors, and a
resolved partition silently re-merges two divergent membership views with no
record of who should have won.

## Design

### 1. `SplitBrainResolver` trait (`src/runtime/cluster.rs`)

A resolver observes the local membership view and decides whether the local
node stays up. It is a pure function of the view — no I/O, no timers — so it
is trivially unit-testable and DST-drivable.

```rust
/// A snapshot of what the local node can currently see.
pub struct MembershipView {
    pub local: NodeId,
    pub members: Vec<NodeInfo>, // all known members with current status
}

pub enum ResolverDecision {
    StayUp,
    DownSelf,
}

pub trait SplitBrainResolver: Send + Sync {
    fn decide(&self, view: &MembershipView) -> ResolverDecision;
}
```

`ClusterState::tick()` consults the resolver once per tick (after the
failure-detection passes). `StayUp` changes nothing. `DownSelf` transitions
the local node to a new `Down` state and emits a new action.

### 2. `StaticQuorumResolver` — the shipped strategy

```rust
pub struct StaticQuorumResolver { pub expected_nodes: usize }
```

Rule: the local node counts the members it currently sees as reachable —
itself plus every member whose status is `Healthy` or `Joining`
(`Suspicious` and `Failed` do not count). If the reachable count is at least
`floor(expected_nodes / 2) + 1`, the node stays up; otherwise it downs
itself.

- Requires only the operator-configured expected cluster size — no live
  count, no consensus, no leader.
- **2-node caveat (documented, not fixed):** with `expected_nodes = 2`, both
  sides see only themselves during a partition (`1 < 2`) and both down
  themselves — the cluster fails closed with no survivor. This is the
  standard static-quorum property; the strategy is only useful for
  `expected_nodes >= 3`. The RFC keeps this behavior because fail-closed is
  the safe failure mode for a 2-node cluster (a silent two-sided split is
  worse than a total outage).
- `expected_nodes == 0` is rejected at configuration time (treated as a
  configuration error, not as "disabled" — see §4).

`keep-majority`/`keep-oldest` are deliberately NOT specified here: they
depend on an accurate live member count, which partial-view membership can
undermine. They may be added later as additional implementations of the same
trait, gated on the partial-view work proving count accuracy (Phase 5
deliverable 6 of `PLAN.md`). The trait contract above is the extension seam.

### 3. Down semantics and the new `ClusterAction` variants

Two new variants join `ClusterAction`:

```rust
pub enum ClusterAction {
    // ... existing variants ...
    /// The resolver decided the local node should leave the cluster.
    Down { node: NodeId },          // node == local node
    /// Minimal periodic liveness probe to a Failed member.
    Probe { to: NodeId, addr: SocketAddr },
}
```

- `Down { node }` is emitted exactly once when the resolver first decides
  `DownSelf`. The runtime marks the local node `Down` (a `local_down` flag on
  `ClusterState`, exposed as `ClusterState::is_down()`); a downed node stops
  emitting heartbeats and gossip (`tick()` returns no actions), stops
  processing cluster packets, and keeps running local actors. Peers learn of
  the down via their own failure detector and mark the node `Failed`
  normally — no new wire message is needed. No `on_member_*` callback fires
  for the downed node's own peers (they discover it through existing
  timeouts), and the downed node's local callbacks (`on_member_left`,
  `on_member_failed`) fire for its own entry so operators can react.
- `Probe { to, addr }` is emitted for each `Failed` member at a configurable
  `probe_interval` (default 5s; heartbeat interval is 500ms — probes are a
  minimal liveness check, not full heartbeating). The runtime sends an
  ordinary `Heartbeat` packet to the address; the wire packet type already
  exists, so **NUL0 v1 is untouched — no new packet type, no version bump**.
  If the probe reaches a live node, the node's existing `handle_heartbeat`
  promotion logic (which already promotes `Suspicious`/`Failed` back to
  `Healthy` on any heartbeat) re-joins it: **this is the self-healing fix** —
  when the network recovers from a partition, both sides' probes succeed and
  the cluster re-merges with no external rejoin. A node that is truly dead
  never answers and stays `Failed` until the 60s retention purge, unchanged
  from today.

### 4. Configuration plumbing (additive, non-breaking)

```rust
pub enum SplitBrainConfig {
    Disabled,                                    // default
    StaticQuorum { expected_nodes: usize },
}

pub struct ClusterConfig {
    pub split_brain: SplitBrainConfig,
    pub probe_interval: Duration,                // default 5s
}
```

New additive API on `Runtime`:

```rust
impl Runtime {
    /// Must be called before `enable_distribution`; defaults to Disabled.
    pub fn set_cluster_config(&mut self, config: ClusterConfig);
}
```

The signature of `enable_distribution(addr, tls_config)` is unchanged
(deliverable 4 of `PLAN.md` Phase 5 reshapes it under its own RFC). The
default configuration is `SplitBrainConfig::Disabled` with a 5s probe
interval — existing embedders get no behavior change except the self-healing
probe to `Failed` members (a fix, not a break: it cannot resurrect a node
that stays down, and it cannot prevent a failed node from being purged).

### 5. Verification requirements

Landing this RFC's implementation requires, in the same change set:

- Unit tests for `StaticQuorumResolver::decide` across every reachable-count
  boundary (`floor(N/2)+1 - 1`, the threshold, the threshold + 1) and the
  `expected_nodes = 0` configuration error.
- A probe test: two `Runtime`s over loopback TCP, one killed transport (hard
  failure), the survivor transitions the peer to `Failed` and then promotes
  it back after the transport is restored — no external rejoin.
- The DST/chaos scenarios `PLAN.md` Phase 5 deliverable 2 names — mutually
  invisible healthy sub-clusters and asymmetric (one-way) partition on 3-node
  and 5-node topologies — asserting the cluster converges to exactly one
  surviving side per the configured strategy and never a stuck two-sided
  split. These land as the verification vehicle for this RFC (deliverable 2
  is sequenced immediately after deliverable 1 in `PLAN.md`).

## Tier Classification

Cluster membership is currently **untiered**: GOVERNANCE.md's Frozen list
covers `.nbc`, NUL0 v1, value layout, Core syntax, and the `IO`/`Spawn`/
`Send`/`Receive` effects; its Stable list covers the capability lattice and
the actor surface; neither names `ClusterState`, `enable_distribution`,
`join_cluster`, or membership behavior generally. This RFC formally assigns
that surface (including the new resolver) to the **Experimental** tier:

- It is a Rust-embedder-only API with no `.nula` syntax and no CLI surface.
- NUL0 wire protocol v1 is untouched (the probe reuses the existing
  `Heartbeat` packet type; `Down` is runtime-local state).
- No language-version bump is triggered.

Graduation to Stable is a follow-up RFC, gated on the DST/chaos suite above
proving the resolver semantics under partition (the evidence-first bar
GOVERNANCE.md §3 sets). The resolver's operational blast radius — a mechanism
that can autonomously shut down a node — is exactly why this RFC exists even
though the surface is formally untiered.

## Backwards Compatibility

- No existing program or embedder breaks: the resolver defaults to
  `Disabled`, and `enable_distribution`'s signature is unchanged.
- The only behavior change for existing embedders is the self-healing probe:
  `Failed` members are now contacted with a minimal heartbeat every
  `probe_interval` instead of never. A live-but-partitioned node rejoins
  automatically when the network recovers (previously: manual rejoin);
  a dead node behaves exactly as before (stays `Failed`, purged after 60s).
- No deprecation cycle is required; nothing is removed.

## Alternatives Considered

1. **Raft/consensus-backed membership.** Rejected for this RFC by reference:
   `PERFORMANCE_ANALYSIS.md` row 3.4 defers native Raft ("CRDTs cover 80% of
   distributed state needs"), and a split-brain resolver provides partition
   safety without consensus. This RFC does not relitigate that deferral.
2. **`keep-majority`/`keep-oldest` now.** Rejected: both need an accurate
   live member count, and partial-view membership (Phase 5 deliverable 6)
   can make live counts unreliable. They are deferred behind the same trait
   until deliverable 6 proves count accuracy — shipping a resolver that
   silently miscounts is worse than shipping `static-quorum` alone.
3. **External rejoin tooling (operator script).** Rejected: the phase goal
   (PLAN.md Phase 5) is a cluster that provably converges to one surviving
   side, not one whose operator must notice and intervene.
4. **Link-quality-based partition detection (TCP keepalive tuning, ICMP).**
   Rejected: the membership view is the system of record, and the heartbeat
   state machine already encodes reachability; a second, independent signal
   would only add disagreement between the resolver and the failure
   detector.

## Open Questions

1. **Down semantics for local actors.** This RFC keeps local actors running
   on a downed node (only cluster participation stops). An alternative is a
   full local halt. The former is less surprising for operators debugging a
   partition; confirm during discussion.
2. **Probe interval default.** 5s is proposed (10× the heartbeat interval).
   A shorter probe speeds rejoin after recovery; a longer one reduces noise
   against a dead node. Not load-bearing — an operator-configurable knob in
   any case.
3. **Operator alerting on resolver decisions.** Proposed: reuse the existing
   `on_member_failed`-style callback pattern so a `DownSelf` decision is
   observable from Rust. Whether a log line suffices for v1 is open.
4. **`expected_nodes` drift.** If the operator's configured expected size is
   wrong (too high), a healthy cluster downs itself; too low, and a
   partition may keep both sides up. Should the resolver warn when the
   reachable count exceeds `expected_nodes` (a likely misconfiguration)?
   Non-blocking: the failure mode is already conservative (config error is
   loud at startup, and the strategy is opt-in).

## Resolution

(To be filled on accept/reject.)
