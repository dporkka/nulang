//! `impl` blocks that tie `nulang_ai` types to the core `Runtime`.
//!
//! The traits (`PipelineRuntime`, `DebateRuntime`, `SupervisorRuntime`) and
//! the structs they operate on (`Pipeline`, `Debate`, `SupervisorTeam`) live
//! in the `nulang-ai` crate so the pure AI types stay extractable. These
//! impl blocks stay in core because they depend on `Runtime` internals.

use crate::bytecode::Constant;
use crate::runtime::{Actor, Runtime};
use crate::vm::Value;
use nulang_ai::{DebateRuntime, PipelineRuntime, SupervisorRuntime};

// ---------------------------------------------------------------------------
// PipelineRuntime
// ---------------------------------------------------------------------------

impl PipelineRuntime for Runtime {
    fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String> {
        let behavior_id = self
            .behavior_id_for(agent_id, "ask")
            .or_else(|| {
                // Agent actors compiled from source keep their behaviors as
                // bytecode offsets rather than native behavior-table entries.
                // Find the index in the actor's bytecode module behavior table.
                let actor = self.actors.get(&agent_id)?;
                let module = actor.bytecode_module.as_ref()?;
                module
                    .behaviors
                    .iter()
                    .position(|b| b.name.ends_with(".ask"))
                    .map(|idx| idx as u16)
            })
            .ok_or_else(|| format!("Actor {} has no 'ask' behavior", agent_id))?;

        let prompt_value = {
            let actor = self
                .actors
                .get_mut(&agent_id)
                .ok_or_else(|| format!("Actor {} not found", agent_id))?;
            actor.allocate_string(prompt)
        };

        let response = self
            .ask_actor_sync(agent_id, behavior_id, &[prompt_value])
            .map_err(|e| format!("Ask failed for actor {}: {}", agent_id, e))?;

        let actor = self
            .actors
            .get(&agent_id)
            .ok_or_else(|| format!("Actor {} disappeared during ask", agent_id))?;
        value_to_string(&response, actor).ok_or_else(|| {
            format!(
                "Could not convert response from actor {} to string",
                agent_id
            )
        })
    }
}

// ---------------------------------------------------------------------------
// DebateRuntime
// ---------------------------------------------------------------------------

impl DebateRuntime for Runtime {
    fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String> {
        PipelineRuntime::ask_agent(self, agent_id, prompt)
    }
}

// ---------------------------------------------------------------------------
// SupervisorRuntime
// ---------------------------------------------------------------------------

impl SupervisorRuntime for Runtime {
    fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String> {
        PipelineRuntime::ask_agent(self, agent_id, prompt)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a VM value returned by an actor into a plain Rust string.
fn value_to_string(value: &Value, actor: &Actor) -> Option<String> {
    if let Some(id) = value.as_string_id() {
        actor
            .bytecode_module
            .as_ref()
            .and_then(|m| m.constants.get(id as usize))
            .and_then(|c| match c {
                Constant::String(s) => Some(s.clone()),
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
