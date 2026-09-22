//! The memo in front of the wire: an answer the door already let a question
//! out for, kept under the exact bytes that went, so the same bytes need not
//! go again (t-6132, [`super::JUDGMENT_CACHE`]).
//!
//! It is one file beside the seats' ledgers and the day's count
//! ([`super::count::REQUESTS_DIR`]), so a run of the same Flow tomorrow finds
//! what a run today learned — a memo that lived in the process would answer
//! nothing after a restart, which is exactly when a QA harness runs its
//! rounds again. Rows are appended, newest last, and the newest row for a key
//! is the one that answers ([`recall`]): a memo remembers the most recent
//! judgment, never the first.
//!
//! What a row holds is the wire's own answer body and a key — never the
//! request. The key is a fingerprint ([`key_of`]): the seat's id and the
//! bytes the door cleared, after every withheld line and every cap, run
//! through [`super::rubric_fingerprint`]. Two questions share an answer when
//! and only when the wire would have seen the same request, and nobody can
//! read a screen's words back out of the file.
//!
//! The file is bounded ([`MEMO_ROWS_CAP`]): past the cap the oldest half is
//! dropped on the next write, the way zo's probe memo starts over rather than
//! grow. Reading it whole on every lookup is the point — a lookup must stay
//! cheaper than the question it answers for ([`super::JUDGMENT_MEMO_DEADLINE_MS`]).
//!
//! Nothing here asks the door. The door asks the memo
//! ([`super::door::pass_remembering`]), after its own four questions and
//! before it counts: a hit is a request that passed and did not leave.

use std::path::Path;

use serde_json::{Value, json};

use super::{JevUse, rubric_fingerprint};

/// The memo's file name, under the Jev folder of a config home.
pub const MEMO_FILE: &str = "judgment-memo.jsonl";

/// The most rows the memo keeps. A row is one answer body (~400 bytes) and a
/// key; 2,000 of them are ~800 KB, read and scanned whole in a few
/// milliseconds, and hold every judgment of a few hundred Flow rounds.
pub const MEMO_ROWS_CAP: usize = 2_000;

/// The row's keys — spelled once, read by the reader and the writer alike.
pub const KEY: &str = "key";
pub const SEAT: &str = "seat";
pub const ANSWER: &str = "answer";
pub const AT: &str = "at";

/// The key a question is remembered under: the seat and the bytes the door
/// let through, fingerprinted by the one function every rubric version is
/// pinned with.
#[must_use]
pub fn key_of(seat: &JevUse, cleared: &[u8]) -> String {
    rubric_fingerprint(|| {
        let mut words = String::with_capacity(seat.id.len() + 1 + cleared.len());
        words.push_str(seat.id);
        words.push('\n');
        words.push_str(&String::from_utf8_lossy(cleared));
        words
    })
}

/// An answer the memo held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recalled {
    /// The wire's answer body, exactly as it arrived.
    pub answer: String,
    /// When it was remembered, in milliseconds since the epoch.
    pub at: i64,
}

/// The newest answer remembered under `key`, or `None` — for a key nobody
/// remembered, a file that does not exist, or a file that will not read.
#[must_use]
pub fn recall(memo: &Path, key: &str) -> Option<Recalled> {
    let text = std::fs::read_to_string(memo).ok()?;
    // Newest last: the last row that names the key is the answer. The
    // substring check keeps JSON parsing to the rows that can match.
    let named = format!("\"{KEY}\":\"{key}\"");
    text.lines()
        .rev()
        .filter(|line| line.contains(&named))
        .find_map(|line| {
            let row: Value = serde_json::from_str(line).ok()?;
            if row.get(KEY)?.as_str()? != key {
                return None;
            }
            Some(Recalled {
                answer: row.get(ANSWER)?.as_str()?.to_string(),
                at: row.get(AT).and_then(Value::as_i64).unwrap_or_default(),
            })
        })
}

/// Remember `answer` under `key`, newest last. Past [`MEMO_ROWS_CAP`] the
/// oldest half is dropped first, so the file never outgrows a lookup's wall.
///
/// # Errors
/// The file could not be written.
pub fn remember(
    memo: &Path,
    seat: &JevUse,
    key: &str,
    answer: &str,
    now_ms: i64,
) -> std::io::Result<()> {
    let row = json!({ KEY: key, SEAT: seat.id, ANSWER: answer, AT: now_ms }).to_string();
    if let Some(folder) = memo.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let held = std::fs::read_to_string(memo).unwrap_or_default();
    let rows = held.lines().filter(|line| !line.is_empty()).count();
    if rows >= MEMO_ROWS_CAP {
        let kept: Vec<&str> = held
            .lines()
            .filter(|line| !line.is_empty())
            .skip(rows - MEMO_ROWS_CAP / 2)
            .collect();
        let mut text = kept.join("\n");
        text.push('\n');
        text.push_str(&row);
        text.push('\n');
        return std::fs::write(memo, text);
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(memo)?;
    std::io::Write::write_all(&mut file, format!("{row}\n").as_bytes())
}

/// How many rows the memo holds — for a reader counting what it remembers.
#[must_use]
pub fn rows(memo: &Path) -> usize {
    std::fs::read_to_string(memo)
        .map(|text| text.lines().filter(|line| !line.is_empty()).count())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
