//! Mock LLM client for tests.

use async_trait::async_trait;

use crate::ai::client::LlmClient;
use crate::ai::request::LlmRequest;
use crate::ai::response::{LlmResponse, ToolCall};

/// A test client that always returns a fixed response.
#[derive(Debug, Clone)]
pub struct MockLlmClient {
    response: LlmResponse,
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, String> {
        Ok(self.response.clone())
    }
}

impl MockLlmClient {
    /// Create a mock client that returns the given response.
    pub fn new(response: LlmResponse) -> Self {
        Self { response }
    }

    /// Create a mock client that returns a plain text response.
    pub fn text(content: impl Into<String>) -> Self {
        Self::new(LlmResponse {
            content: Some(content.into()),
            tool_calls: Vec::new(),
            model: "mock".to_string(),
            finish_reason: "stop".to_string(),
        })
    }

    /// Create a mock client that returns a single tool call.
    pub fn tool_call(
        name: impl Into<String>,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        Self::new(LlmResponse {
            content: None,
            tool_calls: vec![ToolCall {
                id: String::new(),
                name: name.into(),
                arguments,
            }],
            model: "mock".to_string(),
            finish_reason: "tool_calls".to_string(),
        })
    }
}
