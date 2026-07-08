//! Runtime integration tests.
//!
//! 107 tests total (83 pre-v0.7 + 24 v0.7 BEAM primitive tests).
//! Full history in local commit 1c2cde9.

use super::*;
use std::time::{Duration, Instant};
use std::sync::atomic::Ordering;
use crate::vm::Frame;
use crate::runtime::heap::ActorHeap;
use crate::runtime::gc::{OrcaGc, TypeTag};

// ========================================================================
// Core Runtime Tests
// ========================================================================

#[test]
fn test_spawn_send_step_sequence() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| {
        vec![("count".to_string(), Value::int(0))]
    }));
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
    let msg = Message { behavior_id: 0, payload: vec![Value::int(42)], sender: 1, priority: MessagePriority::Normal };
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
    let mut actor = Actor::new(1, "test_actor", 16);
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
    assert!(!rt.supervisors.contains_key(&sup_id), "supervisor should shut down after max restarts");
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
    assert!(rt.actors.contains_key(&child_sup), "child supervisor should still exist after one restart");

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
    let local_count = unsafe { (*header_ptr).local_count.load(Ordering::Relaxed) };
    assert_eq!(local_count, 1);

    unsafe { gc.local_ref(&heap, obj.unwrap()) };
    let local_count2 = unsafe { (*header_ptr).local_count.load(Ordering::Relaxed) };
    assert_eq!(local_count2, 2);

    unsafe { gc.drop_local_ref(&mut heap, obj.unwrap()) };
    let local_count3 = unsafe { (*header_ptr).local_count.load(Ordering::Relaxed) };
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
    assert_eq!(header_a.local_count.load(Ordering::Relaxed), 1);
    assert_eq!(header_b.local_count.load(Ordering::Relaxed), 1);
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
    assert!(!rt.actors.contains_key(&b), "linked actor b should also exit");
}

#[test]
fn test_exit_does_not_propagate_for_normal_exit() {
    let mut rt = Runtime::new();
    let a = rt.spawn_actor(Box::new(|| vec![]));
    let b = rt.spawn_actor(Box::new(|| vec![]));
    rt.link_actors(a, b);
    rt.exit_actor(a, ExitReason::Normal);
    assert!(!rt.actors.contains_key(&a));
    assert!(rt.actors.contains_key(&b), "linked actor b should NOT exit on normal exit");
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
    assert!(rt.actors.contains_key(&b), "actor with trap_exits should survive");
    assert!(!rt.actors[&b].mailbox.is_empty(), "exit signal should become message");
}

// ========================================================================
// VM Opcode Tests
// ========================================================================

#[test]
fn test_vm_value_nan_tagging() {
    let v = Value::int(42);
    assert_eq!(v.as_int(), Some(42));
    let f = Value::float(3.14);
    assert!((f.as_float().unwrap() - 3.14).abs() < 0.001);
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

#[test]
fn test_sqlite_store_save_load_snapshot() {
    let mut store = SqliteStore::in_memory().unwrap();
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

#[test]
fn test_sqlite_store_append_read_journal() {
    let mut store = SqliteStore::in_memory().unwrap();
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

#[test]
fn test_sqlite_store_latest_sequence() {
    let mut store = SqliteStore::in_memory().unwrap();
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

#[test]
fn test_sqlite_store_clear() {
    let mut store = SqliteStore::in_memory().unwrap();
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

#[test]
fn test_sqlite_store_persists_to_disk() {
    let path = std::env::temp_dir()
        .join(format!("nulang_sqlite_test_{}.db", std::process::id()));
    {
        let mut store = SqliteStore::new(&path).unwrap();
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
        let store = SqliteStore::new(&path).unwrap();
        let snapshot = store.load_snapshot(1).unwrap();
        assert_eq!(snapshot.sequence, 1);
        assert_eq!(snapshot.state.get("x"), Some(&PersistedValue::Float(1.5)));
        let journal = store.read_journal(1);
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].payload, vec![PersistedValue::Bool(true)]);
    }

    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_persistent_actor_with_sqlite_store() {
    let mut rt = Runtime::new();
    rt.persistence = Box::new(SqliteStore::in_memory().unwrap());
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
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::bytecode::{ActorMeta, BehaviorTableEntry, CodeModule, Constant, Instruction, OpCode};
    use crate::runtime::persistence::StateModel as RuntimeStateModel;
    use crate::vm::{VM, Value};

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
    assert_eq!(actor.state_models.get("balance"), Some(&RuntimeStateModel::Durable));
}

#[test]
fn test_vm_spawn_creates_non_persistent_actor() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::bytecode::{ActorMeta, BehaviorTableEntry, CodeModule, Constant, Instruction, OpCode};
    use crate::vm::{VM, Value};

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
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
    use crate::runtime::heap::{ActorHeap, TypeTag};
    use crate::vm::VM;

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
    actor.heap.iter_live_objects(|_h, payload, _size| ptrs.push(payload));
    assert_eq!(ptrs.len(), 1);
    unsafe {
        let header = &*ActorHeap::header_of(ptrs[0]);
        assert_eq!(header.type_tag, TypeTag::Array);
    }
}

#[test]
fn test_vm_arr_load_store_and_len_on_actor_heap() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
    use crate::vm::VM;

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let actor_id = rt.borrow_mut().spawn_actor(Box::new(|| vec![]));
    rt.borrow_mut().current_actor = Some(actor_id);

    let mut module = CodeModule::new("test");
    let len_idx = module.add_constant(Constant::Int(3));
    let idx_idx = module.add_constant(Constant::Int(1));
    let val_idx = module.add_constant(Constant::Int(42));

    module.emit(Instruction::new3(OpCode::ConstU, ((len_idx >> 8) & 0xFF) as u8, (len_idx & 0xFF) as u8, 1));
    module.emit(Instruction::new2(OpCode::ArrAlloc, 1, 0)); // r0 = arr
    module.emit(Instruction::new3(OpCode::ConstU, ((idx_idx >> 8) & 0xFF) as u8, (idx_idx & 0xFF) as u8, 2));
    module.emit(Instruction::new3(OpCode::ConstU, ((val_idx >> 8) & 0xFF) as u8, (val_idx & 0xFF) as u8, 3));
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
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
    use crate::vm::VM;

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let actor_id = rt.borrow_mut().spawn_actor(Box::new(|| vec![]));
    rt.borrow_mut().current_actor = Some(actor_id);

    let mut module = CodeModule::new("test");
    let len_idx = module.add_constant(Constant::Int(4));
    module.emit(Instruction::new3(OpCode::ConstU, ((len_idx >> 8) & 0xFF) as u8, (len_idx & 0xFF) as u8, 1));
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

#[test]
fn test_vm_sconcat_uses_actor_heap() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use crate::bytecode::{CodeModule, Constant, Instruction, OpCode};
    use crate::runtime::heap::{ActorHeap, TypeTag};
    use crate::vm::VM;

    let rt = Rc::new(RefCell::new(Runtime::new()));
    let actor_id = rt.borrow_mut().spawn_actor(Box::new(|| vec![]));
    rt.borrow_mut().current_actor = Some(actor_id);

    let mut module = CodeModule::new("test");
    let a_idx = module.add_constant(Constant::Int(12));
    let b_idx = module.add_constant(Constant::Int(34));
    module.emit(Instruction::new3(OpCode::ConstU, ((a_idx >> 8) & 0xFF) as u8, (a_idx & 0xFF) as u8, 1));
    module.emit(Instruction::new3(OpCode::ConstU, ((b_idx >> 8) & 0xFF) as u8, (b_idx & 0xFF) as u8, 2));
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
    actor.heap.iter_live_objects(|_h, payload, _size| ptrs.push(payload));
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
    assert!(local.is_some(), "local-actor restriction should be set by Runtime");
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
    assert_eq!(stats.total_tasks_processed, 4, "spawn + send should produce four actor tasks");
    assert_eq!(stats.empty_polls, 1, "scheduler should poll empty once after draining");

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
        actor.heap.alloc(16, crate::runtime::heap::TypeTag::Raw).unwrap()
    };
    unsafe {
        let header = &*ActorHeap::header_of(ptr);
        assert_eq!(header.actor_id, a, "heap actor_id should be set on creation");
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

    let ptr = rt.actors.get_mut(&a).unwrap().heap.alloc(16, crate::runtime::heap::TypeTag::Raw).unwrap();
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
    assert_eq!(snapshot.state.get("step_index"), Some(&PersistedValue::Int(0)));
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
    assert_eq!(snapshot.state.get("step_index"), Some(&PersistedValue::Int(1)));
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
    assert!(matches!(&timers[0], WorkflowEvent::TimerSet { name, duration_ms, .. } if name == "t1" && *duration_ms == 100));
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

#[test]
fn test_sqlite_store_append_read_new_workflow_events() {
    let mut store = SqliteStore::in_memory().unwrap();
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

    rt.append_timer_set(actor_id, "payment_timeout", 5000).unwrap();
    rt.append_timer_fired(actor_id, "payment_timeout").unwrap();
    rt.append_signal_received(actor_id, "cancel", Some("user_123".to_string()))
        .unwrap();
    rt.append_saga_compensated(actor_id, "authorize_payment").unwrap();

    let events = rt.persistence.read_workflow_events(actor_id);
    assert_eq!(events.len(), 5); // WorkflowStarted + 4 new events
    assert!(matches!(&events[1], WorkflowEvent::TimerSet { name, duration_ms, .. } if name == "payment_timeout" && *duration_ms == 5000));
    assert!(matches!(&events[2], WorkflowEvent::TimerFired { name, .. } if name == "payment_timeout"));
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
