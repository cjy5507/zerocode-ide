//! Whether a launch briefing really went into the pane (t-74381).
//!
//! A delivery's `Delivered` says the words were written and the Enter went. It
//! cannot say the program kept them: Claude Code takes its first input handler
//! down while it boots and drops what the handler held, and the pane is left
//! with an empty composer while the ledger writes a success. What can say is
//! the screen: a briefing that was taken stands on it — echoed in the history,
//! folded to a placeholder, or still in the composer.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::ready::Outcome;

/// How long a screen is watched for the briefing before it is typed again, and
/// again — from the moment the retype's own delivery says it was submitted —
/// before it is given up on. Longer than the 1.8 s a booting Claude Code
/// spends before its real handler stands, shorter than the launch patience.
pub const BRIEFING_WINDOW: Duration = Duration::from_secs(6);

/// What the watch asks of its caller this round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchStep {
    /// Nothing yet, and there is time.
    Waiting,
    /// The briefing stands on the screen.
    Taken,
    /// It does not, and has not been typed again: type it once more, and
    /// hand that delivery's outcome to [`BriefingWatch::retype_settled`].
    Retype,
    /// It does not, after being typed twice — or the second typing was never
    /// submitted: the pane holds no briefing.
    Undelivered,
}

/// The watch over one launch briefing.
#[derive(Debug)]
pub struct BriefingWatch {
    window: Duration,
    since: Instant,
    retyped: bool,
    /// The retype's delivery has not said how it ended. Its words can stand
    /// in the composer while it is still out, before their Enter or under one
    /// it withholds, so the screen proves nothing until it is back
    /// (t-74919).
    retype_out: bool,
}

impl BriefingWatch {
    pub fn new(now: Instant, window: Duration) -> Self {
        Self {
            window,
            since: now,
            retyped: false,
            retype_out: false,
        }
    }

    /// One look. `shown` is [`briefing_shows`] over the pane's text.
    ///
    /// A briefing not yet on the screen is waited for the whole window — a
    /// program still booting answers its Enter late, up to 9.2 s in the
    /// measured worst case — and only then typed again, once. While that
    /// retype is out the watch waits whatever the screen shows, on no clock
    /// of its own: the delivery has its agent's deadline. A second empty
    /// window, counted from the retype's submission, is the verdict.
    pub fn poll(&mut self, shown: bool, now: Instant) -> WatchStep {
        if self.retype_out {
            return WatchStep::Waiting;
        }
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
        self.retype_out = true;
        WatchStep::Retype
    }

    /// The retype's delivery ended `outcome`. Submitted, its second window
    /// starts now; anything else — the Enter withheld, the words refused,
    /// the composer never ready — is the verdict.
    pub fn retype_settled(&mut self, outcome: Outcome, now: Instant) -> WatchStep {
        self.retype_out = false;
        if outcome == Outcome::Delivered {
            self.since = now;
            WatchStep::Waiting
        } else {
            WatchStep::Undelivered
        }
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

/// How often the screen is read while the watch waits.
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// The pane a launch briefing went into, as the watch needs it.
pub trait BriefingPane {
    /// The pane's scrollback and visible text; `None` once the pane is gone.
    fn screen(&mut self) -> Option<String>;
    /// Type the briefing once more, and say where that delivery's outcome
    /// arrives.
    fn retype(&mut self) -> Receiver<Outcome>;
    /// Take back a retype the watch no longer waits for.
    fn withdraw(&mut self);
}

/// Why a briefing is said not to stand in its pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unheard {
    /// Typed twice, and the screen never showed it.
    NeverShown,
    /// The briefing typed again ended this way, not submitted.
    RetypeEnded(Outcome),
    /// The briefing typed again never answered within its patience.
    RetypeUnanswered,
}

/// What the watch found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The briefing stands on the screen.
    Taken,
    /// The pane went away before the briefing was seen on it.
    Gone,
    /// The pane holds no briefing.
    Undelivered(Unheard),
}

/// Watch `pane` for `briefing` after its first delivery said it was done:
/// [`BriefingWatch`] over the screen, and one retype when it says so.
///
/// The retype's outcome comes first and the screen after it (t-74919): its
/// pasted words stand in the composer before their Enter goes, and stay there
/// when the Enter is withheld, so a watch that read the screen while it was
/// out called an unsent briefing taken. `retype_patience` is how long that
/// outcome is waited for — the caller's covering the delivery's own deadline
/// and its submission receipt; the delivery always ends itself by then.
pub fn confirm(
    pane: &mut impl BriefingPane,
    briefing: &str,
    window: Duration,
    retype_patience: Duration,
) -> Verdict {
    let mut watch = BriefingWatch::new(Instant::now(), window);
    loop {
        let Some(screen) = pane.screen() else {
            return Verdict::Gone;
        };
        match watch.poll(briefing_shows(&screen, briefing), Instant::now()) {
            WatchStep::Taken => return Verdict::Taken,
            WatchStep::Waiting => std::thread::sleep(LOOK_EVERY),
            WatchStep::Retype => {
                let Ok(outcome) = pane.retype().recv_timeout(retype_patience) else {
                    pane.withdraw();
                    return match pane.screen() {
                        Some(_) => Verdict::Undelivered(Unheard::RetypeUnanswered),
                        None => Verdict::Gone,
                    };
                };
                if watch.retype_settled(outcome, Instant::now()) == WatchStep::Undelivered {
                    return match pane.screen() {
                        Some(_) => Verdict::Undelivered(Unheard::RetypeEnded(outcome)),
                        None => Verdict::Gone,
                    };
                }
            }
            WatchStep::Undelivered => {
                pane.withdraw();
                return Verdict::Undelivered(Unheard::NeverShown);
            }
        }
    }
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
        let submitted = start + window + Duration::from_secs(1);
        assert_eq!(
            watch.retype_settled(Outcome::Delivered, submitted),
            WatchStep::Waiting
        );
        // The second window counts from the retype's submission.
        assert_eq!(
            watch.poll(false, submitted + Duration::from_secs(5)),
            WatchStep::Waiting
        );
        assert_eq!(
            watch.poll(false, submitted + window),
            WatchStep::Undelivered
        );
    }

    /// R2 8 (t-74919): the retyped words in the composer before their Enter
    /// are no answer while the retype is out, and a withheld Enter is the
    /// verdict however the screen looks.
    #[test]
    fn the_screen_says_nothing_while_the_retype_is_out_and_a_withheld_enter_is_the_verdict() {
        use crate::ready::Refusal;
        let start = Instant::now();
        let window = Duration::from_secs(6);
        let mut watch = BriefingWatch::new(start, window);
        assert_eq!(watch.poll(false, start + window), WatchStep::Retype);
        assert_eq!(
            watch.poll(true, start + window + Duration::from_millis(30)),
            WatchStep::Waiting,
            "words pasted and not yet entered were taken for a briefing"
        );
        assert_eq!(
            watch.retype_settled(
                Outcome::Unsubmitted(Refusal::LaunchChanged),
                start + window + Duration::from_millis(60)
            ),
            WatchStep::Undelivered
        );
        for outcome in [
            Outcome::TimedOut,
            Outcome::Refused(Refusal::LaunchChanged),
            Outcome::Unsubmitted(Refusal::NotTaken),
        ] {
            let mut watch = BriefingWatch::new(start, window);
            assert_eq!(watch.poll(false, start + window), WatchStep::Retype);
            assert_eq!(
                watch.retype_settled(outcome, start + window * 2),
                WatchStep::Undelivered,
                "{outcome:?} was taken for a submitted retype"
            );
        }
    }

    /// R2 9 (t-74919): a retype whose composer stands 7 s after it was typed
    /// — past a 6 s window, inside the delivery's 8 s — is not given up on
    /// at 6 s: the watch keeps no clock while the retype is out.
    #[test]
    fn a_retype_out_past_one_window_is_not_given_up_on() {
        let start = Instant::now();
        let window = Duration::from_secs(6);
        let mut watch = BriefingWatch::new(start, window);
        let retyped = start + window;
        assert_eq!(watch.poll(false, retyped), WatchStep::Retype);
        assert_eq!(
            watch.poll(false, retyped + Duration::from_secs(6)),
            WatchStep::Waiting,
            "the watch gave up on the retype at its own 6 s"
        );
        let submitted = retyped + Duration::from_secs(7);
        assert_eq!(
            watch.retype_settled(Outcome::Delivered, submitted),
            WatchStep::Waiting
        );
        assert_eq!(watch.poll(true, submitted), WatchStep::Taken);
    }

    #[test]
    fn a_briefing_that_shows_ends_the_watch_at_once() {
        let start = Instant::now();
        let mut watch = BriefingWatch::new(start, Duration::from_secs(6));
        assert_eq!(watch.poll(true, start), WatchStep::Taken);
    }
}
