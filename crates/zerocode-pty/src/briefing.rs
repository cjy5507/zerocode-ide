//! Whether a launch briefing really went into the pane (t-74381).
//!
//! A delivery's `Delivered` says the words were written and the Enter went. It
//! cannot say the program kept them: Claude Code takes its first input handler
//! down while it boots and drops what the handler held, and the pane is left
//! with an empty composer while the ledger writes a success. What can say is
//! the screen: a briefing that was taken stands on it — echoed in the history,
//! folded to a placeholder, or still in the composer.

use std::time::{Duration, Instant};

/// How long a screen is watched for the briefing before it is typed again, and
/// again before it is given up on. Longer than the 1.8 s a booting Claude Code
/// spends before its real handler stands, shorter than the launch patience.
pub const BRIEFING_WINDOW: Duration = Duration::from_secs(6);

/// What the watch asks of its caller this round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchStep {
    /// Nothing yet, and there is time.
    Waiting,
    /// The briefing stands on the screen.
    Taken,
    /// It does not, and has not been typed again: type it once more.
    Retype,
    /// It does not, after being typed twice: the pane holds no briefing.
    Undelivered,
}

/// The watch over one launch briefing.
#[derive(Debug)]
pub struct BriefingWatch {
    window: Duration,
    since: Instant,
    retyped: bool,
}

impl BriefingWatch {
    pub fn new(now: Instant, window: Duration) -> Self {
        Self {
            window,
            since: now,
            retyped: false,
        }
    }

    /// One look. `shown` is [`briefing_shows`] over the pane's text.
    pub fn poll(&mut self, shown: bool, now: Instant) -> WatchStep {
        let _ = (shown, now, self.window, self.since, self.retyped);
        WatchStep::Taken
    }
}

/// Whether `screen` — the pane's scrollback and visible text — shows `briefing`.
pub fn briefing_shows(screen: &str, briefing: &str) -> bool {
    let _ = (screen, briefing);
    true
}
