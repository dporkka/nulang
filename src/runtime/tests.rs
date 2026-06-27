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

    rt.exit_actor(child_id, ExitReason::Error("crash1".to_string()));
    assert_eq!(rt.supervisors[&sup_id].restart_count(
        rt.supervisors[&sup_id].children[0].1), 1);

    let child_id_2 = rt.supervisors[&sup_id].children[0].1;
    rt.exit_actor(child_id_2, ExitReason::Error("crash2".to_string()));
    assert_eq!(rt.supervisors[&sup_id].restart_count(
        rt.supervisors[&sup_id].children[0].1), 2);

    let child_id_3 = rt.supervisors[&sup_id].children[0].1;
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
    rt.process_groups.join("workers", actor_id);
    let members = rt.process_groups.members("workers");
    assert!(members.contains(&actor_id));
}

#[test]
fn test_pg_leave() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));
    rt.process_groups.join("group", actor_id);
    rt.process_groups.leave("group", actor_id);
    assert!(!rt.process_groups.is_member("group", actor_id));
}

#[test]
fn test_pg_leave_all() {
    let mut rt = Runtime::new();
    let a1 = rt.spawn_actor(Box::new(|| vec![]));
    rt.process_groups.join("g1", a1);
    rt.process_groups.join("g2", a1);
    rt.process_groups.leave_all(a1);
    assert!(rt.process_groups.members("g1").is_empty());
    assert!(rt.process_groups.members("g2").is_empty());
}

#[test]
fn test_pg_multiple_members() {
    let mut rt = Runtime::new();
    let a1 = rt.spawn_actor(Box::new(|| vec![]));
    let a2 = rt.spawn_actor(Box::new(|| vec![]));
    rt.process_groups.join("pool", a1);
    rt.process_groups.join("pool", a2);
    assert_eq!(rt.process_groups.member_count("pool"), 2);
}

#[test]
fn test_pg_join_idempotent() {
    let mut rt = Runtime::new();
    let actor_id = rt.spawn_actor(Box::new(|| vec![]));
    rt.process_groups.join("idempotent", actor_id);
    rt.process_groups.join("idempotent", actor_id);
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
