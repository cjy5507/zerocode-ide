//! What the window does about a judgment (t-26583): from a judgment and what has
//! already been done for the attempt, which acts come next and in which order.

use super::{CHECKPOINT_MIN_GAP_MS, Judgement, Mode, Verdict};

/// How long a stop is given to land before it is asked again: thirty seconds.
pub const STOP_RETRY_MS: i64 = 30_000;

/// How long an attempt must stay calm before a second time it goes wrong is
/// told to the coordinator again: ten minutes.
pub const REARM_MS: i64 = 10 * 60 * 1_000;

/// How old the last snapshot may be for a stop to go without a fresh one: ten
/// seconds.
pub const STOP_SNAPSHOT_MAX_AGE_MS: i64 = 10_000;

/// One thing the window does, in the order it does them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Save the worker's tree as a restore point.
    Snapshot,
    /// Tell the coordinator the worker's judgment has reached this level.
    Tell(Verdict),
    /// End the worker.
    Stop,
}

/// What has been done for one attempt.
#[derive(Clone, Debug, Default)]
pub struct Standing;

impl Standing {
    /// The acts this judgment calls for, given what was already done.
    pub fn plan(&mut self, _judgement: &Judgement, _mode: Mode, _now_ms: i64) -> Vec<Act> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests;
