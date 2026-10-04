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
//! - any other JSON is [`Digest::Facts`]: its top-level values, bounded;
//! - a log that holds a test runner's summary or a failed test's line is
//!   [`Digest::Tests`]: the tests that passed, failed and were ignored, and
//!   the names of the failed ones (`run_log`).
//!
//! The extension only says how to read the bytes ([`Format`]): as JSON line by
//! line or as one document, as the lines of a text log, or as the one number
//! of an exit-code file. A text file with nothing to count has no digest and
//! is shown as the text it is.
//!
//! What a file cannot say of itself — the exit code of the run that wrote it,
//! and whether the run was meant to fail — is told from outside ([`told`]).
//!
//! Every number is a field of the artifact table ([`Limits`]). A log is read
//! one line at a time and only counters and the kept rows are held, so its
//! cost in memory is the table's and not the file's.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, Read as _};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::artifact::Limits;

mod run_log;

pub use run_log::{Tests, Told, Verdict, exit_code};

/// How an evidence file's bytes are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One JSON record per line (`.jsonl`).
    Lines,
    /// One JSON document (`.json`).
    Document,
    /// The lines of a text log (`.log`, `.out`, `.txt`).
    Text,
    /// The exit code of a run, alone in its file (`.rc`).
    ExitCode,
}

/// The extensions a digest is read from, lower-cased and without the dot.
const FORMAT_BY_EXTENSION: &[(&str, Format)] =
    &[("jsonl", Format::Lines), ("json", Format::Document)];

/// How many different verbs a log's count may name before the rest are summed
/// unnamed: a log is not trusted to hold few of them.
const VERBS_TRACKED_MAX: usize = 64;

/// How many different tasks a log's count may name, for the same reason. A
/// step of a task past it is still counted as a step that names a task.
const TASKS_TRACKED_MAX: usize = 64;

/// The keys of a step's `observation` that say who the step was taken for: the
/// task of the worker the ledger seats in the pane the command came from, and
/// that pane. The window's recorder writes them and this module reads the
/// task; a step no seated worker asked for carries neither.
pub const STEP_TASK_KEY: &str = "task";
pub const STEP_PANE_KEY: &str = "pane";

impl Format {
    /// The format a path's extension names; `None` for a file no digest is
    /// read from.
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
    Tests(Tests),
}

/// What one listed row weighs beside its texts — its numbers, its flags and the
/// allocations' own headers — when the preview cache counts a digest against
/// its byte cap. An estimate: the cap bounds memory, it does not meter it.
const ROW_WEIGHT: u64 = 96;

impl Digest {
    /// About how many bytes the digest holds: every text it keeps, and
    /// a fixed weight (`ROW_WEIGHT`) for each row, verb, fact, name and for
    /// itself.
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
                    let tasks: u64 = steps
                        .tasks
                        .iter()
                        .map(|held| ROW_WEIGHT + text(&held.task))
                        .sum();
                    rows + verbs + tasks
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
                Self::Tests(tests) => tests
                    .failed_names
                    .iter()
                    .map(|name| ROW_WEIGHT + text(name))
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
    /// Steps that name the task they were taken for. A session's log runs
    /// across tasks, and a step taken for none names none.
    pub tasked: usize,
    /// The tasks named, the one with most steps first, as many as the table
    /// lists.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<TaskSteps>,
    /// What the drawer lists: steps, and the folded runs between them.
    pub rows: Vec<Row>,
}

/// How many steps of a log one task has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskSteps {
    pub task: String,
    pub steps: usize,
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

/// Read one evidence file into its digest; `None` when it holds nothing the
/// format's reader counts.
#[must_use]
pub fn read(format: Format, reader: impl BufRead, limits: &Limits) -> Option<Digest> {
    match format {
        Format::Lines => read_lines(reader, limits),
        Format::Document => read_document(reader, limits),
        // Red (t-36910 stage 2): a text log and an exit code are not read yet.
        Format::Text | Format::ExitCode => None,
    }
}

/// A file's digest as what is known from outside the file qualifies it: a test
/// log is judged again with the run's exit code and with what the hand-in said
/// to expect; a file with nothing to count becomes a verdict once an exit code
/// is known for it; a digest of any other shape is as it was.
#[must_use]
pub fn told(digest: Option<Digest>, said: Told) -> Option<Digest> {
    // Red (t-36910 stage 2): nothing told reaches a digest yet.
    let _ = said;
    digest
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

impl StepRead {
    /// The task the step was taken for, when its observation names one.
    fn task(&self) -> Option<&str> {
        self.observation
            .as_ref()?
            .get(STEP_TASK_KEY)?
            .as_str()
            .filter(|task| !task.is_empty())
    }
}

/// The keys of a step's `observation` that say how long it took, in the order
/// they are tried.
const DURATION_KEYS: [&str; 2] = ["act_ms", "elapsed_ms"];

/// Hand a reader's lines to `see`, one at a time and each with its newline,
/// until the table's bytes are spent; answers whether the file goes on past
/// them. A line the last byte fell inside is not handed over: it is where the
/// reading stopped, not a torn line of the file.
fn each_line(mut reader: impl BufRead, limits: &Limits, mut see: impl FnMut(&[u8])) -> bool {
    let mut budget = limits.digest_bytes_max;
    let mut line: Vec<u8> = Vec::new();
    loop {
        line.clear();
        let Ok(read) = (&mut reader).take(budget).read_until(b'\n', &mut line) else {
            return false;
        };
        if read == 0 {
            return budget == 0 && more_to_read(&mut reader);
        }
        budget = budget.saturating_sub(read as u64);
        if budget == 0 && !line.ends_with(b"\n") && more_to_read(&mut reader) {
            return true;
        }
        see(&line);
    }
}

fn read_lines(reader: impl BufRead, limits: &Limits) -> Option<Digest> {
    let mut fold = StepFold::new(limits);
    let mut other: Option<serde_json::Value> = None;
    let mut records = 0usize;
    let mut skipped = 0usize;
    let cut = each_line(reader, limits, |line| {
        let text = line.trim_ascii();
        if text.is_empty() {
            return;
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
    });
    if fold.out.total > 0 {
        return Some(Digest::Steps(fold.finish(skipped + records, cut)));
    }
    let first = other?;
    Some(Digest::Facts(facts_of(&first, records, cut, limits)))
}

/// The tasks the step lines of a log name, each once and in the order first
/// named, as many as the table lists — what the catalog tags a step log's row
/// with, so a task finds the sessions that worked for it without reading them.
#[must_use]
pub fn tasks_named(reader: impl BufRead, limits: &Limits) -> Vec<String> {
    // Red (t-36910 stage 2): a step log names no task yet.
    let _ = (reader, limits);
    Vec::new()
}

/// A text log, read for what its test runners said.
fn read_text(reader: impl BufRead, limits: &Limits) -> Option<Digest> {
    let mut fold = run_log::TestsFold::new(limits);
    let cut = each_line(reader, limits, |line| fold.see(line));
    fold.finish(cut).map(Digest::Tests)
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

/// Count one more of `name` — unless the count already names `cap` others, and
/// then answer that it was not counted by name.
fn count_by_name(counts: &mut BTreeMap<String, usize>, name: &str, cap: usize) -> bool {
    if let Some(held) = counts.get_mut(name) {
        *held += 1;
    } else if counts.len() < cap {
        counts.insert(name.to_string(), 1);
    } else {
        return false;
    }
    true
}

/// A count by name as a list, the most counted first. The map already ordered
/// equal counts by name, and the sort keeps that order.
fn most_first(counts: BTreeMap<String, usize>) -> Vec<(String, usize)> {
    let mut held: Vec<(String, usize)> = counts.into_iter().collect();
    held.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    held
}

/// The single pass over a log's steps: counts everything, keeps the rows worth
/// showing and folds the runs between them.
struct StepFold<'a> {
    limits: &'a Limits,
    out: Steps,
    verbs: BTreeMap<String, usize>,
    tasks: BTreeMap<String, usize>,
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
            tasks: BTreeMap::new(),
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
        if !count_by_name(&mut self.verbs, &read.verb, VERBS_TRACKED_MAX) {
            self.out.other_verbs += 1;
        }
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
        let mut verbs = most_first(std::mem::take(&mut self.verbs));
        let named = self.limits.digest_verbs_max.min(verbs.len());
        self.out.other_verbs += verbs[named..].iter().map(|(_, steps)| steps).sum::<usize>();
        verbs.truncate(named);
        self.out.verbs = verbs
            .into_iter()
            .map(|(verb, steps)| VerbCount { verb, steps })
            .collect();
        self.out.tasks = most_first(std::mem::take(&mut self.tasks))
            .into_iter()
            .take(self.limits.digest_tasks_max)
            .map(|(task, steps)| TaskSteps { task, steps })
            .collect();
        self.out.skipped = skipped;
        self.out.cut = cut;
        self.out
    }
}

#[cfg(test)]
mod tests;
