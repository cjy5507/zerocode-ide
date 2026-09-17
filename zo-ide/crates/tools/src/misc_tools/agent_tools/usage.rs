use std::path::{Path, PathBuf};

use core_types::TokenUsage;
use serde::{Deserialize, Serialize};

use super::{AgentRegistry, manifest};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AgentUsageCost {
    pub requests: u64,
    pub unreported_requests: u64,
    pub unpriced_requests: u64,
    pub rejected_requests: u64,
    pub estimated_cost_usd: f64,
}

impl AgentUsageCost {
    pub(crate) fn accumulate(&mut self, other: &Self) {
        self.requests = self.requests.saturating_add(other.requests);
        self.unreported_requests = self.unreported_requests.saturating_add(other.unreported_requests);
        self.unpriced_requests = self.unpriced_requests.saturating_add(other.unpriced_requests);
        self.rejected_requests = self.rejected_requests.saturating_add(other.rejected_requests);
        self.estimated_cost_usd += other.estimated_cost_usd;
    }

    fn record_usage(&mut self, model: &str, usage: &TokenUsage, complete: bool) {
        if complete {
            self.unreported_requests = self.unreported_requests.saturating_sub(1);
        }
        if let Some(pricing) = core_types::pricing_for_model(model) {
            self.estimated_cost_usd += usage.estimate_cost_usd_with_pricing(pricing).total_cost_usd();
        } else {
            self.unpriced_requests = self.unpriced_requests.saturating_add(1);
        }
    }
}

pub(crate) fn agent_usage_cost_for_attempt(
    registry: &AgentRegistry,
    id: &str,
    generation: u64,
) -> Option<AgentUsageCost> {
    let output = registry.manifest_by_id(id)?;
    (output.run_generation == generation).then_some(output.activity.usage_cost).flatten()
}

pub(super) struct RequestUsageGuard {
    manifest_path: Option<PathBuf>,
    generation: Option<u64>,
    model: String,
    usage: Option<TokenUsage>,
    complete: bool,
    rejected: bool,
}

impl RequestUsageGuard {
    pub(super) fn open(path: Option<&Path>, generation: Option<u64>, model: &str) -> Self {
        if let (Some(path), Some(generation)) = (path, generation) {
            manifest::update_agent_usage_cost(path, generation, |cost| {
                cost.requests = cost.requests.saturating_add(1);
                cost.unreported_requests = cost.unreported_requests.saturating_add(1);
            });
        }
        Self {
            manifest_path: path.map(Path::to_path_buf),
            generation,
            model: model.to_owned(),
            usage: None,
            complete: false,
            rejected: false,
        }
    }

    pub(super) fn reject(&mut self, error: &api::ApiError) {
        self.rejected = self.usage.is_none() && matches!(error, api::ApiError::Api { status, .. }
            if matches!(status.as_u16(), 401 | 403 | 429 | 529));
    }

    pub(super) fn observe(&mut self, usage: &TokenUsage) {
        if self.observe_start(usage) {
            self.complete = true;
        }
    }

    pub(super) fn observe_start(&mut self, usage: &TokenUsage) -> bool {
        if usage.input_tokens == 0 && usage.output_tokens == 0
            && usage.cache_creation_input_tokens == 0 && usage.cache_read_input_tokens == 0
        {
            return false;
        }
        // Stream adapters report cumulative usage; start and delta frames can
        // repeat counters or omit the input counters in later frames.
        let total = self.usage.get_or_insert_with(TokenUsage::default);
        total.input_tokens = total.input_tokens.max(usage.input_tokens);
        total.output_tokens = total.output_tokens.max(usage.output_tokens);
        total.cache_creation_input_tokens = total.cache_creation_input_tokens.max(usage.cache_creation_input_tokens);
        total.cache_read_input_tokens = total.cache_read_input_tokens.max(usage.cache_read_input_tokens);
        true
    }
}

impl Drop for RequestUsageGuard {
    fn drop(&mut self) {
        if let (Some(path), Some(generation)) = (&self.manifest_path, self.generation) {
            manifest::update_agent_usage_cost(path, generation, |cost| {
                if self.rejected {
                    cost.unreported_requests = cost.unreported_requests.saturating_sub(1);
                    cost.rejected_requests = cost.rejected_requests.saturating_add(1);
                } else if let Some(usage) = &self.usage {
                    cost.record_usage(&self.model, usage, self.complete);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_models_are_priced_separately_including_cache() {
        let sample = TokenUsage {
            input_tokens: 1_000, output_tokens: 500,
            cache_creation_input_tokens: 200, cache_read_input_tokens: 100,
            ..Default::default()
        };
        let mut cost = AgentUsageCost { requests: 2, unreported_requests: 2, ..Default::default() };
        let first = "claude-sonnet-4-6";
        let second = "claude-opus-4-6";
        cost.record_usage(first, &sample, true);
        cost.record_usage(second, &sample, true);
        let price = |model| sample.estimate_cost_usd_with_pricing(core_types::pricing_for_model(model).unwrap()).total_cost_usd();
        assert!((cost.estimated_cost_usd - price(first) - price(second)).abs() < 1e-12);
        assert!((cost.estimated_cost_usd - 2.0 * price(second)).abs() > 1e-6);
        assert_eq!(cost.unreported_requests, 0);
    }

    #[test]
    fn empty_and_partial_usage_are_not_claimed_complete() {
        let mut guard = RequestUsageGuard::open(None, None, "claude-sonnet-4-6");
        guard.observe(&TokenUsage::default());
        assert!(!guard.complete);
        assert!(guard.usage.is_none());
        guard.observe_start(&TokenUsage { input_tokens: 100, ..Default::default() });
        assert!(!guard.complete);
        let mut cost = AgentUsageCost { requests: 1, unreported_requests: 1, ..Default::default() };
        cost.record_usage(&guard.model, guard.usage.as_ref().unwrap(), guard.complete);
        assert_eq!(cost.unreported_requests, 1);
        assert!(cost.estimated_cost_usd > 0.0);
    }

    #[test]
    fn missing_price_is_not_default_tier_or_free() {
        let mut cost = AgentUsageCost { requests: 1, unreported_requests: 1, ..Default::default() };
        cost.record_usage("unpriced-test-model-xyz", &TokenUsage { output_tokens: 100, ..Default::default() }, true);
        assert_eq!(cost.unreported_requests, 0);
        assert_eq!(cost.unpriced_requests, 1);
        assert!(cost.estimated_cost_usd.abs() < f64::EPSILON);
    }

    #[test]
    fn repeated_stream_frames_are_not_added_twice() {
        let mut guard = RequestUsageGuard::open(None, None, "unknown");
        let start = TokenUsage { input_tokens: 100, cache_read_input_tokens: 50, ..Default::default() };
        let delta = TokenUsage { output_tokens: 30, ..Default::default() };
        guard.observe(&start);
        guard.observe(&delta);
        guard.observe(&delta);
        assert_eq!(guard.usage, Some(TokenUsage { input_tokens: 100, output_tokens: 30, cache_read_input_tokens: 50, ..Default::default() }));
    }

    #[test]
    fn incomplete_requests_remain_visible_when_aggregated() {
        let mut cost = AgentUsageCost::default();
        cost.accumulate(&AgentUsageCost { requests: 2, unreported_requests: 1, unpriced_requests: 1, estimated_cost_usd: 0.0, ..Default::default() });
        cost.accumulate(&AgentUsageCost { requests: 1, estimated_cost_usd: 0.25, ..Default::default() });
        assert_eq!(cost.requests, 3);
        assert_eq!(cost.unreported_requests, 1);
        assert_eq!(cost.unpriced_requests, 1);
        assert!((cost.estimated_cost_usd - 0.25).abs() < f64::EPSILON);
    }
}
