//! No-progress / stall detection for long-horizon loops (ALP §3, generalized).
//!
//! A loop that fails for the *same reason* two turns running is wasting budget —
//! the next repair would almost certainly reproduce the same failure. This
//! tracker folds each failing turn's failure signature into a streak and reports
//! a stall once the streak crosses a threshold, so the loop can stop honestly
//! instead of burning the rest of its turns.
//!
//! The signature MUST be derived from the *failure set* (e.g. the validator
//! failures), NOT a diff/file hash: a diff hash is identical across a
//! revert→reapply and different on every cosmetic edit, both of which produce
//! false stalls. Pure and serializable.

use serde::{Deserialize, Serialize};

/// Default streak length (identical failures in a row) that counts as a stall.
pub const STALL_THRESHOLD: u32 = 2;

/// Per-loop progress state. Serializable so a resumed loop keeps its streak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProgressTracker {
    last: Option<u64>,
    identical_streak: u32,
}

/// The verdict for one observed failing turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// The loop is still making progress (a new/different failure).
    Advancing,
    /// The loop has repeated the same failure `STALL_THRESHOLD` times.
    Stalled(StallKind),
}

/// Why a loop is considered stalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallKind {
    /// The same failure signature repeated without resolution.
    RepeatedFailure,
}

impl ProgressTracker {
    /// Fold one *failing* turn's [`failure_signature`] into the streak. Call this
    /// only for turns that did not pass — a different signature (a new failure)
    /// resets the streak; the same signature `STALL_THRESHOLD` times in a row is
    /// a stall.
    pub fn observe(&mut self, signature: u64) -> Progress {
        if self.last == Some(signature) {
            self.identical_streak = self.identical_streak.saturating_add(1);
        } else {
            self.last = Some(signature);
            self.identical_streak = 1;
        }
        if self.identical_streak >= STALL_THRESHOLD {
            Progress::Stalled(StallKind::RepeatedFailure)
        } else {
            Progress::Advancing
        }
    }
}

/// Monotone objective-criteria progress: how many of the goal's objective
/// checks have EVER passed at once. The stall/block trackers only see failure
/// shapes; this is the one *positive* progress signal — a turn that newly
/// passes a criterion is demonstrably not stuck, so the caller may reset those
/// streaks. Deliberately narrow: only a decidable check flipping green counts
/// (research or re-planning "feels like progress" without bound; the observed
/// runaway proved it), and only a new BEST counts (oscillating between which
/// checks pass is not progress).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CriteriaProgress {
    best_passed: u32,
}

impl CriteriaProgress {
    /// Fold one turn's objective-check outcome (`passed` of `total` green).
    /// Returns `true` only when `passed` sets a new all-time best — genuine
    /// forward movement on the goal's own success criteria.
    pub fn observe(&mut self, passed: u32) -> bool {
        if passed > self.best_passed {
            self.best_passed = passed;
            true
        } else {
            false
        }
    }

    /// Best simultaneous pass count so far (for digests/tests).
    #[must_use]
    pub const fn best(&self) -> u32 {
        self.best_passed
    }
}

/// FNV-1a hash of a failure set, order-independent. Callers pass the validator
/// failure strings; we normalize (trim, collapse internal whitespace, lowercase,
/// drop blanks, sort, dedup) so cosmetic reordering/reformatting of the same
/// failures hashes equal while genuinely different failures do not.
#[must_use]
pub fn failure_signature(failures: &[String]) -> u64 {
    let mut normalized: Vec<String> = failures
        .iter()
        .map(|failure| {
            failure
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase()
        })
        .filter(|failure| !failure.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for item in &normalized {
        for byte in item.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // Separator so ["ab", "c"] and ["a", "bc"] do not collide.
        hash ^= u64::from(b'\n');
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_failure_stalls_on_second_identical_observation() {
        let mut tracker = ProgressTracker::default();
        let sig = failure_signature(&["cargo test failed: foo".to_string()]);
        assert_eq!(tracker.observe(sig), Progress::Advancing, "first failure");
        assert_eq!(
            tracker.observe(sig),
            Progress::Stalled(StallKind::RepeatedFailure),
            "same failure twice → stalled"
        );
    }

    #[test]
    fn changing_failures_never_stall() {
        let mut tracker = ProgressTracker::default();
        let a = failure_signature(&["error A".to_string()]);
        let b = failure_signature(&["error B".to_string()]);
        assert_eq!(tracker.observe(a), Progress::Advancing);
        assert_eq!(tracker.observe(b), Progress::Advancing, "different failure resets");
        assert_eq!(tracker.observe(a), Progress::Advancing, "changed again");
    }

    #[test]
    fn signature_is_order_and_whitespace_independent() {
        let one = failure_signature(&[
            "  Cargo   Test   FAILED  ".to_string(),
            "clippy: unused".to_string(),
        ]);
        let two = failure_signature(&[
            "clippy:   unused".to_string(),
            "cargo test failed".to_string(),
        ]);
        assert_eq!(one, two, "reordered + reformatted same failures hash equal");
    }

    #[test]
    fn distinct_failures_have_distinct_signatures() {
        assert_ne!(
            failure_signature(&["a".to_string()]),
            failure_signature(&["b".to_string()])
        );
        // Boundary: concatenation must not collide thanks to the separator.
        assert_ne!(
            failure_signature(&["ab".to_string(), "c".to_string()]),
            failure_signature(&["a".to_string(), "bc".to_string()])
        );
    }

    #[test]
    fn tracker_roundtrips_through_serde() {
        let mut tracker = ProgressTracker::default();
        tracker.observe(7);
        let json = serde_json::to_string(&tracker).expect("serialize");
        let back: ProgressTracker = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(tracker, back);
    }

    #[test]
    fn criteria_progress_counts_only_new_bests() {
        let mut criteria = CriteriaProgress::default();
        assert!(criteria.observe(1), "first pass is a new best");
        assert!(!criteria.observe(1), "same count is not progress");
        assert!(!criteria.observe(0), "regression is not progress");
        assert!(criteria.observe(3), "a higher count is progress again");
        assert!(
            !criteria.observe(2),
            "oscillating back below the best is not progress"
        );
        assert_eq!(criteria.best(), 3);
    }
}
