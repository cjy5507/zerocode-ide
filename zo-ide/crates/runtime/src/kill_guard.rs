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
//! Three places ask, and each does what it can with the answer:
//! [`crate::PermissionPolicy`] puts a question to a person when one can be
//! asked and the mode or the rules would have asked about the call anyway,
//! and refuses it where they would run the call unasked (t-82420);
//! [`crate::permission_enforcer::PermissionEnforcer::check_bash`] has
//! nobody to ask, so a question is a refusal there; and the bash chokepoint
//! ([`crate::execute_bash`]) refuses the window in every mode and on every
//! path, a question having been the permission layer's to ask — save in a
//! pane the ledger summoned as a worker, where no path can have asked it, so
//! a question is a refusal there too.

use zerocode_core::agent_teams::{TEAM_LEADER_VAR, TEAM_PANE_VAR};
use zerocode_core::shell_rule::kill::{self, KillCall, KillVerdict, ProcessTable, ProtectedRoot};

/// Said after every refusal: the guard reads the processes, not the
/// permission mode, so the mode advice a denial otherwise carries would send
/// the model after an escalation that changes nothing.
const NOT_THE_MODE: &str = "Permission audit: the kill guard decides this in every permission mode, so a wider mode changes nothing; do not retry the identical call.";

/// The judgment for `command` run by this process; `None` when it makes no
/// kill call, and then the process table is not read.
#[must_use]
pub fn judge(command: &str) -> Option<KillVerdict> {
    judge_with(
        command,
        std::process::id(),
        |name| std::env::var(name).ok(),
        || ProcessTable::read(kill::TABLE_READ_WALL),
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

/// What a session that runs calls unasked can do instead of a kill that
/// needs a person's yes — the ways out the window's hook gives a pane: a
/// process this session started is its own to end by a pid it names, and one
/// it did not start is not (t-82420).
const WAY_OUT: &str = "Nobody was asked, since this session runs calls without asking. \
To end what this session started, name it so it can be checked before it runs: \
its pid as a number (`echo $!` or `pgrep -f <name>`, then `kill 4242`), \
`kill $(pgrep -f <name>)`, or its job (`kill %1`, `kill $!` in the same command). \
A process this session did not start is the person's to end, or the pane's that started it";

/// The refusal a [`KillVerdict::Ask`] becomes where the mode or the rules
/// would run the call without asking anyone (t-82420): the question is not
/// put, and the refusal says what to do instead.
#[must_use]
pub fn unasked(reason: &str) -> String {
    format!("{} {WAY_OUT}.", unanswered(reason))
}

#[cfg(test)]
mod tests;
