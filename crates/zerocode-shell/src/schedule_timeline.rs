//! The task board's schedule (t-67482): the build line's jobs, the
//! integration gates among them and the release lanes on one time axis — what
//! ran and when, what runs now and what waits, and how long each running or
//! waiting one is expected to take.
//!
//! A reader of two other processes' files, and only a reader. The build line
//! is the coordinator's runner: a queue folder whose `in/` holds what waits,
//! `run/` what runs and `done/` what ended (and every job's log, from its
//! first line), with a `runner.log` it appends a `RUN` and a `DONE` line to
//! per job. The release lane is `tools/release/lane.sh`: one
//! `lane-<sha8>.log` per sha in the release folder's `out/`, and its
//! `status.json`. Nothing here writes, moves, renames or locks a file of
//! either, and nothing here opens the ledger — a task's own times ride the
//! desk's reading (`desk::TaskTimes`).
//!
//! What a thing will take is read off the past alone: the median of the last
//! five runs of the same name that passed — a lane's stage by its name (a
//! stage of a lane that ended green counts whatever its own code), a build-line
//! job by the `-g…` tail its kind repeats in every landing. A job with no such
//! past is bounded by its cap, and says so. Where on the clock a
//! waiting job will start, and how much of a running one is left, is the
//! window's to place (`ui/shell-board-schedule.js`): that moves with the
//! time, and this answer moves only when a file does.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::{File, Metadata};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::artifact_flow::Landed;

#[cfg(test)]
mod tests;

/// Where the build line's queue lives under a project's workspace folder —
/// the folder the window's workspace setting puts the repository's checkouts
/// in — as the runner's own note says it moved there ("build-line/build-queue").
pub(crate) const LINE_FOLDER: [&str; 2] = ["build-line", "build-queue"];

/* The runner's names for its own files, spelled as the runner spells them. */
const RUNNER_LOG: &str = "runner.log";
const QUEUE_IN: &str = "in";
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

/// The lane's log for one sha under the release folder's `out/`
/// (`LOG=$OUT/lane-$SHA8.log` in `tools/release/lane.sh`).
const LANE_LOG_PREFIX: &str = "lane-";

/// An estimate is the median of the last five runs that passed.
const SAMPLES: usize = 5;
/// The lane logs read, newest first by their last write: eight hold more than
/// five runs of every stage even when one sha was launched three times.
const LANE_FILES: usize = 8;
/// The lane logs stamped: when the newest [`LANE_FILES`] hold no lane that
/// ended green, the older of these are read too, until one does —
/// the stages ahead of a running lane are that lane's.
const LANE_FILES_FAR: usize = 32;
/// How often the lane folder is listed again although it did not change: an
/// old sha launched again appends to a file the newest eight may not hold.
const LANE_RELIST_MS: i64 = 60_000;
/// The most of the runner's log one look reads — a first look at a long log,
/// or a look at one that grew far since the last: its last stretch holds the
/// past of every kind and the week the board can show.
const LOOK_BYTES: u64 = 4 * 1024 * 1024;
/// How many bytes before the offset a look remembers, to know the file it
/// reads on is still the one it read: a log cut and grown past the offset is
/// told by them.
const SEAM_BYTES: usize = 32;
/// A lane log's last stretch, read in full by a look: a lane's log is a few
/// hundred KiB to a few MiB, and its newest runs are at its end.
const LANE_LOG_BYTES: u64 = 4 * 1024 * 1024;
/// `DISK_FLOOR` holds one number of GiB.
const FLOOR_BYTES: u64 = 64;
/// Finished jobs kept between looks, the newest last.
const KEPT_JOBS: usize = 3_000;
/// The most finished jobs one answer carries: the newest.
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

/// A log the window asks to read: a build-line job by its name, or a lane by
/// its sha8. Only a job or a lane this reading listed is opened.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LogAsk {
    Job(String),
    Lane(String),
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
/// open — and the release lane's folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Roots {
    pub(crate) line: Option<PathBuf>,
    pub(crate) release: Option<PathBuf>,
}

impl Roots {
    /// The queue under a project's workspace folder, beside the lane's folder.
    pub(crate) fn of(workspaces: Option<&Path>, release: Option<PathBuf>) -> Self {
        Self {
            line: workspaces.map(|root| {
                LINE_FOLDER
                    .iter()
                    .fold(root.to_path_buf(), |dir, part| dir.join(part))
            }),
            release,
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
    pub(crate) lanes: Vec<LaneRun>,
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

/// The build line as a whole: where it was looked for, and the flags its
/// runner obeys.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct LineFacts {
    pub(crate) folder: Option<String>,
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
    /// Where the lane logs were looked for.
    pub(crate) lanes_folder: Option<String>,
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

/// What a running or waiting thing is expected to take: the median of
/// `samples` past runs that passed — or, with none, the cap that bounds it
/// (a lane's stage has no cap and says 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Expected {
    pub(crate) secs: u32,
    pub(crate) samples: u8,
}

/// A lane run's result when it neither passed nor failed to the window's
/// reading: no verdict, or one nobody can tell the word of (the other words
/// are the job states [`PASSED`], [`FAILED`] and [`RUNNING`]).
const ENDED: &str = "ended";

/// One run of the release lane over one sha.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LaneRun {
    pub(crate) sha: String,
    pub(crate) version: Option<String>,
    pub(crate) start_ms: i64,
    /// Its verdict's line, or the last line of a run that ended without one.
    pub(crate) end_ms: Option<i64>,
    /// The lane's own word (`green`, `red`, …) and why, where it gave one.
    pub(crate) verdict: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) running: bool,
    /// How it ended, in the board's words: `running`, `passed` (the lane's
    /// explicit green verdict), `failed` (red) or `ended` (anything else).
    /// The window compares none of the lane's own words.
    pub(crate) result: &'static str,
    pub(crate) stages: Vec<LaneStage>,
    /// While it runs: the stage it is in, then the stages the last full green
    /// lane ran after it, each with what it is expected to take.
    pub(crate) ahead: Vec<StageAhead>,
    /// The stages after the one it is in came from such a lane. Without one
    /// `ahead` holds the stage it is in alone, and what is left of the lane is
    /// not known.
    pub(crate) ahead_planned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LaneStage {
    pub(crate) name: String,
    pub(crate) start_ms: i64,
    pub(crate) end_ms: Option<i64>,
    pub(crate) rc: Option<i32>,
    /// The stage's own count of seconds — one that ran beside another
    /// (`build-zo` beside `build-app`) counts more than its wall clock.
    pub(crate) secs: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct StageAhead {
    pub(crate) name: String,
    pub(crate) expected: Expected,
}

/// The end of a log the window opened.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct LogTail {
    pub(crate) job: Option<String>,
    pub(crate) lane: Option<String>,
    pub(crate) lines: Vec<String>,
    /// Only the log's last lines are here.
    pub(crate) cut: bool,
    /// Nothing to read: no such job or lane in this reading, or no file.
    pub(crate) missing: bool,
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
    now_ms: i64,
    offset_secs: i64,
) -> Timeline {
    let mut book = BOOK.lock().unwrap_or_else(PoisonError::into_inner);
    book.land(landings);
    book.read(roots, ask, now_ms, offset_secs)
}

/// What one look keeps for the next: the runner log read so far and what it
/// told, the queued jobs' headers, the lane logs read, and the revision.
#[derive(Debug, Default)]
struct Book {
    roots: Roots,
    runner: RunnerTail,
    headers: HashMap<String, (FileStamp, Header)>,
    script: ScriptCaps,
    floor: Option<(FileStamp, Option<u32>)>,
    lanes: LaneBook,
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

    fn read(&mut self, roots: &Roots, ask: &Ask, now_ms: i64, offset_secs: i64) -> Timeline {
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
        let lanes_folder = roots
            .release
            .as_ref()
            .map(|release| release.join(crate::update_runtime::OUT_DIR));
        let status_file = roots
            .release
            .as_ref()
            .map(|release| release.join(crate::update_runtime::STATUS_FILE));
        let status_stamp = status_file.as_deref().and_then(stamp_plain);
        self.lanes.look(lanes_folder.as_deref(), now_ms);
        let read_so_far = signature_of(queue.as_ref(), &self.lanes, status_stamp, self.landed);
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
            let status = status_file.as_deref().and_then(lane_status);
            self.lanes.read(offset_secs, status.as_ref());
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
        let paused = queue.as_ref().is_some_and(|look| look.paused);
        let mut jobs = self.jobs(queue.as_ref(), from_ms);
        self.tie_ended(&mut jobs);
        #[cfg(test)]
        if let Some(act) = self.after_tie {
            act(&self.roots);
        }
        // What this answer tied is no change the next time: signed with the
        // stamps the tie was made from, never the disk's a moment later — a
        // file that came in between is a change the next look reports.
        self.signature = with_ties(read_so_far, self.ties_tied());
        Timeline {
            revision: self.revision,
            unchanged: false,
            read_ms: now_ms,
            line: LineFacts {
                folder: roots.line.as_ref().map(|path| path.display().to_string()),
                found: queue.as_ref().is_some_and(|look| look.found),
                paused,
                pause_note: self.runner.pause_note.clone().filter(|_| paused),
                disk_floor_gib: self.floor.and_then(|(_, value)| value),
                disk_low: self.runner.disk_low,
                quiet_wanted: queue.as_ref().is_some_and(|look| look.quiet_wanted),
                runner_lanes: self.runner.lanes.iter().copied().collect(),
                lanes_folder: lanes_folder.as_ref().map(|path| path.display().to_string()),
            },
            task_lines: task_lines(&jobs),
            jobs,
            lanes: self
                .lanes
                .runs
                .iter()
                .filter(|run| run.running || run.end_ms.unwrap_or(run.start_ms) >= from_ms)
                .cloned()
                .collect(),
            landings: self.landings.clone(),
            log,
        }
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

    /// The end of a log, for a job or a lane this reading listed — never a path
    /// the window spelled.
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
            LogAsk::Lane(sha) => LogTail {
                lane: Some(sha.clone()),
                ..tail_of(self.lanes.logs.get(sha).map(PathBuf::as_path))
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
    let missing = LogTail {
        missing: true,
        ..LogTail::default()
    };
    let Some(mut file) = path.and_then(crate::plain_read::open_plain) else {
        return missing;
    };
    let len = file.metadata().map_or(0, |meta| meta.len());
    let start = len.saturating_sub(LOG_TAIL_BYTES);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err()
        || file.take(LOG_TAIL_BYTES).read_to_end(&mut bytes).is_err()
    {
        return missing;
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let first = lines.len().saturating_sub(LOG_TAIL_LINES);
    LogTail {
        lines: lines[first..]
            .iter()
            .map(|line| (*line).to_string())
            .collect(),
        cut: start > 0 || first > 0,
        ..LogTail::default()
    }
}

/* ---- the queue folder ----------------------------------------------------- */

/// A file's size, times and identity: what a look compares, never reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
struct FileStamp {
    len: u64,
    modified_ms: i64,
    /// The inode's last change: a rename moves it, so a job's `run/` file
    /// says when the runner took it.
    changed_ms: i64,
    /// Device and inode: a runner log replaced by another file is read again.
    file: (u64, u64),
}

fn stamp(path: &Path) -> Option<FileStamp> {
    std::fs::metadata(path).ok().map(|meta| stamp_of(&meta))
}

/// The stamp of a plain file at `path` — none for a link, a pipe or a folder:
/// a file another process owns is not followed to wherever it points.
fn stamp_plain(path: &Path) -> Option<FileStamp> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(Metadata::is_file)
        .map(|meta| stamp_of(&meta))
}

fn stamp_of(meta: &Metadata) -> FileStamp {
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis() as i64);
    #[cfg(unix)]
    let (changed_ms, file) = {
        use std::os::unix::fs::MetadataExt;
        (
            meta.ctime() * 1000 + meta.ctime_nsec() / 1_000_000,
            (meta.dev(), meta.ino()),
        )
    };
    #[cfg(not(unix))]
    let (changed_ms, file) = (modified_ms, (0, 0));
    FileStamp {
        len: meta.len(),
        modified_ms,
        changed_ms,
        file,
    }
}

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
fn signature_of(
    queue: Option<&QueueLook>,
    lanes: &LaneBook,
    status: Option<FileStamp>,
    landed: u64,
) -> u64 {
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
    lanes.folder_stamp.hash(&mut hasher);
    for (path, held) in &lanes.files {
        path.hash(&mut hasher);
        held.hash(&mut hasher);
    }
    status.hash(&mut hasher);
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

/// The middle of `secs` — the mean of the two middles for an even count.
fn median(secs: &[u32]) -> Option<u32> {
    if secs.is_empty() {
        return None;
    }
    let mut sorted = secs.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    Some(if sorted.len().is_multiple_of(2) {
        ((u64::from(sorted[middle - 1]) + u64::from(sorted[middle])) / 2) as u32
    } else {
        sorted[middle]
    })
}

/* ---- the release lane ------------------------------------------------------------ */

/// The lane's own word on the sha it runs, from `status.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LaneStatus {
    sha: String,
    outcome: Option<String>,
    version: Option<String>,
    phase: Option<String>,
    /// The steps the lane says it runs, in order (`plan`, t-83513) — none from
    /// a lane that does not say them.
    plan: Vec<String>,
}

fn lane_status(path: &Path) -> Option<LaneStatus> {
    let value = crate::update_runtime::read_json(path)?;
    let word = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|said| !said.is_empty())
            .map(str::to_string)
    };
    let plan = value
        .get("plan")
        .and_then(serde_json::Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .filter_map(|step| step.get("name")?.as_str())
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(LaneStatus {
        sha: word("sha")?,
        outcome: word("outcome"),
        version: word("version"),
        phase: word("phase"),
        plan,
    })
}

/// A dated line of a lane log, in a run.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LaneMark {
    /// `== <name> [note]`: a stage begins — or, for a word no stage ever ended
    /// with, the lane's verdict.
    Begin {
        name: String,
        at_ms: i64,
        note: String,
    },
    /// `<name> rc=<n> <secs>s …`: a stage ends.
    End {
        name: String,
        at_ms: i64,
        rc: i32,
        secs: u32,
    },
    /// `publish: v<version> on …`
    Version(String),
    /// Any other line: the run went on until here.
    Line(i64),
}

/// One run of a lane log: from its `== lane <sha8> start` to the next.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedRun {
    sha: String,
    start_ms: i64,
    marks: Vec<LaneMark>,
}

/// The runs of one lane log, its lines dated back from the file's last write
/// (`anchor_ms`, [`dated_back`]).
fn lane_runs(text: &str, anchor_ms: i64, offset_secs: i64) -> Vec<ParsedRun> {
    let dated: Vec<(u32, &str)> = text
        .lines()
        .filter_map(|line| {
            let (clock, rest) = line.split_once(' ')?;
            Some((clock_secs(clock)?, rest))
        })
        .collect();
    // One writer: each line at or before the one below it.
    let clocks: Vec<u32> = dated.iter().map(|(clock, _)| *clock).collect();
    let times = dated_back(&clocks, anchor_ms, 0, offset_secs);
    let mut runs: Vec<ParsedRun> = Vec::new();
    for ((_, rest), at_ms) in dated.into_iter().zip(times) {
        if let Some(sha) = lane_start(rest) {
            runs.push(ParsedRun {
                sha,
                start_ms: at_ms,
                marks: Vec::new(),
            });
            continue;
        }
        let Some(run) = runs.last_mut() else {
            continue;
        };
        let mark = if let Some(begun) = rest.strip_prefix("== ") {
            let (name, note) = begun.split_once(' ').unwrap_or((begun, ""));
            LaneMark::Begin {
                name: name.to_string(),
                at_ms,
                note: note.trim().to_string(),
            }
        } else if let Some(end) = stage_end(rest, at_ms) {
            end
        } else if let Some(version) = published_version(rest) {
            run.marks.push(LaneMark::Version(version));
            LaneMark::Line(at_ms)
        } else {
            LaneMark::Line(at_ms)
        };
        run.marks.push(mark);
    }
    runs
}

/// `== lane <sha8> start (dry=0)`
fn lane_start(rest: &str) -> Option<String> {
    let words: Vec<&str> = rest.split_whitespace().take(4).collect();
    match words.as_slice() {
        ["==", "lane", sha, "start"] => Some((*sha).to_string()),
        _ => None,
    }
}

/// `<name> rc=<n> <secs>s free=… load=…`
fn stage_end(rest: &str, at_ms: i64) -> Option<LaneMark> {
    let mut words = rest.split_whitespace();
    let name = words.next()?;
    let rc = words.next()?.strip_prefix("rc=")?.parse().ok()?;
    let secs = words.next()?.strip_suffix('s')?.parse().ok()?;
    Some(LaneMark::End {
        name: name.to_string(),
        at_ms,
        rc,
        secs,
    })
}

/// `publish: v1.1.82 on <repository> …`
fn published_version(rest: &str) -> Option<String> {
    let (version, tail) = rest.strip_prefix("publish: v")?.split_once(' ')?;
    tail.starts_with("on ").then(|| version.to_string())
}

/// The lane logs read and the runs they hold.
#[derive(Debug, Default)]
struct LaneBook {
    folder: Option<PathBuf>,
    folder_stamp: Option<FileStamp>,
    listed_ms: i64,
    /// The newest lane logs, newest first, and their stamps this look.
    files: Vec<(PathBuf, FileStamp)>,
    parsed: HashMap<PathBuf, (FileStamp, Vec<ParsedRun>)>,
    /// The file each listed run's sha was read from, for the window's log.
    logs: HashMap<String, PathBuf>,
    runs: Vec<LaneRun>,
}

impl LaneBook {
    /// Which lane logs to read: the newest few, the folder listed again when it
    /// changed or a minute went by, the files themselves stamped every look.
    fn look(&mut self, folder: Option<&Path>, now_ms: i64) {
        let Some(folder) = folder else {
            *self = Self::default();
            return;
        };
        let folder_stamp = stamp(folder);
        if self.folder.as_deref() != Some(folder)
            || self.folder_stamp != folder_stamp
            || now_ms - self.listed_ms >= LANE_RELIST_MS
        {
            self.folder = Some(folder.to_path_buf());
            self.folder_stamp = folder_stamp;
            self.listed_ms = now_ms;
            self.files = newest_lane_logs(folder);
            return;
        }
        for (path, held) in &mut self.files {
            if let Some(fresh) = stamp(path) {
                *held = fresh;
            }
        }
    }

    /// Read the files that changed, and fold every run they hold. The newest
    /// [`LANE_FILES`] are read; while a running lane has no stages ahead of it
    /// to plan by (no lane in them ended green), the older ones too.
    fn read(&mut self, offset_secs: i64, status: Option<&LaneStatus>) {
        let listed: HashSet<&PathBuf> = self.files.iter().map(|(path, _)| path).collect();
        self.parsed.retain(|path, _| listed.contains(path));
        let mut take = LANE_FILES.min(self.files.len());
        loop {
            self.fold(take, offset_secs, status);
            let unplanned = self
                .runs
                .iter()
                .any(|run| run.running && !run.ahead_planned);
            if !unplanned || take >= self.files.len() {
                break;
            }
            take = (take * 2).min(self.files.len());
        }
    }

    /// The runs of the newest `take` lane logs, as the window is told them.
    fn fold(&mut self, take: usize, offset_secs: i64, status: Option<&LaneStatus>) {
        let files = &self.files[..take];
        for (path, held) in files {
            if self
                .parsed
                .get(path)
                .is_some_and(|(before, _)| before == held)
            {
                continue;
            }
            let runs = crate::plain_read::read_plain_tail(path, LANE_LOG_BYTES)
                .map(|(bytes, cut)| {
                    let text = String::from_utf8_lossy(&bytes);
                    // The line a long log's tail cuts through is not a line.
                    let body: &str = match text.split_once('\n') {
                        Some((_, rest)) if cut => rest,
                        _ => &text,
                    };
                    lane_runs(body, held.modified_ms, offset_secs)
                })
                .unwrap_or_default();
            self.parsed.insert(path.clone(), (*held, runs));
        }
        // A log copied under a second name holds the same runs: each once.
        let mut seen = HashSet::new();
        let mut parsed: Vec<(&PathBuf, &ParsedRun)> = Vec::new();
        for (path, _) in files {
            for run in self
                .parsed
                .get(path)
                .map_or(&[][..], |(_, runs)| runs.as_slice())
            {
                if seen.insert((run.sha.clone(), run.start_ms)) {
                    parsed.push((path, run));
                }
            }
        }
        self.logs = parsed
            .iter()
            .rev()
            .map(|(path, run)| (run.sha.clone(), (*path).clone()))
            .collect();
        let runs: Vec<&ParsedRun> = parsed.into_iter().map(|(_, run)| run).collect();
        self.runs = lane_view(&runs, status);
    }
}

/// The lane logs under `folder`, newest first, at most [`LANE_FILES_FAR`].
fn newest_lane_logs(folder: &Path) -> Vec<(PathBuf, FileStamp)> {
    let Ok(listing) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut logs: Vec<(PathBuf, FileStamp)> = listing
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(LANE_LOG_PREFIX) && name.ends_with(LOG_SUFFIX))
        })
        .filter_map(|entry| {
            let meta = entry.metadata().ok().filter(Metadata::is_file)?;
            Some((entry.path(), stamp_of(&meta)))
        })
        .collect();
    logs.sort_by_key(|(_, held)| std::cmp::Reverse(held.modified_ms));
    logs.truncate(LANE_FILES_FAR);
    logs
}

/// Every run, oldest first: its stages, its verdict, and — for the one
/// `status.json` says runs — the stages ahead of it with what each is
/// expected to take: the steps that file says the lane runs, or with none
/// said, the steps of the last full green. A word is a stage when some run
/// ended a stage of that name (`<name> rc=`), or the lane says it runs it; a
/// `== <word>` no stage ever ended with is a verdict.
fn lane_view(parsed: &[&ParsedRun], status: Option<&LaneStatus>) -> Vec<LaneRun> {
    let stage_names: HashSet<&str> = parsed
        .iter()
        .flat_map(|run| &run.marks)
        .filter_map(|mark| match mark {
            LaneMark::End { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .chain(
            status
                .iter()
                .flat_map(|said| said.plan.iter().map(String::as_str)),
        )
        .collect();
    let mut order: Vec<&ParsedRun> = parsed.to_vec();
    order.sort_by_key(|run| run.start_ms);
    // The run status.json speaks for: the newest of its sha, while the lane has
    // given no outcome and the run's last word is a stage — a known one, or
    // the phase status.json names.
    let running = status
        .filter(|said| said.outcome.is_none())
        .and_then(|said| {
            let run = order
                .iter()
                .rev()
                .find(|run| said.sha.starts_with(&run.sha))?;
            let last = run.marks.iter().rev().find_map(|mark| match mark {
                LaneMark::Begin { name, .. } => Some(name.as_str()),
                _ => None,
            })?;
            (stage_names.contains(last) || said.phase.as_deref() == Some(last))
                .then_some((run.sha.as_str(), run.start_ms))
        });
    let mut runs: Vec<LaneRun> = order
        .iter()
        .map(|run| {
            folded_run(
                run,
                &stage_names,
                running == Some((run.sha.as_str(), run.start_ms)),
            )
        })
        .collect();
    // One sha is one version: a run that stopped before it published says the
    // version its sha published under — or the one status.json names for it.
    let mut versions: HashMap<String, String> = runs
        .iter()
        .filter_map(|run| Some((run.sha.clone(), run.version.clone()?)))
        .collect();
    if let Some(status) = status
        && let Some(version) = &status.version
    {
        for run in runs.iter().filter(|run| status.sha.starts_with(&run.sha)) {
            versions
                .entry(run.sha.clone())
                .or_insert_with(|| version.clone());
        }
    }
    for run in &mut runs {
        if run.version.is_none() {
            run.version = versions.get(&run.sha).cloned();
        }
    }
    let past = stage_past(&runs);
    // The steps ahead of the running lane are the ones it says it runs
    // (t-83513); a lane that says none is planned by the last full green.
    let plan = status
        .map(|said| said.plan.clone())
        .filter(|said| !said.is_empty())
        .unwrap_or_else(|| lane_plan(&runs));
    for run in &mut runs {
        run.result = result_of(run);
    }
    for run in runs.iter_mut().filter(|run| run.running) {
        let Some(current) = run.stages.last().filter(|stage| stage.end_ms.is_none()) else {
            continue;
        };
        let mut ahead = vec![current.name.clone()];
        if let Some(at) = plan.iter().position(|name| *name == current.name) {
            ahead.extend(plan[at + 1..].iter().cloned());
            run.ahead_planned = true;
        }
        run.ahead = ahead
            .into_iter()
            .map(|name| StageAhead {
                expected: expected_of(past.get(&name).map(Vec::as_slice)),
                name,
            })
            .collect();
    }
    runs
}

/// One run's stages and verdict out of its marks. A running run's last
/// `== <word>` is the stage it is in, whatever the word.
fn folded_run(run: &ParsedRun, stage_names: &HashSet<&str>, running: bool) -> LaneRun {
    let last_begin = run
        .marks
        .iter()
        .rposition(|mark| matches!(mark, LaneMark::Begin { .. }));
    let mut folded = LaneRun {
        sha: run.sha.clone(),
        version: None,
        start_ms: run.start_ms,
        end_ms: None,
        verdict: None,
        reason: None,
        running,
        result: ENDED,
        stages: Vec::new(),
        ahead: Vec::new(),
        ahead_planned: false,
    };
    let mut last_ms = run.start_ms;
    for (index, mark) in run.marks.iter().enumerate() {
        match mark {
            LaneMark::Begin { name, at_ms, note } => {
                last_ms = last_ms.max(*at_ms);
                if stage_names.contains(name.as_str()) || (running && Some(index) == last_begin) {
                    folded.stages.push(LaneStage {
                        name: name.clone(),
                        start_ms: *at_ms,
                        end_ms: None,
                        rc: None,
                        secs: None,
                    });
                } else {
                    folded.verdict = Some(name.clone());
                    folded.reason = note
                        .strip_prefix('(')
                        .and_then(|said| said.strip_suffix(')'))
                        .filter(|said| !said.is_empty())
                        .map(str::to_string);
                    folded.end_ms = Some(*at_ms);
                }
            }
            LaneMark::End {
                name,
                at_ms,
                rc,
                secs,
            } => {
                last_ms = last_ms.max(*at_ms);
                if let Some(stage) = folded
                    .stages
                    .iter_mut()
                    .rev()
                    .find(|stage| stage.name == *name && stage.end_ms.is_none())
                {
                    stage.end_ms = Some(*at_ms);
                    stage.rc = Some(*rc);
                    stage.secs = Some(*secs);
                }
            }
            LaneMark::Version(version) => folded.version = Some(version.clone()),
            LaneMark::Line(at_ms) => last_ms = last_ms.max(*at_ms),
        }
    }
    if running {
        folded.end_ms = None;
    } else if folded.end_ms.is_none() {
        folded.end_ms = Some(last_ms);
    }
    folded
}

/// Each stage's wall-clock seconds over the runs that ended, oldest first: the
/// stages that passed, and every stage of a run that ended green — a gate whose
/// red the flaky-test judge cleared ran its whole length, while a stage of a
/// run that ended red may have hung or been cut.
fn stage_past(runs: &[LaneRun]) -> HashMap<String, Vec<u32>> {
    let mut past: HashMap<String, Vec<u32>> = HashMap::new();
    for run in runs.iter().filter(|run| !run.running) {
        let done = result_of(run) == PASSED;
        for stage in &run.stages {
            if let (Some(rc), Some(end_ms)) = (stage.rc, stage.end_ms)
                && (rc == 0 || done)
            {
                let secs = ((end_ms - stage.start_ms).max(0) / 1000) as u32;
                past.entry(stage.name.clone()).or_default().push(secs);
            }
        }
    }
    past
}

/// The protocol written by tools/release/lane.sh: judge_sha sets OUTCOME to
/// green only after the release succeeds, or red on failure. sweep_sha writes
/// `== $OUTCOME` after its own rc=0 cleanup, even for an unknown sha rejected
/// before archive. Stage codes cannot define success: cleanup passes on red,
/// and a failed gate can be cleared by the solo flake judge on green.
/// Refused, superseded, missing and unknown verdicts merely ended.
fn result_of(run: &LaneRun) -> &'static str {
    if run.running {
        return RUNNING;
    }
    match run.verdict.as_deref() {
        Some(crate::update_runtime::LANE_GREEN) => PASSED,
        Some(crate::update_runtime::LANE_RED) => FAILED,
        _ => ENDED,
    }
}

/// A stage's expected seconds: the median of its last passing runs.
fn expected_of(past: Option<&[u32]>) -> Expected {
    let all = past.unwrap_or_default();
    let recent = &all[all.len().saturating_sub(SAMPLES)..];
    Expected {
        secs: median(recent).unwrap_or(0),
        samples: recent.len() as u8,
    }
}

/// The stages a lane runs, in order, as the latest lane that ended green ran
/// them — the longest such list, so a lane
/// that bowed out early does not cut the plan short.
fn lane_plan(runs: &[LaneRun]) -> Vec<String> {
    let passed: Vec<&LaneRun> = runs
        .iter()
        .rev()
        .filter(|run| result_of(run) == PASSED && !run.stages.is_empty())
        .collect();
    let longest = passed.iter().map(|run| run.stages.len()).max().unwrap_or(0);
    passed
        .into_iter()
        .find(|run| run.stages.len() == longest)
        .map(|run| run.stages.iter().map(|stage| stage.name.clone()).collect())
        .unwrap_or_default()
}
