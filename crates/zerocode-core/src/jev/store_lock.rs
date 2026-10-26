//! The exclusive lock a Jev store's writer holds, ended by its guard rather
//! than by the last copy of its descriptor.
//!
//! A `flock` belongs to the open file description, not to one descriptor. A
//! process launch on another thread that forks before it execs — std's
//! `Command` does whenever it sets `PATH` and names a bare program — holds a
//! copy of every descriptor until the exec, so closing ours can leave the lock
//! alive in that copy, and the next writer waits out its bound and is refused
//! (t-66257). Unlocking ends it for every copy. The review store's lock does
//! the same ([`super::learning`]).

use std::fs::File;
use std::ops::Deref;

/// A locked file whose lock ends when this drops.
#[derive(Debug)]
pub(crate) struct StoreLock(File);

impl StoreLock {
    /// Holds `file`, which the caller has just locked.
    pub(crate) fn held(file: File) -> Self {
        Self(file)
    }
}

impl Deref for StoreLock {
    type Target = File;

    fn deref(&self) -> &File {
        &self.0
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
