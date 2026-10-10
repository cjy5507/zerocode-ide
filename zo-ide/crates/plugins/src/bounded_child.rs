//! A child under a deadline, and the one order in which its process group is
//! ended (t-19897).
//!
//! Five places ended a timed child's process group, each in its own order: the
//! runtime's hooks, formatter, strict check and foreground bash, the runtime's
//! tmux asks, and the plugin runner. This module holds the signals and the reaps
//! they share, under one rule:
//!
//! - A leader's exit is looked for without reaping it ([`observe_exit`]). A
//!   zombie keeps its pid, and that pid is also the group's id, so the pid cannot
//!   be handed on while the group is still being ended.
//! - A group is ended only while its leader is unreaped (`end_group`, unix only).
//! - The leader is reaped last ([`reap`], [`try_reap`]).
//!
//! Limits, grace periods and captured output stay with the callers, which pass
//! them in. Where no exit can be seen without reaping, [`observe_exit`] answers
//! `None`, and the caller keeps the order it had before this module (Windows, and
//! any unix target without kqueue or waitid).

use std::io;
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

/// How a process group is ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// SIGKILL at once.
    Kill,
    /// SIGTERM, then SIGKILL once `grace` has passed.
    Terminate { grace: Duration },
}

/// How often an exit is looked for where the kernel gives no watch on the child
/// (t-19897). The tmux ask has always looked at this interval, and its fallback
/// still does, so the two wake-up rates are the same.
pub const EXIT_LOOK_EVERY: Duration = Duration::from_millis(5);

/// Reaps the child, and returns its status.
pub fn reap(child: &mut Child) -> io::Result<ExitStatus> {
    let pid = child.id();
    let status = child.wait();
    record_reaped(pid);
    status
}

/// Reaps the child if it has exited, and returns its status; `None` while it runs.
pub fn try_reap(child: &mut Child) -> io::Result<Option<ExitStatus>> {
    let pid = child.id();
    let status = child.try_wait()?;
    if status.is_some() {
        record_reaped(pid);
    }
    Ok(status)
}

/// Records a reap made through another API, such as a tokio child's `wait`, so
/// the order tests see it. It does nothing outside those tests.
pub fn note_reaped(pid: u32) {
    record_reaped(pid);
}

/// Whether the child `pid` has exited, looked for until `until` (`None`: until it
/// has). The child is not reaped, so a zombie answers `Some(true)`. `None` where
/// this platform has no such look, or the look failed.
#[cfg(target_os = "macos")]
#[must_use]
pub fn observe_exit(pid: u32, until: Option<Instant>) -> Option<bool> {
    use nix::errno::Errno;
    use nix::sys::event::{EventFilter, EventFlag, FilterFlag, KEvent, Kqueue};

    let queue = Kqueue::new().ok()?;
    let watch = KEvent::new(
        usize::try_from(pid).unwrap_or(usize::MAX),
        EventFilter::EVFILT_PROC,
        EventFlag::EV_ADD | EventFlag::EV_ONESHOT,
        FilterFlag::NOTE_EXIT,
        0,
        0,
    );
    let mut none: [KEvent; 0] = [];
    match queue.kevent(&[watch], &mut none, Some(timespec_of(Duration::ZERO))) {
        Ok(_) => {}
        // The kernel does not watch a zombie: a process it refuses has exited.
        Err(Errno::ESRCH) => return Some(true),
        Err(_) => return None,
    }
    let mut events = [blank()];
    loop {
        let left = until.map(|deadline| deadline.saturating_duration_since(Instant::now()));
        match queue.kevent(&[], &mut events, left.map(timespec_of)) {
            Ok(0) if left.is_some_and(|left| left.is_zero()) => return Some(false),
            // The wait ran out before the deadline, or was interrupted: look again.
            Ok(0) | Err(Errno::EINTR) => {}
            Ok(_) => return Some(true),
            Err(_) => return None,
        }
    }
}

/// Whether the child `pid` has exited, looked for until `until` (`None`: until it
/// has). The child is not reaped: `WNOWAIT` leaves the zombie waitable.
#[cfg(target_os = "linux")]
#[must_use]
pub fn observe_exit(pid: u32, until: Option<Instant>) -> Option<bool> {
    use nix::errno::Errno;
    use nix::sys::wait::{waitid, Id, WaitPidFlag, WaitStatus};
    use nix::unistd::Pid;

    let child = Pid::from_raw(i32::try_from(pid).ok()?);
    loop {
        let flags = if until.is_some() {
            WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT
        } else {
            WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT
        };
        match waitid(Id::Pid(child), flags) {
            Ok(WaitStatus::StillAlive) => {
                let deadline = until?;
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Some(false);
                }
                std::thread::sleep(left.min(EXIT_LOOK_EVERY));
            }
            Ok(_) => return Some(true),
            Err(Errno::EINTR) => {}
            Err(_) => return None,
        }
    }
}

/// No exit can be seen without reaping on this platform: the caller keeps its
/// own order.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[must_use]
pub fn observe_exit(_pid: u32, _until: Option<Instant>) -> Option<bool> {
    None
}

/// Ends the process group that the child `pid` leads, while `pid` is unreaped.
/// A group already gone, or holding only zombies (macOS answers EPERM to a
/// signal for those), has nothing left to stop, which is not an error.
#[cfg(unix)]
pub fn end_group(pid: u32, how: Group) -> Result<(), nix::errno::Errno> {
    let Some(end) = GroupEnd::begin(pid, how) else {
        return Ok(());
    };
    if let Some(grace) = end.first()? {
        std::thread::sleep(grace);
        end.finish()?;
    }
    Ok(())
}

/// One group end, split at its grace so that a caller can hold its own lock
/// around each half (t-19897). `begin` names the group. `first` sends SIGTERM,
/// or SIGKILL for `Group::Kill`, and says how long to wait before `finish`, which
/// sends SIGKILL. The policy for ESRCH and EPERM lives here, once.
#[cfg(unix)]
pub struct GroupEnd {
    pid: u32,
    group: nix::unistd::Pid,
    how: Group,
}

#[cfg(unix)]
impl GroupEnd {
    /// `None` for a pid that this module may not signal (see `spawned_group`).
    #[must_use]
    pub fn begin(pid: u32, how: Group) -> Option<Self> {
        Some(Self {
            pid,
            group: spawned_group(pid)?,
            how,
        })
    }

    /// Sends the first signal. `Ok(Some(grace))`: the group may remain, and
    /// `finish` must follow once `grace` has passed. `Ok(None)`: nothing is left.
    pub fn first(&self) -> Result<Option<Duration>, nix::errno::Errno> {
        use nix::errno::Errno;
        use nix::sys::signal::{killpg, Signal};

        record_ended(self.pid);
        match self.how {
            Group::Kill => nothing_left(killpg(self.group, Signal::SIGKILL)).map(|()| None),
            Group::Terminate { grace } => match killpg(self.group, Signal::SIGTERM) {
                Ok(()) => Ok(Some(grace)),
                Err(Errno::ESRCH | Errno::EPERM) => Ok(None),
                Err(error) => Err(error),
            },
        }
    }

    /// Sends SIGKILL, after the grace that `first` named.
    pub fn finish(&self) -> Result<(), nix::errno::Errno> {
        use nix::sys::signal::{killpg, Signal};
        nothing_left(killpg(self.group, Signal::SIGKILL))
    }
}

/// `Ok` for a signal that found nothing to stop (ESRCH), or only zombies (EPERM).
#[cfg(unix)]
fn nothing_left(result: Result<(), nix::errno::Errno>) -> Result<(), nix::errno::Errno> {
    use nix::errno::Errno;
    match result {
        Ok(()) | Err(Errno::ESRCH | Errno::EPERM) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Reaps the child if it has exited, and returns its status; `None` while it runs.
/// Where its exit can be seen without reaping, the group is ended first when
/// `sweep` asks for it, while the leader is still unreaped, and only then is the
/// child reaped. Where that look does not exist, the child is reaped as
/// `try_reap` does, and no group is ended.
pub fn reap_if_exited(child: &mut Child, sweep: Option<Group>) -> io::Result<Option<ExitStatus>> {
    #[cfg(unix)]
    {
        let pid = child.id();
        match observe_exit(pid, Some(Instant::now())) {
            Some(true) => {
                if let Some(how) = sweep {
                    let _ = end_group(pid, how);
                }
                reap(child).map(Some)
            }
            Some(false) => Ok(None),
            None => try_reap(child),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = sweep;
        try_reap(child)
    }
}

/// The group a child spawned here leads, which is the only group this module
/// signals: never 0 (`killpg` reads it as the caller's own group), never init's,
/// and never this process's own.
#[cfg(unix)]
fn spawned_group(pid: u32) -> Option<nix::unistd::Pid> {
    let group = nix::unistd::Pid::from_raw(i32::try_from(pid).ok().filter(|raw| *raw > 1)?);
    (group != nix::unistd::getpid() && group != nix::unistd::getpgrp()).then_some(group)
}

#[cfg(target_os = "macos")]
fn blank() -> nix::sys::event::KEvent {
    use nix::sys::event::{EventFilter, EventFlag, FilterFlag, KEvent};
    KEvent::new(0, EventFilter::EVFILT_READ, EventFlag::empty(), FilterFlag::empty(), 0, 0)
}

#[cfg(target_os = "macos")]
fn timespec_of(wait: Duration) -> nix::libc::timespec {
    use nix::libc::{c_long, time_t, timespec};
    timespec {
        tv_sec: time_t::try_from(wait.as_secs()).unwrap_or(time_t::MAX),
        tv_nsec: c_long::from(wait.subsec_nanos()),
    }
}

#[cfg(all(unix, any(test, feature = "order-trace")))]
fn record_ended(pid: u32) {
    trace::note(trace::Step::Ended(pid));
}

#[cfg(all(unix, not(any(test, feature = "order-trace"))))]
fn record_ended(_pid: u32) {}

#[cfg(all(unix, any(test, feature = "order-trace")))]
fn record_reaped(pid: u32) {
    trace::note(trace::Step::Reaped(pid));
}

#[cfg(not(all(unix, any(test, feature = "order-trace"))))]
fn record_reaped(_pid: u32) {}

/// What a test sees of the order: a group ended, or a leader reaped, per pid.
/// Compiled for this crate's tests, and for the runtime's tests through the
/// `order-trace` feature, which only the runtime's dev-dependencies turn on.
#[cfg(all(unix, any(test, feature = "order-trace")))]
pub mod trace {
    use std::sync::{Mutex, PoisonError};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Step {
        Ended(u32),
        Reaped(u32),
    }

    impl Step {
        fn pid(self) -> u32 {
            match self {
                Step::Ended(pid) | Step::Reaped(pid) => pid,
            }
        }
    }

    static STEPS: Mutex<Vec<Step>> = Mutex::new(Vec::new());

    pub(crate) fn note(step: Step) {
        STEPS.lock().unwrap_or_else(PoisonError::into_inner).push(step);
    }

    /// A place in the recorded order. A test takes one before it starts its child,
    /// and then looks only at the steps recorded after it. A pid is handed on
    /// again only after its previous child has been reaped, so a step recorded
    /// before the mark cannot belong to the child the test starts (t-19897).
    #[must_use]
    pub fn mark() -> usize {
        STEPS.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// The steps taken on the child `pid` after `mark`, in the order they were taken.
    #[must_use]
    pub fn order_since(mark: usize, pid: u32) -> Vec<Step> {
        let steps = STEPS.lock().unwrap_or_else(PoisonError::into_inner);
        steps
            .get(mark..)
            .unwrap_or(&[])
            .iter()
            .copied()
            .filter(|step| step.pid() == pid)
            .collect()
    }

    /// Asserts that the group of `pid` was ended after `mark`, and that no end came
    /// after its leader was reaped.
    pub fn assert_ended_before_reaped(mark: usize, pid: u32) {
        let order = order_since(mark, pid);
        assert!(order.contains(&Step::Ended(pid)), "the group of {pid} was never ended: {order:?}");
        if let Some(reaped) = order.iter().position(|step| matches!(step, Step::Reaped(_))) {
            assert!(
                !order[reaped..].iter().any(|step| matches!(step, Step::Ended(_))),
                "the group of {pid} was ended after its leader was reaped: {order:?}"
            );
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use nix::sys::signal::kill;
    use nix::unistd::{getpgrp, getpid, Pid};

    use super::{end_group, reap, spawned_group, Group};

    /// Only the group a child leads is ever signalled: not 0, not init's, not this
    /// process's own, and not a pid that reads as negative (t-18917).
    #[test]
    fn only_the_group_a_child_leads_is_ever_signalled() {
        let own = u32::try_from(getpid().as_raw()).expect("own pid");
        let own_group = u32::try_from(getpgrp().as_raw()).expect("own group");
        for refused in [0, 1, own, own_group, u32::MAX] {
            assert!(spawned_group(refused).is_none(), "{refused} could be signalled");
        }
    }

    /// A child that has exited is heard as exited, and the look does not reap it:
    /// the status is still there to be taken afterwards (t-19897).
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_child_that_has_exited_is_seen_without_being_reaped() {
        let mut quick = Command::new("/bin/sleep").arg("0.1").spawn().expect("spawn");
        let started = Instant::now();
        assert_eq!(super::observe_exit(quick.id(), Some(started + Duration::from_secs(5))), Some(true));
        assert!(started.elapsed() < Duration::from_secs(1), "the exit was heard after {:?}", started.elapsed());
        assert!(quick.try_wait().expect("try_wait").is_some(), "the look reaped the child");
    }

    /// A child still running at the deadline is not heard as exited, and the look
    /// keeps its deadline.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_deadline_is_kept_for_a_child_still_running() {
        let mut slow = Command::new("/bin/sleep").arg("5").spawn().expect("spawn");
        let started = Instant::now();
        assert_eq!(super::observe_exit(slow.id(), Some(started + Duration::from_millis(300))), Some(false));
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(300) && took < Duration::from_millis(600),
            "the deadline was not kept: {took:?}"
        );
        let _ = slow.kill();
        let _ = slow.wait();
    }

    /// Ending the group of an unreaped leader reaches the children it started, and
    /// the leader is still there to be reaped afterwards. Nothing is left in the
    /// group at the end (t-19897).
    #[test]
    fn ending_the_group_reaches_the_grandchildren_and_leaves_the_leader_to_reap() {
        use std::io::{BufRead as _, BufReader};

        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg("sleep 30 & echo $!; wait").stdout(Stdio::piped());
        command.process_group(0);
        let mut child = command.spawn().expect("spawn");
        let leader = child.id();
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut line)
            .expect("read the grandchild pid");
        let grandchild = Pid::from_raw(line.trim().parse::<i32>().expect("grandchild pid"));

        end_group(leader, Group::Kill).expect("end the group");
        let status = reap(&mut child).expect("reap the leader");
        assert!(!status.success(), "the leader was not ended: {status:?}");

        let until = Instant::now() + Duration::from_secs(5);
        while kill(grandchild, None).is_ok() {
            assert!(Instant::now() < until, "the grandchild outlived its group");
            std::thread::sleep(Duration::from_millis(10));
        }
        let group = Pid::from_raw(i32::try_from(leader).expect("leader pid"));
        assert!(nix::sys::signal::killpg(group, None).is_err(), "the group {leader} still has members");
    }
}
