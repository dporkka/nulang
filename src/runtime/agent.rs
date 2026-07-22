//! Agent subsystem: LLM client management, token budgets, pipelines,
//! supervisor teams, debates, and the agent LLM completion pipeline.
//!
//! All functions in this module take `&Runtime` or `&mut Runtime` and
//! access its public fields directly. This extraction follows the pattern
//! established by `workflow.rs`, `exit.rs`, `distribution.rs`, and
//! `spawn.rs` — shrink the Runtime god-object (`mod.rs`) by moving
//! self-contained subsystems into their own modules.
//!
//! These methods are classified as Experimental (RFC 0004). They remain
//! functional during the deprecation cycle.

use std::sync::Arc;

use crate::ai::{EpisodicMemory, LlmClient, LlmMessage, LlmRequest, LlmResponse, ModelPricing,
    TokenBudget};
use crate::runtime::Runtime;

// ---------------------------------------------------------------------------
// LLM client + token budget
// ---------------------------------------------------------------------------

/// Install an LLM client for `perform LLM.ask(...)` calls.
pub(crate) fn set_llm_client(rt: &mut Runtime, client: Box<dyn LlmClient>) {
    rt.llm.client = Some(Arc::from(client));
}

/// Set a token budget that caps total LLM token consumption.
///
/// After the budget is exhausted `complete_llm_request` returns
/// `LlmError::BudgetExceeded`.  Charges are applied after each
/// successful response based on the actual token count returned
/// by the provider.
pub(crate) fn set_token_budget(rt: &mut Runtime, limit: u64) {
    rt.llm.token_budget = Some(std::sync::Arc::new(TokenBudget::new(limit)));
}

/// Remove any configured token budget.
pub(crate) fn clear_token_budget(rt: &mut Runtime) {
    rt.llm.token_budget = None;
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// Create a new empty pipeline and return its ID.
pub(crate) fn pipeline_new(rt: &mut Runtime) -> u64 {
    rt.ai.create_pipeline()
}

/// Add a stage to an existing pipeline. Returns the same pipeline ID on
/// success so fluent construction can continue.
pub(crate) fn pipeline_stage(
    rt: &mut Runtime,
    id: u64,
    name: &str,
    agent_id: u64,
    template: &str,
) -> Result<u64, String> {
    rt.ai.add_pipeline_stage(id, name, agent_id, template)
}

/// Run a pipeline, returning the output of the final stage.
pub(crate) fn pipeline_run(rt: &mut Runtime, id: u64, input: &str) -> Result<String, String> {
    let pipeline = rt
        .ai
        .pipelines
        .get(&id)
        .cloned()
        .ok_or_else(|| format!("Pipeline {} not found", id))?;
    pipeline.run(rt, input)
}

// ---------------------------------------------------------------------------
// Supervisor teams
// ---------------------------------------------------------------------------

pub(crate) fn supervisor_new(rt: &mut Runtime) -> u64 {
    rt.supervisor_teams.create()
}

pub(crate) fn supervisor_worker(
    rt: &mut Runtime,
    id: u64,
    name: &str,
    agent_id: u64,
    description: &str,
) -> Result<u64, String> {
    rt.supervisor_teams
        .add_worker(id, name, agent_id, description)
}

pub(crate) fn supervisor_run(rt: &mut Runtime, id: u64, task: &str) -> Result<String, String> {
    let team = rt
        .supervisor_teams
        .teams
        .get(&id)
        .cloned()
        .ok_or_else(|| format!("Supervisor team {} not found", id))?;
    team.run(rt, task)
}

// ---------------------------------------------------------------------------
// Debates
// ---------------------------------------------------------------------------

/// Create a new debate and return its ID.
pub(crate) fn debate_new(rt: &mut Runtime, topic: &str, rounds: i64, threshold: f64) -> u64 {
    rt.ai.create_debate(topic, rounds, threshold)
}

/// Add a participant to an existing debate. Returns the same debate ID on
/// success so fluent construction can continue.
pub(crate) fn debate_participant(
    rt: &mut Runtime,
    id: u64,
    name: &str,
    stance: &str,
    agent_id: u64,
) -> Result<u64, String> {
    rt.ai.add_debate_participant(id, name, stance, agent_id)
}

/// Run a debate and return the moderator's synthesis.
pub(crate) fn debate_run(rt: &mut Runtime, id: u64) -> Result<String, String> {
    let debate = rt
        .ai
        .debates
        .get(&id)
        .cloned()
        .ok_or_else(|| format!("Debate {} not found", id))?;
    debate.run(rt)
}

// ---------------------------------------------------------------------------
// VM value ↔ string helpers
// ---------------------------------------------------------------------------

/// Convert a VM value to a Rust string using the actor's bytecode module
/// constant pool for string-id values and reading pointer payloads as
/// null-terminated UTF-8.
pub(crate) fn vm_value_to_string(
    value: &crate::vm::Value,
    module: Option<&crate::bytecode::CodeModule>,
) -> Option<String> {
    if let Some(id) = value.as_string_id() {
        module
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

// ---------------------------------------------------------------------------
// Agent LLM completion pipeline
// ---------------------------------------------------------------------------

/// Execute an LLM request for an agent actor, reading the agent's model,
/// system prompt, and episodic memory from durable state. The memory is
/// updated with the user prompt and assistant response before being saved
/// back to state.
pub(crate) fn complete_agent_llm(rt: &mut Runtime, actor_id: u64, prompt: &str) -> Option<String> {
    let prev_current_actor = rt.current_actor;
    rt.current_actor = Some(actor_id);

    let result = complete_agent_llm_inner(rt, actor_id, prompt);

    rt.current_actor = prev_current_actor;
    result
}

fn complete_agent_llm_inner(rt: &mut Runtime, actor_id: u64, prompt: &str) -> Option<String> {
    let request = build_agent_llm_request(rt, actor_id, prompt)?;
    let module = rt.actors.get(&actor_id)?.bytecode_module.clone()?;
    let response = rt
        .complete_llm_with_tools(request, Vec::new(), &module)
        .ok()?;
    finish_agent_llm(rt, actor_id, prompt, &response)
}

/// Build the LLM request for an agent actor from its durable state
/// (model, system prompt, episodic memory, pricing) without issuing any
/// network call. Pure read/build: safe to run before handing the request
/// to a background worker thread.
pub(crate) fn build_agent_llm_request(rt: &Runtime, actor_id: u64, prompt: &str) -> Option<LlmRequest> {
    let (model, system_prompt, memory_json, pricing, module) = {
        let actor = rt.actors.get(&actor_id)?;
        let module = actor.bytecode_module.clone()?;
        let model = vm_value_to_string(&actor.get_state_field("model")?, Some(&module))?;
        let system_prompt =
            vm_value_to_string(&actor.get_state_field("system_prompt")?, Some(&module))?;
        let memory_json = vm_value_to_string(
            &actor.get_state_field("episodic_memory")?,
            Some(&module),
        )?;
        let pricing = ModelPricing {
            input_cost_per_1k: actor.get_state_field("pricing_input")?.as_float()?,
            output_cost_per_1k: actor.get_state_field("pricing_output")?.as_float()?,
        };
        (model, system_prompt, memory_json, pricing, module)
    };

    let memory: EpisodicMemory = serde_json::from_str(&memory_json)
        .unwrap_or_else(|_| EpisodicMemory::new(50));

    let mut messages = Vec::new();
    if !system_prompt.is_empty() {
        messages.push(LlmMessage {
            role: "system".to_string(),
            content: system_prompt,
        });
    }
    messages.extend(memory.to_messages());
    messages.push(LlmMessage {
        role: "user".to_string(),
        content: prompt.to_string(),
    });

    Some(LlmRequest {
        model,
        messages,
        tools: module.tools.clone(),
        memory: Vec::new(),
        pricing: Some(pricing),
        response_format: None,
    })
}

/// Finish an agent LLM call on the scheduler thread: accumulate token
/// usage and cost, append the exchange to episodic memory, and write the
/// durable state back. Returns the response content. Episodic memory is
/// re-read fresh here (never reuse the build-time snapshot).
pub(crate) fn finish_agent_llm(
    rt: &mut Runtime,
    actor_id: u64,
    prompt: &str,
    response: &LlmResponse,
) -> Option<String> {
    let (pricing, usage_prompt, usage_completion, usage_cost, memory_json) = {
        let actor = rt.actors.get(&actor_id)?;
        let module = actor.bytecode_module.clone()?;
        let pricing = ModelPricing {
            input_cost_per_1k: actor.get_state_field("pricing_input")?.as_float()?,
            output_cost_per_1k: actor.get_state_field("pricing_output")?.as_float()?,
        };
        let usage_prompt = actor.get_state_field("usage_prompt")?.as_int()? as u32;
        let usage_completion = actor.get_state_field("usage_completion")?.as_int()? as u32;
        let usage_cost = actor.get_state_field("usage_cost")?.as_float()?;
        let memory_json = vm_value_to_string(
            &actor.get_state_field("episodic_memory")?,
            Some(&module),
        )?;
        (
            pricing,
            usage_prompt,
            usage_completion,
            usage_cost,
            memory_json,
        )
    };
    let content = response.content.clone().unwrap_or_default();

    // Accumulate token usage and cost into durable state.
    let new_cost = crate::ai::estimated_cost(&response.usage, &pricing);
    let updated_prompt = usage_prompt.saturating_add(response.usage.prompt);
    let updated_completion = usage_completion.saturating_add(response.usage.completion);
    let updated_cost = usage_cost + new_cost;

    let mut memory: EpisodicMemory = serde_json::from_str(&memory_json)
        .unwrap_or_else(|_| EpisodicMemory::new(50));
    memory.add_turn("user", prompt);
    memory.add_turn("assistant", &content);
    let updated_memory = serde_json::to_string(&memory).ok()?;

    let actor = rt.actors.get_mut(&actor_id)?;
    let ptr = actor.allocate_string(&updated_memory);
    actor.set_state_field("episodic_memory", ptr);
    actor.set_state_field("usage_prompt", crate::vm::Value::int(updated_prompt as i64));
    actor.set_state_field(
        "usage_completion",
        crate::vm::Value::int(updated_completion as i64),
    );
    actor.set_state_field("usage_cost", crate::vm::Value::float(updated_cost));
    Some(content)
}

/// Build a bare LLM request for a non-agent actor bytecode behavior,
/// with `tools` filled from the actor's bytecode module. Pure
/// read/build: safe to run before handing the request to a background
/// worker thread.
pub(crate) fn build_actor_llm_request(
    rt: &Runtime,
    actor_id: u64,
    model: &str,
    prompt: &str,
) -> Option<LlmRequest> {
    let module = rt.actors.get(&actor_id)?.bytecode_module.clone()?;
    Some(LlmRequest {
        model: model.to_string(),
        messages: vec![LlmMessage {
            role: "user".to_string(),
            content: prompt.to_string(),
        }],
        tools: module.tools.clone(),
        memory: Vec::new(),
        pricing: None,
        response_format: None,
    })
}

/// Read an actor's state field as a plain string, resolving string-id
/// values through the runtime VM's constant pools (heap pointer values
/// are read directly). Useful for tests and tooling that inspect actor
/// state produced by bytecode behaviors.
pub(crate) fn actor_state_string(rt: &Runtime, actor_id: u64, field: &str) -> Option<String> {
    let actor = rt.actors.get(&actor_id)?;
    let value = actor.get_state_field(field)?;
    if value.as_string_id().is_some() {
        let vm = rt.vm.as_ref()?;
        let module_idx = actor.bytecode_module_idx?;
        return Some(vm.value_to_string(module_idx, value));
    }
    vm_value_to_string(&value, actor.bytecode_module.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Runtime;

    #[test]
    fn test_set_clear_token_budget() {
        let mut rt = Runtime::new();
        assert!(rt.llm.token_budget.is_none());
        set_token_budget(&mut rt, 1000);
        assert!(rt.llm.token_budget.is_some());
        clear_token_budget(&mut rt);
        assert!(rt.llm.token_budget.is_none());
    }

    #[test]
    fn test_pipeline_new_and_stage() {
        let mut rt = Runtime::new();
        let id = pipeline_new(&mut rt);
        // Add a stage — needs a valid agent_id; use 0 as placeholder
        let result = pipeline_stage(&mut rt, id, "test_stage", 0, "Hello");
        assert!(result.is_ok());
    }

    #[test]
    fn test_debate_new_and_participant() {
        let mut rt = Runtime::new();
        let id = debate_new(&mut rt, "test topic", 3, 0.6);
        let result = debate_participant(&mut rt, id, "Alice", "pro", 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_supervisor_new_and_worker() {
        let mut rt = Runtime::new();
        let id = supervisor_new(&mut rt);
        let result = supervisor_worker(&mut rt, id, "worker1", 0, "does stuff");
        assert!(result.is_ok());
    }
}
