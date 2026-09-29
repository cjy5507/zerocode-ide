use std::collections::HashSet;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WaitStop {
    Cancelled,
    Deadline,
}

pub(crate) struct WaitState {
    /// The wall clock somebody named for this phase and when it runs out. A
    /// phase nobody named a limit for has none: it is ended by nothing but
    /// its agents finishing, their inactivity, or a cancel (t-12076).
    wall: Option<(Duration, Instant)>,
    pub(super) started_at: u64,
    pub(super) startup_extensions: HashSet<String>,
}

impl WaitState {
    pub(super) fn new(now: Instant, wall: Option<Duration>) -> Self {
        Self {
            // A limit too long to add to the clock is no limit at all — never
            // a deadline that is already past.
            wall: wall.and_then(|limit| Some((limit, now.checked_add(limit)?))),
            started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or(0),
            startup_extensions: HashSet::new(),
        }
    }

    /// The wall clock somebody named, when one was — what a stop names.
    pub(super) fn wall(&self) -> Option<Duration> {
        self.wall.map(|(limit, _)| limit)
    }

    pub(super) fn next_slice(&self, now: Instant, cancelled: bool) -> Result<Duration, WaitStop> {
        if cancelled {
            return Err(WaitStop::Cancelled);
        }
        let slice = Duration::from_secs(2);
        let Some((_, deadline)) = self.wall else {
            return Ok(slice);
        };
        let remaining = deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            Err(WaitStop::Deadline)
        } else {
            Ok(remaining.min(slice))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observing_does_not_reset_deadline() {
        let start = Instant::now();
        let state = WaitState::new(start, Some(Duration::from_secs(5)));
        assert_eq!(state.next_slice(start, false), Ok(Duration::from_secs(2)));
        assert_eq!(
            state.next_slice(start + Duration::from_secs(4), false),
            Ok(Duration::from_secs(1))
        );
        assert_eq!(
            state.next_slice(start + Duration::from_secs(5), false),
            Err(WaitStop::Deadline)
        );
    }

    /// A phase nobody named a limit for has no deadline at all: it is up
    /// after any number of hours only if somebody stops it (t-12076).
    #[test]
    fn a_wait_with_no_named_limit_never_reaches_a_deadline() {
        let start = Instant::now();
        let state = WaitState::new(start, None);
        assert_eq!(state.wall(), None);
        assert_eq!(
            state.next_slice(start + Duration::from_secs(1000 * 3600), false),
            Ok(Duration::from_secs(2)),
            "a thousand hours in, still no wall"
        );
        assert_eq!(
            state.next_slice(start, true),
            Err(WaitStop::Cancelled),
            "a cancel still stops it"
        );
        let named = WaitState::new(start, Some(Duration::from_secs(5)));
        assert_eq!(named.wall(), Some(Duration::from_secs(5)), "a named limit is what a stop names");
        assert_eq!(
            WaitState::new(start, Some(Duration::MAX)).next_slice(start, false),
            Ok(Duration::from_secs(2)),
            "a limit too long for the clock is no limit, not one already past"
        );
    }

    #[test]
    fn cancellation_is_distinct_from_deadline() {
        let now = Instant::now();
        let state = WaitState::new(now, Some(Duration::ZERO));
        assert_eq!(state.next_slice(now, true), Err(WaitStop::Cancelled));
        assert_eq!(state.next_slice(now, false), Err(WaitStop::Deadline));
    }

    #[test]
    fn extension_is_once_per_agent_across_observations() {
        let start = Instant::now();
        let mut state = WaitState::new(start, Some(Duration::from_secs(5)));
        assert!(state.startup_extensions.insert("a".to_owned()));
        assert_eq!(state.next_slice(start + Duration::from_secs(4), false), Ok(Duration::from_secs(1)));
        assert!(!state.startup_extensions.insert("a".to_owned()));
        assert_eq!(state.next_slice(start + Duration::from_secs(5), false), Err(WaitStop::Deadline));
    }
}
