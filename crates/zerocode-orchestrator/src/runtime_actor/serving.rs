//! What the actor thread is serving now, read without asking it (t-75646).
//!
//! On 2026-10-26 the window stood twice for minutes — 07:30 and 15:21 — and
//! both times its goodbye, which asks this actor, did not answer within its
//! 1.5 s, while the main thread took the person's close at once. Every
//! caller waits on this thread without a deadline, so a request it cannot
//! finish stands the whole orchestration still, and nothing said which. A
//! watcher reads this instead: it never queues behind the actor, and a
//! reader that meets the actor writing simply reads nothing that once.

use std::sync::Mutex;
use std::time::Instant;

use super::RuntimeRequest;

/// The request the actor took last and when, until it answers.
#[derive(Debug, Default)]
pub struct Serving {
    now: Mutex<Option<Served>>,
}

/// One request the actor is serving: its name — never its payload — and the
/// moment it was taken, on the monotonic clock: a wall clock set back an hour
/// would make a standing request look as if it had just begun (t-75646 r2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Served {
    pub verb: &'static str,
    pub since: Instant,
}

impl Serving {
    /// The actor took `verb`; it is served until the returned guard drops.
    pub(super) fn begin(&self, verb: &'static str, since: Instant) -> Begun<'_> {
        *self.now.lock().unwrap_or_else(|held| held.into_inner()) = Some(Served { verb, since });
        Begun { serving: self }
    }

    /// What the actor is serving: `Some(None)` while it waits for mail, and
    /// `None` when the actor was writing at this very moment — the reader
    /// never waits for it.
    #[must_use]
    pub fn peek(&self) -> Option<Option<Served>> {
        self.now.try_lock().ok().map(|held| *held)
    }
}

/// Clears the request on every way out of serving it, a panic's included.
pub(super) struct Begun<'a> {
    serving: &'a Serving,
}

impl Drop for Begun<'_> {
    fn drop(&mut self) {
        *self
            .serving
            .now
            .lock()
            .unwrap_or_else(|held| held.into_inner()) = None;
    }
}

impl RuntimeRequest {
    /// The request's name for a line a person reads: the variant, or a
    /// verb's own name from the verb table — never a word the caller wrote.
    pub(super) fn verb(&self) -> &'static str {
        match self {
            Self::Plan(command) => command
                .argv
                .first()
                .and_then(|said| {
                    zerocode_core::orchestration::VERBS
                        .iter()
                        .find(|(verb, ..)| verb == said)
                })
                .map_or("plan", |(verb, ..)| verb),
            Self::GateSession(_) => "gate_session",
            Self::View => "view",
            Self::TerminalGone { .. } => "terminal_gone",
            Self::PaneTurnEnded { .. } => "pane_turn_ended",
            Self::QuietSweep { .. } => "quiet_sweep",
            Self::IdleSweep { .. } => "idle_sweep",
            Self::QuotaWalls { .. } => "quota_walls",
            Self::LandingWatch { .. } => "landing_watch",
            Self::QuotaLifts { .. } => "quota_lifts",
            Self::ClassifierDeclines { .. } => "classifier_declines",
            Self::ModelDeviations { .. } => "model_deviations",
            Self::StallCauses { .. } => "stall_causes",
            Self::HandoverBegin { .. } => "handover_begin",
            Self::HandoverStep { .. } => "handover_step",
            Self::HandoverSettle { .. } => "handover_settle",
            Self::ResumeBegin { .. } => "resume_begin",
            Self::ResumeSettle { .. } => "resume_settle",
            Self::ReadinessSweep { .. } => "readiness_sweep",
            Self::PanesMissing { .. } => "panes_missing",
            Self::PanesSeen { .. } => "panes_seen",
            Self::CheckoutExamined { .. } => "checkout_examined",
            Self::Observation { .. } => "observation",
            Self::SeatObservation { .. } => "seat_observation",
            Self::RetentionSweep { .. } => "retention_sweep",
            Self::PaneTakenOver { .. } => "pane_taken_over",
            Self::PaneAttention { .. } => "pane_attention",
            Self::DeadlineLook { .. } => "deadline_look",
            Self::WindowExiting { .. } => "window_exiting",
            Self::WorkerPaneResumed { .. } => "worker_pane_resumed",
            Self::ResumedPaneNeverStarted { .. } => "resumed_pane_never_started",
            Self::SleeperExpired { .. } => "sleeper_expired",
            Self::SleeperUnrecoverable { .. } => "sleeper_unrecoverable",
            Self::WorkerExitSeen { .. } => "worker_exit_seen",
            Self::WorkerSessionReported { .. } => "worker_session_reported",
            Self::WorkerSeated { .. } => "worker_seated",
            Self::CoordinatorReturned { .. } => "coordinator_returned",
            Self::CoordinatorFollowed { .. } => "coordinator_followed",
            Self::RunProjectNoted { .. } => "run_project_noted",
            Self::ProjectReach { .. } => "project_reach",
            Self::PrepareWorkerReseat { .. } => "prepare_worker_reseat",
            Self::WorkerReseated { .. } => "worker_reseated",
            Self::FinishSleepingReseat { .. } => "finish_sleeping_reseat",
            Self::WorkerRestedForSwitch { .. } => "worker_rested_for_switch",
            Self::AccountSwitched { .. } => "account_switched",
            Self::GateJudged { .. } => "gate_judged",
            Self::LandCheck { .. } => "land_check",
            Self::LandBatch { .. } => "land_batch",
            Self::PersonSettle { .. } => "person_settle",
            Self::SeatNeverOpened { .. } => "seat_never_opened",
            Self::SeatNeverReady { .. } => "seat_never_ready",
            Self::Federation(_) => "federation",
            Self::PrepareEffect { .. } => "prepare_effect",
            Self::HandoverSplit { .. } => "handover_split",
            Self::SettleEffect { .. } => "settle_effect",
            Self::WindowRestarted { .. } => "window_restarted",
            Self::TeamDissolved { .. } => "team_dissolved",
            Self::LookAgain { .. } => "look_again",
            Self::ServeReceipt { .. } => "serve_receipt",
            Self::HandoverRevoked { .. } => "handover_revoked",
            Self::WorkerTerminalSettled { .. } => "worker_terminal_settled",
            Self::ReleaseUnobserved { .. } => "release_unobserved",
            Self::ReleaseSettled { .. } => "release_settled",
            Self::RememberServed { .. } => "remember_served",
            Self::SettleRecovered { .. } => "settle_recovered",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_served_until_its_guard_drops_and_a_reader_never_waits() {
        let serving = Serving::default();
        assert_eq!(serving.peek(), Some(None));
        let since = Instant::now();
        {
            let _begun = serving.begin("view", since);
            assert_eq!(
                serving.peek(),
                Some(Some(Served {
                    verb: "view",
                    since
                }))
            );
            let writing = serving.now.lock().unwrap();
            assert_eq!(
                serving.peek(),
                None,
                "a reader that meets a writer reads nothing"
            );
            drop(writing);
        }
        assert_eq!(serving.peek(), Some(None));
    }
}
