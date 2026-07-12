//! Unbounded MPSC mailbox: lock-free queue per actor using crossbeam's
//! `SegQueue`.
//!
//! BEAM/OTP semantics assume unbounded actor mailboxes. A bounded queue
//! forces a destructive trade-off between blocking senders (cascading
//! scheduler deadlocks) and dropping messages (violating actor reliability
//! guarantees — supervisor signals must never be lost).
//!
//! Uses `crossbeam::queue::SegQueue`, a segmented lock-free queue that
//! grows dynamically. Memory is reclaimed via crossbeam's epoch-based
//! garbage collection.
//!
//! Backpressure is handled at the language level (actor-level flow
//! control) rather than at the mailbox transport level.

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

/// Unbounded MPSC mailbox backed by `crossbeam::queue::SegQueue`.
///
/// The queue grows dynamically as messages are pushed. There is no
/// capacity limit, no overflow policy, and no blocking on push.
///
/// Memory is reclaimed via crossbeam's epoch-based memory management,
/// so popped segments are freed only after all concurrent readers have
/// exited the critical section.
pub struct Mailbox {
    queue: SegQueue<Message>,
}

impl Mailbox {
    /// Create a new unbounded mailbox.
    pub fn new(_capacity: usize) -> Self {
        // Capacity argument is ignored — the queue is unbounded.
        // Kept for API compatibility with existing Actor::new() calls.
        Mailbox {
            queue: SegQueue::new(),
        }
    }

    /// Lock-free push into the MPSC mailbox.
    ///
    /// Always succeeds — the queue is unbounded. Never blocks, never
    /// drops messages. This preserves BEAM/OTP reliability guarantees:
    /// supervisor exit signals and monitor DOWN messages are never lost
    /// in transit.
    pub fn push(&self, msg: Message) -> Result<(), Message> {
        self.queue.push(msg);
        Ok(())
    }

    /// Lock-free pop from the mailbox.
    ///
    /// Delegates to `SegQueue::pop`. Returns `None` if the mailbox is empty.
    pub fn pop(&self) -> Option<Message> {
        self.queue.pop()
    }

    /// Selective receive: scan the mailbox in FIFO order for the first
    /// message whose behavior id appears in `behavior_ids`.
    ///
    /// Matching is by mailbox order, not arm order: the first message that
    /// matches ANY id wins. Skipped (non-matching) messages are requeued in
    /// their original order, so relative FIFO order is preserved.
    ///
    /// Returns `Some((arm_index, payload))` where `arm_index` is the
    /// position of the matched id within `behavior_ids`, or `None` when no
    /// queued message matches.
    pub fn receive_match(&self, behavior_ids: &[u16]) -> Option<(usize, Vec<Value>)> {
        let mut skipped: Vec<Message> = Vec::new();
        let mut found = None;
        while let Some(msg) = self.queue.pop() {
            if let Some(pos) = behavior_ids.iter().position(|&id| id == msg.behavior_id) {
                found = Some((pos, msg.payload));
                break;
            }
            skipped.push(msg);
        }
        for msg in skipped {
            self.queue.push(msg);
        }
        found
    }

    /// Return the current number of messages in the mailbox.
    ///
    /// Note: `SegQueue::len` is approximate — concurrent push/pop
    /// operations may cause the returned value to be slightly stale.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Return `true` if the mailbox contains no messages.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Return a cloned snapshot of all messages currently in the mailbox.
    ///
    /// Since the queue is unbounded, there is no risk of the snapshot
    /// failing due to capacity constraints (unlike the bounded ArrayQueue
    /// version where concurrent pushes could consume freed slots during
    /// the restore phase).
    pub fn drain(&self) -> Vec<Message> {
        let mut snapshot = Vec::with_capacity(self.len());
        while let Some(msg) = self.queue.pop() {
            snapshot.push(msg);
        }
        // Restore all popped messages. With an unbounded queue, there
        // is always room to restore.
        for msg in &snapshot {
            self.queue.push(msg.clone());
        }
        snapshot
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
        let mb = Mailbox::new(2); // capacity argument is ignored

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

        let mb = Arc::new(Mailbox::new(4));
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
}
