//! Distribution subsystem: network transport, cluster membership, CRDT sync,
//! remote spawn, and gossip. These free functions orchestrate the distributed
//! context (transport, cluster, resolver) owned by `Runtime`.

use crate::runtime::distributed;
use crate::runtime::Runtime;
use crate::runtime::{
    ActorAddress, AddressResolver, ClusterAction, ClusterState, CrdtManager, NodeId, Packet, Value,
};
use crate::runtime::{GOSSIP_PAYLOAD_MAX_ENTRIES};

/// Enable the distributed actor system, binding to `bind_addr` for incoming
/// connections and advertising ourselves under this address.
pub(crate) fn enable_distribution(
    rt: &mut Runtime,
    bind_addr: std::net::SocketAddr,
) -> std::io::Result<()> {
    let transport = Box::new(crate::runtime::network::TcpTransport::bind(bind_addr)?);
    let listen_addr = transport.listen_addr();
    let node_id = NodeId(transport.node_id().0);
    let cluster = ClusterState::new(node_id, listen_addr);
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
    rt.spawnable_behaviors
        .insert(name.to_string(), handler);
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
        }
    }
}


/// Synchronize CRDT state with all healthy cluster members using delta-state
/// replication, with a periodic full-state repair every
/// `CRDT_FULL_SYNC_INTERVAL` rounds.
pub(crate) fn sync_crdts(rt: &mut Runtime) {
    if !rt.distributed.enabled {
        return;
    }
    rt.crdt_sync_rounds = rt.crdt_sync_rounds.wrapping_add(1);
    if crdt_sync_is_full_round(rt.crdt_sync_rounds) {
        rt.sync_crdts_full();
    } else {
        crate::runtime::distributed::sync_crdts_delta(rt);
    }
}

/// True when the given 1-based sync round should ship full state.
pub(crate) fn crdt_sync_is_full_round(round: u64) -> bool {
    round % crate::runtime::CRDT_FULL_SYNC_INTERVAL == 1
}
