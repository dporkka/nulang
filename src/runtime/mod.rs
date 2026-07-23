//! Actor runtime system for Nulang.
//!
//! Provides: actor lifecycle, scheduler, mailbox, heap, GC, supervision,
//! distribution.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

mod actor;
mod gc;
pub mod heap;
pub(crate) mod heap_serialize;
mod mailbox;
mod scheduler;
pub use heap_serialize::*;
mod cluster;
mod distributed;
mod distributed_context;
mod network;
mod orca_cycle;
pub mod quic_transport;
mod supervisor;
mod supervisor_registry;
use distributed_context::DistributedContext;
mod crdt;
mod crdt_manager;
mod crdt_reg;
mod persistence;
mod process_groups;
mod registry;
mod timer;
mod llm;
mod ai_registry;
mod workflow;
mod exit;
mod distribution;
mod spawn;
mod agent;





#[cfg(test)]
mod tests;

pub use actor::*;
pub use cluster::*;
pub use crdt::*;
pub use crdt_manager::*;
pub use crdt_reg::{LWWRegister, MVRegister, RGAElement, RGA};
pub use distributed::*;
pub use gc::{ForeignRefOp, GcStats, OrcaCoordinator, OrcaGc, OrcaHeap};
pub use heap::*;
pub use mailbox::*;
pub use network::*;
pub use orca_cycle::*;
pub use persistence::*;
pub use process_groups::*;
pub use registry::*;
pub use scheduler::*;
pub use supervisor::*;
pub use supervisor_registry::*;
pub use timer::*;

use crate::ai::{
    complete_sync, LlmClient, LlmError, LlmErrorKind, LlmMessage, LlmRequest, LlmResponse,
};
use crate::types::{ExitReason, VmSuspension};
use crate::vm::Value;

// ---------------------------------------------------------------------------
// Global actor ID generator
// ---------------------------------------------------------------------------

static ACTOR_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a fresh, globally unique actor ID.
pub fn fresh_actor_id() -> u64 {
    ACTOR_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Maximum number of membership entries carried by a single gossip packet.
const GOSSIP_PAYLOAD_MAX_ENTRIES: usize = 256;

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

/// Persisted `waiting_signal` marker for a workflow step suspended on a
/// background LLM call.  A signal wait stores the awaited signal's name so
/// recovery can re-trigger the in-flight step; an LLM suspend has no
/// signal, so this reserved marker plays the same role.  The suspended VM
/// state itself cannot be persisted, so recovery re-runs the step from
/// its last pre-suspend checkpoint and the re-executed `LLM.ask` starts
/// a fresh background call.
const LLM_SUSPEND_MARKER: &str = "__llm_ask_pending__";

/// Choose the `waiting_signal` value for a freshly captured suspension:
/// the awaited signal's name for a signal wait, or the reserved LLM
/// marker for a workflow step suspended on a background LLM call (plain
/// actors store nothing; their suspensions are not re-driven on
/// recovery).
fn suspension_marker(actor: &Actor, signal_name: Option<String>) -> Option<String> {
    match signal_name {
        Some(name) => Some(name),
        None if actor.is_workflow => Some(LLM_SUSPEND_MARKER.to_string()),
        None => None,
    }
}

/// Map the argument of `perform Actor.exit(reason)` onto an `ExitReason`.
/// Ints and strings select the reason kind (`0`/`"normal"`, `1`/`"error"`,
/// `2`/`"kill"`); any other value is a custom reason, and a missing or
/// non-int/non-string argument defaults to a normal exit.
fn actor_exit_reason(value: Option<&Value>, constants: &[crate::bytecode::Constant]) -> ExitReason {
    let Some(value) = value else {
        return ExitReason::Normal;
    };
    if let Some(n) = value.as_int() {
        return match n {
            0 => ExitReason::Normal,
            1 => ExitReason::Error("error".to_string()),
            2 => ExitReason::Kill,
            other => ExitReason::Custom(other.to_string()),
        };
    }
    if let Some(id) = value.as_string_id() {
        let name = match constants.get(id as usize) {
            Some(crate::bytecode::Constant::String(s)) => s.as_str(),
            _ => "",
        };
        return match name {
            "normal" => ExitReason::Normal,
            "error" => ExitReason::Error("error".to_string()),
            "kill" => ExitReason::Kill,
            other => ExitReason::Custom(other.to_string()),
        };
    }
    ExitReason::Normal
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

    // Heaps of exited actors that still have outstanding foreign
    // references.  Dropping a heap while another actor holds a pointer
    // into it would dangle, so such heaps are retired here instead and
    // reclaimed by `reclaim_retired_heaps` once every foreign reference
    // (in-flight op or receiver hold) has drained.
    retired_heaps: Vec<ActorHeap>,

    // Distributed actor system (v0.5)
    pub distributed: DistributedContext,
    // Acknowledged packet sequence numbers (transport-level reliability).
    pub acked_packets: HashSet<u64>,

    // CRDT manager (v0.6)
    pub crdt_manager: Option<CrdtManager>,

    // Number of `sync_crdts` calls made; delta-state syncs run on most
    // rounds, with a full-state repair sync every CRDT_FULL_SYNC_INTERVAL.
    pub(crate) crdt_sync_rounds: u64,

    // Timer wheel (v0.7)
    pub timer_wheel: TimerWheel,
    // Virtual clock for deterministic testing (v0.14). When set, all timer
    // expiry and deadline calculations use this clock instead of wall time.
    pub virtual_clock: Option<VirtualClock>,
    // LLM subsystem (v0.9 AI Runtime): client, worker thread, token budget,
    // completion channel, and non-blocking suspension state.
    pub llm: llm::LlmState,

    // Actor name registry (v0.7)
    pub registry: ActorRegistry,

    // Process groups (v0.7)
    pub process_groups: ProcessGroups,

    // Persistence engine (v0.7)
    pub persistence: Box<dyn PersistenceStore>,

    // VM used to execute bytecode behavior handlers.
    vm: Option<crate::vm::VM>,

    // Depth of in-flight calls on the shared runtime VM
    // (`run_bytecode_at_offset`, `resume_suspended_*`). While > 0 a
    // behavior is mid-execution, so receive-wait wakes requested by
    // `send_message_by_id` must be deferred: resuming the target would
    // nest a second `vm.resume()`/`run_from` inside the running one and
    // clobber the shared frames.
    vm_execution_depth: u32,

    // Actors whose receive-wait wake was deferred while the shared VM was
    // executing (deduplicated). Drained by `vm_exec_end` once the
    // outermost VM call returns; a resumed behavior can itself send and
    // re-queue a wake, so the drain loops until empty.
    pending_receive_wakes: Vec<u64>,

    // True while `vm_exec_end` is draining `pending_receive_wakes`. Nested
    // `vm_exec_end` calls (from resumes issued by the drain) then skip
    // their own drain, so the backlog is processed iteratively instead of
    // by unbounded recursion.
    draining_receive_wakes: bool,

    // Bytecode modules for actors that may need to be recovered after a
    // runtime restart.  Maps actor_id -> (bytecode_module, behavior_offsets,
    // compensation_offsets).
    pub(crate) recovery_modules: HashMap<u64, (crate::bytecode::CodeModule, Vec<usize>, Vec<Option<usize>>)>,
    // Pipelines and debates (v0.9 AI Runtime) — extracted into a registry so
    // the god-object shrinks and the subsystems can evolve independently.
    pub ai: ai_registry::AiRuntimeRegistry,
    // Supervisor teams (v0.9 AI Runtime) — extracted into a registry so the
    // god-object shrinks and the subsystem can evolve independently.
    pub supervisor_teams: SupervisorTeamRegistry,

    // Remote spawn support (v0.5+): behaviors a remote node may spawn here
    // by name (see `register_spawnable_behavior`), plus the results of
    // spawn requests WE issued, keyed by request id
    // (`Some(actor_id)` = spawned, `None` = rejected).
    pub spawnable_behaviors: HashMap<String, fn(&mut Actor, &[Value])>,
    pub pending_spawn_responses: HashMap<u64, Option<u64>>,
    /// Actor ID of the dead-letter queue (created lazily).
    /// Undeliverable messages are routed here.
    pub dlq_actor_id: Option<u64>,
    /// Callback invoked when the scheduler loop reaches true quiescence
    /// (empty run queue, no inflight LLM calls, no pending timers).
    /// The embedder (e.g. NLC guest agent) wires this to host signaling.
    pub idle_callback: Option<Box<dyn FnMut()>>,
    // Test effect handlers — installed via `install_test_handler` to
    // intercept `perform Effect.op` calls in tests.  Key is the qualified
    // name (e.g. "IO.print", "DB.write").  A handler returns `Some(value)`
    // to mock the effect or `None` to fall through to real dispatch.
    pub test_handlers: HashMap<String, Box<dyn Fn(&[Value]) -> Option<Value>>>,
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
            vm_execution_depth: 0,
            retired_heaps: Vec::new(),
            distributed: DistributedContext::new(),
            acked_packets: HashSet::new(),
            crdt_manager: None,
            virtual_clock: None,
            crdt_sync_rounds: 0,
            timer_wheel: TimerWheel::new(),
            registry: ActorRegistry::new(),
            process_groups: ProcessGroups::new(),
            persistence: Box::new(MemoryStore::new()),
            vm: None,
            llm: llm::LlmState::new(),
            pending_receive_wakes: Vec::new(),
            draining_receive_wakes: false,
            idle_callback: None,
            recovery_modules: HashMap::new(),
            ai: ai_registry::AiRuntimeRegistry::new(),
            supervisor_teams: SupervisorTeamRegistry::new(),
            spawnable_behaviors: HashMap::new(),
            pending_spawn_responses: HashMap::new(),
            dlq_actor_id: None,
            test_handlers: HashMap::new(),
        }
    }

    /// Install a test handler that intercepts `perform Effect.op` calls.
    ///
    /// The `effect_name` should be the qualified operation name (e.g.
    /// `"IO.print"`, `"DB.write"`).  The handler receives the frame
    /// registers (r0..rn as set up by the compiler before `Perform`) and
    /// returns `Some(value)` to mock the effect or `None` to fall through
    /// to real dispatch.
    ///
    /// # Example
    /// ```ignore
    /// rt.install_test_handler("DB.write", |regs| {
    ///     // regs[0] = key, regs[1] = value
    ///     Some(Value::unit())  // pretend write succeeded
    pub fn install_test_handler<F>(&mut self, effect_name: &str, handler: F)
    where
        F: Fn(&[Value]) -> Option<Value> + 'static,
    {
        self.test_handlers
            .insert(effect_name.to_string(), Box::new(handler));
    }

    /// Check whether a test handler is installed for `qualified_name` and
    /// return its result if so.
    pub fn check_test_handler(&self, qualified_name: &str, regs: &[Value]) -> Option<Value> {
        self.test_handlers
            .get(qualified_name)
            .and_then(|handler| handler(regs))
    }

    pub fn spawn_actor(&mut self, init: Box<dyn FnOnce() -> Vec<(String, Value)>>) -> u64 {
        spawn::spawn_actor_with_models(self, init, HashMap::new(), false, None)
    }

    pub fn spawn_persistent_actor(
        &mut self,
        init: Box<dyn FnOnce() -> Vec<(String, Value)>>,
        state_models: HashMap<String, StateModel>,
    ) -> u64 {
        spawn::spawn_actor_with_models(self, init, state_models, true, None)
    }

    /// Spawn a durable workflow actor.  Workflows are always persistent and
    /// keep an append-only event journal in addition to snapshots.
    pub fn spawn_workflow_actor(
        &mut self,
        name: &str,
        init: Box<dyn FnOnce() -> Vec<(String, Value)>>,
        state_models: HashMap<String, StateModel>,
    ) -> u64 {
        spawn::spawn_actor_with_models(self, init, state_models, true, Some(name))
    }


    /// Spawn an actor for `module`'s behavior `behavior_idx`, seeded with
    /// the `init` state fields, and wire up its bytecode handlers. Shared
    /// body of both VM-callback `spawn_actor` impls: `RuntimeVmCallbacks`
    /// (spawns from the top-level VM) and `BytecodeRuntimeCallbacks`
    /// (spawns from inside a scheduler-driven behavior on the shared
    /// runtime VM).
    pub fn spawn_from_module(
        &mut self,
        module: &crate::bytecode::CodeModule,
        behavior_idx: usize,
        init: Vec<(String, Value)>,
    ) -> Value {
        spawn::spawn_from_module(self, module, behavior_idx, init)
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
        spawn::register_recovery_module(self, actor_id, module, offsets, compensation_offsets)
    }


    /// Install an LLM client for `perform LLM.ask(...)` calls.
    pub fn set_llm_client(&mut self, client: Box<dyn LlmClient>) {
        agent::set_llm_client(self, client)
    }

    /// Create a new empty pipeline and return its ID.
    pub fn pipeline_new(&mut self) -> u64 {
        agent::pipeline_new(self)
    }
    /// Add a stage to an existing pipeline. Returns the same pipeline ID on
    /// success so fluent construction can continue.
    pub fn pipeline_stage(
        &mut self,
        id: u64,
        name: &str,
        agent_id: u64,
        template: &str,
    ) -> Result<u64, String> {
        agent::pipeline_stage(self, id, name, agent_id, template)
    }
    /// Run a pipeline, returning the output of the final stage.
    pub fn pipeline_run(&mut self, id: u64, input: &str) -> Result<String, String> {
        agent::pipeline_run(self, id, input)
    }
    pub fn supervisor_new(&mut self) -> u64 {
        agent::supervisor_new(self)
    }

    pub fn supervisor_worker(
        &mut self,
        id: u64,
        name: &str,
        agent_id: u64,
        description: &str,
    ) -> Result<u64, String> {
        agent::supervisor_worker(self, id, name, agent_id, description)
    }

    pub fn supervisor_run(&mut self, id: u64, task: &str) -> Result<String, String> {
        agent::supervisor_run(self, id, task)
    }

    /// Create a new debate and return its ID.
    pub fn debate_new(&mut self, topic: &str, rounds: i64, threshold: f64) -> u64 {
        agent::debate_new(self, topic, rounds, threshold)
    }
    /// Add a participant to an existing debate. Returns the same debate ID on
    /// success so fluent construction can continue.
    pub fn debate_participant(
        &mut self,
        id: u64,
        name: &str,
        stance: &str,
        agent_id: u64,
    ) -> Result<u64, String> {
        agent::debate_participant(self, id, name, stance, agent_id)
    }
    /// Run a debate and return the moderator's synthesis.
    pub fn debate_run(&mut self, id: u64) -> Result<String, String> {
        agent::debate_run(self, id)
    }

    /// Convert a VM value to a Rust string using the actor's bytecode module
    /// constant pool for string-id values and reading pointer payloads as
    /// null-terminated UTF-8.
    fn vm_value_to_string(
        value: &crate::vm::Value,
        module: Option<&crate::bytecode::CodeModule>,
    ) -> Option<String> {
        agent::vm_value_to_string(value, module)
    }

    /// Execute an LLM request for an agent actor, reading the agent's model,
    /// system prompt, and episodic memory from durable state. The memory is
    /// updated with the user prompt and assistant response before being saved
    /// back to state.
    pub fn complete_agent_llm(&mut self, actor_id: u64, prompt: &str) -> Option<String> {
        agent::complete_agent_llm(self, actor_id, prompt)
    }

    /// Build a bare LLM request for a non-agent actor bytecode behavior,
    /// with `tools` filled from the actor's bytecode module. Pure
    /// read/build: safe to run before handing the request to a background
    /// worker thread.
    fn build_actor_llm_request(
        &self,
        actor_id: u64,
        model: &str,
        prompt: &str,
    ) -> Option<LlmRequest> {
        agent::build_actor_llm_request(self, actor_id, model, prompt)
    }

    /// Read an actor's state field as a plain string, resolving string-id
    /// values through the runtime VM's constant pools (heap pointer values
    /// are read directly). Useful for tests and tooling that inspect actor
    /// state produced by bytecode behaviors.
    pub fn actor_state_string(&self, actor_id: u64, field: &str) -> Option<String> {
        agent::actor_state_string(self, actor_id, field)
    }

    /// Set a token budget that caps total LLM token consumption.
    ///
    /// After the budget is exhausted `complete_llm_request` returns
    /// `LlmError::BudgetExceeded`.  Charges are applied after each
    /// successful response based on the actual token count returned
    /// by the provider.
    pub fn set_token_budget(&mut self, limit: u64) {
        agent::set_token_budget(self, limit)
    }

    /// Remove any configured token budget.
    pub fn clear_token_budget(&mut self) {
        agent::clear_token_budget(self)
    }
    /// Execute a chat-completion request using the configured LLM client.
    ///
    /// The provided `memory` messages are stored on the request before it is
    /// sent to the provider.
    pub fn complete_llm_request(
        &self,
        mut request: LlmRequest,
        memory: Vec<LlmMessage>,
    ) -> Result<LlmResponse, LlmError> {
        // Check token budget before calling the provider.
        if let Some(ref budget) = self.llm.token_budget {
            if budget.is_exhausted() {
                return Err(LlmError::new(
                    LlmErrorKind::BudgetExceeded,
                    format!("Token budget exhausted (limit: {})", budget.limit()),
                ));
            }
        }
        request.memory = memory;
        let client = self
            .llm.client
            .as_ref()
            .ok_or_else(|| LlmError::from_string("No LLM client configured"))?;
        let response = complete_sync(client.as_ref(), request)?;
        // Charge the budget for actual tokens consumed.
        if let Some(ref budget) = self.llm.token_budget {
            budget.charge(response.usage.total as u64);
        }
        Ok(response)
    }

    /// Execute an LLM request, optionally running tool calls from the response.
    ///
    /// The request's `tools` list is populated from `module.tools`. If the
    /// response contains tool calls, the named functions are looked up in the
    /// module exports, invoked with the provided JSON arguments, and the results
    /// are sent back to the model for a final response.
    /// Execute an LLM request, optionally running tool calls from the response.
    ///
    /// The request's `tools` list is populated from `module.tools`. If the
    /// response contains tool calls, the named functions are looked up in the
    /// module exports, invoked with the provided JSON arguments, and the results
    /// are sent back to the model for a final response. The supplied `memory`
    /// messages are preserved across tool-call rounds.
    pub fn complete_llm_with_tools(
        &mut self,
        mut request: LlmRequest,
        memory: Vec<LlmMessage>,
        module: &crate::bytecode::CodeModule,
    ) -> Result<LlmResponse, LlmError> {
        request.tools = module.tools.clone();
        request.memory = memory.clone();
        let response = self.complete_llm_request(request.clone(), memory.clone())?;
        self.finish_tool_calls(module, response)
    }

    /// Post-process an LLM response on the scheduler thread: invoke any tool
    /// calls named in the response against `module` and synthesize the
    /// response content from their results. Must run on the scheduler thread
    /// because tool invocation executes module functions against runtime
    /// state.
    fn finish_tool_calls(
        &mut self,
        module: &crate::bytecode::CodeModule,
        mut response: LlmResponse,
    ) -> Result<LlmResponse, LlmError> {
        if !response.tool_calls.is_empty() {
            let mut results = Vec::new();
            for call in &response.tool_calls {
                let result =
                    self.invoke_agent_tool_function(module, &call.name, &call.arguments)?;
                results.push((call.name.clone(), result));
            }

            // For agent workflows, return the tool results directly so the
            // caller can decide whether to continue the conversation. Preserve
            // the original tool_calls and usage while surfacing a synthesized
            // content string for memory/logging.
            let result_content = results
                .iter()
                .map(|(name, result)| format!("{}: {}", name, result))
                .collect::<Vec<_>>()
                .join("\n");
            response.content = Some(result_content);
        }

        Ok(response)
    }

    /// Invoke a tool for an agent, routing memory behaviors to the agent's
    /// durable state and falling back to the module's exported function for
    /// other tools.
    fn invoke_agent_tool_function(
        &mut self,
        module: &crate::bytecode::CodeModule,
        name: &str,
        arguments: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String> {
        if let Some(actor_id) = self.current_actor {
            if self.actor_is_agent(actor_id) && self.is_semantic_memory_behavior(name) {
                return self.invoke_semantic_memory_tool(actor_id, name, arguments);
            }
            if self.actor_is_agent(actor_id) && self.is_procedural_memory_behavior(name) {
                return self.invoke_procedural_memory_tool(actor_id, name, arguments);
            }
        }
        self.invoke_tool_function(module, name, arguments)
    }

    /// Execute a semantic-memory tool call against the current agent.
    fn invoke_semantic_memory_tool(
        &mut self,
        actor_id: u64,
        name: &str,
        arguments: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String> {
        if name == "store_fact" {
            let content = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mut metadata = std::collections::HashMap::new();
            if let Some(topic) = arguments.get("topic").and_then(|v| v.as_str()) {
                metadata.insert("topic".to_string(), topic.to_string());
            }
            let id = self.semantic_memory_store_with_metadata(actor_id, &content, metadata);
            Ok(format!(
                "stored: {}",
                self.vm_value_to_string_or_default(actor_id, &id)
            ))
        } else if name == "recall" {
            let query = arguments
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let top_k = arguments.get("top_k").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            let value = self.semantic_memory_recall(actor_id, &query, top_k);
            Ok(self.vm_value_to_string_or_default(actor_id, &value))
        } else {
            Err(format!("Unknown semantic-memory tool '{}'", name))
        }
    }

    /// Execute a procedural-memory tool call against the current agent.
    fn invoke_procedural_memory_tool(
        &mut self,
        actor_id: u64,
        name: &str,
        arguments: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<String, String> {
        match name {
            "store_pattern" => {
                let key = arguments
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input_pattern = arguments
                    .get("input_pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let output_template = arguments
                    .get("output_template")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let value = self.procedural_memory_store_pattern(
                    actor_id,
                    &key,
                    &input_pattern,
                    &output_template,
                );
                Ok(self.vm_value_to_string_or_default(actor_id, &value))
            }
            "get_pattern" => {
                let key = arguments
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let value = self.procedural_memory_get_pattern(actor_id, &key);
                Ok(self.vm_value_to_string_or_default(actor_id, &value))
            }
            "add_example" => {
                let task = arguments
                    .get("task")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = arguments
                    .get("input")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let output = arguments
                    .get("output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                self.procedural_memory_add_example(actor_id, &task, &input, &output);
                Ok("ok".to_string())
            }
            "get_examples" => {
                let task = arguments
                    .get("task")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let query = arguments
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let top_k = arguments.get("top_k").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
                let value = self.procedural_memory_get_examples(actor_id, &task, &query, top_k);
                Ok(self.vm_value_to_string_or_default(actor_id, &value))
            }
            _ => Err(format!("Unknown procedural-memory tool '{}'", name)),
        }
    }

    /// Convert a VM value into a Rust string, returning a default for missing actors.
    fn vm_value_to_string_or_default(&self, actor_id: u64, value: &crate::vm::Value) -> String {
        self.actors
            .get(&actor_id)
            .and_then(|actor| self.vm_value_to_string_in_actor(value, actor))
            .unwrap_or_default()
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
            let json_val = arguments
                .get(param_name)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            frame.regs[i] = json_to_vm_value(&mut vm, json_val)?;
        }

        vm.set_current_frame(frame);
        let result = vm
            .run_from(module_idx, offset)
            .map_err(|e| format!("Tool '{}' execution failed: {}", name, e))?;
        Ok(vm.value_to_string(module_idx, result))
    }

    /// Record an emitted event on an actor. Delegates to the workflow subsystem.
    pub fn emit_event(&mut self, actor_id: u64, event: &str, args: &[crate::vm::Value]) {
        workflow::emit_event(self, actor_id, event, args)
    }

    /// Append a `TimerSet` workflow event and checkpoint the actor.
    pub fn append_timer_set(
        &mut self,
        actor_id: u64,
        name: &str,
        duration_ms: u64,
    ) -> std::io::Result<()> {
        workflow::append_timer_set(self, actor_id, name, duration_ms)
    }

    /// Append a `TimerFired` workflow event and checkpoint the actor.
    pub fn append_timer_fired(&mut self, actor_id: u64, name: &str) -> std::io::Result<()> {
        workflow::append_timer_fired(self, actor_id, name)
    }

    /// Append a `SignalReceived` workflow event and checkpoint the actor.
    pub fn append_signal_received(
        &mut self,
        actor_id: u64,
        name: &str,
        payload: Option<String>,
    ) -> std::io::Result<()> {
        workflow::append_signal_received(self, actor_id, name, payload)
    }

    /// Append a `SagaCompensated` workflow event and checkpoint the actor.
    pub fn append_saga_compensated(
        &mut self,
        actor_id: u64,
        step_name: &str,
    ) -> std::io::Result<()> {
        workflow::append_saga_compensated(self, actor_id, step_name)
    }

    /// Send a named signal to a workflow actor.
    ///
    /// The signal is appended to the durable workflow journal and, if the actor
    /// is currently suspended waiting for this signal, its execution is resumed.
    /// Deliver a signal to a workflow actor. Delegates to workflow subsystem.
    pub fn signal_workflow(&mut self, actor_id: u64, name: &str, payload: Option<String>) {
        workflow::signal_workflow(self, actor_id, name, payload)
    }

    /// Register a read-only query handler on a workflow actor.
    ///
    /// The handler is a function/closure value invoked by `query_workflow`
    /// with the workflow actor bound as `self`, so it can read the actor's
    /// current state.  Registration is a no-op for missing or non-workflow
    /// actors: queries are a workflow-only concept.  Handlers are not
    /// journaled, so they must be re-registered after a node restart.
    /// Register a read-only query handler on a workflow actor.
    pub fn register_workflow_query(&mut self, actor_id: u64, name: &str, handler: Value) {
        workflow::register_workflow_query(self, actor_id, name, handler)
    }

    /// Invoke a registered query handler on a workflow actor and return its
    /// result.  Returns `None` when the actor is missing, is not a workflow,
    /// has no handler registered under `name`, or the handler value does not
    /// resolve to a function in the actor's bytecode module.
    ///
    /// Queries are read-only: unlike `signal_workflow` they append nothing
    /// to the durable workflow journal, force no checkpoint, and never
    /// resume a suspended step.  The handler runs on a private VM with the
    /// workflow actor bound as `self`, so a query performed from inside a
    /// running behavior cannot disturb that behavior's frames; handlers
    /// must therefore be immediate (non-capturing) functions, since closure
    /// environments live on the VM that created them.
    /// Invoke a registered query handler on a workflow actor. Delegates to workflow subsystem.
    pub fn query_workflow(&mut self, actor_id: u64, name: &str) -> Option<Value> {
        workflow::query_workflow(self, actor_id, name)
    }

    /// Drain completed background LLM calls and resume the suspended actors
    /// waiting for them.
    fn poll_llm_completions(&mut self) {
        while let Ok((actor_id, result)) = self.llm.rx.try_recv() {
            self.store_llm_completion(actor_id, result);
        }
    }

    /// Record a completed background LLM call on its actor and resume the
    /// actor's suspended behavior, if any. Errors trigger the retry/fallback
    /// pipeline when the actor has a configured agent retry or fallback.
    fn store_llm_completion(&mut self, actor_id: u64, result: Result<LlmResponse, LlmError>) {
        self.llm.inflight_count = self.llm.inflight_count.saturating_sub(1);
        match result {
            Ok(response) => {
                if let Some(actor) = self.actors.get_mut(&actor_id) {
                    actor.llm_inflight = false;
                    actor.llm_pending_prompt = None;
                    actor.llm_completed = Some(Ok(response));
                }
                if self
                    .actors
                    .get(&actor_id)
                    .map(|a| a.suspended_execution.is_some())
                    .unwrap_or(false)
                {
                    self.resume_suspended_llm_step(actor_id);
                }
            }
            Err(error) => {
                self.handle_llm_error(actor_id, error);
            }
        }
    }

    /// Process an LLM error: decide whether to retry, fall back, or fail.
    fn handle_llm_error(&mut self, actor_id: u64, error: LlmError) {
        // Only agent actors have retry/fallback config.
        let is_agent = self
            .actors
            .get(&actor_id)
            .map(|a| a.is_agent)
            .unwrap_or(false);
        if !is_agent {
            // Non-agent actors: store the error and resume.
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                actor.llm_inflight = false;
                actor.llm_pending_prompt = None;
                actor.llm_completed = Some(Err(error));
                if actor.suspended_execution.is_some() {
                    self.resume_suspended_llm_step(actor_id);
                    return;
                }
            }
            return;
        }

        // Read retry/fallback config from cached actor fields (parsed once at
        // agent init), plus mutable state for attempt tracking and prompt.
        let (retry_config, fallback_config, attempt, fallback_step, prompt) = {
            let actor = match self.actors.get(&actor_id) {
                Some(a) => a,
                None => return,
            };
            let retry = actor.retry_config.clone();
            let fallback = actor.fallback_config.clone();
            let attempt_val = actor
                .get_state_field("llm_attempt")
                .and_then(|v| v.as_int())
                .unwrap_or(0) as u32;
            let fallback_step_val = actor
                .get_state_field("llm_fallback_step")
                .and_then(|v| v.as_int())
                .unwrap_or(0) as usize;
            let prompt_val = actor.llm_pending_prompt.clone().unwrap_or_default();
            (retry, fallback, attempt_val, fallback_step_val, prompt_val)
        };

        // --- Retry path ---
        if let Some(ref retry) = retry_config {
            if attempt < retry.max_attempts {
                let new_attempt = attempt + 1;
                // Update llm_attempt in actor state.
                if let Some(actor) = self.actors.get_mut(&actor_id) {
                    actor.llm_inflight = false; // will be set true again on re-dispatch
                    actor.set_state_field("llm_attempt", crate::vm::Value::int(new_attempt as i64));
                }
                let delay_ms = compute_backoff(retry, attempt, actor_id);
                self.timer_wheel
                    .schedule_llm_retry(std::time::Duration::from_millis(delay_ms), actor_id);
                return;
            }
        }

        // --- Fallback path ---
        if fallback_step < fallback_config.len() {
            let error_kind_name = format!("{:?}", error.kind); // "Timeout", "RateLimit", etc.
            let fb = &fallback_config[fallback_step];
            let fb_matches = fb.on.is_empty() || fb.on.iter().any(|k| *k == error_kind_name);
            let new_fallback_step = fallback_step + 1;
            if fb_matches {
                // Swap model and apply context pruning if needed.
                if let Some(actor) = self.actors.get_mut(&actor_id) {
                    actor.llm_inflight = false;
                    let model_ptr = actor.allocate_string(&fb.model);
                    actor.set_state_field("model", model_ptr);
                    actor.set_state_field("llm_attempt", crate::vm::Value::int(0));
                    actor.set_state_field(
                        "llm_fallback_step",
                        crate::vm::Value::int(new_fallback_step as i64),
                    );
                    if let Some(max_tokens) = fb.max_tokens {
                        self.prune_episodic_memory(actor_id, max_tokens);
                    }
                }
                // Re-dispatch the LLM request with the new model.
                self.redispatch_llm_request(actor_id, &prompt);
                return;
            }
            // Current fallback entry's `on` list didn't match this error;
            // advance to the next entry and retry the decision.
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                actor.set_state_field("llm_attempt", crate::vm::Value::int(0));
                actor.set_state_field(
                    "llm_fallback_step",
                    crate::vm::Value::int(new_fallback_step as i64),
                );
            }
            self.handle_llm_error(actor_id, error);
            return;
        }

        // --- Terminal: all retries and fallbacks exhausted ---
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            actor.llm_inflight = false;
            actor.llm_pending_prompt = None;
            actor.llm_completed = Some(Err(error));
            if actor.suspended_execution.is_some() {
                self.resume_suspended_llm_step(actor_id);
            }
        }
    }

    /// Re-dispatch an in-flight LLM request on retry timer fire.
    fn handle_llm_retry_timer(&mut self, actor_id: u64) {
        let prompt = self
            .actors
            .get(&actor_id)
            .and_then(|a| a.llm_pending_prompt.clone())
            .unwrap_or_default();
        // Clear old pending prompt so re-dispatch doesn't duplicate.
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            actor.llm_pending_prompt = None;
        }
        self.redispatch_llm_request(actor_id, &prompt);
    }

    /// Build and dispatch an LLM request for the actor, marking it in-flight.
    fn redispatch_llm_request(&mut self, actor_id: u64, prompt: &str) {
        let is_agent = self
            .actors
            .get(&actor_id)
            .map(|a| a.is_agent)
            .unwrap_or(false);
        let request = if is_agent {
            agent::build_agent_llm_request(self, actor_id, prompt)
        } else {
            let model = self
                .actors
                .get(&actor_id)
                .and_then(|a| {
                    let module = a.bytecode_module.as_ref()?;
                    Self::vm_value_to_string(&a.get_state_field("model")?, Some(module))
                })
                .unwrap_or_default();
            self.build_actor_llm_request(actor_id, &model, prompt)
        };
        let Some(request) = request else {
            // Build failed: store nil error and resume.
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                actor.llm_completed = Some(Ok(LlmResponse {
                    content: None,
                    tool_calls: Vec::new(),
                    model: String::new(),
                    finish_reason: "error".to_string(),
                    usage: Default::default(),
                }));
                if actor.suspended_execution.is_some() {
                    self.resume_suspended_llm_step(actor_id);
                    return;
                }
            }
            return;
        };
        if !self.dispatch_llm_request(actor_id, request, prompt) {
            // Dispatch failed (e.g. worker thread exited): fail gracefully.
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                actor.llm_completed = Some(Ok(LlmResponse {
                    content: None,
                    tool_calls: Vec::new(),
                    model: String::new(),
                    finish_reason: "error".to_string(),
                    usage: Default::default(),
                }));
                if actor.suspended_execution.is_some() {
                    self.resume_suspended_llm_step(actor_id);
                }
            }
        }
    }

    /// Send an LLM request to the persistent worker thread for execution.
    /// Returns true if the request was dispatched, false if the worker
    /// channel is unavailable (caller should roll back in-flight state).
    fn dispatch_llm_request(&mut self, actor_id: u64, request: LlmRequest, prompt: &str) -> bool {
        let Some(client) = self.llm.client.clone() else {
            return false;
        };
        let Some(tx) = self.llm.request_tx.as_ref() else {
            return false;
        };
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            actor.llm_inflight = true;
            actor.llm_pending_prompt = Some(prompt.to_string());
        }
        self.llm.inflight_count += 1;
        tx.send(llm::LlmWorkItem {
            actor_id,
            request,
            client,
        })
        .is_ok()
    }

    /// Prune an agent's episodic memory to fit within `max_tokens`, using a
    /// character-count heuristic (chars / 4). Always preserves the system
    /// prompt (which lives in its own state field).
    fn prune_episodic_memory(&mut self, actor_id: u64, max_tokens: usize) {
        let memory_json = {
            let actor = match self.actors.get(&actor_id) {
                Some(a) => a,
                None => return,
            };
            let module = match actor.bytecode_module.as_ref() {
                Some(m) => m,
                None => return,
            };
            Self::vm_value_to_string(
                &actor
                    .get_state_field("episodic_memory")
                    .unwrap_or(crate::vm::Value::nil()),
                Some(module),
            )
            .unwrap_or_default()
        };
        let mut memory: crate::ai::EpisodicMemory = serde_json::from_str(&memory_json)
            .unwrap_or_else(|_| crate::ai::EpisodicMemory::new(50));

        let max_chars = max_tokens.saturating_mul(4);
        let total_chars: usize = memory.turns.iter().map(|t| t.content.len()).sum();
        while total_chars > max_chars && !memory.turns.is_empty() {
            // Remove oldest non-system turn.
            if memory.turns.len() > 1 {
                memory.turns.remove(0);
            } else {
                break;
            }
        }

        let updated_json = serde_json::to_string(&memory).unwrap_or_default();
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            let ptr = actor.allocate_string(&updated_json);
            actor.set_state_field("episodic_memory", ptr);
        }
    }

    /// Resume an actor whose bytecode behavior suspended on
    /// `perform LLM.ask` once the background worker has delivered the
    /// response. The re-executed `LlmAsk` picks the response up from
    /// `actor.llm_completed` via the VM callback.
    fn resume_suspended_llm_step(&mut self, actor_id: u64) {
        let suspended = match self.actors.get_mut(&actor_id) {
            Some(actor) => actor.suspended_execution.take(),
            None => return,
        };
        let Some(suspended) = suspended else { return };

        if self.vm.is_none() {
            // No VM available; put the suspension back so a later message
            // can re-trigger the step.
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                actor.suspended_execution = Some(suspended);
            }
            return;
        }

        let self_ptr: *mut Runtime = self;
        unsafe {
            let vm = (*self_ptr).vm.as_mut().unwrap();
            // Re-install callbacks bound to THIS actor: other actors may have
            // run on the shared VM while this one was suspended.
            vm.set_actor_callbacks(Box::new(BytecodeRuntimeCallbacks::new(self_ptr, actor_id)));
            vm.set_distributed_callbacks(Box::new(BytecodeDistributedCallbacks {
                runtime: self_ptr,
            }));
            vm.restore_suspended_state(suspended.vm_state);
            let saved_suspend = (*self_ptr).llm.suspend_enabled;
            (*self_ptr).llm.suspend_enabled = true;
            (*self_ptr).vm_exec_begin();
            let result = vm.resume();
            (*self_ptr).llm.suspend_enabled = saved_suspend;
            match result {
                Ok(_) => {
                    // The suspended step ran to completion. For workflow
                    // actors record the completion the same way
                    // resume_suspended_workflow_step does: clear the
                    // suspension marker, advance step_index, append
                    // StepCompleted, and checkpoint.
                    if (*self_ptr).actor_is_workflow(actor_id) {
                        if let Some(actor) = (*self_ptr).actors.get_mut(&actor_id) {
                            actor.waiting_signal = None;
                            if let Some(n) =
                                actor.get_state_field("step_index").and_then(|v| v.as_int())
                            {
                                actor.set_state_field("step_index", Value::int(n + 1));
                            }
                        }
                        let seq = (*self_ptr).next_sequence(actor_id);
                        let _ = (*self_ptr).persistence.append_workflow_event(
                            actor_id,
                            WorkflowEvent::StepCompleted {
                                sequence: seq,
                                step_name: suspended.step_name,
                            },
                        );
                        (*self_ptr).checkpoint_actor(actor_id);
                    }
                }
                Err(crate::types::NuError::Suspended(_)) => {
                    // Suspended again (e.g. a chained `perform LLM.ask` or a
                    // signal wait): re-capture the VM state so the next
                    // completion or signal can resume it.
                    if let Some(vm_state) = vm.take_suspended_state() {
                        let signal_name = vm.suspended_signal_name.take();
                        let receive_timeout = vm.suspended_receive_timeout.take();
                        if let Some(actor) = (*self_ptr).actors.get_mut(&actor_id) {
                            let marker = suspension_marker(actor, signal_name);
                            actor.waiting_signal = marker;
                            actor.suspended_execution =
                                Some(crate::runtime::actor::SuspendedExecution {
                                    vm_state,
                                    behavior_idx: suspended.behavior_idx,
                                    step_name: suspended.step_name,
                                });
                        }
                        // A chained receive-after suspend arms its timeout
                        // here; a no-op for the other sentinels.
                        (*self_ptr).maybe_schedule_receive_wait(actor_id, receive_timeout);
                    }
                }
                // Other errors: the send-path result is discarded anyway,
                // matching step_actor semantics.
                Err(_) => {}
            }
            // End the VM-execution window only after any suspend-state
            // re-capture above: draining deferred wakes runs other actors
            // on the shared VM, which would clobber the frames an
            // un-captured suspend still needs. Runs on every path, so
            // wakes of other actors are not lost when THIS one suspends.
            (*self_ptr).vm_exec_end();
        }
        // The suspension resolved (completed or failed): if messages queued
        // up while the behavior was suspended, schedule the actor to drain
        // them — step_actor leaves mail untouched while a suspension is live.
        self.requeue_if_mail_pending(actor_id);
    }

    /// Re-enqueue an actor whose suspension has resolved if messages queued
    /// up while it was suspended.  step_actor refuses to run new messages
    /// while a suspension is live, so without this the queued mail would
    /// sit until an unrelated send happened to re-enqueue the actor.
    fn requeue_if_mail_pending(&mut self, actor_id: u64) {
        let needs_requeue = self
            .actors
            .get(&actor_id)
            .map(|a| a.suspended_execution.is_none() && !a.mailbox.is_empty())
            .unwrap_or(false);
        if needs_requeue {
            self.enqueue_actor(actor_id);
        }
    }

    /// Enqueue an actor on the scheduler at its current priority. All
    /// scheduler enqueue paths go through here so a priority set via
    /// `perform Actor.set_priority` takes effect on the next (re)queue;
    /// unknown actors (e.g. already exited) enqueue at the Normal default.
    pub(crate) fn enqueue_actor(&self, actor_id: u64) {
        let priority = self
            .actors
            .get(&actor_id)
            .map(|a| a.priority)
            .unwrap_or_default();
        self.scheduler.enqueue_with_priority(actor_id, priority);
    }

    /// Mark the start of a call into the shared runtime VM. While the
    /// depth is non-zero, receive-wait wakes are deferred onto
    /// `pending_receive_wakes` (see `send_message_by_id`).
    fn vm_exec_begin(&mut self) {
        self.vm_execution_depth += 1;
    }

    /// Mark the end of a call into the shared runtime VM. When the
    /// outermost call returns, drain the deferred receive-wait wakes: a
    /// resumed behavior can itself send and re-queue a wake, so loop until
    /// the backlog is empty. The drain flag keeps this iterative — a
    /// nested `vm_exec_end` (from a resume issued by the drain) returns
    /// without draining again.
    fn vm_exec_end(&mut self) {
        self.vm_execution_depth = self.vm_execution_depth.saturating_sub(1);
        if self.vm_execution_depth > 0 || self.draining_receive_wakes {
            return;
        }
        self.draining_receive_wakes = true;
        while let Some(target_id) = self.pending_receive_wakes.pop() {
            // The drain can run inside another actor's step: attribute
            // sends by the resumed behavior to the resumed actor, not to
            // the interrupted one.
            let prev_current_actor = self.current_actor;
            self.current_actor = Some(target_id);
            self.resume_suspended_receive_wait(target_id);
            self.current_actor = prev_current_actor;
        }
        self.draining_receive_wakes = false;
    }

    /// Resume a workflow actor that is suspended waiting for a signal.
    pub(crate) fn resume_suspended_workflow_step(&mut self, actor_id: u64) {
        let suspended = match self.actors.get_mut(&actor_id) {
            Some(actor) => actor.suspended_execution.take(),
            None => return,
        };
        let Some(suspended) = suspended else { return };

        if self.vm.is_none() {
            // No VM available; put the suspension back so a later message
            // can re-trigger the step.
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                actor.suspended_execution = Some(suspended);
            }
            return;
        }

        let behavior_idx = suspended.behavior_idx;
        let step_name = suspended.step_name;
        let self_ptr: *mut Runtime = self;
        let result = unsafe {
            let vm = (*self_ptr).vm.as_mut().unwrap();
            // Re-install callbacks bound to THIS actor: other actors may have
            // run on the shared VM while this one was suspended, and a resumed
            // `LLM.ask` must record its in-flight call (and later completion)
            // on this actor — same as resume_suspended_llm_step.
            vm.set_distributed_callbacks(Box::new(BytecodeDistributedCallbacks {
                runtime: self_ptr,
            }));
            vm.set_actor_callbacks(Box::new(BytecodeRuntimeCallbacks::new(self_ptr, actor_id)));
            vm.restore_suspended_state(suspended.vm_state);
            // A signal-resumed step is still scheduler-context execution: a
            // `perform LLM.ask` after the wait must suspend (non-blocking)
            // instead of blocking the caller thread on the HTTP call.
            let saved_suspend = (*self_ptr).llm.suspend_enabled;
            (*self_ptr).llm.suspend_enabled = true;
            (*self_ptr).vm_exec_begin();
            let result = vm.resume();
            (*self_ptr).llm.suspend_enabled = saved_suspend;
            result
        };

        if let Some(actor) = self.actors.get_mut(&actor_id) {
            actor.waiting_signal = None;
        }

        match result {
            Ok(_) => {
                if self.actor_is_workflow(actor_id) {
                    if let Some(actor) = self.actors.get_mut(&actor_id) {
                        if let Some(n) =
                            actor.get_state_field("step_index").and_then(|v| v.as_int())
                        {
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
            Err(crate::types::NuError::Suspended(_)) => {
                // Suspended again — waiting for another signal OR on a
                // background LLM call (`perform LLM.ask` after the wait).
                // Re-capture the VM state so the next matching signal or the
                // pumped LLM completion can resume the step.  The marker is
                // the awaited signal's name for a signal wait, or the
                // reserved LLM marker (via suspension_marker) for an LLM
                // suspend, whose completion flows through
                // resume_suspended_llm_step — that path performs the workflow
                // completion bookkeeping.  BytecodeRuntimeCallbacks::
                // suspend_for_signal is a no-op, so the capture must happen
                // here — same as in run_bytecode_at_offset and
                // resume_suspended_llm_step.
                let recaptured = match self.vm.as_mut() {
                    Some(vm) => vm.take_suspended_state().map(|vm_state| {
                        let signal_name = vm.suspended_signal_name.take();
                        let receive_timeout = vm.suspended_receive_timeout.take();
                        (vm_state, signal_name, receive_timeout)
                    }),
                    None => None,
                };
                if let Some((vm_state, signal_name, receive_timeout)) = recaptured {
                    if let Some(actor) = self.actors.get_mut(&actor_id) {
                        let marker = suspension_marker(actor, signal_name);
                        actor.waiting_signal = marker;
                        actor.suspended_execution =
                            Some(crate::runtime::actor::SuspendedExecution {
                                vm_state,
                                behavior_idx,
                                step_name,
                            });
                    }
                    // A chained receive-after suspend arms its timeout
                    // here; a no-op for the other sentinels.
                    self.maybe_schedule_receive_wait(actor_id, receive_timeout);
                }
            }
            Err(_) => {
                // Step failed after resumption: run saga compensations.
                if self.actor_is_workflow(actor_id) {
                    self.run_saga_compensation(actor_id, behavior_idx);
                }
            }
        }
        // End the VM-execution window only after the match above: the
        // re-capture arm reads the shared VM's frames, which draining
        // deferred wakes would clobber; the compensation arm runs nested
        // bytecode whose own begin/end must stay inside this window. Runs
        // on every path so wakes of other actors are not lost.
        self.vm_exec_end();
        // The suspension resolved (completed or failed): drain any mail
        // that queued up while the step was suspended.
        self.requeue_if_mail_pending(actor_id);
    }

    pub fn send_message(&mut self, target_id: u64, behavior: &str, args: &[Value]) {
        let behavior_id = self.behavior_id_for(target_id, behavior).unwrap_or(0);
        self.send_message_by_id(target_id, behavior_id, args);
    }

    /// Synchronously run a single behavior on an actor and return its result.
    /// Used by the VM's `Ask` opcode when a real runtime is attached.
    pub fn ask_actor_sync(
        &mut self,
        actor_id: u64,
        behavior_id: u16,
        args: &[Value],
    ) -> crate::types::NuResult<Value> {
        // Synchronous asks (pipelines, supervisors, debates, nested `Ask`)
        // always block on LLM calls; only scheduler-driven behaviors
        // suspend. Force suspension off for the whole body.
        let saved_suspend = self.llm.suspend_enabled;
        self.llm.suspend_enabled = false;
        let result = self.ask_actor_sync_inner(actor_id, behavior_id, args);
        self.llm.suspend_enabled = saved_suspend;
        result
    }

    fn ask_actor_sync_inner(
        &mut self,
        actor_id: u64,
        behavior_id: u16,
        args: &[Value],
    ) -> crate::types::NuResult<Value> {
        let behavior_idx = behavior_id as usize;

        // Intercept semantic-memory behaviors generated by compile_agent.  These
        // are bytecode behaviors at compile time, but their semantics are
        // implemented directly by the runtime so they can mutate and read the
        // durable `semantic_memory` JSON field.
        let behavior_name = self.step_name_for(actor_id, behavior_idx);
        if self.actor_is_agent(actor_id) && self.is_semantic_memory_behavior(&behavior_name) {
            self.current_actor = Some(actor_id);
            let result = if behavior_name == "store_fact" {
                let content = args
                    .get(0)
                    .and_then(|v| {
                        self.actors
                            .get(&actor_id)
                            .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                    })
                    .unwrap_or_default();
                self.semantic_memory_store(actor_id, &content)
            } else {
                let query = args
                    .get(0)
                    .and_then(|v| {
                        self.actors
                            .get(&actor_id)
                            .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                    })
                    .unwrap_or_default();
                let top_k = args.get(1).and_then(|v| v.as_int()).unwrap_or(1) as usize;
                self.semantic_memory_recall(actor_id, &query, top_k)
            };
            self.checkpoint_actor(actor_id);
            self.current_actor = None;
            return Ok(result);
        }

        // Intercept procedural-memory behaviors generated by compile_agent.
        if self.actor_is_agent(actor_id) && self.is_procedural_memory_behavior(&behavior_name) {
            self.current_actor = Some(actor_id);
            let result = match behavior_name.as_str() {
                "store_pattern" => {
                    let key = args
                        .get(0)
                        .and_then(|v| {
                            self.actors
                                .get(&actor_id)
                                .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                        })
                        .unwrap_or_default();
                    let input_pattern = args
                        .get(1)
                        .and_then(|v| {
                            self.actors
                                .get(&actor_id)
                                .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                        })
                        .unwrap_or_default();
                    let output_template = args
                        .get(2)
                        .and_then(|v| {
                            self.actors
                                .get(&actor_id)
                                .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                        })
                        .unwrap_or_default();
                    self.procedural_memory_store_pattern(
                        actor_id,
                        &key,
                        &input_pattern,
                        &output_template,
                    )
                }
                "get_pattern" => {
                    let key = args
                        .get(0)
                        .and_then(|v| {
                            self.actors
                                .get(&actor_id)
                                .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                        })
                        .unwrap_or_default();
                    self.procedural_memory_get_pattern(actor_id, &key)
                }
                "add_example" => {
                    let task = args
                        .get(0)
                        .and_then(|v| {
                            self.actors
                                .get(&actor_id)
                                .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                        })
                        .unwrap_or_default();
                    let input = args
                        .get(1)
                        .and_then(|v| {
                            self.actors
                                .get(&actor_id)
                                .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                        })
                        .unwrap_or_default();
                    let output = args
                        .get(2)
                        .and_then(|v| {
                            self.actors
                                .get(&actor_id)
                                .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                        })
                        .unwrap_or_default();
                    self.procedural_memory_add_example(actor_id, &task, &input, &output)
                }
                "get_examples" => {
                    let task = args
                        .get(0)
                        .and_then(|v| {
                            self.actors
                                .get(&actor_id)
                                .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                        })
                        .unwrap_or_default();
                    let query = args
                        .get(1)
                        .and_then(|v| {
                            self.actors
                                .get(&actor_id)
                                .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                        })
                        .unwrap_or_default();
                    let top_k = args.get(2).and_then(|v| v.as_int()).unwrap_or(1) as usize;
                    self.procedural_memory_get_examples(actor_id, &task, &query, top_k)
                }
                _ => crate::vm::Value::nil(),
            };
            self.checkpoint_actor(actor_id);
            self.current_actor = None;
            return Ok(result);
        }

        let is_native = self
            .actors
            .get(&actor_id)
            .and_then(|a| a.behavior_table.get(behavior_idx))
            .map(|e| !e.name.is_empty())
            .unwrap_or(false);
        if is_native {
            let handler =
                self.actors.get(&actor_id).unwrap().behavior_table[behavior_idx].handler_fn;
            self.current_actor = Some(actor_id);
            if self.actor_is_persistent(actor_id) {
                let seq = self.next_sequence(actor_id);
                let payload = args.iter().map(PersistedValue::from_value).collect();
                let _ = self.persistence.append_journal(
                    actor_id,
                    JournalEntry {
                        sequence: seq,
                        behavior_id,
                        payload,
                    },
                );
            }
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                handler(actor, args);
            }
            self.checkpoint_actor(actor_id);
            self.current_actor = None;
            return Ok(Value::nil());
        }
        if self.has_bytecode_handler(actor_id, behavior_idx) {
            let result = self.run_bytecode_behavior(actor_id, behavior_idx, args);
            self.checkpoint_actor(actor_id);
            self.current_actor = None;
            return result;
        }
        self.current_actor = None;
        Ok(Value::nil())
    }

    pub fn behavior_id_for(&self, target_id: u64, behavior: &str) -> Option<u16> {
        let actor = self.actors.get(&target_id)?;
        let suffix = format!(".{}", behavior);
        // Search the per-actor behavior table first (native handlers).
        if let Some(idx) = actor
            .behavior_table
            .iter()
            .position(|entry| entry.name == behavior || entry.name.ends_with(&suffix))
        {
            return Some(idx as u16);
        }
        // Fall back to the module-level behavior table (bytecode handlers).
        // Returns the GLOBAL index into module.behaviors, which matches
        // what bytecode_offsets expects.
        let module = actor.bytecode_module.as_ref()?;
        module
            .behaviors
            .iter()
            .position(|b| b.name == behavior || b.name.ends_with(&suffix))
            .map(|idx| idx as u16)
    }
    pub fn send_message_by_id(&mut self, target_id: u64, behavior_id: u16, args: &[Value]) {
        let msg = Message {
            behavior_id,
            payload: args.to_vec(),
            sender: self.current_actor.unwrap_or(0),
            priority: MessagePriority::Normal,
        };
        let target_exists = self.actors.contains_key(&target_id);
        if target_exists {
            if let Some(actor) = self.actors.get_mut(&target_id) {
                actor
                    .flight_recorder
                    .record(self.current_actor.unwrap_or(0), behavior_id, args);
            }
            if let Err(_dropped) = self.actors.get_mut(&target_id).unwrap().mailbox.push(msg) {
                // Mailbox is full (capacity > 0). Route to DLQ with a simple notification.
                self.route_to_dlq(
                    &Message {
                        behavior_id,
                        payload: args.to_vec(),
                        sender: self.current_actor.unwrap_or(0),
                        priority: MessagePriority::System,
                    },
                    "mailbox full",
                );
            }
        } else {
            self.route_to_dlq(
                &Message {
                    behavior_id,
                    payload: args.to_vec(),
                    sender: self.current_actor.unwrap_or(0),
                    priority: MessagePriority::System,
                },
                "target actor not found",
            );
        }
        for arg in args {
            if let Some(ptr) = arg.as_ptr() {
                if ptr.is_null() {
                    continue;
                }
                if self.current_actor.is_some() {
                    // The true owner is recorded in the object's header: an
                    // actor forwarding a reference it received from a third
                    // actor must not be mistaken for the owner (that tripped
                    // the ownership assert in `send_ref_to` and registered
                    // the cycle-detector edge under the wrong actor).
                    // SAFETY: TAG_PTR values carry ActorHeap payload pointers
                    // with a uniform OrcaHeader layout; the sender holds a
                    // counted reference (a local ref or a receiver hold), so
                    // the heap is live — or retired — and the header valid.
                    let source_header = unsafe { crate::runtime::heap::ActorHeap::header_of(ptr) };
                    let owner_id = unsafe { (*source_header).actor_id };

                    if let Some(owner) = self.actors.get_mut(&owner_id) {
                        let op = unsafe { owner.orca_gc.send_ref_to(&owner.heap, ptr, target_id) };
                        self.coordinator.submit_op(op);
                    } else {
                        // The owner has exited: its heap is retired (kept
                        // alive by the sender's hold), so the header is
                        // still valid.  Bump the in-flight count directly
                        // and queue the decrement op; `process_gc_ops`
                        // applies it on the retired heap.
                        // SAFETY: as above; the single scheduler thread is
                        // the only mutator of any header.
                        unsafe { (*source_header).foreign_count += 1 };
                        self.coordinator.submit_op(ForeignRefOp {
                            target_actor: target_id,
                            owner_actor: owner_id,
                            object_header: source_header,
                            delta: -1,
                        });
                    }
                    // Register the cross-actor reference with the cycle detector.
                    // The receiving actor is represented by its pinned sentinel;
                    // the edge target_sentinel -> source_object records that the
                    // target actor holds a reference to the source object.
                    if self.actors.contains_key(&owner_id) && self.actors.contains_key(&target_id) {
                        if let Some(target_actor) = self.actors.get_mut(&target_id) {
                            if let Some(sentinel) = target_actor.cycle_sentinel() {
                                self.cycle_detector.register_foreign_ref(
                                    target_id,
                                    sentinel,
                                    owner_id,
                                    source_header,
                                );
                            }
                        }
                    }
                }
            }
        }
        self.enqueue_actor(target_id);
        // Wake an actor suspended in a timed selective receive: resume it
        // so the VM re-executes the ReceiveWait scan. A match resolves the
        // wait; otherwise the behavior re-suspends on its original deadline.
        // (An already-fired timeout is resolved by the timer-fire path.)
        let wake_for_receive = self
            .actors
            .get(&target_id)
            .map(|a| {
                a.suspended_execution.is_some()
                    && a.receive_wait.map(|w| !w.timed_out).unwrap_or(false)
            })
            .unwrap_or(false);
        if wake_for_receive {
            if self.vm_execution_depth > 0 {
                // A behavior is mid-flight on the shared runtime VM:
                // resuming the target now would nest a second
                // `vm.resume()` inside the running one and clobber the
                // shared frames. Defer the wake; `vm_exec_end` drains it
                // once the outermost VM call returns.
                if !self.pending_receive_wakes.contains(&target_id) {
                    self.pending_receive_wakes.push(target_id);
                }
            } else {
                self.resume_suspended_receive_wait(target_id);
            }
        }
    }

    pub fn process_gc_ops(&mut self) {
        let ops = std::mem::take(&mut self.coordinator.pending_ops);
        for op in ops {
            // The owning actor is recorded on the op at send time.  Never
            // dereference `object_header` to discover the owner: if the
            // owner has exited its actor entry is gone, and reading the
            // header first would be a use-after-free once its heap drops.
            let source_header = op.object_header;
            let source_actor = op.owner_actor;

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
            }

            // The ORCA operation must be applied on the *owning* actor's heap,
            // because that is where the object (and its header) lives.  Freeing
            // on the target actor's heap would corrupt the wrong allocator.
            if let Some(source_actor_ref) = self.actors.get_mut(&source_actor) {
                source_actor_ref
                    .orca_gc
                    .process_foreign_op(&mut source_actor_ref.heap, op);
            } else {
                // The owner has exited.  Its heap was retired (not freed)
                // precisely because this in-flight op kept the object's
                // foreign_count positive, so the header is still valid and
                // the decrement can be applied directly.  Individual objects
                // are not freed here; the whole retired heap is reclaimed by
                // `reclaim_retired_heaps` once all foreign refs drain.
                // SAFETY: retired heap memory stays mapped until every
                // foreign reference drains; the single scheduler thread is
                // the only mutator of any header.
                unsafe {
                    let header = &mut *source_header;
                    if op.delta >= 0 {
                        header.foreign_count += op.delta as u32;
                    } else {
                        header.foreign_count -= (-op.delta) as u32;
                    }
                }
            }
        }
        self.reclaim_retired_heaps();
        let should_detect = self.cycle_detector.should_detect();
        if should_detect {
            let local_ids: std::collections::HashSet<u64> = self.actors.keys().copied().collect();
            self.cycle_detector.set_local_actors(local_ids);
            // Take the detector out of `self` so it and the runtime are two
            // disjoint &mut borrows; `incremental_detect` only touches
            // `self.actors` via the CycleRuntime impl.
            let mut detector = std::mem::take(&mut self.cycle_detector);
            detector.incremental_detect(self);
            self.cycle_detector = detector;
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
        let mut total = GcStats::default();
        for actor in self.actors.values() {
            let stats = actor.orca_gc.stats();
            total.objects_allocated += stats.objects_allocated;
            total.objects_freed += stats.objects_freed;
            total.local_refs_created += stats.local_refs_created;
            total.local_refs_dropped += stats.local_refs_dropped;
            total.foreign_refs_sent += stats.foreign_refs_sent;
            total.foreign_refs_received += stats.foreign_refs_received;
            total.cycles_detected += stats.cycles_detected;
            total.bytes_allocated += stats.bytes_allocated;
            total.bytes_freed += stats.bytes_freed;
        }
        total
    }
    /// Return the DLQ actor id, creating the DLQ actor if needed.
    /// The DLQ actor is intentionally never scheduled — messages accumulate
    /// in its mailbox for inspection via `dlq_depth()`.
    pub fn ensure_dlq_actor(&mut self) -> u64 {
        if let Some(id) = self.dlq_actor_id {
            if self.actors.contains_key(&id) {
                return id;
            }
        }
        let id = fresh_actor_id();
        let mut actor = Actor::new(id, "__dlq", 0);
        actor.set_state_field("count", Value::int(0));
        self.actors.insert(id, actor);
        self.dlq_actor_id = Some(id);
        id
    }

    /// Route an undeliverable message to the DLQ.
    /// The DLQ actor is never scheduled, so messages accumulate in its mailbox.
    pub fn route_to_dlq(&mut self, _msg: &Message, _reason: &str) {
        let dlq_id = self.ensure_dlq_actor();
        // Push a simple notification to the DLQ's mailbox directly.
        // We don't use send_message_by_id because it would try to ORCA-track args.
        if let Some(actor) = self.actors.get_mut(&dlq_id) {
            let _ = actor.mailbox.push(Message {
                behavior_id: 0,
                payload: vec![Value::int(1)],
                sender: 0,
                priority: MessagePriority::System,
            });
        }
    }

    /// Number of messages currently queued in the DLQ actor's mailbox.
    pub fn dlq_depth(&self) -> usize {
        self.dlq_actor_id
            .and_then(|id| self.actors.get(&id))
            .map(|actor| actor.mailbox.len())
            .unwrap_or(0)
    }

    pub fn run_scheduler(&mut self) {
        // How often (in scheduler ticks) deferred local decrements are
        // retried while actors are still running.
        const GC_PUMP_INTERVAL: u64 = 256;
        let mut ticks: u64 = 0;
        loop {
            let actor_id = match self.scheduler.dequeue() {
                Some(actor_id) => actor_id,
                None => {
                    if self.llm.inflight_count == 0 && self.timer_wheel.is_empty() {
                        if let Some(ref mut cb) = self.idle_callback {
                            cb();
                        }
                        break;
                    }
                    // The run queue is drained but background LLM calls are
                    // still in flight or timers are pending: block briefly
                    // for the next completion or timer deadline so
                    // run_scheduler keeps its "run until quiescent"
                    // semantics — an actor whose last turn armed a timer
                    // must still receive the fired message.
                    let wait = match self.timer_wheel.next_deadline() {
                        Some(deadline) => deadline
                            .saturating_duration_since(self.now())
                            .min(std::time::Duration::from_millis(10)),
                        None => std::time::Duration::from_millis(10),
                    };
                    match self.llm.rx.recv_timeout(wait) {
                        Ok((actor_id, result)) => {
                            self.store_llm_completion(actor_id, result);
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    // Deliver any timers that matured while waiting; fired
                    // messages re-enqueue their target actors, so the next
                    // dequeue resumes work.
                    self.tick_timers();
                    continue;
                }
            };
            self.poll_llm_completions();
            self.tick_timers();
            self.step_actor(actor_id);
            ticks += 1;
            if ticks % GC_PUMP_INTERVAL == 0 {
                // Safe at any cadence: process_deferred only frees objects
                // whose local and foreign counts have already reached zero.
                self.process_deferred_all();
            }
        }
        // Deliver pending foreign-ref decrements and run cycle detection only
        // once the run queue has drained. Receiver-side holds now keep
        // `foreign_count` elevated for as long as a receiving actor holds a
        // pointer, so the -1 ops only release the *in-flight* count; applying
        // them mid-run is still deferred to keep mailbox pointers counted by
        // the in-flight bump until they are received (and held). Note: an
        // actor that yielded with a non-empty mailbox is re-enqueued, so a
        // drained queue implies drained mailboxes for terminating programs.
        self.process_gc_ops();
        self.process_deferred_all();
    }

    /// Retry deferred local decrements on every actor's heap. Objects whose
    /// `foreign_count` has since dropped to zero are freed.
    fn process_deferred_all(&mut self) {
        for actor in self.actors.values_mut() {
            actor.orca_gc.process_deferred(&mut actor.heap);
        }
    }

    // -- ORCA receiver holds & retired heaps --

    /// Take a receiver-side ORCA hold for every heap pointer in a message
    /// payload that `receiver_id` has just popped from its mailbox.
    ///
    /// Each hold increments the owning object's `foreign_count`, so the
    /// object survives until the receiver exits — even if the sender drops
    /// its local references or exits first.  Holds are recorded on the
    /// receiver's `OrcaGc` and released by [`release_held_foreign_refs`].
    fn hold_payload_refs(&mut self, receiver_id: u64, payload: &[Value]) {
        for value in payload {
            let Some(ptr) = value.as_ptr() else { continue };
            if ptr.is_null() {
                continue;
            }
            // SAFETY: TAG_PTR values carry ActorHeap payload pointers with a
            // uniform OrcaHeader layout.  The pointer is valid because the
            // in-flight send bump (or the sender's local ref) keeps the
            // owning heap live — heaps with outstanding foreign refs are
            // retired, never freed.
            let header = unsafe { crate::runtime::heap::ActorHeap::header_of(ptr) };
            let owner_id = unsafe { (*header).actor_id };
            if let Some(owner) = self.actors.get_mut(&owner_id) {
                // SAFETY: `ptr` points to a live object owned by `owner_id`.
                unsafe { owner.orca_gc.inc_foreign_hold(&owner.heap, ptr) };
            } else {
                // The owner has exited: its heap is retired (kept alive by
                // the in-flight send bump), so bump the header directly.
                // SAFETY: as above; single scheduler thread.
                unsafe { (*header).foreign_count += 1 };
            }
            if let Some(receiver) = self.actors.get_mut(&receiver_id) {
                receiver.orca_gc.record_held_ref(owner_id, header);
            }
        }
    }

    /// Release every receiver-side foreign hold taken by `actor_id`.
    ///
    /// Called when the actor exits.  For a live owner the release goes
    /// through the owner's `OrcaGc` (which may free the object); for an
    /// exited owner the decrement is applied directly against its retired
    /// heap.  Idempotent: the hold list is drained on the first call.
    pub(crate) fn release_held_foreign_refs(&mut self, actor_id: u64) {
        let holds = match self.actors.get_mut(&actor_id) {
            Some(actor) => actor.orca_gc.take_held_refs(),
            None => return,
        };
        for (owner_id, header) in holds {
            if let Some(owner) = self.actors.get_mut(&owner_id) {
                owner.orca_gc.process_foreign_op(
                    &mut owner.heap,
                    ForeignRefOp {
                        target_actor: actor_id,
                        owner_actor: owner_id,
                        object_header: header,
                        delta: -1,
                    },
                );
            } else {
                // SAFETY: the hold kept foreign_count > 0, so the owner's
                // heap was retired (not freed) and the header is valid.
                unsafe { (*header).foreign_count -= 1 };
            }
        }
        self.reclaim_retired_heaps();
    }

    /// True if any live object on `heap` still has foreign references.
    fn heap_has_outstanding_foreign_refs(heap: &ActorHeap) -> bool {
        let mut outstanding = false;
        heap.iter_live_objects(|header, _, _| {
            // SAFETY: iter_live_objects yields live headers on the scheduler
            // thread; no mutation happens during the scan.
            if unsafe { (*header).foreign_count } > 0 {
                outstanding = true;
            }
        });
        outstanding
    }

    /// Remove an actor from the runtime, releasing its receiver holds and
    /// deferring heap destruction while other actors still reference its
    /// objects.  A heap with outstanding foreign refs is moved into
    /// `retired_heaps` instead of being dropped, so in-flight ops and
    /// receiver holds held elsewhere never dangle.
    pub(crate) fn remove_actor_reaping(&mut self, actor_id: u64) {
        self.release_held_foreign_refs(actor_id);
        if let Some(mut actor) = self.actors.remove(&actor_id) {
            if Self::heap_has_outstanding_foreign_refs(&actor.heap) {
                // Swap in a fresh empty heap so `actor` drops cleanly; the
                // real heap moves to the retired list.
                let heap = std::mem::replace(&mut actor.heap, ActorHeap::new(64));
                self.retired_heaps.push(heap);
            }
        }
    }

    /// Drop retired heaps whose foreign references have all drained.
    ///
    /// Every foreign-count mutation on a retired heap goes through the
    /// runtime (direct bumps/decrements in `send_message_by_id`,
    /// `hold_payload_refs`, `release_held_foreign_refs`, and
    /// `process_gc_ops`), so scanning at those points is exact.
    fn reclaim_retired_heaps(&mut self) {
        if self.retired_heaps.is_empty() {
            return;
        }
        self.retired_heaps
            .retain(|heap| Self::heap_has_outstanding_foreign_refs(heap));
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
                    // A behavior suspended on a signal wait or a background
                    // LLM call owns the actor until it resumes: leave queued
                    // messages in the mailbox instead of running them over
                    // the suspension.  A second suspending behavior would
                    // overwrite `suspended_execution`, hijack the first
                    // call's completion (a single `llm_completed` slot), and
                    // lose the first behavior forever.  The resume paths
                    // re-enqueue the actor once the suspension resolves.
                    if actor.suspended_execution.is_some() {
                        None
                    } else {
                        actor.receive()
                    }
                }
                _ => {
                    self.current_actor = None;
                    return;
                }
            }
        };
        let should_requeue = if let Some(msg) = msg_opt {
            let behavior_idx = msg.behavior_id as usize;

            // ORCA receiver protocol: hold every heap pointer in the
            // received payload so the owning objects (and any retired
            // owner heap) stay alive until this actor exits.
            self.hold_payload_refs(actor_id, &msg.payload);

            // Intercept semantic-memory behaviors generated by compile_agent.
            // They are bytecode behaviors but are implemented directly by the
            // runtime against the durable `semantic_memory` state field.
            let behavior_name = self.step_name_for(actor_id, behavior_idx);
            if self.actor_is_agent(actor_id) && self.is_semantic_memory_behavior(&behavior_name) {
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
                let content = msg
                    .payload
                    .get(0)
                    .and_then(|v| {
                        self.actors
                            .get(&actor_id)
                            .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                    })
                    .unwrap_or_default();
                if behavior_name == "store_fact" {
                    self.semantic_memory_store(actor_id, &content);
                } else {
                    let query = content;
                    let top_k = msg.payload.get(1).and_then(|v| v.as_int()).unwrap_or(1) as usize;
                    self.semantic_memory_recall(actor_id, &query, top_k);
                }
                self.checkpoint_actor(actor_id);
                self.current_actor = None;
                return;
            }

            // Intercept procedural-memory behaviors generated by compile_agent.
            if self.actor_is_agent(actor_id) && self.is_procedural_memory_behavior(&behavior_name) {
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
                match behavior_name.as_str() {
                    "store_pattern" => {
                        let key = msg
                            .payload
                            .get(0)
                            .and_then(|v| {
                                self.actors
                                    .get(&actor_id)
                                    .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                            })
                            .unwrap_or_default();
                        let input_pattern = msg
                            .payload
                            .get(1)
                            .and_then(|v| {
                                self.actors
                                    .get(&actor_id)
                                    .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                            })
                            .unwrap_or_default();
                        let output_template = msg
                            .payload
                            .get(2)
                            .and_then(|v| {
                                self.actors
                                    .get(&actor_id)
                                    .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                            })
                            .unwrap_or_default();
                        self.procedural_memory_store_pattern(
                            actor_id,
                            &key,
                            &input_pattern,
                            &output_template,
                        );
                    }
                    "get_pattern" => {
                        let key = msg
                            .payload
                            .get(0)
                            .and_then(|v| {
                                self.actors
                                    .get(&actor_id)
                                    .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                            })
                            .unwrap_or_default();
                        self.procedural_memory_get_pattern(actor_id, &key);
                    }
                    "add_example" => {
                        let task = msg
                            .payload
                            .get(0)
                            .and_then(|v| {
                                self.actors
                                    .get(&actor_id)
                                    .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                            })
                            .unwrap_or_default();
                        let input = msg
                            .payload
                            .get(1)
                            .and_then(|v| {
                                self.actors
                                    .get(&actor_id)
                                    .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                            })
                            .unwrap_or_default();
                        let output = msg
                            .payload
                            .get(2)
                            .and_then(|v| {
                                self.actors
                                    .get(&actor_id)
                                    .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                            })
                            .unwrap_or_default();
                        self.procedural_memory_add_example(actor_id, &task, &input, &output);
                    }
                    "get_examples" => {
                        let task = msg
                            .payload
                            .get(0)
                            .and_then(|v| {
                                self.actors
                                    .get(&actor_id)
                                    .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                            })
                            .unwrap_or_default();
                        let query = msg
                            .payload
                            .get(1)
                            .and_then(|v| {
                                self.actors
                                    .get(&actor_id)
                                    .and_then(|actor| self.vm_value_to_string_in_actor(v, actor))
                            })
                            .unwrap_or_default();
                        let top_k =
                            msg.payload.get(2).and_then(|v| v.as_int()).unwrap_or(1) as usize;
                        self.procedural_memory_get_examples(actor_id, &task, &query, top_k);
                    }
                    _ => {}
                }
                self.checkpoint_actor(actor_id);
                self.current_actor = None;
                return;
            }

            // Backend dispatch: WASM component actors are handled by the
            // component runtime, not the native VM.
            {
                let actor = match self.actors.get(&actor_id) {
                    Some(a) => a,
                    None => {
                        self.current_actor = None;
                        return;
                    }
                };
                if let crate::runtime::actor::ActorBackend::WasmComponent { .. } = &actor.backend {
                    self.current_actor = None;
                    return; // stub: WASM component runtime not yet integrated
                }
            }

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
                // Enable non-blocking LLM suspension for this
                // scheduler-driven behavior invocation. Nested synchronous
                // entry points (ask_actor_sync) force it back off.
                let saved_suspend = self.llm.suspend_enabled;
                self.llm.suspend_enabled = true;
                let result = self.run_bytecode_behavior(actor_id, behavior_idx, &payload);
                self.llm.suspend_enabled = saved_suspend;
                match result {
                    Ok(_) => {
                        self.checkpoint_actor(actor_id);
                        processed = true;
                    }
                    Err(crate::types::NuError::Suspended(_)) => {
                        // The step yielded waiting for a signal or a
                        // background LLM call. Do not mark it completed, do
                        // not run compensations, and do not checkpoint the
                        // partially-mutated durable state: persist only the
                        // suspension marker so recovery can re-drive the
                        // step from its last pre-suspend checkpoint.
                        self.persist_suspension_marker(actor_id);
                        processed = false;
                    }
                    Err(_) => {
                        self.checkpoint_actor(actor_id);
                        // A workflow step failed: run saga compensations for previously
                        // completed steps in reverse order.
                        if self.actor_is_workflow(actor_id) {
                            self.run_saga_compensation(actor_id, behavior_idx);
                        }
                        processed = false;
                    }
                }
            }
            if processed
                && self.actor_is_workflow(actor_id)
                && !self.is_internal_behavior(actor_id, behavior_idx)
            {
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
                        if let Some(n) =
                            actor.get_state_field("step_index").and_then(|v| v.as_int())
                        {
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
            actor.increment_reductions(1);
            // Flush the selective-receive skip-buffer back to the normal
            // queue so the next turn starts clean and is_empty() correctly
            // reflects pending messages.
            actor.mailbox.flush_skip_buffer();
            if actor.mailbox.is_empty() {
                // Turn over: next scheduling starts with a fresh budget.
                actor.reset_reductions();
                false
            } else if actor.should_yield() {
                // Reduction budget exhausted with mail pending: yield —
                // reset the counter and requeue at the back of the
                // scheduler queue so other actors get a turn first.
                actor.reset_reductions();
                true
            } else {
                true
            }
        } else {
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                if actor.state == ActorState::Running {
                    actor.state = ActorState::Waiting;
                }
                // Waiting actors start their next turn with a fresh budget.
                actor.reset_reductions();
            }
            false
        };
        if should_requeue {
            self.enqueue_actor(actor_id);
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
        workflow::actor_is_workflow(self, actor_id)
    }

    fn actor_is_agent(&self, actor_id: u64) -> bool {
        self.actors
            .get(&actor_id)
            .map(|a| a.is_agent)
            .unwrap_or(false)
    }

    /// Return true if the behavior name is a semantic-memory behavior generated
    /// by `compile_agent` for agents configured with `semantic_memory`.
    fn is_semantic_memory_behavior(&self, name: &str) -> bool {
        name == "store_fact" || name == "recall"
    }

    /// Read an agent's durable `semantic_memory` state field as a `SemanticMemory`.
    fn read_semantic_memory(&self, actor: &Actor) -> Option<crate::ai::SemanticMemory> {
        let value = actor.get_state_field("semantic_memory")?;
        let ptr = value.as_ptr()?;
        if ptr.is_null() {
            return None;
        }
        let json = unsafe {
            std::ffi::CStr::from_ptr(ptr as *const std::ffi::c_char)
                .to_string_lossy()
                .into_owned()
        };
        serde_json::from_str(&json).ok()
    }

    /// Write a `SemanticMemory` back to an agent's durable `semantic_memory` state field.
    fn write_semantic_memory(actor: &mut Actor, memory: &crate::ai::SemanticMemory) {
        if let Ok(json) = serde_json::to_string(memory) {
            let ptr = actor.allocate_string(&json);
            actor.set_state_field("semantic_memory", ptr);
        }
    }

    /// Convert a VM value into a Rust string, reading pointer payloads as
    /// null-terminated UTF-8 and string-id values via the actor's bytecode module.
    fn vm_value_to_string_in_actor(
        &self,
        value: &crate::vm::Value,
        actor: &Actor,
    ) -> Option<String> {
        if let Some(id) = value.as_string_id() {
            actor
                .bytecode_module
                .as_ref()
                .and_then(|m| m.constants.get(id as usize))
                .and_then(|c| match c {
                    crate::bytecode::Constant::String(s) => Some(s.clone()),
                    _ => None,
                })
        } else if let Some(ptr) = value.as_ptr() {
            if ptr.is_null() {
                Some(String::new())
            } else {
                Some(unsafe {
                    std::ffi::CStr::from_ptr(ptr as *const std::ffi::c_char)
                        .to_string_lossy()
                        .into_owned()
                })
            }
        } else {
            None
        }
    }

    /// Store a fact in an agent's semantic memory and return the document id.
    fn semantic_memory_store(&mut self, actor_id: u64, content: &str) -> crate::vm::Value {
        self.semantic_memory_store_with_metadata(
            actor_id,
            content,
            std::collections::HashMap::new(),
        )
    }

    /// Store a fact with metadata in an agent's semantic memory and return the document id.
    fn semantic_memory_store_with_metadata(
        &mut self,
        actor_id: u64,
        content: &str,
        metadata: std::collections::HashMap<String, String>,
    ) -> crate::vm::Value {
        let memory_opt = if let Some(actor) = self.actors.get(&actor_id) {
            self.read_semantic_memory(actor)
        } else {
            None
        };
        let mut memory = memory_opt.unwrap_or_else(|| crate::ai::SemanticMemory::new(64, None));
        let id = memory.store(content, metadata);
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            Self::write_semantic_memory(actor, &memory);
            return actor.allocate_string(&id);
        }
        crate::vm::Value::nil()
    }

    /// Search an agent's semantic memory and return the top result's content.
    fn semantic_memory_recall(
        &mut self,
        actor_id: u64,
        query: &str,
        top_k: usize,
    ) -> crate::vm::Value {
        let content = if let Some(actor) = self.actors.get(&actor_id) {
            self.read_semantic_memory(actor).and_then(|memory| {
                let results = memory.search(query, top_k);
                results.first().map(|(doc, _)| doc.content.clone())
            })
        } else {
            None
        };
        if let Some(content) = content {
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                return actor.allocate_string(&content);
            }
        }
        crate::vm::Value::nil()
    }

    // -------------------------------------------------------------------------
    // Procedural memory helpers
    // -------------------------------------------------------------------------

    /// Return true if the behavior name is a procedural-memory behavior generated
    /// by `compile_agent` for agents configured with `procedural_memory`.
    fn is_procedural_memory_behavior(&self, name: &str) -> bool {
        matches!(
            name,
            "store_pattern" | "get_pattern" | "add_example" | "get_examples"
        )
    }

    /// Read an agent's durable `procedural_memory` state field as a `ProceduralMemory`.
    fn read_procedural_memory(&self, actor: &Actor) -> Option<crate::ai::ProceduralMemory> {
        let value = actor.get_state_field("procedural_memory")?;
        let ptr = value.as_ptr()?;
        if ptr.is_null() {
            return None;
        }
        let json = unsafe {
            std::ffi::CStr::from_ptr(ptr as *const std::ffi::c_char)
                .to_string_lossy()
                .into_owned()
        };
        serde_json::from_str(&json).ok()
    }

    /// Write a `ProceduralMemory` back to an agent's durable `procedural_memory` state field.
    fn write_procedural_memory(actor: &mut Actor, memory: &crate::ai::ProceduralMemory) {
        if let Ok(json) = serde_json::to_string(memory) {
            let ptr = actor.allocate_string(&json);
            actor.set_state_field("procedural_memory", ptr);
        }
    }

    /// Store a pattern in an agent's procedural memory and return the key.
    fn procedural_memory_store_pattern(
        &mut self,
        actor_id: u64,
        key: &str,
        input_pattern: &str,
        output_template: &str,
    ) -> crate::vm::Value {
        let memory_opt = self
            .actors
            .get(&actor_id)
            .and_then(|actor| self.read_procedural_memory(actor));
        let mut memory = memory_opt.unwrap_or_else(|| crate::ai::ProceduralMemory::new("default"));
        let key = memory.store_pattern(key, input_pattern, output_template);
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            Self::write_procedural_memory(actor, &memory);
            return actor.allocate_string(&key);
        }
        crate::vm::Value::nil()
    }

    /// Retrieve a pattern by key from an agent's procedural memory.
    fn procedural_memory_get_pattern(&mut self, actor_id: u64, key: &str) -> crate::vm::Value {
        let content = self
            .actors
            .get(&actor_id)
            .and_then(|actor| self.read_procedural_memory(actor))
            .and_then(|memory| memory.get_pattern(key).map(|p| p.output_template.clone()));
        if let Some(content) = content {
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                return actor.allocate_string(&content);
            }
        }
        crate::vm::Value::nil()
    }

    /// Add a few-shot example to an agent's procedural memory.
    fn procedural_memory_add_example(
        &mut self,
        actor_id: u64,
        task: &str,
        input: &str,
        output: &str,
    ) -> crate::vm::Value {
        let memory_opt = self
            .actors
            .get(&actor_id)
            .and_then(|actor| self.read_procedural_memory(actor));
        let mut memory = memory_opt.unwrap_or_else(|| crate::ai::ProceduralMemory::new("default"));
        memory.add_example(task, input, output);
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            Self::write_procedural_memory(actor, &memory);
        }
        crate::vm::Value::nil()
    }

    /// Retrieve the top-k examples for a task/query from an agent's procedural memory.
    fn procedural_memory_get_examples(
        &mut self,
        actor_id: u64,
        task: &str,
        query: &str,
        top_k: usize,
    ) -> crate::vm::Value {
        let examples = self
            .actors
            .get(&actor_id)
            .and_then(|actor| self.read_procedural_memory(actor))
            .map(|memory| memory.get_examples(task, query, top_k));
        if let Some(examples) = examples {
            let formatted = examples
                .iter()
                .map(|example| format!("IN: {}\nOUT: {}", example.input, example.output))
                .collect::<Vec<_>>()
                .join("\n---\n");
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                return actor.allocate_string(&formatted);
            }
        }
        crate::vm::Value::nil()
    }

    fn has_bytecode_handler(&self, actor_id: u64, behavior_idx: usize) -> bool {
        self.actors
            .get(&actor_id)
            .map(|a| a.bytecode_module.is_some() && behavior_idx < a.bytecode_offsets.len())
            .unwrap_or(false)
    }

    fn next_sequence(&self, actor_id: u64) -> u64 {
        workflow::next_sequence(self, actor_id)
    }

    /// Schedule a durable timer for a workflow actor.
    ///
    /// Appends a `TimerSet` event, checkpoints state, and arms the runtime's
    /// timer wheel. When the timer fires the runtime will append a
    /// `TimerFired` event and deliver a `__timer_fired` message to the actor.
    pub fn schedule_workflow_timer(&mut self, actor_id: u64, name: &str, duration_ms: u64) {
        workflow::schedule_workflow_timer(self, actor_id, name, duration_ms)
    }

    /// Re-arm a timer from the durable journal without appending a new event.
    /// Used during recovery to restore timers that have not yet fired.
    pub(crate) fn rearm_timer(&mut self, actor_id: u64, name: &str, duration_ms: u64) {
        let behavior_id = self.behavior_id_for(actor_id, "__timer_fired").unwrap_or(0);
        self.timer_wheel.send_after_with_context(
            std::time::Duration::from_millis(duration_ms),
            actor_id,
            behavior_id,
            vec![],
            name.to_string(),
        );
    }

    /// Return the current logical time: the virtual clock's view if one is
    /// installed, otherwise real wall-clock time.
    pub fn now(&self) -> std::time::Instant {
        match &self.virtual_clock {
            Some(vc) => vc.now(),
            None => std::time::Instant::now(),
        }
    }

    /// Install a virtual clock, freezing time at the current wall-clock
    /// moment. All subsequent timer expiry and deadline calculations use
    /// this clock. Call `advance_time` to move time forward.
    pub fn install_virtual_clock(&mut self) {
        self.virtual_clock = Some(VirtualClock::new());
    }

    /// Advance the virtual clock by `duration`. Timers whose fire time lies
    /// at or before the new virtual time will fire on the next scheduler
    /// iteration. Panics if no virtual clock is installed.
    pub fn advance_time(&mut self, duration: std::time::Duration) {
        match &mut self.virtual_clock {
            Some(vc) => vc.advance(duration),
            None => panic!("advance_time called without a virtual clock installed"),
        }
    }

    /// Remove the virtual clock, returning to real wall-clock time.
    pub fn remove_virtual_clock(&mut self) {
        self.virtual_clock = None;
    }

    /// Tick the timer wheel and deliver any fired timers.
    pub fn tick_timers(&mut self) {
        self.tick_timers_at(self.now());
    }

    // -- Timed selective receive (receive-after) --

    /// Arm the timeout for an actor's first receive-wait suspension.
    ///
    /// Called at every suspend-capture site with the timeout the VM staged
    /// in `suspended_receive_timeout`. A re-suspension of the SAME wait
    /// (a wake found no matching message) must not restart the clock, so
    /// the timer is scheduled only when the actor has no live receive-wait
    /// state; the original deadline stands.
    fn maybe_schedule_receive_wait(&mut self, actor_id: u64, timeout_ms: Option<i64>) {
        let Some(ms) = timeout_ms else { return };
        if ms <= 0 {
            return;
        }
        let already_waiting = self
            .actors
            .get(&actor_id)
            .map(|a| a.receive_wait.is_some())
            .unwrap_or(false);
        if already_waiting {
            return;
        }
        let timer_id = self
            .timer_wheel
            .receive_wait_timeout(std::time::Duration::from_millis(ms as u64), actor_id);
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            actor.receive_wait = Some(crate::runtime::actor::ReceiveWaitState {
                timer_id,
                timed_out: false,
            });
        }
    }

    /// Drop an actor's receive-wait state once the wait has resolved,
    /// cancelling the timeout timer if it is still pending. Called on every
    /// terminal outcome of a resumed receive-suspended behavior (the match
    /// path cancels earlier, via `receive_wait_matched`).
    fn clear_receive_wait(&mut self, actor_id: u64) {
        let wait = self
            .actors
            .get_mut(&actor_id)
            .and_then(|a| a.receive_wait.take());
        if let Some(wait) = wait {
            self.timer_wheel.cancel(wait.timer_id);
        }
    }

    /// A receive-wait timeout timer fired: mark the actor's wait as timed
    /// out and resume its suspended behavior. The re-executed `ReceiveWait`
    /// consumes the marker, writes the no-match sentinel, and continues
    /// into the after body.
    fn fire_receive_wait_timeout(&mut self, actor_id: u64) {
        let has_suspension = self
            .actors
            .get(&actor_id)
            .map(|a| a.suspended_execution.is_some())
            .unwrap_or(false);
        if !has_suspension {
            // Nothing to wake (actor exited or the wait already resolved):
            // drop any stale wait state instead of poisoning a later wait.
            self.clear_receive_wait(actor_id);
            return;
        }
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            if let Some(wait) = actor.receive_wait.as_mut() {
                wait.timed_out = true;
            }
        }
        self.resume_suspended_receive_wait(actor_id);
    }

    /// Resume an actor whose bytecode behavior suspended on a timed
    /// selective receive (`receive ... after ms =>`). Called when a message
    /// was pushed to the actor's mailbox (the re-scan may match) or when
    /// the wait's timer fired (the wait resolves with the no-match
    /// sentinel). Mirrors `resume_suspended_llm_step`: the actor's
    /// callbacks are re-installed on the shared VM before `vm.resume()`.
    fn resume_suspended_receive_wait(&mut self, actor_id: u64) {
        let suspended = match self.actors.get_mut(&actor_id) {
            Some(actor) => actor.suspended_execution.take(),
            None => return,
        };
        let Some(suspended) = suspended else { return };

        if self.vm.is_none() {
            // No VM available; put the suspension back so a later wake can
            // re-trigger it.
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                actor.suspended_execution = Some(suspended);
            }
            return;
        }

        let self_ptr: *mut Runtime = self;
        unsafe {
            let vm = (*self_ptr).vm.as_mut().unwrap();
            // Re-install callbacks bound to THIS actor: other actors may have
            // run on the shared VM while this one was suspended.
            vm.set_distributed_callbacks(Box::new(BytecodeDistributedCallbacks {
                runtime: self_ptr,
            }));
            vm.set_actor_callbacks(Box::new(BytecodeRuntimeCallbacks::new(self_ptr, actor_id)));
            vm.restore_suspended_state(suspended.vm_state);
            // A resumed behavior is still scheduler-context execution: a
            // `perform LLM.ask` after the wait must suspend (non-blocking).
            let saved_suspend = (*self_ptr).llm.suspend_enabled;
            (*self_ptr).llm.suspend_enabled = true;
            (*self_ptr).vm_exec_begin();
            let result = vm.resume();
            (*self_ptr).llm.suspend_enabled = saved_suspend;
            match result {
                Ok(_) => {
                    // The wait resolved (match or timeout) and the behavior
                    // ran to completion: drop any leftover wait state and
                    // record workflow completion like the LLM resume path.
                    (*self_ptr).clear_receive_wait(actor_id);
                    if (*self_ptr).actor_is_workflow(actor_id) {
                        if let Some(actor) = (*self_ptr).actors.get_mut(&actor_id) {
                            actor.waiting_signal = None;
                            if let Some(n) =
                                actor.get_state_field("step_index").and_then(|v| v.as_int())
                            {
                                actor.set_state_field("step_index", Value::int(n + 1));
                            }
                        }
                        let seq = (*self_ptr).next_sequence(actor_id);
                        let _ = (*self_ptr).persistence.append_workflow_event(
                            actor_id,
                            WorkflowEvent::StepCompleted {
                                sequence: seq,
                                step_name: suspended.step_name,
                            },
                        );
                        (*self_ptr).checkpoint_actor(actor_id);
                    }
                }
                Err(crate::types::NuError::Suspended(VmSuspension::ReceiveWait)) => {
                    // Re-suspended on the same wait (the waking message did
                    // not match): keep the original timer and re-capture the
                    // VM state so the next message or the timeout can resume
                    // it. maybe_schedule_receive_wait is a no-op while the
                    // wait state is live, so the deadline is not restarted.
                    if let Some(vm_state) = vm.take_suspended_state() {
                        let timeout = vm.suspended_receive_timeout.take();
                        if let Some(actor) = (*self_ptr).actors.get_mut(&actor_id) {
                            actor.suspended_execution =
                                Some(crate::runtime::actor::SuspendedExecution {
                                    vm_state,
                                    behavior_idx: suspended.behavior_idx,
                                    step_name: suspended.step_name,
                                });
                        }
                        (*self_ptr).maybe_schedule_receive_wait(actor_id, timeout);
                    }
                }
                Err(crate::types::NuError::Suspended(_)) => {
                    // Suspended on something else (a signal wait or a
                    // background LLM call) past the receive: the wait is
                    // over. Re-capture so the matching signal or pumped
                    // completion can resume the behavior.
                    (*self_ptr).clear_receive_wait(actor_id);
                    if let Some(vm_state) = vm.take_suspended_state() {
                        let signal_name = vm.suspended_signal_name.take();
                        if let Some(actor) = (*self_ptr).actors.get_mut(&actor_id) {
                            let marker = suspension_marker(actor, signal_name);
                            actor.waiting_signal = marker;
                            actor.suspended_execution =
                                Some(crate::runtime::actor::SuspendedExecution {
                                    vm_state,
                                    behavior_idx: suspended.behavior_idx,
                                    step_name: suspended.step_name,
                                });
                        }
                    }
                }
                // Other errors: the wait is over; the send-path result is
                // discarded anyway, matching step_actor semantics.
                Err(_) => (*self_ptr).clear_receive_wait(actor_id),
            }
            // End the VM-execution window only after any suspend-state
            // re-capture above: draining deferred wakes runs other actors
            // on the shared VM, which would clobber the frames an
            // un-captured suspend still needs. Runs on every path, so
            // wakes of other actors are not lost when THIS one suspends.
            (*self_ptr).vm_exec_end();
        }
        // The suspension resolved (completed or failed): if messages queued
        // up while the behavior was suspended, schedule the actor to drain
        // them — step_actor leaves mail untouched while a suspension is live.
        self.requeue_if_mail_pending(actor_id);
    }

    fn tick_timers_at(&mut self, now: std::time::Instant) {
        let fired = self.timer_wheel.tick(now);
        for (target_actor, message) in fired {
            match message {
                TimerMessage::SendWithContext {
                    behavior_id,
                    payload,
                    context,
                } => {
                    if self.actor_is_workflow(target_actor) {
                        let _ = self.append_timer_fired(target_actor, &context);
                    }
                    self.send_message_by_id(target_actor, behavior_id, &payload);
                }
                TimerMessage::Send {
                    behavior_id,
                    payload,
                } => {
                    self.send_message_by_id(target_actor, behavior_id, &payload);
                }
                TimerMessage::Exit { reason } => {
                    self.exit_actor(target_actor, ExitReason::Error(reason));
                }
                TimerMessage::Kill => {
                    self.kill_actor(target_actor);
                }
                TimerMessage::ReceiveWaitTimeout => {
                    self.fire_receive_wait_timeout(target_actor);
                }
                TimerMessage::LlmRetry => {
                    self.handle_llm_retry_timer(target_actor);
                }
            }
        }
    }

    /// Snapshot durable fields of an actor to the persistence store.
    /// The snapshot is skipped entirely when no fields have changed since
    /// the last checkpoint (dirty-bit optimization).
    pub fn checkpoint_actor(&mut self, actor_id: u64) {
        workflow::checkpoint_actor(self, actor_id)
    }

    /// Persist only the suspension marker of a persistent actor whose
    /// bytecode behavior has just suspended (signal wait or background LLM
    /// call), without snapshotting the step's partially-mutated durable
    /// state.  Recovery reads the marker (`waiting_signal`, or the
    /// `LLM_SUSPEND_MARKER` sentinel for LLM suspends) to decide that the
    /// in-flight step must be re-driven; the state it re-runs from is the
    /// last pre-step checkpoint.  A no-op when the actor has no snapshot
    /// yet — without one there is nothing to recover anyway.
    fn persist_suspension_marker(&mut self, actor_id: u64) {
        let waiting_signal = match self.actors.get(&actor_id) {
            Some(actor) if actor.persistent => actor.waiting_signal.clone(),
            _ => return,
        };
        if let Some(mut snapshot) = self.persistence.load_snapshot(actor_id) {
            if snapshot.waiting_signal == waiting_signal {
                return;
            }
            snapshot.waiting_signal = waiting_signal;
            let _ = self.persistence.save_snapshot(snapshot);
        }
    }

    /// Lay out a workflow actor's native behavior table so that bytecode step
    /// ids (0..n-1) do not collide with internal runtime behaviors such as
    /// `__timer_fired`.
    pub(crate) fn layout_workflow_behavior_table(&mut self, actor_id: u64) {
        spawn::layout_workflow_behavior_table(self, actor_id)
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
            actor
                .bytecode_offsets
                .get(behavior_idx)
                .copied()
                .unwrap_or(0)
        };
        let result = self.run_bytecode_at_offset(actor_id, code_offset, args);
        // If the step suspended waiting for a signal or a background LLM
        // call, record which behavior and step name it was executing so
        // recovery/resumption can continue.
        if let Err(crate::types::NuError::Suspended(_)) = result {
            let step_name = self.step_name_for(actor_id, behavior_idx);
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                if let Some(ref mut suspended) = actor.suspended_execution {
                    suspended.behavior_idx = behavior_idx;
                    suspended.step_name = step_name;
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
            match actor
                .compensation_offsets
                .get(behavior_idx)
                .copied()
                .flatten()
            {
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

            let module_idx = if let Some(idx) = (*self_ptr)
                .actors
                .get(&actor_id)
                .unwrap()
                .bytecode_module_idx
            {
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
            vm.set_distributed_callbacks(Box::new(BytecodeDistributedCallbacks {
                runtime: self_ptr,
            }));

            let mut frame = crate::vm::Frame::new(None, module_idx);
            frame.pc = code_offset;
            for (i, arg) in args.iter().enumerate().take(256) {
                frame.regs[i] = *arg;
            }
            vm.set_current_frame(frame);

            (*self_ptr).vm_exec_begin();
            let result = vm.run_from(module_idx, code_offset);
            // Capture VM state for a workflow signal wait, a non-blocking
            // LLM call, or a timed selective receive. Doing this here avoids
            // aliasing the Runtime through the callback while the VM borrow
            // is active.
            if let Err(crate::types::NuError::Suspended(_)) = result {
                if let Some(vm_state) = vm.take_suspended_state() {
                    let signal_name = vm.suspended_signal_name.take();
                    let receive_timeout = vm.suspended_receive_timeout.take();
                    if let Some(actor) = self.actors.get_mut(&actor_id) {
                        let marker = suspension_marker(actor, signal_name);
                        actor.waiting_signal = marker;
                        actor.suspended_execution =
                            Some(crate::runtime::actor::SuspendedExecution {
                                vm_state,
                                behavior_idx: 0,
                                step_name: String::new(),
                            });
                    }
                    // Arm the receive-after timeout on the first
                    // suspension; a no-op for the other sentinels.
                    self.maybe_schedule_receive_wait(actor_id, receive_timeout);
                }
            }
            // End the VM-execution window only after the suspend-state
            // capture above: draining deferred wakes runs other actors on
            // the shared VM, which would clobber the frames an un-captured
            // suspend still needs. Runs on every path, so wakes of other
            // actors are not lost when THIS actor suspends.
            (*self_ptr).vm_exec_end();
            // String-id values index into this runtime VM's constant pool. When
            // the result is returned to a different VM (e.g. the top-level VM
            // that invoked `ask`), the id is meaningless there. Convert string
            // results to heap-allocated pointers so they remain valid.
            match result {
                Ok(value) => {
                    if let Some(id) = value.as_string_id() {
                        if let Some(s) = vm.constant_string(module_idx, id) {
                            Ok(vm.allocate_string(&s))
                        } else {
                            Ok(value)
                        }
                    } else {
                        Ok(value)
                    }
                }
                Err(e) => Err(e),
            }
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
        let is_agent = self
            .recovery_modules
            .get(&actor_id)
            .map(|(m, _, _)| m.actor_metadata.iter().any(|meta| meta.is_agent))
            .unwrap_or(false);

        let mut actor = Actor::new(actor_id, format!("actor_{}", actor_id), 0);
        actor.persistent = true;
        actor.is_workflow = is_workflow;
        actor.is_agent = is_agent;
        actor.sequence = snapshot.sequence;
        actor.waiting_signal = snapshot.waiting_signal;
        for (name, value) in snapshot.state {
            // Rehydrate the semantic_memory and procedural_memory JSON strings
            // by allocating them on the actor heap so runtime helpers can read
            // them as pointer values.
            if name == "semantic_memory" || name == "procedural_memory" {
                if let PersistedValue::String(json) = &value {
                    let ptr = actor.allocate_string(json);
                    actor.set_state_field(name, ptr);
                    continue;
                }
            }
            actor.set_state_field(name, value.to_value());
        }
        // Parse cached retry/fallback configs from restored state for agents.
        if is_agent {
            if let Some(module) = actor
                .bytecode_module
                .as_ref()
                .or_else(|| self.recovery_modules.get(&actor_id).map(|(m, _, _)| m))
            {
                for (name, c) in module.actor_metadata.iter().flat_map(|m| &m.state_defaults) {
                    if let crate::bytecode::Constant::String(json) = c {
                        if name == "retry_config" {
                            actor.retry_config = serde_json::from_str(&json).ok();
                        } else if name == "fallback_config" {
                            actor.fallback_config = serde_json::from_str(&json).unwrap_or_default();
                        }
                    }
                }
            }
        }
        // Replay event-sourced events to reconstruct EventSourced fields.
        let events = self.persistence.read_events(actor_id);
        if !events.is_empty() {
            for entry in &events {
                if let Some(current) = actor
                    .get_state_field(&entry.field_name)
                    .and_then(|v| v.as_int())
                {
                    actor.set_state_field(&entry.field_name, Value::int(current + 1));
                } else {
                    actor.set_state_field(&entry.field_name, Value::int(1));
                }
                let current_seq = actor
                    .event_sourced_sequences
                    .get(&entry.field_name)
                    .copied()
                    .unwrap_or(0);
                if entry.sequence > current_seq {
                    actor
                        .event_sourced_sequences
                        .insert(entry.field_name.clone(), entry.sequence);
                }
            }
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
                    name, duration_ms, ..
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
                    let handler = self
                        .actors
                        .get(&actor_id)
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
        self.enqueue_actor(actor_id);
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
        if a == b {
            return;
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
        if watcher == target {
            return;
        }
        if let Some(actor) = self.actors.get_mut(&target) {
            if !actor.monitors.contains(&watcher) {
                actor.monitors.push(watcher);
            }
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
        exit::exit_actor(self, actor_id, reason)
    }

    pub fn kill_actor(&mut self, actor_id: u64) {
        exit::kill_actor(self, actor_id)
    }

    pub fn handle_actor_exit(&mut self, actor_id: u64, reason: ExitReason) {
        exit::handle_actor_exit(self, actor_id, reason)
    }

    /// Exit-protocol cleanup for an actor being removed: mark it terminated,
    /// release its receiver-side ORCA holds, unregister its names, leave its
    /// process groups, send DOWN to its monitors, propagate abnormal exits
    /// to linked actors, then reap it (retiring the heap while foreign
    /// references are outstanding).
    ///
    /// Shared by `handle_actor_exit` and by supervisor mass-removal paths
    /// (`restart_all`/`restart_from`/`shutdown_supervisor`), which remove
    /// LIVING children and therefore must not bypass the protocol.  Does not
    /// dispatch to the actor's supervisor — supervision is handled by the
    /// callers, which is why this is not simply `handle_actor_exit`.
    fn reap_living_actor(&mut self, actor_id: u64, reason: ExitReason) {
        exit::reap_living_actor(self, actor_id, reason)
    }


    // -- Builtin Actor Effects (Actor.*) --

    /// Dispatch a built-in `Actor.*` effect performed by `actor_id` (the
    /// current actor, when any). Every op yields nil except `whereis`,
    /// which yields the actor ref or nil for unknown names. Ops that need
    /// a current actor are no-ops outside one, matching the standalone
    /// VM's nil fallback. Returns `None` for unknown op names so the
    /// caller can fall through to other built-in handlers.
    fn perform_actor_builtin(
        &mut self,
        actor_id: Option<u64>,
        op_name: Option<&str>,
        constants: &[crate::bytecode::Constant],
        regs: &[Value],
    ) -> Option<Value> {
        let string_arg = |idx: usize| -> Option<String> {
            let id = regs.get(idx)?.as_string_id()?;
            match constants.get(id as usize) {
                Some(crate::bytecode::Constant::String(s)) => Some(s.clone()),
                _ => None,
            }
        };
        match op_name {
            Some("link") | Some("unlink") | Some("monitor") | Some("demonitor") => {
                let target = regs.get(0)?.as_actor_id()?;
                let Some(me) = actor_id else {
                    return Some(Value::nil());
                };
                match op_name {
                    Some("link") => self.link_actors(me, target),
                    Some("unlink") => self.unlink_actors(me, target),
                    Some("monitor") => self.monitor(me, target),
                    _ => self.demonitor(me, target),
                }
                Some(Value::nil())
            }
            Some("trap_exit") => {
                let flag = regs.get(0)?.as_bool()?;
                if let Some(me) = actor_id {
                    if let Some(actor) = self.actors.get_mut(&me) {
                        actor.trap_exits = flag;
                    }
                }
                Some(Value::nil())
            }
            Some("set_priority") => {
                // 0 = High, 1 = Normal, 2 = Low; any other value selects
                // Normal. Takes effect on the actor's next (re)queue.
                let level = regs.get(0)?.as_int()?;
                if let Some(me) = actor_id {
                    if let Some(actor) = self.actors.get_mut(&me) {
                        actor.priority = match level {
                            0 => ActorPriority::High,
                            2 => ActorPriority::Low,
                            _ => ActorPriority::Normal,
                        };
                    }
                }
                Some(Value::nil())
            }
            Some("exit") => {
                let reason = actor_exit_reason(regs.get(0), constants);
                if let Some(me) = actor_id {
                    self.exit_actor(me, reason);
                }
                Some(Value::nil())
            }
            Some("register") => {
                let name = string_arg(0)?;
                if let Some(me) = actor_id {
                    let _ = self.registry.register(&name, me);
                }
                Some(Value::nil())
            }
            Some("unregister") => {
                let name = string_arg(0)?;
                let _ = self.registry.unregister(&name);
                Some(Value::nil())
            }
            Some("whereis") => {
                let name = string_arg(0)?;
                Some(match self.registry.whereis(&name) {
                    Some(id) => Value::actor_ref(id),
                    None => Value::nil(),
                })
            }
            _ => None,
        }
    }

    // -- Builtin OTP Supervisor Effects (Otp.*) --

    /// Dispatch a built-in `Otp.*` supervisor effect performed from
    /// bytecode. Unlike `Actor.*`, these ops manage supervisors directly
    /// and do not need a current actor; unknown supervisor ids are nil
    /// no-ops (matching the `Actor.*` outside-an-actor contract). Returns
    /// `None` for unknown op names so the caller can fall through to other
    /// built-in handlers. `module` is the performing module: string args
    /// resolve against its constant pool and actor-type templates against
    /// its actor metadata (`find_actor_template`).
    fn perform_otp_builtin(
        &mut self,
        op_name: Option<&str>,
        module: &crate::bytecode::CodeModule,
        regs: &[Value],
    ) -> Option<Value> {
        let string_arg = |idx: usize| -> Option<String> {
            let id = regs.get(idx)?.as_string_id()?;
            match module.constants.get(id as usize) {
                Some(crate::bytecode::Constant::String(s)) => Some(s.clone()),
                _ => None,
            }
        };
        match op_name {
            // Strategy: 0=one_for_one, 1=one_for_all, 2=rest_for_one,
            // 3=simple_one_for_one; any other value is a nil no-op.
            Some("create_supervisor") => {
                let name = string_arg(0)?;
                let strategy = match regs.get(1)?.as_int()? {
                    0 => RestartStrategy::OneForOne,
                    1 => RestartStrategy::OneForAll,
                    2 => RestartStrategy::RestForOne,
                    3 => RestartStrategy::SimpleOneForOne,
                    _ => return Some(Value::nil()),
                };
                let id = self.create_supervisor(&name, strategy);
                Some(Value::int(id as i64))
            }
            // Policy: 0=permanent, 1=temporary, 2=transient; any other
            // value is a nil no-op.
            Some("supervise_child") => {
                let sup = regs.get(0)?.as_int()? as u64;
                let child = regs.get(1)?.as_actor_id()?;
                let policy = match regs.get(2)?.as_int()? {
                    0 => RestartPolicy::Permanent,
                    1 => RestartPolicy::Temporary,
                    2 => RestartPolicy::Transient,
                    _ => return Some(Value::nil()),
                };
                if self.supervisors.contains_key(&sup) {
                    let spec = ChildSpec::new(format!("child_{}", child), policy);
                    self.supervise_child(sup, spec, child);
                }
                Some(Value::nil())
            }
            Some("set_template") => {
                let sup = regs.get(0)?.as_int()? as u64;
                let type_name = string_arg(1)?;
                let _ = self.set_supervisor_template(sup, &type_name, module);
                Some(Value::nil())
            }
            Some("start_child") => {
                let sup = regs.get(0)?.as_int()? as u64;
                Some(match self.start_supervised_child(sup, Vec::new()) {
                    Some(id) => Value::actor_ref(id),
                    None => Value::nil(),
                })
            }
            Some("terminate_child") => {
                let sup = regs.get(0)?.as_int()? as u64;
                let child = regs.get(1)?.as_actor_id()?;
                let _ = self.terminate_supervised_child(sup, child);
                Some(Value::nil())
            }
            Some("child_count") => {
                let sup = regs.get(0)?.as_int()? as u64;
                Some(match self.supervisors.get(&sup) {
                    Some(supervisor) => Value::int(supervisor.child_count() as i64),
                    None => Value::nil(),
                })
            }
            _ => None,
        }
    }

    /// Resolve an actor type by name to the `(module, behavior_idx)` pair
    /// `spawn_from_module` expects. Searches the performing module first,
    /// then the runtime VM's loaded modules, then the recovery modules
    /// registered by previous spawns — so a type declared anywhere in the
    /// running program resolves even before its first spawn.
    fn find_actor_template(
        &self,
        name: &str,
        performing: &crate::bytecode::CodeModule,
    ) -> Option<(crate::bytecode::CodeModule, usize)> {
        fn find_in(module: &crate::bytecode::CodeModule, name: &str) -> Option<usize> {
            module
                .actor_metadata
                .iter()
                .find(|meta| meta.name == name)
                .and_then(|meta| meta.behavior_indices.first().copied())
        }
        if let Some(idx) = find_in(performing, name) {
            return Some((performing.clone(), idx));
        }
        if let Some(vm) = &self.vm {
            for module in &vm.modules {
                if let Some(idx) = find_in(module, name) {
                    return Some((module.clone(), idx));
                }
            }
        }
        for (module, _, _) in self.recovery_modules.values() {
            if let Some(idx) = find_in(module, name) {
                return Some((module.clone(), idx));
            }
        }
        None
    }

    // -- Supervisor Management --

    pub fn create_supervisor(&mut self, name: &str, strategy: RestartStrategy) -> u64 {
        let id = fresh_actor_id();
        let mut actor = Actor::new(id, name.to_string(), 0);
        actor.state = ActorState::Running;
        self.actors.insert(id, actor);
        let supervisor = Supervisor::new(id, name, strategy);
        self.supervisors.insert(id, supervisor);
        self.enqueue_actor(id);
        id
    }

    pub fn supervise_child(&mut self, supervisor_id: u64, spec: ChildSpec, child_id: u64) {
        // Snapshot everything a restart needs to rebuild the child, so a
        // supervised restart restores behaviors/bytecode/state instead of
        // producing a bare actor that silently drops every message.
        let restart = self.actors.get(&child_id).map(|actor| RestartTemplate {
            state_data: actor
                .state_data
                .iter()
                .map(|(name, value)| (name.clone(), *value))
                .collect(),
            state_models: actor.state_models.clone(),
            behaviors: actor
                .behavior_table
                .iter()
                .map(|entry| (entry.name.clone(), entry.handler_fn))
                .collect(),
            bytecode_module: actor.bytecode_module.clone(),
            bytecode_offsets: actor.bytecode_offsets.clone(),
            compensation_offsets: actor.compensation_offsets.clone(),
            persistent: actor.persistent,
            is_workflow: actor.is_workflow,
            is_agent: actor.is_agent,
        });
        let spec = ChildSpec { restart, ..spec };
        if let Some(child) = self.actors.get_mut(&child_id) {
            child.parent = Some(supervisor_id);
        }
        if let Some(supervisor) = self.supervisors.get_mut(&supervisor_id) {
            supervisor.add_child(spec, child_id);
        }
    }

    /// Set the child template of a `SimpleOneForOne` supervisor by actor
    /// type name. Returns false when the supervisor does not exist or the
    /// actor type cannot be resolved (see `find_actor_template`).
    pub fn set_supervisor_template(
        &mut self,
        supervisor_id: u64,
        type_name: &str,
        performing: &crate::bytecode::CodeModule,
    ) -> bool {
        let Some((module, behavior_idx)) = self.find_actor_template(type_name, performing) else {
            return false;
        };
        match self.supervisors.get_mut(&supervisor_id) {
            Some(supervisor) => {
                supervisor.template = Some(ChildTemplate {
                    type_name: type_name.to_string(),
                    module,
                    behavior_idx,
                });
                true
            }
            None => false,
        }
    }

    /// Start a dynamic child of a `SimpleOneForOne` supervisor from its
    /// child template. Returns the new child's actor id, or `None` for an
    /// unknown supervisor, a missing template, or a non-dynamic strategy.
    pub fn start_supervised_child(
        &mut self,
        supervisor_id: u64,
        init_args: Vec<(String, Value)>,
    ) -> Option<u64> {
        let mut supervisor = self.supervisors.remove(&supervisor_id)?;
        let result = supervisor.start_child(self, init_args);
        self.supervisors.insert(supervisor_id, supervisor);
        result
    }

    /// Terminate a supervised child WITHOUT restarting it (clean Normal
    /// exit). Returns false when the supervisor or the child is unknown.
    pub fn terminate_supervised_child(&mut self, supervisor_id: u64, actor_id: u64) -> bool {
        let Some(mut supervisor) = self.supervisors.remove(&supervisor_id) else {
            return false;
        };
        let result = supervisor.terminate_child(self, actor_id);
        self.supervisors.insert(supervisor_id, supervisor);
        result
    }

    // -- Internal Helpers --

    fn send_down_message(&mut self, watcher_id: u64, target_id: u64, reason: &ExitReason) {
        exit::send_down_message(self, watcher_id, target_id, reason)
    }

    /// Shut a supervisor down, removing its children and the supervisor
    /// actor itself through the full exit protocol so registered names and
    /// process groups are cleaned up and monitors/links are notified.
    ///
    /// The `supervisor` value is passed in because callers remove it from
    /// `self.supervisors` before deciding to shut it down — looking it up in

    // -- Distributed Actor System --

    pub fn enable_distribution(&mut self, bind_addr: std::net::SocketAddr) -> std::io::Result<()> {
        distribution::enable_distribution(self, bind_addr)
    }

    pub fn join_cluster(&mut self, seed_addr: std::net::SocketAddr) {
        distribution::join_cluster(self, seed_addr)
    }

    /// Register a behavior that remote nodes are allowed to spawn on this
    /// node by name (via `Packet::SpawnRequest`).
    ///
    /// This is the MVP scope of remote spawn: only native behaviors
    /// explicitly registered here can be spawned remotely — a node cannot
    /// make a peer run arbitrary code it never offered. When a spawn
    /// request for `name` arrives, the runtime spawns a fresh actor with
    /// the request's initial state and registers this handler as its sole
    pub fn register_spawnable_behavior(&mut self, name: &str, handler: fn(&mut Actor, &[Value])) {
        distribution::register_spawnable_behavior(self, name, handler)
    }

    /// Take the result of a previously issued remote spawn request.
    ///
    /// Returns `None` while the response has not arrived yet; otherwise
    /// `Some(Some(actor_id))` on success (the real actor id on the remote
    /// node — combine it with the node id into an `ActorAddress::remote`)
    /// or `Some(None)` if the remote node rejected the request (unknown
    /// behavior name).
    pub fn take_spawn_response(&mut self, request_id: u64) -> Option<Option<u64>> {
        distribution::take_spawn_response(self, request_id)
    }

    /// Check whether a packet with the given sequence number has been
    /// acknowledged by the receiver.
    pub fn is_acked(&self, seq: u64) -> bool {
        distribution::is_acked(self, seq)
    }

    /// Drain and return all acknowledged packet sequence numbers.
    ///
    /// Callers should drain periodically to avoid unbounded growth of the
    /// acked-packets set.
    pub fn drain_acked(&mut self) -> HashSet<u64> {
        distribution::drain_acked(self)
    }

    pub fn send_distributed(&mut self, target: ActorAddress, behavior: &str, args: &[Value]) {
        distribution::send_distributed(self, target, behavior, args)
    }

    pub fn process_network(&mut self) {
        distribution::process_network(self)
    }

    // -- CRDT Synchronization (v0.6) --

    /// Synchronize CRDT state with all healthy cluster members.
    ///
    /// Most rounds ship **delta-state** ops (`Packet::CrdtDeltaSync`) —
    /// only the changes since the previous round, with never-synced
    /// entries sent in full (the join fallback). Every
    /// `CRDT_FULL_SYNC_INTERVAL`-th round (starting with the first) ships
    /// full state (`Packet::CrdtSync`) instead: the sync base advances
    /// when deltas are generated, so a lost delta is never re-sent and
    /// these periodic full syncs are the repair mechanism.
    pub fn sync_crdts(&mut self) {
        distribution::sync_crdts(self)
    }

    /// Full-state CRDT sync: ship every entry to all healthy members.
    pub(crate) fn sync_crdts_full(&mut self) {
        let ops = match &mut self.crdt_manager {
            Some(m) => m.generate_sync_ops(),
            None => return,
        };
        if ops.is_empty() {
            return;
        }
        let packet = Packet::CrdtSync { ops };
        if let Some(cluster) = &self.distributed.cluster {
            for member in cluster.healthy_members() {
                if let Some(transport) = &mut self.distributed.transport {
                    let net_node_id = NodeId(member.node_id.0);
                    transport.send(net_node_id, member.address, packet.clone());
                }
            }
        }
    }
}

/// Interval (in `sync_crdts` rounds) between full-state repair syncs.
/// Round 1 is full; rounds 2..=N are delta; round N+1 is full again.
const CRDT_FULL_SYNC_INTERVAL: u64 = 16;

/// True when the given 1-based sync round should ship full state.

// ---------------------------------------------------------------------------
// CycleRuntime implementation
// ---------------------------------------------------------------------------

impl crate::runtime::orca_cycle::CycleRuntime for Runtime {
    unsafe fn free_object(&mut self, actor_id: u64, header: *mut crate::runtime::heap::OrcaHeader) {
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            // Remove from deferred-decrement list first so a later
            // `process_deferred` pass does not touch freed memory.
            actor.orca_gc.remove_deferred(header);

            // Compute the payload pointer and free on the owning actor's heap.
            let header_size = std::mem::size_of::<crate::runtime::heap::OrcaHeader>();
            let payload_ptr = (header as *mut u8).add(header_size);
            actor.heap.free(payload_ptr);
        }
    }
}

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

    fn alloc(&mut self, size: usize, type_tag: crate::runtime::heap::TypeTag) -> Option<*mut u8> {
        let mut rt = self.runtime.borrow_mut();
        if let Some(actor_id) = rt.current_actor {
            if let Some(actor) = rt.actors.get_mut(&actor_id) {
                return actor.heap.alloc(size, type_tag);
            }
        }
        None
    }

    // SAFETY: trait-impl signature is fixed; `ptr` always comes from the
    // VM's own heap allocations (the current actor's ActorHeap).
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn drop_ref(&mut self, ptr: *mut u8) {
        let mut rt = self.runtime.borrow_mut();
        if let Some(actor_id) = rt.current_actor {
            if let Some(actor) = rt.actors.get_mut(&actor_id) {
                // Route through ORCA so objects with outstanding foreign
                // references are deferred instead of freed out from under
                // other actors.
                unsafe {
                    actor.orca_gc.drop_local_ref(&mut actor.heap, ptr);
                }
            }
        }
    }

    // SAFETY: trait-impl signature is fixed; `ptr` always comes from the
    // VM's own heap allocations (the current actor's ActorHeap).
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn retain_ref(&mut self, ptr: *mut u8) {
        let mut rt = self.runtime.borrow_mut();
        if let Some(actor_id) = rt.current_actor {
            if let Some(actor) = rt.actors.get_mut(&actor_id) {
                unsafe {
                    actor.orca_gc.local_ref(&actor.heap, ptr);
                }
            }
        }
    }

    // SAFETY: trait-impl signature is fixed; `ptr` always comes from the
    // VM's own heap allocations (the current actor's ActorHeap).
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
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
        self.runtime
            .borrow_mut()
            .spawn_from_module(module, behavior_idx, init)
    }

    fn send_message(
        &mut self,
        target: crate::vm::Value,
        behavior_id: u16,
        args: &[crate::vm::Value],
    ) {
        if let Some(actor_id) = target.as_actor_id() {
            let mut rt = self.runtime.borrow_mut();
            rt.send_message_by_id(actor_id, behavior_id, args);
        }
    }

    fn ask_actor(
        &mut self,
        target: crate::vm::Value,
        behavior_id: u16,
        args: &[crate::vm::Value],
    ) -> crate::vm::Value {
        if let Some(actor_id) = target.as_actor_id() {
            let mut rt = self.runtime.borrow_mut();
            match rt.ask_actor_sync(actor_id, behavior_id, args) {
                Ok(value) => return value,
                Err(_) => {}
            }
        }
        crate::vm::Value::nil()
    }

    fn get_state_field(&self, field: &str) -> crate::vm::Value {
        let rt = self.runtime.borrow();
        if let Some(actor_id) = rt.current_actor {
            if let Some(actor) = rt.actors.get(&actor_id) {
                return actor
                    .get_state_field(field)
                    .unwrap_or(crate::vm::Value::nil());
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

    fn perform_effect(
        &mut self,
        effect_name: &str,
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
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

    fn perform_builtin_effect(
        &mut self,
        effect_name: &str,
        op_name: Option<&str>,
        constants: &[crate::bytecode::Constant],
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
        if effect_name == "Workflow" && op_name == Some("query") {
            let workflow_id = regs.get(0)?.as_actor_id()?;
            let string_id = regs.get(1)?.as_string_id()?;
            let query_name = match constants.get(string_id as usize) {
                Some(crate::bytecode::Constant::String(s)) => s.clone(),
                _ => return None,
            };
            let mut rt = self.runtime.borrow_mut();
            return rt.query_workflow(workflow_id, &query_name);
        }
        #[cfg(feature = "sqlite")]
        if effect_name == "DB" && op_name == Some("query") {
            let sql = match regs.first().and_then(|v| v.as_string_id()) {
                Some(id) => match constants.get(id as usize) {
                    Some(crate::bytecode::Constant::String(s)) => s.clone(),
                    _ => return Some(crate::vm::Value::nil()),
                },
                None => return Some(crate::vm::Value::nil()),
            };
            let params: Vec<crate::vm::Value> = regs.iter().skip(1).copied().collect();
            let mut rt = self.runtime.borrow_mut();
            let result = match rt.persistence.query(&sql, &params) {
                Ok(rows) => {
                    let json = serde_json::to_string(&rows).unwrap_or_default();
                    let bytes = json.into_bytes();
                    if let Some(ref mut vm) = rt.vm {
                        vm.add_runtime_string(0, String::from_utf8_lossy(&bytes).into_owned())
                    } else {
                        crate::vm::Value::nil()
                    }
                }
                Err(_) => crate::vm::Value::nil(),
            };
            return Some(result);
        }
        if effect_name == "Timer" && op_name == Some("after") {
            let ms = regs.first().and_then(|v| v.as_int()).unwrap_or(0);
            if ms > 0 {
                let callback_id = regs.get(1).and_then(|v| v.as_string_id());
                let callback_name = callback_id.and_then(|id| {
                    constants.get(id as usize).and_then(|c| match c {
                        crate::bytecode::Constant::String(s) => Some(s.clone()),
                        _ => None,
                    })
                });
                if let Some(callback_name) = callback_name {
                    let rt = self.runtime.borrow_mut();
                    let actor_id = rt.current_actor.unwrap_or(0);
                    let behavior_id = rt.behavior_id_for(actor_id, &callback_name).unwrap_or(0);
                    if behavior_id > 0 {
                        rt.timer_wheel.send_after(
                            std::time::Duration::from_millis(ms as u64),
                            actor_id,
                            behavior_id,
                            vec![],
                        );
                    }
                }
            }
            return Some(crate::vm::Value::unit());
        }
        if effect_name == "Int" && op_name == Some("to_string") {
            let n = regs.first().and_then(|v| v.as_int()).unwrap_or(0);
            let s = format!("{}", n);
            let mut rt = self.runtime.borrow_mut();
            return Some(match &mut rt.vm {
                Some(vm) => vm.allocate_string(&s),
                None => crate::vm::Value::nil(),
            });
        }
        if effect_name == "Provider" && op_name == Some("ask") {
            // General runtime-registered provider dispatch. The first arg is
            // the provider name (string); the second is the prompt/request
            // (string). This is the longevity path: `perform Provider.ask`
            // references no transient technology, only an eternal "provider"
            // abstraction. The "llm" provider reuses the existing LLM client.
            let provider = match regs.get(0).and_then(|v| v.as_string_id()) {
                Some(id) => match constants.get(id as usize) {
                    Some(crate::bytecode::Constant::String(s)) => s.clone(),
                    _ => return None,
                },
                None => return None,
            };
            let prompt = match regs.get(1) {
                Some(v) => {
                    if let Some(id) = v.as_string_id() {
                        constants
                            .get(id as usize)
                            .and_then(|c| match c {
                                crate::bytecode::Constant::String(s) => Some(s.clone()),
                                _ => None,
                            })
                            .unwrap_or_default()
                    } else {
                        v.to_string_repr()
                    }
                }
                None => return None,
            };
            if provider == "llm" {
                let mut rt = self.runtime.borrow_mut();
                if rt.llm.client.is_none() {
                    return Some(crate::vm::Value::nil());
                }
                let request = crate::ai::LlmRequest {
                    model: String::new(),
                    messages: vec![crate::ai::LlmMessage {
                        role: "user".to_string(),
                        content: prompt,
                    }],
                    tools: Vec::new(),
                    memory: Vec::new(),
                    pricing: None,
                    response_format: None,
                };
                let result = rt.complete_llm_request(request, Vec::new());
                return Some(match result {
                    Ok(resp) => match resp.content {
                        Some(c) => match &mut rt.vm {
                            Some(vm) => vm.add_runtime_string(0, c),
                            None => crate::vm::Value::nil(),
                        },
                        None => crate::vm::Value::nil(),
                    },
                    Err(_) => crate::vm::Value::nil(),
                });
            }
            return None;
        }
        if effect_name == "Actor" {
            let mut rt = self.runtime.borrow_mut();
            let actor_id = rt.current_actor;
            return rt.perform_actor_builtin(actor_id, op_name, constants, regs);
        }
        if effect_name == "IO" {
            if let (Some("print") | Some("println"), Some(first)) = (op_name, regs.first()) {
                let msg = crate::vm::resolve_value_string(constants, *first);
                println!("{}", msg);
                return Some(crate::vm::Value::unit());
            }
        }
        self.perform_effect(effect_name, regs)
    }

    fn perform_builtin_effect_in_module(
        &mut self,
        effect_name: &str,
        op_name: Option<&str>,
        module: &crate::bytecode::CodeModule,
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
        let qualified = match op_name {
            Some(op) => format!("{}.{}", effect_name, op),
            None => effect_name.to_string(),
        };
        // Check test handlers before real dispatch — allows tests to
        // intercept effects without a `handle` block in source.
        {
            let rt = self.runtime.borrow();
            if let Some(result) = rt.check_test_handler(&qualified, regs) {
                return Some(result);
            }
        }
        if effect_name == "Otp" {
            let mut rt = self.runtime.borrow_mut();
            return rt.perform_otp_builtin(op_name, module, regs);
        }
        self.perform_builtin_effect(effect_name, op_name, &module.constants, regs)
    }

    fn complete_llm(&mut self, model: &str, prompt: &str) -> Option<String> {
        let mut rt = self.runtime.borrow_mut();
        if let Some(actor_id) = rt.current_actor {
            if rt
                .actors
                .get(&actor_id)
                .map(|a| a.is_agent)
                .unwrap_or(false)
            {
                return rt.complete_agent_llm(actor_id, prompt);
            }
        }
        // Top-level (non-actor) LLM ask: issue a direct request without
        // agent state or memory handling.
        let request = LlmRequest {
            model: model.to_string(),
            messages: vec![LlmMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            tools: Vec::new(),
            memory: Vec::new(),
            pricing: None,
            response_format: None,
        };
        rt.complete_llm_request(request, Vec::new()).ok()?.content
    }

    fn pipeline_new(&mut self) -> i64 {
        self.runtime.borrow_mut().pipeline_new() as i64
    }

    fn pipeline_stage(&mut self, id: i64, name: &str, actor_id: u64, template: &str) -> i64 {
        self.runtime
            .borrow_mut()
            .pipeline_stage(id as u64, name, actor_id, template)
            .map(|id| id as i64)
            .unwrap_or(-1)
    }

    fn pipeline_run(&mut self, id: i64, input: &str) -> Option<String> {
        self.runtime
            .borrow_mut()
            .pipeline_run(id as u64, input)
            .ok()
    }

    fn supervisor_new(&mut self) -> i64 {
        self.runtime.borrow_mut().supervisor_new() as i64
    }

    fn supervisor_worker(&mut self, id: i64, name: &str, actor_id: u64, description: &str) -> i64 {
        self.runtime
            .borrow_mut()
            .supervisor_worker(id as u64, name, actor_id, description)
            .map(|id| id as i64)
            .unwrap_or(-1)
    }

    fn supervisor_run(&mut self, id: i64, task: &str) -> Option<String> {
        self.runtime
            .borrow_mut()
            .supervisor_run(id as u64, task)
            .ok()
    }

    fn debate_new(&mut self, topic: &str, rounds: i64, threshold: f64) -> i64 {
        self.runtime
            .borrow_mut()
            .debate_new(topic, rounds, threshold) as i64
    }

    fn debate_participant(&mut self, id: i64, name: &str, stance: &str, actor_id: u64) -> i64 {
        self.runtime
            .borrow_mut()
            .debate_participant(id as u64, name, stance, actor_id)
            .map(|id| id as i64)
            .unwrap_or(-1)
    }

    fn debate_run(&mut self, id: i64) -> Option<String> {
        self.runtime.borrow_mut().debate_run(id as u64).ok()
    }

    fn try_receive(&mut self) -> Option<(u16, crate::vm::Value)> {
        let mut rt = self.runtime.borrow_mut();
        let actor_id = rt.current_actor?;
        let msg = rt.actors.get(&actor_id)?.mailbox.pop()?;
        // ORCA receiver protocol: hold heap pointers carried by the message.
        rt.hold_payload_refs(actor_id, &msg.payload);
        let val = msg
            .payload
            .into_iter()
            .next()
            .unwrap_or(crate::vm::Value::unit());
        Some((msg.behavior_id, val))
    }

    fn try_receive_match(
        &mut self,
        behavior_ids: &[u16],
    ) -> Option<(usize, Vec<crate::vm::Value>)> {
        let mut rt = self.runtime.borrow_mut();
        let actor_id = rt.current_actor?;
        let (pos, payload) = rt
            .actors
            .get(&actor_id)?
            .mailbox
            .receive_match(behavior_ids)?;
        // ORCA receiver protocol: hold heap pointers carried by the message.
        rt.hold_payload_refs(actor_id, &payload);
        Some((pos, payload))
    }

    fn commit_receive_match(&mut self) {
        let mut rt = self.runtime.borrow_mut();
        if let Some(actor_id) = rt.current_actor {
            if let Some(actor) = rt.actors.get_mut(&actor_id) {
                actor.mailbox.commit_receive_match();
            }
        }
    }

    fn reset_receive_match(&mut self) {
        let mut rt = self.runtime.borrow_mut();
        if let Some(actor_id) = rt.current_actor {
            if let Some(actor) = rt.actors.get_mut(&actor_id) {
                actor.mailbox.reset_receive_match();
            }
        }
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
        unsafe {
            (*self.runtime)
                .actors
                .get_mut(&self.actor_id)?
                .heap
                .alloc(size, type_tag)
        }
    }

    fn drop_ref(&mut self, ptr: *mut u8) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                // Route through ORCA so objects with outstanding foreign
                // references are deferred instead of freed out from under
                // other actors.
                actor.orca_gc.drop_local_ref(&mut actor.heap, ptr);
            }
        }
    }

    fn retain_ref(&mut self, ptr: *mut u8) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                actor.orca_gc.local_ref(&actor.heap, ptr);
            }
        }
    }

    fn array_len(&self, ptr: *mut u8) -> Option<usize> {
        unsafe {
            let _actor = (*self.runtime).actors.get(&self.actor_id)?;
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
    }

    fn spawn_actor(
        &mut self,
        module: &crate::bytecode::CodeModule,
        behavior_idx: usize,
        init: Vec<(String, crate::vm::Value)>,
    ) -> crate::vm::Value {
        // SAFETY: the callback is installed on the shared runtime VM only
        // while the runtime drives a behavior on the single scheduler
        // thread, so `runtime` is a live, exclusively-borrowed pointer.
        // Spawning mutates runtime state but never re-enters the VM.
        unsafe { (*self.runtime).spawn_from_module(module, behavior_idx, init) }
    }

    fn send_message(
        &mut self,
        target: crate::vm::Value,
        behavior_id: u16,
        args: &[crate::vm::Value],
    ) {
        if let Some(target_id) = target.as_actor_id() {
            // SAFETY: as above. `send_message_by_id` is safe mid-behavior:
            // it pushes mail, bumps ORCA foreign counts, and enqueues the
            // target; the receive-wait wake is deferred while the shared
            // VM is executing (see `Runtime::pending_receive_wakes`).
            unsafe { (*self.runtime).send_message_by_id(target_id, behavior_id, args) }
        }
    }

    fn get_state_field(&self, field: &str) -> crate::vm::Value {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get(&self.actor_id) {
                return actor
                    .get_state_field(field)
                    .unwrap_or(crate::vm::Value::nil());
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

    fn perform_effect(
        &mut self,
        effect_name: &str,
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
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

    fn perform_builtin_effect(
        &mut self,
        effect_name: &str,
        op_name: Option<&str>,
        constants: &[crate::bytecode::Constant],
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
        unsafe {
            if effect_name == "Workflow" && op_name == Some("query") {
                let workflow_id = regs.get(0)?.as_actor_id()?;
                let string_id = regs.get(1)?.as_string_id()?;
                let query_name = match constants.get(string_id as usize) {
                    Some(crate::bytecode::Constant::String(s)) => s.clone(),
                    _ => return None,
                };
                return (*self.runtime).query_workflow(workflow_id, &query_name);
            }
            #[cfg(feature = "sqlite")]
            if effect_name == "DB" && op_name == Some("query") {
                let sql = match regs.first().and_then(|v| v.as_string_id()) {
                    Some(id) => match constants.get(id as usize) {
                        Some(crate::bytecode::Constant::String(s)) => s.clone(),
                        _ => return Some(crate::vm::Value::nil()),
                    },
                    None => return Some(crate::vm::Value::nil()),
                };
                let params: Vec<crate::vm::Value> = regs.iter().skip(1).copied().collect();
                return match (*self.runtime).persistence.query(&sql, &params) {
                    Ok(rows) => {
                        let json = serde_json::to_string(&rows).unwrap_or_default();
                        if let Some(ref mut vm) = (*self.runtime).vm {
                            Some(vm.add_runtime_string(0, json))
                        } else {
                            Some(crate::vm::Value::nil())
                        }
                    }
                    Err(_) => Some(crate::vm::Value::nil()),
                };
            }
            if effect_name == "Actor" {
                return (*self.runtime).perform_actor_builtin(
                    Some(self.actor_id),
                    op_name,
                    constants,
                    regs,
                );
            }
            if effect_name == "Timer" && op_name == Some("after") {
                let ms = regs.first().and_then(|v| v.as_int()).unwrap_or(0);
                if ms > 0 {
                    let callback_id = regs.get(1).and_then(|v| v.as_string_id());
                    let callback_name = callback_id.and_then(|id| {
                        constants.get(id as usize).and_then(|c| match c {
                            crate::bytecode::Constant::String(s) => Some(s.clone()),
                            _ => None,
                        })
                    });
                    if let Some(callback_name) = callback_name {
                        let behavior_id = (*self.runtime)
                            .behavior_id_for(self.actor_id, &callback_name)
                            .unwrap_or(0);
                        if behavior_id > 0 {
                            (*self.runtime).timer_wheel.send_after(
                                std::time::Duration::from_millis(ms as u64),
                                self.actor_id,
                                behavior_id,
                                vec![],
                            );
                        }
                    }
                }
                return Some(crate::vm::Value::unit());
            }
            if effect_name == "Int" && op_name == Some("to_string") {
                let n = regs.first().and_then(|v| v.as_int()).unwrap_or(0);
                let s = format!("{}", n);
                if let Some(vm) = &mut (*self.runtime).vm {
                    return Some(vm.allocate_string(&s));
                }
                return Some(crate::vm::Value::nil());
            }
            if effect_name == "Provider" && op_name == Some("ask") {
                // General runtime-registered provider dispatch (actor path).
                // Mirrors RuntimeVmCallbacks::perform_builtin_effect's Provider
                // branch. The "llm" provider reuses the agent-aware complete_llm.
                let provider = match regs.get(0).and_then(|v| v.as_string_id()) {
                    Some(id) => match constants.get(id as usize) {
                        Some(crate::bytecode::Constant::String(s)) => s.clone(),
                        _ => return None,
                    },
                    None => return None,
                };
                let prompt = match regs.get(1) {
                    Some(v) => {
                        if let Some(id) = v.as_string_id() {
                            constants
                                .get(id as usize)
                                .and_then(|c| match c {
                                    crate::bytecode::Constant::String(s) => Some(s.clone()),
                                    _ => None,
                                })
                                .unwrap_or_default()
                        } else {
                            v.to_string_repr()
                        }
                    }
                    None => return None,
                };
                if provider == "llm" {
                    let content = self.complete_llm("", &prompt);
                    let rt = &mut *self.runtime;
                    return Some(match content {
                        Some(c) => match &mut rt.vm {
                            Some(vm) => vm.add_runtime_string(0, c),
                            None => crate::vm::Value::nil(),
                        },
                        None => crate::vm::Value::nil(),
                    });
                }
                return None;
            }
            if effect_name == "IO" {
                if let (Some("print") | Some("println"), Some(first)) = (op_name, regs.first()) {
                    let msg = crate::vm::resolve_value_string(constants, *first);
                    println!("{}", msg);
                    return Some(crate::vm::Value::unit());
                }
            }
            self.perform_effect(effect_name, regs)
        }
    }

    fn perform_builtin_effect_in_module(
        &mut self,
        effect_name: &str,
        op_name: Option<&str>,
        module: &crate::bytecode::CodeModule,
        regs: &[crate::vm::Value],
    ) -> Option<crate::vm::Value> {
        let qualified = match op_name {
            Some(op) => format!("{}.{}", effect_name, op),
            None => effect_name.to_string(),
        };
        unsafe {
            // Check test handlers before real dispatch.
            if let Some(result) = (*self.runtime).check_test_handler(&qualified, regs) {
                return Some(result);
            }
            if effect_name == "Otp" {
                return (*self.runtime).perform_otp_builtin(op_name, module, regs);
            }
            self.perform_builtin_effect(effect_name, op_name, &module.constants, regs)
        }
    }

    fn complete_llm(&mut self, model: &str, prompt: &str) -> Option<String> {
        unsafe {
            let rt = &mut *self.runtime;
            if rt
                .actors
                .get(&self.actor_id)
                .map(|a| a.is_agent)
                .unwrap_or(false)
            {
                return rt.complete_agent_llm(self.actor_id, prompt);
            }
            let request = rt.build_actor_llm_request(self.actor_id, model, prompt)?;
            let module = rt.actors.get(&self.actor_id)?.bytecode_module.clone()?;
            rt.complete_llm_with_tools(request, Vec::new(), &module)
                .ok()?
                .content
        }
    }

    fn llm_ask(&mut self, model: &str, prompt: &str) -> crate::vm::LlmAskResult {
        use crate::vm::LlmAskResult;
        unsafe {
            let rt = &mut *self.runtime;
            let actor_id = self.actor_id;

            // Nested synchronous paths (pipelines, ask_actor_sync) keep the
            // blocking behavior.
            if !rt.llm.suspend_enabled {
                return LlmAskResult::Ready(self.complete_llm(model, prompt));
            }

            // Re-executed after a resume: a completed response is waiting.
            let completed = rt
                .actors
                .get_mut(&actor_id)
                .and_then(|actor| actor.llm_completed.take());
            if let Some(result) = completed {
                return match result {
                    Ok(response) => {
                        // Finish on the scheduler thread: tool invocation and
                        // durable-state write-back must not run on the worker.
                        let prev_current_actor = rt.current_actor;
                        rt.current_actor = Some(actor_id);
                        let is_agent = rt
                            .actors
                            .get(&actor_id)
                            .map(|a| a.is_agent)
                            .unwrap_or(false);
                        let content = if is_agent {
                            let module = rt
                                .actors
                                .get(&actor_id)
                                .and_then(|a| a.bytecode_module.clone());
                            let processed = match module {
                                Some(m) => rt.finish_tool_calls(&m, response),
                                None => Ok(response),
                            };
                            match processed {
                                Ok(resp) => agent::finish_agent_llm(rt, actor_id, prompt, &resp),
                                Err(_) => None,
                            }
                        } else {
                            let module = rt
                                .actors
                                .get(&actor_id)
                                .and_then(|a| a.bytecode_module.clone());
                            match module {
                                Some(m) => rt
                                    .finish_tool_calls(&m, response)
                                    .ok()
                                    .and_then(|r| r.content),
                                None => response.content,
                            }
                        };
                        rt.current_actor = prev_current_actor;
                        LlmAskResult::Ready(content)
                    }
                    Err(_) => LlmAskResult::Ready(None),
                };
            }

            // A call is already in flight (defensive; should not happen).
            if rt
                .actors
                .get(&actor_id)
                .map(|a| a.llm_inflight)
                .unwrap_or(false)
            {
                return LlmAskResult::Pending;
            }

            // Build the request on the scheduler thread, then hand it to a
            // background worker for the HTTP call.
            let is_agent = rt
                .actors
                .get(&actor_id)
                .map(|a| a.is_agent)
                .unwrap_or(false);
            let request = if is_agent {
                agent::build_agent_llm_request(rt, actor_id, prompt)
            } else {
                rt.build_actor_llm_request(actor_id, model, prompt)
            };
            // Build failure (e.g. missing agent state fields): nil response.
            let Some(request) = request else {
                return LlmAskResult::Ready(None);
            };
            if !(*rt).dispatch_llm_request(actor_id, request, prompt) {
                // Dispatch failed: fall back to a nil response.
                rt.llm.inflight_count = rt.llm.inflight_count.saturating_sub(1);
                if let Some(actor) = rt.actors.get_mut(&actor_id) {
                    actor.llm_inflight = false;
                    actor.llm_pending_prompt = None;
                }
                return LlmAskResult::Ready(None);
            }
            LlmAskResult::Pending
        }
    }

    fn pipeline_new(&mut self) -> i64 {
        unsafe { (*self.runtime).pipeline_new() as i64 }
    }

    fn pipeline_stage(&mut self, id: i64, name: &str, actor_id: u64, template: &str) -> i64 {
        unsafe {
            (*self.runtime)
                .pipeline_stage(id as u64, name, actor_id, template)
                .map(|id| id as i64)
                .unwrap_or(-1)
        }
    }

    fn pipeline_run(&mut self, id: i64, input: &str) -> Option<String> {
        unsafe { (*self.runtime).pipeline_run(id as u64, input).ok() }
    }

    fn supervisor_new(&mut self) -> i64 {
        unsafe { (*self.runtime).supervisor_new() as i64 }
    }

    fn supervisor_worker(&mut self, id: i64, name: &str, actor_id: u64, description: &str) -> i64 {
        unsafe {
            (*self.runtime)
                .supervisor_worker(id as u64, name, actor_id, description)
                .map(|id| id as i64)
                .unwrap_or(-1)
        }
    }

    fn supervisor_run(&mut self, id: i64, task: &str) -> Option<String> {
        unsafe { (*self.runtime).supervisor_run(id as u64, task).ok() }
    }

    fn debate_new(&mut self, topic: &str, rounds: i64, threshold: f64) -> i64 {
        unsafe { (*self.runtime).debate_new(topic, rounds, threshold) as i64 }
    }

    fn debate_participant(&mut self, id: i64, name: &str, stance: &str, actor_id: u64) -> i64 {
        unsafe {
            (*self.runtime)
                .debate_participant(id as u64, name, stance, actor_id)
                .map(|id| id as i64)
                .unwrap_or(-1)
        }
    }

    fn debate_run(&mut self, id: i64) -> Option<String> {
        unsafe { (*self.runtime).debate_run(id as u64).ok() }
    }

    fn try_receive(&mut self) -> Option<(u16, crate::vm::Value)> {
        unsafe {
            let actor = (*self.runtime).actors.get(&self.actor_id)?;
            let msg = actor.mailbox.pop()?;
            // ORCA receiver protocol: hold heap pointers carried by the message.
            (*self.runtime).hold_payload_refs(self.actor_id, &msg.payload);
            let val = msg
                .payload
                .into_iter()
                .next()
                .unwrap_or(crate::vm::Value::unit());
            Some((msg.behavior_id, val))
        }
    }

    fn try_receive_match(
        &mut self,
        behavior_ids: &[u16],
    ) -> Option<(usize, Vec<crate::vm::Value>)> {
        unsafe {
            let actor = (*self.runtime).actors.get(&self.actor_id)?;
            let (pos, payload) = actor.mailbox.receive_match(behavior_ids)?;
            // ORCA receiver protocol: hold heap pointers carried by the message.
            (*self.runtime).hold_payload_refs(self.actor_id, &payload);
            Some((pos, payload))
        }
    }

    fn receive_wait_suspend(&mut self, timeout_ms: i64) -> bool {
        unsafe {
            let rt = &mut *self.runtime;
            let Some(actor) = rt.actors.get_mut(&self.actor_id) else {
                return false;
            };
            // A fired timeout resolves the wait exactly once: consume the
            // marker so the re-executed ReceiveWait writes the no-match
            // sentinel and a later wait starts clean.
            if actor.receive_wait.map(|w| w.timed_out).unwrap_or(false) {
                actor.receive_wait = None;
                return false;
            }
            // Non-positive timeouts poll once (Erlang-style non-blocking
            // receive). Synchronous entry points (ask_actor_sync: pipelines,
            // supervisors, debates, `Ask`) never suspend — same gating as
            // the non-blocking LLM path.
            if timeout_ms <= 0 || !rt.llm.suspend_enabled {
                return false;
            }
            true
        }
    }

    fn receive_wait_matched(&mut self) {
        unsafe {
            let rt = &mut *self.runtime;
            let wait = rt
                .actors
                .get_mut(&self.actor_id)
                .and_then(|a| a.receive_wait.take());
            // A match resolves the wait: cancel the pending timeout so it
            // cannot fire into a later wait on this actor.
            if let Some(wait) = wait {
                rt.timer_wheel.cancel(wait.timer_id);
            }
        }
    }

    fn commit_receive_match(&mut self) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                actor.mailbox.commit_receive_match();
            }
        }
    }

    fn reset_receive_match(&mut self) {
        unsafe {
            if let Some(actor) = (*self.runtime).actors.get_mut(&self.actor_id) {
                actor.mailbox.reset_receive_match();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Distributed callbacks for the bytecode VM — bridges RSend/RAsk/RSpawn
// opcodes to the runtime's send_distributed infrastructure.
// ---------------------------------------------------------------------------

/// Raw-pointer callbacks for distributed VM opcodes (`RSend`, `RAsk`,
/// `Migrate`, `RSpawn`, `Gossip`).  Mirrors [`BytecodeRuntimeCallbacks`]
/// in using a transient `*mut Runtime` borrow — the VM calls these only
/// while the runtime holds `&mut self`, so the pointer is valid and unique.
#[derive(Debug)]
struct BytecodeDistributedCallbacks {
    runtime: *mut Runtime,
}

// SAFETY: the VM only invokes these callbacks while the calling
// `Runtime` method holds `&mut self`.  The raw pointer is therefore the
// sole active borrow of the runtime.
unsafe impl Send for BytecodeDistributedCallbacks {}
unsafe impl Sync for BytecodeDistributedCallbacks {}

impl crate::vm::DistributedVmCallbacks for BytecodeDistributedCallbacks {
    fn node_id(&self) -> u64 {
        unsafe {
            (*self.runtime)
                .distributed
                .node_id
                .map(|n| n.0)
                .unwrap_or(0)
        }
    }

    fn remote_send(
        &mut self,
        target_actor: u64,
        target_node: u64,
        behavior: &str,
        args: &[crate::vm::Value],
    ) {
        unsafe {
            let rt = &mut *self.runtime;
            // Take distributed fields out so send_distributed can borrow
            // them independently of rt itself.
            let mut transport = rt.distributed.transport.take();
            let mut resolver = rt.distributed.resolver.take();
            let cluster = rt.distributed.cluster.take();
            if let (Some(ref mut t), Some(ref c), Some(ref mut r)) =
                (&mut transport, &cluster, &mut resolver)
            {
                let target = ActorAddress::remote(NodeId(target_node), target_actor);
                send_distributed(rt, t, c, r, target, behavior, args);
            }
            rt.distributed.transport = transport;
            rt.distributed.resolver = resolver;
            rt.distributed.cluster = cluster;
        }
    }

    fn migrate(&mut self, _actor_id: u64, _target_node_id: u64) {}
    fn remote_ask(
        &mut self,
        _target_actor: u64,
        _behavior: &str,
        _args: &[crate::vm::Value],
        _timeout_ms: u64,
    ) -> crate::vm::Value {
        crate::vm::Value::nil()
    }
    fn gossip(&mut self, _message: &str) -> crate::vm::Value {
        crate::vm::Value::unit()
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

/// Compute the backoff delay in milliseconds for the given retry attempt
/// (0-indexed). Uses actor_id as a seed for ±25% jitter to avoid
/// deterministic thundering-herd on clusters.
fn compute_backoff(config: &crate::ast::AgentRetryConfig, attempt: u32, actor_id: u64) -> u64 {
    let base_ms = match &config.backoff {
        crate::ast::AgentBackoff::Exponential {
            initial_ms,
            factor,
            max_ms,
        } => {
            let exp = (*factor).powi(attempt as i32);
            let delay = (*initial_ms as f64 * exp).min(*max_ms as f64);
            delay as u64
        }
        crate::ast::AgentBackoff::Fixed { delay_ms } => *delay_ms,
    };
    // ±25% jitter, seeded from actor_id so different actors (or the same
    // actor on different nodes with different ids) get different jitter.
    let seed = actor_id
        .wrapping_mul(6364136223846793005)
        .wrapping_add(attempt as u64);
    let r = (seed >> 33) as f64 / (1u64 << 31) as f64; // [0, 1)
    let jittered = base_ms as f64 + (base_ms as f64 * 0.5 * (r - 0.5));
    jittered.max(0.0) as u64
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
