//! Request types for LLM provider clients.

use serde::{Deserialize, Serialize};

/// A single chat-completion request to an LLM provider.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    /// Model identifier used by the provider.
    pub model: String,
    /// Conversation messages in provider order.
    pub messages: Vec<LlmMessage>,
    /// Tool schemas the model may invoke.
    pub tools: Vec<ToolSchema>,
    /// Episodic memory messages prepended to `messages` before sending.
    pub memory: Vec<LlmMessage>,
    /// Optional per-model pricing information for cost estimation.
    pub pricing: Option<ModelPricing>,
}

/// Pricing rates for a model, expressed as cost per 1k tokens.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Cost in USD per 1k prompt/input tokens.
    pub input_cost_per_1k: f64,
    /// Cost in USD per 1k completion/output tokens.
    pub output_cost_per_1k: f64,
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
