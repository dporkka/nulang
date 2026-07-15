//! MPSC mailbox with priority bands and optional capacity limit.
//!
//! Two priority bands (`System` and `Normal`/`Bulk`) ensure that supervisor
//! exit signals and monitor DOWN messages are never delayed behind a queue
//! of regular application messages.  When a capacity limit is configured,
//! `System` messages always bypass the limit — preserving BEAM/OTP
//! reliability guarantees — while `Normal` and `Bulk` messages are
//! rejected with backpressure when the mailbox is full.
//!
//! Uses `crossbeam::queue::SegQueue` (lock-free, unbounded segments) for
//! each band.  Memory is reclaimed via crossbeam's epoch-based garbage
//! collection.

use crate::vm::Value;
use crossbeam::queue::SegQueue;

/// Message sent between actors.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub behavior_id: u16,
    pub payload: Vec<Value>,
    pub sender: u64, // Actor ID of sender
    pub priority: MessagePriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePriority {
    System = 0, // Urgent (failure signals, monitoring)
    Normal = 1, // Regular messages
    Bulk = 2,   // Bulk/non-urgent
}

/// MPSC mailbox with priority bands and optional capacity.
///
/// Two `SegQueue` instances provide priority ordering without starving
/// normal messages: every `pop` / `receive_match` drains the system band
/// completely before touching the normal band.
///
/// When `capacity > 0`, `push` rejects `Normal` and `Bulk` messages once
/// the total message count reaches the limit.  `System` messages always
/// succeed, preserving BEAM/OTP reliability guarantees.
pub struct Mailbox {
    system_queue: SegQueue<Message>,
    normal_queue: SegQueue<Message>,
    capacity: usize,
}

impl Mailbox {
    /// Create a new mailbox.
    ///
    /// `capacity`: maximum total messages allowed.  `0` = unbounded
    /// (BEAM/OTP semantics).  `System` messages always bypass the limit.
    pub fn new(capacity: usize) -> Self {
        Mailbox {
            system_queue: SegQueue::new(),
            normal_queue: SegQueue::new(),
            capacity,
        }
    }

    /// Push a message into the mailbox.
    ///
    /// `System` messages always succeed.  `Normal` and `Bulk` messages are
    /// rejected with `Err(msg)` when the mailbox is at capacity (a
    /// non-zero `capacity` was configured and both queues together hold
    /// that many messages).
    pub fn push(&self, msg: Message) -> Result<(), Message> {
        if msg.priority == MessagePriority::System {
            self.system_queue.push(msg);
            return Ok(());
        }
        if self.capacity > 0 && self.len() >= self.capacity {
            return Err(msg);
        }
        self.normal_queue.push(msg);
        Ok(())
    }

    /// Pop the highest-priority message.
    ///
    /// Always drains the system queue first; falls back to the normal
    /// queue only when no system messages are pending.
    pub fn pop(&self) -> Option<Message> {
        self.system_queue.pop().or_else(|| self.normal_queue.pop())
    }

    /// Selective receive: scan both queues in priority order for the first
    /// message whose behavior id appears in `behavior_ids`.
    ///
    /// System messages are scanned first, preserving priority even across
    /// selective dispatch.  Non-matching messages are re-queued into their
    /// original band so relative FIFO order is preserved.
    pub fn receive_match(&self, behavior_ids: &[u16]) -> Option<(usize, Vec<Value>)> {
        // Scan system queue first.
        if let Some(result) = Self::scan_queue(&self.system_queue, behavior_ids) {
            return Some(result);
        }
        // Then scan normal queue.
        Self::scan_queue(&self.normal_queue, behavior_ids)
    }

    /// Drain and scan a single queue for a matching message.
    fn scan_queue(
        queue: &SegQueue<Message>,
        behavior_ids: &[u16],
    ) -> Option<(usize, Vec<Value>)> {
        let mut drained: Vec<Message> = Vec::new();
        while let Some(msg) = queue.pop() {
            drained.push(msg);
        }
        let mut found = None;
        let mut requeue: Vec<Message> = Vec::with_capacity(drained.len());
        for msg in drained {
            if found.is_none() {
                if let Some(pos) = behavior_ids.iter().position(|&id| id == msg.behavior_id) {
                    found = Some((pos, msg.payload));
                    continue;
                }
            }
            requeue.push(msg);
        }
        for msg in requeue {
            queue.push(msg);
        }
        found
    }

    /// Total message count across both queues (approximate).
    pub fn len(&self) -> usize {
        self.system_queue.len() + self.normal_queue.len()
    }

    /// True when both queues are empty.
    pub fn is_empty(&self) -> bool {
        self.system_queue.is_empty() && self.normal_queue.is_empty()
    }

    /// Drain both queues (system first) into a cloned snapshot, then
    /// restore all messages.
    pub fn drain(&self) -> Vec<Message> {
        let mut snapshot = Vec::with_capacity(self.len());
        // Drain system first.
        while let Some(msg) = self.system_queue.pop() {
            snapshot.push(msg);
        }
        while let Some(msg) = self.normal_queue.pop() {
            snapshot.push(msg);
        }
        // Restore: system messages go back to system_queue, normal to normal_queue.
        for msg in &snapshot {
            if msg.priority == MessagePriority::System {
                self.system_queue.push(msg.clone());
            } else {
                self.normal_queue.push(msg.clone());
            }
        }
        snapshot
    }

    /// Return the configured capacity (0 = unbounded).
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build a test message with minimal boilerplate.
    fn make_msg(behavior_id: u16, sender: u64) -> Message {
        Message {
            behavior_id,
            payload: vec![Value::int(42)],
            sender,
            priority: MessagePriority::Normal,
        }
    }

    // Test 1: Basic push/pop round-trip.
    #[test]
    fn test_push_and_pop() {
        let mb = Mailbox::new(4);
        let msg = make_msg(1, 100);

        assert!(mb.is_empty());
        assert_eq!(mb.len(), 0);

        mb.push(msg.clone()).unwrap();
        assert!(!mb.is_empty());
        assert_eq!(mb.len(), 1);

        let popped = mb.pop().unwrap();
        assert_eq!(popped.behavior_id, 1);
        assert_eq!(popped.sender, 100);
        assert_eq!(popped.payload, vec![Value::int(42)]);

        assert!(mb.is_empty());
        assert_eq!(mb.pop(), None);
    }

    // Test 2: Unbounded — push never fails, even with many messages.
    #[test]
    fn test_unbounded_never_fails() {
        let mb = Mailbox::new(0); // 0 = unbounded

        for i in 0..10000 {
            let result = mb.push(make_msg(i as u16, i as u64));
            assert!(
                result.is_ok(),
                "push {} should never fail on unbounded queue",
                i
            );
        }
        assert_eq!(mb.len(), 10000);

        // Pop all messages
        for i in 0..10000 {
            let msg = mb.pop().expect(&format!("pop {} should succeed", i));
            assert_eq!(msg.behavior_id, i as u16);
        }
        assert!(mb.is_empty());
    }

    // Test 3: Supervisor signals never dropped.
    #[test]
    fn test_supervisor_signals_never_dropped() {
        let mb = Mailbox::new(4);

        // Flood with system-priority exit signals
        for i in 0..1000 {
            let signal = Message {
                behavior_id: 0, // System message
                payload: vec![Value::int(i)],
                sender: i as u64,
                priority: MessagePriority::System,
            };
            mb.push(signal).unwrap();
        }

        // All 1000 signals must be present
        assert_eq!(mb.len(), 1000);

        // Verify every signal is recoverable
        let mut count = 0;
        while mb.pop().is_some() {
            count += 1;
        }
        assert_eq!(count, 1000, "no supervisor signals should be lost");
    }

    // Test 4: len and is_empty track correctly across operations.
    #[test]
    fn test_len_and_is_empty() {
        let mb = Mailbox::new(4);
        assert!(mb.is_empty());
        assert_eq!(mb.len(), 0);

        mb.push(make_msg(10, 1)).unwrap();
        assert!(!mb.is_empty());
        assert_eq!(mb.len(), 1);

        mb.push(make_msg(20, 2)).unwrap();
        mb.push(make_msg(30, 3)).unwrap();
        assert_eq!(mb.len(), 3);

        mb.pop().unwrap();
        assert_eq!(mb.len(), 2);

        mb.pop().unwrap();
        mb.pop().unwrap();
        assert!(mb.is_empty());
        assert_eq!(mb.len(), 0);
    }

    // Test 5: drain returns a cloned snapshot without removing messages.
    #[test]
    fn test_drain_snapshot() {
        let mb = Mailbox::new(4);
        mb.push(make_msg(1, 10)).unwrap();
        mb.push(make_msg(2, 20)).unwrap();
        mb.push(make_msg(3, 30)).unwrap();

        let snapshot = mb.drain();
        assert_eq!(snapshot.len(), 3);
        assert_eq!(snapshot[0].behavior_id, 1);
        assert_eq!(snapshot[1].behavior_id, 2);
        assert_eq!(snapshot[2].behavior_id, 3);

        // Mailbox should still contain all messages after drain.
        assert_eq!(mb.len(), 3);
        assert_eq!(mb.pop().unwrap().behavior_id, 1);
        assert_eq!(mb.pop().unwrap().behavior_id, 2);
        assert_eq!(mb.pop().unwrap().behavior_id, 3);
    }

    // Test 6: Concurrent push from multiple threads.
    #[test]
    fn test_concurrent_push() {
        use std::sync::Arc;
        use std::thread;

        let mb = Arc::new(Mailbox::new(0)); // 0 = unbounded for concurrent test
        let mut handles = Vec::new();

        for t in 0..4 {
            let mb_clone = Arc::clone(&mb);
            handles.push(thread::spawn(move || {
                for i in 0..100 {
                    mb_clone
                        .push(make_msg((t * 100 + i) as u16, (t * 100 + i) as u64))
                        .unwrap();
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // All 400 messages should be present
        assert_eq!(mb.len(), 400);

        let mut count = 0;
        while mb.pop().is_some() {
            count += 1;
        }
        assert_eq!(count, 400);
    }

    // Test 7: receive_match preserves the relative FIFO order of ALL
    // non-matched messages, including those queued behind the match.
    #[test]
    fn test_receive_match_preserves_skipped_order() {
        let mb = Mailbox::new(4);
        mb.push(make_msg(1, 100)).unwrap(); // A: skipped (no match)
        mb.push(make_msg(2, 200)).unwrap(); // B: matched
        mb.push(make_msg(3, 300)).unwrap(); // C: queued behind the match

        let found = mb.receive_match(&[2]);
        assert_eq!(found, Some((0, vec![Value::int(42)])));

        // The mailbox must still serve A before C: selective receive only
        // removes the matched message, it must not reorder the rest.
        assert_eq!(mb.len(), 2);
        assert_eq!(mb.pop().unwrap().behavior_id, 1);
        assert_eq!(mb.pop().unwrap().behavior_id, 3);
        assert!(mb.is_empty());
    }
}
