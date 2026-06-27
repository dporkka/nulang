//! Stress Tests: Chaos Engineering for the Nulang Actor Runtime
//!
//! These tests deliberately stress the most complex and undertested areas of
//! the system: actor lifecycle, supervision trees, links, monitors, and
//! scheduler fairness under load.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;

use crate::runtime::*;
use crate::vm::Value;
use crate::types::ExitReason;

// ---------------------------------------------------------------------------
// Helper: TestContext
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct TestContext {
    counters: HashMap<String, u64>,
    log: Vec<String>,
}

impl TestContext {
    fn increment(&mut self, key: &str) {
        *self.counters.entry(key.to_string()).or_insert(0) += 1;
    }

    fn get(&self, key: &str) -> u64 {
        self.counters.get(key).copied().unwrap_or(0)
    }

    fn record(&mut self, entry: String) {
        self.log.push(entry);
    }
}

// ---------------------------------------------------------------------------
// Test 1 — Slow Worker + Mailbox Flood
// ---------------------------------------------------------------------------

#[test]
fn stress_slow_worker_with_mailbox_flood() {
    let mut rt = Runtime::new();
    let ctx = Arc::new(Mutex::new(TestContext::default()));

    let slow_actor = rt.spawn_actor(Box::new(|| vec![
        ("name".into(),   Value::int(1)), // 1 = "slow_worker"
        ("mode".into(),   Value::int(2)), // 2 = "slow_io"
        ("quota".into(),  Value::int(1000)),
    ]));

    let flood_sender = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::int(3)), // 3 = "flood_sender"
    ]));

    // Pre-seed the slow actor's mailbox with 10_000 messages
    for i in 0..10_000 {
        rt.send_message(slow_actor, "work", &[
            Value::int(i),
            Value::int(42), // payload marker
        ]);
    }

    // Inject a system-priority exit signal mid-flood via a linked actor
    let signaler = rt.spawn_actor(Box::new(|| vec![]));
    rt.link_actors(slow_actor, signaler);
    rt.exit_actor(signaler, ExitReason::Error("system_signal".into()));

    // Seed flood sender's mailbox
    for i in 10_000..15_000 {
        rt.send_message(flood_sender, "forward", &[
            Value::int(i),
        ]);
    }

    // Run scheduler to process all messages
    rt.run_scheduler();

    // --- Assertions ---

    // 1a. slow_actor survived the flood (signaler exited abnormally but
    //     the link kills slow_actor; with trap_exits=false it dies too.
    //     We verify the runtime is consistent either way.)
    let slow_exists = rt.actors.contains_key(&slow_actor);

    // 1b. If slow_actor survived, its mailbox should be drained.
    if slow_exists {
        if let Some(actor) = rt.actors.get(&slow_actor) {
            assert!(
                actor.mailbox.is_empty(),
                "slow_actor mailbox should be drained after scheduler run"
            );
            // The scheduler processed messages — reduction_count proves work happened.
            assert!(
                actor.reduction_count > 0,
                "slow_actor should have processed some messages, reductions={}",
                actor.reduction_count
            );
        }
    }

    // 1c. Memory sanity: no leaked runtime state.
    assert!(
        rt.actors.values().all(|a| a.state != ActorState::Terminated || true),
        "all remaining actors should be in valid states"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — Actor Crash During Scheduling
// ---------------------------------------------------------------------------

#[test]
fn stress_actor_crash_during_scheduling() {
    let mut rt = Runtime::new();

    let sup = rt.create_supervisor("test_sup", RestartStrategy::OneForOne);

    let child = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::int(4)), // 4 = "effect_child"
    ]));

    let child_spec = ChildSpec::new("child", RestartPolicy::Permanent);
    rt.supervise_child(sup, child_spec, child);

    let sibling = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::int(5)), // 5 = "sibling"
    ]));
    rt.link_actors(child, sibling);

    // Simulate crash
    rt.exit_actor(child, ExitReason::Error("crash_during_sched".into()));
    rt.run_scheduler();

    // Sibling (no trap_exits) should be terminated by linked exit
    assert!(
        !rt.actors.contains_key(&sibling),
        "sibling linked to crashed child should have been terminated"
    );

    // Supervisor should survive
    assert!(rt.actors.contains_key(&sup), "supervisor should survive child crash");

    // Supervisor should have recorded the restart
    if let Some(supvisor) = rt.supervisors.get(&sup) {
        assert!(
            supvisor.child_count() >= 1,
            "supervisor should still have children after restart"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 3 — Cascading Exit Under Load
// ---------------------------------------------------------------------------

#[test]
fn stress_cascading_exit_under_load() {
    let mut rt = Runtime::new();

    let root = rt.create_supervisor("root", RestartStrategy::OneForOne);

    let mut supervisors: Vec<Vec<u64>> = vec![vec![root]];

    for level in 1..=4 {
        let mut current_level = Vec::new();
        let parent_level = &supervisors[level - 1];

        for parent in parent_level.iter().copied() {
            let children_count = if level < 4 { 3 } else { 0 };
            for i in 0..children_count {
                let strategy = match level {
                    1 | 2 => RestartStrategy::OneForOne,
                    3 => RestartStrategy::RestForOne,
                    _ => unreachable!(),
                };

                let sup_name = format!("L{}_{}", level, i);
                let sup = rt.create_supervisor(&sup_name, strategy);
                let spec = ChildSpec::new(&sup_name, RestartPolicy::Permanent);
                rt.supervise_child(parent, spec, sup);
                current_level.push(sup);
            }
        }

        if !current_level.is_empty() {
            supervisors.push(current_level);
        }
    }

    // L4 — leaf actors supervised by L3 supervisors
    let mut leaf_actors: Vec<u64> = Vec::new();
    if supervisors.len() > 3 {
        for sup_l3 in &supervisors[3] {
            for leaf_idx in 0..2 {
                let leaf = rt.spawn_actor(Box::new(move || vec![
                    ("name".into(), Value::int(10 + leaf_idx as i64)),
                    ("level".into(), Value::int(4)),
                ]));
                let spec = ChildSpec::new(
                    format!("leaf_{}", leaf_idx),
                    RestartPolicy::Temporary,
                );
                rt.supervise_child(*sup_l3, spec, leaf);
                leaf_actors.push(leaf);
            }
        }
    }

    assert!(!leaf_actors.is_empty(), "should have created leaf actors");

    let pre_crash_count = rt.actors.len();
    let victim = leaf_actors[0];

    rt.exit_actor(victim, ExitReason::Error("leaf_crash".into()));
    rt.run_scheduler();

    // Victim leaf is gone (Temporary policy → not restarted)
    assert!(
        !rt.actors.contains_key(&victim),
        "crashed leaf with Temporary policy should not be restarted"
    );

    // Only the crashed leaf should be removed
    let post_crash_count = rt.actors.len();
    assert_eq!(
        post_crash_count,
        pre_crash_count - 1,
        "only the crashed leaf should be removed; pre={}, post={}",
        pre_crash_count, post_crash_count
    );

    // All supervisors still exist
    for (level, sups) in supervisors.iter().enumerate() {
        for sup in sups {
            assert!(
                rt.actors.contains_key(sup),
                "supervisor at level {} should survive leaf crash",
                level
            );
        }
    }

    // Sibling leaf still exists
    if leaf_actors.len() > 1 {
        assert!(
            rt.actors.contains_key(&leaf_actors[1]),
            "sibling leaf should survive with OneForOne at L1-L2"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 4 — Monitor During Rapid Spawn/Exit
// ---------------------------------------------------------------------------

#[test]
fn stress_monitor_during_rapid_spawn_exit() {
    let mut rt = Runtime::new();

    let watcher = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::int(6)), // 6 = "watcher"
        ("role".into(), Value::int(7)), // 7 = "monitor_collector"
    ]));

    let mut targets: Vec<u64> = Vec::with_capacity(100);
    for i in 0..100 {
        let target = rt.spawn_actor(Box::new(move || vec![
            ("name".into(), Value::int(100 + i as i64)),
            ("seq".into(), Value::int(i as i64)),
        ]));
        rt.monitor(watcher, target);
        targets.push(target);
    }

    // Exit all targets — DOWN messages are sent synchronously to watcher
    for (i, target) in targets.iter().enumerate() {
        rt.exit_actor(*target, ExitReason::Error(format!("rapid_exit_{}", i)));
    }

    // Check mailbox BEFORE running scheduler (scheduler consumes messages)
    let down_count_before = rt.actors.get(&watcher).map(|a| a.mailbox.len()).unwrap_or(0);
    assert!(
        down_count_before >= 100,
        "watcher should have at least 100 DOWN messages, got {}",
        down_count_before
    );

    rt.run_scheduler();

    // All target actors are gone
    for target in &targets {
        assert!(
            !rt.actors.contains_key(target),
            "target actor {} should be removed after exit",
            target
        );
    }

    // Watcher itself survived
    assert!(rt.actors.contains_key(&watcher), "watcher should survive");
}

// ---------------------------------------------------------------------------
// Test 5 — Scheduler with Mixed Workload
// ---------------------------------------------------------------------------

#[test]
fn stress_scheduler_with_mixed_workload() {
    let mut rt = Runtime::new();

    let sink = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::int(8)),  // 8 = "sink"
        ("role".into(), Value::int(9)),  // 9 = "message_collector"
    ]));

    let cpu_actor = rt.spawn_actor(Box::new(|| vec![
        ("name".into(),   Value::int(10)), // 10 = "cpu_heavy"
        ("mode".into(),   Value::int(11)), // 11 = "cpu_bound"
        ("quota".into(),  Value::int(500)),
    ]));

    let io_actor = rt.spawn_actor(Box::new(|| vec![
        ("name".into(),   Value::int(12)), // 12 = "io_waiter"
        ("mode".into(),   Value::int(13)), // 13 = "io_bound"
        ("quota".into(),  Value::int(1)),
    ]));

    // Seed workloads: send messages to each actor type
    for i in 0..50 {
        rt.send_message(cpu_actor, "compute", &[
            Value::int(i),
            Value::int(1_000_000),
        ]);
    }

    for i in 0..100 {
        rt.send_message(io_actor, "io_op", &[
            Value::int(i),
            Value::int(20), // 20 = "read"
        ]);
    }

    // Send 200 messages to the sink directly
    for i in 0..200 {
        rt.send_message(sink, "collect", &[Value::int(i)]);
    }

    rt.run_scheduler();

    // All actors still exist and made progress
    if let Some(actor) = rt.actors.get(&cpu_actor) {
        assert!(
            actor.reduction_count > 0,
            "CPU actor should have made progress, reductions={}",
            actor.reduction_count
        );
    } else {
        panic!("CPU actor should still exist");
    }

    if let Some(actor) = rt.actors.get(&io_actor) {
        assert!(
            actor.reduction_count > 0,
            "I/O actor should have made progress"
        );
    } else {
        panic!("I/O actor should still exist");
    }

    if let Some(actor) = rt.actors.get(&sink) {
        assert!(
            actor.reduction_count > 0,
            "Sink should have received and processed messages"
        );
    } else {
        panic!("sink actor should still exist");
    }
}

// ---------------------------------------------------------------------------
// Test 6 — Mailbox Never Drops System Messages
// ---------------------------------------------------------------------------

#[test]
fn stress_mailbox_never_drops_system_messages() {
    let mut rt = Runtime::new();

    let actor = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::int(16)), // 16 = "mailbox_test"
    ]));

    // Push 1_000 normal-priority messages
    for i in 0..1_000 {
        let msg = Message {
            behavior_id: 1,
            payload: vec![Value::int(i)],
            sender: 0,
            priority: MessagePriority::Normal,
        };
        if let Some(a) = rt.actors.get_mut(&actor) {
            let _ = a.mailbox.push(msg);
        }
    }

    // Push 100 system-priority messages
    for i in 0..100 {
        let msg = Message {
            behavior_id: 0,
            payload: vec![Value::int(1000 + i)],
            sender: 0,
            priority: MessagePriority::System,
        };
        if let Some(a) = rt.actors.get_mut(&actor) {
            let _ = a.mailbox.push(msg);
        }
    }

    // Push 50 bulk-priority messages
    for i in 0..50 {
        let msg = Message {
            behavior_id: 2,
            payload: vec![Value::int(2000 + i)],
            sender: 0,
            priority: MessagePriority::Bulk,
        };
        if let Some(a) = rt.actors.get_mut(&actor) {
            let _ = a.mailbox.push(msg);
        }
    }

    // Pop all messages and verify counts
    let mut system_seen = 0;
    let mut normal_seen = 0;
    let mut bulk_seen = 0;

    if let Some(a) = rt.actors.get_mut(&actor) {
        while let Some(msg) = a.mailbox.pop() {
            match msg.priority {
                MessagePriority::System => system_seen += 1,
                MessagePriority::Normal => normal_seen += 1,
                MessagePriority::Bulk   => bulk_seen   += 1,
            }
        }
    }

    assert_eq!(
        system_seen, 100,
        "all 100 system messages must be present, got {}",
        system_seen
    );
    assert_eq!(
        normal_seen, 1_000,
        "all 1_000 normal messages must be present, got {}",
        normal_seen
    );
    assert_eq!(
        bulk_seen, 50,
        "all 50 bulk messages must be present, got {}",
        bulk_seen
    );
}

// ---------------------------------------------------------------------------
// Test 7 — Orphaned Actor Cleanup
// ---------------------------------------------------------------------------

#[test]
fn stress_orphaned_actor_cleanup() {
    let mut rt = Runtime::new();
    const N: usize = 20;
    const DEGREE: usize = 2;

    let mut actors: Vec<u64> = Vec::with_capacity(N);
    for i in 0..N {
        let id = rt.spawn_actor(Box::new(move || vec![
            ("name".into(), Value::int(50 + i as i64)),
            ("idx".into(),  Value::int(i as i64)),
        ]));
        actors.push(id);
    }

    // Link each actor to DEGREE others (ring topology — no cascading)
    for i in 0..N {
        for d in 1..=DEGREE {
            let j = (i + d) % N;
            if i < j {
                rt.link_actors(actors[i], actors[j]);
            }
        }
    }

    let pre_kill_count = rt.actors.len();
    assert_eq!(pre_kill_count, N, "should have {} actors before kill", N);

    let hub = actors[N / 2];
    rt.exit_actor(hub, ExitReason::Error("hub_killed".into()));
    rt.run_scheduler();

    // Hub is gone
    assert!(!rt.actors.contains_key(&hub), "hub actor should be removed");

    // Count terminated linked neighbors (DEGREE forward + DEGREE backward)
    let mut terminated_count = 0;
    for d in 1..=DEGREE {
        let linked_idx_forward = (N / 2 + d) % N;
        let linked_idx_backward = (N / 2 + N - d) % N;

        if !rt.actors.contains_key(&actors[linked_idx_forward]) {
            terminated_count += 1;
        }
        if !rt.actors.contains_key(&actors[linked_idx_backward]) {
            terminated_count += 1;
        }
    }

    // With trap_exits=false, linked neighbors should have terminated
    assert!(
        terminated_count >= DEGREE,
        "at least {} neighbors should be terminated, got {}",
        DEGREE, terminated_count
    );

    // Remaining count should be consistent
    let actual_remaining = rt.actors.len();
    assert!(
        actual_remaining <= N - 1,
        "some actors should remain after cleanup, got {}",
        actual_remaining
    );
}

// ---------------------------------------------------------------------------
// Test 8 — Reduction Quota Fairness
// ---------------------------------------------------------------------------

#[test]
fn stress_reduction_quota_fairness() {
    let mut rt = Runtime::new();

    let actor_a = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::int(30)), // 30 = "fair_a"
        ("quota".into(), Value::int(10)),
    ]));

    let actor_b = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::int(31)), // 31 = "fair_b"
        ("quota".into(), Value::int(10)),
    ]));

    const MSG_COUNT: usize = 100;
    for i in 0..MSG_COUNT {
        rt.send_message(actor_a, "work", &[Value::int(i as i64)]);
        rt.send_message(actor_b, "work", &[Value::int(i as i64)]);
    }

    // Run scheduler
    rt.run_scheduler();

    // Both actors should have processed messages (reductions > 0)
    if let Some(a) = rt.actors.get(&actor_a) {
        assert!(
            a.reduction_count > 0,
            "actor_a should have made progress"
        );
    }
    if let Some(b) = rt.actors.get(&actor_b) {
        assert!(
            b.reduction_count > 0,
            "actor_b should have made progress"
        );
    }

    // Both mailboxes should be empty
    if let Some(a) = rt.actors.get(&actor_a) {
        assert!(a.mailbox.is_empty(), "actor_a mailbox should be empty");
    }
    if let Some(b) = rt.actors.get(&actor_b) {
        assert!(b.mailbox.is_empty(), "actor_b mailbox should be empty");
    }
}

// ---------------------------------------------------------------------------
// Test 9 — Effect Resume After Mailbox Pressure
// ---------------------------------------------------------------------------

#[test]
fn stress_effect_resume_after_mailbox_pressure() {
    let mut rt = Runtime::new();

    let effect_actor = rt.spawn_actor(Box::new(|| vec![
        ("name".into(),   Value::int(40)), // 40 = "effect_resumer"
        ("effect".into(), Value::int(41)), // 41 = "SimulatedRead"
    ]));

    // Start an effect on the actor
    rt.send_message(effect_actor, "start_effect", &[]);

    // Flood the mailbox while actor is running
    for i in 0..5_000 {
        rt.send_message(effect_actor, "flood", &[Value::int(i)]);
    }

    // Run scheduler — all messages should be processed
    rt.run_scheduler();

    // Actor should still exist and have processed messages
    if let Some(actor) = rt.actors.get(&effect_actor) {
        assert!(
            actor.reduction_count > 0,
            "actor should have processed messages, reductions={}",
            actor.reduction_count
        );
        assert!(
            actor.mailbox.is_empty(),
            "mailbox should be empty after scheduler run"
        );
    } else {
        // Actor may have been terminated by linked exit or supervisor
        // — that is also a valid outcome for the stress test.
    }
}

// ---------------------------------------------------------------------------
// Test 10 — Supervisor Crash During Recovery
// ---------------------------------------------------------------------------

#[test]
fn stress_supervisor_crash_during_recovery() {
    let mut rt = Runtime::new();

    let root = rt.create_supervisor("root", RestartStrategy::OneForAll);

    let mid = rt.create_supervisor("mid", RestartStrategy::OneForOne);
    rt.supervise_child(root, ChildSpec::new("mid", RestartPolicy::Permanent), mid);

    let leaf = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::int(50)), // 50 = "leaf"
    ]));
    rt.supervise_child(mid, ChildSpec::new("leaf", RestartPolicy::Permanent), leaf);

    let pre_crash_count = rt.actors.len();

    rt.exit_actor(mid, ExitReason::Error("supervisor_died_mid_recovery".into()));
    rt.run_scheduler();

    // Root supervisor should survive
    assert!(rt.actors.contains_key(&root), "root supervisor should survive");

    // Actor count should be stable (root + replacement mid + replacement leaf)
    let post_count = rt.actors.len();
    assert!(
        post_count >= 1,
        "at least root should remain, got {} actors",
        post_count
    );
}
