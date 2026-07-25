//! Procedural memory for learned patterns and few-shot examples.
//!
//! `ProceduralMemory` stores reusable patterns and task-specific examples that
//! agents can retrieve at inference time to improve few-shot performance and
//! to apply learned output formats.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A learned pattern keyed by name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    /// Human-readable name for the pattern.
    pub name: String,
    /// Input-matching heuristic, e.g. a glob or keyword substring.
    pub input_pattern: String,
    /// Template used to format the output.
    pub output_template: String,
}

/// A single input/output example for a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Example {
    /// The example input.
    pub input: String,
    /// The expected output.
    pub output: String,
}

/// Persistent procedural memory organized by namespace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProceduralMemory {
    /// Logical grouping for the stored patterns/examples.
    pub namespace: String,
    /// Named patterns keyed by their stable identifier.
    pub patterns: HashMap<String, Pattern>,
    /// Examples grouped by task name.
    pub examples: HashMap<String, Vec<Example>>,
}

impl ProceduralMemory {
    /// Create an empty procedural memory in the given namespace.
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            patterns: HashMap::new(),
            examples: HashMap::new(),
        }
    }

    /// Store or replace a pattern and return its key.
    pub fn store_pattern(
        &mut self,
        key: impl Into<String>,
        input_pattern: impl Into<String>,
        output_template: impl Into<String>,
    ) -> String {
        let key = key.into();
        let pattern = Pattern {
            name: key.clone(),
            input_pattern: input_pattern.into(),
            output_template: output_template.into(),
        };
        self.patterns.insert(key.clone(), pattern);
        key
    }

    /// Retrieve a pattern by key.
    pub fn get_pattern(&self, key: &str) -> Option<&Pattern> {
        self.patterns.get(key)
    }

    /// Remove a pattern by key. Returns true if it existed.
    pub fn delete_pattern(&mut self, key: &str) -> bool {
        self.patterns.remove(key).is_some()
    }

    /// Add a few-shot example for a task.
    pub fn add_example(
        &mut self,
        task: impl Into<String>,
        input: impl Into<String>,
        output: impl Into<String>,
    ) {
        let task = task.into();
        let example = Example {
            input: input.into(),
            output: output.into(),
        };
        self.examples.entry(task).or_default().push(example);
    }

    /// Return up to `top_k` examples for a task, ranked by keyword overlap with
    /// the query. Falls back to the most recently added examples when the query
    /// is empty.
    pub fn get_examples(&self, task: &str, query: &str, top_k: usize) -> Vec<Example> {
        if top_k == 0 {
            return Vec::new();
        }

        let all = match self.examples.get(task) {
            Some(examples) => examples,
            None => return Vec::new(),
        };

        let query_tokens = token_set(query);
        if query_tokens.is_empty() {
            return all.iter().rev().take(top_k).cloned().collect();
        }

        let mut scored: Vec<(usize, Example)> = all
            .iter()
            .map(|example| {
                let input_tokens = token_set(&example.input);
                let output_tokens = token_set(&example.output);
                let overlap = query_tokens
                    .iter()
                    .filter(|token| input_tokens.contains(*token) || output_tokens.contains(*token))
                    .count();
                (overlap, example.clone())
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.input.cmp(&b.1.input)));
        scored.truncate(top_k);
        scored.into_iter().map(|(_, example)| example).collect()
    }

    /// List all stored pattern keys.
    pub fn pattern_keys(&self) -> Vec<&String> {
        self.patterns.keys().collect()
    }

    /// List all task names that have examples.
    pub fn task_names(&self) -> Vec<&String> {
        self.examples.keys().collect()
    }

    /// Return the total number of stored patterns and examples.
    pub fn len(&self) -> usize {
        self.patterns.len()
            + self
                .examples
                .values()
                .map(|examples| examples.len())
                .sum::<usize>()
    }
}

fn token_set(text: &str) -> std::collections::HashSet<String> {
    text.to_lowercase()
        .split_whitespace()
        .map(|token| {
            token
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string()
        })
        .filter(|token| !token.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_and_get_pattern() {
        let mut memory = ProceduralMemory::new("test");
        let key = memory.store_pattern(
            "format_research_output",
            "research_*",
            "{title}\n\n{summary}\n\nSources: {sources}",
        );
        assert_eq!(key, "format_research_output");

        let pattern = memory.get_pattern("format_research_output").unwrap();
        assert_eq!(pattern.input_pattern, "research_*");
        assert_eq!(
            pattern.output_template,
            "{title}\n\n{summary}\n\nSources: {sources}"
        );
    }

    #[test]
    fn test_delete_pattern() {
        let mut memory = ProceduralMemory::new("test");
        memory.store_pattern("p1", "in", "out");
        assert!(memory.delete_pattern("p1"));
        assert!(memory.get_pattern("p1").is_none());
        assert!(!memory.delete_pattern("p1"));
    }

    #[test]
    fn test_add_and_retrieve_examples() {
        let mut memory = ProceduralMemory::new("test");
        memory.add_example(
            "code_review",
            "fn bad() { let x = 1; x }",
            "Issue: Unused variable. Fix: Remove `x` or use it.",
        );
        memory.add_example(
            "code_review",
            "fn unsafe() { unsafe { *ptr } }",
            "Issue: Raw pointer dereference without null check.",
        );

        let examples = memory.get_examples("code_review", "unused variable", 3);
        assert_eq!(examples.len(), 2);
        assert!(examples[0].input.contains("unused") || examples[0].output.contains("Unused"));
    }

    #[test]
    fn test_example_query_falls_back_to_recent() {
        let mut memory = ProceduralMemory::new("test");
        memory.add_example("summarize", "a", "A");
        memory.add_example("summarize", "b", "B");

        let examples = memory.get_examples("summarize", "", 1);
        assert_eq!(examples.len(), 1);
        assert_eq!(examples[0].input, "b");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut memory = ProceduralMemory::new("my_app");
        memory.store_pattern("format", "*", "{result}");
        memory.add_example("task", "in", "out");

        let json = serde_json::to_string(&memory).unwrap();
        let restored: ProceduralMemory = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.namespace, "my_app");
        assert_eq!(restored.len(), 2);
        assert!(restored.get_pattern("format").is_some());
        assert_eq!(restored.get_examples("task", "in", 1).len(), 1);
    }
}
