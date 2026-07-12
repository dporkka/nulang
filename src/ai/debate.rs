//! Debate pattern for multi-agent critical reasoning.
//!
//! A [`Debate`] brings together participant agents with assigned stances and a
//! moderator agent.  Over a fixed number of rounds each participant responds to
//! the topic and the accumulated arguments, and the moderator synthesizes a
//! final conclusion.

use crate::runtime::Runtime;

// ---------------------------------------------------------------------------
// Runtime abstraction
// ---------------------------------------------------------------------------

/// Minimal runtime capability required to execute a debate.
pub trait DebateRuntime {
    /// Send `prompt` to `agent_id` and return the textual response.
    fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String>;
}

impl DebateRuntime for Runtime {
    fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String> {
        crate::ai::PipelineRuntime::ask_agent(self, agent_id, prompt)
    }
}

// ---------------------------------------------------------------------------
// Debate definition
// ---------------------------------------------------------------------------

/// A stance a participant can take in the debate.
#[derive(Debug, Clone, PartialEq)]
pub enum Stance {
    /// Argues in favor of the proposition.
    Pro,
    /// Argues against the proposition.
    Con,
    /// Neutral observer or moderator.
    Neutral,
}

impl Stance {
    fn as_str(&self) -> &'static str {
        match self {
            Stance::Pro => "pro",
            Stance::Con => "con",
            Stance::Neutral => "neutral",
        }
    }
}

/// A single participant in a debate.
#[derive(Debug, Clone)]
pub struct Participant {
    /// Logical name for the participant.
    pub name: String,
    /// Assigned stance.
    pub stance: Stance,
    /// Target actor id.
    pub agent_id: u64,
}

/// A multi-round debate with participants and a moderator.
#[derive(Debug, Clone, Default)]
pub struct Debate {
    /// Topic under discussion.
    pub topic: String,
    /// Number of debate rounds to run.
    pub rounds: usize,
    /// Consensus threshold in `[0, 1]`; currently advisory.
    pub consensus_threshold: f64,
    /// Debate participants.  The last participant is typically the moderator.
    pub participants: Vec<Participant>,
}

impl Debate {
    /// Create a new debate with the given topic and defaults.
    pub fn new(topic: impl Into<String>, rounds: usize, consensus_threshold: f64) -> Self {
        Self {
            topic: topic.into(),
            rounds,
            consensus_threshold: consensus_threshold.clamp(0.0, 1.0),
            participants: Vec::new(),
        }
    }

    /// Append a participant and return `self` for fluent construction.
    pub fn participant(
        mut self,
        name: impl Into<String>,
        stance: impl Into<String>,
        agent_id: u64,
    ) -> Self {
        let stance_str = stance.into().to_lowercase();
        let stance = if stance_str == "pro" || stance_str == "for" {
            Stance::Pro
        } else if stance_str == "con" || stance_str == "against" {
            Stance::Con
        } else {
            Stance::Neutral
        };
        self.participants.push(Participant {
            name: name.into(),
            stance,
            agent_id,
        });
        self
    }

    /// Run the debate and return the moderator's synthesis.
    ///
    /// Returns an error if there are no participants or if any agent call fails.
    /// The current MVP returns the final moderator response as a plain string.
    pub fn run<R: DebateRuntime>(&self, runtime: &mut R) -> Result<String, String> {
        if self.participants.is_empty() {
            return Err("Debate has no participants".to_string());
        }

        let mut arguments: Vec<(String, Stance, String)> = Vec::new();

        for round in 1..=self.rounds {
            for participant in &self.participants {
                let prompt = build_participant_prompt(
                    &self.topic,
                    round,
                    self.rounds,
                    participant,
                    &arguments,
                );
                let response = runtime.ask_agent(participant.agent_id, &prompt)?;
                arguments.push((
                    participant.name.clone(),
                    participant.stance.clone(),
                    response,
                ));
            }
        }

        // The last participant is treated as the moderator and asked to
        // synthesize a conclusion from the full argument record.
        if let Some(moderator) = self.participants.last() {
            let prompt = build_moderator_prompt(&self.topic, &arguments, self.consensus_threshold);
            return runtime.ask_agent(moderator.agent_id, &prompt);
        }

        Err("Debate has no moderator".to_string())
    }
}

fn build_participant_prompt(
    topic: &str,
    round: usize,
    total_rounds: usize,
    participant: &Participant,
    arguments: &[(String, Stance, String)],
) -> String {
    let stance_text = match participant.stance {
        Stance::Pro => format!(
            "You are {} and you ARGUE IN FAVOR of the topic.",
            participant.name
        ),
        Stance::Con => format!(
            "You are {} and you ARGUE AGAINST the topic.",
            participant.name
        ),
        Stance::Neutral => format!("You are {} and you OBSERVE NEUTRALLY.", participant.name),
    };

    let mut prompt = format!(
        "Debate topic: {}\nRound {}/{}\n{}\n\n",
        topic, round, total_rounds, stance_text
    );

    if arguments.is_empty() {
        prompt.push_str("No arguments have been made yet. Present your opening position.");
    } else {
        prompt.push_str("Previous arguments:\n");
        for (name, stance, argument) in arguments {
            prompt.push_str(&format!("- {} ({}): {}\n", name, stance.as_str(), argument));
        }
        prompt.push_str("\nRespond to the topic and the arguments above.");
    }

    prompt
}

fn build_moderator_prompt(
    topic: &str,
    arguments: &[(String, Stance, String)],
    consensus_threshold: f64,
) -> String {
    let mut prompt = format!(
        "You are the moderator. The debate topic was: {}\n\nAll arguments:\n",
        topic
    );
    for (name, stance, argument) in arguments {
        prompt.push_str(&format!("- {} ({}): {}\n", name, stance.as_str(), argument));
    }
    prompt.push_str(&format!(
        "\nSynthesize a final conclusion. Consensus threshold: {}. \
         State whether consensus was reached and summarize the key points.",
        consensus_threshold
    ));
    prompt
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

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

    impl DebateRuntime for MockRuntime {
        fn ask_agent(&mut self, agent_id: u64, prompt: &str) -> Result<String, String> {
            self.calls.borrow_mut().push((agent_id, prompt.to_string()));
            self.responses
                .get(&agent_id)
                .cloned()
                .ok_or_else(|| format!("No response configured for agent {}", agent_id))
        }
    }

    #[test]
    fn test_empty_debate_errors() {
        let debate = Debate::new("topic", 1, 0.8);
        let mut rt = MockRuntime::new(HashMap::new());
        assert_eq!(
            debate.run(&mut rt),
            Err("Debate has no participants".to_string())
        );
    }

    #[test]
    fn test_debate_runs_participants_and_moderator() {
        let debate = Debate::new("microservices vs monolith", 1, 0.8)
            .participant("pro", "pro", 1)
            .participant("con", "con", 2)
            .participant("moderator", "neutral", 3);
        let mut rt = MockRuntime::new(HashMap::from([
            (1, "pro argument".to_string()),
            (2, "con argument".to_string()),
            (3, "conclusion".to_string()),
        ]));

        let result = debate.run(&mut rt).unwrap();
        assert_eq!(result, "conclusion");

        let calls = rt.calls.into_inner();
        // pro, con, moderator-as-participant, moderator-as-synthesizer
        assert_eq!(calls.len(), 4);
        assert!(calls[0].1.contains("pro"));
        assert!(calls[1].1.contains("con"));
        assert!(calls[3].1.contains("moderator"));
    }

    #[test]
    fn test_multiple_rounds_chain_arguments() {
        let debate = Debate::new("topic", 2, 0.5)
            .participant("p", "pro", 1)
            .participant("m", "neutral", 2);
        let mut rt = MockRuntime::new(HashMap::from([
            (1, "arg1".to_string()),
            (2, "arg2".to_string()),
        ]));

        let result = debate.run(&mut rt).unwrap();
        assert_eq!(result, "arg2");

        let calls = rt.calls.into_inner();
        // 2 rounds * 2 participants + moderator synthesis = 5
        assert_eq!(calls.len(), 5);
        // Round 2 participant prompt should include round 1 argument.
        assert!(calls[2].1.contains("arg1"));
    }

    #[test]
    fn test_stance_parsing() {
        let debate = Debate::new("topic", 1, 0.8)
            .participant("a", "PRO", 1)
            .participant("b", "against", 2)
            .participant("c", "neutral", 3);
        assert_eq!(debate.participants[0].stance, Stance::Pro);
        assert_eq!(debate.participants[1].stance, Stance::Con);
        assert_eq!(debate.participants[2].stance, Stance::Neutral);
    }
}
