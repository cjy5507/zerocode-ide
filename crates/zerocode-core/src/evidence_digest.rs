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

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, Read as _};
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
        let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
        FORMAT_BY_EXTENSION
            .iter()
            .find(|(name, _)| *name == extension)
            .map(|(_, format)| *format)
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

/// What one listed row weighs beside its texts — its numbers, its flags and the
/// allocations' own headers — when the preview cache counts a digest against
/// its byte cap. An estimate: the cap bounds memory, it does not meter it.
const ROW_WEIGHT: u64 = 96;

impl Digest {
    /// About how many bytes the digest holds: every text it keeps, and
    /// a fixed weight (`ROW_WEIGHT`) for each row, verb, fact and for itself.
    #[must_use]
    pub fn weight(&self) -> u64 {
        let text = |held: &str| held.len() as u64;
        let optional = |held: &Option<String>| held.as_deref().map_or(0, text);
        ROW_WEIGHT
            + match self {
                Self::Steps(steps) => {
                    let rows: u64 = steps
                        .rows
                        .iter()
                        .map(|row| match row {
                            Row::Step(step) => {
                                ROW_WEIGHT
                                    + text(&step.tool)
                                    + text(&step.verb)
                                    + text(&step.target)
                                    + optional(&step.error)
                                    + optional(&step.code)
                            }
                            Row::Fold { .. } => ROW_WEIGHT,
                        })
                        .sum();
                    let verbs: u64 = steps
                        .verbs
                        .iter()
                        .map(|held| ROW_WEIGHT + text(&held.verb))
                        .sum();
                    rows + verbs
                }
                Self::State(state) => {
                    optional(&state.last_verb)
                        + optional(&state.last_error)
                        + state
                            .recent
                            .iter()
                            .map(|held| ROW_WEIGHT + text(held))
                            .sum::<u64>()
                }
                Self::Facts(facts) => facts
                    .facts
                    .iter()
                    .map(|fact| {
                        ROW_WEIGHT
                            + text(&fact.key)
                            + match &fact.value {
                                FactValue::Text(held) | FactValue::Number(held) => text(held),
                                _ => 0,
                            }
                    })
                    .sum(),
            }
    }
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
    match format {
        Format::Lines => read_lines(reader, limits),
        Format::Document => read_document(reader, limits),
    }
}

/// One line of a step log, as far as a digest reads it. The window's recorder
/// writes more (`frame`, `acts`, …); what is not named here is skipped.
#[derive(Deserialize)]
struct StepRead {
    #[serde(default)]
    n: usize,
    #[serde(default)]
    at_epoch_ms: i64,
    #[serde(default)]
    tool: String,
    verb: String,
    #[serde(default)]
    argv: Vec<String>,
    ok: bool,
    #[serde(default)]
    observation: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    shot: Option<String>,
}

/// The keys of a step's `observation` that say how long it took, in the order
/// they are tried.
const DURATION_KEYS: [&str; 2] = ["act_ms", "elapsed_ms"];

fn read_lines(mut reader: impl BufRead, limits: &Limits) -> Option<Digest> {
    let mut budget = limits.digest_bytes_max;
    let mut line: Vec<u8> = Vec::new();
    let mut fold = StepFold::new(limits);
    let mut other: Option<serde_json::Value> = None;
    let mut records = 0usize;
    let mut skipped = 0usize;
    let mut cut = false;
    loop {
        line.clear();
        let Ok(read) = (&mut reader).take(budget).read_until(b'\n', &mut line) else {
            break;
        };
        if read == 0 {
            cut = budget == 0 && more_to_read(&mut reader);
            break;
        }
        budget = budget.saturating_sub(read as u64);
        if budget == 0 && !line.ends_with(b"\n") && more_to_read(&mut reader) {
            // The table's last byte fell inside this line: it is not a torn
            // line of the log, it is where the reading stopped.
            cut = true;
            break;
        }
        let text = line.trim_ascii();
        if text.is_empty() {
            continue;
        }
        if let Ok(step) = serde_json::from_slice::<StepRead>(text) {
            fold.push(step);
        } else if let Ok(value) = serde_json::from_slice::<serde_json::Value>(text)
            && value.is_object()
        {
            records += 1;
            if other.is_none() {
                other = Some(value);
            }
        } else {
            skipped += 1;
        }
    }
    if fold.out.total > 0 {
        return Some(Digest::Steps(fold.finish(skipped + records, cut)));
    }
    let first = other?;
    Some(Digest::Facts(facts_of(&first, records, cut, limits)))
}

/// Whether the reader still holds bytes — asked only once the table's bytes
/// are spent, to tell a log that ended there from one that goes on.
fn more_to_read(reader: &mut impl BufRead) -> bool {
    reader.fill_buf().is_ok_and(|rest| !rest.is_empty())
}

fn read_document(reader: impl BufRead, limits: &Limits) -> Option<Digest> {
    let mut held = Vec::new();
    let mut bounded = reader.take(limits.digest_bytes_max.saturating_add(1));
    bounded.read_to_end(&mut held).ok()?;
    let cut = held.len() as u64 > limits.digest_bytes_max;
    if cut {
        // A document cut short is not a document any more.
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&held).ok()?;
    let is_state = value
        .as_object()
        .is_some_and(|map| STATE_KEYS.iter().all(|key| map.contains_key(*key)));
    if is_state {
        let state: State = serde_json::from_value(value).ok()?;
        return Some(Digest::State(State {
            last_verb: state.last_verb.map(|said| bounded_line(&[said], limits)),
            last_error: state.last_error.map(|said| bounded_line(&[said], limits)),
            recent: state
                .recent
                .into_iter()
                .take(limits.digest_facts_max)
                .map(|said| bounded_line(&[said], limits))
                .collect(),
            actions: state.actions,
            last_ok: state.last_ok,
            last_at_ms: state.last_at_ms,
            consecutive_failures: state.consecutive_failures,
            unchanged_looks: state.unchanged_looks,
            stuck: state.stuck,
        }));
    }
    match &value {
        serde_json::Value::Object(_) => Some(Digest::Facts(facts_of(&value, 1, false, limits))),
        serde_json::Value::Array(items) => {
            let first = items.iter().find(|item| item.is_object())?;
            Some(Digest::Facts(facts_of(first, items.len(), false, limits)))
        }
        _ => None,
    }
}

/// The top-level values of one record, bounded by the table.
fn facts_of(record: &serde_json::Value, records: usize, cut: bool, limits: &Limits) -> Facts {
    let Some(map) = record.as_object() else {
        return Facts {
            records,
            cut,
            ..Facts::default()
        };
    };
    let facts = map
        .iter()
        .take(limits.digest_facts_max)
        .map(|(key, value)| Fact {
            key: bounded_line(std::slice::from_ref(key), limits),
            value: match value {
                serde_json::Value::Null => FactValue::Nothing,
                serde_json::Value::Bool(flag) => FactValue::Flag(*flag),
                serde_json::Value::Number(number) => FactValue::Number(number.to_string()),
                serde_json::Value::String(text) => {
                    FactValue::Text(bounded_line(std::slice::from_ref(text), limits))
                }
                serde_json::Value::Array(items) => FactValue::List(items.len()),
                serde_json::Value::Object(fields) => FactValue::Record(fields.len()),
            },
        })
        .collect();
    Facts {
        records,
        cut,
        facts,
        more: map.len().saturating_sub(limits.digest_facts_max),
    }
}

/// Words on one line: runs of whitespace (a script's newlines among them) are
/// one space, and the line stops at the table's characters. Built word by
/// word, so an argument of a megabyte costs what is kept of it.
fn bounded_line(words: &[String], limits: &Limits) -> String {
    let max = limits.digest_text_chars;
    let mut line = String::new();
    let mut held = 0usize;
    for word in words.iter().flat_map(|word| word.split_whitespace()) {
        if held >= max {
            break;
        }
        if held > 0 {
            line.push(' ');
            held += 1;
        }
        for glyph in word.chars() {
            if held >= max {
                break;
            }
            line.push(glyph);
            held += 1;
        }
    }
    line
}

/// How long a step took, from its `observation`.
fn duration_of(observation: Option<&serde_json::Value>) -> Option<u64> {
    let observation = observation?;
    DURATION_KEYS
        .iter()
        .find_map(|key| observation.get(*key)?.as_u64())
}

/// The single pass over a log's steps: counts everything, keeps the rows worth
/// showing and folds the runs between them.
struct StepFold<'a> {
    limits: &'a Limits,
    out: Steps,
    verbs: BTreeMap<String, usize>,
    /// Passed steps that may still be shown: as the neighbours of a failed
    /// step that follows them, or as the end of the log.
    held: VecDeque<StepRow>,
    /// Steps left out since the last row shown, and the failed among them.
    folded: usize,
    folded_failed: usize,
    /// Steps still to show after a failed one.
    after: usize,
    /// Step rows shown so far.
    shown: usize,
}

impl<'a> StepFold<'a> {
    fn new(limits: &'a Limits) -> Self {
        Self {
            limits,
            out: Steps::default(),
            verbs: BTreeMap::new(),
            held: VecDeque::new(),
            folded: 0,
            folded_failed: 0,
            after: 0,
            shown: 0,
        }
    }

    fn push(&mut self, read: StepRead) {
        let limits = self.limits;
        self.out.total += 1;
        if !read.ok {
            self.out.failed += 1;
        }
        if self.out.started_ms.is_none() {
            self.out.started_ms = Some(read.at_epoch_ms);
        }
        self.out.ended_ms = Some(read.at_epoch_ms);
        self.count_verb(&read.verb);
        let words: &[String] = match read.argv.split_first() {
            Some((first, rest)) if *first == read.verb => rest,
            _ => &read.argv,
        };
        let row = StepRow {
            n: if read.n > 0 { read.n } else { self.out.total },
            at_ms: read.at_epoch_ms,
            target: bounded_line(words, limits),
            ms: duration_of(read.observation.as_ref()),
            error: read
                .error
                .map(|said| bounded_line(std::slice::from_ref(&said), limits)),
            code: read.code,
            framed: read.shot.is_some(),
            ok: read.ok,
            tool: read.tool,
            verb: read.verb,
        };
        if self.out.total <= limits.digest_head_steps {
            self.show(row);
        } else if !row.ok {
            // A failed step is shown with the steps on each side of it.
            let keep = limits.digest_context_steps.min(self.held.len());
            self.fold_held_down_to(keep);
            while let Some(before) = self.held.pop_front() {
                self.show(before);
            }
            self.show(row);
            self.after = limits.digest_context_steps;
        } else if self.after > 0 {
            self.after -= 1;
            self.show(row);
        } else {
            self.held.push_back(row);
            let room = limits.digest_context_steps.max(limits.digest_tail_steps);
            self.fold_held_down_to(room);
        }
    }

    fn count_verb(&mut self, verb: &str) {
        if let Some(steps) = self.verbs.get_mut(verb) {
            *steps += 1;
        } else if self.verbs.len() < VERBS_TRACKED_MAX {
            self.verbs.insert(verb.to_string(), 1);
        } else {
            self.out.other_verbs += 1;
        }
    }

    /// Leave the oldest held steps out until `keep` are held.
    fn fold_held_down_to(&mut self, keep: usize) {
        while self.held.len() > keep {
            self.held.pop_front();
            self.folded += 1;
        }
    }

    /// Put one step on the list — or, once the table's rows are spent, count
    /// it among the folded.
    fn show(&mut self, row: StepRow) {
        if self.shown >= self.limits.digest_rows_max {
            self.folded += 1;
            if !row.ok {
                self.folded_failed += 1;
            }
            return;
        }
        self.close_fold();
        self.out.rows.push(Row::Step(row));
        self.shown += 1;
    }

    /// Write the run left out so far as one row.
    fn close_fold(&mut self) {
        if self.folded > 0 {
            self.out.rows.push(Row::Fold {
                steps: self.folded,
                failed: self.folded_failed,
            });
            self.folded = 0;
            self.folded_failed = 0;
        }
    }

    fn finish(mut self, skipped: usize, cut: bool) -> Steps {
        let keep = self.limits.digest_tail_steps.min(self.held.len());
        self.fold_held_down_to(keep);
        while let Some(last) = self.held.pop_front() {
            self.show(last);
        }
        self.close_fold();
        let mut verbs: Vec<VerbCount> = std::mem::take(&mut self.verbs)
            .into_iter()
            .map(|(verb, steps)| VerbCount { verb, steps })
            .collect();
        // The most used first; the map already ordered equal counts by name,
        // and the sort keeps that order.
        verbs.sort_by_key(|held| std::cmp::Reverse(held.steps));
        let named = self.limits.digest_verbs_max.min(verbs.len());
        self.out.other_verbs += verbs[named..].iter().map(|held| held.steps).sum::<usize>();
        verbs.truncate(named);
        self.out.verbs = verbs;
        self.out.skipped = skipped;
        self.out.cut = cut;
        self.out
    }
}

#[cfg(test)]
mod tests;
