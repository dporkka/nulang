//! Registries for AI-runtime pipelines, debates, and supervisor teams.
//!
//! Both [`AiRuntimeRegistry`] (pipelines + debates) and
//! [`SupervisorTeamRegistry`] are pure state — a monotonically-increasing id
//! counter plus a `HashMap` — with `run_*` methods that delegate through the
//! trait-object runtime abstractions ([`PipelineRuntime`], [`DebateRuntime`],
//! [`SupervisorRuntime`]).
//!
//! The core `nulang` crate mounts one instance of each on its `Runtime` and
//! implements the abstraction traits. Keeping the registries here means the
//! `Runtime` god-object never grows a new AI field: adding a new
//! orchestration primitive is a `nulang-ai`-only change.
//!
//! Classified as Experimental (RFC 0004).

use std::collections::HashMap;

use crate::debate::{Debate, DebateRuntime};
use crate::pipeline::{Pipeline, PipelineRuntime, PipelineStage};
use crate::supervisor::{SupervisorRuntime, SupervisorTeam, Worker};

// ---------------------------------------------------------------------------
// Pipelines + debates
// ---------------------------------------------------------------------------

/// Owns the AI-runtime pipeline and debate bookkeeping.
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
        pipeline.stages.push(PipelineStage {
            name: name.to_string(),
            agent_id,
            prompt_template: template.to_string(),
        });
        Ok(id)
    }

    /// Run a pipeline, returning the output of the final stage.
    pub fn run_pipeline<R: PipelineRuntime>(
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
    pub fn run_debate<R: DebateRuntime>(&self, id: u64, runtime: &mut R) -> Result<String, String> {
        let debate = self
            .debates
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("Debate {} not found", id))?;
        debate.run(runtime)
    }
}

// ---------------------------------------------------------------------------
// Supervisor teams
// ---------------------------------------------------------------------------

/// Owns the AI-runtime supervisor-team bookkeeping.
#[derive(Debug, Default)]
pub struct SupervisorTeamRegistry {
    pub next_id: u64,
    pub teams: HashMap<u64, SupervisorTeam>,
}

impl SupervisorTeamRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        SupervisorTeamRegistry {
            next_id: 1,
            teams: HashMap::new(),
        }
    }

    /// Create a new supervisor team and return its id.
    pub fn create(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.teams.insert(id, SupervisorTeam::new());
        id
    }

    /// Add a worker to an existing team.
    pub fn add_worker(
        &mut self,
        id: u64,
        name: &str,
        agent_id: u64,
        description: &str,
    ) -> Result<u64, String> {
        let team = self
            .teams
            .get_mut(&id)
            .ok_or_else(|| format!("Supervisor team {} not found", id))?;
        team.workers.push(Worker {
            name: name.to_string(),
            agent_id,
            description: description.to_string(),
        });
        Ok(id)
    }

    /// Run a supervisor team, returning the final worker's output.
    pub fn run<R: SupervisorRuntime>(
        &self,
        id: u64,
        runtime: &mut R,
        task: &str,
    ) -> Result<String, String> {
        let team = self
            .teams
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("Supervisor team {} not found", id))?;
        team.run(runtime, task)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
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

    impl SupervisorRuntime for MockRuntime {
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

    #[test]
    fn test_supervisor_registry_creates_incrementing_ids() {
        let mut reg = SupervisorTeamRegistry::new();
        let id1 = reg.create();
        let id2 = reg.create();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert!(reg.teams.contains_key(&id1));
        assert!(reg.teams.contains_key(&id2));
    }

    #[test]
    fn test_supervisor_add_worker_errors_on_unknown_team() {
        let mut reg = SupervisorTeamRegistry::new();
        let err = reg.add_worker(999, "w", 0, "d").unwrap_err();
        assert!(err.contains("not found"));
    }
}
