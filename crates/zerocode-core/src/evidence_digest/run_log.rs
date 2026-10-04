//! What a test run's log says (t-36910): the counts its runners printed, the
//! names of the tests that failed, and — beside the log — how the run ended.
//!
//! Two runners write the lines read here, and one log may hold both:
//!
//! - cargo's test binaries: one `test result:` line each (`12 passed; 1
//!   failed; 2 ignored; 0 measured; 3 filtered out`), a `test <name> ...
//!   FAILED` line for each failed test, and the list of their names under
//!   `failures:`;
//! - the window's suites: a `FAIL  <name>` line for each failed check and one
//!   `N/M passed` line.
//!
//! Whether a failure was *meant* is never read off a file's name. It is known
//! from outside the log ([`Told`]): the exit code of the run that wrote it — a
//! script that ended with 0 over failed tests ended as it expected to — or the
//! hand-in saying the run was meant to fail.
//!
//! A log is read one line at a time; what is held is the counts, the names the
//! table lets it list and, for the suite being read, the names it has already
//! met.

use std::collections::BTreeSet;
use std::io::{BufRead, Read as _};

use serde::Serialize;

use super::bounded_line;
use crate::artifact::Limits;

/// A test run, counted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Tests {
    pub passed: usize,
    /// Tests that failed: always the named ones and `more`.
    pub failed: usize,
    /// Tests marked `#[ignore]`, which the run left out.
    pub ignored: usize,
    /// Tests a name filter left out.
    pub filtered: usize,
    /// Summary lines read: one for each test binary and each window suite run.
    pub suites: usize,
    /// The failed tests the log names, in its order, as many as the table lists.
    pub failed_names: Vec<String>,
    /// Failed tests beyond the named ones.
    pub more: usize,
    /// The log is longer than the table reads; every count is of what was read.
    pub cut: bool,
    /// The exit code of the run that wrote the log, when something said it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rc: Option<i32>,
    pub verdict: Verdict,
}

/// How a run ended, in the four words the window has for it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    /// It failed, and that is what the run was for: a test written before its
    /// fix.
    Intended,
    /// Nothing read says either way.
    #[default]
    Unknown,
}

/// What is known of a run from outside its log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Told {
    /// The exit code of the script that ran it: what its `.rc` holds.
    pub rc: Option<i32>,
    /// The hand-in said the run was meant to fail.
    pub meant_to_fail: bool,
}

impl Tests {
    /// The same counts, judged again with what is known from outside the log.
    /// An exit code the counts already carry stays unless a new one is told.
    #[must_use]
    pub fn judged(mut self, told: Told) -> Self {
        self.rc = told.rc.or(self.rc);
        self.verdict = Verdict::of(&self, told.meant_to_fail);
        self
    }
}

impl Verdict {
    /// The word for a run, from what was counted and what was told.
    ///
    /// A failure is a failed test or an exit code other than 0. It was meant
    /// when the hand-in said so, or when tests failed and the script still
    /// ended with 0. With no failure, an exit code of 0 is a pass; without an
    /// exit code a pass needs a test that passed and a log read to its end —
    /// what lies past a cut may have failed.
    fn of(tests: &Tests, meant_to_fail: bool) -> Self {
        let tests_failed = tests.failed > 0;
        let run_failed = tests.rc.is_some_and(|rc| rc != 0);
        if tests_failed || run_failed {
            let ended_as_expected = tests_failed && tests.rc == Some(0);
            return if meant_to_fail || ended_as_expected {
                Self::Intended
            } else {
                Self::Fail
            };
        }
        match tests.rc {
            Some(_) => Self::Pass,
            None if tests.passed > 0 && !tests.cut => Self::Pass,
            None => Self::Unknown,
        }
    }
}

/// Where a cargo test binary's summary starts. Its counts follow, divided by
/// semicolons, each a number and the word for it. It is looked for anywhere in
/// a line: a test that prints without a newline leaves its text in front.
const CARGO_SUMMARY: &str = "test result:";

/// What divides the counts of a cargo summary.
const CARGO_COUNT_DIVIDER: char = ';';

/// One count of a suite's summary.
#[derive(Clone, Copy)]
enum Count {
    Passed,
    Failed,
    Ignored,
    Filtered,
}

/// The words of a cargo summary this digest counts. `measured` is a bench, not
/// a test that passed or failed, and has no row.
const CARGO_COUNTS: [(&str, Count); 4] = [
    ("passed", Count::Passed),
    ("failed", Count::Failed),
    ("ignored", Count::Ignored),
    ("filtered out", Count::Filtered),
];

/// How cargo writes one failed test: `test <name> ... FAILED`.
const CARGO_TEST_OPENS: &str = "test ";
const CARGO_TEST_FAILED: &str = " ... FAILED";

/// The line that heads a cargo binary's list of failed names, and how each
/// name under it is indented. The same word heads the failed tests' output a
/// few lines earlier; a blank line follows it there, so nothing is read as a
/// name.
const CARGO_FAILURES_HEAD: &str = "failures:";
const CARGO_FAILURES_INDENT: &str = "    ";

/// How a window suite writes a failed check (`FAIL  <name>  — <detail>`) and
/// its summary (`N/M passed`): the words of `ui/tests/window-runner.mjs`.
const WINDOW_FAIL: &str = "FAIL ";
const WINDOW_DETAIL: &str = "  — ";
const WINDOW_SUMMARY_ENDS: &str = " passed";
const WINDOW_SUMMARY_DIVIDER: char = '/';

/// How many failed names one suite is watched for, so a name its list repeats
/// is counted once. Past it the suite's summary line still counts every
/// failure; only the telling apart stops.
const SUITE_NAMES_TRACKED_MAX: usize = 512;

/// The most a `.rc` file may hold and still be an exit code: a signed number
/// and the newline a shell's `echo` ends it with.
const EXIT_CODE_BYTES_MAX: u64 = 16;

/// What one summary line counted.
#[derive(Default)]
struct SuiteCounts {
    passed: usize,
    failed: usize,
    ignored: usize,
    filtered: usize,
}

impl SuiteCounts {
    fn set(&mut self, count: Count, tests: usize) {
        match count {
            Count::Passed => self.passed = tests,
            Count::Failed => self.failed = tests,
            Count::Ignored => self.ignored = tests,
            Count::Filtered => self.filtered = tests,
        }
    }
}

/// The counts of a cargo summary, from the text after its opening words.
fn cargo_counts(said: &str) -> SuiteCounts {
    let mut counts = SuiteCounts::default();
    for part in said.split(CARGO_COUNT_DIVIDER) {
        let part = part.trim();
        let counted = CARGO_COUNTS.iter().find_map(|(word, count)| {
            let number = part.strip_suffix(word)?.split_whitespace().next_back()?;
            Some((*count, number.parse::<usize>().ok()?))
        });
        if let Some((count, tests)) = counted {
            counts.set(count, tests);
        }
    }
    counts
}

/// The name on a line of a cargo binary's list of failed tests.
fn listed_name(line: &str) -> Option<&str> {
    line.strip_prefix(CARGO_FAILURES_INDENT)
        .filter(|name| !name.is_empty() && !name.starts_with(char::is_whitespace))
}

/// The name on a cargo line that says one test failed.
fn cargo_failed(said: &str) -> Option<&str> {
    said.strip_prefix(CARGO_TEST_OPENS)?
        .strip_suffix(CARGO_TEST_FAILED)
}

/// The name on a window suite's line that says one check failed.
fn window_failed(said: &str) -> Option<&str> {
    let rest = said.strip_prefix(WINDOW_FAIL)?.trim_start();
    let name = rest
        .split_once(WINDOW_DETAIL)
        .map_or(rest, |(name, _detail)| name)
        .trim_end();
    (!name.is_empty()).then_some(name)
}

/// The counts of a window suite's summary line, `N/M passed`.
fn window_counts(said: &str) -> Option<SuiteCounts> {
    let (passed, all) = said
        .strip_suffix(WINDOW_SUMMARY_ENDS)?
        .split_once(WINDOW_SUMMARY_DIVIDER)?;
    let passed: usize = passed.parse().ok()?;
    let all: usize = all.parse().ok()?;
    Some(SuiteCounts {
        passed,
        failed: all.checked_sub(passed)?,
        ..SuiteCounts::default()
    })
}

/// The single pass over a log's lines.
pub(super) struct TestsFold<'a> {
    limits: &'a Limits,
    out: Tests,
    /// The failed names of the suite being read; its summary line empties it.
    suite: BTreeSet<String>,
    /// The line before headed a list of failed names, or was a name in it.
    listing: bool,
}

impl<'a> TestsFold<'a> {
    pub(super) fn new(limits: &'a Limits) -> Self {
        Self {
            limits,
            out: Tests::default(),
            suite: BTreeSet::new(),
            listing: false,
        }
    }

    pub(super) fn see(&mut self, line: &[u8]) {
        let text = String::from_utf8_lossy(line);
        let line = text.trim_end();
        if std::mem::take(&mut self.listing)
            && let Some(name) = listed_name(line)
        {
            self.listing = true;
            self.name_failed(name);
            return;
        }
        let said = line.trim_start();
        if said == CARGO_FAILURES_HEAD {
            self.listing = true;
        } else if let Some(at) = said.find(CARGO_SUMMARY) {
            self.close_suite(&cargo_counts(&said[at + CARGO_SUMMARY.len()..]));
        } else if let Some(name) = cargo_failed(said).or_else(|| window_failed(said)) {
            self.name_failed(name);
        } else if let Some(counts) = window_counts(said) {
            self.close_suite(&counts);
        }
    }

    /// One failed test of the suite being read, named once however often the
    /// log names it.
    fn name_failed(&mut self, name: &str) {
        if self.suite.len() >= SUITE_NAMES_TRACKED_MAX {
            return;
        }
        let name = bounded_line(std::slice::from_ref(&name.to_string()), self.limits);
        if !self.suite.insert(name.clone()) {
            return;
        }
        if self.out.failed_names.len() < self.limits.digest_failures_max {
            self.out.failed_names.push(name);
        }
    }

    /// Add a suite's summary to the run's. A suite failed at least what it
    /// named: a summary may count fewer when a line of it was torn.
    fn close_suite(&mut self, counts: &SuiteCounts) {
        self.out.suites += 1;
        self.out.passed += counts.passed;
        self.out.ignored += counts.ignored;
        self.out.filtered += counts.filtered;
        self.out.failed += counts.failed.max(self.suite.len());
        self.suite.clear();
    }

    /// The run, or `None` for a log with no line this reader counts.
    pub(super) fn finish(mut self, cut: bool) -> Option<Tests> {
        // A suite that ended without its summary still failed what it named.
        self.out.failed += self.suite.len();
        if self.out.suites == 0 && self.out.failed == 0 {
            return None;
        }
        self.out.more = self.out.failed - self.out.failed_names.len();
        self.out.cut = cut;
        Some(self.out.judged(Told::default()))
    }
}

/// The exit code a `.rc` file holds: one whole number and nothing else.
#[must_use]
pub fn exit_code(reader: impl BufRead) -> Option<i32> {
    let mut held = String::new();
    reader
        .take(EXIT_CODE_BYTES_MAX + 1)
        .read_to_string(&mut held)
        .ok()?;
    if held.len() as u64 > EXIT_CODE_BYTES_MAX {
        return None;
    }
    held.trim().parse().ok()
}

/// The run a `.rc` file tells of.
pub(super) fn read_exit_code(reader: impl BufRead) -> Option<Tests> {
    let rc = exit_code(reader)?;
    Some(Tests::default().judged(Told {
        rc: Some(rc),
        ..Told::default()
    }))
}

#[cfg(test)]
mod tests;
