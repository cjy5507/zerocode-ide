//! What the whole day has cost, as far as the window watched it (t-26583).
//!
//! The day's budget is a person's — "all my agents, together, this much a day"
//! — and no one worker knows it. The window adds every model call it reads into
//! one rolling day, a bucket a minute, and keeps it across restarts; a restart
//! that forgot the morning would let the afternoon spend it again.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::civil::MS_PER_DAY;

/// One bucket's span: a minute. A day is at most 1,440 of them and the whole
/// ledger a few dozen kilobytes, however many calls the day held.
pub const BUCKET_MS: i64 = 60_000;

/// A rolling day of spend, in API-equivalent dollars.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DaySpend {
    /// `(minute, dollars)`, oldest first; a minute is `now_ms / BUCKET_MS`.
    buckets: VecDeque<(i64, f64)>,
}

impl DaySpend {
    /// Adds what a model call cost. A clock that stepped back files the cost in
    /// the newest bucket and never reorders the ledger; a cost that is not a
    /// number of dollars is nothing.
    pub fn add(&mut self, now_ms: i64, usd: f64) {
        if !usd.is_finite() || usd < 0.0 {
            return;
        }
        self.prune(now_ms);
        let minute = now_ms.div_euclid(BUCKET_MS);
        match self.buckets.back_mut() {
            Some((newest, held)) if *newest >= minute => *held += usd,
            _ => self.buckets.push_back((minute, usd)),
        }
    }

    /// What the last day has cost, to `now_ms`.
    #[must_use]
    pub fn spent(&self, now_ms: i64) -> f64 {
        self.buckets
            .iter()
            .filter(|(minute, _)| Self::within(*minute, now_ms))
            .map(|(_, usd)| usd)
            .sum()
    }

    fn prune(&mut self, now_ms: i64) {
        while self
            .buckets
            .front()
            .is_some_and(|(minute, _)| !Self::within(*minute, now_ms))
        {
            self.buckets.pop_front();
        }
    }

    /// Whether the minute's bucket ends inside the day before `now_ms`.
    fn within(minute: i64, now_ms: i64) -> bool {
        minute.saturating_add(1).saturating_mul(BUCKET_MS) > now_ms.saturating_sub(MS_PER_DAY)
    }
}

#[cfg(test)]
mod tests;
