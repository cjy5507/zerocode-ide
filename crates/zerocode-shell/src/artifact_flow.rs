//! The flow of the project a person is looking at (t-68458) — its shape
//! only, before anything is read: the answer the window reads and the names of
//! its readers, every reader answering nothing yet. `tests.rs` says what each
//! one must answer.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zerocode_core::artifact::Artifact;
use zerocode_core::orchestration::Ledger;
use zerocode_core::orchestration::attempt_time::task_time;

/// What a line carries of the catalog: its report, its page and the counts of
/// what it left.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Chips {
    pub(crate) report: Option<String>,
    pub(crate) page: Option<String>,
    pub(crate) parts: BTreeMap<String, usize>,
    pub(crate) pairs: usize,
    pub(crate) singles: usize,
    pub(crate) sessions: usize,
    pub(crate) attempts: usize,
    #[serde(skip)]
    pub(crate) lead_text: Option<String>,
}

/// Every number the flow has, in one place.
pub(crate) struct FlowTable {
    /// Releases drawn one line each, newest first; the older ones fold into
    /// one line.
    pub(crate) releases_kept: usize,
    /// First-parent commits of the base branch read for merges and releases.
    /// The twelve newest releases of this repository lay within 55 of them on
    /// 2026-10-21.
    pub(crate) commits_walked: usize,
    /// Tasks one group carries; the count carries the rest.
    pub(crate) group_tasks_max: usize,
    /// Rows no task is linked to, in one group's 「기록」.
    pub(crate) records_max: usize,
    /// Characters of a line's sentence, of a task's title, of a before →
    /// after, of one of a report's three lines and of a hand-in's words.
    pub(crate) said_chars: usize,
    pub(crate) title_chars: usize,
    pub(crate) gain_chars: usize,
    pub(crate) three_chars: usize,
    pub(crate) summary_chars: usize,
    /// How long git's answer stands before the base's commit is asked again.
    pub(crate) git_fresh_ms: i64,
    /// The longest one git question may take before it is killed.
    pub(crate) git_budget: Duration,
    /// The release notes, read at the base commit.
    pub(crate) notes_file: &'static str,
}

pub(crate) const FLOW: FlowTable = FlowTable {
    releases_kept: 12,
    commits_walked: 240,
    group_tasks_max: 40,
    records_max: 6,
    said_chars: 140,
    title_chars: 80,
    gain_chars: 48,
    three_chars: 200,
    summary_chars: 480,
    git_fresh_ms: 5_000,
    git_budget: Duration::from_secs(3),
    notes_file: "CHANGELOG.md",
};

/// Where a line's sentence came from — the order it is chosen in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Said {
    /// The first sentence of the hand-in the worker wrote for a person.
    Summary,
    /// The words after the 「 — 」 of the merge commit's subject.
    Merge,
    /// The release notes' line for the task.
    Notes,
    /// Nothing said what was done: the task's own title.
    Title,
}

/// Who told a step's facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Told {
    Git,
    Ledger,
}

/// What the window asks: the project's folder as it lists the project, and the
/// folders the project's rows can lie under (the project and its worktrees —
/// the gallery's own project filter).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct FlowAsk {
    #[serde(default)]
    pub(crate) root: String,
    #[serde(default)]
    pub(crate) roots: Vec<String>,
}

/// One project's flow.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Flow {
    pub(crate) root: String,
    /// The branch walked — what 「main에 반영됨」 names.
    pub(crate) base: Option<String>,
    /// Work not on the base yet: carried, reported, verified, held.
    pub(crate) now: FlowGroup,
    /// Since the newest release: landed, and work that ended without landing.
    pub(crate) next: FlowGroup,
    /// The kept releases, newest first.
    pub(crate) releases: Vec<FlowRelease>,
    /// The releases past the kept ones that the walk saw, as one line.
    pub(crate) older: Option<FlowOlder>,
    /// Whether this window's ledger answered. Without it the flow is git's alone.
    pub(crate) ledger: bool,
    /// Where the lines' facts came from, counted over every line.
    pub(crate) sources: FlowSources,
}

/// One group's lines, and the rows no task is linked to in its time.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct FlowGroup {
    pub(crate) tasks: Vec<FlowTask>,
    /// Tasks past [`FlowTable::group_tasks_max`].
    pub(crate) more: usize,
    pub(crate) records: Vec<Artifact>,
}

/// One release and the work it shipped.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct FlowRelease {
    pub(crate) version: String,
    pub(crate) commit: String,
    pub(crate) at_ms: i64,
    #[serde(flatten)]
    pub(crate) group: FlowGroup,
}

/// The releases past the kept ones, said as one line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct FlowOlder {
    pub(crate) releases: usize,
    pub(crate) newest: String,
    pub(crate) oldest: String,
    pub(crate) tasks: usize,
    pub(crate) until_ms: i64,
}

/// How many lines took their facts from where — the parts add up to the lines.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct FlowSources {
    pub(crate) said: BTreeMap<Said, usize>,
    pub(crate) land: BTreeMap<Told, usize>,
    pub(crate) ship: BTreeMap<Told, usize>,
}

/// A report's three lines as the plain-report rule writes them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Three {
    pub(crate) what: String,
    pub(crate) why: String,
    pub(crate) cost: String,
}

/// One task's line.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct FlowTask {
    pub(crate) task: String,
    /// The run that holds it; `None` for a task only git names.
    pub(crate) run: Option<String>,
    pub(crate) title: String,
    /// What was done, in one sentence.
    pub(crate) said: String,
    pub(crate) said_from: Said,
    /// Before → after, as the hand-in says it.
    pub(crate) gain: Option<String>,
    /// The desk's one stage word ([`stage_of`]), moved on by what git says
    /// landed (`merged`) and shipped (`deployed`).
    pub(crate) stage: &'static str,
    /// The stage word as the ledger alone says it; `None` for a task only git
    /// names.
    pub(crate) ledger_stage: Option<&'static str>,
    pub(crate) three: Option<Three>,
    /// The hand-in's words, bounded — what the line opens to when its report
    /// has no three lines.
    pub(crate) summary: Option<String>,
    pub(crate) opened_ms: Option<i64>,
    /// The task's newest moment — what a group orders its lines by.
    pub(crate) last_ms: i64,
    pub(crate) attempts: Vec<FlowAttempt>,
    pub(crate) hand_in: Option<FlowHandIn>,
    pub(crate) review: Option<FlowReview>,
    pub(crate) land: Option<FlowLand>,
    pub(crate) ship: Option<FlowShip>,
    pub(crate) chips: Chips,
}

/// One attempt on a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FlowAttempt {
    pub(crate) dispatch: String,
    pub(crate) worker: String,
    pub(crate) agent: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) started_ms: i64,
    pub(crate) ended_ms: Option<i64>,
    pub(crate) succeeded: Option<bool>,
    /// What it handed in: the commit, or the report.
    pub(crate) source: Option<String>,
    /// The first sentence of the last report it sent.
    pub(crate) said: Option<String>,
}

/// The hand-in a review is about, or the newest one before any review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FlowHandIn {
    pub(crate) attempt: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) at_ms: Option<i64>,
}

/// A coordinator's review that stands for the work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FlowReview {
    pub(crate) at_ms: Option<i64>,
    pub(crate) source: Option<String>,
}

/// Where the work landed on the base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FlowLand {
    pub(crate) at_ms: Option<i64>,
    pub(crate) commit: Option<String>,
    /// The merge subject's words after its 「 — 」.
    pub(crate) line: Option<String>,
    pub(crate) told: Told,
}

/// The release the work shipped in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FlowShip {
    pub(crate) version: String,
    pub(crate) at_ms: Option<i64>,
    pub(crate) told: Told,
}

/* ---- git --------------------------------------------------------------- */

/// The mark between a merge subject's head and its one line, and between a
/// release note's line and the rest of it.
const LINE_MARK: &str = " — ";

/// The format the walk asks git for: commit, parents, commit time, subject and
/// the tags on it, a unit separator apart.
const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%ct%x1f%s%x1f%D";

/// One merge on the base branch's first-parent line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GitMerge {
    pub(crate) commit: String,
    /// The commit it brought in: the `at <sha>` its subject names, else its
    /// second parent.
    pub(crate) head: Option<String>,
    pub(crate) at_ms: i64,
    /// The subject's words after its 「 — 」.
    pub(crate) line: Option<String>,
    /// The tasks its subject names: the first in what was merged, and every
    /// one in its parentheses.
    pub(crate) tasks: Vec<String>,
}

/// A release, and the merges between it and the release before it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GitRelease {
    pub(crate) version: String,
    pub(crate) commit: String,
    pub(crate) at_ms: i64,
    pub(crate) merges: Vec<GitMerge>,
}

/// A release note's line for one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Note {
    pub(crate) version: String,
    pub(crate) line: String,
}

/// What git says of a project's history, read once per base commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GitFacts {
    pub(crate) base: Option<String>,
    /// Merges after the newest release, newest first.
    pub(crate) unreleased: Vec<GitMerge>,
    /// Releases, newest first.
    pub(crate) releases: Vec<GitRelease>,
    /// The release notes' line for each task they name.
    pub(crate) notes: HashMap<String, Note>,
}

/// The walk of the base branch's first-parent line, newest first.
pub(crate) fn parse_log(_text: &str) -> (Vec<GitMerge>, Vec<GitRelease>) {
    (Vec::new(), Vec::new())
}

/// A merge the project's tasks can be found in.
fn merge_of(_commit: &str, _parents: &[&str], _at_ms: i64, _subject: &str) -> Option<GitMerge> {
    None
}

/// Every task id a text names.
pub(crate) fn task_ids(_text: &str) -> Vec<String> {
    Vec::new()
}

/// The release notes' line for each task they name.
pub(crate) fn parse_notes(_text: &str, _releases: usize) -> HashMap<String, Note> {
    HashMap::new()
}

/// What git says, kept for one repository.
struct GitHeld;

fn git_held() -> &'static Mutex<Vec<GitHeld>> {
    static HELD: OnceLock<Mutex<Vec<GitHeld>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Vec::new()))
}

/// What git says of the repository at `root`.
fn git_facts(_root: &Path, _now_ms: i64) -> Arc<GitFacts> {
    Arc::default()
}

/// The first sentence of words a person reads, bounded.
pub(crate) fn first_sentence(_text: &str, _chars: usize) -> Option<String> {
    None
}

/// A line's before → after.
pub(crate) fn gain_of(
    _numbers: Option<&Value>,
    _summary: Option<&str>,
    _chars: usize,
) -> Option<String> {
    None
}

/// What, why and cost — a report's three lines.
pub(crate) fn three_of(_text: &str, _chars: usize) -> Option<Three> {
    None
}

/// What a flow is read from.
pub(crate) struct Reading<'a> {
    pub(crate) root: &'a str,
    pub(crate) project: Option<&'a str>,
    pub(crate) ledger: Option<&'a Ledger>,
    pub(crate) git: &'a GitFacts,
    pub(crate) table: &'a FlowTable,
}

/// The flow of one project, as the ledger and git say it.
pub(crate) fn assemble(reading: &Reading<'_>) -> Flow {
    Flow {
        root: reading.root.to_string(),
        ..Flow::default()
    }
}

/// Every line of a flow.
fn lines_of(flow: &Flow) -> impl Iterator<Item = &FlowTask> {
    flow.now.tasks.iter().chain(&flow.next.tasks).chain(
        flow.releases
            .iter()
            .flat_map(|release| &release.group.tasks),
    )
}

impl Flow {
    /// The tasks its lines are about.
    pub(crate) fn task_ids(&self) -> BTreeSet<String> {
        lines_of(self).map(|line| line.task.clone()).collect()
    }

    /// The earliest moment a 「기록」 can stand in.
    pub(crate) fn records_since(&self, _git: &GitFacts) -> i64 {
        i64::MIN
    }
}

/// Lay the catalog over a flow.
pub(crate) fn dress(
    _flow: &mut Flow,
    _chips: &BTreeMap<String, Chips>,
    _records: Vec<Artifact>,
    _table: &FlowTable,
) {
}

/// The flow of the project at `ask.root`.
pub(crate) fn read(ask: &FlowAsk, _now_ms: i64) -> Flow {
    Flow {
        root: ask.root.clone(),
        ..Flow::default()
    }
}

#[cfg(test)]
mod tests;
