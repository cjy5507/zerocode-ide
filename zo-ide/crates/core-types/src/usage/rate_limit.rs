//! Anthropic *unified* rate-limit windows (5-hour / 7-day) parsed from the
//! subscription/OAuth response headers and surfaced to the HUD gauge.

/// One unified rate-limit window's utilization and reset time, parsed from
/// Anthropic's `anthropic-ratelimit-unified-{5h,7d}-*` response headers
/// (returned on subscription / OAuth requests).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimitWindow {
    /// Fraction of the window consumed; clamped to `0.0..=1.0` on read.
    pub utilization: f64,
    /// Unix epoch seconds at which the window fully resets, when the header
    /// supplied a parseable reset time.
    pub resets_at_unix: Option<u64>,
}

impl RateLimitWindow {
    /// Percentage form (`0..=100`) for display, rounded to the nearest int.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn used_percent(self) -> u8 {
        // clamp keeps the product in 0.0..=100.0, so the cast never truncates
        // meaningfully or wraps a negative.
        (self.utilization.clamp(0.0, 1.0) * 100.0).round() as u8
    }
}

/// Which unified window the server flags as the binding constraint, from
/// `anthropic-ratelimit-unified-representative-claim`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitWindowKind {
    FiveHour,
    SevenDay,
}

/// Snapshot of Anthropic's *unified* rate-limit headers — the 5-hour and
/// 7-day rolling windows surfaced to Claude Pro/Max subscription tokens.
///
/// Every field is optional: a response may omit a window, and API-key
/// requests omit the unified headers entirely (the snapshot is then empty and
/// the HUD shows no gauge).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RateLimitSnapshot {
    pub five_hour: Option<RateLimitWindow>,
    pub seven_day: Option<RateLimitWindow>,
    /// Window the server flags as currently most constraining, if reported.
    pub representative: Option<RateLimitWindowKind>,
}

impl RateLimitSnapshot {
    /// True when at least one window carries data worth displaying.
    #[must_use]
    pub fn has_data(self) -> bool {
        self.five_hour.is_some() || self.seven_day.is_some()
    }
}
