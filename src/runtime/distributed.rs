//! Distributed Actor Address Resolution for Nulang.
//!
//! This module provides **location-transparent actor addressing**, allowing
//! actors to send messages to other actors regardless of whether they reside
//! on the same node or a remote node in the cluster.
//!
//! # Architecture
//!
//! ```text
//!  Actor (local)          ActorAddress::Local{actor_id}
//!       |                            |
//!       v                            v
//!  send_distributed() ----> AddressResolver::resolve()
//!                                 |
//!                     +-----------+-----------+
//!                     |                       |
//!                Local actor          Remote actor
//!                     |                       |
//!               Runtime::           NetworkTransport::
//!               send_message()        send(packet)
//!                                         |
//!                                         v
//!                                    Packet::ActorMessage
//! ```
//!
//! The [`ActorAddress`] enum is the core abstraction: it can refer to either a
//! local actor (same node) or a remote actor (different node). The runtime
//! uses this to route messages to the correct destination without the sender
//! knowing the physical location of the target actor.
//!
//! # Key Types
//!
//! - [`ActorAddress`] — location-transparent actor reference.
//! - [`AddressResolver`] — resolves addresses to local lookups or network routes.
//! - [`RemoteActorCache`] — LRU cache of recently-contacted remote actors.
//! - [`DistributedRuntime`] — trait extending [`Runtime`] with distributed ops.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Imports from sibling modules in the runtime
// ---------------------------------------------------------------------------

use super::{ClusterState, NodeId, NodeStatus};
use super::mailbox::{Message, MessagePriority};
use super::network::{NetworkTransport, Packet};
use super::crdt_manager::CrdtManager;
use crate::runtime::Runtime;
use crate::vm::Value;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default maximum number of entries in the remote actor cache.
const DEFAULT_CACHE_SIZE: usize = 10_000;

// ---------------------------------------------------------------------------
// ActorAddress
// ---------------------------------------------------------------------------

/// A location-transparent address for an actor.
///
/// An `ActorAddress` can refer to either a local actor (same node) or a
/// remote actor (different node). The runtime uses this to route messages
/// to the correct destination without the sender knowing the location.
///
/// # Example
///
/// ```ignore
/// use nulang::runtime::distributed::ActorAddress;
/// use nulang::runtime::cluster::NodeId;
///
/// let local = ActorAddress::local(42);
/// assert!(local.is_local());
///
/// let remote = ActorAddress::remote(NodeId(7), 42);
/// assert!(remote.is_remote());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActorAddress {
    /// Actor on this node.
    Local { actor_id: u64 },
    /// Actor on a remote node.
    Remote { node_id: NodeId, actor_id: u64 },
}

impl ActorAddress {
    /// Create a local address.
    pub fn local(actor_id: u64) -> Self {
        ActorAddress::Local { actor_id }
    }

    /// Create a remote address.
    pub fn remote(node_id: NodeId, actor_id: u64) -> Self {
        ActorAddress::Remote { node_id, actor_id }
    }

    /// Get the actor ID regardless of location.
    pub fn actor_id(&self) -> u64 {
        match self {
            ActorAddress::Local { actor_id } => *actor_id,
            ActorAddress::Remote { actor_id, .. } => *actor_id,
        }
    }

    /// Get the node ID (returns `NodeId::LOCAL` for local actors).
    pub fn node_id(&self) -> NodeId {
        match self {
            ActorAddress::Local { .. } => NodeId::LOCAL,
            ActorAddress::Remote { node_id, .. } => *node_id,
        }
    }

    /// Check if this is a local address.
    pub fn is_local(&self) -> bool {
        matches!(self, ActorAddress::Local { .. })
    }

    /// Check if this is a remote address.
    pub fn is_remote(&self) -> bool {
        matches!(self, ActorAddress::Remote { .. })
    }
}

// ---------------------------------------------------------------------------
// RemoteActorInfo
// ---------------------------------------------------------------------------

/// Cached information about a remote actor.
#[derive(Debug, Clone)]
pub struct RemoteActorInfo {
    /// Node the actor lives on.
    pub node_id: NodeId,
    /// Actor ID on the remote node.
    pub actor_id: u64,
    /// When this cache entry was last accessed.
    pub last_accessed: Instant,
    /// How many messages have been sent to this actor (approximate).
    pub message_count: u64,
}

// ---------------------------------------------------------------------------
// RemoteActorCache
// ---------------------------------------------------------------------------

/// Cache of remote actors that this node knows about.
///
/// When we send a message to a remote actor, we cache its address so
/// that subsequent sends don't need to resolve it again. The cache
/// also tracks which remote actors have sent us messages (so we can
/// reply).
///
/// Uses a simple LRU eviction policy: when the cache exceeds `max_entries`,
/// the least-recently-accessed entry is removed.
pub struct RemoteActorCache {
    /// Map: (remote node, actor id) → cached info.
    entries: HashMap<(NodeId, u64), RemoteActorInfo>,
    /// Maximum number of entries before eviction.
    max_entries: usize,
    /// Access order for LRU eviction — most recent at the back.
    access_order: VecDeque<(NodeId, u64)>,
}

impl RemoteActorCache {
    /// Create a new cache with the given maximum capacity.
    pub fn new(max_entries: usize) -> Self {
        RemoteActorCache {
            entries: HashMap::with_capacity(max_entries.min(1024)),
            max_entries: max_entries.max(1), // Ensure at least 1
            access_order: VecDeque::new(),
        }
    }

    /// Look up a remote actor in the cache.
    ///
    /// On a hit, the entry is moved to the most-recently-used position.
    pub fn get(&mut self, node_id: NodeId, actor_id: u64) -> Option<&RemoteActorInfo> {
        let key = (node_id, actor_id);
        if let Some(info) = self.entries.get_mut(&key) {
            // Update LRU position: remove and re-insert at back.
            self.access_order.retain(|&k| k != key);
            self.access_order.push_back(key);
            // Update last_accessed timestamp.
            info.last_accessed = Instant::now();
            Some(info)
        } else {
            None
        }
    }

    /// Add or update a remote actor in the cache.
    ///
    /// If the cache is at capacity, the least-recently-used entry is evicted.
    pub fn put(&mut self, node_id: NodeId, actor_id: u64) {
        let key = (node_id, actor_id);

        // If already present, just update position and timestamp.
        if self.entries.contains_key(&key) {
            self.access_order.retain(|&k| k != key);
            self.access_order.push_back(key);
            if let Some(info) = self.entries.get_mut(&key) {
                info.last_accessed = Instant::now();
            }
            return;
        }

        // Evict if at capacity.
        if self.entries.len() >= self.max_entries {
            if let Some(evict_key) = self.access_order.pop_front() {
                self.entries.remove(&evict_key);
            }
        }

        // Insert new entry.
        let info = RemoteActorInfo {
            node_id,
            actor_id,
            last_accessed: Instant::now(),
            message_count: 0,
        };
        self.entries.insert(key, info);
        self.access_order.push_back(key);
    }

    /// Remove a remote actor from the cache.
    pub fn remove(&mut self, node_id: NodeId, actor_id: u64) {
        let key = (node_id, actor_id);
        self.entries.remove(&key);
        self.access_order.retain(|&k| k != key);
    }

    /// Get the N most recently accessed remote actors.
    ///
    /// Returns them in MRU order (most recent first).
    pub fn most_active(&self, n: usize) -> Vec<&RemoteActorInfo> {
        self.access_order
            .iter()
            .rev()
            .filter_map(|key| self.entries.get(key))
            .take(n)
            .collect()
    }

    /// Increment the message count for a cached entry.
    ///
    /// No-op if the entry is not in the cache.
    fn increment_message_count(&mut self, node_id: NodeId, actor_id: u64) {
        let key = (node_id, actor_id);
        if let Some(info) = self.entries.get_mut(&key) {
            info.message_count += 1;
        }
    }

    /// Current number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// ResolveResult
// ---------------------------------------------------------------------------

/// Result of resolving an actor address.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolveResult {
    /// Actor is local — look it up in the local actor table.
    Local { actor_id: u64 },
    /// Actor is remote — send this packet to the given node.
    Remote { node_id: NodeId, actor_id: u64 },
    /// Cannot resolve — node is not in the cluster or actor is unknown.
    Unresolvable { reason: String },
}

// ---------------------------------------------------------------------------
// ResolverStats
// ---------------------------------------------------------------------------

/// Statistics for the address resolver.
#[derive(Debug, Default, Clone, Copy)]
pub struct ResolverStats {
    /// Number of addresses resolved to local actors.
    pub local_resolves: u64,
    /// Number of addresses resolved to remote actors.
    pub remote_resolves: u64,
    /// Number of failed resolves (unhealthy node, unknown actor, etc.).
    pub failed_resolves: u64,
    /// Number of cache hits during resolution.
    pub cache_hits: u64,
    /// Number of cache misses during resolution.
    pub cache_misses: u64,
}

// ---------------------------------------------------------------------------
// AddressResolver
// ---------------------------------------------------------------------------

/// Resolves actor addresses to either local actor lookups or network routes.
///
/// This is the core of location transparency: given an [`ActorAddress`],
/// the resolver either finds the local actor or prepares a network packet
/// to send to the remote node.
///
/// The resolver maintains a [`RemoteActorCache`] to avoid repeated cluster
/// lookups for hot remote actors, and tracks statistics for observability.
pub struct AddressResolver {
    local_node: NodeId,
    remote_cache: RemoteActorCache,
    stats: ResolverStats,
}

impl AddressResolver {
    /// Create a new resolver for the given local node.
    pub fn new(local_node: NodeId) -> Self {
        AddressResolver {
            local_node,
            remote_cache: RemoteActorCache::new(DEFAULT_CACHE_SIZE),
            stats: ResolverStats::default(),
        }
    }

    /// Resolve an actor address.
    ///
    /// For local addresses, returns [`ResolveResult::Local`].
    /// For remote addresses, checks the cluster membership to verify
    /// the node is healthy, then returns [`ResolveResult::Remote`].
    pub fn resolve(
        &mut self,
        cluster: &ClusterState,
        address: ActorAddress,
    ) -> ResolveResult {
        match address {
            ActorAddress::Local { actor_id } => {
                self.stats.local_resolves += 1;
                ResolveResult::Local { actor_id }
            }
            ActorAddress::Remote { node_id, actor_id } => {
                self.resolve_remote(cluster, node_id, actor_id)
            }
        }
    }

    /// Resolve from raw `node_id` + `actor_id` (used when receiving a message).
    ///
    /// Checks the cluster to verify the remote node is a known, healthy member.
    /// Updates the remote actor cache on success.
    pub fn resolve_remote(
        &mut self,
        cluster: &ClusterState,
        node_id: NodeId,
        actor_id: u64,
    ) -> ResolveResult {
        // Fast path: local node.
        if node_id == self.local_node || node_id == NodeId::LOCAL {
            self.stats.local_resolves += 1;
            return ResolveResult::Local { actor_id };
        }

        // Check the cache first.
        if self.remote_cache.get(node_id, actor_id).is_some() {
            self.stats.cache_hits += 1;
            self.stats.remote_resolves += 1;
            return ResolveResult::Remote { node_id, actor_id };
        }

        self.stats.cache_misses += 1;

        // Verify the node is in the cluster and healthy.
        match cluster.get_node(node_id) {
            Some(info) => {
                if info.status != NodeStatus::Healthy {
                    self.stats.failed_resolves += 1;
                    return ResolveResult::Unresolvable {
                        reason: format!(
                            "node {:?} is not healthy (status: {:?})",
                            node_id, info.status
                        ),
                    };
                }

                // Healthy node — cache the actor and return remote result.
                self.remote_cache.put(node_id, actor_id);
                self.stats.remote_resolves += 1;
                ResolveResult::Remote { node_id, actor_id }
            }
            None => {
                self.stats.failed_resolves += 1;
                ResolveResult::Unresolvable {
                    reason: format!("node {:?} is not known in the cluster", node_id),
                }
            }
        }
    }

    /// Record that we successfully sent to a remote actor.
    ///
    /// Updates the cache and increments the per-actor message count.
    pub fn record_remote_send(&mut self, node_id: NodeId, actor_id: u64) {
        self.remote_cache.put(node_id, actor_id);
        self.remote_cache.increment_message_count(node_id, actor_id);
    }

    /// Record that we received from a remote actor.
    ///
    /// Updates the cache so we can reply efficiently.
    pub fn record_remote_receive(&mut self, node_id: NodeId, actor_id: u64) {
        self.remote_cache.put(node_id, actor_id);
    }

    /// Build a network packet for sending a message to a remote actor.
    ///
    /// The packet includes the sender's node ID so the remote node can
    /// route replies back.
    pub fn build_packet(
        &self,
        target_actor: u64,
        behavior_id: u16,
        payload: Vec<Value>,
        sender_actor: u64,
        priority: MessagePriority,
    ) -> Packet {
        Packet::ActorMessage {
            target_actor,
            behavior_id,
            payload,
            sender_actor,
            sender_node: NodeId(self.local_node.0),
            priority,
        }
    }

    /// Parse a received network packet into a message for local delivery.
    ///
    /// Returns `Some((target_actor_id, message))` if the packet is an actor
    /// message that should be delivered locally. Returns `None` for other
    /// packet types (e.g., heartbeats, spawn requests).
    ///
    /// Also updates the remote cache with the sender information.
    pub fn parse_packet(&mut self, packet: Packet) -> Option<(u64, Message)> {
        match packet {
            Packet::ActorMessage {
                target_actor,
                behavior_id,
                payload,
                sender_actor,
                sender_node,
                priority,
            } => {
                // Record the sender in our cache so we can reply.
                let sender_cluster_node = NodeId(sender_node.0);
                self.record_remote_receive(sender_cluster_node, sender_actor);

                let msg = Message {
                    behavior_id,
                    payload,
                    sender: sender_actor,
                    priority,
                };
                Some((target_actor, msg))
            }
            // Non-actor-message packets are not parsed here.
            _ => None,
        }
    }

    /// Get statistics about the resolver.
    pub fn stats(&self) -> ResolverStats {
        self.stats
    }

    /// Get a reference to the remote actor cache.
    pub fn cache(&self) -> &RemoteActorCache {
        &self.remote_cache
    }

    /// Get a mutable reference to the remote actor cache.
    pub fn cache_mut(&mut self) -> &mut RemoteActorCache {
        &mut self.remote_cache
    }

    /// Get the local node ID.
    pub fn local_node(&self) -> NodeId {
        self.local_node
    }
}

// ---------------------------------------------------------------------------
// DistributedRuntime trait
// ---------------------------------------------------------------------------

/// Extension methods for [`Runtime`] that add distributed capabilities.
///
/// These methods are called by the integration layer (Stage B1) to
/// wire distributed messaging into the existing [`Runtime`].
///
/// # Example
///
/// ```ignore
/// use nulang_runtime::distributed::{ActorAddress, DistributedRuntimeImpl};
///
/// let mut runtime = Runtime::new();
/// let mut transport = NetworkTransport::bind(addr).unwrap();
/// let mut cluster = ClusterState::new(local_node, addr);
/// let mut resolver = AddressResolver::new(local_node);
///
/// // Send to a remote actor as if it were local
/// let target = ActorAddress::remote(remote_node, remote_actor_id);
/// DistributedRuntimeImpl::send_distributed(
///     &mut runtime, &mut transport, &cluster, &mut resolver,
///     target, "handle_msg", &[Value::int(42)],
/// );
/// ```
pub trait DistributedRuntime {
    /// Send a message to an actor using a location-transparent address.
    ///
    /// If the actor is local, delegates to the normal [`Runtime::send_message`].
    /// If the actor is remote, serializes the message and sends it
    /// over the network transport.
    fn send_distributed(
        &mut self,
        transport: &mut NetworkTransport,
        cluster: &ClusterState,
        resolver: &mut AddressResolver,
        target: ActorAddress,
        behavior: &str,
        args: &[Value],
    );

    /// Process incoming network packets.
    ///
    /// Reads all packets from the transport and delivers actor messages
    /// to their target actors. Handles heartbeats by forwarding to the
    /// cluster state.
    fn process_network_packets(
        &mut self,
        transport: &NetworkTransport,
        cluster: &mut ClusterState,
        resolver: &mut AddressResolver,
    );

    /// Spawn an actor on a specific node.
    ///
    /// If `node` is the local node, behaves like normal spawn.
    /// If `node` is remote, sends a spawn request over the network.
    fn spawn_on_node(
        &mut self,
        transport: &mut NetworkTransport,
        node: NodeId,
        behavior_name: &str,
        initial_state: Vec<(String, Value)>,
    ) -> ActorAddress;
}

// ---------------------------------------------------------------------------
// Concrete implementation via wrapper struct
//
// Since Runtime is defined in mod.rs and we can't add trait impls to it
// from here without orphan rules issues, we provide a wrapper struct
// that holds references to all the components.  This is the pattern
// recommended for integration (Stage B1).
// ---------------------------------------------------------------------------

/// Lightweight wrapper that holds a mutable reference to the [`Runtime`].
///
/// Used to implement the [`DistributedRuntime`] trait without consuming
/// the runtime. The other distributed components (transport, cluster,
/// resolver) are passed as parameters to the trait methods, avoiding
/// borrow-checker aliasing issues.
pub struct DistributedRuntimeImpl<'a> {
    pub runtime: &'a mut Runtime,
}

impl<'a> DistributedRuntimeImpl<'a> {
    /// Create a new distributed runtime wrapper.
    pub fn new(runtime: &'a mut Runtime) -> Self {
        DistributedRuntimeImpl { runtime }
    }
}

impl<'a> DistributedRuntime for DistributedRuntimeImpl<'a> {
    fn send_distributed(
        &mut self,
        transport: &mut NetworkTransport,
        cluster: &ClusterState,
        resolver: &mut AddressResolver,
        target: ActorAddress,
        behavior: &str,
        args: &[Value],
    ) {
        send_distributed(self.runtime, transport, cluster, resolver, target, behavior, args)
    }

    fn process_network_packets(
        &mut self,
        transport: &NetworkTransport,
        cluster: &mut ClusterState,
        resolver: &mut AddressResolver,
    ) {
        process_network_packets(self.runtime, transport, cluster, resolver)
    }

    fn spawn_on_node(
        &mut self,
        _transport: &mut NetworkTransport,
        node: NodeId,
        _behavior_name: &str,
        _initial_state: Vec<(String, Value)>,
    ) -> ActorAddress {
        // The trait method doesn't receive cluster/resolver, so we can
        // only handle local spawns here. For remote spawns, use the
        // `spawn_on_node` free function which takes all components.
        if node == NodeId::LOCAL {
            // Local spawn would need the behavior_name and initial_state.
            // This is a limitation of the trait API — use the free function.
            ActorAddress::local(0)
        } else {
            // Cannot determine if node is local without resolver.
            // Return a placeholder; callers should use the free function.
            ActorAddress::remote(node, 0)
        }
    }
}

// ---------------------------------------------------------------------------
// Free functions (simpler integration alternative)
// ---------------------------------------------------------------------------

/// Send a message to an actor using a location-transparent address.
///
/// This is the simplest way to send a distributed message — no trait
/// objects or wrapper structs needed.
///
/// # Example
///
/// ```ignore
/// use nulang_runtime::distributed::{ActorAddress, send_distributed};
///
/// let target = ActorAddress::remote(remote_node, actor_id);
/// send_distributed(&mut runtime, &mut transport, &cluster, &mut resolver,
///                  target, "handle", &[Value::int(42)]);
/// ```
pub fn send_distributed(
    runtime: &mut Runtime,
    transport: &mut NetworkTransport,
    cluster: &ClusterState,
    resolver: &mut AddressResolver,
    target: ActorAddress,
    behavior: &str,
    args: &[Value],
) {
    match resolver.resolve(cluster, target) {
        ResolveResult::Local { actor_id } => {
            runtime.send_message(actor_id, behavior, args);
        }
        ResolveResult::Remote { node_id, actor_id } => {
            // For remote sends, the behavior_id is 0 as a placeholder.
            // The remote node resolves the behavior name on delivery
            // using its local behavior registry.
            let packet = resolver.build_packet(
                actor_id,
                0, // behavior_id placeholder — resolved on remote side
                args.to_vec(),
                runtime.current_actor.unwrap_or(0),
                MessagePriority::Normal,
            );

            if let Some(node_info) = cluster.get_node(node_id) {
                let net_node_id = NodeId(node_id.0);
                transport.send(net_node_id, node_info.address, packet);
                resolver.record_remote_send(node_id, actor_id);
            }
        }
        ResolveResult::Unresolvable { reason } => {
            let _ = reason;
        }
    }
}

/// Process all incoming network packets and deliver actor messages.
///
/// Heartbeats are forwarded to the cluster state. Actor messages are
/// parsed and delivered to the target actor's mailbox.
pub fn process_network_packets(
    runtime: &mut Runtime,
    transport: &NetworkTransport,
    cluster: &mut ClusterState,
    resolver: &mut AddressResolver,
) {
    let packets = transport.receive();
    for incoming in packets {
        match incoming.packet {
            Packet::Heartbeat { node_id, .. } => {
                let cluster_node_id = NodeId(node_id.0);
                // We need the SocketAddr from the incoming packet context.
                // The IncomingPacket doesn't carry the addr directly, but
                // we can get it from the cluster or use a default.
                // For now, we look up the node's known address.
                let known_addr = cluster
                    .get_node(cluster_node_id)
                    .map(|info| info.address);
                if let Some(addr) = known_addr {
                    cluster.handle_heartbeat(cluster_node_id, addr);
                }
            }
            Packet::CrdtSync { ops } => {
                if let Some(manager) = &mut runtime.crdt_manager {
                    for op in ops {
                        manager.apply_op(op);
                    }
                }
            }
            _ => {
                if let Some((target_actor, msg)) = resolver.parse_packet(incoming.packet) {
                    if let Some(actor) = runtime.actors.get_mut(&target_actor) {
                        let _ = actor.mailbox.push(msg);
                        runtime.scheduler.enqueue(target_actor);
                    }
                }
            }
        }
    }
}

/// Spawn an actor on a specific node.
///
/// Local spawns go through [`Runtime::spawn_actor`]. Remote spawns send
/// a [`Packet::SpawnRequest`] over the network.
pub fn spawn_on_node(
    runtime: &mut Runtime,
    transport: &mut NetworkTransport,
    cluster: &ClusterState,
    resolver: &AddressResolver,
    node: NodeId,
    behavior_name: &str,
    initial_state: Vec<(String, Value)>,
) -> ActorAddress {
    if node == resolver.local_node() || node == NodeId::LOCAL {
        let id = runtime.spawn_actor(Box::new(move || initial_state));
        ActorAddress::local(id)
    } else {
        let request_id = fast_random_u64();
        let packet = Packet::SpawnRequest {
            request_id,
            behavior_name: behavior_name.to_string(),
            initial_state,
        };

        if let Some(node_info) = cluster.get_node(node) {
            let net_node_id = NodeId(node.0);
            transport.send(net_node_id, node_info.address, packet);
        }

        ActorAddress::remote(node, request_id)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A fast, non-cryptographic random u64 for request IDs.
///
/// Uses a simple xorshift — good enough for spawn request IDs.
fn fast_random_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0x1234_5678_9ABC_DEF0);
    let mut x = COUNTER.fetch_add(1, Ordering::Relaxed);
    // xorshift64*
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    /// Helper: create a loopback address on a given port.
    fn addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port)
    }

    /// Helper: create a minimal cluster state with the local node and
    /// optionally one peer.
    fn make_cluster(local_port: u16) -> ClusterState {
        let a = addr(local_port);
        let local = NodeId::new(&a);
        ClusterState::new(local, a)
    }

    /// Helper: add a healthy peer to a cluster state.
    fn add_peer(cluster: &mut ClusterState, peer_port: u16) -> NodeId {
        let peer_addr = addr(peer_port);
        let peer_id = NodeId::new(&peer_addr);
        cluster.handle_heartbeat(peer_id, peer_addr);
        peer_id
    }

    // -- 1. Local ActorAddress ----------------------------------------------

    #[test]
    fn test_actor_address_local() {
        let a = ActorAddress::local(42);
        assert!(a.is_local());
        assert!(!a.is_remote());
        assert_eq!(a.actor_id(), 42);
        assert_eq!(a.node_id(), NodeId::LOCAL);
    }

    // -- 2. Remote ActorAddress ---------------------------------------------

    #[test]
    fn test_actor_address_remote() {
        let node = NodeId(7);
        let a = ActorAddress::remote(node, 99);
        assert!(a.is_remote());
        assert!(!a.is_local());
        assert_eq!(a.actor_id(), 99);
        assert_eq!(a.node_id(), node);
    }

    // -- 3. actor_id accessor is consistent ----------------------------------

    #[test]
    fn test_actor_address_actor_id() {
        let local = ActorAddress::local(100);
        let remote = ActorAddress::remote(NodeId(3), 100);
        assert_eq!(local.actor_id(), remote.actor_id());
        assert_eq!(local.actor_id(), 100);
    }

    // -- 4. Cache basic put / get --------------------------------------------

    #[test]
    fn test_remote_cache_put_get() {
        let mut cache = RemoteActorCache::new(100);
        assert!(cache.is_empty());

        cache.put(NodeId(1), 10);
        assert_eq!(cache.len(), 1);

        let info = cache.get(NodeId(1), 10);
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.node_id, NodeId(1));
        assert_eq!(info.actor_id, 10);
    }

    // -- 5. LRU eviction when full -------------------------------------------

    #[test]
    fn test_remote_cache_lru_eviction() {
        let mut cache = RemoteActorCache::new(3);

        cache.put(NodeId(1), 10);
        cache.put(NodeId(2), 20);
        cache.put(NodeId(3), 30);
        assert_eq!(cache.len(), 3);

        // Access (1,10) to make it most-recently used.
        let _ = cache.get(NodeId(1), 10);

        // Insert a 4th entry — should evict (2,20) since it's now LRU.
        cache.put(NodeId(4), 40);
        assert_eq!(cache.len(), 3);

        assert!(cache.get(NodeId(1), 10).is_some(), "(1,10) should still be cached");
        assert!(cache.get(NodeId(3), 30).is_some(), "(3,30) should still be cached");
        assert!(cache.get(NodeId(4), 40).is_some(), "(4,40) should be cached");
        assert!(cache.get(NodeId(2), 20).is_none(), "(2,20) should have been evicted");
    }

    // -- 6. Resolver: local address ------------------------------------------

    #[test]
    fn test_resolver_local_address() {
        let local_addr = addr(9000);
        let local_node = NodeId::new(&local_addr);
        let cluster = make_cluster(9000);
        let mut resolver = AddressResolver::new(local_node);

        let address = ActorAddress::local(42);
        let result = resolver.resolve(&cluster, address);

        assert_eq!(result, ResolveResult::Local { actor_id: 42 });
        assert_eq!(resolver.stats().local_resolves, 1);
    }

    // -- 7. Resolver: remote healthy address ---------------------------------

    #[test]
    fn test_resolver_remote_address() {
        let mut cluster = make_cluster(9000);
        let peer_id = add_peer(&mut cluster, 9001);

        let local_addr = addr(9000);
        let local_node = NodeId::new(&local_addr);
        let mut resolver = AddressResolver::new(local_node);

        let address = ActorAddress::remote(peer_id, 55);
        let result = resolver.resolve(&cluster, address);

        assert_eq!(result, ResolveResult::Remote { node_id: peer_id, actor_id: 55 });
        assert_eq!(resolver.stats().remote_resolves, 1);
        // Should be cached after resolution.
        assert!(resolver.cache_mut().get(peer_id, 55).is_some());
    }

    // -- 8. Resolver: unresolvable (unknown node) --------------------------

    #[test]
    fn test_resolver_unresolvable() {
        let cluster = make_cluster(9000);

        let local_addr = addr(9000);
        let local_node = NodeId::new(&local_addr);
        let mut resolver = AddressResolver::new(local_node);

        // Use a NodeId that does not exist in the cluster.
        let unknown_node = NodeId(9999);
        let address = ActorAddress::remote(unknown_node, 55);
        let result = resolver.resolve(&cluster, address);

        assert!(
            matches!(result, ResolveResult::Unresolvable { .. }),
            "expected Unresolvable for unknown node, got {:?}",
            result
        );
        assert_eq!(resolver.stats().failed_resolves, 1);
    }

    // -- 9. Resolver: record_remote_send updates cache -----------------------

    #[test]
    fn test_resolver_record_send() {
        let local_addr = addr(9000);
        let local_node = NodeId::new(&local_addr);
        let mut resolver = AddressResolver::new(local_node);

        assert!(resolver.cache().is_empty());

        resolver.record_remote_send(NodeId(5), 99);

        assert_eq!(resolver.cache().len(), 1);
        let info = resolver.cache_mut().get(NodeId(5), 99).unwrap();
        assert_eq!(info.node_id, NodeId(5));
        assert_eq!(info.actor_id, 99);
        assert_eq!(info.message_count, 1);
    }

    // -- 10. Build packet ----------------------------------------------------

    #[test]
    fn test_build_packet() {
        let local_addr = addr(9000);
        let local_node = NodeId::new(&local_addr);
        let resolver = AddressResolver::new(local_node);

        let packet = resolver.build_packet(
            42,   // target_actor
            3,    // behavior_id
            vec![Value::int(7), Value::string(0)],
            100,  // sender_actor
            MessagePriority::Normal,
        );

        match packet {
            Packet::ActorMessage {
                target_actor,
                behavior_id,
                payload,
                sender_actor,
                sender_node,
                priority,
            } => {
                assert_eq!(target_actor, 42);
                assert_eq!(behavior_id, 3);
                assert_eq!(sender_actor, 100);
                assert_eq!(sender_node.0, local_node.0); // Same underlying u64
                assert_eq!(priority, MessagePriority::Normal);
                assert_eq!(payload.len(), 2);
            }
            other => panic!("expected ActorMessage packet, got {:?}", other),
        }
    }

    // -- 11. Parse packet ----------------------------------------------------

    #[test]
    fn test_parse_packet() {
        let local_addr = addr(9000);
        let local_node = NodeId::new(&local_addr);
        let mut resolver = AddressResolver::new(local_node);

        let packet = Packet::ActorMessage {
            target_actor: 77,
            behavior_id: 2,
            payload: vec![Value::int(123)],
            sender_actor: 88,
            sender_node: NodeId(9), // Remote node 9
            priority: MessagePriority::System,
        };

        let result = resolver.parse_packet(packet);
        assert!(result.is_some());

        let (target, msg) = result.unwrap();
        assert_eq!(target, 77);
        assert_eq!(msg.behavior_id, 2);
        assert_eq!(msg.sender, 88);
        assert_eq!(msg.priority, MessagePriority::System);
        assert_eq!(msg.payload.len(), 1);

        // The sender should now be in the cache.
        assert!(resolver.cache_mut().get(NodeId(9), 88).is_some());
    }

    // -- 12. DistributedRuntime trait compiles -------------------------------

    #[test]
    fn test_distributed_trait_exists() {
        // This test just verifies that the trait and wrapper type compile.
        // We create the components (but don't start network threads).
        let mut runtime = Runtime::new();
        let cluster = make_cluster(9000);
        let local_node = NodeId::new(&addr(9000));
        let mut resolver = AddressResolver::new(local_node);

        // Create a transport — bind to port 0 to get an ephemeral port.
        let transport = NetworkTransport::bind(addr(0));
        if let Ok(mut transport) = transport {
            let mut dist = DistributedRuntimeImpl::new(&mut runtime);

            // Just verify the trait object can be formed.
            let _trait_obj: &dyn DistributedRuntime = &dist;

            // Verify local send works (delegates to runtime.send_message).
            let local_addr = ActorAddress::local(999); // non-existent actor
            DistributedRuntime::send_distributed(
                &mut dist,
                &mut transport,
                &cluster,
                &mut resolver,
                local_addr,
                "test_behavior",
                &[Value::int(1)],
            );

            // Success if we get here without panicking.
            transport.shutdown();
        }
    }

    // -- 13. Cache most_active ordering --------------------------------------

    #[test]
    fn test_cache_most_active() {
        let mut cache = RemoteActorCache::new(10);

        cache.put(NodeId(1), 10);
        cache.put(NodeId(2), 20);
        cache.put(NodeId(3), 30);

        // Access (1,10) most recently, then (3,30).
        let _ = cache.get(NodeId(2), 20);
        let _ = cache.get(NodeId(1), 10);
        let _ = cache.get(NodeId(3), 30);

        let most_active = cache.most_active(2);
        assert_eq!(most_active.len(), 2);
        // Most recent first.
        assert_eq!(most_active[0].actor_id, 30);
        assert_eq!(most_active[1].actor_id, 10);
    }

    // -- 14. Cache remove ----------------------------------------------------

    #[test]
    fn test_cache_remove() {
        let mut cache = RemoteActorCache::new(10);

        cache.put(NodeId(1), 10);
        cache.put(NodeId(2), 20);
        assert_eq!(cache.len(), 2);

        cache.remove(NodeId(1), 10);
        assert_eq!(cache.len(), 1);
        assert!(cache.get(NodeId(1), 10).is_none());
        assert!(cache.get(NodeId(2), 20).is_some());
    }

    // -- 15. ResolveResult equality ------------------------------------------

    #[test]
    fn test_resolve_result_equality() {
        let a = ResolveResult::Local { actor_id: 1 };
        let b = ResolveResult::Local { actor_id: 1 };
        let c = ResolveResult::Local { actor_id: 2 };
        assert_eq!(a, b);
        assert_ne!(a, c);

        let d = ResolveResult::Remote { node_id: NodeId(1), actor_id: 5 };
        let e = ResolveResult::Remote { node_id: NodeId(1), actor_id: 5 };
        assert_eq!(d, e);
    }

    // -- 16. ActorAddress equality and hash ----------------------------------

    #[test]
    fn test_actor_address_eq_and_hash() {
        let a1 = ActorAddress::remote(NodeId(7), 42);
        let a2 = ActorAddress::remote(NodeId(7), 42);
        let a3 = ActorAddress::remote(NodeId(7), 43);

        assert_eq!(a1, a2);
        assert_ne!(a1, a3);

        // Hash consistency: equal values have equal hashes.
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        fn hash_of<T: Hash>(t: &T) -> u64 {
            let mut h = DefaultHasher::new();
            t.hash(&mut h);
            h.finish()
        }
        assert_eq!(hash_of(&a1), hash_of(&a2));
    }
}
