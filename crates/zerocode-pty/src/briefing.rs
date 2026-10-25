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
    ///
    /// A briefing not yet on the screen is waited for the whole window — a
    /// program still booting answers its Enter late, up to 9.2 s in the
    /// measured worst case — and only then typed again, once; a second empty
    /// window is the verdict.
    pub fn poll(&mut self, shown: bool, now: Instant) -> WatchStep {
        if shown {
            return WatchStep::Taken;
        }
        if now.saturating_duration_since(self.since) < self.window {
            return WatchStep::Waiting;
        }
        if self.retyped {
            return WatchStep::Undelivered;
        }
        self.retyped = true;
        self.since = now;
        WatchStep::Retype
    }
}

/// How many of the briefing's own characters are looked for on the screen.
const PROBE_CHARS: usize = 24;

/// Whether `screen` — the pane's scrollback and visible text — shows `briefing`.
///
/// Three things say a briefing was kept: the head of its words, which every
/// agent echoes into its history or leaves in its composer; or the
/// placeholder a long paste is folded to (`[Pasted text #1 +37 lines]`,
/// `[Pasted Content 6021 chars]`). Whitespace and the box-drawing bars a
/// composer wraps its text in are ignored on both sides, so a head that the
/// pane wrapped across rows is still found. A briefing with no visible
/// characters is nothing to look for, and shows.
pub fn briefing_shows(screen: &str, briefing: &str) -> bool {
    let bare = |text: &str| -> String {
        text.chars()
            .filter(|c| !c.is_whitespace() && !c.is_control() && !"│┃|".contains(*c))
            .collect()
    };
    let probe: String = bare(briefing).chars().take(PROBE_CHARS).collect();
    if probe.is_empty() {
        return true;
    }
    let seen = bare(screen);
    seen.contains(&probe) || seen.to_lowercase().contains("[pasted")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BRIEFING: &str = "You are a worker in this window's orchestration, carrying task X";

    #[test]
    fn an_empty_composer_shows_no_briefing() {
        assert!(!briefing_shows("Claude Code v2\n\n❯ \n", BRIEFING));
        assert!(!briefing_shows("", BRIEFING));
    }

    #[test]
    fn the_head_of_the_words_shows_it_wherever_the_pane_wrapped_them() {
        assert!(briefing_shows(
            "❯ You are a worker in this window's orchestration",
            BRIEFING
        ));
        // Wrapped across rows and wrapped in a composer's box.
        assert!(briefing_shows(
            "│ You are a worker in\n│ this window's orchestra\n",
            BRIEFING
        ));
    }

    #[test]
    fn a_long_paste_folded_to_a_placeholder_shows_it() {
        assert!(briefing_shows("❯ [Pasted text #1 +37 lines]", BRIEFING));
        assert!(briefing_shows("› [Pasted Content 6021 chars]", BRIEFING));
    }

    #[test]
    fn a_briefing_with_nothing_to_see_shows_on_any_screen() {
        assert!(briefing_shows("❯ ", "  \n\t"));
    }

    #[test]
    fn the_watch_waits_its_window_types_again_once_and_then_gives_up() {
        let start = Instant::now();
        let window = Duration::from_secs(6);
        let mut watch = BriefingWatch::new(start, window);
        assert_eq!(
            watch.poll(false, start + Duration::from_secs(5)),
            WatchStep::Waiting
        );
        assert_eq!(watch.poll(false, start + window), WatchStep::Retype);
        // The second window counts from the retype.
        assert_eq!(
            watch.poll(false, start + window + Duration::from_secs(5)),
            WatchStep::Waiting
        );
        assert_eq!(
            watch.poll(false, start + window * 2),
            WatchStep::Undelivered
        );
    }

    #[test]
    fn a_briefing_that_shows_ends_the_watch_at_once() {
        let start = Instant::now();
        let mut watch = BriefingWatch::new(start, Duration::from_secs(6));
        assert_eq!(watch.poll(true, start), WatchStep::Taken);
    }
}
