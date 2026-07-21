//! LLM provider client module for the v0.9 AI Runtime.
//!
//! This module exposes provider-agnostic request/response types, an async
//! client trait, a synchronous wrapper, and concrete provider implementations.

#[cfg(feature = "ai-runtime")]
pub mod client;
#[cfg(not(feature = "ai-runtime"))]
#[path = "client_stub.rs"]
pub mod client;
pub mod debate;
pub mod memory;
#[cfg(feature = "ai-runtime")]
pub mod mock;
#[cfg(not(feature = "ai-runtime"))]
#[path = "mock_stub.rs"]
pub mod mock;
pub mod pipeline;
pub mod procedural_memory;
#[cfg(feature = "ai-runtime")]
pub mod providers;
#[cfg(not(feature = "ai-runtime"))]
#[path = "providers_stub.rs"]
pub mod providers;
pub mod request;
pub mod response;
pub mod schema;
pub mod semantic_memory;
pub mod supervisor;
pub mod usage;

pub use client::{complete_sync, LlmClient};
pub use debate::{Debate, DebateRuntime, Participant, Stance};
pub use memory::{EpisodicMemory, Turn};
pub use mock::MockLlmClient;
pub use pipeline::{Pipeline, PipelineRuntime, PipelineStage};
pub use procedural_memory::{Pattern, ProceduralMemory};
pub use providers::ollama::OllamaClient;
pub use providers::openai::OpenAiClient;
pub use request::{LlmMessage, LlmRequest, ModelPricing, ToolSchema};
pub use response::{LlmError, LlmErrorKind, LlmResponse, TokenUsage, ToolCall};
pub use schema::{function_to_tool_schema, type_to_json_schema};
pub use semantic_memory::{Document, SemanticMemory};
pub use supervisor::{SupervisorRuntime, SupervisorTeam, Worker};
pub use usage::{estimated_cost, TokenBudget, UsageSummary};
