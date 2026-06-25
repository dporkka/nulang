//! Actor runtime system for Nulang.
//!
//! Provides: actor lifecycle, scheduler, mailbox, heap, GC, supervision,
//! distribution.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

mod actor;
mod scheduler;
mod mailbox;
mod heap;
mod gc;
mod orca_cycle;
mod supervisor;
mod cluster;
mod network;
mod distributed;
mod crdt;
mod crdt_reg;
mod crdt_manager;
mod timer;
mod registry;
mod process_groups;

#[cfg(test)]
mod tests;

pub use actor::*;
pub use scheduler::*;
pub use mailbox::*;
pub use heap::*;
pub use gc::*;
pub use supervisor::*;
pub use orca_cycle::*;
pub use cluster::*;
pub use distributed::*;
pub use network::*;
pub use crdt::*;
pub use crdt_reg::*;
pub use crdt_manager::*;
pub use timer::*;
pub use registry::*;
pub use process_groups::*;

use crate::types::ExitReason;
use crate::vm::Value;

// ---------------------------------------------------------------------------
// Actor ID Generation
// ---------------------------------------------------------------------------

static ACTOR_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn fresh_actor_id() -> u64 {
    ACTOR_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

pub struct Runtime {
    pub actors: HashMap<u64, Actor>,
    pub supervisors: HashMap<u64, Supervisor>,
    pub scheduler: Scheduler,
    pub current_actor: Option<u64>,
    pub next_reductions: u32,
    pub coordinator: OrcaCoordinator,
    pub cycle_detector: CycleDetector,

    // Distributed actor system (v0.5)
    pub transport: Option<NetworkTransport>,
    pub cluster: Option<ClusterState>,
    pub resolver: Option<AddressResolver>,
    pub node_id: Option<cluster::NodeId>,
    pub distributed_enabled: bool,

    // CRDT manager (v0.6)
    pub crdt_manager: Option<CrdtManager>,

    // Timer wheel (v0.7)
    pub timer_wheel: TimerWheel,

    // Actor name registry (v0.7)
    pub registry: ActorRegistry,

    // Process groups (v0.7)
    pub process_groups: ProcessGroups,
}

impl Runtime {
    pub fn new() -> Self {
        Runtime {
            actors: HashMap::new(),
            supervisors: HashMap::new(),
            scheduler: Scheduler::new(4),
            current_actor: None,
            next_reductions: 1000,
            coordinator: OrcaCoordinator::new(),
            cycle_detector: CycleDetector::new(),

            transport: None,
            cluster: None,
            resolver: None,
            node_id: None,
            distributed_enabled: false,

            crdt_manager: None,

            timer_wheel: TimerWheel::new(),
            registry: ActorRegistry::new(),
            process_groups: ProcessGroups::new(),
        }
    }

    pub fn spawn_actor(
        &mut self,
        init: Box<dyn FnOnce() -> Vec<(String, Value)>>,
    ) -> u64 {
        let id = fresh_actor_id();
        let mut actor = Actor::new(id, format!("actor_{}", id), 256);
        let state_fields = init();
        for (name, value) in state_fields {
            actor.set_state_field(name, value);
        }
        actor.state = ActorState::Running;
        self.actors.insert(id, actor);
        self.scheduler.enqueue(id);
        id
    }

    pub fn send_message(&mut self, target_id: u64, behavior: &str, args: &[Value]) {
        let actor = match self.actors.get(&target_id) {
            Some(a) => a,
            None => return,
        };
        let behavior_id = actor
            .behavior_table
            .iter()
            .position(|entry| entry.name == behavior)
            .map(|idx| idx as u16)
            .unwrap_or(0);
        let msg = Message {
            behavior_id,
            payload: args.to_vec(),
            sender: self.current_actor.unwrap_or(0),
            priority: MessagePriority::Normal,
        };
        if let Some(actor) = self.actors.get_mut(&target_id) {
            if let Err(_dropped) = actor.mailbox.push(msg) {}
        }
        for arg in args {
            if let Some(ptr) = arg.as_ptr::<u8>() {
                if let Some(source_actor_id) = self.current_actor {
                    if let Some(source_actor) = self.actors.get_mut(&source_actor_id) {
                        let op = unsafe {
                            source_actor.orca_gc.send_ref_to(
                                &source_actor.heap,
                                ptr,
                                target_id,
                            )
                        };
                        self.coordinator.submit_op(op);
                    }
                }
            }
        }
        self.scheduler.enqueue(target_id);
    }

    pub fn process_gc_ops(&mut self) {
        let ops = std::mem::take(&mut self.coordinator.pending_ops);
        for op in ops {
            if let Some(target_actor) = self.actors.get_mut(&op.target_actor) {
                target_actor.orca_gc.process_foreign_op(&mut target_actor.heap, op);
            }
        }
        let should_detect = self.cycle_detector.should_detect();
        if should_detect {
            let rt = self as *const Runtime;
            let detector = &mut self.cycle_detector;
            unsafe {
                detector.incremental_detect(&*rt);
            }
        }
    }

    pub fn gc_stats(&self) -> super::gc::GcStats {
        let mut total = super::gc::GcStats::default();
        for actor in self.actors.values() {
            let stats = actor.orca_gc.stats();
            total.objects_allocated.fetch_add(
                stats.objects_allocated.load(Ordering::Relaxed), Ordering::Relaxed);
            total.objects_freed.fetch_add(
                stats.objects_freed.load(Ordering::Relaxed), Ordering::Relaxed);
            total.local_refs_created.fetch_add(
                stats.local_refs_created.load(Ordering::Relaxed), Ordering::Relaxed);
            total.local_refs_dropped.fetch_add(
                stats.local_refs_dropped.load(Ordering::Relaxed), Ordering::Relaxed);
            total.foreign_refs_sent.fetch_add(
                stats.foreign_refs_sent.load(Ordering::Relaxed), Ordering::Relaxed);
            total.foreign_refs_received.fetch_add(
                stats.foreign_refs_received.load(Ordering::Relaxed), Ordering::Relaxed);
            total.cycles_detected.fetch_add(
                stats.cycles_detected.load(Ordering::Relaxed), Ordering::Relaxed);
            total.bytes_allocated.fetch_add(
                stats.bytes_allocated.load(Ordering::Relaxed), Ordering::Relaxed);
            total.bytes_freed.fetch_add(
                stats.bytes_freed.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        total
    }

    pub fn current_actor_id(&self) -> Option<u64> {
        self.current_actor
    }

    pub fn run_scheduler(&mut self) {
        while let Some(actor_id) = self.scheduler.dequeue() {
            self.step_actor(actor_id);
        }
    }

    pub fn step_actor(&mut self, actor_id: u64) {
        self.current_actor = Some(actor_id);
        let msg_opt = {
            let actor = match self.actors.get_mut(&actor_id) {
                Some(a) => a,
                None => {
                    self.current_actor = None;
                    return;
                }
            };
            match actor.state {
                ActorState::Running | ActorState::Created | ActorState::Waiting => {
                    actor.receive()
                }
                _ => {
                    self.current_actor = None;
                    return;
                }
            }
        };
        let should_requeue = if let Some(msg) = msg_opt {
            let handler_fn: Option<fn(&mut Actor, &[Value])> = {
                let actor = match self.actors.get(&actor_id) {
                    Some(a) => a,
                    None => {
                        self.current_actor = None;
                        return;
                    }
                };
                let behavior_idx = msg.behavior_id as usize;
                if behavior_idx < actor.behavior_table.len() {
                    Some(actor.behavior_table[behavior_idx].handler_fn)
                } else {
                    None
                }
            };
            if let Some(handler) = handler_fn {
                let actor = match self.actors.get_mut(&actor_id) {
                    Some(a) => a,
                    None => {
                        self.current_actor = None;
                        return;
                    }
                };
                handler(actor, &msg.payload);
            }
            let actor = match self.actors.get_mut(&actor_id) {
                Some(a) => a,
                None => {
                    self.current_actor = None;
                    return;
                }
            };
            actor.reduction_count += 1;
            !actor.mailbox.is_empty() && !actor.should_yield()
        } else {
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                if actor.state == ActorState::Running {
                    actor.state = ActorState::Waiting;
                }
            }
            false
        };
        if should_requeue {
            self.scheduler.enqueue(actor_id);
        }
        self.current_actor = None;
    }

    // -- Fault Tolerance: Links --

    pub fn link_actors(&mut self, a: u64, b: u64) {
        if a == b { return; }
        if let Some(actor_a) = self.actors.get_mut(&a) {
            if !actor_a.links.contains(&b) { actor_a.links.push(b); }
        }
        if let Some(actor_b) = self.actors.get_mut(&b) {
            if !actor_b.links.contains(&a) { actor_b.links.push(a); }
        }
    }

    pub fn unlink_actors(&mut self, a: u64, b: u64) {
        if let Some(actor_a) = self.actors.get_mut(&a) {
            actor_a.links.retain(|&id| id != b);
        }
        if let Some(actor_b) = self.actors.get_mut(&b) {
            actor_b.links.retain(|&id| id != a);
        }
    }

    // -- Fault Tolerance: Monitors --

    pub fn monitor(&mut self, watcher: u64, target: u64) {
        if watcher == target { return; }
        if let Some(actor) = self.actors.get_mut(&target) {
            if !actor.monitors.contains(&watcher) { actor.monitors.push(watcher); }
        } else {
            self.send_down_message(watcher, target, &ExitReason::Error("noproc".to_string()));
        }
    }

    pub fn demonitor(&mut self, watcher: u64, target: u64) {
        if let Some(actor) = self.actors.get_mut(&target) {
            actor.monitors.retain(|&id| id != watcher);
        }
    }

    // -- Fault Tolerance: Actor Exit --

    pub fn exit_actor(&mut self, actor_id: u64, reason: ExitReason) {
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            actor.state = ActorState::Terminated;
        }
        let reason_clone = reason.clone();
        self.handle_actor_exit(actor_id, reason_clone);
    }

    pub fn kill_actor(&mut self, actor_id: u64) {
        self.exit_actor(actor_id, ExitReason::Kill);
    }

    pub fn handle_actor_exit(&mut self, actor_id: u64, reason: ExitReason) {
        let (monitors, links, parent) = {
            let actor = match self.actors.get(&actor_id) {
                Some(a) => a,
                None => return,
            };
            (actor.monitors.clone(), actor.links.clone(), actor.parent)
        };

        self.registry.unregister_by_actor(actor_id);
        self.process_groups.leave_all(actor_id);

        for watcher_id in monitors {
            self.send_down_message(watcher_id, actor_id, &reason);
        }

        let is_abnormal = !matches!(reason, ExitReason::Normal);
        for linked_id in links {
            if linked_id == actor_id { continue; }
            let linked_alive = self.actors.get(&linked_id).map(|a| a.state != ActorState::Terminated).unwrap_or(false);
            if !linked_alive { continue; }

            if is_abnormal {
                let traps = self.actors.get(&linked_id).map(|a| a.trap_exits).unwrap_or(false);
                if traps {
                    let exit_msg = Message {
                        behavior_id: 0,
                        payload: vec![Value::int(actor_id as i64), Value::int(linked_id as i64)],
                        sender: actor_id,
                        priority: MessagePriority::System,
                    };
                    if let Some(actor) = self.actors.get_mut(&linked_id) {
                        let _ = actor.mailbox.push(exit_msg);
                    }
                    self.scheduler.enqueue(linked_id);
                } else {
                    let linked_reason = ExitReason::Error(format!("linked actor {} exited with {:?}", actor_id, reason));
                    if let Some(actor) = self.actors.get_mut(&linked_id) {
                        actor.state = ActorState::Terminated;
                    }
                    self.handle_actor_exit(linked_id, linked_reason);
                }
            }
        }

        if let Some(supervisor_id) = parent {
            let mut supervisor = match self.supervisors.remove(&supervisor_id) {
                Some(s) => s,
                None => {
                    self.actors.remove(&actor_id);
                    return;
                }
            };
            let action = supervisor.handle_exit(actor_id, reason.clone(), self);
            match action {
                SupervisorAction::Restarted(_new_id) => {
                    self.supervisors.insert(supervisor_id, supervisor);
                }
                SupervisorAction::Shutdown => {
                    let sup_parent = supervisor.parent;
                    self.shutdown_supervisor(supervisor_id);
                    if let Some(parent_id) = sup_parent {
                        let escalate_reason = ExitReason::Error("child supervisor shutdown".to_string());
                        self.handle_supervisor_parent_exit(parent_id, supervisor_id, escalate_reason);
                    }
                }
                SupervisorAction::Ignore => {
                    self.supervisors.insert(supervisor_id, supervisor);
                }
                SupervisorAction::Escalate => {
                    self.supervisors.insert(supervisor_id, supervisor);
                    if let Some(parent_id) = parent {
                        let escalate_reason = reason.clone();
                        self.handle_supervisor_parent_exit(parent_id, actor_id, escalate_reason);
                    }
                }
            }
        } else {
            self.actors.remove(&actor_id);
        }
    }

    fn handle_supervisor_parent_exit(
        &mut self,
        parent_id: u64,
        child_supervisor_id: u64,
        reason: ExitReason,
    ) {
        let mut parent_sup = match self.supervisors.remove(&parent_id) {
            Some(s) => s,
            None => return,
        };
        let parent_action = parent_sup.handle_exit(child_supervisor_id, reason, self);
        match parent_action {
            SupervisorAction::Shutdown => {
                let grandparent = parent_sup.parent;
                self.shutdown_supervisor(parent_id);
                if let Some(gp_id) = grandparent {
                    let gp_reason = ExitReason::Error("supervisor shutdown cascaded".to_string());
                    self.handle_supervisor_parent_exit(gp_id, parent_id, gp_reason);
                }
            }
            _ => {
                self.supervisors.insert(parent_id, parent_sup);
            }
        }
    }

    // -- Supervisor Management --

    pub fn create_supervisor(&mut self, name: &str, strategy: RestartStrategy) -> u64 {
        let id = fresh_actor_id();
        let mut actor = Actor::new(id, name.to_string(), 256);
        actor.state = ActorState::Running;
        self.actors.insert(id, actor);
        let supervisor = Supervisor::new(id, name, strategy);
        self.supervisors.insert(id, supervisor);
        self.scheduler.enqueue(id);
        id
    }

    pub fn supervise_child(&mut self, supervisor_id: u64, spec: ChildSpec, child_id: u64) {
        if let Some(child) = self.actors.get_mut(&child_id) {
            child.parent = Some(supervisor_id);
        }
        if let Some(supervisor) = self.supervisors.get_mut(&supervisor_id) {
            supervisor.add_child(spec, child_id);
        }
    }

    // -- Internal Helpers --

    fn send_down_message(&mut self, watcher_id: u64, target_id: u64, reason: &ExitReason) {
        let reason_str = reason.tag();
        let down_msg = Message {
            behavior_id: 0,
            payload: vec![
                Value::int(target_id as i64),
                Value::int(watcher_id as i64),
                Value::int(match reason {
                    ExitReason::Normal => 0,
                    ExitReason::Error(_) => 1,
                    ExitReason::Kill => 2,
                    ExitReason::Killed => 3,
                    ExitReason::Shutdown(_) => 4,
                    ExitReason::Custom(_) => 5,
                }),
            ],
            sender: target_id,
            priority: MessagePriority::System,
        };
        if let Some(watcher) = self.actors.get_mut(&watcher_id) {
            let _ = watcher.mailbox.push(down_msg);
            let _ = reason_str;
        }
        self.scheduler.enqueue(watcher_id);
    }

    fn shutdown_supervisor(&mut self, supervisor_id: u64) {
        let child_ids: Vec<u64> = self.supervisors.get(&supervisor_id).map(|s| s.children.iter().map(|(_, id)| *id).collect()).unwrap_or_default();
        for child_id in child_ids {
            self.actors.remove(&child_id);
        }
        self.actors.remove(&supervisor_id);
        self.supervisors.remove(&supervisor_id);
    }

    // -- Distributed Actor System --

    pub fn enable_distribution(&mut self, bind_addr: std::net::SocketAddr) -> std::io::Result<()> {
        let transport = NetworkTransport::bind(bind_addr)?;
        let node_id = cluster::NodeId(transport.node_id().0);
        let cluster = ClusterState::new(node_id, bind_addr);
        let resolver = AddressResolver::new(node_id);
        self.transport = Some(transport);
        self.cluster = Some(cluster);
        self.resolver = Some(resolver);
        self.node_id = Some(node_id);
        self.distributed_enabled = true;
        self.crdt_manager = Some(CrdtManager::new(node_id.0));
        Ok(())
    }

    pub fn join_cluster(&mut self, seed_addr: std::net::SocketAddr) {
        if let Some(cluster) = &mut self.cluster {
            cluster.join_cluster(seed_addr);
        }
    }

    pub fn send_distributed(&mut self, target: ActorAddress, behavior: &str, args: &[Value]) {
        if !self.distributed_enabled {
            let actor_id = match target {
                ActorAddress::Local { actor_id } => actor_id,
                ActorAddress::Remote { actor_id, .. } => actor_id,
            };
            self.send_message(actor_id, behavior, args);
            return;
        }
        if let ActorAddress::Local { actor_id } = target {
            self.send_message(actor_id, behavior, args);
            return;
        }
        let mut transport = self.transport.take().unwrap();
        let cluster = self.cluster.take().unwrap();
        let mut resolver = self.resolver.take().unwrap();
        distributed::send_distributed(self, &mut transport, &cluster, &mut resolver, target, behavior, args);
        self.transport = Some(transport);
        self.cluster = Some(cluster);
        self.resolver = Some(resolver);
    }

    pub fn process_network(&mut self) {
        if !self.distributed_enabled { return; }
        let transport = self.transport.as_ref().unwrap();
        let mut cluster = self.cluster.take().unwrap();
        let mut resolver = self.resolver.take().unwrap();
        distributed::process_network_packets(self, transport, &mut cluster, &mut resolver);
        self.cluster = Some(cluster);
        self.resolver = Some(resolver);
        let actions = {
            let cluster = self.cluster.as_mut().unwrap();
            cluster.tick()
        };
        for action in actions {
            match action {
                ClusterAction::SendHeartbeat { to, addr } => {
                    if let Some(transport) = &mut self.transport {
                        let net_node_id = network::NodeId(to.0);
                        let packet = Packet::Heartbeat {
                            node_id: net_node_id,
                            timestamp: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
                        };
                        transport.send(net_node_id, addr, packet);
                    }
                }
                ClusterAction::NodeJoined { node, addr } => {
                    if let Some(transport) = &mut self.transport {
                        let net_node_id = network::NodeId(node.0);
                        let _ = transport.connect(net_node_id, addr);
                    }
                }
                ClusterAction::NodeFailed { node } => {
                    if let Some(transport) = &mut self.transport {
                        let net_node_id = network::NodeId(node.0);
                        transport.disconnect(net_node_id);
                    }
                }
                ClusterAction::NodeLeft { node } => {
                    if let Some(transport) = &mut self.transport {
                        let net_node_id = network::NodeId(node.0);
                        transport.disconnect(net_node_id);
                    }
                }
                ClusterAction::SendGossip { .. } => {}
            }
        }
    }

    // -- CRDT Synchronization (v0.6) --

    pub fn sync_crdts(&mut self) {
        if !self.distributed_enabled { return; }
        let ops = match &mut self.crdt_manager {
            Some(m) => m.generate_sync_ops(),
            None => return,
        };
        if ops.is_empty() { return; }
        let packet = Packet::CrdtSync { ops };
        if let Some(cluster) = &self.cluster {
            for member in cluster.healthy_members() {
                if let Some(transport) = &mut self.transport {
                    let net_node_id = network::NodeId(member.node_id.0);
                    transport.send(net_node_id, member.address, packet.clone());
                }
            }
        }
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
