//! Distributed actor system context extracted from the Runtime god-object.
//!
//! Groups the fields that support location-transparent message passing,
//! cluster membership, gossip, and remote spawn.

use crate::runtime::AddressResolver;
use crate::runtime::cluster::{ClusterState, NodeId};
use crate::runtime::network::NetworkTransport;

/// Distributed-subsystem state owned by [`Runtime`](super::Runtime).
#[derive(Default)]
pub struct DistributedContext {
    pub transport: Option<NetworkTransport>,
    pub cluster: Option<ClusterState>,
    pub resolver: Option<AddressResolver>,
    pub node_id: Option<NodeId>,
    pub enabled: bool,
}

impl DistributedContext {
    pub fn new() -> Self {
        Self::default()
    }
}
