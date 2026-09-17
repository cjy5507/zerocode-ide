//! One background scan, its answer, and the flag that says it is running.
//!
//! Three ledgers are read this way — Claude's transcripts, Codex's rollouts,
//! OpenCode's database — and each held the same three things: a `Mutex` with
//! the last answer, an `AtomicBool` saying a scan is in flight, and a `Drop`
//! guard to clear that flag however the scan ends. Written three times, that
//! guard is three chances to forget it, and forgetting it leaves a pane saying
//! "reading…" for the rest of the session with every later ask refused.
//!
//! So it is written once here, and each ledger keeps only the walk that is
//! actually its own.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The last answer of one background scan.
pub struct ScanCell<T> {
    held: Mutex<Option<T>>,
    running: AtomicBool,
}

impl<T: Clone + Send + 'static> ScanCell<T> {
    /// An empty cell, for a `static`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            held: Mutex::new(None),
            running: AtomicBool::new(false),
        }
    }

    /// The held answer, kept so a settings pane opens on a number rather than
    /// on a spinner.
    #[must_use]
    pub fn held(&self) -> Option<T> {
        self.held.lock().ok().and_then(|held| held.clone())
    }

    /// Whether a scan is running right now.
    #[must_use]
    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Runs `walk` on its own thread unless a scan is already running, and
    /// answers whether this call started one.
    ///
    /// The answer matters: it is what lets the pane say "reading…" honestly
    /// rather than guessing, and what stops a second ask from starting a
    /// second walk over the same thousands of files.
    pub fn start(&'static self, walk: impl FnOnce() -> T + Send + 'static) -> bool {
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        std::thread::spawn(move || {
            // Dropped however the walk ends, including on a panic parsing
            // somebody else's file format.
            let _flag = Running(&self.running);
            let answer = walk();
            if let Ok(mut held) = self.held.lock() {
                *held = Some(answer);
            }
        });
        true
    }
}

/// Clears the in-flight flag when the thread leaves, panic or not.
struct Running<'a>(&'a AtomicBool);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static CELL: ScanCell<u32> = ScanCell::new();

    /// A scan runs, is held, and a second one cannot start while the first is
    /// in flight.
    #[test]
    fn one_walk_at_a_time_and_the_answer_is_kept() {
        assert!(CELL.held().is_none());
        assert!(!CELL.running());

        let started = CELL.start(|| {
            std::thread::sleep(std::time::Duration::from_millis(150));
            7
        });
        assert!(started);
        assert!(
            CELL.running(),
            "the flag was not set by the caller's thread"
        );
        assert!(
            !CELL.start(|| 9),
            "a second walk started over the top of the first"
        );

        for _ in 0..100 {
            if CELL.held().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(CELL.held(), Some(7));
        assert!(!CELL.running(), "the in-flight flag outlived the walk");
    }

    static PANICS: ScanCell<u32> = ScanCell::new();

    /// A walk that panics still clears the flag, so the pane recovers.
    #[test]
    fn a_panicking_walk_does_not_wedge_the_flag() {
        assert!(PANICS.start(|| panic!("the file format changed")));
        for _ in 0..100 {
            if !PANICS.running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!PANICS.running(), "a panic left the scan marked as running");
        assert!(PANICS.held().is_none(), "a panic recorded an answer");
        assert!(PANICS.start(|| 3), "the cell never accepted another walk");
    }
}
