//! Mock LLM client available when the optional AI runtime is disabled.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::ai::client::LlmClient;
use crate::ai::request::LlmRequest;
use crate::ai::response::{LlmError, LlmResponse, TokenUsage, ToolCall};

#[derive(Debug, Clone)]
pub struct MockLlmClient {
    response: LlmResponse,
    responses: Vec<Result<LlmResponse, LlmError>>,
    index: Arc<AtomicUsize>,
    calls: Arc<Mutex<Vec<LlmRequest>>>,
    delay: std::time::Duration,
}

impl LlmClient for MockLlmClient {
    fn complete(
        &self,
        request: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse, LlmError>> + Send + '_>> {
        Box::pin(async move {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(request);
            }
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            let idx = self.index.fetch_add(1, Ordering::SeqCst);
            if idx < self.responses.len() {
                self.responses[idx].clone()
            } else {
                Ok(self.response.clone())
            }
        })
    }
}

impl MockLlmClient {
    pub fn new(response: LlmResponse) -> Self {
        Self {
            response,
            responses: Vec::new(),
            index: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(Mutex::new(Vec::new())),
            delay: std::time::Duration::ZERO,
        }
    }

    pub fn text(content: impl Into<String>) -> Self {
        Self::with_usage(content, TokenUsage::default())
    }

    pub fn delayed(content: impl Into<String>, delay: std::time::Duration) -> Self {
        let mut client = Self::text(content);
        client.delay = delay;
        client
    }

    pub fn with_usage(content: impl Into<String>, usage: TokenUsage) -> Self {
        Self::new(LlmResponse {
            content: Some(content.into()),
            tool_calls: Vec::new(),
            model: "mock".to_string(),
            finish_reason: "stop".to_string(),
            usage,
        })
    }

    pub fn tool_call(
        name: impl Into<String>,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        Self::tool_call_with_usage(name, arguments, TokenUsage::default())
    }

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

    pub fn sequence(responses: Vec<LlmResponse>) -> Self {
        Self::sequence_with_errors(responses.into_iter().map(Ok).collect())
    }

    pub fn sequence_with_errors(responses: Vec<Result<LlmResponse, LlmError>>) -> Self {
        let default_response = LlmResponse {
            content: None,
            tool_calls: Vec::new(),
            model: "mock".to_string(),
            finish_reason: "stop".to_string(),
            usage: TokenUsage::default(),
        };
        Self {
            response: default_response,
            responses,
            index: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(Mutex::new(Vec::new())),
            delay: std::time::Duration::ZERO,
        }
    }

    pub fn recorded_calls(&self) -> Vec<LlmRequest> {
        self.calls.lock().map_or_else(|_| Vec::new(), |g| g.clone())
    }
}
