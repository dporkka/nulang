//! Bounded mailbox: atomic ring buffer.

use crate::types::Value;

/// Message sent to an actor.
#[derive(Debug, Clone)]
pub struct Message {
    pub behavior_id: u32,
    pub payload: Vec<Value>,
    pub sender: u32,
}

/// Lock-free bounded mailbox.
pub struct Mailbox {
    buffer: Vec<Option<Message>>,
    capacity: usize,
    head: std::sync::atomic::AtomicUsize,
    tail: std::sync::atomic::AtomicUsize,
}

impl Mailbox {
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two();
        let mut buffer = Vec::with_capacity(cap);
        for _ in 0..cap {
            buffer.push(None);
        }
        Mailbox {
            buffer,
            capacity: cap,
            head: std::sync::atomic::AtomicUsize::new(0),
            tail: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn send(&self, msg: Message) -> Result<(), MailboxError> {
        let tail = self.tail.load(std::sync::atomic::Ordering::Relaxed);
        let head = self.head.load(std::sync::atomic::Ordering::Acquire);
        if tail - head >= self.capacity {
            return Err(MailboxError::Full);
        }
        let idx = tail & (self.capacity - 1);
        self.buffer[idx].replace(msg);
        self.tail.store(tail + 1, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    pub fn receive(&self) -> Option<Message> {
        let head = self.head.load(std::sync::atomic::Ordering::Relaxed);
        let tail = self.tail.load(std::sync::atomic::Ordering::Acquire);
        if head >= tail {
            return None;
        }
        let idx = head & (self.capacity - 1);
        let msg = self.buffer[idx].take()?;
        self.head.store(head + 1, std::sync::atomic::Ordering::Release);
        Some(msg)
    }

    pub fn len(&self) -> usize {
        let tail = self.tail.load(std::sync::atomic::Ordering::Acquire);
        let head = self.head.load(std::sync::atomic::Ordering::Acquire);
        tail - head
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() >= self.capacity
    }
}

#[derive(Debug)]
pub enum MailboxError {
    Full,
}

impl std::fmt::Display for MailboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MailboxError::Full => write!(f, "Mailbox is full"),
        }
    }
}

impl std::error::Error for MailboxError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mailbox_send_receive() {
        let mb = Mailbox::new(4);
        let msg = Message { behavior_id: 0, payload: vec![Value::int(42)], sender: 0 };
        mb.send(msg.clone()).unwrap();
        let received = mb.receive().unwrap();
        assert_eq!(received.behavior_id, 0);
        assert_eq!(received.payload[0].as_int(), Some(42));
    }

    #[test]
    fn test_mailbox_fifo() {
        let mb = Mailbox::new(4);
        mb.send(Message { behavior_id: 0, payload: vec![Value::int(1)], sender: 0 }).unwrap();
        mb.send(Message { behavior_id: 1, payload: vec![Value::int(2)], sender: 0 }).unwrap();
        assert_eq!(mb.receive().unwrap().behavior_id, 0);
        assert_eq!(mb.receive().unwrap().behavior_id, 1);
    }

    #[test]
    fn test_mailbox_full() {
        let mb = Mailbox::new(2);
        mb.send(Message { behavior_id: 0, payload: vec![], sender: 0 }).unwrap();
        mb.send(Message { behavior_id: 1, payload: vec![], sender: 0 }).unwrap();
        assert!(mb.send(Message { behavior_id: 2, payload: vec![], sender: 0 }).is_err());
    }

    #[test]
    fn test_mailbox_empty() {
        let mb = Mailbox::new(4);
        assert!(mb.receive().is_none());
        assert!(mb.is_empty());
    }
}
