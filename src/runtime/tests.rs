//! Runtime integration tests.

#[cfg(test)]
mod integration_tests {
    use crate::runtime::Runtime;
    use crate::types::Value;

    #[test]
    fn test_runtime_spawn_multiple_actors() {
        let mut rt = Runtime::new();
        let a1 = rt.spawn_actor(Value::int(0));
        let a2 = rt.spawn_actor(Value::int(1));
        let a3 = rt.spawn_actor(Value::int(2));
        assert_ne!(a1.id, a2.id);
        assert_ne!(a2.id, a3.id);
    }

    #[test]
    fn test_supervisor_restarts() {
        use crate::runtime::supervisor::{Supervisor, RestartPolicy, Strategy, ChildSpec};
        use crate::runtime::actor::Addr;
        use std::time::Duration;

        let mut sup = Supervisor::new();
        let addr = Addr::local(1);
        sup.monitor(addr);
        sup.handle_exit(addr, "normal");
        assert_eq!(sup.actor_count(), 1);
    }
}
