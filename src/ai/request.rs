//! Request types for LLM provider clients.

use serde::{Deserialize, Serialize};

/// A single chat-completion request to an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    /// Model identifier used by the provider.
    pub model: String,
    /// Conversation messages in provider order.
    pub messages: Vec<LlmMessage>,
    /// Tool schemas the model may invoke.
    pub tools: Vec<ToolSchema>,
}

/// A chat message exchanged with an LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    /// Message role, e.g. `"system"`, `"user"`, or `"assistant"`.
    pub role: String,
    /// Message content.
    pub content: String,
}

/// JSON-schema description of a tool exposed to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON schema for the tool arguments.
    pub parameters: serde_json::Value,
}
