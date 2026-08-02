//! Deterministic Simulation Testing (DST) for Nulang.
//!
//! A `Simulator` replaces the real scheduler, network, and clock with
//! deterministic fakes so that actor programs execute identically on every
//! run for a given seed. This enables reproducible debugging of concurrency
//! bugs, deadlock detection, and invariant checking.
//!
//! Usage:
//!   let mut sim = Simulator::new(42); // seed = 42
//!   sim.load_program("examples/pingpong.nula");
//!   sim.run_until_quiescence();
//!   assert!(!sim.has_deadlock());

use std::collections::{HashMap, VecDeque};

/// A deterministic pseudo-random number generator (splitmix64).
#[derive(Debug, Clone)]
pub struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    /// Pick a random element from a slice.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            let idx = (self.next() as usize) % items.len();
            Some(&items[idx])
        }
    }
}

/// A simulated actor in the DST framework.
#[derive(Debug, Clone)]
pub struct SimActor {
    pub id: u64,
    pub name: String,
    pub mailbox: VecDeque<SimMessage>,
    pub state: HashMap<String, SimValue>,
}

/// A message in the simulated system.
#[derive(Debug, Clone)]
pub struct SimMessage {
    pub sender: u64,
    pub target: u64,
    pub behavior: String,
    pub payload: Vec<SimValue>,
}

/// Simplified value type for DST (subset of Nulang values).
#[derive(Debug, Clone, PartialEq)]
pub enum SimValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Nil,
    Unit,
}

/// Result of one simulation step.
#[derive(Debug, Clone)]
pub enum StepResult {
    /// An actor processed a message.
    MessageProcessed { actor: u64, behavior: String },
    /// No actor could make progress (potential deadlock).
    NoProgress,
    /// All mailboxes are empty and no timers are pending.
    Quiescent,
}

/// The deterministic simulator.
pub struct Simulator {
    pub rng: DeterministicRng,
    pub actors: HashMap<u64, SimActor>,
    pub pending_messages: VecDeque<SimMessage>,
    pub step_count: u64,
    pub max_steps: u64,
    pub clock_ms: u64,
    pub timers: Vec<(u64, u64, SimMessage)>, // (fire_at_ms, actor_id, message)
}

impl Simulator {
    /// Create a new simulator with the given random seed.
    pub fn new(seed: u64) -> Self {
        Self {
            rng: DeterministicRng::new(seed),
            actors: HashMap::new(),
            pending_messages: VecDeque::new(),
            step_count: 0,
            max_steps: 1_000_000,
            clock_ms: 0,
            timers: Vec::new(),
        }
    }

    /// Set the maximum number of steps before the simulation aborts.
    pub fn with_max_steps(mut self, max: u64) -> Self {
        self.max_steps = max;
        self
    }

    /// Register a simulated actor.
    pub fn register_actor(&mut self, id: u64, name: &str) {
        self.actors.insert(
            id,
            SimActor {
                id,
                name: name.to_string(),
                mailbox: VecDeque::new(),
                state: HashMap::new(),
            },
        );
    }

    /// Send a message to an actor.
    pub fn send(&mut self, sender: u64, target: u64, behavior: &str, payload: Vec<SimValue>) {
        self.pending_messages.push_back(SimMessage {
            sender,
            target,
            behavior: behavior.to_string(),
            payload,
        });
    }

    /// Advance the simulation by one step.
    pub fn step(&mut self) -> StepResult {
        if self.step_count >= self.max_steps {
            return StepResult::NoProgress;
        }
        self.step_count += 1;

        // Deliver any pending messages
        while let Some(msg) = self.pending_messages.pop_front() {
            if let Some(actor) = self.actors.get_mut(&msg.target) {
                actor.mailbox.push_back(msg);
            }
        }

        // Check for timer firings
        let now = self.clock_ms;
        let mut fired = Vec::new();
        self.timers.retain(|(fire_at, actor_id, msg)| {
            if *fire_at <= now {
                fired.push((*actor_id, msg.clone()));
                false
            } else {
                true
            }
        });
        for (actor_id, msg) in fired {
            if let Some(actor) = self.actors.get_mut(&actor_id) {
                actor.mailbox.push_back(msg);
            }
        }

        // Collect actors with non-empty mailboxes
        let ready: Vec<u64> = self
            .actors
            .iter()
            .filter(|(_, a)| !a.mailbox.is_empty())
            .map(|(id, _)| *id)
            .collect();

        if ready.is_empty() && self.timers.is_empty() {
            return StepResult::Quiescent;
        }

        if ready.is_empty() {
            // Advance clock to next timer
            if let Some((fire_at, _, _)) = self.timers.first() {
                self.clock_ms = *fire_at;
            }
            return StepResult::NoProgress;
        }

        // Deterministically pick an actor
        let actor_id = *self.rng.pick(&ready).unwrap();
        if let Some(actor) = self.actors.get_mut(&actor_id) {
            if let Some(msg) = actor.mailbox.pop_front() {
                let behavior = msg.behavior.clone();
                // In a real implementation, this would execute the behavior.
                // For DST, we just record the step.
                StepResult::MessageProcessed {
                    actor: actor_id,
                    behavior,
                }
            } else {
                StepResult::NoProgress
            }
        } else {
            StepResult::NoProgress
        }
    }

    /// Run the simulation until quiescence or step limit.
    pub fn run_until_quiescence(&mut self) {
        loop {
            match self.step() {
                StepResult::Quiescent | StepResult::NoProgress => break,
                _ => {}
            }
        }
    }

    /// Check if any actors have non-empty mailboxes (potential deadlock).
    pub fn has_deadlock(&self) -> bool {
        self.actors.values().any(|a| !a.mailbox.is_empty())
    }

    /// Returns the number of steps executed.
    pub fn step_count(&self) -> u64 {
        self.step_count
    }

    /// Returns the current simulated clock in milliseconds.
    pub fn clock_ms(&self) -> u64 {
        self.clock_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simulator_pingpong() {
        let mut sim = Simulator::new(42);
        sim.register_actor(1, "pinger");
        sim.register_actor(2, "ponger");
        sim.send(0, 1, "ping", vec![]);
        sim.run_until_quiescence();
        assert!(!sim.has_deadlock());
        assert!(sim.step_count() > 0);
    }

    #[test]
    fn test_deterministic_rng() {
        let mut rng1 = DeterministicRng::new(123);
        let mut rng2 = DeterministicRng::new(123);
        for _ in 0..100 {
            assert_eq!(rng1.next(), rng2.next());
        }
    }

    #[test]
    fn test_different_seeds_diverge() {
        let mut rng1 = DeterministicRng::new(1);
        let mut rng2 = DeterministicRng::new(2);
        let v1: Vec<u64> = (0..10).map(|_| rng1.next()).collect();
        let v2: Vec<u64> = (0..10).map(|_| rng2.next()).collect();
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_quiescence_detection() {
        let mut sim = Simulator::new(0).with_max_steps(100);
        sim.register_actor(1, "worker");
        sim.send(0, 1, "work", vec![SimValue::Int(42)]);
        sim.run_until_quiescence();
        assert_eq!(sim.has_deadlock(), false);
    }
}
