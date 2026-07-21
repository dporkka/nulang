//! LLM subsystem for the actor runtime.
//!
//! Manages the persistent LLM worker thread, request dispatch, completion
//! polling, and non-blocking suspension for `perform LLM.ask` in bytecode
//! behaviors.

use std::sync::Arc;

use crate::ai::{LlmClient, LlmError, LlmRequest, LlmResponse, TokenBudget};

/// Work item sent to the persistent LLM worker thread.
pub(crate) struct LlmWorkItem {
    pub(crate) actor_id: u64,
    pub(crate) request: LlmRequest,
    pub(crate) client: Arc<dyn LlmClient>,
}

// Safety: LlmWorkItem is Send because all fields are Send.
unsafe impl Send for LlmWorkItem {}

/// Consolidated LLM subsystem state.
///
/// Extracted from the Runtime god-object to group related fields and
/// clarify ownership. The worker thread is spawned in [`LlmState::new`]
/// and runs for the lifetime of the runtime.
pub struct LlmState {
    /// Token budget for LLM calls. When set, the runtime rejects
    /// LLM requests that would exceed the configured token limit.
    pub token_budget: Option<Arc<TokenBudget>>,

    /// LLM client for the v0.9 AI Runtime. Shared (Arc) so background worker
    /// threads can perform non-blocking `perform LLM.ask` calls.
    pub client: Option<Arc<dyn LlmClient>>,

    /// Channel receiving results from the persistent LLM worker thread.
    /// Drained by `poll_llm_completions`.
    pub rx: std::sync::mpsc::Receiver<(u64, Result<LlmResponse, LlmError>)>,

    /// Number of LLM calls currently in flight. Incremented on dispatch,
    /// decremented when the completion is stored.
    pub inflight_count: usize,

    /// Channel to dispatch work to the persistent LLM worker thread.
    /// `None` after the runtime is dropped (sender half is owned by the
    /// worker thread, which outlives the runtime).
    pub(crate) request_tx: Option<crossbeam::channel::Sender<LlmWorkItem>>,

    /// True while executing a scheduler-driven bytecode behavior, enabling
    /// non-blocking suspension on `perform LLM.ask`. Nested synchronous entry
    /// points force it back to false so they keep blocking behavior.
    pub suspend_enabled: bool,
}

impl LlmState {
    /// Create the LLM subsystem, spawning the persistent worker thread.
    ///
    /// The worker thread owns its own single-threaded tokio runtime and
    /// processes requests sequentially. Results are sent back through the
    /// `rx` channel.
    pub fn new() -> Self {
        let (llm_tx, llm_rx) = std::sync::mpsc::channel();
        let llm_tx_worker = llm_tx.clone();
        let (llm_request_tx, llm_request_rx) =
            crossbeam::channel::unbounded::<LlmWorkItem>();

        // Spawn a persistent LLM worker thread.
        let _worker = std::thread::Builder::new()
            .name("nulang-llm".to_string())
            .spawn(move || {
                let tokio_rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(_) => return,
                };
                while let Ok(item) = llm_request_rx.recv() {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            tokio_rt.block_on(item.client.complete(item.request))
                        }))
                        .unwrap_or_else(|_| {
                            Err(LlmError::from_string("LLM worker thread panicked"))
                        });
                    let _ = llm_tx_worker.send((item.actor_id, result));
                }
            });

        LlmState {
            token_budget: None,
            client: None,
            rx: llm_rx,
            inflight_count: 0,
            request_tx: Some(llm_request_tx),
            suspend_enabled: false,
        }
    }

    /// Set the LLM client provider.
    pub fn set_client(&mut self, client: Box<dyn LlmClient>) {
        self.client = Some(Arc::from(client));
    }

    /// Set a token budget limit. Requests exceeding this are rejected.
    pub fn set_token_budget(&mut self, limit: u64) {
        self.token_budget = Some(Arc::new(TokenBudget::new(limit)));
    }

    /// Remove the token budget limit.
    pub fn clear_token_budget(&mut self) {
        self.token_budget = None;
    }

    /// Check whether the token budget allows the given estimated tokens.
    /// Returns `true` if the request is allowed (budget not exhausted).
    pub fn check_token_budget(&self, _estimated_tokens: u64) -> bool {
        if let Some(ref budget) = self.token_budget {
            !budget.is_exhausted()
        } else {
            true
        }
    }

    /// Record token usage against the budget.
    pub fn record_token_usage(&self, tokens: u64) {
        if let Some(ref budget) = self.token_budget {
            budget.charge(tokens);
        }
    }
}

impl Default for LlmState {
    fn default() -> Self {
        Self::new()
    }
}
