use crate::types::{ActorId, Capability, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

/// Actor states in the lifecycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ActorState {
    Ready,       // Just created, not yet scheduled
    Running,     // Currently executing
    Waiting,     // Blocked on a receive or async operation
    Suspended,   // Paused (for GC, migration, or supervision)
    Terminated,  // Exited, being cleaned up
}

/// Per-actor mailbox.
#[derive(Debug)]
pub struct Mailbox {
    messages: Vec<Message>,
    capacity: usize,
}

impl Mailbox {
    pub fn new(capacity: usize) -> Self {
        Mailbox {
            messages: Vec::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, msg: Message) -> Result<(), &'static str> {
        if self.messages.len() >= self.capacity {
            Err("mailbox full")
        } else {
            self.messages.push(msg);
            Ok(())
        }
    }

    pub fn pop(&mut self) -> Option<Message> {
        self.messages.pop()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// A message sent to an actor's mailbox.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub behavior_id: u16,
    pub payload: Vec<Value>,
    pub sender: Option<u64>, // Actor ID of sender
}

/// Actor — the core unit of concurrent computation in Nulang.
///
/// Each actor has:
/// - A unique ID
/// - Private mutable state (boxed for stable pointer)
/// - A mailbox for incoming messages
/// - Links to other actors (bidirectional fault propagation)
/// - Monitors from other actors (unilateral observation)
/// - Capability constraints
/// - State in the actor lifecycle
#[derive(Debug)]
pub struct Actor {
    pub id: u64,
    pub name: String,
    pub state_ptr: Box<dyn std::any::Any + Send>,
    pub mailbox: Mailbox,
    pub behavior_map: Vec<(String, u16)>, // name -> id
    pub cap: Capability,
    pub reductions: u64, // budget before yield
    pub state: ActorState,
    pub trap_exits: bool, // If true, exit signals become messages instead of killing
    pub links: HashSet<u64>,      // Actor IDs this actor is linked to
    pub monitors: HashSet<u64>,   // Actor IDs monitoring this actor
    pub exit_reason: Option<crate::types::ExitReason>, // Set when actor exits
}

static ACTOR_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

impl Actor {
    pub fn new(name: String, cap: Capability) -> Self {
        let id = ACTOR_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
        Actor {
            id,
            name: name.clone(),
            state_ptr: Box::new(()),
            mailbox: Mailbox::new(1000),
            behavior_map: Vec::new(),
            cap,
            reductions: 0,
            state: ActorState::Ready,
            trap_exits: false,
            links: HashSet::new(),
            monitors: HashSet::new(),
            exit_reason: None,
        }
    }

    pub fn add_link(&mut self, other: u64) {
        self.links.insert(other);
    }

    pub fn remove_link(&mut self, other: u64) {
        self.links.remove(&other);
    }

    pub fn add_monitor(&mut self, watcher: u64) {
        self.monitors.insert(watcher);
    }

    pub fn remove_monitor(&mut self, watcher: u64) {
        self.monitors.remove(&watcher);
    }

    pub fn reset_reductions(&mut self) {
        self.reductions = 1000;
    }
}

/// Supervisor node in the supervision tree.
#[derive(Debug)]
pub struct SupervisorNode {
    pub parent: Option<u64>,
    pub children: Vec<u64>,
    pub strategy: SupervisorStrategy,
    pub max_restarts: u32,
    pub restart_window: u64, // ms
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SupervisorStrategy {
    OneForOne,  // Restart only the failed child
    OneForAll,  // Restart all children if one fails
    RestForOne, // Restart failed child and all children started after it
}

impl SupervisorNode {
    pub fn new(strategy: SupervisorStrategy, max_restarts: u32) -> Self {
        SupervisorNode {
            parent: None,
            children: Vec::new(),
            strategy,
            max_restarts,
            restart_window: 5000,
        }
    }
}
