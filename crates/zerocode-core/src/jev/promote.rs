//! Whether a seat left on `auto` has earned the right to act, and whether one
//! that is acting has lost it (docs/design/jev-settings-20260917.md §4).
//!
//! `auto` says a seat records until its own evidence promotes it. Until this
//! module there was nothing that promoted: [`crate::jev::JevUse::promotes`] was a field
//! only tests had ever read, so every `auto` in the product was `shadow` under
//! a name that promised otherwise. A switch that says it will decide and never
//! does is worse than one that says it will not.
//!
//! The judgment is pure. What it needs from outside — the window's tally, the
//! apply stage's own deadline, how often the judgment named what the probe
//! named, whether labels exist and what they said — is handed in, because
//! each of those is owned somewhere else and a judge that went and read them
//! would be a second copy of all four.
//!
//! Nobody has to label anything for a seat to rise (2026-09-20). The first
//! judge held every seat until a person had written twenty labels, and no
//! person had: the one seat that promotes sat at "labels are needed" for three
//! days, on a window whose floor its width could not reach. What carries a
//! seat up now is its own ledger — that it answers, in time, well-formed, and
//! that when it is asked the same question as the probe it is replacing, it
//! says what the probe says often enough that applying it changes few routes
//! (a route-change budget, [`crate::jev::JevUse::agreement_floor_permille`]).
//! Labels a person did write still outrank that: they are the only evidence
//! of being right rather than merely the same.
//!
//! No seat rises with no marks to hand, whatever kind its marks are
//! (2026-09-22, t-6155 F1). For one afternoon the seats whose marks are
//! hindsight — was the note read, was the pane left where it was put, was
//! the dropped block read again — were carried up by their own ledger
//! alone, on the reasoning that a thin sample of hindsight falls through the
//! way thin labels do rather than holding the way the agreement line does.
//! That let the compaction seat, whose act is a loss the summary never sees
//! again, reach Applying on twenty answered rows and not one mark. The
//! sample floor ([`crate::jev::JevUse::agreement_rows_wanted`]) now holds
//! every seat the same way: until that many marks are in hand the judge says
//! `too_few_compared`, and answer rate, latency and shape alone never make a
//! seat act. The seats whose marks arrive late write them themselves
//! (t-5806); a seat whose marks never arrive is one that stays recording,
//! which is what `auto` promised.
//!
//! Evidence is one version's (2026-09-23, t-6187). Every request names the
//! vendor's alias unless a person pinned a version, and the alias answers
//! with whatever version the vendor ships under it — the answer's own
//! `model` says which ([`crate::jev::summary::MODEL`]). A window that ran
//! across the day the alias moved would promote the new version on the old
//! one's record, so the judge reads the ledger from the newest version's
//! first row on ([`on_the_newest_version`]). A row that names no version —
//! a refusal, a timeout, a label, every row written before versions were
//! recorded — belongs to the version of the nearest row after it that names
//! one: the seats already standing on their ledgers keep the evidence they
//! stand on, and only a real change of version starts a window again.

use serde_json::{Value, json};

use crate::jev::{A_WINDOW_OF_COMPARISONS, JevUse};

use crate::jev::summary::{
    AT, JUDGED_EVERY_ROWS, MODEL, TRANSITION, Tally, WILSON_Z_95, asked_something,
    rows_that_can_clear_forgiving, wilson_lower,
};

/// What the judge said of a ledger's rows, and the window it said it on —
/// the one reading a judge that writes transitions and a screen that shows
/// the numbers both take.
#[derive(Debug, Clone, PartialEq)]
pub struct Judged {
    pub verdict: Verdict,
    /// The window the lines were read over: the last requests the seat's
    /// answer floor can be cleared on.
    pub window: Tally,
    /// How many requests that window wants before the floor can be cleared
    /// at all, for a screen that says "17 of 73".
    pub window_wanted: usize,
    /// How often, over that window, the judgment named what the reader it
    /// would replace named.
    pub agreement: Agreement,
    /// Control rows the agreement was read over beside the window's own —
    /// the routing seat's probe run once more for a sampled active turn
    /// ([`crate::jev::summary::CONTROL`]), joined to the window by task.
    /// Zero for a seat that has no such sample.
    pub control_rows: usize,
    /// The version the window was read on — the newest the rows name
    /// ([`OnVersion::model`]); `None` for a ledger no row names one in.
    pub model: Option<String>,
    /// The version the rows were cut away from, when a change of version
    /// cut them ([`OnVersion::cut`]) — the reason a thinner sample gives
    /// for itself beside the line it holds on.
    pub cut: Option<String>,
}

/// A ledger read from its newest answering version's first row on — the rows
/// a seat is judged on (t-6187).
///
/// Counted back from the newest row: the cut falls at the first row that
/// names a version other than the newest one. A row that names none belongs
/// to the nearest named row after it, so a ledger no row names a version in
/// is read whole, as every ledger was before versions were recorded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnVersion<'rows> {
    /// The version answering now: the one the newest request that names a
    /// version was answered by — a label grading an older answer does not
    /// move it — or, in a ledger whose requests name none, the one the
    /// newest row that names a version says.
    pub model: Option<&'rows str>,
    /// The version met where the rows were cut, when they were.
    pub cut: Option<&'rows str>,
    /// The rows the window's requests are counted from: every row after the
    /// newest request another version answered.
    pub requests: &'rows [Value],
    /// The rows the marks are counted from: every row after the newest row
    /// of any kind that names another version — so a label written down
    /// with the version it graded is cut with that version even when it
    /// was written after the other version's last request.
    pub marks: &'rows [Value],
}

/// The version `row` names as the one that answered it, if it names one.
fn named_version(row: &Value) -> Option<&str> {
    MODEL
        .read(row)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
}

/// `rows` from the newest answering version's first row on ([`OnVersion`]).
#[must_use]
pub fn on_the_newest_version(rows: &[Value]) -> OnVersion<'_> {
    let model = rows
        .iter()
        .rev()
        .filter(|row| asked_something(row).is_some())
        .find_map(named_version)
        .or_else(|| rows.iter().rev().find_map(named_version));
    let another = |row: &Value| named_version(row).is_some_and(|named| Some(named) != model);
    let after = |found: Option<usize>| found.map_or(0, |at| at + 1);
    let requests_from = after(
        rows.iter()
            .rposition(|row| asked_something(row).is_some() && another(row)),
    );
    let marks_from = after(rows.iter().rposition(another));
    OnVersion {
        model,
        cut: marks_from
            .checked_sub(1)
            .and_then(|at| rows.get(at))
            .and_then(named_version),
        requests: &rows[requests_from..],
        marks: &rows[marks_from..],
    }
}

/// How many requests the judgment's cadence counts: the ones the newest
/// answering version was asked ([`on_the_newest_version`]). One reader,
/// because the cadence ([`judgment_due`]) and the countdown a screen draws
/// ([`rows_to_next_judgment`]) must land on the same row.
#[must_use]
pub fn asked_toward_judgment(rows: &[Value]) -> usize {
    on_the_newest_version(rows)
        .requests
        .iter()
        .filter(|row| asked_something(row).is_some())
        .count()
}

/// Judge a seat on its own ledger rows, by the table's lines alone: the
/// window the seat's answer floor can be cleared on, the seat's apply wall,
/// and the [`crate::jev::summary::AGREED`] marks its rows carry — all of
/// them the newest answering version's ([`on_the_newest_version`]). `None`
/// for a seat the table says never rises.
///
/// The orchestration seats are judged here, in the process that writes their
/// rows (the window) and in the counter that shows them (`zo jev summary`),
/// from one function. zo's routing seat keeps its own reading of agreement —
/// the probe's answer beside the judgment's, axis by axis — and hands the
/// rest to the same [`judge`].
///
/// Where the seat stands is read from the whole ledger: a standing earned on
/// one version is not forgotten by the next, only judged on the next one's
/// rows — a seat already acting keeps acting until one of them breaks a line
/// ([`judge`]).
#[must_use]
pub fn judge_seat(seat: &JevUse, rows: &[Value]) -> Option<Judged> {
    let floor = seat.answer_floor_permille?;
    let agreement_floor = seat.agreement_floor_permille?;
    let deadline_ms = seat.apply_deadline_ms?;
    let window_wanted = window_wanted_for(seat)?;
    let version = on_the_newest_version(rows);
    let held = crate::jev::summary::last_asked(version.requests, window_wanted);
    let since_ms = held
        .first()
        .and_then(|row| AT.read(row).and_then(Value::as_i64))
        .unwrap_or(i64::MIN);
    let window = crate::jev::summary::summarize_rows(held.iter().copied(), i64::MIN);
    let agreement = crate::jev::summary::agreement_since(version.marks, since_ms);
    let verdict = judge(
        stand_from(rows),
        &Evidence {
            window: &window,
            floor_permille: floor,
            deadline_ms,
            agreement_floor_permille: agreement_floor,
            agreement,
            agreement_rows_wanted: seat
                .agreement_rows_wanted
                .unwrap_or(A_WINDOW_OF_COMPARISONS),
            window_forgives: seat.window_forgives.unwrap_or(0),
            labels: None,
            fallbacks_in_a_row: crate::jev::summary::failures_in_a_row(version.requests),
        },
    );
    Some(Judged {
        verdict,
        window,
        window_wanted,
        agreement,
        control_rows: 0,
        model: version.model.map(str::to_string),
        cut: version.cut.map(str::to_string),
    })
}

/// The window `seat` is judged on: the last requests its answer floor can be
/// cleared on, given what the seat forgives ([`JevUse::window_forgives`]).
///
/// One reader, because three things need it and must agree — the judge, the
/// cadence that decides when to judge ([`judgment_due`]), and the screen that
/// says "17 of 73". `None` for a seat that names no floor, which is a seat
/// that never rises.
#[must_use]
pub fn window_wanted_for(seat: &JevUse) -> Option<usize> {
    Some(rows_that_can_clear_forgiving(
        seat.answer_floor_permille?,
        seat.window_forgives.unwrap_or(0),
    ))
}

/// Whether the rows say a judgment of `seat` is due: every
/// [`JUDGED_EVERY_ROWS`] requests once its window can be full, or at once for
/// an acting seat that has fallen back [`FALLBACKS_THAT_END_IT`] times
/// running (§4).
///
/// Counted from the window and not from the first row, because a judgment
/// made before the window can be full has only one thing it can say. The
/// window seats' ledgers are small and the judgments were being spent on it:
/// of the four judgments this machine's placement, summon and routing seats
/// had been given in their whole lives by 2026-09-22, three said
/// `too_few_rows` off a window arithmetic had already decided could not be
/// full, and placement — 38 rows against a window of 25 — never got another
/// one, because its second boundary at row 40 had not arrived.
///
/// Counted on the newest answering version's requests
/// ([`asked_toward_judgment`]), for the same reason: after a change of
/// version the window starts again, and a judgment before it is full again
/// has only `too_few_rows` to say.
#[must_use]
pub fn judgment_due(seat: &JevUse, rows: &[Value]) -> bool {
    let asked = asked_toward_judgment(rows);
    let wanted = window_wanted_for(seat).unwrap_or(JUDGED_EVERY_ROWS);
    let at_boundary = asked >= wanted && (asked - wanted).is_multiple_of(JUDGED_EVERY_ROWS);
    let ending_it = stand_from(rows) == Stand::Applying
        && crate::jev::summary::failures_in_a_row(on_the_newest_version(rows).requests)
            >= FALLBACKS_THAT_END_IT;
    at_boundary || ending_it
}

/// How many more requests before `seat`'s next judgment — the same cadence
/// [`judgment_due`] runs on, so the screen's countdown and the judgment land
/// on the same row. `None` for a seat that never rises.
#[must_use]
pub fn rows_to_next_judgment(seat: &JevUse, asked: usize) -> Option<usize> {
    let wanted = window_wanted_for(seat)?;
    Some(match asked.checked_sub(wanted) {
        None => wanted - asked,
        Some(past) => JUDGED_EVERY_ROWS - past % JUDGED_EVERY_ROWS,
    })
}

/// The word every schema refusal's token is built from: zo writes it alone
/// when a reply failed its checks, and [`crate::jev::choice::ChoiceRefusal`]
/// writes it with the broken rule after it. Spelled once here, and a contract
/// holds the rest to it — a seat is not promoted while any reply is arriving
/// malformed, so the judge has to be able to recognise one.
pub const SCHEMA: &str = "schema";

/// How many fallbacks in a row end an applying seat's turn at acting (§4).
///
/// Three, and not one: a single timeout is the wire having a bad minute, and
/// a seat that fell back to recording on every one of those would spend its
/// life climbing back. Three in a row is the wire, the key or the model, none
/// of which the next request will fix.
pub const FALLBACKS_THAT_END_IT: u32 = 3;

/// Whether an outcome token names a reply that arrived and failed its checks.
#[must_use]
pub fn names_a_schema_failure(token: &str) -> bool {
    token == SCHEMA
        || token
            .strip_prefix(SCHEMA)
            .is_some_and(|rest| rest.starts_with('_'))
}

/// Where a seat stands when it is judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stand {
    /// Recording only — what `auto` starts as.
    Recording,
    /// Acting on answers that pass their checks.
    Applying,
}

/// Why a screen walk pressed nothing though its judgment named a control:
/// the seat it walks under is only recording ([`Stand::Recording`]), so the
/// answer was written down and no hand went out.
///
/// It is a word rather than a sentence because a row carries it, the walk's
/// own prose repeats it and a reader greps for it — one spelling, or the
/// three drift. It lives beside the stand it names and not beside the walk
/// because the walk is not the only seat that can be standing here.
///
/// It exists because a walk that pressed nothing said nothing about why: the
/// 2026-09-20 measurement of v1.1.3 spent thirteen browser and desktop walks
/// on a seat at `auto` that judged every screen with confidence 0.95 and
/// higher, pressed none of them, and answered `pressed: 0, reached: false`
/// with no error and no reason. The seat was raised by hand at 16:19 and the
/// same walks pressed six times out of six (t-5455).
pub const SEAT_RECORDING: &str = "seat_recording";

/// How often the judgment named what the probe it would replace named, over
/// the window's rows where both answered — every judged axis of every such
/// row is one comparison.
///
/// Not evidence of being right: two readers saying the same thing says
/// nothing about whether either is. It is evidence of something narrower and
/// enough to act on — that applying the judgment in the probe's place moves
/// few routes. A seat that agrees with the probe four times in five and
/// answers in a tenth of the time is a faster probe; one that agrees half the
/// time is a different router nobody measured. Measured on this machine's 25
/// answered rows (2026-09-20): 53% over every axis, 77% over the axes the
/// judgment was at least half sure of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Agreement {
    /// Axes compared: rows where both answered, times the judged axes.
    pub compared: usize,
    /// Of those, the ones where the judgment's choice was the probe's.
    pub agreed: usize,
}

impl Agreement {
    /// The 95% Wilson lower bound on the agreed share, `None` when nothing
    /// was compared.
    #[must_use]
    pub fn lower_bound(&self) -> Option<f64> {
        (self.compared > 0).then(|| wilson_lower(self.agreed, self.compared, WILSON_Z_95))
    }
}

/// What the evidence says about labels, when a person has written any.
///
/// Labels outrank agreement: they are the one comparison that says a reader
/// is right and not merely the same as the other one. Fewer than a window's
/// worth of them say nothing yet, and the seat is judged on agreement
/// instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Labels {
    /// Labelled rows compared.
    pub compared: usize,
    /// Rows where the judgment named what the label named.
    pub judgment_right: usize,
    /// Rows where the probe did.
    pub probe_right: usize,
}

impl Labels {
    /// Whether the judgment is at least as right as the probe it would replace.
    #[must_use]
    pub const fn judgment_at_least_as_right(&self) -> bool {
        self.judgment_right >= self.probe_right
    }
}

/// Everything the judgment reads.
#[derive(Debug, Clone, Copy)]
pub struct Evidence<'window> {
    /// The recent window's rows.
    pub window: &'window Tally,
    /// The use's own rise line, per thousand.
    pub floor_permille: u16,
    /// The apply stage's deadline in milliseconds — the same constant the
    /// stage falls back on, read and not respelled.
    pub deadline_ms: u64,
    /// The use's own route-change budget, per thousand: the share of compared
    /// axes on which the judgment must bound above in naming what the probe
    /// named.
    pub agreement_floor_permille: u16,
    /// How often it did, over the window.
    pub agreement: Agreement,
    /// How many marks must be in hand before that budget can be read at all
    /// ([`JevUse::agreement_rows_wanted`]); a thinner sample holds
    /// promotion, whatever kind of mark the seat writes.
    pub agreement_rows_wanted: usize,
    /// How many of the window's rows may have missed and the seat still clear
    /// its answer floor ([`JevUse::window_forgives`]) — what the window's own
    /// width was derived from.
    pub window_forgives: usize,
    /// What labels said, when a person wrote any.
    pub labels: Option<Labels>,
    /// Fallbacks in a row while applying.
    pub fallbacks_in_a_row: u32,
}

/// Why a seat may not act, in the order §4 asks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Line {
    /// Fewer rows than a judgment window.
    TooFewRows { rows: usize, wanted: usize },
    /// The answered share's lower bound is under the use's floor.
    Answered {
        bound_permille: u16,
        floor_permille: u16,
    },
    /// The calls' p95 is over the apply stage's deadline.
    Latency { p95_ms: u64, deadline_ms: u64 },
    /// Replies arrived malformed in the window.
    Schema { rows: usize },
    /// Too few rows where both the judgment and the probe answered to say
    /// how often they agree.
    TooFewCompared { compared: usize, wanted: usize },
    /// The agreed share's lower bound is under the use's route-change budget.
    Agreement {
        bound_permille: u16,
        floor_permille: u16,
    },
    /// Labels say the probe is righter.
    Labels {
        judgment_right: usize,
        probe_right: usize,
    },
    /// It fell back this many times in a row while acting.
    Fallbacks { in_a_row: u32 },
}

/// What the judgment decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Record-only, and this is the first line it does not clear.
    Hold(Line),
    /// Rise to acting: every line clear.
    Rise,
    /// Keep acting.
    Keep,
    /// Fall back to recording, on this line.
    Fall(Line),
}

/// The share, per thousand, a bound in `[0, 1]` reaches. Floored, so a bound
/// is never read as clearing a line it sits a hair under.
#[must_use]
pub fn permille(share: f64) -> u16 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let floored = (share.clamp(0.0, 1.0) * 1000.0).floor() as u16;
    floored
}

/// Rows in the window whose reply arrived and failed its checks.
#[must_use]
pub fn schema_rows(window: &Tally) -> usize {
    window
        .failures
        .iter()
        .filter(|(token, _)| names_a_schema_failure(token))
        .map(|(_, rows)| *rows)
        .sum()
}

/// The first line the evidence does not clear, in §4's order, or `None` when
/// it clears them all.
#[must_use]
pub fn first_broken_line(evidence: &Evidence) -> Option<Line> {
    let window = evidence.window;
    // The window a floor can be cleared on, not the judgment's cadence: a
    // seat judged every twenty rows on the last twenty could never bound
    // twenty answers above 0.95 (2026-09-17..20).
    let wanted = rows_that_can_clear_forgiving(evidence.floor_permille, evidence.window_forgives);
    if window.asked() < wanted {
        return Some(Line::TooFewRows {
            rows: window.asked(),
            wanted,
        });
    }
    let bound = permille(window.answered_lower_bound().unwrap_or(0.0));
    if bound < evidence.floor_permille {
        return Some(Line::Answered {
            bound_permille: bound,
            floor_permille: evidence.floor_permille,
        });
    }
    if let Some(p95) = window.p95_ms.filter(|p95| *p95 > evidence.deadline_ms) {
        return Some(Line::Latency {
            p95_ms: p95,
            deadline_ms: evidence.deadline_ms,
        });
    }
    let malformed = schema_rows(window);
    if malformed > 0 {
        return Some(Line::Schema { rows: malformed });
    }
    // A person's labels, when there are a window's worth, are the last word;
    // otherwise the seat is held to its route-change budget.
    if let Some(labels) = evidence
        .labels
        .filter(|labels| labels.compared >= JUDGED_EVERY_ROWS)
    {
        return (!labels.judgment_at_least_as_right()).then_some(Line::Labels {
            judgment_right: labels.judgment_right,
            probe_right: labels.probe_right,
        });
    }
    // The sample floor holds every seat, whatever kind of mark it writes
    // (t-6155 F1): a hindsight seat with no marks yet is a seat nothing has
    // graded, not one that has passed.
    let agreement = evidence.agreement;
    if agreement.compared < evidence.agreement_rows_wanted {
        return Some(Line::TooFewCompared {
            compared: agreement.compared,
            wanted: evidence.agreement_rows_wanted,
        });
    }
    let bound = agreement.lower_bound().map(permille)?;
    (bound < evidence.agreement_floor_permille).then_some(Line::Agreement {
        bound_permille: bound,
        floor_permille: evidence.agreement_floor_permille,
    })
}

/// Judge a seat on its window (§4).
///
/// An applying seat is asked the fallback question first: a wire that has
/// failed three times running is not a seat's fault and not something the
/// other lines can see, and the whole point of that rule is that it does not
/// wait for the next twenty rows.
#[must_use]
pub fn judge(stand: Stand, evidence: &Evidence) -> Verdict {
    if stand == Stand::Applying && evidence.fallbacks_in_a_row >= FALLBACKS_THAT_END_IT {
        return Verdict::Fall(Line::Fallbacks {
            in_a_row: evidence.fallbacks_in_a_row,
        });
    }
    match (stand, first_broken_line(evidence)) {
        (Stand::Recording, None) => Verdict::Rise,
        (Stand::Recording, Some(line)) => Verdict::Hold(line),
        (Stand::Applying, None) => Verdict::Keep,
        // A seat already acting is not held to the window's width: it earned
        // its place on a full one, and a fresh window is not evidence against
        // it. Only a line it actually breaks takes it back.
        (Stand::Applying, Some(Line::TooFewRows { .. } | Line::TooFewCompared { .. })) => {
            Verdict::Keep
        }
        (Stand::Applying, Some(line)) => Verdict::Fall(line),
    }
}

#[cfg(test)]
mod tests;

/// What a rising row's [`TRANSITION`] says.
pub const ROSE: &str = "rise";
/// What a falling row's says.
pub const FELL: &str = "fall";
/// The key holding the line a fall was decided on, for a person reading back.
pub const ON_LINE: &str = "line";

impl Line {
    /// The word this line writes in a transition row. Closed, and carrying
    /// none of the request — a ledger says which rule, never what was asked.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::TooFewRows { .. } => "too_few_rows",
            Self::Answered { .. } => "answered",
            Self::Latency { .. } => "latency",
            Self::Schema { .. } => SCHEMA,
            Self::TooFewCompared { .. } => "too_few_compared",
            Self::Agreement { .. } => "agreement",
            Self::Labels { .. } => "labels",
            Self::Fallbacks { .. } => "fallbacks",
        }
    }
}

impl Stand {
    /// The word a row and a screen use for where a seat stands.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Applying => "applying",
        }
    }
}

impl Verdict {
    /// The word this verdict is named by. A rise and a fall reuse the words
    /// their transition rows carry, so a screen and a ledger cannot disagree.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Hold(_) => "hold",
            Self::Rise => ROSE,
            Self::Keep => "keep",
            Self::Fall(_) => FELL,
        }
    }

    /// The line it turned on, when it turned on one.
    #[must_use]
    pub const fn line(self) -> Option<Line> {
        match self {
            Self::Hold(line) | Self::Fall(line) => Some(line),
            Self::Rise | Self::Keep => None,
        }
    }
}

/// The settings key naming the file of labels a person wrote, under the Jev
/// block ([`crate::jev::door::JEV_SETTINGS_KEY`]) — `smart.jev.labels`.
///
/// §4 will not raise a seat without labels, so the product has to know where
/// they live; without this key the rule is a door with no handle, and the
/// screen's "twenty labels are needed" names nothing a person can act on.
pub const LABELS_KEY: &str = "labels";

/// The settings key that turns on label drafts (`smart.jev.labelDrafts`).
///
/// §4 will not raise a seat until labels say its judgment is at least as right
/// as the probe's, and the labels join the ledger by a task's fingerprint. But
/// the ledger carries no words — deliberately — and neither, it turns out, do
/// the transcripts: on this machine not one of the ten judged tasks could be
/// matched back to the text it was judged on (measured 2026-09-19). So the
/// rule had no road to it at all: a person could see that a seat wanted twenty
/// labels and had nothing to write them against.
///
/// With this on, each judged task is drafted where it is asked — the one place
/// the words are still in hand — for a person to fill the axes in. Off by
/// default and named for what it does, because it writes their own prompts to
/// a file, which the ledger beside it goes out of its way not to do.
pub const LABEL_DRAFTS_KEY: &str = "labelDrafts";

/// Whether the person asked for label drafts.
#[must_use]
pub fn label_drafts_wanted(root: &Value) -> bool {
    root.get(crate::jev::SMART_SETTINGS_KEY)
        .and_then(|smart| smart.get(crate::jev::door::JEV_SETTINGS_KEY))
        .and_then(|jev| jev.get(LABEL_DRAFTS_KEY))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The labels file a person named, if they named one.
#[must_use]
pub fn labels_path_in(root: &Value) -> Option<&str> {
    root.get(crate::jev::SMART_SETTINGS_KEY)?
        .get(crate::jev::door::JEV_SETTINGS_KEY)?
        .get(LABELS_KEY)?
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
}

/// Where a seat stands, read back from the transitions its ledger recorded.
///
/// No transition means it has never risen, which is what `auto` starts as.
#[must_use]
pub fn stand_from(rows: &[Value]) -> Stand {
    rows.iter()
        .rev()
        .find_map(|row| match TRANSITION.read(row).and_then(Value::as_str) {
            Some(ROSE) => Some(Stand::Applying),
            Some(FELL) => Some(Stand::Recording),
            _ => None,
        })
        .unwrap_or(Stand::Recording)
}

/// The row a rise or a fall appends. `None` for a verdict that changed
/// nothing — a ledger of "still recording" every twenty rows is a ledger
/// nobody can read.
#[must_use]
pub fn transition_row(now_ms: i64, verdict: Verdict, window: &Tally) -> Option<Value> {
    let (word, line) = match verdict {
        Verdict::Rise => (ROSE, None),
        Verdict::Fall(line) => (FELL, Some(line)),
        Verdict::Hold(_) | Verdict::Keep => return None,
    };
    let mut row = json!({
        AT.canonical: now_ms,
        (TRANSITION.canonical): word,
        "rows": window.rows,
        "answered": window.answered,
        "answeredLowerBoundPermille": window.answered_lower_bound().map(permille),
        "p95Ms": window.p95_ms,
    });
    if let Some(line) = line {
        row[ON_LINE] = Value::from(line.token());
    }
    Some(row)
}
