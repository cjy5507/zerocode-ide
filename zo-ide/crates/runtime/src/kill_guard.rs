//! zo's side of the kill guard (t-80732): whether a bash command may send the
//! signals it would send.
//!
//! Twice on 10-28 an example command inside a quoted instruction ran as a
//! command, and a `pkill -f` ended the window, its panes and a build runner.
//! The rules — which calls a command makes, and whose process each target is
//! — are core's ([`zerocode_core::shell_rule::kill`], t-80602), the same ones
//! the window's pre-tool hook reads. zo only names itself as the caller and,
//! from the pane's environment, the window it runs in as kept
//! ([`zerocode_core::shell_rule::kill::ProtectedRoot::from_window_env`]).

use zerocode_core::shell_rule::kill::{self, KillVerdict, ProcessTable};

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
/// pid, its environment and the process table, read only when a call needs
/// it. Not read yet: the red half of t-80732.
pub(crate) fn judge_with(
    _command: &str,
    _me: u32,
    _env: impl Fn(&str) -> Option<String>,
    _read_table: impl FnOnce() -> ProcessTable,
) -> Option<KillVerdict> {
    None
}

/// The refusal a [`KillVerdict::Deny`] becomes, in every mode.
#[must_use]
pub fn refusal(reason: &str) -> String {
    reason.to_string()
}

/// The refusal a [`KillVerdict::Ask`] becomes when no person said yes.
#[must_use]
pub fn unanswered(reason: &str) -> String {
    reason.to_string()
}

#[cfg(test)]
mod tests;
