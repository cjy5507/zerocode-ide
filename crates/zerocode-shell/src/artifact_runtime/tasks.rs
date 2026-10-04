//! The catalog read by task (t-36910): one line for each task, and everything
//! one task holds.
//!
//! The gallery kept three warehouses — pages, reports, evidence — and a person
//! who wanted to know what a task left had to search each. Here the same rows
//! are grouped by the task the ledger vouched for when each was written
//! (`origin.task`), and a Computer Use session's step log joins the tasks its
//! steps were taken for (the `task:<id>` tags, [`in_place_tags`]). Nothing is
//! grouped by where a file lies or by what it is called.
//!
//! Every number a line says is counted from rows in one pass, and the parts
//! always add up to the whole ([`Parts`]): a line that said "52 files" beside
//! "27 files" would be two different sums. A row whose file is gone is counted
//! as gone and as nothing else.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::{BufReader, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use serde::Serialize;
use zerocode_core::artifact::{Artifact, ArtifactKind, Limits, Source};
use zerocode_core::evidence_digest::{self, Digest, Format, Tests, Told, Verdict};
use zerocode_core::hand_in::Expect;

use super::{Filter, Row, Store, is_gone};

/// What a step log's row is tagged with for each task its steps name.
pub(crate) const TASK_TAG_PREFIX: &str = "task:";

/// The task a tag names, when it is one of ours.
fn task_of_tag(tag: &str) -> Option<&str> {
    tag.strip_prefix(TASK_TAG_PREFIX)
        .filter(|task| !task.is_empty())
}

/// The tasks a row belongs to: the one its origin names, and — for a session's
/// step log — the ones its tags name.
fn tasks_of(artifact: &Artifact) -> impl Iterator<Item = &str> {
    artifact
        .origin
        .task
        .as_deref()
        .into_iter()
        .chain(artifact.tags.iter().filter_map(|tag| task_of_tag(tag)))
}

/// Whether a row is a step log catalogued where it lies: the one file whose
/// row is tagged with tasks.
fn is_step_log(path: &Path, source: Source) -> bool {
    source == Source::Evidence
        && path.file_name() == Some(OsStr::new(crate::run_evidence::STEPS_FILE))
}

/// The tags of a row registered in place. A step log says which tasks its
/// steps were taken for, so a task finds the sessions that worked for it
/// without a log being read again: the row keeps the tags it had, and only the
/// bytes appended since it was last read are read for more. A log shorter than
/// it was has been written anew and is read from its start. Every other row
/// keeps its tags as they were.
pub(super) fn in_place_tags(
    path: &Path,
    source: Source,
    previous: Option<(u64, Vec<String>)>,
    len: u64,
    limits: &Limits,
) -> Vec<String> {
    // Red (t-36910 stage 2): a step log's row is tagged with nothing yet.
    let _ = (path, source, len, limits);
    previous.map(|(_, tags)| tags).unwrap_or_default()
}

/// The worker a report is counted under: the one its origin names, or — for a
/// report whose origin names none — itself.
fn worker_of(row: &Artifact) -> &str {
    row.origin.worker.as_deref().unwrap_or(&row.id)
}

/// What one row is to the task that holds it. The first row of
/// [`PART_BY_KIND`] that names its kind decides; a text log among the evidence
/// is a log, and everything else is another file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Report,
    Page,
    Picture,
    Log,
    Other,
}

const PART_BY_KIND: &[(&[ArtifactKind], Part)] = &[
    (&[ArtifactKind::Report], Part::Report),
    (
        &[
            ArtifactKind::Page,
            ArtifactKind::Document,
            ArtifactKind::Web,
        ],
        Part::Page,
    ),
    (&[ArtifactKind::Screenshot], Part::Picture),
];

fn part_of(artifact: &Artifact) -> Part {
    PART_BY_KIND
        .iter()
        .find(|(kinds, _)| kinds.contains(&artifact.kind))
        .map_or_else(
            || match Format::of(&artifact.path) {
                Some(Format::Text) if artifact.kind == ArtifactKind::Evidence => Part::Log,
                _ => Part::Other,
            },
            |(_, part)| *part,
        )
}

/// What a task holds, counted so that the parts add up: `files` is always
/// `pictures + logs + other`. Reports and pages are counted beside the files,
/// and a row whose file is gone is `missing` and none of the rest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Parts {
    pub(crate) reports: usize,
    /// Pages, documents and claude.ai artifacts.
    pub(crate) pages: usize,
    pub(crate) files: usize,
    pub(crate) pictures: usize,
    pub(crate) logs: usize,
    pub(crate) other: usize,
    /// Rows whose file is gone.
    pub(crate) missing: usize,
}

impl Parts {
    fn count(&mut self, part: Part, gone: bool) {
        if gone {
            self.missing += 1;
            return;
        }
        match part {
            Part::Report => self.reports += 1,
            Part::Page => self.pages += 1,
            Part::Picture => self.pictures += 1,
            Part::Log => self.logs += 1,
            Part::Other => self.other += 1,
        }
        if matches!(part, Part::Picture | Part::Log | Part::Other) {
            self.files += 1;
        }
    }
}

/// One task's line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct TaskLine {
    pub(crate) task: String,
    /// The task in the coordinator's words — the newest row that carries them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) work: Option<String>,
    /// When its newest row was written.
    pub(crate) modified_ms: i64,
    /// The row a reader opens first: the newest report; without a report, the
    /// newest page or document; without either, the newest row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lead: Option<String>,
    /// That row's title — a report's own first heading.
    pub(crate) title: String,
    pub(crate) parts: Parts,
    /// Workers that handed a report in: how many attempts the task took.
    pub(crate) attempts: usize,
    /// Sessions of Computer Use whose steps name the task.
    pub(crate) sessions: usize,
    /// The checkout it was worked in and the commit it handed in, as its newest
    /// row that says either says them — what its landed state is asked by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) worktree: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) commit: Option<String>,
}

/// The rows no task is linked to, in the same parts. A Computer Use session
/// leaves many files, so its files are said twice over: how many files, and in
/// how many sessions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Unlinked {
    pub(crate) parts: Parts,
    /// Reports whose origin says nothing at all.
    pub(crate) reports_without_origin: usize,
    /// Files of Computer Use sessions among the files, and the sessions they
    /// lie in.
    pub(crate) session_files: usize,
    pub(crate) sessions: usize,
}

/// Every task, newest first, bounded by the table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct TaskListing {
    pub(crate) tasks: Vec<TaskLine>,
    /// Tasks the filter admits, before the table's bound.
    pub(crate) total: usize,
    pub(crate) truncated: bool,
    pub(crate) unlinked: Unlinked,
}

/// One task while its rows are being read.
#[derive(Default)]
struct Gathering<'a> {
    parts: Parts,
    newest_ms: i64,
    workers: BTreeSet<&'a str>,
    sessions: usize,
    /// The newest present row of each rank a lead is chosen from.
    report: Option<&'a Artifact>,
    page: Option<&'a Artifact>,
    any: Option<&'a Artifact>,
    work: Option<(i64, &'a str)>,
    worktree: Option<(i64, &'a Path)>,
    commit: Option<(i64, &'a str)>,
}

/// Keep `candidate` when it is newer than what is held; the id breaks a tie so
/// the answer does not depend on the order rows were read in.
fn newer<'a>(held: &mut Option<&'a Artifact>, candidate: &'a Artifact) {
    let wins = held
        .is_none_or(|held| (candidate.modified_ms, &held.id) > (held.modified_ms, &candidate.id));
    if wins {
        *held = Some(candidate);
    }
}

fn newest<'a, T: ?Sized>(held: &mut Option<(i64, &'a T)>, at_ms: i64, value: Option<&'a T>) {
    if let Some(value) = value
        && held.is_none_or(|(held_ms, _)| at_ms > held_ms)
    {
        *held = Some((at_ms, value));
    }
}

impl<'a> Gathering<'a> {
    fn take(&mut self, artifact: &'a Artifact, gone: bool) {
        let part = part_of(artifact);
        self.parts.count(part, gone);
        self.newest_ms = self.newest_ms.max(artifact.modified_ms);
        let origin = &artifact.origin;
        newest(
            &mut self.work,
            artifact.modified_ms,
            origin.work_summary.as_deref(),
        );
        newest(
            &mut self.worktree,
            artifact.modified_ms,
            origin.worktree.as_deref(),
        );
        newest(
            &mut self.commit,
            artifact.modified_ms,
            origin.commit.as_deref(),
        );
        if gone {
            return;
        }
        if origin.task.is_none() {
            // Linked by its tags: a session's step log.
            self.sessions += 1;
        }
        match part {
            Part::Report => {
                self.workers.insert(worker_of(artifact));
                newer(&mut self.report, artifact);
            }
            Part::Page => newer(&mut self.page, artifact),
            _ => {}
        }
        newer(&mut self.any, artifact);
    }

    fn line(self, task: &str) -> TaskLine {
        let lead = self.report.or(self.page).or(self.any);
        TaskLine {
            task: task.to_string(),
            work: self.work.map(|(_, work)| work.to_string()),
            modified_ms: self.newest_ms,
            lead: lead.map(|row| row.id.clone()),
            title: lead.map(|row| row.title.clone()).unwrap_or_default(),
            parts: self.parts,
            attempts: self.workers.len(),
            sessions: self.sessions,
            worktree: self.worktree.map(|(_, path)| path.to_path_buf()),
            commit: self.commit.map(|(_, commit)| commit.to_string()),
        }
    }
}

/// Whether every word of `needle` (lower-cased) is somewhere in a line's task
/// id, its words or its title.
fn line_matches(line: &TaskLine, needle: &[String]) -> bool {
    if needle.is_empty() {
        return true;
    }
    let hay = format!(
        "{}\n{}\n{}",
        line.task,
        line.work.as_deref().unwrap_or_default(),
        line.title
    )
    .to_lowercase();
    needle.iter().all(|word| hay.contains(word.as_str()))
}

/// How a report's place among its task's attempts is told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Attempted {
    pub(crate) id: String,
    /// Which attempt its worker was: 1 for the worker that reported first.
    pub(crate) attempt: usize,
}

/// Which side of a comparison a picture's name says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Before,
    After,
}

/// The words in a picture's name that say it was taken before or after the
/// work. A name is split where it is not a letter or a digit, and a word must
/// be one of these whole.
const SIDE_WORDS: &[(&str, Side)] = &[
    ("before", Side::Before),
    ("after", Side::After),
    ("전", Side::Before),
    ("후", Side::After),
];

/// The side a picture's name says, and the name without that word — what two
/// pictures must share to be one comparison.
fn side_of(path: &Path) -> Option<(Side, String)> {
    // Red (t-36910 stage 2): no picture's name says a side yet.
    let _ = path;
    None
}

/// A before-and-after pair, or a picture that stands alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Picture {
    /// What the pair shares in its names; empty for a picture that stands alone.
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) single: Option<String>,
}

/// Pair a task's pictures by the table of names: a picture whose name says
/// "before" with the one whose name says "after" and is otherwise the same —
/// the newest of each when a name was used twice. Everything else stands alone,
/// in the order given.
fn pictures_of(rows: &[&Artifact]) -> Vec<Picture> {
    let mut sides: BTreeMap<String, [Option<&Artifact>; 2]> = BTreeMap::new();
    for row in rows {
        if let Some((side, name)) = side_of(&row.path) {
            newer(&mut sides.entry(name).or_default()[side as usize], row);
        }
    }
    let paired: BTreeSet<&str> = sides
        .values()
        .filter(|held| held.iter().all(Option::is_some))
        .flat_map(|held| held.iter().flatten().map(|row| row.id.as_str()))
        .collect();
    let mut pictures: Vec<Picture> = sides
        .iter()
        .filter_map(|(name, held)| match held {
            [Some(before), Some(after)] => Some(Picture {
                name: name.clone(),
                before: Some(before.id.clone()),
                after: Some(after.id.clone()),
                single: None,
            }),
            _ => None,
        })
        .collect();
    pictures.extend(
        rows.iter()
            .filter(|row| !paired.contains(row.id.as_str()))
            .map(|row| Picture {
                single: Some(row.id.clone()),
                ..Picture::default()
            }),
    );
    pictures
}

/// What a session's step log says of one task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct SessionSteps {
    /// Steps taken for this task, and steps in all.
    pub(crate) of_task: usize,
    pub(crate) total: usize,
    pub(crate) failed: usize,
}

/// One file of a task's evidence, with what it says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct EvidenceLine {
    pub(crate) id: String,
    /// A test run's counts and verdict, for a log that holds one or an exit
    /// code that is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tests: Option<Tests>,
    /// A Computer Use session's steps, for a step log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) steps: Option<SessionSteps>,
}

/// A task's test runs, added up where they are listed: every number here is the
/// sum of the lines' own, so a reader can check it against them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Tally {
    /// Lines that carry a test run.
    pub(crate) runs: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) ignored: usize,
    /// The runs by verdict.
    pub(crate) pass: usize,
    pub(crate) fail: usize,
    pub(crate) intended: usize,
    pub(crate) unknown: usize,
}

impl Tally {
    fn add(&mut self, tests: &Tests) {
        self.runs += 1;
        self.passed += tests.passed;
        self.failed += tests.failed;
        self.ignored += tests.ignored;
        match tests.verdict {
            Verdict::Pass => self.pass += 1,
            Verdict::Fail => self.fail += 1,
            Verdict::Intended => self.intended += 1,
            Verdict::Unknown => self.unknown += 1,
        }
    }
}

/// Everything one task holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Bundle {
    pub(crate) line: TaskLine,
    /// Its reports, newest first: the first is the final one.
    pub(crate) reports: Vec<Attempted>,
    pub(crate) pictures: Vec<Picture>,
    /// Its logs and other files, newest first. Every file is its own line: a
    /// sum is said only by [`Tally`], beside its parts.
    pub(crate) evidence: Vec<EvidenceLine>,
    pub(crate) tally: Tally,
    /// Lines past the table's count that were not read for what they say.
    pub(crate) unread: usize,
    /// Its pages, documents and claude.ai artifacts, newest first.
    pub(crate) pages: Vec<String>,
    /// Every row named above — and the rows whose file is gone — for the
    /// window's catalog; `missing` names the gone ones.
    pub(crate) rows: Vec<Artifact>,
    pub(crate) missing: Vec<String>,
}

impl Store {
    /// One line for each task the filter admits, newest first. The filter is
    /// the gallery's own — project, agent, period, origin — and its words are
    /// asked of the line (the task's id, its words, its title), not of every
    /// row: a search on this tab is for a task.
    pub(crate) fn tasks(&self, filter: &Filter) -> TaskListing {
        // Red (t-36910 stage 2): no task has a line yet.
        let _ = filter;
        TaskListing::default()
    }

    /// Everything one task holds, with what each of its logs says. `None` for
    /// a task no row names.
    pub(crate) fn bundle(&self, task: &str) -> Option<Bundle> {
        // Red (t-36910 stage 2): no task has a bundle yet.
        let _ = task;
        None
    }

    /// The rows of one task, as copies: the files are read with the catalog's
    /// lock released, so a long log does not hold up a listing.
    fn rows_of(&self, task: &str) -> Vec<Artifact> {
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        index
            .rows
            .values()
            .map(|row| &row.artifact)
            .filter(|artifact| tasks_of(artifact).any(|held| held == task))
            .cloned()
            .collect()
    }

    /// What an evidence file says, as the drawer shows it: its digest, told
    /// what the catalog knows of its run.
    pub(super) fn digest_of(&self, artifact: &Artifact, limits: &Limits) -> Option<Digest> {
        evidence_digest::told(
            super::evidence_digest_of(artifact, limits),
            self.told_about(artifact),
        )
    }

    /// What is known of a row's run from outside its file ([`told_of`]), with
    /// the catalog's own rows as the ones a worker may have handed in beside it
    /// and what its worker said to expect of it.
    pub(super) fn told_about(&self, artifact: &Artifact) -> Told {
        let expect = self.expected_of(artifact);
        // The folder is asked first and needs no lock; the catalog is looked
        // through only for a file the store keeps in a folder of its own.
        let beside = told_of(artifact, std::iter::empty(), expect);
        if beside.rc.is_some() || artifact.origin.task.is_none() {
            return beside;
        }
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        told_of(
            artifact,
            index.rows.values().map(|row| &row.artifact),
            expect,
        )
    }

    /// What its worker said to expect of a row it handed in (the hand-in's own
    /// record, `Entry::expect`): the newest hand-in that names the file
    /// decides. An intended failure is known from the worker's word, never
    /// from a file's name.
    fn expected_of(&self, artifact: &Artifact) -> Option<Expect> {
        let origin = &artifact.origin;
        let (run, task) = (origin.run.as_deref()?, origin.task.as_deref()?);
        let book = self.hand_ins();
        book.of_task(run, task)
            .into_iter()
            .flat_map(|manifest| &manifest.entries)
            .find(|entry| entry.artifact.as_deref() == Some(artifact.id.as_str()))
            .and_then(|entry| entry.expect)
    }

    /// One file of a task's evidence, with what it says of the task. What a
    /// file says is read through the drawer's own previews ([`Store::preview`]):
    /// a file is read once for the list and for the reader, and again only
    /// when it — or what qualifies it — moved. A file no digest is read from
    /// is listed without being opened.
    fn evidence_line(&self, row: &Artifact, task: &str) -> EvidenceLine {
        let digest = if row.kind == ArtifactKind::Evidence && Format::of(&row.path).is_some() {
            self.preview(&row.id)
                .ok()
                .and_then(|preview| preview.digest.clone())
        } else {
            None
        };
        let mut line = EvidenceLine {
            id: row.id.clone(),
            ..EvidenceLine::default()
        };
        match digest {
            Some(Digest::Tests(tests)) => line.tests = Some(tests),
            Some(Digest::Steps(steps)) => {
                line.steps = Some(SessionSteps {
                    of_task: steps
                        .tasks
                        .iter()
                        .find(|held| held.task == task)
                        .map_or(0, |held| held.steps),
                    total: steps.total,
                    failed: steps.failed,
                });
            }
            _ => {}
        }
        line
    }
}

/// The extension of the file a run's exit code is written to, beside its log.
const EXIT_CODE_EXTENSION: &str = "rc";

/// Whether a row is a log a run's exit code can qualify: a text log among the
/// evidence.
fn is_run_log(artifact: &Artifact) -> bool {
    artifact.kind == ArtifactKind::Evidence && Format::of(&artifact.path) == Some(Format::Text)
}

/// Whether `held` is the exit code of the run that wrote `log`: the `.rc` of
/// the same name in the same folder, or — where the store keeps each handed-in
/// file in a folder of its own — the one the same worker handed in for the same
/// task.
fn is_exit_code_of(held: &Artifact, log: &Artifact) -> bool {
    let beside = log.path.with_extension(EXIT_CODE_EXTENSION);
    if held.path == beside {
        return true;
    }
    let (origin, other) = (&log.origin, &held.origin);
    origin.task.is_some()
        && origin.worker.is_some()
        && other.task == origin.task
        && other.worker == origin.worker
        && held.path.file_name() == beside.file_name()
}

/// What is known of a log's run from outside the log: the exit code its `.rc`
/// holds ([`is_exit_code_of`] — the file beside it is read whether or not the
/// catalog holds it), and whether the worker who handed the log in said it was
/// meant to fail. Nothing here reads a name for what the run was meant to do,
/// and only a log among the evidence is told anything.
fn told_of<'a>(
    artifact: &Artifact,
    handed_in: impl Iterator<Item = &'a Artifact>,
    expect: Option<Expect>,
) -> Told {
    // Red (t-36910 stage 2): a log is told nothing of its run yet.
    let _ = (artifact, handed_in, expect);
    Told::default()
}

/// The logs a newly catalogued exit code qualifies, by id — the other way
/// round of [`told_of`]. Their previews are read again: a verdict drawn before
/// the run ended is not kept.
pub(super) fn told_by<'a>(
    exit_code: &Artifact,
    rows: impl Iterator<Item = &'a Artifact>,
) -> Vec<String> {
    if exit_code.kind != ArtifactKind::Evidence
        || Format::of(&exit_code.path) != Some(Format::ExitCode)
    {
        return Vec::new();
    }
    rows.filter(|row| is_run_log(row) && is_exit_code_of(exit_code, row))
        .map(|row| row.id.clone())
        .collect()
}

/// A task's reports with the attempt each one's worker was: workers are
/// numbered by when each first reported, so a worker that reported twice is
/// one attempt.
fn attempts_of(reports: &[&Artifact]) -> Vec<Attempted> {
    let mut first: BTreeMap<&str, i64> = BTreeMap::new();
    for row in reports {
        let at = first.entry(worker_of(row)).or_insert(row.modified_ms);
        *at = (*at).min(row.modified_ms);
    }
    let mut order: Vec<(i64, &str)> = first.into_iter().map(|(name, at)| (at, name)).collect();
    order.sort_unstable();
    reports
        .iter()
        .map(|row| Attempted {
            id: row.id.clone(),
            attempt: order
                .iter()
                .position(|(_, name)| *name == worker_of(row))
                .map_or(1, |at| at + 1),
        })
        .collect()
}

impl Filter {
    /// Everything the filter asks of a row but its words, its task, its kinds
    /// and whether its file is there — the part a listing of rows and a
    /// listing of tasks share.
    pub(super) fn admits_origin(&self, row: &Row) -> bool {
        let artifact = &row.artifact;
        let origin = &artifact.origin;
        let same = |asked: &Option<String>, held: Option<&str>| {
            asked.as_deref().is_none_or(|wanted| held == Some(wanted))
        };
        self.remote
            .is_none_or(|remote| (artifact.kind == ArtifactKind::Web) == remote)
            && self
                .published
                .is_none_or(|published| super::is_publication(artifact) == published)
            && same(&self.run, origin.run.as_deref())
            && same(&self.worker, origin.worker.as_deref())
            && same(&self.automation, origin.automation.as_deref())
            && same(&self.agent, origin.agent.as_deref())
            && self.worktree.as_deref().is_none_or(|wanted| {
                origin
                    .worktree
                    .as_deref()
                    .is_some_and(|held| held == Path::new(wanted))
            })
            && (self.roots.is_empty()
                || [origin.project.as_deref(), origin.worktree.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|path| self.roots.iter().any(|root| path.starts_with(root))))
            && self
                .since_ms
                .is_none_or(|since| artifact.modified_ms >= since)
    }
}

#[cfg(test)]
mod tests;
