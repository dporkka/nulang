//! Actor definition and lifecycle.

use super::*;
use super::gc::OrcaGc;
use crate::vm::Value;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

/// Actor state machine: Created → Running → Waiting → Suspended → Terminated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorState {
    Created,
    Running,
    Waiting,    // Mailbox empty, no work
    Suspended,  // Explicitly suspended
    Terminated, // Actor has exited
}

/// An actor: independent unit of computation with isolated state and mailbox.
pub struct Actor {
    pub id: u64,
    pub name: String,
    pub state: ActorState,
    pub mailbox: Mailbox,
    pub heap: ActorHeap,
    pub orca_gc: OrcaGc,                // ORCA GC engine for this actor
    pub state_data: Vec<(String, Value)>, // Named actor state fields
    pub state_models: HashMap<String, StateModel>, // Persistence model per field
    pub event_log: Vec<(String, Vec<Value>)>, // Emitted events for event_sourced actors
    pub persistent: bool,              // Whether this actor survives restarts
    pub is_workflow: bool,             // True if generated from a workflow declaration
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
    pub parent: Option<u64>,       // Supervisor
    pub children: Vec<u64>,        // Supervised actors
    pub monitors: Vec<u64>,        // Actors monitoring this one
    pub links: Vec<u64>,           // Bidirectional links
    pub trap_exits: bool,          // If true, exit signals become messages instead of killing this actor
    pub reduction_count: u32,      // Reductions since last yield
    pub max_reductions: u32,       // Max reductions before yield (preemption)
    pub sequence: u64,             // Last persisted sequence number
    /// Sentinel heap object used by the cycle detector to represent this
    /// actor as a holder of foreign references.
    cycle_sentinel: Option<*mut OrcaHeader>,
    /// Suspended VM state for a workflow step waiting on a signal.
    pub suspended_execution: Option<SuspendedExecution>,
    /// Name of the signal this workflow actor is currently waiting for, if any.
    pub waiting_signal: Option<String>,
    /// Signals that have been received by this workflow actor (name, payload).
    pub received_signals: Vec<(String, Option<String>)>,
    /// True if this actor was generated from an `agent` declaration.
    pub is_agent: bool,
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
            orca_gc: OrcaGc::new(id),         // ORCA GC engine
            state_data: Vec::new(),
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
            reduction_count: 0,
            max_reductions: 1000,
            sequence: 0,
            cycle_sentinel: None,
            suspended_execution: None,
            waiting_signal: None,
            received_signals: Vec::new(),
            is_agent: false,
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
                    (*header).sticky.store(true, Ordering::Relaxed);
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
        let name = name.into();
        if let Some(existing) = self.state_data.iter_mut().find(|(n, _)| n == &name) {
            existing.1 = value;
        } else {
            self.state_data.push((name, value));
        }
    }

    /// Get a named state field.
    pub fn get_state_field(&self, name: &str) -> Option<Value> {
        self.state_data.iter().find(|(n, _)| n == name).map(|(_, v)| *v)
    }

    /// Check if the actor has exceeded its reduction quota and should yield.
    pub fn should_yield(&self) -> bool {
        self.reduction_count >= self.max_reductions
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

    /// Reset the reduction count (called after yielding).
    pub fn reset_reductions(&mut self) {
        self.reduction_count = 0;
    }

    /// Increment the reduction count.
    pub fn increment_reductions(&mut self, count: u32) {
        self.reduction_count += count;
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
