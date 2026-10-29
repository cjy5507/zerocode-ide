//! zo's side of the kill guard (t-80732): whether a bash command may send the
//! signals it would send.
//!
//! Twice on 10-28 an example command inside a quoted instruction ran as a
//! command, and a `pkill -f` ended the window, its panes and a build runner.
//! The rules — which calls a command makes, and whose process each target is
//! — are core's ([`zerocode_core::shell_rule::kill`], t-80602), the same ones
//! the window's pre-tool hook reads. zo only names itself as the caller and,
//! from the pane's environment, the window it runs in as kept
//! ([`ProtectedRoot::from_window_env`]).
//!
//! Each place that asks does what it can with the answer:
//! [`crate::PermissionPolicy`] puts a question to a person when one can be
//! asked and the mode or the rules would have asked about the call anyway,
//! and refuses it where they would run the call unasked (t-82420);
//! [`crate::permission_enforcer::PermissionEnforcer::check_bash`] and a
//! command file's `!` line have nobody to ask, so a question is a refusal
//! there ([`judge_where_nobody_can_ask`]); and the bash chokepoint
//! ([`crate::execute_bash`]) refuses the window in every mode and on every
//! path, a question having been the permission layer's to ask — save in a
//! pane the ledger summoned as a worker, where no path can have asked it, so
//! a question is a refusal there too.

use zerocode_core::agent_teams::{TEAM_LEADER_VAR, TEAM_PANE_VAR};
use zerocode_core::shell_rule::kill::{
    self, KillCall, KillVerdict, PaneMark, ProcessTable, ProtectedRoot, Refused,
};

/// Said after every refusal: the guard reads the processes, not the
/// permission mode, so the mode advice a denial otherwise carries would send
/// the model after an escalation that changes nothing.
const NOT_THE_MODE: &str = "Permission audit: the kill guard decides this in every permission mode, so a wider mode changes nothing; do not retry the identical call.";

/// How long [`judge`] gives `ps` to list the processes, both values in one
/// place so a test can hold zo's (t-81315). In zo, the guard's
/// [`kill::TABLE_READ_WALL`], past which a kill call is a question. In this
/// crate's tests, a minute: they judge real processes, and their answer must
/// not turn on how fast `ps` is under a release lane's load — it took up to
/// 9.8 s at load 22, and a kill at the window ran (t-83535). A table that
/// does not read is judged with [`judge_with`].
const fn table_read_time(test_build: bool) -> std::time::Duration {
    if test_build {
        std::time::Duration::from_secs(60)
    } else {
        kill::TABLE_READ_WALL
    }
}

/// [`table_read_time`] for this build.
const TABLE_READ_TIME: std::time::Duration = table_read_time(cfg!(test));

/// The judgment for `command` run by this process; `None` when it makes no
/// kill call, and then the process table is not read.
#[must_use]
pub fn judge(command: &str) -> Option<KillVerdict> {
    judge_with(
        command,
        std::process::id(),
        |name| std::env::var(name).ok(),
        || ProcessTable::read(TABLE_READ_TIME),
    )
}

/// [`judge`] with what it reads from the machine handed in: the caller's
/// pid, its environment and the process table, read once and only when a
/// call needs it.
pub(crate) fn judge_with(
    command: &str,
    me: u32,
    env: impl Fn(&str) -> Option<String>,
    read_table: impl FnOnce() -> ProcessTable,
) -> Option<KillVerdict> {
    let calls = kill::kill_calls(command);
    if calls.is_empty() {
        return None;
    }
    let table = if calls.iter().any(KillCall::needs_table) {
        read_table()
    } else {
        ProcessTable::new(Vec::new())
    };
    let kept = ProtectedRoot::from_window_env(env, &table, me);
    Some(kill::judge_kill(&calls, &table, Some(me), &kept))
}

/// Whether this run's questions have nobody to answer them: zo runs in a
/// pane the ledger summoned as a worker, which nobody watches. Its pane
/// is named, and is not its team leader's (a coordinator's pane is its own
/// leader; a pane a person opened names neither). Used only to refuse more.
#[must_use]
pub fn in_a_summoned_worker_pane() -> bool {
    summoned_worker(
        std::env::var(TEAM_PANE_VAR).ok().as_deref(),
        std::env::var(TEAM_LEADER_VAR).ok().as_deref(),
    )
}

/// [`in_a_summoned_worker_pane`] for the pane and leader handed in.
pub(crate) fn summoned_worker(pane: Option<&str>, leader: Option<&str>) -> bool {
    pane.is_some_and(|pane| !pane.is_empty() && leader != Some(pane))
}

/// The refusal a [`KillVerdict::Deny`] becomes, in every mode.
#[must_use]
pub fn refusal(reason: &str) -> String {
    format!("refused to run: {reason}. {NOT_THE_MODE}")
}

/// The refusal a [`KillVerdict::Ask`] becomes when no person said yes —
/// nobody could be asked, or the person said no.
#[must_use]
pub fn unanswered(reason: &str) -> String {
    format!("refused, since no person said yes to it: {reason}. {NOT_THE_MODE}")
}

/// Why a session that runs calls unasked put no question (t-82420).
const NOBODY_ASKED: &str = "Nobody was asked, since this session runs calls without asking.";

/// What this session may still end itself instead of a kill that needs a
/// person's yes, in the words the window's hook gives a pane ([`kill::way_out`],
/// t-82854). Whether a job it set loose is still its own turns on the pane
/// mark it carries: zo is started with the window's pid and its pane's key in
/// its environment, which is what the judgment reads of its own process.
fn way_out() -> String {
    kill::way_out(
        Refused::Session,
        PaneMark::from_env(|name| std::env::var(name).ok()).is_some(),
    )
}

/// The refusal a [`KillVerdict::Ask`] becomes where the mode or the rules
/// would run the call without asking anyone (t-82420): the question is not
/// put, and the refusal says what to do instead.
#[must_use]
pub fn unasked(reason: &str) -> String {
    format!("{} {NOBODY_ASKED} {}", unanswered(reason), way_out())
}

/// A bash command the guard let through where nobody can be asked. Only
/// [`judge_where_nobody_can_ask`] makes one — its field is this module's
/// alone — and the policy decides the rest from it without reading the
/// processes again ([`crate::PermissionPolicy::authorize_let_through`],
/// t-81315).
#[derive(Debug)]
pub struct LetThrough {
    command: String,
}

impl LetThrough {
    /// The command the guard let through.
    pub(crate) fn command(&self) -> &str {
        &self.command
    }
}

/// The guard's answer for `command` where nobody can be asked at all,
/// whatever the mode or the rules — a sub-agent's or a Workflow helper's bash
/// ([`crate::permission_enforcer::PermissionEnforcer::check_bash`]) and a
/// command file's `!` line. A [`KillVerdict::Deny`] is refused, and a
/// [`KillVerdict::Ask`] is refused with what to do instead, not with why
/// nobody was asked (t-82854); the refusal is the `Err`. Anything else is let
/// through.
pub fn judge_where_nobody_can_ask(command: &str) -> Result<LetThrough, String> {
    match judge(command) {
        Some(KillVerdict::Deny { reason }) => Err(refusal(&reason)),
        Some(KillVerdict::Ask { reason }) => Err(format!("{} {}", unanswered(&reason), way_out())),
        Some(KillVerdict::Allow) | None => Ok(LetThrough {
            command: command.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests;
