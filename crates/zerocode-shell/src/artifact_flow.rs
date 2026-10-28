//! The flow of the project a person is looking at (t-68458): what happened in
//! it, task by task — the artifact tab's first view.
//!
//! The person passed the second draft (`p-4958d4720ba9e103` v3): three groups —
//! now, before the next release, released — and one line for each task that
//! says what was done, its before → after number, where the work stands on six
//! steps (task, attempt, hand-in, review, landing, release) and what it left.
//! Every fact a line says is one the ledger or git wrote down:
//!
//! - the task, its attempts, the hand-in and the review are the ledger's, read
//!   the way the desk reads them (the desk's one stage word,
//!   [`worker_summary_in`], [`task_time`]);
//! - landing and release are git's. The ledger stamps no moment for a release
//!   (`AfterReport::deployed_ms` is always `None`), and its `merged_ms` is when
//!   the coordinator wrote `merged`, not when main took the work, so the merge
//!   commit on the base branch's first-parent line and the release it falls
//!   under say when — and the line says which of the two told it ([`Told`]);
//! - the release notes at the base give a shipped task's sentence when neither
//!   the hand-in nor the merge has one ([`Said`]).
//!
//! What is read, and when: the command runs off the window's main thread; the
//! ledger is the image this window already holds ([`ledger_image`]); the
//! catalog is the store's memory; git is asked through the host boundary, and
//! its answer is kept by the base's commit — asked again only when that commit
//! moved, which is checked at most every [`FlowTable::git_fresh_ms`]. git is
//! asked only at a repository this window lists, for a folder that belongs to
//! it ([`vouch`], t-69070 P2): the folder a webview names is a request, never
//! git's working directory.
//!
//! [`ledger_image`]: crate::orchestration::ledger_image

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zerocode_core::artifact::Artifact;
use zerocode_core::host::Host;
use zerocode_core::orchestration::attempt_time::task_time;
use zerocode_core::orchestration::settle::{Left, Named, PRESS_TASKS_MAX, left_behind_by};
use zerocode_core::orchestration::{
    Dispatch, Ledger, Message, MessageKind, ResultAuthor, Run, SETTLE_QUIET_DAYS, Task, TaskStatus,
    commit_named, summary_of_body, worker_summary_in,
};

use crate::artifact_runtime::Chips;
use crate::orchestration::desk::stage_of;
use crate::orchestration::desk::word_awaits_review;

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
    /// How long a git failure is remembered before git is asked again.
    pub(crate) git_retry_ms: i64,
    /// The most bytes of a git answer one read keeps: the first-parent walk
    /// and the release notes' blob. A repository somebody else wrote may hold
    /// either at any size.
    pub(crate) walk_bytes_max: usize,
    pub(crate) notes_bytes_max: usize,
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
    git_retry_ms: 20_000,
    // 240 first-parent commits are about 60 KiB on this repository.
    walk_bytes_max: 2 * 1024 * 1024,
    // This repository's CHANGELOG is 0.5 MiB after a month.
    notes_bytes_max: 4 * 1024 * 1024,
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

/// Whether git was asked for a flow (t-69070 P2): only for a folder in a
/// repository this window lists ([`vouch`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GitAsked {
    /// The folder is in a listed repository, and git was asked at the root the
    /// window listed for it.
    #[default]
    Kept,
    /// The folder is in no listed repository: git was not run, and the flow is
    /// the ledger's and the catalog's alone.
    Outside,
    /// git did not answer in time and the flow stands on the last answer it
    /// gave while it is read again (t-69107 ③).
    Stale,
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
    /// Open work nobody is coming back to (t-71220, [`left_behind`]): what a
    /// 「정리」 closes, and what stays open for a look. Not 「지금」.
    pub(crate) left: FlowPile,
    /// Finished work no review stands for, and nobody is coming back to
    /// write one: what a 「검토 생략」 waives. Not 「지금」.
    pub(crate) unreviewed: FlowPile,
    /// How many tasks one press may name ([`PRESS_TASKS_MAX`]): a longer
    /// pile is cleared that many at a time, oldest first.
    pub(crate) press_max: usize,
    /// The quiet time the piles are read with ([`SETTLE_QUIET_DAYS`]), in days.
    pub(crate) quiet_days: i64,
    /// Whether this window's ledger answered. Without it the flow is git's alone.
    pub(crate) ledger: bool,
    /// Whether git was asked — only in a repository this window lists.
    pub(crate) git: GitAsked,
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

/// A pile of work nobody is coming back to (t-71220): its lines, oldest first
/// and at most [`FlowTable::group_tasks_max`], how many more, and every task a
/// press over it names — all of a finished pile's, and the ones a settle
/// closes of an open one ([`Left::Settle`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct FlowPile {
    pub(crate) tasks: Vec<FlowTask>,
    pub(crate) more: usize,
    pub(crate) named: Vec<Named>,
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
    /// The pile it stands in when nobody is coming back to it (t-71220);
    /// `None` on every line of 「지금」 and of the groups after it.
    pub(crate) left: Option<Left>,
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

/// The format the walk asks git for: commit, parents, commit time, the tags on
/// it and its subject, a unit separator apart. The subject is last, and the
/// line is split into five at the first four marks, so a subject that holds the
/// mark itself stays a subject (t-69107 ④).
const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%ct%x1f%D%x1f%s";

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
    /// git did not answer in time and this is the last answer it gave — the
    /// repository is being read again.
    pub(crate) stale: bool,
}

/// The walk of the base branch's first-parent line, as git printed it in
/// [`LOG_FORMAT`], newest first: the merges after the newest release, and every
/// release with the merges between it and the one before it.
pub(crate) fn parse_log(text: &str) -> (Vec<GitMerge>, Vec<GitRelease>) {
    let mut unreleased = Vec::new();
    let mut releases: Vec<GitRelease> = Vec::new();
    for line in text.lines() {
        let mut fields = line.splitn(5, '\u{1f}');
        let (Some(commit), Some(parents), Some(at), Some(refs), Some(subject)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let at_ms = at
            .trim()
            .parse::<i64>()
            .map_or(0, |seconds| seconds.saturating_mul(1000));
        if let Some(version) = release_of(refs, subject) {
            releases.push(GitRelease {
                version,
                commit: commit.trim().to_string(),
                at_ms,
                merges: Vec::new(),
            });
            continue;
        }
        let parents: Vec<&str> = parents.split_whitespace().collect();
        let Some(merge) = merge_of(commit.trim(), &parents, at_ms, subject) else {
            continue;
        };
        match releases.last_mut() {
            Some(release) => release.merges.push(merge),
            None => unreleased.push(merge),
        }
    }
    (unreleased, releases)
}

/// The version a first-parent commit releases: a version tag on it, or a
/// subject that says `release: v1.2.3`.
fn release_of(refs: &str, subject: &str) -> Option<String> {
    refs.split(',')
        .filter_map(|name| name.trim().strip_prefix("tag: "))
        .find(|tag| is_version(tag))
        .map(str::to_string)
        .or_else(|| {
            let word = subject
                .trim()
                .strip_prefix("release:")?
                .split_whitespace()
                .next()?;
            is_version(word).then(|| word.to_string())
        })
}

/// `v1.2.3` or `1.2.3`, with anything after the third number's digits.
fn is_version(word: &str) -> bool {
    let mut parts = bare_version(word).splitn(3, '.');
    let digits = |part: Option<&str>| {
        part.is_some_and(|part| {
            part.bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_digit())
        })
    };
    let (major, minor, patch) = (parts.next(), parts.next(), parts.next());
    let whole = |part: Option<&str>| {
        part.is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    whole(major) && whole(minor) && digits(patch)
}

/// A version without its leading `v`: what a tag, the release notes and a
/// coordinator's record are compared by.
fn bare_version(word: &str) -> &str {
    word.strip_prefix('v').unwrap_or(word)
}

/// A merge the project's tasks can be found in: a first-parent commit with two
/// parents or more whose subject reads `Merge <what> at <sha> (…) — <line>`.
fn merge_of(commit: &str, parents: &[&str], at_ms: i64, subject: &str) -> Option<GitMerge> {
    if parents.len() < 2 {
        return None;
    }
    let rest = subject.trim().strip_prefix("Merge ")?;
    let (head_part, line) = match rest.split_once(LINE_MARK) {
        Some((head, line)) => (head, Some(line.trim().to_string())),
        None => (rest, None),
    };
    let head = head_part
        .split_whitespace()
        .skip_while(|word| *word != "at")
        .nth(1)
        .map(|word| word.trim_matches(|glyph: char| !glyph.is_ascii_hexdigit()))
        .and_then(commit_named)
        .or_else(|| parents.get(1).map(|parent| (*parent).to_string()));
    Some(GitMerge {
        commit: commit.to_string(),
        head,
        at_ms,
        line: line.filter(|line| !line.is_empty()),
        tasks: merged_tasks(head_part),
    })
}

/// The tasks a merge subject's head names: the first one in what was merged
/// (`wt/t-123/…`, `t-123`) and every one inside its parentheses. A task id a
/// branch's later words carry (`…/2-zo-t-456-plan`) is a name, not a task.
fn merged_tasks(head_part: &str) -> Vec<String> {
    let mut tasks: Vec<String> = head_part
        .split_whitespace()
        .next()
        .map(task_ids)
        .unwrap_or_default()
        .into_iter()
        .take(1)
        .collect();
    let mut rest = head_part;
    while let Some(open) = rest.find('(') {
        let inside = &rest[open + 1..];
        let close = inside.find(')').unwrap_or(inside.len());
        for task in task_ids(&inside[..close]) {
            if !tasks.contains(&task) {
                tasks.push(task);
            }
        }
        rest = &inside[close..];
    }
    tasks
}

/// Every task id (`t-` and digits, standing as a word of its own) a text names,
/// in order, each once.
pub(crate) fn task_ids(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut found: Vec<String> = Vec::new();
    let mut at = 0;
    while let Some(offset) = text[at..].find("t-") {
        let start = at + offset;
        let digits_from = start + 2;
        let digits = bytes[digits_from..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        let end = digits_from + digits;
        let alone_before = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let alone_after = bytes
            .get(end)
            .is_none_or(|byte| !byte.is_ascii_alphanumeric());
        if digits > 0 && alone_before && alone_after {
            let id = text[start..end].to_string();
            if !found.contains(&id) {
                found.push(id);
            }
        }
        at = digits_from;
    }
    found
}

/// The release notes' line for each task they name, over the newest `releases`
/// sections: under `## [1.2.3] — date`, a bullet `- <line> — <more> (t-1, t-2)`;
/// the line is the bullet's words before its first 「 — 」, and the newest
/// section that names a task keeps it.
pub(crate) fn parse_notes(text: &str, releases: usize) -> HashMap<String, Note> {
    let mut notes: HashMap<String, Note> = HashMap::new();
    let mut version: Option<String> = None;
    let mut sections = 0;
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("## [") {
            sections += 1;
            if sections > releases {
                break;
            }
            version = head
                .split(']')
                .next()
                .map(|word| bare_version(word.trim()).to_string());
            continue;
        }
        let (Some(version), Some(bullet)) = (version.as_deref(), line.strip_prefix("- ")) else {
            continue;
        };
        let Some((words, named)) = bullet.trim_end().rsplit_once('(') else {
            continue;
        };
        let said = words.split(LINE_MARK).next().unwrap_or(words).trim();
        if said.is_empty() {
            continue;
        }
        for task in task_ids(named) {
            notes.entry(task).or_insert_with(|| Note {
                version: version.to_string(),
                line: said.to_string(),
            });
        }
    }
    notes
}

/// What git says, kept for one repository: the base's commit it was read at,
/// and when that commit was last asked.
struct GitHeld {
    root: PathBuf,
    oid: String,
    checked_ms: i64,
    /// The last answer git gave, as it gave it.
    facts: Arc<GitFacts>,
    /// What a read is handed: `facts`, or while git is failing a copy of them
    /// that says so ([`GitFacts::stale`]).
    shown: Arc<GitFacts>,
    /// The last ask failed: git is asked again after [`FlowTable::git_retry_ms`].
    failed: bool,
}

/// How many repositories' answers are kept. A window looks at a few projects;
/// past this the one used longest ago is asked again.
const GIT_HELD_MAX: usize = 8;

fn git_held() -> &'static Mutex<Vec<GitHeld>> {
    static HELD: OnceLock<Mutex<Vec<GitHeld>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Vec::new()))
}

/// The branch the flow walks and the commit it is at, asked of git once: the
/// remote's default branch as its own local branch when there is one — merges
/// land on it before they are pushed — else the remote's; without a remote
/// default, the first of the usual names the repository has, in the order a
/// workspace's base is probed ([`crate::worktree_runtime::default_base_probe`]).
/// One `for-each-ref` instead of a probe, a check and a resolve: every git
/// process costs its start, about 14 ms on a busy machine (t-68458).
fn base_of(host: &Host, root: &Path) -> Option<(String, String)> {
    use crate::worktree_runtime::{DEFAULT_BASE_NAMES, ORIGIN_HEAD_REF};
    let heads: Vec<String> = DEFAULT_BASE_NAMES
        .iter()
        .map(|name| format!("refs/heads/{name}"))
        .collect();
    let mut args = vec![
        "for-each-ref",
        "--format=%(refname)%09%(objectname)%09%(symref)",
        ORIGIN_HEAD_REF,
    ];
    args.extend(heads.iter().map(String::as_str));
    let said = host.vcs().text_within(root, &args, FLOW.git_budget)?;
    let refs: HashMap<&str, (&str, &str)> = said
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some((
                fields.next()?,
                (fields.next()?, fields.next().unwrap_or_default()),
            ))
        })
        .collect();
    let local = |name: &str| {
        refs.get(format!("refs/heads/{name}").as_str())
            .map(|(oid, _)| (name.to_string(), (*oid).to_string()))
    };
    if let Some((oid, target)) = refs.get(ORIGIN_HEAD_REF)
        && let Some(name) = target.strip_prefix("refs/remotes/origin/")
    {
        return local(name).or_else(|| Some((format!("origin/{name}"), (*oid).to_string())));
    }
    DEFAULT_BASE_NAMES.iter().find_map(|name| local(name))
}

/// The base's first-parent walk from `at` (a commit or a branch), as git prints
/// it in [`LOG_FORMAT`].
///
/// `None` when git gave no answer — it failed, ran out of time or answered more
/// than [`FlowTable::walk_bytes_max`]. The revision is ended by `--`, so a file
/// in the work tree named like the branch does not make it ambiguous (t-69107 ⑤).
fn walk_at(root: &Path, at: &str) -> Option<String> {
    Host::for_workspace(root).vcs().text_within_max(
        root,
        &[
            "log",
            "--first-parent",
            "--no-color",
            LOG_FORMAT,
            "--decorate-refs=refs/tags/",
            &format!("--max-count={}", FLOW.commits_walked),
            at,
            "--",
        ],
        FLOW.git_budget,
        FLOW.walk_bytes_max,
    )
}

/// The release notes' line for each task, read at `at` (a commit or a branch).
/// Notes past [`FlowTable::notes_bytes_max`] are not read: the lines the flow
/// takes are the newest, and a file that size is not a changelog.
fn notes_at(root: &Path, at: &str) -> HashMap<String, Note> {
    Host::for_workspace(root)
        .vcs()
        .text_within_max(
            root,
            &["cat-file", "blob", &format!("{at}:{}", FLOW.notes_file)],
            FLOW.git_budget,
            FLOW.notes_bytes_max,
        )
        .map(|text| parse_notes(&text, FLOW.releases_kept + 1))
        .unwrap_or_default()
}

/// The walk and the notes at `at`: two questions git answers apart, so they are
/// asked at once, the notes on a thread of their own.
fn walk_and_notes(root: &Path, at: &str) -> (Option<String>, HashMap<String, Note>) {
    std::thread::scope(|scope| {
        let notes = scope.spawn(|| notes_at(root, at));
        let walked = walk_at(root, at);
        (walked, notes.join().unwrap_or_default())
    })
}

/// What git says of the repository at `root`, read again only when its base's
/// commit moved. A repository git cannot answer for has no facts.
///
/// Every git process costs its start — about 14 ms on a busy machine — so a read
/// asks side by side what it can (t-68458). The first read of a repository walks
/// from the usual base while git says which base it is, and keeps that walk
/// when git names that base at the commit the walk began from; a later read asks
/// only where the base stands, and walks again only when it moved.
pub(crate) fn git_facts(root: &Path, now_ms: i64) -> Arc<GitFacts> {
    git_facts_in(git_held(), GIT_HELD_MAX, root, now_ms)
}

/// [`git_facts`] over the answers `held` keeps, at most `most` of them — the
/// last used kept, the one used longest ago let go.
fn git_facts_in(
    held: &Mutex<Vec<GitHeld>>,
    most: usize,
    root: &Path,
    now_ms: i64,
) -> Arc<GitFacts> {
    let known = {
        let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = held.iter().position(|one| {
            let stands = if one.failed {
                FLOW.git_retry_ms
            } else {
                FLOW.git_fresh_ms
            };
            one.root == root && now_ms - one.checked_ms < stands
        }) {
            let one = held.remove(at);
            let shown = Arc::clone(&one.shown);
            held.push(one);
            return shown;
        }
        held.iter().any(|one| one.root == root)
    };
    let host = Host::for_workspace(root);
    let guess = crate::worktree_runtime::DEFAULT_BASE_NAMES[0];
    let (based, ahead) = if known {
        (base_of(&host, root), None)
    } else {
        std::thread::scope(|scope| {
            let ahead = scope.spawn(|| walk_and_notes(root, guess));
            let based = base_of(&host, root);
            (based, ahead.join().ok())
        })
    };
    let Some((base, oid)) = based else {
        return git_failed(held, most, root, now_ms);
    };
    {
        let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = held
            .iter()
            .position(|one| one.root == root && one.oid == oid)
        {
            let mut one = held.remove(at);
            one.checked_ms = now_ms;
            one.failed = false;
            one.shown = Arc::clone(&one.facts);
            let shown = Arc::clone(&one.shown);
            held.push(one);
            return shown;
        }
    }
    // The walk read ahead stands when git named its base at the commit it began
    // from; otherwise the base is walked where git says it is.
    let (walked, notes) = match ahead {
        Some((Some(walked), notes)) if base == guess && walked.starts_with(oid.as_str()) => {
            (Some(walked), notes)
        }
        _ => walk_and_notes(root, &oid),
    };
    let Some(walked) = walked else {
        return git_failed(held, most, root, now_ms);
    };
    let (unreleased, releases) = parse_log(&walked);
    let facts = Arc::new(GitFacts {
        base: Some(base),
        unreleased,
        releases,
        notes,
        stale: false,
    });
    let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
    held.retain(|one| one.root != root);
    if held.len() >= most {
        held.remove(0);
    }
    held.push(GitHeld {
        root: root.to_path_buf(),
        oid,
        checked_ms: now_ms,
        facts: Arc::clone(&facts),
        shown: Arc::clone(&facts),
        failed: false,
    });
    facts
}

/// git did not answer for `root`: a failure is remembered — it is not asked
/// again for [`FlowTable::git_retry_ms`] — and the last answer it gave stands,
/// saying it is [`GitFacts::stale`]. A repository git never answered for has
/// no facts, and that is remembered the same way.
fn git_failed(held: &Mutex<Vec<GitHeld>>, most: usize, root: &Path, now_ms: i64) -> Arc<GitFacts> {
    let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
    let mut one = match held.iter().position(|one| one.root == root) {
        Some(at) => held.remove(at),
        None => GitHeld {
            root: root.to_path_buf(),
            oid: String::new(),
            checked_ms: now_ms,
            facts: Arc::default(),
            shown: Arc::default(),
            failed: true,
        },
    };
    one.checked_ms = now_ms;
    one.failed = true;
    if one.facts.base.is_some() {
        one.shown = Arc::new(GitFacts {
            stale: true,
            ..(*one.facts).clone()
        });
    }
    let shown = Arc::clone(&one.shown);
    if held.len() >= most {
        held.remove(0);
    }
    held.push(one);
    shown
}

/* ---- words a person reads ---------------------------------------------- */

/// `text` cut to `chars` characters at a word where one is near, with 「…」.
fn bounded(text: &str, chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(chars).collect();
    let at_word = cut
        .rfind(char::is_whitespace)
        .filter(|at| cut[..*at].chars().count() * 2 >= chars)
        .map_or(cut.as_str(), |at| &cut[..at]);
    format!("{}…", at_word.trim_end())
}

/// A hand-in's words without the head a worker's summary often opens with —
/// `t-123`, `at abc1234`, a parenthesis and a colon.
fn without_task_head(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("t-") else {
        return text;
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return text;
    }
    let mut rest = rest[digits..].trim_start();
    if let Some(after) = rest.strip_prefix("at ") {
        let hex = after.bytes().take_while(u8::is_ascii_hexdigit).count();
        rest = after[hex..].trim_start();
    }
    if rest.starts_with('(')
        && let Some(close) = rest.find(')')
    {
        rest = rest[close + 1..].trim_start();
    }
    let rest = rest.trim_start_matches([':', '—', '–', '-']).trim_start();
    if rest.is_empty() { text } else { rest }
}

/// The first sentence of words a person reads, bounded: without a task head,
/// up to its first sentence end — a full stop, 「。」, 「!」 or 「?」 before a
/// space, a semicolon, or a new line.
pub(crate) fn first_sentence(text: &str, chars: usize) -> Option<String> {
    let text = without_task_head(text.trim());
    let mut end = text.len();
    let mut glyphs = text.char_indices().peekable();
    while let Some((at, glyph)) = glyphs.next() {
        let next = glyphs.peek().map(|(_, next)| *next);
        let ends = match glyph {
            '\n' => true,
            '.' | '。' | '!' | '?' | ';' => next.is_none_or(char::is_whitespace),
            _ => false,
        };
        if ends {
            end = at;
            break;
        }
    }
    let sentence = text[..end].trim();
    (!sentence.is_empty()).then(|| bounded(sentence, chars))
}

/// Where a clause around a 「→」 stops.
const CLAUSE_ENDS: [char; 5] = [',', ';', '(', ')', '·'];

/// The first 「A → B」 a text says, with up to two words before it and two
/// after — 「검색 42 → 38 ms」.
fn arrow_clause(text: &str, chars: usize) -> Option<String> {
    let (at, mark) = ["→", "->"]
        .iter()
        .filter_map(|mark| text.find(mark).map(|at| (at, *mark)))
        .min_by_key(|(at, _)| *at)?;
    let before = &text[..at];
    let after = &text[at + mark.len()..];
    let start = before
        .char_indices()
        .rev()
        .find(|(_, glyph)| CLAUSE_ENDS.contains(glyph))
        .map_or(0, |(at, glyph)| at + glyph.len_utf8());
    let left: Vec<&str> = before[start..].split_whitespace().collect();
    let left = left[left.len().saturating_sub(2)..].join(" ");
    let end = after.find(CLAUSE_ENDS).unwrap_or(after.len());
    let right = after[..end]
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let right = right.trim_end_matches(['.', '。']);
    (!left.is_empty() && !right.is_empty()).then(|| bounded(&format!("{left} → {right}"), chars))
}

/// A line's before → after: the hand-in's own `numbers` when it sent them (a
/// sentence, or a table of sentences), else the first 「A → B」 its summary says.
pub(crate) fn gain_of(
    numbers: Option<&Value>,
    summary: Option<&str>,
    chars: usize,
) -> Option<String> {
    let said = match numbers {
        Some(Value::String(text)) => arrow_clause(text, chars),
        Some(Value::Object(table)) => table
            .values()
            .filter_map(Value::as_str)
            .find_map(|text| arrow_clause(text, chars)),
        _ => None,
    };
    said.or_else(|| summary.and_then(|text| arrow_clause(text, chars)))
}

/// The labels of a report's three lines, Korean and English.
const THREE_LABELS: [(&str, &str, usize); 3] =
    [("무엇", "What", 0), ("왜", "Why", 1), ("대가", "Cost", 2)];

/// What, why and cost — a report's three lines as the plain-report rule writes
/// them (`- **무엇**: …`), read off the excerpt the catalog keeps. All three, or
/// none.
pub(crate) fn three_of(text: &str, chars: usize) -> Option<Three> {
    let mut said: [Option<String>; 3] = [None, None, None];
    for line in text.lines() {
        let Some(rest) = line
            .trim_start()
            .strip_prefix("- **")
            .or_else(|| line.trim_start().strip_prefix("* **"))
        else {
            continue;
        };
        let Some((label, words)) = rest.split_once("**") else {
            continue;
        };
        let Some(&(_, _, slot)) = THREE_LABELS.iter().find(|(korean, english, _)| {
            label.trim() == *korean || label.trim().eq_ignore_ascii_case(english)
        }) else {
            continue;
        };
        let words = words.trim_start_matches([':', ' ', '—']).trim();
        if !words.is_empty() && said[slot].is_none() {
            said[slot] = Some(bounded(words, chars));
        }
    }
    let [Some(what), Some(why), Some(cost)] = said else {
        return None;
    };
    Some(Three { what, why, cost })
}

/// The release a coordinator wrote down for a task — the first version word of
/// its `release`, `shippedIn` or `deployedAs` — read only from a result the
/// run's coordinator seat wrote, as every review key is.
pub(crate) fn ledger_release(task: &Task) -> Option<String> {
    if !matches!(task.result_author, Some(ResultAuthor::Coordinator { .. })) {
        return None;
    }
    let result: Value = serde_json::from_str(task.result.as_str()).ok()?;
    ["release", "shippedIn", "deployedAs"]
        .iter()
        .filter_map(|key| result.get(*key).and_then(Value::as_str))
        .flat_map(str::split_whitespace)
        .map(|word| word.trim_matches(|glyph: char| matches!(glyph, ',' | '(' | ')' | ';')))
        .find(|word| is_version(word))
        .map(str::to_string)
}

/* ---- the ledger --------------------------------------------------------- */

/// One run's attempts by task and the last report of each attempt, indexed in
/// one pass.
struct RunIndex<'a> {
    run: &'a Run,
    attempts: HashMap<&'a str, Vec<&'a Dispatch>>,
    reports: HashMap<&'a str, &'a Message>,
}

impl<'a> RunIndex<'a> {
    fn of(run: &'a Run) -> Self {
        let mut attempts: HashMap<&str, Vec<&Dispatch>> = HashMap::new();
        for dispatch in &run.dispatches {
            attempts
                .entry(dispatch.task.as_str())
                .or_default()
                .push(dispatch);
        }
        for held in attempts.values_mut() {
            held.sort_by_key(|dispatch| dispatch.started_ms);
        }
        let mut reports: HashMap<&str, &Message> = HashMap::new();
        for message in run.messages() {
            if message.kind != MessageKind::WorkerDone {
                continue;
            }
            if let Some(dispatch) = message.dispatch.as_deref() {
                reports.insert(dispatch, message);
            }
        }
        Self {
            run,
            attempts,
            reports,
        }
    }

    fn attempts_of(&self, task: &str) -> &[&'a Dispatch] {
        self.attempts
            .get(task)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

/// A task's newest moment: its writing, and its attempts' starts and ends.
fn last_moment(task: &Task, attempts: &[&Dispatch]) -> i64 {
    attempts
        .iter()
        .flat_map(|attempt| [Some(attempt.started_ms), attempt.ended_ms])
        .flatten()
        .chain([task.created_ms])
        .max()
        .unwrap_or(task.created_ms)
}

/// A task's own words for a roster: its title, else its spec's first line.
fn title_of(task: &Task, chars: usize) -> String {
    let title = task.title.as_str().trim();
    let title = if title.is_empty() {
        task.spec
            .as_str()
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
    } else {
        title
    };
    bounded(title, chars)
}

/// One task's line as the ledger says it — before git and the catalog dress it.
fn ledger_line(index: &RunIndex<'_>, task: &Task, table: &FlowTable) -> FlowTask {
    let run = index.run;
    let stage = stage_of(run, task);
    let attempts: Vec<FlowAttempt> = index
        .attempts_of(&task.id)
        .iter()
        .map(|dispatch| {
            let worker = run.worker(&dispatch.worker);
            FlowAttempt {
                dispatch: dispatch.id.clone(),
                worker: dispatch.worker.clone(),
                agent: worker.map(|worker| worker.agent.clone()),
                model: worker.and_then(|worker| worker.model.clone()),
                started_ms: dispatch.started_ms,
                ended_ms: dispatch.ended_ms,
                succeeded: dispatch.succeeded,
                source: dispatch.source.clone(),
                said: index
                    .reports
                    .get(dispatch.id.as_str())
                    .and_then(|report| summary_of_body(report.body.as_str()))
                    .and_then(|words| first_sentence(&words, table.said_chars)),
            }
        })
        .collect();
    let after = task_time(run, task).after_report;
    let review = run.review_of(task);
    let summary = worker_summary_in(run, task);
    let numbers: Option<Value> = after
        .attempt
        .as_deref()
        .and_then(|attempt| index.reports.get(attempt))
        .and_then(|report| serde_json::from_str::<Value>(report.body.as_str()).ok())
        .and_then(|body| body.get("numbers").cloned());
    let hand_in = (after.source.is_some() || after.reported_ms.is_some()).then(|| FlowHandIn {
        attempt: after.attempt.clone(),
        source: after.source.clone(),
        at_ms: after.reported_ms,
    });
    let said = summary
        .as_deref()
        .and_then(|words| first_sentence(words, table.said_chars));
    FlowTask {
        task: task.id.clone(),
        run: Some(run.id.clone()),
        title: title_of(task, table.title_chars),
        said_from: if said.is_some() {
            Said::Summary
        } else {
            Said::Title
        },
        said: said.unwrap_or_else(|| title_of(task, table.said_chars)),
        gain: gain_of(numbers.as_ref(), summary.as_deref(), table.gain_chars),
        stage,
        ledger_stage: Some(stage),
        left: None,
        three: None,
        summary: summary
            .as_deref()
            .map(|words| bounded(without_task_head(words), table.summary_chars)),
        opened_ms: Some(task.created_ms),
        last_ms: last_moment(task, index.attempts_of(&task.id)),
        attempts,
        hand_in,
        review: review.verified.then(|| FlowReview {
            at_ms: review.verified_ms,
            source: review.source.clone(),
        }),
        land: review.merged.then(|| FlowLand {
            at_ms: None,
            commit: review.merge_head.clone(),
            line: None,
            told: Told::Ledger,
        }),
        ship: ledger_release(task)
            .filter(|_| review.deployed)
            .map(|version| FlowShip {
                version,
                at_ms: None,
                told: Told::Ledger,
            }),
        chips: Chips::default(),
    }
}

/// A line for a task only git names: what landed and shipped, in git's words.
fn git_line(task: &str, merge: &GitMerge, table: &FlowTable) -> FlowTask {
    let said = merge
        .line
        .as_deref()
        .map(|line| bounded(line, table.said_chars));
    FlowTask {
        task: task.to_string(),
        run: None,
        title: said.clone().unwrap_or_else(|| task.to_string()),
        said_from: if said.is_some() {
            Said::Merge
        } else {
            Said::Title
        },
        said: said.unwrap_or_else(|| task.to_string()),
        gain: None,
        stage: STAGE_MERGED,
        ledger_stage: None,
        left: None,
        three: None,
        summary: None,
        opened_ms: None,
        last_ms: merge.at_ms,
        attempts: Vec::new(),
        hand_in: None,
        review: None,
        land: None,
        ship: None,
        chips: Chips::default(),
    }
}

/* ---- the flow ----------------------------------------------------------- */

/// The desk's words this file places lines by.
const STAGE_MERGED: &str = "merged";
const STAGE_DEPLOYED: &str = "deployed";
/// Folded into another task, handed to another run or outdated: over without
/// being work anybody did, so it has no line.
const STAGE_CLOSED: &str = "closed";

/// The stages of work not on the base yet: their lines stand in 「지금」. The
/// other endings — failed, nothing to review, nothing to land, unreadable —
/// stand with the time they ended in.
const NOW_STAGES: [&str; 8] = [
    "pending",
    "ready",
    "dispatched",
    "gate",
    "blocked",
    "reported",
    "stale",
    "verified",
];

/// Where a line stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Now,
    /// Left behind, open ([`Flow::left`]) or finished ([`Flow::unreviewed`]).
    Left,
    Unreviewed,
    Next,
    Release(usize),
    /// Shipped in a release past the kept ones.
    Older,
}

/// The base branch's merges, by the tasks they name — the newest merge of a task
/// first — with the release each is in (`None`: not released yet).
fn merges_by_task(git: &GitFacts) -> HashMap<&str, (Option<usize>, &GitMerge)> {
    let mut placed: HashMap<&str, (Option<usize>, &GitMerge)> = HashMap::new();
    let released = git
        .releases
        .iter()
        .enumerate()
        .flat_map(|(at, release)| release.merges.iter().map(move |merge| (Some(at), merge)));
    for (release, merge) in git
        .unreleased
        .iter()
        .map(|merge| (None, merge))
        .chain(released)
    {
        for task in &merge.tasks {
            placed.entry(task.as_str()).or_insert((release, merge));
        }
    }
    placed
}

/// The merge a coordinator's `mergeHead` names, when no subject names the task.
fn merge_named<'a>(git: &'a GitFacts, head: &str) -> Option<(Option<usize>, &'a GitMerge)> {
    let head = commit_named(head)?;
    let named =
        |merge: &GitMerge| merge.commit.starts_with(&head) || head.starts_with(&merge.commit);
    git.unreleased
        .iter()
        .find(|merge| named(merge))
        .map(|merge| (None, merge))
        .or_else(|| {
            git.releases.iter().enumerate().find_map(|(at, release)| {
                release
                    .merges
                    .iter()
                    .find(|merge| named(merge))
                    .map(|merge| (Some(at), merge))
            })
        })
}

/// Lay what git says over a line: the merge that landed it and the release it
/// shipped in, moving its stage on — a merge on the base is landed work, and a
/// release after it shipped it.
fn dress_with_git(
    line: &mut FlowTask,
    git: &GitFacts,
    release: Option<usize>,
    merge: &GitMerge,
    table: &FlowTable,
) {
    line.land = Some(FlowLand {
        at_ms: Some(merge.at_ms),
        commit: Some(merge.commit.clone()),
        line: merge.line.clone(),
        told: Told::Git,
    });
    match release.and_then(|at| git.releases.get(at)) {
        Some(shipped) => {
            line.ship = Some(FlowShip {
                version: shipped.version.clone(),
                at_ms: Some(shipped.at_ms),
                told: Told::Git,
            });
            line.stage = STAGE_DEPLOYED;
        }
        None => {
            if line.stage != STAGE_DEPLOYED {
                line.stage = STAGE_MERGED;
            }
        }
    }
    if line.said_from > Said::Merge
        && let Some(words) = &merge.line
    {
        line.said = bounded(words, table.said_chars);
        line.said_from = Said::Merge;
    }
}

/// The release a moment falls under: the oldest release at it or after it.
/// `None` for a moment after the newest release.
fn release_after(git: &GitFacts, at_ms: i64) -> Option<usize> {
    git.releases
        .iter()
        .rposition(|release| release.at_ms >= at_ms)
}

/// A release past the kept ones stands in the one folded line.
const fn kept(at: usize, table: &FlowTable) -> Place {
    if at < table.releases_kept {
        Place::Release(at)
    } else {
        Place::Older
    }
}

/// Where a line stands, once git has dressed it. A closed task has no line.
fn place_of(line: &FlowTask, git: &GitFacts, table: &FlowTable) -> Option<Place> {
    if line.stage == STAGE_CLOSED {
        return None;
    }
    // Work nobody is coming back to stands in its pile — until git says it
    // landed, which moves it on like any other line.
    if NOW_STAGES.contains(&line.stage) {
        match line.left {
            Some(Left::Unreviewed) => return Some(Place::Unreviewed),
            Some(Left::Settle | Left::Look) => return Some(Place::Left),
            None => {}
        }
    }
    if let Some(ship) = &line.ship {
        // A release the walk did not reach is older than every one it did.
        return Some(
            git.releases
                .iter()
                .position(|release| bare_version(&release.version) == bare_version(&ship.version))
                .map_or(Place::Older, |at| kept(at, table)),
        );
    }
    if NOW_STAGES.contains(&line.stage) {
        return Some(Place::Now);
    }
    // Shipped by the ledger's word alone, and landed or ended work, stand with
    // the release their time falls under — or before the next one.
    let when = line
        .land
        .as_ref()
        .and_then(|land| land.at_ms)
        .unwrap_or(line.last_ms);
    match release_after(git, when) {
        Some(at) if line.stage == STAGE_DEPLOYED => Some(kept(at, table)),
        Some(_) if line.stage == STAGE_MERGED || line.land.is_some() => Some(Place::Next),
        Some(_) => None,
        None => Some(Place::Next),
    }
}

/* ---- the schedule's landings (t-71840) ---------------------------------- */

/// When one task's work landed on the base and when it shipped, as git says
/// it: the merge's commit time and the release's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
pub(crate) struct Landed {
    pub(crate) landed_ms: Option<i64>,
    pub(crate) released_ms: Option<i64>,
}

/// One task as the board's desk carries it: what its landing is found by.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LandingAsk<'a> {
    pub(crate) task: &'a str,
    /// The commit the coordinator wrote the work landed as (`mergeHead`).
    pub(crate) merge_head: Option<&'a str>,
    /// The release the coordinator wrote it shipped in ([`ledger_release`]).
    pub(crate) release: Option<&'a str>,
}

/// When each task landed and shipped, as git says it — found the way the flow
/// finds it ([`merges_by_task`], [`merge_named`]): the newest merge on the
/// base's first-parent line that names the task, else the merge its `mergeHead`
/// names, and the release that merge falls under; with no merge, the release
/// the coordinator wrote it shipped in, when the walk holds that release. A task
/// git says nothing of is left out, and the board's schedule ends its bar on the
/// ledger's own times instead — never on the present.
pub(crate) fn landings(git: &GitFacts, tasks: &[LandingAsk<'_>]) -> BTreeMap<String, Landed> {
    let by_task = merges_by_task(git);
    let released = |at: Option<usize>| {
        at.and_then(|at| git.releases.get(at))
            .map(|release| release.at_ms)
    };
    let mut landed = BTreeMap::new();
    for ask in tasks {
        let merged = by_task
            .get(ask.task)
            .copied()
            .or_else(|| ask.merge_head.and_then(|head| merge_named(git, head)));
        let found = match merged {
            Some((release, merge)) => Landed {
                landed_ms: Some(merge.at_ms),
                released_ms: released(release),
            },
            None => Landed {
                landed_ms: None,
                released_ms: ask.release.and_then(|version| {
                    git.releases
                        .iter()
                        .find(|release| bare_version(&release.version) == bare_version(version))
                        .map(|release| release.at_ms)
                }),
            },
        };
        if found != Landed::default() {
            landed.insert(ask.task.to_string(), found);
        }
    }
    landed
}

/// What a flow is read from.
pub(crate) struct Reading<'a> {
    pub(crate) root: &'a str,
    /// The project as the ledger keys a run ([`zerocode_core::git_dir::project_root`]).
    pub(crate) project: Option<&'a str>,
    pub(crate) ledger: Option<&'a Ledger>,
    pub(crate) git: &'a GitFacts,
    pub(crate) table: &'a FlowTable,
    /// The moment the read is made: what a task left behind is measured
    /// against ([`left_behind`]).
    pub(crate) now_ms: i64,
}

/// The flow of one project, as the ledger and git say it — before the catalog
/// dresses its lines ([`dress`]).
pub(crate) fn assemble(reading: &Reading<'_>) -> Flow {
    let Reading {
        root,
        project,
        ledger,
        git,
        table,
        now_ms,
    } = *reading;
    let indexes: Vec<RunIndex<'_>> = ledger.map_or_else(Vec::new, |ledger| {
        ledger.runs().iter().map(RunIndex::of).collect()
    });
    let ours = |index: &RunIndex<'_>| {
        project.is_some_and(|project| {
            index
                .run
                .project
                .as_ref()
                .is_some_and(|stated| stated.root == project)
        })
    };
    // Every task this project's runs hold — and those of a run that states no
    // project — by id: a merge on this project's base may carry a task another
    // run of it holds. A run of another project is that project's: its
    // attempts, models and hand-in words are not this flow's to show
    // (t-69107 ①).
    let mut held: HashMap<&str, (usize, &Task)> = HashMap::new();
    for (at, index) in indexes.iter().enumerate() {
        if index.run.project.is_some() && !ours(index) {
            continue;
        }
        for task in &index.run.tasks {
            held.entry(task.id.as_str()).or_insert((at, task));
        }
    }
    let by_task = merges_by_task(git);
    // Work that ended before the release past the kept ones stands in none of
    // them, unless a merge in them names it.
    let horizon_ms = git
        .releases
        .get(table.releases_kept)
        .map(|release| release.at_ms);
    let mut lines: Vec<(Place, FlowTask)> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for index in indexes.iter().filter(|index| ours(index)) {
        // The lines in view are read first, each with its stage, and finished
        // work waits on a review by the stage its line read — no stage is read
        // twice (t-71220). Work out of view never stood in 「지금」: of it, only
        // an open task joins a pile, which is where a person clears it.
        let read: Vec<(&Task, Option<FlowTask>)> = index
            .run
            .tasks
            .iter()
            .map(|task| {
                let recent = horizon_ms.is_none_or(|horizon| {
                    last_moment(task, index.attempts_of(&task.id)) >= horizon
                });
                let in_view = recent
                    || by_task.contains_key(task.id.as_str())
                    || task.status == TaskStatus::Dispatched;
                (task, in_view.then(|| ledger_line(index, task, table)))
            })
            .collect();
        let awaiting: HashSet<&str> = read
            .iter()
            .filter(|(_, line)| {
                line.as_ref()
                    .and_then(|line| line.ledger_stage)
                    .is_some_and(word_awaits_review)
            })
            .map(|(task, _)| task.id.as_str())
            .collect();
        let left = left_behind_by(index.run, now_ms, |task| {
            awaiting.contains(task.id.as_str())
        });
        for (task, line) in read {
            let behind = left.get(task.id.as_str()).copied();
            let Some(mut line) = line.or_else(|| behind.map(|_| ledger_line(index, task, table)))
            else {
                continue;
            };
            line.left = behind;
            let named = by_task.get(task.id.as_str()).copied();
            let merged = named.or_else(|| {
                line.land
                    .as_ref()
                    .and_then(|land| land.commit.as_deref())
                    .and_then(|head| merge_named(git, head))
            });
            if let Some((release, merge)) = merged {
                dress_with_git(&mut line, git, release, merge, table);
            }
            seen.insert(task.id.as_str());
            if let Some(place) = place_of(&line, git, table) {
                if !matches!(place, Place::Left | Place::Unreviewed) {
                    line.left = None;
                }
                lines.push((place, line));
            }
        }
    }
    // The tasks the base's merges name that no run of this project holds, in
    // the order the walk met them.
    let walked = git
        .unreleased
        .iter()
        .chain(git.releases.iter().flat_map(|release| &release.merges))
        .flat_map(|merge| &merge.tasks);
    for task in walked {
        let task = task.as_str();
        if !seen.insert(task) {
            continue;
        }
        let Some(&(release, merge)) = by_task.get(task) else {
            continue;
        };
        let mut line = match held.get(task) {
            Some(&(at, held_task)) => ledger_line(&indexes[at], held_task, table),
            None => git_line(task, merge, table),
        };
        dress_with_git(&mut line, git, release, merge, table);
        if let Some(place) = place_of(&line, git, table) {
            lines.push((place, line));
        }
    }
    for (_, line) in &mut lines {
        if line.said_from > Said::Notes
            && let Some(note) = git.notes.get(&line.task)
        {
            line.said = bounded(&note.line, table.said_chars);
            line.said_from = Said::Notes;
        }
    }
    let mut flow = Flow {
        root: root.to_string(),
        base: git.base.clone(),
        ledger: ledger.is_some(),
        releases: git
            .releases
            .iter()
            .take(table.releases_kept)
            .map(|release| FlowRelease {
                version: release.version.clone(),
                commit: release.commit.clone(),
                at_ms: release.at_ms,
                group: FlowGroup::default(),
            })
            .collect(),
        press_max: PRESS_TASKS_MAX,
        quiet_days: SETTLE_QUIET_DAYS,
        ..Flow::default()
    };
    let mut older_tasks = 0;
    for (place, line) in lines {
        match place {
            Place::Now => flow.now.tasks.push(line),
            Place::Left => flow.left.tasks.push(line),
            Place::Unreviewed => flow.unreviewed.tasks.push(line),
            Place::Next => flow.next.tasks.push(line),
            Place::Release(at) => flow.releases[at].group.tasks.push(line),
            Place::Older => older_tasks += 1,
        }
    }
    // Newest first — by the moment it landed where it landed — and by its id
    // where two moments are one, so the order never depends on the reading.
    let landed = |line: &FlowTask| {
        line.land
            .as_ref()
            .and_then(|land| land.at_ms)
            .unwrap_or(line.last_ms)
    };
    let newest_first =
        |a: &FlowTask, b: &FlowTask| landed(b).cmp(&landed(a)).then_with(|| a.task.cmp(&b.task));
    flow.now
        .tasks
        .sort_by(|a, b| b.last_ms.cmp(&a.last_ms).then_with(|| a.task.cmp(&b.task)));
    flow.next.tasks.sort_by(newest_first);
    for release in &mut flow.releases {
        release.group.tasks.sort_by(newest_first);
    }
    for group in std::iter::once(&mut flow.now)
        .chain(std::iter::once(&mut flow.next))
        .chain(flow.releases.iter_mut().map(|release| &mut release.group))
    {
        group.more = group.tasks.len().saturating_sub(table.group_tasks_max);
        group.tasks.truncate(table.group_tasks_max);
    }
    pile_up(&mut flow.left, Some(Left::Settle), table);
    pile_up(&mut flow.unreviewed, None, table);
    let past = &git.releases[table.releases_kept.min(git.releases.len())..];
    if let (Some(newest), Some(oldest)) = (past.first(), past.last()) {
        flow.older = Some(FlowOlder {
            releases: past.len(),
            newest: newest.version.clone(),
            oldest: oldest.version.clone(),
            tasks: older_tasks,
            until_ms: oldest.at_ms,
        });
    }
    flow.sources = sources_of(&flow);
    flow
}

/// A pile in its order — oldest first, the work that has waited longest on
/// top — with every task a press over it names (`closes`: only those a settle
/// closes; `None`: all of them), and its lines bounded like a group's.
fn pile_up(pile: &mut FlowPile, closes: Option<Left>, table: &FlowTable) {
    pile.tasks
        .sort_by(|a, b| a.last_ms.cmp(&b.last_ms).then_with(|| a.task.cmp(&b.task)));
    pile.named = pile
        .tasks
        .iter()
        .filter(|line| closes.is_none_or(|closes| line.left == Some(closes)))
        .filter_map(|line| {
            Some(Named {
                run: line.run.clone()?,
                task: line.task.clone(),
                was: None,
            })
        })
        .collect();
    pile.more = pile.tasks.len().saturating_sub(table.group_tasks_max);
    pile.tasks.truncate(table.group_tasks_max);
}

/// Every line of a flow.
fn lines_of(flow: &Flow) -> impl Iterator<Item = &FlowTask> {
    flow.now.tasks.iter().chain(&flow.next.tasks).chain(
        flow.releases
            .iter()
            .flat_map(|release| &release.group.tasks),
    )
}

fn sources_of(flow: &Flow) -> FlowSources {
    let mut sources = FlowSources::default();
    for line in lines_of(flow) {
        *sources.said.entry(line.said_from).or_default() += 1;
        if let Some(land) = &line.land {
            *sources.land.entry(land.told).or_default() += 1;
        }
        if let Some(ship) = &line.ship {
            *sources.ship.entry(ship.told).or_default() += 1;
        }
    }
    sources
}

impl Flow {
    /// The tasks its lines are about — what the catalog is asked the chips of.
    pub(crate) fn task_ids(&self) -> BTreeSet<String> {
        lines_of(self).map(|line| line.task.clone()).collect()
    }

    /// The earliest moment a 「기록」 can stand in: the start of the oldest kept
    /// release's time, or any time at all when no release bounds it.
    pub(crate) fn records_since(&self, git: &GitFacts) -> i64 {
        git.releases
            .get(self.releases.len())
            .map_or(i64::MIN, |release| release.at_ms)
    }
}

/// Lay the catalog over a flow: each line's chips and its report's three lines,
/// and the rows no task is linked to — each in the group its time falls in,
/// newest first and at most the table's count a group.
pub(crate) fn dress(
    flow: &mut Flow,
    chips: &BTreeMap<String, Chips>,
    records: Vec<Artifact>,
    table: &FlowTable,
) {
    let groups = std::iter::once(&mut flow.now)
        .chain(std::iter::once(&mut flow.next))
        .chain(flow.releases.iter_mut().map(|release| &mut release.group));
    for group in groups {
        for line in &mut group.tasks {
            let Some(held) = chips.get(&line.task) else {
                continue;
            };
            line.three = held
                .lead_text
                .as_deref()
                .and_then(|text| three_of(text, table.three_chars));
            line.chips = held.clone();
        }
    }
    // A release's time runs from the release before it; the newest release's
    // ends where 「다음 발행 전」 begins.
    let starts: Vec<i64> = flow.releases.iter().map(|release| release.at_ms).collect();
    for record in records {
        let at = record.modified_ms;
        let group = match starts.iter().position(|start| at > *start) {
            Some(0) => &mut flow.next,
            Some(release) => &mut flow.releases[release - 1].group,
            None if starts.is_empty() => &mut flow.next,
            None => match flow.releases.last_mut() {
                Some(last) => &mut last.group,
                None => &mut flow.next,
            },
        };
        if group.records.len() < table.records_max {
            group.records.push(record);
        }
    }
}

/// What one read may touch, vouched against the projects this window lists
/// (t-69070 P2).
///
/// The folders a webview names are a request, never an authority — the way a
/// clicked path is vouched before it becomes a working directory
/// (`set_active_worktree`) or evidence (`worktree_evidence_runtime`). git is
/// asked only at a root the window listed itself, and only when the asked
/// folder belongs to that root's repository; the asked folder is never git's
/// working directory, so git never follows its `.git` or a parent's. A folder
/// in no listed repository is answered by the ledger and the catalog alone,
/// and its flow says so ([`GitAsked::Outside`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Vouched {
    /// The project as the ledger keys a run
    /// ([`zerocode_core::git_dir::project_root`]): the folder made canonical,
    /// then its repository's shared git directory when a checkout holds it.
    pub(crate) project: Option<PathBuf>,
    /// The listed root git is asked at; `None` for a folder in no listed
    /// repository.
    pub(crate) repo: Option<PathBuf>,
    /// The asked roots that are the project's own folders — the project, a
    /// folder inside it, its worktrees — and nothing wider.
    pub(crate) roots: Vec<PathBuf>,
}

/// The folders `ask` names, vouched against the `listed` projects. A listed
/// project answers for the folder when it is a checkout whose shared git
/// directory is the folder's project — its main checkout, a linked worktree or
/// a folder inside either; a listed folder no checkout holds answers for none,
/// git having nothing to say there. A path that is not absolute names no
/// folder. Files are read and no process runs: a canonical path and a `.git`
/// pointer or two a folder.
pub(crate) fn vouch(ask: &FlowAsk, listed: &[PathBuf]) -> Vouched {
    use zerocode_core::git_dir::{common_of, project_root};
    let folder = Path::new(ask.root.trim());
    let project = if folder.is_absolute() {
        project_root(folder)
    } else {
        None
    };
    let Some(key) = project.as_deref() else {
        return Vouched::default();
    };
    let repo = listed
        .iter()
        .find(|root| {
            common_of(root)
                .and_then(|shared| shared.canonicalize().ok())
                .is_some_and(|shared| shared == key)
        })
        .cloned();
    let roots = ask
        .roots
        .iter()
        .map(PathBuf::from)
        .filter(|root| root.is_absolute() && project_root(root).as_deref() == Some(key))
        .collect();
    Vouched {
        project,
        repo,
        roots,
    }
}

/// The last vouch, kept for the next read of the same project: the window asks
/// again on every ledger change, and a vouch reads a canonical path and a
/// `.git` pointer or two a folder — 0.93 ms for this repository's twelve
/// worktrees (t-68458 job a7), where git's kept answer costs 0.011 ms. It
/// stands while the folder, its roots and the list are the same, and for as
/// long as git's answer does. Whatever it says, git is only ever asked at a
/// root the window listed.
struct VouchHeld {
    root: String,
    roots: Vec<String>,
    listed: Vec<PathBuf>,
    checked_ms: i64,
    vouched: Vouched,
}

fn vouch_held() -> &'static Mutex<Option<VouchHeld>> {
    static HELD: OnceLock<Mutex<Option<VouchHeld>>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(None))
}

/// [`vouch`], answered from `held` for the same ask against the same list
/// within [`FlowTable::git_fresh_ms`], and kept there.
fn vouch_kept(
    held: &mut Option<VouchHeld>,
    ask: &FlowAsk,
    listed: &[PathBuf],
    now_ms: i64,
) -> Vouched {
    if let Some(one) = held.as_ref().filter(|one| {
        one.root == ask.root
            && one.roots == ask.roots
            && one.listed == listed
            && now_ms - one.checked_ms < FLOW.git_fresh_ms
    }) {
        return one.vouched.clone();
    }
    let vouched = vouch(ask, listed);
    *held = Some(VouchHeld {
        root: ask.root.clone(),
        roots: ask.roots.clone(),
        listed: listed.to_vec(),
        checked_ms: now_ms,
        vouched: vouched.clone(),
    });
    vouched
}

/// The flow of the project at `ask.root`: the ledger image this window holds,
/// git at the listed root the folder belongs to ([`vouch`]), and the catalog's
/// memory. Runs off the main thread.
pub(crate) fn read(ask: &FlowAsk, listed: &[PathBuf], now_ms: i64) -> Flow {
    let root = ask.root.trim();
    if root.is_empty() {
        return Flow::default();
    }
    let vouched = vouch_kept(
        &mut vouch_held().lock().unwrap_or_else(PoisonError::into_inner),
        ask,
        listed,
        now_ms,
    );
    let project = vouched
        .project
        .as_ref()
        .map(|project| project.to_string_lossy().into_owned());
    let git = vouched
        .repo
        .as_deref()
        .map_or_else(Arc::default, |repo| git_facts(repo, now_ms));
    let ledger = crate::orchestration::ledger_image();
    let mut flow = assemble(&Reading {
        root,
        project: project.as_deref(),
        ledger: ledger.as_deref(),
        git: &git,
        table: &FLOW,
        now_ms,
    });
    if vouched.repo.is_none() {
        flow.git = GitAsked::Outside;
    } else if git.stale {
        flow.git = GitAsked::Stale;
    }
    if let Some(store) = crate::artifact_runtime::store() {
        let chips = store.chips(&flow.task_ids());
        let records = store.loose(
            &vouched.roots,
            flow.records_since(&git),
            FLOW.records_max * (FLOW.releases_kept + 2),
        );
        dress(&mut flow, &chips, records, &FLOW);
    }
    flow
}

#[cfg(test)]
mod tests;
