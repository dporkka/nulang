//! Ollama LLM provider client.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ai::client::LlmClient;
use crate::ai::request::{LlmMessage, LlmRequest, ToolSchema};
use crate::ai::response::{LlmResponse, TokenUsage, ToolCall};

/// Client for the Ollama HTTP API.
#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    model: String,
    client: reqwest::Client,
}

#[async_trait]
impl LlmClient for OllamaClient {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, String> {
        let url = format!("{}/api/chat", self.base_url);
        let model = if request.model.is_empty() {
            self.model.clone()
        } else {
            request.model
        };
        let messages: Vec<LlmMessage> = request
            .memory
            .into_iter()
            .chain(request.messages)
            .collect();
        let body = OllamaChatRequest {
            model,
            messages,
            tools: request.tools.into_iter().map(into_ollama_tool).collect(),
            stream: false,
        };

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json::<OllamaChatResponse>()
            .await
            .map_err(|e| e.to_string())?;

        let message = response.message;
        let content = if message.content.is_empty() {
            None
        } else {
            Some(message.content)
        };

        let tool_calls: Vec<ToolCall> = message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|tc| {
                let args = parse_arguments(tc.function.arguments);
                ToolCall {
                    id: String::new(),
                    name: tc.function.name,
                    arguments: args,
                }
            })
            .collect();

        let prompt_tokens = response.prompt_eval_count.unwrap_or(0);
        let completion_tokens = response.eval_count.unwrap_or(0);

        Ok(LlmResponse {
            content,
            tool_calls,
            model: response.model,
            finish_reason: response.done_reason.unwrap_or_default(),
            usage: TokenUsage::new(prompt_tokens, completion_tokens),
        })
    }
}

impl OllamaClient {
    /// Create a new Ollama client pointing at the given base URL and model.
    pub fn new(base_url: &str, model: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            model: model.to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Default client for a local Ollama instance running on port 11434.
    pub fn default() -> Self {
        Self::new("http://localhost:11434", "llama3.1")
    }
}

fn into_ollama_tool(tool: ToolSchema) -> OllamaTool {
    OllamaTool {
        ty: "function".to_string(),
        function: OllamaFunction {
            name: tool.name,
            description: tool.description,
            parameters: tool.parameters,
        },
    }
}

fn parse_arguments(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => map,
        serde_json::Value::String(s) => serde_json::from_str(&s).unwrap_or_default(),
        _ => serde_json::Map::new(),
    }
}

#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<LlmMessage>,
    tools: Vec<OllamaTool>,
    #[serde(rename = "stream")]
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaTool {
    #[serde(rename = "type")]
    ty: String,
    function: OllamaFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    model: String,
    message: OllamaMessage,
    done_reason: Option<String>,
    /// Number of tokens evaluated for the prompt, if reported by Ollama.
    prompt_eval_count: Option<u32>,
    /// Number of tokens generated for the completion, if reported by Ollama.
    eval_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OllamaMessage {
    _role: Option<String>,
    content: String,
    tool_calls: Option<Vec<OllamaToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OllamaToolCall {
    function: OllamaToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct OllamaToolCallFunction {
    name: String,
    arguments: serde_json::Value,
}
