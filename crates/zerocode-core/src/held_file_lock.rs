//! A file lock that ends with its guard, not with its last descriptor.
//!
//! A lock taken with `File::lock` or `File::try_lock` belongs to the open
//! file, and every copy of the descriptor shares it: a close ends it only
//! when no copy is left. A process launch on another thread that forks keeps
//! a copy of every descriptor until its exec, and one that stands before its
//! exec keeps it for good — measured 2026-10-25 04:01 (t-73907), when a
//! window's stuck launch kept the settings lock and three new windows waited
//! on it. [`HeldFileLock`] ends the lock itself when it is dropped, which
//! frees it for every copy at once.

use std::fs::File;

/// A file whose lock its holder took, ended when this is dropped.
#[derive(Debug)]
#[must_use = "the lock ends as soon as this is dropped"]
pub struct HeldFileLock(File);

impl HeldFileLock {
    /// `file`, whose lock the caller has just taken.
    pub fn locked(file: File) -> Self {
        Self(file)
    }
}

impl std::ops::Deref for HeldFileLock {
    type Target = File;

    fn deref(&self) -> &File {
        &self.0
    }
}

impl Drop for HeldFileLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// The copy a forked launch would keep: the lock ends with the guard all
    /// the same, and a bare close of the same kind does not end it.
    #[test]
    fn the_lock_ends_with_the_guard_while_a_copy_of_its_descriptor_lives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".lock");
        let open = || {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
                .unwrap()
        };

        let closed = open();
        closed.lock().unwrap();
        let kept = closed.try_clone().unwrap();
        drop(closed);
        assert!(
            open().try_lock().is_err(),
            "the control: a close leaves the lock to the copy"
        );
        drop(kept);

        let held = HeldFileLock::locked({
            let file = open();
            file.lock().unwrap();
            file
        });
        let kept = held.try_clone().unwrap();
        drop(held);
        let taken = open().try_lock();
        drop(kept);
        assert!(taken.is_ok(), "a copy of the descriptor kept the lock");
    }
}
