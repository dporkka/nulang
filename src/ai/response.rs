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
