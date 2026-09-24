//! What the challenger arm has spent and reserved in a local day — the one
//! durable source the day's share is read from and charged against.
//!
//! One file per day under zo's config home, beside the door's own day count
//! ([`crate::jev::count`]), one JSON line per event, appended with `O_APPEND`
//! so two programs writing at once cannot interleave a line. Three events:
//! an attempt is **reserved** what its challenger design is expected to cost
//! when it is drawn, **settled** for what the design actually cost once its
//! row is written, or **released** when nothing was sent after all. The share
//! is read by folding the day's lines ([`fold`]): what is still reserved and
//! what was spent, one word per attempt.
//!
//! The fold is pure and the writer is not here: the program that owns the
//! clock and the lock appends, and this module fixes only the line and the
//! reading of it, so the counter in zo, the window's dashboard and a replay
//! read the same day the same way. It is not the seat's ledger — the ledger
//! row carries the settled cost too ([`super::COST_MICROS`]), for the
//! comparison it belongs to; this file is what the budget reads, machine-wide,
//! because a day's spend is the machine's and the ledgers are one per project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::DaySpend;
use crate::jev::count::REQUESTS_DIR;

/// What a day file's name starts and ends with, around the local date.
pub const SPEND_FILE_PREFIX: &str = "challenger-spend-";
pub const SPEND_FILE_SUFFIX: &str = ".jsonl";

/// The key an event names its attempt under.
pub const ATTEMPT_KEY: &str = "attempt";
/// The key an event names what it is under ([`Op::token`]).
pub const OP_KEY: &str = "op";
/// The key a reservation or a settlement carries its amount under.
pub const MICROS_KEY: &str = "micros";

/// The day file for `day` (`YYYY-MM-DD`, local) under `config_home` — in the
/// door's own folder, so one directory holds everything a day's budget reads.
#[must_use]
pub fn spend_path(config_home: &Path, day: &str) -> PathBuf {
    config_home
        .join(REQUESTS_DIR)
        .join(format!("{SPEND_FILE_PREFIX}{day}{SPEND_FILE_SUFFIX}"))
}

/// One event in a day file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Drawn: the expected cost is charged against the share until a
    /// settlement or a release takes it back.
    Reserve,
    /// The design came back (or failed after leaving): this is what it cost,
    /// and the reservation is over.
    Settle,
    /// Nothing left the machine: the reservation is over and nothing was
    /// spent.
    Release,
}

impl Op {
    /// The word a line writes.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Reserve => "reserve",
            Self::Settle => "settle",
            Self::Release => "release",
        }
    }

    fn from_token(word: &str) -> Option<Self> {
        [Self::Reserve, Self::Settle, Self::Release]
            .into_iter()
            .find(|op| op.token() == word)
    }
}

/// One event as its line is written, newline included: the attempt, the
/// word, and the amount where the word carries one.
#[must_use]
pub fn line(attempt: &str, op: Op, micros: u64) -> String {
    let mut event = json!({ ATTEMPT_KEY: attempt, OP_KEY: op.token() });
    if op != Op::Release {
        event[MICROS_KEY] = Value::from(micros);
    }
    let mut text = event.to_string();
    text.push('\n');
    text
}

/// Whether `text` holds any event for `attempt` — the question a writer
/// asks before reserving, so one attempt is never drawn into a day twice
/// (a second opening of the same attempt is a retry, whatever caused it).
#[must_use]
pub fn names(text: &str, attempt: &str) -> bool {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|event| event.get(ATTEMPT_KEY).and_then(Value::as_str) == Some(attempt))
}

/// Forget the books of days before `today`'s that nothing will settle into
/// again — what the first reservation of a day does, under the door's own
/// rule for what a past day leaves behind
/// ([`crate::jev::count::forget_earlier_days`]: never today's book, never a
/// later day's).
///
/// Yesterday's book is kept while it holds a reservation nothing has settled
/// or released: a draw charged before midnight whose design is still out
/// settles into its own day, and its book is where that settlement belongs.
/// A book that cannot be read is kept too — what this cannot read, it
/// cannot call settled. Two days back, a reservation still live is a draw
/// that died before it settled: it held its day's share until that day
/// ended, which is the arm's whole rule for a crash, and the day is over.
pub fn forget_settled_days(today: &Path) {
    let yesterday = crate::jev::count::day_named(today, SPEND_FILE_PREFIX, SPEND_FILE_SUFFIX)
        .and_then(|day| crate::jev::count::day_before(&day));
    crate::jev::count::forget_earlier_days(
        today,
        SPEND_FILE_PREFIX,
        SPEND_FILE_SUFFIX,
        |day, book| {
            yesterday.as_deref() == Some(day)
                && !std::fs::read_to_string(book).is_ok_and(|text| fold(&text).reserved == 0)
        },
    );
}

/// A day's book, folded from its lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Book {
    /// Reserved by attempts that have neither settled nor released.
    pub reserved_micros: u64,
    /// Settled by attempts whose design came back or failed after leaving.
    pub spent_micros: u64,
    /// Attempts with a live reservation.
    pub reserved: usize,
    /// Attempts that settled.
    pub settled: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    Reserved(u64),
    Settled(u64),
    Released,
}

/// Fold a day's lines into what is reserved and what was spent.
///
/// One word per attempt, the first of each kind binding: a second
/// reservation of the same attempt is the same draw seen twice (a retry of
/// the writer, or two readers racing) and not a second charge; a settlement
/// ends the reservation and counts once, however many times it is written;
/// a release ends it and counts nothing. A settlement with no reservation
/// before it still counts — the design left and cost what it cost, whether
/// or not the reservation line survived. A line that does not parse is
/// skipped: a torn write at the tail is a crash's leftover, not a reason to
/// forget the day.
#[must_use]
pub fn fold(text: &str) -> Book {
    let mut standing: BTreeMap<String, Standing> = BTreeMap::new();
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(attempt) = event.get(ATTEMPT_KEY).and_then(Value::as_str) else {
            continue;
        };
        let Some(op) = event
            .get(OP_KEY)
            .and_then(Value::as_str)
            .and_then(Op::from_token)
        else {
            continue;
        };
        let micros = event.get(MICROS_KEY).and_then(Value::as_u64).unwrap_or(0);
        let entry = standing.entry(attempt.to_string());
        match op {
            Op::Reserve => {
                entry.or_insert(Standing::Reserved(micros));
            }
            Op::Settle => {
                let slot = entry.or_insert(Standing::Settled(micros));
                if let Standing::Reserved(_) | Standing::Released = slot {
                    *slot = Standing::Settled(micros);
                }
            }
            Op::Release => {
                let slot = entry.or_insert(Standing::Released);
                if let Standing::Reserved(_) = slot {
                    *slot = Standing::Released;
                }
            }
        }
    }
    let mut book = Book::default();
    for state in standing.values() {
        match state {
            Standing::Reserved(micros) => {
                book.reserved_micros = book.reserved_micros.saturating_add(*micros);
                book.reserved += 1;
            }
            Standing::Settled(micros) => {
                book.spent_micros = book.spent_micros.saturating_add(*micros);
                book.settled += 1;
            }
            Standing::Released => {}
        }
    }
    book
}

/// The day's spend as the share is read from it: the arm's own book, and
/// the day's other model spend as the caller summed it from its own ledgers.
///
/// The arm's spend enters the day's whole exactly once, here — the caller's
/// sum is everything ELSE the day bought, and a caller that had already
/// added the arm's rows to it would be charging the share against itself
/// twice. With the arm inside the denominator the ceiling works out to a
/// ninth of the rest (one tenth of the whole), which is the user's decision
/// read as written: a tenth of what the day spends, the arm included.
#[must_use]
pub fn day_spend(book: &Book, other_micros: u64) -> DaySpend {
    DaySpend {
        challenger_micros: book.spent_micros,
        reserved_micros: book.reserved_micros,
        day_micros: other_micros.saturating_add(book.spent_micros),
    }
}
