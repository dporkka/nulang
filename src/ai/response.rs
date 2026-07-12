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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_response_construction() {
        let response = LlmResponse {
            content: Some("Hello".to_string()),
            tool_calls: vec![],
            model: "gpt-4".to_string(),
            finish_reason: "stop".to_string(),
            usage: TokenUsage::new(10, 5),
        };
        assert_eq!(response.content.as_deref(), Some("Hello"));
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.model, "gpt-4");
        assert_eq!(response.finish_reason, "stop");
        assert_eq!(response.usage.prompt, 10);
        assert_eq!(response.usage.completion, 5);
        assert_eq!(response.usage.total, 15);
    }

    #[test]
    fn test_token_usage_new() {
        let usage = TokenUsage::new(100, 50);
        assert_eq!(usage.prompt, 100);
        assert_eq!(usage.completion, 50);
        assert_eq!(usage.total, 150);
    }

    #[test]
    fn test_token_usage_default() {
        let usage = TokenUsage::default();
        assert_eq!(usage.prompt, 0);
        assert_eq!(usage.completion, 0);
        assert_eq!(usage.total, 0);
    }

    #[test]
    fn test_tool_call_construction() {
        let mut args = serde_json::Map::new();
        args.insert(
            "location".to_string(),
            serde_json::Value::String("NYC".to_string()),
        );
        let call = ToolCall {
            id: "call_1".to_string(),
            name: "get_weather".to_string(),
            arguments: args,
        };
        assert_eq!(call.id, "call_1");
        assert_eq!(call.name, "get_weather");
        assert_eq!(call.arguments.get("location").unwrap(), "NYC");
    }
}
