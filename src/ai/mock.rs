//! Mock LLM client for tests.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::ai::client::LlmClient;
use crate::ai::request::LlmRequest;
use crate::ai::response::{LlmResponse, TokenUsage, ToolCall};

/// A test client that always returns a fixed response and optionally records
/// the requests it receives.
#[derive(Debug, Clone)]
pub struct MockLlmClient {
    response: LlmResponse,
    calls: Arc<Mutex<Vec<LlmRequest>>>,
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, String> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(request);
        }
        Ok(self.response.clone())
    }
}

impl MockLlmClient {
    /// Create a mock client that returns the given response.
    pub fn new(response: LlmResponse) -> Self {
        Self {
            response,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a mock client that returns a plain text response.
    pub fn text(content: impl Into<String>) -> Self {
        Self::with_usage(content, TokenUsage::default())
    }

    /// Create a mock client that returns a plain text response with usage.
    pub fn with_usage(content: impl Into<String>, usage: TokenUsage) -> Self {
        Self::new(LlmResponse {
            content: Some(content.into()),
            tool_calls: Vec::new(),
            model: "mock".to_string(),
            finish_reason: "stop".to_string(),
            usage,
        })
    }

    /// Create a mock client that returns a single tool call.
    pub fn tool_call(
        name: impl Into<String>,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        Self::tool_call_with_usage(name, arguments, TokenUsage::default())
    }

    /// Create a mock client that returns a single tool call with usage.
    pub fn tool_call_with_usage(
        name: impl Into<String>,
        arguments: serde_json::Map<String, serde_json::Value>,
        usage: TokenUsage,
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
            usage,
        })
    }

    /// Return the requests recorded by this mock client.
    pub fn recorded_calls(&self) -> Vec<LlmRequest> {
        self.calls.lock().map_or_else(|_| Vec::new(), |g| g.clone())
    }
}
