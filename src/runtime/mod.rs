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

use crate::vm::Value;
// Note: OrcaCoordinator, OrcaGc, ForeignRefOp, and CycleDetector are
// brought into scope by the `pub use gc::*;` and `pub use orca_cycle::*;`
// re-exports above.

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
    pub current_actor: Option<u64>, // Which actor is currently running
    pub next_reductions: u32,       // Remaining reductions before yield
    pub coordinator: OrcaCoordinator,  // NEW: ORCA coordinator for cross-actor GC
    pub cycle_detector: CycleDetector, // NEW: incremental cycle detection

    // Distributed actor system (v0.5)
    pub transport: Option<NetworkTransport>,      // Network transport (None if not bound)
    pub cluster: Option<ClusterState>,            // Cluster membership (None if not in cluster)
    pub resolver: Option<AddressResolver>,        // Address resolver for distributed routing
    pub node_id: Option<cluster::NodeId>,          // This node's ID (None for local-only mode)
    pub distributed_enabled: bool,                // Flag to enable/disable distributed mode
}

impl Runtime {
    pub fn new() -> Self {
        Runtime {
            actors: HashMap::new(),
            supervisors: HashMap::new(),
            scheduler: Scheduler::new(4), // 4 worker threads default
            current_actor: None,
            next_reductions: 1000, // Default reduction quota
            coordinator: OrcaCoordinator::new(),  // NEW: ORCA coordinator
            cycle_detector: CycleDetector::new(), // NEW: cycle detector

            // Distributed actor system (disabled by default)
            transport: None,
            cluster: None,
            resolver: None,
            node_id: None,
            distributed_enabled: false,
        }
    }

    /// Spawn a new actor with a fresh ID.
    ///
    /// The `init` closure is called to produce the initial state fields
    /// (name-value pairs). The actor is initialized with `ActorState::Running`,
    /// added to the actors map, and enqueued in the scheduler.
    ///
    /// Returns the new actor's unique ID.
    pub fn spawn_actor(
        &mut self,
        init: Box<dyn FnOnce() -> Vec<(String, Value)>>,
    ) -> u64 {
        let id = fresh_actor_id();
        let mut actor = Actor::new(id, format!("actor_{}", id), 256);

        // Initialize state from the closure
        let state_fields = init();
        for (name, value) in state_fields {
            actor.set_state_field(name, value);
        }

        actor.state = ActorState::Running;
        self.actors.insert(id, actor);
        self.scheduler.enqueue(id);
        id
    }

    /// Send a message to a target actor.
    ///
    /// Looks up the target actor by ID, resolves the behavior name to a
    /// behavior index, constructs a `Message`, and pushes it into the
    /// target's mailbox. The actor is then enqueued in the scheduler.
    ///
    /// If the target actor does not exist, the message is silently dropped (MVP).
    pub fn send_message(&mut self, target_id: u64, behavior: &str, args: &[Value]) {
        // Look up the target actor
        let actor = match self.actors.get(&target_id) {
            Some(a) => a,
            None => return, // Actor not found - silently drop in MVP
        };

        // Find behavior ID by name (index in behavior_table)
        let behavior_id = actor
            .behavior_table
            .iter()
            .position(|entry| entry.name == behavior)
            .map(|idx| idx as u16)
            .unwrap_or(0); // Default to 0 if behavior not found

        let msg = Message {
            behavior_id,
            payload: args.to_vec(),
            sender: self.current_actor.unwrap_or(0),
            priority: MessagePriority::Normal,
        };

        // Push message to mailbox
        // Note: we need mutable access to the actor, so look it up again mutably
        if let Some(actor) = self.actors.get_mut(&target_id) {
            if let Err(_dropped) = actor.mailbox.push(msg) {
                // Message dropped due to overflow policy
                // In MVP, silently drop
            }
        }

        // ------------------------------------------------------------------
        // ORCA protocol: when sending references across actors, increment
        // foreign count and queue the operation for delivery.
        // ------------------------------------------------------------------
        for arg in args {
            // Check if the argument is a heap pointer (payload reference).
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

        // Enqueue the actor in the scheduler so it gets a chance to run
        self.scheduler.enqueue(target_id);
    }

    /// Process pending ORCA operations and run incremental cycle detection.
    /// Call this between scheduling rounds to keep GC moving.
    pub fn process_gc_ops(&mut self) {
        // Deliver pending foreign ref operations
        let ops = std::mem::take(&mut self.coordinator.pending_ops);
        for op in ops {
            if let Some(target_actor) = self.actors.get_mut(&op.target_actor) {
                target_actor.orca_gc.process_foreign_op(&mut target_actor.heap, op);
            }
        }

        // Run incremental cycle detection if due.
        //
        // SAFETY: incremental_detect takes &Runtime but only reads from it
        // (the current implementation does not mutate the runtime). We use
        // a raw pointer to work around the borrow checker: a &mut to
        // cycle_detector and a & to the runtime would overlap because
        // cycle_detector is a field of Runtime. In practice these borrows
        // are disjoint (the detector never accesses runtime fields through
        // its &mut self). A future refactoring can restructure the API to
        // avoid this pattern.
        let should_detect = self.cycle_detector.should_detect();
        if should_detect {
            let rt = self as *const Runtime;
            let detector = &mut self.cycle_detector;
            unsafe {
                detector.incremental_detect(&*rt);
            }
        }
    }

    /// Aggregate GC statistics from all actors.
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

    /// Return the ID of the currently executing actor, if any.
    pub fn current_actor_id(&self) -> Option<u64> {
        self.current_actor
    }

    /// Run the scheduler until no more work is available.
    ///
    /// Continuously dequeues actors from the scheduler and processes
    /// one message each, re-enqueueing them if they still have work.
    pub fn run_scheduler(&mut self) {
        while let Some(actor_id) = self.scheduler.dequeue() {
            self.step_actor(actor_id);
        }
    }

    /// Process one message for the given actor.
    ///
    /// Steps:
    /// 1. Pop a message from the actor's mailbox.
    /// 2. Look up the behavior handler in the behavior table.
    /// 3. Call the handler with the actor and message payload.
    /// 4. Increment the actor's reduction count.
    /// 5. Re-enqueue the actor if it still has messages and hasn't yielded.
    ///
    /// Sets `current_actor` before processing and clears it after.
    pub fn step_actor(&mut self, actor_id: u64) {
        // Set the current actor before processing
        self.current_actor = Some(actor_id);

        // Step 1: Pop a message from the actor's mailbox
        let msg_opt = {
            let actor = match self.actors.get_mut(&actor_id) {
                Some(a) => a,
                None => {
                    self.current_actor = None;
                    return;
                }
            };

            // Only process if the actor is in a runnable state
            match actor.state {
                ActorState::Running | ActorState::Created | ActorState::Waiting => {
                    actor.receive()
                }
                _ => {
                    // Actor is suspended or terminated - don't process messages
                    self.current_actor = None;
                    return;
                }
            }
        };

        // Step 2: Process the message if one was received
        let should_requeue = if let Some(msg) = msg_opt {
            // Find the handler function pointer from the behavior table
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
                    // Behavior not found - in MVP, silently drop the message
                    None
                }
            };

            // Call the behavior handler
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

            // Increment reduction count and check if should requeue
            let actor = match self.actors.get_mut(&actor_id) {
                Some(a) => a,
                None => {
                    self.current_actor = None;
                    return;
                }
            };

            actor.reduction_count += 1;

            // Re-enqueue if there are more messages and actor hasn't yielded
            !actor.mailbox.is_empty() && !actor.should_yield()
        } else {
            // No message available - actor goes to Waiting state
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                if actor.state == ActorState::Running {
                    actor.state = ActorState::Waiting;
                }
            }
            false
        };

        // Step 3: Re-enqueue the actor if it has more work
        if should_requeue {
            self.scheduler.enqueue(actor_id);
        }

        // Clear current actor
        self.current_actor = None;
    }

    // -----------------------------------------------------------------------
    // Fault Tolerance: Links
    // -----------------------------------------------------------------------

    /// Link two actors bidirectionally.
    ///
    /// If either actor exits (abnormally), the other will also exit
    /// (unless it traps exits). Links are symmetric: `link_actors(a, b)`
    /// is equivalent to `link_actors(b, a)`.
    ///
    /// If either actor does not exist, the operation is a no-op.
    pub fn link_actors(&mut self, a: u64, b: u64) {
        if a == b {
            return; // No self-links.
        }
        if let Some(actor_a) = self.actors.get_mut(&a) {
            if !actor_a.links.contains(&b) {
                actor_a.links.push(b);
            }
        }
        if let Some(actor_b) = self.actors.get_mut(&b) {
            if !actor_b.links.contains(&a) {
                actor_b.links.push(a);
            }
        }
    }

    /// Unlink two actors (removes the bidirectional link).
    ///
    /// If either actor does not exist, the operation is a no-op.
    pub fn unlink_actors(&mut self, a: u64, b: u64) {
        if let Some(actor_a) = self.actors.get_mut(&a) {
            actor_a.links.retain(|&id| id != b);
        }
        if let Some(actor_b) = self.actors.get_mut(&b) {
            actor_b.links.retain(|&id| id != a);
        }
    }

    // -----------------------------------------------------------------------
    // Fault Tolerance: Monitors
    // -----------------------------------------------------------------------

    /// Monitor an actor from another actor.
    ///
    /// If the `target` actor exits, the `watcher` will receive a `DOWN`
    /// message in its mailbox. Monitors are unidirectional and automatically
    /// removed when the target exits.
    ///
    /// If the target actor does not exist, the watcher immediately receives
    /// a DOWN message.
    pub fn monitor(&mut self, watcher: u64, target: u64) {
        if watcher == target {
            return; // No self-monitoring.
        }
        if let Some(actor) = self.actors.get_mut(&target) {
            if !actor.monitors.contains(&watcher) {
                actor.monitors.push(watcher);
            }
        } else {
            // Target doesn't exist — immediately send DOWN.
            self.send_down_message(watcher, target, &ExitReason::Error("noproc".to_string()));
        }
    }

    /// Demonitor an actor.
    ///
    /// Removes the watcher from the target's monitor list. If either actor
    /// does not exist, the operation is a no-op.
    pub fn demonitor(&mut self, watcher: u64, target: u64) {
        if let Some(actor) = self.actors.get_mut(&target) {
            actor.monitors.retain(|&id| id != watcher);
        }
    }

    // -----------------------------------------------------------------------
    // Fault Tolerance: Actor Exit
    // -----------------------------------------------------------------------

    /// Terminate an actor with a given reason and notify monitors/links.
    ///
    /// This is the graceful way to exit an actor. The actor is marked as
    /// `Terminated`, and all linked actors and monitors are notified.
    /// If the exit reason is abnormal, linked actors that don't trap exits
    /// will also exit (cascading failure).
    pub fn exit_actor(&mut self, actor_id: u64, reason: ExitReason) {
        // Mark the actor as terminated.
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            actor.state = ActorState::Terminated;
        }
        // Handle notifications and cleanup.
        let reason_clone = reason.clone();
        self.handle_actor_exit(actor_id, reason_clone);
    }

    /// Kill an actor unconditionally.
    ///
    /// This is equivalent to `exit_actor(actor_id, ExitReason::Kill)`.
    /// The kill reason is treated as abnormal, so linked actors will also
    /// exit (unless they trap exits).
    pub fn kill_actor(&mut self, actor_id: u64) {
        self.exit_actor(actor_id, ExitReason::Kill);
    }

    /// Handle actor exit: notify monitors, linked actors, and supervisor.
    ///
    /// This internal method performs the actual notification and cleanup:
    /// 1. Sends `DOWN` messages to all monitoring actors.
    /// 2. Propagates exit signals to all linked actors (cascading exits for
    ///    abnormal reasons unless the linked actor traps exits).
    /// 3. Notifies the parent supervisor (if any) to apply restart strategy.
    /// 4. Removes the actor from the runtime's actor map.
    ///
    /// This method takes care to avoid infinite loops during cascading exit
    /// propagation by checking actor state before recursively exiting.
    pub fn handle_actor_exit(&mut self, actor_id: u64, reason: ExitReason) {
        // Collect information about the actor before removing it.
        let (monitors, links, parent) = {
            let actor = match self.actors.get(&actor_id) {
                Some(a) => a,
                None => return, // Already removed.
            };
            (
                actor.monitors.clone(),
                actor.links.clone(),
                actor.parent,
            )
        };

        // 1. Notify monitors: send DOWN message to each watcher.
        for watcher_id in monitors {
            self.send_down_message(watcher_id, actor_id, &reason);
        }

        // 2. Notify linked actors.
        let is_abnormal = !matches!(reason, ExitReason::Normal);
        for linked_id in links {
            if linked_id == actor_id {
                continue;
            }
            // Check if the linked actor exists and is still alive.
            let linked_alive = self
                .actors
                .get(&linked_id)
                .map(|a| a.state != ActorState::Terminated)
                .unwrap_or(false);

            if !linked_alive {
                continue;
            }

            if is_abnormal {
                // For abnormal exits, the linked actor also exits unless it traps exits.
                let traps = self
                    .actors
                    .get(&linked_id)
                    .map(|a| a.trap_exits)
                    .unwrap_or(false);

                if traps {
                    // Convert exit signal to a message in the mailbox.
                    let exit_msg = Message {
                        behavior_id: 0, // System message
                        payload: vec![
                            Value::int(actor_id as i64),
                            Value::int(linked_id as i64),
                        ],
                        sender: actor_id,
                        priority: MessagePriority::System,
                    };
                    if let Some(actor) = self.actors.get_mut(&linked_id) {
                        let _ = actor.mailbox.push(exit_msg);
                    }
                    self.scheduler.enqueue(linked_id);
                } else {
                    // Cascading exit: the linked actor dies too.
                    let linked_reason = ExitReason::Error(format!(
                        "linked actor {} exited with {:?}",
                        actor_id, reason
                    ));
                    // Mark as terminated first to avoid loops.
                    if let Some(actor) = self.actors.get_mut(&linked_id) {
                        actor.state = ActorState::Terminated;
                    }
                    // Recursively handle the linked actor's exit.
                    self.handle_actor_exit(linked_id, linked_reason);
                }
            }
            // For normal exits, linked actors are NOT affected (per Erlang semantics).
        }

        // 3. Notify the parent supervisor.
        if let Some(supervisor_id) = parent {
            // Take the supervisor out of the map to avoid double-borrow of self.
            let mut supervisor = match self.supervisors.remove(&supervisor_id) {
                Some(s) => s,
                None => {
                    // No supervisor found — remove actor and return.
                    self.actors.remove(&actor_id);
                    return;
                }
            };

            let action = supervisor.handle_exit(actor_id, reason.clone(), self);

            match action {
                SupervisorAction::Restarted(_new_id) => {
                    // Child was restarted — put supervisor back.
                    self.supervisors.insert(supervisor_id, supervisor);
                }
                SupervisorAction::Shutdown => {
                    // Max restarts exceeded — shut down the supervisor itself.
                    // Do NOT re-insert the supervisor.
                    let sup_parent = supervisor.parent;

                    // Remove the supervisor actor and all remaining children.
                    self.shutdown_supervisor(supervisor_id);

                    // Escalate to the supervisor's parent if it has one.
                    if let Some(parent_id) = sup_parent {
                        // Recursively handle the supervisor's own exit.
                        let escalate_reason =
                            ExitReason::Error("child supervisor shutdown".to_string());
                        self.handle_supervisor_parent_exit(parent_id, supervisor_id, escalate_reason);
                    }
                }
                SupervisorAction::Ignore => {
                    // Child removed, no action needed — put supervisor back.
                    self.supervisors.insert(supervisor_id, supervisor);
                }
                SupervisorAction::Escalate => {
                    // Propagate to the supervisor's parent — put supervisor back first.
                    self.supervisors.insert(supervisor_id, supervisor);
                    if let Some(parent_id) = parent {
                        let escalate_reason = reason.clone();
                        self.handle_supervisor_parent_exit(parent_id, actor_id, escalate_reason);
                    }
                }
            }
        } else {
            // No parent supervisor — just remove the actor.
            self.actors.remove(&actor_id);
        }
    }

    /// Helper: handle a supervisor's parent when a child supervisor shuts down or escalates.
    ///
    /// Looks up the parent supervisor, temporarily removes it, calls handle_exit,
    /// and re-inserts it (unless it also shuts down).
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
                // Parent also shuts down — cascade.
                let grandparent = parent_sup.parent;
                self.shutdown_supervisor(parent_id);
                if let Some(gp_id) = grandparent {
                    let gp_reason = ExitReason::Error("supervisor shutdown cascaded".to_string());
                    self.handle_supervisor_parent_exit(gp_id, parent_id, gp_reason);
                }
            }
            _ => {
                // For Restarted, Ignore, Escalate — re-insert the parent.
                self.supervisors.insert(parent_id, parent_sup);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Fault Tolerance: Supervisor Management
    // -----------------------------------------------------------------------

    /// Create a new supervisor actor and register it in the runtime.
    ///
    /// Returns the actor ID of the supervisor. The supervisor actor itself
    /// is a regular actor that can receive messages (e.g., for dynamic
    /// child management).
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

    /// Register a child actor under a supervisor.
    ///
    /// The child's `parent` field is set to the supervisor's actor ID.
    /// If the supervisor does not exist, this is a no-op.
    pub fn supervise_child(&mut self, supervisor_id: u64, spec: ChildSpec, child_id: u64) {
        // Set the child's parent.
        if let Some(child) = self.actors.get_mut(&child_id) {
            child.parent = Some(supervisor_id);
        }

        // Register with the supervisor.
        if let Some(supervisor) = self.supervisors.get_mut(&supervisor_id) {
            supervisor.add_child(spec, child_id);
        }
    }

    // -----------------------------------------------------------------------
    // Internal Helpers
    // -----------------------------------------------------------------------

    /// Send a DOWN message to a watcher actor.
    ///
    /// The DOWN message is sent as a system-priority message with the
    /// monitored actor's ID and the exit reason encoded in the payload.
    fn send_down_message(&mut self, watcher_id: u64, target_id: u64, reason: &ExitReason) {
        let reason_str = match reason {
            ExitReason::Normal => "normal",
            ExitReason::Error(_) => "error",
            ExitReason::Kill => "kill",
            ExitReason::Killed => "killed",
        };

        let down_msg = Message {
            behavior_id: 0, // Reserved for system messages
            payload: vec![
                Value::int(target_id as i64),
                Value::int(watcher_id as i64),
                Value::int(match reason {
                    ExitReason::Normal => 0,
                    ExitReason::Error(_) => 1,
                    ExitReason::Kill => 2,
                    ExitReason::Killed => 3,
                }),
            ],
            sender: target_id,
            priority: MessagePriority::System,
        };

        if let Some(watcher) = self.actors.get_mut(&watcher_id) {
            let _ = watcher.mailbox.push(down_msg);
            // Mark the reason string for use.
            let _ = reason_str;
        }
        self.scheduler.enqueue(watcher_id);
    }

    /// Shut down a supervisor and all its children.
    ///
    /// This removes the supervisor actor, all child actors, and the
    /// supervisor's state from the runtime. Used when a supervisor
    /// exceeds its maximum restart intensity.
    fn shutdown_supervisor(&mut self, supervisor_id: u64) {
        // Collect child IDs to remove.
        let child_ids: Vec<u64> = self
            .supervisors
            .get(&supervisor_id)
            .map(|s| s.children.iter().map(|(_, id)| *id).collect())
            .unwrap_or_default();

        // Remove all children.
        for child_id in child_ids {
            self.actors.remove(&child_id);
        }

        // Remove the supervisor itself.
        self.actors.remove(&supervisor_id);
        self.supervisors.remove(&supervisor_id);
    }

    // ========================================================================
    // Distributed Actor System (Stage B1)
    // ========================================================================

    /// Enable distributed mode by binding to a network address.
    ///
    /// This initializes the network transport, cluster state, and address
    /// resolver. After calling this, the runtime can communicate with
    /// other Nulang nodes in the cluster.
    ///
    /// Returns an error if the transport cannot bind to the address.
    pub fn enable_distribution(&mut self, bind_addr: std::net::SocketAddr) -> std::io::Result<()> {
        let transport = NetworkTransport::bind(bind_addr)?;
        // Convert network::NodeId to cluster::NodeId (same inner u64).
        let node_id = cluster::NodeId(transport.node_id().0);
        let cluster = ClusterState::new(node_id, bind_addr);
        let resolver = AddressResolver::new(node_id);

        self.transport = Some(transport);
        self.cluster = Some(cluster);
        self.resolver = Some(resolver);
        self.node_id = Some(node_id);
        self.distributed_enabled = true;

        Ok(())
    }

    /// Join an existing cluster by contacting a seed node.
    ///
    /// Must call `enable_distribution` first.
    pub fn join_cluster(&mut self, seed_addr: std::net::SocketAddr) {
        if let Some(cluster) = &mut self.cluster {
            cluster.join_cluster(seed_addr);
        }
    }

    /// Send a message to an actor using a location-transparent address.
    ///
    /// If the actor is local, this behaves like `send_message`.
    /// If the actor is remote, the message is serialized and sent over
    /// the network.
    ///
    /// If distributed mode is not enabled, falls back to local send.
    pub fn send_distributed(
        &mut self,
        target: ActorAddress,
        behavior: &str,
        args: &[Value],
    ) {
        if !self.distributed_enabled {
            // Fallback: treat everything as local
            let actor_id = match target {
                ActorAddress::Local { actor_id } => actor_id,
                ActorAddress::Remote { actor_id, .. } => actor_id,
            };
            self.send_message(actor_id, behavior, args);
            return;
        }

        // Local address — use the existing send path directly
        if let ActorAddress::Local { actor_id } = target {
            self.send_message(actor_id, behavior, args);
            return;
        }

        // Remote address — temporarily take distributed components out of
        // self to avoid borrow-checker conflicts with send_message.
        let mut transport = self.transport.take().unwrap();
        let cluster = self.cluster.take().unwrap();
        let mut resolver = self.resolver.take().unwrap();

        // Delegate to the free function in the distributed module.
        distributed::send_distributed(
            self,
            &mut transport,
            &cluster,
            &mut resolver,
            target,
            behavior,
            args,
        );

        // Restore components
        self.transport = Some(transport);
        self.cluster = Some(cluster);
        self.resolver = Some(resolver);
    }

    /// Process incoming network packets and cluster maintenance.
    ///
    /// Should be called regularly (e.g., every scheduling iteration).
    /// This method:
    /// 1. Reads incoming packets from the transport
    /// 2. Delivers actor messages to their target actors
    /// 3. Processes heartbeat packets through the cluster state
    /// 4. Runs cluster tick (failure detection, gossip)
    pub fn process_network(&mut self) {
        if !self.distributed_enabled {
            return;
        }

        // Step 1: Process incoming packets using the free function.
        // Temporarily take cluster and resolver to avoid borrow conflicts.
        let transport = self.transport.as_ref().unwrap();
        let mut cluster = self.cluster.take().unwrap();
        let mut resolver = self.resolver.take().unwrap();

        distributed::process_network_packets(self, transport, &mut cluster, &mut resolver);

        // Restore cluster and resolver before the cluster tick (which may
        // need to access self.transport for action handling).
        self.cluster = Some(cluster);
        self.resolver = Some(resolver);

        // Step 2: Run cluster maintenance tick.
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
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                        };
                        transport.send(net_node_id, addr, packet);
                    }
                }
                ClusterAction::NodeJoined { node, addr } => {
                    // Connect to the new node
                    if let Some(transport) = &mut self.transport {
                        let net_node_id = network::NodeId(node.0);
                        let _ = transport.connect(net_node_id, addr);
                    }
                }
                ClusterAction::NodeFailed { node } => {
                    // Disconnect from failed node
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
                ClusterAction::SendGossip { targets: _ } => {
                    // In MVP, gossip is piggybacked on heartbeats.
                    // Future versions will send explicit gossip packets here.
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
