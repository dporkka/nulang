//! Response types for LLM provider clients.

use serde::{Deserialize, Serialize};

/// A chat-completion response from an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    /// Text content returned by the model, if any.
    pub content: Option<String>,
    /// Tool calls requested by the model.
    pub tool_calls: Vec<ToolCall>,
    /// Model that produced the response.
    pub model: String,
    /// Provider-specific finish reason.
    pub finish_reason: String,
    /// Token usage reported by the provider.
    pub usage: TokenUsage,
}

/// Token usage statistics for a single LLM request/response.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Number of tokens in the prompt.
    pub prompt: u32,
    /// Number of tokens in the completion.
    pub completion: u32,
    /// Total number of tokens used.
    pub total: u32,
}

impl TokenUsage {
    /// Create a new usage record with `total` set to `prompt + completion`.
    pub fn new(prompt: u32, completion: u32) -> Self {
        Self {
            prompt,
            completion,
            total: prompt + completion,
        }
    }
}

/// A single tool invocation produced by an LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-specific tool-call identifier.
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// Parsed tool arguments.
    pub arguments: serde_json::Map<String, serde_json::Value>,
}
