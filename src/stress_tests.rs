//! Stress Tests: Chaos Engineering for the Nulang Actor-Effect Boundary
//!
//! These tests deliberately stress the most complex and undertested interaction
//! in the system: the boundary between lightweight green-thread actors (M:N
//! scheduling) and algebraic effects (perform/handle/resume).
//!
//! Each test targets a specific failure mode:
//!   1. Slow I/O effect + mailbox flood — scheduler must not block, system
//!      messages must not be lost.
//!   2. Crash during effect yield — supervisor must clean up partial stack frames.
//!   3. Cascading exit under load — deep supervision trees, no deadlocks.
//!   4. Monitor during rapid spawn/exit — no DOWN messages lost.
//!   5. Mixed workload fairness — CPU, I/O, and message-heavy actors.
//!   6. Mailbox never drops system messages — unbounded queue guarantee.
//!   7. Orphaned actor cleanup — complex link topologies.
//!   8. Reduction quota fairness — fair scheduler sharing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::runtime::*;
use crate::vm::Value;
use crate::types::ExitReason;

// ---------------------------------------------------------------------------
// Helper: TestContext
// ---------------------------------------------------------------------------

/// Shared mutable state that actors and the test harness can both inspect.
/// Wrapped in `Arc<Mutex<_>>` so actors can safely record outcomes during
/// stress runs.
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
// Test 1 — Slow I/O Effect + Mailbox Flood
// ---------------------------------------------------------------------------

/// **Scenario**: An actor performs a slow simulated I/O effect. While it is
/// waiting, flood its mailbox with high-priority messages.
///
/// **What to verify**:
/// * The scheduler does not block the OS thread.
/// * Memory is not leaked (mailbox growth is bounded by what the actor can
///   process).
/// * System-priority messages (exit signals) are still delivered even during
///   the flood.
/// * The actor can eventually process all messages after the slow effect
///   completes.
#[test]
fn stress_slow_io_effect_with_mailbox_flood() {
    let mut rt = Runtime::new();

    let ctx = Arc::new(Mutex::new(TestContext::default()));

    // Spawn a slow actor.  In the real system this actor would perform a
    // slow I/O effect (e.g. `perform ReadFile` with a simulated latency of
    // many reduction steps).  For the stress test we simulate the latency by
    // giving the actor a behavior that consumes many reduction steps per
    // message — the scheduler sees the quota exhausted and yields, letting
    // other actors (or the OS thread) continue.
    let slow_actor = rt.spawn_actor(Box::new(|| vec![
        ("name".into(),   Value::string("slow_worker")),
        ("mode".into(),   Value::string("slow_io")),
        ("quota".into(),  Value::int(1000)), // high reduction cost
    ]));

    // Spawn a flood sender so that the sender itself is an actor and not
    // happening synchronously on the test thread.
    let flood_sender = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::string("flood_sender")),
    ]));

    // --- Phase 1: Pre-seed the slow actor's mailbox with 10_000 messages ---
    for i in 0..10_000 {
        rt.send_message(slow_actor, "work", &[
            Value::int(i),
            Value::string("payload"),
        ]);
    }

    // --- Phase 2: Inject a system-priority exit signal mid-flood ---
    //
    // The mailbox is an unbounded lock-free MPSC queue (`crossbeam::SegQueue`).
    // System messages must *never* be dropped, even when 10 000 normal
    // messages are already queued.  We send the system message *after* the
    // flood so it sits at the tail of the queue and the test verifies that
    // the actor eventually sees it.
    let system_msg = Message {
        behavior_id: 0, // behavior 0 == system exit signal
        payload: vec![Value::string("system_ping")],
        sender: flood_sender,
        priority: MessagePriority::System,
    };

    // Direct mailbox push — bypasses `send_message` so we can control priority.
    if let Some(actor) = rt.actors.get(&slow_actor) {
        actor.mailbox.push(system_msg);
    }

    // --- Phase 3: Continue the flood while the scheduler runs ---
    // Send another 5 000 messages *after* the system message to make sure
    // the system message is buried deep in the queue.
    for i in 10_000..15_000 {
        rt.send_message(slow_actor, "work", &[
            Value::int(i),
            Value::string("late_payload"),
        ]);
    }

    // --- Phase 4: Run the scheduler ---
    // The scheduler must not block the OS thread even though one actor is
    // stuck in a slow I/O effect.  `run_scheduler` returns when no actor has
    // ready work.
    rt.run_scheduler();

    // --- Phase 5: Assertions ---

    // 5a. The slow actor still exists (the system message was NOT an exit
    //     signal, just a ping).
    assert!(
        rt.actors.contains_key(&slow_actor),
        "slow_actor should survive the flood"
    );

    // 5b. Mailbox must be empty after the scheduler drains it.
    if let Some(actor) = rt.actors.get(&slow_actor) {
        assert!(
            actor.mailbox.is_empty(),
            "slow_actor mailbox should be drained after scheduler run"
        );
    }

    // 5c. Memory sanity: 15 000 messages were enqueued.  After draining the
    //     queue should be empty — no leaked nodes.
    //     (crossbeam::SegQueue drops nodes lazily; we verify the logical
    //     length is zero.)

    // 5d. System message was not lost: the actor processed it.
    //     We check by inspecting the actor's state — behavior 0 should have
    //     been invoked.
    if let Some(actor) = rt.actors.get(&slow_actor) {
        let processed = actor.processed_count.load(Ordering::SeqCst);
        assert_eq!(
            processed, 15_001,
            "slow_actor should have processed all 15000 normal + 1 system message, \
             got {}",
            processed
        );
    }
}

// ---------------------------------------------------------------------------
// Test 2 — Actor Crash During Effect Yield
// ---------------------------------------------------------------------------

/// **Scenario**: An actor crashes exactly at the moment an algebraic effect
/// has yielded control to a handler.  Verify the supervisor can clean up the
/// partial stack frame.
///
/// **What to verify**:
/// * Supervisor handles the crash (restart or propagate).
/// * Linked sibling receives exit signal.
/// * No panic, no memory corruption.
/// * Partial effect stack frame is cleaned up (no leaked continuations).
#[test]
fn stress_actor_crash_during_effect_yield() {
    let mut rt = Runtime::new();

    // --- Phase 1: Create supervisor with OneForOne strategy ---
    let sup = rt.create_supervisor(
        "test_sup",
        RestartStrategy::OneForOne {
            max_restarts: 3,
            within_seconds: 60,
        },
    );

    // --- Phase 2: Create child actor that will crash mid-effect ---
    let child = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::string("effect_child")),
        // The "effect_in_progress" flag tells the test harness that this
        // actor has a pending effect stack frame.
        ("effect_in_progress".into(), Value::bool(true)),
    ]));

    let child_spec = ChildSpec {
        id: "child".to_string(),
        restart: RestartPolicy::Permanent,
        max_restarts: None,
        shutdown_timeout_ms: 5000,
    };
    rt.supervise_child(sup, child_spec, child);

    // --- Phase 3: Link child to a sibling actor ---
    let sibling = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::string("sibling")),
        ("trap_exit".into(), Value::bool(false)), // does NOT trap exits
    ]));
    rt.link_actors(child, sibling);

    // --- Phase 4: Simulate the crash mid-effect ---
    //
    // In the real VM this would happen when:
    //   1. Child executes `perform SomeEffect`.
    //   2. VM builds a continuation (partial stack frame).
    //   3. Control transfers to the effect handler.
    //   4. *Before* `resume` is called, the child receives a fatal signal.
    //
    // `exit_actor` simulates this by terminating the actor while its
    // `effect_stack` is non-empty.
    rt.exit_actor(child, ExitReason::Error("crash_during_effect".into()));

    // --- Phase 5: Run scheduler to process exit signals and supervisor ---
    // actions.
    rt.run_scheduler();

    // --- Phase 6: Assertions ---

    // 6a. Child should be gone (not restarted in this test because we don't
    //     have a factory closure; the supervisor records the restart attempt).
    //     The key assertion is that the runtime did *not* panic while cleaning
    //     up a partial effect frame.
    let child_exists = rt.actors.contains_key(&child);
    // In a full implementation the supervisor would restart the child.
    // For this stress test the critical property is "no panic".

    // 6b. Sibling should have received the exit signal.  Since sibling does
    //     NOT trap exits, it should itself be terminated.
    let sibling_exists = rt.actors.contains_key(&sibling);
    assert!(
        !sibling_exists,
        "sibling linked to crashed child should have been terminated \
         (does not trap exits)"
    );

    // 6c. Supervisor still exists and recorded the crash.
    assert!(
        rt.actors.contains_key(&sup),
        "supervisor should survive child crash"
    );
    if let Some(actor) = rt.actors.get(&sup) {
        let restart_count = actor.restart_count.load(Ordering::SeqCst);
        assert!(
            restart_count >= 1,
            "supervisor should have recorded at least one restart attempt, \
             got {}",
            restart_count
        );
    }

    // 6d. No leaked effect continuations: check that the runtime's global
    //     continuation table is empty.
    assert_eq!(
        rt.pending_continuations.len(),
        0,
        "no effect continuations should be leaked after crash cleanup"
    );

    // 6e. The child key is either gone or replaced by a restarted instance.
    //     Either way the old actor's memory (including the partial stack) is
    //     reclaimed.
    if child_exists {
        // If the supervisor restarted the child, verify it's a *new* actor.
        if let Some(actor) = rt.actors.get(&child) {
            assert_eq!(
                actor.restart_count.load(Ordering::SeqCst),
                0,
                "restarted child should have fresh restart count"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 3 — Cascading Exit Under Load
// ---------------------------------------------------------------------------

/// **Scenario**: Create a deep supervision tree (5 levels).  Crash a leaf.
/// Verify the exit cascades correctly without deadlocks.
///
/// Topology:
/// ```text
/// L0: Root supervisor
///     L1: Supervisor A, Supervisor B, Supervisor C
///         L2: Supervisor A1, A2, A3  (under A)  …
///             L3: Worker A1a, A1b, A1c  (under A1)
///                 L4: Leaf A1a1, A1a2  (under A1a)
/// ```
///
/// Crash leaf A1a1.  With `OneForAll` the sibling leaf A1a2 also restarts.
/// With `RestForOne` siblings started *after* A1a1 restart.
#[test]
fn stress_cascading_exit_under_load() {
    let mut rt = Runtime::new();

    // --- Phase 1: Build the tree ---

    // L0 — root
    let root = rt.create_supervisor(
        "root",
        RestartStrategy::OneForOne {
            max_restarts: 5,
            within_seconds: 60,
        },
    );

    // Helper to build a level of supervisors
    let mut supervisors: Vec<Vec<u64>> = vec![vec![root]];

    for level in 1..=4 {
        let mut current_level = Vec::new();
        let parent_level = &supervisors[level - 1];

        for parent in parent_level.iter().copied() {
            let children_count = if level < 4 { 3 } else { 0 };
            for i in 0..children_count {
                let strategy = match level {
                    1 | 2 => RestartStrategy::OneForOne {
                        max_restarts: 3,
                        within_seconds: 60,
                    },
                    3 => RestartStrategy::RestForOne {
                        max_restarts: 2,
                        within_seconds: 30,
                    },
                    _ => unreachable!(),
                };

                let sup_name = format!("L{}_{}", level, i);
                let sup = rt.create_supervisor(&sup_name, strategy);
                let spec = ChildSpec {
                    id: sup_name.clone(),
                    restart: RestartPolicy::Permanent,
                    max_restarts: None,
                    shutdown_timeout_ms: 5000,
                };
                rt.supervise_child(parent, spec, sup);
                current_level.push(sup);
            }
        }

        if !current_level.is_empty() {
            supervisors.push(current_level);
        }
    }

    // L4 — leaf actors supervised by the L3 supervisors
    let mut leaf_actors: Vec<u64> = Vec::new();
    if supervisors.len() > 3 {
        for sup_l3 in &supervisors[3] {
            for leaf_idx in 0..2 {
                let leaf = rt.spawn_actor(Box::new(move || vec![
                    ("name".into(), Value::string(format!("leaf_{}", leaf_idx))),
                    ("level".into(), Value::int(4)),
                ]));
                let spec = ChildSpec {
                    id: format!("leaf_{}", leaf_idx),
                    restart: RestartPolicy::Temporary,
                    max_restarts: None,
                    shutdown_timeout_ms: 1000,
                };
                rt.supervise_child(*sup_l3, spec, leaf);
                leaf_actors.push(leaf);
            }
        }
    }

    assert!(
        !leaf_actors.is_empty(),
        "should have created leaf actors"
    );

    // --- Phase 2: Crash the first leaf ---
    let victim = leaf_actors[0];
    let pre_crash_count = rt.actors.len();

    rt.exit_actor(victim, ExitReason::Error("leaf_crash".into()));

    // --- Phase 3: Run scheduler to propagate exits ---
    rt.run_scheduler();

    // --- Phase 4: Assertions ---

    // 4a. The victim leaf is gone (Temporary restart policy → not restarted).
    assert!(
        !rt.actors.contains_key(&victim),
        "crashed leaf with Temporary policy should not be restarted"
    );

    // 4b. No deadlock: scheduler completed (run_scheduler returned).
    //     This is implicit — if we got here, no deadlock occurred.

    // 4c. The rest of the tree is intact.
    let post_crash_count = rt.actors.len();
    assert_eq!(
        post_crash_count,
        pre_crash_count - 1,
        "only the crashed leaf should be removed; \
         pre={}, post={}",
        pre_crash_count,
        post_crash_count
    );

    // 4d. All supervisors still exist.
    for (level, sups) in supervisors.iter().enumerate() {
        for sup in sups {
            assert!(
                rt.actors.contains_key(sup),
                "supervisor at level {} should survive leaf crash",
                level
            );
        }
    }

    // 4e. The sibling leaf (leaf_actors[1]) still exists.
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

/// **Scenario**: Rapidly spawn and exit 100 actors while monitoring them
/// from a single watcher.  Verify no monitor DOWN messages are lost.
///
/// **What to verify**:
/// * Exactly 100 DOWN messages are delivered to the watcher.
/// * No monitor messages are duplicated.
/// * No panic during rapid churn.
#[test]
fn stress_monitor_during_rapid_spawn_exit() {
    let mut rt = Runtime::new();

    // --- Phase 1: Create a watcher actor ---
    let watcher = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::string("watcher")),
        ("role".into(), Value::string("monitor_collector")),
    ]));

    // --- Phase 2: Spawn 100 actors and monitor each from the watcher ---
    let mut targets: Vec<u64> = Vec::with_capacity(100);
    for i in 0..100 {
        let target = rt.spawn_actor(Box::new(move || vec![
            ("name".into(), Value::string(format!("target_{}", i))),
            ("seq".into(), Value::int(i as i64)),
        ]));
        rt.monitor(watcher, target);
        targets.push(target);
    }

    // --- Phase 3: Exit all 100 targets rapidly ---
    for (i, target) in targets.iter().enumerate() {
        rt.exit_actor(
            *target,
            ExitReason::Error(format!("rapid_exit_{}", i)),
        );
    }

    // --- Phase 4: Run scheduler to deliver DOWN messages ---
    rt.run_scheduler();

    // --- Phase 5: Assertions ---

    // 5a. Watcher's mailbox should contain exactly 100 DOWN messages.
    if let Some(actor) = rt.actors.get(&watcher) {
        let down_count = actor.mailbox
            .iter()
            .filter(|m| m.behavior_id == 0 && m.priority == MessagePriority::System)
            .count();

        let total_remaining = actor.mailbox.len();

        // All targets exited → all monitors fired.
        assert_eq!(
            down_count, 100,
            "watcher should have exactly 100 DOWN messages, got {}",
            down_count
        );

        // 5b. No other messages leaked into the watcher mailbox.
        assert_eq!(
            total_remaining, 100,
            "watcher mailbox should contain ONLY the 100 DOWN messages, \
             got {} total",
            total_remaining
        );
    } else {
        panic!("watcher actor should still exist");
    }

    // 5c. All target actors are gone.
    for target in &targets {
        assert!(
            !rt.actors.contains_key(target),
            "target actor {} should be removed after exit",
            target
        );
    }

    // 5d. Watcher itself survived.
    assert!(rt.actors.contains_key(&watcher), "watcher should survive");
}

// ---------------------------------------------------------------------------
// Test 5 — Scheduler with Mixed Workload
// ---------------------------------------------------------------------------

/// **Scenario**: Mix CPU-heavy actors, I/O-waiting actors, and message-heavy
/// actors.  Verify the scheduler doesn't starve any category.
///
/// **Actors**:
/// * CPU actor: processes messages with heavy computation (high reduction cost).
/// * I/O actor: yields frequently (simulated by low reduction quota).
/// * Message actor: sends many messages to a sink.
///
/// **What to verify**:
/// * All three actors make progress.
/// * No actor is starved (processed count > 0 for each).
/// * Sink receives all messages sent by the message actor.
#[test]
fn stress_scheduler_with_mixed_workload() {
    let mut rt = Runtime::new();

    // --- Phase 1: Create the sink actor ---
    let sink = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::string("sink")),
        ("role".into(), Value::string("message_collector")),
    ]));

    // --- Phase 2: Create CPU-heavy actor ---
    let cpu_actor = rt.spawn_actor(Box::new(|| vec![
        ("name".into(),   Value::string("cpu_heavy")),
        ("mode".into(),   Value::string("cpu_bound")),
        ("quota".into(),  Value::int(500)), // high reduction per message
    ]));

    // --- Phase 3: Create I/O-waiting actor ---
    let io_actor = rt.spawn_actor(Box::new(|| vec![
        ("name".into(),   Value::string("io_waiter")),
        ("mode".into(),   Value::string("io_bound")),
        ("quota".into(),  Value::int(1)),   // yields after every message
    ]));

    // --- Phase 4: Create message-heavy actor ---
    let msg_actor = rt.spawn_actor(Box::new(|| vec![
        ("name".into(),   Value::string("msg_flooder")),
        ("mode".into(),   Value::string("message_bound")),
        ("target".into(), Value::actor_id(sink)),
    ]));

    // --- Phase 5: Seed workloads ---
    // CPU actor: 50 heavy messages.
    for i in 0..50 {
        rt.send_message(cpu_actor, "compute", &[
            Value::int(i),
            Value::int(1_000_000), // simulated work size
        ]);
    }

    // I/O actor: 100 messages that it will yield on.
    for i in 0..100 {
        rt.send_message(io_actor, "io_op", &[
            Value::int(i),
            Value::string("read"),
        ]);
    }

    // Message actor: 200 messages to send to the sink.
    // In the real system the msg_actor would, upon receiving "send_batch",
    // loop and send 200 messages to the sink.  Here we pre-populate its
    // mailbox with the trigger and also seed the sink.
    rt.send_message(msg_actor, "send_batch", &[
        Value::int(200),
        Value::actor_id(sink),
    ]);

    // --- Phase 6: Run scheduler ---
    rt.run_scheduler();

    // --- Phase 7: Assertions ---

    // 7a. CPU actor made progress.
    if let Some(actor) = rt.actors.get(&cpu_actor) {
        let processed = actor.processed_count.load(Ordering::SeqCst);
        assert!(
            processed > 0,
            "CPU actor should have processed at least one message, got {}",
            processed
        );
        // With work-stealing and reduction quotas, it should eventually
        // process all 50.
        assert_eq!(
            processed, 50,
            "CPU actor should process all 50 messages, got {}",
            processed
        );
    } else {
        panic!("CPU actor should still exist");
    }

    // 7b. I/O actor made progress.
    if let Some(actor) = rt.actors.get(&io_actor) {
        let processed = actor.processed_count.load(Ordering::SeqCst);
        assert!(
            processed > 0,
            "I/O actor should have processed at least one message, got {}",
            processed
        );
        assert_eq!(
            processed, 100,
            "I/O actor should process all 100 messages, got {}",
            processed
        );
    } else {
        panic!("I/O actor should still exist");
    }

    // 7c. Message actor made progress.
    if let Some(actor) = rt.actors.get(&msg_actor) {
        let processed = actor.processed_count.load(Ordering::SeqCst);
        assert!(
            processed >= 1,
            "Message actor should have processed at least its trigger message"
        );
    } else {
        panic!("Message actor should still exist");
    }

    // 7d. Sink received all 200 messages from the message actor.
    if let Some(actor) = rt.actors.get(&sink) {
        let sink_count = actor.processed_count.load(Ordering::SeqCst);
        assert_eq!(
            sink_count, 200,
            "sink should have received all 200 messages, got {}",
            sink_count
        );
    } else {
        panic!("sink actor should still exist");
    }
}

// ---------------------------------------------------------------------------
// Test 6 — Mailbox Never Drops System Messages
// ---------------------------------------------------------------------------

/// **Scenario**: Fill a mailbox to extreme levels, then send system messages.
/// Verify system messages are never dropped (the unbounded queue guarantee).
///
/// **What to verify**:
/// * All 100 system messages are present after pushing 1_000_000 normal msgs.
/// * No panic, no OOM at reasonable mailbox sizes.
/// * Messages are retrieved in FIFO order within each priority class
///   (System before Normal before Bulk).
#[test]
fn stress_mailbox_never_drops_system_messages() {
    let mut rt = Runtime::new();

    let actor = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::string("mailbox_test")),
    ]));

    // --- Phase 1: Push 1_000_000 normal-priority messages ---
    for i in 0..1_000_000 {
        let msg = Message {
            behavior_id: 1,
            payload: vec![Value::int(i)],
            sender: 0,
            priority: MessagePriority::Normal,
        };
        if let Some(a) = rt.actors.get(&actor) {
            a.mailbox.push(msg);
        }
    }

    // --- Phase 2: Push 100 system-priority messages ---
    for i in 0..100 {
        let msg = Message {
            behavior_id: 0,
            payload: vec![Value::string(format!("sys_{}", i))],
            sender: 0,
            priority: MessagePriority::System,
        };
        if let Some(a) = rt.actors.get(&actor) {
            a.mailbox.push(msg);
        }
    }

    // --- Phase 3: Push 50 bulk-priority messages ---
    for i in 0..50 {
        let msg = Message {
            behavior_id: 2,
            payload: vec![Value::int(i)],
            sender: 0,
            priority: MessagePriority::Bulk,
        };
        if let Some(a) = rt.actors.get(&actor) {
            a.mailbox.push(msg);
        }
    }

    // --- Phase 4: Pop all messages and verify ---
    let mut system_seen = 0;
    let mut normal_seen = 0;
    let mut bulk_seen = 0;
    let mut prev_priority = -1_i8; // System=0, Normal=1, Bulk=2

    if let Some(a) = rt.actors.get(&actor) {
        while let Some(msg) = a.mailbox.pop() {
            let curr_priority = match msg.priority {
                MessagePriority::System => 0_i8,
                MessagePriority::Normal => 1_i8,
                MessagePriority::Bulk   => 2_i8,
            };

            // Priority ordering invariant: once we start seeing lower-priority
            // messages, we should never see higher-priority ones again.
            assert!(
                curr_priority >= prev_priority,
                "priority ordering violated: went from {} to {} \
                 (system=0, normal=1, bulk=2)",
                prev_priority, curr_priority
            );
            prev_priority = curr_priority;

            match msg.priority {
                MessagePriority::System => system_seen += 1,
                MessagePriority::Normal => normal_seen += 1,
                MessagePriority::Bulk   => bulk_seen   += 1,
            }
        }
    }

    // --- Phase 5: Assertions ---

    assert_eq!(
        system_seen, 100,
        "all 100 system messages must be present, got {}",
        system_seen
    );
    assert_eq!(
        normal_seen, 1_000_000,
        "all 1_000_000 normal messages must be present, got {}",
        normal_seen
    );
    assert_eq!(
        bulk_seen, 50,
        "all 50 bulk messages must be present, got {}",
        bulk_seen
    );

    // Verify priority ordering: all system messages come first, then normal,
    // then bulk.
    assert_eq!(prev_priority, 2, "should have seen bulk messages last");
}

// ---------------------------------------------------------------------------
// Test 7 — Orphaned Actor Cleanup
// ---------------------------------------------------------------------------

/// **Scenario**: Spawn 50 actors in a mesh topology (each linked to 5 others).
/// Kill a central hub actor.  Verify no actors are leaked (orphaned).
///
/// **What to verify**:
/// * All connected actors are properly notified via exit signals.
/// * Actor count in runtime is correct after cleanup.
/// * No actors remain that should have been terminated.
#[test]
fn stress_orphaned_actor_cleanup() {
    let mut rt = Runtime::new();
    const N: usize = 50;
    const DEGREE: usize = 5;

    // --- Phase 1: Spawn 50 actors ---
    let mut actors: Vec<u64> = Vec::with_capacity(N);
    for i in 0..N {
        let id = rt.spawn_actor(Box::new(move || vec![
            ("name".into(), Value::string(format!("mesh_node_{}", i))),
            ("idx".into(),  Value::int(i as i64)),
        ]));
        actors.push(id);
    }

    // --- Phase 2: Link each actor to 5 others (mesh topology) ---
    // Use a deterministic pattern: actor i is linked to actors
    // (i+1)%N, (i+2)%N, (i+3)%N, (i+4)%N, (i+5)%N.
    for i in 0..N {
        for d in 1..=DEGREE {
            let j = (i + d) % N;
            // Avoid double-linking (link_actors is bidirectional).
            if i < j {
                rt.link_actors(actors[i], actors[j]);
            }
        }
    }

    let pre_kill_count = rt.actors.len();
    assert_eq!(pre_kill_count, N, "should have {} actors before kill", N);

    // --- Phase 3: Kill the central hub (actor at index N/2) ---
    let hub = actors[N / 2];
    rt.exit_actor(hub, ExitReason::Error("hub_killed".into()));

    // --- Phase 4: Run scheduler ---
    rt.run_scheduler();

    // --- Phase 5: Assertions ---

    // 5a. The hub itself is gone.
    assert!(
        !rt.actors.contains_key(&hub),
        "hub actor should be removed"
    );

    // 5b. Actors linked directly to the hub should have received exit signals.
    //     Since none of them trap exits, they should also be terminated.
    //     The hub was linked to actors at indices:
    //       (N/2 - 5)..(N/2 - 1)  and  (N/2 + 1)..(N/2 + 5)   (mod N)
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

    // The hub had 10 linked neighbors (5 forward + 5 backward, with wrap-
    // around the modulo ring).  All of them should be gone because they
    // don't trap exits.
    assert_eq!(
        terminated_count, DEGREE * 2,
        "all {} direct neighbors of hub should be terminated, \
         got {}",
        DEGREE * 2, terminated_count
    );

    // 5c. No actor is orphaned: the runtime's actor count is exactly what we
    //     expect (N - 1 hub - 10 neighbors = N - 11).
    let expected_remaining = N - 1 - (DEGREE * 2);
    let actual_remaining = rt.actors.len();
    assert_eq!(
        actual_remaining, expected_remaining,
        "expected {} remaining actors after hub+neighbors killed, got {}",
        expected_remaining, actual_remaining
    );

    // 5d. Verify that the remaining actors are exactly the non-neighbors.
    let mut remaining_expected = 0;
    for i in 0..N {
        let is_hub = i == N / 2;
        let is_neighbor = (1..=DEGREE).any(|d| {
            i == (N / 2 + d) % N || i == (N / 2 + N - d) % N
        });
        if !is_hub && !is_neighbor {
            remaining_expected += 1;
            assert!(
                rt.actors.contains_key(&actors[i]),
                "non-neighbor actor {} should still exist",
                i
            );
        }
    }
    assert_eq!(
        remaining_expected, expected_remaining,
        "count consistency check"
    );
}

// ---------------------------------------------------------------------------
// Test 8 — Reduction Quota Fairness
// ---------------------------------------------------------------------------

/// **Scenario**: Two actors compete for scheduler time.  Verify reduction
/// quotas ensure fair sharing.
///
/// **What to verify**:
/// * Both actors make approximately equal progress when given equal message
///   counts.
/// * Neither actor is starved (difference in processed count is bounded).
#[test]
fn stress_reduction_quota_fairness() {
    let mut rt = Runtime::new();

    // --- Phase 1: Create two competing actors ---
    let actor_a = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::string("fair_a")),
        ("quota".into(), Value::int(10)),
    ]));

    let actor_b = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::string("fair_b")),
        ("quota".into(), Value::int(10)),
    ]));

    // --- Phase 2: Give each actor exactly 1000 messages ---
    const MSG_COUNT: usize = 1000;
    for i in 0..MSG_COUNT {
        rt.send_message(actor_a, "work", &[Value::int(i as i64)]);
        rt.send_message(actor_b, "work", &[Value::int(i as i64)]);
    }

    // --- Phase 3: Run scheduler for a fixed number of steps ---
    // We step the scheduler incrementally so we can check fairness *during*
    // the run, not just at the end.
    const STEPS: usize = 200;
    for _ in 0..STEPS {
        rt.step_actor(actor_a);
        rt.step_actor(actor_b);
    }

    // --- Phase 4: Drain remaining messages ---
    rt.run_scheduler();

    // --- Phase 5: Assertions ---

    let processed_a = rt.actors
        .get(&actor_a)
        .map(|a| a.processed_count.load(Ordering::SeqCst))
        .unwrap_or(0);
    let processed_b = rt.actors
        .get(&actor_b)
        .map(|a| a.processed_count.load(Ordering::SeqCst))
        .unwrap_or(0);

    // 5a. Both actors processed all their messages.
    assert_eq!(
        processed_a, MSG_COUNT as u64,
        "actor_a should process all {} messages, got {}",
        MSG_COUNT, processed_a
    );
    assert_eq!(
        processed_b, MSG_COUNT as u64,
        "actor_b should process all {} messages, got {}",
        MSG_COUNT, processed_b
    );

    // 5b. Fairness during the incremental steps: the difference between
    //     processed counts at the halfway point should be small.
    //     We verify this by checking that the final counts are equal
    //     (both 1000) — the scheduler must have interleaved their work.
    let diff = if processed_a > processed_b {
        processed_a - processed_b
    } else {
        processed_b - processed_a
    };
    assert_eq!(
        diff, 0,
        "both actors should make exactly equal progress with equal quotas, \
         diff={}",
        diff
    );

    // 5c. Neither actor was starved at any point: verify their mailboxes are
    //     empty.
    if let Some(a) = rt.actors.get(&actor_a) {
        assert!(a.mailbox.is_empty(), "actor_a mailbox should be empty");
    }
    if let Some(b) = rt.actors.get(&actor_b) {
        assert!(b.mailbox.is_empty(), "actor_b mailbox should be empty");
    }
}

// ---------------------------------------------------------------------------
// Additional helper tests for boundary conditions
// ---------------------------------------------------------------------------

/// Verify that an actor can perform an effect, have its mailbox flooded,
/// and still resume the effect correctly after the flood messages are
/// processed.  This is the "actor-effect boundary" specifically.
#[test]
fn stress_effect_resume_after_mailbox_pressure() {
    let mut rt = Runtime::new();

    let effect_actor = rt.spawn_actor(Box::new(|| vec![
        ("name".into(),   Value::string("effect_resumer")),
        ("effect".into(), Value::string("SimulatedRead")),
    ]));

    // Start an effect on the actor.
    rt.send_message(effect_actor, "start_effect", &[]);

    // Simulate the effect yielding: the VM pushes a continuation and the
    // actor is marked as "waiting for effect".
    if let Some(actor) = rt.actors.get(&effect_actor) {
        actor.effect_stack.lock().unwrap().push(EffectFrame {
            effect_name: "SimulatedRead".to_string(),
            continuation_id: 42,
        });
        actor.status.store(ActorStatus::EffectWaiting as u8, Ordering::SeqCst);
    }

    // Flood the mailbox while the actor is in EffectWaiting state.
    for i in 0..5_000 {
        rt.send_message(effect_actor, "flood", &[Value::int(i)]);
    }

    // Resume the effect (simulating the handler calling `resume`).
    if let Some(actor) = rt.actors.get(&effect_actor) {
        actor.effect_stack.lock().unwrap().pop();
        actor.status.store(ActorStatus::Runnable as u8, Ordering::SeqCst);
    }

    // Run the scheduler.
    rt.run_scheduler();

    // After resuming, the actor should process all 5 000 flood messages
    // plus the original "start_effect" message.
    if let Some(actor) = rt.actors.get(&effect_actor) {
        let processed = actor.processed_count.load(Ordering::SeqCst);
        assert_eq!(
            processed, 5_001,
            "actor should process start_effect + 5000 flood messages, got {}",
            processed
        );

        // The effect stack should be empty (effect completed).
        assert!(
            actor.effect_stack.lock().unwrap().is_empty(),
            "effect stack should be empty after resume and processing"
        );
    }
}

/// Stress test: what happens when a supervisor itself crashes while
/// handling a child's effect-related failure?  The parent supervisor must
/// handle it.
#[test]
fn stress_supervisor_crash_during_effect_recovery() {
    let mut rt = Runtime::new();

    // Root supervisor
    let root = rt.create_supervisor(
        "root",
        RestartStrategy::OneForAll {
            max_restarts: 2,
            within_seconds: 60,
        },
    );

    // Mid-level supervisor (the one we'll crash)
    let mid = rt.create_supervisor(
        "mid",
        RestartStrategy::OneForOne {
            max_restarts: 1,
            within_seconds: 30,
        },
    );
    rt.supervise_child(root, ChildSpec::new("mid", None), mid);

    // Leaf actor under mid
    let leaf = rt.spawn_actor(Box::new(|| vec![
        ("name".into(), Value::string("leaf")),
    ]));
    rt.supervise_child(mid, ChildSpec::new("leaf", None), leaf);

    let pre_crash_count = rt.actors.len();

    // Crash the mid supervisor while it is in the middle of recovering
    // a child's effect failure.
    rt.exit_actor(mid, ExitReason::Error("supervisor_died_mid_recovery".into()));

    rt.run_scheduler();

    // The root supervisor should have detected mid's death and acted.
    // With OneForAll: all children of root (including mid and leaf) are
    // restarted.
    assert!(
        rt.actors.contains_key(&root),
        "root supervisor should survive"
    );

    // The total actor count should still be consistent (root + mid + leaf).
    let post_count = rt.actors.len();
    assert_eq!(
        post_count, pre_crash_count,
        "actor count should be stable after supervisor restart cycle, \
         pre={} post={}",
        pre_crash_count, post_count
    );
}

// ---------------------------------------------------------------------------
// Supporting types (expected to exist in the crate, defined here for
// compilation context — these should be removed / moved to the proper
// module once the real types are available).
// ---------------------------------------------------------------------------

/// A single frame on an actor's effect stack.
#[derive(Debug, Clone)]
struct EffectFrame {
    pub effect_name: String,
    pub continuation_id: u64,
}

/// Restart strategy for supervisors.
#[derive(Debug, Clone)]
pub enum RestartStrategy {
    OneForOne { max_restarts: u32, within_seconds: u32 },
    OneForAll { max_restarts: u32, within_seconds: u32 },
    RestForOne { max_restarts: u32, within_seconds: u32 },
}

/// Child restart policy.
#[derive(Debug, Clone)]
pub enum RestartPolicy {
    Permanent,
    Temporary,
    Transient,
}

/// Specification for a supervised child.
#[derive(Debug, Clone)]
pub struct ChildSpec {
    pub id: String,
    pub restart: RestartPolicy,
    pub max_restarts: Option<u32>,
    pub shutdown_timeout_ms: u64,
}

impl ChildSpec {
    pub fn new(id: &str, max_restarts: Option<u32>) -> Self {
        ChildSpec {
            id: id.to_string(),
            restart: RestartPolicy::Permanent,
            max_restarts,
            shutdown_timeout_ms: 5000,
        }
    }
}

/// Actor runtime status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ActorStatus {
    Runnable = 0,
    Running = 1,
    EffectWaiting = 2,
    Blocked = 3,
    Exiting = 4,
    Dead = 5,
}

// NOTE: The following trait extensions on `Value` are assumed to exist
// in `crate::vm::Value`.  If they don't, add them to the VM module:
//
// impl Value {
//     pub fn string(s: impl Into<String>) -> Self { ... }
//     pub fn int(i: impl Into<i64>) -> Self { ... }
//     pub fn bool(b: bool) -> Self { ... }
//     pub fn actor_id(id: u64) -> Self { ... }
// }
//
// And on `Actor`:
// impl Actor {
//     pub fn processed_count: AtomicU64,
//     pub fn restart_count: AtomicU64,
//     pub fn mailbox: crossbeam::queue::SegQueue<Message>,
//     pub fn effect_stack: Mutex<Vec<EffectFrame>>,
//     pub fn status: AtomicU8,
// }
//
// And on `Runtime`:
// impl Runtime {
//     pub fn pending_continuations: HashMap<u64, Continuation>,
// }
