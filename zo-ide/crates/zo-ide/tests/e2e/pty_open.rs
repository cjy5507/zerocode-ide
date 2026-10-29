//! Opening a pty pair for a test — the one place the lib's `#[cfg(test)]` code
//! and the e2e harness open one (two crates, this one file through `#[path]`).

use std::fmt;

use nix::errno::Errno;
use nix::pty::{OpenptyResult, Winsize};

/// XNU's `EREDRIVEOPEN` (`bsd/sys/errno.h:280`), an errno macOS's kernel keeps
/// for itself and lets out of `/dev/ptmx` (t-78154). nix has no name for it —
/// it reads as `UnknownErrno`, like every value nix does not know — so it is
/// compared raw.
pub(crate) const EREDRIVEOPEN: i32 = -6;

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

/// What `open` opens, asked once; a failure keeps the raw errno `open` saw.
pub(crate) fn open_with<T>(mut open: impl FnMut() -> Result<T, i32>) -> Result<T, OpenFailed> {
    open().map_err(|errno| OpenFailed { errno, tries: 1 })
}

/// A pty pair, `size` on its user end when given.
pub(crate) fn open_pty(size: Option<&Winsize>) -> Result<OpenptyResult, OpenFailed> {
    open_with(|| {
        nix::pty::openpty(size, None::<&nix::sys::termios::Termios>).map_err(|_| Errno::last_raw())
    })
}
