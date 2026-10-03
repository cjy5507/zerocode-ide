//! What the whole day has cost, as far as the window watched it (t-26583).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

// The tests reach `MS_PER_DAY` through this module; the stub has no use for it.
#[allow(unused_imports)]
use crate::civil::MS_PER_DAY;

/// One bucket's span: a minute.
pub const BUCKET_MS: i64 = 60_000;

/// A rolling day of spend, in API-equivalent dollars.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DaySpend {
    /// `(minute, dollars)`, oldest first.
    buckets: VecDeque<(i64, f64)>,
}

impl DaySpend {
    /// Adds what a model call cost.
    pub fn add(&mut self, _now_ms: i64, _usd: f64) {}

    /// What the last day has cost, to `now_ms`.
    #[must_use]
    pub fn spent(&self, _now_ms: i64) -> f64 {
        0.0
    }
}

#[cfg(test)]
mod tests;
