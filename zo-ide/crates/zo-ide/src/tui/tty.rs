//! The terminal's foreground group is ours.
//!
//! A terminal whose foreground process group is not zo's is a terminal zo
//! cannot read: every read fails with `EIO`, and crossterm's event thread
//! retries that read forever without ever waking the app (`EventStream`'s wake
//! task loops until a poll returns `Ok(true)`). The app sees no error and no
//! key — the pane sits idle with a composer that echoes nothing, and one core
//! spins on the failing read.
//!
//! That is what a child of zo did on 2026-09-03 ("zo 지금 인풋창에 아무것도
//! 입력안됨"): a test the `bash` tool ran opened an interactive shell, which
//! made its own process group the terminal's foreground group — as every
//! job-control shell does when it comes up — and exited without giving it
//! back. The same wedge was seen on 2026-09-02 under a hung `cargo test`.
//!
//! So, on the tick that follows the pane's size, the app checks that its group
//! is still the foreground one and takes the terminal back when it is not —
//! exactly what a job-control shell does after every job.

/// The terminal's size, `(columns, rows)`, asked of the terminal zo draws on.
///
/// The frame loop asks on every draw (codex's rule — see the app's
/// `reconcile_size`), and crossterm's `terminal::size` opens `/dev/tty` for
/// every call: an `open`, an `ioctl` and a `close` per frame on the thread
/// that reads keys — 11 of its 14 `__open` samples while a reply streamed
/// (2026-09-28). Standard output is that terminal whenever the TUI runs; a
/// size it cannot give falls back to crossterm's own question.
#[cfg(unix)]
pub(super) fn size() -> std::io::Result<(u16, u16)> {
    match rustix::termios::tcgetwinsize(std::io::stdout()) {
        Ok(size) if size.ws_col > 0 && size.ws_row > 0 => Ok((size.ws_col, size.ws_row)),
        _ => crossterm::terminal::size(),
    }
}

#[cfg(not(unix))]
pub(super) fn size() -> std::io::Result<(u16, u16)> {
    crossterm::terminal::size()
}

/// Whether `terminal` has hung up: the window closed the pane.
///
/// A pane's pty is zo's standard streams and nobody's controlling terminal, so
/// closing it sends no SIGHUP; and crossterm's reader answers the end-of-file a
/// closed pty gives every read by polling again, forever, so the input stream
/// never ends either (t-76434). `poll` tells without taking a byte from that
/// reader: a pty whose other end closed is `POLLHUP` (`POLLIN | POLLHUP` on
/// macOS), a live one never is. Windows panes are consoles, not ptys, and have
/// nothing here to ask.
#[cfg(unix)]
pub(super) fn hung_up_on(terminal: std::os::fd::BorrowedFd<'_>) -> bool {
    use nix::poll::{poll, PollFd, PollFlags, PollTimeout};

    let mut fds = [PollFd::new(terminal, PollFlags::POLLIN)];
    matches!(poll(&mut fds, PollTimeout::ZERO), Ok(ready) if ready > 0)
        && fds[0]
            .revents()
            .is_some_and(|revents| revents.contains(PollFlags::POLLHUP))
}

/// What `reclaim_foreground` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foreground {
    /// Our process group is the terminal's foreground group — nothing to do.
    Ours,
    /// Another group had the terminal; it is ours again.
    Reclaimed,
    /// Standard input is not a terminal: a pipe, a file, a test.
    NoTerminal,
    /// The terminal would not give itself back — it is gone, or never ours.
    Failed,
}

/// Make sure our process group is the terminal's foreground group.
#[cfg(unix)]
pub(super) fn reclaim_foreground() -> Foreground {
    use std::io::IsTerminal as _;
    use std::os::fd::AsFd as _;

    use nix::sys::signal::{pthread_sigmask, SigSet, Signal, SigmaskHow};
    use nix::unistd::{getpgrp, tcgetpgrp, tcsetpgrp};

    let input = std::io::stdin();
    if !input.is_terminal() {
        return Foreground::NoTerminal;
    }
    let ours = getpgrp();
    match tcgetpgrp(input.as_fd()) {
        Ok(holder) if holder == ours => Foreground::Ours,
        Ok(_) => {
            // From a background group `tcsetpgrp` stops the caller with
            // SIGTTOU unless the signal is blocked or ignored. Blocking is
            // enough on Linux and macOS both, and a thread's mask is its own,
            // so no handler is installed process-wide.
            let mut ttou = SigSet::empty();
            ttou.add(Signal::SIGTTOU);
            let mut before = SigSet::empty();
            let blocked =
                pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&ttou), Some(&mut before)).is_ok();
            let result = tcsetpgrp(input.as_fd(), ours);
            if blocked {
                let _ = pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&before), None);
            }
            match result {
                Ok(()) => Foreground::Reclaimed,
                Err(_) => Foreground::Failed,
            }
        }
        Err(_) => Foreground::Failed,
    }
}

/// Windows has no process groups on a console in this sense; nothing to hold.
#[cfg(not(unix))]
pub(super) fn reclaim_foreground() -> Foreground {
    Foreground::NoTerminal
}

#[cfg(test)]
mod tests {
    use super::{reclaim_foreground, Foreground};

    /// Under `cargo test` standard input is a pipe or the developer's own
    /// terminal, whose foreground group is the one running the tests. Either
    /// way there is nothing to take back — and nothing fails.
    #[test]
    fn a_terminal_that_is_ours_or_absent_needs_no_reclaiming() {
        assert!(matches!(
            reclaim_foreground(),
            Foreground::Ours | Foreground::NoTerminal
        ));
    }

    /// A pty whose other end is open has not hung up; once that end closes,
    /// the way the window closes a pane, it has — once every copy of it has
    /// closed. A child a parallel test spawned while the end was open inherited
    /// one (`openpty` hands both ends over inheritable) and holds it until the
    /// child exits, so the hang-up is waited for, not looked at once (l81d).
    #[cfg(unix)]
    #[test]
    fn a_pty_has_hung_up_once_its_other_end_closes() {
        use std::os::fd::AsFd as _;
        use std::time::{Duration, Instant};

        let pty = crate::tui::test_pty::open_pty(None).expect("open a pty");
        assert!(!super::hung_up_on(pty.slave.as_fd()), "a live pty has not hung up");
        drop(pty.master);
        let until = Instant::now() + Duration::from_secs(60);
        while !super::hung_up_on(pty.slave.as_fd()) {
            assert!(Instant::now() < until, "a pty whose other end closed has hung up");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A child spawned while a test's pty is open keeps no copy of its master,
    /// so the user end hangs up the moment the test closes it — not when that
    /// child exits. `Command::spawn` returns once the child has exec'd, so an
    /// end marked close-on-exec is already gone from it (t-78154).
    #[cfg(unix)]
    #[test]
    fn a_child_spawned_beside_an_open_pty_keeps_no_copy_of_its_master() {
        use std::os::fd::AsFd as _;
        use std::process::{Command, Stdio};

        let pty = crate::tui::test_pty::open_pty(None).expect("open a pty");
        let mut child = Command::new("/bin/sleep")
            .arg("5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn a child");
        drop(pty.master);
        let hung_up = super::hung_up_on(pty.slave.as_fd());
        let _ = child.kill();
        let _ = child.wait();
        assert!(hung_up, "a child spawned while the pty was open held its master");
    }

    /// Both ends of a test's pty close on exec from the moment they exist.
    #[cfg(unix)]
    #[test]
    fn both_ends_of_a_test_pty_close_on_exec() {
        use nix::fcntl::{fcntl, FcntlArg, FdFlag};
        use std::os::fd::AsRawFd as _;

        let pty = crate::tui::test_pty::open_pty(None).expect("open a pty");
        for (name, end) in [("master", pty.master.as_raw_fd()), ("slave", pty.slave.as_raw_fd())] {
            let flags = FdFlag::from_bits_truncate(fcntl(end, FcntlArg::F_GETFD).expect("the end's flags"));
            assert!(flags.contains(FdFlag::FD_CLOEXEC), "the {name} end is inherited by every child");
        }
    }

    /// What `open_with` makes of an opener that fails with `errnos` in turn and
    /// opens after them, and how many times it asked.
    #[cfg(unix)]
    fn open_after(errnos: &[i32]) -> (Result<(), crate::tui::test_pty::OpenFailed>, usize) {
        let mut asked = 0;
        let opened = crate::tui::test_pty::open_with(|| {
            asked += 1;
            errnos.get(asked - 1).map_or(Ok(()), |&errno| Err(errno))
        });
        (opened, asked)
    }

    /// Two parallel opens of `/dev/ptmx` can pick the same number on macOS,
    /// and the loser's `-6` is gone on the next try: a pair that opens on the
    /// third try is a pair (t-78154).
    #[cfg(unix)]
    #[test]
    fn a_pty_that_opens_after_two_lost_races_is_opened() {
        use crate::tui::test_pty::EREDRIVEOPEN;

        assert_eq!(open_after(&[EREDRIVEOPEN, EREDRIVEOPEN]), (Ok(()), 3));
    }

    /// An error no later try clears — out of descriptors — fails at once.
    #[cfg(unix)]
    #[test]
    fn a_pty_out_of_descriptors_fails_without_another_try() {
        use crate::tui::test_pty::OpenFailed;

        let emfile = nix::errno::Errno::EMFILE as i32;
        assert_eq!(open_after(&[emfile]), (Err(OpenFailed { errno: emfile, tries: 1 }), 1));
    }

    /// Races lost every time end after the last allowed try, with the raw `-6`.
    #[cfg(unix)]
    #[test]
    fn a_pty_that_loses_every_race_fails_after_five_tries_with_the_raw_errno() {
        use crate::tui::test_pty::{OpenFailed, EREDRIVEOPEN};

        assert_eq!(
            open_after(&[EREDRIVEOPEN; 6]),
            (Err(OpenFailed { errno: EREDRIVEOPEN, tries: 5 }), 5)
        );
    }

    /// A lasting error after lost races ends the tries where it comes.
    #[cfg(unix)]
    #[test]
    fn a_lasting_error_after_lost_races_ends_the_tries_where_it_comes() {
        use crate::tui::test_pty::{OpenFailed, EREDRIVEOPEN};

        let emfile = nix::errno::Errno::EMFILE as i32;
        assert_eq!(
            open_after(&[EREDRIVEOPEN, EREDRIVEOPEN, emfile]),
            (Err(OpenFailed { errno: emfile, tries: 3 }), 3)
        );
    }
}
