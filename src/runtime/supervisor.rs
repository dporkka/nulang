//! Supervision trees: actor failure recovery.
//!
//! Implements Erlang/OTP-style supervision with:
//! - Three restart strategies: OneForOne, OneForAll, RestForOne
//! - Three restart policies: Permanent, Temporary, Transient
//! - Rate-limited restarts with configurable time windows
//! - Hierarchical supervision with escalation

use std::time::{Duration, Instant};

use super::*;

// ---------------------------------------------------------------------------
// Exit Reason & Supervisor Action
// ---------------------------------------------------------------------------

/// Reason an actor exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    /// Normal termination.
    Normal,
    /// Error with message.
    Error(String),
    /// Unconditional kill.
    Kill,
    /// Killed by another actor.
    Killed,
}

/// Action taken by a supervisor after handling a child exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorAction {
    /// Child was restarted: contains the new actor_id.
    Restarted(u64),
    /// Max restarts exceeded: shut down the supervisor.
    Shutdown,
    /// No action taken (temporary child, normal exit, etc.).
    Ignore,
    /// Propagate failure to parent supervisor.
    Escalate,
}

// ---------------------------------------------------------------------------
// Restart Strategy & Policy
// ---------------------------------------------------------------------------

/// How a supervisor handles child actor failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartStrategy {
    /// Restart only the failed child.
    OneForOne,
    /// Restart all children when one fails.
    OneForAll,
    /// Restart the failed child and all children started after it.
    RestForOne,
}

/// When to restart a child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Always restart (even on normal exit).
    Permanent,
    /// Never restart.
    Temporary,
    /// Restart only on abnormal (non-normal) exit.
    Transient,
}

// ---------------------------------------------------------------------------
// Child Specification
// ---------------------------------------------------------------------------

/// Specification for a supervised child actor.
#[derive(Debug, Clone)]
pub struct ChildSpec {
    /// Unique identifier for this child within the supervisor.
    pub id: String,
    /// Restart policy for this child.
    pub restart_policy: RestartPolicy,
    /// Maximum number of restarts allowed within the time window.
    pub max_restarts: u32,
    /// Time window (in seconds) for counting restarts.
    pub restart_window_secs: u32,
}

impl ChildSpec {
    /// Create a new child spec with the given ID and restart policy.
    ///
    /// Defaults: max_restarts=5, restart_window_secs=60.
    pub fn new(id: impl Into<String>, policy: RestartPolicy) -> Self {
        ChildSpec {
            id: id.into(),
            restart_policy: policy,
            max_restarts: 5,
            restart_window_secs: 60,
        }
    }

    /// Set custom restart intensity limits.
    pub fn with_limits(mut self, max_restarts: u32, window_secs: u32) -> Self {
        self.max_restarts = max_restarts;
        self.restart_window_secs = window_secs;
        self
    }
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

/// A supervisor node in the supervision tree.
///
/// Each supervisor is itself an actor (identified by `id`) and manages a set
/// of child actors according to a restart strategy. If a child fails, the
/// supervisor applies the strategy to determine which children to restart.
///
/// If the maximum restart intensity is exceeded for any child, the supervisor
/// itself shuts down (returning `SupervisorAction::Shutdown`), which typically
/// causes escalation to a parent supervisor.
pub struct Supervisor {
    /// The actor ID of this supervisor.
    pub id: u64,
    /// Human-readable name.
    pub name: String,
    /// Restart strategy applied when a child fails.
    pub strategy: RestartStrategy,
    /// Children: (spec, actor_id) pairs in start order.
    pub children: Vec<(ChildSpec, u64)>,
    /// Restart history: (actor_id, restart_time) for rate limiting.
    pub restart_history: Vec<(u64, Instant)>,
    /// Parent supervisor actor ID (if any).
    pub parent: Option<u64>,
}

impl Supervisor {
    /// Create a new supervisor.
    ///
    /// The `id` should be the actor ID of the supervisor actor itself.
    pub fn new(id: u64, name: impl Into<String>, strategy: RestartStrategy) -> Self {
        Supervisor {
            id,
            name: name.into(),
            strategy,
            children: Vec::new(),
            restart_history: Vec::new(),
            parent: None,
        }
    }

    /// Register a child actor under this supervisor.
    pub fn add_child(&mut self, spec: ChildSpec, actor_id: u64) {
        self.children.push((spec, actor_id));
    }

    /// Find the child spec for a given actor ID.
    fn find_child_spec(&self, actor_id: u64) -> Option<&ChildSpec> {
        self.children
            .iter()
            .find(|(_, id)| *id == actor_id)
            .map(|(spec, _)| spec)
    }

    /// Find the index of a child by actor ID.
    fn find_child_index(&self, actor_id: u64) -> Option<usize> {
        self.children.iter().position(|(_, id)| *id == actor_id)
    }

    /// Update a child's actor ID (used after restart).
    fn update_child_id(&mut self, old_id: u64, new_id: u64) {
        if let Some((_, id)) = self.children.iter_mut().find(|(_, id)| *id == old_id) {
            *id = new_id;
        }
    }

    /// Remove a child by actor ID.
    fn remove_child(&mut self, actor_id: u64) {
        self.children.retain(|(_, id)| *id != actor_id);
    }

    /// Prune old restart history entries that fall outside all children's
    /// time windows. This prevents unbounded growth.
    fn prune_restart_history(&mut self) {
        let now = Instant::now();
        // Use the largest window among all children as the conservative bound.
        let max_window_secs = self
            .children
            .iter()
            .map(|(spec, _)| spec.restart_window_secs)
            .max()
            .unwrap_or(60);
        let max_window = Duration::from_secs(max_window_secs as u64);

        self.restart_history
            .retain(|(_, time)| now.duration_since(*time) < max_window);
    }

    /// Check if a child should be restarted based on its restart policy and
    /// the rate limit (max restarts within the time window).
    ///
    /// Returns `false` if:
    /// - The child is not found.
    /// - The policy forbids restart (Temporary, or Transient with normal exit).
    /// - The rate limit has been exceeded.
    pub fn should_restart(
        &self,
        actor_id: u64,
        policy: RestartPolicy,
        reason: &ExitReason,
    ) -> bool {
        // Check policy first.
        let policy_allows = match policy {
            RestartPolicy::Permanent => true,
            RestartPolicy::Temporary => false,
            RestartPolicy::Transient => !matches!(reason, ExitReason::Normal),
        };

        if !policy_allows {
            return false;
        }

        // Check rate limit: count restarts for this specific child within its window.
        let spec = match self.find_child_spec(actor_id) {
            Some(s) => s,
            None => return false, // Unknown child.
        };

        let now = Instant::now();
        let window = Duration::from_secs(spec.restart_window_secs as u64);

        let recent_restarts = self
            .restart_history
            .iter()
            .filter(|(id, time)| *id == actor_id && now.duration_since(*time) < window)
            .count() as u32;

        recent_restarts < spec.max_restarts
    }

    /// Restart a single child actor.
    ///
    /// 1. Removes the old actor from the runtime.
    /// 2. Creates a new actor with the same name.
    /// 3. Updates the supervisor's child tracking.
    /// 4. Enqueues the new actor in the scheduler.
    ///
    /// Returns `Some(new_actor_id)` on success, `None` if rate limit exceeded.
    pub fn restart_child(&mut self, actor_id: u64, runtime: &mut Runtime) -> Option<u64> {
        let now = Instant::now();

        // Find the child's spec.
        let spec = self.find_child_spec(actor_id)?.clone();

        // Check rate limit before restarting.
        let window = Duration::from_secs(spec.restart_window_secs as u64);
        let recent_restarts = self
            .restart_history
            .iter()
            .filter(|(id, time)| *id == actor_id && now.duration_since(*time) < window)
            .count() as u32;

        if recent_restarts >= spec.max_restarts {
            return None;
        }

        // Remove the old actor.
        runtime.actors.remove(&actor_id);

        // Create a new actor to replace the old one.
        let new_id = fresh_actor_id();
        let child_name = format!("{}_child_{}", self.name, spec.id);
        let mut new_actor = Actor::new(new_id, child_name, 256);
        new_actor.state = ActorState::Running;
        new_actor.parent = Some(self.id);

        // Register the new actor.
        runtime.actors.insert(new_id, new_actor);

        // Update supervisor state.
        self.update_child_id(actor_id, new_id);
        self.restart_history.push((new_id, now));
        self.prune_restart_history();

        // Enqueue the new actor.
        runtime.scheduler.enqueue(new_id);

        Some(new_id)
    }

    /// Restart all children (used by OneForAll strategy).
    ///
    /// Each child is terminated and recreated with a fresh actor ID.
    /// The restart history is updated for each restarted child.
    pub fn restart_all(&mut self, runtime: &mut Runtime) {
        let now = Instant::now();
        // Collect current child IDs so we can mutate children while iterating.
        let child_ids: Vec<u64> = self.children.iter().map(|(_, id)| *id).collect();

        for old_id in child_ids {
            // Find the spec before removing.
            if let Some(spec) = self.find_child_spec(old_id).cloned() {
                // Remove old actor.
                runtime.actors.remove(&old_id);

                // Create new actor.
                let new_id = fresh_actor_id();
                let child_name = format!("{}_child_{}", self.name, spec.id);
                let mut new_actor = Actor::new(new_id, child_name, 256);
                new_actor.state = ActorState::Running;
                new_actor.parent = Some(self.id);
                runtime.actors.insert(new_id, new_actor);
                runtime.scheduler.enqueue(new_id);

                self.update_child_id(old_id, new_id);
                self.restart_history.push((new_id, now));
            }
        }
        self.prune_restart_history();
    }

    /// Restart the failed child and all children started after it
    /// (used by RestForOne strategy).
    ///
    /// Children are restarted in order (failed child first, then subsequent).
    pub fn restart_from(&mut self, actor_id: u64, runtime: &mut Runtime) {
        let idx = match self.find_child_index(actor_id) {
            Some(i) => i,
            None => return,
        };

        let now = Instant::now();
        // Collect children to restart (from idx onwards).
        let to_restart: Vec<(ChildSpec, u64)> = self
            .children
            .iter()
            .skip(idx)
            .map(|(spec, id)| (spec.clone(), *id))
            .collect();

        for (spec, old_id) in to_restart {
            runtime.actors.remove(&old_id);

            let new_id = fresh_actor_id();
            let child_name = format!("{}_child_{}", self.name, spec.id);
            let mut new_actor = Actor::new(new_id, child_name, 256);
            new_actor.state = ActorState::Running;
            new_actor.parent = Some(self.id);
            runtime.actors.insert(new_id, new_actor);
            runtime.scheduler.enqueue(new_id);

            self.update_child_id(old_id, new_id);
            self.restart_history.push((new_id, now));
        }
        self.prune_restart_history();
    }

    /// Handle a child exit according to the supervisor's restart strategy.
    ///
    /// This is the main entry point called by the runtime when a supervised
    /// actor exits. It determines the appropriate action based on:
    /// 1. The child's restart policy (Permanent/Temp/Transient).
    /// 2. The exit reason (normal vs abnormal).
    /// 3. The rate limit (max restarts within time window).
    /// 4. The supervisor's restart strategy.
    ///
    /// Returns the action taken. If `Shutdown` is returned, the supervisor
    /// itself should be terminated (typically escalating to its parent).
    pub fn handle_exit(
        &mut self,
        actor_id: u64,
        reason: ExitReason,
        runtime: &mut Runtime,
    ) -> SupervisorAction {
        // Find the child's spec.
        let spec = match self.find_child_spec(actor_id) {
            Some(s) => s.clone(),
            None => {
                // Not our child — nothing to do.
                return SupervisorAction::Ignore;
            }
        };

        // Check if we should restart this child.
        if !self.should_restart(actor_id, spec.restart_policy, &reason) {
            // Policy says don't restart. Clean up and return appropriate action.

            // For Temporary children: always ignore.
            if spec.restart_policy == RestartPolicy::Temporary {
                self.remove_child(actor_id);
                runtime.actors.remove(&actor_id);
                return SupervisorAction::Ignore;
            }

            // For Transient with normal exit: ignore.
            if spec.restart_policy == RestartPolicy::Transient && reason == ExitReason::Normal {
                self.remove_child(actor_id);
                runtime.actors.remove(&actor_id);
                return SupervisorAction::Ignore;
            }

            // Check if rate limit is the reason we can't restart.
            let now = Instant::now();
            let window = Duration::from_secs(spec.restart_window_secs as u64);
            let recent_restarts = self
                .restart_history
                .iter()
                .filter(|(id, time)| *id == actor_id && now.duration_since(*time) < window)
                .count() as u32;

            if recent_restarts >= spec.max_restarts {
                return SupervisorAction::Shutdown;
            }

            // Permanent child but can't restart for some other reason.
            self.remove_child(actor_id);
            runtime.actors.remove(&actor_id);
            return SupervisorAction::Ignore;
        }

        // We should restart. Apply the restart strategy.
        match self.strategy {
            RestartStrategy::OneForOne => {
                match self.restart_child(actor_id, runtime) {
                    Some(new_id) => SupervisorAction::Restarted(new_id),
                    None => SupervisorAction::Shutdown,
                }
            }
            RestartStrategy::OneForAll => {
                self.restart_all(runtime);
                // Return Restarted with the failed child's new ID.
                match self.children.iter().find(|(s, _)| s.id == spec.id) {
                    Some((_, new_id)) => SupervisorAction::Restarted(*new_id),
                    None => SupervisorAction::Escalate,
                }
            }
            RestartStrategy::RestForOne => {
                self.restart_from(actor_id, runtime);
                match self.children.iter().find(|(s, _)| s.id == spec.id) {
                    Some((_, new_id)) => SupervisorAction::Restarted(*new_id),
                    None => SupervisorAction::Escalate,
                }
            }
        }
    }

    /// Count how many times a given child has been restarted within its time window.
    ///
    /// Useful for testing and introspection.
    pub fn restart_count(&self, actor_id: u64) -> u32 {
        let now = Instant::now();
        let spec = match self.find_child_spec(actor_id) {
            Some(s) => s,
            None => return 0,
        };
        let window = Duration::from_secs(spec.restart_window_secs as u64);

        self.restart_history
            .iter()
            .filter(|(id, time)| *id == actor_id && now.duration_since(*time) < window)
            .count() as u32
    }

    /// Return the number of currently supervised children.
    pub fn child_count(&self) -> usize {
        self.children.len()
    }
}
