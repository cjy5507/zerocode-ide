//! Token-usage accounting for the runtime and CLI ledger.
//!
//! [`TokenUsage`] is the central value object (the provider's token counters
//! plus the cost it derives). Focused siblings own the rest: `pricing`
//! (per-model pricing data, the [`UsageCostEstimate`] type, and dollar
//! formatting), `rate_limit` (the unified 5h/7d windows), and `tracker`
//! (session-level aggregation). Re-exports keep `core_types::usage::*` and the
//! `core_types::*` re-exports unchanged.

use std::fmt::Write as _;

mod dashboard;
mod pricing;
mod rate_limit;
mod tracker;

pub use dashboard::{
    UsageDashboardRecord, UsageDashboardSnapshot, UsageModelRow, UsagePeriodRow,
    UsageSavingsSummary, UsageTokenTotals, estimate_usage_cost,
};
pub use pricing::{ModelPricing, UsageCostEstimate, format_usd, pricing_for_model};
pub use rate_limit::{RateLimitSnapshot, RateLimitWindow, RateLimitWindowKind};
pub use tracker::UsageTracker;

/// Anthropic's verbatim `usage.output_tokens_details` breakdown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OutputTokensDetails {
    pub thinking_tokens: u32,
}

/// Token counters accumulated for a conversation turn or session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
    pub output_tokens_details: Option<OutputTokensDetails>,
}

impl TokenUsage {
    /// Field-wise saturating subtraction — the "what did THIS process
    /// consume" delta between a cumulative reading and its rehydrated
    /// baseline. Saturating because an imported/edited transcript can carry a
    /// baseline larger than a later reading; a wrapped counter would report
    /// astronomical usage.
    #[must_use]
    pub fn saturating_sub(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_sub(other.input_tokens),
            output_tokens: self.output_tokens.saturating_sub(other.output_tokens),
            cache_creation_input_tokens: self
                .cache_creation_input_tokens
                .saturating_sub(other.cache_creation_input_tokens),
            cache_read_input_tokens: self
                .cache_read_input_tokens
                .saturating_sub(other.cache_read_input_tokens),
            // The provider reports this decomposition for one response, not
            // for cumulative usage snapshots. Never manufacture a delta.
            output_tokens_details: None,
        }
    }

    #[must_use]
    pub fn total_tokens(self) -> u32 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_input_tokens)
            .saturating_add(self.cache_read_input_tokens)
    }

    #[must_use]
    pub fn total_tokens_u64(self) -> u64 {
        u64::from(self.input_tokens)
            .saturating_add(u64::from(self.output_tokens))
            .saturating_add(u64::from(self.cache_creation_input_tokens))
            .saturating_add(u64::from(self.cache_read_input_tokens))
    }

    /// Tokens that actually occupied the model's context window on the
    /// request side of a turn: the prompt the model read (`input`) plus
    /// everything served from or written to the prompt cache. Output
    /// tokens are deliberately excluded — they are the model's *response*,
    /// not context the next request must carry.
    ///
    /// This is the honest "ctx used" figure for the live ledger: unlike
    /// `total_tokens` it never double-counts generation, and unlike a
    /// chars/4 transcript estimate it is the provider's own count.
    #[must_use]
    pub fn context_tokens(self) -> u32 {
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens)
            .saturating_add(self.cache_creation_input_tokens)
    }

    #[must_use]
    pub fn estimate_cost_usd(self) -> UsageCostEstimate {
        self.estimate_cost_usd_with_pricing(ModelPricing::default_sonnet_tier())
    }

    #[must_use]
    pub fn estimate_cost_usd_with_pricing(self, pricing: ModelPricing) -> UsageCostEstimate {
        UsageCostEstimate {
            input_cost_usd: cost_for_tokens(self.input_tokens, pricing.input_cost_per_million),
            output_cost_usd: cost_for_tokens(self.output_tokens, pricing.output_cost_per_million),
            cache_creation_cost_usd: cost_for_tokens(
                self.cache_creation_input_tokens,
                pricing.cache_creation_cost_per_million,
            ),
            cache_read_cost_usd: cost_for_tokens(
                self.cache_read_input_tokens,
                pricing.cache_read_cost_per_million,
            ),
        }
    }

    #[must_use]
    pub fn summary_lines_for_model(self, label: &str, model: Option<&str>) -> Vec<String> {
        let pricing = model.and_then(pricing_for_model);
        let cost = pricing.map_or_else(
            || self.estimate_cost_usd(),
            |pricing| self.estimate_cost_usd_with_pricing(pricing),
        );
        let mut summary = String::new();
        let _ = write!(
            &mut summary,
            "{label}: total_tokens={} input={} output={} cache_write={} cache_read={} estimated_cost={}",
            self.total_tokens(),
            self.input_tokens,
            self.output_tokens,
            self.cache_creation_input_tokens,
            self.cache_read_input_tokens,
            format_usd(cost.total_cost_usd()),
        );
        if let Some(model_name) = model {
            let _ = write!(&mut summary, " model={model_name}");
            if pricing.is_none() {
                summary.push_str(" pricing=estimated-default");
            }
        }

        let mut breakdown = String::new();
        let _ = write!(
            &mut breakdown,
            "  cost breakdown: input={} output={} cache_write={} cache_read={}",
            format_usd(cost.input_cost_usd),
            format_usd(cost.output_cost_usd),
            format_usd(cost.cache_creation_cost_usd),
            format_usd(cost.cache_read_cost_usd),
        );

        vec![summary, breakdown]
    }
}

fn cost_for_tokens(tokens: u32, usd_per_million_tokens: f64) -> f64 {
    f64::from(tokens) / 1_000_000.0 * usd_per_million_tokens
}

#[cfg(test)]
mod tests;
