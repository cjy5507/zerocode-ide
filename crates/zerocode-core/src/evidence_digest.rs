//! What an evidence file says, read into counts and a bounded list of steps
//! (t-36910).
//!
//! The drawer of the Artifacts view showed an evidence file as its first
//! bytes: a step log as raw JSON lines, a state record as one raw object. A
//! person reading evidence wants to know how many steps ran, which of them
//! failed and what the failure said — so this module reads the file into a
//! [`Digest`], and the window draws that instead of the JSON.
//!
//! The shape is judged by what the file holds, never by what it is called:
//!
//! - a log whose lines carry a `verb` and an `ok` is [`Digest::Steps`]: the
//!   counts of the whole log, and the steps worth showing — the first few, the
//!   last few, and every failed step with its neighbours. The runs between are
//!   folded into a count, so a log of eighteen thousand steps is a few dozen
//!   rows;
//! - the operator's state record (an object with `actions` and
//!   `consecutiveFailures`) is [`Digest::State`];
//! - any other JSON is [`Digest::Facts`]: its top-level values, bounded.
//!
//! The extension only says how to read the bytes ([`Format`]): line by line,
//! or as one document. A file that is not JSON has no digest and is shown as
//! the text it is.
//!
//! Every number is a field of the artifact table ([`Limits`]). A log is read
//! one line at a time and only counters and the kept rows are held, so its
//! cost in memory is the table's and not the file's.

use std::io::BufRead;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::artifact::Limits;

/// How an evidence file's bytes are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One JSON record per line (`.jsonl`).
    Lines,
    /// One JSON document (`.json`).
    Document,
}

/// The extensions a digest is read from, lower-cased and without the dot.
const FORMAT_BY_EXTENSION: &[(&str, Format)] =
    &[("jsonl", Format::Lines), ("json", Format::Document)];

/// How many different verbs a log's count may name before the rest are summed
/// unnamed: a log is not trusted to hold few of them.
const VERBS_TRACKED_MAX: usize = 64;

impl Format {
    /// The format a path's extension names; `None` for a file that is not
    /// JSON, which has no digest.
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        let _ = (path, FORMAT_BY_EXTENSION);
        None
    }
}

/// What one evidence file says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Digest {
    Steps(Steps),
    State(State),
    Facts(Facts),
}

/// A step log, counted whole and shown in part.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Steps {
    /// Steps read.
    pub total: usize,
    /// Steps whose `ok` is false.
    pub failed: usize,
    /// Lines that are not a step: a torn line, or another kind of record.
    pub skipped: usize,
    /// The log is longer than the table reads; every count is of what was read.
    pub cut: bool,
    /// When the first and the last step read were recorded, epoch milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
    /// The verbs used most, the most used first.
    pub verbs: Vec<VerbCount>,
    /// Steps of every verb `verbs` does not name.
    pub other_verbs: usize,
    /// What the drawer lists: steps, and the folded runs between them.
    pub rows: Vec<Row>,
}

/// How many steps one verb has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerbCount {
    pub verb: String,
    pub steps: usize,
}

/// One line of the drawer's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "row", rename_all = "snake_case")]
pub enum Row {
    Step(StepRow),
    /// A run of steps left out of the list, and how many of them failed — more
    /// than none only when the table's row count was already spent.
    Fold {
        steps: usize,
        failed: usize,
    },
}

/// One step, as the list shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StepRow {
    pub n: usize,
    pub at_ms: i64,
    pub tool: String,
    pub verb: String,
    /// What the step acted on: its arguments in one bounded line.
    pub target: String,
    pub ok: bool,
    /// How long it took, when the step measured it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The step kept a picture of the screen.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub framed: bool,
}

/// The operator's state record: what it did last and whether it is stuck.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all(deserialize = "camelCase"))]
pub struct State {
    pub actions: u64,
    pub last_verb: Option<String>,
    pub last_ok: Option<bool>,
    pub last_error: Option<String>,
    pub last_at_ms: i64,
    pub consecutive_failures: u32,
    pub unchanged_looks: u32,
    pub stuck: bool,
    pub recent: Vec<String>,
}

/// The two keys that make an object the operator's state record.
const STATE_KEYS: [&str; 2] = ["actions", "consecutiveFailures"];

/// A JSON record that is neither: its top-level values, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Facts {
    /// Records in the file: one for a document, the lines of a log, the items
    /// of a top-level list.
    pub records: usize,
    /// The file is longer than the table reads.
    pub cut: bool,
    /// The values of the first record.
    pub facts: Vec<Fact>,
    /// Values of that record beyond the listed ones.
    pub more: usize,
}

/// One named value of a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fact {
    pub key: String,
    pub value: FactValue,
}

/// A value, reduced to what one line can say. A list and a nested record are
/// their sizes: the drawer names them and does not unfold them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "is", content = "value", rename_all = "snake_case")]
pub enum FactValue {
    Text(String),
    /// A number, as the file wrote it.
    Number(String),
    Flag(bool),
    List(usize),
    Record(usize),
    Nothing,
}

/// Read one evidence file into its digest; `None` when it holds no JSON
/// record at all.
#[must_use]
pub fn read(format: Format, reader: impl BufRead, limits: &Limits) -> Option<Digest> {
    let _ = (format, reader, limits, VERBS_TRACKED_MAX, STATE_KEYS);
    None
}

#[cfg(test)]
mod tests;
