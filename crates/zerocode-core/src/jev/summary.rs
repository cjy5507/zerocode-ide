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

/// The z for the 95% one-sided Wilson bound the rise line is read against
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
];

/// The word a row carries when its judgment answered and passed its checks.
pub const ANSWERED: &str = "answered";

/// One use's ledger over one window, counted.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tally {
    /// Rows in the window.
    pub rows: usize,
    /// Rows whose judgment answered and passed its checks.
    pub answered: usize,
    /// Rows whose judgment went over the wire — the latency population. A
    /// memo hit answered without asking, so it is counted as an answer and
    /// not as a call.
    pub called: usize,
    /// Rows that asked and did not answer, by outcome token, most frequent
    /// first, then by token so the order is the same twice.
    pub failures: Vec<(String, usize)>,
    /// Requests spent, retries included.
    pub requests: u64,
    /// Lines the door withheld across the window.
    pub redacted_lines: u64,
    /// Input tokens the window's calls billed.
    pub input_tokens: u64,
    /// Nearest-rank percentiles of the calls' elapsed milliseconds.
    pub p50_ms: Option<u64>,
    pub p95_ms: Option<u64>,
}

impl Tally {
    /// The share of rows that answered, or `None` when the window is empty —
    /// which is not zero, and a screen that drew it as zero would be lying
    /// about a seat nobody has used yet.
    #[must_use]
    pub fn answered_share(&self) -> Option<f64> {
        (self.rows > 0).then(|| {
            #[allow(clippy::cast_precision_loss)]
            {
                self.answered as f64 / self.rows as f64
            }
        })
    }

    /// The 95% Wilson lower bound on that share — what the rise line is read
    /// against (§4), and what a thin window is honest about: twenty answers
    /// out of twenty bound at 0.839, not at 1.0.
    #[must_use]
    pub fn answered_lower_bound(&self) -> Option<f64> {
        (self.rows > 0).then(|| wilson_lower(self.answered, self.rows, WILSON_Z_95))
    }

    /// How many more rows this use owes before its next auto judgment (§4).
    #[must_use]
    pub const fn rows_to_next_judgment(&self) -> usize {
        JUDGED_EVERY_ROWS - self.rows % JUDGED_EVERY_ROWS
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
/// A ledger holds two kinds of line: the rows a use wrote when it asked
/// something, and the notes the judge wrote about them. Counting the second
/// kind as the first would lower every seat's answered share by the very act
/// of judging it.
#[must_use]
pub fn asked_something(row: &Value) -> Option<&str> {
    if TRANSITION.read(row).is_some() {
        return None;
    }
    OUTCOME.read(row).and_then(Value::as_str)
}

/// The last `n` requests, counted — the window §4 judges on.
///
/// A count and not a clock: a seat asked twice a day and one asked twice a
/// minute earn their promotion on the same amount of evidence, and a window
/// measured in hours would hand the busy one a verdict off a hundred rows
/// while the quiet one never filled its own.
#[must_use]
pub fn summarize_last(rows: &[Value], n: usize) -> Tally {
    let asked: Vec<&Value> = rows
        .iter()
        .filter(|row| asked_something(row).is_some())
        .collect();
    let from = asked.len().saturating_sub(n);
    let held: Vec<Value> = asked[from..].iter().map(|row| (*row).clone()).collect();
    summarize(&held, i64::MIN)
}

/// How many requests at the end of the ledger did not answer, stopping at the
/// first that did — §4's "in a row", read from the rows themselves rather
/// than from a counter some other process would have to keep.
#[must_use]
pub fn failures_in_a_row(rows: &[Value]) -> u32 {
    let mut held = 0;
    for row in rows.iter().rev() {
        match asked_something(row) {
            None => continue,
            Some(ANSWERED) => break,
            Some(_) => held += 1,
        }
    }
    held
}

/// Count the rows of one ledger whose `at` is at or after `since_ms`.
///
/// Rows the door refused never reached the wire, so they are neither answers
/// nor failures of the judgment — they are counted in `rows` and named in
/// `failures` by their own token, because a seat that sends nothing because
/// a workspace is not consented must not read as a seat that is working.
#[must_use]
pub fn summarize(rows: &[Value], since_ms: i64) -> Tally {
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
        tally.requests += REQUESTS.read(row).and_then(Value::as_u64).unwrap_or(0);
        tally.redacted_lines += REDACTED_LINES
            .read(row)
            .and_then(Value::as_u64)
            .unwrap_or(0);
        tally.input_tokens += INPUT_TOKENS.read(row).and_then(Value::as_u64).unwrap_or(0);
        let outcome = OUTCOME
            .read(row)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if outcome == ANSWERED {
            tally.answered += 1;
        } else if let Some(token) = OUTCOME.read(row).and_then(Value::as_str) {
            *failures.entry(token).or_default() += 1;
        }
        let cached = CACHED.read(row).and_then(Value::as_bool).unwrap_or(false);
        if let Some(ms) = ELAPSED_MS
            .read(row)
            .and_then(Value::as_u64)
            .filter(|_| !cached)
        {
            tally.called += 1;
            elapsed.push(ms);
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
