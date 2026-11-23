//! Pure usage-dashboard view models and aggregation helpers.
//!
//! This module deliberately owns no file I/O and has no TUI dependency. Runtime
//! and CLI code hand it already-observed usage counters; TUI widgets receive the
//! resulting snapshot and only render/navigate it. That keeps usage math,
//! persistence, and drawing as separate responsibilities.

use std::collections::BTreeMap;

use model_prices::{CostTotal, RequestTokens};

use super::{ModelPricing, TokenUsage, UsageCostEstimate, pricing::pricing_at};

/// Dashboard-only token counters that can safely aggregate many sessions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageTokenTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

impl UsageTokenTotals {
    #[must_use]
    pub fn from_usage(usage: TokenUsage) -> Self {
        Self {
            input_tokens: u64::from(usage.input_tokens),
            output_tokens: u64::from(usage.output_tokens),
            cache_creation_input_tokens: u64::from(usage.cache_creation_input_tokens),
            cache_read_input_tokens: u64::from(usage.cache_read_input_tokens),
        }
    }

    #[must_use]
    pub fn total_tokens(self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_input_tokens)
            .saturating_add(self.cache_read_input_tokens)
    }

    #[must_use]
    pub fn to_saturated_usage(self) -> TokenUsage {
        TokenUsage {
            input_tokens: u32::try_from(self.input_tokens).unwrap_or(u32::MAX),
            output_tokens: u32::try_from(self.output_tokens).unwrap_or(u32::MAX),
            cache_creation_input_tokens: u32::try_from(self.cache_creation_input_tokens)
                .unwrap_or(u32::MAX),
            cache_read_input_tokens: u32::try_from(self.cache_read_input_tokens)
                .unwrap_or(u32::MAX),
            output_tokens_details: None,
        }
    }

    fn add(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(other.cache_creation_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(other.cache_read_input_tokens);
    }
}

/// One observed usage bucket from a persisted or live session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageDashboardRecord {
    /// Stable session id. Used only for diagnostics/tests; aggregation is by
    /// date/model.
    pub session_id: String,
    /// Session-level timestamp in Unix milliseconds. Persisted sessions do not
    /// yet store per-turn timestamps, so callers pass the session update time.
    pub occurred_at_ms: u64,
    /// Model attributed to this session. Prefer session preferences; fallback to
    /// the active model for legacy sessions with no per-session model file.
    pub model: String,
    /// Token counters observed for this session.
    pub usage: UsageTokenTotals,
}

/// One row in a period-oriented usage table (daily/monthly).
#[derive(Debug, Clone, PartialEq)]
pub struct UsagePeriodRow {
    /// Human label for the bucket, e.g. `2026-03-31` or `2026-03`.
    pub label: String,
    /// Tokens billed/observed in this bucket.
    pub tokens: u64,
    /// Estimated USD cost in this bucket.
    pub cost_usd: Option<f64>,
    pub unpriced_records: u64,
    /// Estimated USD saved in this bucket.
    pub saved_usd: f64,
    /// Dominant model for the bucket.
    pub top_model: String,
}

/// One model-share row.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageModelRow {
    /// Model alias/display name.
    pub model: String,
    /// Token total attributed to this model.
    pub tokens: u64,
    /// Estimated USD cost attributed to this model.
    pub cost_usd: Option<f64>,
    pub unpriced_records: u64,
    /// Estimated USD saved for this model.
    pub saved_usd: f64,
    /// Share of total dashboard tokens (`0.0..=1.0`).
    pub share: f64,
}

/// Estimated savings breakdown shown by the dashboard.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSavingsSummary {
    /// Actual estimated cost.
    pub actual_cost_usd: Option<f64>,
    pub unpriced_records: u64,
    /// Baseline cost before cache/model-mix savings.
    pub baseline_cost_usd: f64,
    /// Savings from prompt-cache reads being cheaper than normal input tokens.
    pub cache_savings_usd: f64,
    /// Savings from cheaper model mix vs the active/baseline model.
    pub model_mix_savings_usd: f64,
    /// Total estimated savings.
    pub total_savings_usd: f64,
}

/// Immutable snapshot consumed by the `/usage` modal.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageDashboardSnapshot {
    /// Active/baseline model at the moment the snapshot was built.
    pub model: String,
    /// Number of usage records/sessions folded into the snapshot.
    pub turns: u32,
    /// Cumulative token counters, saturated for legacy callers that still expect
    /// `TokenUsage`. UI totals use [`Self::total_tokens`] to avoid truncation.
    pub total_usage: TokenUsage,
    /// Unsaturated cumulative token count for dashboard display and shares.
    pub total_tokens: u64,
    /// Cumulative estimated cost.
    pub total_cost_usd: Option<f64>,
    pub unpriced_records: u64,
    /// Daily rows aggregated from persisted/live session usage.
    pub daily: Vec<UsagePeriodRow>,
    /// Monthly rows aggregated from persisted/live session usage.
    pub monthly: Vec<UsagePeriodRow>,
    /// Model-share rows aggregated from persisted/live session usage.
    pub models: Vec<UsageModelRow>,
    /// Savings summary.
    pub savings: UsageSavingsSummary,
    /// Honest data-quality note for the UI footer/detail pane.
    pub note: String,
}

#[derive(Debug, Clone, Default)]
struct UsageAccumulator {
    usage: UsageTokenTotals,
    cost: CostTotal,
    comparable_cost_usd: f64,
    cache_savings_usd: f64,
    baseline_cost_usd: f64,
    model_tokens: BTreeMap<String, u64>,
}

impl UsageAccumulator {
    fn add(&mut self, record: &UsageDashboardRecord, amounts: &RecordCost) {
        self.usage.add(record.usage);
        self.cost.record(amounts.estimate);
        self.cache_savings_usd += amounts.cache_savings;
        if let Some((baseline, actual)) = amounts.baseline.zip(amounts.estimate) {
            self.baseline_cost_usd += baseline;
            self.comparable_cost_usd += actual;
        }
        *self.model_tokens.entry(record.model.clone()).or_default() += record.usage.total_tokens();
    }

    fn tokens(&self) -> u64 {
        self.usage.total_tokens()
    }

    fn top_model(&self) -> String {
        self.model_tokens
            .iter()
            .max_by(|left, right| left.1.cmp(right.1).then_with(|| right.0.cmp(left.0)))
            .map_or_else(|| "unknown".to_string(), |(model, _)| model.clone())
    }

    fn saved_usd(&self) -> f64 {
        let model_mix = (self.baseline_cost_usd - self.comparable_cost_usd).max(0.0);
        self.cache_savings_usd + model_mix
    }
}

impl UsageDashboardSnapshot {
    /// Build a no-I/O dashboard snapshot from persisted/live usage records.
    #[must_use]
    pub fn from_records(
        baseline_model: impl Into<String>,
        records: impl IntoIterator<Item = UsageDashboardRecord>,
        note: impl Into<String>,
    ) -> Self {
        let baseline_model = baseline_model.into();
        let mut total = UsageAccumulator::default();
        let mut daily: BTreeMap<String, UsageAccumulator> = BTreeMap::new();
        let mut monthly: BTreeMap<String, UsageAccumulator> = BTreeMap::new();
        let mut models: BTreeMap<String, UsageAccumulator> = BTreeMap::new();
        let mut record_count = 0u32;

        for record in records {
            record_count = record_count.saturating_add(1);
            let day = day_label(record.occurred_at_ms);
            let month = month_label(record.occurred_at_ms);
            let amounts = RecordCost::estimate(&record, &baseline_model);
            total.add(&record, &amounts);
            daily.entry(day).or_default().add(&record, &amounts);
            monthly.entry(month).or_default().add(&record, &amounts);
            models
                .entry(record.model.clone())
                .or_default()
                .add(&record, &amounts);
        }

        let total_tokens = total.tokens();
        let mut daily_rows = period_rows(daily);
        daily_rows.sort_by(|left, right| right.label.cmp(&left.label));
        let mut monthly_rows = period_rows(monthly);
        monthly_rows.sort_by(|left, right| right.label.cmp(&left.label));
        let mut model_rows: Vec<UsageModelRow> = models
            .into_iter()
            .map(|(model, acc)| UsageModelRow {
                model,
                tokens: acc.tokens(),
                cost_usd: acc.cost.cost_usd,
                unpriced_records: acc.cost.unpriced_requests,
                saved_usd: acc.saved_usd(),
                share: token_share(acc.tokens(), total_tokens),
            })
            .collect();
        model_rows.sort_by(|left, right| {
            right
                .cost_usd
                .partial_cmp(&left.cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.tokens.cmp(&left.tokens))
                .then_with(|| left.model.cmp(&right.model))
        });

        let model_mix_savings_usd = (total.baseline_cost_usd - total.comparable_cost_usd).max(0.0);
        let total_savings_usd = total.cache_savings_usd + model_mix_savings_usd;
        Self {
            model: baseline_model,
            turns: record_count,
            total_usage: total.usage.to_saturated_usage(),
            total_tokens,
            total_cost_usd: total.cost.cost_usd,
            unpriced_records: total.cost.unpriced_requests,
            daily: daily_rows,
            monthly: monthly_rows,
            models: model_rows,
            savings: UsageSavingsSummary {
                actual_cost_usd: total.cost.cost_usd,
                unpriced_records: total.cost.unpriced_requests,
                baseline_cost_usd: total.cost.cost_usd.unwrap_or(0.0) + total_savings_usd,
                cache_savings_usd: total.cache_savings_usd,
                model_mix_savings_usd,
                total_savings_usd,
            },
            note: note.into(),
        }
    }

    /// Build a no-I/O dashboard snapshot from the live session usage.
    #[must_use]
    pub fn from_session(model: impl Into<String>, usage: TokenUsage, turns: u32) -> Self {
        let model = model.into();
        let mut snapshot = Self::from_records(
            model.clone(),
            [UsageDashboardRecord {
                session_id: "live".to_string(),
                occurred_at_ms: current_unix_millis(),
                model,
                usage: UsageTokenTotals::from_usage(usage),
            }],
            format!("Live session snapshot · {turns} recorded turn(s)"),
        );
        snapshot.turns = turns;
        snapshot
    }

    /// Whether no usage has been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total_tokens == 0 && self.turns == 0
    }
}

fn period_rows(buckets: BTreeMap<String, UsageAccumulator>) -> Vec<UsagePeriodRow> {
    buckets
        .into_iter()
        .map(|(label, acc)| UsagePeriodRow {
            label,
            tokens: acc.tokens(),
            cost_usd: acc.cost.cost_usd,
            unpriced_records: acc.cost.unpriced_requests,
            saved_usd: acc.saved_usd(),
            top_model: acc.top_model(),
        })
        .collect()
}

fn token_share(tokens: u64, total_tokens: u64) -> f64 {
    if total_tokens == 0 {
        return 0.0;
    }
    let mut numerator = tokens.min(total_tokens);
    let mut denominator = total_tokens;
    while denominator > u64::from(u32::MAX) {
        numerator = numerator.saturating_add(1) / 2;
        denominator = denominator.saturating_add(1) / 2;
    }
    let numerator = u32::try_from(numerator).unwrap_or(u32::MAX);
    let denominator = u32::try_from(denominator).unwrap_or(u32::MAX);
    if denominator == 0 {
        0.0
    } else {
        f64::from(numerator) / f64::from(denominator)
    }
}

fn cache_read_savings_usd(usage: UsageTokenTotals, pricing: ModelPricing) -> f64 {
    let undiscounted = cost_for_tokens(usage.cache_read_input_tokens, pricing.input_cost_per_million);
    let discounted = cost_for_tokens(usage.cache_read_input_tokens, pricing.cache_read_cost_per_million);
    (undiscounted - discounted).max(0.0)
}

struct RecordCost {
    estimate: Option<f64>,
    baseline: Option<f64>,
    cache_savings: f64,
}

impl RecordCost {
    fn estimate(record: &UsageDashboardRecord, baseline: &str) -> Self {
        let day = model_prices::day_at_millis(i64::try_from(record.occurred_at_ms).unwrap_or(i64::MAX));
        let tokens = record.usage.request_tokens();
        let estimate = |model| model_prices::aggregate_cost(model, day, tokens).map(UsageCostEstimate::total_cost_usd);
        let cost = estimate(record.model.as_str());
        let cache_savings = cost.and_then(|_| pricing_at(&record.model, day))
            .map_or(0.0, |pricing| cache_read_savings_usd(record.usage, pricing));
        Self {
            estimate: cost,
            baseline: estimate(baseline),
            cache_savings,
        }
    }
}

impl UsageTokenTotals {
    fn request_tokens(self) -> RequestTokens {
        RequestTokens {
            input: self.input_tokens,
            output: self.output_tokens,
            cache_read: self.cache_read_input_tokens,
            cache_write: self.cache_creation_input_tokens,
            ..RequestTokens::default()
        }
    }
}

fn cost_for_tokens(tokens: u64, usd_per_million_tokens: f64) -> f64 {
    tokens_to_f64(tokens) / 1_000_000.0 * usd_per_million_tokens
}

fn tokens_to_f64(tokens: u64) -> f64 {
    let mut scaled = tokens;
    let mut divisor = 1.0;
    while scaled > u64::from(u32::MAX) {
        scaled = scaled.saturating_add(1) / 2;
        divisor *= 2.0;
    }
    f64::from(u32::try_from(scaled).unwrap_or(u32::MAX)) * divisor
}

fn current_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn day_label(ms: u64) -> String {
    let days = i64::try_from(ms / 86_400_000).unwrap_or(i64::MAX);
    let (year, month, day) = crate::date::civil_from_unix_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn month_label(ms: u64) -> String {
    let days = i64::try_from(ms / 86_400_000).unwrap_or(i64::MAX);
    let (year, month, _) = crate::date::civil_from_unix_days(days);
    format!("{year:04}-{month:02}")
}

/// Estimate cumulative usage when the counters identify a single rate.
#[must_use]
pub fn estimate_usage_cost(model: &str, usage: TokenUsage) -> Option<UsageCostEstimate> {
    model_prices::aggregate_cost(model, model_prices::today(), UsageTokenTotals::from_usage(usage).request_tokens())
}

/// Estimate one request, including any hour-long cache writes it reported.
#[must_use]
pub fn estimate_request_cost(model: &str, usage: TokenUsage, cache_write_1h: u64) -> Option<UsageCostEstimate> {
    let tokens = RequestTokens {
        cache_write_1h,
        ..UsageTokenTotals::from_usage(usage).request_tokens()
    };
    model_prices::request_cost(model, model_prices::today(), tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_snapshot_groups_daily_monthly_models_and_savings() {
        let records = vec![
            UsageDashboardRecord {
                session_id: "a".to_string(),
                occurred_at_ms: 1_788_134_400_000, // 2026-08-31 UTC
                model: "claude-sonnet-4".to_string(),
                usage: UsageTokenTotals {
                    input_tokens: 1_000,
                    output_tokens: 500,
                    cache_creation_input_tokens: 200,
                    cache_read_input_tokens: 10_000,
                },
            },
            UsageDashboardRecord {
                session_id: "b".to_string(),
                occurred_at_ms: 1_788_220_800_000, // 2026-09-01 UTC
                model: "gpt-5.5".to_string(),
                usage: UsageTokenTotals {
                    input_tokens: 2_000,
                    output_tokens: 800,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 5_000,
                },
            },
        ];
        let snapshot = UsageDashboardSnapshot::from_records(
            "claude-sonnet-4",
            records,
            "Historical sessions scanned",
        );
        assert_eq!(snapshot.turns, 2);
        assert_eq!(snapshot.daily.len(), 2);
        assert_eq!(snapshot.monthly.len(), 2);
        assert_eq!(snapshot.models.len(), 2);
        assert_eq!(snapshot.daily[0].label, "2026-09-01");
        assert_eq!(snapshot.daily[1].label, "2026-08-31");
        assert!(snapshot.savings.cache_savings_usd > 0.0);
        assert!(snapshot.savings.total_savings_usd >= snapshot.savings.cache_savings_usd);
    }

    #[test]
    fn dashboard_marks_incomplete_totals_without_hiding_the_model() {
        let at = 1_787_616_000_000;
        let record = |model: &str| UsageDashboardRecord {
            session_id: model.to_owned(),
            occurred_at_ms: at,
            model: model.to_owned(),
            usage: UsageTokenTotals { input_tokens: 1_000, ..UsageTokenTotals::default() },
        };
        let first = record("gpt-5.4");
        let second = record("synthetic-provider-model");
        let snapshot = UsageDashboardSnapshot::from_records("gpt-5.4", [first, second], "");
        assert!(snapshot.total_cost_usd.is_some_and(|cost| cost > 0.0));
        assert_eq!(snapshot.unpriced_records, 1);
        assert_eq!(snapshot.models[1].model, "synthetic-provider-model");
        assert_eq!(snapshot.models[1].cost_usd, None);
    }

    #[test]
    fn live_session_snapshot_preserves_turn_count() {
        let snapshot = UsageDashboardSnapshot::from_session(
            "gpt-5.5",
            TokenUsage {
                input_tokens: 1,
                output_tokens: 2,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens_details: None,
            },
            9,
        );
        assert_eq!(snapshot.turns, 9);
        assert_eq!(snapshot.total_tokens, 3);
    }

    #[test]
    fn dashboard_totals_do_not_saturate_large_aggregates() {
        let huge = u64::from(u32::MAX) + 10;
        let snapshot = UsageDashboardSnapshot::from_records(
            "gpt-5.5",
            [UsageDashboardRecord {
                session_id: "huge".to_string(),
                occurred_at_ms: 1_711_843_200_000,
                model: "gpt-5.5".to_string(),
                usage: UsageTokenTotals {
                    input_tokens: huge,
                    output_tokens: huge,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                },
            }],
            "huge",
        );
        assert_eq!(snapshot.total_tokens, huge.saturating_mul(2));
        assert_eq!(snapshot.daily[0].tokens, huge.saturating_mul(2));
        assert_eq!(snapshot.total_usage.input_tokens, u32::MAX);
    }

    #[test]
    fn token_share_preserves_large_token_ratios() {
        let half = token_share(u64::from(u32::MAX) + 1, (u64::from(u32::MAX) + 1) * 2);
        assert!((half - 0.5).abs() < 0.000_001);
    }

    #[test]
    fn empty_snapshot_is_detected() {
        let snapshot = UsageDashboardSnapshot::from_records("gpt-5.5", [], "empty");
        assert!(snapshot.is_empty());
        assert!(snapshot.daily.is_empty());
        assert!(snapshot.monthly.is_empty());
        assert!(snapshot.models.is_empty());
    }

    #[test]
    fn unix_date_labels_are_stable() {
        assert_eq!(day_label(0), "1970-01-01");
        assert_eq!(month_label(1_711_843_200_000), "2024-03");
    }
}
