//! Sequential agent pipeline for the v0.9 AI Runtime.
//!
//! A [`Pipeline`] chains one or more agent stages together. Each stage sends an
//! interpolated prompt to its target agent and feeds the agent's response into
//! the next stage as the `{input}` placeholder.
//!
//! Implementors of [`PipelineRuntime`] provide the actor `ask` behavior. Test
//! code can use a mock implementation to avoid spinning up a real actor system.

// ---------------------------------------------------------------------------
// Runtime abstraction
// ---------------------------------------------------------------------------

/// Minimal runtime capability required to execute a pipeline.
///
/// Test code can provide a mock implementation to avoid spinning up a real
/// actor system.
pub trait PipelineRuntime {
    /// Send `prompt` to `agent_id` and return the textual response.
    fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// Pipeline definition
// ---------------------------------------------------------------------------

/// A single stage in an agent pipeline.
#[derive(Debug, Clone)]
pub struct PipelineStage {
    /// Human-readable name for the stage.
    pub name: String,
    /// Target actor id for this stage.
    pub agent_id: u64,
    /// Prompt template; occurrences of `{input}` are replaced with the previous
    /// stage's output (or the pipeline input for the first stage).
    pub prompt_template: String,
}

/// A chain of agent stages executed sequentially.
#[derive(Debug, Clone, Default)]
pub struct Pipeline {
    pub stages: Vec<PipelineStage>,
}

impl Pipeline {
    /// Create an empty pipeline.
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Append a stage and return `self` for fluent construction.
    pub fn stage(
        mut self,
        name: impl Into<String>,
        agent_id: u64,
        prompt_template: impl Into<String>,
    ) -> Self {
        self.stages.push(PipelineStage {
            name: name.into(),
            agent_id,
            prompt_template: prompt_template.into(),
        });
        self
    }

    /// Run the pipeline, returning the output of the final stage.
    ///
    /// Returns an error if any stage fails or if the pipeline has no stages.
    pub fn run<R: PipelineRuntime>(&self, runtime: &mut R, input: &str) -> Result<String, String> {
        if self.stages.is_empty() {
            return Err("Pipeline has no stages".to_string());
        }

        let mut current = input.to_string();
        for stage in &self.stages {
            let prompt = interpolate_template(&stage.prompt_template, &current);
            current = runtime.ask_agent(stage.agent_id, &prompt)?;
        }
        Ok(current)
    }
}

/// Replace every occurrence of `{input}` in `template` with `input`.
fn interpolate_template(template: &str, input: &str) -> String {
    template.replace("{input}", input)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// Mock runtime that records calls and returns configured responses.
    struct MockRuntime {
        responses: HashMap<u64, String>,
        calls: RefCell<Vec<(u64, String)>>,
    }

    impl MockRuntime {
        fn new(responses: HashMap<u64, String>) -> Self {
            Self {
                responses,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl PipelineRuntime for MockRuntime {
        fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String> {
            self.calls.borrow_mut().push((agent_id, prompt.to_string()));
            self.responses
                .get(&agent_id)
                .cloned()
                .ok_or_else(|| format!("No response configured for agent {}", agent_id))
        }
    }

    #[test]
    fn test_interpolate_template() {
        assert_eq!(
            interpolate_template("Summarize: {input}", "hello world"),
            "Summarize: hello world"
        );
        assert_eq!(interpolate_template("{input} and {input}", "x"), "x and x");
        assert_eq!(
            interpolate_template("no placeholder", "x"),
            "no placeholder"
        );
        assert_eq!(interpolate_template("{input}", ""), "");
    }

    #[test]
    fn test_empty_pipeline_errors() {
        let pipeline = Pipeline::new();
        let mut rt = MockRuntime::new(HashMap::new());
        assert_eq!(
            pipeline.run(&mut rt, "hello"),
            Err("Pipeline has no stages".to_string())
        );
    }

    #[test]
    fn test_single_stage() {
        let pipeline = Pipeline::new().stage("summarize", 1, "Summarize: {input}");
        let mut rt = MockRuntime::new(HashMap::from([(1, "summary".to_string())]));

        let result = pipeline.run(&mut rt, "hello world").unwrap();
        assert_eq!(result, "summary");

        let calls = rt.calls.into_inner();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], (1, "Summarize: hello world".to_string()));
    }

    #[test]
    fn test_multiple_stages_chain_output() {
        let pipeline = Pipeline::new().stage("expand", 1, "Expand: {input}").stage(
            "summarize",
            2,
            "Summarize: {input}",
        );
        let mut rt = MockRuntime::new(HashMap::from([
            (1, "expanded text".to_string()),
            (2, "final summary".to_string()),
        ]));

        let result = pipeline.run(&mut rt, "topic").unwrap();
        assert_eq!(result, "final summary");

        let calls = rt.calls.into_inner();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], (1, "Expand: topic".to_string()));
        assert_eq!(calls[1], (2, "Summarize: expanded text".to_string()));
    }

    #[test]
    fn test_stage_error_propagates() {
        let pipeline = Pipeline::new()
            .stage("ok", 1, "{input}")
            .stage("fail", 2, "{input}");
        let mut rt = MockRuntime::new(HashMap::from([(1, "intermediate".to_string())]));

        assert_eq!(
            pipeline.run(&mut rt, "start"),
            Err("No response configured for agent 2".to_string())
        );
    }
}
