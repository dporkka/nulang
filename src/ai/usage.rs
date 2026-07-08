//! Token-usage tracking and cost estimation for the AI Runtime.

use crate::ai::request::ModelPricing;
use crate::ai::response::TokenUsage;

/// Estimated cost in USD for a given usage and pricing rates.
pub fn estimated_cost(usage: &TokenUsage, pricing: &ModelPricing) -> f64 {
    let input_cost = (usage.prompt as f64) * pricing.input_cost_per_1k / 1000.0;
    let output_cost = (usage.completion as f64) * pricing.output_cost_per_1k / 1000.0;
    input_cost + output_cost
}

/// Accumulated token usage and cost summary for an agent.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UsageSummary {
    pub prompt: u32,
    pub completion: u32,
    pub total: u32,
    pub cost: f64,
}

impl UsageSummary {
    /// Create an empty summary.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add a single request's usage to the summary, computing cost when pricing
    /// is available.
    pub fn accumulate(&mut self, usage: &TokenUsage, pricing: Option<&ModelPricing>) {
        self.prompt = self.prompt.saturating_add(usage.prompt);
        self.completion = self.completion.saturating_add(usage.completion);
        self.total = self.total.saturating_add(usage.total);
        if let Some(p) = pricing {
            self.cost += estimated_cost(usage, p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimated_cost_known_pricing() {
        let usage = TokenUsage::new(1000, 500);
        let pricing = ModelPricing {
            input_cost_per_1k: 0.03,
            output_cost_per_1k: 0.06,
        };
        // 1000 input tokens @ $0.03/1k = $0.03
        // 500 output tokens @ $0.06/1k = $0.03
        assert!((estimated_cost(&usage, &pricing) - 0.06).abs() < 1e-9);
    }

    #[test]
    fn test_usage_summary_accumulate() {
        let mut summary = UsageSummary::empty();
        let pricing = ModelPricing {
            input_cost_per_1k: 0.01,
            output_cost_per_1k: 0.02,
        };

        summary.accumulate(&TokenUsage::new(1000, 500), Some(&pricing));
        assert_eq!(summary.prompt, 1000);
        assert_eq!(summary.completion, 500);
        assert_eq!(summary.total, 1500);
        assert!((summary.cost - 0.02).abs() < 1e-9);

        summary.accumulate(&TokenUsage::new(2000, 1000), Some(&pricing));
        assert_eq!(summary.prompt, 3000);
        assert_eq!(summary.completion, 1500);
        assert_eq!(summary.total, 4500);
        assert!((summary.cost - 0.06).abs() < 1e-9);
    }
}
