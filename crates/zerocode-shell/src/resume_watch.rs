//! Sleep's far edge, heard without the OS announcing it (P0-17 잔여).
//!
//! Orca rides Electron's `powerMonitor`: resume sends `system:resumed` to
//! every window, and a suspend of a minute or more leaves a breadcrumb
//! (`system-resume-broadcast.ts:28,55-66`). Tauri has no power monitor, so
//! this is the honest cross-platform approximation — the same stance the
//! pump's idle clock takes on event-driven I/O: a watcher naps a fixed
//! [`POLL`] and compares the wall clock's account of that nap against the
//! nap itself. Only a machine that stopped running opens a gap of a minute
//! inside the shared watchdog cadence. The arithmetic is `std::time` alone, so
//! Windows and Linux walk the identical road.
//!
//! What resumes ride on it: the window re-checks its SSH links (Orca's own
//! resume handler reconnects every active target, `ipc/ssh.ts` onResume) —
//! the "복귀 후 SSH 죽은 채 방치" half of P0-17's original complaint.

use std::time::Duration;

/// The existing watcher now also carries main-thread pings. One cadence from
/// the crash table serves both judgments; suspend still uses its minute floor.
pub const POLL: Duration = Duration::from_millis(crate::crash::Limits::DEFAULT.ping_ms);

/// Orca's floor, to the millisecond (`MIN_REPORTABLE_SUSPEND_MS = 60_000`,
/// system-resume-broadcast.ts:28): gaps shorter than a minute only delay a
/// heartbeat and explain nothing.
pub const MIN_REPORTABLE_SUSPEND: Duration = Duration::from_millis(60_000);

/// The judgment alone: a nap that the wall clock says lasted `wall_delta`
/// slept for the overshoot, when that overshoot clears the floor. Scheduler
/// jitter is seconds at worst; the floor is a minute — the gap between the
/// two is what keeps this from ever crying wolf on a busy machine.
#[must_use]
pub fn slept_for(nap: Duration, wall_delta: Duration) -> Option<Duration> {
    let over = wall_delta.checked_sub(nap)?;
    (over >= MIN_REPORTABLE_SUSPEND).then_some(over)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The floor is Orca's own number, and the judgment sits exactly on it.
    #[test]
    fn a_minute_of_missing_time_is_a_sleep_and_a_second_is_jitter() {
        assert_eq!(MIN_REPORTABLE_SUSPEND, Duration::from_millis(60_000));
        // Jitter: the wall clock agrees with the nap, give or take seconds.
        assert_eq!(slept_for(POLL, POLL), None);
        assert_eq!(slept_for(POLL, POLL + Duration::from_secs(5)), None);
        // A clock that ran BACKWARDS is ntp, not sleep.
        assert_eq!(slept_for(POLL, Duration::from_secs(1)), None);
        // The floor itself, and past it: the overshoot is the slept span.
        assert_eq!(
            slept_for(POLL, POLL + MIN_REPORTABLE_SUSPEND),
            Some(MIN_REPORTABLE_SUSPEND)
        );
        assert_eq!(
            slept_for(POLL, POLL + Duration::from_secs(3600)),
            Some(Duration::from_secs(3600))
        );
    }
}
