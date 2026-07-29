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
use std::collections::VecDeque;
use std::sync::Arc;

/// Message sent between actors.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub behavior_id: u16,
    /// Payload values, shared via `Arc` to avoid cloning on every
    /// `receive_match` scan. The VM never mutates incoming payloads,
    /// so `Arc` is safe.
    pub payload: Arc<Vec<Value>>,
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
///
/// All methods that access the skip-buffer take `&mut self` because they
/// run exclusively on the single scheduler thread — no `RefCell` needed.
pub struct Mailbox {
    system_queue: SegQueue<Message>,
    normal_queue: SegQueue<Message>,
    capacity: usize,
    /// Skip-buffer for non-matching normal messages drained during selective
    /// receive (`receive_match`). Messages stay here in FIFO order until a
    /// later `receive_match` finds a match. System messages are NOT placed
    /// here — they are scanned directly from `system_queue`.
    skip_buffer: VecDeque<(Message, bool)>,
}

// SAFETY: `Mailbox` is `Sync` because all mutable state (`skip_buffer`)
// is accessed exclusively from the scheduler thread (all `receive_match`/
// `pop`/`flush_skip_buffer` calls happen within `step_actor` or the VM's
// `ReceiveMatch` handler, both running on the single scheduler thread).
// The `SegQueue` fields are already `Sync` (lock-free concurrent queues)
// and may be safely pushed from multiple threads.
unsafe impl Sync for Mailbox {}

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
            skip_buffer: VecDeque::new(),
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
    /// Checks the system queue first (priority), then the skip-buffer
    /// (non-matching normal messages staged during a prior `receive_match`),
    /// then the normal queue.
    pub fn pop(&mut self) -> Option<Message> {
        self.system_queue
            .pop()
            .or_else(|| self.skip_buffer.pop_front().map(|(m, _)| m))
            .or_else(|| self.normal_queue.pop())
    }

    /// Selective receive: scan for the first message whose behavior id
    /// appears in `behavior_ids`.
    ///
    /// System messages are scanned first (via `scan_queue` — they are rare
    /// and must preserve priority).  Normal messages use the skip-buffer:
    /// on the first call the `normal_queue` is drained into the buffer and
    /// scanned; non-matching messages stay in the buffer so the next call
    /// does not re-drain the concurrent queue.  This makes repeated
    /// selective receive O(skipped) amortized instead of O(N) per call.
    pub fn receive_match(&mut self, behavior_ids: &[u16]) -> Option<(usize, Arc<Vec<Value>>)> {
        // Scan system queue first (small, rare — drain-scan-requeue is fine).
        if let Some(result) = Self::scan_queue(&self.system_queue, behavior_ids) {
            return Some(result);
        }
        // Try the skip-buffer: scan for the first un-tried message whose
        // behavior id matches. Mark it "tried" and return a clone of its
        // payload. The message stays in the buffer until `commit_receive_match`
        // removes it or `reset_receive_match` clears the tried flag.
        for i in 0..self.skip_buffer.len() {
            let (tried, bid) = (self.skip_buffer[i].1, self.skip_buffer[i].0.behavior_id);
            if !tried {
                if let Some(pos) = behavior_ids.iter().position(|&id| id == bid) {
                    self.skip_buffer[i].1 = true; // mark tried
                    return Some((pos, Arc::clone(&self.skip_buffer[i].0.payload)));
                }
            }
        }
        // Drain the normal queue into the buffer, then scan again.
        while let Some(msg) = self.normal_queue.pop() {
            self.skip_buffer.push_back((msg, false));
        }
        for i in 0..self.skip_buffer.len() {
            let (tried, bid) = (self.skip_buffer[i].1, self.skip_buffer[i].0.behavior_id);
            if !tried {
                if let Some(pos) = behavior_ids.iter().position(|&id| id == bid) {
                    self.skip_buffer[i].1 = true; // mark tried
                    return Some((pos, Arc::clone(&self.skip_buffer[i].0.payload)));
                }
            }
        }
        None
    }

    /// Drain and scan a single queue for a matching message.  Used for the
    /// system queue only (small, rare); the normal queue uses the skip-buffer.
    fn scan_queue(
        queue: &SegQueue<Message>,
        behavior_ids: &[u16],
    ) -> Option<(usize, Arc<Vec<Value>>)> {
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

    /// Total message count across system queue, skip-buffer, and normal
    /// queue (approximate — concurrent queue lengths are snapshots).
    pub fn len(&self) -> usize {
        self.system_queue.len() + self.skip_buffer.len() + self.normal_queue.len()
    }

    /// True when all queues and the skip-buffer are empty.
    pub fn is_empty(&self) -> bool {
        self.system_queue.is_empty() && self.skip_buffer.is_empty() && self.normal_queue.is_empty()
    }

    /// Drain system queue, skip-buffer, and normal queue (in priority/FIFO
    /// order) into a cloned snapshot, then restore all messages.
    pub fn drain(&mut self) -> Vec<Message> {
        let mut snapshot = Vec::with_capacity(self.len());
        // Drain system first.
        while let Some(msg) = self.system_queue.pop() {
            snapshot.push(msg);
        }
        // Then skip-buffer (normal messages staged during selective receive).
        while let Some((msg, _)) = self.skip_buffer.pop_front() {
            snapshot.push(msg);
        }
        // Then normal queue.
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

    /// Return all skip-buffer messages to `normal_queue`, then clear the buffer.
    pub fn flush_skip_buffer(&mut self) {
        while let Some((msg, _)) = self.skip_buffer.pop_front() {
            self.normal_queue.push(msg);
        }
    }

    /// Return the configured capacity (0 = unbounded).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Commit a selective receive: remove the first "tried" message from
    /// the skip-buffer and clear remaining "tried" flags. Called after a
    /// pattern+guard check succeeds.
    pub fn commit_receive_match(&mut self) {
        // Remove the first tried entry.
        if let Some(idx) = self.skip_buffer.iter().position(|(_, tried)| *tried) {
            self.skip_buffer.remove(idx);
        }
        // Clear remaining tried flags.
        for (_, tried) in self.skip_buffer.iter_mut() {
            *tried = false;
        }
    }

    /// Reset "tried" flags in the skip-buffer. Called when
    /// `receive_match` returns `None`, preparing the buffer for the next
    /// receive expression.
    pub fn reset_receive_match(&mut self) {
        for (_, tried) in self.skip_buffer.iter_mut() {
            *tried = false;
        }
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
            payload: Arc::new(vec![Value::int(42)]),
            sender,
            priority: MessagePriority::Normal,
        }
    }

    // Test 1: Basic push/pop round-trip.
    #[test]
    fn test_push_and_pop() {
        let mut mb = Mailbox::new(4);
        let msg = make_msg(1, 100);

        assert!(mb.is_empty());
        assert_eq!(mb.len(), 0);

        mb.push(msg.clone()).unwrap();
        assert!(!mb.is_empty());
        assert_eq!(mb.len(), 1);

        let popped = mb.pop().unwrap();
        assert_eq!(popped.behavior_id, 1);
        assert_eq!(popped.sender, 100);
        assert_eq!(*popped.payload, vec![Value::int(42)]);

        assert!(mb.is_empty());
        assert_eq!(mb.pop(), None);
    }

    // Test 2: Unbounded — push never fails, even with many messages.
    #[test]
    fn test_unbounded_never_fails() {
        let mut mb = Mailbox::new(0); // 0 = unbounded

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

    #[test]
    fn test_supervisor_signals_never_dropped() {
        let mut mb = Mailbox::new(4);

        // Flood with system-priority exit signals
        for i in 0..1000 {
            let signal = Message {
                behavior_id: 0, // System message
                payload: Arc::new(vec![Value::int(i)]),
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
        let mut mb = Mailbox::new(4);
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
        let mut mb = Mailbox::new(4);
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

        // Recover the owned Mailbox so we can call &mut self methods.
        let mut mb = Arc::try_unwrap(mb).unwrap_or_else(|_| panic!("Arc still has live clones"));
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
        let mut mb = Mailbox::new(4);
        mb.push(make_msg(1, 100)).unwrap(); // A: skipped (no match)
        mb.push(make_msg(2, 200)).unwrap(); // B: matched
        mb.push(make_msg(3, 300)).unwrap(); // C: queued behind the match

        let found = mb.receive_match(&[2]);
        assert_eq!(found, Some((0, Arc::new(vec![Value::int(42)]))));
        // Commit: remove the matched ("tried") message from the skip-buffer.
        mb.commit_receive_match();

        // The mailbox must still serve A before C: selective receive only
        // removes the matched message, it must not reorder the rest.
        assert_eq!(mb.len(), 2);
        assert_eq!(mb.pop().unwrap().behavior_id, 1);
        assert_eq!(mb.pop().unwrap().behavior_id, 3);
        assert!(mb.is_empty());
    }
}
