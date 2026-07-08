//! Actor runtime system for Nulang.
//!
//! Provides: actor lifecycle, scheduler, mailbox, heap, GC, supervision,
//! distribution.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

mod actor;
mod scheduler;
mod mailbox;
pub mod heap;
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
mod persistence;

#[cfg(test)]
mod tests;

pub use actor::*;
pub use scheduler::*;
pub use mailbox::*;
pub use heap::*;
pub use gc::{ForeignRefOp, GcStats, OrcaCoordinator, OrcaGc, OrcaHeap, SharedHeapGc};
pub use supervisor::*;
pub use orca_cycle::*;
pub use cluster::*;
pub use distributed::*;
pub use network::*;
pub use crdt::*;
pub use crdt_reg::{ElementId, LWWRegister, MVRegister, RGA, RGAElement};
pub use crdt_manager::*;
pub use timer::*;
pub use registry::*;
pub use process_groups::*;
pub use persistence::*;

use crate::ai::{complete_sync, LlmClient, LlmMessage, LlmRequest, LlmResponse};
use crate::types::ExitReason;
use crate::vm::Value;

// ---------------------------------------------------------------------------
// Global actor ID generator
// ---------------------------------------------------------------------------

static ACTOR_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a fresh, globally unique actor ID.
pub fn fresh_actor_id() -> u64 {
    ACTOR_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Native handler for durable workflow timer-fired messages.
///
/// Advances the workflow's step_index so the workflow can proceed past the
/// step that was waiting on the timer.
fn timer_fired_handler(actor: &mut Actor, _args: &[Value]) {
    if let Some(n) = actor.get_state_field("step_index").and_then(|v| v.as_int()) {
        actor.set_state_field("step_index", Value::int(n + 1));
    }
}

/// Placeholder native handler for bytecode workflow steps.
///
/// Workflow steps are dispatched via `bytecode_offsets`, but the behavior-id
/// space is shared with native handlers. Empty-name placeholders reserve the
/// step ids so internal runtime behaviors (e.g. `__timer_fired`) can live at
/// higher indices without colliding.
fn bytecode_step_placeholder(_actor: &mut Actor, _args: &[Value]) {}

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
    pub node_id: Option<NodeId>,
    pub distributed_enabled: bool,

    // CRDT manager (v0.6)
    pub crdt_manager: Option<CrdtManager>,

    // Timer wheel (v0.7)
    pub timer_wheel: TimerWheel,

    // Actor name registry (v0.7)
    pub registry: ActorRegistry,

    // Process groups (v0.7)
    pub process_groups: ProcessGroups,

    // Persistence engine (v0.7)
    pub persistence: Box<dyn PersistenceStore>,

    // VM used to execute bytecode behavior handlers.
    vm: Option<crate::vm::VM>,

    // LLM client for the v0.9 AI Runtime.
    llm_client: Option<Box<dyn LlmClient>>,

    // Bytecode modules for actors that may need to be recovered after a
    // runtime restart.  Maps actor_id -> (bytecode_module, behavior_offsets,
    // compensation_offsets).
    recovery_modules: HashMap<u64, (crate::bytecode::CodeModule, Vec<usize>, Vec<Option<usize>>)>,
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
            persistence: Box::new(MemoryStore::new()),
            vm: None,
            llm_client: None,
            recovery_modules: HashMap::new(),
        }
    }

    pub fn spawn_actor(
        &mut self,
        init: Box<dyn FnOnce() -> Vec<(String, Value)>>,
    ) -> u64 {
        self.spawn_actor_with_models(init, HashMap::new(), false, None)
    }

    pub fn spawn_persistent_actor(
        &mut self,
        init: Box<dyn FnOnce() -> Vec<(String, Value)>>,
        state_models: HashMap<String, StateModel>,
    ) -> u64 {
        self.spawn_actor_with_models(init, state_models, true, None)
    }

    /// Spawn a durable workflow actor.  Workflows are always persistent and
    /// keep an append-only event journal in addition to snapshots.
    pub fn spawn_workflow_actor(
        &mut self,
        name: &str,
        init: Box<dyn FnOnce() -> Vec<(String, Value)>>,
        state_models: HashMap<String, StateModel>,
    ) -> u64 {
        let id = self.spawn_actor_with_models(init, state_models, true, Some(name));
        id
    }

    fn spawn_actor_with_models(
        &mut self,
        init: Box<dyn FnOnce() -> Vec<(String, Value)>>,
        state_models: HashMap<String, StateModel>,
        persistent: bool,
        workflow: Option<&str>,
    ) -> u64 {
        let id = fresh_actor_id();
        let mut actor = Actor::new(id, format!("actor_{}", id), 256);
        let state_fields = init();
        for (name, value) in state_fields {
            actor.set_state_field(name, value);
        }
        actor.state_models = state_models;
        actor.persistent = persistent;
        let workflow_name = workflow.map(|n| n.to_string());
        if let Some(name) = workflow {
            actor.is_workflow = true;
            actor.name = name.to_string();
            actor.register_behavior("__timer_fired", timer_fired_handler);
        }
        actor.state = ActorState::Running;
        self.actors.insert(id, actor);
        if workflow.is_some() {
            // Seed the workflow event journal with a WorkflowStarted event.
            let seq = self.next_sequence(id);
            let state = {
                let actor = self.actors.get(&id).unwrap();
                let mut state = Vec::new();
                for (field_name, value) in &actor.state_data {
                    let model = actor.state_models.get(field_name).copied().unwrap_or(StateModel::Local);
                    if model.is_persistent() {
                        state.push(PersistedValue::from_value(value));
                    }
                }
                state
            };
            let _ = self.persistence.append_workflow_event(
                id,
                WorkflowEvent::WorkflowStarted {
                    sequence: seq,
                    name: workflow_name.as_ref().unwrap().clone(),
                    state,
                },
            );
            self.checkpoint_actor(id);
        }
        self.scheduler.enqueue(id);
        id
    }

    /// Register bytecode metadata so that a persistent actor can be recovered
    /// after a runtime restart.  The runtime stores the module, behavior
    /// offsets, and saga compensation offsets; `recover_actor` will restore
    /// them on the recreated actor.
    pub fn register_recovery_module(
        &mut self,
        actor_id: u64,
        module: crate::bytecode::CodeModule,
        offsets: Vec<usize>,
        compensation_offsets: Vec<Option<usize>>,
    ) {
        self.recovery_modules
            .insert(actor_id, (module, offsets, compensation_offsets));
    }

    /// Install an LLM client for `perform LLM.ask(...)` calls.
    pub fn set_llm_client(&mut self, client: Box<dyn LlmClient>) {
        self.llm_client = Some(client);
    }

    /// Execute a chat-completion request using the configured LLM client.
    pub fn complete_llm_request(&self, request: LlmRequest) -> Result<LlmResponse, String> {
        let client = self
            .llm_client
            .as_ref()
            .ok_or_else(|| "No LLM client configured".to_string())?;
        complete_sync(client.as_ref(), request)
    }

    /// Execute an LLM request, optionally running tool calls from the response.
    ///
    /// The request's `tools` list is populated from `module.tools`. If the
    /// response contains tool calls, the named functions are looked up in the
    /// module exports, invoked with the provided JSON arguments, and the results
    /// are sent back to the model for a final response.
    pub fn complete_llm_with_tools(
        &self,
        mut request: LlmRequest,
        module: &crate::bytecode::CodeModule,
    ) -> Result<LlmResponse, String> {
        request.tools = module.tools.clone();
        let mut response = self.complete_llm_request(request.clone())?;

        for _ in 0..3 {
            if response.tool_calls.is_empty() {
                break;
            }

            let mut results = Vec::new();
            for call in &response.tool_calls {
                let result = self.invoke_tool_function(module, &call.name, &call.arguments)?;
                results.push((call.name.clone(), result));
            }

            for (name, result) in results {
                request.messages.push(LlmMessage {
                    role: "tool".to_string(),
                    content: format!("{}: {}", name, result),
                });
            }

            response = self.complete_llm_request(request.clone())?;
        }

        Ok(response)
    }

    /// Look up a tool by name and invoke the corresponding exported function.
    fn invoke_tool_function(
        &self,
        module: &crate::bytecode::CodeModule,
        name: &str,
        arguments: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String> {
        let tool = module
            .tools
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| format!("Tool '{}' not found", name))?;

        let export_idx = module
            .exports
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, idx)| *idx)
            .ok_or_else(|| format!("Tool function '{}' is not exported", name))?;

        let func_idx = match module.constants.get(export_idx) {
            Some(crate::bytecode::Constant::FunctionRef(idx)) => *idx,
            _ => return Err(format!("Export '{}' is not a function reference", name)),
        };

        let offset = *module
            .function_table
            .get(func_idx)
            .ok_or_else(|| format!("Function table missing entry for '{}'", name))?;

        let properties = tool
            .parameters
            .get("properties")
            .and_then(|v| v.as_object())
            .ok_or_else(|| format!("Tool '{}' has no parameter schema", name))?;

        let mut vm = crate::vm::VM::new();
        vm.load_module(module.clone());
        let module_idx = 0;
        let mut frame = crate::vm::Frame::new(None, module_idx);
        frame.pc = offset;

        for (i, (param_name, _)) in properties.iter().enumerate().take(256) {
            let json_val = arguments.get(param_name).cloned().unwrap_or(serde_json::Value::Null);
            frame.regs[i] = json_to_vm_value(&mut vm, json_val)?;
        }

        vm.set_current_frame(frame);
        let result = vm
            .run_from(module_idx, offset)
            .map_err(|e| format!("Tool '{}' execution failed: {}", name, e))?;
        Ok(vm.value_to_string(module_idx, result))
    }

    /// Record an emitted event on an actor.  For the event-sourced MVP, each
    /// event also increments every `event_sourced` integer counter by one.
    /// For workflow actors the event is also appended to the durable workflow
    /// journal and a checkpoint is forced.
    pub fn emit_event(&mut self, actor_id: u64, event: &str, args: &[crate::vm::Value]) {
        let is_workflow = self
            .actors
            .get(&actor_id)
            .map(|a| a.is_workflow)
            .unwrap_or(false);
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            actor.event_log.push((event.to_string(), args.to_vec()));
            // MVP: increment all event_sourced Int state fields.
            let event_sourced_names: Vec<String> = actor
                .state_models
                .iter()
                .filter(|(_, model)| **model == StateModel::EventSourced)
                .map(|(name, _)| name.clone())
                .collect();
            for name in event_sourced_names {
                if let Some(n) = actor.get_state_field(&name).and_then(|v| v.as_int()) {
                    actor.set_state_field(name, crate::vm::Value::int(n + 1));
                }
            }
        }
        if is_workflow {
            let seq = self.next_sequence(actor_id);
            if event == "ParallelBranchCompleted" && args.len() == 2 {
                let parallel_step_name =
                    self.resolve_string_constant(actor_id, &args[0]).unwrap_or_default();
                let branch_name =
                    self.resolve_string_constant(actor_id, &args[1]).unwrap_or_default();
                let _ = self.persistence.append_parallel_branch_completed(
                    actor_id,
                    seq,
                    parallel_step_name,
                    branch_name,
                );
                // Persist the progress counter so the snapshot captures which
                // branches have already completed.
                if let Some(actor) = self.actors.get_mut(&actor_id) {
                    let current = actor
                        .get_state_field("parallel_progress")
                        .and_then(|v| v.as_int())
                        .unwrap_or(0);
                    actor.set_state_field("parallel_progress", Value::int(current + 1));
                }
            } else {
                let payload: Vec<PersistedValue> =
                    args.iter().map(PersistedValue::from_value).collect();
                let _ = self.persistence.append_workflow_event(
                    actor_id,
                    WorkflowEvent::Custom {
                        sequence: seq,
                        name: event.to_string(),
                        args: payload,
                    },
                );
            }
            self.checkpoint_actor(actor_id);
        }
    }

    /// Resolve a string-id value to the original string using the actor's
    /// bytecode module constant pool.  Used when persisting emitted events
    /// that carry string metadata (e.g. `ParallelBranchCompleted`).
    fn resolve_string_constant(&self, actor_id: u64, value: &crate::vm::Value) -> Option<String> {
        let string_id = value.as_string_id()?;
        let actor = self.actors.get(&actor_id)?;
        let module = actor.bytecode_module.as_ref()?;
        module.constants.get(string_id as usize).and_then(|c| match c {
            crate::bytecode::Constant::String(s) => Some(s.clone()),
            _ => None,
        })
    }

    /// Append a `TimerSet` workflow event and checkpoint the actor.
    pub fn append_timer_set(&mut self, actor_id: u64, name: &str, duration_ms: u64) -> std::io::Result<()> {
        let seq = self.next_sequence(actor_id);
        self.persistence
            .append_timer_set(actor_id, seq, name.to_string(), duration_ms)?;
        self.checkpoint_actor(actor_id);
        Ok(())
    }

    /// Append a `TimerFired` workflow event and checkpoint the actor.
    pub fn append_timer_fired(&mut self, actor_id: u64, name: &str) -> std::io::Result<()> {
        let seq = self.next_sequence(actor_id);
        self.persistence
            .append_timer_fired(actor_id, seq, name.to_string())?;
        self.checkpoint_actor(actor_id);
        Ok(())
    }

    /// Append a `SignalReceived` workflow event and checkpoint the actor.
    pub fn append_signal_received(
        &mut self,
        actor_id: u64,
        name: &str,
        payload: Option<String>,
    ) -> std::io::Result<()> {
        let seq = self.next_sequence(actor_id);
        self.persistence
            .append_signal_received(actor_id, seq, name.to_string(), payload)?;
        self.checkpoint_actor(actor_id);
        Ok(())
    }

    /// Append a `SagaCompensated` workflow event and checkpoint the actor.
    pub fn append_saga_compensated(&mut self, actor_id: u64, step_name: &str) -> std::io::Result<()> {
        let seq = self.next_sequence(actor_id);
        self.persistence
            .append_saga_compensated(actor_id, seq, step_name.to_string())?;
        self.checkpoint_actor(actor_id);
        Ok(())
    }

    /// Send a named signal to a workflow actor.
    ///
    /// The signal is appended to the durable workflow journal and, if the actor
    /// is currently suspended waiting for this signal, its execution is resumed.
    pub fn signal_workflow(&mut self, actor_id: u64, name: &str, payload: Option<String>) {
        let _ = self.append_signal_received(actor_id, name, payload.clone());

        let should_resume = {
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                actor.received_signals.push((name.to_string(), payload));
                actor.waiting_signal.as_ref().map(|s| s == name).unwrap_or(false)
            } else {
                false
            }
        };

        if should_resume {
            self.resume_suspended_workflow_step(actor_id);
        }
    }

    /// Resume a workflow actor that is suspended waiting for a signal.
    fn resume_suspended_workflow_step(&mut self, actor_id: u64) {
        let suspended = match self.actors.get_mut(&actor_id) {
            Some(actor) => actor.suspended_execution.take(),
            None => return,
        };
        let Some(suspended) = suspended else { return };

        let vm = match self.vm.as_mut() {
            Some(vm) => vm,
            None => {
                // No VM available; put the suspension back so a later message
                // can re-trigger the step.
                if let Some(actor) = self.actors.get_mut(&actor_id) {
                    actor.suspended_execution = Some(suspended);
                }
                return;
            }
        };

        vm.restore_suspended_state(suspended.vm_state);
        let behavior_idx = suspended.behavior_idx;
        let step_name = suspended.step_name;
        let result = vm.resume();

        if let Some(actor) = self.actors.get_mut(&actor_id) {
            actor.waiting_signal = None;
        }

        match result {
            Ok(_) => {
                if self.actor_is_workflow(actor_id) {
                    if let Some(actor) = self.actors.get_mut(&actor_id) {
                        if let Some(n) = actor.get_state_field("step_index").and_then(|v| v.as_int()) {
                            actor.set_state_field("step_index", Value::int(n + 1));
                        }
                    }
                    let seq = self.next_sequence(actor_id);
                    let _ = self.persistence.append_workflow_event(
                        actor_id,
                        WorkflowEvent::StepCompleted {
                            sequence: seq,
                            step_name,
                        },
                    );
                    self.checkpoint_actor(actor_id);
                }
            }
            Err(crate::types::NuError::VMError(ref msg)) if msg == "SignalWait:suspend" => {
                // Suspended again for another signal; state has already been
                // captured by the callback.
            }
            Err(_) => {
                // Step failed after resumption: run saga compensations.
                if self.actor_is_workflow(actor_id) {
                    self.run_saga_compensation(actor_id, behavior_idx);
                }
            }
        }
    }

    pub fn send_message(&mut self, target_id: u64, behavior: &str, args: &[Value]) {
        let behavior_id = self.behavior_id_for(target_id, behavior).unwrap_or(0);
        self.send_message_by_id(target_id, behavior_id, args);
    }

    fn behavior_id_for(&self, target_id: u64, behavior: &str) -> Option<u16> {
        let actor = self.actors.get(&target_id)?;
        let suffix = format!(".{}", behavior);
        actor
            .behavior_table
            .iter()
            .position(|entry| entry.name == behavior || entry.name.ends_with(&suffix))
            .map(|idx| idx as u16)
    }

    pub fn send_message_by_id(&mut self, target_id: u64, behavior_id: u16, args: &[Value]) {
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
            if let Some(ptr) = arg.as_ptr() {
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
                    // Register the cross-actor reference with the cycle detector.
                    // The receiving actor is represented by its pinned sentinel;
                    // the edge target_sentinel -> source_object records that the
                    // target actor holds a reference to the source object.
                    if self.actors.contains_key(&source_actor_id)
                        && self.actors.contains_key(&target_id)
                    {
                        let source_header = unsafe {
                            crate::runtime::heap::ActorHeap::header_of(ptr)
                        };
                        if let Some(target_actor) = self.actors.get_mut(&target_id) {
                            if let Some(sentinel) = target_actor.cycle_sentinel() {
                                self.cycle_detector.register_foreign_ref(
                                    target_id,
                                    sentinel,
                                    source_actor_id,
                                    source_header,
                                );
                            }
                        }
                    }
                }
            }
        }
        self.scheduler.enqueue(target_id);
    }

    pub fn process_gc_ops(&mut self) {
        let ops = std::mem::take(&mut self.coordinator.pending_ops);
        for op in ops {
            // The object_header points to the source actor's heap object.
            let source_header = op.object_header as *mut crate::runtime::heap::OrcaHeader;
            let source_actor = unsafe { (*source_header).actor_id };
            // Remove the edge from the cycle detector graph before applying the
            // ORCA decrement so the graph stays consistent with the ref count.
            if let Some(target_actor) = self.actors.get_mut(&op.target_actor) {
                if let Some(sentinel) = target_actor.cycle_sentinel() {
                    self.cycle_detector.remove_foreign_ref(
                        op.target_actor,
                        sentinel,
                        source_actor,
                        source_header,
                    );
                }
                target_actor.orca_gc.process_foreign_op(&mut target_actor.heap, op);
            }
        }
        let should_detect = self.cycle_detector.should_detect();
        if should_detect {
            let local_ids: std::collections::HashSet<u64> = self.actors.keys().copied().collect();
            self.cycle_detector.set_local_actors(local_ids);
            let rt = self as *mut Runtime;
            let detector = &mut self.cycle_detector;
            unsafe {
                detector.incremental_detect(&mut *rt);
            }
        }
    }

    /// Return a snapshot of scheduler profiling statistics.
    pub fn scheduler_stats(&self) -> SchedulerStats {
        self.scheduler.stats()
    }

    /// Reset scheduler profiling statistics to zero.
    pub fn reset_scheduler_stats(&self) {
        self.scheduler.reset_stats()
    }

    pub fn gc_stats(&self) -> GcStats {
        let total = GcStats::default();
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
            self.tick_timers();
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
            let behavior_idx = msg.behavior_id as usize;
            let handler_fn: Option<fn(&mut Actor, &[Value])> = {
                let actor = match self.actors.get(&actor_id) {
                    Some(a) => a,
                    None => {
                        self.current_actor = None;
                        return;
                    }
                };
                if behavior_idx < actor.behavior_table.len() {
                    Some(actor.behavior_table[behavior_idx].handler_fn)
                } else {
                    None
                }
            };
            let mut processed = false;
            let is_placeholder = self
                .actors
                .get(&actor_id)
                .and_then(|a| a.behavior_table.get(behavior_idx))
                .map(|e| e.name.is_empty())
                .unwrap_or(false);
            if let Some(handler) = handler_fn {
                if !is_placeholder {
                    // Journal the message before handling so recovery can replay it.
                    if self.actor_is_persistent(actor_id) {
                        let seq = self.next_sequence(actor_id);
                        let payload = msg.payload.iter().map(PersistedValue::from_value).collect();
                        let _ = self.persistence.append_journal(
                            actor_id,
                            JournalEntry {
                                sequence: seq,
                                behavior_id: msg.behavior_id,
                                payload,
                            },
                        );
                    }
                    let actor = match self.actors.get_mut(&actor_id) {
                        Some(a) => a,
                        None => {
                            self.current_actor = None;
                            return;
                        }
                    };
                    handler(actor, &msg.payload);
                    // Snapshot durable state after the message is processed.
                    self.checkpoint_actor(actor_id);
                    processed = true;
                }
            }
            if !processed && self.has_bytecode_handler(actor_id, behavior_idx) {
                // Journal before executing bytecode as well.
                if self.actor_is_persistent(actor_id) {
                    let seq = self.next_sequence(actor_id);
                    let payload = msg.payload.iter().map(PersistedValue::from_value).collect();
                    let _ = self.persistence.append_journal(
                        actor_id,
                        JournalEntry {
                            sequence: seq,
                            behavior_id: msg.behavior_id,
                            payload,
                        },
                    );
                }
                let payload = msg.payload.clone();
                let result = self.run_bytecode_behavior(actor_id, behavior_idx, &payload);
                self.checkpoint_actor(actor_id);
                match result {
                    Ok(_) => processed = true,
                    Err(crate::types::NuError::VMError(ref msg)) if msg == "SignalWait:suspend" => {
                        // The step yielded waiting for a signal. Do not mark it
                        // completed and do not run compensations.
                        processed = false;
                    }
                    Err(_) => {
                        // A workflow step failed: run saga compensations for previously
                        // completed steps in reverse order.
                        if self.actor_is_workflow(actor_id) {
                            self.run_saga_compensation(actor_id, behavior_idx);
                        }
                        processed = false;
                    }
                }
            }
            if processed && self.actor_is_workflow(actor_id) && !self.is_internal_behavior(actor_id, behavior_idx) {
                let seq = self.next_sequence(actor_id);
                let step_name = self.step_name_for(actor_id, behavior_idx);
                let _ = self.persistence.append_workflow_event(
                    actor_id,
                    WorkflowEvent::StepCompleted {
                        sequence: seq,
                        step_name,
                    },
                );
                // Synthetic parallel steps do not increment step_index in their
                // bytecode (so signal-waiting branches do not double-increment);
                // advance it here when the step completes.
                if self.is_parallel_step(actor_id, behavior_idx) {
                    if let Some(actor) = self.actors.get_mut(&actor_id) {
                        if let Some(n) = actor.get_state_field("step_index").and_then(|v| v.as_int()) {
                            actor.set_state_field("step_index", Value::int(n + 1));
                        }
                    }
                }
                self.checkpoint_actor(actor_id);
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

    fn actor_is_persistent(&self, actor_id: u64) -> bool {
        self.actors
            .get(&actor_id)
            .map(|a| a.persistent)
            .unwrap_or(false)
    }

    fn actor_is_workflow(&self, actor_id: u64) -> bool {
        self.actors
            .get(&actor_id)
            .map(|a| a.is_workflow)
            .unwrap_or(false)
    }

    fn has_bytecode_handler(&self, actor_id: u64, behavior_idx: usize) -> bool {
        self.actors
            .get(&actor_id)
            .map(|a| {
                a.bytecode_module.is_some()
                    && behavior_idx < a.bytecode_offsets.len()
            })
            .unwrap_or(false)
    }

    fn next_sequence(&self, actor_id: u64) -> u64 {
        self.persistence.latest_sequence(actor_id) + 1
    }

    /// Schedule a durable timer for a workflow actor.
    ///
    /// Appends a `TimerSet` event, checkpoints state, and arms the runtime's
    /// timer wheel. When the timer fires the runtime will append a
    /// `TimerFired` event and deliver a `__timer_fired` message to the actor.
    pub fn schedule_workflow_timer(&mut self, actor_id: u64, name: &str, duration_ms: u64) {
        if self.actor_is_workflow(actor_id) {
            let _ = self.append_timer_set(actor_id, name, duration_ms);
        }
        self.rearm_timer(actor_id, name, duration_ms);
    }

    /// Re-arm a timer from the durable journal without appending a new event.
    /// Used during recovery to restore timers that have not yet fired.
    fn rearm_timer(&mut self, actor_id: u64, name: &str, duration_ms: u64) {
        let behavior_id = self.behavior_id_for(actor_id, "__timer_fired").unwrap_or(0);
        self.timer_wheel.send_after_with_context(
            std::time::Duration::from_millis(duration_ms),
            actor_id,
            behavior_id,
            vec![],
            name.to_string(),
        );
    }

    /// Tick the timer wheel and deliver any fired timers.
    pub fn tick_timers(&mut self) {
        self.tick_timers_at(std::time::Instant::now());
    }

    fn tick_timers_at(&mut self, now: std::time::Instant) {
        let fired = self.timer_wheel.tick(now);
        for (target_actor, message) in fired {
            match message {
                TimerMessage::SendWithContext { behavior_id, payload, context } => {
                    if self.actor_is_workflow(target_actor) {
                        let _ = self.append_timer_fired(target_actor, &context);
                    }
                    self.send_message_by_id(target_actor, behavior_id, &payload);
                }
                TimerMessage::Send { behavior_id, payload } => {
                    self.send_message_by_id(target_actor, behavior_id, &payload);
                }
                TimerMessage::Exit { reason } => {
                    self.exit_actor(target_actor, ExitReason::Error(reason));
                }
                TimerMessage::Kill => {
                    self.kill_actor(target_actor);
                }
            }
        }
    }

    /// Snapshot durable fields of an actor to the persistence store.
    pub fn checkpoint_actor(&mut self, actor_id: u64) {
        let actor = match self.actors.get(&actor_id) {
            Some(a) => a,
            None => return,
        };
        if !actor.persistent {
            return;
        }
        let seq = self.next_sequence(actor_id);
        let mut state = HashMap::new();
        for (name, value) in &actor.state_data {
            let model = actor.state_models.get(name).copied().unwrap_or(StateModel::Local);
            if model.is_persistent() {
                state.insert(name.clone(), PersistedValue::from_value(value));
            }
        }
        let snapshot = ActorSnapshot {
            actor_id,
            sequence: seq,
            state,
            waiting_signal: actor.waiting_signal.clone(),
        };
        let _ = self.persistence.save_snapshot(snapshot);
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            actor.sequence = seq;
        }
    }

    /// Lay out a workflow actor's native behavior table so that bytecode step
    /// ids (0..n-1) do not collide with internal runtime behaviors such as
    /// `__timer_fired`.
    fn layout_workflow_behavior_table(&mut self, actor_id: u64) {
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            if !actor.is_workflow {
                return;
            }
            let step_count = actor.bytecode_offsets.len();
            // Strip any previously registered runtime placeholders/internal
            // behaviors; we'll rebuild them below.
            actor
                .behavior_table
                .retain(|e| !e.name.is_empty() && e.name != "__timer_fired");
            for _ in 0..step_count {
                actor.behavior_table.push(BehaviorEntry {
                    name: String::new(),
                    handler_fn: bytecode_step_placeholder,
                });
            }
            actor.register_behavior("__timer_fired", timer_fired_handler);
        }
    }

    /// Execute a bytecode behavior for an actor.
    fn run_bytecode_behavior(
        &mut self,
        actor_id: u64,
        behavior_idx: usize,
        args: &[Value],
    ) -> crate::types::NuResult<Value> {
        let code_offset = {
            let actor = match self.actors.get(&actor_id) {
                Some(a) => a,
                None => return Ok(Value::nil()),
            };
            actor.bytecode_offsets.get(behavior_idx).copied().unwrap_or(0)
        };
        let result = self.run_bytecode_at_offset(actor_id, code_offset, args);
        // If the step suspended waiting for a signal, record which behavior
        // and step name it was executing so recovery/resumption can continue.
        if let Err(crate::types::NuError::VMError(ref msg)) = result {
            if msg == "SignalWait:suspend" {
                let step_name = self.step_name_for(actor_id, behavior_idx);
                if let Some(actor) = self.actors.get_mut(&actor_id) {
                    if let Some(ref mut suspended) = actor.suspended_execution {
                        suspended.behavior_idx = behavior_idx;
                        suspended.step_name = step_name;
                    }
                }
            }
        }
        result
    }

    /// Execute a saga compensation expression for a completed workflow step.
    fn run_compensation(
        &mut self,
        actor_id: u64,
        behavior_idx: usize,
    ) -> crate::types::NuResult<Value> {
        let code_offset = {
            let actor = match self.actors.get(&actor_id) {
                Some(a) => a,
                None => return Ok(Value::nil()),
            };
            match actor.compensation_offsets.get(behavior_idx).copied().flatten() {
                Some(offset) => offset,
                None => return Ok(Value::nil()),
            }
        };
        self.run_bytecode_at_offset(actor_id, code_offset, &[])
    }

    /// Execute bytecode at a specific code offset for an actor.
    fn run_bytecode_at_offset(
        &mut self,
        actor_id: u64,
        code_offset: usize,
        args: &[Value],
    ) -> crate::types::NuResult<Value> {
        let module = match self.actors.get(&actor_id) {
            Some(a) => match a.bytecode_module.clone() {
                Some(m) => m,
                None => return Ok(Value::nil()),
            },
            None => return Ok(Value::nil()),
        };

        let self_ptr: *mut Runtime = self;
        unsafe {
            if (*self_ptr).vm.is_none() {
                (*self_ptr).vm = Some(crate::vm::VM::new());
            }
            let vm = (*self_ptr).vm.as_mut().unwrap();

            let module_idx = if let Some(idx) = (*self_ptr).actors.get(&actor_id).unwrap().bytecode_module_idx {
                idx
            } else {
                let idx = vm.modules.len();
                vm.load_module(module);
                if let Some(actor) = (*self_ptr).actors.get_mut(&actor_id) {
                    actor.bytecode_module_idx = Some(idx);
                }
                idx
            };

            vm.set_actor_callbacks(Box::new(BytecodeRuntimeCallbacks::new(self_ptr, actor_id)));

            let mut frame = crate::vm::Frame::new(None, module_idx);
            frame.pc = code_offset;
            for (i, arg) in args.iter().enumerate().take(256) {
                frame.regs[i] = *arg;
            }
            vm.set_current_frame(frame);

            let result = vm.run_from(module_idx, code_offset);
            // Capture VM state for a workflow signal wait. Doing this here
            // avoids aliasing the Runtime through the callback while the VM
            // borrow is active.
            if let Err(crate::types::NuError::VMError(ref msg)) = result {
                if msg == "SignalWait:suspend" {
                    if let Some(vm_state) = vm.take_suspended_state() {
                        let signal_name = vm.suspended_signal_name.take();
                        if let Some(actor) = self.actors.get_mut(&actor_id) {
                            actor.waiting_signal = signal_name;
                            actor.suspended_execution = Some(crate::runtime::actor::SuspendedExecution {
                                vm_state,
                                behavior_idx: 0,
                                step_name: String::new(),
                            });
                        }
                    }
                }
            }
            result
        }
    }

    /// Run saga compensations for a workflow step that failed.
    /// Walks backwards through completed steps and executes each compensation
    /// expression in reverse order, skipping steps already marked compensated.
    fn run_saga_compensation(&mut self, actor_id: u64, _failed_behavior_idx: usize) {
        let step_index = self
            .actors
            .get(&actor_id)
            .and_then(|a| a.get_state_field("step_index").and_then(|v| v.as_int()))
            .unwrap_or(0) as usize;

        for behavior_idx in (0..step_index).rev() {
            let already_compensated = {
                let actor = match self.actors.get(&actor_id) {
                    Some(a) => a,
                    None => return,
                };
                let step_name = self.step_name_for(actor_id, behavior_idx);
                actor.compensated_steps.contains(&step_name)
            };
            if already_compensated {
                continue;
            }

            let result = self.run_compensation(actor_id, behavior_idx);
            let step_name = self.step_name_for(actor_id, behavior_idx);
            if result.is_err() {
                // Compensation failed: do not record it as completed.
                continue;
            }
            let _ = self.append_saga_compensated(actor_id, &step_name);
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                if !actor.compensated_steps.contains(&step_name) {
                    actor.compensated_steps.push(step_name);
                }
            }
        }
    }

    /// Return the step name for a workflow behavior index.
    fn step_name_for(&self, actor_id: u64, behavior_idx: usize) -> String {
        if let Some(actor) = self.actors.get(&actor_id) {
            // Prefer real behavior names; skip placeholder entries used to
            // reserve step ids in workflow actors.
            if let Some(entry) = actor.behavior_table.get(behavior_idx) {
                if !entry.name.is_empty() {
                    if let Some(pos) = entry.name.rfind('.') {
                        return entry.name[pos + 1..].to_string();
                    }
                    return entry.name.clone();
                }
            }
            if let Some(module) = &actor.bytecode_module {
                if let Some(entry) = module.behaviors.get(behavior_idx) {
                    if let Some(pos) = entry.name.rfind('.') {
                        return entry.name[pos + 1..].to_string();
                    }
                    return entry.name.clone();
                }
            }
        }
        format!("step_{}", behavior_idx)
    }

    /// Return true if the behavior index belongs to an internal runtime behavior
    /// (not a user-defined workflow step). Internal behaviors do not generate
    /// `StepCompleted` events.
    fn is_internal_behavior(&self, actor_id: u64, behavior_idx: usize) -> bool {
        self.actors
            .get(&actor_id)
            .and_then(|a| a.behavior_table.get(behavior_idx))
            .map(|entry| entry.name == "__timer_fired")
            .unwrap_or(false)
    }

    /// Return true if the workflow behavior at `behavior_idx` is a synthetic
    /// parallel step.  Parallel steps advance step_index in the runtime rather
    /// than in their bytecode.
    fn is_parallel_step(&self, actor_id: u64, behavior_idx: usize) -> bool {
        self.actors
            .get(&actor_id)
            .and_then(|a| a.bytecode_module.as_ref())
            .and_then(|m| m.behaviors.get(behavior_idx))
            .map(|entry| entry.parallel_branches.is_some())
            .unwrap_or(false)
    }

    /// Recover a persistent actor from the latest snapshot and replay the journal.
    ///
    /// For workflow actors the durable workflow event journal is replayed
    /// instead of the message journal, restoring the current step index and
    /// any other state captured in workflow events.
    pub fn recover_actor(&mut self, actor_id: u64) -> Option<u64> {
        let snapshot = self.persistence.load_snapshot(actor_id)?;
        let workflow_events = self.persistence.read_workflow_events(actor_id);
        let is_workflow = self
            .recovery_modules
            .get(&actor_id)
            .map(|(m, _, _)| m.actor_metadata.iter().any(|meta| meta.is_workflow))
            .unwrap_or(!workflow_events.is_empty());

        let mut actor = Actor::new(actor_id, format!("actor_{}", actor_id), 256);
        actor.persistent = true;
        actor.is_workflow = is_workflow;
        actor.sequence = snapshot.sequence;
        actor.waiting_signal = snapshot.waiting_signal;
        for (name, value) in snapshot.state {
            actor.set_state_field(name, value.to_value());
        }
        // Restore bytecode metadata registered for recovery.
        if let Some((module, offsets, comp_offsets)) = self.recovery_modules.get(&actor_id) {
            actor.bytecode_module = Some(module.clone());
            actor.bytecode_offsets = offsets.clone();
            actor.compensation_offsets = comp_offsets.clone();
        }
        if is_workflow {
            self.actors.insert(actor_id, actor);
            self.layout_workflow_behavior_table(actor_id);
        } else {
            self.actors.insert(actor_id, actor);
        }

        if is_workflow {
            // Replay workflow events that arrived after the snapshot.
            let events_to_replay: Vec<_> = workflow_events
                .iter()
                .filter(|e| e.sequence() > snapshot.sequence)
                .cloned()
                .collect();
            let mut fired_timer_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for event in &events_to_replay {
                if let WorkflowEvent::TimerFired { name, .. } = event {
                    fired_timer_names.insert(name.clone());
                }
            }
            for event in &events_to_replay {
                if let Some(actor) = self.actors.get_mut(&actor_id) {
                    Self::apply_workflow_event(actor, event);
                    actor.sequence = event.sequence();
                }
            }
            // Re-arm timers that were set before the snapshot/replay but have
            // not yet fired. Timers are reconstructed from the full durable
            // journal, not just events after the snapshot, because snapshots do
            // not capture pending timers.
            let all_timer_events = self.persistence.read_timer_events(actor_id);
            let mut fired_timer_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for event in &all_timer_events {
                if let WorkflowEvent::TimerFired { name, .. } = event {
                    fired_timer_names.insert(name.clone());
                }
            }
            for event in &all_timer_events {
                if let WorkflowEvent::TimerSet {
                    name,
                    duration_ms,
                    ..
                } = event
                {
                    if !fired_timer_names.contains(name) {
                        self.rearm_timer(actor_id, name, *duration_ms);
                    }
                }
            }
            // If the workflow was in the middle of a step waiting on a signal,
            // re-trigger that step so it can resume from replayed events. We
            // use step_index as the behavior id because each step is compiled
            // to a behavior at the same index.
            let should_resume = self
                .actors
                .get(&actor_id)
                .map(|a| a.waiting_signal.is_some() || a.suspended_execution.is_some())
                .unwrap_or(false);
            if should_resume {
                let current_step = self
                    .actors
                    .get(&actor_id)
                    .and_then(|a| a.get_state_field("step_index"))
                    .and_then(|v| v.as_int())
                    .unwrap_or(0) as u16;
                let has_behavior = self
                    .actors
                    .get(&actor_id)
                    .and_then(|a| a.bytecode_module.as_ref())
                    .map(|m| (current_step as usize) < m.behaviors.len())
                    .unwrap_or(false);
                if has_behavior {
                    self.send_message_by_id(actor_id, current_step, &[]);
                }
            }
        } else {
            // Replay journal entries that arrived after the snapshot.
            let journal = self.persistence.read_journal(actor_id);
            let entries_to_replay: Vec<_> = journal
                .iter()
                .filter(|e| e.sequence > snapshot.sequence)
                .cloned()
                .collect();
            for entry in entries_to_replay {
                let behavior_idx = entry.behavior_id as usize;
                let payload: Vec<Value> = entry.payload.iter().map(|p| p.to_value()).collect();
                if self.has_native_handler(actor_id, behavior_idx) {
                    let handler = self.actors.get(&actor_id)
                        .and_then(|a| a.behavior_table.get(behavior_idx))
                        .map(|b| b.handler_fn)?;
                    if let Some(actor) = self.actors.get_mut(&actor_id) {
                        handler(actor, &payload);
                        actor.sequence = entry.sequence;
                    }
                } else if self.has_bytecode_handler(actor_id, behavior_idx) {
                    self.current_actor = Some(actor_id);
                    let _ = self.run_bytecode_behavior(actor_id, behavior_idx, &payload);
                    self.current_actor = None;
                    if let Some(actor) = self.actors.get_mut(&actor_id) {
                        actor.sequence = entry.sequence;
                    }
                }
            }
        }
        self.scheduler.enqueue(actor_id);
        Some(actor_id)
    }

    /// Apply a single workflow event to an actor's state.  Used during recovery
    /// replay to restore step index and accumulated event-sourced state.
    fn apply_workflow_event(actor: &mut Actor, event: &WorkflowEvent) {
        match event {
            WorkflowEvent::WorkflowStarted { .. } => {
                if actor.get_state_field("step_index").is_some() {
                    actor.set_state_field("step_index", Value::int(0));
                }
            }
            WorkflowEvent::StepCompleted { .. } => {
                if let Some(n) = actor.get_state_field("step_index").and_then(|v| v.as_int()) {
                    actor.set_state_field("step_index", Value::int(n + 1));
                }
                // A completed step (sequential or parallel) clears any stale
                // parallel-progress counter.
                actor.set_state_field("parallel_progress", Value::int(0));
            }
            WorkflowEvent::SagaCompensated { step_name, .. } => {
                // Replay marks the step as already compensated so the runtime
                // does not run its compensation expression again.
                if !actor.compensated_steps.contains(step_name) {
                    actor.compensated_steps.push(step_name.clone());
                }
            }
            // Foundation: timer events are persisted but their runtime
            // scheduling is handled by the timer feature scope.
            WorkflowEvent::TimerSet { .. } | WorkflowEvent::TimerFired { .. } => {}
            WorkflowEvent::SignalReceived { name, payload, .. } => {
                actor.received_signals.push((name.clone(), payload.clone()));
            }
            WorkflowEvent::ParallelBranchCompleted { .. } => {
                let current = actor
                    .get_state_field("parallel_progress")
                    .and_then(|v| v.as_int())
                    .unwrap_or(0);
                actor.set_state_field("parallel_progress", Value::int(current + 1));
            }
            WorkflowEvent::Custom { name, args, .. } => {
                let values: Vec<Value> = args.iter().map(|a| a.to_value()).collect();
                actor.event_log.push((name.clone(), values));
            }
        }
    }

    fn has_native_handler(&self, actor_id: u64, behavior_idx: usize) -> bool {
        self.actors
            .get(&actor_id)
            .and_then(|a| a.behavior_table.get(behavior_idx))
            .map(|e| !e.name.is_empty())
            .unwrap_or(false)
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
        let node_id = NodeId(transport.node_id().0);
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
        let transport = self.transport.take().unwrap();
        let mut cluster = self.cluster.take().unwrap();
        let mut resolver = self.resolver.take().unwrap();
        distributed::process_network_packets(self, &transport, &mut cluster, &mut resolver);
        self.transport = Some(transport);
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
                        let net_node_id = NodeId(to.0);
                        let packet = Packet::Heartbeat {
                            node_id: net_node_id,
                            timestamp: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
                        };
                        transport.send(net_node_id, addr, packet);
                    }
                }
                ClusterAction::NodeJoined { node, addr } => {
                    if let Some(transport) = &mut self.transport {
                        let net_node_id = NodeId(node.0);
                        let _ = transport.connect(net_node_id, addr);
                    }
                }
                ClusterAction::NodeFailed { node } => {
                    if let Some(transport) = &mut self.transport {
                        let net_node_id = NodeId(node.0);
                        transport.disconnect(net_node_id);
                    }
                }
                ClusterAction::NodeLeft { node } => {
                    if let Some(transport) = &mut self.transport {
                        let net_node_id = NodeId(node.0);
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
                    let net_node_id = NodeId(member.node_id.0);
                    transport.send(net_node_id, member.address, packet.clone());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// VM runtime callbacks
// ---------------------------------------------------------------------------

use std::cell::RefCell;
use std::rc::Rc;

/// Bridges the standalone VM to a real `Runtime`.
///
/// Used in tests and in any context where bytecode should create real actors
/// and allocate on the current actor's heap.
pub struct RuntimeVmCallbacks {
    runtime: Rc<RefCell<Runtime>>,
}

impl RuntimeVmCallbacks {
    pub fn new(runtime: Rc<RefCell<Runtime>>) -> Self {
        RuntimeVmCallbacks { runtime }
    }
}

impl std::fmt::Debug for RuntimeVmCallbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeVmCallbacks").finish_non_exhaustive()
    }
}

impl crate::vm::ActorVmCallbacks for RuntimeVmCallbacks {
    fn current_actor_id(&self) -> Option<u64> {
        self.runtime.borrow().current_actor
    }

    fn alloc(
        &mut self,
        size: usize,
        type_tag: crate::runtime::heap::TypeTag,
    ) -> Option<*mut u8> {
        let mut rt = self.runtime.borrow_mut();
        if let Some(actor_id) = rt.current_actor {
            if let Some(actor) = rt.actors.get_mut(&actor_id) {
                return actor.heap.alloc(size, type_tag);
            }
        }
        None
    }

    fn drop_ref(&mut self, ptr: *mut u8) {
        let mut rt = self.runtime.borrow_mut();
        if let Some(actor_id) = rt.current_actor {
            if let Some(actor) = rt.actors.get_mut(&actor_id) {
                unsafe { actor.heap.free(ptr); }
            }
        }
    }

    fn array_len(&self, ptr: *mut u8) -> Option<usize> {
        let rt = self.runtime.borrow();
        if let Some(actor_id) = rt.current_actor {
            if rt.actors.get(&actor_id).is_some() {
                unsafe {
                    let header = &*crate::runtime::heap::ActorHeap::header_of(ptr);
                    if header.type_tag == crate::runtime::heap::TypeTag::Array {
                        let payload_size = header
                            .size
                            .saturating_sub(crate::runtime::heap::ActorHeap::HEADER_SIZE);
                        Some(payload_size / std::mem::size_of::<crate::vm::Value>())
                    } else {
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    fn spawn_actor(
        &mut self,
        module: &crate::bytecode::CodeModule,
        behavior_idx: usize,
        init: Vec<(String, crate::vm::Value)>,
    ) -> crate::vm::Value {
        let mut rt = self.runtime.borrow_mut();
        let meta = module
            .actor_metadata
            .iter()
            .find(|m| m.behavior_indices.contains(&behavior_idx));
        let id = if let Some(meta) = meta {
            let state_models: HashMap<String, crate::runtime::persistence::StateModel> = meta
                .state_models
                .iter()
                .map(|(name, model)| (name.clone(), map_ast_state_model(*model)))
                .collect();
            let defaults = meta.state_defaults.clone();
            rt.spawn_actor_with_models(
                Box::new(move || {
                    let mut fields: Vec<(String, crate::vm::Value)> = defaults
                        .iter()
                        .map(|(name, c)| (name.clone(), crate::vm::constant_to_value(c)))
                        .collect();
                    fields.extend(init);
                    fields
                }),
                state_models,
                meta.persistent,
                if meta.is_workflow {
                    Some(meta.name.as_str())
                } else {
                    None
                },
            )
        } else {
            rt.spawn_actor(Box::new(move || init))
        };
        // Record bytecode behavior offsets so the runtime can execute bytecode handlers.
        let mut offsets = vec![0; module.behaviors.len()];
        let mut compensation_offsets: Vec<Option<usize>> = vec![None; module.behaviors.len()];
        if let Some(meta) = meta {
            for &idx in &meta.behavior_indices {
                if let Some(entry) = module.behaviors.get(idx) {
                    offsets[idx] = entry.code_offset;
                    compensation_offsets[idx] = entry.compensate_offset;
                }
            }
        }
        if let Some(actor) = rt.actors.get_mut(&id) {
            actor.bytecode_module = Some(module.clone());
            actor.bytecode_offsets = offsets.clone();
            actor.compensation_offsets = compensation_offsets.clone();
        }
        if meta.map(|m| m.is_workflow).unwrap_or(false) {
            rt.layout_workflow_behavior_table(id);
        }
        // Keep a copy for recovery after a runtime restart.
        rt.register_recovery_module(id, module.clone(), offsets, compensation_offsets);
        crate::vm::Value::actor_ref(id)
    }

    fn send_message(&mut self, target: crate::vm::Value, behavior_id: u16, args: &[crate::vm::Value]) {
        if let Some(actor_id) = target.as_actor_id() {
            let mut rt = self.runtime.borrow_mut();
            rt.send_message_by_id(actor_id, behavior_id, args);
        }
    }

    fn ask_actor(&mut self, target: crate::vm::Value, behavior_id: u16, args: &[crate::vm::Value]) -> crate::vm::Value {
        // Synchronous request/response requires a response mailbox and
        // dedicated ask protocol; for now treat ask as fire-and-forget.
        self.send_message(target, behavior_id, args);
        crate::vm::Value::nil()
    }

    fn get_state_field(&self, field: &str) -> crate::vm::Value {
        let rt = self.runtime.borrow();
        if let Some(actor_id) = rt.current_actor {
            if let Some(actor) = rt.actors.get(&actor_id) {
                return actor.get_state_field(field).unwrap_or(crate::vm::Value::nil());
            }
        }
        crate::vm::Value::nil()
    }

    fn set_state_field(&mut self, field: &str, value: crate::vm::Value) {
        let mut rt = self.runtime.borrow_mut();
        if let Some(actor_id) = rt.current_actor {
            if let Some(actor) = rt.actors.get_mut(&actor_id) {
                actor.set_state_field(field, value);
            }
        }
    }

    fn emit_event(&mut self, event: &str, args: &[crate::vm::Value]) {
        let mut rt = self.runtime.borrow_mut();
        if let Some(actor_id) = rt.current_actor {
            rt.emit_event(actor_id, event, args);
        }
    }

    fn perform_effect(&mut self, effect_name: &str, regs: &[crate::vm::Value]) -> Option<crate::vm::Value> {
        if effect_name != "Timer" {
            return None;
        }
        let mut rt = self.runtime.borrow_mut();
        let actor_id = rt.current_actor?;
        if !rt.actor_is_workflow(actor_id) {
            return None;
        }
        let name = {
            let vm = rt.vm.as_mut()?;
            let module_idx = vm.current_module_idx()?;
            let string_id = regs.get(0)?.as_string_id()?;
            vm.constant_string(module_idx, string_id)?
        };
        let duration_ms = regs.get(1)?.as_int()? as u64;
        rt.schedule_workflow_timer(actor_id, &name, duration_ms);
        Some(crate::vm::Value::unit())
    }

    fn complete_llm(&mut self, model: &str, prompt: &str) -> Option<String> {
        let rt = self.runtime.borrow();
        let request = LlmRequest {
            model: model.to_string(),
            messages: vec![LlmMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            tools: Vec::new(),
        };
        rt.complete_llm_request(request).ok()?.content
    }
}

/// Raw-pointer callbacks used when the runtime itself executes an actor's
/// bytecode behavior. Holds a transient borrow of the executing `Runtime`.
#[derive(Debug)]
struct BytecodeRuntimeCallbacks {
    runtime: *mut Runtime,
    actor_id: u64,
}

unsafe impl Send for BytecodeRuntimeCallbacks {}
unsafe impl Sync for BytecodeRuntimeCallbacks {}

impl BytecodeRuntimeCallbacks {
    fn new(runtime: *mut Runtime, actor_id: u64) -> Self {
        BytecodeRuntimeCallbacks { runtime, actor_id }
    }
}

impl crate::vm::ActorVmCallbacks for BytecodeRuntimeCallbacks {
    fn current_actor_id(&self) -> Option<u64> {
        Some(self.actor_id)
    }

    fn alloc(&mut self, size: usize, type_tag: crate::runtime::heap::TypeTag) -> Option<*mut u8> {
        unsafe { (*self.runtime).actors.get_mut(&self.actor_id)?.heap.alloc(size, type_tag) }
    }

    fn drop_ref(&mut self, ptr: *mut u8) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                actor.heap.free(ptr);
            }
        }
    }

    fn array_len(&self, ptr: *mut u8) -> Option<usize> {
        unsafe {
            let _actor = (*self.runtime).actors.get(&self.actor_id)?;
            let header = &*crate::runtime::heap::ActorHeap::header_of(ptr);
            if header.type_tag == crate::runtime::heap::TypeTag::Array {
                let payload_size = header.size.saturating_sub(crate::runtime::heap::ActorHeap::HEADER_SIZE);
                Some(payload_size / std::mem::size_of::<crate::vm::Value>())
            } else {
                None
            }
        }
    }

    fn spawn_actor(
        &mut self,
        _module: &crate::bytecode::CodeModule,
        _behavior_idx: usize,
        _init: Vec<(String, crate::vm::Value)>,
    ) -> crate::vm::Value {
        crate::vm::Value::actor_ref(0)
    }

    fn send_message(&mut self, _target: crate::vm::Value, _behavior_id: u16, _args: &[crate::vm::Value]) {}

    fn get_state_field(&self, field: &str) -> crate::vm::Value {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get(&self.actor_id) {
                return actor.get_state_field(field).unwrap_or(crate::vm::Value::nil());
            }
        }
        crate::vm::Value::nil()
    }

    fn set_state_field(&mut self, field: &str, value: crate::vm::Value) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                actor.set_state_field(field, value);
            }
        }
    }

    fn emit_event(&mut self, event: &str, args: &[crate::vm::Value]) {
        unsafe {
            (*self.runtime).emit_event(self.actor_id, event, args);
        }
    }

    fn wait_signal(&mut self, name: &str) -> crate::vm::SignalWaitResult {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get(&self.actor_id) {
                if actor.received_signals.iter().any(|(n, _)| n == name) {
                    return crate::vm::SignalWaitResult::Ready(crate::vm::Value::unit());
                }
            }
            crate::vm::SignalWaitResult::NotReady
        }
    }

    fn suspend_for_signal(&mut self, _name: &str, _vm_state: Option<crate::vm::SuspendedVmState>) {
        // State capture is handled by run_bytecode_at_offset after run_from
        // returns, avoiding aliasing the Runtime through this raw-pointer
        // callback while the VM borrow is active.
    }

    fn perform_effect(&mut self, effect_name: &str, regs: &[crate::vm::Value]) -> Option<crate::vm::Value> {
        unsafe {
            if effect_name != "Timer" {
                return None;
            }
            let actor = (*self.runtime).actors.get(&self.actor_id)?;
            if !actor.is_workflow {
                return None;
            }
            let vm = (*self.runtime).vm.as_mut()?;
            let module_idx = vm.current_module_idx()?;
            let string_id = regs.get(0)?.as_string_id()?;
            let name = vm.constant_string(module_idx, string_id)?;
            let duration_ms = regs.get(1)?.as_int()? as u64;
            (*self.runtime).schedule_workflow_timer(self.actor_id, &name, duration_ms);
            Some(crate::vm::Value::unit())
        }
    }

    fn complete_llm(&mut self, model: &str, prompt: &str) -> Option<String> {
        unsafe {
            let rt = &*self.runtime;
            let module = rt
                .actors
                .get(&self.actor_id)?
                .bytecode_module
                .clone()?;
            let request = LlmRequest {
                model: model.to_string(),
                messages: vec![LlmMessage {
                    role: "user".to_string(),
                    content: prompt.to_string(),
                }],
                tools: module.tools.clone(),
            };
            rt.complete_llm_with_tools(request, &module).ok()?.content
        }
    }
}

fn map_ast_state_model(model: crate::ast::StateModel) -> crate::runtime::persistence::StateModel {
    use crate::ast::StateModel as AstModel;
    use crate::runtime::persistence::StateModel as RuntimeModel;
    match model {
        AstModel::Local => RuntimeModel::Local,
        AstModel::Durable => RuntimeModel::Durable,
        AstModel::EventSourced => RuntimeModel::EventSourced,
        AstModel::Crdt => RuntimeModel::Crdt,
    }
}

/// Convert a JSON value into a Nulang VM value for tool-call arguments.
fn json_to_vm_value(
    vm: &mut crate::vm::VM,
    value: serde_json::Value,
) -> Result<crate::vm::Value, String> {
    match value {
        serde_json::Value::Null => Ok(crate::vm::Value::nil()),
        serde_json::Value::Bool(b) => Ok(crate::vm::Value::bool(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(crate::vm::Value::int(i))
            } else {
                Ok(crate::vm::Value::float(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Ok(vm.allocate_string(&s)),
        _ => Err("Unsupported tool argument type".to_string()),
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
