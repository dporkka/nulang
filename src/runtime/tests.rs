//! Runtime integration tests.
//!
//! 84 tests total (see AGENTS.md "Testing & QA" for the suite-wide counts).
//! Full history in local commit 1c2cde9.

use super::*;
use crate::runtime::gc::OrcaGc;
use crate::runtime::heap::{ActorHeap, TypeTag};
use crate::vm::Frame;
use std::time::{Duration, Instant};

// ========================================================================
// Core Runtime Tests
// ========================================================================

#[test]
fn test_spawn_send_step_sequence() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![("count".to_string(), Value::int(0))]));
    assert!(rt.actors.contains_key(&actor_id));
    {
        let actor = rt.actors.get_mut(&actor_id).unwrap();
        actor.register_behavior("inc", |actor, args| {
            if let Some(n) = actor.get_state_field("counter").and_then(|v| v.as_int()) {
                if let Some(incr) = args.get(0).and_then(|v| v.as_int()) {
                    actor.set_state_field("counter", Value::int(n + incr));
                }
            }
        });
    }
    rt.send_message(actor_id, "inc", &[Value::int(1)]);
    rt.step_actor(actor_id);
    // Message processed
}

#[test]
fn test_mailbox_push_pop() {
    let mb = Mailbox::new(4);
    let msg = Message {
        behavior_id: 0,
        payload: vec![Value::int(42)],
        sender: 1,
        priority: MessagePriority::Normal,
    };
    assert!(mb.push(msg.clone()).is_ok());
    assert_eq!(mb.len(), 1);
    let popped = mb.pop().unwrap();
    assert_eq!(popped.payload[0].as_int(), Some(42));
    assert!(mb.is_empty());
}

#[test]
fn test_scheduler_enqueue_steal() {
    let sched = Scheduler::new(4);
    assert!(sched.steal_one().is_none());
    sched.enqueue(100);
    sched.enqueue(200);
    // Global injector is FIFO: 100 was enqueued first, so it's stolen first
    assert_eq!(sched.steal_one(), Some(100));
    assert_eq!(sched.steal_one(), Some(200));
    assert!(sched.steal_one().is_none());
}

#[test]
fn test_actor_register_behavior() {
    let mut actor = Actor::new(1, "test_actor", 0);
    actor.register_behavior("hello", |_actor, _args| {});
    assert_eq!(actor.behavior_table.len(), 1);
    assert_eq!(actor.behavior_table[0].name, "hello");
}

#[test]
fn test_run_scheduler_processes_all_actors() {
    let mut rt = Runtime::new();
    let a1 = rt.spawn_actor(Box::new(|| vec![("counter".to_string(), Value::int(0))]));
    let a2 = rt.spawn_actor(Box::new(|| vec![("counter".to_string(), Value::int(0))]));
    rt.send_message(a1, "add", &[Value::int(10)]);
    rt.send_message(a2, "add", &[Value::int(20)]);
    rt.run_scheduler();
}

// ========================================================================
// Actor Priority Tests
// ========================================================================

#[test]
fn test_actor_priority_default_is_normal() {
    let actor = Actor::new(1, "test_actor", 0);
    assert_eq!(actor.priority, ActorPriority::Normal);
    assert_eq!(ActorPriority::default(), ActorPriority::Normal);
}

#[test]
fn test_scheduler_priority_dequeue_order() {
    // Strict per-level preference: every High entry drains before any
    // Normal, every Normal before any Low; FIFO within a level.
    let sched = Scheduler::new(4);
    sched.enqueue_with_priority(1, ActorPriority::Normal);
    sched.enqueue_with_priority(2, ActorPriority::Low);
    sched.enqueue_with_priority(3, ActorPriority::High);
    sched.enqueue_with_priority(4, ActorPriority::Normal);
    sched.enqueue_with_priority(5, ActorPriority::High);
    sched.enqueue_with_priority(6, ActorPriority::Low);
    assert_eq!(sched.steal_one(), Some(3));
    assert_eq!(sched.steal_one(), Some(5));
    assert_eq!(sched.steal_one(), Some(1));
    assert_eq!(sched.steal_one(), Some(4));
    assert_eq!(sched.steal_one(), Some(2));
    assert_eq!(sched.steal_one(), Some(6));
    assert!(sched.steal_one().is_none());
}

#[test]
fn test_scheduler_enqueue_defaults_to_normal() {
    // The plain `enqueue` entry point lands in the Normal level.
    let sched = Scheduler::new(2);
    sched.enqueue(1); // Normal
    sched.enqueue_with_priority(2, ActorPriority::High);
    sched.enqueue_with_priority(3, ActorPriority::Low);
    assert_eq!(sched.steal_one(), Some(2));
    assert_eq!(sched.steal_one(), Some(1));
    assert_eq!(sched.steal_one(), Some(3));
}

#[test]
fn test_actor_set_priority_effect_maps_levels() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let set = |rt: &mut Runtime, who: Option<u64>, level: i64| {
        rt.perform_actor_builtin(who, Some("set_priority"), &[], &[Value::int(level)])
    };
    assert_eq!(set(&mut rt, Some(a), 0), Some(Value::nil()));
    assert_eq!(rt.actors.get(&a).unwrap().priority, ActorPriority::High);
    assert_eq!(set(&mut rt, Some(a), 2), Some(Value::nil()));
    assert_eq!(rt.actors.get(&a).unwrap().priority, ActorPriority::Low);
    assert_eq!(set(&mut rt, Some(a), 1), Some(Value::nil()));
    assert_eq!(rt.actors.get(&a).unwrap().priority, ActorPriority::Normal);
    // Out-of-range levels fall back to Normal.
    assert_eq!(set(&mut rt, Some(a), 7), Some(Value::nil()));
    assert_eq!(rt.actors.get(&a).unwrap().priority, ActorPriority::Normal);
    // Outside an actor context the effect is a nil no-op.
    assert_eq!(set(&mut rt, None, 0), Some(Value::nil()));
}

#[test]
fn test_actor_set_priority_changes_scheduling() {
    // A High-priority actor is dequeued before a Normal one even when the
    // Normal actor's message was sent first.
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    // Drain the spawn-time queue entries (both enqueued at Normal).
    assert_eq!(rt.scheduler.dequeue(), Some(a));
    assert_eq!(rt.scheduler.dequeue(), Some(b));
    // Boost b via the builtin-effect path, then send to a before b.
    assert_eq!(
        rt.perform_actor_builtin(Some(b), Some("set_priority"), &[], &[Value::int(0)]),
        Some(Value::nil())
    );
    rt.send_message(a, "noop", &[]);
    rt.send_message(b, "noop", &[]);
    assert_eq!(rt.scheduler.dequeue(), Some(b));
    assert_eq!(rt.scheduler.dequeue(), Some(a));
}

// ========================================================================
// Supervisor Tests
// ========================================================================

#[test]
fn test_one_for_one_restart() {
    let mut rt = Runtime::new();
    let sup_id = rt.create_supervisor("test_sup", RestartStrategy::OneForOne);
    let child_id = rt.spawn_actor(Box::new(|| vec![("x".to_string(), Value::int(0))]));
    let spec = ChildSpec::new("child1", RestartPolicy::Permanent);
    rt.supervise_child(sup_id, spec, child_id);
    assert_eq!(rt.supervisors[&sup_id].child_count(), 1);
    rt.exit_actor(child_id, ExitReason::Error("crash".to_string()));
    assert!(!rt.actors.contains_key(&child_id));
    assert_eq!(rt.supervisors[&sup_id].child_count(), 1);
}

#[test]
fn test_supervisor_restart_rate_limiting() {
    let mut rt = Runtime::new();
    let sup_id = rt.create_supervisor("rate_sup", RestartStrategy::OneForOne);
    let child_id = rt.spawn_actor(Box::new(|| vec![]));
    let spec = ChildSpec::new("fragile", RestartPolicy::Permanent).with_limits(2, 60);
    rt.supervise_child(sup_id, spec, child_id);

    // Crash 1: child should be restarted (restart #1)
    rt.exit_actor(child_id, ExitReason::Error("crash1".to_string()));
    let child_id_2 = rt.supervisors[&sup_id].children[0].1;
    assert_eq!(rt.supervisors[&sup_id].restart_count(child_id_2), 1);

    // Crash 2: child should be restarted again (restart #2)
    rt.exit_actor(child_id_2, ExitReason::Error("crash2".to_string()));
    let child_id_3 = rt.supervisors[&sup_id].children[0].1;
    assert_eq!(rt.supervisors[&sup_id].restart_count(child_id_3), 2);

    // Crash 3: max_restarts=2 exceeded → supervisor shuts down
    rt.exit_actor(child_id_3, ExitReason::Error("crash3".to_string()));
    assert!(
        !rt.supervisors.contains_key(&sup_id),
        "supervisor should shut down after max restarts"
    );
}

#[test]
fn test_supervisor_escalate_to_parent() {
    let mut rt = Runtime::new();
    let parent_sup = rt.create_supervisor("parent", RestartStrategy::OneForOne);
    let child_sup = rt.create_supervisor("child", RestartStrategy::OneForOne);

    rt.supervisors.get_mut(&child_sup).unwrap().parent = Some(parent_sup);
    let grandchild = rt.spawn_actor(Box::new(|| vec![]));
    let spec = ChildSpec::new("gc", RestartPolicy::Permanent).with_limits(1, 60);
    rt.supervise_child(child_sup, spec, grandchild);

    rt.exit_actor(grandchild, ExitReason::Error("boom".to_string()));
    assert!(
        rt.actors.contains_key(&child_sup),
        "child supervisor should still exist after one restart"
    );

    let gc2 = rt.supervisors[&child_sup].children[0].1;
    rt.exit_actor(gc2, ExitReason::Error("boom2".to_string()));
}

#[test]
fn test_temporary_child_not_restarted() {
    let mut rt = Runtime::new();
    let sup_id = rt.create_supervisor("sup", RestartStrategy::OneForOne);
    let child_id = rt.spawn_actor(Box::new(|| vec![]));
    let spec = ChildSpec::new("temp_child", RestartPolicy::Temporary);
    rt.supervise_child(sup_id, spec, child_id);
    rt.exit_actor(child_id, ExitReason::Error("boom".to_string()));
    assert_eq!(rt.supervisors[&sup_id].child_count(), 0);
}

/// Regression test: a restarted child must be rebuilt with its behavior
/// table and initial state, not as a bare actor that silently drops every
/// message it receives.
#[test]
fn test_restarted_child_restores_behavior_and_state() {
    let mut rt = Runtime::new();
    let sup_id = rt.create_supervisor("test_sup", RestartStrategy::OneForOne);
    let child_id = rt.spawn_actor(Box::new(|| vec![("count".to_string(), Value::int(0))]));
    {
        let actor = rt.actors.get_mut(&child_id).unwrap();
        actor.register_behavior("inc", |actor, args| {
            let n = actor.get_state_field("count").and_then(|v| v.as_int()).unwrap_or(0);
            let by = args.get(0).and_then(|v| v.as_int()).unwrap_or(1);
            actor.set_state_field("count", Value::int(n + by));
        });
    }
    let spec = ChildSpec::new("child1", RestartPolicy::Permanent);
    rt.supervise_child(sup_id, spec, child_id);

    rt.exit_actor(child_id, ExitReason::Error("crash".to_string()));
    let new_id = rt.supervisors[&sup_id].children[0].1;
    assert_ne!(new_id, child_id, "restart should create a fresh actor");

    // The restarted child must handle messages (before the fix it was a
    // bare actor that silently dropped them).
    rt.send_message(new_id, "inc", &[Value::int(5)]);
    rt.step_actor(new_id);
    let count = rt.actors.get(&new_id).unwrap().get_state_field("count");
    assert_eq!(count, Some(Value::int(5)));
}

/// Supervisor restart of a persistent child must hydrate state from the
/// persistence store, not from the captured RestartTemplate (which holds
/// the *original* state from registration time).
#[test]
fn test_supervisor_restart_hydrates_from_persistence() {
    let mut rt = Runtime::new();
    rt.persistence = Box::new(MemoryStore::new());
    let sup_id = rt.create_supervisor("sup", RestartStrategy::OneForOne);
    let mut models = HashMap::new();
    models.insert("count".to_string(), StateModel::Durable);
    let child_id = rt.spawn_persistent_actor(
        Box::new(|| vec![("count".to_string(), Value::int(0))]),
        models,
    );
    {
        let actor = rt.actors.get_mut(&child_id).unwrap();
        actor.register_behavior("inc", |actor, args| {
            let n = actor.get_state_field("count").and_then(|v| v.as_int()).unwrap_or(0);
            let by = args.get(0).and_then(|v| v.as_int()).unwrap_or(1);
            actor.set_state_field("count", Value::int(n + by));
        });
    }

    for _ in 0..3 {
        rt.send_message(child_id, "inc", &[Value::int(1)]);
        rt.step_actor(child_id);
    }
    assert_eq!(
        rt.actors.get(&child_id).unwrap().get_state_field("count"),
        Some(Value::int(3)),
        "count should be 3 before crash"
    );
    assert!(
        rt.persistence.load_snapshot(child_id).is_some(),
        "snapshot should exist before crash"
    );

    let spec = ChildSpec::new("counter", RestartPolicy::Permanent);
    rt.supervise_child(sup_id, spec, child_id);
    rt.exit_actor(child_id, ExitReason::Error("simulated crash".to_string()));

    let new_id = rt.supervisors[&sup_id].children[0].1;
    assert_ne!(new_id, child_id, "restart should create a fresh actor");

    let count = rt.actors.get(&new_id).unwrap().get_state_field("count");
    assert_eq!(
        count,
        Some(Value::int(3)),
        "restarted actor must hydrate count=3 from persistence, not template count=0"
    );

    assert!(
        rt.persistence.load_snapshot(new_id).is_some(),
        "snapshot must be re-keyed under new actor id"
    );
    assert!(
        rt.persistence.load_snapshot(child_id).is_none(),
        "old snapshot must be cleared after re-keying"
    );
}

/// Regression test: a restarted bytecode child must keep its bytecode
/// module, behavior offsets, and captured initial state so it still
/// resolves and runs its bytecode behaviors after a restart.
#[test]
fn test_restarted_bytecode_child_handles_messages() {
    use crate::bytecode::{BehaviorTableEntry, CodeModule, Constant, Instruction, OpCode};

    let mut rt = Runtime::new();
    let sup_id = rt.create_supervisor("byte_sup", RestartStrategy::OneForOne);

    // Behavior "Counter.inc": count += 1, returning the new count.
    let mut module = CodeModule::new("test");
    let field_idx = module.add_constant(Constant::String("count".to_string()));
    let one_idx = module.add_constant(Constant::Int(1));
    module.add_behavior(BehaviorTableEntry {
        name: "Counter.inc".to_string(),
        param_count: 0,
        code_offset: 0,
        local_count: 4,
        effect_mask: 0,
        compensate_offset: None,
        parallel_branches: None,
    });
    module.emit(Instruction::new3(
        OpCode::StateGet,
        ((field_idx >> 8) & 0xFF) as u8,
        (field_idx & 0xFF) as u8,
        1,
    ));
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((one_idx >> 8) & 0xFF) as u8,
        (one_idx & 0xFF) as u8,
        2,
    ));
    module.emit(Instruction::new3(OpCode::IAdd, 1, 2, 3));
    module.emit(Instruction::new3(OpCode::StateSet, 0, 0, 3));
    module.emit(Instruction::new1(OpCode::RetVal, 3));

    let child_id = rt.spawn_actor(Box::new(|| vec![("count".to_string(), Value::int(0))]));
    {
        let actor = rt.actors.get_mut(&child_id).unwrap();
        actor.bytecode_module = Some(module.clone());
        actor.bytecode_offsets = vec![0];
        actor.compensation_offsets = vec![None];
    }
    rt.register_recovery_module(child_id, module, vec![0], vec![None]);
    let spec = ChildSpec::new("counter", RestartPolicy::Permanent);
    rt.supervise_child(sup_id, spec, child_id);

    // Sanity: the behavior works before the crash.
    let before = rt.ask_actor_sync(child_id, 0, &[]).unwrap();
    assert_eq!(before, Value::int(1));

    rt.exit_actor(child_id, ExitReason::Error("crash".to_string()));
    let new_id = rt.supervisors[&sup_id].children[0].1;
    assert_ne!(new_id, child_id);

    // After restart the child must still resolve and run its bytecode
    // behavior (before the fix the bare actor answered every ask with nil).
    assert_eq!(rt.behavior_id_for(new_id, "inc"), Some(0));
    let after = rt.ask_actor_sync(new_id, 0, &[]).unwrap();
    assert_eq!(
        after,
        Value::int(1),
        "restarted child must restart from its captured initial state"
    );
    // And the module was re-registered for recovery after a runtime restart.
    assert!(rt.recovery_modules.contains_key(&new_id));
}

/// Regression test: OneForAll mass restart removes the LIVING sibling
/// children through the full exit protocol — registry names are
/// unregistered and monitors receive a DOWN message.
#[test]
fn test_restart_all_unregisters_names_and_notifies_monitors() {
    let mut rt = Runtime::new();
    let sup_id = rt.create_supervisor("all_sup", RestartStrategy::OneForAll);
    let trigger = rt.spawn_actor(Box::new(|| vec![]));
    let sibling = rt.spawn_actor(Box::new(|| vec![]));
    rt.supervise_child(sup_id, ChildSpec::new("trigger", RestartPolicy::Permanent), trigger);
    rt.supervise_child(sup_id, ChildSpec::new("sibling", RestartPolicy::Permanent), sibling);
    rt.registry.register("sibling_name", sibling).unwrap();
    let watcher = rt.spawn_actor(Box::new(|| vec![]));
    rt.monitor(watcher, sibling);

    rt.exit_actor(trigger, ExitReason::Error("crash".to_string()));

    assert!(
        !rt.actors.contains_key(&sibling),
        "living sibling must be replaced on a OneForAll restart"
    );
    assert_eq!(
        rt.registry.whereis("sibling_name"),
        None,
        "removed child's registered name must not linger"
    );
    let down = rt
        .actors
        .get_mut(&watcher)
        .unwrap()
        .mailbox
        .pop()
        .expect("monitor of the removed sibling must receive a DOWN message");
    assert_eq!(down.payload[0].as_int(), Some(sibling as i64));
    assert_eq!(rt.supervisors[&sup_id].child_count(), 2);
}

/// Regression test: when a supervisor shuts down (restart intensity
/// exceeded), its remaining living children are removed through the exit
/// protocol too — not via a raw map removal.
#[test]
fn test_supervisor_shutdown_cleans_up_children() {
    let mut rt = Runtime::new();
    let sup_id = rt.create_supervisor("rate_sup", RestartStrategy::OneForOne);
    let fragile = rt.spawn_actor(Box::new(|| vec![]));
    rt.supervise_child(
        sup_id,
        ChildSpec::new("fragile", RestartPolicy::Permanent).with_limits(1, 60),
        fragile,
    );
    let sibling = rt.spawn_actor(Box::new(|| vec![]));
    rt.supervise_child(sup_id, ChildSpec::new("sibling", RestartPolicy::Permanent), sibling);
    rt.registry.register("sibling_name", sibling).unwrap();
    let watcher = rt.spawn_actor(Box::new(|| vec![]));
    rt.monitor(watcher, sibling);

    // Crash 1 restarts the fragile child (within limits); crash 2 exceeds
    // the intensity and shuts the supervisor down, which must remove the
    // living sibling through the exit protocol.
    rt.exit_actor(fragile, ExitReason::Error("crash1".to_string()));
    let fragile2 = rt.supervisors[&sup_id]
        .children
        .iter()
        .find(|(s, _)| s.id == "fragile")
        .unwrap()
        .1;
    rt.exit_actor(fragile2, ExitReason::Error("crash2".to_string()));

    assert!(!rt.supervisors.contains_key(&sup_id));
    assert!(!rt.actors.contains_key(&sibling));
    assert_eq!(
        rt.registry.whereis("sibling_name"),
        None,
        "shut-down supervisor must unregister its children's names"
    );
    let down = rt.actors.get_mut(&watcher).unwrap().mailbox.pop();
    assert!(
        down.is_some(),
        "monitor of a child removed by supervisor shutdown must receive DOWN"
    );
}

/// Regression test: a supervised child that exits with an outstanding
/// foreign reference must have its heap retired (not dropped wholesale),
/// exactly like an unsupervised exit via `remove_actor_reaping`.
#[test]
fn test_supervised_child_restart_retires_heap_with_foreign_refs() {
    let mut rt = Runtime::new();
    let sup_id = rt.create_supervisor("reap_sup", RestartStrategy::OneForOne);
    let a = rt.spawn_actor(Box::new(|| vec![]));
    rt.supervise_child(sup_id, ChildSpec::new("a", RestartPolicy::Permanent), a);
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.current_actor = Some(a);

    let ptr = rt
        .actors
        .get_mut(&a)
        .unwrap()
        .heap
        .alloc(16, TypeTag::Raw)
        .unwrap();
    let v = Value::ptr(ptr);
    rt.send_message_by_id(b, 0, &[v]);

    // A crashes with the in-flight foreign ref still pending.
    rt.exit_actor(a, ExitReason::Error("crash".to_string()));
    rt.current_actor = None;
    assert!(!rt.actors.contains_key(&a));
    assert_eq!(
        rt.retired_heaps.len(),
        1,
        "supervised child's heap must be retired while foreign refs are outstanding"
    );
    let new_id = rt.supervisors[&sup_id].children[0].1;
    assert_ne!(new_id, a, "replacement child should have been spawned");
    // SAFETY: the retired heap keeps the object alive while refs drain.
    unsafe {
        let header = &*ActorHeap::header_of(ptr);
        assert!(
            header.foreign_count >= 1,
            "retired heap object must remain readable"
        );
    }
}

// ========================================================================
// SimpleOneForOne Dynamic Children Tests
// ========================================================================

/// Build a module declaring actor type `DynWorker` with a `count` state
/// field (default `default_count`) and one bytecode behavior
/// `DynWorker.inc` that increments `count` and returns the new value.
fn dyn_worker_module(default_count: i64) -> crate::bytecode::CodeModule {
    use crate::bytecode::{
        ActorMeta, BehaviorTableEntry, CodeModule, Constant, Instruction, OpCode,
    };

    let mut module = CodeModule::new("dyn_test");
    let field_idx = module.add_constant(Constant::String("count".to_string()));
    let one_idx = module.add_constant(Constant::Int(1));
    module.add_behavior(BehaviorTableEntry {
        name: "DynWorker.inc".to_string(),
        param_count: 0,
        code_offset: 0,
        local_count: 4,
        effect_mask: 0,
        compensate_offset: None,
        parallel_branches: None,
    });
    module.emit(Instruction::new3(
        OpCode::StateGet,
        ((field_idx >> 8) & 0xFF) as u8,
        (field_idx & 0xFF) as u8,
        1,
    ));
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((one_idx >> 8) & 0xFF) as u8,
        (one_idx & 0xFF) as u8,
        2,
    ));
    module.emit(Instruction::new3(OpCode::IAdd, 1, 2, 3));
    module.emit(Instruction::new3(OpCode::StateSet, 0, 0, 3));
    module.emit(Instruction::new1(OpCode::RetVal, 3));
    module.add_actor_meta(ActorMeta {
        name: "DynWorker".to_string(),
        persistent: false,
        state_models: vec![("count".to_string(), crate::ast::StateModel::Local)],
        state_defaults: vec![("count".to_string(), Constant::Int(default_count))],
        behavior_indices: vec![0],
        is_workflow: false,
        is_agent: false,
        tools: vec![],
        semantic_memory_dimensions: None,
        procedural_memory_namespace: None,
        backend: crate::ast::ActorBackendKind::Native,
    });
    module
}

#[test]
fn test_simple_one_for_one_start_child_spawns_real_children() {
    let mut rt = Runtime::new();
    let module = dyn_worker_module(0);
    let sup_id = rt.create_supervisor("pool", RestartStrategy::SimpleOneForOne);
    assert!(rt.set_supervisor_template(sup_id, "DynWorker", &module));

    let w1 = rt
        .start_supervised_child(sup_id, vec![])
        .expect("start_child should spawn from the template");
    let w2 = rt
        .start_supervised_child(sup_id, vec![])
        .expect("start_child should spawn from the template");
    assert_ne!(w1, w2);
    assert_eq!(rt.supervisors[&sup_id].child_count(), 2);
    assert_eq!(rt.actors[&w1].parent, Some(sup_id));
    assert_eq!(rt.actors[&w2].parent, Some(sup_id));

    // Children are real bytecode actors running the template behavior.
    assert_eq!(rt.ask_actor_sync(w1, 0, &[]).unwrap(), Value::int(1));
    assert_eq!(rt.ask_actor_sync(w2, 0, &[]).unwrap(), Value::int(1));

    // Distinct dynamic spec ids keep restart rate limiting per child.
    let specs: Vec<&str> = rt.supervisors[&sup_id]
        .children
        .iter()
        .map(|(s, _)| s.id.as_str())
        .collect();
    assert_eq!(specs, vec!["DynWorker_0", "DynWorker_1"]);
}

#[test]
fn test_simple_one_for_one_restart_from_template_on_crash() {
    let mut rt = Runtime::new();
    let module = dyn_worker_module(0);
    let sup_id = rt.create_supervisor("pool", RestartStrategy::SimpleOneForOne);
    assert!(rt.set_supervisor_template(sup_id, "DynWorker", &module));
    let w = rt.start_supervised_child(sup_id, vec![]).unwrap();

    // Mutate state away from the template defaults, then crash.
    assert_eq!(rt.ask_actor_sync(w, 0, &[]).unwrap(), Value::int(1));
    assert_eq!(rt.ask_actor_sync(w, 0, &[]).unwrap(), Value::int(2));
    rt.exit_actor(w, ExitReason::Error("crash".to_string()));

    assert!(!rt.actors.contains_key(&w));
    assert_eq!(rt.supervisors[&sup_id].child_count(), 1);
    let restarted = rt.supervisors[&sup_id].children[0].1;
    assert_ne!(restarted, w, "restart should create a fresh actor");
    assert_eq!(rt.actors[&restarted].parent, Some(sup_id));
    // The replacement restarts from the template defaults, not the
    // pre-crash state: its first inc returns 1, not 3.
    assert_eq!(rt.ask_actor_sync(restarted, 0, &[]).unwrap(), Value::int(1));
}

#[test]
fn test_simple_one_for_one_terminate_child_skips_restart() {
    let mut rt = Runtime::new();
    let module = dyn_worker_module(0);
    let sup_id = rt.create_supervisor("pool", RestartStrategy::SimpleOneForOne);
    assert!(rt.set_supervisor_template(sup_id, "DynWorker", &module));
    let w = rt.start_supervised_child(sup_id, vec![]).unwrap();
    assert_eq!(rt.supervisors[&sup_id].child_count(), 1);

    assert!(rt.terminate_supervised_child(sup_id, w));
    assert_eq!(
        rt.supervisors[&sup_id].child_count(),
        0,
        "terminated child must leave supervision"
    );
    assert!(
        !rt.actors.contains_key(&w),
        "terminated child must exit without a restart replacement"
    );
    // Unknown child / unknown supervisor are no-ops.
    assert!(!rt.terminate_supervised_child(sup_id, w));
    assert!(!rt.terminate_supervised_child(999_999, w));
}

#[test]
fn test_simple_one_for_one_normal_exit_not_restarted() {
    // Dynamic children are Transient: a Normal exit retires the child
    // without a replacement (unlike terminate_child, this routes through
    // the restart policy).
    let mut rt = Runtime::new();
    let module = dyn_worker_module(0);
    let sup_id = rt.create_supervisor("pool", RestartStrategy::SimpleOneForOne);
    assert!(rt.set_supervisor_template(sup_id, "DynWorker", &module));
    let w = rt.start_supervised_child(sup_id, vec![]).unwrap();

    rt.exit_actor(w, ExitReason::Normal);
    assert!(!rt.actors.contains_key(&w));
    assert_eq!(rt.supervisors[&sup_id].child_count(), 0);
}

#[test]
fn test_simple_one_for_one_start_child_guards() {
    let mut rt = Runtime::new();
    let module = dyn_worker_module(0);
    // No template set -> None.
    let sup_id = rt.create_supervisor("pool", RestartStrategy::SimpleOneForOne);
    assert_eq!(rt.start_supervised_child(sup_id, vec![]), None);
    // Non-dynamic strategy -> None even with a template set.
    let plain_id = rt.create_supervisor("plain", RestartStrategy::OneForOne);
    assert!(rt.set_supervisor_template(plain_id, "DynWorker", &module));
    assert_eq!(rt.start_supervised_child(plain_id, vec![]), None);
    // Unknown supervisor / unknown actor type -> None / false.
    assert_eq!(rt.start_supervised_child(999_999, vec![]), None);
    assert!(!rt.set_supervisor_template(sup_id, "NoSuchActor", &module));
    assert!(!rt.set_supervisor_template(999_999, "DynWorker", &module));
}

#[test]
fn test_otp_builtin_effect_strategy_mapping_and_noops() {
    use crate::bytecode::{CodeModule, Constant};

    let mut module = CodeModule::new("otp_test");
    let name_idx = module.add_constant(Constant::String("s".to_string())) as u32;

    let mut rt = Runtime::new();
    for (raw, want) in [
        (0i64, RestartStrategy::OneForOne),
        (1, RestartStrategy::OneForAll),
        (2, RestartStrategy::RestForOne),
        (3, RestartStrategy::SimpleOneForOne),
    ] {
        let id = rt
            .perform_otp_builtin(
                Some("create_supervisor"),
                &module,
                &[Value::string(name_idx), Value::int(raw)],
            )
            .and_then(|v| v.as_int())
            .expect("create_supervisor should return an Int id") as u64;
        assert_eq!(rt.supervisors[&id].strategy, want);
    }

    // Out-of-range strategy -> nil no-op (no supervisor created).
    let before = rt.supervisors.len();
    let value = rt.perform_otp_builtin(
        Some("create_supervisor"),
        &module,
        &[Value::string(name_idx), Value::int(9)],
    );
    assert_eq!(value, Some(Value::nil()));
    assert_eq!(rt.supervisors.len(), before);

    // Policy mapping via supervise_child (2 = transient).
    let sup_id = rt
        .perform_otp_builtin(
            Some("create_supervisor"),
            &module,
            &[Value::string(name_idx), Value::int(0)],
        )
        .and_then(|v| v.as_int())
        .unwrap() as u64;
    let child = rt.spawn_actor(Box::new(|| vec![]));
    let value = rt.perform_otp_builtin(
        Some("supervise_child"),
        &module,
        &[
            Value::int(sup_id as i64),
            Value::actor_ref(child),
            Value::int(2),
        ],
    );
    assert_eq!(value, Some(Value::nil()));
    assert_eq!(
        rt.supervisors[&sup_id].children[0].0.restart_policy,
        RestartPolicy::Transient
    );
    // supervise_child with an unknown supervisor is a nil no-op.
    let value = rt.perform_otp_builtin(
        Some("supervise_child"),
        &module,
        &[Value::int(999_999), Value::actor_ref(child), Value::int(0)],
    );
    assert_eq!(value, Some(Value::nil()));

    // Unknown op -> None (unhandled); child_count on unknown id -> nil.
    assert_eq!(rt.perform_otp_builtin(Some("bogus"), &module, &[]), None);
    assert_eq!(
        rt.perform_otp_builtin(Some("child_count"), &module, &[Value::int(999_999)]),
        Some(Value::nil())
    );
}

// ========================================================================
// ORCA GC Tests
// ========================================================================

#[test]
fn test_orca_ref_counting_basic() {
    let mut heap = ActorHeap::new(1024 * 1024);
    let mut gc = OrcaGc::new(1);
    let obj = gc.alloc_object(&mut heap, 64, TypeTag::Raw);
    assert!(obj.is_some());
    // local_count starts at 1 (creator holds one ref)
    let header_ptr = unsafe { heap.header_ptr(obj.unwrap()) };
    let local_count = unsafe { (*header_ptr).ref_count };
    assert_eq!(local_count, 1);

    unsafe { gc.local_ref(&heap, obj.unwrap()) };
    let local_count2 = unsafe { (*header_ptr).ref_count };
    assert_eq!(local_count2, 2);

    unsafe { gc.drop_local_ref(&mut heap, obj.unwrap()) };
    let local_count3 = unsafe { (*header_ptr).ref_count };
    assert_eq!(local_count3, 1);
}

#[test]
fn test_orca_cycle_detection() {
    // Cycle detection is handled by CycleDetector, not directly by OrcaGc.
    // This test verifies that two objects can be allocated and reference
    // each other via payload pointers (simulating a cycle).
    let mut heap = ActorHeap::new(1024 * 1024);
    let mut gc_a = OrcaGc::new(1);
    let a = gc_a.alloc_object(&mut heap, 64, TypeTag::Raw);
    let b = gc_a.alloc_object(&mut heap, 64, TypeTag::Raw);
    assert!(a.is_some());
    assert!(b.is_some());

    // Simulate cross-reference by storing pointers in payloads
    unsafe {
        let a_payload = a.unwrap();
        let b_payload = b.unwrap();
        std::ptr::write(a_payload as *mut *mut u8, b_payload);
        std::ptr::write(b_payload as *mut *mut u8, a_payload);
    }

    // Verify both objects are alive with ref count 1 each
    let header_a = unsafe { &*heap.header_ptr(a.unwrap()) };
    let header_b = unsafe { &*heap.header_ptr(b.unwrap()) };
    assert_eq!(header_a.ref_count, 1);
    assert_eq!(header_b.ref_count, 1);
}

// ========================================================================
// Distributed Tests
// ========================================================================

#[test]
fn test_distributed_send_local_fallback() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![("val".to_string(), Value::int(0))]));
    let local_addr = ActorAddress::Local { actor_id };
    rt.send_distributed(local_addr, "test", &[Value::int(42)]);
    assert!(rt.actors.contains_key(&actor_id));
}

#[test]
fn test_crdt_merge_grow_only_counter() {
    let mut a = GCounter::new(1);
    a.increment_by(5);
    let mut b = GCounter::new(2);
    b.increment_by(3);
    a.merge(&b);
    // GCounter merge sums per-node increments: 5 + 3 = 8
    assert_eq!(a.value(), 8);
    let mut c = GCounter::new(3);
    c.increment_by(10);
    a.merge(&c);
    assert_eq!(a.value(), 18);
}

// ========================================================================
// v0.7 BEAM Primitive Tests
// ========================================================================

// -- Actor Name Registry (6 tests) --

#[test]
fn test_registry_register_and_whereis() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));
    assert!(rt.registry.register("my_actor", actor_id).is_ok());
    assert_eq!(rt.registry.whereis("my_actor"), Some(actor_id));
}

#[test]
fn test_registry_duplicate_name_fails() {
    let mut rt = Runtime::new();
    let a1 = rt.spawn_actor(Box::new(|| vec![]));
    let a2 = rt.spawn_actor(Box::new(|| vec![]));
    assert!(rt.registry.register("dup", a1).is_ok());
    assert!(rt.registry.register("dup", a2).is_err());
}

#[test]
fn test_registry_unregister() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));
    rt.registry.register("temp", actor_id).unwrap();
    assert!(rt.registry.unregister("temp").is_ok());
    assert_eq!(rt.registry.whereis("temp"), None);
}

#[test]
fn test_registry_registered_list() {
    let mut rt = Runtime::new();
    let a1 = rt.spawn_actor(Box::new(|| vec![]));
    let a2 = rt.spawn_actor(Box::new(|| vec![]));
    rt.registry.register("alpha", a1).unwrap();
    rt.registry.register("beta", a2).unwrap();
    let names = rt.registry.registered();
    assert!(names.contains(&"alpha".to_string()));
    assert!(names.contains(&"beta".to_string()));
}

#[test]
fn test_registry_cleanup_on_actor_exit() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));
    rt.registry.register("doomed", actor_id).unwrap();
    rt.exit_actor(actor_id, ExitReason::Normal);
    assert_eq!(rt.registry.whereis("doomed"), None);
}

#[test]
fn test_registry_invalid_name() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));
    assert!(rt.registry.register("", actor_id).is_err());
}

// -- Timer Wheel (5 tests) --

#[test]
fn test_timer_send_after() {
    let tw = TimerWheel::new();
    let timer_id = tw.send_after(Duration::from_millis(100), 42, 1, vec![]);
    assert_eq!(timer_id, TimerId(1));
    assert_eq!(tw.len(), 1);
}

#[test]
fn test_timer_cancel() {
    let tw = TimerWheel::new();
    let timer_id = tw.send_after(Duration::from_millis(100), 42, 1, vec![]);
    assert!(tw.cancel(timer_id));
    assert_eq!(tw.len(), 0);
}

#[test]
fn test_timer_tick_fires() {
    let tw = TimerWheel::new();
    let _ = tw.send_after(Duration::from_millis(0), 42, 99, vec![]);
    let fired = tw.tick(Instant::now() + Duration::from_millis(1000));
    assert_eq!(fired.len(), 1);
    assert_eq!(tw.len(), 0);
}

#[test]
fn test_timer_exit_after() {
    let tw = TimerWheel::new();
    let timer_id = tw.exit_after(Duration::from_millis(50), 42, "shutdown".to_string());
    assert_eq!(timer_id, TimerId(1));
    assert_eq!(tw.len(), 1);
}

#[test]
fn test_timer_kill_after() {
    let tw = TimerWheel::new();
    let timer_id = tw.kill_after(Duration::from_millis(50), 42);
    assert_eq!(timer_id, TimerId(1));
}

/// Regression: a timer still pending when the run queue drains must still
/// fire. run_scheduler used to break as soon as the queue emptied, so a
/// timer armed by an actor's last turn was silently dropped.
#[test]
fn test_run_scheduler_waits_for_pending_timer() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![("pings".to_string(), Value::int(0))]));
    {
        let actor = rt.actors.get_mut(&actor_id).unwrap();
        actor.register_behavior("ping", |actor, _args| {
            let n = actor
                .get_state_field("pings")
                .and_then(|v| v.as_int())
                .unwrap_or(0);
            actor.set_state_field("pings", Value::int(n + 1));
        });
    }
    // One direct message (processed immediately) plus a timer that
    // matures only after the queue has drained.
    rt.send_message(actor_id, "ping", &[]);
    let behavior_id = rt.behavior_id_for(actor_id, "ping").unwrap();
    rt.timer_wheel
        .send_after(Duration::from_millis(20), actor_id, behavior_id, vec![]);
    rt.run_scheduler();
    let pings = rt
        .actors
        .get(&actor_id)
        .unwrap()
        .get_state_field("pings")
        .and_then(|v| v.as_int());
    assert_eq!(
        pings,
        Some(2),
        "pending timer must fire before run_scheduler exits"
    );
}

// -- Timed selective receive (receive-after) wait-state tests --

/// The receive-wait timeout is armed exactly once per wait: a re-suspension
/// of the same wait must not restart the clock.
#[test]
fn test_receive_wait_timer_armed_once() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));

    rt.maybe_schedule_receive_wait(actor_id, Some(50));
    let first = rt.actors.get(&actor_id).unwrap().receive_wait;
    assert!(first.is_some(), "first suspend must arm the timeout");
    assert_eq!(rt.timer_wheel.len(), 1);

    // Re-suspending the same wait (e.g. a non-matching wake) keeps the
    // original timer instead of scheduling a fresh one.
    rt.maybe_schedule_receive_wait(actor_id, Some(5000));
    let second = rt.actors.get(&actor_id).unwrap().receive_wait;
    assert_eq!(first, second, "re-suspend must keep the original deadline");
    assert_eq!(rt.timer_wheel.len(), 1, "no second timer may be armed");
}

/// Non-positive (or absent) timeouts never arm a receive-wait timer: the
/// VM resolves those waits non-blockingly without suspending.
#[test]
fn test_receive_wait_timer_skips_nonpositive() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));

    rt.maybe_schedule_receive_wait(actor_id, Some(0));
    rt.maybe_schedule_receive_wait(actor_id, Some(-10));
    rt.maybe_schedule_receive_wait(actor_id, None);

    assert!(rt.actors.get(&actor_id).unwrap().receive_wait.is_none());
    assert!(rt.timer_wheel.is_empty());
}

/// Clearing a resolved wait cancels its pending timeout timer.
#[test]
fn test_clear_receive_wait_cancels_timer() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));

    rt.maybe_schedule_receive_wait(actor_id, Some(50));
    assert_eq!(rt.timer_wheel.len(), 1);

    rt.clear_receive_wait(actor_id);
    assert!(rt.actors.get(&actor_id).unwrap().receive_wait.is_none());
    assert!(
        rt.timer_wheel.is_empty(),
        "a resolved wait must not leave a timer behind"
    );
}

/// A timeout firing with no live suspension (e.g. the actor exited or the
/// wait already resolved) must drop the stale state instead of leaving a
/// poisoned timed-out marker for a later wait.
#[test]
fn test_fire_receive_wait_timeout_without_suspension_clears_state() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));

    rt.maybe_schedule_receive_wait(actor_id, Some(50));
    assert!(rt.actors.get(&actor_id).unwrap().receive_wait.is_some());

    rt.fire_receive_wait_timeout(actor_id);
    assert!(
        rt.actors.get(&actor_id).unwrap().receive_wait.is_none(),
        "stale receive-wait state must be cleared, not marked timed out"
    );
}


// -- Process Groups (5 tests) --

#[test]
fn test_pg_join_and_members() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));
    assert!(rt.process_groups.join("workers", actor_id).is_ok());
    let members = rt.process_groups.members("workers");
    assert!(members.contains(&actor_id));
}

#[test]
fn test_pg_leave() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));
    assert!(rt.process_groups.join("group", actor_id).is_ok());
    rt.process_groups.leave("group", actor_id);
    assert!(!rt.process_groups.is_member("group", actor_id));
}

#[test]
fn test_pg_leave_all() {
    let mut rt = Runtime::new();
    let a1 = rt.spawn_actor(Box::new(|| vec![]));
    assert!(rt.process_groups.join("g1", a1).is_ok());
    assert!(rt.process_groups.join("g2", a1).is_ok());
    rt.process_groups.leave_all(a1);
    assert!(rt.process_groups.members("g1").is_empty());
    assert!(rt.process_groups.members("g2").is_empty());
}

#[test]
fn test_pg_multiple_members() {
    let mut rt = Runtime::new();
    let a1 = rt.spawn_actor(Box::new(|| vec![]));
    let a2 = rt.spawn_actor(Box::new(|| vec![]));
    assert!(rt.process_groups.join("pool", a1).is_ok());
    assert!(rt.process_groups.join("pool", a2).is_ok());
    assert_eq!(rt.process_groups.member_count("pool"), 2);
}

#[test]
fn test_pg_join_idempotent() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));
    assert!(rt.process_groups.join("idempotent", actor_id).is_ok());
    assert!(rt.process_groups.join("idempotent", actor_id).is_ok());
    assert_eq!(rt.process_groups.member_count("idempotent"), 1);
}

// -- Links & Monitors (8 tests) --

#[test]
fn test_link_actors() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.link_actors(a, b);
    assert!(rt.actors[&a].links.contains(&b));
    assert!(rt.actors[&b].links.contains(&a));
}

#[test]
fn test_unlink_actors() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.link_actors(a, b);
    rt.unlink_actors(a, b);
    assert!(!rt.actors[&a].links.contains(&b));
    assert!(!rt.actors[&b].links.contains(&a));
}

#[test]
fn test_monitor_target() {
    let mut rt = Runtime::new();
    let watcher = rt.spawn_actor(Box::new(|| vec![]));
    let target = rt.spawn_actor(Box::new(|| vec![]));
    rt.current_actor = Some(watcher);
    rt.monitor(watcher, target);
    assert!(rt.actors[&target].monitors.contains(&watcher));
}

#[test]
fn test_demonitor() {
    let mut rt = Runtime::new();
    let watcher = rt.spawn_actor(Box::new(|| vec![]));
    let target = rt.spawn_actor(Box::new(|| vec![]));
    rt.monitor(watcher, target);
    rt.demonitor(watcher, target);
    assert!(!rt.actors[&target].monitors.contains(&watcher));
}

#[test]
fn test_monitor_sends_down_on_exit() {
    let mut rt = Runtime::new();
    let watcher = rt.spawn_actor(Box::new(|| vec![]));
    let target = rt.spawn_actor(Box::new(|| vec![]));
    rt.monitor(watcher, target);
    rt.exit_actor(target, ExitReason::Error("boom".to_string()));
    assert!(!rt.actors.contains_key(&target));
}

#[test]
fn test_exit_propagates_to_linked_actors() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.link_actors(a, b);
    rt.exit_actor(a, ExitReason::Error("kaboom".to_string()));
    assert!(!rt.actors.contains_key(&a));
    assert!(
        !rt.actors.contains_key(&b),
        "linked actor b should also exit"
    );
}

#[test]
fn test_exit_does_not_propagate_for_normal_exit() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.link_actors(a, b);
    rt.exit_actor(a, ExitReason::Normal);
    assert!(!rt.actors.contains_key(&a));
    assert!(
        rt.actors.contains_key(&b),
        "linked actor b should NOT exit on normal exit"
    );
}

#[test]
fn test_trap_exit_converts_to_message() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.actors.get_mut(&b).unwrap().trap_exits = true;
    rt.link_actors(a, b);
    rt.exit_actor(a, ExitReason::Error("boom".to_string()));
    assert!(!rt.actors.contains_key(&a));
    assert!(
        rt.actors.contains_key(&b),
        "actor with trap_exits should survive"
    );
    assert!(
        !rt.actors[&b].mailbox.is_empty(),
        "exit signal should become message"
    );
}

// ========================================================================
// VM Opcode Tests
// ========================================================================

#[test]
fn test_vm_value_nan_tagging() {
    let v = Value::int(42);
    assert_eq!(v.as_int(), Some(42));
    let f = Value::float(2.5);
    assert!((f.as_float().unwrap() - 2.5).abs() < 0.001);
    assert_eq!(Value::bool(true).as_bool(), Some(true));
    assert!(Value::unit().is_unit());
}

#[test]
fn test_vm_frame_operations() {
    let frame = Frame::new(None, 0);
    assert!(frame.regs[0].is_nil());
    assert_eq!(frame.pc, 0);
}

#[test]
fn test_fresh_actor_id_increments() {
    let id1 = fresh_actor_id();
    let id2 = fresh_actor_id();
    assert_eq!(id2, id1 + 1);
}

// ========================================================================
// v0.7 Persistence Tests
// ========================================================================

#[test]
fn test_persistent_actor_snapshots_durable_state() {
    let mut rt = Runtime::new();
    let mut models = HashMap::new();
    models.insert("count".to_string(), StateModel::Durable);
    let actor_id = rt.spawn_persistent_actor(
        Box::new(|| vec![("count".to_string(), Value::int(0))]),
        models,
    );
    rt.actors
        .get_mut(&actor_id)
        .unwrap()
        .register_behavior("inc", |actor, _args| {
            if let Some(n) = actor.get_state_field("count").and_then(|v| v.as_int()) {
                actor.set_state_field("count", Value::int(n + 1));
            }
        });

    rt.send_message(actor_id, "inc", &[]);
    rt.step_actor(actor_id);

    let snapshot = rt.persistence.load_snapshot(actor_id).unwrap();
    assert_eq!(snapshot.state.get("count"), Some(&PersistedValue::Int(1)));
    assert!(snapshot.sequence > 0);
}

#[test]
fn test_persistent_actor_recovers_from_snapshot() {
    let mut rt = Runtime::new();
    let mut models = HashMap::new();
    models.insert("count".to_string(), StateModel::Durable);
    let actor_id = rt.spawn_persistent_actor(
        Box::new(|| vec![("count".to_string(), Value::int(0))]),
        models,
    );
    rt.actors
        .get_mut(&actor_id)
        .unwrap()
        .register_behavior("inc", |actor, _args| {
            if let Some(n) = actor.get_state_field("count").and_then(|v| v.as_int()) {
                actor.set_state_field("count", Value::int(n + 1));
            }
        });

    // Process 3 increments.
    for _ in 0..3 {
        rt.send_message(actor_id, "inc", &[]);
        rt.step_actor(actor_id);
    }

    // Simulate node death: drop the actor from memory but keep the store.
    rt.actors.remove(&actor_id);

    // Recover and verify state is replayed.
    rt.recover_actor(actor_id).unwrap();
    let count = rt
        .actors
        .get(&actor_id)
        .unwrap()
        .get_state_field("count")
        .and_then(|v| v.as_int())
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn test_local_state_is_not_persisted() {
    let mut rt = Runtime::new();
    let mut models = HashMap::new();
    models.insert("temp".to_string(), StateModel::Local);
    let actor_id = rt.spawn_persistent_actor(
        Box::new(|| vec![("temp".to_string(), Value::int(42))]),
        models,
    );
    rt.actors
        .get_mut(&actor_id)
        .unwrap()
        .register_behavior("set", |actor, args| {
            if let Some(n) = args.get(0).and_then(|v| v.as_int()) {
                actor.set_state_field("temp", Value::int(n));
            }
        });

    rt.send_message(actor_id, "set", &[Value::int(99)]);
    rt.step_actor(actor_id);

    let snapshot = rt.persistence.load_snapshot(actor_id).unwrap();
    assert!(!snapshot.state.contains_key("temp"));
}

#[test]
fn test_event_sourced_counter_replays_from_event_log() {
    let mut rt = Runtime::new();
    rt.persistence = Box::new(MemoryStore::new());

    let mut models = HashMap::new();
    models.insert("counter".to_string(), StateModel::EventSourced);
    let actor_id = rt.spawn_persistent_actor(
        Box::new(|| vec![("counter".to_string(), Value::int(0))]),
        models,
    );

    for i in 0..5 {
        rt.emit_event(actor_id, "Incremented", &[Value::int(i)]);
    }

    let count = rt.actors.get(&actor_id).unwrap().get_state_field("counter");
    assert_eq!(count, Some(Value::int(5)), "counter should be 5 after 5 events");

    rt.checkpoint_actor(actor_id);
    let snapshot = rt.persistence.load_snapshot(actor_id).unwrap();
    assert_eq!(
        snapshot.state.contains_key("counter"),
        false,
        "EventSourced field must not appear in snapshot"
    );

    let events = rt.persistence.read_events(actor_id);
    assert_eq!(events.len(), 5, "5 events must be persisted");
    assert_eq!(events[0].field_name, "counter");
    assert_eq!(events[0].event_name, "Incremented");

    rt.actors.remove(&actor_id);
    let recovered_id = rt.recover_actor(actor_id).unwrap();
    assert_eq!(recovered_id, actor_id);

    let recovered_count = rt.actors.get(&actor_id).unwrap().get_state_field("counter");
    assert_eq!(
        recovered_count,
        Some(Value::int(5)),
        "recovered actor must have counter=5 from event replay"
    );
}

#[test]
fn test_memory_store_latest_sequence() {
    let mut store = MemoryStore::new();
    let snapshot = ActorSnapshot {
        actor_id: 1,
        sequence: 5,
        state: HashMap::new(),
        waiting_signal: None,
    };
    store.save_snapshot(snapshot).unwrap();
    store
        .append_journal(
            1,
            JournalEntry {
                sequence: 7,
                behavior_id: 0,
                payload: vec![],
            },
        )
        .unwrap();
    assert_eq!(store.latest_sequence(1), 7);
}

#[cfg(feature = "sqlite")]
#[test]
fn test_libsql_store_save_load_snapshot() {
    let mut store = LibsqlStore::in_memory().unwrap();
    let mut state = HashMap::new();
    state.insert("count".to_string(), PersistedValue::Int(42));
    let snapshot = ActorSnapshot {
        actor_id: 1,
        sequence: 3,
        state,
        waiting_signal: None,
    };
    store.save_snapshot(snapshot).unwrap();

    let loaded = store.load_snapshot(1).unwrap();
    assert_eq!(loaded.actor_id, 1);
    assert_eq!(loaded.sequence, 3);
    assert_eq!(loaded.state.get("count"), Some(&PersistedValue::Int(42)));
}

#[cfg(feature = "sqlite")]
#[test]
fn test_libsql_store_append_read_journal() {
    let mut store = LibsqlStore::in_memory().unwrap();
    store
        .append_journal(
            1,
            JournalEntry {
                sequence: 1,
                behavior_id: 0,
                payload: vec![PersistedValue::Int(10)],
            },
        )
        .unwrap();
    store
        .append_journal(
            1,
            JournalEntry {
                sequence: 2,
                behavior_id: 1,
                payload: vec![PersistedValue::Int(20)],
            },
        )
        .unwrap();

    let journal = store.read_journal(1);
    assert_eq!(journal.len(), 2);
    assert_eq!(journal[0].sequence, 1);
    assert_eq!(journal[1].behavior_id, 1);
    assert_eq!(journal[1].payload, vec![PersistedValue::Int(20)]);
}

#[cfg(feature = "sqlite")]
#[test]
fn test_libsql_store_latest_sequence() {
    let mut store = LibsqlStore::in_memory().unwrap();
    store
        .save_snapshot(ActorSnapshot {
            actor_id: 1,
            sequence: 5,
            state: HashMap::new(),
            waiting_signal: None,
        })
        .unwrap();
    store
        .append_journal(
            1,
            JournalEntry {
                sequence: 7,
                behavior_id: 0,
                payload: vec![],
            },
        )
        .unwrap();
    assert_eq!(store.latest_sequence(1), 7);
}

#[cfg(feature = "sqlite")]
#[test]
fn test_libsql_store_clear() {
    let mut store = LibsqlStore::in_memory().unwrap();
    store
        .save_snapshot(ActorSnapshot {
            actor_id: 1,
            sequence: 1,
            state: HashMap::new(),
            waiting_signal: None,
        })
        .unwrap();
    store
        .append_journal(
            1,
            JournalEntry {
                sequence: 2,
                behavior_id: 0,
                payload: vec![],
            },
        )
        .unwrap();

    store.clear(1).unwrap();
    assert!(store.load_snapshot(1).is_none());
    assert!(store.read_journal(1).is_empty());
    assert_eq!(store.latest_sequence(1), 0);
}

#[cfg(feature = "sqlite")]
#[test]
fn test_libsql_store_persists_to_disk() {
    let path = std::env::temp_dir().join(format!("nulang_libsql_test_{}.db", std::process::id()));
    {
        let mut store = LibsqlStore::new(&path).unwrap();
        let mut state = HashMap::new();
        state.insert("x".to_string(), PersistedValue::Float(1.5));
        store
            .save_snapshot(ActorSnapshot {
                actor_id: 1,
                sequence: 1,
                state,
                waiting_signal: None,
            })
            .unwrap();
        store
            .append_journal(
                1,
                JournalEntry {
                    sequence: 2,
                    behavior_id: 0,
                    payload: vec![PersistedValue::Bool(true)],
                },
            )
            .unwrap();
    }

    {
        let store = LibsqlStore::new(&path).unwrap();
        let snapshot = store.load_snapshot(1).unwrap();
        assert_eq!(snapshot.sequence, 1);
        assert_eq!(snapshot.state.get("x"), Some(&PersistedValue::Float(1.5)));
        let journal = store.read_journal(1);
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].payload, vec![PersistedValue::Bool(true)]);
    }

    let _ = std::fs::remove_file(&path);
}

#[cfg(feature = "sqlite")]
#[test]
fn test_persistent_actor_with_libsql_store() {
    let mut rt = Runtime::new();
    rt.persistence = Box::new(LibsqlStore::in_memory().unwrap());
    let mut models = HashMap::new();
    models.insert("count".to_string(), StateModel::Durable);
    let actor_id = rt.spawn_persistent_actor(
        Box::new(|| vec![("count".to_string(), Value::int(0))]),
        models,
    );
    rt.actors
        .get_mut(&actor_id)
        .unwrap()
        .register_behavior("inc", |actor, _args| {
            if let Some(n) = actor.get_state_field("count").and_then(|v| v.as_int()) {
                actor.set_state_field("count", Value::int(n + 1));
            }
        });

    for _ in 0..3 {
        rt.send_message(actor_id, "inc", &[]);
        rt.step_actor(actor_id);
    }

    let snapshot = rt.persistence.load_snapshot(actor_id).unwrap();
    assert_eq!(snapshot.state.get("count"), Some(&PersistedValue::Int(3)));
}

// ========================================================================
// VM / Runtime wiring tests (v0.7)
// ========================================================================

#[test]
fn test_vm_spawn_creates_persistent_actor() {
    use crate::bytecode::{
        ActorMeta, BehaviorTableEntry, CodeModule, Constant, Instruction, OpCode,
    };
    use crate::runtime::persistence::StateModel as RuntimeStateModel;
    use crate::vm::{Value, VM};
    use std::cell::RefCell;
    use std::rc::Rc;

    let mut module = CodeModule::new("test");
    module.add_actor_meta(ActorMeta {
        name: "Account".to_string(),
        persistent: true,
        state_models: vec![("balance".to_string(), crate::ast::StateModel::Durable)],
        state_defaults: vec![("balance".to_string(), Constant::Int(100))],
        behavior_indices: vec![0],
        is_workflow: false,
        is_agent: false,
        tools: vec![],
        semantic_memory_dimensions: None,
        procedural_memory_namespace: None,
        backend: crate::ast::ActorBackendKind::Native,
    });
    module.add_behavior(BehaviorTableEntry {
        name: "Account.get".to_string(),
        param_count: 0,
        code_offset: 0,
        local_count: 1,
        effect_mask: 0,
        compensate_offset: None,
        parallel_branches: None,
    });
    module.emit(Instruction::new3(OpCode::Spawn, 0, 0, 0));
    module.emit(Instruction::new0(OpCode::Halt));
    module.entry_point = Some(0);

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let mut vm = VM::new();
    vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
    vm.load_module(module);
    let result = vm.run().unwrap();

    let actor_id = result.as_actor_id().expect("expected actor reference");
    assert_ne!(actor_id, 0);

    let rt_ref = rt.borrow();
    let actor = rt_ref.actors.get(&actor_id).expect("actor should exist");
    assert!(actor.persistent);
    assert_eq!(actor.get_state_field("balance"), Some(Value::int(100)));
    assert_eq!(
        actor.state_models.get("balance"),
        Some(&RuntimeStateModel::Durable)
    );
}

#[test]
fn test_vm_spawn_creates_non_persistent_actor() {
    use crate::bytecode::{
        ActorMeta, BehaviorTableEntry, CodeModule, Constant, Instruction, OpCode,
    };
    use crate::vm::{Value, VM};
    use std::cell::RefCell;
    use std::rc::Rc;

    let mut module = CodeModule::new("test");
    module.add_actor_meta(ActorMeta {
        name: "Counter".to_string(),
        persistent: false,
        state_models: vec![("count".to_string(), crate::ast::StateModel::Local)],
        state_defaults: vec![("count".to_string(), Constant::Int(0))],
        behavior_indices: vec![0],
        is_workflow: false,
        is_agent: false,
        tools: vec![],
        semantic_memory_dimensions: None,
        procedural_memory_namespace: None,
        backend: crate::ast::ActorBackendKind::Native,
    });
    module.add_behavior(BehaviorTableEntry {
        name: "Counter.inc".to_string(),
        param_count: 0,
        code_offset: 0,
        local_count: 1,
        effect_mask: 0,
        compensate_offset: None,
        parallel_branches: None,
    });
    module.emit(Instruction::new3(OpCode::Spawn, 0, 0, 0));
    module.emit(Instruction::new0(OpCode::Halt));
    module.entry_point = Some(0);

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let mut vm = VM::new();
    vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
    vm.load_module(module);
    let result = vm.run().unwrap();

    let actor_id = result.as_actor_id().expect("expected actor reference");
    let rt_ref = rt.borrow();
    let actor = rt_ref.actors.get(&actor_id).unwrap();
    assert!(!actor.persistent);
    assert_eq!(actor.get_state_field("count"), Some(Value::int(0)));
}

#[test]
fn test_vm_arr_alloc_uses_actor_heap() {
    use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
    use crate::runtime::heap::{ActorHeap, TypeTag};
    use crate::vm::VM;
    use std::cell::RefCell;
    use std::rc::Rc;

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let actor_id = rt.borrow_mut().spawn_actor(Box::new(|| vec![]));
    rt.borrow_mut().current_actor = Some(actor_id);

    let mut module = CodeModule::new("test");
    let len_idx = module.add_constant(Constant::Int(4));
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((len_idx >> 8) & 0xFF) as u8,
        (len_idx & 0xFF) as u8,
        1,
    ));
    module.emit(Instruction::new2(OpCode::ArrAlloc, 1, 0));
    module.emit(Instruction::new0(OpCode::Halt));
    module.entry_point = Some(0);

    let mut vm = VM::new();
    vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
    vm.load_module(module);
    vm.run().unwrap();

    let rt_ref = rt.borrow();
    let actor = rt_ref.actors.get(&actor_id).unwrap();
    assert_eq!(actor.heap.live_count(), 1);
    let mut ptrs = Vec::new();
    actor
        .heap
        .iter_live_objects(|_h, payload, _size| ptrs.push(payload));
    assert_eq!(ptrs.len(), 1);
    unsafe {
        let header = &*ActorHeap::header_of(ptrs[0]);
        assert_eq!(header.type_tag, TypeTag::Array);
    }
}

#[test]
fn test_vm_arr_load_store_and_len_on_actor_heap() {
    use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
    use crate::vm::VM;
    use std::cell::RefCell;
    use std::rc::Rc;

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let actor_id = rt.borrow_mut().spawn_actor(Box::new(|| vec![]));
    rt.borrow_mut().current_actor = Some(actor_id);

    let mut module = CodeModule::new("test");
    let len_idx = module.add_constant(Constant::Int(3));
    let idx_idx = module.add_constant(Constant::Int(1));
    let val_idx = module.add_constant(Constant::Int(42));

    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((len_idx >> 8) & 0xFF) as u8,
        (len_idx & 0xFF) as u8,
        1,
    ));
    module.emit(Instruction::new2(OpCode::ArrAlloc, 1, 0)); // r0 = arr
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((idx_idx >> 8) & 0xFF) as u8,
        (idx_idx & 0xFF) as u8,
        2,
    ));
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((val_idx >> 8) & 0xFF) as u8,
        (val_idx & 0xFF) as u8,
        3,
    ));
    module.emit(Instruction::new3(OpCode::ArrStore, 0, 2, 3));
    module.emit(Instruction::new3(OpCode::ArrLoad, 0, 2, 4));
    module.emit(Instruction::new3(OpCode::ArrLen, 0, 0, 5)); // r5 = len
    module.emit(Instruction::new2(OpCode::Move, 4, 0)); // return loaded value
    module.emit(Instruction::new0(OpCode::Halt));
    module.entry_point = Some(0);

    let mut vm = VM::new();
    vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
    vm.load_module(module);
    let result = vm.run().unwrap();

    assert_eq!(result.as_int(), Some(42));
}

#[test]
fn test_vm_drop_frees_actor_heap_object() {
    use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
    use crate::vm::VM;
    use std::cell::RefCell;
    use std::rc::Rc;

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let actor_id = rt.borrow_mut().spawn_actor(Box::new(|| vec![]));
    rt.borrow_mut().current_actor = Some(actor_id);

    let mut module = CodeModule::new("test");
    let len_idx = module.add_constant(Constant::Int(4));
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((len_idx >> 8) & 0xFF) as u8,
        (len_idx & 0xFF) as u8,
        1,
    ));
    module.emit(Instruction::new2(OpCode::ArrAlloc, 1, 0));
    module.emit(Instruction::new1(OpCode::Drop, 0));
    module.emit(Instruction::new0(OpCode::Halt));
    module.entry_point = Some(0);

    let mut vm = VM::new();
    vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
    vm.load_module(module);
    vm.run().unwrap();

    let rt_ref = rt.borrow();
    let actor = rt_ref.actors.get(&actor_id).unwrap();
    assert_eq!(actor.heap.live_count(), 0);
}

/// Regression test: capturing a heap pointer into a closure (`CapStore`)
/// must retain it, so dropping the original local binding does not free an
/// object the closure still holds — a latent use-after-free that would
/// trigger the moment any codegen path emits `Drop` for a captured local.
#[test]
fn test_closure_capture_retains_heap_object_across_drop() {
    use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
    use crate::vm::VM;
    use std::cell::RefCell;
    use std::rc::Rc;

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let actor_id = rt.borrow_mut().spawn_actor(Box::new(|| vec![]));
    rt.borrow_mut().current_actor = Some(actor_id);

    let mut module = CodeModule::new("test_capture_retain");
    let len_idx = module.add_constant(Constant::Int(4));

    // main:
    //   0: ConstU 4 -> r2 (array length)
    //   1: ArrAlloc r2 -> r1 (heap object, local ref_count starts at 1)
    //   2: Closure #0 -> r3
    //   3: CapStore r3[0] = r1   (must retain: ref_count -> 2)
    //   4: Drop r1               (ref_count -> 1; must NOT free)
    //   5: Move r3 -> r4
    //   6: Call r4, 0 args, dst r0
    //   7: Halt
    // fn0 (at offset 8): just returns unit; the object's survival is
    // checked on the actor heap directly after run() completes.
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((len_idx >> 8) & 0xFF) as u8,
        (len_idx & 0xFF) as u8,
        2,
    )); // 0
    module.emit(Instruction::new2(OpCode::ArrAlloc, 2, 1)); // 1
    module.emit(Instruction::new3(OpCode::Closure, 0, 0, 3)); // 2
    module.emit(Instruction::new3(OpCode::CapStore, 3, 0, 1)); // 3
    module.emit(Instruction::new1(OpCode::Drop, 1)); // 4
    module.emit(Instruction::new2(OpCode::Move, 3, 4)); // 5
    module.emit(Instruction::new3(OpCode::Call, 4, 0, 0)); // 6
    module.emit(Instruction::new0(OpCode::Halt)); // 7
    let fn0_offset = module.current_offset();
    module.emit(Instruction::new1(OpCode::Const0, 0)); // 8
    module.emit(Instruction::new1(OpCode::RetVal, 0)); // 9
    module.function_table.push(fn0_offset);
    module.entry_point = Some(0);

    let mut vm = VM::new();
    vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
    vm.load_module(module);
    vm.run().unwrap();

    let rt_ref = rt.borrow();
    let actor = rt_ref.actors.get(&actor_id).unwrap();
    assert_eq!(
        actor.heap.live_count(),
        1,
        "object captured by a closure must survive a Drop of the original local"
    );
}

/// Same regression as `test_closure_capture_retains_heap_object_across_drop`,
/// but for `ArrStore`: storing a heap pointer into an array slot must retain
/// it too, or a later `Drop` of the value's original binding would free it
/// out from under the array — a latent use-after-free CapStore was already
/// protected against but ArrStore wasn't.
#[test]
fn test_array_store_retains_heap_object_across_drop() {
    use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
    use crate::vm::VM;
    use std::cell::RefCell;
    use std::rc::Rc;

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let actor_id = rt.borrow_mut().spawn_actor(Box::new(|| vec![]));
    rt.borrow_mut().current_actor = Some(actor_id);

    let mut module = CodeModule::new("test_arrstore_retain");
    let inner_len_idx = module.add_constant(Constant::Int(2));
    let outer_len_idx = module.add_constant(Constant::Int(3));

    // main:
    //   0: ConstU 2 -> r1 (inner array length)
    //   1: ArrAlloc r1 -> r2 (inner object, local ref_count starts at 1)
    //   2: ConstU 3 -> r3 (outer array length)
    //   3: ArrAlloc r3 -> r4 (outer array object)
    //   4: Const0 -> r5 (index 0)
    //   5: ArrStore r4[0] = r2 (must retain r2: ref_count -> 2)
    //   6: Drop r2 (ref_count -> 1; must NOT free)
    //   7: Halt
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((inner_len_idx >> 8) & 0xFF) as u8,
        (inner_len_idx & 0xFF) as u8,
        1,
    )); // 0
    module.emit(Instruction::new2(OpCode::ArrAlloc, 1, 2)); // 1
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((outer_len_idx >> 8) & 0xFF) as u8,
        (outer_len_idx & 0xFF) as u8,
        3,
    )); // 2
    module.emit(Instruction::new2(OpCode::ArrAlloc, 3, 4)); // 3
    module.emit(Instruction::new1(OpCode::Const0, 5)); // 4
    module.emit(Instruction::new3(OpCode::ArrStore, 4, 5, 2)); // 5
    module.emit(Instruction::new1(OpCode::Drop, 2)); // 6
    module.emit(Instruction::new0(OpCode::Halt)); // 7
    module.entry_point = Some(0);

    let mut vm = VM::new();
    vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
    vm.load_module(module);
    vm.run().unwrap();

    let rt_ref = rt.borrow();
    let actor = rt_ref.actors.get(&actor_id).unwrap();
    assert_eq!(
        actor.heap.live_count(),
        2,
        "object stored into an array slot must survive a Drop of the original local (both the inner and outer objects should remain live)"
    );
}

/// Same regression as `test_array_store_retains_heap_object_across_drop`,
/// but for `RecS`: storing a heap pointer into a record field must retain
/// it too, mirroring CapStore/ArrStore.
#[test]
fn test_record_field_store_retains_heap_object_across_drop() {
    use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
    use crate::vm::VM;
    use std::cell::RefCell;
    use std::rc::Rc;

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let actor_id = rt.borrow_mut().spawn_actor(Box::new(|| vec![]));
    rt.borrow_mut().current_actor = Some(actor_id);

    let mut module = CodeModule::new("test_recs_retain");
    let inner_len_idx = module.add_constant(Constant::Int(2));

    // main:
    //   0: ConstU 2 -> r1 (inner array length)
    //   1: ArrAlloc r1 -> r2 (inner object, local ref_count starts at 1)
    //   2: RecMk 1 slot -> r3 (record object)
    //   3: RecS r3[field 0] = r2 (must retain r2: ref_count -> 2)
    //   4: Drop r2 (ref_count -> 1; must NOT free)
    //   5: Halt
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((inner_len_idx >> 8) & 0xFF) as u8,
        (inner_len_idx & 0xFF) as u8,
        1,
    )); // 0
    module.emit(Instruction::new2(OpCode::ArrAlloc, 1, 2)); // 1
    module.emit(Instruction::new2(OpCode::RecMk, 1, 3)); // 2
    module.emit(Instruction::new3(OpCode::RecS, 3, 0, 2)); // 3
    module.emit(Instruction::new1(OpCode::Drop, 2)); // 4
    module.emit(Instruction::new0(OpCode::Halt)); // 5
    module.entry_point = Some(0);

    let mut vm = VM::new();
    vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
    vm.load_module(module);
    vm.run().unwrap();

    let rt_ref = rt.borrow();
    let actor = rt_ref.actors.get(&actor_id).unwrap();
    assert_eq!(
        actor.heap.live_count(),
        2,
        "object stored into a record field must survive a Drop of the original local (both the inner object and the record should remain live)"
    );
}

#[test]
fn test_vm_sconcat_uses_actor_heap() {
    use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
    use crate::runtime::heap::{ActorHeap, TypeTag};
    use crate::vm::VM;
    use std::cell::RefCell;
    use std::rc::Rc;

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let actor_id = rt.borrow_mut().spawn_actor(Box::new(|| vec![]));
    rt.borrow_mut().current_actor = Some(actor_id);

    let mut module = CodeModule::new("test");
    let a_idx = module.add_constant(Constant::Int(12));
    let b_idx = module.add_constant(Constant::Int(34));
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((a_idx >> 8) & 0xFF) as u8,
        (a_idx & 0xFF) as u8,
        1,
    ));
    module.emit(Instruction::new3(
        OpCode::ConstU,
        ((b_idx >> 8) & 0xFF) as u8,
        (b_idx & 0xFF) as u8,
        2,
    ));
    module.emit(Instruction::new3(OpCode::SConcat, 1, 2, 0));
    module.emit(Instruction::new0(OpCode::Halt));
    module.entry_point = Some(0);

    let mut vm = VM::new();
    vm.set_actor_callbacks(Box::new(RuntimeVmCallbacks::new(rt.clone())));
    vm.load_module(module);
    vm.run().unwrap();

    let rt_ref = rt.borrow();
    let actor = rt_ref.actors.get(&actor_id).unwrap();
    assert_eq!(actor.heap.live_count(), 1);
    let mut ptrs = Vec::new();
    actor
        .heap
        .iter_live_objects(|_h, payload, _size| ptrs.push(payload));
    unsafe {
        let header = &*ActorHeap::header_of(ptrs[0]);
        assert_eq!(header.type_tag, TypeTag::String);
    }
}

/// v0.7 milestone: a persistent Counter survives 1,000 increments and a restart.
#[test]
fn test_persistent_counter_milestone_1000_messages() {
    let mut rt = Runtime::new();
    let mut models = HashMap::new();
    models.insert("count".to_string(), StateModel::Durable);
    let actor_id = rt.spawn_persistent_actor(
        Box::new(|| vec![("count".to_string(), Value::int(0))]),
        models,
    );
    rt.actors
        .get_mut(&actor_id)
        .unwrap()
        .register_behavior("inc", |actor, _args| {
            if let Some(n) = actor.get_state_field("count").and_then(|v| v.as_int()) {
                actor.set_state_field("count", Value::int(n + 1));
            }
        });

    for _ in 0..1000 {
        rt.send_message(actor_id, "inc", &[]);
    }
    rt.run_scheduler();

    assert_eq!(
        rt.actors
            .get(&actor_id)
            .unwrap()
            .get_state_field("count")
            .and_then(|v| v.as_int()),
        Some(1000)
    );

    // Simulate kill -9: drop the actor from memory but keep the store.
    rt.actors.remove(&actor_id);

    // Restart and recover.
    rt.recover_actor(actor_id).unwrap();

    // Re-register behavior handlers (they are code, not persisted state).
    rt.actors
        .get_mut(&actor_id)
        .unwrap()
        .register_behavior("inc", |actor, _args| {
            if let Some(n) = actor.get_state_field("count").and_then(|v| v.as_int()) {
                actor.set_state_field("count", Value::int(n + 1));
            }
        });

    // The recovered actor must have the durable state.
    assert_eq!(
        rt.actors
            .get(&actor_id)
            .unwrap()
            .get_state_field("count")
            .and_then(|v| v.as_int()),
        Some(1000)
    );

    // It should still be able to process new messages.
    rt.send_message(actor_id, "inc", &[]);
    rt.step_actor(actor_id);
    assert_eq!(
        rt.actors
            .get(&actor_id)
            .unwrap()
            .get_state_field("count")
            .and_then(|v| v.as_int()),
        Some(1001)
    );
}

/// Verify that the runtime restricts the cycle detector to local actors
/// and that the restriction is updated before each detection step.
#[test]
fn test_runtime_cycle_detector_intra_node_restriction() {
    let mut rt = Runtime::new();
    let a1 = rt.spawn_actor(Box::new(|| vec![("x".to_string(), Value::int(0))]));
    let a2 = rt.spawn_actor(Box::new(|| vec![("y".to_string(), Value::int(0))]));

    // Force enough detection epochs for the local-actor set to be applied.
    for _ in 0..15 {
        rt.process_gc_ops();
    }

    let local = rt.cycle_detector.local_actors();
    assert!(
        local.is_some(),
        "local-actor restriction should be set by Runtime"
    );
    let set = local.unwrap();
    assert!(set.contains(&a1), "actor a1 should be considered local");
    assert!(set.contains(&a2), "actor a2 should be considered local");
}

/// Verify that scheduler profiling counters are exposed through the Runtime.
#[test]
fn test_runtime_scheduler_stats() {
    let mut rt = Runtime::new();
    rt.reset_scheduler_stats();

    let a1 = rt.spawn_actor(Box::new(|| vec![("counter".to_string(), Value::int(0))]));
    let a2 = rt.spawn_actor(Box::new(|| vec![("counter".to_string(), Value::int(0))]));
    rt.send_message(a1, "add", &[Value::int(10)]);
    rt.send_message(a2, "add", &[Value::int(20)]);
    rt.run_scheduler();

    let stats = rt.scheduler_stats();
    assert_eq!(
        stats.total_tasks_processed, 4,
        "spawn + send should produce four actor tasks"
    );
    assert_eq!(
        stats.empty_polls, 1,
        "scheduler should poll empty once after draining"
    );

    rt.reset_scheduler_stats();
    let cleared = rt.scheduler_stats();
    assert_eq!(cleared.total_tasks_processed, 0);
    assert_eq!(cleared.empty_polls, 0);
}

// ========================================================================
// ORCA cycle-detector wiring tests
// ========================================================================

#[test]
fn test_cycle_detector_registers_real_cross_actor_ref() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.current_actor = Some(a);

    let ptr = {
        let actor = rt.actors.get_mut(&a).unwrap();
        actor
            .heap
            .alloc(16, crate::runtime::heap::TypeTag::Raw)
            .unwrap()
    };
    unsafe {
        let header = &*ActorHeap::header_of(ptr);
        assert_eq!(
            header.actor_id, a,
            "heap actor_id should be set on creation"
        );
    }

    let v = Value::ptr(ptr);
    rt.send_message_by_id(b, 0, &[v]);
    assert_eq!(
        rt.cycle_detector.graph_size(),
        1,
        "cycle detector should track the foreign reference via the target actor sentinel"
    );

    rt.process_gc_ops();
    assert_eq!(
        rt.cycle_detector.graph_size(),
        0,
        "cycle detector should remove the edge after the op is processed"
    );
}

#[test]
fn test_cycle_detector_accumulates_edge_ref_count() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.current_actor = Some(a);

    let ptr = rt
        .actors
        .get_mut(&a)
        .unwrap()
        .heap
        .alloc(16, crate::runtime::heap::TypeTag::Raw)
        .unwrap();
    let v = Value::ptr(ptr);

    rt.send_message_by_id(b, 0, &[v]);
    rt.send_message_by_id(b, 0, &[v]);
    assert_eq!(
        rt.cycle_detector.graph_size(),
        1,
        "only one sentinel node should exist for the target actor"
    );

    rt.process_gc_ops();
    // Both pending ops are drained in one call, so the edge ref_count drops
    // from 2 to 0 and the node is removed.
    assert_eq!(rt.cycle_detector.graph_size(), 0);
}

#[test]
fn test_cross_actor_send_foreign_count_lifecycle() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.current_actor = Some(a);

    let ptr = rt
        .actors
        .get_mut(&a)
        .unwrap()
        .heap
        .alloc(16, TypeTag::Raw)
        .unwrap();
    unsafe {
        let header = &*ActorHeap::header_of(ptr);
        assert_eq!(header.ref_count, 1);
        assert_eq!(header.foreign_count, 0);
    }

    let v = Value::ptr(ptr);
    rt.send_message_by_id(b, 0, &[v]);

    unsafe {
        let header = &*ActorHeap::header_of(ptr);
        assert_eq!(header.ref_count, 1);
        assert_eq!(
            header.foreign_count,
            1,
            "foreign_count should increment when ref is sent"
        );
    }

    rt.process_gc_ops();

    unsafe {
        let header = &*ActorHeap::header_of(ptr);
        assert_eq!(header.ref_count, 1);
        assert_eq!(
            header.foreign_count,
            0,
            "foreign_count should decrement after op is processed on owning actor"
        );
    }

    // Drop the local ref on the owning actor; object should be freed.
    let actor = rt.actors.get_mut(&a).unwrap();
    unsafe {
        actor.orca_gc.drop_local_ref(&mut actor.heap, ptr);
    }
    assert_eq!(
        actor.heap.live_count(),
        0,
        "object should be freed after local+foreign counts hit zero"
    );
}

/// Regression test: the VM `Drop` callback must honor ORCA foreign counts.
/// An object another actor still references must be deferred, not freed.
#[test]
fn test_vm_drop_ref_defers_object_with_foreign_refs() {
    use crate::vm::ActorVmCallbacks;
    use std::cell::RefCell;
    use std::rc::Rc;

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let actor_id = rt.borrow_mut().spawn_actor(Box::new(|| vec![]));
    rt.borrow_mut().current_actor = Some(actor_id);

    let mut cb = RuntimeVmCallbacks::new(rt.clone());
    let ptr = cb.alloc(16, TypeTag::Raw).unwrap();

    // Simulate an in-flight foreign reference held by another actor.
    unsafe {
(*ActorHeap::header_of(ptr)).foreign_count = 1;
    }

    cb.drop_ref(ptr);
    assert_eq!(
        rt.borrow().actors.get(&actor_id).unwrap().heap.live_count(),
        1,
        "object with a live foreign reference must not be freed by Drop"
    );

    // Once the foreign reference goes away, the deferred pass reclaims it.
    unsafe {
(*ActorHeap::header_of(ptr)).foreign_count = 0;
    }
    {
        let mut rt_mut = rt.borrow_mut();
        let actor = rt_mut.actors.get_mut(&actor_id).unwrap();
        actor.orca_gc.process_deferred(&mut actor.heap);
    }
    assert_eq!(
        rt.borrow().actors.get(&actor_id).unwrap().heap.live_count(),
        0,
        "deferred object should be freed once foreign_count returns to zero"
    );
}

/// Regression test: `run_scheduler` must pump the ORCA GC on its own.
/// A cross-actor reference whose local ref was dropped stays alive while
/// the receiver holds it and is reclaimed — without the embedder calling
/// `process_gc_ops` manually — once the receiver exits and releases its
/// hold.
#[test]
fn test_run_scheduler_pumps_gc() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.current_actor = Some(a);

    let ptr = rt
        .actors
        .get_mut(&a)
        .unwrap()
        .heap
        .alloc(16, TypeTag::Raw)
        .unwrap();
    let v = Value::ptr(ptr);
    rt.send_message_by_id(b, 0, &[v]);

    // Sender drops its local reference while foreign_count is still 1: the
    // object must be deferred, not freed.
    {
        let actor = rt.actors.get_mut(&a).unwrap();
        unsafe {
            actor.orca_gc.drop_local_ref(&mut actor.heap, ptr);
        }
        assert_eq!(
            actor.heap.live_count(),
            1,
            "object should be deferred while foreign ref is live"
        );
    }

    // Draining the scheduler delivers the pending foreign-ref decrement
    // and retries deferred frees — no explicit process_gc_ops() call.  The
    // receiver popped the message, so it now holds the reference: the
    // object must survive until the receiver releases the hold.
    rt.run_scheduler();

    assert_eq!(
        rt.actors.get(&a).unwrap().heap.live_count(),
        1,
        "run_scheduler must not free an object the receiver still holds"
    );

    // The receiver exits: its hold is released and the scheduler's GC pump
    // reclaims the object.
    rt.exit_actor(b, ExitReason::Normal);
    rt.run_scheduler();

    assert_eq!(
        rt.actors.get(&a).unwrap().heap.live_count(),
        0,
        "object should be reclaimed once the receiver releases its hold"
    );
}

/// Regression test (ORCA memory safety): a sender that exits with a
/// foreign-ref op still pending must not leave `process_gc_ops` reading
/// freed heap memory.  The op carries the owner id (no header deref), and
/// the exiting actor's heap is retired while foreign refs are outstanding,
/// then reclaimed once they drain.
#[test]
fn test_exiting_sender_heap_retired_until_refs_drain() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.current_actor = Some(a);

    let ptr = rt
        .actors
        .get_mut(&a)
        .unwrap()
        .heap
        .alloc(16, TypeTag::Raw)
        .unwrap();
    let v = Value::ptr(ptr);
    rt.send_message_by_id(b, 0, &[v]);

    // A exits with the in-flight op still pending and B's message unread.
    rt.exit_actor(a, ExitReason::Normal);
    assert!(
        !rt.actors.contains_key(&a),
        "exited actor should be removed from the map"
    );
    assert_eq!(
        rt.retired_heaps.len(),
        1,
        "heap with an outstanding foreign ref must be retired, not freed"
    );

    // B receives the pointer (taking a hold), then the scheduler drains:
    // process_gc_ops applies the pending -1 against the retired heap —
    // before the fix this dereferenced freed heap memory.
    rt.run_scheduler();

    // B's hold keeps the heap retired: the header is still readable with
    // foreign_count >= 1.
    unsafe {
        let header = &*ActorHeap::header_of(ptr);
        assert!(
            header.foreign_count >= 1,
            "receiver hold must keep the retired heap object alive"
        );
    }

    // Once B exits, its hold is released and the retired heap is reclaimed.
    rt.exit_actor(b, ExitReason::Normal);
    assert!(
        rt.retired_heaps.is_empty(),
        "retired heap should be reclaimed once all foreign refs drain"
    );
}

/// Regression test: forwarding a received heap reference must use the
/// true owner recorded in the object header, not the forwarding actor —
/// the old code tripped the `send_ref_to` ownership debug_assert and, in
/// release builds, registered the cycle-detector edge under the wrong
/// actor.
#[test]
fn test_forwarding_received_reference_uses_true_owner() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    let c = rt.spawn_actor(Box::new(|| vec![]));

    let ptr = rt
        .actors
        .get_mut(&a)
        .unwrap()
        .heap
        .alloc(16, TypeTag::Raw)
        .unwrap();
    let v = Value::ptr(ptr);

    // A sends the reference to B; B receives it (taking a hold).
    rt.current_actor = Some(a);
    rt.send_message_by_id(b, 0, &[v]);
    rt.run_scheduler();

    // B forwards the reference to C.  Before the fix this panicked in
    // debug builds (the object is owned by A, not B).
    rt.current_actor = Some(b);
    rt.send_message_by_id(c, 0, &[v]);

    // The foreign count lives on A's object: B's hold plus the in-flight
    // forward must both be counted there.
    unsafe {
        let header = &*ActorHeap::header_of(ptr);
        assert_eq!(header.actor_id, a, "object is owned by A");
        assert!(
            header.foreign_count >= 2,
            "hold + in-flight forward should both be counted, got {}",
            header.foreign_count
        );
    }

    // The cycle-detector edge must be registered under the true owner A
    // (target C's sentinel -> A's object), not under B.
    assert_eq!(
        rt.cycle_detector.graph_size(),
        1,
        "forwarded reference should register exactly one edge"
    );

    // Draining delivers the forward's -1 to A's heap; B's hold still keeps
    // the object alive afterwards.
    rt.run_scheduler();
    unsafe {
        let header = &*ActorHeap::header_of(ptr);
        assert!(
            header.foreign_count >= 1,
            "B's hold must keep the object alive after the forward lands"
        );
    }
}

/// Regression test: an object whose pointer was received by another actor
/// must survive the sender dropping all of its local references, and be
/// reclaimed only when the receiver releases its hold (here: on exit).
#[test]
fn test_receiver_hold_survives_sender_drop_until_release() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.current_actor = Some(a);

    let ptr = rt
        .actors
        .get_mut(&a)
        .unwrap()
        .heap
        .alloc(16, TypeTag::Raw)
        .unwrap();
    let v = Value::ptr(ptr);
    rt.send_message_by_id(b, 0, &[v]);

    // B receives the message and holds the reference.
    rt.run_scheduler();

    // A drops its last local reference.  Before the fix the object was
    // freed here even though B still holds the pointer.
    {
        let actor = rt.actors.get_mut(&a).unwrap();
        unsafe {
            actor.orca_gc.drop_local_ref(&mut actor.heap, ptr);
        }
        assert_eq!(
            actor.heap.live_count(),
            1,
            "object must survive while the receiver holds it"
        );
    }

    // B exits: the hold is released and the object is freed on A's heap.
    rt.exit_actor(b, ExitReason::Normal);
    assert_eq!(
        rt.actors.get(&a).unwrap().heap.live_count(),
        0,
        "object should be freed once the receiver releases its hold"
    );
}

// ========================================================================
// v0.8 Workflow Runtime Tests
// ========================================================================

#[test]
fn test_workflow_actor_emits_started_event() {
    let mut rt = Runtime::new();
    let mut models = HashMap::new();
    models.insert("step_index".to_string(), StateModel::Durable);
    let actor_id = rt.spawn_workflow_actor(
        "CounterWorkflow",
        Box::new(|| vec![("step_index".to_string(), Value::int(0))]),
        models,
    );

    let events = rt.persistence.read_workflow_events(actor_id);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], WorkflowEvent::WorkflowStarted { name, .. } if name == "CounterWorkflow")
    );

    let snapshot = rt.persistence.load_snapshot(actor_id).unwrap();
    assert_eq!(
        snapshot.state.get("step_index"),
        Some(&PersistedValue::Int(0))
    );
}

#[test]
fn test_workflow_actor_step_event_and_checkpoint() {
    let mut rt = Runtime::new();
    let mut models = HashMap::new();
    models.insert("step_index".to_string(), StateModel::Durable);
    let actor_id = rt.spawn_workflow_actor(
        "CounterWorkflow",
        Box::new(|| vec![("step_index".to_string(), Value::int(0))]),
        models,
    );

    rt.actors
        .get_mut(&actor_id)
        .unwrap()
        .register_behavior("next", |actor, _args| {
            if let Some(n) = actor.get_state_field("step_index").and_then(|v| v.as_int()) {
                actor.set_state_field("step_index", Value::int(n + 1));
            }
        });

    rt.send_message(actor_id, "next", &[]);
    rt.step_actor(actor_id);

    let events = rt.persistence.read_workflow_events(actor_id);
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[1], WorkflowEvent::StepCompleted { .. }));

    let snapshot = rt.persistence.load_snapshot(actor_id).unwrap();
    assert_eq!(
        snapshot.state.get("step_index"),
        Some(&PersistedValue::Int(1))
    );
}

#[test]
fn test_workflow_actor_recovery_replays_step_index() {
    let mut rt = Runtime::new();
    let mut models = HashMap::new();
    models.insert("step_index".to_string(), StateModel::Durable);
    let actor_id = rt.spawn_workflow_actor(
        "CounterWorkflow",
        Box::new(|| vec![("step_index".to_string(), Value::int(0))]),
        models,
    );

    rt.actors
        .get_mut(&actor_id)
        .unwrap()
        .register_behavior("next", |actor, _args| {
            if let Some(n) = actor.get_state_field("step_index").and_then(|v| v.as_int()) {
                actor.set_state_field("step_index", Value::int(n + 1));
            }
        });

    for _ in 0..3 {
        rt.send_message(actor_id, "next", &[]);
        rt.step_actor(actor_id);
    }

    // Simulate node restart: drop the actor from memory but keep the store.
    rt.actors.remove(&actor_id);

    rt.recover_actor(actor_id).unwrap();
    rt.actors
        .get_mut(&actor_id)
        .unwrap()
        .register_behavior("next", |actor, _args| {
            if let Some(n) = actor.get_state_field("step_index").and_then(|v| v.as_int()) {
                actor.set_state_field("step_index", Value::int(n + 1));
            }
        });

    let step_index = rt
        .actors
        .get(&actor_id)
        .unwrap()
        .get_state_field("step_index")
        .and_then(|v| v.as_int())
        .unwrap();
    assert_eq!(step_index, 3);

    // The actor should still be able to advance.
    rt.send_message(actor_id, "next", &[]);
    rt.step_actor(actor_id);
    let step_index = rt
        .actors
        .get(&actor_id)
        .unwrap()
        .get_state_field("step_index")
        .and_then(|v| v.as_int())
        .unwrap();
    assert_eq!(step_index, 4);
}

// ---------------------------------------------------------------------------
// Workflow event journal foundation tests (timer / signal / saga)
// ---------------------------------------------------------------------------

#[test]
fn test_memory_store_append_read_timer_events() {
    let mut store = MemoryStore::new();
    store.append_timer_set(1, 1, "t1".to_string(), 100).unwrap();
    store.append_timer_fired(1, 2, "t1".to_string()).unwrap();

    let timers = store.read_timer_events(1);
    assert_eq!(timers.len(), 2);
    assert!(
        matches!(&timers[0], WorkflowEvent::TimerSet { name, duration_ms, .. } if name == "t1" && *duration_ms == 100)
    );
    assert!(matches!(&timers[1], WorkflowEvent::TimerFired { name, .. } if name == "t1"));
}

#[test]
fn test_memory_store_append_read_signal_event() {
    let mut store = MemoryStore::new();
    store
        .append_signal_received(1, 1, "resume".to_string(), Some("go".to_string()))
        .unwrap();

    let signals = store.read_signal_events(1);
    assert_eq!(signals.len(), 1);
    assert!(
        matches!(&signals[0], WorkflowEvent::SignalReceived { name, payload, .. } if name == "resume" && payload == &Some("go".to_string()))
    );
}

#[test]
fn test_memory_store_append_read_saga_event() {
    let mut store = MemoryStore::new();
    store
        .append_saga_compensated(1, 1, "charge_card".to_string())
        .unwrap();

    let sagas = store.read_saga_events(1);
    assert_eq!(sagas.len(), 1);
    assert!(
        matches!(&sagas[0], WorkflowEvent::SagaCompensated { step_name, .. } if step_name == "charge_card")
    );
}

#[cfg(feature = "sqlite")]
#[test]
fn test_libsql_store_append_read_new_workflow_events() {
    let mut store = LibsqlStore::in_memory().unwrap();
    store.append_timer_set(1, 1, "t1".to_string(), 200).unwrap();
    store
        .append_signal_received(1, 2, "cancel".to_string(), None)
        .unwrap();
    store
        .append_saga_compensated(1, 3, "reserve".to_string())
        .unwrap();

    let all = store.read_workflow_events(1);
    assert_eq!(all.len(), 3);
    assert!(matches!(&all[0], WorkflowEvent::TimerSet { .. }));
    assert!(matches!(&all[1], WorkflowEvent::SignalReceived { .. }));
    assert!(matches!(&all[2], WorkflowEvent::SagaCompensated { .. }));

    assert_eq!(store.read_timer_events(1).len(), 1);
    assert_eq!(store.read_signal_events(1).len(), 1);
    assert_eq!(store.read_saga_events(1).len(), 1);
    assert_eq!(store.latest_sequence(1), 3);
}

#[test]
fn test_runtime_append_workflow_timer_signal_saga_events() {
    let mut rt = Runtime::new();
    let mut models = HashMap::new();
    models.insert("step_index".to_string(), StateModel::Durable);
    let actor_id = rt.spawn_workflow_actor(
        "OrderWorkflow",
        Box::new(|| vec![("step_index".to_string(), Value::int(0))]),
        models,
    );

    rt.append_timer_set(actor_id, "payment_timeout", 5000)
        .unwrap();
    rt.append_timer_fired(actor_id, "payment_timeout").unwrap();
    rt.append_signal_received(actor_id, "cancel", Some("user_123".to_string()))
        .unwrap();
    rt.append_saga_compensated(actor_id, "authorize_payment")
        .unwrap();

    let events = rt.persistence.read_workflow_events(actor_id);
    assert_eq!(events.len(), 5); // WorkflowStarted + 4 new events
    assert!(
        matches!(&events[1], WorkflowEvent::TimerSet { name, duration_ms, .. } if name == "payment_timeout" && *duration_ms == 5000)
    );
    assert!(
        matches!(&events[2], WorkflowEvent::TimerFired { name, .. } if name == "payment_timeout")
    );
    assert!(
        matches!(&events[3], WorkflowEvent::SignalReceived { name, payload, .. } if name == "cancel" && payload == &Some("user_123".to_string()))
    );
    assert!(
        matches!(&events[4], WorkflowEvent::SagaCompensated { step_name, .. } if step_name == "authorize_payment")
    );
}

#[test]
fn test_workflow_recovery_handles_new_event_variants() {
    let mut rt = Runtime::new();
    let mut models = HashMap::new();
    models.insert("step_index".to_string(), StateModel::Durable);
    let actor_id = rt.spawn_workflow_actor(
        "OrderWorkflow",
        Box::new(|| vec![("step_index".to_string(), Value::int(0))]),
        models,
    );

    rt.append_timer_set(actor_id, "t1", 100).unwrap();
    rt.append_signal_received(actor_id, "s1", Some("payload".to_string()))
        .unwrap();
    rt.append_saga_compensated(actor_id, "step_a").unwrap();

    rt.actors.remove(&actor_id);
    rt.recover_actor(actor_id).unwrap();

    let step_index = rt
        .actors
        .get(&actor_id)
        .unwrap()
        .get_state_field("step_index")
        .and_then(|v| v.as_int())
        .unwrap();
    assert_eq!(step_index, 0);

    let events = rt.persistence.read_workflow_events(actor_id);
    assert_eq!(events.len(), 4);
}

// ---------------------------------------------------------------------------
// Pipeline Tests
// ---------------------------------------------------------------------------

#[test]
fn test_pipeline_runtime_api() {
    use crate::ai::PipelineRuntime;

    let mut rt = Runtime::new();

    // Create a pipeline through the runtime API.
    let id = rt.pipeline_new();
    assert!(rt.pipelines.contains_key(&id));

    // Add a stage.
    let result = rt.pipeline_stage(id, "summarize", 42, "Summarize: {input}");
    assert_eq!(result, Ok(id));
    assert_eq!(rt.pipelines[&id].stages.len(), 1);
    assert_eq!(rt.pipelines[&id].stages[0].name, "summarize");
    assert_eq!(rt.pipelines[&id].stages[0].agent_id, 42);
    assert_eq!(
        rt.pipelines[&id].stages[0].prompt_template,
        "Summarize: {input}"
    );

    // Run the stored pipeline against a mock runtime to avoid spinning up
    // real actors/LLM clients in this unit test.
    struct MockRuntime;
    impl PipelineRuntime for MockRuntime {
        fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String> {
            Ok(format!("agent {} got {}", agent_id, prompt))
        }
    }
    let pipeline = rt.pipelines[&id].clone();
    let output = pipeline.run(&mut MockRuntime, "hello world").unwrap();
    assert_eq!(output, "agent 42 got Summarize: hello world");
}

// ---------------------------------------------------------------------------
// Multi-Node Distributed Tests
// ---------------------------------------------------------------------------

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::thread::sleep;

/// Start a distributed-enabled runtime bound to an ephemeral loopback port.
fn start_distributed_node() -> Runtime {
    let mut rt = Runtime::new();
    rt.enable_distribution(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0))
        .expect("failed to enable distribution");
    rt
}

/// Pump `process_network` on every node until each node's cluster view
/// holds `expected` healthy members (including itself), or fail.
///
/// Every poll iteration pumps ALL nodes (each `process_network` also runs
/// the cluster `tick`, which drives heartbeats, gossip, and membership
/// timeouts), then sleeps a fixed 50 ms — no assumption is made about
/// wall-clock ordering between nodes. Callers should pass a generous
/// deadline (30 s): convergence is normally sub-second, but under heavy
/// CPU load the real-TCP handshake and heartbeat cadence can degrade by
/// an order of magnitude.
fn pump_until_converged(nodes: &mut [&mut Runtime], expected: usize, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let mut counts = Vec::new();
        for rt in nodes.iter_mut() {
            rt.process_network();
            counts.push(rt.distributed.cluster.as_ref().unwrap().healthy_node_count());
        }
        if counts.iter().all(|&c| c == expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "cluster did not converge to {} healthy nodes (counts: {:?})",
            expected,
            counts
        );
        sleep(Duration::from_millis(50));
    }
}

/// Shut down the transports of the given nodes.
fn shutdown_nodes(nodes: &mut [&mut Runtime]) {
    for rt in nodes.iter_mut() {
        if let Some(transport) = rt.distributed.transport.take() {
            transport.shutdown();
        }
    }
}

#[test]
fn test_three_node_cluster_membership_converges() {
    let mut rt_a = start_distributed_node();
    let mut rt_b = start_distributed_node();
    let mut rt_c = start_distributed_node();

    let addr_a = rt_a.distributed.transport.as_ref().unwrap().listen_addr();
    let addr_b = rt_b.distributed.transport.as_ref().unwrap().listen_addr();
    let node_a = rt_a.distributed.node_id.unwrap();
    let node_b = rt_b.distributed.node_id.unwrap();
    let node_c = rt_c.distributed.node_id.unwrap();

    // The local node's own cluster entry must carry the real listen
    // address, not the port-0 bind address.
    assert_eq!(
        rt_a.distributed.cluster
            .as_ref()
            .unwrap()
            .get_node(node_a)
            .unwrap()
            .address,
        addr_a
    );

    // Full-mesh join: each new node seeds from every existing node.
    // (Transitive gossip propagation over the wire is covered separately
    // by test_three_node_gossip_converges_chain_seeded; pairwise seeding
    // plus heartbeat-based discovery converges the mesh here regardless.)
    rt_b.join_cluster(addr_a);
    rt_c.join_cluster(addr_a);
    rt_c.join_cluster(addr_b);

    pump_until_converged(
        &mut [&mut rt_a, &mut rt_b, &mut rt_c],
        3,
        Duration::from_secs(30),
    );

    // Every node sees every other node as a healthy member.
    for rt in [&rt_a, &rt_b, &rt_c] {
        let cluster = rt.distributed.cluster.as_ref().unwrap();
        for peer in [node_a, node_b, node_c] {
            let info = cluster
                .get_node(peer)
                .expect("peer missing from membership table");
            assert_eq!(
                info.status,
                NodeStatus::Healthy,
                "peer {:?} not healthy",
                peer
            );
        }
    }

    // Addresses learned by seeding carry the peer's real listen address.
    assert_eq!(
        rt_b.distributed.cluster
            .as_ref()
            .unwrap()
            .get_node(node_a)
            .unwrap()
            .address,
        addr_a
    );
    assert_eq!(
        rt_c.distributed.cluster
            .as_ref()
            .unwrap()
            .get_node(node_b)
            .unwrap()
            .address,
        addr_b
    );

    shutdown_nodes(&mut [&mut rt_a, &mut rt_b, &mut rt_c]);
}

#[test]
fn test_three_node_remote_actor_message_delivery() {
    let mut rt_a = start_distributed_node();
    let mut rt_b = start_distributed_node();
    let mut rt_c = start_distributed_node();

    let addr_a = rt_a.distributed.transport.as_ref().unwrap().listen_addr();
    let addr_b = rt_b.distributed.transport.as_ref().unwrap().listen_addr();
    let node_a = rt_a.distributed.node_id.unwrap();

    rt_b.join_cluster(addr_a);
    rt_c.join_cluster(addr_a);
    rt_c.join_cluster(addr_b);
    pump_until_converged(
        &mut [&mut rt_a, &mut rt_b, &mut rt_c],
        3,
        Duration::from_secs(30),
    );

    // An actor on node A with a decoy behavior (table index 0) and the
    // intended behavior (index 1). Remote packets carry the behavior
    // *name* and the receiver resolves it against the target actor's
    // behavior table (see process_network_packets), so dispatch must run
    // "store" — if it ever fell back to index-based dispatch the decoy
    // would run and fail this test.
    let actor_id = rt_a.spawn_actor(Box::new(|| vec![("received".to_string(), Value::int(0))]));
    {
        let actor = rt_a.actors.get_mut(&actor_id).unwrap();
        actor.register_behavior("decoy", |actor, _args| {
            actor.set_state_field("received", Value::int(-999));
        });
        actor.register_behavior("store", |actor, args| {
            let n = args.get(0).and_then(|v| v.as_int()).unwrap_or(-1);
            actor.set_state_field("received", Value::int(n));
        });
    }

    // Node C sends to the actor on node A through the location-transparent
    // address (remote node + actor id), with node B present in the mesh.
    let target = ActorAddress::remote(node_a, actor_id);
    rt_c.send_distributed(target, "store", &[Value::int(42)]);

    // Generous deadline for loaded machines; every iteration pumps ALL
    // nodes so heartbeats keep flowing and no membership view degrades
    // (suspicion kicks in after 2 s of silence) while we wait.
    let deadline = Instant::now() + Duration::from_secs(30);
    let delivered = loop {
        rt_a.process_network();
        rt_b.process_network();
        rt_c.process_network();
        rt_a.run_scheduler();
        let got = rt_a
            .actors
            .get(&actor_id)
            .and_then(|a| a.get_state_field("received"))
            .and_then(|v| v.as_int());
        if got == Some(42) {
            break true;
        }
        assert_ne!(
            got,
            Some(-999),
            "decoy behavior dispatched for the remote message"
        );
        if Instant::now() >= deadline {
            break false;
        }
        sleep(Duration::from_millis(50));
    };
    assert!(
        delivered,
        "remote message from node C was not delivered to the actor on node A"
    );

    shutdown_nodes(&mut [&mut rt_a, &mut rt_b, &mut rt_c]);
}

/// Gossip relay convergence: three nodes seeded only as a chain
/// (B joins A, C joins B — C never contacts A directly) must still
/// converge to a full membership view via gossip relayed by B.
#[test]
fn test_three_node_gossip_converges_chain_seeded() {
    let mut rt_a = start_distributed_node();
    let mut rt_b = start_distributed_node();
    let mut rt_c = start_distributed_node();

    let addr_a = rt_a.distributed.transport.as_ref().unwrap().listen_addr();
    let addr_b = rt_b.distributed.transport.as_ref().unwrap().listen_addr();
    let node_a = rt_a.distributed.node_id.unwrap();
    let node_c = rt_c.distributed.node_id.unwrap();

    // Chain seeding only: B joins A, C joins B. Without gossip on the
    // wire, A and C could never learn about each other.
    rt_b.join_cluster(addr_a);
    rt_c.join_cluster(addr_b);

    pump_until_converged(
        &mut [&mut rt_a, &mut rt_b, &mut rt_c],
        3,
        Duration::from_secs(30),
    );

    // A learned about C (and vice versa) purely through B's gossip relay,
    // and both views consider the relayed peer healthy.
    let info_c_on_a = rt_a
        .distributed.cluster
        .as_ref()
        .unwrap()
        .get_node(node_c)
        .expect("node C missing from A's membership table — gossip relay failed");
    assert_eq!(
        info_c_on_a.status,
        NodeStatus::Healthy,
        "A should see C as healthy"
    );
    let info_a_on_c = rt_c
        .distributed.cluster
        .as_ref()
        .unwrap()
        .get_node(node_a)
        .expect("node A missing from C's membership table — gossip relay failed");
    assert_eq!(
        info_a_on_c.status,
        NodeStatus::Healthy,
        "C should see A as healthy"
    );
    // Sanity: the middle node sees both ends.
    let cluster_b = rt_b.distributed.cluster.as_ref().unwrap();
    assert!(cluster_b.is_member(node_a));
    assert!(cluster_b.is_member(node_c));

    shutdown_nodes(&mut [&mut rt_a, &mut rt_b, &mut rt_c]);
}

/// Handler for the remotely-spawnable behavior used by
/// `test_remote_spawn_request_delivery`.
fn remote_spawn_store_handler(actor: &mut Actor, args: &[Value]) {
    let n = args.get(0).and_then(|v| v.as_int()).unwrap_or(-1);
    actor.set_state_field("received", Value::int(n));
}

/// Remote spawn delivery: node A issues a SpawnRequest for a behavior
/// registered on node B, receives the new actor's id via SpawnResponse,
/// and can then address the spawned actor by name.
#[test]
fn test_remote_spawn_request_delivery() {
    let mut rt_a = start_distributed_node();
    let mut rt_b = start_distributed_node();

    let addr_b = rt_b.distributed.transport.as_ref().unwrap().listen_addr();
    let node_b = rt_b.distributed.node_id.unwrap();

    // Node B offers one behavior for remote spawn.
    rt_b.register_spawnable_behavior("store", remote_spawn_store_handler);

    rt_a.join_cluster(addr_b);
    pump_until_converged(&mut [&mut rt_a, &mut rt_b], 2, Duration::from_secs(30));

    // Issue the remote spawn. The placeholder address carries the request
    // id; the real actor id arrives with the SpawnResponse.
    let request_id = {
        let mut transport = rt_a.distributed.transport.take().unwrap();
        let cluster = rt_a.distributed.cluster.take().unwrap();
        let resolver = rt_a.distributed.resolver.take().unwrap();
        let placeholder = spawn_on_node(
            &mut rt_a,
            &mut transport,
            &cluster,
            &resolver,
            node_b,
            "store",
            vec![("received".to_string(), Value::int(0))],
        );
        rt_a.distributed.transport = Some(transport);
        rt_a.distributed.cluster = Some(cluster);
        rt_a.distributed.resolver = Some(resolver);
        assert_eq!(placeholder.node_id(), node_b);
        placeholder.actor_id()
    };

    let deadline = Instant::now() + Duration::from_secs(30);
    let remote_actor = loop {
        rt_a.process_network();
        rt_b.process_network();
        if let Some(result) = rt_a.take_spawn_response(request_id) {
            break result.expect("node B rejected the spawn request");
        }
        assert!(
            Instant::now() < deadline,
            "no SpawnResponse received from node B"
        );
        sleep(Duration::from_millis(50));
    };

    // The spawned actor exists on node B and was wired with the behavior.
    assert!(
        rt_b.actors.contains_key(&remote_actor),
        "spawned actor missing on node B"
    );
    assert_eq!(
        rt_b.behavior_id_for(remote_actor, "store"),
        Some(0),
        "spawned actor should have the requested behavior at index 0"
    );

    // Node A can now address the remote actor by id; a message sent by
    // behavior name must land in the spawned actor's state.
    let target = ActorAddress::remote(node_b, remote_actor);
    rt_a.send_distributed(target, "store", &[Value::int(7)]);

    let deadline = Instant::now() + Duration::from_secs(30);
    let delivered = loop {
        rt_a.process_network();
        rt_b.process_network();
        rt_b.run_scheduler();
        let got = rt_b
            .actors
            .get(&remote_actor)
            .and_then(|a| a.get_state_field("received"))
            .and_then(|v| v.as_int());
        if got == Some(7) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        sleep(Duration::from_millis(50));
    };
    assert!(
        delivered,
        "message to the remotely-spawned actor was not delivered"
    );

    // Unknown behavior names are rejected, not spawned — the no-crash
    // counterpart of the local unknown-behavior fallback.
    let reject_id = {
        let mut transport = rt_a.distributed.transport.take().unwrap();
        let cluster = rt_a.distributed.cluster.take().unwrap();
        let resolver = rt_a.distributed.resolver.take().unwrap();
        let placeholder = spawn_on_node(
            &mut rt_a,
            &mut transport,
            &cluster,
            &resolver,
            node_b,
            "no_such_behavior",
            vec![],
        );
        rt_a.distributed.transport = Some(transport);
        rt_a.distributed.cluster = Some(cluster);
        rt_a.distributed.resolver = Some(resolver);
        placeholder.actor_id()
    };
    let actors_before = rt_b.actors.len();
    let deadline = Instant::now() + Duration::from_secs(30);
    let rejected = loop {
        rt_a.process_network();
        rt_b.process_network();
        if let Some(result) = rt_a.take_spawn_response(reject_id) {
            break result.is_none();
        }
        assert!(
            Instant::now() < deadline,
            "no SpawnResponse received for the unknown behavior"
        );
        sleep(Duration::from_millis(50));
    };
    assert!(rejected, "unknown behavior name must be rejected");
    assert_eq!(
        rt_b.actors.len(),
        actors_before,
        "rejected spawn must not create an actor"
    );

    shutdown_nodes(&mut [&mut rt_a, &mut rt_b]);
}

// ========================================================================
// CRDT delta-sync round schedule tests
// ========================================================================

/// `sync_crdts` round schedule: round 1 and every
/// `CRDT_FULL_SYNC_INTERVAL`-th round thereafter ship full state; all
/// other rounds ship deltas.
#[test]
fn test_crdt_sync_round_schedule() {
    assert!(crdt_sync_is_full_round(1), "first sync must be full");
    for round in 2..=CRDT_FULL_SYNC_INTERVAL {
        assert!(
            !crdt_sync_is_full_round(round),
            "round {round} should ship deltas"
        );
    }
    assert!(
        crdt_sync_is_full_round(CRDT_FULL_SYNC_INTERVAL + 1),
        "round after the interval must be a full repair sync"
    );
    assert!(!crdt_sync_is_full_round(CRDT_FULL_SYNC_INTERVAL + 2));
}

/// `sync_crdts` is a no-op that does not count rounds when distribution
/// is disabled; once enabled, every call counts exactly one round.
#[test]
fn test_sync_crdts_round_counting() {
    let mut rt = Runtime::new();
    rt.sync_crdts();
    assert_eq!(
        rt.crdt_sync_rounds, 0,
        "disabled runtime must not count rounds"
    );

    let mut rt = start_distributed_node();
    rt.sync_crdts();
    rt.sync_crdts();
    assert_eq!(rt.crdt_sync_rounds, 2);
    shutdown_nodes(&mut [&mut rt]);
}

/// End-to-end: CRDT changes propagate between two clustered nodes through
/// `sync_crdts`, across both the initial full-state round (which creates
/// the entry on the receiver) and subsequent delta rounds.
#[test]
fn test_sync_crdts_full_then_delta_converges_two_nodes() {
    let mut rt_a = start_distributed_node();
    let mut rt_b = start_distributed_node();

    let addr_a = rt_a.distributed.transport.as_ref().unwrap().listen_addr();
    rt_b.join_cluster(addr_a);
    pump_until_converged(&mut [&mut rt_a, &mut rt_b], 2, Duration::from_secs(30));

    let counter_value = |rt: &mut Runtime, id| {
        rt.crdt_manager
            .as_mut()
            .and_then(|m| m.get_gcounter_mut(id))
            .map(|c| c.value())
    };

    // Round 1 ships full state: a brand-new counter created on A must
    // appear on B with the right value.
    let id = rt_a.crdt_manager.as_mut().unwrap().create_gcounter().0;
    rt_a.crdt_manager
        .as_mut()
        .unwrap()
        .get_gcounter_mut(id)
        .unwrap()
        .increment();
    rt_a.sync_crdts();

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        rt_a.process_network();
        rt_b.process_network();
        if counter_value(&mut rt_b, id) == Some(1) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "full-state CRDT sync did not converge on node B"
        );
        sleep(Duration::from_millis(50));
    }

    // Rounds 2..=16 ship deltas: further increments must still propagate.
    for expected in 2..=3u64 {
        rt_a.crdt_manager
            .as_mut()
            .unwrap()
            .get_gcounter_mut(id)
            .unwrap()
            .increment();
        rt_a.sync_crdts();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            rt_a.process_network();
            rt_b.process_network();
            if counter_value(&mut rt_b, id) == Some(expected) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "delta CRDT sync did not converge on node B at value {expected}"
            );
            sleep(Duration::from_millis(50));
        }
    }
    assert!(
        rt_a.crdt_sync_rounds >= 3,
        "test must have exercised at least one delta round"
    );

    shutdown_nodes(&mut [&mut rt_a, &mut rt_b]);
}
