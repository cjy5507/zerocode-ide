//! `/restart` — persist the session, tear the TUI down, and re-exec the newest
//! build on disk so a redeploy takes effect without losing the conversation.
//!
//! The always-on sidebar badge ("/restart · new build on disk …", see
//! [`crate::tui::stale_binary`]) advertises this action; this module owns the
//! machinery behind it.
//!
//! The flow is split so the parts that *can* be unit-tested are pure and the
//! part that cannot (the `exec` + terminal handoff) is a thin, isolated shim:
//!
//! - [`evaluate_restart_readiness`] is the pure pre-flight gate — it decides,
//!   from whether a turn is running and how many messages are queued, whether a
//!   restart may proceed. No teardown happens until it says `Ready`.
//! - [`RestartPlan`] captures the fully-resolved re-exec (which binary, which
//!   workspace, which session) so the caller can run it *after* the terminal is
//!   restored. [`RestartPlan::resolve`] does the one fallible pre-flight step
//!   (locate the running binary) up front, so a missing `current_exe` is
//!   reported as a command error instead of a failure mid-teardown.
//!
//! ## How the child resumes the session
//!
//! `zo --resume <path>` is a *headless* inspector (it prints a summary and
//! exits), and bare `zo` starts a *fresh* session — neither reopens a session
//! into the interactive TUI. So the re-exec signals the boot path out-of-band
//! with the [`RESTART_RESUME_ENV`] environment variable: `run_repl` reads it and
//! reseeds the fresh session from that transcript before the loop starts. No
//! argv from the original launch is re-passed (an initial `-p` prompt or other
//! flags must not silently re-run); the child is a bare `zo` plus that one
//! env var. The active model and effort ride along in the session's own
//! preference sidecar (written by the pre-exec `persist_session`), so the child
//! restores them from disk rather than from re-passed flags.

use std::path::PathBuf;
use std::process::Command;

/// Environment variable the re-exec sets to ask `run_repl` to resume a specific
/// session transcript into the interactive TUI at boot. Internal handoff only —
/// consumed and cleared once by the boot path.
pub(crate) const RESTART_RESUME_ENV: &str = "ZO_RESTART_RESUME";

/// Outcome of the pure pre-flight gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RestartReadiness {
    /// Safe to persist, tear down, and re-exec.
    Ready,
    /// In-flight work would be disrupted or silently lost; do not restart. The
    /// string is an actionable, already-formatted report body.
    Blocked(String),
}

/// Decide whether `/restart` may proceed.
///
/// A restart cannot be undone once the TUI is torn down, so it refuses while
/// there is live work to protect:
///
/// - a **running turn** would be cut off mid-flight, and
/// - **queued messages** live only in memory (they are never persisted), so a
///   restart would drop them with no way to recover — and because the restart
///   re-execs immediately, a mere on-screen "warning" would be wiped before it
///   could be read. Refusing with an actionable message is the honest choice.
///
/// Both conditions are enumerated together so the user sees every reason at
/// once. This is deliberately stricter than a bare "warn about the queue": it
/// trades a rare, easily-satisfied wait for a guarantee of no silent loss.
pub(crate) fn evaluate_restart_readiness(
    turn_active: bool,
    queued_messages: usize,
) -> RestartReadiness {
    let mut reasons = Vec::new();
    if turn_active {
        reasons.push("a turn is still running".to_string());
    }
    if queued_messages > 0 {
        let plural = if queued_messages == 1 { "" } else { "s" };
        reasons.push(format!(
            "{queued_messages} queued message{plural} would be lost (the queue is never persisted)"
        ));
    }
    if reasons.is_empty() {
        return RestartReadiness::Ready;
    }
    RestartReadiness::Blocked(format!(
        "Restart\n  Not restarted    {}\n  Try again        once the turn finishes and the input queue is empty",
        reasons.join("; ")
    ))
}

/// A fully-resolved re-exec of the running CLI, ready to run once the terminal
/// has been restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestartPlan {
    /// The binary to launch. Re-resolved from `current_exe`'s *path* at boot, so
    /// an in-place redeploy (unlink + create → new inode) is picked up.
    program: PathBuf,
    /// Workspace to launch in — the session's stable cwd, not the live process
    /// cwd, which may have been changed by an `EnterWorktree` mid-session.
    cwd: PathBuf,
    /// Absolute path of the session transcript the child should resume.
    session_path: PathBuf,
}

impl RestartPlan {
    /// Resolve the re-exec, performing the one fallible pre-flight step (locating
    /// the running binary) up front so it can be reported as a command error
    /// before any teardown, never as a mid-restart failure.
    pub(crate) fn resolve(session_path: PathBuf, cwd: PathBuf) -> Result<Self, String> {
        let program = std::env::current_exe()
            .map_err(|error| format!("cannot locate the running zo binary: {error}"))?;
        Ok(Self {
            program,
            cwd,
            session_path,
        })
    }

    /// Build the [`Command`] the re-exec runs: a bare launch of the resolved
    /// binary in the session's workspace, carrying only the resume handoff env
    /// var. Deliberately passes **no** argv — the original launch flags/prompt
    /// must not re-run.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .current_dir(&self.cwd)
            .env(RESTART_RESUME_ENV, &self.session_path);
        command
    }

    /// Replace the current process image with the resolved re-exec.
    ///
    /// On success this **never returns** (the image is replaced); the returned
    /// [`std::io::Error`] is the exec failure. Callers must have restored the
    /// terminal first — the child re-initializes it from a clean slate.
    #[cfg(unix)]
    pub(crate) fn exec(&self) -> std::io::Error {
        use std::os::unix::process::CommandExt as _;
        self.command().exec()
    }

    /// Non-unix fallback: `exec` (image replacement) is unix-only. zo deploys
    /// to unix hosts; this keeps the crate compiling elsewhere with an honest
    /// error instead of a panic.
    #[cfg(not(unix))]
    pub(crate) fn exec(&self) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "restart via exec is only supported on unix",
        )
    }

    /// Manual-recovery hint shown if the `exec` fails: the session is already
    /// persisted, so the user can reopen it by relaunching and running
    /// `/resume`. `--resume` is a headless inspector, so the hint names the
    /// interactive path.
    pub(crate) fn manual_recovery_hint(&self) -> String {
        format!(
            "your session was saved — relaunch zo and run /resume {} to reopen it",
            self.session_path.display()
        )
    }

    #[cfg(test)]
    pub(crate) fn program(&self) -> &std::path::Path {
        &self.program
    }

    #[cfg(test)]
    pub(crate) fn session_path(&self) -> &std::path::Path {
        &self.session_path
    }
}

/// Read (and clear) the [`RESTART_RESUME_ENV`] handoff. Returns the session path
/// the boot path should resume, or `None` for a normal launch.
///
/// One-shot: the variable is removed on read so a subsequent in-process `/new`
/// or manual relaunch inside the same shell environment cannot re-trigger a
/// resume. Fail-safe: an empty value is treated as absent.
pub(crate) fn take_boot_resume_request() -> Option<PathBuf> {
    let value = std::env::var_os(RESTART_RESUME_ENV)?;
    // Remove immediately so the signal is consumed exactly once.
    std::env::remove_var(RESTART_RESUME_ENV);
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_when_idle_and_no_queue() {
        assert_eq!(evaluate_restart_readiness(false, 0), RestartReadiness::Ready);
    }

    #[test]
    fn blocked_while_a_turn_is_running() {
        let RestartReadiness::Blocked(message) = evaluate_restart_readiness(true, 0) else {
            panic!("a running turn must block the restart");
        };
        assert!(message.contains("a turn is still running"), "{message}");
        // Actionable: names when to retry.
        assert!(message.contains("Try again"), "{message}");
    }

    #[test]
    fn blocked_when_messages_are_queued_singular_and_plural() {
        let one = match evaluate_restart_readiness(false, 1) {
            RestartReadiness::Blocked(message) => message,
            RestartReadiness::Ready => panic!("a queued message must block"),
        };
        assert!(one.contains("1 queued message would be lost"), "{one}");
        // Singular: no trailing plural "s" on "message".
        assert!(!one.contains("1 queued messages"), "{one}");

        let many = match evaluate_restart_readiness(false, 3) {
            RestartReadiness::Blocked(message) => message,
            RestartReadiness::Ready => panic!("queued messages must block"),
        };
        assert!(many.contains("3 queued messages would be lost"), "{many}");
    }

    #[test]
    fn blocked_message_enumerates_every_reason_together() {
        let RestartReadiness::Blocked(message) = evaluate_restart_readiness(true, 2) else {
            panic!("turn + queue must block");
        };
        assert!(message.contains("a turn is still running"), "{message}");
        assert!(message.contains("2 queued messages would be lost"), "{message}");
    }

    #[test]
    fn plan_resolve_captures_the_session_and_workspace() {
        let session = PathBuf::from("/home/u/.zo/projects/p/sessions/abc.jsonl");
        let cwd = PathBuf::from("/work/repo");
        let plan = RestartPlan::resolve(session.clone(), cwd)
            .expect("current_exe resolves in the test harness");
        assert_eq!(plan.session_path(), session.as_path());
        // The binary is the test runner here, but it must be a resolved absolute
        // path (current_exe always yields one), never empty.
        assert!(plan.program().is_absolute(), "{:?}", plan.program());
    }

    #[test]
    fn plan_command_carries_only_the_resume_env_and_no_argv() {
        let session = PathBuf::from("/sessions/abc.jsonl");
        let cwd = PathBuf::from("/work/repo");
        let plan = RestartPlan {
            program: PathBuf::from("/opt/homebrew/bin/zo"),
            cwd: cwd.clone(),
            session_path: session,
        };
        let command = plan.command();

        // No argv is re-passed: the original launch's prompt/flags must not re-run.
        assert_eq!(command.get_args().count(), 0);

        // The launch happens in the stable workspace, not the live process cwd.
        assert_eq!(command.get_current_dir(), Some(cwd.as_path()));

        // The one handoff: the resume env var pointing at this session.
        let resume = command
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new(RESTART_RESUME_ENV))
            .map(|(_, value)| value);
        assert_eq!(
            resume,
            Some(Some(std::ffi::OsStr::new("/sessions/abc.jsonl"))),
            "the resume env var must name the session transcript"
        );
    }

    #[test]
    fn boot_resume_request_round_trips_is_one_shot_and_ignores_empty() {
        // One test owns the process-global env var so parallel test threads never
        // race on it (there is no other reader/writer of it in this binary).
        let path = "/sessions/live.jsonl";
        std::env::set_var(RESTART_RESUME_ENV, path);
        assert_eq!(take_boot_resume_request(), Some(PathBuf::from(path)));
        // Consumed exactly once: a second read sees nothing.
        assert_eq!(take_boot_resume_request(), None);

        // An empty value is treated as absent (fail-safe), and is also consumed.
        std::env::set_var(RESTART_RESUME_ENV, "");
        assert_eq!(take_boot_resume_request(), None);
        assert!(std::env::var_os(RESTART_RESUME_ENV).is_none());
    }
}
