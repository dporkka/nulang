//! MPSC bounded mailbox: atomic ring buffer per actor.

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

/// Bounded MPSC mailbox using atomic ring buffer.
pub struct Mailbox {
    buffer: Vec<Option<Message>>,
    capacity: usize,
    head: std::sync::atomic::AtomicUsize, // Read position
    tail: std::sync::atomic::AtomicUsize, // Write position
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
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two();
        let mut buffer = Vec::with_capacity(cap);
        buffer.resize_with(cap, || None::<Message>);
        Mailbox {
            buffer,
            capacity: cap,
            head: std::sync::atomic::AtomicUsize::new(0),
            tail: std::sync::atomic::AtomicUsize::new(0),
            overflow_policy: OverflowPolicy::DropOldest,
        }
    }

    /// Lock-free push into the MPSC mailbox.
    ///
    /// Uses a CAS loop on the tail index to reserve a slot, then writes
    /// the message. The head is loaded with Acquire ordering to synchronize
    /// with the consumer and detect a full buffer.
    pub fn push(&self, msg: Message) -> Result<(), Message> {
        loop {
            let tail = self.tail.load(std::sync::atomic::Ordering::Relaxed);
            let head = self.head.load(std::sync::atomic::Ordering::Acquire);

            // Check if mailbox is full
            if tail.wrapping_sub(head) >= self.capacity {
                match self.overflow_policy {
                    OverflowPolicy::Block => {
                        // In MVP, block means return the message as an error
                        return Err(msg);
                    }
                    OverflowPolicy::DropNewest => {
                        // Silently drop the incoming message
                        return Err(msg);
                    }
                    OverflowPolicy::DropOldest => {
                        // Remove the oldest message to make room, then retry
                        let _ = self.pop();
                        continue;
                    }
                    OverflowPolicy::Crash => {
                        panic!(
                            "Mailbox overflow: actor mailbox exceeded capacity {}",
                            self.capacity
                        );
                    }
                }
            }

            // Try to reserve a slot via CAS
            match self.tail.compare_exchange_weak(
                tail,
                tail.wrapping_add(1),
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let idx = tail & (self.capacity - 1);
                    // SAFETY: We own this slot (reserved via CAS on tail).
                    // No other producer will write to this slot.
                    unsafe {
                        let slot = self.buffer.as_ptr().add(idx) as *mut Option<Message>;
                        slot.write(Some(msg));
                    }
                    // Ensure the message write is visible before the tail
                    // update is observed by other threads.
                    std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
                    return Ok(());
                }
                Err(_) => {
                    // CAS failed, another producer reserved this slot.
                    // Retry with the updated tail value.
                    std::hint::spin_loop();
                    continue;
                }
            }
        }
    }

    /// Lock-free pop from the mailbox.
    ///
    /// The consumer (single thread per actor) reads the head index,
    /// checks against the tail (Acquire ordering to sync with producers),
    /// and takes the message from the buffer slot.
    pub fn pop(&self) -> Option<Message> {
        let head = self.head.load(std::sync::atomic::Ordering::Relaxed);
        let tail = self.tail.load(std::sync::atomic::Ordering::Acquire);

        if head >= tail {
            return None;
        }

        let idx = head & (self.capacity - 1);
        // SAFETY: Only the consumer (actor's thread) reads from head,
        // and we verified head < tail, so this slot contains a valid message.
        let msg = unsafe {
            let slot = self.buffer.as_ptr().add(idx) as *mut Option<Message>;
            (*slot).take()
        };

        self.head
            .store(head.wrapping_add(1), std::sync::atomic::Ordering::Relaxed);

        msg
    }

    pub fn len(&self) -> usize {
        let head = self.head.load(std::sync::atomic::Ordering::Acquire);
        let tail = self.tail.load(std::sync::atomic::Ordering::Acquire);
        tail.wrapping_sub(head)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() >= self.capacity
    }

    /// Read all messages without removing them.
    /// Returns a cloned snapshot of all current messages in the mailbox.
    pub fn drain(&self) -> Vec<Message> {
        let mut result = Vec::new();
        let head = self.head.load(std::sync::atomic::Ordering::Relaxed);
        let tail = self.tail.load(std::sync::atomic::Ordering::Acquire);
        let count = tail.wrapping_sub(head);

        for i in 0..count {
            let idx = head.wrapping_add(i) & (self.capacity - 1);
            // SAFETY: We only read slots that are between head and tail,
            // which are valid message slots.
            unsafe {
                let slot = self.buffer.as_ptr().add(idx) as *const Option<Message>;
                if let Some(ref msg) = *slot {
                    result.push(msg.clone());
                }
            }
        }

        result
    }
}
