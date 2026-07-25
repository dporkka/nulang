//! Re-export facade for the `nulang-ai` crate.
//!
//! The AI types (LLM clients, request/response, memory, pipelines, debates,
//! supervisor teams, usage tracking) live in the standalone `nulang-ai` crate
//! which has no dependency on the core `nulang` crate.  The concrete `Runtime`
//! implementations that tie the traits to the core actor system live in
//! `runtime_impls`.

#[cfg(feature = "ai-runtime")]
pub(crate) mod runtime_impls;
#[cfg(feature = "ai-runtime")]
pub mod schema;

#[cfg(feature = "ai-runtime")]
pub use nulang_ai::{
    complete_sync, estimated_cost, Debate, DebateRuntime, Document, EpisodicMemory, LlmClient,
    LlmError, LlmErrorKind, LlmMessage, LlmRequest, LlmResponse, MockLlmClient, ModelPricing,
    OllamaClient, OpenAiClient, Participant, Pattern, Pipeline, PipelineRuntime, PipelineStage,
    ProceduralMemory, SemanticMemory, Stance, SupervisorRuntime, SupervisorTeam, TokenBudget,
    TokenUsage, ToolCall, ToolSchema, Turn, UsageSummary, Worker,
};
