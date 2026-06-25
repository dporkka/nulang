//! MPSC bounded mailbox: lock-free queue per actor using crossbeam's
//! `ArrayQueue`.
//!
//! Replaces the hand-rolled atomic CAS ring buffer with `crossbeam::queue::ArrayQueue`
//! for:
//! - Epoch-based memory reclamation (eliminates ABA problems)
//! - Cache-line-optimized slot layout (head/tail stamps padded to cache lines)
//! - Battle-tested lock-free FIFO correctness (Vyukov's MPMC algorithm)

use crossbeam::queue::ArrayQueue;
use crate::vm::Value;

/// Message sent between actors.
#[derive(Debug, Clone)]
pub struct Message {
    pub behavior_id: u16,
    pub payload: Vec<Value>,
    pub sender: u64, // Actor ID of sender
    pub priority: MessagePriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePriority {
    System = 0,  // Urgent (failure signals, monitoring)
    Normal = 1,  // Regular messages
    Bulk = 2,    // Bulk/non-urgent
}

/// Bounded MPSC mailbox backed by `crossbeam::queue::ArrayQueue`.
///
/// Uses Vyukov's cache-line-padded MPMC queue algorithm with stamp-based
/// ABA protection. Each slot carries a generation stamp so that stale
/// CAS operations are detected and retried automatically.
pub struct Mailbox {
    queue: ArrayQueue<Message>,
    capacity: usize,
    overflow_policy: OverflowPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowPolicy {
    Block,       // Backpressure: sender blocks/waits
    DropOldest,  // Remove oldest message
    DropNewest,  // Drop the new incoming message
    Crash,       // Actor crashes on overflow
}

impl Mailbox {
    /// Create a new mailbox with the given capacity.
    ///
    /// Capacity is rounded up to the next power of two (as required by
    /// `ArrayQueue` for efficient stamp-to-index masking).
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two();
        Mailbox {
            queue: ArrayQueue::new(cap),
            capacity: cap,
            overflow_policy: OverflowPolicy::DropOldest,
        }
    }

    /// Set the overflow policy for this mailbox.
    pub fn set_overflow_policy(&mut self, policy: OverflowPolicy) {
        self.overflow_policy = policy;
    }

    /// Return the mailbox capacity (rounded up to the next power of two).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Lock-free push into the MPSC mailbox.
    pub fn push(&self, msg: Message) -> Result<(), Message> {
        match self.queue.push(msg) {
            Ok(()) => Ok(()),
            Err(msg) => {
                match self.overflow_policy {
                    OverflowPolicy::Block => Err(msg),
                    OverflowPolicy::DropNewest => Err(msg),
                    OverflowPolicy::DropOldest => {
                        let _ = self.queue.pop();
                        self.queue.push(msg).map_err(|e| e)
                    }
                    OverflowPolicy::Crash => {
                        panic!("Mailbox overflow: actor mailbox exceeded capacity {}", self.capacity);
                    }
                }
            }
        }
    }

    /// Lock-free pop from the mailbox.
    pub fn pop(&self) -> Option<Message> {
        self.queue.pop()
    }

    pub fn len(&self) -> usize { self.queue.len() }
    pub fn is_empty(&self) -> bool { self.queue.is_empty() }
    pub fn is_full(&self) -> bool { self.queue.is_full() }

    /// Return a cloned snapshot of all messages currently in the mailbox.
    pub fn drain(&self) -> Vec<Message> {
        let mut snapshot = Vec::with_capacity(self.len());
        while let Some(msg) = self.queue.pop() {
            snapshot.push(msg);
        }
        for msg in &snapshot {
            if self.queue.push(msg.clone()).is_err() {}
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(behavior_id: u16, sender: u64) -> Message {
        Message { behavior_id, payload: vec![Value::int(42)], sender, priority: MessagePriority::Normal }
    }

    #[test]
    fn test_push_and_pop() {
        let mb = Mailbox::new(4);
        let msg = make_msg(1, 100);
        assert!(mb.is_empty());
        mb.push(msg.clone()).unwrap();
        assert_eq!(mb.len(), 1);
        let popped = mb.pop().unwrap();
        assert_eq!(popped.behavior_id, 1);
        assert!(mb.is_empty());
    }

    #[test]
    fn test_overflow_block() {
        let mut mb = Mailbox::new(2);
        mb.set_overflow_policy(OverflowPolicy::Block);
        mb.push(make_msg(1, 10)).unwrap();
        mb.push(make_msg(2, 20)).unwrap();
        assert!(mb.push(make_msg(3, 30)).is_err());
        assert_eq!(mb.len(), 2);
    }

    #[test]
    fn test_overflow_drop_oldest() {
        let mut mb = Mailbox::new(2);
        mb.set_overflow_policy(OverflowPolicy::DropOldest);
        mb.push(make_msg(1, 10)).unwrap();
        mb.push(make_msg(2, 20)).unwrap();
        mb.push(make_msg(3, 30)).unwrap();
        assert_eq!(mb.pop().unwrap().behavior_id, 2);
        assert_eq!(mb.pop().unwrap().behavior_id, 3);
    }

    #[test]
    fn test_fifo_ordering() {
        let mb = Mailbox::new(8);
        for i in 0..5 { mb.push(make_msg(i as u16, i as u64)).unwrap(); }
        for i in 0..5 { assert_eq!(mb.pop().unwrap().behavior_id, i as u16); }
        assert!(mb.is_empty());
    }

    #[test]
    fn test_drain_snapshot() {
        let mb = Mailbox::new(4);
        mb.push(make_msg(1, 10)).unwrap();
        mb.push(make_msg(2, 20)).unwrap();
        let snapshot = mb.drain();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(mb.len(), 2);
    }

    #[test]
    #[should_panic(expected = "Mailbox overflow")]
    fn test_overflow_crash() {
        let mut mb = Mailbox::new(1);
        mb.set_overflow_policy(OverflowPolicy::Crash);
        mb.push(make_msg(1, 10)).unwrap();
        mb.push(make_msg(2, 20)).unwrap();
    }
}
