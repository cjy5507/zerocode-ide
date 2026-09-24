//! Every Jev seat's ledger, counted and put in one answer — `zo jev summary
//! --json` (docs/design/jev-settings-20260917.md §5).
//!
//! One reader, because the numbers have three audiences and they must agree:
//! the person who runs the command, the auto judge that promotes a seat on
//! them (§4), and the settings screen, which asks zo rather than counting for
//! itself (the window↔zo exec boundary). `tools/decision_shadow_summary.py`
//! read one seat of the seven and is replaced by this.
//!
//! A seat's ledger is found by its name and not by a road written here: the
//! seats zo owns append under the project's state directory, the seats the
//! window owns append under the Jev requests directory in the settings home,
//! and a reader that knew which was which would carry a second copy of a fact
//! the table already holds. Both roots are asked for the file the table names.

use std::path::{Path, PathBuf};

use serde_json::Value;
use zerocode_core::jev::count::REQUESTS_DIR;
use zerocode_core::jev::promote::{self, Stand, Verdict};
use zerocode_core::jev::recent::{self, Decision};
use zerocode_core::jev::summary::{self, Tally};
use zerocode_core::jev::{JEV_USES, JevMode, JevUse};

/// What a reader of this report needs from the table's own counter, re-said
/// here so a caller does not have to take a dependency on the shared core to
/// read an answer this crate already built.
pub use zerocode_core::jev::promote::Agreement as SeatAgreement;
pub use zerocode_core::jev::recent::Decision as SeatDecision;
pub use zerocode_core::jev::summary::{
    JUDGED_EVERY_ROWS, Tally as SeatTally, asked_something, failures_in_a_row, summarize_last,
};

use super::shadow_ledger::shadow_ledger_dir;

/// How many days the wider window looks back (§5: "오늘/7일").
///
/// A week rather than a month because a seat's answer rate is a statement
/// about the model and the prompt as they are now, and both move faster than
/// a month: a window that outlives its subject reports an average of two
/// different things.
pub const WINDOW_DAYS: i64 = 7;

const MS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

/// One local day of a seat's rows, for the trend the dashboard draws beside
/// the week (docs/design/jev-dashboard-and-perfection-20260921.md §2 (c)):
/// the same counter as the week, over the rows whose `at` fell in the day.
#[derive(Debug, Clone, PartialEq)]
pub struct DayTally {
    /// Midnight the day began at, in the person's own zone.
    pub start_ms: i64,
    pub tally: Tally,
    /// The comparisons the seat's rows carried that day — the marks its own
    /// writer left, counted the way the judge counts them.
    pub agreement: promote::Agreement,
}

/// One seat, counted.
#[derive(Debug, Clone, PartialEq)]
pub struct SeatReport {
    pub id: &'static str,
    pub setting: &'static str,
    pub mode: JevMode,
    /// The ledger file the table names for this seat.
    pub ledger: &'static str,
    /// Where it was found, when it exists. A seat that has never been asked
    /// has no file, which is not an error and not a zero answer rate.
    pub found: Option<PathBuf>,
    /// Today's and the week's rows of the seat's current series — the rows
    /// asked under the words the seat asks now ([`promote::on_the_newest_version`],
    /// t-6877), which is what the judge reads; a rubric that moved on starts
    /// the card again as it starts the judge.
    pub today: Tally,
    pub week: Tally,
    /// What the week's billed input tokens cost, when the price table names
    /// the judgment's model — every row's, whatever words asked it.
    pub cost_usd: Option<f64>,
    /// The model the seat asks for — the person's pin (`smart.jevModel`) or
    /// the vendor's alias (t-6187). The same for every seat, and on every
    /// seat's line because it is the other half of what the line says: which
    /// id was asked, and which version answered.
    pub asked_model: String,
    /// The version the newest request that names one was answered by
    /// ([`promote::on_the_newest_version`]) — `None` for a seat whose rows
    /// name no version, which is every row written before versions were.
    pub model: Option<String>,
    /// The version the seat's rows were cut away from at the last change of
    /// version, when there was one.
    pub cut: Option<String>,
    /// The share the seat's answers must bound above before `auto` may rise
    /// (§4), in parts per thousand — `None` for a seat that never rises.
    pub rise_floor_permille: Option<u16>,
    /// Whether the judged window's lower bound clears that line. `None` when
    /// the seat never rises or the window is empty.
    pub clears_rise_floor: Option<bool>,
    /// The window the judge read — the last requests the seat's floor can be
    /// cleared on — with how often the judgment agreed with the reader it
    /// would replace over it. `None` for a seat that never rises. It is not
    /// the week: the week is a clock and the window is a count, and the
    /// numbers a seat is promoted on are these.
    pub judged: Option<promote::Judged>,
    /// How often, over the week, a row of this seat said its judgment agreed
    /// with what then happened — every row that carries the seat's `agreed`
    /// mark ([`summary::agreement_since`]), whether the seat rises or not.
    /// The judged window's agreement above is the number a seat is promoted
    /// on; this one is the number a seat that never rises (recall) still
    /// earns, and the one a week's trend is read from (t-5806).
    pub agreement_week: promote::Agreement,
    /// The requests the judgment's cadence counts: every one the newest
    /// answering version was asked ([`promote::asked_toward_judgment`]).
    pub asked_toward_judgment: usize,
    /// The cheapest reader the seat is held against, as the table names it
    /// ([`zerocode_core::jev::Baseline::kind`], t-6342).
    pub baseline: &'static str,
    /// How many disagreeing marks the table asks the seat's record to hold.
    pub negatives_wanted: Option<usize>,
    /// Where the seat stands, read back from its own transitions under the
    /// words it asks now ([`promote::standing`]).
    pub stand: Stand,
    /// Whether the seat acts right now: its mode, and for `auto` its standing.
    pub applies: bool,
    /// Today and the [`WINDOW_DAYS`]` - 1` days before it, oldest first.
    pub days: Vec<DayTally>,
    /// The last requests, newest first — as many as the caller asked for
    /// ([`report_with_recent`]); none for a caller that wants the numbers.
    pub recent: Vec<Decision>,
}

impl SeatReport {
    /// What the judge says of the judged window — `None` for a seat whose
    /// `auto` can never rise, which is not a seat with a bad verdict.
    #[must_use]
    pub fn verdict(&self) -> Option<Verdict> {
        self.judged.as_ref().map(|judged| judged.verdict)
    }

    /// How many more rows before the next auto judgment (§4), for a seat that
    /// can rise at all — counted on every request the ledger holds, which is
    /// the count the judge's cadence runs on, and read from the judge itself
    /// so the countdown and the judgment land on the same row.
    #[must_use]
    pub fn rows_to_next_judgment(&self) -> Option<usize> {
        promote::rows_to_next_judgment(zerocode_core::jev::jev_use(self.id)?, self.asked_toward_judgment)
    }
}

/// The deadline the seat's apply stage falls back on, read from that stage
/// rather than written here.
///
/// A seat has one exactly when it has somewhere to act: today that is the
/// routing judgment, whose 1.5 s the router already spells. A contract holds
/// the two together, so a seat that gains a rise line without an apply stage
/// to time it cannot slip through.
#[must_use]
pub fn deadline_ms_for(seat: &JevUse) -> Option<u64> {
    seat.apply_deadline_ms
}

/// Both roots a seat's ledger may live under, in the order they are asked.
#[must_use]
pub fn ledger_roots(cwd: &Path) -> [PathBuf; 2] {
    [shadow_ledger_dir(cwd), runtime::default_config_home().join(REQUESTS_DIR)]
}

/// The rows of one ledger, newest last, skipping lines that do not parse —
/// a half-written line at the tail is a crash's leftover, not a reason to
/// refuse the other nine hundred.
#[must_use]
pub fn read_rows(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

/// Whether `seat`'s ledger says it has been raised to acting: its last
/// transition a rise, decided on the words the seat asks now, read with only
/// the transition lines parsed ([`promote::standing_in`], t-6877). A ledger
/// that cannot be read never rose.
#[must_use]
pub fn raised_in(seat: &JevUse, ledger: &Path) -> bool {
    std::fs::read_to_string(ledger).is_ok_and(|text| promote::standing_in(seat, &text) == Stand::Applying)
}

/// Count every seat in the table, looking for each seat's ledger under the
/// given roots.
///
/// The roots are handed in and not read from the environment here: a counter
/// that asked the process where home was would read the person's own ledgers
/// from inside a test, and a test that passes because the machine it runs on
/// has rows is not a test.
#[must_use]
pub fn report(
    roots: &[PathBuf],
    sessions: Option<&Path>,
    settings: Option<&Value>,
    now_ms: i64,
    offset_s: i64,
) -> Vec<SeatReport> {
    report_with_recent(roots, sessions, settings, now_ms, offset_s, 0)
}

/// [`report`], with each seat's last `recent` requests read alongside its
/// numbers — the dashboard's ask, in the same pass over the same rows, so
/// the list under the table and the table above it are one reading of the
/// file.
#[must_use]
pub fn report_with_recent(
    roots: &[PathBuf],
    sessions: Option<&Path>,
    settings: Option<&Value>,
    now_ms: i64,
    offset_s: i64,
    recent: usize,
) -> Vec<SeatReport> {
    JEV_USES
        .iter()
        .map(|seat| one_with(seat, roots, sessions, settings, now_ms, offset_s, recent))
        .collect()
}

/// Every ledger file a seat has under a folder of Computer Use sessions —
/// `<sessions>/<session>/<seat.ledger>`, oldest session first.
///
/// The window's screen seats (browser, desktop, emulator) append beside each
/// walk's evidence rather than under one root, and a counter that looked
/// only under the roots read all three as "never asked" with 41 rows on this
/// machine (2026-09-20). Session folders are named by their start time, so
/// their name order is their time order.
#[must_use]
pub fn session_ledgers(sessions: &Path, ledger: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(sessions) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(ledger))
        .filter(|path| path.is_file())
        .collect();
    found.sort();
    found
}

/// A seat's ledger and its rows: the file the table names under the first
/// root that has it, else the copies beside each Computer Use session. The
/// one place the choice is made, so the numbers, the days and the recent
/// list are read from the same rows.
#[must_use]
pub fn rows_of(seat: &JevUse, roots: &[PathBuf], sessions: Option<&Path>) -> (Option<PathBuf>, Vec<Value>) {
    let under_roots = roots.iter().map(|root| root.join(seat.ledger)).find(|path| path.is_file());
    let per_session = sessions.map(|dir| session_ledgers(dir, seat.ledger)).unwrap_or_default();
    let found = under_roots.clone().or_else(|| per_session.first().cloned());
    // The root is the seat's ledger and the session folders hold a COPY of
    // the same rows: since 2026-09-21 a screen seat's walk writes both at
    // once (`computer_use::errand::write_rows`,
    // docs/design/jev-seats-accuracy-wave-20260921.md §4, decision 3), and
    // the window always hands this counter its sessions folder. Adding the
    // two would count every walk twice: a judgment window that fills at
    // twice the rate on the same evidence, and a week's bill read at double
    // what the door was actually asked. The root is read when it has the
    // file; the session copies are what a seat whose rows predate that
    // change still has, and they are read only then.
    let rows = match under_roots.as_deref() {
        Some(ledger) => read_rows(ledger),
        None => per_session.iter().flat_map(|ledger| read_rows(ledger)).collect(),
    };
    (found, rows)
}

/// Today and the days before it, oldest first, each counted by the week's
/// own counter over the rows whose `at` fell in it. Calendar days in the
/// person's zone, not seven rolling spans: a trend is read against the day
/// a person remembers doing something.
#[must_use]
pub fn days_of(rows: &[&Value], now_ms: i64, offset_s: i64) -> Vec<DayTally> {
    let today = start_of_day_ms(now_ms, offset_s);
    (0..WINDOW_DAYS)
        .rev()
        .map(|back| {
            let start_ms = today - back * MS_PER_DAY;
            let end_ms = start_ms + MS_PER_DAY;
            let held: Vec<&Value> = rows
                .iter()
                .copied()
                .filter(|row| {
                    let at = summary::AT.read(row).and_then(Value::as_i64).unwrap_or(0);
                    (start_ms..end_ms).contains(&at)
                })
                .collect();
            DayTally {
                start_ms,
                tally: summary::summarize_rows(held.iter().copied(), i64::MIN),
                agreement: summary::agreement_rows(held.iter().copied(), i64::MIN),
            }
        })
        .collect()
}

fn one_with(
    seat: &'static JevUse,
    roots: &[PathBuf],
    sessions: Option<&Path>,
    settings: Option<&Value>,
    now_ms: i64,
    offset_s: i64,
    recent: usize,
) -> SeatReport {
    let (found, rows) = rows_of(seat, roots, sessions);
    // The card counts the series the judge reads (t-6877): the rows asked
    // under the words the seat asks now and the marks that grade them, from
    // the one reader every number on the card and the judge share. The bill
    // alone is every row's: a request asked under older words still cost
    // what it cost.
    let version = promote::on_the_newest_version(seat, &rows);
    let today = summary::summarize_rows(version.rows.iter().copied(), start_of_day_ms(now_ms, offset_s));
    let week_since_ms = now_ms - WINDOW_DAYS * MS_PER_DAY;
    let week = summary::summarize_rows(version.rows.iter().copied(), week_since_ms);
    let agreement_week = summary::agreement_rows(version.marks.iter().copied(), week_since_ms);
    let asked_model = zerocode_core::jev::model_in(settings.unwrap_or(&Value::Null)).to_string();
    let cost_usd = cost_of(summary::summarize(&rows, week_since_ms).input_tokens, &asked_model);
    let asked_toward_judgment = version.asked();
    // The judge's own reading of the same rows, not a second Evidence built
    // here from the week: routing reads its agreement off the probe beside
    // the judgment, every other rising seat off the `agreed` marks its own
    // writer left (the window's orchestration seats).
    let judged = if seat.id == zerocode_core::jev::ROUTING.id {
        super::decision_shadow::judge_rows(&rows, settings)
    } else {
        promote::judge_seat(seat, &rows)
    };
    let clears_rise_floor = seat.answer_floor_permille.and_then(|floor| {
        judged.as_ref()?.window.answered_lower_bound().map(|bound| clears(bound, floor))
    });
    let stand = promote::standing(seat, &rows);
    SeatReport {
        id: seat.id,
        setting: seat.setting,
        mode: settings.map_or(JevMode::default(), |root| seat.mode_in(root)),
        ledger: seat.ledger,
        found,
        today,
        week,
        cost_usd,
        model: version.model.map(str::to_string),
        cut: version.cut.map(str::to_string),
        asked_model,
        rise_floor_permille: seat.answer_floor_permille,
        clears_rise_floor,
        judged,
        agreement_week,
        asked_toward_judgment,
        baseline: seat.baseline.kind(),
        negatives_wanted: seat.negatives_wanted,
        stand,
        applies: seat
            .mode_in(settings.unwrap_or(&Value::Null))
            .applies_with(stand == Stand::Applying),
        days: days_of(&version.rows, now_ms, offset_s),
        recent: recent::recent(&rows, recent),
    }
}

/// Whether a bound in `[0, 1]` reaches a line written per thousand. Compared
/// in the line's own units, so the answer does not turn on a float's last bit.
#[must_use]
pub fn clears(bound: f64, floor_permille: u16) -> bool {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bound_permille = (bound * 1000.0).floor() as u64;
    bound_permille >= u64::from(floor_permille)
}

/// Midnight of the local day `now_ms` falls in — "today" as the person
/// reading the screen means it, not as UTC means it.
///
/// The offset is handed in rather than read here: this workspace has no
/// calendar crate and forbids `unsafe`, so the one place that asks the OS
/// asks it once per process, and a pure function is what a test can pin at
/// a boundary (a row at 23:59:59.999 and the one a millisecond later).
#[must_use]
pub fn start_of_day_ms(now_ms: i64, offset_s: i64) -> i64 {
    let local = now_ms + offset_s * 1000;
    local - local.rem_euclid(MS_PER_DAY) - offset_s * 1000
}

/// What a window's billed input cost, at the rate of the model the seat asks
/// for.
///
/// The System One rate table is read and not the chat one: a judgment call
/// bills input only and is never a candidate the plan scorer could rank, and
/// the two are kept apart on purpose (`api::systemone_rate`). Asked of the
/// chat table this answered `None` for every seat and the card drew no cost at
/// all, against a price that has been written down since the launch post.
///
/// Priced by the id the seat ASKS with — the person's pin, or the alias — not
/// the one the wire answers with: the bill is for the request. A pinned
/// version bills at its family's row, and a dated id no row names is
/// unpriced rather than billed at a neighbour's rate (`api::systemone_rate`).
pub(super) fn cost_of(input_tokens: u64, asked: &str) -> Option<f64> {
    Some(api::systemone_rate(asked)?.input_cost_usd(input_tokens))
}

#[cfg(test)]
mod tests;

/// Every seat's ledger on this machine graded again by t-6342's rules — the
/// label audit's measurement (`tools/label-audit`).
#[cfg(test)]
mod label_audit_tests;

/// A line's own word, as a function a caller can hand to `map` — the word
/// itself is the line's to say, and this only saves a reader from naming the
/// core's path to reach it.
#[must_use]
pub fn line_token(line: promote::Line) -> &'static str {
    line.token()
}
