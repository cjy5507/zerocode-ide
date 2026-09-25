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
//! Marks in hand are the seat's, not only its window's (2026-09-25, t-9087).
//! The window is a count of requests sized by the answer floor, and a seat
//! that marks one request in four — the notify seat grades a ring only when
//! the person was there to turn to it — held 15 marks in its 53-ring window
//! with 110 on its record, and sat at `too_few_compared` for good. The
//! window's marks reach back to the width the seat's agreement line can be
//! cleared on with the negatives it asks inside ([`marks_that_can_clear`],
//! t-9468; [`crate::jev::summary::marks_from`]) — within the seat's series,
//! the words it asks now and the version answering now, and never past it; a
//! window that holds that width reads its own.
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
//! stand on, and only a real change of version starts a window again. Only
//! a request or a mark names one (t-6284): zo's step seat files a row for
//! every step of a turn between its judgments and labels, and those rows
//! carried the chat model each step ran on under the same key — read as
//! versions, they cut the seat's marks away at every step.
//!
//! And evidence is one rubric's (2026-09-24, t-6877). A seat's words move
//! too — the text guard's went from version 1 to 2 the day this was written
//! — and a request asked under other words answered another question. The
//! judge reads a seat's ledger as the series of the words its row asks now
//! ([`JevUse::rubric_version`]): the requests stamped with that version
//! ([`crate::jev::summary::RUBRIC_VERSION`]; one that names none is the
//! first rubric's), the marks that grade those requests — a label joined to
//! one request by the name the seat's row says a request carries
//! ([`JevUse::request_name`]), picking a request out as that row says the
//! name does ([`JevUse::names`]), and the time it was asked
//! ([`crate::jev::summary::REQUEST_AT`]), the request being the authority
//! on which rubric the label belongs to and on which version answered it;
//! a label that could mean two requests grades neither — and, inside that
//! series, the newest answering version's rows as before. Where the seat
//! stands is one rubric's too: a transition names the rubric it was
//! decided on ([`transition_row`]), and a seat stands on it only while it
//! asks exactly those words ([`standing`]). A seat whose words moved on
//! records again until the new words have earned their place; a seat whose
//! words went back starts recording as well, on requests asked since — the
//! old run's rows and its rise are behind the newer words' requests and
//! revive nothing ([`on_the_newest_version`]). A seat is one question: the
//! skills seat, which asked two into one ledger, is two seats now
//! ([`crate::jev::SKILLS`], [`crate::jev::SKILL_SUGGESTION`]), each judged
//! on its own rows and standing on its own rise.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use crate::jev::questions::UNVERSIONED_RUBRIC;
use crate::jev::{A_WINDOW_OF_COMPARISONS, Baseline, JevUse, Naming};

use crate::jev::summary::{
    AGREED, ANSWERED, AT, BASELINE_AGREED, JUDGED_EVERY_ROWS, LABEL, MODEL, NOT_COMPARED,
    REQUEST_AT, RUBRIC_VERSION, RUBRIC_VERSIONS, TRANSITION, Tally, WILSON_Z_95, asked_something,
    failures_in_a_row_of, is_control_row, is_request_or_mark, rows_that_can_clear_forgiving,
    wilson_lower,
};
use crate::jev::threshold::{CALIBRATION, Graded, answer_confidence};

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
    /// How often, over that window's marks — reached back to the width the
    /// agreement line can be cleared on ([`marks_that_can_clear`],
    /// [`crate::jev::summary::marks_from`]) — the judgment named what the
    /// reader it would replace named.
    pub agreement: Agreement,
    /// Why that window's rows that compared nothing say so, word by word
    /// ([`crate::jev::summary::not_compared_words`], t-9556): the
    /// agreement's `not_compared`, told apart.
    pub not_compared_by: BTreeMap<String, usize>,
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
    /// The act line the marks were read at ([`judge_seat_at`], t-9468) —
    /// `None` for a seat judged on every mark, as every seat was before its
    /// labels drew one.
    pub act_line: Option<u16>,
}

/// A ledger read as one seat's current series (t-6877) from its newest
/// answering version's first row on (t-6187) — the rows a seat is judged on.
///
/// The series is the requests asked under the words the seat asks now
/// ([`JevUse::rubric_version`]): a request or control row stamped with that
/// version ([`crate::jev::summary::RUBRIC_VERSION`]; a row that names none
/// is the first rubric's, a row that names something that is not a version
/// is nobody's), and the marks that grade them. A label names its request
/// by the name the seat's row says a request carries
/// ([`JevUse::request_name`]) and, when it carries it, by the time that
/// request was asked ([`crate::jev::summary::REQUEST_AT`]), and the name
/// picks out what the seat's row says it does ([`JevUse::names`], t-6877
/// round 3): an id picks out the one request above the label carrying it;
/// the fingerprints of the words asked pick out no asking on their own —
/// the same words asked again carry them again — so a label of the recall,
/// mention or skills seats grades only the asking made at the time it
/// names, and a label naming no time grades nothing; and the routing seat's
/// attempt names a turn, whose several rows stand as one while they agree
/// on the rubric, the answering version and the side of the series' start.
/// Wherever the name and the time could mean two requests, the label grades
/// neither — the reader guesses none, and never the newest of two. A label
/// that names no request — none above it, a time no request above it was
/// asked at, its own request trimmed away — grades nothing; a label of a
/// seat whose labels name no request grades nothing; and of two labels
/// naming one request, and one part of it where the seat's labels grade
/// parts ([`JevUse::label_part`]), the newest counts. The request is the
/// authority on everything the label inherits: which rubric it belongs to,
/// and which version answered it — a label spelling a rubric or a version
/// its request contradicts grades nothing, and a label's own version is
/// read only where its request names none (every request written before
/// versions were recorded). A mark on a request row is that row's. Which
/// marks are the WINDOW's is a matter of time and not of naming
/// ([`judge_seat`]): every mark of the series written since the window's
/// first request, because a seat's marks lag its requests and a window
/// narrower than the marks a line needs (placement's 25 rows against 40
/// marks) fills with the labels of requests it has already left — a late
/// label of an older request of the SAME words and the SAME answering
/// version is a window sample, and one of other words or another version is
/// not.
///
/// The series starts after the newest request of a rubric NEWER than the
/// seat asks: a seat whose words went back starts recording on the requests
/// asked since, and the older run's rows behind that request revive nothing.
/// A request of an older rubric among the series' rows is left out and cuts
/// nothing — an older binary still writing beside a newer one restarts no
/// window.
///
/// Inside the series, counted back from the newest row: the requests are
/// read from the first request after the newest one another version
/// answered, and the marks from that same request on, less every mark
/// whose request another version answered — a row that names no version
/// (a timeout, a refusal) belongs to the version answering around it, so a
/// series no row names a version in is read whole, as every ledger was
/// before versions were recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct OnVersion<'rows> {
    /// The version answering now: the one the newest request that names a
    /// version was answered by — a label grading an older answer does not
    /// move it — or, in a series whose requests name none, the one the
    /// newest row that names a version says.
    pub model: Option<&'rows str>,
    /// The version met where the rows were cut, when they were: the newest
    /// other version a row of the series was answered by.
    pub cut: Option<&'rows str>,
    /// Every row of the series, in the ledger's order: the requests and
    /// control rows the seat's rubric asked and the marks that grade them —
    /// what a counter of the series reads (a card's today, week and days).
    /// Never a transition, and never a row that is neither asked nor a mark.
    pub rows: Vec<&'rows Value>,
    /// The rows the window's requests are counted from: every series row
    /// after the newest request another version answered.
    pub requests: Vec<&'rows Value>,
    /// The rows the marks are counted from: the series rows from the newest
    /// answering version's first request on, less every row another version
    /// answered — a label's version being its request's, so a late label of
    /// a request the older version answered is that version's comparison
    /// and not this one's, and cuts nothing of this one's either.
    pub marks: Vec<&'rows Value>,
    /// Beside each of [`Self::marks`], the confidence the answer it grades
    /// was given with (t-9468): a request's own, a label's its request's —
    /// the request is the authority on it as on the version — or the
    /// label's own copy where the request carries none (the guards' labels
    /// carry the deciding answer's lean). `None` where neither says one.
    pub mark_confidences: Vec<Option<f64>>,
    /// Beside each of [`Self::requests`], the confidence the same way: a
    /// request's own, or the copy its newest label carries.
    pub request_confidences: Vec<Option<f64>>,
}

impl<'rows> OnVersion<'rows> {
    /// How many of the series' requests the judgment's cadence counts.
    #[must_use]
    pub fn asked(&self) -> usize {
        self.requests
            .iter()
            .filter(|row| asked_something(row).is_some())
            .count()
    }

    /// The series' graded answers (t-9468): every mark
    /// ([`crate::jev::summary::AGREED`]) with the confidence its answer was
    /// given with, where the rows say one, beside the baseline's mark on the
    /// same fact — what a seat's act line is read off
    /// ([`crate::jev::threshold::calibrate`]).
    #[must_use]
    pub fn graded(&self) -> Vec<Graded> {
        self.marks
            .iter()
            .zip(&self.mark_confidences)
            .filter_map(|(row, confidence)| {
                Some(Graded {
                    confidence: *confidence,
                    agreed: AGREED.read(row).and_then(Value::as_bool)?,
                    baseline: BASELINE_AGREED.read(row).and_then(Value::as_bool),
                })
            })
            .collect()
    }

    /// The confidence each answered request of the series was given with —
    /// `None` for one whose rows say none, which no act line acts on.
    #[must_use]
    pub fn answered(&self) -> Vec<Option<f64>> {
        answered_confidences(
            self.requests
                .iter()
                .copied()
                .zip(self.request_confidences.iter().copied()),
        )
    }

    /// The judged window — the last `n` requests of the series — each with
    /// the confidence its answer was given with.
    #[must_use]
    pub fn window(&self, n: usize) -> Vec<(&'rows Value, Option<f64>)> {
        crate::jev::summary::last_asked_with(
            self.requests
                .iter()
                .copied()
                .zip(self.request_confidences.iter().copied()),
            n,
        )
    }

    /// The marks a seat acting from `line` is judged on (t-9468): with no
    /// line every mark, as before; with one, the marks of the answers the
    /// line lets act — an answer that says no confidence is not one of them.
    #[must_use]
    pub fn marks_from_line(&self, line: Option<u16>) -> Vec<&'rows Value> {
        self.marks
            .iter()
            .zip(&self.mark_confidences)
            .filter(|(_, confidence)| {
                line.is_none_or(|line| {
                    confidence.is_some_and(|confidence| crate::jev::reaches(confidence, line))
                })
            })
            .map(|(row, _)| *row)
            .collect()
    }
}

/// The confidence each answered request among `rows` was given with.
fn answered_confidences<'a>(
    rows: impl IntoIterator<Item = (&'a Value, Option<f64>)>,
) -> Vec<Option<f64>> {
    rows.into_iter()
        .filter(|(row, _)| asked_something(row) == Some(ANSWERED))
        .map(|(_, confidence)| confidence)
        .collect()
}

/// The version `row` names as the one that answered it, if it names one —
/// read off a request or a mark alone ([`is_request_or_mark`]), because a
/// row that is neither answered nothing whatever it spells under
/// [`MODEL`] (t-6284).
fn named_version(row: &Value) -> Option<&str> {
    if !is_request_or_mark(row) {
        return None;
    }
    MODEL
        .read(row)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
}

/// What a row says of the rubric that asked it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rubric {
    /// It names none: the first rubric's ([`UNVERSIONED_RUBRIC`]).
    Absent,
    /// It names this one.
    Named(u32),
    /// It names something that is not a version — null, a word, a negative,
    /// a zero: no rubric's evidence, and no reason to read it as any.
    Malformed,
}

impl Rubric {
    /// The rubric `row` names under [`RUBRIC_VERSION`].
    fn of(row: &Value) -> Self {
        RUBRIC_VERSION.read(row).map_or(Self::Absent, Self::from)
    }

    /// The version a rubric reads as, when it reads as one.
    const fn version(self) -> Option<u32> {
        match self {
            Self::Absent => Some(UNVERSIONED_RUBRIC),
            Self::Named(version) => Some(version),
            Self::Malformed => None,
        }
    }
}

impl From<&Value> for Rubric {
    fn from(value: &Value) -> Self {
        value
            .as_u64()
            .and_then(|version| u32::try_from(version).ok())
            .filter(|version| *version >= UNVERSIONED_RUBRIC)
            .map_or(Self::Malformed, Self::Named)
    }
}

/// Whether `row` is one a seat was asked — a request, or a control row the
/// routing seat writes beside a judgment already acted on.
fn is_asked(row: &Value) -> bool {
    asked_something(row).is_some() || is_control_row(row)
}

/// Whether `row` grades something: it carries a mark, says why it carries
/// none, or names a request.
fn is_mark(row: &Value) -> bool {
    AGREED.read(row).is_some()
        || NOT_COMPARED.read(row).is_some()
        || BASELINE_AGREED.read(row).is_some()
        || LABEL.read(row).is_some()
}

/// Whether `row` is a request of words newer than `seat` asks — the fence
/// a seat's series starts behind, and the one thing a standing read off a
/// ledger's text has to read a request line for. One reader for the series
/// and for both standing reads ([`series_from`], [`stands_on`]): a request
/// or a control row, stamped with a version — an integer, and newer —
/// because a label spelling a newer version fences nothing, and neither
/// does a request spelling something that is not a version.
fn fences(seat: &JevUse, row: &Value) -> bool {
    is_asked(row)
        && matches!(Rubric::of(row), Rubric::Named(version) if version > seat.rubric_version)
}

/// The name a request carries and a label repeats: a number as the writer
/// wrote it (the guards' `judged`), a word (an attempt, a worker), or two
/// joined by `:` (`query:notes`). A label spells a number as text, so a
/// text that reads as a number is that number — and nothing is copied out
/// of the row for a name of one key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Name<'row> {
    Number(u64),
    Text(&'row str),
    Joined(String),
}

impl<'row> Name<'row> {
    /// The name `row` carries under `keys` ([`JevUse::request_name`]).
    /// `None` for a row missing any of them.
    fn of(keys: &[&str], row: &'row Value) -> Option<Self> {
        match keys {
            [] => None,
            [key] => Self::one(row.get(*key)?),
            many => {
                let mut name = String::new();
                for (at, key) in many.iter().enumerate() {
                    if at > 0 {
                        name.push(':');
                    }
                    match row.get(*key)? {
                        Value::String(text) => name.push_str(text),
                        Value::Number(number) => name.push_str(&number.to_string()),
                        _ => return None,
                    }
                }
                Some(Self::Joined(name))
            }
        }
    }

    /// The name a label row repeats under [`LABEL`], spelled as `keys` would
    /// spell it.
    fn of_label(keys: &[&str], row: &'row Value) -> Option<Self> {
        let label = LABEL.read(row)?;
        if keys.len() > 1 {
            return match label {
                Value::String(text) => Some(Self::Joined(text.clone())),
                _ => None,
            };
        }
        Self::one(label)
    }

    fn one(value: &'row Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_u64().map(Self::Number),
            Value::String(text) => Some(
                text.parse::<u64>()
                    .map_or_else(|_| Self::Text(text.as_str()), Self::Number),
            ),
            _ => None,
        }
    }
}

/// What a row is to a seat's series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InSeries {
    /// A request or control row asked under the seat's rubric.
    Asked,
    /// A label naming this request (a ledger index) of the seat's rubric.
    Grades(usize),
}

/// Where `seat`'s series begins in `rows`: after the newest request of a
/// rubric newer than the seat asks now ([`fences`]) — the requests of words
/// the seat has since gone back from — or at the first row when there is
/// none.
fn series_from(seat: &JevUse, rows: &[Value]) -> usize {
    rows.iter()
        .rposition(|row| fences(seat, row))
        .map_or(0, |at| at + 1)
}

/// The request the label at `at` grades, by ledger index — of the requests
/// above it carrying the name it repeats (`named`, by
/// [`JevUse::request_name`]), the ones asked at the time the label names
/// ([`REQUEST_AT`]), or every one of them for a label naming no time — as
/// the seat's row says its name picks a request out ([`JevUse::names`],
/// t-6877 round 3):
///
/// - [`Naming::Request`]: the one such request. Two carrying an id made
///   for one request are two requests nothing tells apart, and the label
///   grades neither.
/// - [`Naming::Words`]: the one asking made at the time the label names. A
///   label naming no time grades nothing — the same words asked again carry
///   the same name, and the asking it meant may have been trimmed away with
///   another left in its place — and a time two askings share names
///   neither.
/// - [`Naming::Turn`]: the newest, standing for the turn's rows while they
///   could hand the label nothing different — the same rubric, the same
///   answering version, the same side of the series' start (`from`). Rows
///   that disagree could be two turns, and the label grades neither.
///
/// `None` too for a label no request above answers to. The reader guesses
/// no request, and never the newest of two.
fn request_of(
    seat: &JevUse,
    named: &HashMap<Name<'_>, Vec<usize>>,
    rows: &[Value],
    at: usize,
    from: usize,
) -> Option<usize> {
    let label = &rows[at];
    let asked = named.get(&Name::of_label(seat.request_name, label)?)?;
    let above = &asked[..asked.partition_point(|index| *index < at)];
    let when = match REQUEST_AT.read(label) {
        // A time that is not a time names no request.
        Some(when) => Some(when.as_i64()?),
        None if seat.names == Naming::Words => return None,
        None => None,
    };
    let mut found = above.iter().copied().filter(|index| {
        when.is_none_or(|when| AT.read(&rows[*index]).and_then(Value::as_i64) == Some(when))
    });
    let newest = found.next_back()?;
    match seat.names {
        Naming::Request | Naming::Words => found.next().is_none().then_some(newest),
        Naming::Turn => {
            let inherits = |index: usize| {
                (
                    Rubric::of(&rows[index]).version(),
                    named_version(&rows[index]),
                    index >= from,
                )
            };
            let one = inherits(newest);
            found.all(|index| inherits(index) == one).then_some(newest)
        }
    }
}

/// `rows` read as `seat`'s current series from its newest answering
/// version's first row on ([`OnVersion`]).
#[must_use]
pub fn on_the_newest_version<'rows>(seat: &JevUse, rows: &'rows [Value]) -> OnVersion<'rows> {
    let from = series_from(seat, rows);
    let current = |rubric: Rubric| rubric.version() == Some(seat.rubric_version);
    // Every request's name, whatever its rubric — a label of another
    // rubric's request joins that request, and leaves with it. Requests
    // alone: a control row is asked beside a request and is named by no
    // label.
    let mut named: HashMap<Name<'rows>, Vec<usize>> = HashMap::new();
    if !seat.request_name.is_empty() {
        for (at, row) in rows.iter().enumerate() {
            if asked_something(row).is_some()
                && let Some(name) = Name::of(seat.request_name, row)
            {
                named.entry(name).or_default().push(at);
            }
        }
    }
    // What each row is to the series, by ledger index: asked under the
    // seat's rubric, or grading such a request. The newest label of each
    // request — of each part of it, where the seat's labels grade parts
    // ([`JevUse::label_part`]).
    let mut kind: Vec<Option<InSeries>> = vec![None; rows.len()];
    let mut newest_label: HashMap<(usize, Option<Name<'rows>>), usize> = HashMap::new();
    for (at, row) in rows.iter().enumerate() {
        if TRANSITION.read(row).is_some() {
            continue;
        }
        if is_asked(row) {
            if at >= from && current(Rubric::of(row)) {
                kind[at] = Some(InSeries::Asked);
            }
            continue;
        }
        if !is_mark(row) {
            continue;
        }
        // A label spells no rubric of its own — its request carries it — but
        // one that does must agree with its request, and one that spells
        // something that is not a version grades nothing.
        let spelled = Rubric::of(row);
        if spelled == Rubric::Malformed {
            continue;
        }
        let Some(request) = request_of(seat, &named, rows, at, from) else {
            continue;
        };
        let Some(version) = Rubric::of(&rows[request]).version() else {
            continue;
        };
        if matches!(spelled, Rubric::Named(own) if own != version) {
            continue;
        }
        // Nor may it spell a version its request was not answered by.
        if let (Some(own), Some(answered)) = (named_version(row), named_version(&rows[request]))
            && own != answered
        {
            continue;
        }
        if request >= from && version == seat.rubric_version {
            kind[at] = Some(InSeries::Grades(request));
            newest_label.insert((request, Name::of(seat.label_part, row)), at);
        }
    }
    // The copy of the answer's confidence each request's newest label
    // carries — what a request that says none is read by (the guards').
    let mut labelled: HashMap<usize, f64> = HashMap::new();
    for (at, row) in rows.iter().enumerate() {
        if let Some(InSeries::Grades(request)) = kind[at]
            && newest_label.get(&(request, Name::of(seat.label_part, row))) == Some(&at)
            && let Some(confidence) = answer_confidence(row)
        {
            labelled.insert(request, confidence);
        }
    }
    // The series, and beside each row the version that answered it and
    // the confidence the answer was given with: a request's own, a label's
    // its request's.
    let mut series: Vec<&'rows Value> = Vec::new();
    let mut answered_by: Vec<Option<&'rows str>> = Vec::new();
    let mut given_with: Vec<Option<f64>> = Vec::new();
    for (at, row) in rows.iter().enumerate() {
        let (version, confidence) = match kind[at] {
            None => continue,
            Some(InSeries::Asked) => (
                named_version(row),
                answer_confidence(row).or_else(|| labelled.get(&at).copied()),
            ),
            Some(InSeries::Grades(request)) => {
                // Of two labels naming one request and one part of it, the
                // newest.
                if newest_label.get(&(request, Name::of(seat.label_part, row))) != Some(&at) {
                    continue;
                }
                // The request's version and confidence; the label's own
                // only where the request names none (every request written
                // before versions were recorded; a guard's request, whose
                // label carries the deciding answer's lean).
                (
                    named_version(&rows[request]).or_else(|| named_version(row)),
                    answer_confidence(&rows[request]).or_else(|| answer_confidence(row)),
                )
            }
        };
        series.push(row);
        answered_by.push(version);
        given_with.push(confidence);
    }
    let model = series
        .iter()
        .zip(&answered_by)
        .rev()
        .filter(|(row, _)| asked_something(row).is_some())
        .find_map(|(_, version)| *version)
        .or_else(|| answered_by.iter().rev().find_map(|version| *version));
    let another = |version: Option<&str>| version.is_some_and(|named| Some(named) != model);
    let requests_from = series
        .iter()
        .zip(&answered_by)
        .rposition(|(row, version)| asked_something(row).is_some() && another(*version))
        .map_or(0, |at| at + 1);
    let cut = answered_by
        .iter()
        .rev()
        .find_map(|version| version.filter(|_| another(*version)));
    let (marks, mark_confidences): (Vec<&'rows Value>, Vec<Option<f64>>) = series[requests_from..]
        .iter()
        .zip(&answered_by[requests_from..])
        .zip(&given_with[requests_from..])
        .filter(|((_, version), _)| !another(**version))
        .map(|((row, _), confidence)| (*row, *confidence))
        .unzip();
    OnVersion {
        model,
        cut,
        requests: series[requests_from..].to_vec(),
        marks,
        mark_confidences,
        request_confidences: given_with[requests_from..].to_vec(),
        rows: series,
    }
}

/// How many requests the judgment's cadence counts: the ones the newest
/// answering version was asked under the words the seat asks now
/// ([`on_the_newest_version`]). One reader, because the cadence
/// ([`judgment_due`]) and the countdown a screen draws
/// ([`rows_to_next_judgment`]) must land on the same row.
#[must_use]
pub fn asked_toward_judgment(seat: &JevUse, rows: &[Value]) -> usize {
    on_the_newest_version(seat, rows).asked()
}

/// Judge a seat on its own ledger rows, by the table's lines alone: the
/// window the seat's answer floor can be cleared on, the seat's apply wall,
/// and the [`crate::jev::summary::AGREED`] marks its rows carry — all of
/// them the current rubric's and the newest answering version's
/// ([`on_the_newest_version`]). `None` for a seat the table says never
/// rises.
///
/// The orchestration seats are judged here, in the process that writes their
/// rows (the window) and in the counter that shows them (`zo jev summary`),
/// from one function. zo's routing seat keeps its own reading of agreement —
/// the probe's answer beside the judgment's, axis by axis — and hands the
/// rest to the same [`judge`].
///
/// Where the seat stands is the current rubric's ([`standing`]): a standing
/// earned on one model version is not forgotten by the next, only judged on
/// the next one's rows — a seat already acting keeps acting until one of
/// them breaks a line ([`judge`]) — but one earned under other words is not
/// a standing under these.
///
/// The window's marks are the series' marks written since its first
/// request, reached back within the series to the width the agreement line
/// can be cleared on ([`marks_that_can_clear`], t-9468;
/// [`crate::jev::summary::marks_from`], t-9087): a seat whose marks are
/// sparser than its requests is judged on its marks, not held for them —
/// and on marks of the words it asks now and the version answering now
/// alone, however far back they reach.
#[must_use]
pub fn judge_seat(seat: &JevUse, rows: &[Value]) -> Option<Judged> {
    judge_seat_in(seat, rows, None)
}

/// [`judge_seat`] for a seat acting from `line` ([`judge_seat_at`]).
#[must_use]
pub fn judge_seat_in(seat: &JevUse, rows: &[Value], line: Option<u16>) -> Option<Judged> {
    judge_seat_at(seat, &on_the_newest_version(seat, rows), rows, line)
}

/// [`judge_seat`] on a series already read ([`on_the_newest_version`]) —
/// what a writer that has just asked whether a judgment is due
/// ([`judgment_due_on`]) hands over, so a full ledger's series is read
/// once and not twice on the way to one verdict.
#[must_use]
pub fn judge_seat_on(seat: &JevUse, version: &OnVersion<'_>, rows: &[Value]) -> Option<Judged> {
    judge_seat_at(seat, version, rows, None)
}

/// [`judge_seat_on`] for a seat that acts from `line` — the act line its
/// graded answers drew, as the product reads it
/// ([`crate::jev::threshold::Thresholds::line_of`], t-9468). Its marks are
/// the marks of the answers the line lets act
/// ([`OnVersion::marks_from_line`]) — the agreement, the negatives and the
/// baseline are read on what the seat would do, not on what it would leave
/// to today's path — and it is held to acting on enough of its window
/// ([`Line::ApplyShare`]). With no line every mark is read, and nothing
/// else changes: a seat with no line of its own is judged as it always was.
#[must_use]
pub fn judge_seat_at(
    seat: &JevUse,
    version: &OnVersion<'_>,
    rows: &[Value],
    line: Option<u16>,
) -> Option<Judged> {
    let floor = seat.answer_floor_permille?;
    let agreement_floor = seat.agreement_floor_permille?;
    let deadline_ms = seat.apply_deadline_ms?;
    let window_wanted = window_wanted_for(seat)?;
    let sample_floor = seat
        .agreement_rows_wanted
        .unwrap_or(A_WINDOW_OF_COMPARISONS);
    let held_with = version.window(window_wanted);
    let held: Vec<&Value> = held_with.iter().map(|(row, _)| *row).collect();
    let since_ms = held
        .first()
        .and_then(|row| AT.read(row).and_then(Value::as_i64))
        .unwrap_or(i64::MIN);
    let window = crate::jev::summary::summarize_rows(held.iter().copied(), i64::MIN);
    let marks = version.marks_from_line(line);
    // The window's marks are the series' marks written since its first
    // request — a late label of an older request of the same words counts,
    // a label of other words is not in the series at all — reached back
    // within the series while they hold fewer than the agreement line can
    // be cleared on with the negatives inside (t-9087, t-9468): the width
    // the rows' side reads off its own floor for the same reason
    // ([`rows_that_can_clear_forgiving`]), and not the sample floor, which
    // is how many marks the line may speak on, not how many it can pass on.
    let reach = marks_that_can_clear(seat).unwrap_or(sample_floor);
    let marks_since = crate::jev::summary::marks_from(marks.iter().copied(), since_ms, reach);
    let agreement = crate::jev::summary::agreement_rows(marks.iter().copied(), marks_since);
    let not_compared_by =
        crate::jev::summary::not_compared_words(marks.iter().copied(), marks_since);
    // The label's whole record on this version, not the window's: a seat
    // that is right almost every time is not held for being right lately.
    let record = crate::jev::summary::agreement_rows(marks.iter().copied(), i64::MIN);
    let verdict = judge(
        standing(seat, rows),
        &Evidence {
            window: &window,
            floor_permille: floor,
            deadline_ms,
            agreement_floor_permille: agreement_floor,
            agreement,
            agreement_rows_wanted: sample_floor,
            window_forgives: seat.window_forgives.unwrap_or(0),
            labels: None,
            fallbacks_in_a_row: failures_in_a_row_of(version.requests.iter().copied()),
            negatives_wanted: seat.negatives_wanted.unwrap_or(0),
            disagreed_on_record: record.disagreed(),
            baseline: seat.baseline,
            apply_share: apply_share_of(held_with.iter().copied(), line),
        },
    );
    Some(Judged {
        verdict,
        window,
        window_wanted,
        agreement,
        not_compared_by,
        control_rows: 0,
        model: version.model.map(str::to_string),
        cut: version.cut.map(str::to_string),
        act_line: line,
    })
}

/// The share of a window's answered requests a seat acting from `line`
/// acts on — the window as [`OnVersion::window`] hands it, each request
/// beside its answer's confidence — with the floor it must reach
/// ([`CALIBRATION`]); `None` for a seat with no line, which is not held to
/// one (t-9468). A request that says no confidence is one the line does not
/// act on.
#[must_use]
pub fn apply_share_of<'a>(
    window: impl IntoIterator<Item = (&'a Value, Option<f64>)>,
    line: Option<u16>,
) -> Option<ApplyShare> {
    let line = line?;
    let answered = answered_confidences(window);
    let acted = answered
        .iter()
        .flatten()
        .filter(|confidence| crate::jev::reaches(**confidence, line))
        .count();
    #[allow(clippy::cast_precision_loss)]
    let share = if answered.is_empty() {
        0.0
    } else {
        acted as f64 / answered.len() as f64
    };
    Some(ApplyShare {
        share_permille: permille(share),
        floor_permille: CALIBRATION.apply_share_floor_permille,
    })
}

/// How much of its window a seat acting from an act line acts on, and the
/// floor under which `auto` does not rise on the line (t-9468).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyShare {
    pub share_permille: u16,
    pub floor_permille: u16,
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

/// The fewest marks on which `seat`'s agreement can bound above its budget
/// with the disagreements its record must hold inside them (t-6342) — never
/// fewer than the sample floor. Forty at the 800‰ line with three
/// disagreements ([`crate::jev::NEGATIVES_WANTED`]). `None` for a seat that
/// never rises.
#[must_use]
pub fn marks_that_can_clear(seat: &JevUse) -> Option<usize> {
    let floor = seat.agreement_floor_permille?;
    let misses = seat.negatives_wanted?;
    let sample = seat.agreement_rows_wanted?;
    (sample.max(misses)..).find(|marks| {
        permille(crate::jev::summary::wilson_lower(
            marks - misses,
            *marks,
            WILSON_Z_95,
        )) >= floor
    })
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
/// Counted on the current rubric's and the newest answering version's
/// requests ([`asked_toward_judgment`]), for the same reason: after a change
/// of version — of the words, or of what answers them — the window starts
/// again, and a judgment before it is full again has only `too_few_rows` to
/// say.
#[must_use]
pub fn judgment_due(seat: &JevUse, rows: &[Value]) -> bool {
    judgment_due_on(seat, &on_the_newest_version(seat, rows), rows)
}

/// [`judgment_due`] on a series already read ([`on_the_newest_version`]).
#[must_use]
pub fn judgment_due_on(seat: &JevUse, version: &OnVersion<'_>, rows: &[Value]) -> bool {
    let asked = version.asked();
    let wanted = window_wanted_for(seat).unwrap_or(JUDGED_EVERY_ROWS);
    let at_boundary = asked >= wanted && (asked - wanted).is_multiple_of(JUDGED_EVERY_ROWS);
    let ending_it = standing(seat, rows) == Stand::Applying
        && failures_in_a_row_of(version.requests.iter().copied()) >= FALLBACKS_THAT_END_IT;
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
    /// Marks of the seat's cheapest baseline over the same ledger
    /// ([`crate::jev::summary::BASELINE_AGREED`], t-6342).
    pub baseline_compared: usize,
    /// Of those, the ones where the baseline was right.
    pub baseline_agreed: usize,
    /// Label rows that compared nothing and said why
    /// ([`crate::jev::summary::NOT_COMPARED`]) — a recall turn that touched no
    /// note, a pane nobody was in front of, a move nobody carried.
    pub not_compared: usize,
}

impl Agreement {
    /// The 95% Wilson lower bound on the agreed share, `None` when nothing
    /// was compared.
    #[must_use]
    pub fn lower_bound(&self) -> Option<f64> {
        (self.compared > 0).then(|| wilson_lower(self.agreed, self.compared, WILSON_Z_95))
    }

    /// The baseline's share over its marks — the line a seat's lower bound
    /// has to clear — `None` when the baseline marked nothing.
    #[must_use]
    pub fn baseline_share(&self) -> Option<f64> {
        (self.baseline_compared > 0).then(|| {
            #[allow(clippy::cast_precision_loss)]
            {
                self.baseline_agreed as f64 / self.baseline_compared as f64
            }
        })
    }

    /// The marks that said the judgment was wrong.
    #[must_use]
    pub const fn disagreed(&self) -> usize {
        self.compared.saturating_sub(self.agreed)
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
    /// How many disagreeing marks the record must hold before the agreement
    /// may speak ([`JevUse::negatives_wanted`], t-6342).
    pub negatives_wanted: usize,
    /// The disagreeing marks the answering version's whole record holds —
    /// the evidence that the seat's label can say no at all.
    pub disagreed_on_record: usize,
    /// The cheapest reader the seat is held against ([`JevUse::baseline`]).
    pub baseline: Baseline,
    /// How much of the window a seat acting from an act line acts on
    /// ([`apply_share_of`], t-9468) — `None` for a seat with no line.
    pub apply_share: Option<ApplyShare>,
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
    /// The answers' p95 is over the apply stage's deadline — the time the
    /// calls that answered took ([`Tally::p95_ms`]); a call that did not
    /// answer is the answered line's miss, forgiven there or not (t-9427).
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
    /// No mark at all, and label rows that said why each compares nothing
    /// (t-6342): the seat's label found nothing to grade — "no label" —
    /// which is not a seat still counting a thin sample.
    Unlabeled { withheld: usize },
    /// The label has said no fewer times than the seat asks of it (t-6342):
    /// agreement from a label that cannot say no is not evidence.
    OneSided { disagreed: usize, wanted: usize },
    /// Too few of the baseline's own marks to hold the seat to it (t-6342).
    TooFewBaseline { compared: usize, wanted: usize },
    /// The agreement's lower bound does not clear the baseline's share over
    /// the same marks (t-6342): the cheapest reader does as well.
    Baseline {
        bound_permille: u16,
        baseline_permille: u16,
    },
    /// A seat acting from an act line acts on too small a share of its
    /// window's answers (t-9468, [`ApplyShare`]).
    ApplyShare {
        share_permille: u16,
        floor_permille: u16,
    },
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
    // graded, not one that has passed. One whose every label row said why it
    // compares nothing has no label at all, which is a different sentence.
    let agreement = evidence.agreement;
    if agreement.compared < evidence.agreement_rows_wanted {
        if agreement.compared == 0 && agreement.not_compared > 0 {
            return Some(Line::Unlabeled {
                withheld: agreement.not_compared,
            });
        }
        return Some(Line::TooFewCompared {
            compared: agreement.compared,
            wanted: evidence.agreement_rows_wanted,
        });
    }
    // A label that has never said no is not evidence (t-6342), counted over
    // the answering version's whole record.
    if evidence.disagreed_on_record < evidence.negatives_wanted {
        return Some(Line::OneSided {
            disagreed: evidence.disagreed_on_record,
            wanted: evidence.negatives_wanted,
        });
    }
    let bound = agreement.lower_bound().map(permille)?;
    if bound < evidence.agreement_floor_permille {
        return Some(Line::Agreement {
            bound_permille: bound,
            floor_permille: evidence.agreement_floor_permille,
        });
    }
    // And a floor is not enough: the cheapest reader over the same marks
    // has to be beaten (t-6342).
    if evidence.baseline.binds() {
        if agreement.baseline_compared < evidence.agreement_rows_wanted {
            return Some(Line::TooFewBaseline {
                compared: agreement.baseline_compared,
                wanted: evidence.agreement_rows_wanted,
            });
        }
        let baseline = agreement.baseline_share().map(permille)?;
        if bound <= baseline {
            return Some(Line::Baseline {
                bound_permille: bound,
                baseline_permille: baseline,
            });
        }
    }
    // A seat acting from an act line acts on enough of what it answers, or
    // it does not rise on the line (t-9468).
    evidence
        .apply_share
        .filter(|apply| apply.share_permille < apply.floor_permille)
        .map(|apply| Line::ApplyShare {
            share_permille: apply.share_permille,
            floor_permille: apply.floor_permille,
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
        // it. Only a line it actually breaks takes it back — and a line
        // acting on little of the window still acts well on what it acts on:
        // the apply share is a floor to rise on (t-9468).
        (
            Stand::Applying,
            Some(
                Line::TooFewRows { .. }
                | Line::TooFewCompared { .. }
                | Line::Unlabeled { .. }
                | Line::TooFewBaseline { .. }
                | Line::ApplyShare { .. },
            ),
        ) => Verdict::Keep,
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
            Self::Unlabeled { .. } => "unlabeled",
            Self::OneSided { .. } => "one_sided",
            Self::TooFewBaseline { .. } => "too_few_baseline",
            Self::Baseline { .. } => "baseline",
            Self::ApplyShare { .. } => "apply_share",
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

/// Where `seat` stands: read back from the newest transition its ledger
/// recorded, and only while that transition was decided on the words the
/// seat asks now (t-6877).
///
/// No transition means it has never risen, which is what `auto` starts as.
/// A transition names the rubrics it was decided on
/// ([`transition_row`], [`RUBRIC_VERSIONS`]); one that names none was
/// decided under the first rubric ([`UNVERSIONED_RUBRIC`]) — every rise
/// written before transitions named one. A seat stands on it only while its
/// row asks exactly those words: a rise earned under version 1 is not a
/// rise under version 2, and a seat whose words went back to version 1
/// starts recording too — the newest transition is version 2's, and a
/// version 1 rise behind version 2's requests is behind the series
/// ([`on_the_newest_version`]) and revives nothing. A transition that names
/// something that is not a version stands for nothing.
///
/// Read newest row first, one row at a time (`stands_on`) — the step
/// [`standing_in`] walks a ledger's text with, so the two cannot read a row
/// apart. [`stand_from`] is the word of the newest transition whatever
/// rubric wrote it — what this reads, before it asks the rubric.
#[must_use]
pub fn standing(seat: &JevUse, rows: &[Value]) -> Stand {
    rows.iter()
        .rev()
        .find_map(|row| stands_on(seat, row))
        .unwrap_or(Stand::Recording)
}

/// What one row, read newest first, says of where `seat` stands — the one
/// step both standing readers take ([`standing`], [`standing_in`], t-6877
/// round 3). A request of words newer than the seat's ([`fences`]) puts it
/// behind its series, and so recording, whatever else the row carries; a
/// transition stands it where the transition says while it was decided on
/// the words the seat asks now, and at recording otherwise; any other row
/// says nothing, and the reader goes on to the row before it.
fn stands_on(seat: &JevUse, row: &Value) -> Option<Stand> {
    if fences(seat, row) {
        return Some(Stand::Recording);
    }
    let stand = stand_of(row)?;
    Some(if decided_on_these_words(seat, row) {
        stand
    } else {
        Stand::Recording
    })
}

/// [`standing`] read off a ledger's text, newest line first, parsing only
/// the lines that may say something (`may_speak`: a line holding an escape,
/// the transition's key, or a rubric that is not a plain integer no newer
/// than the seat's) and reading each of those with the step the rows are
/// read with (`stands_on`), so the text and the rows cannot disagree on
/// what a line says (t-6877 round 3, astra R2). An `auto` seat reads
/// its standing on every turn it is asked about, and a full ledger is
/// thousands of request rows around a transition or two: parsing every row
/// cost 35.6 ms at the 8 MiB cap (4,720 routing rows of the second
/// version, 2026-09-24, t-6346). A request line whose rubric is spelled as
/// a plain integer no newer than the seat's — every request line of a
/// seat's own rubric — is never parsed, which is what keeps the read at the
/// cost of the transition lines alone on a ledger the seat wrote itself.
#[must_use]
pub fn standing_in(seat: &JevUse, text: &str) -> Stand {
    let keys = QuotedKeys::new();
    text.lines()
        .rev()
        .filter(|line| may_speak(seat, line, &keys))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|row| stands_on(seat, &row))
        .unwrap_or(Stand::Recording)
}

/// The keys a ledger line is searched for, each spelling in its quotes as a
/// line spells a key: the transition's ([`TRANSITION`]) and the rubric's
/// ([`RUBRIC_VERSION`]).
struct QuotedKeys {
    transition: Vec<String>,
    rubric: Vec<String>,
}

impl QuotedKeys {
    fn new() -> Self {
        let quoted = |key: crate::jev::summary::LedgerKey| -> Vec<String> {
            key.spellings().map(|name| format!("\"{name}\"")).collect()
        };
        Self {
            transition: quoted(TRANSITION),
            rubric: quoted(RUBRIC_VERSION),
        }
    }
}

/// Whether a ledger line may say where `seat` stands ([`stands_on`]), read
/// off the text alone (t-6877 round 3, astra R2). `false` only for a line
/// that provably cannot: it holds no escape ([`holds_an_escape`]), spells no
/// transition's key ([`spells_a_key`]) and is no request of newer words as
/// it is spelled ([`may_fence_as_spelled`]). Everything else is parsed and
/// read as the rows are read; the text decides nothing itself. The escape
/// is looked for once, before the spellings only an escape could hide.
fn may_speak(seat: &JevUse, line: &str, keys: &QuotedKeys) -> bool {
    holds_an_escape(line)
        || spells_a_key(line, &keys.transition)
        || may_fence_as_spelled(seat, line, &keys.rubric)
}

/// Whether a ledger line holds a JSON escape. Where it holds none, every
/// key in it is spelled as the parser reads it, and a search of its text
/// for a key's spelling finds the key or proves it absent; where it holds
/// one, `"\u0074ransition"` is `transition` and `"rubric\u0056ersion"` the
/// rubric, and only the parser can say.
fn holds_an_escape(line: &str) -> bool {
    line.contains('\\')
}

/// Whether a ledger line spells one of `keys` (a key's spellings in their
/// quotes) anywhere — as a key of the row, of an object inside it, or as a
/// word in a value; the step the line is read with asks the row itself.
/// Read on a line that holds no escape ([`holds_an_escape`]), where a key's
/// spelling is the key.
fn spells_a_key(line: &str, keys: &[String]) -> bool {
    keys.iter().any(|key| line.contains(key.as_str()))
}

/// Whether a ledger line could be a request of words newer than `seat`'s
/// ([`fences`]), read off its spelling — `keys` are the spellings of
/// [`RUBRIC_VERSION`], each in its quotes — on a line that holds no escape
/// ([`holds_an_escape`]; an escape could spell the key as
/// `"rubric\u0056ersion"`). `false` only for a line that cannot be one:
/// every spelling of the key it carries is followed by a colon and a plain
/// integer no newer than the seat's rubric, the value ending there.
/// Everything else — a newer integer, a fraction, a word, a null, a key
/// with no colon after it, a line torn before the value ends — is `true`,
/// and the line is parsed and read as the rows are read. One search for
/// what every spelling opens with, then the spellings at that spot: a line
/// is searched once, not once per spelling.
fn may_fence_as_spelled(seat: &JevUse, line: &str, keys: &[String]) -> bool {
    line.match_indices(RUBRIC_KEY_OPENS).any(|(at, _)| {
        let opens = &line[at..];
        let Some(key) = keys.iter().find(|key| opens.starts_with(key.as_str())) else {
            return false;
        };
        let Some(value) = opens[key.len()..].trim_start().strip_prefix(':') else {
            return true;
        };
        let digits = value.trim_start();
        let end = digits
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(digits.len());
        let ends_there = {
            let rest = digits[end..].trim_start();
            rest.is_empty() || rest.starts_with([',', '}'])
        };
        match digits[..end].parse::<u32>() {
            Ok(version) if ends_there => version > seat.rubric_version,
            _ => true,
        }
    })
}

/// What every spelling of [`RUBRIC_VERSION`] opens with, as a ledger line
/// spells it — the one thing [`may_fence_as_spelled`] searches a line for.
const RUBRIC_KEY_OPENS: &str = "\"rubric";

/// Whether a transition row was decided on exactly the words `seat` asks
/// now: the rubrics it names ([`RUBRIC_VERSIONS`]; a single one under
/// [`RUBRIC_VERSION`]; none is the first rubric's) are the seat's own, and
/// only the seat's own — a transition decided on two questions at once
/// (the skills seat's, before it was two seats) stands for neither.
fn decided_on_these_words(seat: &JevUse, row: &Value) -> bool {
    let Some(mut named) = rubrics_of_transition(row) else {
        return false;
    };
    named.sort_unstable();
    named.dedup();
    named == [seat.rubric_version]
}

/// The rubrics a transition row names — `None` for one that names something
/// that is not a version.
fn rubrics_of_transition(row: &Value) -> Option<Vec<u32>> {
    match RUBRIC_VERSIONS.read(row) {
        Some(Value::Array(versions)) => versions
            .iter()
            .map(|version| match Rubric::from(version) {
                Rubric::Named(version) => Some(version),
                Rubric::Absent | Rubric::Malformed => None,
            })
            .collect(),
        Some(_) => None,
        None => Rubric::of(row).version().map(|version| vec![version]),
    }
}

/// The word of the newest transition a ledger's rows hold, whatever rubric
/// wrote it — the primitive [`standing`] reads before it asks the rubric.
/// A seat's standing is [`standing`]; this is what a reader that compares
/// the two standing reads of one ledger takes as "every row parsed".
///
/// No transition means it has never risen, which is what `auto` starts as.
#[must_use]
pub fn stand_from(rows: &[Value]) -> Stand {
    rows.iter()
        .rev()
        .find_map(stand_of)
        .unwrap_or(Stand::Recording)
}

/// [`stand_from`] read off a ledger's text, parsing only the lines that
/// may carry a transition — its key, or an escape that could spell it —
/// newest first (t-6346). A seat's standing is [`standing_in`], which asks the rubric
/// too.
#[must_use]
pub fn stand_in(text: &str) -> Stand {
    let keys = QuotedKeys::new();
    text.lines()
        .rev()
        .filter(|line| holds_an_escape(line) || spells_a_key(line, &keys.transition))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|row| stand_of(&row))
        .unwrap_or(Stand::Recording)
}

/// What one row says of a seat's standing: a rise, a fall, or nothing.
fn stand_of(row: &Value) -> Option<Stand> {
    match TRANSITION.read(row).and_then(Value::as_str) {
        Some(ROSE) => Some(Stand::Applying),
        Some(FELL) => Some(Stand::Recording),
        _ => None,
    }
}

/// The row a rise or a fall of `seat` appends. `None` for a verdict that
/// changed nothing — a ledger of "still recording" every twenty rows is a
/// ledger nobody can read. It names the rubric it was decided on
/// ([`RUBRIC_VERSIONS`], t-6877 — a list, because a transition written
/// while the skills seat asked two questions names two), so that a seat
/// whose words move on does not stand on it ([`standing`]).
#[must_use]
pub fn transition_row(
    seat: &JevUse,
    now_ms: i64,
    verdict: Verdict,
    window: &Tally,
) -> Option<Value> {
    let (word, line) = match verdict {
        Verdict::Rise => (ROSE, None),
        Verdict::Fall(line) => (FELL, Some(line)),
        Verdict::Hold(_) | Verdict::Keep => return None,
    };
    let mut row = json!({
        AT.canonical: now_ms,
        (TRANSITION.canonical): word,
        (RUBRIC_VERSIONS.canonical): [seat.rubric_version],
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
