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
