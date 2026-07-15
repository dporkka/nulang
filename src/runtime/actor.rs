//! Actor definition and lifecycle.

use super::gc::OrcaGc;
use super::*;
use crate::vm::Value;
use std::collections::HashMap;

/// Actor state machine: Created → Running → Waiting → Suspended → Terminated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorState {
    Created,
    Running,
    Waiting,    // Mailbox empty, no work
    Suspended,  // Explicitly suspended
    Terminated, // Actor has exited
}

/// Scheduling priority of an actor (Erlang-style process priority).
///
/// The scheduler dequeues ready High-priority actors before Normal, and
/// Normal before Low (strict per-level preference, FIFO within a level —
/// see `Scheduler::enqueue_with_priority`). Priority affects scheduling
/// order only; it does not touch message delivery order
/// (`Mailbox::receive_match` stays FIFO and ignores `Message::priority`).
/// Set from Nulang via `perform Actor.set_priority(0|1|2)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActorPriority {
    High,
    #[default]
    Normal,
    Low,
}

/// An actor: independent unit of computation with isolated state and mailbox.
pub struct Actor {
    pub id: u64,
    pub name: String,
    pub state: ActorState,
    pub mailbox: Mailbox,
    pub heap: ActorHeap,
    pub orca_gc: OrcaGc,                  // ORCA GC engine for this actor
    pub state_data: HashMap<String, Value>, // Named actor state fields
    pub state_models: HashMap<String, StateModel>, // Persistence model per field
    pub event_log: Vec<(String, Vec<Value>)>, // Emitted events for event_sourced actors
    pub persistent: bool,                 // Whether this actor survives restarts
    pub is_workflow: bool,                // True if generated from a workflow declaration
    pub behavior_table: Vec<BehaviorEntry>,
    /// Bytecode behavior offsets by behavior_id. Empty entries mean no bytecode
    /// handler for that behavior (native handler or missing).
    pub bytecode_offsets: Vec<usize>,
    /// Saga compensation code offsets by behavior_id. `None` means the step has
    /// no compensation expression.
    pub compensation_offsets: Vec<Option<usize>>,
    /// Names of steps already compensated (used during recovery replay).
    pub compensated_steps: Vec<String>,
    /// Bytecode module used by this actor's bytecode behaviors.
    pub bytecode_module: Option<crate::bytecode::CodeModule>,
    /// Index of the loaded bytecode module in the runtime VM.
    pub bytecode_module_idx: Option<usize>,
    pub parent: Option<u64>,  // Supervisor
    pub children: Vec<u64>,   // Supervised actors
    pub monitors: Vec<u64>,   // Actors monitoring this one
    pub links: Vec<u64>,      // Bidirectional links
    pub trap_exits: bool,     // If true, exit signals become messages instead of killing this actor
    /// Scheduling priority, consulted by the scheduler on every enqueue.
    pub priority: ActorPriority,
    pub reduction_count: u32, // Lifetime messages handled (monotonic progress metric)
    turn_reductions: u32,     // Messages handled in the current scheduling turn
    pub max_reductions: u32,  // Max reductions per turn before yield (preemption)
    pub sequence: u64,        // Last persisted sequence number
    /// Sentinel heap object used by the cycle detector to represent this
    /// actor as a holder of foreign references.
    cycle_sentinel: Option<*mut OrcaHeader>,
    /// Suspended VM state for a workflow step waiting on a signal.
    pub suspended_execution: Option<SuspendedExecution>,
    /// Name of the signal this workflow actor is currently waiting for, if any.
    pub waiting_signal: Option<String>,
    /// Signals that have been received by this workflow actor (name, payload).
    pub received_signals: Vec<(String, Option<String>)>,
    /// Read-only query handlers registered on a workflow actor, keyed by
    /// query name.  A handler is either a function/closure value invoked
    /// with the actor bound as `self`, or a plain value returned as-is.
    /// Handlers are ephemeral: they are not journaled and must be
    /// re-registered after a node restart.
    pub query_handlers: HashMap<String, Value>,
    /// True if this actor was generated from an `agent` declaration.
    pub is_agent: bool,
    /// True while a background worker thread holds an in-flight LLM request
    /// issued by this actor's suspended bytecode behavior.
    pub llm_inflight: bool,
    /// Prompt of the in-flight LLM request, if any (kept for resume
    /// bookkeeping; cleared when the completion is pumped).
    pub llm_pending_prompt: Option<String>,
    /// Completed background LLM result waiting to be consumed when the
    /// suspended behavior re-executes its `LlmAsk` instruction.
    pub llm_completed: Option<Result<crate::ai::LlmResponse, String>>,
    /// State of an in-flight timed selective receive (`receive ... after
    /// ms =>`), from the first suspension until the wait resolves (match,
    /// timeout, or the behavior ends). `None` when no receive-wait is live.
    pub receive_wait: Option<ReceiveWaitState>,
}

/// State of an actor's in-flight timed selective receive.
///
/// The timeout timer is armed exactly once per wait (at the first
/// suspension), so a wake-then-re-suspend cycle (a non-matching message
/// arrived) keeps the original deadline instead of restarting the clock.
/// `timed_out` is set by the timer-fire path; the re-executed `ReceiveWait`
/// consumes it and resolves the wait with the no-match sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiveWaitState {
    /// The armed timeout timer (cancelled when the wait resolves by match).
    pub timer_id: TimerId,
    /// True once the timeout timer has fired.
    pub timed_out: bool,
}

/// Captured VM state plus metadata for resuming a workflow step.
#[derive(Debug)]
pub struct SuspendedExecution {
    pub vm_state: crate::vm::SuspendedVmState,
    pub behavior_idx: usize,
    pub step_name: String,
}

/// A behavior entry: maps behavior name to handler.
pub struct BehaviorEntry {
    pub name: String,
    pub handler_fn: fn(&mut Actor, &[Value]),
}

impl Actor {
    pub fn new(id: u64, name: impl Into<String>, mailbox_cap: usize) -> Self {
        Actor {
            id,
            name: name.into(),
            state: ActorState::Created,
            mailbox: Mailbox::new(mailbox_cap),
            heap: {
                let mut heap = ActorHeap::new(64 * 1024); // 64KB initial heap
                heap.set_actor_id(id);
                heap
            },
            orca_gc: OrcaGc::new(id), // ORCA GC engine
            state_data: HashMap::new(),
            state_models: HashMap::new(),
            event_log: Vec::new(),
            persistent: false,
            is_workflow: false,
            behavior_table: Vec::new(),
            bytecode_offsets: Vec::new(),
            compensation_offsets: Vec::new(),
            compensated_steps: Vec::new(),
            bytecode_module: None,
            bytecode_module_idx: None,
            parent: None,
            children: Vec::new(),
            monitors: Vec::new(),
            links: Vec::new(),
            trap_exits: false,
            priority: ActorPriority::Normal,
            reduction_count: 0,
            turn_reductions: 0,
            max_reductions: 1000,
            sequence: 0,
            cycle_sentinel: None,
            suspended_execution: None,
            waiting_signal: None,
            received_signals: Vec::new(),
            query_handlers: HashMap::new(),
            is_agent: false,
            llm_inflight: false,
            llm_pending_prompt: None,
            llm_completed: None,
            receive_wait: None,
        }
    }

    /// Return the cycle-detector sentinel header for this actor.
    ///
    /// The sentinel is lazily allocated on the actor's heap and pinned
    /// (sticky) so it is never collected. It represents the actor itself as
    /// a holder of foreign references for coarse-grained cycle detection.
    pub fn cycle_sentinel(&mut self) -> Option<*mut OrcaHeader> {
        if self.cycle_sentinel.is_none() {
            if let Some(ptr) = self.heap.alloc(8, TypeTag::Raw) {
                let header = unsafe { ActorHeap::header_of(ptr) };
                unsafe {
                    // SAFETY: fresh allocation on this actor's heap; the
                    // single scheduler thread is the only mutator.
                    (*header).sticky = true;
                }
                self.cycle_sentinel = Some(header);
            }
        }
        self.cycle_sentinel
    }

    /// Pop a message from the mailbox.
    pub fn receive(&mut self) -> Option<Message> {
        self.mailbox.pop()
    }

    /// Push a message into the mailbox.
    pub fn send(&mut self, msg: Message) -> Result<(), Message> {
        self.mailbox.push(msg)
    }

    /// Set or update a named state field.
    pub fn set_state_field(&mut self, name: impl Into<String>, value: Value) {
        self.state_data.insert(name.into(), value);
    }

    /// Get a named state field.
    pub fn get_state_field(&self, name: &str) -> Option<Value> {
        self.state_data.get(name).copied()
    }

    /// Check if the actor has exceeded its per-turn reduction quota and should yield.
    pub fn should_yield(&self) -> bool {
        self.turn_reductions >= self.max_reductions
    }

    /// Register a named behavior handler.
    ///
    /// The behavior name is used to route messages to the correct handler.
    /// The handler function receives a mutable reference to the actor and
    /// the message payload.
    pub fn register_behavior(
        &mut self,
        name: impl Into<String>,
        handler: fn(&mut Actor, &[Value]),
    ) {
        self.behavior_table.push(BehaviorEntry {
            name: name.into(),
            handler_fn: handler,
        });
    }

    /// Reset the per-turn reduction budget (called after yielding or when the
    /// actor goes waiting). Does not touch the monotonic `reduction_count`.
    pub fn reset_reductions(&mut self) {
        self.turn_reductions = 0;
    }

    /// Increment the reduction count: both the monotonic lifetime metric and
    /// the per-turn budget counter.
    pub fn increment_reductions(&mut self, count: u32) {
        self.reduction_count += count;
        self.turn_reductions += count;
    }

    /// Allocate a null-terminated string on the actor heap and return a pointer
    /// value. Returns nil if allocation fails.
    pub fn allocate_string(&mut self, s: &str) -> Value {
        let bytes = s.as_bytes();
        match self.heap.alloc(bytes.len() + 1, TypeTag::String) {
            Some(ptr) => {
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                    *ptr.add(bytes.len()) = 0;
                }
                Value::ptr(ptr)
            }
            None => Value::nil(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_actor_new() {
        let actor = Actor::new(1, "test", 0);
        assert_eq!(actor.id, 1);
        assert_eq!(actor.name, "test");
        assert_eq!(actor.state, ActorState::Created);
        assert!(!actor.persistent);
        assert!(!actor.is_workflow);
        assert!(!actor.is_agent);
        assert_eq!(actor.max_reductions, 1000);
        assert_eq!(actor.reduction_count, 0);
    }

    #[test]
    fn test_actor_set_and_get_state_field() {
        let mut actor = Actor::new(1, "test", 0);
        actor.set_state_field("key", Value::int(42));
        assert_eq!(actor.get_state_field("key"), Some(Value::int(42)));
    }

    #[test]
    fn test_actor_get_state_field_missing() {
        let actor = Actor::new(1, "test", 0);
        assert_eq!(actor.get_state_field("missing"), None);
    }

    #[test]
    fn test_actor_set_state_field_updates() {
        let mut actor = Actor::new(1, "test", 0);
        actor.set_state_field("key", Value::int(1));
        actor.set_state_field("key", Value::int(2));
        assert_eq!(actor.get_state_field("key"), Some(Value::int(2)));
    }

    #[test]
    fn test_actor_should_yield_false() {
        let actor = Actor::new(1, "test", 0);
        assert!(!actor.should_yield());
    }

    #[test]
    fn test_actor_should_yield_true() {
        let mut actor = Actor::new(1, "test", 0);
        actor.increment_reductions(1000);
        assert!(actor.should_yield());
    }

    #[test]
    fn test_actor_reset_reductions() {
        let mut actor = Actor::new(1, "test", 0);
        actor.increment_reductions(500);
        assert_eq!(actor.reduction_count, 500);
        actor.reset_reductions();
        // The monotonic lifetime count survives the reset; only the per-turn
        // budget is cleared.
        assert_eq!(actor.reduction_count, 500);
        assert!(!actor.should_yield());
    }

    #[test]
    fn test_actor_register_behavior() {
        let mut actor = Actor::new(1, "test", 0);
        fn handler(_actor: &mut Actor, _args: &[Value]) {}
        actor.register_behavior("my_handler", handler);
        assert_eq!(actor.behavior_table.len(), 1);
        assert_eq!(actor.behavior_table[0].name, "my_handler");
    }

    #[test]
    fn test_actor_allocate_string() {
        let mut actor = Actor::new(1, "test", 0);
        let val = actor.allocate_string("hello");
        assert!(!val.is_nil(), "allocation should return a non-nil value");
    }

    #[test]
    fn test_actor_send_receive() {
        let mut actor = Actor::new(1, "test", 0);
        let msg = Message {
            behavior_id: 1,
            payload: vec![Value::int(42)],
            sender: 99,
            priority: MessagePriority::Normal,
        };
        assert!(actor.send(msg.clone()).is_ok());
        let received = actor.receive().expect("should receive a message");
        assert_eq!(received.behavior_id, 1);
        assert_eq!(received.sender, 99);
        assert_eq!(received.payload, vec![Value::int(42)]);
    }
}
