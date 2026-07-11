//! Mock LLM client for tests.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::ai::client::LlmClient;
use crate::ai::request::LlmRequest;
use crate::ai::response::{LlmResponse, TokenUsage, ToolCall};

/// A test client that returns a fixed response, a sequence of responses, or
/// tool calls, and optionally records the requests it receives.
#[derive(Debug, Clone)]
pub struct MockLlmClient {
    response: LlmResponse,
    responses: Vec<LlmResponse>,
    index: Arc<AtomicUsize>,
    calls: Arc<Mutex<Vec<LlmRequest>>>,
    delay: std::time::Duration,
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, String> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(request);
        }
        if !self.delay.is_zero() {
            // Runs on the caller's thread under `block_on`, so a blocking
            // sleep is fine here (used to simulate slow providers).
            std::thread::sleep(self.delay);
        }
        let idx = self.index.fetch_add(1, Ordering::SeqCst);
        if idx < self.responses.len() {
            Ok(self.responses[idx].clone())
        } else {
            Ok(self.response.clone())
        }
    }
}

impl MockLlmClient {
    /// Create a mock client that returns the given response.
    pub fn new(response: LlmResponse) -> Self {
        Self {
            response,
            responses: Vec::new(),
            index: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(Mutex::new(Vec::new())),
            delay: std::time::Duration::ZERO,
        }
    }

    /// Create a mock client that returns a plain text response.
    pub fn text(content: impl Into<String>) -> Self {
        Self::with_usage(content, TokenUsage::default())
    }

    /// Create a mock client that waits for `delay` before returning a plain
    /// text response. The sleep runs inside the async `complete`; callers run
    /// it under `block_on` on their own thread, so a blocking sleep is fine.
    pub fn delayed(content: impl Into<String>, delay: std::time::Duration) -> Self {
        let mut client = Self::text(content);
        client.delay = delay;
        client
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

    /// Create a mock client that returns each response in order.
    pub fn sequence(responses: Vec<LlmResponse>) -> Self {
        Self {
            response: LlmResponse {
                content: None,
                tool_calls: Vec::new(),
                model: "mock".to_string(),
                finish_reason: "stop".to_string(),
                usage: TokenUsage::default(),
            },
            responses,
            index: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(Mutex::new(Vec::new())),
            delay: std::time::Duration::ZERO,
        }
    }

    /// Return the requests recorded by this mock client.
    pub fn recorded_calls(&self) -> Vec<LlmRequest> {
        self.calls.lock().map_or_else(|_| Vec::new(), |g| g.clone())
    }
}
