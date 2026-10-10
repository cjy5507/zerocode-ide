//! The evidence a success report owes for each pass condition its task wrote
//! (t-26587), and the one place that reads it.
//!
//! A task's spec names its pass conditions one per line: a line that begins
//! with [`CONDITION_MARK`] (after its spaces and an optional list number or
//! bullet) is one condition, numbered from one in spec order. A `worker_done`
//! that says `ok: true` for such a task has to show, for every condition, an
//! `evidence` entry — the build-line job that ran it, its exit code and what it
//! printed — before the ledger closes the task. A receipt the report names
//! beside the job (`<job>.rc`) is read by the window and must say the same exit
//! code: a receipt that says otherwise is not a receipt.
//!
//! This module is pure. The window reads the receipt files and hands them in as
//! [`ReceiptFiles`]; nothing here touches the disk. The send verb judges a
//! report with [`judge_success`], the board reads a task with [`hand_in_of`],
//! and the briefing reads the conditions with [`briefing_paragraph`], so the
//! three cannot disagree about what a condition is.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use super::{MessageKind, Run, Task, worker_done_succeeded};
use crate::hand_in;

/// The mark a spec line begins with to be a pass condition. One constant, so
/// the spec the coordinator writes and the words the briefing and the board
/// read back are the same words.
pub const CONDITION_MARK: &str = "통과 전";
/// The exit code a condition passes at.
pub const PASSING_RC: i64 = 0;
/// The most bytes of one receipt the window reads. An exit code is one line;
/// a longer file is not a receipt.
pub const RECEIPT_BYTES_MAX: u64 = 64;
/// The extension a job's receipt carries: `<job>.rc` is the receipt of `<job>`.
pub const RECEIPT_EXTENSION: &str = "rc";

/// The body key the evidence list is named under.
pub const LIST_KEY: &str = "evidence";
/// The keys one evidence entry carries.
pub const CONDITION_KEY: &str = "condition";
pub const JOB_KEY: &str = "job";
pub const RC_KEY: &str = "rc";
pub const NUMBERS_KEY: &str = "numbers";

/// The optional keys of a hand-in the board shows beside its task: what the
/// worker decided, what blocks it, and what comes next.
pub const DECISIONS_KEY: &str = "decisions";
pub const BLOCKED_KEY: &str = "blocked";
pub const NEXT_KEY: &str = "next";

/// The conditions a spec writes, in spec order, each as the words after its mark.
///
/// A line is a condition when, after its surrounding spaces and an optional
/// list marker (`1.`, `2)`, `-`, `*`, `•`), it begins with [`CONDITION_MARK`]
/// and the mark ends at a space, a colon or the end of the line — so `통과 전체`
/// is not one.
#[must_use]
pub fn conditions(spec: &str) -> Vec<&str> {
    spec.lines().filter_map(condition_words).collect()
}

fn condition_words(line: &str) -> Option<&str> {
    let rest = strip_marker(line.trim()).strip_prefix(CONDITION_MARK)?;
    match rest.chars().next() {
        None => Some(""),
        Some(next) if next.is_whitespace() || next == ':' => {
            Some(rest.trim_matches(|c: char| c.is_whitespace() || c == ':'))
        }
        Some(_) => None,
    }
}

/// The line without a leading list marker: any bullets, then a number and a
/// `.` or `)`.
fn strip_marker(line: &str) -> &str {
    let bare = line.trim_start_matches(['-', '*', '•']).trim_start();
    let digits = bare.trim_start_matches(|c: char| c.is_ascii_digit());
    match digits.strip_prefix(['.', ')']) {
        Some(after) if digits.len() < bare.len() => after.trim_start(),
        _ => bare,
    }
}

/// The receipt files a payload names: its evidence paths that end in `.rc`.
#[must_use]
pub fn receipts_named(payload: &str) -> Vec<String> {
    hand_in::named(payload, "")
        .evidence
        .into_iter()
        .map(|item| item.path.to_string_lossy().into_owned())
        .filter(|path| is_receipt(path))
        .collect()
}

fn is_receipt(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension == RECEIPT_EXTENSION)
}

/// The job a receipt path belongs to: `<job>.rc` is the receipt of `<job>`.
fn job_of_receipt(path: &str) -> Option<&str> {
    let path = Path::new(path);
    if !is_receipt(path.to_str()?) {
        return None;
    }
    path.file_stem()?.to_str()
}

/// What the window read of the receipt files a report names, keyed by the path
/// exactly as the payload names it. A path that could not be read is absent, and
/// a report that names an absent receipt is refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiptFiles(BTreeMap<String, String>);

impl ReceiptFiles {
    pub fn insert(&mut self, path: impl Into<String>, text: impl Into<String>) {
        self.0.insert(path.into(), text.into());
    }

    #[must_use]
    pub fn get(&self, path: &str) -> Option<&str> {
        self.0.get(path).map(String::as_str)
    }
}

/// One evidence entry, read whole.
struct Entry<'a> {
    condition: usize,
    job: &'a str,
    rc: i64,
}

/// Reads one evidence entry: a condition number in range, the job's name, its
/// exit code, and the numbers it printed (required, though not judged).
fn entry_of(entry: &Value, written: usize) -> Result<Entry<'_>, String> {
    let object = entry.as_object().ok_or("it is not an object")?;
    let in_range = |number: &u64| (1..=written as u64).contains(number);
    let condition = object
        .get(CONDITION_KEY)
        .and_then(Value::as_u64)
        .filter(in_range)
        .ok_or_else(|| format!("{CONDITION_KEY} must be a condition number from 1 to {written}"))?;
    let job = object
        .get(JOB_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|job| !job.is_empty())
        .ok_or_else(|| format!("{JOB_KEY} must name the build-line job that ran the condition"))?;
    let rc = object
        .get(RC_KEY)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{RC_KEY} must be the job's exit code as a whole number"))?;
    object
        .get(NUMBERS_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|numbers| !numbers.is_empty())
        .ok_or_else(|| format!("{NUMBERS_KEY} must say what the job printed"))?;
    Ok(Entry {
        condition: condition as usize,
        job,
        rc,
    })
}

/// Judges a success report for a task whose spec writes pass conditions.
///
/// `Ok` when every condition has an evidence entry, every entry is well formed
/// and passes ([`PASSING_RC`]), and each entry agrees with the receipt the
/// payload names for its job, when there is one. Anything else is `Err` with
/// the sentence the worker reads, and the report is not taken. A spec with no
/// conditions is always `Ok`.
pub fn judge_success(
    spec: &str,
    body: &str,
    payload: &str,
    receipts: &ReceiptFiles,
) -> Result<(), String> {
    let written = conditions(spec).len();
    if written == 0 {
        return Ok(());
    }
    let said: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let entries: &[Value] = match said.get(LIST_KEY) {
        None => &[],
        Some(Value::Array(list)) => list,
        Some(_) => {
            return Err(format!(
                "{LIST_KEY} must be a list, one entry per condition"
            ));
        }
    };
    let named = receipts_named(payload);
    let mut shown = vec![false; written];
    for (index, value) in entries.iter().enumerate() {
        let entry = entry_of(value, written)
            .map_err(|why| format!("evidence entry {} is malformed: {why}", index + 1))?;
        let condition = entry.condition;
        if entry.rc != PASSING_RC {
            return Err(format!(
                "condition {condition}: job {} exited with rc {}, and a condition passes only at rc {PASSING_RC}",
                entry.job, entry.rc
            ));
        }
        if let Some(path) = named
            .iter()
            .find(|path| job_of_receipt(path) == Some(entry.job))
        {
            match receipts.get(path).map(|text| text.trim().parse::<i64>()) {
                None => {
                    return Err(format!(
                        "condition {condition}: the receipt {path} is named but could not be read, so job {} cannot be checked",
                        entry.job
                    ));
                }
                Some(Err(_)) => {
                    return Err(format!(
                        "condition {condition}: the receipt {path} holds no exit code"
                    ));
                }
                Some(Ok(file_rc)) if file_rc != entry.rc => {
                    return Err(format!(
                        "condition {condition}: the receipt for job {} says rc {file_rc}, and the report says rc {}",
                        entry.job, entry.rc
                    ));
                }
                Some(Ok(_)) => {}
            }
        }
        shown[condition - 1] = true;
    }
    let missing: Vec<String> = shown
        .iter()
        .enumerate()
        .filter(|(_, seen)| !**seen)
        .map(|(index, _)| format!("condition {}", index + 1))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "worker_done says ok:true, but no evidence was sent for {} of {written} conditions: {}. \
         Send the same --retry-request again with one \"{LIST_KEY}\" entry per condition, for example \
         {{\"{CONDITION_KEY}\":1,\"{JOB_KEY}\":\"<build-line job name>\",\"{RC_KEY}\":0,\"{NUMBERS_KEY}\":\"<what the job printed>\"}}. \
         The task stays open until then.",
        missing.len(),
        missing.join(", ")
    ))
}

/// Where a condition stands on the board, read from the last hand-in that named
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionState {
    /// No passing evidence was sent for it.
    Missing,
    /// Passing evidence was sent, and no receipt the ledger read stands behind it.
    Claimed,
    /// The report was taken, and the receipt it names agrees with the evidence.
    Checked,
}

/// One condition of a task as the board shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConditionRow {
    pub number: usize,
    pub text: String,
    pub state: ConditionState,
}

/// What the board shows of a task's last hand-in: its conditions, and the notes
/// the worker left.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TaskHandIn {
    pub conditions: Vec<ConditionRow>,
    pub decisions: Option<String>,
    pub blocked: Option<String>,
    pub next: Option<String>,
}

impl TaskHandIn {
    /// Whether there is nothing to show: a task that writes no conditions and
    /// whose hand-in left no notes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.conditions.is_empty()
            && self.decisions.is_none()
            && self.blocked.is_none()
            && self.next.is_none()
    }
}

/// The hand-in view of `task` in `run`: the state of each condition and the
/// notes of the last `worker_done` that named the task. Read from that message
/// as it was filed; nothing is stored beside the task.
#[must_use]
pub fn hand_in_of(run: &Run, task: &Task) -> TaskHandIn {
    let written = conditions(&task.spec);
    let last = run.messages.iter().rev().find(|one| {
        one.kind == MessageKind::WorkerDone && one.task.as_deref() == Some(task.id.as_str())
    });
    let (states, said) = match last {
        Some(message) => (
            states_of(
                message.body.as_str(),
                message.payload.as_str(),
                written.len(),
            ),
            serde_json::from_str::<Value>(message.body.as_str()).unwrap_or(Value::Null),
        ),
        None => (vec![ConditionState::Missing; written.len()], Value::Null),
    };
    TaskHandIn {
        conditions: written
            .iter()
            .zip(states)
            .enumerate()
            .map(|(index, (text, state))| ConditionRow {
                number: index + 1,
                text: (*text).to_string(),
                state,
            })
            .collect(),
        decisions: note(&said, DECISIONS_KEY),
        blocked: note(&said, BLOCKED_KEY),
        next: note(&said, NEXT_KEY),
    }
}

/// The state of each condition, read from one filed `worker_done`. A condition
/// is `Checked` only when the report was taken as a success and names a receipt
/// for its job (the ledger agreed with that receipt before taking the report);
/// `Claimed` when a passing entry stands without one; `Missing` otherwise.
fn states_of(body: &str, payload: &str, written: usize) -> Vec<ConditionState> {
    let mut states = vec![ConditionState::Missing; written];
    let taken = worker_done_succeeded(body).is_ok_and(|ok| ok);
    let said: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let Some(Value::Array(list)) = said.get(LIST_KEY) else {
        return states;
    };
    let named = receipts_named(payload);
    for value in list {
        let Ok(entry) = entry_of(value, written) else {
            continue;
        };
        if entry.rc != PASSING_RC {
            continue;
        }
        let receipt = named
            .iter()
            .any(|path| job_of_receipt(path) == Some(entry.job));
        let state = match taken && receipt {
            true => ConditionState::Checked,
            false => ConditionState::Claimed,
        };
        let slot = &mut states[entry.condition - 1];
        *slot = (*slot).max(state);
    }
    states
}

/// A note the worker left in its hand-in, when it is a non-empty string.
fn note(said: &Value, key: &str) -> Option<String> {
    said.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The paragraph a worker is briefed with when its task writes pass conditions:
/// each condition, and the evidence its success report owes for each. Empty for
/// a task that writes none, so the briefing of such a task does not change.
#[must_use]
pub fn briefing_paragraph(spec: &str) -> String {
    let written = conditions(spec);
    if written.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!(
        "This task writes {} pass conditions, and a success report (ok:true) is taken only when every one is shown:",
        written.len()
    )];
    lines.extend(
        written
            .iter()
            .enumerate()
            .map(|(index, words)| format!("{}. {words}", index + 1)),
    );
    lines.push(format!(
        "Put one \"{LIST_KEY}\" entry per condition in the --body JSON, beside ok and summary, for example \
         {{\"{CONDITION_KEY}\":1,\"{JOB_KEY}\":\"<the build-line job that ran it>\",\"{RC_KEY}\":0,\"{NUMBERS_KEY}\":\"<what the job printed, with units>\"}}. \
         A condition passes only at rc {PASSING_RC}. A receipt named in --payload evidencePaths as <job>.rc is read by the window, \
         and a receipt that says another exit code refuses the report. A success report without the evidence of every condition \
         is refused and the task stays open: send the same --retry-request again with the evidence."
    ));
    lines.join("\n")
}
