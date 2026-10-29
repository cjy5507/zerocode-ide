//! Opening a pty pair for a test — the one place the lib's `#[cfg(test)]` code
//! and the e2e harness open one (two crates, this one file through `#[path]`),
//! so both close the same ends on exec and try the same errors as often.
//!
//! - Both ends close on exec from the moment they exist. `openpty` hands them
//!   over inheritable, and a child a parallel test spawns meanwhile keeps a
//!   copy: a closed end then does not hang up until that child exits (l81d;
//!   44 % of opens under constant spawning, still 14 % with `FD_CLOEXEC` set
//!   right after `openpty`). `/dev/ptmx` and the user end open with
//!   `O_CLOEXEC` instead, so the gap between `openpty` and `fcntl` is gone.
//!   On macOS too: rustix's `openpt` passes no `CLOEXEC` there, `open` does.
//!   A child another thread forked still holds a copy until it execs, so a
//!   test that looks for a hang-up waits for it.
//! - Opening `/dev/ptmx` can fail for a moment. macOS's `ptmx_clone` picks a
//!   free number without a lock, and the loser of two parallel opens gets
//!   [`EREDRIVEOPEN`] once the kernel's own ten tries are spent. That, `ENXIO`
//!   and `EAGAIN` are tried again — at most [`TRIES`] times, with no wait in
//!   between: the kernel already waited, about 0.4 s a try (10 ms × 6…10), so
//!   a test is held about 2 s at worst.

use std::fmt;
use std::io;

use nix::pty::{OpenptyResult, Winsize};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

/// XNU's `EREDRIVEOPEN` (`bsd/sys/errno.h:280`), an errno macOS's kernel keeps
/// for itself and lets out of `/dev/ptmx` (t-78154). rustix hands it over raw;
/// nix would read it as `UnknownErrno`, like every value nix does not know —
/// so it is compared raw.
pub(crate) const EREDRIVEOPEN: i32 = -6;

/// The raw errnos a later try may not meet.
const TRANSIENT: [i32; 3] = [EREDRIVEOPEN, Errno::NXIO.raw_os_error(), Errno::AGAIN.raw_os_error()];

/// How many times a pair is asked for before its last failure is the answer —
/// the window's own test ptys use the same (t-77996); about 2 s at worst.
pub(crate) const TRIES: usize = 5;

/// Why a pair did not open: the raw errno of the last try, and how many tries
/// were made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenFailed {
    pub(crate) errno: i32,
    pub(crate) tries: usize,
}

impl fmt::Display for OpenFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no pty opened: raw errno {} after {} tries", self.errno, self.tries)
    }
}

impl std::error::Error for OpenFailed {}

impl From<OpenFailed> for io::Error {
    fn from(failed: OpenFailed) -> Self {
        Self::new(Self::from_raw_os_error(failed.errno).kind(), failed)
    }
}

/// What `open` opens, asked again while it fails with an errno in
/// [`TRANSIENT`] and fewer than [`TRIES`] tries were made. A failure keeps the
/// raw errno of the last try and the number of tries.
pub(crate) fn open_with<T>(mut open: impl FnMut() -> Result<T, i32>) -> Result<T, OpenFailed> {
    let mut tries = 0;
    loop {
        tries += 1;
        match open() {
            Ok(opened) => return Ok(opened),
            Err(errno) if TRANSIENT.contains(&errno) && tries < TRIES => {}
            Err(errno) => return Err(OpenFailed { errno, tries }),
        }
    }
}

/// A pty pair, both ends close-on-exec, `size` on its user end when given.
pub(crate) fn open_pty(size: Option<&Winsize>) -> Result<OpenptyResult, OpenFailed> {
    open_with(|| open_once(size).map_err(Errno::raw_os_error))
}

/// One try at a close-on-exec pair: the steps `openpty` takes, each end opened
/// with `O_CLOEXEC`.
fn open_once(size: Option<&Winsize>) -> rustix::io::Result<OpenptyResult> {
    let flags = OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC;
    let master = rustix::fs::open("/dev/ptmx", flags, Mode::empty())?;
    rustix::pty::grantpt(&master)?;
    rustix::pty::unlockpt(&master)?;
    let name = rustix::pty::ptsname(&master, Vec::new())?;
    let slave = rustix::fs::open(name.as_c_str(), flags, Mode::empty())?;
    if let Some(size) = size {
        let size = rustix::termios::Winsize {
            ws_row: size.ws_row,
            ws_col: size.ws_col,
            ws_xpixel: size.ws_xpixel,
            ws_ypixel: size.ws_ypixel,
        };
        rustix::termios::tcsetwinsize(&slave, size)?;
    }
    Ok(OpenptyResult { master, slave })
}
