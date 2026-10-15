//! Where a browser pane's main document stands — by the clock, not by wry.
//!
//! wry reports `on_page_load` Started and Finished for the main document and
//! nothing for a provisional navigation that failed: a dev server that died
//! under an open tab leaves the tab "loading" forever (2026-09-07). This
//! module is the record every reader shares — `zerocode-browser tabs`,
//! `diagnose`, and the window's own spinner (`browser:nav{state:"dead"}`) —
//! and the one rule that closes the gap: a Started with no Finished after
//! [`NAV_DEAD_AFTER_SECS`] is `dead`. The word says whose verdict it is.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Seconds a main-document load may stand Started without Finished before
/// the record calls it dead. Twenty: a slow page is seconds, a dead server is
/// forever, and an agent's `wait` budget is seven.
pub(super) const NAV_DEAD_AFTER_SECS: u64 = 20;

/// Request numbers for every pane the window ever opens: a label that is
/// closed and opened again never meets an answer meant for its old record.
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

/// What the engine said about a known request once it took it. Either
/// way only a later idle reading settles the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NavAnswer {
    /// The request is a load the engine now carries.
    Loading,
    /// The engine ended the request with no new document: refused, failed
    /// before commit, moved within the same document, or showed no load
    /// right after taking it.
    Resolved,
}

impl NavAnswer {
    /// `ended`: the engine refused the request, failed it before commit, or
    /// moved within the same document. `loading`: it shows a load.
    pub(super) fn from_engine(ended: bool, loading: bool) -> Self {
        if ended || !loading {
            NavAnswer::Resolved
        } else {
            NavAnswer::Loading
        }
    }
}

/// Chromium's answer to the `Page.navigate` it was sent: an error, an
/// `errorText`, or no new `loaderId` (a same-document move) ends the
/// request with no new document.
#[cfg(any(
    all(target_os = "macos", feature = "chromium-browser"),
    target_os = "windows",
    test
))]
pub(super) fn page_navigate_answer(succeeded: bool, result: &serde_json::Value) -> NavAnswer {
    let failed = result
        .get("errorText")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|text| !text.is_empty());
    let new_document = result
        .get("loaderId")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|loader| !loader.is_empty());
    NavAnswer::from_engine(!succeeded || failed || !new_document, true)
}

/// One pane's main-frame navigations as WebView2 numbers them. A completion
/// ends only the navigation with its own number. The loads are unknown until
/// a navigation seen starting completes having replaced the document, with
/// nothing else in flight: a load under way before the watch began was never
/// seen starting, and nothing short of a replaced document ends it.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Default)]
pub(super) struct NavigationIds {
    known: bool,
    started: std::collections::BTreeSet<u64>,
}

#[cfg(any(target_os = "windows", test))]
impl NavigationIds {
    pub(super) fn starting(&mut self, navigation: u64) {
        self.started.insert(navigation);
    }

    /// `replaced`: the navigation committed a document or an error page,
    /// rather than being cancelled.
    pub(super) fn completed(&mut self, navigation: u64, replaced: bool) {
        if self.started.remove(&navigation) && replaced {
            self.known |= self.started.is_empty();
        }
    }

    /// `None` while the loads are unknown.
    pub(super) fn loading(&self) -> Option<bool> {
        self.known.then_some(!self.started.is_empty())
    }
}

/// Whether a known request, or a load the engine carries, still stands
/// between a handoff and the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NavProof {
    Ready,
    Waiting,
}

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
/// `request` is the one known navigation a handoff waits for. Started,
/// Finished and failure carry no request number, so they never touch it.
#[derive(Debug, Clone)]
pub(super) struct NavRecord {
    state: NavState,
    started_at: Option<Instant>,
    request: Option<Request>,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Request {
    number: u64,
    stage: Stage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Admitted,
    Sent,
    Answered(NavAnswer),
}

fn next_request() -> u64 {
    NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
}

impl NavRecord {
    /// A pane at birth: blank when born on the blank page, otherwise its
    /// first document is already on its way — sent by the pane's creation,
    /// and not yet answered by the engine.
    pub(super) fn born(blank: bool, now: Instant) -> Self {
        if blank {
            NavRecord {
                state: NavState::Blank,
                started_at: None,
                request: None,
                generation: 0,
            }
        } else {
            NavRecord {
                state: NavState::Loading,
                started_at: Some(now),
                request: Some(Request {
                    number: next_request(),
                    stage: Stage::Sent,
                }),
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

    /// A known request is admitted: answers the number its native send and
    /// the engine's answer must present.
    pub(super) fn admit(&mut self) -> u64 {
        let number = next_request();
        self.request = Some(Request {
            number,
            stage: Stage::Admitted,
        });
        number
    }

    /// Called on the engine's thread just before the native send: `false`
    /// when the request is no longer the one admitted, so nothing is sent.
    pub(super) fn sent(&mut self, number: u64) -> bool {
        match &mut self.request {
            Some(request) if request.number == number && request.stage == Stage::Admitted => {
                request.stage = Stage::Sent;
                true
            }
            _ => false,
        }
    }

    /// The request was refused before any native send.
    pub(super) fn withdrawn(&mut self, number: u64) {
        if self.request
            == Some(Request {
                number,
                stage: Stage::Admitted,
            })
        {
            self.request = None;
        }
    }

    /// The engine's own answer to the request it took.
    pub(super) fn answered(&mut self, number: u64, answer: NavAnswer) {
        if let Some(request) = &mut self.request
            && request.number == number
            && request.stage == Stage::Sent
        {
            request.stage = Stage::Answered(answer);
        }
    }

    /// The pane's creation sent its first document, and the engine that
    /// created the pane has taken it.
    #[cfg(target_os = "windows")]
    pub(super) fn birth_taken(&mut self) {
        if let Some(request) = &mut self.request
            && request.stage == Stage::Sent
        {
            request.stage = Stage::Answered(NavAnswer::Loading);
        }
    }

    /// The engine's loading state, read on its thread in the same turn as
    /// this record. Only an answered request, with nothing loading after
    /// that answer, is settled; a load the page began itself keeps the
    /// handoff waiting while the engine still carries it.
    pub(super) fn settle_if_idle(&mut self, loading: bool) -> NavProof {
        if !loading
            && matches!(
                self.request,
                Some(Request {
                    stage: Stage::Answered(_),
                    ..
                })
            )
        {
            self.request = None;
        }
        if loading {
            NavProof::Waiting
        } else {
            self.proof()
        }
    }

    pub(super) fn proof(&self) -> NavProof {
        if self.request.is_some() {
            NavProof::Waiting
        } else {
            NavProof::Ready
        }
    }

    #[cfg(test)]
    pub(super) fn pending(&self) -> bool {
        self.proof() != NavProof::Ready
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

    #[cfg(all(target_os = "macos", feature = "chromium-browser"))]
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
    /// was made — the caller emits once, never for a load that moved on,
    /// finished, failed, or was already marked. Both engines arm it.
    pub(super) fn dead_if_still(&mut self, generation: u64, now: Instant) -> bool {
        if self.generation != generation
            || self.state != NavState::Loading
            || self.observed(now) != NavState::Dead
        {
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
