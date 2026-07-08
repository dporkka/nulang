//! LLM provider client module for the v0.9 AI Runtime.
//!
//! This module exposes provider-agnostic request/response types, an async
//! client trait, a synchronous wrapper, and concrete provider implementations.

pub mod client;
pub mod memory;
pub mod mock;
pub mod providers;
pub mod request;
pub mod response;
pub mod schema;

pub use client::{complete_sync, LlmClient};
pub use memory::{EpisodicMemory, Turn};
pub use mock::MockLlmClient;
pub use providers::ollama::OllamaClient;
pub use request::{LlmMessage, LlmRequest, ToolSchema};
pub use response::{LlmResponse, ToolCall};
pub use schema::{function_to_tool_schema, type_to_json_schema};
