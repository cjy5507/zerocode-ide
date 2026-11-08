//! The task board's schedule (t-67482): the project's builds, the build
//! line's jobs and the integration gates among them on one time axis — what
//! ran and when, what runs now and what waits, and how long each running or
//! waiting one is expected to take.
//!
//! A reader, and only a reader. A build is any build that left its records
//! (`zerocode_core::builds`): every run each keeps, read off the shelf the
//! build card reads (`cmd::board`). The build line is the coordinator's
//! runner: a queue folder whose `in/` holds what waits, `run/` what runs and
//! `done/` what ended (and every job's log, from its first line), with a
//! `runner.log` it appends a `RUN` and a `DONE` line to per job — a project
//! with no such folder has no line. Nothing here writes, moves, renames or
//! locks a file of either, and nothing here opens the ledger — a task's own
//! times ride the desk's reading (`desk::TaskTimes`).
//!
//! What a thing will take is read off the past alone: the median of the last
//! five runs of the same name that passed — a build's step by its name, a
//! build-line job by the `-g…` tail its kind repeats in every landing. A job
//! with no such past is bounded by its cap, and says so. Where on the clock a
//! waiting job will start, and how much of a running one is left, is the
//! window's to place (`ui/shell-board-schedule.js`): that moves with the
//! time, and this answer moves only when a file does.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::{File, Metadata};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use zerocode_core::builds::{Build, History, State as BuildState};
use zerocode_core::file_stamp::{FileStamp, stamp, stamp_of, stamp_plain};
use zerocode_core::usual::{Expected, SAMPLES, median};

use crate::artifact_flow::Landed;

#[cfg(test)]
mod tests;

/// Where the build line's queue lives under a project's workspace folder —
/// the folder the window's workspace setting puts the repository's checkouts
/// in — as the runner's own note says it moved there ("build-line/build-queue").
pub(crate) const LINE_FOLDER: [&str; 2] = ["build-line", "build-queue"];

/* The runner's names for its own files, spelled as the runner spells them. */
const RUNNER_LOG: &str = "runner.log";
pub(crate) const QUEUE_IN: &str = "in";
const QUEUE_RUN: &str = "run";
const QUEUE_DONE: &str = "done";
const PAUSE_FLAG: &str = "PAUSE";
const DISK_FLOOR_FLAG: &str = "DISK_FLOOR";
const QUIET_FLAG: &str = "QUIET_WANTED";
const JOB_SUFFIX: &str = ".job";
/// A job a guard stood aside: it still waits, and the runner does not pick it
/// until the guard puts it back.
const HELD_SUFFIX: &str = ".job.hold";
const LOG_SUFFIX: &str = ".log";
const RC_SUFFIX: &str = ".rc";
/// The runner's script, `runner<n>.sh`, in the queue folder: the one place its
/// caps are decided, and so the one place this reads them from.
const RUNNER_SCRIPT_HEAD: &str = "runner";
const RUNNER_SCRIPT_TAIL: &str = ".sh";
/// How much of that script is read: a script of a few hundred lines.
const SCRIPT_BYTES: u64 = 64 * 1024;
/// How long past its cap a `RUN` with no `DONE` waits before the window lets
/// it go as lost: the runner kills at the cap and writes its line a few
/// seconds later, so a job this far over has no end line coming.
const LOST_AFTER_CAP_MS: i64 = 10 * 60_000;
/// A re-run keeps the kind of the job it repeats (`…-g5-window-실측-rerun`).
const RERUN_MARK: &str = "rerun";

/// The most of the runner's log one look reads — a first look at a long log,
/// or a look at one that grew far since the last: its last stretch holds the
/// past of every kind and the week the board can show.
const LOOK_BYTES: u64 = 4 * 1024 * 1024;
/// How many bytes before the offset a look remembers, to know the file it
/// reads on is still the one it read: a log cut and grown past the offset is
/// told by them.
const SEAM_BYTES: usize = 32;
/// `DISK_FLOOR` holds one number of GiB.
const FLOOR_BYTES: u64 = 64;
/// Finished jobs kept between looks, the newest last.
const KEPT_JOBS: usize = 3_000;
/// The most finished jobs one answer carries: the newest. The week's seven days
/// hold 362 to 837 on a busy machine, and what is left out is said
/// ([`JobsLeftOut`]): at 1,000 a changed beat of the busy week took 19 ms on the
/// low-end profile, over the 16 ms the window allows it, and at 400 it took 10 ms.
const SENT_JOBS: usize = 400;
const DAY_MS: i64 = 86_400_000;
/// How far back an answer reaches at most, and when the window names nothing.
const REACH_MS: i64 = 8 * DAY_MS;
const DEFAULT_REACH_MS: i64 = DAY_MS;
/// A log the window opens: its last bytes, and of those its last lines.
const LOG_TAIL_BYTES: u64 = 64 * 1024;
const LOG_TAIL_LINES: usize = 200;
/// How much of a job's file the runner's header reader can reach: the header
/// is the first lines of a short script.
const HEADER_BYTES: u64 = 8 * 1024;
/// Two lanes append to one runner log, so a line may carry a clock a little
/// ahead of the line below it — never a day.
const LINE_SLACK_MS: i64 = 5 * 60_000;
/// A log's last line reads its clock before it is written, so it is never
/// later than the file's own time; this much more is room for a coarse clock.
const LAST_WRITE_SLACK_MS: i64 = 60_000;

/* A job's state, one word each — the window has a sentence for every one. */
const PASSED: &str = "passed";
const FAILED: &str = "failed";
const CAPPED: &str = "capped";
const REFUSED: &str = "refused";
const LOST: &str = "lost";
const RUNNING: &str = "running";
const WAITING: &str = "waiting";
const HELD: &str = "held";

/* The runner's three classes, by the name's head: `0-`, none, `9-`. */
const FIRST_CLASS: &str = "first";
const PLAIN_CLASS: &str = "plain";
const LAST_CLASS: &str = "last";

/* ---- what the window asks, and what it is told -------------------------- */

/// A log the window asks to read: a build-line job by its name, or a build's
/// run by the build and the moment the run began. Only a job this reading
/// listed, or a run its build keeps, is opened.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LogAsk {
    Job(String),
    Build(BuildRun),
}

/// One run of one build: the build's id and the moment the run began.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BuildRun {
    pub(crate) id: String,
    pub(crate) started_ms: i64,
}

/// One look: the revision the window holds (a look where nothing moved
/// answers `unchanged` and nothing else), how far back the board shows, and
/// a log to read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Ask {
    pub(crate) known: Option<u64>,
    pub(crate) from_ms: Option<i64>,
    pub(crate) log: Option<LogAsk>,
}

/// The two folders read: the build line's queue — none with no repository
/// open — and the shared git folder of the repository whose builds are shown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Roots {
    pub(crate) line: Option<PathBuf>,
    pub(crate) builds: Option<PathBuf>,
    /// zo's home, whose machine-wide build line (`zo line`, 11-08) stands for a
    /// project with no coordinator queue. None reads no machine line.
    pub(crate) machine: Option<PathBuf>,
}

impl Roots {
    /// The queue under a project's workspace folder, beside the shared git
    /// folder its builds report under.
    pub(crate) fn of(workspaces: Option<&Path>, builds: Option<PathBuf>) -> Self {
        Self {
            line: workspaces.map(|root| {
                LINE_FOLDER
                    .iter()
                    .fold(root.to_path_buf(), |dir, part| dir.join(part))
            }),
            builds,
            machine: None,
        }
    }

    /// The same roots, reading the machine's line under zo's home `zo_home`.
    pub(crate) fn with_machine(self, zo_home: Option<PathBuf>) -> Self {
        Self {
            machine: zo_home,
            ..self
        }
    }
}

/// The schedule as the window draws it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Timeline {
    /// Moves when a file read here moved. The window asks again with it and is
    /// told `unchanged` — with nothing else — until something did.
    pub(crate) revision: u64,
    pub(crate) unchanged: bool,
    pub(crate) read_ms: i64,
    pub(crate) line: LineFacts,
    pub(crate) jobs: Vec<Job>,
    /// The finished jobs in the range this answer could not carry, sent only
    /// when there are any: the window says so instead of drawing a day short.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) jobs_left_out: Option<JobsLeftOut>,
    /// The project's build runs that ended in the range or have not ended,
    /// builds in the project's order and each build's newest run first.
    pub(crate) builds: Vec<Build>,
    /// The newest run that ended, sent only when the range holds none: the
    /// board's first line still names the last build (t-84799).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_build: Option<Build>,
    /// When the board's tasks landed and shipped, as git at the open
    /// repository says it, by task (t-71840, [`crate::artifact_flow::landings`]):
    /// the window ends a finished task's bar there before the times the desk
    /// carries. A task git says nothing of is not here.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) landings: BTreeMap<String, Landed>,
    /// What the build line ran for each task its jobs' `# task:` lines name
    /// (t-73910), over the jobs this answer sends. A task no job names is not
    /// here: its line time is unknown, never zero.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) task_lines: BTreeMap<String, TaskLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) log: Option<LogTail>,
}

/// The oldest finished jobs of a range past [`SENT_JOBS`]: how many, and the
/// moment the newest of them is older than — the oldest job the answer carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct JobsLeftOut {
    pub(crate) count: usize,
    pub(crate) before_ms: i64,
}

/// One task's time on the build line: the jobs that named it and ran to an
/// end, how many of them were red, and their run time — each job's whole,
/// never shared out among the tasks it named.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct TaskLine {
    pub(crate) jobs: u32,
    pub(crate) red: u32,
    pub(crate) secs: u64,
    /// The most tasks one of those jobs named: above 1, the time is a batch's.
    pub(crate) shared: u32,
}

/// The build line as a whole: the flags its runner obeys.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct LineFacts {
    /// The folder holds a queue (an `in/`): without one no job row stands.
    pub(crate) found: bool,
    /// `PAUSE`: the runner starts nothing, and what runs finishes.
    pub(crate) paused: bool,
    /// Why, in the coordinator's words on the last `PAUSE set` line.
    pub(crate) pause_note: Option<String>,
    pub(crate) disk_floor_gib: Option<u32>,
    /// The runner's last word on the disk was that it waits under the floor.
    pub(crate) disk_low: bool,
    /// `QUIET_WANTED`: lane 1 waits to start a measurement on a quiet
    /// machine, and lane 2 holds.
    pub(crate) quiet_wanted: bool,
    /// The runner's lanes its log named (`lane=` on a `RUN` or a `DONE`
    /// line), in order — none before it names one. The window places a
    /// waiting job's start on these.
    pub(crate) runner_lanes: Vec<u8>,
}

/// One build-line job — ended, running, waiting or held — and, running or
/// waiting, what it is expected to take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Job {
    pub(crate) name: String,
    pub(crate) who: String,
    /// `passed`, `failed`, `capped`, `refused`, `lost` (started, never ended,
    /// and no longer running), `running`, `waiting` or `held`.
    pub(crate) state: &'static str,
    /// The runner's class: `first` (`0-…`), `plain`, `last` (`9-…`).
    pub(crate) class: &'static str,
    pub(crate) lane: Option<u8>,
    pub(crate) cap_secs: u32,
    pub(crate) queued_ms: Option<i64>,
    pub(crate) start_ms: Option<i64>,
    pub(crate) end_ms: Option<i64>,
    pub(crate) secs: Option<u32>,
    /// The exit code, where the job ran to one.
    pub(crate) rc: Option<i32>,
    /// The `-g…` tail its kind repeats in every landing (`g1-build`).
    pub(crate) kind: Option<String>,
    /// The landing a gate job is part of (`l60`), and which part (`r`).
    pub(crate) gate: Option<String>,
    pub(crate) part: Option<String>,
    /// The header's `# what:`, while the job is in the queue.
    pub(crate) what: Option<String>,
    /// The tasks its header's `# task:` line names ([`tasks_of`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) tasks: Vec<String>,
    pub(crate) expected: Option<Expected>,
}

/// The end of a log the window opened.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct LogTail {
    pub(crate) job: Option<String>,
    pub(crate) build: Option<BuildRun>,
    pub(crate) lines: Vec<String>,
    /// Only the log's last lines are here.
    pub(crate) cut: bool,
    /// Nothing to read: no such job or run in this reading, or no file.
    pub(crate) missing: bool,
}

/* ---- the machine's line (`zo line`, 11-08) ------------------------------- */

/// The machine's slots a waiting job may be placed on: the two `zo build` and
/// `zo line` share by default, and every slot a job of the line ran in.
const MACHINE_SLOTS_DEFAULT: u8 = 2;

fn machine_lanes_of(jobs: &[zerocode_core::line::Job]) -> Vec<u8> {
    let mut lanes: Vec<u8> = (1..=MACHINE_SLOTS_DEFAULT)
        .chain(
            jobs.iter()
                .filter_map(|job| job.slot)
                .filter_map(|slot| u8::try_from(slot.saturating_add(1)).ok()),
        )
        .collect();
    lanes.sort_unstable();
    lanes.dedup();
    lanes
}

/// One job of the machine's line in the schedule's words: its slot is the
/// lane (counted from 1), a job taken out before it ran is not shown, and one
/// that ended before `from_ms` is out of the range.
fn machine_job(job: &zerocode_core::line::Job, from_ms: i64) -> Option<Job> {
    use zerocode_core::line::{Class, JobState};
    let state = match job.state {
        JobState::Waiting => "waiting",
        JobState::Running => "running",
        JobState::Passed => "passed",
        JobState::Failed | JobState::Stopped => "failed",
        JobState::Capped => "capped",
        JobState::Lost => "lost",
        JobState::Withdrawn => return None,
    };
    if job.ended_ms.is_some_and(|ended| ended < from_ms) {
        return None;
    }
    Some(Job {
        name: job.id.clone(),
        who: job.owner.clone(),
        state,
        class: match job.class {
            Class::First => "first",
            Class::Plain => "plain",
            Class::Last => "last",
        },
        lane: job
            .slot
            .and_then(|slot| u8::try_from(slot.saturating_add(1)).ok()),
        cap_secs: job.cap_secs,
        queued_ms: Some(job.queued_ms),
        start_ms: job.started_ms,
        end_ms: job.ended_ms,
        secs: job
            .started_ms
            .zip(job.ended_ms)
            .and_then(|(start, end)| u32::try_from((end - start).max(0) / 1000).ok()),
        rc: job.rc,
        kind: None,
        gate: None,
        part: None,
        what: Some(job.what.clone()).filter(|what| !what.is_empty()),
        tasks: job.task.iter().cloned().collect(),
        expected: None,
    })
}

/* ---- one look ------------------------------------------------------------ */

static BOOK: LazyLock<Mutex<Book>> = LazyLock::new(Mutex::default);

/// The open repository's workspace folder, as the window's workspace setting
/// places it. The setting is read off disk, so a five-second look does not
/// read it again: only a look that holds no revision, a repository that
/// changed, or a minute gone by does.
static WORKSPACES: Mutex<Option<Workspaces>> = Mutex::new(None);

/// How long a resolved workspace folder stands without a fresh look.
const SETTINGS_STAND_MS: i64 = 60_000;

#[derive(Debug)]
struct Workspaces {
    repo: Option<PathBuf>,
    resolved_ms: i64,
    folder: Option<PathBuf>,
}

/// The workspace folder of `repo`, resolved by `resolve` when `fresh`, when the
/// repository changed, or when the last answer is a minute old.
pub(crate) fn workspaces_of(
    repo: Option<&Path>,
    fresh: bool,
    now_ms: i64,
    resolve: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    let mut held = WORKSPACES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(known) = held.as_ref()
        && !fresh
        && known.repo.as_deref() == repo
        && now_ms - known.resolved_ms < SETTINGS_STAND_MS
    {
        return known.folder.clone();
    }
    let folder = resolve();
    *held = Some(Workspaces {
        repo: repo.map(Path::to_path_buf),
        resolved_ms: now_ms,
        folder: folder.clone(),
    });
    folder
}

/// Read the schedule as of `now_ms`, local clocks being `offset_secs` east of
/// UTC, with what git says of the board's tasks (`landings`, t-71840).
/// Blocking: the command calls it off the main thread.
pub(crate) fn read(
    roots: &Roots,
    ask: &Ask,
    landings: BTreeMap<String, Landed>,
    builds: History,
    now_ms: i64,
    offset_secs: i64,
) -> Timeline {
    let mut book = BOOK.lock().unwrap_or_else(PoisonError::into_inner);
    book.land(landings);
    book.read(roots, ask, builds, now_ms, offset_secs)
}

/// What one look keeps for the next: the runner log read so far and what it
/// told, the queued jobs' headers, and the revision.
#[derive(Debug, Default)]
struct Book {
    roots: Roots,
    runner: RunnerTail,
    headers: HashMap<String, (FileStamp, Header)>,
    script: ScriptCaps,
    floor: Option<(FileStamp, Option<u32>)>,
    /// What git said of the board's tasks at the last look, and its hash: a
    /// landing moves the revision as a file does (t-71840).
    landings: BTreeMap<String, Landed>,
    landed: u64,
    signature: u64,
    revision: u64,
    /// The `# task:` line of each ended job's file in `done/`, read once per
    /// version of it; no stamp while the file is not there yet.
    ended_tasks: BTreeMap<String, (Option<FileStamp>, Vec<String>)>,
    /// Runs between tying the ended jobs and signing what was tied, so a test
    /// can put a file into that gap (the review R5).
    #[cfg(test)]
    after_tie: Option<fn(&Roots)>,
}

impl Book {
    /// What git says of the board's tasks now, kept for the next look.
    fn land(&mut self, landings: BTreeMap<String, Landed>) {
        let mut hasher = DefaultHasher::new();
        landings.hash(&mut hasher);
        self.landed = hasher.finish();
        self.landings = landings;
    }

    fn read(
        &mut self,
        roots: &Roots,
        ask: &Ask,
        builds: History,
        now_ms: i64,
        offset_secs: i64,
    ) -> Timeline {
        if self.roots != *roots {
            *self = Self {
                roots: roots.clone(),
                revision: self.revision,
                landings: std::mem::take(&mut self.landings),
                landed: self.landed,
                ..Self::default()
            };
        }
        let queue = roots
            .line
            .as_deref()
            .map(|folder| look_at_queue(folder, &mut self.headers, &mut self.script));
        // The machine's line moves the answer too: its stamp is read without a record (`Line::stamp`).
        let machine = roots.machine.as_deref().map(zerocode_core::line::Line::at);
        let machine_stamp = machine.as_ref().map_or(0, zerocode_core::line::Line::stamp);
        let read_so_far =
            signature_of(queue.as_ref(), builds.revision, self.landed) ^ machine_stamp;
        // With the ended jobs' files as they stand: one that came after its
        // runner line, or was written again, moves the revision (the review R2,
        // P2-4).
        let signature = with_ties(read_so_far, self.ties_now());
        let files_changed = signature != self.signature || self.revision == 0;
        if files_changed {
            self.signature = signature;
            if let (Some(folder), Some(queue)) = (roots.line.as_deref(), queue.as_ref()) {
                self.runner.caps = queue.caps;
                self.runner
                    .follow(&folder.join(RUNNER_LOG), queue.runner_log, offset_secs);
                self.floor = queue.floor.map(|held| {
                    let value = match self.floor {
                        Some((before, value)) if before == held => value,
                        _ => read_floor(&folder.join(DISK_FLOOR_FLAG)),
                    };
                    (held, value)
                });
            }
        }
        // A missing run can cross its cap and grace while every file stays
        // quiet. Expire before the unchanged answer, and move the revision
        // only when a job actually changed (or a file did).
        let expired = queue.as_ref().is_some_and(|queue| {
            let running = queue.running.iter().map(|job| job.name.as_str()).collect();
            self.runner.expire(now_ms, &running)
        });
        if files_changed || expired {
            self.revision += 1;
        }
        let log = ask
            .log
            .as_ref()
            .map(|asked| self.log_tail(asked, queue.as_ref()));
        if ask.known == Some(self.revision) {
            return Timeline {
                revision: self.revision,
                unchanged: true,
                read_ms: now_ms,
                log,
                ..Timeline::default()
            };
        }
        let from_ms = ask
            .from_ms
            .unwrap_or(now_ms - DEFAULT_REACH_MS)
            .max(now_ms - REACH_MS);
        let mut paused = queue.as_ref().is_some_and(|look| look.paused);
        let mut jobs = self.jobs(queue.as_ref(), from_ms);
        self.tie_ended(&mut jobs);
        // A project with no coordinator queue shows its jobs on the machine's line (`zo line`, 11-08).
        let coordinator = queue.as_ref().is_some_and(|look| look.found);
        let machine_look = machine
            .as_ref()
            .filter(|_| !coordinator)
            .map(zerocode_core::line::Line::look);
        let project = roots
            .builds
            .as_deref()
            .map(|path| path.display().to_string());
        let mut machine_lanes: Vec<u8> = Vec::new();
        let mut machine_note = None;
        if let (Some(look), Some(project)) = (machine_look.as_ref(), project.as_deref()) {
            let ours: Vec<Job> = look
                .jobs
                .iter()
                .filter(|job| job.project == project)
                .filter_map(|job| machine_job(job, from_ms))
                .collect();
            if !ours.is_empty() || look.pause.is_some() {
                machine_lanes = machine_lanes_of(&look.jobs);
                paused |= look.pause.is_some();
                machine_note = look
                    .pause
                    .as_ref()
                    .map(|pause| pause.why.clone())
                    .filter(|why| !why.is_empty());
            }
            jobs.extend(ours);
        }
        #[cfg(test)]
        if let Some(act) = self.after_tie {
            act(&self.roots);
        }
        // What this answer tied is no change the next time: signed with the
        // stamps the tie was made from, never the disk's a moment later — a
        // file that came in between is a change the next look reports.
        self.signature = with_ties(read_so_far, self.ties_tied());
        // A run that has not ended stands whatever the range; one that ended
        // stands when it ended in it — by its record's last write. A range
        // holding no ended run still names the last one that ended.
        let open = |run: &Build| matches!(run.state, BuildState::Running | BuildState::Waiting);
        let mut runs = builds.runs;
        let last_build = (!runs
            .iter()
            .any(|run| !open(run) && run.updated_ms >= from_ms))
        .then(|| {
            runs.iter()
                .filter(|run| !open(run))
                .max_by_key(|run| run.updated_ms)
                .cloned()
        })
        .flatten();
        runs.retain(|run| open(run) || run.updated_ms >= from_ms);
        Timeline {
            revision: self.revision,
            unchanged: false,
            read_ms: now_ms,
            line: LineFacts {
                found: coordinator || !machine_lanes.is_empty(),
                paused,
                pause_note: if coordinator {
                    self.runner.pause_note.clone().filter(|_| paused)
                } else {
                    machine_note
                },
                disk_floor_gib: self.floor.and_then(|(_, value)| value),
                disk_low: self.runner.disk_low,
                quiet_wanted: queue.as_ref().is_some_and(|look| look.quiet_wanted),
                runner_lanes: if coordinator {
                    self.runner.lanes.iter().copied().collect()
                } else {
                    machine_lanes
                },
            },
            task_lines: task_lines(&jobs),
            jobs,
            jobs_left_out: self.jobs_left_out(from_ms),
            builds: runs,
            last_build,
            landings: self.landings.clone(),
            log,
        }
    }

    /// What [`Self::jobs`] could not carry of the finished jobs from `from_ms` on.
    fn jobs_left_out(&self, from_ms: i64) -> Option<JobsLeftOut> {
        let mut in_range = self
            .runner
            .finished
            .iter()
            .rev()
            .filter_map(|job| job.end_ms.or(job.start_ms))
            .filter(|at| *at >= from_ms);
        let before_ms = in_range.nth(SENT_JOBS - 1)?;
        let count = in_range.count();
        (count > 0).then_some(JobsLeftOut { count, before_ms })
    }

    /// The jobs the board draws from `from_ms` on: those that ended, those
    /// that started and never ended, what runs and what waits.
    fn jobs(&self, queue: Option<&QueueLook>, from_ms: i64) -> Vec<Job> {
        let mut jobs: Vec<Job> = self
            .runner
            .finished
            .iter()
            .rev()
            .filter(|job| job.end_ms.or(job.start_ms).is_some_and(|at| at >= from_ms))
            .take(SENT_JOBS)
            .cloned()
            .collect();
        jobs.reverse();
        let Some(queue) = queue else {
            return jobs;
        };
        let running: HashSet<&str> = queue.running.iter().map(|job| job.name.as_str()).collect();
        // Started, never ended, and no longer in `run/`: the runner stopped
        // under it — or its end line is a moment away, its exit code written.
        let mut unended: Vec<Job> = self
            .runner
            .open
            .iter()
            .filter(|(name, started)| {
                !running.contains(name.as_str()) && started.start_ms >= from_ms
            })
            .map(|(name, started)| {
                let ending = self.roots.line.as_deref().is_some_and(|folder| {
                    folder
                        .join(QUEUE_DONE)
                        .join(format!("{name}{RC_SUFFIX}"))
                        .exists()
                });
                Job {
                    state: if ending { RUNNING } else { LOST },
                    ..self.started_job(name, started)
                }
            })
            .collect();
        unended.sort_by_key(|job| job.start_ms);
        jobs.extend(unended);
        for queued in &queue.running {
            jobs.push(match self.runner.open.get(&queued.name) {
                Some(started) => Job {
                    what: queued.header.what.clone(),
                    tasks: queued.header.tasks.clone(),
                    ..self.started_job(&queued.name, started)
                },
                // In `run/` with no `RUN` line read: it started when the
                // runner moved it there.
                None => {
                    let cap_secs = queue.caps.cap_secs(queued.header.cap.as_deref());
                    let named = Job::named(&queued.name, queue.caps);
                    Job {
                        who: queued.header.who.clone().unwrap_or_default(),
                        state: RUNNING,
                        cap_secs,
                        start_ms: Some(queued.stamp.changed_ms),
                        what: queued.header.what.clone(),
                        tasks: queued.header.tasks.clone(),
                        expected: Some(self.runner.expected(named.kind.as_deref(), cap_secs)),
                        ..named
                    }
                }
            });
        }
        for queued in &queue.waiting {
            let cap_secs = queue.caps.cap_secs(queued.header.cap.as_deref());
            let named = Job::named(&queued.name, queue.caps);
            jobs.push(Job {
                who: queued.header.who.clone().unwrap_or_default(),
                state: if queued.held { HELD } else { WAITING },
                cap_secs,
                queued_ms: named.queued_ms.or(Some(queued.stamp.modified_ms)),
                what: queued.header.what.clone(),
                tasks: queued.header.tasks.clone(),
                expected: Some(self.runner.expected(named.kind.as_deref(), cap_secs)),
                ..named
            });
        }
        jobs
    }

    /// Ties each ended job to the tasks its file in `done/` names.
    fn tie_ended(&mut self, jobs: &mut [Job]) {
        let Some(done) = self.roots.line.as_ref().map(|line| line.join(QUEUE_DONE)) else {
            return;
        };
        let mut seen = HashSet::new();
        for job in jobs
            .iter_mut()
            .filter(|job| job.end_ms.is_some() && one_name(&job.name))
        {
            let path = done.join(format!("{}{JOB_SUFFIX}", job.name));
            let now = stamp(&path);
            seen.insert(job.name.clone());
            job.tasks = match self.ended_tasks.get(&job.name) {
                Some((before, tasks)) if *before == now => tasks.clone(),
                _ => {
                    let tasks = now.map(|_| read_header(&path).tasks).unwrap_or_default();
                    self.ended_tasks
                        .insert(job.name.clone(), (now, tasks.clone()));
                    tasks
                }
            };
        }
        self.ended_tasks.retain(|name, _| seen.contains(name));
    }

    /// The ended jobs' files in `done/` as they stand now: each one the last
    /// answer tied, or found missing, by name and the stamp the disk shows.
    fn ties_now(&self) -> u64 {
        let done = self.roots.line.as_ref().map(|line| line.join(QUEUE_DONE));
        hash_ties(self.ended_tasks.keys().map(|name| {
            let on_disk = done
                .as_ref()
                .and_then(|done| stamp(&done.join(format!("{name}{JOB_SUFFIX}"))));
            (name, on_disk)
        }))
    }

    /// The same ties by the stamps the last tie was made from, with no disk.
    fn ties_tied(&self) -> u64 {
        hash_ties(
            self.ended_tasks
                .iter()
                .map(|(name, (tied_from, _))| (name, *tied_from)),
        )
    }

    /// A job the runner's log says started, still without an end.
    fn started_job(&self, name: &str, started: &Started) -> Job {
        let named = Job::named(name, self.runner.caps);
        Job {
            who: started.who.clone(),
            state: RUNNING,
            lane: started.lane,
            cap_secs: started.cap_secs,
            start_ms: Some(started.start_ms),
            expected: Some(
                self.runner
                    .expected(named.kind.as_deref(), started.cap_secs),
            ),
            ..named
        }
    }

    /// The end of a log, for a job this reading listed or a run its build keeps
    /// — never a path the window spelled.
    fn log_tail(&self, asked: &LogAsk, queue: Option<&QueueLook>) -> LogTail {
        match asked {
            LogAsk::Job(name) => {
                let listed = self.runner.open.contains_key(name)
                    || self.runner.finished.iter().any(|job| job.name == *name)
                    || queue.is_some_and(|look| {
                        look.waiting
                            .iter()
                            .chain(&look.running)
                            .any(|job| job.name == *name)
                    });
                let path = self
                    .roots
                    .line
                    .as_ref()
                    .filter(|_| listed && one_name(name))
                    .map(|folder| folder.join(QUEUE_DONE).join(format!("{name}{LOG_SUFFIX}")));
                LogTail {
                    job: Some(name.clone()),
                    ..tail_of(path.as_deref())
                }
            }
            LogAsk::Build(run) => LogTail {
                build: Some(run.clone()),
                ..tail_of(
                    self.roots
                        .builds
                        .as_deref()
                        .and_then(|shared| {
                            zerocode_core::builds::log_of(shared, &run.id, run.started_ms)
                        })
                        .as_deref(),
                )
            },
        }
    }
}

/// A name that is one file's name and nothing more.
fn one_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\'])
}

/// The last lines of a log: its last bytes, the line they cut through dropped.
fn tail_of(path: Option<&Path>) -> LogTail {
    let Some((bytes, cut)) =
        path.and_then(|path| crate::plain_read::read_plain_tail(path, LOG_TAIL_BYTES))
    else {
        return LogTail {
            missing: true,
            ..LogTail::default()
        };
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if cut && !lines.is_empty() {
        lines.remove(0);
    }
    let first = lines.len().saturating_sub(LOG_TAIL_LINES);
    LogTail {
        lines: lines[first..]
            .iter()
            .map(|line| (*line).to_string())
            .collect(),
        cut: cut || first > 0,
        ..LogTail::default()
    }
}

/* ---- the queue folder ----------------------------------------------------- */

/// The `# key:` lines the runner reads off a job's head.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Header {
    who: Option<String>,
    what: Option<String>,
    cap: Option<String>,
    tasks: Vec<String>,
}

/// The tasks a job's `# task:` line names (t-73910): ledger ids — `t-` and
/// digits — split on commas. Any other piece is dropped, never guessed at;
/// `# what:` is the person's words and is not read for one.
fn tasks_of(said: Option<&str>) -> Vec<String> {
    let mut tasks: Vec<String> = said
        .into_iter()
        .flat_map(|line| line.split(','))
        .map(str::trim)
        .filter(|piece| {
            piece.strip_prefix("t-").is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .map(str::to_string)
        .collect();
    tasks.sort();
    tasks.dedup();
    tasks
}

/// The ended jobs' names with the stamps of their files, as one number.
fn hash_ties<'a>(ties: impl Iterator<Item = (&'a String, Option<FileStamp>)>) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (name, stamp) in ties {
        name.hash(&mut hasher);
        stamp.hash(&mut hasher);
    }
    hasher.finish()
}

/// A reading's signature with the ended jobs' files' ([`Book::ties_now`]).
fn with_ties(read: u64, ties: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    (read, ties).hash(&mut hasher);
    hasher.finish()
}

/// Each task's [`TaskLine`] over `jobs`: a job that ran to an end counts its
/// whole run time to every task it names.
fn task_lines(jobs: &[Job]) -> BTreeMap<String, TaskLine> {
    let mut lines: BTreeMap<String, TaskLine> = BTreeMap::new();
    for job in jobs {
        let Some(secs) = job.secs else {
            continue;
        };
        let named = u32::try_from(job.tasks.len()).unwrap_or(u32::MAX);
        for task in &job.tasks {
            let line = lines.entry(task.clone()).or_default();
            line.jobs += 1;
            line.red += u32::from(job.state != PASSED);
            line.secs += u64::from(secs);
            line.shared = line.shared.max(named);
        }
    }
    lines
}

/// A job file in `in/` or `run/`.
#[derive(Debug, Clone)]
struct Queued {
    name: String,
    held: bool,
    stamp: FileStamp,
    header: Header,
}

/// The queue this moment: what waits in `in/` (a guard's held job too, a job
/// still being written not), what runs in `run/`, the flags and the runner
/// log's stamp. A header is read once per version of its file.
#[derive(Debug, Default)]
struct QueueLook {
    found: bool,
    waiting: Vec<Queued>,
    running: Vec<Queued>,
    paused: bool,
    floor: Option<FileStamp>,
    quiet_wanted: bool,
    runner_log: Option<FileStamp>,
    /// What the runner caps a job at, as its script says.
    caps: Caps,
}

/// What the runner caps a job at: the number it gives a job that names none,
/// and the most it grants. The runner's script decides them, so they are read
/// off its text — not copied here to drift from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Caps {
    default_secs: u32,
    most_secs: u32,
}

impl Default for Caps {
    fn default() -> Self {
        Self::LAST_KNOWN
    }
}

impl Caps {
    /// The runner's numbers as they stood when this was written, for a queue
    /// folder with no script of its own to read them off.
    const LAST_KNOWN: Caps = Caps {
        default_secs: 900,
        most_secs: 1800,
    };

    /// The two numbers in a runner script's text, the way its `cap` lines say
    /// them: `case "$cap" in ''|*[!0-9]*) cap=<default>;; esac; [ $cap -gt
    /// <most> ] && cap=<most>`. None where either is not there.
    fn of_script(text: &str) -> Option<Self> {
        let number = |rest: &str| -> Option<u32> {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            rest[..digits].parse().ok()
        };
        let default_secs = text.lines().find_map(|line| {
            let (_, rest) = line.split_once("*[!0-9]*) cap=")?;
            number(rest)
        })?;
        let most_secs = text.lines().find_map(|line| {
            let (_, rest) = line.split_once("[ $cap -gt ")?;
            let (_, rest) = rest.split_once("&& cap=")?;
            number(rest)
        })?;
        Some(Self {
            default_secs,
            most_secs,
        })
    }

    /// The cap as the runner reads it: digits, or its default — never above its most.
    fn cap_secs(self, said: Option<&str>) -> u32 {
        said.filter(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|word| word.parse::<u64>().ok())
            .map_or(self.default_secs, |secs| {
                secs.min(u64::from(self.most_secs)) as u32
            })
    }
}

/// The caps last read off the runner's script, and what that read stood on:
/// the queue folder's and the script's stamps. A look where neither moved
/// reads nothing; the folder is listed again only when it changed.
#[derive(Debug, Default)]
struct ScriptCaps {
    folder: Option<FileStamp>,
    script: Option<(PathBuf, Option<FileStamp>)>,
    caps: Caps,
    looked: bool,
}

impl ScriptCaps {
    fn look(&mut self, folder: &Path) -> Caps {
        let folder_stamp = stamp(folder);
        let script_stamp = self.script.as_ref().map(|(path, _)| stamp_plain(path));
        let held = self.script.as_ref().map(|(_, held)| *held);
        if self.looked && self.folder == folder_stamp && script_stamp == held {
            return self.caps;
        }
        self.looked = true;
        self.folder = folder_stamp;
        let path = runner_script(folder);
        let text = path
            .as_deref()
            .and_then(|path| crate::plain_read::read_plain(path, SCRIPT_BYTES))
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        self.caps = text
            .as_deref()
            .and_then(Caps::of_script)
            .unwrap_or(Caps::LAST_KNOWN);
        self.script = path.map(|path| {
            let held = stamp_plain(&path);
            (path, held)
        });
        self.caps
    }
}

/// The runner's script in the queue folder: `runner<n>.sh` — the highest
/// number, a copy or a backup beside it (`runner10.sh.bak-…`) not counted.
fn runner_script(folder: &Path) -> Option<PathBuf> {
    std::fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let number: u32 = name
                .strip_prefix(RUNNER_SCRIPT_HEAD)?
                .strip_suffix(RUNNER_SCRIPT_TAIL)?
                .parse()
                .ok()?;
            entry
                .metadata()
                .ok()
                .filter(Metadata::is_file)
                .map(|_| (number, entry.path()))
        })
        .max_by_key(|(number, _)| *number)
        .map(|(_, path)| path)
}

fn look_at_queue(
    folder: &Path,
    headers: &mut HashMap<String, (FileStamp, Header)>,
    script: &mut ScriptCaps,
) -> QueueLook {
    let waiting_dir = folder.join(QUEUE_IN);
    if !waiting_dir.is_dir() {
        headers.clear();
        return QueueLook::default();
    }
    let mut seen = HashSet::new();
    let waiting = queued_files(&waiting_dir, QUEUE_IN, headers, &mut seen);
    let running = queued_files(&folder.join(QUEUE_RUN), QUEUE_RUN, headers, &mut seen)
        .into_iter()
        .filter(|job| !job.held)
        .collect();
    headers.retain(|key, _| seen.contains(key));
    QueueLook {
        found: true,
        waiting,
        running,
        paused: folder.join(PAUSE_FLAG).exists(),
        floor: stamp_plain(&folder.join(DISK_FLOOR_FLAG)),
        quiet_wanted: folder.join(QUIET_FLAG).exists(),
        runner_log: stamp_plain(&folder.join(RUNNER_LOG)),
        caps: script.look(folder),
    }
}

/// The job files of one queue folder, by name — the order the runner lists
/// them in. A name that starts with a dot is a job still being written.
fn queued_files(
    dir: &Path,
    place: &str,
    headers: &mut HashMap<String, (FileStamp, Header)>,
    seen: &mut HashSet<String>,
) -> Vec<Queued> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut queued = Vec::new();
    for entry in listing.filter_map(Result::ok) {
        let Ok(file) = entry.file_name().into_string() else {
            continue;
        };
        if file.starts_with('.') {
            continue;
        }
        let (name, held) = match file.strip_suffix(HELD_SUFFIX) {
            Some(name) => (name.to_string(), true),
            None => match file.strip_suffix(JOB_SUFFIX) {
                Some(name) => (name.to_string(), false),
                None => continue,
            },
        };
        let Some(meta) = entry.metadata().ok().filter(Metadata::is_file) else {
            continue;
        };
        let stamp = stamp_of(&meta);
        let key = format!("{place}/{file}");
        let header = match headers.get(&key) {
            Some((before, header)) if *before == stamp => header.clone(),
            _ => {
                let header = read_header(&entry.path());
                headers.insert(key.clone(), (stamp, header.clone()));
                header
            }
        };
        seen.insert(key);
        queued.push(Queued {
            name,
            held,
            stamp,
            header,
        });
    }
    queued.sort_by(|a, b| a.name.cmp(&b.name));
    queued
}

/// A job's header, as the runner reads it: the first `# <key>:` line of each.
fn read_header(path: &Path) -> Header {
    let mut bytes = Vec::new();
    let read = crate::plain_read::open_plain(path)
        .map(|file| file.take(HEADER_BYTES).read_to_end(&mut bytes));
    if !matches!(read, Some(Ok(_))) {
        return Header::default();
    }
    let text = String::from_utf8_lossy(&bytes);
    let field = |key: &str| {
        text.lines().find_map(|line| {
            line.strip_prefix("# ")?
                .strip_prefix(key)?
                .strip_prefix(':')
                .map(|value| value.trim().to_string())
        })
    };
    Header {
        who: field("who"),
        what: field("what"),
        cap: field("cap"),
        tasks: tasks_of(field("task").as_deref()),
    }
}

/// The disk floor the runner reads (`DISK_FLOOR`, whole GiB).
fn read_floor(path: &Path) -> Option<u32> {
    let bytes = crate::plain_read::read_plain(path, FLOOR_BYTES)?;
    std::str::from_utf8(&bytes).ok()?.trim().parse().ok()
}

/// What a look compares with the last: every stamp, listing and flag it read.
/// The same signature is the same reading.
fn signature_of(queue: Option<&QueueLook>, builds: u64, landed: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    if let Some(queue) = queue {
        queue.found.hash(&mut hasher);
        queue.runner_log.hash(&mut hasher);
        for place in [&queue.waiting, &queue.running] {
            place.len().hash(&mut hasher);
            for job in place {
                job.name.hash(&mut hasher);
                job.held.hash(&mut hasher);
                job.stamp.hash(&mut hasher);
            }
        }
        queue.paused.hash(&mut hasher);
        queue.floor.hash(&mut hasher);
        queue.quiet_wanted.hash(&mut hasher);
        queue.caps.hash(&mut hasher);
    }
    builds.hash(&mut hasher);
    landed.hash(&mut hasher);
    hasher.finish()
}

/* ---- the runner's log --------------------------------------------------------- */

/// A line of the runner's log, as the runner writes it. Every other line —
/// the orchestration's echo, tmux, a handoff, a message id — is `Other`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Said {
    Run {
        name: String,
        who: String,
        lane: Option<u8>,
        /// The `cap=` the line names: none on a line that names none, which
        /// the runner's default caps.
        cap_secs: Option<u32>,
        clock: u32,
    },
    Done {
        name: String,
        ending: Ending,
        secs: u32,
        lane: Option<u8>,
        clock: u32,
    },
    Refused {
        name: String,
        clock: u32,
    },
    Pause {
        set: bool,
        note: String,
        clock: u32,
    },
    Disk {
        low: bool,
        clock: u32,
    },
    Other,
}

impl Said {
    /// The clock the runner stamped the line with — none on a line not its own.
    fn clock(&self) -> Option<u32> {
        match self {
            Said::Run { clock, .. }
            | Said::Done { clock, .. }
            | Said::Refused { clock, .. }
            | Said::Pause { clock, .. }
            | Said::Disk { clock, .. } => Some(*clock),
            Said::Other => None,
        }
    }
}

/// A `DONE` line's `rc=`: a code, the cap's kill, or a word nobody wrote down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    Code(i32),
    Capped,
    Other,
}

fn said_of(text: &str) -> Said {
    let said = match text.split_whitespace().next() {
        Some("RUN") => run_line(text),
        Some("DONE") => done_line(text),
        Some("REFUSED") => refused_line(text),
        Some("PAUSE") => pause_line(text),
        Some("DISK") => disk_line(text),
        _ => None,
    };
    said.unwrap_or(Said::Other)
}

/// `RUN <name> who=<who> lane=<n> cap=<n>s HH:MM:SS`
fn run_line(text: &str) -> Option<Said> {
    let mut words = text.split_whitespace().skip(1);
    let name = words.next()?.to_string();
    let (mut who, mut lane, mut cap, mut clock) = (String::new(), None, None, None);
    for word in words {
        if let Some(value) = word.strip_prefix("who=") {
            who = value.to_string();
        } else if let Some(value) = word.strip_prefix("lane=") {
            lane = value.parse().ok();
        } else if let Some(value) = word.strip_prefix("cap=") {
            cap = value.strip_suffix('s').and_then(|secs| secs.parse().ok());
        } else if clock.is_none() {
            clock = clock_secs(word);
        }
    }
    Some(Said::Run {
        name,
        who,
        lane,
        cap_secs: cap,
        clock: clock?,
    })
}

/// `DONE <name> rc=<n|capped(<n>s)> <secs>s lane=<n> HH:MM:SS load=<n>`
fn done_line(text: &str) -> Option<Said> {
    let mut words = text.split_whitespace().skip(1);
    let name = words.next()?.to_string();
    let ending = ending_of(words.next()?.strip_prefix("rc=")?);
    let secs = words.next()?.strip_suffix('s')?.parse().ok()?;
    let (mut lane, mut clock) = (None, None);
    for word in words {
        if let Some(value) = word.strip_prefix("lane=") {
            lane = value.parse().ok();
        } else if clock.is_none() {
            clock = clock_secs(word);
        }
    }
    Some(Said::Done {
        name,
        ending,
        secs,
        lane,
        clock: clock?,
    })
}

fn ending_of(rc: &str) -> Ending {
    if rc.starts_with("capped") {
        return Ending::Capped;
    }
    rc.parse().map_or(Ending::Other, Ending::Code)
}

/// `REFUSED <name>: who or cwd missing (…) HH:MM:SS`
fn refused_line(text: &str) -> Option<Said> {
    let mut words = text.split_whitespace();
    words.next();
    let name = words.next()?.trim_end_matches(':').to_string();
    let clock = clock_secs(words.next_back()?)?;
    Some(Said::Refused { name, clock })
}

/// `PAUSE set HH:MM:SS by <who>: <why>` and `PAUSE lifted HH:MM:SS by <who>: <why>`
fn pause_line(text: &str) -> Option<Said> {
    let mut words = text.split_whitespace().skip(1);
    let set = match words.next()? {
        "set" => true,
        "lifted" => false,
        _ => return None,
    };
    let clock = clock_secs(words.next()?)?;
    let note = text.split_once(": ").map_or("", |(_, note)| note).trim();
    Some(Said::Pause {
        set,
        note: note.to_string(),
        clock,
    })
}

/// `DISK LOW <n>G < <n>G — lane <n> waits HH:MM:SS` and `DISK OK <n>G lane <n> HH:MM:SS`
fn disk_line(text: &str) -> Option<Said> {
    let mut words = text.split_whitespace();
    words.next();
    let low = match words.next()? {
        "LOW" => true,
        "OK" => false,
        _ => return None,
    };
    let clock = clock_secs(words.next_back()?)?;
    Some(Said::Disk { low, clock })
}

/// `HH:MM:SS` as seconds after local midnight.
fn clock_secs(word: &str) -> Option<u32> {
    let mut parts = word.split(':');
    let fields = [parts.next()?, parts.next()?, parts.next()?];
    if parts.next().is_some()
        || fields
            .iter()
            .any(|field| field.len() != 2 || !field.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let [hours, minutes, seconds] = fields.map(|field| field.parse::<u32>().unwrap_or(u32::MAX));
    (hours < 24 && minutes < 60 && seconds < 60).then_some(hours * 3600 + minutes * 60 + seconds)
}

/// The last moment at or before `ceiling_ms` (to the second) that a clock
/// `offset_secs` east of UTC reads `clock`.
fn clock_at_or_before(ceiling_ms: i64, clock: u32, offset_secs: i64) -> i64 {
    let shift = offset_secs * 1000;
    let local = (ceiling_ms + shift).div_euclid(1000) * 1000;
    let mut at = local.div_euclid(DAY_MS) * DAY_MS + i64::from(clock) * 1000;
    if at > local {
        at -= DAY_MS;
    }
    at - shift
}

/// When each line of a log was written, from the clocks its lines carry and
/// the file's last write: the last line at or before that write, each line
/// above at or before the one below it (`slack_ms` later where two writers
/// share the log) — so a run past midnight counts the day back. A job's name
/// says when it was queued, but not reliably: a worker may name its job after
/// a moment still to come, so the log alone dates its lines. A silence of a
/// day or more inside what one look reads would shift the lines above it.
fn dated_back(clocks: &[u32], last_write_ms: i64, slack_ms: i64, offset_secs: i64) -> Vec<i64> {
    let mut times = vec![0_i64; clocks.len()];
    let mut ceiling = last_write_ms + LAST_WRITE_SLACK_MS;
    for (index, clock) in clocks.iter().enumerate().rev() {
        let at = clock_at_or_before(ceiling, *clock, offset_secs);
        times[index] = at;
        ceiling = at + slack_ms;
    }
    times
}

/// What a job's name says, the coordinator spelling it
/// `[0-|9-]<epoch>-<who>-<rest>` and a gate's rest `l<NN><part>-g<step>-<what>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NameParts {
    class: &'static str,
    queued_ms: Option<i64>,
    kind: Option<String>,
    gate: Option<String>,
    part: Option<String>,
}

fn name_parts(name: &str) -> NameParts {
    let tokens: Vec<&str> = name.split('-').collect();
    let (class, mut at) = match tokens.first() {
        Some(&"0") => (FIRST_CLASS, 1),
        Some(&"9") => (LAST_CLASS, 1),
        _ => (PLAIN_CLASS, 0),
    };
    let queued_ms = tokens
        .get(at)
        .filter(|token| is_epoch(token))
        .and_then(|token| token.parse::<i64>().ok())
        .map(|secs| secs * 1000);
    if queued_ms.is_some() {
        at += 1;
    }
    let mut parts = NameParts {
        class,
        queued_ms,
        ..NameParts::default()
    };
    let step = tokens
        .iter()
        .enumerate()
        .skip(at)
        .find(|(index, token)| is_step(token) && index + 1 < tokens.len())
        .map(|(index, _)| index);
    if let Some(step) = step {
        let mut kind = tokens[step..].to_vec();
        if kind.len() > 2 && kind.last() == Some(&RERUN_MARK) {
            kind.pop();
        }
        parts.kind = Some(kind.join("-"));
        if step > at
            && let Some((gate, part)) = landing(tokens[step - 1])
        {
            parts.gate = Some(gate);
            parts.part = Some(part);
        }
    }
    parts
}

fn is_epoch(token: &str) -> bool {
    (9..=11).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_digit())
}

/// A gate's step: `g1`, `g3b2`, `g5b`.
fn is_step(token: &str) -> bool {
    let mut bytes = token.bytes();
    bytes.next() == Some(b'g')
        && bytes.next().is_some_and(|byte| byte.is_ascii_digit())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// A landing's token: `l60r` is landing `l60`, part `r`.
fn landing(token: &str) -> Option<(String, String)> {
    let rest = token.strip_prefix('l')?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let (number, part) = rest.split_at(digits);
    (digits > 0 && part.bytes().all(|byte| byte.is_ascii_lowercase()))
        .then(|| (format!("l{number}"), part.to_string()))
}

impl Job {
    /// A job as its name alone tells it.
    fn named(name: &str, caps: Caps) -> Self {
        let parts = name_parts(name);
        Self {
            name: name.to_string(),
            who: String::new(),
            state: WAITING,
            class: parts.class,
            lane: None,
            cap_secs: caps.default_secs,
            queued_ms: parts.queued_ms,
            start_ms: None,
            end_ms: None,
            secs: None,
            rc: None,
            kind: parts.kind,
            gate: parts.gate,
            part: parts.part,
            what: None,
            tasks: Vec::new(),
            expected: None,
        }
    }
}

/// A job the log says started: by whom, on which lane, under which cap, when.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Started {
    who: String,
    lane: Option<u8>,
    cap_secs: u32,
    start_ms: i64,
}

/// The runner's log read so far, and what it told.
#[derive(Debug, Default)]
struct RunnerTail {
    /// The file read: a runner log replaced by another (a new runner, a
    /// rotation) is read again from its first byte.
    file: Option<(u64, u64)>,
    /// What the file stood as at the last look: a look at the same stamp reads
    /// nothing.
    seen: Option<FileStamp>,
    offset: u64,
    /// The bytes just before the offset, as read: the file at the offset must
    /// still have them before it, or it is not the file that was read.
    seam: Vec<u8>,
    /// A look that starts mid-file drops the line it cut.
    cut_first: bool,
    /// What the runner caps a job at (the queue's script says).
    caps: Caps,
    /// Started and not ended, by name.
    open: HashMap<String, Started>,
    finished: VecDeque<Job>,
    /// A kind's last passing runs, oldest first: the job (a log read again
    /// counts a run once) and its seconds. A runner log that is replaced or
    /// rotated starts over; what its kinds took does not.
    past: HashMap<String, VecDeque<(String, u32)>>,
    pause_note: Option<String>,
    disk_low: bool,
    /// Every lane a `RUN` or a `DONE` line named.
    lanes: BTreeSet<u8>,
}

impl RunnerTail {
    /// Read what the log gained since the last look, its lines dated back from
    /// the file's last write ([`dated_back`]). A line still being written is
    /// read whole on the next look. A gain past [`LOOK_BYTES`] is read by its
    /// last stretch; a file that is another one (replaced, cut shorter, or
    /// written over where the offset was) is read again from its first byte.
    fn follow(&mut self, path: &Path, stamp: Option<FileStamp>, offset_secs: i64) {
        let Some(stamp) = stamp else {
            self.start_over(None);
            return;
        };
        if self.seen == Some(stamp) {
            return;
        }
        if self.file != Some(stamp.file) || stamp.len < self.offset {
            self.start_over(Some(stamp.file));
        }
        let Some(mut log) = crate::plain_read::open_plain(path) else {
            return;
        };
        if self.offset > 0 && !self.seam_holds(&mut log) {
            self.start_over(Some(stamp.file));
        }
        self.seen = Some(stamp);
        if stamp.len <= self.offset {
            return;
        }
        if stamp.len - self.offset > LOOK_BYTES {
            self.offset = stamp.len - LOOK_BYTES;
            self.cut_first = true;
            self.seam.clear();
        }
        let mut bytes = Vec::new();
        if log.seek(SeekFrom::Start(self.offset)).is_err()
            || log
                .take(stamp.len - self.offset)
                .read_to_end(&mut bytes)
                .is_err()
        {
            self.seen = None;
            return;
        }
        let Some(last) = bytes.iter().rposition(|byte| *byte == b'\n') else {
            return;
        };
        self.offset += last as u64 + 1;
        self.seam = bytes[..=last][last + 1 - (last + 1).min(SEAM_BYTES)..].to_vec();
        let mut lines = bytes[..last].split(|byte| *byte == b'\n');
        if std::mem::take(&mut self.cut_first) {
            lines.next();
        }
        // A line that is none of the runner's own (the orchestration's echo,
        // tmux, a handoff, a message id) tells nothing and dates nothing.
        let said: Vec<Said> = lines
            .map(|line| said_of(&String::from_utf8_lossy(line)))
            .filter(|said| said.clock().is_some())
            .collect();
        let clocks: Vec<u32> = said.iter().filter_map(Said::clock).collect();
        let times = dated_back(&clocks, stamp.modified_ms, LINE_SLACK_MS, offset_secs);
        for (said, at_ms) in said.into_iter().zip(times) {
            self.heard(said, at_ms);
        }
    }

    /// The bytes just before the offset are the ones read there.
    fn seam_holds(&self, log: &mut File) -> bool {
        let Some(from) = self.offset.checked_sub(self.seam.len() as u64) else {
            return false;
        };
        let mut found = vec![0_u8; self.seam.len()];
        log.seek(SeekFrom::Start(from)).is_ok()
            && log.read_exact(&mut found).is_ok()
            && found == self.seam
    }

    /// Read the log again from its first byte: what it told is dropped, except
    /// what the kinds took — a rotation does not send every estimate back to
    /// its cap — and the caps in force.
    fn start_over(&mut self, file: Option<(u64, u64)>) {
        *self = Self {
            file,
            caps: self.caps,
            past: std::mem::take(&mut self.past),
            ..Self::default()
        };
    }

    /// Let go of the runs that have no end line coming: started, never ended,
    /// not in `run/` (the runner's own to end), and [`LOST_AFTER_CAP_MS`] past
    /// their cap. They stay on the board, lost, among the finished.
    fn expire(&mut self, now_ms: i64, running: &HashSet<&str>) -> bool {
        let stale: Vec<String> = self
            .open
            .iter()
            .filter(|(name, started)| {
                !running.contains(name.as_str())
                    && now_ms - started.start_ms
                        > i64::from(started.cap_secs) * 1000 + LOST_AFTER_CAP_MS
            })
            .map(|(name, _)| name.clone())
            .collect();
        let changed = !stale.is_empty();
        for name in stale {
            let Some(started) = self.open.remove(&name) else {
                continue;
            };
            let named = Job::named(&name, self.caps);
            self.keep(Job {
                who: started.who,
                state: LOST,
                lane: started.lane,
                cap_secs: started.cap_secs,
                start_ms: Some(started.start_ms),
                ..named
            });
        }
        changed
    }

    /// One of the runner's own lines, written at `at_ms`.
    fn heard(&mut self, said: Said, at_ms: i64) {
        match said {
            Said::Run {
                name,
                who,
                lane,
                cap_secs,
                ..
            } => {
                self.lanes.extend(lane);
                self.open.insert(
                    name,
                    Started {
                        who,
                        lane,
                        cap_secs: cap_secs.unwrap_or(self.caps.default_secs),
                        start_ms: at_ms,
                    },
                );
            }
            Said::Done {
                name,
                ending,
                secs,
                lane,
                ..
            } => {
                self.lanes.extend(lane);
                let started = self.open.remove(&name);
                if started.is_none() {
                    // An end line that comes after the run was let go as lost.
                    self.finished
                        .retain(|job| !(job.name == name && job.state == LOST));
                }
                // Its start and its count date the end to the second; a job
                // that started before what this look read ends at its own line.
                let end_ms = started
                    .as_ref()
                    .map_or(at_ms, |started| started.start_ms + i64::from(secs) * 1000);
                let (state, rc) = match ending {
                    Ending::Code(0) => (PASSED, Some(0)),
                    Ending::Code(code) => (FAILED, Some(code)),
                    Ending::Capped => (CAPPED, None),
                    Ending::Other => (FAILED, None),
                };
                let named = Job::named(&name, self.caps);
                if state == PASSED
                    && let Some(kind) = &named.kind
                {
                    let past = self.past.entry(kind.clone()).or_default();
                    if !past.iter().any(|(job, _)| *job == name) {
                        past.push_back((name.clone(), secs));
                    }
                    if past.len() > SAMPLES {
                        past.pop_front();
                    }
                }
                let job = match started {
                    Some(started) => Job {
                        who: started.who,
                        lane: lane.or(started.lane),
                        cap_secs: started.cap_secs,
                        start_ms: Some(started.start_ms),
                        ..named
                    },
                    None => Job {
                        lane,
                        start_ms: Some(end_ms - i64::from(secs) * 1000),
                        ..named
                    },
                };
                self.keep(Job {
                    state,
                    end_ms: Some(end_ms),
                    secs: Some(secs),
                    rc,
                    ..job
                });
            }
            Said::Refused { name, .. } => self.keep(Job {
                state: REFUSED,
                start_ms: Some(at_ms),
                end_ms: Some(at_ms),
                ..Job::named(&name, self.caps)
            }),
            Said::Pause { set, note, .. } => self.pause_note = set.then_some(note),
            Said::Disk { low, .. } => self.disk_low = low,
            Said::Other => {}
        }
    }

    fn keep(&mut self, job: Job) {
        self.finished.push_back(job);
        if self.finished.len() > KEPT_JOBS {
            self.finished.pop_front();
        }
    }

    /// What a job of `kind` is expected to take: the median of the kind's last
    /// passing runs, never above the cap — or the cap, with no such past.
    fn expected(&self, kind: Option<&str>, cap_secs: u32) -> Expected {
        let past: Vec<u32> = kind
            .and_then(|kind| self.past.get(kind))
            .map(|held| held.iter().map(|(_, secs)| *secs).collect())
            .unwrap_or_default();
        median(&past).map_or(
            Expected {
                secs: cap_secs,
                samples: 0,
            },
            |secs| Expected {
                secs: secs.min(cap_secs),
                samples: past.len() as u8,
            },
        )
    }
}
