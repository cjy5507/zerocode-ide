//! What the window does about a judgment (t-26583): from a judgment and what has
//! already been done for the attempt, which acts come next and in which order.
//!
//! The beat judges every worker every second, and a worker that has gone wrong
//! stays wrong for minutes. The judgment is therefore not the unit of action —
//! the CHANGE in it is: a snapshot once per [`crate::continue_gate::CHECKPOINT_MIN_GAP_MS`],
//! a word to the coordinator once per rising level, a stop once per
//! [`STOP_RETRY_MS`]. What was already done is the [`Standing`] of the attempt;
//! it is the whole state, a few numbers, and nothing here touches a pane, a git
//! tree or a ledger — the window runs the acts this answers.

use super::{CHECKPOINT_MIN_GAP_MS, Judgement, Mode, Verdict};

/// How long a stop is given to land before it is asked again: thirty seconds —
/// a `worker-stop` verb walks the ledger and closes a pane, which takes a
/// moment, and a beat that asked again each second would stack requests on the
/// one it is waiting for.
pub const STOP_RETRY_MS: i64 = 30_000;

/// How long an attempt must stay calm — nothing worse than a checkpoint — before
/// a second time it goes wrong is told to the coordinator again: ten minutes. A
/// worker that flaps between a loop and its way out is one episode, not a
/// notice a minute.
pub const REARM_MS: i64 = 10 * 60 * 1_000;

/// How old the last snapshot may be for a stop to go without a fresh one: ten
/// seconds. A stop is the one act that cannot be taken back, so what the
/// worker's tree held a moment before it is saved first.
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
pub struct Standing {
    /// The highest level the coordinator was told since the attempt last was
    /// calm.
    told: Verdict,
    /// Since when nothing worse than a checkpoint was said.
    calm_since_ms: Option<i64>,
    /// When the last snapshot was asked for.
    snapshot_ms: Option<i64>,
    /// When the last stop was asked for.
    stop_ms: Option<i64>,
}

impl Standing {
    /// The acts this judgment calls for, given what was already done, and the
    /// standing after them — as though they all were done.
    pub fn plan(&mut self, judgement: &Judgement, mode: Mode, now_ms: i64) -> Vec<Act> {
        let mut acts = Vec::new();
        if mode == Mode::Off {
            return acts;
        }
        let verdict = judgement.verdict;
        self.rearm(verdict, now_ms);
        let stopping = mode == Mode::Stop
            && verdict == Verdict::Stop
            && self
                .stop_ms
                .is_none_or(|at_ms| now_ms.saturating_sub(at_ms) >= STOP_RETRY_MS);
        let gap = if stopping {
            STOP_SNAPSHOT_MAX_AGE_MS
        } else {
            CHECKPOINT_MIN_GAP_MS
        };
        if verdict >= Verdict::Checkpoint
            && self
                .snapshot_ms
                .is_none_or(|at_ms| now_ms.saturating_sub(at_ms) >= gap)
        {
            self.snapshot_ms = Some(now_ms);
            acts.push(Act::Snapshot);
        }
        if verdict >= Verdict::Pause && verdict > self.told {
            self.told = verdict;
            acts.push(Act::Tell(verdict));
        }
        if stopping {
            self.stop_ms = Some(now_ms);
            acts.push(Act::Stop);
        }
        acts
    }

    /// A calm attempt for long enough is told about afresh the next time it
    /// goes wrong.
    fn rearm(&mut self, verdict: Verdict, now_ms: i64) {
        if verdict > Verdict::Checkpoint {
            self.calm_since_ms = None;
            return;
        }
        let since = *self.calm_since_ms.get_or_insert(now_ms);
        if now_ms.saturating_sub(since) >= REARM_MS {
            self.told = Verdict::Continue;
        }
    }
}

#[cfg(test)]
mod tests;
