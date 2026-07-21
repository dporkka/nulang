//! Provider stubs used when the optional AI runtime is disabled.

use std::future::Future;
use std::pin::Pin;

use crate::ai::client::LlmClient;
use crate::ai::request::LlmRequest;
use crate::ai::response::{LlmError, LlmErrorKind, LlmResponse};

fn disabled_error(provider: &str) -> LlmError {
    LlmError::new(
        LlmErrorKind::ProviderError,
        format!(
            "{} provider unavailable; rebuild with --features ai-runtime",
            provider
        ),
    )
}

pub mod ollama {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct OllamaClient {
        #[allow(dead_code)]
        base_url: String,
        #[allow(dead_code)]
        model: String,
    }

    impl OllamaClient {
        pub fn new(base_url: &str, model: &str) -> Self {
            Self {
                base_url: base_url.to_string(),
                model: model.to_string(),
            }
        }
    }

    impl Default for OllamaClient {
        fn default() -> Self {
            Self::new("http://localhost:11434", "llama3.1")
        }
    }

    impl LlmClient for OllamaClient {
        fn complete(
            &self,
            _request: LlmRequest,
        ) -> Pin<Box<dyn Future<Output = Result<LlmResponse, LlmError>> + Send + '_>> {
            Box::pin(async { Err(disabled_error("Ollama")) })
        }
    }
}

pub mod openai {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct OpenAiClient {
        #[allow(dead_code)]
        base_url: String,
        #[allow(dead_code)]
        api_key: String,
        #[allow(dead_code)]
        model: String,
    }

    impl OpenAiClient {
        pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
            Self {
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: api_key.into(),
                model: model.into(),
            }
        }

        pub fn with_base_url(
            base_url: impl Into<String>,
            api_key: impl Into<String>,
            model: impl Into<String>,
        ) -> Self {
            Self {
                base_url: base_url.into(),
                api_key: api_key.into(),
                model: model.into(),
            }
        }

        pub fn gpt4o() -> Result<Self, String> {
            let api_key = std::env::var("OPENAI_API_KEY")
                .map_err(|_| "OPENAI_API_KEY environment variable not set".to_string())?;
            Ok(Self::new(api_key, "gpt-4o"))
        }
    }

    impl LlmClient for OpenAiClient {
        fn complete(
            &self,
            _request: LlmRequest,
        ) -> Pin<Box<dyn Future<Output = Result<LlmResponse, LlmError>> + Send + '_>> {
            Box::pin(async { Err(disabled_error("OpenAI")) })
        }
    }
}
