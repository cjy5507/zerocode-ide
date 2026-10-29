//! Memory curation on the existing session idle clock; no model turn or daemon.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;


pub(crate) struct IdleDreamer {
    enabled: bool,
    delay_ms: u64,
    idle_after: u64,
    next_attempt: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    failures: Arc<AtomicU32>,
}

pub(crate) struct DreamAttempt {
    next_attempt: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    failures: Arc<AtomicU32>,
}

impl IdleDreamer {
    pub(crate) fn new(enabled: bool, now: u64, delay_ms: u64) -> Self {
        Self {
            enabled,
            delay_ms: delay_ms.max(1),
            idle_after: now.saturating_add(delay_ms.max(1)),
            next_attempt: Arc::new(AtomicU64::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            failures: Arc::new(AtomicU32::new(0)),
        }
    }

    pub(crate) fn activity(&mut self, now: u64) {
        self.idle_after = now.saturating_add(self.delay_ms);
    }

    pub(crate) fn next_wakeup(&self, may_write: bool) -> Option<u64> {
        (self.enabled && may_write).then(|| {
            self.idle_after.max(self.next_attempt.load(Ordering::Acquire))
        })
    }

    pub(crate) fn begin(&self, now: u64, may_write: bool) -> Option<DreamAttempt> {
        if self.next_wakeup(may_write).is_none_or(|due| due > now) {
            return None;
        }
        // Advance before either spawning or observing a still-running worker.
        // Otherwise a due timestamp would spin the shared idle select loop.
        self.next_attempt.store(now.saturating_add(self.delay_ms), Ordering::Release);
        self.running.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).ok()?;
        Some(DreamAttempt {
            next_attempt: Arc::clone(&self.next_attempt),
            running: Arc::clone(&self.running),
            failures: Arc::clone(&self.failures),
        })
    }
}

impl DreamAttempt {
    pub(crate) fn completed(&self, now: u64) {
        self.failures.store(0, Ordering::Release);
        let interval = runtime::memory::DEFAULT_AUTO_DREAM_INTERVAL.as_millis();
        let interval = u64::try_from(interval).unwrap_or(u64::MAX);
        self.next_attempt.store(now.saturating_add(interval), Ordering::Release);
    }

    /// A pass that failed looks again later each time it fails in a row, on
    /// the runtime's own retry steps, and never sooner than the idle delay.
    pub(crate) fn retry_later(&self, now: u64) {
        let failures = self.failures.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        let wait = runtime::memory::auto_dream_retry_interval(failures).as_millis();
        let wait = u64::try_from(wait).unwrap_or(u64::MAX);
        self.next_attempt.fetch_max(now.saturating_add(wait), Ordering::AcqRel);
    }

    pub(crate) fn not_due_until(&self, _until: u64, _now: u64) {}

    pub(crate) fn awaits_activity(&self) {}
}

impl Drop for DreamAttempt {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_enabled_writable_idle_session_becomes_due() {
        let mut clock = IdleDreamer::new(true, 0, 60_000);
        assert!(clock.begin(59_999, true).is_none());
        clock.activity(40_000);
        assert_eq!(clock.next_wakeup(true), Some(100_000));
        assert!(clock.begin(100_000, false).is_none());
        assert!(clock.begin(100_000, true).is_some());
        assert!(IdleDreamer::new(false, 0, 60_000).next_wakeup(true).is_none());
    }

    #[test]
    fn a_pending_or_failed_pass_never_spins_or_overlaps() {
        let clock = IdleDreamer::new(true, 0, 60_000);
        let attempt = clock.begin(60_000, true).unwrap();
        assert_eq!(clock.next_wakeup(true), Some(120_000));
        assert!(clock.begin(120_000, true).is_none());
        assert_eq!(clock.next_wakeup(true), Some(180_000));
        drop(attempt);
        assert!(clock.begin(180_000, true).is_some());
    }

    #[test]
    fn successful_curation_preserves_its_cooldown_across_more_activity() {
        let mut clock = IdleDreamer::new(true, 0, 60_000);
        let attempt = clock.begin(60_000, true).unwrap();
        attempt.completed(61_000);
        drop(attempt);
        clock.activity(70_000);
        assert_eq!(clock.next_wakeup(true), Some(61_000 + 6 * 60 * 60 * 1000));
        assert!(clock.begin(131_000, true).is_none());
    }

    #[test]
    fn a_failing_pass_waits_longer_each_time_until_one_succeeds() {
        let minute = 60_000;
        let clock = IdleDreamer::new(true, 0, minute);
        let mut now = minute;
        for wait in [1, 2, 4, 8, 16, 32, 60, 60].map(|minutes| minutes * minute) {
            let attempt = clock.begin(now, true).unwrap();
            attempt.retry_later(now);
            drop(attempt);
            assert_eq!(clock.next_wakeup(true), Some(now + wait));
            now += wait;
        }

        let attempt = clock.begin(now, true).unwrap();
        attempt.completed(now);
        drop(attempt);
        now = clock.next_wakeup(true).unwrap();
        let attempt = clock.begin(now, true).unwrap();
        attempt.retry_later(now);
        drop(attempt);
        assert_eq!(clock.next_wakeup(true), Some(now + minute));
    }

    #[test]
    fn a_pass_the_shared_marker_holds_sleeps_until_it_lets_go_but_never_past_one_interval() {
        let minute = 60_000;
        let interval = 6 * 60 * minute;
        let clock = IdleDreamer::new(true, 0, minute);
        let attempt = clock.begin(minute, true).unwrap();
        attempt.not_due_until(minute + interval - 5_000, minute);
        drop(attempt);
        assert_eq!(clock.next_wakeup(true), Some(minute + interval - 5_000));

        let now = minute + interval - 5_000;
        let attempt = clock.begin(now, true).unwrap();
        attempt.not_due_until(u64::MAX, now);
        drop(attempt);
        assert_eq!(clock.next_wakeup(true), Some(now + interval));
    }

    #[test]
    fn a_pass_with_nothing_to_curate_waits_for_the_next_turn_even_one_that_began_during_it() {
        let minute = 60_000;
        let mut clock = IdleDreamer::new(true, 0, minute);
        let attempt = clock.begin(minute, true).unwrap();
        attempt.awaits_activity();
        drop(attempt);
        assert_eq!(clock.next_wakeup(true), None);
        assert!(clock.begin(100 * minute, true).is_none());
        clock.activity(100 * minute);
        assert_eq!(clock.next_wakeup(true), Some(101 * minute));

        let attempt = clock.begin(101 * minute, true).unwrap();
        clock.activity(101 * minute + 1);
        attempt.awaits_activity();
        drop(attempt);
        assert_eq!(clock.next_wakeup(true), Some(102 * minute + 1));
    }
}
