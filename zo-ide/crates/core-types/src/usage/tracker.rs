//! Running aggregation of [`TokenUsage`] across a session's turns.

use crate::session::Session;

use super::TokenUsage;

/// Aggregates token usage across a running session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageTracker {
    latest_turn: TokenUsage,
    cumulative: TokenUsage,
    /// Sum of tokens that were not served from cache — the truly new input
    /// consumed on each turn.
    cumulative_new_input_tokens: u32,
    turns: u32,
    /// The cumulative usage this tracker was REHYDRATED with (a resumed
    /// session's stored history). `cumulative - rehydrated` is what THIS
    /// process actually consumed — the number a per-run report (headless
    /// `--output-format json`) must bill, or every stage of a resumed
    /// multi-stage run re-reports the whole session and a consumer that sums
    /// runs double-counts (observed: a 3-stage bench inflated zo's token
    /// total by ~75% vs the provider-billed truth).
    rehydrated: TokenUsage,
}

impl UsageTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_session(session: &Session) -> Self {
        let mut tracker = Self::new();
        for message in session.messages.iter() {
            if let Some(usage) = message.usage {
                tracker.record(usage);
            }
        }
        tracker.rehydrated = tracker.cumulative;
        tracker
    }

    /// The cumulative usage carried in from a resumed session's stored
    /// history at construction — zero for a fresh session. Subtract from
    /// [`Self::cumulative_usage`] for this process's own consumption.
    #[must_use]
    pub fn rehydrated_usage(&self) -> TokenUsage {
        self.rehydrated
    }

    pub fn record(&mut self, usage: TokenUsage) {
        self.latest_turn = usage;
        // Saturating accumulation: the per-field counters are `u32`, and a long
        // (or imported) session can push cumulative token totals past
        // `u32::MAX`. A plain `+=` panics in debug and silently wraps in
        // release, corrupting `/cost` and usage summaries. `TokenUsage`'s own
        // helpers already saturate, so the tracker must match rather than be the
        // one place that overflows.
        self.cumulative.input_tokens = self
            .cumulative
            .input_tokens
            .saturating_add(usage.input_tokens);
        self.cumulative.output_tokens = self
            .cumulative
            .output_tokens
            .saturating_add(usage.output_tokens);
        self.cumulative.cache_creation_input_tokens = self
            .cumulative
            .cache_creation_input_tokens
            .saturating_add(usage.cache_creation_input_tokens);
        self.cumulative.cache_read_input_tokens = self
            .cumulative
            .cache_read_input_tokens
            .saturating_add(usage.cache_read_input_tokens);
        let new_tokens = usage
            .input_tokens
            .saturating_sub(usage.cache_read_input_tokens);
        self.cumulative_new_input_tokens =
            self.cumulative_new_input_tokens.saturating_add(new_tokens);
        self.turns = self.turns.saturating_add(1);
    }

    #[must_use]
    pub fn current_turn_usage(&self) -> TokenUsage {
        self.latest_turn
    }

    #[must_use]
    pub fn cumulative_usage(&self) -> TokenUsage {
        self.cumulative
    }

    #[must_use]
    pub fn turns(&self) -> u32 {
        self.turns
    }

    /// Returns the total number of input tokens that were not served from cache
    /// across all recorded turns — i.e. the truly new input consumed.
    #[must_use]
    pub fn cumulative_new_input_tokens(&self) -> u32 {
        self.cumulative_new_input_tokens
    }

    /// Returns a one-line summary of cumulative new-input-token consumption.
    #[must_use]
    pub fn new_input_summary_line(&self) -> String {
        format!(
            "new_input_tokens_cumulative={}",
            self.cumulative_new_input_tokens
        )
    }
}
