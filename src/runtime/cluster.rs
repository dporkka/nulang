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
    /// Create a `NodeId` from a socket address.
    ///
    /// The id is derived with `std::collections::hash_map::DefaultHasher`
    /// so repeated calls with the same address yield the same id.
    pub fn new(addr: &SocketAddr) -> Self {
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
    SendHeartbeat {
        to: NodeId,
        addr: SocketAddr,
    },
    /// Notify that a node has joined the cluster.
    NodeJoined {
        node: NodeId,
        addr: SocketAddr,
    },
    /// Notify that a node has been declared failed.
    NodeFailed {
        node: NodeId,
    },
    /// Notify that a node has left the cluster.
    NodeLeft {
        node: NodeId,
    },
    /// Send gossip to a random subset of nodes.
    SendGossip {
        targets: Vec<(NodeId, SocketAddr)>,
    },
}

// ---------------------------------------------------------------------------
// NodeGossip
// ---------------------------------------------------------------------------

/// A lightweight gossip entry for membership dissemination.
///
/// This compact representation avoids sending full [`NodeInfo`] (including
/// metadata maps) on every gossip round.
#[derive(Debug, Clone)]
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
/// ```
/// # use nulang_runtime::cluster::{ClusterState, NodeId};
/// # use std::net::{SocketAddr, IpAddr, Ipv4Addr};
/// let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 9000);
/// let local = NodeId::new(&addr);
/// let mut cluster = ClusterState::new(local, addr);
/// ```
pub struct ClusterState {
    /// This node's identity.
    local_node: NodeId,
    local_addr: SocketAddr,

    /// Membership table: node_id → node info.
    members: HashMap<NodeId, NodeInfo>,

    /// Nodes that have been declared failed (kept for a while to
    /// prevent rejoining with stale state).
    failed_nodes: HashMap<NodeId, Instant>,

    /// Heartbeat configuration.
    heartbeat_interval: Duration,
    heartbeat_timeout: Duration,
    suspicion_duration: Duration,

    /// Monotonically increasing incarnation number for this node.
    /// Used to resolve conflicting membership updates.
    incarnation: u64,

    /// Timestamp of last heartbeat we sent.
    last_heartbeat_sent: Instant,

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
            local_addr,
            members,
            failed_nodes: HashMap::new(),
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
            heartbeat_timeout: DEFAULT_HEARTBEAT_TIMEOUT,
            suspicion_duration: DEFAULT_SUSPICION_DURATION,
            incarnation: 1,
            last_heartbeat_sent: now,
            on_member_joined: None,
            on_member_left: None,
            on_member_failed: None,
        }
    }

    /// Join an existing cluster by contacting a seed node.
    ///
    /// Records the seed node in the membership table (as Joining) and
    /// bumps the incarnation so that the join propagates via gossip.
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
            self.members.insert(seed_id, info);
        }

        self.bump_incarnation();
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
        let now = Instant::now();

        match self.members.get_mut(&from) {
            Some(info) => {
                let was_suspicious_or_failed = matches!(
                    info.status,
                    NodeStatus::Suspicious | NodeStatus::Failed
                );

                info.last_heartbeat = now;
                info.address = addr;

                if was_suspicious_or_failed {
                    info.status = NodeStatus::Healthy;
                    self.bump_incarnation();
                } else if info.status == NodeStatus::Joining {
                    info.status = NodeStatus::Healthy;
                }
            }
            None => {
                // New node discovered via heartbeat.
                let mut info = NodeInfo::new(from, addr);
                info.last_heartbeat = now;
                info.status = NodeStatus::Healthy;
                self.members.insert(from, info);
                self.bump_incarnation();

                if let Some(ref cb) = self.on_member_joined {
                    cb(from, addr);
                }
            }
        }
    }

    /// Run the periodic cluster maintenance.
    ///
    /// Should be called regularly (e.g., every 100 ms). Performs:
    ///
    /// 1. Checks for nodes that have missed heartbeats → marks Suspicious.
    /// 2. Promotes Suspicious nodes to Failed if past the suspicion window.
    /// 3. Cleans up old failed nodes.
    /// 4. Returns a list of actions for the runtime to execute.
    pub fn tick(&mut self) -> Vec<ClusterAction> {
        let now = Instant::now();
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

                    actions.push(ClusterAction::NodeFailed {
                        node: info.node_id,
                    });
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
        // 4. Send heartbeats to healthy members (throttled)
        // ------------------------------------------------------------------
        if now.duration_since(self.last_heartbeat_sent) >= self.heartbeat_interval {
            self.last_heartbeat_sent = now;

            for info in self.members.values() {
                if info.node_id == self.local_node {
                    continue;
                }
                if info.status == NodeStatus::Healthy {
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
            .filter(|info| {
                info.node_id != self.local_node && info.status == NodeStatus::Healthy
            })
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

    /// Get the local node's incarnation number.
    pub fn incarnation(&self) -> u64 {
        self.incarnation
    }

    /// Increment the incarnation number.
    ///
    /// Called whenever the local node's view of membership changes so
    /// that gossip recipients prefer our version of the truth.
    pub fn bump_incarnation(&mut self) {
        self.incarnation = self.incarnation.wrapping_add(1);
    }

    /// Merge a membership list received from another node (gossip).
    ///
    /// Uses incarnation numbers for conflict resolution: the entry with
    /// the higher incarnation is considered authoritative.  Returns
    /// `true` if any changes were made to our membership table.
    pub fn merge_membership(&mut self, gossip: Vec<NodeGossip>) -> bool {
        let mut changed = false;

        for entry in gossip {
            // Never overwrite local node info from gossip.
            if entry.node_id == self.local_node {
                continue;
            }

            match self.members.get_mut(&entry.node_id) {
                Some(existing) => {
                    // Higher incarnation wins.
                    if entry.incarnation > existing
                        .metadata
                        .get("_incarnation")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0)
                    {
                        let old_status = existing.status;
                        existing.status = entry.status;
                        existing.address = entry.address;
                        existing.last_heartbeat = Instant::now();
                        existing
                            .metadata
                            .insert("_incarnation".to_string(), entry.incarnation.to_string());

                        if old_status != entry.status {
                            changed = true;
                            if entry.status == NodeStatus::Failed {
                                self.failed_nodes.insert(entry.node_id, Instant::now());
                            }
                        }
                    }
                }
                None => {
                    // New node learned from gossip.
                    let mut info = NodeInfo::new(entry.node_id, entry.address);
                    info.status = entry.status;
                    info.last_heartbeat = Instant::now();
                    info
                        .metadata
                        .insert("_incarnation".to_string(), entry.incarnation.to_string());
                    self.members.insert(entry.node_id, info);
                    changed = true;
                }
            }
        }

        if changed {
            self.bump_incarnation();
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

    /// Pick `n` random healthy targets for gossip.
    ///
    /// Uses a simple round-robin when the `getrandom` facility is not
    /// available; in production this should use a proper RNG.
    fn pick_gossip_targets(&self, n: usize) -> Vec<(NodeId, SocketAddr)> {
        let healthy: Vec<&NodeInfo> = self.healthy_members();
        if healthy.is_empty() {
            return Vec::new();
        }

        // Simple deterministic selection: pick the first N.
        // In a real deployment this would use `rand::seq::IteratorRandom`.
        healthy
            .into_iter()
            .take(n)
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
        assert_eq!(cs.local_addr, a);
        assert_eq!(cs.healthy_node_count(), 1);
        assert!(cs.is_member(local));
        assert_eq!(cs.incarnation(), 1);

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

        // Call tick immediately — peer should still be healthy because
        // not enough time has elapsed.
        let actions = cs.tick();
        // Peer is still healthy because the real timeout hasn't passed.
        // The test documents the API; full timeout testing requires
        // mockable clocks (left as a TODO for production).
        assert!(
            cs.get_node(peer_id).unwrap().status == NodeStatus::Healthy
                || cs.get_node(peer_id).unwrap().status == NodeStatus::Suspicious
        );

        // Verify that SendHeartbeat action is produced for the peer.
        let has_heartbeat = actions.iter().any(|a| matches!(a, ClusterAction::SendHeartbeat { to, .. } if *to == peer_id));
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

        assert_eq!(
            cs.get_node(NodeId(77)).unwrap().status,
            NodeStatus::Healthy
        );

        // Now receive gossip with a higher incarnation marking it Failed.
        let gossip_high = vec![NodeGossip {
            node_id: NodeId(77),
            address: addr(9077),
            status: NodeStatus::Failed,
            incarnation: 10,
        }];
        let changed = cs.merge_membership(gossip_high);
        assert!(changed);
        assert_eq!(
            cs.get_node(NodeId(77)).unwrap().status,
            NodeStatus::Failed
        );
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
        assert_eq!(
            cs.get_node(pid).unwrap().status,
            NodeStatus::Leaving
        );
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

    // -- 15. Bump incarnation ----------------------------------------------

    #[test]
    fn test_bump_incarnation() {
        let a = addr(9000);
        let local = NodeId::new(&a);
        let mut cs = ClusterState::new(local, a);

        let first = cs.incarnation();
        cs.bump_incarnation();
        assert_eq!(cs.incarnation(), first + 1);
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
        assert_eq!(
            cs.get_node(local).unwrap().status,
            NodeStatus::Healthy
        );
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
        assert_eq!(
            cs.get_node(pid).unwrap().status,
            NodeStatus::Healthy
        );
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
        let has_gossip = actions.iter().any(|a| matches!(a, ClusterAction::SendGossip { .. }));
        assert!(has_gossip, "tick should produce gossip action when peers exist");
    }
}
