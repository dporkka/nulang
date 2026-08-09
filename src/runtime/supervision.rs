//! Cross-node supervision: tracking remote links and monitors.

use super::cluster::NodeId;
use std::collections::{HashMap, HashSet};

/// A remote link (symmetric).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RemoteLink {
    pub node_id: NodeId,
    pub actor_id: u64,
}

/// Tracking for cross-node links.
#[derive(Default, Debug)]
pub struct RemoteLinkRegistry {
    // Map (target_node, target_actor) -> set of (watcher_node, watcher_actor)
    pub links: HashMap<(NodeId, u64), HashSet<RemoteLink>>,
}

impl RemoteLinkRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, target: RemoteLink, watcher: RemoteLink) {
        self.links
            .entry((target.node_id, target.actor_id))
            .or_default()
            .insert(watcher);
    }

    pub fn unregister(&mut self, target: RemoteLink, watcher: RemoteLink) {
        if let Some(set) = self.links.get_mut(&(target.node_id, target.actor_id)) {
            set.remove(&watcher);
        }
    }

    pub fn get_watchers(&self, target: RemoteLink) -> Option<&HashSet<RemoteLink>> {
        self.links.get(&(target.node_id, target.actor_id))
    }

    /// Remove all watchers for a given target (e.g. when the target exits).
    pub fn clear_target(&mut self, target: RemoteLink) {
        self.links.remove(&(target.node_id, target.actor_id));
    }

    /// Remove all entries whose target is on the given node.
    ///
    /// Returns `(target, watcher)` pairs so callers can deliver a
    /// node-failure `DOWN` to each watcher with the target actor id it was
    /// watching.
    pub fn clear_node(&mut self, node_id: NodeId) -> Vec<(RemoteLink, RemoteLink)> {
        let mut watchers = Vec::new();
        self.links.retain(|&(n, actor_id), w| {
            if n == node_id {
                let target = RemoteLink {
                    node_id: n,
                    actor_id,
                };
                for watcher in w.iter() {
                    watchers.push((target, *watcher));
                }
                false
            } else {
                true
            }
        });
        watchers
    }
}

/// Tracking for cross-node monitors (asymmetric).
#[derive(Default, Debug)]
pub struct RemoteMonitorRegistry {
    // Map (target_node, target_actor) -> set of (watcher_node, watcher_actor)
    pub monitors: HashMap<(NodeId, u64), HashSet<RemoteLink>>,
}

impl RemoteMonitorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, target: RemoteLink, watcher: RemoteLink) {
        self.monitors
            .entry((target.node_id, target.actor_id))
            .or_default()
            .insert(watcher);
    }

    pub fn unregister(&mut self, target: RemoteLink, watcher: RemoteLink) {
        if let Some(set) = self.monitors.get_mut(&(target.node_id, target.actor_id)) {
            set.remove(&watcher);
        }
    }

    pub fn get_watchers(&self, target: RemoteLink) -> Option<&HashSet<RemoteLink>> {
        self.monitors.get(&(target.node_id, target.actor_id))
    }

    /// Remove all entries whose target is on the given node.
    ///
    /// Returns `(target, watcher)` pairs so callers can deliver a
    /// node-failure `DOWN` to each watcher with the target actor id it was
    /// watching.
    pub fn clear_node(&mut self, node_id: NodeId) -> Vec<(RemoteLink, RemoteLink)> {
        let mut watchers = Vec::new();
        self.monitors.retain(|&(n, actor_id), w| {
            if n == node_id {
                let target = RemoteLink {
                    node_id: n,
                    actor_id,
                };
                for watcher in w.iter() {
                    watchers.push((target, *watcher));
                }
                false
            } else {
                true
            }
        });
        watchers
    }

    /// Remove all watchers for a given target (e.g. when the target exits).
    pub fn clear_target(&mut self, target: RemoteLink) {
        self.monitors.remove(&(target.node_id, target.actor_id));
    }
}
