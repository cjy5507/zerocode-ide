use std::collections::HashSet;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WaitStop {
    Cancelled,
    Deadline,
}

pub(crate) struct WaitState {
    deadline: Instant,
    pub(super) started_at: u64,
    pub(super) startup_extensions: HashSet<String>,
}

impl WaitState {
    pub(super) fn new(now: Instant, timeout: Duration) -> Self {
        Self {
            deadline: now.checked_add(timeout).unwrap_or(now),
            started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or(0),
            startup_extensions: HashSet::new(),
        }
    }

    pub(super) fn next_slice(&self, now: Instant, cancelled: bool) -> Result<Duration, WaitStop> {
        if cancelled {
            return Err(WaitStop::Cancelled);
        }
        let remaining = self.deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            Err(WaitStop::Deadline)
        } else {
            Ok(remaining.min(Duration::from_secs(2)))
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observing_does_not_reset_deadline() {
        let start = Instant::now();
        let state = WaitState::new(start, Duration::from_secs(5));
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

    #[test]
    fn cancellation_is_distinct_from_deadline() {
        let now = Instant::now();
        let state = WaitState::new(now, Duration::ZERO);
        assert_eq!(state.next_slice(now, true), Err(WaitStop::Cancelled));
        assert_eq!(state.next_slice(now, false), Err(WaitStop::Deadline));
    }

    #[test]
    fn extension_is_once_per_agent_across_observations() {
        let start = Instant::now();
        let mut state = WaitState::new(start, Duration::from_secs(5));
        assert!(state.startup_extensions.insert("a".to_owned()));
        assert_eq!(state.next_slice(start + Duration::from_secs(4), false), Ok(Duration::from_secs(1)));
        assert!(!state.startup_extensions.insert("a".to_owned()));
        assert_eq!(state.next_slice(start + Duration::from_secs(5), false), Err(WaitStop::Deadline));
    }
}
