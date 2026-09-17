//! The one unattended driver both frontends run.
//!
//! A `/goal`, `/loop` or cron turn is dispatched by its controller
//! ([`PlainSession::dispatch_goal_turn`] / [`PlainSession::dispatch_loop_turn`]
//! / [`PlainSession::dispatch_cron_turn`]), run as an ordinary turn under the
//! autonomous permission clamp, receipted ([`TurnReceipt`]) and recorded —
//! and the driver goes again until nothing is due. A `ScheduleWakeup` is a
//! loop turn: the session folds the tool's records into its wakeup loop
//! before dispatching. That sequence is the same in the TUI and in the
//! append-only frontend. What differs is how a turn is run and how its
//! outcome is shown, and that is all `AutonomyFrontend` asks of a frontend.

use std::time::{Duration, Instant};

use runtime::message_stream::SystemLevel;

use super::crons;
use super::loops::LoopTurn;
use super::runner::{elapsed_millis, TurnReceipt};
use super::scheduler::now_unix_ms;
use super::wakeup;
use crate::ide::reporter::HookReporter;
use crate::session::plain_session::PlainSession;

/// What one turn came back with, in the frontend's own terms.
#[derive(Debug, Default)]
pub struct TurnOutcome {
    pub summary: Option<runtime::TurnSummary>,
    pub error: Option<String>,
    pub permission_blocked: bool,
    pub question_blocked: bool,
    pub cancelled: bool,
}

/// How a `/loop` turn ended, for the frontend to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopTurnEnd<'a> {
    Recorded {
        /// The turn's own `loop_schedule` said nothing changed.
        noop: bool,
        quiet_streak: u32,
        notice: &'a str,
    },
    /// Not recorded: the frontend left mid-turn, or the loop state could
    /// not be saved (the driver has already said so).
    Dropped,
}

/// Why the driver returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drive {
    /// Nothing is due — back to waiting for input or the next wakeup.
    Idle,
    /// The frontend is on its way out (EOF, `/exit`, session gone).
    Leaving,
}

/// What a frontend lends the driver. `pub(crate)`: an `async fn` in a public
/// trait would want auto-trait bounds neither frontend needs.
pub(crate) trait AutonomyFrontend {
    fn session(&mut self) -> Option<&mut PlainSession>;
    /// Already leaving (the TUI's exit reason is set).
    fn leaving(&self) -> bool;
    fn note(&mut self, level: SystemLevel, text: &str);
    fn reporter(&self) -> Option<&HookReporter>;
    /// Run `prompt` as one autonomous turn; `None` when the frontend is leaving.
    async fn run_turn(&mut self, prompt: &str, allow_writes: bool) -> Option<TurnOutcome>;
    /// The goal/loop status changed — a turn was dispatched or recorded.
    fn status_changed(&mut self);
    /// A loop turn is about to run; `dynamic_iteration` is a self-paced
    /// iteration whose quiet outcome the TUI folds into one row.
    fn loop_turn_started(&mut self, turn: &LoopTurn, dynamic_iteration: bool);
    fn loop_turn_ended(&mut self, turn: &LoopTurn, dynamic_iteration: bool, end: LoopTurnEnd<'_>);
}

/// Drive every due `/goal` turn, then every due `/loop` (and wakeup) turn,
/// then every due cron turn — and go around again after a cron turn, since
/// the model may have scheduled a wakeup in it that only the loop leg sweeps.
pub(crate) async fn drive<F: AutonomyFrontend>(front: &mut F) -> Drive {
    loop {
        if front.session().as_deref().is_some_and(PlainSession::goal_running)
            && drive_goal(front).await == Drive::Leaving
        {
            return Drive::Leaving;
        }
        if drive_loops(front).await == Drive::Leaving {
            return Drive::Leaving;
        }
        match drive_crons(front).await {
            CronPass::Leaving => return Drive::Leaving,
            CronPass::Ran => {}
            CronPass::Idle => return Drive::Idle,
        }
    }
}

/// How the cron leg ended: nothing was due, at least one turn ran, or the
/// frontend is leaving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CronPass {
    Idle,
    Ran,
    Leaving,
}

/// Sleep until `at_unix_ms`; never, when nothing is scheduled.
pub(crate) async fn wait_for_wakeup(at_unix_ms: Option<u64>) {
    let Some(at_unix_ms) = at_unix_ms else {
        return std::future::pending().await;
    };
    tokio::time::sleep(Duration::from_millis(at_unix_ms.saturating_sub(now_unix_ms()))).await;
}

/// The hook event a goal notice announces, if any.
#[must_use]
pub fn goal_event(notice: &str) -> Option<&'static str> {
    if notice.contains("completed") {
        Some("GoalAchieved")
    } else if notice.contains("(paused)") {
        Some("GoalPaused")
    } else {
        None
    }
}

/// Explicitly armed `/goal start` turns until a gate succeeds or one of the
/// limits / permission / no-change guards pauses it. There is no idle timer:
/// when the goal is off this is never entered.
async fn drive_goal<F: AutonomyFrontend>(front: &mut F) -> Drive {
    loop {
        if front.leaving() {
            return Drive::Leaving;
        }
        let Some(session) = front.session() else {
            return Drive::Leaving;
        };
        let turn = match session.dispatch_goal_turn() {
            Ok(Some(turn)) => turn,
            Ok(None) => {
                front.status_changed();
                return Drive::Idle;
            }
            Err(error) => {
                front.note(SystemLevel::Error, &format!("goal state could not be saved: {error}"));
                return Drive::Idle;
            }
        };
        let expected_gate = session.expected_goal_gate(&turn).map(str::to_string);
        let started = TurnStart::capture(session);
        front.status_changed();
        front.note(SystemLevel::Info, &format!("autonomous → {}", turn.kind.label()));
        let Some(outcome) = front.run_turn(&turn.prompt, turn.allow_writes).await else {
            return Drive::Leaving;
        };
        let Some(session) = front.session() else {
            return Drive::Leaving;
        };
        let report = started.receipt(session, expected_gate.as_deref(), outcome).into_goal_report();
        let session_id = session.handle.id.clone();
        match session.record_goal_turn(&turn, report) {
            Ok(notice) => {
                front.note(SystemLevel::Info, &notice);
                if let (Some(reporter), Some(event)) = (front.reporter(), goal_event(&notice)) {
                    reporter.autonomy_goal(event, &session_id, &notice);
                }
                front.status_changed();
            }
            Err(error) => {
                front.note(SystemLevel::Error, &format!("goal state could not be saved: {error}"));
                return Drive::Idle;
            }
        }
    }
}

/// Print what the scheduler did while nobody was looking — the notices a
/// dispatch or a receipt queued on the session. `true` when there were any:
/// each one is a loop armed, re-armed or stopped, so the status changed too.
fn flush_notices<F: AutonomyFrontend>(front: &mut F) -> bool {
    let notices = front
        .session()
        .map(PlainSession::take_autonomy_notices)
        .unwrap_or_default();
    let any = !notices.is_empty();
    for notice in notices {
        front.note(SystemLevel::Info, &notice);
    }
    any
}

async fn drive_loops<F: AutonomyFrontend>(front: &mut F) -> Drive {
    loop {
        if front.leaving() {
            return Drive::Leaving;
        }
        let Some(session) = front.session() else {
            return Drive::Leaving;
        };
        let dispatched = session.dispatch_loop_turn();
        if flush_notices(front) {
            front.status_changed();
        }
        let Some(session) = front.session() else {
            return Drive::Leaving;
        };
        let turn = match dispatched {
            Ok(Some(turn)) => turn,
            Ok(None) => return Drive::Idle,
            Err(error) => {
                front.note(SystemLevel::Error, &format!("loop state could not be saved: {error}"));
                return Drive::Idle;
            }
        };
        let expected_gate = session.expected_loop_gate(&turn).map(str::to_string);
        let started = TurnStart::capture(session);
        let dynamic_iteration = turn.dynamic && PlainSession::loop_turn_is_iteration(&turn);
        // A self-paced iteration may call `loop_schedule`; the scope collects it.
        let _schedule_scope = dynamic_iteration.then(wakeup::begin_scope);
        front.loop_turn_started(&turn, dynamic_iteration);
        front.note(
            SystemLevel::Info,
            &format!("autonomous → {} · {}", turn.id, turn.kind.label()),
        );
        let Some(outcome) = front.run_turn(&turn.prompt, turn.allow_writes).await else {
            front.loop_turn_ended(&turn, dynamic_iteration, LoopTurnEnd::Dropped);
            return Drive::Leaving;
        };
        let Some(session) = front.session() else {
            front.loop_turn_ended(&turn, dynamic_iteration, LoopTurnEnd::Dropped);
            return Drive::Leaving;
        };
        let receipt = started.receipt(session, expected_gate.as_deref(), outcome);
        let noop = receipt
            .loop_schedule
            .as_ref()
            .is_some_and(|schedule| schedule.noop);
        let session_id = session.handle.id.clone();
        match session.record_loop_turn(&turn, receipt) {
            Ok(notice) => {
                let quiet_streak = session.loop_quiet_streak(&turn.id).unwrap_or_default();
                front.loop_turn_ended(
                    &turn,
                    dynamic_iteration,
                    LoopTurnEnd::Recorded { noop, quiet_streak, notice: &notice },
                );
                if PlainSession::loop_turn_is_iteration(&turn) {
                    if let Some(reporter) = front.reporter() {
                        reporter.loop_iteration(&session_id, &turn.id, noop, &notice);
                    }
                }
                let _ = flush_notices(front);
                front.status_changed();
            }
            Err(error) => {
                front.loop_turn_ended(&turn, dynamic_iteration, LoopTurnEnd::Dropped);
                front.note(SystemLevel::Error, &format!("loop state could not be saved: {error}"));
                return Drive::Idle;
            }
        }
    }
}

/// Due cron records, one turn each, under the session's own permission mode
/// (registering a cron already took full access). The run was recorded in
/// the registry at dispatch, so a blocked or failed turn is said, not retried
/// for the same minute.
async fn drive_crons<F: AutonomyFrontend>(front: &mut F) -> CronPass {
    let mut ran = CronPass::Idle;
    loop {
        if front.leaving() {
            return CronPass::Leaving;
        }
        let Some(session) = front.session() else {
            return CronPass::Leaving;
        };
        let turn = match session.dispatch_cron_turn() {
            Ok(Some(turn)) => turn,
            Ok(None) => return ran,
            Err(refusal) => {
                front.note(SystemLevel::Warn, &refusal);
                return ran;
            }
        };
        let started = TurnStart::capture(session);
        front.status_changed();
        front.note(
            SystemLevel::Info,
            &format!("autonomous → {} {}", crons::TURN_LABEL, turn.id),
        );
        let Some(outcome) = front.run_turn(&turn.prompt, true).await else {
            return CronPass::Leaving;
        };
        let Some(session) = front.session() else {
            return CronPass::Leaving;
        };
        ran = CronPass::Ran;
        let receipt = started.receipt(session, None, outcome);
        match session.record_cron_turn(&turn, &receipt) {
            Ok(notice) => {
                front.note(SystemLevel::Info, &notice);
                front.status_changed();
            }
            Err(error) => {
                front.note(SystemLevel::Error, &format!("cron budget could not be saved: {error}"));
                return ran;
            }
        }
    }
}

/// The two things a receipt needs from before the turn ran.
struct TurnStart {
    workspace_before: String,
    at: Instant,
}

impl TurnStart {
    fn capture(session: &PlainSession) -> Self {
        Self {
            workspace_before: tokio::task::block_in_place(|| {
                TurnReceipt::capture_workspace(&session.cwd)
            }),
            at: Instant::now(),
        }
    }

    fn receipt(
        self,
        session: &PlainSession,
        expected_gate: Option<&str>,
        outcome: TurnOutcome,
    ) -> TurnReceipt {
        // The attempt the turn that just ended was spent on — still the
        // runtime's current one, since the next turn has not begun.
        let attempt = session
            .runtime
            .try_runtime()
            .map(|runtime| runtime.attempt().to_string())
            .unwrap_or_default();
        TurnReceipt::from_turn_outcome(
            &session.cwd,
            self.workspace_before,
            expected_gate,
            elapsed_millis(self.at),
            outcome,
            &attempt,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::goal_event;

    #[test]
    fn a_goal_notice_names_its_hook_event() {
        assert_eq!(goal_event("goal completed: gate passed"), Some("GoalAchieved"));
        assert_eq!(goal_event("approval needed (paused)"), Some("GoalPaused"));
        assert_eq!(goal_event("action 2/8 recorded"), None);
    }
}
