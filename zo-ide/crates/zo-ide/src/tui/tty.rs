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
}
