//! Actor model: spawn, send, receive, supervision.

use crate::types::Value;

/// Unique actor address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Addr {
    pub node: u16,
    pub id: u32,
    pub generation: u16,
}

impl Addr {
    pub fn local(id: u32) -> Self {
        Addr { node: 0, id, generation: 0 }
    }

    pub fn to_value(&self) -> Value {
        Value::actor_ref(self.node, self.id, self.generation)
    }
}

/// Actor context (accessible within behavior handlers).
pub struct ActorContext {
    pub self_addr: Addr,
    pub state: Value,
}

impl ActorContext {
    pub fn new(addr: Addr, state: Value) -> Self {
        ActorContext { self_addr: addr, state }
    }

    pub fn update_state(&mut self, new_state: Value) {
        self.state = new_state;
    }
}

/// Trait for actor behaviors.
pub trait Behavior: Send + Sync {
    fn handle(&self, ctx: &mut ActorContext, args: &[Value]) -> Value;
    fn name(&self) -> &str;
}

/// Actor system: manages all actors.
pub struct ActorSystem {
    next_id: u32,
}

impl ActorSystem {
    pub fn new() -> Self {
        ActorSystem { next_id: 1 }
    }

    pub fn spawn(&mut self, _initial_state: Value) -> Addr {
        let addr = Addr::local(self.next_id);
        self.next_id += 1;
        addr
    }

    pub fn send(&self, _to: Addr, _behavior: &str, _args: &[Value]) {
        // Placeholder
    }

    pub fn ask(&self, _to: Addr, _behavior: &str, _args: &[Value]) -> Value {
        Value::null()
    }
}

impl Default for ActorSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addr_creation() {
        let addr = Addr::local(42);
        assert_eq!(addr.id, 42);
        assert_eq!(addr.node, 0);
    }

    #[test]
    fn test_actor_system_spawn() {
        let mut sys = ActorSystem::new();
        let a1 = sys.spawn(Value::int(0));
        let a2 = sys.spawn(Value::int(1));
        assert_ne!(a1.id, a2.id);
    }
}
