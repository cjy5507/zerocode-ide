//! The one beat of the session's periodic looks (t-17057).
//!
//! An idle zo woke 3.2 times a second in t-11961's final run (Codex 2.0,
//! Gemini 1.2): the idle loop's size-and-foreground poll, the roster watcher and
//! the completion pump's stall watch each had a one-second `interval` that ran
//! whether or not anything of the session did. A look at a roster with no
//! helper in it, or at a terminal nobody has touched, finds what the last one
//! found; what changes it — a helper starting, a task ending — is an event the
//! tools crate already tells ([`runtime::helper_activity`]).
//!
//! A [`Pulse`] is what those loops wait on instead of an interval:
//!
//! - while something of the session runs it comes once a second, and every
//!   looker's beat falls on the turn of the wall clock's second, so the timer
//!   driver wakes once for all of them instead of once for each;
//! - the moment the first thing starts it comes at once, and once more, a beat
//!   later, when the last one has ended — the look that sees it gone;
//! - while nothing runs it does not come at all (`idle_after: None`), or only
//!   after the interval a looker names for what no event tells it (a resize the
//!   window never signalled, another zo's write to the same store).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;

/// The time to the next turn of the wall clock's second — where every beat
/// falls, so that the loopers' wakes coincide.
fn until_next_beat() -> Duration {
    let into = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    Duration::from_nanos(u64::from(1_000_000_000 - into)).max(Duration::from_millis(1))
}

/// A looker's view of the beat. One per looker: each keeps what it has seen of
/// what runs, so an edge is a look for it however many others saw it too.
pub(crate) struct Pulse {
    activity: watch::Receiver<usize>,
    /// The activity as of the last time the pulse looked: something ran.
    busy: bool,
    /// Something ran and has ended, and the look that sees it gone is owed.
    settling: bool,
    /// The channel still has a sender; when it does not, no news will come and
    /// the pulse must not ask for any (a closed channel answers at once, which
    /// is a spin).
    news: bool,
}

impl Pulse {
    /// The process's beat: what its session's helpers and background tasks do.
    pub(crate) fn new() -> Self {
        Self::over(runtime::helper_activity::watch())
    }

    /// A beat over another activity — a test's.
    pub(crate) fn over(mut activity: watch::Receiver<usize>) -> Self {
        let busy = *activity.borrow_and_update() > 0;
        Self {
            activity,
            busy,
            settling: false,
            news: true,
        }
    }

    /// Wait for the next look.
    ///
    /// It comes at the next second's turn while something runs, at once when
    /// something starts, and a beat after the last thing ends. While nothing
    /// runs it comes after `idle_after`, or never when that is `None`.
    ///
    /// Cancelling the wait loses nothing: what the pulse has seen of the
    /// activity is kept in it, and the next call starts from there.
    pub(crate) async fn next(&mut self, idle_after: Option<Duration>) {
        loop {
            let running = *self.activity.borrow_and_update() > 0;
            if running && !self.busy {
                self.busy = true;
                self.settling = false;
                return;
            }
            if !running && self.busy {
                self.busy = false;
                self.settling = true;
            }
            if running || self.settling {
                tokio::select! {
                    () = tokio::time::sleep(until_next_beat()) => {
                        self.settling = false;
                        return;
                    }
                    changed = self.activity.changed(), if self.news => {
                        self.news = changed.is_ok();
                    }
                }
            } else if let Some(after) = idle_after {
                tokio::select! {
                    () = tokio::time::sleep(after) => return,
                    changed = self.activity.changed(), if self.news => {
                        self.news = changed.is_ok();
                    }
                }
            } else if self.news {
                self.news = self.activity.changed().await.is_ok();
            } else {
                std::future::pending::<()>().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use runtime::helper_activity::{Activity, Count};

    use super::{until_next_beat, Pulse};

    async fn arrives_within(pulse: &mut Pulse, idle_after: Option<Duration>, limit: Duration) -> bool {
        tokio::time::timeout(limit, pulse.next(idle_after)).await.is_ok()
    }

    /// The point of the pulse: with nothing running there is no timer to wake
    /// for, however long the session sits.
    #[tokio::test(start_paused = true)]
    async fn nothing_running_means_no_beat_at_all() {
        let activity = Activity::new();
        let mut pulse = Pulse::over(activity.watch());
        assert!(
            !arrives_within(&mut pulse, None, Duration::from_secs(3600)).await,
            "a pulse beat for an hour over a session that ran nothing"
        );
    }

    /// What no event tells — a resize the window never signalled, a write by
    /// another zo — is looked for on the interval the looker names, and not
    /// before it.
    #[tokio::test(start_paused = true)]
    async fn an_idle_look_comes_after_the_interval_it_was_asked_for() {
        let activity = Activity::new();
        let mut pulse = Pulse::over(activity.watch());
        let mut next = Box::pin(pulse.next(Some(Duration::from_secs(10))));
        assert!(
            tokio::time::timeout(Duration::from_millis(9_900), &mut next).await.is_err(),
            "the idle look came before its interval"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut next).await.is_ok(),
            "the idle look never came"
        );
    }

    /// The first thing to start is looked at now, not at the next beat, and
    /// the edge is a look for every looker, not for the first one only.
    #[tokio::test(start_paused = true)]
    async fn something_starting_is_looked_at_at_once_by_every_looker() {
        let activity = Activity::new();
        let mut first = Pulse::over(activity.watch());
        let mut second = Pulse::over(activity.watch());
        let mut waiting_first = Box::pin(first.next(None));
        let mut waiting_second = Box::pin(second.next(None));
        assert!(tokio::time::timeout(Duration::from_secs(5), &mut waiting_first).await.is_err());
        activity.set(Count::Workers, 1);
        assert!(tokio::time::timeout(Duration::from_millis(1), &mut waiting_first).await.is_ok());
        assert!(tokio::time::timeout(Duration::from_millis(1), &mut waiting_second).await.is_ok());
    }

    /// While something runs the beat comes once a second — never more often,
    /// and each one within the second.
    #[tokio::test(start_paused = true)]
    async fn while_something_runs_the_beat_comes_once_a_second() {
        let activity = Activity::new();
        activity.set(Count::Background, 1);
        let mut pulse = Pulse::over(activity.watch());
        for beat in 0..5 {
            let started = tokio::time::Instant::now();
            assert!(
                arrives_within(&mut pulse, None, Duration::from_millis(1_050)).await,
                "beat {beat} did not come within a second"
            );
            let waited = started.elapsed();
            assert!(waited <= Duration::from_millis(1_050), "beat {beat} took {waited:?}");
        }
    }

    /// The last thing ending is one look more — the look that sees it gone —
    /// and then the pulse is parked again.
    #[tokio::test(start_paused = true)]
    async fn the_last_thing_ending_is_looked_at_once_more_and_then_the_beat_stops() {
        let activity = Activity::new();
        activity.set(Count::Workers, 2);
        let mut pulse = Pulse::over(activity.watch());
        assert!(arrives_within(&mut pulse, None, Duration::from_millis(1_050)).await);
        activity.set(Count::Workers, 1);
        assert!(arrives_within(&mut pulse, None, Duration::from_millis(1_050)).await, "one still runs");
        activity.set(Count::Workers, 0);
        assert!(
            arrives_within(&mut pulse, None, Duration::from_millis(1_050)).await,
            "the look that sees the last one gone never came"
        );
        assert!(
            !arrives_within(&mut pulse, None, Duration::from_secs(600)).await,
            "the beat went on after everything had ended"
        );
    }

    /// A pulse whose activity is gone keeps its idle interval and does not
    /// spin: a closed channel answers at once, and asking again would be a hot
    /// loop.
    #[tokio::test(start_paused = true)]
    async fn a_pulse_whose_activity_is_gone_keeps_its_idle_interval() {
        let activity = Activity::new();
        let mut pulse = Pulse::over(activity.watch());
        drop(activity);
        let mut next = Box::pin(pulse.next(Some(Duration::from_secs(10))));
        assert!(tokio::time::timeout(Duration::from_millis(9_900), &mut next).await.is_err());
        assert!(tokio::time::timeout(Duration::from_millis(200), &mut next).await.is_ok());
        let mut parked = Pulse::over(runtime::helper_activity::Activity::new().watch());
        assert!(!arrives_within(&mut parked, None, Duration::from_secs(60)).await);
    }

    /// Every beat falls on the turn of a second of the wall clock, so loopers
    /// that started at different moments still wake together.
    #[test]
    fn a_beat_falls_within_the_next_second() {
        let wait = until_next_beat();
        assert!(wait > Duration::ZERO && wait <= Duration::from_secs(1), "{wait:?}");
    }
}
