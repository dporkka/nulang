//! Actor spawn subsystem: creates new actors with state, bytecode handlers,
//! and recovery metadata. These free functions take `&mut Runtime` to access
//! the runtime's public fields.

use std::collections::HashMap;

use crate::runtime::actor::{Actor, ActorBackend, BehaviorEntry};
use crate::runtime::persistence::{PersistedValue, StateModel, WorkflowEvent};
use crate::runtime::Runtime;
use crate::runtime::{bytecode_step_placeholder, fresh_actor_id, map_ast_state_model};
use crate::runtime::{timer_fired_handler};
use crate::vm::Value;

/// Core spawn logic shared by all spawn entry points.
pub(crate) fn spawn_actor_with_models(
    rt: &mut Runtime,
    init: Box<dyn FnOnce() -> Vec<(String, Value)>>,
    state_models: HashMap<String, StateModel>,
    persistent: bool,
    workflow: Option<&str>,
) -> u64 {
    let id = fresh_actor_id();
    let mut actor = Actor::new(id, format!("actor_{}", id), 0);
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
    actor.state = crate::runtime::ActorState::Running;
    rt.actors.insert(id, actor);
    if workflow.is_some() {
        let seq = crate::runtime::workflow::next_sequence(rt, id);
        let state = {
            let actor = rt.actors.get(&id).unwrap();
            let mut state = Vec::new();
            for (field_name, value) in &actor.state_data {
                let model = actor
                    .state_models
                    .get(field_name)
                    .copied()
                    .unwrap_or(StateModel::Local);
                if model.is_persistent() {
                    state.push(PersistedValue::from_value(value));
                }
            }
            state
        };
        let _ = rt.persistence.append_workflow_event(
            id,
            WorkflowEvent::WorkflowStarted {
                sequence: seq,
                name: workflow_name.as_ref().unwrap().clone(),
                state,
            },
        );
        crate::runtime::workflow::checkpoint_actor(rt, id);
    }
    rt.enqueue_actor(id);
    id
}

/// Spawn an actor for `module`'s behavior `behavior_idx`, seeded with the
/// `init` state fields, and wire up its bytecode handlers. Shared body of
/// both VM-callback `spawn_actor` impls.
pub(crate) fn spawn_from_module(
    rt: &mut Runtime,
    module: &crate::bytecode::CodeModule,
    behavior_idx: usize,
    init: Vec<(String, Value)>,
) -> Value {
    let meta = module
        .actor_metadata
        .iter()
        .find(|m| m.behavior_indices.contains(&behavior_idx));
    let id = if let Some(meta) = meta {
        let state_models: HashMap<String, StateModel> = meta
            .state_models
            .iter()
            .map(|(name, model)| (name.clone(), map_ast_state_model(*model)))
            .collect();
        let defaults = meta.state_defaults.clone();
        spawn_actor_with_models(
            rt,
            Box::new(move || {
                let mut fields: Vec<(String, Value)> = defaults
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
        spawn_actor_with_models(rt, Box::new(move || init), HashMap::new(), false, None)
    };
    let offsets: Vec<usize> = module.behaviors.iter().map(|b| b.code_offset).collect();
    let compensation_offsets: Vec<Option<usize>> = module
        .behaviors
        .iter()
        .map(|b| b.compensate_offset)
        .collect();
    if let Some(actor) = rt.actors.get_mut(&id) {
        actor.bytecode_module = Some(module.clone());
        actor.bytecode_offsets = offsets.clone();
        actor.compensation_offsets = compensation_offsets.clone();
        if let Some(meta) = meta {
            if meta.is_agent {
                actor.is_agent = true;
                for (name, c) in &meta.state_defaults {
                    if let crate::bytecode::Constant::String(json) = c {
                        if name == "retry_config" {
                            actor.retry_config = serde_json::from_str(json).ok();
                        } else if name == "fallback_config" {
                            actor.fallback_config = serde_json::from_str(json).unwrap_or_default();
                        }
                    }
                }
            }
            for (name, c) in &meta.state_defaults {
                if let crate::bytecode::Constant::String(s) = c {
                    let ptr = actor.allocate_string(s);
                    actor.set_state_field(name, ptr);
                }
            }
            actor.backend = match meta.backend {
                crate::ast::ActorBackendKind::Native => ActorBackend::Native,
                crate::ast::ActorBackendKind::WasmComponent => ActorBackend::WasmComponent {
                    component_path: String::new(),
                },
            };
        }
    }
    if meta.map(|m| m.is_workflow).unwrap_or(false) {
        layout_workflow_behavior_table(rt, id);
    }
    register_recovery_module(rt, id, module.clone(), offsets, compensation_offsets);
    Value::actor_ref(id)
}

/// Populate a workflow actor's behavior table with placeholder entries for
/// each bytecode step plus the internal `__timer_fired` handler.
pub(crate) fn layout_workflow_behavior_table(rt: &mut Runtime, actor_id: u64) {
    if let Some(actor) = rt.actors.get_mut(&actor_id) {
        if !actor.is_workflow {
            return;
        }
        let step_count = actor.bytecode_offsets.len();
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

/// Register bytecode metadata so that a persistent actor can be recovered
/// after a runtime restart.
pub(crate) fn register_recovery_module(
    rt: &mut Runtime,
    actor_id: u64,
    module: crate::bytecode::CodeModule,
    offsets: Vec<usize>,
    compensation_offsets: Vec<Option<usize>>,
) {
    rt.recovery_modules
        .insert(actor_id, (module, offsets, compensation_offsets));
}
