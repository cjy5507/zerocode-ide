//! One tmux call that no tmux can hold a wait with (t-18917).
//!
//! A pane wait asked tmux whether its pane still stood from inside its own
//! loop, with `Command::output()` and no limit. While the call ran the wait did
//! nothing else — an answer that had landed waited, a limit ran over by as long
//! as the call took, a cancel went unseen — and a first exec under parallel
//! tests took 1 to 3.4 s (`reports/t-17057`); a hung server never answers.
//!
//! Here a call runs on a thread of its own, which spawns the tmux, rests until
//! it exits or [`TMUX_ASK_BOUND`] has passed, and ends it then. The caller holds
//! an [`Ask`] and looks at it when it likes, or waits for it a while; dropping
//! one that has not answered ends its tmux at once. The thread rests on the
//! kernel's word that the tmux exited ([`child_watch::exited_by`]), not on a
//! look every few milliseconds, and wakes the asking wait's watch only when
//! the wait handed it one to wake; either way an ask costs the wait no look on
//! a timer — wake-ups are what the pane probe counts.
//!
//! **Only the tmux's own group is ever signalled.** On unix a tmux leads a
//! process group of its own, because a tmux can be a script — the window's is
//! `sh` running `curl` in a subshell — and ending only the process that was
//! spawned would leave the rest running out deadlines of their own. The group
//! is signalled only while its leader is unreaped: the leader's pid is taken
//! back before it is reaped, so a pid the system may have handed on is never
//! signalled, and [`spawned_group`] refuses 0 and this process's own group.

use std::ffi::OsString;
use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::child_watch::{self, Waker};
use super::TMUX_ASK_BOUND;

/// More than any `list-panes` prints. What a tmux prints past it is not read.
const PRINTED_LIMIT: u64 = 1 << 20;

/// How often an exit is looked for where the kernel does not report one.
const EXIT_LOOK_EVERY: Duration = Duration::from_millis(5);

/// What one tmux call came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Said {
    /// It succeeded, and this is what it printed.
    Yes(String),
    /// It answered with a failure.
    No,
    /// It could not be run, or said nothing within [`TMUX_ASK_BOUND`].
    Nothing,
}

/// One tmux call, answered on a thread of its own.
pub(super) struct Ask {
    shared: Arc<Shared>,
    started: Instant,
    /// Whether dropping the handle ends the tmux: [`Ask::let_finish`] clears it.
    end_on_drop: bool,
}

struct Shared {
    state: Mutex<State>,
    settled: Condvar,
    /// The asking wait's watch, woken when the answer is in.
    wake: Option<Waker>,
}

#[derive(Default)]
struct State {
    /// The tmux's pid while it may be signalled: set once it is spawned, and
    /// taken back before it is reaped.
    pid: Option<u32>,
    /// Nobody waits for the answer any more: the tmux is ended, and what it
    /// says goes unheard.
    abandoned: bool,
    said: Option<Said>,
}

fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Ask {
    /// Run `program` with `args` on a thread of its own. `printing` says
    /// whether what it prints is wanted; `wake` is woken once it has answered.
    pub(super) fn start(program: &std::ffi::OsStr, args: &[&str], printing: bool, wake: Option<Waker>) -> Self {
        let started = Instant::now();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            settled: Condvar::new(),
            wake,
        });
        let program = program.to_os_string();
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let asking = Arc::clone(&shared);
        let running = std::thread::Builder::new()
            .name("zo-tmux-ask".to_string())
            .spawn(move || {
                let said = ask(&asking, &program, &args, printing, started + TMUX_ASK_BOUND);
                settle(&asking, said);
            });
        if running.is_err() {
            settle(&shared, Said::Nothing);
        }
        Self {
            shared,
            started,
            end_on_drop: true,
        }
    }

    /// What the tmux said — [`Said::Nothing`] once [`TMUX_ASK_BOUND`] has passed
    /// without a word, which covers a spawn that is itself stuck — or `None`
    /// while it may still speak.
    pub(super) fn heard(&self) -> Option<Said> {
        lock(&self.shared)
            .said
            .clone()
            .or_else(|| (self.started.elapsed() >= TMUX_ASK_BOUND).then_some(Said::Nothing))
    }

    /// Wait up to `within` for what the tmux says; `None` if it has not said it
    /// by then.
    pub(super) fn answer_within(&self, within: Duration) -> Option<Said> {
        let state = lock(&self.shared);
        let (state, _) = self
            .shared
            .settled
            .wait_timeout_while(state, within, |state| state.said.is_none())
            .unwrap_or_else(PoisonError::into_inner);
        state.said.clone()
    }

    /// Stop waiting, and let the tmux finish on its own — which it does, or is
    /// ended, within [`TMUX_ASK_BOUND`] of its start.
    pub(super) fn let_finish(mut self) {
        self.end_on_drop = false;
    }
}

impl Drop for Ask {
    fn drop(&mut self) {
        if !self.end_on_drop {
            return;
        }
        let mut state = lock(&self.shared);
        if state.said.is_some() {
            return;
        }
        state.abandoned = true;
        // Signalled here and now, so the thread hears the exit at once rather
        // than at its deadline — and under the lock the pid is taken back
        // under before any reap, so never after one.
        #[cfg(unix)]
        if let Some(pid) = state.pid {
            end_group(pid);
        }
    }
}

/// The answer is in: kept, and told to whoever waits for it.
fn settle(shared: &Shared, said: Said) {
    {
        let mut state = lock(shared);
        state.pid = None;
        state.said = Some(said);
    }
    shared.settled.notify_all();
    if let Some(wake) = &shared.wake {
        wake.wake();
    }
}

/// The call itself, on its own thread.
fn ask(shared: &Shared, program: &std::ffi::OsStr, args: &[OsString], printing: bool, deadline: Instant) -> Said {
    // What a tmux prints goes to an unnamed file rather than a pipe: nothing
    // has to read it while the tmux runs, and a process left holding a pipe
    // cannot keep a read waiting (`probe_codex_version` does the same).
    let printed = if printing {
        let Ok(file) = tempfile::tempfile() else {
            return Said::Nothing;
        };
        Some(file)
    } else {
        None
    };
    let stdout = match printed.as_ref().map(File::try_clone) {
        Some(Ok(file)) => Stdio::from(file),
        Some(Err(_)) => return Said::Nothing,
        None => Stdio::null(),
    };
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(stdout).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let Ok(mut child) = command.spawn() else {
        return Said::Nothing;
    };
    let pid = child.id();
    {
        let mut state = lock(shared);
        // The spawn itself can take longer than the bound: a first exec is
        // checked by the system before it runs.
        if state.abandoned || Instant::now() >= deadline {
            drop(state);
            end(&mut child, pid);
            return Said::Nothing;
        }
        state.pid = Some(pid);
    }
    let status = match exit_by(shared, &mut child, pid, deadline) {
        Exit::Reaped(status) => status,
        // Reaped by someone else: its pid is not this module's to signal.
        Exit::Gone => return Said::Nothing,
        Exit::Running => {
            lock(shared).pid = None;
            end(&mut child, pid);
            return Said::Nothing;
        }
    };
    if !status.success() {
        return Said::No;
    }
    let Some(mut printed) = printed else {
        return Said::Yes(String::new());
    };
    let mut bytes = Vec::new();
    if printed.seek(SeekFrom::Start(0)).is_err() || printed.take(PRINTED_LIMIT).read_to_end(&mut bytes).is_err() {
        return Said::Nothing;
    }
    Said::Yes(String::from_utf8_lossy(&bytes).into_owned())
}

/// How a rest for a tmux's exit ended.
enum Exit {
    /// It exited, and this is its status: it is reaped.
    Reaped(ExitStatus),
    /// It is gone, but not reaped here — somebody else reaped it.
    Gone,
    /// It still runs at the deadline, or nobody waits for it any more.
    Running,
}

/// Rest until the tmux exits or `deadline` passes.
fn exit_by(shared: &Shared, child: &mut Child, pid: u32, deadline: Instant) -> Exit {
    match child_watch::exited_by(pid, deadline) {
        Some(true) => {
            lock(shared).pid = None;
            // What the tmux left running in its group goes with it. The
            // leader is not reaped yet, so the group is still only its own.
            #[cfg(unix)]
            end_group(pid);
            child.wait().map_or(Exit::Gone, Exit::Reaped)
        }
        Some(false) => Exit::Running,
        None => look_for_exit(shared, child, deadline),
    }
}

/// [`exit_by`] where the kernel does not report an exit: a look every
/// [`EXIT_LOOK_EVERY`]. A look reaps, so it is made under the lock, and the pid
/// is taken back in the same breath.
fn look_for_exit(shared: &Shared, child: &mut Child, deadline: Instant) -> Exit {
    loop {
        {
            let mut state = lock(shared);
            match child.try_wait() {
                Ok(Some(status)) => {
                    state.pid = None;
                    return Exit::Reaped(status);
                }
                Ok(None) if state.abandoned => return Exit::Running,
                Ok(None) => {}
                Err(_) => {
                    state.pid = None;
                    return Exit::Gone;
                }
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Exit::Running;
        }
        std::thread::sleep(left.min(EXIT_LOOK_EVERY));
    }
}

/// End a tmux that has not exited, with whatever it started, and reap it.
fn end(child: &mut Child, pid: u32) {
    #[cfg(unix)]
    end_group(pid);
    #[cfg(not(unix))]
    let _ = pid;
    let _ = child.kill();
    let _ = child.wait();
}

/// Kill the process group the tmux `pid` leads.
#[cfg(unix)]
fn end_group(pid: u32) {
    if let Some(group) = spawned_group(pid) {
        // ESRCH: nothing is left of it. EPERM: only its unreaped leader is.
        let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
    }
}

/// The group a tmux spawned here leads, which is the only group this module
/// signals — never 0, which `killpg` reads as the caller's own group, nor
/// init's, nor this process's own.
#[cfg(unix)]
fn spawned_group(pid: u32) -> Option<nix::unistd::Pid> {
    let group = nix::unistd::Pid::from_raw(i32::try_from(pid).ok().filter(|raw| *raw > 1)?);
    (group != nix::unistd::getpid() && group != nix::unistd::getpgrp()).then_some(group)
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::{Duration, Instant};

    use nix::sys::signal::kill;
    use nix::unistd::{getpgid, getpgrp, getpid, Pid};

    use super::{lock, spawned_group, Ask, Said};

    /// Only the group a tmux leads is ever signalled: not 0 (this process's
    /// own group, to `killpg`), not init's, not this process or its group,
    /// and not a pid that reads as a negative number (t-18917).
    #[test]
    fn only_the_group_a_tmux_leads_is_ever_signalled() {
        let own = u32::try_from(getpid().as_raw()).expect("own pid");
        let own_group = u32::try_from(getpgrp().as_raw()).expect("own group");
        for refused in [0, 1, own, own_group, u32::MAX] {
            assert!(spawned_group(refused).is_none(), "{refused} could be signalled");
        }

        // An asked process leads a group of its own, and that group — its own
        // pid — is the one that would be ended.
        let ask = Ask::start(std::ffi::OsStr::new("/bin/sleep"), &["5"], false, None);
        let until = Instant::now() + Duration::from_secs(2);
        let pid = loop {
            if let Some(pid) = lock(&ask.shared).pid {
                break pid;
            }
            assert!(Instant::now() < until, "the sleep was not spawned within 2 s");
            std::thread::sleep(Duration::from_millis(2));
        };
        let raw = Pid::from_raw(i32::try_from(pid).expect("pid"));
        assert_eq!(getpgid(Some(raw)).expect("its group"), raw, "the asked process shares a group");
        assert_eq!(spawned_group(pid), Some(raw));

        // Dropped unanswered, it is ended with its group at once.
        let dropped = Instant::now();
        drop(ask);
        while kill(raw, None).is_ok() && dropped.elapsed() < Duration::from_millis(250) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(kill(raw, None).is_err(), "a dropped ask left its process running");
    }

    /// A call that succeeds says what it printed; one that fails says no; one
    /// that cannot run says nothing — and none of them is waited for past its
    /// answer.
    #[test]
    fn a_call_says_what_it_printed_or_that_it_failed_or_nothing() {
        let printed = Ask::start(std::ffi::OsStr::new("/bin/echo"), &["%1"], true, None);
        assert_eq!(printed.answer_within(Duration::from_secs(2)), Some(Said::Yes("%1\n".to_string())));
        let failed = Ask::start(std::ffi::OsStr::new("/usr/bin/false"), &[], true, None);
        assert_eq!(failed.answer_within(Duration::from_secs(2)), Some(Said::No));
        let missing = Ask::start(std::ffi::OsStr::new("/nonexistent/tmux-that-is-not-there"), &[], true, None);
        assert_eq!(missing.answer_within(Duration::from_secs(2)), Some(Said::Nothing));
    }
}
