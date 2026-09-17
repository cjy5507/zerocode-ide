//! The continuous eye (docs/design/computer-use-full-operator.md §7.1, V1):
//! while the desktop is being looked at, the helper keeps a stream of the
//! display and numbers every repaint with where it fell ([`Change`]). What a
//! wait makes of those repaints is decided here once — the look after an act
//! ([`settle`]), `watch` ([`watched`]), and an OCR wait that reads the screen
//! again only after it changed ([`Ignore::counts`]) — and the window polls.
//!
//! Two kinds of repaint are not the caller's business ([`Ignore`]):
//! ZeroCode's own windows where nothing stands in front of them — the pane
//! the agent's words scroll in keeps repainting while it waits — and whatever
//! was already repainting just before the act or the watch began (a clock, a
//! video, a spinner). Neither holds a look back or ends a watch.
//!
//! What ZeroCode's own windows show is not read either ([`own_region`]): a
//! desktop OCR read leaves it out, as the hands never press it — the agent's
//! own command, scrolling in its pane, is not the app's answer.

use serde_json::Value;

use super::marks::DesktopWindow;
use super::render::{Rect, fully_covered};
use crate::computer_use::{COMPUTER_SETTLE_MS, EYE_BACKGROUND_MS, EYE_QUIET_MS, EYE_SETTLE_MAX_MS};

/// The helper's methods: open the eye, ask for its repaints, read its
/// newest frame; and its word for an eye that is not open.
pub const START_METHOD: &str = "eyeStart";
pub const CHANGES_METHOD: &str = "eyeChanges";
pub const FRAME_METHOD: &str = "eyeFrame";
pub const NOT_WATCHING: &str = "not_watching";
/// The words of the helper's stream answers.
pub const CHANGES_KEY: &str = "changes";
pub const SEQ_KEY: &str = "seq";
pub const NOW_KEY: &str = "nowMs";
pub const ACT_KEY: &str = "act";
pub const AT_KEY: &str = "atMs";
pub const RECTS_KEY: &str = "rects";
pub const WHOLE_KEY: &str = "whole";
pub const STREAMING_KEY: &str = "streaming";
/// Identity of one stream lifetime; repaint numbers alone restart at zero.
pub const STREAM_ID_KEY: &str = "streamId";
/// A watch cannot judge an interval whose repaint history was lost.
pub const HISTORY_LOST: &str = "eye_history_lost";
/// The two ways to ask for repaints: after a repaint's number, from a moment.
pub const AFTER_KEY: &str = "after";
pub const FROM_KEY: &str = "fromMs";
/// A desktop OCR read's parameter: what ZeroCode's own windows show
/// ([`own_region`], `[x, y, width, height]` rectangles in screen points) —
/// the reading leaves it out.
pub const OWN_REGION_KEY: &str = "ownRegion";
/// Regions excluded from OCR results. Unread text has no known line count.
pub const EXCLUDED_REGIONS_KEY: &str = "excludedRegions";

/// A point in the stream: a repaint's number and when it happened, on the
/// helper's clock. An act's mark is the stream's number when it began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub seq: u64,
    pub at_ms: i64,
}

/// A position tied to a stream lifetime, not just a reusable repaint number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub mark: Mark,
    pub stream_id: Option<String>,
}

/// One repaint: its number, when, and where it fell in screen points.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub seq: u64,
    pub at_ms: i64,
    pub rects: Vec<Rect>,
}

impl Change {
    fn from_row(row: &Value) -> Option<Self> {
        Some(Self {
            seq: row.get(SEQ_KEY)?.as_u64()?,
            at_ms: row.get(AT_KEY)?.as_i64()?,
            rects: row
                .get(RECTS_KEY)?
                .as_array()?
                .iter()
                .map(|rect| {
                    let edge = |at: usize| rect.get(at).and_then(Value::as_f64);
                    let (x, y, width, height) = (edge(0)?, edge(1)?, edge(2)?, edge(3)?);
                    (x.is_finite()
                        && y.is_finite()
                        && width.is_finite()
                        && height.is_finite()
                        && width >= 0.0
                        && height >= 0.0)
                        .then_some(Rect::new(x, y, width, height))
                })
                .collect::<Option<_>>()?,
        })
    }
}

/// The helper's answer about its stream: the newest repaint's number, its
/// clock, the last act's mark, and the repaints it was asked for, oldest
/// first. `whole` is false when older ones it was asked for were dropped.
#[derive(Debug, Clone, PartialEq)]
pub struct Changes {
    pub seq: u64,
    pub now_ms: i64,
    pub act: Option<Mark>,
    pub changes: Vec<Change>,
    pub whole: bool,
    pub streaming: bool,
    pub stream_id: Option<String>,
}

impl Changes {
    #[must_use]
    pub fn from_answer(answer: &Value) -> Option<Self> {
        let act = match answer.get(ACT_KEY) {
            None | Some(Value::Null) => None,
            Some(act) => Some(Mark {
                seq: act.get(SEQ_KEY)?.as_u64()?,
                at_ms: act.get(AT_KEY)?.as_i64()?,
            }),
        };
        Some(Self {
            seq: answer.get(SEQ_KEY)?.as_u64()?,
            now_ms: answer.get(NOW_KEY)?.as_i64()?,
            act,
            // A malformed row is missing evidence, never an empty interval.
            changes: match answer.get(CHANGES_KEY) {
                Some(rows) => rows
                    .as_array()?
                    .iter()
                    .map(Change::from_row)
                    .collect::<Option<_>>()?,
                None => Vec::new(),
            },
            whole: match answer.get(WHOLE_KEY) {
                Some(value) => value.as_bool()?,
                None => true,
            },
            streaming: match answer.get(STREAMING_KEY) {
                Some(value) => value.as_bool()?,
                None => false,
            },
            stream_id: match answer.get(STREAM_ID_KEY) {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    value
                        .as_str()
                        .filter(|id| !id.trim().is_empty())?
                        .to_string(),
                ),
            },
        })
    }

    #[must_use]
    pub fn cursor(&self) -> Cursor {
        Cursor {
            mark: Mark {
                seq: self.seq,
                at_ms: self.now_ms,
            },
            stream_id: self.stream_id.clone(),
        }
    }

    /// Older helpers omit the identity; sequence/time regressions and lost
    /// history still invalidate them. Identity-aware helpers also detect a
    /// replacement stream that already advanced beyond the old sequence.
    #[must_use]
    pub fn continues(&self, cursor: &Cursor) -> bool {
        self.streaming
            && self.whole
            && self.stream_id == cursor.stream_id
            && self.seq >= cursor.mark.seq
            && self.now_ms >= cursor.mark.at_ms
    }
}

/// What ZeroCode's own windows show of the screen, in pieces that do not
/// overlap: each own window less every window in front of it that hides
/// what is under it (`windows` front to back, as `listAllWindows` answers
/// with every layer; an overlay hides nothing). A point is in it exactly
/// when the frontmost window there is ZeroCode's.
#[must_use]
pub fn own_region(windows: &[DesktopWindow]) -> Vec<Rect> {
    let seen: Vec<&DesktopWindow> = windows.iter().filter(|window| window.covers()).collect();
    seen.iter()
        .enumerate()
        .filter(|(_, window)| window.own)
        .flat_map(|(at, window)| {
            seen[..at].iter().fold(vec![window.rect], |pieces, front| {
                pieces
                    .iter()
                    .flat_map(|piece| piece.minus(&front.rect))
                    .collect()
            })
        })
        .collect()
}

/// Whether ZeroCode's own windows show all of `area`. A piece over the bare
/// desktop, or under another app's window, is not theirs.
#[must_use]
pub fn owned(area: Rect, windows: &[DesktopWindow]) -> bool {
    fully_covered(area, &own_region(windows))
}

/// The repaints a wait does not count, from the moment it counts from.
#[derive(Debug)]
pub struct Ignore {
    own: Vec<Rect>,
    background: Vec<Rect>,
    area: Option<Rect>,
}

impl Ignore {
    /// ZeroCode's own windows (front to back, as `listAllWindows` answers),
    /// and what repainted in the `EYE_BACKGROUND_MS` before `from`.
    #[must_use]
    pub fn new(changes: &[Change], from: Mark, windows: &[DesktopWindow]) -> Self {
        let since = from.at_ms - i64::try_from(EYE_BACKGROUND_MS).unwrap_or(i64::MAX);
        let background = changes
            .iter()
            .filter(|change| change.seq <= from.seq && change.at_ms >= since)
            .flat_map(|change| change.rects.iter().copied())
            .collect();
        Self {
            own: own_region(windows),
            background,
            area: None,
        }
    }

    /// Only repaints that touch `area` count — what an OCR wait reads.
    #[must_use]
    pub fn within(mut self, area: Rect) -> Self {
        self.area = Some(area);
        self
    }

    /// Whether a repaint is the caller's business: some part of it on the
    /// area, not ZeroCode's own and not the screen's own motion.
    #[must_use]
    pub fn counts(&self, change: &Change) -> bool {
        change.rects.iter().any(|rect| {
            self.area.is_none_or(|area| area.intersects(rect))
                && !self.owns(rect)
                && !fully_covered(*rect, &self.background)
        })
    }

    /// Whether ZeroCode's own windows show all of `rect`.
    #[must_use]
    pub fn owns(&self, rect: &Rect) -> bool {
        fully_covered(*rect, &self.own)
    }

    /// The counted repaints after `from`, oldest first.
    #[must_use]
    pub fn after<'c>(&self, changes: &'c [Change], from: Mark) -> Vec<&'c Change> {
        changes
            .iter()
            .filter(|change| change.seq > from.seq && self.counts(change))
            .collect()
    }
}

/// Where the look after an act stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// Ask again after the poll.
    Wait,
    /// Look now. `settled` is false when the screen was still moving at the
    /// table's cap: the look shows it as it is.
    Look { settled: bool },
}

/// The look after an act: once what the act did has finished painting —
/// nothing counted repainted for `EYE_QUIET_MS` — or, when nothing counted
/// repainted at all, once the table's settle has passed since the act; never
/// later than `EYE_SETTLE_MAX_MS` after it. `act` is the act's mark (with no
/// act known, the moment the settle began); `changes` reach back
/// `EYE_BACKGROUND_MS` before it; `now_ms` is the helper's clock.
#[must_use]
pub fn settle(changes: &[Change], act: Mark, windows: &[DesktopWindow], now_ms: i64) -> Settle {
    let ignore = Ignore::new(changes, act, windows);
    let since_act = now_ms - act.at_ms;
    let reached = |ms: u64| since_act >= i64::try_from(ms).unwrap_or(i64::MAX);
    match ignore.after(changes, act).last() {
        None if reached(COMPUTER_SETTLE_MS) => Settle::Look { settled: true },
        None => Settle::Wait,
        Some(last) if now_ms - last.at_ms >= i64::try_from(EYE_QUIET_MS).unwrap_or(i64::MAX) => {
            Settle::Look { settled: true }
        }
        Some(_) if reached(EYE_SETTLE_MAX_MS) => Settle::Look { settled: false },
        Some(_) => Settle::Wait,
    }
}

/// What `watch` waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Until {
    /// The first counted repaint.
    Change,
    /// `EYE_QUIET_MS` with no counted repaint.
    Quiet,
}

impl Until {
    #[must_use]
    pub fn from_word(word: Option<&str>) -> Option<Self> {
        match word.unwrap_or("change") {
            "change" => Some(Self::Change),
            "quiet" => Some(Self::Quiet),
            _ => None,
        }
    }
}

/// Where a watch stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Watched {
    Wait,
    /// It changed: where (every counted rect since the start) and when the
    /// first counted repaint happened.
    Changed {
        first_ms: i64,
        rects: Vec<Rect>,
    },
    /// It went still: when the last counted repaint happened (the start,
    /// when none did), and where the counted ones fell.
    Quiet {
        last_ms: i64,
        rects: Vec<Rect>,
    },
}

/// A watch that began at `start`.
#[must_use]
pub fn watched(
    until: Until,
    changes: &[Change],
    start: Mark,
    windows: &[DesktopWindow],
    now_ms: i64,
) -> Watched {
    let ignore = Ignore::new(changes, start, windows);
    let counted = ignore.after(changes, start);
    let rects = || -> Vec<Rect> {
        counted
            .iter()
            .flat_map(|change| change.rects.iter().copied())
            .filter(|rect| !ignore.owns(rect))
            .collect()
    };
    match until {
        Until::Change => counted
            .first()
            .map_or(Watched::Wait, |first| Watched::Changed {
                first_ms: first.at_ms,
                rects: rects(),
            }),
        Until::Quiet => {
            let last_ms = counted.last().map_or(start.at_ms, |last| last.at_ms);
            if now_ms - last_ms >= i64::try_from(EYE_QUIET_MS).unwrap_or(i64::MAX) {
                Watched::Quiet {
                    last_ms,
                    rects: rects(),
                }
            } else {
                Watched::Wait
            }
        }
    }
}

#[cfg(test)]
mod tests;
