//! Episodic memory for conversational LLM context.
//!
//! `EpisodicMemory` stores a bounded history of conversation turns and can
//! materialize them into provider-agnostic [`LlmMessage`] values that are
//! prepended to outgoing LLM requests.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::ai::request::LlmMessage;

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
        if self.max_turns == 0 {
            return;
        }
        if self.turns.len() >= self.max_turns {
            self.turns.pop_front();
        }
        self.turns.push_back(Turn {
            role: role.into(),
            content: content.into(),
        });
    }

    /// Return the `n` most recent turns, oldest first.
    pub fn recent(&self, n: usize) -> Vec<&Turn> {
        self.turns.iter().rev().take(n).collect::<Vec<_>>().into_iter().rev().collect()
    }

    /// Materialize all stored turns as [`LlmMessage`] values.
    pub fn to_messages(&self) -> Vec<LlmMessage> {
        self.turns
            .iter()
            .map(|turn| LlmMessage {
                role: turn.role.clone(),
                content: turn.content.clone(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_episodic_memory_add_turn_and_recent() {
        let mut mem = EpisodicMemory::new(10);
        assert_eq!(mem.len(), 0);

        mem.add_turn("system", "You are helpful.");
        mem.add_turn("user", "Hello!");
        mem.add_turn("assistant", "Hi there!");

        assert_eq!(mem.len(), 3);

        let recent = mem.recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].role, "user");
        assert_eq!(recent[0].content, "Hello!");
        assert_eq!(recent[1].role, "assistant");
        assert_eq!(recent[1].content, "Hi there!");
    }

    #[test]
    fn test_episodic_memory_to_messages() {
        let mut mem = EpisodicMemory::new(5);
        mem.add_turn("user", "What is 2+2?");
        mem.add_turn("assistant", "4");

        let messages = mem.to_messages();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "What is 2+2?");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "4");
    }

    #[test]
    fn test_episodic_memory_max_turns_eviction() {
        let mut mem = EpisodicMemory::new(3);
        mem.add_turn("user", "one");
        mem.add_turn("user", "two");
        mem.add_turn("user", "three");
        mem.add_turn("user", "four");

        assert_eq!(mem.len(), 3);

        let messages = mem.to_messages();
        assert_eq!(messages[0].content, "two");
        assert_eq!(messages[1].content, "three");
        assert_eq!(messages[2].content, "four");
    }
}
