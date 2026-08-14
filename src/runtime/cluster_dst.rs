//! Deterministic multi-node cluster harness (PLAN.md Phase 1 bullet 2:
//! cluster/network determinism).
//!
//! [`DeterministicCluster`] drives N REAL [`Runtime`] instances over the
//! in-memory [`DeterministicNetworkTransport`] — no threads, no sleeps, no
//! wall-clock reads that affect state — with per-node virtual clocks
//! advanced in lockstep and ONE seeded RNG governing node execution order
//! and per-node actor selection, so the same seed reproduces the same run
//! while different seeds explore different interleavings.
//!
//! This is the vehicle for the 10³-seeds-per-commit cluster invariant
//! sweep the real-TCP chaos tests (`tests.rs`) cannot scale to: each round
//! is pure compute in virtual time (microseconds), so hundreds of seeds ×
//! hundreds of rounds complete in seconds.
//!
//! Fidelity notes (mirroring `cluster_sim.rs`):
//! - The transport is zero-latency FIFO per link (TCP-like), so within a
//!   link ordering is preserved; cross-link ordering is seeded via the
//!   per-round node execution order.
//! - Heartbeats, gossip, probes, and actor messages all cross the same
//!   fabric; `set_partition` drops outbound packets exactly like a
//!   firewall (the production failure detector then reacts in virtual
//!   time).
//! - The cluster tick cadence runs on the virtual clock
//!   (`ClusterState::set_clock`), and every node's `ClusterState` carries
//!   a seeded RNG (`set_rng`) so gossip/repair picks are bit-reproducible.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::dst::DeterministicRng;
use crate::runtime::network::DeterministicNetworkTransport;
use crate::runtime::{ActorAddress, NodeId, Runtime};

/// Wall-clock step per simulated round (the real runtime ticks cluster
/// maintenance roughly every 100 ms; heartbeats fire on the virtual clock
/// at the same cadence).
const ROUND_STEP: Duration = Duration::from_millis(100);

/// Scheduler step budget per node per round. A node runs until its local
/// actors Quiesce or this budget is exhausted; the budget makes a runaway
/// behavior fail as `StepLimitExceeded` instead of hanging the harness.
const STEPS_BUDGET: u64 = 100_000;

/// Deterministic multi-node cluster harness.
pub(crate) struct DeterministicCluster {
    /// The real runtimes, one per simulated node (index-aligned with
    /// `addrs`).
    pub nodes: Vec<Runtime>,
    /// Node addresses; each node's id is derived from its address.
    pub addrs: Vec<SocketAddr>,
    /// Master seeded RNG: drives per-round node order and hands each
    /// node's scheduler its selections from one shared stream.
    rng: DeterministicRng,
    /// Outbound partition sets, index-aligned with `nodes`. The transport
    /// contract replaces the whole set on `set_partition`, so the harness
    /// owns the sets and re-applies them (multiple `partition` calls
    /// accumulate; `heal` clears one node's set).
    partitions: Vec<std::collections::HashSet<NodeId>>,
    /// Rounds executed.
    pub round: u64,
    /// How many times a node hit the per-round step budget
    /// (`STEPS_BUDGET`, i.e. `StepLimitExceeded`) — a livelock signal.
    pub limit_hits: u64,
}

impl DeterministicCluster {
    /// Create `addrs.len()` real `Runtime`s, each with its own virtual
    /// clock and an in-memory `DeterministicNetworkTransport` registered
    /// on a shared bus, joined into a full mesh (every node seeds every
    /// other). `seed` seeds the master RNG; each node's `ClusterState`
    /// gets its own derived seeded RNG.
    pub fn new(addrs: &[SocketAddr], seed: u64) -> Self {
        let bus = Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let mut rng = DeterministicRng::new(seed);
        let mut nodes = Vec::with_capacity(addrs.len());
        for &addr in addrs {
            let mut rt = Runtime::new();
            // Clock BEFORE distribution is enabled so `enable_distribution`
            // clones it into the ClusterState (all cluster time queries —
            // heartbeat cadence, suspicion, probes — then run virtual).
            rt.install_virtual_clock();
            let transport = DeterministicNetworkTransport::bind_with_bus(addr, bus.clone())
                .expect("dst transport binds");
            // Register on the shared bus while still concrete (the trait
            // object hides `register_on_bus`).
            transport.register_on_bus();
            rt.enable_distribution_with_transport(Box::new(transport))
                .expect("dst distribution enables");
            if let Some(cluster) = rt.distributed.cluster.as_mut() {
                cluster.set_rng(Box::new(DeterministicRng::new(rng.next())));
            }
            nodes.push(rt);
        }
        // Join the mesh: every node seeds every other.
        for rt in nodes.iter_mut() {
            for &peer in addrs {
                rt.join_cluster(peer);
            }
        }
        DeterministicCluster {
            nodes,
            addrs: addrs.to_vec(),
            rng,
            partitions: vec![std::collections::HashSet::new(); addrs.len()],
            round: 0,
            limit_hits: 0,
        }
    }

    /// The node id of the node at `index`.
    pub fn id(&self, index: usize) -> NodeId {
        NodeId::new(&self.addrs[index])
    }

    /// Immutable access to the runtime at `index`.
    pub fn node(&self, index: usize) -> &Runtime {
        &self.nodes[index]
    }

    /// Mutable access to the runtime at `index`.
    pub fn node_mut(&mut self, index: usize) -> &mut Runtime {
        &mut self.nodes[index]
    }

    /// Cut `from`'s outbound link to `to` (a firewall-style partition;
    /// every packet from `from` to `to` is silently dropped). Accumulates:
    /// multiple `partition` calls on the same node stay active together.
    pub fn partition(&mut self, from: usize, to: usize) {
        let pid = self.id(to);
        self.partitions[from].insert(pid);
        self.apply_partitions();
    }

    /// Restore every outbound link of the node at `index`.
    pub fn heal(&mut self, index: usize) {
        self.partitions[index].clear();
        self.apply_partitions();
    }

    /// Push the harness-owned partition sets into the transports
    /// (`set_partition` replaces, so the full set is always re-applied).
    fn apply_partitions(&mut self) {
        for (i, peers) in self.partitions.iter().enumerate() {
            let transport = self.nodes[i]
                .distributed
                .transport
                .as_mut()
                .expect("transport");
            transport.set_partition(peers.clone());
        }
    }

    /// Run one round: advance every node's virtual clock by `ROUND_STEP`,
    /// then execute the nodes in a seed-permuted order — each node first
    /// drains its transport (packet delivery + cluster tick) and then runs
    /// its deterministic scheduler until its local actors Quiesce or the
    /// step budget is exhausted.
    pub fn step_round(&mut self) {
        let n = self.nodes.len();
        // Seeded Fisher-Yates over node indices: which node runs first in
        // this round is part of what the seed permutes.
        let mut order: Vec<usize> = (0..n).collect();
        for i in 0..n {
            let j = (self.rng.next() as usize) % (n - i);
            order.swap(i, i + j);
        }
        for idx in order {
            let rt = &mut self.nodes[idx];
            rt.advance_time(ROUND_STEP);
            rt.process_network();
            let result = rt.run_scheduler_deterministic_with_rng(&mut self.rng, STEPS_BUDGET);
            if matches!(
                result,
                crate::runtime::DeterministicRunResult::StepLimitExceeded { .. }
            ) {
                self.limit_hits += 1;
            }
        }
        self.round += 1;
    }

    /// Run `rounds` rounds.
    pub fn run_rounds(&mut self, rounds: u64) {
        for _ in 0..rounds {
            self.step_round();
        }
    }

    /// The status of `node` in node `viewer`'s cluster view.
    pub fn cluster_status(
        &self,
        viewer: usize,
        node: NodeId,
    ) -> Option<crate::runtime::NodeStatus> {
        self.nodes[viewer]
            .distributed
            .cluster
            .as_ref()
            .and_then(|c| c.get_node(node))
            .map(|info| info.status)
    }

    /// True once every node has every OTHER node in its ACTIVE view (the
    /// failure detector watches only the active view, which fills through
    /// the repair cycle + reciprocal heartbeat confirmation).
    pub fn active_views_converged(&self) -> bool {
        let ids: Vec<NodeId> = self
            .nodes
            .iter()
            .map(|rt| rt.distributed.node_id.unwrap())
            .collect();
        self.nodes.iter().all(|rt| {
            let c = rt.distributed.cluster.as_ref().expect("cluster");
            let active: Vec<NodeId> = c.active_view().to_vec();
            let local = rt.distributed.node_id.unwrap();
            ids.iter().all(|id| *id == local || active.contains(id))
        })
    }

    /// Send a message from node `from` to a remote actor on node `to`'s
    /// runtime (location-transparent addressing over the in-memory fabric).
    pub fn send_remote(
        &mut self,
        from: usize,
        to: usize,
        actor_id: u64,
        behavior: &str,
        args: &[crate::vm::Value],
    ) {
        let target = ActorAddress::remote(self.id(to), actor_id);
        self.nodes[from].send_distributed(target, behavior, args);
    }

    /// A compact digest of the cluster's observable state — every node's
    /// view of every peer's status — for same-seed reproducibility checks
    /// (the scheduler interleaving itself is not directly observable, but
    /// a different interleaving that changed membership/gossip timing
    /// shows up here).
    pub fn digest(&self) -> String {
        let mut out = String::new();
        for (i, rt) in self.nodes.iter().enumerate() {
            let local = rt.distributed.node_id.unwrap();
            out.push_str(&format!("n{i}[{:x}]:", local.0));
            if let Some(c) = &rt.distributed.cluster {
                for peer in &self.addrs {
                    let pid = NodeId::new(peer);
                    if pid == local {
                        continue;
                    }
                    let status = c
                        .get_node(pid)
                        .map(|info| info.status)
                        .unwrap_or(crate::runtime::NodeStatus::Joining);
                    out.push_str(&format!("{:x}:{:?};", pid.0, status));
                }
            }
            out.push('|');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::NodeStatus;
    use crate::vm::Value;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    /// Spawn a counter actor with an `inc` behavior that adds its arg to a
    /// `count` state field, and return its id.
    fn spawn_counter(rt: &mut Runtime) -> u64 {
        let id = rt.spawn_actor(Box::new(|| vec![("count".to_string(), Value::int(0))]));
        {
            let actor = rt.actors.get_mut(&id).unwrap();
            actor.register_behavior("inc", |actor, args| {
                let n = actor
                    .get_state_field("count")
                    .and_then(|v| v.as_int())
                    .unwrap_or(0);
                let by = args.get(0).and_then(|v| v.as_int()).unwrap_or(0);
                actor.set_state_field("count", Value::int(n + by));
            });
        }
        id
    }

    fn counter_value(cluster: &DeterministicCluster, node: usize, id: u64) -> i64 {
        cluster
            .node(node)
            .actors
            .get(&id)
            .and_then(|a| a.get_state_field("count"))
            .and_then(|v| v.as_int())
            .unwrap_or(-1)
    }

    /// PLAN.md Phase 1 bullet 2 (DST): cluster/network determinism — the
    /// in-memory-fabric harness is bit-reproducible per seed. Two runs with
    /// the same seed must produce identical per-round digests (node order,
    /// gossip/repair picks, actor selection are all seed-driven).
    #[test]
    fn test_dst_cluster_same_seed_reproducible() {
        const ROUNDS: u64 = 40;
        let run = |seed: u64| -> (Vec<(String, i64)>, i64) {
            let mut cluster = DeterministicCluster::new(&[addr(9101), addr(9102)], seed);
            // Converge membership first: the resolver refuses to route to a
            // node that is not yet Healthy, so early sends would be dropped.
            cluster.run_rounds(20);
            let counter = spawn_counter(&mut cluster.node_mut(0));
            // Burst of remote messages from node 1 to the counter on node 0.
            for _ in 0..20 {
                cluster.send_remote(1, 0, counter, "inc", &[Value::int(1)]);
            }
            let mut trace = Vec::new();
            for _ in 0..ROUNDS {
                cluster.step_round();
                trace.push((cluster.digest(), counter_value(&cluster, 0, counter)));
            }
            (trace, counter_value(&cluster, 0, counter))
        };
        let (trace_a, count_a) = run(42);
        let (trace_b, count_b) = run(42);
        assert_eq!(
            trace_a, trace_b,
            "same seed must produce the same cluster evolution (digest + counter per round)"
        );
        assert_eq!(count_a, count_b, "same seed must produce the same count");
        assert_eq!(count_a, 20, "all 20 remote messages delivered");
    }

    /// PLAN.md Phase 1 bullet 2 (DST): the cluster seed-sweep invariant
    /// test. N seeds × a burst of M remote messages across the in-memory
    /// fabric; for EVERY seed:
    ///  1. The cluster converges to a full-Healthy membership (the fabric
    ///     plus virtual-clock gossip actually forms a cluster).
    ///  2. No node hits the step budget (no deadlock/livelock).
    ///  3. AtMostOnce delivery: the counter reaches exactly M.
    #[test]
    fn test_dst_cluster_remote_delivery_seed_sweep() {
        const MESSAGES: i64 = 30;
        let seeds = crate::dst::dst_seed_count(50);
        const ROUNDS: u64 = 40;

        for seed in 0..seeds {
            let mut cluster = DeterministicCluster::new(&[addr(9111), addr(9112)], seed);
            // Converge membership before sending (resolver refuses to route
            // to a non-Healthy node).
            cluster.run_rounds(20);
            let counter = spawn_counter(&mut cluster.node_mut(0));
            for _ in 0..MESSAGES {
                cluster.send_remote(1, 0, counter, "inc", &[Value::int(1)]);
            }
            cluster.run_rounds(ROUNDS);

            assert_eq!(
                cluster.limit_hits, 0,
                "seed {seed}: step budget exceeded — possible deadlock/livelock"
            );
            // Membership converged: each node sees the other Healthy.
            let id1 = cluster.id(1);
            let id0 = cluster.id(0);
            assert_eq!(
                cluster.cluster_status(0, id1),
                Some(NodeStatus::Healthy),
                "seed {seed}: node 0 must see node 1 healthy"
            );
            assert_eq!(
                cluster.cluster_status(1, id0),
                Some(NodeStatus::Healthy),
                "seed {seed}: node 1 must see node 0 healthy"
            );
            let count = counter_value(&cluster, 0, counter);
            assert_eq!(
                count, MESSAGES,
                "seed {seed}: counter must reach exactly {MESSAGES} (AtMostOnce), got {count}"
            );
        }
    }

    /// PLAN.md Phase 1 bullet 2 (DST): partition + failure detection +
    /// self-healing over the deterministic fabric, end to end through the
    /// REAL runtime. A 3-node cluster forms, C is partitioned away from
    /// {A, B} (firewall-style drop both directions), both sides detect the
    /// other as `Failed` through the REAL virtual-clock failure detector,
    /// the partition heals, all three reconverge to `Healthy` via the
    /// probe path, and a remote message then delivers across the former
    /// partition boundary.
    #[test]
    fn test_dst_cluster_partition_detects_heals_and_delivers() {
        let mut cluster = DeterministicCluster::new(&[addr(9121), addr(9122), addr(9123)], 7);
        let a = cluster.id(0);
        let b = cluster.id(1);
        let c = cluster.id(2);
        let counter = spawn_counter(&mut cluster.node_mut(2)); // actor on C

        // Phase 1: converge. Active views fill only through the repair
        // cycle (~5 s virtual = 50 rounds), so run plenty of rounds.
        cluster.run_rounds(80);
        assert!(
            cluster.active_views_converged(),
            "cluster must converge before the partition (round {})",
            cluster.round
        );
        for (viewer, peer) in [(0, b), (0, c), (1, a), (1, c), (2, a), (2, b)] {
            assert_eq!(
                cluster.cluster_status(viewer, peer),
                Some(NodeStatus::Healthy),
                "node {viewer} must see node {} healthy before partition",
                peer.0
            );
        }

        // Phase 2: partition C away from {A, B}.
        cluster.partition(0, 2); // A -> C dropped
        cluster.partition(2, 0); // C -> A dropped
        cluster.partition(1, 2); // B -> C dropped
        cluster.partition(2, 1); // C -> B dropped
                                 // Failure needs 2 s (Suspicious) + 5 s (Failed) of silence = 70
                                 // rounds; the last heartbeat could have landed up to 500 ms into
                                 // the partition, so 120 rounds gives comfortable headroom.
        cluster.run_rounds(120);
        assert_eq!(
            cluster.cluster_status(0, c),
            Some(NodeStatus::Failed),
            "A must mark C failed"
        );
        assert_eq!(
            cluster.cluster_status(1, c),
            Some(NodeStatus::Failed),
            "B must mark C failed"
        );
        assert_eq!(
            cluster.cluster_status(2, a),
            Some(NodeStatus::Failed),
            "C must mark A failed"
        );
        assert_eq!(
            cluster.cluster_status(2, b),
            Some(NodeStatus::Failed),
            "C must mark B failed"
        );
        // The majority sub-cluster stays internally healthy.
        assert_eq!(
            cluster.cluster_status(0, b),
            Some(NodeStatus::Healthy),
            "A and B stay healthy through the partition"
        );

        // Phase 3: heal the partition. Probes re-promote via the
        // heartbeat-reply path (probe interval 5 s = 50 rounds); gossip
        // reconverges membership.
        cluster.heal(0);
        cluster.heal(1);
        cluster.heal(2);
        cluster.run_rounds(120);
        for (viewer, peer) in [(0, b), (0, c), (1, a), (1, c), (2, a), (2, b)] {
            assert_eq!(
                cluster.cluster_status(viewer, peer),
                Some(NodeStatus::Healthy),
                "node {viewer} must reconverge to healthy with node {} after healing",
                peer.0
            );
        }

        // Phase 4: real work across the former boundary — A sends to the
        // actor on C.
        cluster.send_remote(0, 2, counter, "inc", &[Value::int(99)]);
        cluster.run_rounds(20);
        let count = counter_value(&cluster, 2, counter);
        assert_eq!(
            count, 99,
            "remote message must deliver across the healed boundary"
        );
    }

    /// PLAN.md Phase 1 bullet 2 (DST): cross-shard determinism. In sharded
    /// mode (`new_sharded`), messages route through the cross-shard
    /// channels; `run_scheduler_deterministic` must drain them (the
    /// production scheduler does) so sharded runs are deterministic too.
    /// Sweep seeds: every run delivers exactly `MESSAGES` increments to
    /// the actor on the owning shard and Quiesces.
    #[test]
    fn test_dst_cross_shard_delivery_seed_sweep() {
        const MESSAGES: i64 = 25;
        let seeds = crate::dst::dst_seed_count(30);

        for seed in 0..seeds {
            let mut shards = Runtime::new_sharded(2);
            assert_eq!(shards.len(), 2);
            // Actor ids come from a process-global counter, so spawn on
            // shard 1 until the fresh id's parity routes there too
            // (`target % shard_count == 1`): a spawn lands on the calling
            // shard, and cross-shard sends route by `id % shard_count`, so
            // the actor must sit on its routing shard or messages go to
            // the wrong shard's DLQ. Parity alternates per spawn, so the
            // loop terminates within 2 iterations barring interference
            // from parallel tests.
            let mut target =
                shards[1].spawn_actor(Box::new(|| vec![("count".to_string(), Value::int(0))]));
            while target % 2 != 1 {
                target =
                    shards[1].spawn_actor(Box::new(|| vec![("count".to_string(), Value::int(0))]));
            }
            {
                let actor = shards[1].actors.get_mut(&target).unwrap();
                actor.register_behavior("inc", |actor, args| {
                    let n = actor
                        .get_state_field("count")
                        .and_then(|v| v.as_int())
                        .unwrap_or(0);
                    let by = args.get(0).and_then(|v| v.as_int()).unwrap_or(0);
                    actor.set_state_field("count", Value::int(n + by));
                });
            }
            // Shard 0 sends to the actor on shard 1: cross-shard routing.
            for _ in 0..MESSAGES {
                shards[0].send_message(target, "inc", &[Value::int(1)]);
            }
            // Drive both shards deterministically, interleaving from one
            // seeded stream.
            let mut rng = DeterministicRng::new(seed);
            let mut steps = 0u64;
            loop {
                if steps >= 100_000 {
                    panic!("seed {seed}: step limit exceeded — possible deadlock");
                }
                let quiescent = shards.iter_mut().all(|shard| {
                    matches!(
                        shard.run_scheduler_deterministic_with_rng(&mut rng, 100),
                        crate::runtime::DeterministicRunResult::Quiescent { .. }
                    )
                });
                steps += 1;
                if quiescent {
                    break;
                }
            }
            let count = shards[1]
                .actors
                .get(&target)
                .and_then(|a| a.get_state_field("count"))
                .and_then(|v| v.as_int())
                .unwrap_or(-1);
            assert_eq!(
                count, MESSAGES,
                "seed {seed}: cross-shard counter must reach exactly {MESSAGES}, got {count}"
            );
        }
    }
}
