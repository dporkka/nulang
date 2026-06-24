//! Supervision trees for fault tolerance.

use crate::runtime::actor::Addr;

use std::collections::HashMap;

/// Supervision strategy.
#[derive(Debug, Clone)]
pub enum Strategy {
    OneForOne,   // Restart only failed child
    OneForAll,   // Restart all children
    RestForOne,  // Restart failed and subsequent children
}

/// Child specification.
#[derive(Debug, Clone)]
pub struct ChildSpec {
    pub id: String,
    pub restart: RestartPolicy,
    pub max_restarts: u32,
    pub restart_window: std::time::Duration,
}

#[derive(Debug, Clone)]
pub enum RestartPolicy {
    Permanent,   // Always restart
    Temporary,   // Never restart
    Transient,   // Restart only on abnormal exit
}

/// Supervisor node in the tree.
#[derive(Debug)]
pub struct SupervisorNode {
    pub addr: Addr,
    pub children: Vec<ChildSpec>,
    pub strategy: Strategy,
    pub restart_counts: HashMap<String, Vec<std::time::Instant>>,
}

/// Root supervisor managing all actors.
pub struct Supervisor {
    actors: Vec<Addr>,
}

impl Supervisor {
    pub fn new() -> Self {
        Supervisor { actors: Vec::new() }
    }

    pub fn monitor(&mut self, addr: Addr) {
        self.actors.push(addr);
    }

    pub fn demonitor(&mut self, addr: Addr) {
        self.actors.retain(|&a| a != addr);
    }

    pub fn handle_exit(&self, _addr: Addr, _reason: &str) {
        // Placeholder: apply restart strategy
    }

    pub fn actor_count(&self) -> usize {
        self.actors.len()
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supervisor_monitor() {
        let mut sup = Supervisor::new();
        let addr = Addr::local(1);
        sup.monitor(addr);
        assert_eq!(sup.actor_count(), 1);
    }
}
