//! Core LLM client trait and synchronous wrapper.

use async_trait::async_trait;

use crate::ai::request::LlmRequest;
use crate::ai::response::LlmResponse;

/// Async trait implemented by all LLM provider clients.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Request a chat completion from the provider.
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, String>;
}

/// Synchronous wrapper around an async [`LlmClient`].
///
/// Uses the current Tokio runtime handle when one exists, otherwise builds a
/// temporary single-threaded runtime for the call.
pub fn complete_sync(client: &dyn LlmClient, request: LlmRequest) -> Result<LlmResponse, String> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(client.complete(request)),
        Err(_) => {
            let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
            rt.block_on(client.complete(request))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::request::LlmRequest;
    use crate::ai::response::{LlmResponse, TokenUsage};

    struct TestClient;

    #[async_trait]
    impl LlmClient for TestClient {
        async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, String> {
            Ok(LlmResponse {
                content: Some("test".to_string()),
                tool_calls: vec![],
                model: "test".to_string(),
                finish_reason: "stop".to_string(),
                usage: TokenUsage::new(0, 0),
            })
        }
    }

    #[test]
    fn test_complete_sync_requires_runtime() {
        // Calling complete_sync without a Tokio runtime should work because
        // the function creates a temporary single-threaded runtime when none
        // is available.
        let client = TestClient;
        let request = LlmRequest::default();
        let result = complete_sync(&client, request);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content.as_deref(), Some("test"));
    }
}
