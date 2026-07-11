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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_request_default() {
        let req = LlmRequest::default();
        assert!(req.model.is_empty());
        assert!(req.messages.is_empty());
        assert!(req.tools.is_empty());
        assert!(req.memory.is_empty());
        assert!(req.pricing.is_none());
    }

    #[test]
    fn test_model_pricing_default() {
        let pricing = ModelPricing::default();
        assert_eq!(pricing.input_cost_per_1k, 0.0);
        assert_eq!(pricing.output_cost_per_1k, 0.0);
    }

    #[test]
    fn test_tool_schema_construction() {
        let schema = ToolSchema {
            name: "get_weather".to_string(),
            description: "Get the weather".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        };
        assert_eq!(schema.name, "get_weather");
        assert_eq!(schema.description, "Get the weather");
    }

    #[test]
    fn test_llm_message() {
        let msg = LlmMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        };
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "Hello");
    }
}
