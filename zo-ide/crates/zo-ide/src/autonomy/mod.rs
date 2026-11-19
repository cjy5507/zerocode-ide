//! Shared policy, scheduling, and accounting for unattended goal, loop, cron
//! and wakeup turns — the one idle-time engine both frontends run.

pub mod crons;
pub mod driver;
pub(crate) mod dreaming;
pub mod limits;
pub mod loops;
pub mod runner;
pub mod scheduler;
pub mod wakeup;

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use limits::AutonomyLimits;
use loops::LoopRegistry;
use runner::AutonomyRunner;
use runtime::{PermissionMode, ReadOnlyHold};
use scheduler::Scheduler;

use crate::goal::GoalController;

/// What an unattended turn may write (t-117004): the session's own mode, or
/// read-only for a reason a refused write then names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnWrites {
    /// The session's mode: the run was started with `--allow-writes`, or the
    /// turn is a `ScheduleWakeup` or cron turn.
    Session,
    /// Read-only for this reason — unless the session has Full Access.
    ReadOnly(ReadOnlyHold),
}

impl TurnWrites {
    /// The hold this turn runs under in a session of `mode`. None under a
    /// mode the screen calls Full Access, whatever the run's flags: the
    /// person chose "never ask" and started the run in that session, so
    /// holding it read-only would only have the model ask them for a mode
    /// they already gave.
    #[must_use]
    pub fn hold_in(self, mode: PermissionMode) -> Option<ReadOnlyHold> {
        match self {
            Self::ReadOnly(hold) if !is_full_access(mode) => Some(hold),
            _ => None,
        }
    }
}

/// The modes the screen calls Full Access (`tui::permissions::mode_label`).
const fn is_full_access(mode: PermissionMode) -> bool {
    matches!(mode, PermissionMode::DangerFullAccess | PermissionMode::Allow)
}

/// What a `/goal` or `/loop` start adds, once, when the run will be held
/// read-only in a session that could write (t-117004).
pub const READ_ONLY_RUN_NOTE: &str = "read-only run (add --allow-writes to let it write)";

/// Whether a run started without `--allow-writes` in a session of `mode` is
/// held below a mode that could write: not under Full Access, which it
/// keeps, and not in a read-only session, which holds nothing back.
#[must_use]
pub fn run_is_held_below(mode: PermissionMode) -> bool {
    !is_full_access(mode) && mode != PermissionMode::ReadOnly
}

pub struct Autonomy {
    pub runner: AutonomyRunner,
    pub scheduler: Scheduler,
    pub goal: GoalController,
    pub loops: LoopRegistry,
    pub limits: AutonomyLimits,
    budget_path: PathBuf,
    budget: SessionBudget,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutonomyStatus {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub goal: Option<GoalStatus>,
    #[serde(default)]
    pub loops: Vec<LoopStatus>,
    pub budget: SessionBudgetStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBudgetStatus {
    pub continuations: u32,
    pub max_continuations: u32,
    pub assistant_turns: u32,
    pub max_assistant_turns: u32,
    pub output_tokens: u64,
    pub max_output_tokens: u64,
    pub active_millis: u64,
    pub max_wall_clock_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SessionBudget {
    assistant_turns: u32,
    output_tokens: u64,
    active_millis: u64,
    #[serde(default)]
    blocked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalStatus {
    pub phase: String,
    pub next_at: Option<u64>,
    pub gates_passed: u32,
    pub gates_total: u32,
    pub action_turns: u32,
    pub stalled_turns: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopStatus {
    pub id: String,
    pub phase: String,
    pub trigger: String,
    pub next_at: Option<u64>,
    pub runs: u32,
    pub max_runs: u32,
    pub quiet: u32,
    pub reason: Option<String>,
}

impl Autonomy {
    #[must_use]
    pub fn load(
        session_path: &Path,
        session_goal: Option<&str>,
        resumed: bool,
    ) -> Self {
        let limits = AutonomyLimits::load();
        let budget_path = session_path.with_extension("autonomy.json");
        let budget = std::fs::read(&budget_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            runner: AutonomyRunner,
            scheduler: Scheduler,
            goal: GoalController::load(session_path, session_goal, resumed),
            loops: LoopRegistry::load(session_path, resumed, limits.clone()),
            limits,
            budget_path,
            budget,
        }
    }

    #[must_use]
    pub fn status(&self) -> AutonomyStatus {
        AutonomyStatus {
            goal: self.goal.autonomy_status(),
            loops: self.loops.autonomy_status(),
            budget: self.budget_status(),
        }
    }

    #[must_use]
    pub fn exhaustion(&self) -> Option<String> {
        if let Some(reason) = &self.budget.blocked { return Some(reason.clone()); }
        let continuations = self.budget.assistant_turns.saturating_sub(1);
        if continuations >= self.limits.max_continuations {
            return Some(format!(
                "session continuation limit reached ({continuations}/{})",
                self.limits.max_continuations
            ));
        }
        if self.budget.assistant_turns >= self.limits.max_assistant_turns {
            return Some(format!(
                "session assistant-turn limit reached ({}/{})",
                self.budget.assistant_turns, self.limits.max_assistant_turns
            ));
        }
        if self.budget.output_tokens >= self.limits.max_output_tokens {
            return Some(format!(
                "session output-token limit reached ({}/{})",
                self.budget.output_tokens, self.limits.max_output_tokens
            ));
        }
        if self.budget.active_millis
            >= self.limits.max_wall_clock_secs.saturating_mul(1_000)
        {
            return Some(format!(
                "session active-time limit reached ({}s)",
                self.limits.max_wall_clock_secs
            ));
        }
        None
    }

    /// Unknown scheduled execution cost cannot become a free retry after resume.
    pub(crate) fn block(&mut self, reason: &str) -> io::Result<()> {
        self.budget.blocked = Some(reason.to_string());
        self.persist_budget()
    }

    pub fn record_usage(&mut self, output_tokens: u64, active_millis: u64) -> io::Result<()> {
        self.budget.assistant_turns = self.budget.assistant_turns.saturating_add(1);
        self.budget.output_tokens = self.budget.output_tokens.saturating_add(output_tokens);
        self.budget.active_millis = self.budget.active_millis.saturating_add(active_millis);
        self.persist_budget()
    }

    #[must_use]
    fn budget_status(&self) -> SessionBudgetStatus {
        SessionBudgetStatus {
            continuations: self.budget.assistant_turns.saturating_sub(1),
            max_continuations: self.limits.max_continuations,
            assistant_turns: self.budget.assistant_turns,
            max_assistant_turns: self.limits.max_assistant_turns,
            output_tokens: self.budget.output_tokens,
            max_output_tokens: self.limits.max_output_tokens,
            active_millis: self.budget.active_millis,
            max_wall_clock_secs: self.limits.max_wall_clock_secs,
            blocked: self.budget.blocked.clone(),
        }
    }

    fn persist_budget(&self) -> io::Result<()> {
        let payload = serde_json::to_vec_pretty(&self.budget).map_err(io::Error::other)?;
        if self.budget_path.exists() {
            crate::write_atomic(&self.budget_path, &payload)
        } else {
            core_types::paths::write_private_file(
                &self.budget_path,
                &payload,
                &core_types::paths::ParentDirPolicy::LeaveParent,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Autonomy;

    #[test]
    fn session_budget_combines_goal_and_loop_turn_usage() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let mut autonomy = Autonomy::load(&transcript, None, false);
        autonomy.limits.max_assistant_turns = 2;

        autonomy.record_usage(3, 10).expect("goal usage");
        autonomy.record_usage(4, 20).expect("loop usage");

        assert!(autonomy
            .exhaustion()
            .is_some_and(|reason| reason.contains("assistant-turn")));
        let status = autonomy.status();
        assert_eq!(status.budget.output_tokens, 7);
        assert_eq!(status.budget.active_millis, 30);
    }

    #[test]
    fn session_budget_survives_reload() {
        let temp = tempfile::tempdir().expect("tempdir");
        let transcript = temp.path().join("s.jsonl");
        let mut autonomy = Autonomy::load(&transcript, None, false);
        autonomy.record_usage(9, 11).expect("record");

        let loaded = Autonomy::load(&transcript, None, true);

        assert_eq!(loaded.status().budget.output_tokens, 9);
        assert_eq!(loaded.status().budget.active_millis, 11);
    }
    #[test]
    fn unknown_execution_cost_blocks_automation_after_session_reload() {
        let temp = tempfile::tempdir().unwrap();
        let transcript = temp.path().join("s.jsonl");
        let mut autonomy = Autonomy::load(&transcript, None, false);
        autonomy.block("unknown graph cost").unwrap();
        assert_eq!(autonomy.exhaustion().as_deref(), Some("unknown graph cost"));
        let loaded = Autonomy::load(&transcript, None, true);
        assert_eq!(loaded.status().budget.blocked.as_deref(), Some("unknown graph cost"));
        assert_eq!(loaded.exhaustion().as_deref(), Some("unknown graph cost"));
    }

    /// t-117004: a mode the screen calls Full Access is kept by every
    /// unattended turn, whatever the run's flags; every other mode holds a
    /// read-only turn there — and a start says so only where the session
    /// itself could write.
    #[test]
    fn full_access_is_kept_by_every_unattended_turn() {
        use super::{is_full_access, run_is_held_below, TurnWrites};
        use runtime::{PermissionMode, ReadOnlyHold};

        let all = [
            PermissionMode::ReadOnly,
            PermissionMode::WorkspaceWrite,
            PermissionMode::DangerFullAccess,
            PermissionMode::Prompt,
            PermissionMode::Allow,
        ];
        for mode in all {
            // The rule follows the screen's own word for the mode.
            assert_eq!(
                is_full_access(mode),
                crate::tui::permissions::mode_label(mode) == "Full Access",
                "{mode:?}"
            );
            assert_eq!(TurnWrites::Session.hold_in(mode), None, "{mode:?}");
            for hold in [ReadOnlyHold::RunWithoutAllowWrites, ReadOnlyHold::GoalPlanTurn] {
                assert_eq!(
                    TurnWrites::ReadOnly(hold).hold_in(mode),
                    (!is_full_access(mode)).then_some(hold),
                    "{mode:?}"
                );
            }
        }
        assert!(run_is_held_below(PermissionMode::WorkspaceWrite));
        assert!(run_is_held_below(PermissionMode::Prompt));
        assert!(!run_is_held_below(PermissionMode::ReadOnly));
        assert!(!run_is_held_below(PermissionMode::DangerFullAccess));
        assert!(!run_is_held_below(PermissionMode::Allow));
    }
}
