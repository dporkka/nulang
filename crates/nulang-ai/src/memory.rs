//! Episodic memory for conversational LLM context.
//!
//! `EpisodicMemory` stores a bounded history of conversation turns and can
//! materialize them into provider-agnostic [`LlmMessage`] values that are
//! prepended to outgoing LLM requests.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::request::LlmMessage;

/// A single conversational turn stored in episodic memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// The role of the speaker, e.g. `"system"`, `"user"`, or `"assistant"`.
    pub role: String,
    /// The content of the turn.
    pub content: String,
}

/// A rolling buffer of conversation turns with a configurable size limit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpisodicMemory {
    /// Stored turns, oldest at the front.
    pub turns: VecDeque<Turn>,
    /// Maximum number of turns to retain.
    pub max_turns: usize,
}

impl EpisodicMemory {
    /// Create an empty memory buffer with the given retention limit.
    pub fn new(max_turns: usize) -> Self {
        Self {
            turns: VecDeque::new(),
            max_turns,
        }
    }

    /// Append a new turn to memory, evicting the oldest turn if over capacity.
    pub fn add_turn(&mut self, role: impl Into<String>, content: impl Into<String>) {
        self.turns.push_back(Turn {
            role: role.into(),
            content: content.into(),
        });
        while self.turns.len() > self.max_turns {
            self.turns.pop_front();
        }
    }

    /// Return the `n` most recent turns, oldest first.
    pub fn recent(&self, n: usize) -> Vec<&Turn> {
        let start = self.turns.len().saturating_sub(n);
        self.turns.iter().skip(start).collect()
    }

    /// Materialize all stored turns as [`LlmMessage`] values.
    pub fn to_messages(&self) -> Vec<LlmMessage> {
        self.turns
            .iter()
            .map(|t| LlmMessage {
                role: t.role.clone(),
                content: t.content.clone(),
            })
            .collect()
    }

    /// Clear all stored turns.
    pub fn clear(&mut self) {
        self.turns.clear();
    }

    /// Return the number of turns currently stored.
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// Return true if no turns are stored.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_memory() {
        let mem = EpisodicMemory::new(10);
        assert_eq!(mem.len(), 0);
        assert!(mem.to_messages().is_empty());
    }

    #[test]
    fn test_add_and_retrieve() {
        let mut mem = EpisodicMemory::new(10);
        mem.add_turn("user", "hello");
        mem.add_turn("assistant", "hi there");
        assert_eq!(mem.len(), 2);
        let msgs = mem.to_messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "hello");
    }

    #[test]
    fn test_max_turns_eviction() {
        let mut mem = EpisodicMemory::new(3);
        for i in 0..5 {
            mem.add_turn("user", format!("msg {}", i));
        }
        assert_eq!(mem.len(), 3);
        let msgs = mem.to_messages();
        assert_eq!(msgs[0].content, "msg 2");
        assert_eq!(msgs[2].content, "msg 4");
    }

    #[test]
    fn test_recent() {
        let mut mem = EpisodicMemory::new(10);
        for i in 0..5 {
            mem.add_turn("user", format!("msg {}", i));
        }
        let recent = mem.recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].content, "msg 3");
        assert_eq!(recent[1].content, "msg 4");
    }

    #[test]
    fn test_recent_less_than_n() {
        let mut mem = EpisodicMemory::new(10);
        mem.add_turn("user", "only");
        let recent = mem.recent(5);
        assert_eq!(recent.len(), 1);
    }

    #[test]
    fn test_clear() {
        let mut mem = EpisodicMemory::new(10);
        mem.add_turn("user", "hello");
        mem.clear();
        assert_eq!(mem.len(), 0);
    }

    #[test]
    fn test_zero_max_turns() {
        let mut mem = EpisodicMemory::new(0);
        mem.add_turn("user", "hello");
        assert_eq!(mem.len(), 0);
    }
}
