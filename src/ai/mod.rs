//! LLM provider client module for the v0.9 AI Runtime.
//!
//! This module exposes provider-agnostic request/response types, an async
//! client trait, a synchronous wrapper, and concrete provider implementations.

pub mod client;
pub mod memory;
pub mod mock;
pub mod pipeline;
pub mod procedural_memory;
pub mod debate;
pub mod supervisor;
pub mod providers;
pub mod request;
pub mod response;
pub mod schema;
pub mod semantic_memory;
pub mod usage;

pub use client::{complete_sync, LlmClient};
pub use pipeline::{Pipeline, PipelineRuntime, PipelineStage};
pub use memory::{EpisodicMemory, Turn};
pub use mock::MockLlmClient;
pub use providers::ollama::OllamaClient;
pub use providers::openai::OpenAiClient;
pub use request::{LlmMessage, LlmRequest, ModelPricing, ToolSchema};
pub use response::{LlmResponse, TokenUsage, ToolCall};
pub use procedural_memory::{Pattern, ProceduralMemory};
pub use debate::{Debate, DebateRuntime, Participant, Stance};
pub use schema::{function_to_tool_schema, type_to_json_schema};
pub use supervisor::{SupervisorRuntime, SupervisorTeam, Worker};
pub use semantic_memory::{Document, SemanticMemory};
pub use usage::{estimated_cost, UsageSummary};
