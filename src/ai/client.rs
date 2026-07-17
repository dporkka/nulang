//! Core LLM client trait and synchronous wrapper.

use async_trait::async_trait;

use crate::ai::request::LlmRequest;
use crate::ai::response::{LlmError, LlmResponse};

/// Async trait implemented by all LLM provider clients.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Request a chat completion from the provider.
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
}

/// Synchronous wrapper around an async [`LlmClient`].
///
/// Never blocks on the ambient Tokio runtime: the CLI runs all script
/// execution synchronously inside `#[tokio::main]`, so `Handle::block_on`
/// would panic with "Cannot start a runtime from within a runtime". The
/// request runs on a dedicated scoped worker thread with its own
/// current-thread runtime, mirroring the non-blocking `nulang-llm` suspend
/// path in the actor runtime.
pub fn complete_sync(client: &dyn LlmClient, request: LlmRequest) -> Result<LlmResponse, LlmError> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .name("nulang-llm-sync".to_string())
            .spawn_scoped(s, move || {
                let result = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt.block_on(client.complete(request)),
                    Err(e) => Err(LlmError::from_string(e.to_string())),
                };
                let _ = tx.send(result);
            })
            .map_err(|e| LlmError::from_string(e.to_string()))?;
        rx.recv().map_err(|e| LlmError::from_string(e.to_string()))?
    })
}

/// Default timeout for provider HTTP requests.
const LLM_HTTP_TIMEOUT_SECS: u64 = 300;

/// Build an HTTP client for LLM provider requests with a request timeout.
///
/// Without a timeout a hung provider would never resolve: the LLM in-flight
/// counter could never drain and the actor scheduler would spin forever. A
/// timed-out request surfaces as an error completion instead, which the
/// scheduler drains like any other failed call.
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(LLM_HTTP_TIMEOUT_SECS))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::request::LlmRequest;
    use crate::ai::response::{LlmResponse, TokenUsage};

    struct TestClient;

    #[async_trait]
    impl LlmClient for TestClient {
        async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
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
        // Calling complete_sync without a Tokio runtime must work: the
        // request runs on a dedicated worker thread with its own
        // current-thread runtime.
        let client = TestClient;
        let request = LlmRequest::default();
        let result = complete_sync(&client, request);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content.as_deref(), Some("test"));
    }

    #[test]
    fn test_complete_sync_inside_tokio_runtime() {
        // The CLI runs script execution synchronously inside #[tokio::main],
        // so complete_sync must not panic with "Cannot start a runtime from
        // within a runtime" when a runtime context is active on this thread.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = TestClient;
        let request = LlmRequest::default();
        let result = rt.block_on(async { complete_sync(&client, request) });
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content.as_deref(), Some("test"));
    }

    #[test]
    fn test_http_client_builds() {
        // The shared provider client builder must succeed (timeout config is
        // not observable through the reqwest API, so construction is what we
        // can assert here).
        let _client = http_client();
    }
}
