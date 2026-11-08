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

const RECORD_EXTENSION: &str = "json";
/// A run's output beside its record ([`log_path`]).
const LOG_EXTENSION: &str = "log";

/// Runs kept of one build: the newest, enough ended ones before it to find
/// [`SAMPLES`](crate::usual::SAMPLES) that passed when some of them failed, and
/// the runs a schedule's week reaches back over — a release lane runs a few
/// times a day.
pub const RUNS_KEPT: usize = 48;

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

/// Where a forgotten run waits before it goes, beside the builds' folders: its
/// name is no build id, so no reader takes it for a build ([`is_build_id`]).
const TRASH: &str = ".trash";
/// The file beside a forgotten record that says when it was forgotten.
const FORGOTTEN_EXTENSION: &str = "forgotten";
/// How long a forgotten run can still be brought back ([`restore`]).
pub const TRASH_KEPT_MS: i64 = 24 * 60 * 60 * 1000;

/// The reason a step before the one a run went on from did nothing: the board
/// says it in the person's words.
pub const SKIPPED_BEFORE_FROM: &str = "before the step this run went on from";

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
    /// The units its output named as it ran, when its tool's output is read
    /// ([`crate::build_progress`]): the whole of the step's next run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<u32>,
}

/// How far the running step is, by the units its own output names
/// ([`crate::build_progress`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Progress {
    /// The units its output has begun; the last may still run.
    pub done: u32,
    /// How many make the whole, when known: what its output says, or what its
    /// last passing runs counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub of: Option<u32>,
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
    /// The file the writer keeps this run's output in, when it names one: an
    /// absolute path ([`log_of`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub log: String,
    /// The step the run waits on a person to press (`when = "manual"` in the
    /// order file): until then it is neither running nor ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
    /// How far the running step is, when its output is read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
    /// Why the step that failed failed, as its output and its test reports name
    /// it ([`crate::build_failures`]): the first few, each where it is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<Failure>,
}

/// One failure a failed step named: a test, an error, and where it is when the
/// tool said so — a file (absolute when it stands in the folder the step ran
/// in) and a line.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Failure {
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

impl Run {
    /// `step` starts at `now_ms`.
    pub fn start_step(&mut self, step: &str, now_ms: i64) {
        self.failures.clear();
        self.running = Some(step.to_owned());
        self.running_since_ms = Some(now_ms);
        self.stalled = None;
        self.waiting = None;
        self.progress = None;
    }

    /// `step` ended with `rc` after `secs` — with no seconds given, the time
    /// since it started, when it is the step running. A running step keeps the
    /// units its output named.
    pub fn end_step(&mut self, step: &str, rc: i32, secs: Option<u32>, now_ms: i64) {
        let running = self.running.as_deref() == Some(step);
        let secs = secs.unwrap_or_else(|| {
            self.running_since_ms
                .filter(|_| running)
                .map_or(0, |since| {
                    u32::try_from(now_ms.saturating_sub(since).max(0) / 1000).unwrap_or(u32::MAX)
                })
        });
        let units = self
            .progress
            .filter(|_| running)
            .map(|progress| progress.done);
        self.finish_step(StepDone {
            name: step.to_owned(),
            rc,
            secs,
            skipped: None,
            units,
        });
    }

    /// `step` did nothing, for the reason `why`.
    pub fn skip_step(&mut self, step: &str, why: String) {
        self.finish_step(StepDone {
            name: step.to_owned(),
            rc: 0,
            secs: 0,
            skipped: Some(why),
            units: None,
        });
    }

    /// The run ended as `outcome`, for the reason `reason` when it says one.
    pub fn end(&mut self, outcome: Outcome, reason: String) {
        self.outcome = Some(outcome);
        self.reason = reason;
        self.running = None;
        self.running_since_ms = None;
        self.stalled = None;
        self.waiting = None;
        self.progress = None;
    }

    fn finish_step(&mut self, done: StepDone) {
        if self.running.as_deref() == Some(done.name.as_str()) {
            self.running = None;
            self.running_since_ms = None;
            self.stalled = None;
            self.progress = None;
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
    /// The build keeps no run that began at that moment.
    NoRun(String, i64),
    /// The run is still going: only an ended run is forgotten.
    Live(String),
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
            Self::NoRun(id, started_ms) => {
                write!(f, "`{id}` keeps no run that began at {started_ms}")
            }
            Self::Live(id) => write!(
                f,
                "the run of `{id}` is still going; stop it before forgetting it"
            ),
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
            Self::BadId(_)
            | Self::NotBegun(_)
            | Self::Ended(_)
            | Self::NoRun(..)
            | Self::Live(_) => None,
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
    let path = dir.join(record_name(run.started_ms));
    save(&path, run)?;
    for old in record_paths(&dir).into_iter().skip(RUNS_KEPT) {
        let _ = std::fs::remove_file(old.with_extension(LOG_EXTENSION));
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

/// Forgets the ended run of build `id` that began at `started_ms`: its record,
/// and its log when that is the one beside it, move to the trash beside the
/// builds, where they wait [`TRASH_KEPT_MS`] to be brought back ([`restore`])
/// before they go ([`purge_trash`]). The board shows the build's run before it,
/// or no build when it kept no other.
pub fn forget(
    shared_git_dir: &Path,
    id: &str,
    started_ms: i64,
    now_ms: i64,
) -> Result<(), ReportError> {
    let dir = build_dir(shared_git_dir, id)?;
    let record = dir.join(record_name(started_ms));
    if stamp_plain(&record).is_none() {
        return Err(ReportError::NoRun(id.to_owned(), started_ms));
    }
    let run: Run = serde_json::from_slice(&read_bounded(&record)?)?;
    if run.outcome.is_none() {
        return Err(ReportError::Live(id.to_owned()));
    }
    let trash = trash_dir(shared_git_dir, id);
    std::fs::create_dir_all(&trash)?;
    let gone = trash.join(record_name(started_ms));
    write_atomically(
        &gone.with_extension(FORGOTTEN_EXTENSION),
        now_ms.to_string().as_bytes(),
    )?;
    let log = record.with_extension(LOG_EXTENSION);
    if stamp_plain(&log).is_some() {
        std::fs::rename(&log, gone.with_extension(LOG_EXTENSION))?;
    }
    std::fs::rename(&record, &gone)?;
    purge_trash(shared_git_dir, now_ms);
    Ok(())
}

/// Brings back the run of build `id` that began at `started_ms` from the
/// trash, with its log, while it is still there and no record of that moment
/// stands in its place.
pub fn restore(shared_git_dir: &Path, id: &str, started_ms: i64) -> Result<(), ReportError> {
    let dir = build_dir(shared_git_dir, id)?;
    let gone = trash_dir(shared_git_dir, id).join(record_name(started_ms));
    if stamp_plain(&gone).is_none() {
        return Err(ReportError::NoRun(id.to_owned(), started_ms));
    }
    let record = dir.join(record_name(started_ms));
    if stamp_plain(&record).is_some() {
        return Err(ReportError::NoRun(id.to_owned(), started_ms));
    }
    std::fs::create_dir_all(&dir)?;
    let log = gone.with_extension(LOG_EXTENSION);
    if stamp_plain(&log).is_some() {
        std::fs::rename(&log, record.with_extension(LOG_EXTENSION))?;
    }
    std::fs::rename(&gone, &record)?;
    let _ = std::fs::remove_file(gone.with_extension(FORGOTTEN_EXTENSION));
    Ok(())
}

/// Removes what was forgotten longer than [`TRASH_KEPT_MS`] ago — a forgotten
/// record whose note says when — and the trash folders it leaves empty. What it
/// cannot read it leaves.
pub fn purge_trash(shared_git_dir: &Path, now_ms: i64) {
    let trash = folder(shared_git_dir).join(TRASH);
    let Ok(builds) = std::fs::read_dir(&trash) else {
        return;
    };
    for build in builds.filter_map(Result::ok) {
        let dir = build.path();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for note in entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext == FORGOTTEN_EXTENSION)
            })
        {
            let Some(at) = std::fs::read_to_string(&note)
                .ok()
                .and_then(|text| text.trim().parse::<i64>().ok())
            else {
                continue;
            };
            if now_ms.saturating_sub(at) < TRASH_KEPT_MS {
                continue;
            }
            let _ = std::fs::remove_file(note.with_extension(RECORD_EXTENSION));
            let _ = std::fs::remove_file(note.with_extension(LOG_EXTENSION));
            let _ = std::fs::remove_file(&note);
        }
        let _ = std::fs::remove_dir(&dir);
    }
    let _ = std::fs::remove_dir(&trash);
}

fn trash_dir(shared_git_dir: &Path, id: &str) -> PathBuf {
    folder(shared_git_dir).join(TRASH).join(id)
}

/// The log of the run of build `id` that began at `started_ms`: the file its
/// record names, when that record is one the build keeps and the file is a
/// plain one at an absolute path. The path is only ever the record writer's.
#[must_use]
pub fn log_of(shared_git_dir: &Path, id: &str, started_ms: i64) -> Option<PathBuf> {
    let dir = build_dir(shared_git_dir, id).ok()?;
    let wanted = dir.join(record_name(started_ms));
    let (path, _) = record_files(&dir)
        .into_iter()
        .find(|(path, _)| *path == wanted)?;
    let run: Run = serde_json::from_slice(&read_bounded(&path).ok()?).ok()?;
    let log = PathBuf::from(run.log);
    (log.is_absolute() && stamp_plain(&log).is_some()).then_some(log)
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
    /// The writer named a log the window can open ([`log_of`]).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub log: bool,
    /// The log file the writer named, when that is an absolute path: the board's
    /// 「로그 파일 열기」.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
    /// The step the newest run waits on a person to press ([`Run::waiting`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
    /// Why the newest run's failed step failed ([`Run::failures`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<Failure>,
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
    /// How far it is, while it runs and its output is read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
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
    /// What the checkout's own order file says of itself.
    pub file: OrderFile,
    /// The builds the checkout's root suggests while it has no order file
    /// ([`Look::suggesting`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggest: Vec<BuildName>,
    /// The builds of the order file `zo build` runs, in its order: the others
    /// are reported by another script.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runnable: Vec<String>,
    /// What a build another script reports lets the board do: its retry and
    /// stop, when the order file names them (`zo build act`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub report_acts: BTreeMap<String, ReportActs>,
}

/// The acts a reported build names in the order file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
pub struct ReportActs {
    pub retry: bool,
    pub stop: bool,
}

impl Look {
    /// The look with the builds `suggestion` drafts, for a checkout that has
    /// no order file; one that has its own suggests nothing, and is not asked.
    #[must_use]
    pub fn suggesting(
        mut self,
        suggestion: impl FnOnce() -> std::sync::Arc<crate::build_file::BuildFile>,
    ) -> Self {
        if self.file != OrderFile::None {
            return self;
        }
        self.suggest = suggestion().builds.iter().map(BuildName::from).collect();
        self.revision = revision_of(&(&self.builds, &self.file, &self.suggest, &self.runnable));
        self
    }
}

/// What a checkout's `.zo/build.toml` says of its builds' order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OrderFile {
    /// The checkout has none.
    #[default]
    None,
    /// It reads: the builds stand in its order.
    Read,
    /// It cannot be run as written; the builds stand by id, and `reason`
    /// says why ([`crate::build_file::parse`]).
    Refused { reason: String },
}

/// A build by its id, name and tool: one a checkout's root suggests, or one
/// that keeps runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct BuildName {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Text>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

impl From<&crate::build_file::BuildSpec> for BuildName {
    fn from(build: &crate::build_file::BuildSpec) -> Self {
        Self {
            id: build.id.clone(),
            name: build.name.clone(),
            tool: build.tool.clone(),
        }
    }
}

impl From<&Build> for BuildName {
    fn from(build: &Build) -> Self {
        Self {
            id: build.id.clone(),
            name: build.name.clone(),
            tool: (!build.tool.is_empty()).then(|| build.tool.clone()),
        }
    }
}

/// Every kept run of a repository's builds, as a schedule draws them: builds
/// in the project's order, each build's runs newest first, each run beside
/// what its steps took in the passing runs before it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct History {
    /// Moves when any run moves, as [`Look::revision`] does.
    pub revision: u64,
    pub runs: Vec<Build>,
}

/// The window's memory of one repository's records: a file is parsed once,
/// and again only when its stamp moves. It holds no more than the files the
/// last look listed — the card's look and the schedule's list the same ones.
#[derive(Debug, Default)]
pub struct Shelf {
    folder: PathBuf,
    order: Option<(PathBuf, FileStamp, Order)>,
    runs: HashMap<PathBuf, (FileStamp, Option<Run>)>,
}

/// Each build's kept records, newest first, with their stamps.
type Listed = Vec<(String, Vec<(PathBuf, FileStamp)>)>;

/// What a checkout's order file holds: the ids it names, top to bottom, the
/// ones `zo build` runs, and what it says of itself.
#[derive(Debug, Clone, Default)]
struct Order {
    ids: Vec<String>,
    runs: Vec<String>,
    acts: BTreeMap<String, ReportActs>,
    file: OrderFile,
}

impl Shelf {
    /// The builds of the repository whose shared git folder is
    /// `shared_git_dir`, in the order the `.zo/build.toml` of `checkout` gives,
    /// as of `now_ms`: a running record goes silent with time.
    pub fn look(&mut self, shared_git_dir: &Path, checkout: &Path, now_ms: i64) -> Look {
        let listed = self.list(shared_git_dir, checkout);
        let builds: Vec<Build> = listed
            .into_iter()
            .filter_map(|(id, files)| build_of(id, &self.runs_of(&files), now_ms, false))
            .collect();
        let (file, runnable, report_acts) = self
            .order
            .as_ref()
            .map_or_else(Default::default, |(_, _, order)| {
                (order.file.clone(), order.runs.clone(), order.acts.clone())
            });
        let suggest: Vec<BuildName> = Vec::new();
        Look {
            revision: revision_of(&(&builds, &file, &suggest, &runnable, &report_acts)),
            builds,
            file,
            suggest,
            runnable,
            report_acts,
        }
    }

    /// Every kept run of the same builds, as of `now_ms`: a run that never said
    /// how it ended is silent once a newer run of its build has begun.
    pub fn history(&mut self, shared_git_dir: &Path, checkout: &Path, now_ms: i64) -> History {
        let listed = self.list(shared_git_dir, checkout);
        let runs: Vec<Build> = listed
            .into_iter()
            .flat_map(|(id, files)| {
                let runs = self.runs_of(&files);
                (0..runs.len())
                    .filter_map(|at| build_of(id.clone(), &runs[at..], now_ms, at > 0))
                    .collect::<Vec<_>>()
            })
            .collect();
        History {
            revision: revision_of(&runs),
            runs,
        }
    }

    /// Lists each build's kept records and parses the ones new or moved since
    /// the last list, letting go of the rest.
    fn list(&mut self, shared_git_dir: &Path, checkout: &Path) -> Listed {
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
        for id in build_ids(&self.folder, &order.ids) {
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
        listed
    }

    /// The parsed runs of one build's listed records, newest first; a record
    /// that did not parse is left out.
    fn runs_of(&self, files: &[(PathBuf, FileStamp)]) -> Vec<(&Run, FileStamp)> {
        files
            .iter()
            .filter_map(|(path, stamp)| {
                self.runs
                    .get(path)
                    .and_then(|(_, run)| run.as_ref())
                    .map(|run| (run, *stamp))
            })
            .collect()
    }

    fn order_of(&mut self, checkout: &Path) -> Order {
        let path = joined(checkout, &crate::build_file::FILE);
        let Some(stamp) = stamp_plain(&path) else {
            self.order = None;
            return Order::default();
        };
        if let Some((held_path, held, order)) = &self.order
            && *held_path == path
            && *held == stamp
        {
            return order.clone();
        }
        let order = match crate::build_file::read_text(&path) {
            Ok(Some(text)) => order_in(&text),
            Ok(None) => Order::default(),
            Err(reason) => Order {
                file: OrderFile::Refused { reason },
                ..Order::default()
            },
        };
        self.order = Some((path, stamp, order.clone()));
        order
    }
}

/// The build ids an order file names, top to bottom; none when it cannot be
/// run as written ([`crate::build_file::parse`]), which it then says.
fn order_in(text: &str) -> Order {
    match crate::build_file::parse(text) {
        Ok(file) => Order {
            runs: file
                .builds
                .iter()
                .filter(|build| build.source.runs_here())
                .map(|build| build.id.clone())
                .collect(),
            acts: file
                .builds
                .iter()
                .filter(|build| !build.source.runs_here())
                .map(|build| {
                    let acts = ReportActs {
                        retry: build.act_command(crate::build_file::Act::Retry).is_some(),
                        stop: build.act_command(crate::build_file::Act::Stop).is_some(),
                    };
                    (build.id.clone(), acts)
                })
                .filter(|(_, acts)| acts.retry || acts.stop)
                .collect(),
            ids: file.builds.into_iter().map(|build| build.id).collect(),
            file: OrderFile::Read,
        },
        Err(reason) => Order {
            file: OrderFile::Refused { reason },
            ..Order::default()
        },
    }
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

/// A record's file name: the moment its run began, wide enough to sort.
fn record_name(started_ms: i64) -> String {
    format!("{:015}.{RECORD_EXTENSION}", started_ms.max(0))
}

/// What a step usually takes and how many units its output names, by its
/// passing runs ([`usual`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usual {
    pub secs: Option<u32>,
    pub units: Option<u32>,
}

/// What each step of build `id` usually takes and names, by step: the same
/// reading of its passing runs the board's expectations are ([`expected_of`]);
/// a step with no passing run is not named.
#[must_use]
pub fn usual(shared_git_dir: &Path, id: &str) -> HashMap<String, Usual> {
    let Ok(dir) = build_dir(shared_git_dir, id) else {
        return HashMap::new();
    };
    let runs: Vec<Run> = record_paths(&dir)
        .iter()
        .filter_map(|path| serde_json::from_slice(&read_bounded(path).ok()?).ok())
        .collect();
    let newest = |past: Option<&Vec<u32>>| {
        let usual = expected_of(past.map(Vec::as_slice));
        (usual.samples > 0).then_some(usual.secs)
    };
    let units = passing(runs.iter(), |step| step.units);
    passing_secs(runs.iter())
        .into_iter()
        .map(|(step, secs)| {
            let usual = Usual {
                secs: newest(Some(&secs)),
                units: newest(units.get(step)),
            };
            (step.to_owned(), usual)
        })
        .collect()
}

/// Where `zo build` keeps the output of the run of `id` that began at
/// `started_ms`: beside its record, under the same name.
#[must_use]
pub fn log_path(shared_git_dir: &Path, id: &str, started_ms: i64) -> PathBuf {
    folder(shared_git_dir)
        .join(id)
        .join(record_name(started_ms))
        .with_extension(LOG_EXTENSION)
}

/// The same value answers the same number, one the window holds exactly.
pub(crate) fn revision_of(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish() >> (u64::BITS - REVISION_BITS)
}

pub(crate) fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
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

pub(crate) fn joined(root: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(root.to_path_buf(), |dir, part| dir.join(part))
}

/// The first of `runs` (newest first, each with its file's stamp) as build
/// `id` shows it, beside the passing runs after it. A `superseded` run has a
/// newer one begun after it: unended, it is silent whatever its age.
fn build_of(
    id: String,
    runs: &[(&Run, FileStamp)],
    now_ms: i64,
    superseded: bool,
) -> Option<Build> {
    let (&(newest, stamp), older) = runs.split_first()?;
    let state = match newest.outcome {
        Some(Outcome::Passed) => State::Passed,
        Some(Outcome::Failed) => State::Failed,
        Some(Outcome::Cancelled) => State::Cancelled,
        Some(Outcome::Other) => State::Ended,
        None if superseded => State::Silent,
        // A run waiting on a person waits however long the person takes.
        None if newest.waiting.is_some() => State::Waiting,
        None if now_ms.saturating_sub(stamp.modified_ms) > SILENT_AFTER_MS => State::Silent,
        None if newest.running.is_none() && newest.done.is_empty() => State::Waiting,
        None => State::Running,
    };
    Some(Build {
        steps: steps_of(newest, &passing_secs(older.iter().map(|(run, _)| *run))),
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
        log: !newest.log.is_empty(),
        log_path: Some(newest.log.clone()).filter(|log| Path::new(log).is_absolute()),
        waiting: newest.waiting.clone().filter(|_| state == State::Waiting),
        failures: if state == State::Failed {
            newest.failures.clone()
        } else {
            Vec::new()
        },
    })
}

/// What each step took in the runs among `older` (newest first) that passed —
/// per step, oldest first.
fn passing_secs<'run>(
    older: impl DoubleEndedIterator<Item = &'run Run>,
) -> HashMap<&'run str, Vec<u32>> {
    passing(older, |step| Some(step.secs))
}

/// What `read` reads of each step that passed in the runs among `older`
/// (newest first) that passed — per step, oldest first.
fn passing<'run>(
    older: impl DoubleEndedIterator<Item = &'run Run>,
    read: impl Fn(&StepDone) -> Option<u32>,
) -> HashMap<&'run str, Vec<u32>> {
    let mut past: HashMap<&str, Vec<u32>> = HashMap::new();
    for run in older
        .rev()
        .filter(|run| run.outcome == Some(Outcome::Passed))
    {
        for step in run
            .done
            .iter()
            .filter(|step| step.rc == 0 && step.skipped.is_none())
        {
            if let Some(value) = read(step) {
                past.entry(step.name.as_str()).or_default().push(value);
            }
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
                progress: run.progress.filter(|_| state == StepState::Running),
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

    /// 「기록 지우기」(11-08): an ended run leaves the board for the trash, comes back
    /// on 「되돌리기」, and goes for good once the trash keeps it a day; a run still
    /// going is not forgotten, and a moment no run began at is refused.
    #[test]
    fn a_forgotten_run_leaves_the_board_comes_back_and_goes_after_a_day() {
        let shared = tempfile::tempdir().expect("a git folder");
        let checkout = tempfile::tempdir().expect("a checkout");
        let older =
            begin(shared.path(), "app", &passed(1, &[("compile", 30)])).expect("an older run");
        let newer =
            begin(shared.path(), "app", &passed(2, &[("compile", 40)])).expect("a newer run");
        std::fs::write(newer.with_extension(LOG_EXTENSION), "compiled\n").expect("its log");
        let shown = |at: i64| {
            look(shared.path(), checkout.path())
                .builds
                .iter()
                .map(|build| build.started_ms)
                .collect::<Vec<_>>()
                == vec![at]
        };

        assert!(shown(2), "the newest run stands first");
        forget(shared.path(), "app", 2, 10 * MINUTE_MS).expect("forgotten");
        assert!(shown(1), "the run before it stands once it is forgotten");
        assert!(
            !newer.exists() && !newer.with_extension(LOG_EXTENSION).exists(),
            "record and log left the build's folder"
        );

        restore(shared.path(), "app", 2).expect("brought back");
        assert!(shown(2), "brought back, it stands again");
        assert_eq!(
            std::fs::read_to_string(newer.with_extension(LOG_EXTENSION)).expect("its log is back"),
            "compiled\n"
        );

        forget(shared.path(), "app", 2, 10 * MINUTE_MS).expect("forgotten again");
        purge_trash(shared.path(), 10 * MINUTE_MS + TRASH_KEPT_MS - 1);
        restore(shared.path(), "app", 2).expect("still in the trash a moment before a day");
        forget(shared.path(), "app", 2, 10 * MINUTE_MS).expect("forgotten a third time");
        purge_trash(shared.path(), 10 * MINUTE_MS + TRASH_KEPT_MS);
        assert!(
            matches!(
                restore(shared.path(), "app", 2),
                Err(ReportError::NoRun(_, 2))
            ),
            "a day later it is gone"
        );
        assert!(
            !folder(shared.path()).join(TRASH).exists(),
            "an emptied trash leaves no folder"
        );
        assert!(older.exists(), "the older run was never touched");

        let mut going = Run {
            started_ms: 3,
            plan: plan(&["compile"]),
            ..Run::default()
        };
        going.start_step("compile", 3);
        begin(shared.path(), "app", &going).expect("a run that goes on");
        assert!(
            matches!(
                forget(shared.path(), "app", 3, 4),
                Err(ReportError::Live(_))
            ),
            "a run still going stays"
        );
        assert!(
            matches!(
                forget(shared.path(), "app", 99, 4),
                Err(ReportError::NoRun(_, 99))
            ),
            "no run began then"
        );
        assert!(
            matches!(
                forget(shared.path(), "../app", 1, 4),
                Err(ReportError::BadId(_))
            ),
            "an id that is no folder name"
        );
    }

    #[test]
    fn a_build_shows_the_log_file_its_writer_named_only_when_it_is_absolute() {
        let shared = tempfile::tempdir().expect("a git folder");
        let checkout = tempfile::tempdir().expect("a checkout");
        let mut run = passed(1, &[("compile", 30)]);
        run.log = "relative/out.log".to_owned();
        begin(shared.path(), "app", &run).expect("a run");
        assert_eq!(
            look(shared.path(), checkout.path()).builds[0].log_path,
            None
        );
        let mut run = passed(2, &[("compile", 30)]);
        run.log = log_path(shared.path(), "app", 2).display().to_string();
        begin(shared.path(), "app", &run).expect("a run");
        assert_eq!(
            look(shared.path(), checkout.path()).builds[0].log_path,
            Some(run.log)
        );
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
        let took = |secs| Usual {
            secs: Some(secs),
            units: None,
        };
        assert_eq!(
            usual(shared.path(), "app"),
            HashMap::from([
                ("compile".to_owned(), took(40)),
                ("test".to_owned(), took(100))
            ])
        );
    }

    #[test]
    fn a_steps_progress_stands_while_it_runs_and_its_units_make_the_next_whole() {
        let shared = tempfile::tempdir().expect("a git folder");
        let mut past = Run {
            started_ms: 1,
            plan: plan(&["compile", "test"]),
            ..Run::default()
        };
        past.start_step("compile", 1);
        past.progress = Some(Progress { done: 37, of: None });
        past.end_step("compile", 0, Some(40), 2);
        past.end_step("test", 0, Some(100), 3);
        past.end(Outcome::Passed, String::new());
        begin(shared.path(), "app", &past).expect("a passing run");
        let mut now = Run {
            started_ms: 10,
            plan: plan(&["compile", "test"]),
            ..Run::default()
        };
        now.start_step("compile", 10);
        let reading = Progress {
            done: 12,
            of: Some(37),
        };
        now.progress = Some(reading);
        begin(shared.path(), "app", &now).expect("a running run");

        let steps = &look(shared.path(), shared.path()).builds[0].steps;

        assert_eq!(
            past.done.iter().map(|step| step.units).collect::<Vec<_>>(),
            [Some(37), None],
            "a step that was not running names no units"
        );
        assert_eq!(
            steps.iter().map(|step| step.progress).collect::<Vec<_>>(),
            [Some(reading), None]
        );
        assert_eq!(
            usual(shared.path(), "app")["compile"],
            Usual {
                secs: Some(40),
                units: Some(37)
            }
        );
        now.end_step("compile", 0, None, 20);
        assert_eq!((now.progress, now.done[0].units), (None, Some(12)));
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
            "[[build]]\nid = \"app\"\nname = \"앱\"\nsource = \"report\"\n\n[[build]]\nid = \"image\"\nsource = \"report\"\n",
        )
        .expect("an order file");
        let ids = |checkout: &Path| -> Vec<String> {
            look(shared.path(), checkout)
                .builds
                .into_iter()
                .map(|build| build.id)
                .collect()
        };
        assert_eq!(ids(checkout.path()), ["app", "image", "docs", "zeta"]);

        // A file that cannot be run as written orders nothing: the builds stand by id.
        let broken = tempfile::tempdir().expect("a checkout");
        std::fs::create_dir_all(broken.path().join(".zo")).expect("the .zo folder");
        std::fs::write(
            broken.path().join(".zo/build.toml"),
            "[[build]]\nid = \"zeta\"\nsource = \"report\"\n\n[[build]]\nid = \"image\"\nafter = \"nowhere\"\nsource = \"report\"\n",
        )
        .expect("an order file");
        assert_eq!(ids(broken.path()), ["app", "docs", "image", "zeta"]);
    }

    #[test]
    fn a_build_keeps_its_newest_runs_only_and_their_logs() {
        let shared = tempfile::tempdir().expect("a git folder");
        let runs = RUNS_KEPT as i64 + 3;
        for at in 0..runs {
            begin(shared.path(), "app", &passed(at, &[("build", 1)])).expect("a run");
            std::fs::write(log_path(shared.path(), "app", at), "output\n").expect("its log");
        }

        let kept = record_paths(&folder(shared.path()).join("app"));
        let logs = (0..runs)
            .filter(|at| log_path(shared.path(), "app", *at).exists())
            .count();

        assert_eq!(kept.len(), RUNS_KEPT);
        assert!(kept[0].ends_with(format!("{:015}.json", runs - 1)));
        // The newest run's log is written after its record, so it is the one more kept.
        assert_eq!(logs, RUNS_KEPT);
        assert!(!log_path(shared.path(), "app", 0).exists());
    }

    #[test]
    fn a_run_waiting_on_a_person_waits_however_long_they_take() {
        let shared = tempfile::tempdir().expect("a git folder");
        let mut run = Run {
            started_ms: 1,
            plan: plan(&["build", "push"]),
            ..Run::default()
        };
        run.end_step("build", 0, Some(5), 1);
        run.waiting = Some("push".to_owned());
        let path = begin(shared.path(), "image", &run).expect("a run at its manual step");
        let days_later = SILENT_AFTER_MS * 4
            + i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("the clock")
                    .as_millis(),
            )
            .expect("a moment");

        let build = &Shelf::default()
            .look(shared.path(), shared.path(), days_later)
            .builds[0];

        assert_eq!(
            (build.state, build.waiting.as_deref()),
            (State::Waiting, Some("push"))
        );
        assert_eq!(
            build
                .steps
                .iter()
                .map(|step| step.state)
                .collect::<Vec<_>>(),
            [StepState::Done, StepState::Ahead]
        );
        run.start_step("push", 2);
        save(&path, &run).expect("pressed");
        let pressed = &look(shared.path(), shared.path()).builds[0];
        assert_eq!(
            (pressed.state, pressed.waiting.as_deref()),
            (State::Running, None)
        );
    }

    #[test]
    fn the_look_names_the_builds_zo_build_runs_in_the_files_order() {
        let shared = tempfile::tempdir().expect("a git folder");
        let checkout = tempfile::tempdir().expect("a checkout");
        std::fs::create_dir_all(checkout.path().join(".zo")).expect("the .zo folder");
        std::fs::write(
            checkout.path().join(".zo/build.toml"),
            "[[build]]\nid = \"verify\"\nstep = [{ id = \"test\", run = \"just test\" }]\n\n\
             [[build]]\nid = \"release\"\nsource = \"report\"\n\n\
             [[build]]\nid = \"app\"\nstep = [{ id = \"build\", run = \"make\" }]\n",
        )
        .expect("an order file");

        let shown = look(shared.path(), checkout.path());

        assert_eq!(shown.runnable, ["verify", "app"]);
        assert!(look(shared.path(), shared.path()).runnable.is_empty());
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
    fn a_schedule_reads_every_kept_run_and_one_left_unended_by_a_newer_run_is_silent() {
        let shared = tempfile::tempdir().expect("a git folder");
        for (at, took) in [(1, 30), (2, 50)] {
            begin(shared.path(), "app", &passed(at, &[("build", took)])).expect("a passed run");
        }
        let mut abandoned = Run {
            started_ms: 3,
            plan: plan(&["build"]),
            ..Run::default()
        };
        abandoned.start_step("build", 3);
        begin(shared.path(), "app", &abandoned).expect("a run nobody ended");
        let mut running = abandoned.clone();
        running.started_ms = 4;
        begin(shared.path(), "app", &running).expect("the running run");
        begin(shared.path(), "image", &passed(2, &[("push", 9)])).expect("another build");

        let history = Shelf::default().history(shared.path(), shared.path(), MINUTE_MS);

        assert_eq!(
            history
                .runs
                .iter()
                .map(|run| (run.id.as_str(), run.started_ms, run.state))
                .collect::<Vec<_>>(),
            [
                ("app", 4, State::Running),
                ("app", 3, State::Silent),
                ("app", 2, State::Passed),
                ("app", 1, State::Passed),
                ("image", 2, State::Passed),
            ]
        );
        assert_eq!(
            history.runs[0].steps[0].expected.samples, 2,
            "a run's usual times are the passing runs before it"
        );
        assert_eq!(history.runs[2].steps[0].expected.samples, 1);
        let mut shelf = Shelf::default();
        assert_eq!(
            shelf
                .history(shared.path(), shared.path(), MINUTE_MS)
                .revision,
            history.revision
        );
        begin(shared.path(), "app", &passed(5, &[("build", 40)])).expect("one more run");
        assert_ne!(
            shelf
                .history(shared.path(), shared.path(), MINUTE_MS)
                .revision,
            history.revision
        );
    }

    #[test]
    fn a_runs_log_is_the_plain_file_its_kept_record_names_at_an_absolute_path() {
        let shared = tempfile::tempdir().expect("a git folder");
        let log = shared.path().join("lane.log");
        std::fs::write(&log, "== build\n").expect("a log");
        for (at, named) in [
            (1, log.display().to_string()),
            (2, "lane.log".to_owned()),
            (3, shared.path().display().to_string()),
            (4, String::new()),
        ] {
            let run = Run {
                started_ms: at,
                log: named,
                ..Run::default()
            };
            begin(shared.path(), "release", &run).expect("a run");
        }

        assert_eq!(log_of(shared.path(), "release", 1), Some(log));
        for (at, why) in [
            (2, "a relative path"),
            (3, "a folder"),
            (4, "no log named"),
            (5, "no such run"),
        ] {
            assert_eq!(log_of(shared.path(), "release", at), None, "{why}");
        }
        assert_eq!(
            log_of(shared.path(), "../release", 1),
            None,
            "not a build id"
        );
        let shown = look(shared.path(), shared.path());
        assert!(!shown.builds[0].log, "the newest run named none");
        assert!(
            Shelf::default()
                .history(shared.path(), shared.path(), MINUTE_MS)
                .runs[3]
                .log
        );
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
