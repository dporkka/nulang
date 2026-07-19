//! Extracted supervisor-team registry.
//!
//! This is a small, self-contained piece of state factored out of the
//! `Runtime` god-object (`src/runtime/mod.rs` is ~6000 lines). It owns the
//! AI-runtime supervisor-team bookkeeping (`next_supervisor_id`,
//! `supervisor_teams`). The BEAM-style supervisor tree (`supervisors`
//! HashMap, `Supervisor` struct, restart strategies) stays in `mod.rs` for
//! now — it is too tightly coupled to the actor map and exit protocol to
//! extract without risk.
//!
//! This extraction demonstrates the pattern: subsystem state moves into its
//! own struct, `Runtime` holds it as a field, and methods delegate. Further
//! extractions (Scheduler, GcCoordinator, etc.) follow the same shape.

use std::collections::HashMap;

use crate::ai::SupervisorTeam;

/// Owns the AI-runtime supervisor-team bookkeeping. Extracted from
/// `Runtime` so the god-object shrinks and the supervisor-team subsystem
/// can evolve independently.
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
        team.workers.push(crate::ai::Worker {
            name: name.to_string(),
            agent_id,
            description: description.to_string(),
        });
        Ok(id)
    }

    /// Run a supervisor team, returning the final worker's output. Takes a
    /// `&mut Runtime` because the team's workers call back into the runtime
    /// to execute their LLM-backed tasks.
    pub fn run(
        &self,
        id: u64,
        runtime: &mut crate::runtime::Runtime,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creates_incrementing_ids() {
        let mut reg = SupervisorTeamRegistry::new();
        let id1 = reg.create();
        let id2 = reg.create();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert!(reg.teams.contains_key(&id1));
        assert!(reg.teams.contains_key(&id2));
    }

    #[test]
    fn test_add_worker_errors_on_unknown_team() {
        let mut reg = SupervisorTeamRegistry::new();
        let err = reg.add_worker(999, "w", 0, "d").unwrap_err();
        assert!(err.contains("not found"));
    }
}
