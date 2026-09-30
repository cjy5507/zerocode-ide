//! What of the session is running, told to whoever would look at it once a
//! second (t-17057).
//!
//! An idle zo woke 3.2 times a second in t-11961's final run, and three of
//! those wakes were looks that found nothing: the idle loop's size poll, the
//! roster watcher and the completion pump's stall watch, each on a timer of one
//! second that ran whether or not anything of the session did.
//!
//! Whether anything runs is a count the tools crate already keeps in two
//! places — the helper workers that are alive, and the background marks (the
//! helpers and shell tasks whose end is to be brought back to the main
//! conversation) — and it tells this module each time either changes, from
//! under the lock that owns it, so two changes cannot arrive out of order. A
//! looker reads the sum through [`watch()`]: a channel it can sleep on until the
//! sum is above zero, instead of asking on a timer.

use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;

/// Which of the two counts a change is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    /// Helper workers that are alive: an in-process helper's thread, a pane
    /// child's watcher.
    Workers,
    /// Background marks: the helpers and shell tasks the completion pump
    /// brings back when they end.
    Background,
}

/// The counts and their sum.
#[derive(Debug)]
pub struct Activity {
    counts: Mutex<[usize; 2]>,
    sum: watch::Sender<usize>,
}

impl Default for Activity {
    fn default() -> Self {
        Self::new()
    }
}

impl Activity {
    #[must_use]
    pub fn new() -> Self {
        Self {
            counts: Mutex::new([0, 0]),
            sum: watch::channel(0).0,
        }
    }

    /// Say how many of `count` there are now.
    ///
    /// The caller holds the lock that owns the thing counted, and the sum is
    /// published under this one: a change made later is a change told later,
    /// so the last word said is the one a looker ends up with.
    pub fn set(&self, count: Count, now: usize) {
        let mut counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        counts[count as usize] = now;
        self.sum.send_replace(counts[0] + counts[1]);
    }

    /// A receiver of the sum, above zero while anything runs.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<usize> {
        self.sum.subscribe()
    }
}

/// The time to the next turn of the wall clock's second — where the session's
/// once-a-second looks fall, so that everything that looks once a second wakes
/// together and the timer driver wakes once for all of it. The idle loop's pulse
/// (in the zo-ide crate) and a pane child's wait both rest until it.
#[must_use]
pub fn until_next_beat() -> Duration {
    let into = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    Duration::from_nanos(u64::from(1_000_000_000 - into)).max(Duration::from_millis(1))
}

static PROCESS: OnceLock<Activity> = OnceLock::new();

/// This process's activity: what its session's helpers and background tasks are
/// doing, whichever host asks.
pub fn process() -> &'static Activity {
    PROCESS.get_or_init(Activity::new)
}

/// [`Activity::set`] on this process's.
pub fn set(count: Count, now: usize) {
    process().set(count, now);
}

/// [`Activity::watch`] on this process's.
#[must_use]
pub fn watch() -> watch::Receiver<usize> {
    process().watch()
}

#[cfg(test)]
mod tests {
    use super::{Activity, Count};

    /// The sum of the two counts, and nothing runs at zero.
    #[test]
    fn what_runs_is_the_sum_of_the_workers_and_the_background_marks() {
        let activity = Activity::new();
        let seen = activity.watch();
        assert_eq!(*seen.borrow(), 0, "a session that has started nothing has nothing running");
        activity.set(Count::Workers, 2);
        assert_eq!(*seen.borrow(), 2);
        activity.set(Count::Background, 1);
        assert_eq!(*seen.borrow(), 3, "a background task runs beside the workers");
        activity.set(Count::Workers, 0);
        assert_eq!(*seen.borrow(), 1, "the last worker ending leaves the background task");
        activity.set(Count::Background, 0);
        assert_eq!(*seen.borrow(), 0);
    }

    /// A looker asleep on the channel wakes when the first thing starts and
    /// again when the last one ends — the two edges it has to look at.
    #[tokio::test]
    async fn a_looker_is_woken_by_the_first_start_and_by_the_last_end() {
        let activity = Activity::new();
        let mut seen = activity.watch();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), seen.changed())
                .await
                .is_err(),
            "a looker woke though nothing had happened"
        );
        activity.set(Count::Workers, 1);
        seen.changed().await.expect("woken by the start");
        assert_eq!(*seen.borrow_and_update(), 1);
        activity.set(Count::Workers, 0);
        seen.changed().await.expect("woken by the end");
        assert_eq!(*seen.borrow_and_update(), 0);
    }

    /// Every once-a-second look falls on the turn of a second of the wall clock,
    /// so lookers that started at different moments still wake together.
    #[test]
    fn a_beat_falls_within_the_next_second() {
        let wait = super::until_next_beat();
        assert!(
            wait > std::time::Duration::ZERO && wait <= std::time::Duration::from_secs(1),
            "{wait:?}"
        );
    }

    /// A late joiner reads what is running now, not what happened before it
    /// came.
    #[test]
    fn a_looker_that_comes_late_sees_what_runs_now() {
        let activity = Activity::new();
        activity.set(Count::Background, 3);
        assert_eq!(*activity.watch().borrow(), 3);
    }
}
