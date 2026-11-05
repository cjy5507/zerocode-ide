//! A project's builds as their writers leave them — any build at all: Gradle,
//! Maven, Docker, npm, Cargo, a release script. One JSON record per run, under
//! the repository's shared git folder, so every worktree of the repository
//! reads the same runs and git tracks none of them:
//!
//! ```text
//! <shared git folder>/zo/builds/<build>/<started ms, 15 digits>.json
//! ```
//!
//! `zo build report` writes the records, and so may any script that keeps
//! their shape ([`Run`]). A record is replaced whole, never edited in place,
//! and a build keeps its newest [`RUNS_KEPT`]. The window's build card reads
//! them through a [`Shelf`] and never writes here. The builds stand in the
//! project's own order — the `[[build]]` tables of its `.zo/build.toml`, top to
//! bottom — and the others after them by id.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_file::write_atomically;
use crate::file_stamp::{FileStamp, stamp_plain};
use crate::usual::{Expected, expected_of};

/// Where a repository's builds live under its shared git folder.
const FOLDER: [&str; 2] = ["zo", "builds"];

/// The project's own file that orders its builds, under a checkout's root.
const ORDER_FILE: [&str; 2] = [".zo", "build.toml"];

const RECORD_EXTENSION: &str = "json";

/// Runs kept of one build: the newest, and enough ended ones before it to find
/// [`SAMPLES`](crate::usual::SAMPLES) that passed when some of them failed.
pub const RUNS_KEPT: usize = 12;

/// Builds one project shows at most.
const BUILDS_SHOWN: usize = 16;

/// The bits a revision keeps: the window holds it as a JavaScript number, exact
/// only up to 2^53, and hands it back as `known`.
const REVISION_BITS: u32 = 53;

/// A record, or an order file, larger than this is not one a writer left, and
/// is not read.
const FILE_BYTES_MAX: u64 = 256 * 1024;

/// A running record nobody has written for this long is silent: its writer
/// most likely ended without saying how the build did.
const SILENT_AFTER_MS: i64 = 6 * 60 * 60 * 1000;

/// A build id is a folder name: this long at most.
const BUILD_ID_LONGEST: usize = 64;

/// Words to show: one text, or one per language the writer knew
/// (`{"en": "Compile", "ko": "컴파일"}`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Text {
    One(String),
    ByLanguage(BTreeMap<String, String>),
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    /// Stopped before its end, by a person or by a newer run; the reason says.
    Cancelled,
    /// A word this build does not know: the run ended, and how is the writer's.
    #[serde(other)]
    Other,
}

/// A step a run means to take. `name` is the step's id, the one
/// [`StepDone::name`] and [`Run::running`] use; `words` is what to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Text>,
}

/// A step that ended: its exit code and seconds, or why it did nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepDone {
    pub name: String,
    #[serde(default)]
    pub rc: i32,
    #[serde(default)]
    pub secs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// The running step's command has written nothing for longer than it usually
/// stays quiet.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Stalled {
    pub step: String,
    /// The command, as the writer names it.
    pub what: String,
    pub quiet_since_ms: i64,
    /// Past this the writer ends the step as failed; 0 when it does not.
    #[serde(default)]
    pub cap_secs: u32,
}

/// One run of one build, as its writer left it. Any field may be missing from
/// a record; times are milliseconds since the epoch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Run {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Text>,
    /// What tells this run from the build's others: a version, a tag.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// The tool that builds it, as a person would name it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sha: String,
    pub started_ms: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plan: Vec<PlanStep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub done: Vec<StepDone>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_since_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stalled: Option<Stalled>,
}

impl Run {
    /// `step` starts at `now_ms`.
    pub fn start_step(&mut self, step: &str, now_ms: i64) {
        self.running = Some(step.to_owned());
        self.running_since_ms = Some(now_ms);
        self.stalled = None;
    }

    /// `step` ended with `rc` after `secs` — with no seconds given, the time
    /// since it started, when it is the step running.
    pub fn end_step(&mut self, step: &str, rc: i32, secs: Option<u32>, now_ms: i64) {
        let secs = secs.unwrap_or_else(|| {
            self.running_since_ms
                .filter(|_| self.running.as_deref() == Some(step))
                .map_or(0, |since| {
                    u32::try_from(now_ms.saturating_sub(since).max(0) / 1000).unwrap_or(u32::MAX)
                })
        });
        self.finish_step(StepDone {
            name: step.to_owned(),
            rc,
            secs,
            skipped: None,
        });
    }

    /// `step` did nothing, for the reason `why`.
    pub fn skip_step(&mut self, step: &str, why: String) {
        self.finish_step(StepDone {
            name: step.to_owned(),
            rc: 0,
            secs: 0,
            skipped: Some(why),
        });
    }

    /// The run ended as `outcome`, for the reason `reason` when it says one.
    pub fn end(&mut self, outcome: Outcome, reason: String) {
        self.outcome = Some(outcome);
        self.reason = reason;
        self.running = None;
        self.running_since_ms = None;
        self.stalled = None;
    }

    fn finish_step(&mut self, done: StepDone) {
        if self.running.as_deref() == Some(done.name.as_str()) {
            self.running = None;
            self.running_since_ms = None;
            self.stalled = None;
        }
        self.done.push(done);
    }
}

/// Why a record could not be written.
#[derive(Debug)]
pub enum ReportError {
    /// The id cannot name a build's folder.
    BadId(String),
    /// The build has no run to add to.
    NotBegun(String),
    /// The build's newest run has ended: a step or an end belongs to a new one.
    Ended(String),
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadId(id) => write!(
                f,
                "`{id}` cannot name a build: use at most {BUILD_ID_LONGEST} letters, digits, `.`, `_` or `-`, led by a letter or digit"
            ),
            Self::NotBegun(id) => write!(f, "no run of `{id}` has begun"),
            Self::Ended(id) => write!(f, "the newest run of `{id}` has ended; begin another"),
            Self::Io(error) => error.fmt(f),
            Self::Json(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ReportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::BadId(_) | Self::NotBegun(_) | Self::Ended(_) => None,
        }
    }
}

impl From<io::Error> for ReportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ReportError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// The folder of a repository's builds, under its shared git folder.
#[must_use]
pub fn folder(shared_git_dir: &Path) -> PathBuf {
    joined(shared_git_dir, &FOLDER)
}

/// Whether `id` can name a build. It is a folder name on every platform:
/// ASCII letters, digits, `.`, `_` and `-`, led by a letter or a digit.
#[must_use]
pub fn is_build_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= BUILD_ID_LONGEST
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|rest| rest.is_ascii_alphanumeric() || matches!(rest, '.' | '_' | '-'))
}

/// Begins a run of build `id`: a new record named for `run.started_ms`, beside
/// the build's others, the oldest beyond [`RUNS_KEPT`] removed.
pub fn begin(shared_git_dir: &Path, id: &str, run: &Run) -> Result<PathBuf, ReportError> {
    let dir = build_dir(shared_git_dir, id)?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{:015}.{RECORD_EXTENSION}", run.started_ms.max(0)));
    save(&path, run)?;
    for old in record_paths(&dir).into_iter().skip(RUNS_KEPT) {
        let _ = std::fs::remove_file(old);
    }
    Ok(path)
}

/// The newest run of build `id` while it runs, and where its record is.
pub fn running(shared_git_dir: &Path, id: &str) -> Result<(PathBuf, Run), ReportError> {
    let dir = build_dir(shared_git_dir, id)?;
    let path = record_paths(&dir)
        .into_iter()
        .next()
        .ok_or_else(|| ReportError::NotBegun(id.to_owned()))?;
    let run: Run = serde_json::from_slice(&read_bounded(&path)?)?;
    if run.outcome.is_some() {
        return Err(ReportError::Ended(id.to_owned()));
    }
    Ok((path, run))
}

/// Writes `run` over the record at `path`, whole.
pub fn save(path: &Path, run: &Run) -> Result<(), ReportError> {
    let mut bytes = serde_json::to_vec_pretty(run)?;
    bytes.push(b'\n');
    write_atomically(path, &bytes)?;
    Ok(())
}

/// A build as the window shows it: its newest run, each step beside what it
/// usually takes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Build {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Text>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sha: String,
    pub started_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running_since_ms: Option<i64>,
    /// When the writer last wrote the record.
    pub updated_ms: i64,
    pub state: State,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stalled: Option<Stalled>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Begun, and not one step started yet: it waits its turn.
    Waiting,
    Running,
    /// Running by its record, which nobody has written for a long while.
    Silent,
    Passed,
    Failed,
    Cancelled,
    /// Ended in a word this build does not know.
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Step {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<Text>,
    pub state: StepState,
    /// What it took, once it ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    pub expected: Expected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Done,
    Failed,
    /// Did nothing: the writer skipped it, or the run ended before it.
    Skipped,
    Running,
    Ahead,
}

/// A repository's builds as the window shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Look {
    /// Moves when anything shown moves: the same builds answer the same number,
    /// one the window holds exactly (`REVISION_BITS`).
    pub revision: u64,
    pub builds: Vec<Build>,
}

/// The window's memory of one repository's records: a file is parsed once,
/// and again only when its stamp moves. It holds no more than the files the
/// last look listed.
#[derive(Debug, Default)]
pub struct Shelf {
    folder: PathBuf,
    order: Option<(PathBuf, FileStamp, Vec<String>)>,
    runs: HashMap<PathBuf, (FileStamp, Option<Run>)>,
}

impl Shelf {
    /// The builds of the repository whose shared git folder is
    /// `shared_git_dir`, in the order the `.zo/build.toml` of `checkout` gives,
    /// as of `now_ms`: a running record goes silent with time.
    pub fn look(&mut self, shared_git_dir: &Path, checkout: &Path, now_ms: i64) -> Look {
        let folder = folder(shared_git_dir);
        if self.folder != folder {
            *self = Self {
                folder,
                ..Self::default()
            };
        }
        let order = self.order_of(checkout);
        let mut listed = Vec::new();
        let mut seen = HashSet::new();
        for id in build_ids(&self.folder, &order) {
            let files = record_files(&self.folder.join(&id));
            for (path, stamp) in &files {
                seen.insert(path.clone());
                if self.runs.get(path).is_none_or(|(held, _)| held != stamp) {
                    let run = read_bounded(path)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                    self.runs.insert(path.clone(), (*stamp, run));
                }
            }
            listed.push((id, files));
        }
        self.runs.retain(|path, _| seen.contains(path));
        let builds: Vec<Build> = listed
            .into_iter()
            .filter_map(|(id, files)| {
                let runs: Vec<(&Run, FileStamp)> = files
                    .iter()
                    .filter_map(|(path, stamp)| {
                        self.runs
                            .get(path)
                            .and_then(|(_, run)| run.as_ref())
                            .map(|run| (run, *stamp))
                    })
                    .collect();
                build_of(id, &runs, now_ms)
            })
            .collect();
        let mut hasher = DefaultHasher::new();
        builds.hash(&mut hasher);
        Look {
            revision: hasher.finish() >> (u64::BITS - REVISION_BITS),
            builds,
        }
    }

    fn order_of(&mut self, checkout: &Path) -> Vec<String> {
        let path = joined(checkout, &ORDER_FILE);
        let Some(stamp) = stamp_plain(&path) else {
            self.order = None;
            return Vec::new();
        };
        if let Some((held_path, held, order)) = &self.order
            && *held_path == path
            && *held == stamp
        {
            return order.clone();
        }
        let order = read_bounded(&path)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|text| order_in(&text))
            .unwrap_or_default();
        self.order = Some((path, stamp, order.clone()));
        order
    }
}

/// The build ids an order file names, top to bottom; none when it does not
/// parse.
fn order_in(text: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct OrderFile {
        #[serde(default)]
        build: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
    }
    toml::from_str::<OrderFile>(text)
        .map(|file| file.build.into_iter().map(|entry| entry.id).collect())
        .unwrap_or_default()
}

/// The ids of the builds under `folder`, in the project's order: those it
/// names first, as it names them, then the others by id.
fn build_ids(folder: &Path, order: &[String]) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|id| is_build_id(id))
        .collect();
    let rank = |id: &String| {
        order
            .iter()
            .position(|named| named == id)
            .unwrap_or(usize::MAX)
    };
    ids.sort_by(|one, other| rank(one).cmp(&rank(other)).then_with(|| one.cmp(other)));
    ids.truncate(BUILDS_SHOWN);
    ids
}

/// A build's record files, newest first.
fn record_paths(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == RECORD_EXTENSION)
        })
        .collect();
    paths.sort_unstable_by(|one, other| other.file_name().cmp(&one.file_name()));
    paths
}

/// The newest [`RUNS_KEPT`] records of a build that are plain files, with
/// their stamps.
fn record_files(dir: &Path) -> Vec<(PathBuf, FileStamp)> {
    record_paths(dir)
        .into_iter()
        .take(RUNS_KEPT)
        .filter_map(|path| stamp_plain(&path).map(|stamp| (path, stamp)))
        .collect()
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(FILE_BYTES_MAX + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > FILE_BYTES_MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is larger than {FILE_BYTES_MAX} bytes", path.display()),
        ));
    }
    Ok(bytes)
}

fn build_dir(shared_git_dir: &Path, id: &str) -> Result<PathBuf, ReportError> {
    if is_build_id(id) {
        Ok(folder(shared_git_dir).join(id))
    } else {
        Err(ReportError::BadId(id.to_owned()))
    }
}

fn joined(root: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(root.to_path_buf(), |dir, part| dir.join(part))
}

/// The build `id` as its runs say, newest first, each with its file's stamp.
fn build_of(id: String, runs: &[(&Run, FileStamp)], now_ms: i64) -> Option<Build> {
    let (&(newest, stamp), older) = runs.split_first()?;
    let state = match newest.outcome {
        Some(Outcome::Passed) => State::Passed,
        Some(Outcome::Failed) => State::Failed,
        Some(Outcome::Cancelled) => State::Cancelled,
        Some(Outcome::Other) => State::Ended,
        None if now_ms.saturating_sub(stamp.modified_ms) > SILENT_AFTER_MS => State::Silent,
        None if newest.running.is_none() && newest.done.is_empty() => State::Waiting,
        None => State::Running,
    };
    Some(Build {
        steps: steps_of(newest, &passing_secs(older)),
        id,
        name: newest.name.clone(),
        label: newest.label.clone(),
        tool: newest.tool.clone(),
        sha: newest.sha.clone(),
        started_ms: newest.started_ms,
        running_since_ms: newest.running_since_ms.filter(|_| state == State::Running),
        updated_ms: stamp.modified_ms,
        state,
        reason: newest.reason.clone(),
        stalled: newest.stalled.clone().filter(|_| state == State::Running),
    })
}

/// What each step took in the runs among `older` (newest first) that passed —
/// per step, oldest first.
fn passing_secs<'run>(older: &[(&'run Run, FileStamp)]) -> HashMap<&'run str, Vec<u32>> {
    let mut past: HashMap<&str, Vec<u32>> = HashMap::new();
    for (run, _) in older
        .iter()
        .rev()
        .filter(|(run, _)| run.outcome == Some(Outcome::Passed))
    {
        for step in run
            .done
            .iter()
            .filter(|step| step.rc == 0 && step.skipped.is_none())
        {
            past.entry(step.name.as_str()).or_default().push(step.secs);
        }
    }
    past
}

/// A run's steps in order: the ones it planned, then any other it ran, as it
/// ran them — each beside what it took in the passing runs before.
fn steps_of(run: &Run, past: &HashMap<&str, Vec<u32>>) -> Vec<Step> {
    let ended = run.outcome.is_some();
    let mut names: Vec<&str> = run.plan.iter().map(|step| step.name.as_str()).collect();
    for name in run
        .done
        .iter()
        .map(|step| step.name.as_str())
        .chain(run.running.as_deref())
    {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
        .into_iter()
        .map(|name| {
            let done = run.done.iter().find(|step| step.name == name);
            let state = match done {
                Some(step) if step.skipped.is_some() => StepState::Skipped,
                Some(step) if step.rc == 0 => StepState::Done,
                Some(_) => StepState::Failed,
                None if ended => StepState::Skipped,
                None if run.running.as_deref() == Some(name) => StepState::Running,
                None => StepState::Ahead,
            };
            Step {
                name: name.to_owned(),
                words: run
                    .plan
                    .iter()
                    .find(|step| step.name == name)
                    .and_then(|step| step.words.clone()),
                state,
                secs: done
                    .filter(|step| step.skipped.is_none())
                    .map(|step| step.secs),
                skipped: done.and_then(|step| step.skipped.clone()),
                expected: expected_of(past.get(name).map(Vec::as_slice)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE_MS: i64 = 60_000;

    fn plan(names: &[&str]) -> Vec<PlanStep> {
        names
            .iter()
            .map(|name| PlanStep {
                name: (*name).to_owned(),
                words: None,
            })
            .collect()
    }

    fn passed(started_ms: i64, secs: &[(&str, u32)]) -> Run {
        let mut run = Run {
            started_ms,
            plan: plan(&secs.iter().map(|(name, _)| *name).collect::<Vec<_>>()),
            ..Run::default()
        };
        for (name, took) in secs {
            run.end_step(name, 0, Some(*took), started_ms);
        }
        run.end(Outcome::Passed, String::new());
        run
    }

    fn look(shared: &Path, checkout: &Path) -> Look {
        Shelf::default().look(shared, checkout, MINUTE_MS)
    }

    #[test]
    fn a_running_build_shows_its_plan_with_each_steps_usual_time() {
        let shared = tempfile::tempdir().expect("a git folder");
        for (at, took) in [(1, 30), (2, 50), (3, 40)] {
            begin(
                shared.path(),
                "app",
                &passed(at, &[("compile", took), ("test", 100)]),
            )
            .expect("a passed run");
        }
        let mut run = Run {
            started_ms: 4,
            plan: plan(&["compile", "test"]),
            ..Run::default()
        };
        run.end_step("compile", 0, Some(35), 4);
        run.start_step("test", 4);
        begin(shared.path(), "app", &run).expect("the running run");

        let shown = look(shared.path(), shared.path());

        let build = &shown.builds[0];
        assert_eq!(build.state, State::Running);
        assert_eq!(
            build
                .steps
                .iter()
                .map(|step| (step.name.as_str(), step.state, step.expected.secs))
                .collect::<Vec<_>>(),
            [
                ("compile", StepState::Done, 40),
                ("test", StepState::Running, 100)
            ]
        );
    }

    #[test]
    fn a_failed_run_skips_the_steps_it_never_reached() {
        let shared = tempfile::tempdir().expect("a git folder");
        let mut run = Run {
            started_ms: 1,
            plan: plan(&["compile", "test", "package"]),
            ..Run::default()
        };
        run.end_step("compile", 0, Some(5), 1);
        run.end_step("test", 1, Some(9), 1);
        run.end(Outcome::Failed, "3 tests failed".to_owned());
        begin(shared.path(), "app", &run).expect("the failed run");

        let build = &look(shared.path(), shared.path()).builds[0];

        assert_eq!(
            build
                .steps
                .iter()
                .map(|step| step.state)
                .collect::<Vec<_>>(),
            [StepState::Done, StepState::Failed, StepState::Skipped]
        );
        assert_eq!(
            (build.state, build.reason.as_str()),
            (State::Failed, "3 tests failed")
        );
    }

    #[test]
    fn builds_stand_in_the_order_the_project_gives_then_by_id() {
        let shared = tempfile::tempdir().expect("a git folder");
        let checkout = tempfile::tempdir().expect("a checkout");
        for id in ["zeta", "image", "app", "docs"] {
            begin(shared.path(), id, &passed(1, &[("build", 1)])).expect("a run");
        }
        std::fs::create_dir_all(checkout.path().join(".zo")).expect("the .zo folder");
        std::fs::write(
            checkout.path().join(".zo/build.toml"),
            "[[build]]\nid = \"app\"\nname = \"앱\"\n\n[[build]]\nid = \"image\"\n",
        )
        .expect("an order file");

        let ids: Vec<String> = look(shared.path(), checkout.path())
            .builds
            .into_iter()
            .map(|build| build.id)
            .collect();

        assert_eq!(ids, ["app", "image", "docs", "zeta"]);
    }

    #[test]
    fn a_build_keeps_its_newest_runs_only() {
        let shared = tempfile::tempdir().expect("a git folder");
        for at in 0..(RUNS_KEPT as i64 + 3) {
            begin(shared.path(), "app", &passed(at, &[("build", 1)])).expect("a run");
        }

        let kept = record_paths(&folder(shared.path()).join("app"));

        assert_eq!(kept.len(), RUNS_KEPT);
        assert!(kept[0].ends_with(format!("{:015}.json", RUNS_KEPT as i64 + 2)));
    }

    #[test]
    fn the_same_records_answer_the_same_revision_and_a_new_step_moves_it() {
        let shared = tempfile::tempdir().expect("a git folder");
        let mut run = Run {
            started_ms: 1,
            plan: plan(&["compile", "test"]),
            ..Run::default()
        };
        run.start_step("compile", 1);
        let path = begin(shared.path(), "app", &run).expect("a run");
        let mut shelf = Shelf::default();
        let first = shelf.look(shared.path(), shared.path(), MINUTE_MS).revision;
        let again = shelf.look(shared.path(), shared.path(), MINUTE_MS).revision;

        run.end_step("compile", 0, None, 2);
        save(&path, &run).expect("the step ended");
        let moved = shelf.look(shared.path(), shared.path(), MINUTE_MS).revision;

        assert_eq!(first, again);
        assert_ne!(first, moved);
        assert!(
            [first, moved]
                .iter()
                .all(|revision| revision >> REVISION_BITS == 0),
            "a revision the window cannot hold exactly: {first} {moved}"
        );
    }

    #[test]
    fn a_run_with_no_step_started_waits_its_turn() {
        let shared = tempfile::tempdir().expect("a git folder");
        let waiting = Run {
            started_ms: 1,
            plan: plan(&["build", "push"]),
            ..Run::default()
        };
        begin(shared.path(), "image", &waiting).expect("a queued run");

        let build = &look(shared.path(), shared.path()).builds[0];

        assert_eq!(build.state, State::Waiting);
        assert!(
            build
                .steps
                .iter()
                .all(|step| step.state == StepState::Ahead)
        );
    }

    #[test]
    fn a_running_record_nobody_writes_goes_silent() {
        let shared = tempfile::tempdir().expect("a git folder");
        begin(shared.path(), "app", &Run::default()).expect("a run");

        let later = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock")
            .as_millis() as i64
            + SILENT_AFTER_MS
            + MINUTE_MS;
        let build = &Shelf::default()
            .look(shared.path(), shared.path(), later)
            .builds[0];

        assert_eq!(build.state, State::Silent);
    }

    #[test]
    fn a_step_added_to_an_ended_run_is_refused() {
        let shared = tempfile::tempdir().expect("a git folder");
        begin(shared.path(), "app", &passed(1, &[("build", 1)])).expect("a run");

        let refused = running(shared.path(), "app");

        assert!(matches!(refused, Err(ReportError::Ended(id)) if id == "app"));
    }

    #[test]
    fn a_build_id_is_a_plain_folder_name() {
        assert!(is_build_id("docker-image_2.x"));
        assert!(!is_build_id("../outside"));
        assert!(!is_build_id(".hidden"));
        assert!(!is_build_id(""));
        assert!(!is_build_id(&"a".repeat(BUILD_ID_LONGEST + 1)));
    }

    #[test]
    fn a_record_from_any_writer_reads_with_the_fields_it_left_out() {
        let run: Run = serde_json::from_str(
            r#"{"name":{"en":"App","ko":"앱"},"started_ms":5,"done":[{"name":"compile","rc":0,"secs":3}],"outcome":"tea"}"#,
        )
        .expect("a sparse record");

        assert_eq!(run.outcome, Some(Outcome::Other));
        assert_eq!(run.done[0].secs, 3);
        assert!(run.plan.is_empty());
    }
}
