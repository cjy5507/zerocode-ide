//! The monotonic clock a bounded wait reads — and a test's way to stand it
//! still (t-8938).
//!
//! A bound set for a person's window — three seconds for one `codex queue`
//! attempt, three for a parked pointer's hook to knock — is measured on the
//! machine it runs on. A test that asserts what the waited-on thing DID, not
//! how fast a loaded machine let it do it, must not be measured that way:
//! inside a full parallel suite the bound that is right for the window runs
//! out on a test doing nothing wrong, and the red it leaves names the machine
//! rather than the code. Such a test stands this clock still on its own
//! thread, and its wait then ends with the effect it asserts.
//!
//! Thread-local on purpose: the suite runs its tests in parallel inside one
//! process, and a clock shared between them would stand every other test's
//! waits still with it.

use std::time::Instant;

/// Now, on the clock bounded waits are read against.
pub(crate) fn now() -> Instant {
    #[cfg(test)]
    if let Some(stood) = standing() {
        return stood;
    }
    Instant::now()
}

#[cfg(test)]
thread_local! {
    /// The instant this thread's waits read, and the real moment it stops
    /// standing.
    static STANDING: std::cell::Cell<Option<(Instant, Instant)>> =
        const { std::cell::Cell::new(None) };
}

/// How long a stood clock stands before it runs again.
///
/// Not a bound any test measures — a passing run never comes near it. It is
/// what keeps a wait whose effect never comes from hanging the suite: past
/// it the clock runs, the production bound passes, and the test fails on its
/// own assertion with the machine's answer in hand.
#[cfg(test)]
const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(60);

#[cfg(test)]
fn standing() -> Option<Instant> {
    STANDING
        .with(std::cell::Cell::get)
        .and_then(|(stood, until)| (Instant::now() < until).then_some(stood))
}

/// This thread's bounded waits read one instant until this is dropped.
#[cfg(test)]
pub(crate) struct Stood(());

/// Stand this thread's clock still at the present instant.
#[cfg(test)]
pub(crate) fn stand_still() -> Stood {
    let stood = Instant::now();
    STANDING.with(|held| held.set(Some((stood, stood + HANG_GUARD))));
    Stood(())
}

#[cfg(test)]
impl Drop for Stood {
    fn drop(&mut self) {
        STANDING.with(|held| held.set(None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stood clock reads one instant on its own thread and nowhere else,
    /// and runs again once it is let go.
    #[test]
    fn a_stood_clock_stands_on_its_own_thread_only() {
        let stood = stand_still();
        let first = now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(now(), first, "a stood clock moved");
        let elsewhere = std::thread::spawn(now).join().expect("another thread");
        assert!(elsewhere > first, "another thread's clock stood still too");
        drop(stood);
        assert!(now() > first, "a clock let go kept standing");
    }
}
