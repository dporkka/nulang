//! Runtime system for Nulang: actors, scheduler, heap, GC, supervisor.

pub mod actor;
pub mod gc;
pub mod heap;
pub mod mailbox;
pub mod scheduler;
pub mod supervisor;
pub mod tests;

use actor::{ActorSystem, Addr};
use gc::RefCountGC;
use heap::ActorHeap;
use mailbox::Mailbox;
use scheduler::Scheduler;
use supervisor::Supervisor;
use crate::types::Value;

/// Complete runtime environment.
pub struct Runtime {
    pub actor_system: ActorSystem,
    pub scheduler: Scheduler,
    pub gc: RefCountGC,
    pub supervisor: Supervisor,
    pub global_heap: ActorHeap,
}

impl Runtime {
    pub fn new() -> Self {
        Runtime {
            actor_system: ActorSystem::new(),
            scheduler: Scheduler::new(4),
            gc: RefCountGC::new(),
            supervisor: Supervisor::new(),
            global_heap: ActorHeap::new(3),
        }
    }

    pub fn spawn_actor(&mut self, initial_state: Value) -> Addr {
        let addr = self.actor_system.spawn(initial_state);
        self.supervisor.monitor(addr);
        addr
    }

    pub fn send_message(&self, to: Addr, behavior: &str, args: &[Value]) {
        self.actor_system.send(to, behavior, args);
    }

    pub fn start(&mut self) {
        self.scheduler.start();
    }

    pub fn shutdown(&mut self) {
        self.scheduler.shutdown();
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_creation() {
        let rt = Runtime::new();
        assert_eq!(rt.scheduler.worker_count(), 4);
    }

    #[test]
    fn test_spawn_actor() {
        let mut rt = Runtime::new();
        let addr = rt.spawn_actor(Value::int(42));
        assert_eq!(addr.id, 1);
    }
}
