//! Minimal client surface used when the optional AI runtime is disabled.

use std::future::Future;
use std::pin::Pin;

use crate::ai::request::LlmRequest;
use crate::ai::response::{LlmError, LlmErrorKind, LlmResponse};

fn disabled_error() -> LlmError {
    LlmError::new(
        LlmErrorKind::ProviderError,
        "AI runtime disabled; rebuild with --features ai-runtime",
    )
}

/// Object-safe LLM client trait preserved for callers that compile without the
/// optional provider integrations.
pub trait LlmClient: Send + Sync {
    /// Request a chat completion from the provider.
    fn complete(
        &self,
        _request: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse, LlmError>> + Send + '_>> {
        Box::pin(async { Err(disabled_error()) })
    }
}

/// Synchronous wrapper used by the runtime and CLI entrypoints.
pub fn complete_sync(_client: &dyn LlmClient, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
    Err(disabled_error())
}
