use crate::types::{NuResult, Span, Value};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Actor ID
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActorId(pub u64);

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// Regular async message (no response expected)
    Async { behavior: String, args: Vec<Value> },
    /// Request expecting a response
    Ask { behavior: String, args: Vec<Value>, reply_to: ActorId },
    /// Response to an Ask
    Reply { value: Value },
    /// Actor exit notification
    Exit { actor: ActorId, reason: String },
    /// Monitor notification
    Down { actor: ActorId, reason: String },
}

// ---------------------------------------------------------------------------
// Message Priority
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessagePriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Urgent = 3,
    System = 4, // Exit, Down, Link signals
}

impl MessagePriority {
    pub fn of_message(msg: &Message) -> Self {
        match msg {
            Message::Exit { .. } | Message::Down { .. } => MessagePriority::System,
            Message::Reply { .. } => MessagePriority::High,
            Message::Ask { .. } => MessagePriority::Normal,
            Message::Async { .. } => MessagePriority::Normal,
        }
    }
}

// ---------------------------------------------------------------------------
// Mailbox (priority queue with per-actor ordering)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Mailbox {
    pub normal: Vec<Message>,
    pub high: Vec<Message>,
    pub system: Vec<Message>,
}

impl Mailbox {
    pub fn new() -> Self {
        Mailbox {
            normal: Vec::new(),
            high: Vec::new(),
            system: Vec::new(),
        }
    }

    pub fn send(&mut self, msg: Message) {
        match MessagePriority::of_message(&msg) {
            MessagePriority::System | MessagePriority::Urgent => self.system.push(msg),
            MessagePriority::High | MessagePriority::Normal => self.high.push(msg),
            MessagePriority::Low => self.normal.push(msg),
        }
    }

    pub fn receive(&mut self) -> Option<Message> {
        if let Some(m) = self.system.pop() {
            return Some(m);
        }
        if let Some(m) = self.high.pop() {
            return Some(m);
        }
        self.normal.pop()
    }

    pub fn is_empty(&self) -> bool {
        self.system.is_empty() && self.high.is_empty() && self.normal.is_empty()
    }

    pub fn len(&self) -> usize {
        self.system.len() + self.high.len() + self.normal.len()
    }
}

// ---------------------------------------------------------------------------
// Actor State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorState {
    Running,
    Suspended, // Waiting in a receive
    Exiting,   // Cleaning up before termination
    Exited,    // Terminated
}

// ---------------------------------------------------------------------------
// Actor
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Actor {
    pub id: ActorId,
    pub name: String,
    pub state: ActorState,
    pub mailbox: Mailbox,
    pub local_state: Value,
    pub behavior_table: Vec<String>, // names of available behaviors
    pub links: Vec<ActorId>,         // bidirectional links
    pub monitors: Vec<ActorId>,      // actors monitoring this one
    pub traps_exits: bool,           // if true, receive exit signals as messages
    pub reduction_quota: u32,        // how many instructions before yield
}

impl Actor {
    pub fn new(id: ActorId, name: impl Into<String>, initial_state: Value) -> Self {
        Actor {
            id,
            name: name.into(),
            state: ActorState::Running,
            mailbox: Mailbox::new(),
            local_state: initial_state,
            behavior_table: Vec::new(),
            links: Vec::new(),
            monitors: Vec::new(),
            traps_exits: false,
            reduction_quota: 1000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mailbox_send_receive() {
        let mut mb = Mailbox::new();
        mb.send(Message::Async { behavior: "tick".into(), args: vec![] });
        assert_eq!(mb.len(), 1);

        let msg = mb.receive().unwrap();
        assert!(matches!(msg, Message::Async { behavior, .. } if behavior == "tick"));
    }

    #[test]
    fn test_mailbox_priority() {
        let mut mb = Mailbox::new();
        mb.send(Message::Async { behavior: "a".into(), args: vec![] });
        mb.send(Message::Exit { actor: ActorId(1), reason: "test".into() });
        mb.send(Message::Async { behavior: "b".into(), args: vec![] });

        // System messages come first
        let msg = mb.receive().unwrap();
        assert!(matches!(msg, Message::Exit { .. }));
    }

    #[test]
    fn test_actor_creation() {
        let actor = Actor::new(ActorId(42), "test_actor", Value::Int(0));
        assert_eq!(actor.id, ActorId(42));
        assert_eq!(actor.state, ActorState::Running);
        assert!(actor.mailbox.is_empty());
    }
}
