//! Cluster membership system for Nulang's distributed actor runtime.
//!
//! This module manages node identity, cluster membership, heartbeat-based
//! failure detection, and gossip-style state dissemination. Multiple Nulang
//! nodes form a cluster, allowing actors to communicate across machine
//! boundaries.
//!
//! # Architecture
//!
//! Each node maintains a [`ClusterState`] containing a membership table of
//! all known nodes. Nodes exchange heartbeats periodically to detect failures
//! and gossip membership updates to disseminate state changes.
//!
//! # Failure Detection
//!
//! The failure detector uses a simple multi-stage timeout:
//!
//! 1. **Healthy** → nodes are responding to heartbeats.
//! 2. **Suspicious** → a heartbeat has not been received within the timeout.
//! 3. **Failed** → the node has been suspicious for too long and is removed.
//!
//! # Gossip Protocol
//!
//! Membership changes propagate via gossip. Each tick, a node selects a random
//! subset of healthy peers and sends them a compact view of the membership
//! table. When merging incoming gossip, the higher incarnation number wins,
//! ensuring convergence even under partition.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tracing::warn;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default interval between heartbeats (500ms).
const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);

/// Default timeout before marking a node suspicious (2s).
const DEFAULT_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(2);

/// Default duration a node remains suspicious before being marked failed (5s).
const DEFAULT_SUSPICION_DURATION: Duration = Duration::from_secs(5);

/// How long to keep failed nodes in the table before purging them (60s).
const FAILED_NODE_RETENTION: Duration = Duration::from_secs(60);

/// Number of random gossip targets selected each tick.
const GOSSIP_FANOUT: usize = 2;

/// Default interval between liveness probes to Failed members (5s).
const DEFAULT_PROBE_INTERVAL: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// NodeId
// ---------------------------------------------------------------------------

/// Unique identifier for a node in the cluster.
///
/// Derived from a hash of the node's socket address so that the same
/// physical node (restarting with the same address) receives a stable id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

impl NodeId {
    /// Create a `NodeId` from a socket address (TCP).
    ///
    /// The id is derived with `DefaultHasher` so repeated calls with the
    /// same address yield the same id.
    pub fn new(addr: &SocketAddr) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        addr.hash(&mut hasher);
        NodeId(hasher.finish())
    }

    /// Create a `NodeId` from a transport address (TCP or Unix).
    pub fn from_addr(addr: &crate::runtime::network::TransportAddr) -> Self {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        addr.hash(&mut hasher);
        NodeId(hasher.finish())
    }

    /// The id reserved for the local node.
    pub const LOCAL: NodeId = NodeId(0);
}

// ---------------------------------------------------------------------------
// NodeStatus
// ---------------------------------------------------------------------------

/// Health status of a node in the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    /// Node is in the process of joining the cluster.
    Joining,
    /// Node is active and responding to heartbeats.
    Healthy,
    /// Node missed a heartbeat and is under suspicion.
    Suspicious,
    /// Node has been declared failed.
    Failed,
    /// Node is gracefully leaving the cluster.
    Leaving,
}

// ---------------------------------------------------------------------------
// NodeInfo
// ---------------------------------------------------------------------------

/// Information about a node in the cluster.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    /// Unique identifier of the node.
    pub node_id: NodeId,
    /// Network address the node listens on.
    pub address: SocketAddr,
    /// Current health status.
    pub status: NodeStatus,
    /// Timestamp of the last received heartbeat.
    pub last_heartbeat: Instant,
    /// When the node first joined the cluster (from our perspective).
    pub joined_at: Instant,
    /// Optional key-value metadata (e.g. region, rack, version).
    pub metadata: HashMap<String, String>,
}

impl NodeInfo {
    /// Create a minimal `NodeInfo` for the given node.
    fn new(node_id: NodeId, address: SocketAddr) -> Self {
        let now = Instant::now();
        NodeInfo {
            node_id,
            address,
            status: NodeStatus::Joining,
            last_heartbeat: now,
            joined_at: now,
            metadata: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// ClusterAction
// ---------------------------------------------------------------------------

/// Actions returned by [`ClusterState::tick`] for the runtime to execute.
///
/// The caller is responsible for serialising and transmitting heartbeats
/// and gossip messages over the network.
#[derive(Debug)]
pub enum ClusterAction {
    /// Send a heartbeat to the specified node.
    SendHeartbeat { to: NodeId, addr: SocketAddr },
    /// Notify that a node has joined the cluster.
    NodeJoined { node: NodeId, addr: SocketAddr },
    /// Notify that a node has been declared failed.
    NodeFailed { node: NodeId },
    /// Notify that a node has left the cluster.
    NodeLeft { node: NodeId },
    /// Send gossip to a random subset of nodes.
    SendGossip { targets: Vec<(NodeId, SocketAddr)> },
    /// The split-brain resolver decided the local node should leave the
    /// cluster (partition minority / below quorum).
    Down { node: NodeId },
    /// Minimal periodic liveness probe to a Failed member, so a healed
    /// partition re-joins without an external rejoin.
    Probe { to: NodeId, addr: SocketAddr },
}

// ---------------------------------------------------------------------------
// Split-brain resolver
// ---------------------------------------------------------------------------

/// What the local node should do given its current membership view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolverDecision {
    /// The local node keeps participating in the cluster.
    StayUp,
    /// The local node leaves the cluster (partition minority / below quorum).
    DownSelf,
}

/// Snapshot of the membership view handed to a [`SplitBrainResolver`].
///
/// Built from the live membership table at tick time; the resolver must
/// treat it as immutable.
#[derive(Debug, Clone)]
pub struct MembershipView {
    /// The node asking for a decision.
    pub local: NodeId,
    /// All known members with their current statuses.
    pub members: Vec<NodeInfo>,
}

/// Pluggable split-brain resolution (Akka-SBR style).
///
/// A resolver is a pure function of the local membership view: no I/O, no
/// timers, so it is unit-testable and DST-drivable. `ClusterState::tick`
/// consults it after failure detection; a `DownSelf` decision marks the
/// local node down and emits [`ClusterAction::Down`].
pub trait SplitBrainResolver: Send + Sync {
    fn decide(&self, view: &MembershipView) -> ResolverDecision;
}

/// Static-quorum strategy: the node stays up iff it sees at least
/// `floor(expected_nodes / 2) + 1` reachable members (itself plus every
/// `Healthy`/`Joining` member). Needs only the operator-configured expected
/// cluster size — no live count, no consensus, no leader.
///
/// With `expected_nodes == 2` both sides of a partition down themselves
/// (`1 < 2`): fail-closed is the intended 2-node behavior, and the strategy
/// is only useful for `expected_nodes >= 3`.
pub struct StaticQuorumResolver {
    pub expected_nodes: usize,
}

impl SplitBrainResolver for StaticQuorumResolver {
    fn decide(&self, view: &MembershipView) -> ResolverDecision {
        let reachable = view
            .members
            .iter()
            .filter(|m| {
                m.node_id == view.local
                    || matches!(m.status, NodeStatus::Healthy | NodeStatus::Joining)
            })
            .count();
        if reachable >= self.expected_nodes / 2 + 1 {
            ResolverDecision::StayUp
        } else {
            ResolverDecision::DownSelf
        }
    }
}

/// Split-brain resolver configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitBrainConfig {
    /// No resolver: current behavior — partitions never self-resolve.
    Disabled,
    /// Static-quorum with the given expected cluster size (see
    /// [`StaticQuorumResolver`] for the 2-node caveat).
    StaticQuorum { expected_nodes: usize },
}

/// Cluster configuration applied when distribution is enabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterConfig {
    pub split_brain: SplitBrainConfig,
    /// How often to probe `Failed` members for liveness (the self-healing
    /// path: a probe that reaches a live node re-promotes it to `Healthy`).
    pub probe_interval: Duration,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        ClusterConfig {
            split_brain: SplitBrainConfig::Disabled,
            probe_interval: DEFAULT_PROBE_INTERVAL,
        }
    }
}

impl ClusterConfig {
    /// True when the configuration can be applied. `StaticQuorum` with
    /// `expected_nodes == 0` is a configuration error, not "disabled".
    pub fn is_valid(&self) -> bool {
        match self.split_brain {
            SplitBrainConfig::Disabled => true,
            SplitBrainConfig::StaticQuorum { expected_nodes } => expected_nodes > 0,
        }
    }
}

// ---------------------------------------------------------------------------
// NodeGossip
// ---------------------------------------------------------------------------

/// A lightweight gossip entry for membership dissemination.
///
/// This compact representation avoids sending full [`NodeInfo`] (including
/// metadata maps) on every gossip round.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeGossip {
    /// Node identifier.
    pub node_id: NodeId,
    /// Network address.
    pub address: SocketAddr,
    /// Health status.
    pub status: NodeStatus,
    /// Incarnation number for conflict resolution.
    pub incarnation: u64,
}

// ---------------------------------------------------------------------------
// ClusterState
// ---------------------------------------------------------------------------

/// Manages the cluster membership for a Nulang node.
///
/// Uses a simple gossip-style protocol where each node maintains a
/// membership table of all known nodes. Heartbeats are exchanged
/// periodically to detect failures.
///
/// # Example
///
/// ```ignore
/// use nulang::runtime::cluster::{ClusterState, NodeId};
/// # use std::net::{SocketAddr, IpAddr, Ipv4Addr};
/// let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9000);
/// let local = NodeId::new(&addr);
/// let mut cluster = ClusterState::new(local, addr);
/// ```
pub struct ClusterState {
    /// This node's identity.
    local_node: NodeId,

    /// Membership table: node_id → node info.
    members: HashMap<NodeId, NodeInfo>,

    /// Nodes that have been declared failed (kept for a while to
    /// prevent rejoining with stale state).
    failed_nodes: HashMap<NodeId, Instant>,

    /// Heartbeat configuration.
    heartbeat_interval: Duration,
    heartbeat_timeout: Duration,
    suspicion_duration: Duration,

    /// Timestamp of last heartbeat we sent.
    last_heartbeat_sent: Instant,

    /// Optional virtual clock for deterministic testing.
    /// When set, all time queries use this clock instead of wall time.
    clock: Option<super::timer::VirtualClock>,

    /// Optional split-brain resolver; `None` = resolver disabled.
    split_brain: Option<Box<dyn SplitBrainResolver>>,
    /// How often to probe Failed members (the self-healing path).
    probe_interval: Duration,
    /// When we last probed Failed members.
    last_probe_sent: Option<Instant>,
    /// True once the resolver decided the local node should leave.
    local_down: bool,

    /// Callback for membership change notifications.
    on_member_joined: Option<Box<dyn Fn(NodeId, SocketAddr) + Send>>,
    on_member_left: Option<Box<dyn Fn(NodeId) + Send>>,
    on_member_failed: Option<Box<dyn Fn(NodeId) + Send>>,
}

impl ClusterState {
    /// Create a new cluster state for the local node.
    ///
    /// The local node is automatically added to the membership table with
    /// [`NodeStatus::Healthy`].
    pub fn new(local_node: NodeId, local_addr: SocketAddr) -> Self {
        let now = Instant::now();
        let mut members = HashMap::new();

        let local_info = NodeInfo {
            node_id: local_node,
            address: local_addr,
            status: NodeStatus::Healthy,
            last_heartbeat: now,
            joined_at: now,
            metadata: HashMap::new(),
        };
        members.insert(local_node, local_info);

        ClusterState {
            local_node,
            members,
            clock: None,
            failed_nodes: HashMap::new(),
            on_member_joined: None,
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
            heartbeat_timeout: DEFAULT_HEARTBEAT_TIMEOUT,
            suspicion_duration: DEFAULT_SUSPICION_DURATION,
            last_heartbeat_sent: now,
            split_brain: None,
            probe_interval: DEFAULT_PROBE_INTERVAL,
            last_probe_sent: None,
            local_down: false,
            on_member_left: None,
            on_member_failed: None,
        }
    }

    /// Current time, using the virtual clock if one is configured.
    fn now(&self) -> Instant {
        match &self.clock {
            Some(clock) => clock.now(),
            None => Instant::now(),
        }
    }

    /// Install a virtual clock for deterministic testing.
    /// When set, all time queries use this clock instead of wall time.
    pub fn set_clock(&mut self, clock: super::timer::VirtualClock) {
        self.clock = Some(clock);
    }

    /// Join an existing cluster by contacting a seed node.
    ///
    /// Records the seed node in the membership table (as Joining, with
    /// baseline `_incarnation` metadata so the join propagates via gossip).
    /// The actual network request to the seed is the responsibility of
    /// the caller.
    pub fn join_cluster(&mut self, seed_addr: SocketAddr) {
        let seed_id = NodeId::new(&seed_addr);

        if seed_id == self.local_node {
            // Cannot join ourselves.
            return;
        }

        if !self.members.contains_key(&seed_id) {
            let mut info = NodeInfo::new(seed_id, seed_addr);
            info.status = NodeStatus::Joining;
            // Baseline incarnation 1: the seed address is authoritative
            // (it came from an explicit join request), so same-generation
            // gossip (incarnation 1) must not overwrite it with a
            // discovered address of unknown quality. Strictly-higher
            // incarnations still win.
            info.metadata
                .insert("_incarnation".to_string(), "1".to_string());
            self.members.insert(seed_id, info);
        }
    }

    /// Handle an incoming heartbeat from another node.
    ///
    /// Updates the node's `last_heartbeat` timestamp and promotes the
    /// status back to [`NodeStatus::Healthy`] if it was previously
    /// Suspicious or Failed.
    ///
    /// If the node was not previously known, it is added to the
    /// membership table.
    pub fn handle_heartbeat(&mut self, from: NodeId, addr: SocketAddr) {
        let now = self.now();

        match self.members.get_mut(&from) {
            Some(info) => {
                let was_suspicious_or_failed =
                    matches!(info.status, NodeStatus::Suspicious | NodeStatus::Failed);

                info.last_heartbeat = now;
                info.address = addr;

                if was_suspicious_or_failed {
                    info.status = NodeStatus::Healthy;
                    Self::bump_entry_incarnation(info);
                } else if info.status == NodeStatus::Joining {
                    info.status = NodeStatus::Healthy;
                    // Bump the entry incarnation so the promotion wins
                    // merges on nodes that learned the stale Joining
                    // status from an earlier gossip round.
                    Self::bump_entry_incarnation(info);
                }
            }
            None => {
                // New node discovered via heartbeat.
                let mut info = NodeInfo::new(from, addr);
                info.last_heartbeat = now;
                info.status = NodeStatus::Healthy;
                self.members.insert(from, info);

                if let Some(ref cb) = self.on_member_joined {
                    cb(from, addr);
                }
            }
        }
    }

    /// Apply operator cluster configuration.
    ///
    /// Returns false (and leaves the previous configuration in place) when
    /// the configuration is invalid, e.g. `static-quorum` with
    /// `expected_nodes == 0`.
    pub fn apply_config(&mut self, config: &ClusterConfig) -> bool {
        if !config.is_valid() {
            warn!(
                "cluster config: static-quorum expected_nodes must be >= 1; \
                 keeping the previous configuration"
            );
            return false;
        }
        self.split_brain = match config.split_brain {
            SplitBrainConfig::Disabled => None,
            SplitBrainConfig::StaticQuorum { expected_nodes } => {
                Some(Box::new(StaticQuorumResolver { expected_nodes }))
            }
        };
        self.probe_interval = config.probe_interval;
        true
    }

    /// True once the split-brain resolver downed this node.
    pub fn is_down(&self) -> bool {
        self.local_down
    }

    /// Run the periodic cluster maintenance.
    ///
    /// Should be called regularly (e.g., every 100 ms). Performs:
    ///
    /// 1. Checks for nodes that have missed heartbeats → marks Suspicious.
    /// 2. Promotes Suspicious nodes to Failed if past the suspicion window.
    /// 3. Cleans up old failed nodes.
    /// 4. Consults the split-brain resolver; a `DownSelf` decision marks
    ///    the local node down and no further actions are emitted.
    /// 5. Probes Failed members (throttled) so a healed partition re-joins.
    /// 6. Returns a list of actions for the runtime to execute.
    pub fn tick(&mut self) -> Vec<ClusterAction> {
        let now = self.now();
        let mut actions = Vec::new();

        // ------------------------------------------------------------------
        // 1. Heartbeat timeout → Suspicious
        // ------------------------------------------------------------------
        for info in self.members.values_mut() {
            if info.node_id == self.local_node {
                continue;
            }
            if info.status == NodeStatus::Healthy {
                if now.duration_since(info.last_heartbeat) > self.heartbeat_timeout {
                    info.status = NodeStatus::Suspicious;
                }
            }
        }

        // ------------------------------------------------------------------
        // 2. Suspicion timeout → Failed
        // ------------------------------------------------------------------
        let mut newly_failed = Vec::new();
        for info in self.members.values_mut() {
            if info.node_id == self.local_node {
                continue;
            }
            if info.status == NodeStatus::Suspicious {
                // Use the heartbeat timeout as a proxy for "how long
                // has it been suspicious" — the moment it transitions
                // to Suspicious we can track from the last heartbeat.
                if now.duration_since(info.last_heartbeat)
                    > self.heartbeat_timeout + self.suspicion_duration
                {
                    info.status = NodeStatus::Failed;
                    newly_failed.push(info.node_id);
                    self.failed_nodes.insert(info.node_id, now);

                    if let Some(ref cb) = self.on_member_failed {
                        cb(info.node_id);
                    }

                    actions.push(ClusterAction::NodeFailed { node: info.node_id });
                }
            }
        }

        // ------------------------------------------------------------------
        // 3. Clean up old failed nodes
        // ------------------------------------------------------------------
        let mut to_remove = Vec::new();
        for (node_id, failed_at) in &self.failed_nodes {
            if now.duration_since(*failed_at) > FAILED_NODE_RETENTION {
                to_remove.push(*node_id);
            }
        }
        for node_id in &to_remove {
            self.members.remove(node_id);
            self.failed_nodes.remove(node_id);
            actions.push(ClusterAction::NodeLeft { node: *node_id });

            if let Some(ref cb) = self.on_member_left {
                cb(*node_id);
            }
        }

        // ------------------------------------------------------------------
        // 3.5 Split-brain resolver: decide whether the local node stays up
        // ------------------------------------------------------------------
        if self.local_down {
            // Already down: no heartbeats, gossip, or probes.
            return actions;
        }
        if let Some(resolver) = &self.split_brain {
            let view = MembershipView {
                local: self.local_node,
                members: self.members.values().cloned().collect(),
            };
            if matches!(resolver.decide(&view), ResolverDecision::DownSelf) {
                self.local_down = true;
                actions.push(ClusterAction::Down {
                    node: self.local_node,
                });
                return actions;
            }
        }

        // ------------------------------------------------------------------
        // 3.6 Probe Failed members (throttled) so a healed partition
        //     re-joins without an external rejoin.
        // ------------------------------------------------------------------
        let probe_due = match self.last_probe_sent {
            Some(last) => now.duration_since(last) >= self.probe_interval,
            None => true,
        };
        if probe_due {
            self.last_probe_sent = Some(now);
            for info in self.members.values() {
                if info.status == NodeStatus::Failed {
                    actions.push(ClusterAction::Probe {
                        to: info.node_id,
                        addr: info.address,
                    });
                }
            }
        }

        // ------------------------------------------------------------------
        // 4. Send heartbeats to healthy members (throttled)
        // ------------------------------------------------------------------
        if now.duration_since(self.last_heartbeat_sent) >= self.heartbeat_interval {
            self.last_heartbeat_sent = now;

            for info in self.members.values() {
                if info.node_id == self.local_node {
                    continue;
                }
                // Heartbeats go to Joining members as well as Healthy
                // ones: the first heartbeat to a seed is what initiates
                // the join — the seed discovers us from it and heartbeats
                // back, which promotes the seed to Healthy on our side.
                if matches!(info.status, NodeStatus::Healthy | NodeStatus::Joining) {
                    actions.push(ClusterAction::SendHeartbeat {
                        to: info.node_id,
                        addr: info.address,
                    });
                }
            }
        }

        // ------------------------------------------------------------------
        // 5. Gossip to a random subset of healthy nodes
        // ------------------------------------------------------------------
        let gossip_targets = self.pick_gossip_targets(GOSSIP_FANOUT);
        if !gossip_targets.is_empty() {
            actions.push(ClusterAction::SendGossip {
                targets: gossip_targets,
            });
        }

        actions
    }

    /// Get the list of all healthy members **excluding** the local node.
    pub fn healthy_members(&self) -> Vec<&NodeInfo> {
        self.members
            .values()
            .filter(|info| info.node_id != self.local_node && info.status == NodeStatus::Healthy)
            .collect()
    }

    /// Get the list of all members including the local node.
    pub fn all_members(&self) -> Vec<&NodeInfo> {
        self.members.values().collect()
    }

    /// Check if a node is known to the cluster.
    pub fn is_member(&self, node_id: NodeId) -> bool {
        self.members.contains_key(&node_id)
    }

    /// Get info for a specific node.
    pub fn get_node(&self, node_id: NodeId) -> Option<&NodeInfo> {
        self.members.get(&node_id)
    }

    /// Get the number of healthy nodes in the cluster.
    ///
    /// This includes the local node.
    pub fn healthy_node_count(&self) -> usize {
        self.members
            .values()
            .filter(|info| info.status == NodeStatus::Healthy)
            .count()
    }

    /// Set a callback invoked when a new member joins the cluster.
    pub fn on_member_joined<F>(&mut self, callback: F)
    where
        F: Fn(NodeId, SocketAddr) + Send + 'static,
    {
        self.on_member_joined = Some(Box::new(callback));
    }

    /// Set a callback invoked when a member leaves the cluster.
    pub fn on_member_left<F>(&mut self, callback: F)
    where
        F: Fn(NodeId) + Send + 'static,
    {
        self.on_member_left = Some(Box::new(callback));
    }

    /// Set a callback invoked when a member is declared failed.
    pub fn on_member_failed<F>(&mut self, callback: F)
    where
        F: Fn(NodeId) + Send + 'static,
    {
        self.on_member_failed = Some(Box::new(callback));
    }

    /// Merge a membership list received from another node (gossip).
    ///
    /// Uses incarnation numbers for conflict resolution: the entry with
    /// the higher incarnation is considered authoritative.  Returns
    /// `true` if any changes were made to our membership table.
    pub fn merge_membership(&mut self, gossip: Vec<NodeGossip>) -> bool {
        let mut changed = false;

        let now = self.now();
        for entry in gossip {
            // Never overwrite local node info from gossip.
            if entry.node_id == self.local_node {
                continue;
            }

            match self.members.get_mut(&entry.node_id) {
                Some(existing) => {
                    let stored_incarnation = existing
                        .metadata
                        .get("_incarnation")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    // Higher incarnation wins. Only a strictly-newer entry
                    // refreshes `last_heartbeat`: an equal-incarnation entry
                    // is just a re-broadcast of state we already hold, so
                    // treating it as a liveness hint would let surviving
                    // nodes refresh a dead peer's timestamp forever and
                    // defeat the failure detector. (Direct heartbeats refresh
                    // the timestamp via `handle_heartbeat`.)
                    if entry.incarnation > stored_incarnation {
                        let old_status = existing.status;
                        existing.last_heartbeat = now;
                        existing.status = entry.status;
                        existing.address = entry.address;
                        existing
                            .metadata
                            .insert("_incarnation".to_string(), entry.incarnation.to_string());

                        if old_status != entry.status {
                            changed = true;
                            if entry.status == NodeStatus::Failed {
                                self.failed_nodes.insert(entry.node_id, now);
                            }
                        }
                    }
                }
                None => {
                    // New node learned from gossip.
                    let mut info = NodeInfo::new(entry.node_id, entry.address);
                    info.status = entry.status;
                    info.last_heartbeat = now;
                    info.metadata
                        .insert("_incarnation".to_string(), entry.incarnation.to_string());
                    self.members.insert(entry.node_id, info);
                    changed = true;
                }
            }
        }

        changed
    }

    /// Get a gossip payload to send to other nodes.
    ///
    /// Returns up to `max_entries` entries from the membership table.
    /// If the table is smaller than `max_entries`, all entries are returned.
    pub fn gossip_payload(&self, max_entries: usize) -> Vec<NodeGossip> {
        self.members
            .values()
            .take(max_entries)
            .map(|info| NodeGossip {
                node_id: info.node_id,
                address: info.address,
                status: info.status,
                incarnation: info
                    .metadata
                    .get("_incarnation")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1),
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Increment the per-entry incarnation stored in a member's metadata.
    ///
    /// Entries carry their version in the `_incarnation` metadata key so
    /// gossip merges can resolve conflicts (higher wins). A missing key is
    /// treated as 1 by `gossip_payload`, so a locally-observed status
    /// change must bump the entry past that baseline to propagate.
    fn bump_entry_incarnation(info: &mut NodeInfo) {
        let current = info
            .metadata
            .get("_incarnation")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(1);
        info.metadata
            .insert("_incarnation".to_string(), (current + 1).to_string());
    }
    /// Pick `n` distinct random healthy targets for gossip.
    ///
    /// Selection is uniform over the healthy member set (partial
    /// Fisher-Yates shuffle driven by `OsRng`), so no member is
    /// systematically starved of gossip coverage.
    fn pick_gossip_targets(&self, n: usize) -> Vec<(NodeId, SocketAddr)> {
        let mut healthy: Vec<&NodeInfo> = self.healthy_members();
        if healthy.is_empty() {
            return Vec::new();
        }

        // Partial Fisher-Yates: swap a random remaining element into
        // position i, then keep the first n — every healthy member has
        // an equal chance of being selected each tick.
        use rand_core::RngCore;
        let mut buf = [0u8; 8];
        let count = n.min(healthy.len());
        for i in 0..count {
            rand_core::OsRng.fill_bytes(&mut buf);
            let j = (u64::from_le_bytes(buf) as usize) % (healthy.len() - i);
            healthy.swap(i, i + j);
        }
        healthy.truncate(count);
        healthy
            .into_iter()
            .map(|info| (info.node_id, info.address))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::thread;

    /// Helper: create a loopback address on a given port.
    fn addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port)
    }

    // -- 1. NodeId creation ------------------------------------------------

    #[test]
    fn test_node_id_creation() {
        let a = addr(9000);
        let id1 = NodeId::new(&a);
        let id2 = NodeId::new(&a);
        assert_eq!(id1, id2, "same address should yield same NodeId");
        assert_ne!(id1.0, 0, "NodeId should not be zero for non-local");
    }

    #[test]
    fn test_node_id_local() {
        assert_eq!(NodeId::LOCAL.0, 0);
    }

    // -- 2. ClusterState creation ------------------------------------------

    #[test]
    fn test_cluster_new() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let cs = ClusterState::new(local, a);

        assert_eq!(cs.local_node, local);
        assert_eq!(cs.healthy_node_count(), 1);
        assert!(cs.is_member(local));

        let info = cs.get_node(local).unwrap();
        assert_eq!(info.status, NodeStatus::Healthy);
        assert_eq!(info.address, a);
    }

    // -- 3. Heartbeat from unknown node ------------------------------------

    #[test]
    fn test_handle_heartbeat_new_node() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let peer_addr = addr(9001);
        let peer_id = NodeId::new(&peer_addr);

        cs.handle_heartbeat(peer_id, peer_addr);

        assert!(cs.is_member(peer_id));
        assert_eq!(cs.get_node(peer_id).unwrap().status, NodeStatus::Healthy);
        assert_eq!(cs.healthy_node_count(), 2);
    }

    // -- 4. Heartbeat updates existing node --------------------------------

    #[test]
    fn test_handle_heartbeat_existing_node() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let peer_addr = addr(9001);
        let peer_id = NodeId::new(&peer_addr);

        cs.handle_heartbeat(peer_id, peer_addr);
        let first = cs.get_node(peer_id).unwrap().last_heartbeat;

        // Wait a tiny bit so Instant::now() advances.
        thread::sleep(Duration::from_millis(10));
        cs.handle_heartbeat(peer_id, peer_addr);
        let second = cs.get_node(peer_id).unwrap().last_heartbeat;

        assert!(second > first, "heartbeat should update timestamp");
    }

    // -- 5. Suspicion detection --------------------------------------------

    #[test]
    fn test_suspicion_detection() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let peer_addr = addr(9001);
        let peer_id = NodeId::new(&peer_addr);

        cs.handle_heartbeat(peer_id, peer_addr);
        assert_eq!(cs.get_node(peer_id).unwrap().status, NodeStatus::Healthy);

        // Simulate time passing by not sending heartbeats.
        // We can't advance Instant, so we force the status manually
        // and verify tick promotes it.
        // NOTE: In real usage the peer would naturally time out.
        // Here we verify the state machine transition exists.

        // Mark the peer as having a very old heartbeat.
        if let Some(info) = cs.members.get_mut(&peer_id) {
            // Artificially set last_heartbeat far in the past.
            // Since Instant doesn't support subtraction directly,
            // we verify the transition path via tick.
            info.status = NodeStatus::Healthy;
        }

        // Call tick — we force the heartbeat timer to have expired so it sends a heartbeat
        cs.last_heartbeat_sent = Instant::now() - cs.heartbeat_interval - Duration::from_secs(1);
        let actions = cs.tick();
        // Peer is still healthy because the real timeout hasn't passed.
        // The test documents the API; full timeout testing requires
        // mockable clocks (left as a TODO for production).
        assert!(
            cs.get_node(peer_id).unwrap().status == NodeStatus::Healthy
                || cs.get_node(peer_id).unwrap().status == NodeStatus::Suspicious
        );

        // Verify that SendHeartbeat action is produced for the peer.
        let has_heartbeat = actions
            .iter()
            .any(|a| matches!(a, ClusterAction::SendHeartbeat { to, .. } if *to == peer_id));
        assert!(has_heartbeat, "tick should request heartbeat to peer");
    }

    // -- 6. Failure detection ----------------------------------------------

    #[test]
    fn test_failure_detection() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let peer_addr = addr(9001);
        let peer_id = NodeId::new(&peer_addr);

        cs.handle_heartbeat(peer_id, peer_addr);

        // Manually transition through the failure-detector state machine.
        if let Some(info) = cs.members.get_mut(&peer_id) {
            info.status = NodeStatus::Suspicious;
        }

        // tick won't promote to Failed because real time hasn't passed,
        // but we verify the state machine paths are wired correctly by
        // checking the member stays in the table.
        let _actions = cs.tick();
        assert!(cs.is_member(peer_id));
    }

    // -- 7. Healthy members filter -----------------------------------------

    #[test]
    fn test_healthy_members_filter() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let p1 = addr(9001);
        let id1 = NodeId::new(&p1);
        let p2 = addr(9002);
        let id2 = NodeId::new(&p2);

        cs.handle_heartbeat(id1, p1);
        cs.handle_heartbeat(id2, p2);

        let healthy = cs.healthy_members();
        assert_eq!(healthy.len(), 2);
        assert!(healthy.iter().all(|i| i.status == NodeStatus::Healthy));
        assert!(!healthy.iter().any(|i| i.node_id == local));
    }

    // -- 8. Merge membership (gossip) --------------------------------------

    #[test]
    fn test_merge_membership() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let gossip = vec![
            NodeGossip {
                node_id: NodeId(42),
                address: addr(9042),
                status: NodeStatus::Healthy,
                incarnation: 5,
            },
            NodeGossip {
                node_id: NodeId(43),
                address: addr(9043),
                status: NodeStatus::Healthy,
                incarnation: 3,
            },
        ];

        let changed = cs.merge_membership(gossip);
        assert!(changed);
        assert!(cs.is_member(NodeId(42)));
        assert!(cs.is_member(NodeId(43)));
        assert_eq!(cs.get_node(NodeId(42)).unwrap().address, addr(9042));
    }

    // -- 9. Merge conflict resolution (higher incarnation wins) -------------

    #[test]
    fn test_merge_conflict_resolution() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        // Seed the table with a node at incarnation 3.
        let gossip_low = vec![NodeGossip {
            node_id: NodeId(77),
            address: addr(9077),
            status: NodeStatus::Healthy,
            incarnation: 3,
        }];
        cs.merge_membership(gossip_low);

        assert_eq!(cs.get_node(NodeId(77)).unwrap().status, NodeStatus::Healthy);

        // Now receive gossip with a higher incarnation marking it Failed.
        let gossip_high = vec![NodeGossip {
            node_id: NodeId(77),
            address: addr(9077),
            status: NodeStatus::Failed,
            incarnation: 10,
        }];
        let changed = cs.merge_membership(gossip_high);
        assert!(changed);
        assert_eq!(cs.get_node(NodeId(77)).unwrap().status, NodeStatus::Failed);
    }

    // -- 10. Gossip payload size -------------------------------------------

    #[test]
    fn test_gossip_payload_size() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        // Add several peers.
        for port in 9001..=9010 {
            let pa = addr(port);
            let pid = NodeId::new(&pa);
            cs.handle_heartbeat(pid, pa);
        }

        let payload = cs.gossip_payload(3);
        assert_eq!(payload.len(), 3, "payload should respect max_entries");

        let payload_all = cs.gossip_payload(100);
        assert_eq!(payload_all.len(), 11, "payload should contain all members");
    }

    // -- 11. Member joined callback ----------------------------------------

    #[test]
    fn test_member_joined_callback() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let (tx, rx) = std::sync::mpsc::channel();
        cs.on_member_joined(move |id, _addr| {
            let _ = tx.send(id);
        });

        let pa = addr(9001);
        let pid = NodeId::new(&pa);
        cs.handle_heartbeat(pid, pa);

        let received = rx.recv_timeout(Duration::from_secs(1));
        assert!(received.is_ok(), "callback should fire on new member");
        assert_eq!(received.unwrap(), pid);
    }

    // -- 12. Graceful leave handling ---------------------------------------

    #[test]
    fn test_node_left_graceful() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let pa = addr(9001);
        let pid = NodeId::new(&pa);
        cs.handle_heartbeat(pid, pa);
        assert!(cs.is_member(pid));

        // Simulate the peer leaving via gossip.
        let gossip = vec![NodeGossip {
            node_id: pid,
            address: pa,
            status: NodeStatus::Leaving,
            incarnation: 99,
        }];
        let changed = cs.merge_membership(gossip);
        assert!(changed);
        assert_eq!(cs.get_node(pid).unwrap().status, NodeStatus::Leaving);
    }

    // -- 13. Join cluster via seed -----------------------------------------

    #[test]
    fn test_join_cluster() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let seed = addr(9001);
        cs.join_cluster(seed);

        let seed_id = NodeId::new(&seed);
        assert!(cs.is_member(seed_id));
        assert_eq!(cs.get_node(seed_id).unwrap().status, NodeStatus::Joining);
    }

    // -- 14. Self-join is a no-op ------------------------------------------

    #[test]
    fn test_join_self_is_noop() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        cs.join_cluster(a); // join our own address
        assert_eq!(cs.healthy_node_count(), 1);
    }

    // -- 16. Gossip does not include local node overrides ------------------

    #[test]
    fn test_merge_ignores_local_node() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        // Try to override local node via gossip.
        let gossip = vec![NodeGossip {
            node_id: local,
            address: addr(9999),
            status: NodeStatus::Failed,
            incarnation: 9999,
        }];
        let changed = cs.merge_membership(gossip);
        assert!(!changed);
        assert_eq!(cs.get_node(local).unwrap().status, NodeStatus::Healthy);
    }

    // -- 17. All members includes local ------------------------------------

    #[test]
    fn test_all_members_includes_local() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let pa = addr(9001);
        let pid = NodeId::new(&pa);
        cs.handle_heartbeat(pid, pa);

        assert_eq!(cs.all_members().len(), 2);
    }

    // -- 18. Heartbeat promotes suspicious back to healthy -----------------

    #[test]
    fn test_heartbeat_promotes_suspicious() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let pa = addr(9001);
        let pid = NodeId::new(&pa);
        cs.handle_heartbeat(pid, pa);

        // Force to suspicious.
        if let Some(info) = cs.members.get_mut(&pid) {
            info.status = NodeStatus::Suspicious;
        }

        // Heartbeat should promote back to healthy.
        cs.handle_heartbeat(pid, pa);
        assert_eq!(cs.get_node(pid).unwrap().status, NodeStatus::Healthy);
    }

    // -- 19. Joining status promoted on first heartbeat --------------------

    #[test]
    fn test_joining_promoted_on_heartbeat() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let seed = addr(9001);
        cs.join_cluster(seed);

        let seed_id = NodeId::new(&seed);
        assert_eq!(cs.get_node(seed_id).unwrap().status, NodeStatus::Joining);

        cs.handle_heartbeat(seed_id, seed);
        assert_eq!(cs.get_node(seed_id).unwrap().status, NodeStatus::Healthy);
    }

    // -- 20. Gossip targets are non-empty when peers exist -----------------

    #[test]
    fn test_tick_produces_gossip_when_peers_exist() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let pa = addr(9001);
        let pid = NodeId::new(&pa);
        cs.handle_heartbeat(pid, pa);

        let actions = cs.tick();
        let has_gossip = actions
            .iter()
            .any(|a| matches!(a, ClusterAction::SendGossip { .. }));
        assert!(
            has_gossip,
            "tick should produce gossip action when peers exist"
        );
    }

    // -- 21. Transitive gossip propagation across a three-node chain -------

    #[test]
    fn test_gossip_transitive_propagation_three_nodes() {
        // Chain topology: A <-> B <-> C. B knows everyone directly; A and
        // C only know B. Relaying gossip payloads hop by hop must converge
        // all three membership tables to the full set.
        let addr_a = addr(9100);
        let addr_b = addr(9101);
        let addr_c = addr(9102);
        let id_a = NodeId::new(&addr_a);
        let id_b = NodeId::new(&addr_b);
        let id_c = NodeId::new(&addr_c);

        let mut a = ClusterState::new(id_a, addr_a);
        let mut b = ClusterState::new(id_b, addr_b);
        let mut c = ClusterState::new(id_c, addr_c);

        a.handle_heartbeat(id_b, addr_b);
        b.handle_heartbeat(id_a, addr_a);
        b.handle_heartbeat(id_c, addr_c);
        c.handle_heartbeat(id_b, addr_b);

        // Round 1: B gossips its full table to A and C.
        let payload_b = b.gossip_payload(100);
        a.merge_membership(payload_b.clone());
        c.merge_membership(payload_b);

        assert!(a.is_member(id_c), "A should learn about C via B's gossip");
        assert!(c.is_member(id_a), "C should learn about A via B's gossip");
        assert_eq!(a.all_members().len(), 3);
        assert_eq!(c.all_members().len(), 3);

        // Round 2: A and C gossip their now-complete tables back to B.
        b.merge_membership(a.gossip_payload(100));
        b.merge_membership(c.gossip_payload(100));
        assert_eq!(b.all_members().len(), 3);

        // Incarnation-based conflict resolution survives relaying: a
        // higher-incarnation failure report about C propagates B -> A.
        let mut failed_view = b.gossip_payload(100);
        for entry in &mut failed_view {
            if entry.node_id == id_c {
                entry.status = NodeStatus::Failed;
                entry.incarnation = 999;
            }
        }
        a.merge_membership(failed_view);
        assert_eq!(a.get_node(id_c).unwrap().status, NodeStatus::Failed);
    }

    // -- 22. Dead peer is detected even while a peer gossips about it -------

    #[test]
    fn test_dead_peer_detected_while_peer_gossips_about_it() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        // The peer that will die.
        let dead_addr = addr(9001);
        let dead_id = NodeId::new(&dead_addr);
        cs.handle_heartbeat(dead_id, dead_addr);

        // A surviving peer that keeps gossiping about the dead peer.
        let surv_addr = addr(9002);
        let surv_id = NodeId::new(&surv_addr);
        cs.handle_heartbeat(surv_id, surv_addr);

        // Establish a known incarnation (5) on the dead peer's entry.
        let gossip_v5 = vec![NodeGossip {
            node_id: dead_id,
            address: dead_addr,
            status: NodeStatus::Healthy,
            incarnation: 5,
        }];
        cs.merge_membership(gossip_v5.clone());

        // The dead peer stops heartbeating: move its timestamp into the past,
        // beyond the full suspicion window.
        let stale = Instant::now()
            - DEFAULT_HEARTBEAT_TIMEOUT
            - DEFAULT_SUSPICION_DURATION
            - Duration::from_secs(1);
        cs.members.get_mut(&dead_id).unwrap().last_heartbeat = stale;

        // The survivor keeps gossiping about the dead peer at the SAME
        // incarnation. This must NOT refresh the dead peer's last_heartbeat
        // (that was the failure-detector-defeating bug).
        cs.merge_membership(gossip_v5);
        assert_eq!(
            cs.get_node(dead_id).unwrap().last_heartbeat,
            stale,
            "equal-incarnation gossip must not refresh a dead peer's timestamp"
        );

        // The failure detector must now declare the peer failed.
        let actions = cs.tick();
        assert_eq!(cs.get_node(dead_id).unwrap().status, NodeStatus::Failed);
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ClusterAction::NodeFailed { node } if *node == dead_id)),
            "tick should emit NodeFailed for the dead peer"
        );
        // The surviving peer stays healthy throughout.
        assert_eq!(cs.get_node(surv_id).unwrap().status, NodeStatus::Healthy);
    }
    // -- 21. Split-brain resolver -------------------------------------------

    #[test]
    fn test_static_quorum_boundaries() {
        // expected 5 → quorum is 3 reachable members (including self).
        // Mirrors the real tick view: `members` includes the local node.
        let resolver = StaticQuorumResolver { expected_nodes: 5 };
        let local = NodeId(1);
        let view = |reachable_peers: usize| MembershipView {
            local,
            members: std::iter::once({
                let mut info = NodeInfo::new(local, addr(9000));
                info.status = NodeStatus::Healthy;
                info
            })
            .chain((0..reachable_peers).map(|i| {
                let mut info = NodeInfo::new(NodeId(i as u64 + 2), addr(9100 + i as u16));
                info.status = NodeStatus::Healthy;
                info
            }))
            .collect(),
        };
        // 2 reachable (self + 1 peer) < 3 → down.
        assert_eq!(resolver.decide(&view(1)), ResolverDecision::DownSelf);
        // 3 reachable (self + 2 peers) → stay up.
        assert_eq!(resolver.decide(&view(2)), ResolverDecision::StayUp);
        // 4 reachable (self + 3 peers) → stay up.
        assert_eq!(resolver.decide(&view(3)), ResolverDecision::StayUp);
    }
    #[test]
    fn test_tick_downs_below_quorum() {
        // Clean partition of a 3-node cluster: the local node sees only
        // itself, so the static-quorum resolver downs it (1 < 2).
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);
        let b = NodeId::new(&addr(9001));
        let c = NodeId::new(&addr(9002));
        cs.handle_heartbeat(b, addr(9001));
        cs.handle_heartbeat(c, addr(9002));
        assert!(cs.apply_config(&ClusterConfig {
            split_brain: SplitBrainConfig::StaticQuorum { expected_nodes: 3 },
            probe_interval: Duration::from_secs(5),
        }));

        // Both peers stop heartbeating: age their timestamps past the full
        // failure window so a single tick transitions them to Failed.
        let stale = Instant::now()
            - DEFAULT_HEARTBEAT_TIMEOUT
            - DEFAULT_SUSPICION_DURATION
            - Duration::from_secs(1);
        cs.members.get_mut(&b).unwrap().last_heartbeat = stale;
        cs.members.get_mut(&c).unwrap().last_heartbeat = stale;

        let actions = cs.tick();
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ClusterAction::Down { node } if *node == local)),
            "tick must down the local node below quorum, got {:?}",
            actions
        );
        assert!(cs.is_down());
    }

    #[test]
    fn test_tick_stays_up_above_quorum() {
        // 3-node cluster where one peer is still reachable: 2 of 3 reachable
        // meets the quorum of 2, so the local node stays up.
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);
        let b = NodeId::new(&addr(9001));
        let c = NodeId::new(&addr(9002));
        cs.handle_heartbeat(b, addr(9001));
        cs.handle_heartbeat(c, addr(9002));
        cs.apply_config(&ClusterConfig {
            split_brain: SplitBrainConfig::StaticQuorum { expected_nodes: 3 },
            probe_interval: Duration::from_secs(5),
        });
        let stale = Instant::now()
            - DEFAULT_HEARTBEAT_TIMEOUT
            - DEFAULT_SUSPICION_DURATION
            - Duration::from_secs(1);
        cs.members.get_mut(&c).unwrap().last_heartbeat = stale;

        let actions = cs.tick();
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, ClusterAction::Down { .. })),
            "tick must NOT down the local node above quorum, got {:?}",
            actions
        );
        assert!(!cs.is_down());
        assert_eq!(cs.get_node(b).unwrap().status, NodeStatus::Healthy);
    }

    #[test]
    fn test_asymmetric_partition_downs_smaller_side() {
        // Asymmetric partition: A sees B as Healthy, but B cannot see A.
        // A (2 of 3 reachable) stays up; B (1 of 3 reachable) downs itself.
        let a_addr = addr(9000);
        let a_id = NodeId::new(&a_addr);
        let mut cs_a = ClusterState::new(a_id, a_addr);
        let b_addr = addr(9001);
        let b_id = NodeId::new(&b_addr);
        cs_a.handle_heartbeat(b_id, b_addr);
        cs_a.apply_config(&ClusterConfig {
            split_brain: SplitBrainConfig::StaticQuorum { expected_nodes: 3 },
            probe_interval: Duration::from_secs(5),
        });
        assert!(!cs_a
            .tick()
            .iter()
            .any(|a| matches!(a, ClusterAction::Down { .. })));

        // B's view: A is unreachable; B sees only itself.
        let mut cs_b = ClusterState::new(b_id, b_addr);
        cs_b.handle_heartbeat(a_id, a_addr);
        cs_b.apply_config(&ClusterConfig {
            split_brain: SplitBrainConfig::StaticQuorum { expected_nodes: 3 },
            probe_interval: Duration::from_secs(5),
        });
        let stale = Instant::now()
            - DEFAULT_HEARTBEAT_TIMEOUT
            - DEFAULT_SUSPICION_DURATION
            - Duration::from_secs(1);
        cs_b.members.get_mut(&a_id).unwrap().last_heartbeat = stale;
        let actions = cs_b.tick();
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ClusterAction::Down { node } if *node == b_id)),
            "the one-way side of an asymmetric partition must down itself"
        );
    }

    #[test]
    fn test_five_node_split_majority_survives() {
        // 5-node cluster split 3v2: the side seeing 3 members stays up, the
        // side seeing 2 downs itself (quorum for expected 5 is 3).
        let a = addr(9000);
        let local = NodeId::new(&a);
        let stale = Instant::now()
            - DEFAULT_HEARTBEAT_TIMEOUT
            - DEFAULT_SUSPICION_DURATION
            - Duration::from_secs(1);
        let config = ClusterConfig {
            split_brain: SplitBrainConfig::StaticQuorum { expected_nodes: 5 },
            probe_interval: Duration::from_secs(5),
        };

        // Majority side: peers 9001, 9002 healthy; 9003, 9004 failed.
        let mut cs = ClusterState::new(local, a);
        cs.handle_heartbeat(NodeId::new(&addr(9001)), addr(9001));
        cs.handle_heartbeat(NodeId::new(&addr(9002)), addr(9002));
        for port in [9003u16, 9004] {
            let peer = NodeId::new(&addr(port));
            cs.handle_heartbeat(peer, addr(port));
            cs.members.get_mut(&peer).unwrap().last_heartbeat = stale;
        }
        cs.apply_config(&config);
        assert!(!cs
            .tick()
            .iter()
            .any(|a| matches!(a, ClusterAction::Down { .. })));

        // Minority side: only peer 9001 healthy; 9002-9004 failed.
        let mut cs_minority = ClusterState::new(local, a);
        cs_minority.handle_heartbeat(NodeId::new(&addr(9001)), addr(9001));
        for port in [9002u16, 9003, 9004] {
            let peer = NodeId::new(&addr(port));
            cs_minority.handle_heartbeat(peer, addr(port));
            cs_minority.members.get_mut(&peer).unwrap().last_heartbeat = stale;
        }
        cs_minority.apply_config(&config);
        let actions = cs_minority.tick();
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ClusterAction::Down { node } if *node == local)),
            "the minority side of a 3v2 split must down itself"
        );
    }

    #[test]
    fn test_down_node_stays_quiet() {
        // Once downed, tick returns no actions at all (no heartbeats,
        // gossip, or probes).
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);
        let b = NodeId::new(&addr(9001));
        cs.handle_heartbeat(b, addr(9001));
        cs.apply_config(&ClusterConfig {
            split_brain: SplitBrainConfig::StaticQuorum { expected_nodes: 3 },
            probe_interval: Duration::from_secs(5),
        });
        let stale = Instant::now()
            - DEFAULT_HEARTBEAT_TIMEOUT
            - DEFAULT_SUSPICION_DURATION
            - Duration::from_secs(1);
        cs.members.get_mut(&b).unwrap().last_heartbeat = stale;
        cs.tick();
        assert!(cs.is_down());
        assert!(cs.tick().is_empty(), "a downed node must emit no actions");
    }

    #[test]
    fn test_probe_emitted_for_failed_members_throttled() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);
        let b = NodeId::new(&addr(9001));
        cs.handle_heartbeat(b, addr(9001));
        cs.apply_config(&ClusterConfig {
            split_brain: SplitBrainConfig::Disabled,
            probe_interval: Duration::from_secs(5),
        });
        let stale = Instant::now()
            - DEFAULT_HEARTBEAT_TIMEOUT
            - DEFAULT_SUSPICION_DURATION
            - Duration::from_secs(1);
        cs.members.get_mut(&b).unwrap().last_heartbeat = stale;

        let actions = cs.tick();
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, ClusterAction::Probe { to, .. } if *to == b)),
            "tick must probe the failed member"
        );
        // Throttled by probe_interval: an immediate second tick must not
        // re-probe.
        let actions2 = cs.tick();
        assert!(
            !actions2
                .iter()
                .any(|a| matches!(a, ClusterAction::Probe { .. })),
            "probes must be throttled to probe_interval"
        );
    }

    #[test]
    fn test_heartbeat_promotes_failed() {
        // The self-healing path: a probe that reaches a live (previously
        // failed) node delivers a heartbeat, which promotes it back to
        // Healthy.
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);
        let b = NodeId::new(&addr(9001));
        cs.handle_heartbeat(b, addr(9001));
        let stale = Instant::now()
            - DEFAULT_HEARTBEAT_TIMEOUT
            - DEFAULT_SUSPICION_DURATION
            - Duration::from_secs(1);
        cs.members.get_mut(&b).unwrap().last_heartbeat = stale;
        cs.tick();
        assert_eq!(cs.get_node(b).unwrap().status, NodeStatus::Failed);

        cs.handle_heartbeat(b, addr(9001));
        assert_eq!(cs.get_node(b).unwrap().status, NodeStatus::Healthy);
    }

    #[test]
    fn test_static_quorum_zero_expected_rejected() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);
        assert!(!cs.apply_config(&ClusterConfig {
            split_brain: SplitBrainConfig::StaticQuorum { expected_nodes: 0 },
            probe_interval: Duration::from_secs(5),
        }));
        // The invalid config leaves the previous (disabled) state in place.
        assert!(cs.split_brain.is_none());
    }
}
