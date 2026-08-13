//! Distribution subsystem: network transport, cluster membership, CRDT sync,
//! remote spawn, and gossip. These free functions orchestrate the distributed
//! context (transport, cluster, resolver) owned by `Runtime`.

use crate::runtime::distributed;
use crate::runtime::Runtime;
use crate::runtime::GOSSIP_PAYLOAD_MAX_ENTRIES;
use crate::runtime::{
    ActorAddress, AddressResolver, ClusterAction, ClusterState, CrdtManager, NodeId, Packet, Value,
};
use tracing::warn;

/// Cap on the bare-id → node reverse index (`Runtime::remote_refs`). The
/// forward `RemoteActorCache` (10k, TTL-bounded) still covers explicit
/// `ActorAddress::remote` sends; the reverse index is best-effort on top.
const REMOTE_REFS_MAX: usize = 10_000;

/// A message sent to a spawn@node placeholder before its SpawnResponse
/// arrived. The payload is ALREADY in wire form — string ids rewritten to
/// table indices, contents captured in `string_table` — so the flush on
/// SpawnResponse doesn't need the sender's module-pool context (the
/// sender may have been re-entered by then).
pub(crate) struct PendingSpawnMessage {
    pub behavior_name: String,
    pub payload: Vec<Value>,
    pub string_table: Vec<String>,
    pub sender: u64,
    pub trace_id: Option<String>,
}

/// Queue a message for a spawn@node placeholder whose SpawnResponse has
/// not arrived yet. Resolves string payloads to content NOW (the sender
/// context is valid at send time); the SpawnResponse handler flushes the
/// queued messages to the real actor id.
pub(crate) fn queue_spawn_message(
    rt: &mut Runtime,
    request_id: u64,
    _node: NodeId,
    behavior: &str,
    args: &[Value],
) {
    let (payload, string_table) = match distributed::resolve_wire_strings(rt, args) {
        Some(resolved) => resolved,
        None => {
            warn!(
                "nulang-net: dropping message to spawn placeholder {}: string payload cannot be resolved to content (no sender module context)",
                request_id
            );
            let sender = rt.current_actor.unwrap_or(0);
            crate::runtime::distributed::notify_delivery_failed(
                rt,
                sender,
                "string payload unresolvable",
            );
            return;
        }
    };
    let sender = rt.current_actor.unwrap_or(0);
    let trace_id = rt.current_trace.as_ref().map(|t| t.to_traceparent());
    rt.pending_spawn_messages
        .entry(request_id)
        .or_default()
        .push(PendingSpawnMessage {
            behavior_name: behavior.to_string(),
            payload,
            string_table,
            sender,
            trace_id,
        });
}

/// Record `actor_id → node` in the reverse index (bounded; drops new
/// entries when full rather than growing without limit).
pub(crate) fn record_remote_ref(rt: &mut Runtime, node: NodeId, actor_id: u64) {
    if actor_id == 0 {
        return;
    }
    if rt.remote_refs.len() >= REMOTE_REFS_MAX && !rt.remote_refs.contains_key(&actor_id) {
        return;
    }
    rt.remote_refs.insert(actor_id, node);
}

/// Enable the distributed actor system, binding to `bind_addr` for incoming
/// connections and advertising ourselves under this address.
pub(crate) fn enable_distribution(
    rt: &mut Runtime,
    bind_addr: std::net::SocketAddr,
    tls_config: crate::runtime::network::TlsConfig,
) -> std::io::Result<()> {
    let transport = Box::new(crate::runtime::network::TcpTransport::bind(
        bind_addr, tls_config,
    )?);
    let listen_addr = transport.listen_addr();
    let node_id = NodeId(transport.node_id().0);
    let mut cluster = ClusterState::new(node_id, listen_addr);
    let _ = cluster.apply_config(&rt.cluster_config);
    if let Some(clock) = &rt.virtual_clock {
        cluster.set_clock(clock.clone());
    }
    let resolver = AddressResolver::new(node_id);
    rt.distributed.transport = Some(transport);
    rt.distributed.cluster = Some(cluster);
    rt.distributed.resolver = Some(resolver);
    rt.distributed.node_id = Some(node_id);
    rt.distributed.enabled = true;
    rt.crdt_manager = Some(CrdtManager::new(node_id.0));
    Ok(())
}

/// Join a cluster by connecting to a seed node.
pub(crate) fn join_cluster(rt: &mut Runtime, seed_addr: std::net::SocketAddr) {
    if let Some(cluster) = &mut rt.distributed.cluster {
        cluster.join_cluster(seed_addr);
    }
}

/// Register a behavior that can be spawned remotely.
pub(crate) fn register_spawnable_behavior(
    rt: &mut Runtime,
    name: &str,
    handler: fn(&mut crate::runtime::Actor, &[Value]),
) {
    rt.spawnable_behaviors.insert(name.to_string(), handler);
}

/// Retrieve the result of a remote spawn request.
pub(crate) fn take_spawn_response(rt: &mut Runtime, request_id: u64) -> Option<Option<u64>> {
    rt.pending_spawn_responses.remove(&request_id)
}

/// Check whether a packet with the given sequence number has been acknowledged.
pub(crate) fn is_acked(rt: &Runtime, seq: u64) -> bool {
    rt.acked_packets.contains(&seq)
}

/// Drain all acknowledged packet sequence numbers.
pub(crate) fn drain_acked(rt: &mut Runtime) -> std::collections::HashSet<u64> {
    std::mem::take(&mut rt.acked_packets)
}

/// Send a message to a (possibly remote) actor through location-transparent
/// addressing. Falls back to local `send_message` when distribution is
/// disabled.
pub(crate) fn send_distributed(
    rt: &mut Runtime,
    target: ActorAddress,
    behavior: &str,
    args: &[Value],
) {
    if !rt.distributed.enabled {
        let actor_id = match target {
            ActorAddress::Local { actor_id } => actor_id,
            ActorAddress::Remote { actor_id, .. } => actor_id,
        };
        rt.send_message(actor_id, behavior, args);
        return;
    }
    if let ActorAddress::Local { actor_id } = target {
        rt.send_message(actor_id, behavior, args);
        return;
    }
    let mut transport = match rt.distributed.transport.take() {
        Some(t) => t,
        None => return,
    };
    let cluster = match rt.distributed.cluster.take() {
        Some(c) => c,
        None => {
            rt.distributed.transport = Some(transport);
            return;
        }
    };
    let mut resolver = match rt.distributed.resolver.take() {
        Some(r) => r,
        None => {
            rt.distributed.transport = Some(transport);
            rt.distributed.cluster = Some(cluster);
            return;
        }
    };
    distributed::send_distributed(
        rt,
        &mut transport,
        &cluster,
        &mut resolver,
        target,
        behavior,
        args,
    );
    rt.distributed.transport = Some(transport);
    rt.distributed.cluster = Some(cluster);
    rt.distributed.resolver = Some(resolver);
}

/// Process incoming network packets and cluster actions.
pub(crate) fn process_network(rt: &mut Runtime) {
    if !rt.distributed.enabled {
        return;
    }
    let mut transport = match rt.distributed.transport.take() {
        Some(t) => t,
        None => return,
    };
    let mut cluster = match rt.distributed.cluster.take() {
        Some(c) => c,
        None => {
            rt.distributed.transport = Some(transport);
            return;
        }
    };
    let mut resolver = match rt.distributed.resolver.take() {
        Some(r) => r,
        None => {
            rt.distributed.transport = Some(transport);
            rt.distributed.cluster = Some(cluster);
            return;
        }
    };
    distributed::process_network_packets(rt, &mut transport, &mut cluster, &mut resolver);
    rt.distributed.transport = Some(transport);
    rt.distributed.cluster = Some(cluster);
    rt.distributed.resolver = Some(resolver);
    let actions = {
        if let Some(cluster) = rt.distributed.cluster.as_mut() {
            cluster.tick()
        } else {
            Vec::new()
        }
    };
    for action in actions {
        match action {
            ClusterAction::SendHeartbeat { to, addr } => {
                if let Some(transport) = &mut rt.distributed.transport {
                    let local_id = rt.distributed.node_id.unwrap_or(NodeId::LOCAL);
                    let packet = Packet::Heartbeat {
                        node_id: local_id,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    };
                    transport.send(NodeId(to.0), addr, packet);
                }
            }
            ClusterAction::NodeJoined { node, addr } => {
                if let Some(transport) = &mut rt.distributed.transport {
                    let net_node_id = NodeId(node.0);
                    let _ = transport.connect(net_node_id, addr);
                }
            }
            ClusterAction::NodeFailed { node } => {
                if let Some(transport) = &mut rt.distributed.transport {
                    let net_node_id = NodeId(node.0);
                    transport.disconnect(net_node_id);
                }
                handle_node_failed(rt, NodeId(node.0));
            }
            ClusterAction::NodeLeft { node } => {
                if let Some(transport) = &mut rt.distributed.transport {
                    let net_node_id = NodeId(node.0);
                    transport.disconnect(net_node_id);
                }
            }
            ClusterAction::SendGossip { targets } => {
                if let (Some(transport), Some(cluster)) =
                    (&mut rt.distributed.transport, &rt.distributed.cluster)
                {
                    let members = cluster.gossip_payload(GOSSIP_PAYLOAD_MAX_ENTRIES);
                    if !members.is_empty() {
                        let packet = Packet::Gossip { members };
                        for (to, addr) in targets {
                            transport.send(NodeId(to.0), addr, packet.clone());
                        }
                    }
                }
            }
            ClusterAction::Probe { to, addr } => {
                // A minimal liveness probe to a Failed member: an ordinary
                // Heartbeat packet (no new wire type). If the peer is alive
                // again, its own heartbeat replies re-promote it via
                // `handle_heartbeat` — the self-healing path for a healed
                // partition, no external rejoin needed.
                if let Some(transport) = &mut rt.distributed.transport {
                    let local_id = rt.distributed.node_id.unwrap_or(NodeId::LOCAL);
                    let packet = Packet::Heartbeat {
                        node_id: local_id,
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    };
                    transport.send(NodeId(to.0), addr, packet);
                }
            }
            ClusterAction::Down { node } => {
                // The split-brain resolver decided the local node should
                // leave the cluster. The cluster's `local_down` flag already
                // silences tick(); shut the transport down to end all
                // network participation. Local actors keep running.
                warn!(
                    "nulang-net: split-brain resolver downed local node {:?}; \
                     leaving the cluster",
                    node
                );
                if let Some(transport) = &mut rt.distributed.transport {
                    transport.shutdown();
                }
            }
        }
    }
}

/// React to a peer node being declared `Failed` by the failure detector:
///
/// 1. Invalidate the `RemoteActorCache` entries for that node so sends to
///    its actors fail fast instead of stale-resolving to a dead node.
/// 2. Deliver `DOWN`-with-`noconnection` system messages to every local
///    actor that had linked or monitored an actor known to live on the
///    failed node (Erlang's `{'DOWN', ..., noconnection}` for node loss),
///    and drop the now-dead registry entries.
///
/// Deliberately NOT implemented here: automatic re-spawn of durable actors
/// on another node. Without an explicit supervisor policy confirming the
/// old node is actually gone (not merely partitioned), re-spawn risks two
/// live copies of the same durable-id actor writing to the same store —
/// the same safety gate Kubernetes StatefulSet pod rescheduling enforces.
pub(crate) fn handle_node_failed(rt: &mut Runtime, node: NodeId) {
    // (1) Invalidate cached remote actors on the failed node.
    if let Some(resolver) = rt.distributed.resolver.as_mut() {
        resolver.invalidate_node(node);
    }

    // (2) DOWN-with-noconnection to local watchers of actors on the node.
    let local_node = rt.distributed.node_id.unwrap_or(NodeId::LOCAL);
    let link_pairs = rt.remote_links.clear_node(node);
    let monitor_pairs = rt.remote_monitors.clear_node(node);
    let reason = crate::types::ExitReason::NoConnection;
    for (target, watcher) in link_pairs {
        if watcher.node_id == local_node {
            crate::runtime::exit::send_down_message(rt, watcher.actor_id, target.actor_id, &reason);
        }
    }
    for (target, watcher) in monitor_pairs {
        if watcher.node_id == local_node {
            crate::runtime::exit::send_down_message(rt, watcher.actor_id, target.actor_id, &reason);
        }
    }
}

/// Synchronize CRDT state with all healthy cluster members using delta-state
/// replication, with a periodic full-state repair every
/// `CRDT_FULL_SYNC_INTERVAL` rounds.
pub(crate) fn sync_crdts(rt: &mut Runtime) {
    // Causal-stability tombstone GC runs on every round, clustered or not:
    // with no peers the watermark collapses to the local replica's own
    // observation (everything it holds is trivially stable), which bounds
    // tombstone growth for standalone use too.
    if let Some(mgr) = &mut rt.crdt_manager {
        let healthy: Vec<u64> = rt
            .distributed
            .cluster
            .as_ref()
            .map(|c| c.healthy_members().iter().map(|m| m.node_id.0).collect())
            .unwrap_or_default();
        mgr.gc_stable_tombstones(&healthy);
    }
    if !rt.distributed.enabled {
        return;
    }
    rt.crdt_sync_rounds = rt.crdt_sync_rounds.wrapping_add(1);
    if crdt_sync_is_full_round(rt.crdt_sync_rounds) {
        rt.sync_crdts_full();
    } else {
        crate::runtime::distributed::sync_crdts_delta(rt);
    }
    crate::runtime::distributed::sync_crdts_op(rt);
    rt.sweep_migrated_actors();
    rt.publish_metrics();
}

/// True when the given 1-based sync round should ship full state.
pub(crate) fn crdt_sync_is_full_round(round: u64) -> bool {
    round % crate::runtime::CRDT_FULL_SYNC_INTERVAL == 1
}
