//! What a use's ledger says about itself — the one counter every reader of
//! these rows shares: `zo jev summary --json` (§5), the auto judge that
//! promotes and demotes a seat on that evidence (§4), and the settings screen
//! that puts the same numbers beside the seat's row (§6).
//!
//! It lives here, beside the table it counts for, because the alternative is
//! three counters that drift: a ledger read one way by the command a person
//! runs, another way by the judge that acts on it, and a third by the screen
//! that shows it. A seat promoted on numbers the screen does not show is a
//! seat nobody can check.

use std::collections::BTreeMap;

use serde_json::Value;

/// How many new rows stand between one auto judgment and the next (§4).
///
/// The judgment is not made at the end of every turn: a bound that moves on
/// every row would rise and fall on a single answer, and a person watching the
/// screen would see a seat flicker rather than a seat decide. Twenty rows is
/// small enough that a seat earns its promotion within an hour's work and wide
/// enough that one slow answer cannot carry it.
pub const JUDGED_EVERY_ROWS: usize = 20;

/// The z for the lower endpoint of a two-sided 95% Wilson interval, which
/// the rise line is read against
/// (§4). Spelled once: a bound computed with a different z is a different
/// promise, and the screen's words name this one.
pub const WILSON_Z_95: f64 = 1.959_963_984_540_054;

/// A key a Jev ledger row carries, and every spelling it has been written
/// under.
///
/// One table, so that no reader spells a key itself. The spellings forked
/// once already and nobody saw it: `rerank-shadow` writes `elapsed_ms`,
/// `input_tokens` and `rubric_version` where every other ledger writes
/// `elapsedMs`, `inputTokens` and `rubricVersion` (measured 2026-09-18 over
/// 1,282 rows). A reader that spelled its own keys would have counted that
/// ledger's latencies as absent and said so with a straight face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerKey {
    /// What a row written from here on spells.
    pub canonical: &'static str,
    /// Spellings already on disk, read but never written.
    pub also: &'static [&'static str],
}

impl LedgerKey {
    /// The row's value under this key, under whichever spelling it carries.
    #[must_use]
    pub fn read<'row>(&self, row: &'row Value) -> Option<&'row Value> {
        row.get(self.canonical)
            .or_else(|| self.also.iter().find_map(|name| row.get(*name)))
    }

    /// Every spelling, canonical first — what a contract checks a writer
    /// against and what a reader is allowed to look for.
    pub fn spellings(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.canonical).chain(self.also.iter().copied())
    }
}

/// When the request was made, in milliseconds since the epoch.
pub const AT: LedgerKey = LedgerKey {
    canonical: "at",
    also: &[],
};
/// What became of it: `answered`, a failure token, or a door's refusal token.
pub const OUTCOME: LedgerKey = LedgerKey {
    canonical: "outcome",
    also: &[],
};
/// The call's wall time. Absent on a row the door refused — nothing was sent.
pub const ELAPSED_MS: LedgerKey = LedgerKey {
    canonical: "elapsedMs",
    also: &["elapsed_ms"],
};
/// Requests this row spent: one plus its retries.
pub const REQUESTS: LedgerKey = LedgerKey {
    canonical: "requests",
    also: &[],
};
/// Lines the door withheld from what was sent.
pub const REDACTED_LINES: LedgerKey = LedgerKey {
    canonical: "redactedLines",
    also: &[],
};
/// Whether the answer came from the memo rather than the wire. A memo hit is
/// not a request, so it never joins the latency population.
pub const CACHED: LedgerKey = LedgerKey {
    canonical: "cached",
    also: &[],
};
/// Input tokens the call billed.
pub const INPUT_TOKENS: LedgerKey = LedgerKey {
    canonical: "inputTokens",
    also: &["input_tokens"],
};
/// The judge's own note rather than a request: a rise or a fall it wrote down
/// (`crate::jev::promote`). It lives in the same file as the rows it was
/// decided on, so a counter has to know it is not one of them.
pub const TRANSITION: LedgerKey = LedgerKey {
    canonical: "transition",
    also: &[],
};
/// Whether the judgment named what the reader it would replace named — the
/// coordinator's pinned agent, the room the layout rule chose, what followed
/// a silence. Written by the seat that knows both, on the request's own row
/// or on a later label row; the judge counts every row that carries it
/// ([`agreement_since`]).
pub const AGREED: LedgerKey = LedgerKey {
    canonical: "agreed",
    also: &[],
};
/// Whether the seat's answer is what the product did, as a word: the row's
/// `routeUse` carries [`crate::jev::ROUTE_USE_APPLIED`] when it was, a
/// recording seat's mode word or [`crate::jev::ROUTE_USE_FALLBACK`] when it
/// was not. Written by the routing seat and the screen seats.
pub const ROUTE_USE: LedgerKey = LedgerKey {
    canonical: "routeUse",
    also: &[],
};
/// The same fact as a boolean, on the seats that reorder or place something
/// (recall, placement, the window's effort seat).
pub const APPLIED: LedgerKey = LedgerKey {
    canonical: "applied",
    also: &[],
};
/// The same fact as a screen walk says it: whether the hand went out.
pub const PRESSED: LedgerKey = LedgerKey {
    canonical: "pressed",
    also: &[],
};
/// The answered row a label row is about — the key that row carried as its
/// own name (a stall's key, a placed worker's id, a turn's attempt), so a
/// reader can join the label back to the request it grades without a second
/// spelling of either.
pub const LABEL: LedgerKey = LedgerKey {
    canonical: "label",
    also: &[],
};
/// When the request a label row grades was made — that request row's
/// [`AT`], copied by the writer that knows which request it graded
/// (t-6877). A name alone ([`LABEL`]) picks out one request only while it
/// is asked once: the recall and mention seats name a request by the
/// fingerprints of what was asked, and the same words asked again carry
/// the same name. With the time beside the name a label grades one
/// occurrence of a request and no other; a label that carries none grades a
/// request only where the seat's name picks one out without it — an id, or
/// a turn — and never for a seat that names a request by the words asked
/// ([`crate::jev::JevUse::names`], t-6877 round 3,
/// [`crate::jev::promote::on_the_newest_version`]).
pub const REQUEST_AT: LedgerKey = LedgerKey {
    canonical: "requestAt",
    also: &[],
};
/// Why a row that grades a request carries no [`AGREED`] mark, as a word —
/// the side of the comparison that had nothing to say (t-6342).
///
/// A label rule that finds nothing to compare writes this in place of a mark
/// it has no right to: a silence whose answer named no cause, a recall turn
/// that touched no note, a summons whose agent was never offered. The judge
/// reads only [`AGREED`]; this is for the reader who asks why a seat has so
/// few marks, and it is how a label that could not say no stops passing for
/// one that said yes.
pub const NOT_COMPARED: LedgerKey = LedgerKey {
    canonical: "notCompared",
    also: &[],
};
/// Whether the seat's cheapest baseline ([`crate::jev::Baseline`]) would have
/// been right on the fact a label row grades (t-6342) — written beside
/// [`AGREED`] by the writer that knows both, or alone on a row whose act was
/// the baseline's own (an effort move the rule made and carried). The judge
/// holds the seat's lower bound over its marks to this mark's share
/// ([`crate::jev::promote::Line::Baseline`]).
pub const BASELINE_AGREED: LedgerKey = LedgerKey {
    canonical: "baselineAgreed",
    also: &[],
};

/// The model that answered, as the response named it — the version, not the
/// alias the request asked for (`jev-1.13.0` for `jev-latest`). Written on
/// every row an answer came back on, and on no row that has none: a refusal
/// or a timeout names no version, because nothing answered it.
///
/// Read by the judge ([`crate::jev::promote::on_the_newest_version`]): an
/// alias moves when the vendor ships a version, and a floor or a window
/// fitted to one version silently measures the next (t-6187). Read off a
/// request or a mark alone ([`is_request_or_mark`]): a row that is neither
/// names no version, whatever it carries under this key.
pub const MODEL: LedgerKey = LedgerKey {
    canonical: "model",
    also: &[],
};

/// The model a step of a zo turn ran on, on the step governor's own rows —
/// never the version that answered anything, which is [`MODEL`]'s alone.
///
/// Those rows named it under [`MODEL`] until 2026-09-23 (t-6284), between
/// the step seat's judgments and its labels, one for every request of a
/// turn; read as versions, each step's chat model cut the seat's marks away.
/// Not read here — spelled here so the two keys are told apart in one table.
/// The rows already written are left out by [`is_request_or_mark`].
pub const STEP_MODEL: LedgerKey = LedgerKey {
    canonical: "stepModel",
    also: &[],
};

/// Why a screen walk's hand never went out although the judgment named a
/// control — the walk's own word for the stop; the two guards' words are the
/// core's ([`crate::screen_action::Stopped::word`], t-6187).
pub const BARRED: LedgerKey = LedgerKey {
    canonical: "barred",
    also: &[],
};
/// The kind of control a screen judgment named
/// ([`crate::guarded::ControlKind::word`], t-6187): what the press rule read,
/// and what a reader counts the controls a seat handed over by.
pub const CONTROL_KIND: LedgerKey = LedgerKey {
    canonical: "controlKind",
    also: &[],
};

/// The version of the words that asked a request — the seat's rubric as its
/// row names it ([`crate::jev::JevUse::rubric_version`]), written by the
/// writer that asked, on every request and control row. Read by the judge
/// to keep one rubric's evidence apart from another's (t-6877): a request
/// asked under other words answered another question, and its answer, its
/// marks and the rise they earned say nothing about the words the seat asks
/// now. A row that names none is the first rubric's
/// ([`crate::jev::questions::UNVERSIONED_RUBRIC`]) — every row written
/// before versions were recorded, and every row of a seat whose writer has
/// never versioned its words; a row that names something that is not a
/// version — null, a word, a negative — names no rubric at all and is no
/// series' evidence. A label row carries none of its own: the request it
/// names carries it ([`crate::jev::JevUse::request_name`]).
pub const RUBRIC_VERSION: LedgerKey = LedgerKey {
    canonical: "rubricVersion",
    also: &["rubric_version"],
};
/// The rubrics a transition was decided on — every version the seat asked
/// at the time, as its row said ([`crate::jev::promote::transition_row`],
/// t-6877). A seat stands on a transition only while it asks exactly those
/// words: a rise earned under other words is not its rise, and a transition
/// that names none was decided under the first rubric.
pub const RUBRIC_VERSIONS: LedgerKey = LedgerKey {
    canonical: "rubricVersions",
    also: &[],
};

/// Every key this module reads, so a contract can walk them.
pub const LEDGER_KEYS: &[LedgerKey] = &[
    AT,
    OUTCOME,
    ELAPSED_MS,
    REQUESTS,
    REDACTED_LINES,
    CACHED,
    INPUT_TOKENS,
    TRANSITION,
    AGREED,
    ROUTE_USE,
    APPLIED,
    PRESSED,
    LABEL,
    REQUEST_AT,
    NOT_COMPARED,
    BASELINE_AGREED,
    MODEL,
    BARRED,
    CONTROL_KIND,
    RUBRIC_VERSION,
    RUBRIC_VERSIONS,
];

/// The word a row carries when its judgment answered and passed its checks.
pub const ANSWERED: &str = "answered";

/// The word a control row carries in place of an outcome.
///
/// An acting routing seat skips the chat probe — its rows say `not_run` for
/// it — so the agreement line, which needs rows where both readers answered,
/// never fills once the seat acts. zo's `decision_shadow` therefore runs the
/// probe once more for a sample of its active turns, detached and after the
/// answer, and writes the result beside the judgment that turn already acted
/// on. That row is not a request the seat was asked: the judgment on it was
/// answered on the row before, and counting it again would double the seat's
/// answers and halve its latency. [`asked_something`] leaves it out of every
/// window, share and latency; only the agreement reads it.
pub const CONTROL: &str = "control";

/// One use's ledger over one window, counted.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tally {
    /// Rows in the window.
    pub rows: usize,
    /// Rows whose judgment answered and passed its checks.
    pub answered: usize,
    /// Rows the door refused before anything was sent — no key, Jev off, a
    /// workspace nobody consented to, a spent budget
    /// ([`crate::jev::door::Refused`]). They are in `rows` and named in
    /// `failures`, and they are left out of the answered share: a refusal
    /// says something about the person's key or consent and nothing about
    /// whether the seat answers when it is asked. Counted against the seat,
    /// three rows of a missing key held the routing seat at a bound of 0.728
    /// on a week it answered 25 of the 25 requests that left (2026-09-20).
    pub refused: usize,
    /// Rows whose judgment went over the wire, answered or not. A memo hit
    /// answered without asking, so it is counted as an answer and not as a
    /// call.
    pub called: usize,
    /// Rows that asked and did not answer, by outcome token, most frequent
    /// first, then by token so the order is the same twice.
    pub failures: Vec<(String, usize)>,
    /// Rows whose answer is what the product did ([`applied_of`]) — an
    /// acting seat's answers that cleared their checks. A recording seat has
    /// none, and a seat whose rows carry no such word has none either, which
    /// is not the same as a seat that fell back every time.
    pub applied: usize,
    /// Requests spent, retries included.
    pub requests: u64,
    /// Lines the door withheld across the window.
    pub redacted_lines: u64,
    /// Input tokens the window's calls billed.
    pub input_tokens: u64,
    /// Nearest-rank percentiles of the elapsed milliseconds of the calls that
    /// answered — how long the seat's answers take over the wire, the time
    /// the latency line holds to the apply stage's wall (§4).
    ///
    /// Only an answer has an answer's time (t-9427). A call that did not
    /// answer is the answered share's miss, and a malformed reply the schema
    /// line's too; a timeout's row carries the wall its stage gave up at,
    /// not a reply's time. Read as a latency sample it counted the same miss
    /// twice — once where the answer floor forgives it
    /// ([`crate::jev::JevUse::window_forgives`]) and again as the p95, which
    /// on fewer than twenty calls is the slowest. The recall seat's window
    /// on 2026-09-23 held 53 requests, 46 of them memo hits: its six answers
    /// took 239 to 720 ms and its one timeout carried the 1,500 ms wall plus
    /// two, and the acting seat was judged to fall on latency.
    pub p50_ms: Option<u64>,
    pub p95_ms: Option<u64>,
    /// Presses a screen seat's two guards stopped (t-6187), off the rows'
    /// [`BARRED`] — what the dashboard's drawer says of a screen seat
    /// (t-6277 D6), counted here once for every reader.
    pub guards: Guards,
    /// The controls a screen seat's rows named ([`CONTROL_KIND`]), and the
    /// ones a press cannot take back that it handed to the person.
    pub controls: Controls,
}

/// Presses a screen seat's two guards stopped, by guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Guards {
    /// The screen's own text told an assistant what to do
    /// ([`crate::screen_action::Stopped::Injected`]).
    pub instructed: usize,
    /// The screen was a wall in front of the page the goal expects
    /// ([`crate::screen_action::Stopped::Walled`]).
    pub walled: usize,
}

/// The controls a screen seat's rows named, and the ones it handed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Controls {
    /// Rows that named a control at all — a seat whose rows name none is not
    /// one that presses, which is how a reader tells the two apart without a
    /// list of seats of its own.
    pub named: usize,
    /// Controls a press cannot take back that an acting seat did not press
    /// and stepped back to the person with — under its floor, or stopped by
    /// a guard ([`crate::jev::ROUTE_USE_FALLBACK`]). A recording seat's row
    /// pressed nothing because it only records, and hands nothing over.
    pub destructive_held: usize,
}

impl Controls {
    fn count(&mut self, row: &Value) {
        let Some(kind) = CONTROL_KIND.read(row).and_then(Value::as_str) else {
            return;
        };
        self.named += 1;
        let fell_back =
            ROUTE_USE.read(row).and_then(Value::as_str) == Some(crate::jev::ROUTE_USE_FALLBACK);
        self.destructive_held +=
            usize::from(kind == crate::guarded::ControlKind::Destructive.word() && fell_back);
    }
}

impl Guards {
    fn count(&mut self, row: &Value) {
        use crate::screen_action::Stopped;
        match BARRED.read(row).and_then(Value::as_str) {
            Some(word) if word == Stopped::Injected.word() => self.instructed += 1,
            Some(word) if word == Stopped::Walled.word() => self.walled += 1,
            _ => {}
        }
    }
}

impl Tally {
    /// Rows the seat was actually asked: every row but the ones the door
    /// refused. The population the answered share and its bound are read
    /// over.
    #[must_use]
    pub const fn asked(&self) -> usize {
        self.rows.saturating_sub(self.refused)
    }

    /// The share of asked rows that answered, or `None` when nothing was
    /// asked — which is not zero, and a screen that drew it as zero would be
    /// lying about a seat nobody has used yet, or one the door has refused
    /// every time.
    #[must_use]
    pub fn answered_share(&self) -> Option<f64> {
        (self.asked() > 0).then(|| {
            #[allow(clippy::cast_precision_loss)]
            {
                self.answered as f64 / self.asked() as f64
            }
        })
    }

    /// The 95% Wilson lower bound on that share — what the rise line is read
    /// against (§4), and what a thin window is honest about: twenty answers
    /// out of twenty bound at 0.839, not at 1.0.
    #[must_use]
    pub fn answered_lower_bound(&self) -> Option<f64> {
        (self.asked() > 0).then(|| wilson_lower(self.answered, self.asked(), WILSON_Z_95))
    }

    /// The door's refusals among the failures, by token — the rows
    /// `refused` counts, said one token at a time, so a screen can say
    /// "no key 3 · not consented 60" without a table of the door's words.
    pub fn refusals(&self) -> impl Iterator<Item = &(String, usize)> + '_ {
        self.failures.iter().filter(|(token, _)| is_refusal(token))
    }
}

/// The lower end of the Wilson score interval for `successes` of `trials`.
///
/// Not `successes / trials` minus something: the plain share says a seat that
/// answered its only request is perfect, and a seat is not promoted on one
/// answer. Wilson is the bound that stays inside `[0, 1]` and tightens as the
/// window fills, which is the shape §4's rise line needs.
#[must_use]
pub fn wilson_lower(successes: usize, trials: usize, z: f64) -> f64 {
    if trials == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let (successes, trials) = (successes as f64, trials as f64);
    let share = successes / trials;
    let z2 = z * z;
    let centre = share + z2 / (2.0 * trials);
    let spread = z * ((share * (1.0 - share) + z2 / (4.0 * trials)) / trials).sqrt();
    ((centre - spread) / (1.0 + z2 / trials)).clamp(0.0, 1.0)
}

/// The fewest rows a window must hold before a floor can be cleared on it at
/// all — never fewer than [`JUDGED_EVERY_ROWS`].
///
/// A Wilson bound on a window of `n` rows that all answered is `1 / (1 +
/// z² / n)`, so a floor of 0.95 needs 73 rows and a window of twenty can
/// never clear it: the routing seat was judged on its last twenty rows against
/// that floor from 2026-09-17 until this was written, and the perfect window
/// it was asked for bounded at 0.839 (measured on the 28 rows it had). The
/// window is derived from the floor here, once, so a seat is never held to a
/// line no evidence can reach. A floor of a thousand per thousand has no such
/// window and returns [`usize::MAX`]; the use table's contract keeps floors
/// under it.
#[must_use]
pub fn rows_that_can_clear(floor_permille: u16) -> usize {
    rows_that_can_clear_forgiving(floor_permille, 0)
}

/// The fewest rows a window must hold before a floor can be cleared on it
/// while `forgives` of them missed. [`rows_that_can_clear`] is this width
/// with nothing forgiven.
///
/// A seat needs the forgiving width because the other one is a knife's edge:
/// it is the FEWEST rows on which `n` of `n` bound above the line, so at that
/// width the answer line reads "the last n were all answers", and one timeout
/// anywhere in the window takes the seat back to the start of its climb.
/// Measured on this machine's own ledgers (2026-09-22): recall, placement and
/// summon each held exactly 34 answers of 35 against a 900‰ floor and bounded
/// at 854‰ — one timeout apiece, at recall's row 973, placement's row 22 and
/// summon's row 40 — and summon's would have sat inside its window for 35
/// more requests. Forgiving one takes that floor's window from 35 rows to 53;
/// replayed over recall's own 1,005 rows it cut the seat's transitions from
/// eight rises and eight falls to five and five, which is the flicker
/// [`JUDGED_EVERY_ROWS`] exists to damp.
///
/// A floor of a thousand per thousand has no window at all and returns
/// [`usize::MAX`]; the use table's contract keeps floors under it.
#[must_use]
pub fn rows_that_can_clear_forgiving(floor_permille: u16, forgives: usize) -> usize {
    if floor_permille >= 1000 {
        return usize::MAX;
    }
    let share = f64::from(floor_permille) / 1000.0;
    let z2 = WILSON_Z_95 * WILSON_Z_95;
    // The closed form is the exact width for a perfect window, and a width a
    // perfect window cannot clear no forgiven window clears either, so it is
    // a floor on the search for every `forgives`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut rows = ((z2 * share / (1.0 - share)).ceil() as usize).max(JUDGED_EVERY_ROWS);
    // The closed form is exact; the bound is compared in the floor's own
    // units after flooring, so one more row covers a last-bit disagreement.
    while crate::jev::promote::permille(wilson_lower(
        rows.saturating_sub(forgives),
        rows,
        WILSON_Z_95,
    )) < floor_permille
    {
        rows += 1;
    }
    rows
}

/// The nearest-rank percentile of an already sorted population.
#[must_use]
pub fn percentile(sorted: &[u64], share: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let rank = ((share * sorted.len() as f64).ceil() as usize).max(1);
    sorted.get(rank.min(sorted.len()) - 1).copied()
}

/// Whether a row is a request this use made, and its outcome if so.
///
/// A ledger holds three kinds of line: the rows a use wrote when it asked
/// something, the notes the judge wrote about them, and the control rows the
/// routing seat writes when it runs the probe it skipped ([`CONTROL`]).
/// Counting the second kind as the first would lower every seat's answered
/// share by the very act of judging it; counting the third would raise it by
/// the very act of checking it.
#[must_use]
pub fn asked_something(row: &Value) -> Option<&str> {
    if TRANSITION.read(row).is_some() {
        return None;
    }
    OUTCOME
        .read(row)
        .and_then(Value::as_str)
        .filter(|outcome| *outcome != CONTROL)
}

/// Whether a row is a control row — the probe run once more beside a
/// judgment already acted on ([`CONTROL`]). Read here, beside the counter
/// that leaves such a row out, so the reader that joins it back in for the
/// agreement spells the word the same way.
#[must_use]
pub fn is_control_row(row: &Value) -> bool {
    OUTCOME.read(row).and_then(Value::as_str) == Some(CONTROL)
}

/// Whether a row is one the judge weighs: a request ([`asked_something`]), a
/// mark that grades one ([`AGREED`]), or a control row the routing seat
/// joins to its agreement ([`is_control_row`]).
///
/// A ledger holds other lines beside them — the judge's own notes, and a
/// seat's bookkeeping that nobody asked or graded, such as the `step` row
/// zo's step governor files for every request of a turn between its seat's
/// judgments and labels. None of those is evidence of anything that
/// answered, so the version a row names ([`MODEL`]) is read off these rows
/// alone: every step row written before 2026-09-23 still names the chat
/// model it ran on there (t-6284).
#[must_use]
pub fn is_request_or_mark(row: &Value) -> bool {
    asked_something(row).is_some() || is_control_row(row) || AGREED.read(row).is_some()
}

/// The last `n` requests, counted — the window §4 judges on.
///
/// A count and not a clock: a seat asked twice a day and one asked twice a
/// minute earn their promotion on the same amount of evidence, and a window
/// measured in hours would hand the busy one a verdict off a hundred rows
/// while the quiet one never filled its own.
#[must_use]
pub fn summarize_last(rows: &[Value], n: usize) -> Tally {
    summarize_rows(last_asked(rows, n), i64::MIN)
}

/// The last `n` requests themselves, oldest first — the rows a window is
/// counted from, for a reader that needs more of them than the tally keeps
/// (what each answered, beside what the probe answered).
#[must_use]
pub fn last_asked(rows: &[Value], n: usize) -> Vec<&Value> {
    last_asked_of(rows.iter(), n)
}

/// [`last_asked`] over rows already picked out — one seat's series
/// ([`crate::jev::promote::OnVersion::requests`]), held by reference.
#[must_use]
pub fn last_asked_of<'a>(rows: impl IntoIterator<Item = &'a Value>, n: usize) -> Vec<&'a Value> {
    last_asked_with(rows.into_iter().map(|row| (row, ())), n)
        .into_iter()
        .map(|(row, ())| row)
        .collect()
}

/// [`last_asked_of`] over rows that carry something beside them — the
/// judged window with each request's confidence (t-9468) — picked by the
/// one rule, so the window and what rides beside it cannot fall apart.
#[must_use]
pub fn last_asked_with<'a, T>(
    rows: impl IntoIterator<Item = (&'a Value, T)>,
    n: usize,
) -> Vec<(&'a Value, T)> {
    let mut asked: Vec<(&Value, T)> = rows
        .into_iter()
        .filter(|(row, _)| asked_something(row).is_some())
        .collect();
    let from = asked.len().saturating_sub(n);
    asked.split_off(from)
}

/// How many requests at the end of the ledger did not answer, stopping at the
/// first that did — §4's "in a row", read from the rows themselves rather
/// than from a counter some other process would have to keep.
///
/// A row the door refused is stepped over, not counted: nothing was sent, so
/// nothing fell back on the wire. A key that goes missing for an afternoon
/// would otherwise take an acting seat down on the third refusal and make it
/// earn its place again from nothing once the key is back — the door's rule
/// is the person's to lift, and the seat is not what it says anything about.
#[must_use]
pub fn failures_in_a_row(rows: &[Value]) -> u32 {
    failures_in_a_row_of(rows.iter())
}

/// [`failures_in_a_row`] over rows already picked out — one seat's series,
/// held by reference.
#[must_use]
pub fn failures_in_a_row_of<'a>(rows: impl DoubleEndedIterator<Item = &'a Value>) -> u32 {
    let mut held = 0;
    for row in rows.rev() {
        match asked_something(row) {
            None => continue,
            Some(ANSWERED) => break,
            Some(token) if is_refusal(token) => continue,
            Some(_) => held += 1,
        }
    }
    held
}

/// How often, at or after `since_ms`, a row said the judgment agreed with the
/// reader it would replace — one comparison per row that carries
/// [`AGREED`], asked rows and label rows alike — beside what the seat's
/// baseline said ([`BASELINE_AGREED`]) and how many rows said why they
/// compare nothing ([`NOT_COMPARED`], t-6342).
#[must_use]
pub fn agreement_since(rows: &[Value], since_ms: i64) -> crate::jev::promote::Agreement {
    agreement_rows(rows.iter(), since_ms)
}

/// Where a judgment window's marks are counted from (t-9087): the window's
/// first request, `since_ms` — reached back, when fewer than `wanted` marks
/// ([`AGREED`]) were written since, to the time of the `wanted`th newest, or
/// to the first row when the rows hold fewer. A time is a cut, so marks
/// sharing the time the cut falls on all count: a window reached back holds
/// at least `wanted` marks, not always exactly that many. The judge asks for
/// as many as its agreement line can be cleared on
/// ([`crate::jev::promote::marks_that_can_clear`], t-9468).
///
/// `rows` are one seat's series ([`crate::jev::promote::OnVersion::marks`]):
/// the reach back is as far as the marks of the words the seat asks now and
/// the version answering now go, and a ledger's raw rows would carry it into
/// other words' marks.
///
/// The window is a count of requests, as wide as the seat's answer floor
/// needs; how many marks those requests earn is the seat's own. The notify
/// seat marks a ring only when the person was at the window or turned to it
/// — 110 of the 444 rings this machine's ledger held on 2026-09-25 — so its
/// 53-ring window held 15 marks, and the judge said `too_few_compared` with
/// 110 in hand; the placement seat's 25 requests held 9 of its 87. The
/// judge asks for marks in hand, and a window that already holds them reads
/// its own marks alone.
#[must_use]
pub fn marks_from<'a>(
    rows: impl IntoIterator<Item = &'a Value>,
    since_ms: i64,
    wanted: usize,
) -> i64 {
    // Read the way [`agreement_rows`] reads a mark's time.
    let mut marked: Vec<i64> = rows
        .into_iter()
        .filter(|row| AGREED.read(row).and_then(Value::as_bool).is_some())
        .map(|row| AT.read(row).and_then(Value::as_i64).unwrap_or(0))
        .collect();
    if marked.iter().filter(|at| **at >= since_ms).count() >= wanted {
        return since_ms;
    }
    marked.sort_unstable_by(|older, newer| newer.cmp(older));
    marked
        .get(wanted.saturating_sub(1))
        .map_or(i64::MIN, |at| (*at).min(since_ms))
}

/// [`agreement_since`] over rows already picked out — one local day of a
/// ledger, for the trend a screen draws beside the week.
#[must_use]
pub fn agreement_rows<'a>(
    rows: impl IntoIterator<Item = &'a Value>,
    since_ms: i64,
) -> crate::jev::promote::Agreement {
    let mut agreement = crate::jev::promote::Agreement::default();
    for row in rows {
        if !at_or_after(row, since_ms) {
            continue;
        }
        if let Some(agreed) = AGREED.read(row).and_then(Value::as_bool) {
            agreement.compared += 1;
            agreement.agreed += usize::from(agreed);
        }
        agreement.not_compared += usize::from(withheld(row).is_some());
        if let Some(baseline) = BASELINE_AGREED.read(row).and_then(Value::as_bool) {
            agreement.baseline_compared += 1;
            agreement.baseline_agreed += usize::from(baseline);
        }
    }
    agreement
}

/// Why the rows at or after `since_ms` that grade a request carry no mark,
/// word by word (t-9556): each word a row wrote under [`NOT_COMPARED`] — a
/// ring nobody was there to turn to, a move nobody carried, a summons
/// whose model was pinned — with how many rows wrote it. They are the rows
/// [`agreement_rows`] counts as `not_compared`, told apart: a count alone
/// cannot say that a seat is waiting on a person to move a pane rather than
/// on more answers (t-9427, review (d)①). The words are the writers' own;
/// a value that is not a word is keyed by its JSON, so the counts always
/// add up to `not_compared`.
#[must_use]
pub fn not_compared_words<'a>(
    rows: impl IntoIterator<Item = &'a Value>,
    since_ms: i64,
) -> BTreeMap<String, usize> {
    let mut words: BTreeMap<String, usize> = BTreeMap::new();
    for word in rows
        .into_iter()
        .filter(|row| at_or_after(row, since_ms))
        .filter_map(withheld)
    {
        let word = word
            .as_str()
            .map_or_else(|| word.to_string(), str::to_string);
        *words.entry(word).or_default() += 1;
    }
    words
}

/// What a row that grades a request says in place of a mark
/// ([`NOT_COMPARED`]) — `None` for a row that carries one ([`AGREED`]),
/// whatever else it spells, or says nothing. One reader for the count and
/// the words, so they cannot disagree on which rows compared nothing.
fn withheld(row: &Value) -> Option<&Value> {
    if AGREED.read(row).and_then(Value::as_bool).is_some() {
        return None;
    }
    NOT_COMPARED.read(row)
}

/// Whether a row was written at or after `since_ms` — read the way every
/// counter here reads a row's time: a row with none is at the epoch.
fn at_or_after(row: &Value, since_ms: i64) -> bool {
    AT.read(row).and_then(Value::as_i64).unwrap_or(0) >= since_ms
}

/// How many of a seat's graded answers fell in one stretch of confidence,
/// and how many of those agreed (t-6342).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConfidenceTally {
    pub marks: usize,
    pub agreed: usize,
}

/// The stretches a seat's confidence curve is drawn over: five fifths.
pub const CONFIDENCE_CURVE_BINS: usize = 5;

/// A seat's graded answers — each its confidence and whether its mark
/// agreed — counted per fifth of confidence (t-6342): the curve TypeSafe's
/// guide reads a threshold off ("plot confidence against accuracy on your
/// data"), and the evidence a seat's [`crate::jev::ConfidenceBands`] are
/// moved on. A reading outside `0..=1` is counted nowhere; `1.0` is the top
/// fifth's.
#[must_use]
pub fn confidence_curve(
    graded: impl IntoIterator<Item = (f64, bool)>,
) -> [ConfidenceTally; CONFIDENCE_CURVE_BINS] {
    let mut curve = [ConfidenceTally::default(); CONFIDENCE_CURVE_BINS];
    for (confidence, agreed) in graded {
        if !(0.0..=1.0).contains(&confidence) {
            continue;
        }
        let fifth =
            ((confidence * CONFIDENCE_CURVE_BINS as f64) as usize).min(CONFIDENCE_CURVE_BINS - 1);
        curve[fifth].marks += 1;
        curve[fifth].agreed += usize::from(agreed);
    }
    curve
}

/// The same graded answers counted per band of `seat`'s own lines, in
/// [`crate::jev::Band::ALL`]'s order — `None` for a seat that names no
/// bands.
#[must_use]
pub fn band_tally(
    seat: &crate::jev::JevUse,
    graded: impl IntoIterator<Item = (f64, bool)>,
) -> Option<[ConfidenceTally; 3]> {
    let bands = seat.confidence_bands?;
    let mut tally = [ConfidenceTally::default(); 3];
    for (confidence, agreed) in graded {
        let Some(band) = bands.band_of(confidence) else {
            continue;
        };
        let Some(slot) = crate::jev::Band::ALL.iter().position(|each| *each == band) else {
            continue;
        };
        tally[slot].marks += 1;
        tally[slot].agreed += usize::from(agreed);
    }
    Some(tally)
}

/// Whether a row's answer is what the product did, read off whichever of the
/// three spellings the row carries — [`ROUTE_USE`] first, because a screen
/// walk writes it beside [`PRESSED`] and the word is the more exact of the
/// two; then [`APPLIED`]; then [`PRESSED`]. `None` for a row that says
/// nothing about it: a refusal, a failure, or a seat with no apply stage.
///
/// One reader, because three seats spell the fact three ways and a counter
/// that read one of them would show the recall seat applying nothing.
#[must_use]
pub fn applied_of(row: &Value) -> Option<bool> {
    if let Some(word) = ROUTE_USE.read(row).and_then(Value::as_str) {
        return Some(word == crate::jev::ROUTE_USE_APPLIED);
    }
    APPLIED
        .read(row)
        .and_then(Value::as_bool)
        .or_else(|| PRESSED.read(row).and_then(Value::as_bool))
}

/// Whether an outcome token is the door's — a request that was never sent.
#[must_use]
pub fn is_refusal(token: &str) -> bool {
    crate::jev::door::Refused::from_token(token).is_some()
}

/// Count the rows of one ledger whose `at` is at or after `since_ms`.
///
/// Rows the door refused never reached the wire, so they are neither answers
/// nor failures of the judgment — they are counted in `rows` and named in
/// `failures` by their own token, because a seat that sends nothing because
/// a workspace is not consented must not read as a seat that is working.
#[must_use]
pub fn summarize(rows: &[Value], since_ms: i64) -> Tally {
    summarize_rows(rows.iter(), since_ms)
}

/// [`summarize`] over rows already picked out — the last `n` of a ledger, or
/// a window some other reader holds by reference.
#[must_use]
pub fn summarize_rows<'a>(rows: impl IntoIterator<Item = &'a Value>, since_ms: i64) -> Tally {
    let mut tally = Tally::default();
    let mut failures: BTreeMap<&str, usize> = BTreeMap::new();
    let mut elapsed: Vec<u64> = Vec::new();
    for row in rows {
        if asked_something(row).is_none() {
            continue;
        }
        let at = AT.read(row).and_then(Value::as_i64).unwrap_or(0);
        if at < since_ms {
            continue;
        }
        tally.rows += 1;
        tally.applied += usize::from(applied_of(row) == Some(true));
        tally.requests += REQUESTS.read(row).and_then(Value::as_u64).unwrap_or(0);
        tally.redacted_lines += REDACTED_LINES
            .read(row)
            .and_then(Value::as_u64)
            .unwrap_or(0);
        tally.input_tokens += INPUT_TOKENS.read(row).and_then(Value::as_u64).unwrap_or(0);
        tally.guards.count(row);
        tally.controls.count(row);
        let outcome = OUTCOME
            .read(row)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if outcome == ANSWERED {
            tally.answered += 1;
        } else if let Some(token) = OUTCOME.read(row).and_then(Value::as_str) {
            *failures.entry(token).or_default() += 1;
            tally.refused += usize::from(is_refusal(token));
        }
        let cached = CACHED.read(row).and_then(Value::as_bool).unwrap_or(false);
        if let Some(ms) = ELAPSED_MS
            .read(row)
            .and_then(Value::as_u64)
            .filter(|_| !cached)
        {
            tally.called += 1;
            // An answer's time, and no miss's (t-9427, [`Tally::p95_ms`]).
            if outcome == ANSWERED {
                elapsed.push(ms);
            }
        }
    }
    elapsed.sort_unstable();
    tally.p50_ms = percentile(&elapsed, 0.50);
    tally.p95_ms = percentile(&elapsed, 0.95);
    let mut failures: Vec<(String, usize)> = failures
        .into_iter()
        .map(|(token, count)| (token.to_string(), count))
        .collect();
    failures.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    tally.failures = failures;
    tally
}

#[cfg(test)]
mod tests;
