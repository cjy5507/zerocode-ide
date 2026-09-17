//! Where a browser pane's main document stands — by the clock, not by wry.
//!
//! wry reports `on_page_load` Started and Finished for the main document and
//! nothing for a provisional navigation that failed: a dev server that died
//! under an open tab leaves the tab "loading" forever (2026-09-07). This
//! module is the record every reader shares — `zerocode-browser tabs`,
//! `diagnose`, and the window's own spinner (`browser:nav{state:"dead"}`) —
//! and the one rule that closes the gap: a Started with no Finished after
//! [`NAV_DEAD_AFTER_SECS`] is `dead`. The word says whose verdict it is.

use std::time::{Duration, Instant};

/// Seconds a main-document load may stand Started without Finished before
/// the record calls it dead. Twenty: a slow page is seconds, a dead server is
/// forever, and an agent's `wait` budget is seven.
pub(super) const NAV_DEAD_AFTER_SECS: u64 = 20;

/// The four words `tabs` speaks for a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NavState {
    /// Standing on `about:blank` — born there, or navigated there.
    Blank,
    /// Started, not yet Finished, and not yet past the clock.
    Loading,
    Finished,
    /// Started, no Finished, and the clock ran out — the clock's verdict.
    Dead,
}

impl NavState {
    pub(super) fn word(self) -> &'static str {
        match self {
            NavState::Blank => "blank",
            NavState::Loading => "loading",
            NavState::Finished => "finished",
            NavState::Dead => "dead",
        }
    }
}

/// One pane's navigation record. `generation` counts Starteds so the timer a
/// Started armed can tell whether ITS load is the one still standing — a
/// second navigation inside the twenty seconds must not be called dead by
/// the first one's clock.
#[derive(Debug, Clone)]
pub(super) struct NavRecord {
    state: NavState,
    started_at: Option<Instant>,
    generation: u64,
}

impl NavRecord {
    /// A pane at birth: blank when born on the blank page, otherwise its
    /// first document is already on its way.
    pub(super) fn born(blank: bool, now: Instant) -> Self {
        if blank {
            NavRecord {
                state: NavState::Blank,
                started_at: None,
                generation: 0,
            }
        } else {
            NavRecord {
                state: NavState::Loading,
                started_at: Some(now),
                generation: 1,
            }
        }
    }

    /// The main document's Started. Answers the generation the caller's
    /// clock must present to [`Self::dead_if_still`].
    pub(super) fn started(&mut self, now: Instant) -> u64 {
        self.state = NavState::Loading;
        self.started_at = Some(now);
        self.generation += 1;
        self.generation
    }

    /// The main document's Finished — on the blank page, that is `blank`.
    pub(super) fn finished(&mut self, blank: bool) {
        self.state = if blank {
            NavState::Blank
        } else {
            NavState::Finished
        };
        self.started_at = None;
    }

    pub(super) fn failed(&mut self) {
        self.state = NavState::Dead;
    }

    /// The state as a reader sees it NOW: a load past the clock reads dead
    /// even before the timer that will mark it has fired.
    pub(super) fn observed(&self, now: Instant) -> NavState {
        match (self.state, self.started_at) {
            (NavState::Loading, Some(since))
                if now.saturating_duration_since(since)
                    >= Duration::from_secs(NAV_DEAD_AFTER_SECS) =>
            {
                NavState::Dead
            }
            (state, _) => state,
        }
    }

    /// The timer's call: mark dead if the load this generation armed is the
    /// one still standing and the clock has run out. `true` when the mark
    /// was made — the caller emits once, never for a load that moved on.
    // The wry pane arms this twenty-second timer. The CEF pane does NOT — it
    // emits `dead` only from `on_load_error`, so a load that neither fails nor
    // finishes (a hung dev server) never tells the window's spinner to stop;
    // `observed()` still reads it dead by the clock for `tabs`/`diagnose`.
    // That is the gap wry closed on 2026-09-07 and CEF has yet to (t-3624),
    // not a design. In the Chromium bin build the only caller is the unit
    // tests — compiled-but-unused there (t-3621).
    #[cfg_attr(
        all(target_os = "macos", feature = "chromium-browser"),
        allow(dead_code)
    )]
    pub(super) fn dead_if_still(&mut self, generation: u64, now: Instant) -> bool {
        if self.generation != generation || self.observed(now) != NavState::Dead {
            return false;
        }
        self.state = NavState::Dead;
        true
    }

    /// How long the current load has stood, for the diagnosis's sentence.
    pub(super) fn standing_for(&self, now: Instant) -> Option<Duration> {
        match self.state {
            NavState::Loading | NavState::Dead => self
                .started_at
                .map(|since| now.saturating_duration_since(since)),
            _ => None,
        }
    }
}

/// What the window knows about one pane beyond its address: the jar and the
/// reader it was born with, the agent it actually wears, the title the page
/// last said, and where its main document stands.
#[derive(Debug, Clone)]
pub(super) struct BrowserPaneRecord {
    pub(super) profile: Option<String>,
    pub(super) reader: Option<String>,
    /// The user agent the pane wears — the reader, or Safari's name; a
    /// builder-time agent cannot be asked back, so it is remembered here.
    pub(super) agent: String,
    pub(super) title: String,
    pub(super) nav: NavRecord,
}

/// One row of `zerocode-browser tabs`, already resolved by the clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TabRow {
    pub(super) label: String,
    pub(super) state: NavState,
    pub(super) title: String,
    pub(super) url: String,
    pub(super) profile: Option<String>,
    pub(super) reader: Option<String>,
}

/// `label\tstate\ttitle\turl\tprofile\treader`, panes in birth order. A
/// title's tabs and newlines are folded so a row stays a line; an absent
/// jar or reader is `-`.
pub(super) fn tabs_lines(rows: &[TabRow]) -> String {
    if rows.is_empty() {
        return "(열린 브라우저 판이 없습니다 — `zerocode-browser open <url>`)\n".to_string();
    }
    let mut out = String::new();
    for row in rows {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            row.label,
            row.state.word(),
            row.title.replace(['\t', '\n', '\r'], " "),
            row.url,
            row.profile.as_deref().unwrap_or("-"),
            row.reader.as_deref().unwrap_or("-"),
        ));
    }
    out
}

/// Birth order from the label: `browser-10` after `browser-9`, which a string
/// sort would not say.
pub(super) fn birth_number(label: &str) -> u64 {
    label
        .strip_prefix("browser-")
        .and_then(|n| n.parse().ok())
        .unwrap_or(u64::MAX)
}
