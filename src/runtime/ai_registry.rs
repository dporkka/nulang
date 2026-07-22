//! Extracted AI-runtime registry: pipelines and debates.
//!
//! Groups the v0.9 AI Runtime bookkeeping (`next_pipeline_id`, `pipelines`,
//! `next_debate_id`, `debates`) into a single struct so the `Runtime`
//! god-object (`src/runtime/mod.rs` is ~6100 lines) shrinks and these
//! subsystems can evolve — or be removed — independently.
//!
//! This follows the extraction pattern established by
//! [`SupervisorTeamRegistry`](super::supervisor_registry::SupervisorTeamRegistry).

use std::collections::HashMap;

use crate::ai::{Debate, Pipeline};

/// Owns the AI-runtime pipeline and debate bookkeeping.
///
/// Both are AI-runtime constructs classified as Experimental in the
/// repositioned language (see RFC 0004). They remain functional during
/// the deprecation cycle.
#[derive(Debug, Default)]
pub struct AiRuntimeRegistry {
    /// Next pipeline id.
    pub next_pipeline_id: u64,
    /// Active pipelines, keyed by id.
    pub pipelines: HashMap<u64, Pipeline>,

    /// Next debate id.
    pub next_debate_id: u64,
    /// Active debates, keyed by id.
    pub debates: HashMap<u64, Debate>,
}

impl AiRuntimeRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            next_pipeline_id: 1,
            pipelines: HashMap::new(),
            next_debate_id: 1,
            debates: HashMap::new(),
        }
    }

    /// Create a new empty pipeline and return its id.
    pub fn create_pipeline(&mut self) -> u64 {
        let id = self.next_pipeline_id;
        self.next_pipeline_id = self.next_pipeline_id.wrapping_add(1);
        self.pipelines.insert(id, Pipeline::new());
        id
    }

    /// Add a stage to an existing pipeline. Returns the same pipeline id on
    /// success so fluent construction can continue.
    pub fn add_pipeline_stage(
        &mut self,
        id: u64,
        name: &str,
        agent_id: u64,
        template: &str,
    ) -> Result<u64, String> {
        let pipeline = self
            .pipelines
            .get_mut(&id)
            .ok_or_else(|| format!("Pipeline {} not found", id))?;
        pipeline.stages.push(crate::ai::PipelineStage {
            name: name.to_string(),
            agent_id,
            prompt_template: template.to_string(),
        });
        Ok(id)
    }

    /// Run a pipeline, returning the output of the final stage.
    /// Takes a `&mut dyn PipelineRuntime` because the pipeline calls back
    /// into the runtime to execute its LLM-backed stages.
    pub fn run_pipeline<R: crate::ai::PipelineRuntime>(
        &self,
        id: u64,
        runtime: &mut R,
        input: &str,
    ) -> Result<String, String> {
        let pipeline = self
            .pipelines
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("Pipeline {} not found", id))?;
        pipeline.run(runtime, input)
    }

    /// Create a new debate and return its id.
    pub fn create_debate(&mut self, topic: &str, rounds: i64, threshold: f64) -> u64 {
        let id = self.next_debate_id;
        self.next_debate_id = self.next_debate_id.wrapping_add(1);
        self.debates
            .insert(id, Debate::new(topic, rounds.max(1) as usize, threshold));
        id
    }

    /// Add a participant to an existing debate. Returns the same debate id on
    /// success so fluent construction can continue.
    pub fn add_debate_participant(
        &mut self,
        id: u64,
        name: &str,
        stance: &str,
        agent_id: u64,
    ) -> Result<u64, String> {
        let debate = self
            .debates
            .get_mut(&id)
            .ok_or_else(|| format!("Debate {} not found", id))?;
        *debate = debate.clone().participant(name, stance, agent_id);
        Ok(id)
    }

    /// Run a debate and return the moderator's synthesis.
    /// Takes a `&mut dyn DebateRuntime` because the debate calls back into
    /// the runtime to execute its LLM-backed participant turns.
    pub fn run_debate<R: crate::ai::DebateRuntime>(
        &self,
        id: u64,
        runtime: &mut R,
    ) -> Result<String, String> {
        let debate = self
            .debates
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("Debate {} not found", id))?;
        debate.run(runtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{DebateRuntime, PipelineRuntime};

    struct MockRuntime;

    impl PipelineRuntime for MockRuntime {
        fn ask_agent(&mut self, _agent_id: u64, prompt: &str) -> Result<String, String> {
            Ok(prompt.to_string())
        }
    }

    impl DebateRuntime for MockRuntime {
        fn ask_agent(&mut self, _agent_id: u64, prompt: &str) -> Result<String, String> {
            Ok(prompt.to_string())
        }
    }

    #[test]
    fn test_create_pipeline() {
        let mut reg = AiRuntimeRegistry::new();
        let id = reg.create_pipeline();
        assert_eq!(id, 1);
        assert!(reg.pipelines.contains_key(&id));
    }

    #[test]
    fn test_create_debate() {
        let mut reg = AiRuntimeRegistry::new();
        let id = reg.create_debate("test", 3, 0.8);
        assert_eq!(id, 1);
        assert!(reg.debates.contains_key(&id));
    }

    #[test]
    fn test_pipeline_stage() {
        let mut reg = AiRuntimeRegistry::new();
        let id = reg.create_pipeline();
        reg.add_pipeline_stage(id, "summarize", 42, "Summarize: {input}")
            .expect("stage should succeed");
        let pipeline = reg.pipelines.get(&id).unwrap();
        assert_eq!(pipeline.stages.len(), 1);
        assert_eq!(pipeline.stages[0].name, "summarize");
    }

    #[test]
    fn test_debate_participant() {
        let mut reg = AiRuntimeRegistry::new();
        let id = reg.create_debate("test", 3, 0.8);
        reg.add_debate_participant(id, "Alice", "for", 1)
            .expect("participant should succeed");
        let debate = reg.debates.get(&id).unwrap();
        assert_eq!(debate.participants.len(), 1);
    }
}
