//! What both frame pumps do the same way.
//!
//! iOS and Android reach their pixels through completely different machinery —
//! a framebuffer the helper already holds on one side, an `adb screencap` per
//! picture on the other — but everything AROUND the capture is the same
//! bookkeeping: is this picture the one we already sent, how long to rest after
//! one that was not, how many failures in a row before the pane is told, and
//! how fast the road is actually going. Each pump was carrying its own copy,
//! and the copies had already drifted (a six-step idle ladder on one, eight on
//! the other) before this module existed to hold the one version.

use std::hash::{Hash as _, Hasher as _};
use std::time::{Duration, Instant};

/// A cheap identity for one encoded picture.
///
/// Only ever compared against the previous fingerprint from the same road, so
/// the hash needs to be stable within one process and nothing more — no road
/// stores it, sends it, or compares it against a fingerprint from another run.
pub(super) fn frame_fingerprint(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// How long a road rests after a picture that showed nothing new.
///
/// The rest grows with each unchanged picture in a row and then stops growing,
/// so a pane nobody is touching costs one capture a second instead of thirty,
/// while a screen that starts moving is back at full rate on its first changed
/// frame. Both roads climb the same ladder; they differ only in the size of the
/// step and how many steps there are, which is why those are asked for rather
/// than written here.
#[derive(Clone, Copy)]
pub(super) struct IdleLadder {
    /// The rest after the first unchanged picture, and the size of every rung.
    pub step: Duration,
    /// The longest this ladder ever rests, however many rungs are climbed.
    pub ceiling: Duration,
    /// The rung the climb stops at.
    pub steps: u32,
}

impl IdleLadder {
    pub fn delay(&self, unchanged: u32) -> Duration {
        self.step
            .saturating_mul(unchanged.min(self.steps) + 1)
            .min(self.ceiling)
    }
}

/// Consecutive failures, and the one moment worth saying something about.
///
/// A single miss is a hiccup — a screencap that lost a race, a device that was
/// mid-rotation — and a pane that showed a note for every one of those would
/// blink constantly. The note belongs at the exact miss where the road stops
/// looking like bad luck, and `missed` answers true only on that miss so the
/// caller cannot accidentally emit one per frame afterwards.
pub(super) struct MissCounter {
    budget: u32,
    seen: u32,
}

impl MissCounter {
    pub fn new(budget: u32) -> Self {
        Self { budget, seen: 0 }
    }

    /// Count one failure. True exactly once, on the miss that spends the budget.
    pub fn missed(&mut self) -> bool {
        self.seen = self.seen.saturating_add(1);
        self.seen == self.budget
    }

    pub fn hit(&mut self) {
        self.seen = 0;
    }
}

/// How fast a road is actually delivering, measured only while it is moving.
///
/// A pane resting on an idle screen or paused behind another tab spends most of
/// its wall clock not capturing, and averaging that in makes a healthy pump
/// read like a broken one — so only the gap between two pictures DELIVERED in a
/// row is counted, and any rest, pause or change of road breaks the chain
/// rather than being folded into it.
pub(super) struct RateWatch {
    window: u32,
    delivered: u32,
    moving: Duration,
    last: Option<Instant>,
}

impl RateWatch {
    pub fn new(window: u32) -> Self {
        Self {
            window,
            delivered: 0,
            moving: Duration::ZERO,
            last: None,
        }
    }

    /// Record one delivered picture. Answers the mean milliseconds per frame
    /// once a full window of them has been seen, and nothing until then.
    pub fn delivered(&mut self) -> Option<f64> {
        let mean = if let Some(previous) = self.last {
            self.moving = self.moving.saturating_add(previous.elapsed());
            self.delivered = self.delivered.saturating_add(1);
            (self.delivered >= self.window).then(|| {
                let mean = self.moving.as_secs_f64() * 1000.0 / f64::from(self.delivered);
                self.delivered = 0;
                self.moving = Duration::ZERO;
                mean
            })
        } else {
            None
        };
        self.last = Some(Instant::now());
        mean
    }

    /// The chain is broken — the next delivered picture starts a new one.
    pub fn broke(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_idle_ladder_climbs_to_its_own_ceiling_and_stops_there() {
        let ladder = IdleLadder {
            step: Duration::from_millis(75),
            ceiling: Duration::from_millis(900),
            steps: 8,
        };
        assert_eq!(ladder.delay(0), Duration::from_millis(75));
        assert_eq!(ladder.delay(1), Duration::from_millis(150));
        assert_eq!(ladder.delay(8), Duration::from_millis(675));
        // Past the last rung the rest stops growing rather than running away.
        assert_eq!(ladder.delay(9), Duration::from_millis(675));
        assert_eq!(ladder.delay(u32::MAX), Duration::from_millis(675));
    }

    #[test]
    fn a_ladder_whose_steps_outrun_its_ceiling_is_held_to_the_ceiling() {
        let ladder = IdleLadder {
            step: Duration::from_millis(120),
            ceiling: Duration::from_millis(900),
            steps: 6,
        };
        assert_eq!(ladder.delay(0), Duration::from_millis(120));
        assert_eq!(ladder.delay(6), Duration::from_millis(840));
        assert_eq!(ladder.delay(7), Duration::from_millis(840));
    }

    #[test]
    fn a_miss_counter_speaks_once_and_a_hit_puts_the_budget_back() {
        let mut misses = MissCounter::new(2);
        assert!(!misses.missed());
        assert!(misses.missed());
        // The note belongs to the miss that spent the budget, not to every
        // miss after it.
        assert!(!misses.missed());
        misses.hit();
        assert!(!misses.missed());
        assert!(misses.missed());
    }

    #[test]
    fn the_same_bytes_fingerprint_alike_and_different_bytes_do_not() {
        assert_eq!(frame_fingerprint(b"picture"), frame_fingerprint(b"picture"));
        assert_ne!(frame_fingerprint(b"picture"), frame_fingerprint(b"another"));
    }

    #[test]
    fn a_rate_word_covers_one_window_of_frames_and_a_break_starts_over() {
        let mut rate = RateWatch::new(3);
        // The first delivery has no previous frame to measure against.
        assert!(rate.delivered().is_none());
        assert!(rate.delivered().is_none());
        assert!(rate.delivered().is_none());
        assert!(rate.delivered().is_some());
        rate.broke();
        // After a break the next delivery is a first one again, so the window
        // needs a full count of gaps before it says anything.
        assert!(rate.delivered().is_none());
        assert!(rate.delivered().is_none());
        assert!(rate.delivered().is_none());
        assert!(rate.delivered().is_some());
    }
}
