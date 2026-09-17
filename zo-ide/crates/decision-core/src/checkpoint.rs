//! Unattended-run checkpoints: report progress, then pause, instead of
//! grinding silently.
//!
//! Every existing long-horizon guard is a *stop* device (turn caps, budgets,
//! stall/block/convergence detectors). None of them is a *reporting* device:
//! an autonomous goal that keeps legitimately advancing can run for hours
//! with zero user contact — the observed runaway went 433 messages without a
//! single question to the human. This ledger paces an autonomous run against
//! the user's presence: after a threshold of unacknowledged work (turns,
//! wall-clock, or output tokens — whichever crosses first) it asks the caller
//! to surface a progress digest; after too many unacknowledged digests it
//! asks the caller to pause the run (work preserved, resumable), because a
//! human who has seen several checkpoints and said nothing is not watching.
//!
//! Any user input is an acknowledgement — an actively-supervised session
//! never checkpoints at all. Pure and total (the caller supplies the clock);
//! serializable, mirroring the other per-goal trackers.

use serde::{Deserialize, Serialize};

/// Default goal turns per checkpoint window.
pub const CHECKPOINT_EVERY_TURNS: u32 = 5;
/// Default wall-clock seconds per checkpoint window (30 minutes).
pub const CHECKPOINT_EVERY_WALL_SECS: u64 = 30 * 60;
/// Default output tokens per checkpoint window.
pub const CHECKPOINT_EVERY_OUTPUT_TOKENS: u64 = 300_000;
/// Default unacknowledged checkpoints before the run pauses.
pub const CHECKPOINT_MAX_UNACKED: u32 = 2;

/// Thresholds for one checkpoint window. A zero disables that axis; all axes
/// zero disables checkpointing entirely ([`CheckpointPolicy::enabled`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointPolicy {
    pub every_turns: u32,
    pub every_wall_secs: u64,
    pub every_output_tokens: u64,
    /// Unacknowledged checkpoints allowed before [`CheckpointAction::Pause`]:
    /// the first `max_unacked - 1` crossings report, the `max_unacked`-th
    /// pauses. `0` pauses on the very first crossing.
    pub max_unacked: u32,
}

impl Default for CheckpointPolicy {
    fn default() -> Self {
        Self {
            every_turns: CHECKPOINT_EVERY_TURNS,
            every_wall_secs: CHECKPOINT_EVERY_WALL_SECS,
            every_output_tokens: CHECKPOINT_EVERY_OUTPUT_TOKENS,
            max_unacked: CHECKPOINT_MAX_UNACKED,
        }
    }
}

impl CheckpointPolicy {
    /// False when every axis is zero — checkpointing is off.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.every_turns > 0 || self.every_wall_secs > 0 || self.every_output_tokens > 0
    }
}

/// What the caller should do after folding one turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointAction {
    /// Within the window — keep running.
    None,
    /// A window crossed with acknowledgements to spare: surface a progress
    /// digest to the user and keep running.
    Report,
    /// Too many unacknowledged checkpoints: pause the run (work preserved,
    /// resumable) — the human is evidently not watching.
    Pause,
}

/// Per-run checkpoint state. Serializable like the other per-goal trackers;
/// callers that restore a run into a paused state need not persist it (the
/// resume itself is the acknowledgement).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CheckpointLedger {
    turns_in_window: u32,
    tokens_in_window: u64,
    /// Unix seconds when the current window opened; `0` = opens on the next
    /// observed turn.
    window_opened_unix: u64,
    unacked: u32,
}

impl CheckpointLedger {
    /// Fold one finished turn and decide whether to report, pause, or neither.
    /// `now_unix` is the caller's clock (pure module, no IO).
    pub fn observe_turn(
        &mut self,
        now_unix: u64,
        turn_output_tokens: u64,
        policy: &CheckpointPolicy,
    ) -> CheckpointAction {
        if !policy.enabled() {
            return CheckpointAction::None;
        }
        if self.window_opened_unix == 0 {
            self.window_opened_unix = now_unix;
        }
        self.turns_in_window = self.turns_in_window.saturating_add(1);
        self.tokens_in_window = self.tokens_in_window.saturating_add(turn_output_tokens);

        let crossed = (policy.every_turns > 0 && self.turns_in_window >= policy.every_turns)
            || (policy.every_output_tokens > 0
                && self.tokens_in_window >= policy.every_output_tokens)
            || (policy.every_wall_secs > 0
                && now_unix.saturating_sub(self.window_opened_unix) >= policy.every_wall_secs);
        if !crossed {
            return CheckpointAction::None;
        }

        // Open a fresh window so the next checkpoint needs another full one.
        self.turns_in_window = 0;
        self.tokens_in_window = 0;
        self.window_opened_unix = now_unix;
        self.unacked = self.unacked.saturating_add(1);
        if self.unacked >= policy.max_unacked.max(1) {
            CheckpointAction::Pause
        } else {
            CheckpointAction::Report
        }
    }

    /// The user spoke (any input): reset the window and the unacked count. An
    /// actively-supervised run never checkpoints.
    pub fn acknowledge(&mut self) {
        *self = Self::default();
    }

    /// Unacknowledged checkpoints so far (for the digest's "N before pause").
    #[must_use]
    pub const fn unacked(&self) -> u32 {
        self.unacked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> CheckpointPolicy {
        CheckpointPolicy::default()
    }

    #[test]
    fn no_action_within_the_window() {
        let mut ledger = CheckpointLedger::default();
        for turn in 1..CHECKPOINT_EVERY_TURNS {
            assert_eq!(
                ledger.observe_turn(1_000 + u64::from(turn), 10_000, &policy()),
                CheckpointAction::None,
                "turn {turn}"
            );
        }
    }

    #[test]
    fn turn_threshold_reports_then_pauses() {
        let mut ledger = CheckpointLedger::default();
        // First window: 5 turns → Report (unacked 1 < 2).
        for turn in 1..=CHECKPOINT_EVERY_TURNS {
            let action = ledger.observe_turn(1_000, 0, &policy());
            if turn == CHECKPOINT_EVERY_TURNS {
                assert_eq!(action, CheckpointAction::Report);
            } else {
                assert_eq!(action, CheckpointAction::None);
            }
        }
        // Second unacknowledged window: 5 more turns → Pause.
        for turn in 1..=CHECKPOINT_EVERY_TURNS {
            let action = ledger.observe_turn(1_001, 0, &policy());
            if turn == CHECKPOINT_EVERY_TURNS {
                assert_eq!(action, CheckpointAction::Pause, "second crossing pauses");
            } else {
                assert_eq!(action, CheckpointAction::None);
            }
        }
    }

    #[test]
    fn token_and_wall_axes_cross_independently() {
        // Tokens: one huge turn crosses immediately.
        let mut by_tokens = CheckpointLedger::default();
        assert_eq!(
            by_tokens.observe_turn(1_000, CHECKPOINT_EVERY_OUTPUT_TOKENS, &policy()),
            CheckpointAction::Report
        );
        // Wall clock: two turns 30+ minutes apart cross on the second.
        let mut by_wall = CheckpointLedger::default();
        assert_eq!(
            by_wall.observe_turn(1_000, 0, &policy()),
            CheckpointAction::None
        );
        assert_eq!(
            by_wall.observe_turn(1_000 + CHECKPOINT_EVERY_WALL_SECS, 0, &policy()),
            CheckpointAction::Report
        );
    }

    #[test]
    fn acknowledgement_resets_window_and_unacked_count() {
        let mut ledger = CheckpointLedger::default();
        for _ in 0..CHECKPOINT_EVERY_TURNS {
            ledger.observe_turn(1_000, 0, &policy());
        }
        assert_eq!(ledger.unacked(), 1);
        ledger.acknowledge();
        assert_eq!(ledger.unacked(), 0);
        // After the ack, a full fresh window is needed again — and the next
        // crossing is a Report (not a Pause), because the count restarted.
        for turn in 1..=CHECKPOINT_EVERY_TURNS {
            let action = ledger.observe_turn(2_000, 0, &policy());
            if turn == CHECKPOINT_EVERY_TURNS {
                assert_eq!(action, CheckpointAction::Report);
            } else {
                assert_eq!(action, CheckpointAction::None);
            }
        }
    }

    #[test]
    fn an_acknowledged_session_never_pauses() {
        // Supervised pattern: the user speaks after every checkpoint.
        let mut ledger = CheckpointLedger::default();
        for window in 0..10u64 {
            for turn in 1..=CHECKPOINT_EVERY_TURNS {
                let action = ledger.observe_turn(window, 0, &policy());
                assert_ne!(
                    action,
                    CheckpointAction::Pause,
                    "window {window} turn {turn}: an acknowledged run never pauses"
                );
            }
            ledger.acknowledge();
        }
    }

    #[test]
    fn all_axes_zero_disables_checkpointing() {
        let off = CheckpointPolicy {
            every_turns: 0,
            every_wall_secs: 0,
            every_output_tokens: 0,
            max_unacked: 2,
        };
        let mut ledger = CheckpointLedger::default();
        for _ in 0..100 {
            assert_eq!(
                ledger.observe_turn(1_000, 1_000_000, &off),
                CheckpointAction::None
            );
        }
    }

    #[test]
    fn zero_max_unacked_pauses_on_the_first_crossing() {
        let policy = CheckpointPolicy {
            max_unacked: 0,
            ..CheckpointPolicy::default()
        };
        let mut ledger = CheckpointLedger::default();
        for _ in 1..CHECKPOINT_EVERY_TURNS {
            ledger.observe_turn(1_000, 0, &policy);
        }
        assert_eq!(
            ledger.observe_turn(1_000, 0, &policy),
            CheckpointAction::Pause
        );
    }

    #[test]
    fn ledger_roundtrips_through_serde() {
        let mut ledger = CheckpointLedger::default();
        ledger.observe_turn(1_000, 42, &policy());
        let json = serde_json::to_string(&ledger).expect("serialize");
        let back: CheckpointLedger = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ledger, back);
    }
}
