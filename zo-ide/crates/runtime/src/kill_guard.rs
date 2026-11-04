//! zo's side of the kill guard (t-80732): whether a bash command — or a
//! `PowerShell` one (t-84973) — may send the signals it would send.
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

use std::cell::RefCell;
use std::time::{Duration, Instant};

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
/// does not read is judged with `judge_with` in this crate's tests.
const fn table_read_time(test_build: bool) -> std::time::Duration {
    if test_build {
        std::time::Duration::from_secs(60)
    } else {
        kill::TABLE_READ_WALL
    }
}

/// [`table_read_time`] for this build.
const TABLE_READ_TIME: std::time::Duration = table_read_time(cfg!(test));

/// The shell a tool runs its command in, for what the guard reads of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// The bash tool's.
    Bash,
    /// The `PowerShell` tool's (t-84973): besides the external `kill`, `pkill`
    /// and `killall` bash would run, `Stop-Process` and its aliases,
    /// `taskkill`, and a process object's `.Kill()` (the `powershell` module).
    PowerShell,
}

impl Shell {
    /// The calls `command` makes that would end processes.
    fn kill_calls(self, command: &str) -> Vec<KillCall> {
        let mut calls = kill::kill_calls(command);
        if self == Self::PowerShell {
            calls.extend(powershell::kill_calls(command));
        }
        calls
    }
}

/// The judgment for bash `command` run by this process; `None` when it
/// makes no kill call, and then the process table is not read.
#[must_use]
pub fn judge(command: &str) -> Option<KillVerdict> {
    judge_in(Shell::Bash, command)
}

/// [`judge`] for `command` as `shell` runs it.
#[must_use]
pub fn judge_in(shell: Shell, command: &str) -> Option<KillVerdict> {
    judge_calls_with(
        &shell.kill_calls(command),
        std::process::id(),
        |name| std::env::var(name).ok(),
        || ProcessTable::read(TABLE_READ_TIME),
    )
}

/// [`judge_calls_with`] for bash `command`: the tests' way to judge one with
/// a table handed in.
#[cfg(test)]
pub(crate) fn judge_with(
    command: &str,
    me: u32,
    env: impl Fn(&str) -> Option<String>,
    read_table: impl FnOnce() -> ProcessTable,
) -> Option<KillVerdict> {
    judge_calls_with(&kill::kill_calls(command), me, env, read_table)
}

/// The judgment of the kill calls a command makes, with what it reads from
/// the machine handed in: the caller's pid, its environment and the process
/// table, read once and only when a call needs it.
pub(crate) fn judge_calls_with(
    calls: &[KillCall],
    me: u32,
    env: impl Fn(&str) -> Option<String>,
    read_table: impl FnOnce() -> ProcessTable,
) -> Option<KillVerdict> {
    if calls.is_empty() {
        return None;
    }
    let table = if calls.iter().any(KillCall::needs_table) {
        read_table()
    } else {
        ProcessTable::new(Vec::new())
    };
    let kept = ProtectedRoot::from_window_env(env, &table, me);
    Some(kill::judge_kill(calls, &table, Some(me), &kept))
}

/* ---- one command, one reading of the processes (t-84911) ------------------- */

/// How long a judgment made before a call's dispatch stands for it there
/// ([`for_one_command`]). The streaming turn judges every call of a batch
/// before it runs the first, so a call's judgment can wait while a tool
/// before it runs or a person is asked; one older than this is read again
/// where the command runs, as it was before t-84911.
const HANDED_JUDGMENT_LIFETIME: Duration = Duration::from_secs(2);

/// What the guard answered for one command, and when the processes were
/// read for it (t-84911). Only [`Judgment::of`] makes one, from the
/// processes themselves: it is handed from layer to layer inside this
/// process ([`for_one_command`]), never through the tool's input, which a
/// model writes and could fill with an answer of its own.
#[derive(Debug, Clone)]
pub struct Judgment {
    shell: Shell,
    command: String,
    verdict: Option<KillVerdict>,
    read_at: Instant,
}

impl Judgment {
    /// Judge `command` as `shell` runs it, now ([`judge_in`]).
    #[must_use]
    pub fn of(shell: Shell, command: &str) -> Self {
        let verdict = judge_in(shell, command);
        Self {
            shell,
            command: command.to_string(),
            verdict,
            read_at: Instant::now(),
        }
    }

    /// The answer: `None` when the command makes no kill call.
    #[must_use]
    pub fn verdict(&self) -> Option<&KillVerdict> {
        self.verdict.as_ref()
    }

    /// Whether it may still stand for its command where the command runs.
    fn fresh(&self) -> bool {
        self.read_at.elapsed() <= HANDED_JUDGMENT_LIFETIME
    }
}

/// Whether judging `command` as `shell` runs it reads the process table: it
/// makes a kill call that names a pid, a group, a parent or a pattern. The
/// streaming turn judges only such a command off its task first.
#[must_use]
pub fn reads_the_table(shell: Shell, command: &str) -> bool {
    shell.kill_calls(command).iter().any(KillCall::needs_table)
}

/// Where this thread stands with [`for_one_command`].
enum OneCommand {
    /// No command's way is open on this thread: each judgment reads anew.
    Outside,
    /// A command's way is open, with the guard's answer once there is one.
    Inside(Option<Judgment>),
}

thread_local! {
    /// The command this thread is taking through the permission layers to
    /// where it runs ([`OneCommand`]).
    static ONE_COMMAND: RefCell<OneCommand> = const { RefCell::new(OneCommand::Outside) };
}

/// Run `f` as one command's way from the permission layers to the shell
/// that runs it, on this thread: the guard reads the processes for it at
/// most once inside `f` — the policy, `check_bash` and the bash chokepoint
/// ask in turn, and each read them again before (t-84911). `handed` is a
/// judgment made for it already, off the streaming turn's task; one older
/// than `HANDED_JUDGMENT_LIFETIME` (2 s) is not used. Inside another
/// `for_one_command` with nothing handed, `f` keeps the one it is in.
pub fn for_one_command<T>(handed: Option<Judgment>, f: impl FnOnce() -> T) -> T {
    struct Restore(OneCommand);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = std::mem::replace(&mut self.0, OneCommand::Outside);
            ONE_COMMAND.with(|slot| {
                slot.replace(previous);
            });
        }
    }

    let handed = handed.filter(Judgment::fresh);
    if handed.is_none() && ONE_COMMAND.with(|slot| matches!(*slot.borrow(), OneCommand::Inside(_))) {
        return f();
    }
    let previous = ONE_COMMAND.with(|slot| slot.replace(OneCommand::Inside(handed)));
    let _restore = Restore(previous);
    f()
}

/// The guard's answer for `command` where this thread asks: inside
/// [`for_one_command`], the one already read for it — or read now and kept
/// for the rest of its way; outside, read now. Every layer of zo that judges
/// a command asks here.
pub(crate) fn judged(shell: Shell, command: &str) -> Option<KillVerdict> {
    let kept = ONE_COMMAND.with(|slot| match &*slot.borrow() {
        OneCommand::Inside(Some(judgment)) if judgment.shell == shell && judgment.command == command => {
            Some(judgment.verdict.clone())
        }
        _ => None,
    });
    if let Some(verdict) = kept {
        return verdict;
    }
    let judgment = Judgment::of(shell, command);
    let verdict = judgment.verdict.clone();
    ONE_COMMAND.with(|slot| {
        if let OneCommand::Inside(kept) = &mut *slot.borrow_mut() {
            *kept = Some(judgment);
        }
    });
    verdict
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

/// What the shell's chokepoint — [`crate::execute_bash`], and the
/// `PowerShell` tool's run — refuses of `command` before it runs, in every
/// mode and on every path: a signal the guard refuses, and in a pane the
/// ledger summoned as a worker, which nobody watches, a question too, since
/// no path can have asked it. A question elsewhere was the permission
/// layer's to ask, so it goes ahead.
#[must_use]
pub fn refused_where_it_runs(shell: Shell, command: &str) -> Option<String> {
    match judged(shell, command) {
        Some(KillVerdict::Deny { reason }) => Some(refusal(&reason)),
        Some(KillVerdict::Ask { reason }) if in_a_summoned_worker_pane() => Some(unanswered(&reason)),
        _ => None,
    }
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
    match refused_where_nobody_can_ask(Shell::Bash, command) {
        Some(refused) => Err(refused),
        None => Ok(LetThrough {
            command: command.to_string(),
        }),
    }
}

/// The refusal [`judge_where_nobody_can_ask`] gives `command` as `shell`
/// runs it, in the same words for both: also the `PowerShell` tool of a
/// sub-agent or a Workflow helper (t-84973).
#[must_use]
pub fn refused_where_nobody_can_ask(shell: Shell, command: &str) -> Option<String> {
    match judged(shell, command) {
        Some(KillVerdict::Deny { reason }) => Some(refusal(&reason)),
        Some(KillVerdict::Ask { reason }) => Some(format!("{} {}", unanswered(&reason), way_out())),
        Some(KillVerdict::Allow) | None => None,
    }
}

mod powershell;

#[cfg(test)]
mod tests;
