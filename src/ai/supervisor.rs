//! Supervisor pattern for multi-agent orchestration.
//!
//! A [`SupervisorTeam`] coordinates a set of worker agents.  The supervisor
//! receives a task, delegates it through the workers in order, and returns the
//! final accumulated result.  Each worker is prompted with the previous worker's
//! output (or the original task for the first worker) so the team behaves like
//! a sequential refinement chain.

use crate::runtime::Runtime;

// ---------------------------------------------------------------------------
// Runtime abstraction
// ---------------------------------------------------------------------------

/// Minimal runtime capability required to execute a supervisor team.
///
/// Implemented for [`Runtime`] using the actor `ask` behavior.  Test code can
/// provide a mock implementation to avoid spinning up a real actor system.
pub trait SupervisorRuntime {
    /// Send `prompt` to `agent_id` and return the textual response.
    fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String>;
}

impl SupervisorRuntime for Runtime {
    fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String> {
        crate::ai::PipelineRuntime::ask_agent(self, agent_id, prompt)
    }
}

// ---------------------------------------------------------------------------
// Supervisor definition
// ---------------------------------------------------------------------------

/// A single worker in a supervisor team.
#[derive(Debug, Clone)]
pub struct Worker {
    /// Logical name for the worker.
    pub name: String,
    /// Target actor id.
    pub agent_id: u64,
    /// Description used when prompting the worker.
    pub description: String,
}

/// A supervisor team that delegates tasks to a sequence of workers.
#[derive(Debug, Clone, Default)]
pub struct SupervisorTeam {
    pub workers: Vec<Worker>,
    pub max_iterations: usize,
}

impl SupervisorTeam {
    /// Create an empty supervisor team.
    pub fn new() -> Self {
        Self {
            workers: Vec::new(),
            max_iterations: 10,
        }
    }

    /// Create a team with a maximum iteration limit.
    pub fn with_max_iterations(max_iterations: usize) -> Self {
        Self {
            workers: Vec::new(),
            max_iterations,
        }
    }

    /// Append a worker and return `self` for fluent construction.
    pub fn worker(
        mut self,
        name: impl Into<String>,
        agent_id: u64,
        description: impl Into<String>,
    ) -> Self {
        self.workers.push(Worker {
            name: name.into(),
            agent_id,
            description: description.into(),
        });
        self
    }

    /// Run the team on `task`, returning the final worker's output.
    ///
    /// Each worker receives a prompt that includes its description and the
    /// current accumulated state.  Returns an error if the team has no workers
    /// or if any worker call fails.
    pub fn run<R: SupervisorRuntime>(
        &self,
        runtime: &mut R,
        task: &str,
    ) -> Result<String, String> {
        if self.workers.is_empty() {
            return Err("Supervisor team has no workers".to_string());
        }

        let mut current = task.to_string();
        for worker in &self.workers {
            let prompt = format!(
                "You are {}. {}\n\nTask: {}\n\nCurrent context: {}",
                worker.name, worker.description, task, current
            );
            current = runtime.ask_agent(worker.agent_id, &prompt)?;
        }
        Ok(current)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    struct MockRuntime {
        responses: HashMap<u64, String>,
        calls: RefCell<Vec<(u64, String)>>,
    }

    impl MockRuntime {
        fn new(responses: HashMap<u64, String>) -> Self {
            Self {
                responses,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl SupervisorRuntime for MockRuntime {
        fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String> {
            self.calls
                .borrow_mut()
                .push((agent_id, prompt.to_string()));
            self.responses
                .get(&agent_id)
                .cloned()
                .ok_or_else(|| format!("No response configured for agent {}", agent_id))
        }
    }

    #[test]
    fn test_empty_team_errors() {
        let team = SupervisorTeam::new();
        let mut rt = MockRuntime::new(HashMap::new());
        assert_eq!(
            team.run(&mut rt, "task"),
            Err("Supervisor team has no workers".to_string())
        );
    }

    #[test]
    fn test_single_worker() {
        let team = SupervisorTeam::new().worker("writer", 1, "Writes content");
        let mut rt = MockRuntime::new(HashMap::from([(1, "article".to_string())]));

        let result = team.run(&mut rt, "Write about CRDTs").unwrap();
        assert_eq!(result, "article");

        let calls = rt.calls.into_inner();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1.contains("writer"));
        assert!(calls[0].1.contains("Write about CRDTs"));
    }

    #[test]
    fn test_multiple_workers_chain() {
        let team = SupervisorTeam::new()
            .worker("researcher", 1, "Finds information")
            .worker("writer", 2, "Writes content");
        let mut rt = MockRuntime::new(HashMap::from([
            (1, "research notes".to_string()),
            (2, "final article".to_string()),
        ]));

        let result = team.run(&mut rt, "CRDTs").unwrap();
        assert_eq!(result, "final article");

        let calls = rt.calls.into_inner();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].1.contains("research notes"));
    }

    #[test]
    fn test_worker_error_propagates() {
        let team = SupervisorTeam::new()
            .worker("ok", 1, "ok")
            .worker("fail", 2, "fail");
        let mut rt = MockRuntime::new(HashMap::from([(1, "intermediate".to_string())]));

        assert_eq!(
            team.run(&mut rt, "start"),
            Err("No response configured for agent 2".to_string())
        );
    }
}
