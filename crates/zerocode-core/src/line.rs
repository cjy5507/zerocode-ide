//! The build line: one queue of heavy commands for the whole machine, whatever
//! the project (`zo line`). Workers, coordinators and people put a command in
//! it; it runs when its turn comes, in one of the machine's build slots — the
//! same slots `zo build` runs in — and leaves a record and a log.
//!
//! ```text
//! <zo home>/line/jobs/<id>.json   one record a job, replaced whole
//! <zo home>/line/logs/<id>.log    what the job's command wrote
//! <zo home>/line/last/<owner>     when the owner's last job ended (turns)
//! <zo home>/line/turn             starts so far (two first jobs, then a plain one)
//! <zo home>/line/pause.json       nothing starts while it stands
//! <zo home>/line/lock             held while a turn is decided
//! ```
//!
//! The turn rule is the coordinators' build line's (`runner10.sh`, 2026-09 to
//! 11), written here once so every project has it: each owner's jobs run in the
//! order they were put in and never two at a time; among owners, the one whose
//! last job ended longest ago goes first; a `first` job goes before a plain one
//! — two firsts, then one plain, a quiet one too, so plain work is not starved —
//! and a `last` job only fills a slot nothing else wants. A `solo` job (a whole
//! suite, an e2e run) takes the first slot only; a `quiet` job (a measurement)
//! runs alone: from its turn until the machine is empty nothing new starts.
//! Nothing starts while the line is paused or the disk is below its floor. The
//! window reads the records and never writes them: its buttons run `zo line`.

use std::collections::HashMap;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic_file::write_atomically;

const FOLDER: &str = "line";
const JOBS: &str = "jobs";
const LOGS: &str = "logs";
const LAST: &str = "last";
const TURN: &str = "turn";
const PAUSE: &str = "pause.json";
const LOCK: &str = "lock";
/// The variable that names zo's home, as zo reads it.
const ZO_HOME_ENV: &str = "ZO_HOME";
const ZO_DIR: &str = ".zo";
/// A record larger than this is not one zo wrote.
const RECORD_BYTES_MAX: u64 = 64 * 1024;
/// Ended jobs kept on the line, the newest; older records and logs go.
pub const ENDED_KEPT: usize = 400;
/// How long a job may run when its writer named no cap.
pub const CAP_DEFAULT_SECS: u32 = 30 * 60;
/// The disk floor when the person's settings name none, in GiB (`runner10.sh`'s).
pub const DISK_FLOOR_DEFAULT_GIB: u64 = 16;
/// The slot every job may take; the others take all but solo and quiet jobs.
pub const FIRST_SLOT: u32 = 0;

/// Where zo keeps its state: `$ZO_HOME`, or `.zo` in the person's home
/// (`HOME`, or `USERPROFILE` on Windows) — as zo's own paths read it.
#[must_use]
pub fn zo_home() -> Option<PathBuf> {
    std::env::var_os(ZO_HOME_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(ZO_DIR))
        })
}

/// The line's folder under zo's home.
#[must_use]
pub fn folder(zo_home: &Path) -> PathBuf {
    zo_home.join(FOLDER)
}

/// How a job stands in the queue's order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Before plain work: a landing gate, a release's checks.
    First,
    #[default]
    Plain,
    /// Only when nothing else waits: work for no release in sight.
    Last,
}

/// Where a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Waiting,
    Running,
    Passed,
    Failed,
    /// Ended by the line past its cap.
    Capped,
    /// Ended by a person or its owner (`zo line stop`).
    Stopped,
    /// Taken out of the queue before it ran (`zo line withdraw`).
    Withdrawn,
    /// It said it ran, and its runner is gone without saying how it ended.
    Lost,
}

impl JobState {
    #[must_use]
    pub const fn ended(self) -> bool {
        !matches!(self, Self::Waiting | Self::Running)
    }

    /// The word a letter about the job says it stands at: the line's own
    /// (`zo line`) and the ledger's when the line could not tell it
    /// (t-131644).
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Capped => "ended past its cap",
            Self::Stopped => "stopped",
            Self::Withdrawn => "withdrawn",
            Self::Lost => "lost",
        }
    }
}

/// The name job `id`'s end is told under: the line's own send carries it as
/// its retry name (t-118403) and as its subject, and the ledger's letter about
/// an end the line could not tell carries it as its subject (t-131644) — one
/// name a letter about that end is found by, whoever sent it.
#[must_use]
pub fn told_as(id: &str) -> String {
    format!("zo-line-{id}")
}

/// The name job `id`'s end is bound to the pane that puts the job in under
/// (t-131644, F1 of the review t-131831): `zo line add` asks the ledger's
/// `line-notify` with it as its retry name, and the ledger finds the answer it
/// filed by it before it tells an end the line could not. Not [`told_as`]: a
/// retry name is one request's, and the same pane tells the end under that one.
#[must_use]
pub fn bound_as(id: &str) -> String {
    format!("zo-line-notify-{id}")
}

/// How telling a job's end did not go (t-131644), written in its record by
/// the process that tried.
///
/// The teller signs with the seat of the pane that put the job in — a job's
/// waiter keeps that pane's environment — and a window restart mints every
/// seat anew: an end told after a restart, of a job put in before it, is
/// refused by the new window before anything is planned, so it never lands
/// late. `window_gone` is that case seen at the moment of the failure, and
/// the window tells such an end itself; any other failure stays here, and in
/// the log's last line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyFailed {
    /// When the tell was tried and did not go.
    pub at_ms: i64,
    /// The log's last line's reason: how the ledger's command exited and the
    /// first thing it said, or why it did not start.
    pub why: String,
    /// The window process the teller's seat came from (its environment's
    /// `ZEROCODE_WINDOW_PID`) had ended when the tell was tried.
    #[serde(default)]
    pub window_gone: bool,
}

/// One job on the line, as zo writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// The repository's shared git folder: the project the board shows it under.
    pub project: String,
    /// Where the command runs.
    pub checkout: String,
    /// Whose job it is: a run, a worker, a person. One job of an owner runs at a time.
    pub owner: String,
    /// What it is, in the words of whoever put it in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default)]
    pub class: Class,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quiet: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub solo: bool,
    pub cap_secs: u32,
    pub command: Vec<String>,
    /// Where the ledger is told how it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<String>,
    /// The run the pane that put the job in stood in, when `notify` names an
    /// address another run cannot find (a worker, a pane): its end is told in
    /// that run, whoever tells it (t-119309). A record written before it has
    /// none, and is told as it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_run: Option<String>,
    /// Telling `notify` how the job ended did not go (t-131644): the line's
    /// word for it, beside the log's last line. A record written before it,
    /// or of an end that was told, has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_failed: Option<NotifyFailed>,
    pub state: JobState,
    pub queued_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rc: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u32>,
    /// The process that waits for this job's turn and runs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// The job this one runs again (`zo line retry`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    /// Someone asked it to stop: its runner ends it at the next look.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stop_asked: bool,
    /// Who took it out or stopped it, when not its owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_by: Option<String>,
}

/// The line stands still: nothing new starts. `until` names a build whose run,
/// once it ends, lifts the pause by itself (`zo line pause --until-build`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pause {
    pub by: String,
    pub since_ms: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Until>,
}

/// A build of a project whose next ended run lifts a pause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Until {
    /// The project's shared git folder.
    pub project: String,
    pub build: String,
}

/// Paths of one line.
#[derive(Debug, Clone)]
pub struct Line {
    root: PathBuf,
}

impl Line {
    #[must_use]
    pub fn at(zo_home: &Path) -> Self {
        Self {
            root: folder(zo_home),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn job_path(&self, id: &str) -> PathBuf {
        self.root.join(JOBS).join(format!("{id}.json"))
    }

    #[must_use]
    pub fn log_path(&self, id: &str) -> PathBuf {
        self.root.join(LOGS).join(format!("{id}.log"))
    }

    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK)
    }

    /// Creates the line's folders.
    pub fn make(&self) -> io::Result<()> {
        std::fs::create_dir_all(self.root.join(JOBS))?;
        std::fs::create_dir_all(self.root.join(LOGS))?;
        std::fs::create_dir_all(self.root.join(LAST))
    }

    /// Writes `job` over its record, whole.
    pub fn save(&self, job: &Job) -> io::Result<()> {
        if !is_job_id(&job.id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("`{}` is no job id", job.id),
            ));
        }
        let mut bytes = serde_json::to_vec_pretty(job).map_err(io::Error::other)?;
        bytes.push(b'\n');
        write_atomically(&self.job_path(&job.id), &bytes)
    }

    /// The job `id`, as its record says.
    pub fn job(&self, id: &str) -> io::Result<Job> {
        if !is_job_id(id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("`{id}` is no job id"),
            ));
        }
        read_job(&self.job_path(id))
    }

    /// Every job on the line, oldest put in first.
    #[must_use]
    pub fn jobs(&self) -> Vec<Job> {
        let Ok(entries) = std::fs::read_dir(self.root.join(JOBS)) else {
            return Vec::new();
        };
        let mut jobs: Vec<Job> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|path| read_job(&path).ok())
            .collect();
        jobs.sort_by(|one, other| {
            one.queued_ms
                .cmp(&other.queued_ms)
                .then_with(|| one.id.cmp(&other.id))
        });
        jobs
    }

    /// Removes the records and logs of ended jobs beyond the newest
    /// [`ENDED_KEPT`].
    pub fn prune(&self) {
        let mut ended: Vec<Job> = self
            .jobs()
            .into_iter()
            .filter(|job| job.state.ended())
            .collect();
        if ended.len() <= ENDED_KEPT {
            return;
        }
        ended.sort_by_key(|job| std::cmp::Reverse(job.ended_ms.unwrap_or(job.queued_ms)));
        for old in ended.into_iter().skip(ENDED_KEPT) {
            let _ = std::fs::remove_file(self.log_path(&old.id));
            let _ = std::fs::remove_file(self.job_path(&old.id));
        }
    }

    /// When `owner`'s last job ended, in ms; 0 when it never had one.
    #[must_use]
    pub fn last_end(&self, owner: &str) -> i64 {
        std::fs::read_to_string(self.root.join(LAST).join(owner_file(owner)))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    }

    /// `owner`'s last job ended at `at_ms`.
    pub fn ended(&self, owner: &str, at_ms: i64) -> io::Result<()> {
        write_atomically(
            &self.root.join(LAST).join(owner_file(owner)),
            at_ms.to_string().as_bytes(),
        )
    }

    /// How many jobs the line has started.
    #[must_use]
    pub fn turn(&self) -> u64 {
        std::fs::read_to_string(self.root.join(TURN))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    }

    pub fn count_turn(&self) -> io::Result<()> {
        write_atomically(
            &self.root.join(TURN),
            (self.turn() + 1).to_string().as_bytes(),
        )
    }

    /// The pause that stands, if any.
    #[must_use]
    pub fn pause(&self) -> Option<Pause> {
        let bytes = std::fs::read(self.root.join(PAUSE)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn set_pause(&self, pause: &Pause) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(pause).map_err(io::Error::other)?;
        write_atomically(&self.root.join(PAUSE), &bytes)
    }

    pub fn lift_pause(&self) -> io::Result<()> {
        match std::fs::remove_file(self.root.join(PAUSE)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// The record at `path`, read only as the job its file is named for: one whose
/// id is no job id, or names another job than its file, is not one zo wrote
/// (t-131644, F2 of the review t-131831). [`Line::save`] refuses such an id;
/// any process of the person's can write a file, and the ledger's letter about
/// an end the line could not tell carries the id read here.
fn read_job(path: &Path) -> io::Result<Job> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(RECORD_BYTES_MAX + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > RECORD_BYTES_MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a job record too large",
        ));
    }
    let job: Job = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    let named = path.file_stem().and_then(std::ffi::OsStr::to_str);
    if !is_job_id(&job.id) || named != Some(job.id.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a job record that names no job, or another job than its file",
        ));
    }
    Ok(job)
}

/// Whether `id` can name a job: what [`new_id`] makes.
#[must_use]
pub fn is_job_id(id: &str) -> bool {
    id.len() <= 40
        && id.starts_with("j-")
        && id[2..]
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || char == '-')
}

/// A new job id: the moment it was put in and a few letters of chance.
#[must_use]
pub fn new_id(now_ms: i64, salt: u32) -> String {
    format!("j-{now_ms}-{:04x}", salt & 0xffff)
}

/// An owner's name as a file name: what is not a letter, digit, `.`, `_` or `-`
/// becomes `_`.
fn owner_file(owner: &str) -> String {
    let name: String = owner
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-') {
                char
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() || name.starts_with('.') {
        format!("_{name}")
    } else {
        name
    }
}

/// What the machine says when a turn is decided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    /// Slots taken by something other than this line (a `zo build` run).
    pub taken: Vec<u32>,
    /// How many slots the machine has.
    pub slots: u32,
    /// Free disk below the floor: nothing starts.
    pub disk_low: bool,
    pub paused: bool,
}

/// Why nothing starts in a slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    Paused,
    DiskLow,
    /// A quiet job waits for the machine to empty.
    QuietWanted,
}

/// The job that takes `slot` now, among `jobs`, by the line's rule — or why
/// nothing does. `last_end` says when each owner's last job ended, `turn` how
/// many jobs the line has started. A pure reading: deciding writes nothing.
pub fn next_up<'jobs>(
    jobs: &'jobs [Job],
    slot: u32,
    machine: &Machine,
    last_end: &dyn Fn(&str) -> i64,
    turn: u64,
) -> Result<Option<&'jobs Job>, Hold> {
    if machine.paused {
        return Err(Hold::Paused);
    }
    if machine.disk_low {
        return Err(Hold::DiskLow);
    }
    let running: Vec<&Job> = jobs
        .iter()
        .filter(|job| job.state == JobState::Running)
        .collect();
    if running.iter().any(|job| job.quiet) {
        return Err(Hold::QuietWanted);
    }
    let busy_owners: Vec<&str> = running.iter().map(|job| job.owner.as_str()).collect();
    // Each owner's oldest waiting job, if that owner runs nothing now.
    let mut heads: HashMap<&str, &Job> = HashMap::new();
    for job in jobs
        .iter()
        .filter(|job| job.state == JobState::Waiting && !job.stop_asked)
    {
        if busy_owners.contains(&job.owner.as_str()) {
            continue;
        }
        heads.entry(job.owner.as_str()).or_insert(job);
    }
    let pick = |class: Class, fits: &dyn Fn(&Job) -> bool| -> Option<&'jobs Job> {
        heads
            .values()
            .copied()
            .filter(|job| job.class == class && fits(job))
            .min_by(|one, other| {
                last_end(&one.owner)
                    .cmp(&last_end(&other.owner))
                    .then_with(|| one.queued_ms.cmp(&other.queued_ms))
                    .then_with(|| one.id.cmp(&other.id))
            })
    };
    let anything = |job: &Job| -> bool { slot == FIRST_SLOT || !(job.solo || job.quiet) };
    // The job the first slot would take: a quiet one there holds every slot until the machine is free.
    let first_choice = pick_ordered(&pick, &|_| true, turn);
    if let Some(quiet) = first_choice.filter(|job| job.quiet) {
        let others_busy = !running.is_empty() || !machine.taken.is_empty();
        if others_busy {
            return Err(Hold::QuietWanted);
        }
        return Ok((slot == FIRST_SLOT).then_some(quiet));
    }
    Ok(pick_ordered(&pick, &anything, turn))
}

/// The oldest waiting job of a class that fits the slot being decided, by the owners' turns.
type Pick<'jobs, 'f> = dyn Fn(Class, &dyn Fn(&Job) -> bool) -> Option<&'jobs Job> + 'f;

/// Two first jobs, then a plain one when both wait, quiet or not; a last job
/// only when no other.
fn pick_ordered<'jobs>(
    pick: &Pick<'jobs, '_>,
    fits: &dyn Fn(&Job) -> bool,
    turn: u64,
) -> Option<&'jobs Job> {
    let first = pick(Class::First, fits);
    let plain = pick(Class::Plain, fits);
    match (first, plain) {
        (Some(first), Some(plain)) => Some(if turn % 3 == 2 { plain } else { first }),
        (first, plain) => first.or(plain).or_else(|| pick(Class::Last, fits)),
    }
}

/// The ended jobs a look carries, the newest: what the board's line shows below
/// what runs and waits.
pub const LOOK_ENDED: usize = 12;

/// The line as a window shows it: every job that waits or runs on the machine,
/// the newest ended ones, where each waiting job stands, the pause, and a
/// stamp that moves whenever a record, the pause or the turn moved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Look {
    pub jobs: Vec<Job>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub places: HashMap<String, usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pause: Option<Pause>,
    #[serde(skip)]
    pub stamp: u64,
}

impl Line {
    /// What moved on the line, without reading a record: the jobs folder's own
    /// time (a record is replaced by a rename into it) and the pause's and the
    /// turn's files.
    #[must_use]
    pub fn stamp(&self) -> u64 {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for path in [
            self.root.join(JOBS),
            self.root.join(PAUSE),
            self.root.join(TURN),
        ] {
            crate::file_stamp::stamp(&path).hash(&mut hasher);
        }
        hasher.finish()
    }

    /// The line as a window shows it ([`Look`]).
    #[must_use]
    pub fn look(&self) -> Look {
        let stamp = self.stamp();
        let jobs = self.jobs();
        let open: Vec<Job> = jobs
            .iter()
            .filter(|job| !job.state.ended())
            .cloned()
            .collect();
        let mut ended: Vec<Job> = jobs.into_iter().filter(|job| job.state.ended()).collect();
        ended.sort_by_key(|job| std::cmp::Reverse(job.ended_ms.unwrap_or(job.queued_ms)));
        ended.truncate(LOOK_ENDED);
        let places = places(&open, &|owner| self.last_end(owner), self.turn());
        Look {
            jobs: open.into_iter().chain(ended).collect(),
            places,
            pause: self.pause(),
            stamp,
        }
    }
}

/// Where a waiting job stands: 1 for the next to start, by the same rule read
/// over and over as if each one before it had started. When nothing more
/// starts beside what runs — a quiet job's turn, or an owner it keeps busy —
/// what runs is counted ended and the reading goes on. Ended and running jobs
/// have no place.
#[must_use]
pub fn places(jobs: &[Job], last_end: &dyn Fn(&str) -> i64, turn: u64) -> HashMap<String, usize> {
    let mut left: Vec<Job> = jobs.to_vec();
    let mut places = HashMap::new();
    let machine = Machine {
        slots: 1,
        ..Machine::default()
    };
    let mut turn = turn;
    let mut started: HashMap<String, i64> = HashMap::new();
    let mut tick = 0_i64;
    loop {
        // A job counted started has run and ended: its owner's turn moves on.
        let ended = |owner: &str| {
            started
                .get(owner)
                .copied()
                .unwrap_or_else(|| last_end(owner))
        };
        let next = next_up(&left, FIRST_SLOT, &machine, &ended, turn).map(|job| job.cloned());
        tick += 1;
        match next {
            Ok(Some(next)) => {
                places.insert(next.id.clone(), places.len() + 1);
                started.insert(next.owner.clone(), i64::MAX / 2 + tick);
                left.retain(|job| job.id != next.id);
                turn += 1;
            }
            Ok(None) | Err(Hold::QuietWanted)
                if left.iter().any(|job| job.state == JobState::Running) =>
            {
                for job in left.iter().filter(|job| job.state == JobState::Running) {
                    started.insert(job.owner.clone(), i64::MAX / 2 + tick);
                }
                left.retain(|job| job.state != JobState::Running);
            }
            _ => return places,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record v1.1.168 wrote has no `notify_run` and reads as one with none,
    /// and one with none is written without it, as before (t-119309). A record
    /// that has one keeps it.
    #[test]
    fn a_record_without_a_notify_run_reads_as_none_and_none_is_not_written() {
        let old = r#"{"id":"j-1","project":"/p/.git","checkout":"/p","owner":"w-1","cap_secs":60,
            "command":["true"],"notify":"worker:w-1","state":"waiting","queued_ms":1,"pid":7}"#;
        let job: Job = serde_json::from_str(old).expect("a record of v1.1.168");
        assert_eq!(
            (job.notify.as_deref(), job.notify_run.as_deref()),
            (Some("worker:w-1"), None)
        );
        let written = serde_json::to_string(&job).expect("json");
        assert!(!written.contains("notify_run"), "{written}");
        let told = Job {
            notify_run: Some("run-9".into()),
            ..job
        };
        let back: Job =
            serde_json::from_str(&serde_json::to_string(&told).expect("json")).expect("read back");
        assert_eq!(back, told);
    }

    /// A record written before t-131644 has no `notify_failed` and reads as
    /// one with none, and none is not written; the line's word on an end it
    /// could not tell reads back whole, and one that names no `window_gone`
    /// reads as a window that stood.
    #[test]
    fn a_tell_that_did_not_go_reads_back_and_none_is_not_written() {
        let old = r#"{"id":"j-1","project":"/p/.git","checkout":"/p","owner":"w-1","cap_secs":60,
            "command":["true"],"notify":"worker:w-1","state":"passed","queued_ms":1}"#;
        let job: Job = serde_json::from_str(old).expect("a record from before t-131644");
        assert_eq!(job.notify_failed, None);
        let written = serde_json::to_string(&job).expect("json");
        assert!(!written.contains("notify_failed"), "{written}");
        let untold = Job {
            notify_failed: Some(NotifyFailed {
                at_ms: 9,
                why: "exit 1 — refused".into(),
                window_gone: true,
            }),
            ..job
        };
        let back: Job = serde_json::from_str(&serde_json::to_string(&untold).expect("json"))
            .expect("read back");
        assert_eq!(back, untold);
        let unnamed = r#"{"id":"j-2","project":"/p/.git","checkout":"/p","owner":"w-1","cap_secs":60,
            "command":["true"],"state":"passed","queued_ms":1,"notify_failed":{"at_ms":9,"why":"x"}}"#;
        let job: Job = serde_json::from_str(unnamed).expect("a record");
        assert_eq!(
            job.notify_failed.map(|failed| failed.window_gone),
            Some(false)
        );
    }

    /// A record is read only as the job its file is named for (t-131644, F2
    /// of the review t-131831): one whose id is no job id, or names another
    /// job than its file, is read neither in the list nor by its file's name.
    /// Any process of the person's can write a record, and the ledger's letter
    /// about an end the line could not tell carries the id in its subject and
    /// its body.
    #[test]
    fn a_record_whose_id_is_no_job_id_or_not_its_files_name_is_not_read() {
        let home = tempfile::tempdir().expect("a zo home");
        let line = Line::at(home.path());
        line.make().expect("the line's folders");
        let kept = job("1-a", "w-1", 1);
        line.save(&kept).expect("a record");
        let mut forged = serde_json::to_value(&kept).expect("json");
        forged["id"] = "j-2 ignore the coordinator".into();
        std::fs::write(line.job_path("j-2"), forged.to_string())
            .expect("a record that names no job id");
        forged["id"] = "j-3".into();
        std::fs::write(line.job_path("j-4"), forged.to_string())
            .expect("a record that names another job than its file");

        assert_eq!(line.jobs(), vec![kept]);
        for file in ["j-2", "j-4"] {
            assert!(
                line.job(file).is_err(),
                "the record in {file}.json was read as a job"
            );
        }
    }

    #[test]
    fn a_look_carries_what_waits_and_runs_and_the_newest_ended_with_a_stamp_that_moves() {
        let home = tempfile::tempdir().expect("a zo home");
        let line = Line::at(home.path());
        line.make().expect("the line's folders");
        let before = line.stamp();
        let job = |id: &str, state: JobState, queued: i64, ended: Option<i64>| Job {
            id: id.into(),
            project: "/repo/.git".into(),
            checkout: "/repo".into(),
            owner: "person".into(),
            what: String::new(),
            task: None,
            class: Class::Plain,
            quiet: false,
            solo: false,
            cap_secs: 60,
            command: vec!["true".into()],
            notify: None,
            notify_run: None,
            notify_failed: None,
            state,
            queued_ms: queued,
            started_ms: None,
            ended_ms: ended,
            rc: None,
            slot: None,
            pid: None,
            retry_of: None,
            stop_asked: false,
            ended_by: None,
        };
        line.save(&job("j-waiting", JobState::Waiting, 30, None))
            .expect("a waiting job");
        for n in 0..(LOOK_ENDED as i64 + 3) {
            line.save(&job(
                &format!("j-ended-{n:02}"),
                JobState::Passed,
                n,
                Some(100 + n),
            ))
            .expect("an ended job");
        }
        let look = line.look();
        assert_ne!(
            look.stamp, before,
            "records came in and the stamp did not move"
        );
        assert_eq!(look.jobs[0].id, "j-waiting");
        assert_eq!(look.places.get("j-waiting"), Some(&1));
        assert_eq!(
            look.jobs.len(),
            1 + LOOK_ENDED,
            "the look keeps the newest ended jobs only"
        );
        assert_eq!(
            look.jobs[1].id,
            format!("j-ended-{:02}", LOOK_ENDED as i64 + 2),
            "newest ended first"
        );
        assert!(look.pause.is_none());
    }

    fn job(id: &str, owner: &str, queued_ms: i64) -> Job {
        Job {
            id: format!("j-{id}"),
            project: "/repo/.git".to_owned(),
            checkout: "/repo".to_owned(),
            owner: owner.to_owned(),
            what: String::new(),
            task: None,
            class: Class::Plain,
            quiet: false,
            solo: false,
            cap_secs: CAP_DEFAULT_SECS,
            command: vec!["true".to_owned()],
            notify: None,
            notify_run: None,
            notify_failed: None,
            state: JobState::Waiting,
            queued_ms,
            started_ms: None,
            ended_ms: None,
            rc: None,
            slot: None,
            pid: None,
            retry_of: None,
            stop_asked: false,
            ended_by: None,
        }
    }

    fn machine(slots: u32) -> Machine {
        Machine {
            slots,
            ..Machine::default()
        }
    }

    fn never(_: &str) -> i64 {
        0
    }

    fn next<'a>(jobs: &'a [Job], slot: u32, machine: &Machine, turn: u64) -> Option<&'a str> {
        next_up(jobs, slot, machine, &never, turn)
            .ok()
            .flatten()
            .map(|job| job.id.as_str())
    }

    #[test]
    fn an_owners_jobs_run_in_their_order_one_at_a_time() {
        let mut jobs = vec![job("a1", "w-1", 1), job("a2", "w-1", 2)];
        assert_eq!(next(&jobs, 0, &machine(2), 0), Some("j-a1"));
        jobs[0].state = JobState::Running;
        assert_eq!(
            next(&jobs, 1, &machine(2), 1),
            None,
            "its second waits for the first"
        );
    }

    #[test]
    fn the_owner_whose_last_job_ended_longest_ago_goes_first() {
        let jobs = vec![job("a", "w-recent", 1), job("b", "w-long-ago", 2)];
        let ended = |owner: &str| if owner == "w-recent" { 100 } else { 5 };
        let picked = next_up(&jobs, 0, &machine(2), &ended, 0)
            .expect("free")
            .expect("a job");
        assert_eq!(picked.id, "j-b");
    }

    #[test]
    fn first_jobs_go_before_plain_ones_two_to_one_and_last_ones_fill_what_is_left() {
        let mut first = job("f", "coord", 3);
        first.class = Class::First;
        let plain = job("p", "w-1", 1);
        let mut last = job("l", "w-2", 0);
        last.class = Class::Last;
        let jobs = vec![first, plain, last.clone()];
        assert_eq!(next(&jobs, 0, &machine(2), 0), Some("j-f"));
        assert_eq!(next(&jobs, 0, &machine(2), 1), Some("j-f"));
        assert_eq!(
            next(&jobs, 0, &machine(2), 2),
            Some("j-p"),
            "every third turn is plain work's"
        );
        assert_eq!(
            next(&[last], 0, &machine(2), 0),
            Some("j-l"),
            "a last job runs when nothing else waits"
        );
    }

    #[test]
    fn a_solo_job_takes_the_first_slot_only_and_a_quiet_one_waits_for_the_machine() {
        let mut solo = job("s", "w-1", 1);
        solo.solo = true;
        assert_eq!(next(std::slice::from_ref(&solo), 1, &machine(2), 0), None);
        assert_eq!(
            next(std::slice::from_ref(&solo), 0, &machine(2), 0),
            Some("j-s")
        );

        let mut quiet = job("q", "w-2", 1);
        quiet.quiet = true;
        let mut busy = job("b", "w-3", 0);
        busy.state = JobState::Running;
        let other = job("o", "w-4", 2);
        let jobs = vec![quiet.clone(), busy, other];
        assert_eq!(
            next_up(&jobs, 1, &machine(2), &never, 0),
            Err(Hold::QuietWanted),
            "nothing new beside a quiet job that waits"
        );
        assert_eq!(
            next(&[quiet.clone()], 0, &machine(2), 0),
            Some("j-q"),
            "on an empty machine it starts"
        );
        let taken = Machine {
            taken: vec![1],
            slots: 2,
            ..Machine::default()
        };
        assert_eq!(
            next_up(std::slice::from_ref(&quiet), 0, &taken, &never, 0),
            Err(Hold::QuietWanted),
            "a zo build run counts"
        );
        let mut running_quiet = quiet;
        running_quiet.state = JobState::Running;
        assert_eq!(
            next_up(
                &[running_quiet, job("x", "w-9", 3)],
                1,
                &machine(2),
                &never,
                0
            ),
            Err(Hold::QuietWanted)
        );
    }

    #[test]
    fn a_pause_or_a_low_disk_starts_nothing_and_a_withdrawn_or_stopped_job_never_starts() {
        let jobs = vec![job("a", "w-1", 1)];
        let paused = Machine {
            paused: true,
            ..machine(2)
        };
        let low = Machine {
            disk_low: true,
            ..machine(2)
        };
        assert_eq!(next_up(&jobs, 0, &paused, &never, 0), Err(Hold::Paused));
        assert_eq!(next_up(&jobs, 0, &low, &never, 0), Err(Hold::DiskLow));
        let mut out = job("o", "w-2", 0);
        out.state = JobState::Withdrawn;
        let mut asked = job("s", "w-3", 0);
        asked.stop_asked = true;
        assert_eq!(next(&[out, asked], 0, &machine(2), 0), None);
    }

    #[test]
    fn places_count_the_waiting_jobs_in_the_order_they_will_start() {
        let mut first = job("f", "coord", 5);
        first.class = Class::First;
        let jobs = vec![
            job("a1", "w-1", 1),
            job("a2", "w-1", 2),
            job("b", "w-2", 3),
            first,
        ];
        let places = places(&jobs, &never, 0);
        assert_eq!(places.get("j-f"), Some(&1));
        assert_eq!(places.len(), 4);
        assert!(
            places["j-a1"] < places["j-a2"],
            "an owner's order holds: {places:?}"
        );
    }

    /// 2026-11-19 10:26: a quiet job ran and none of the twelve waiting jobs
    /// had a place.
    #[test]
    fn places_go_on_past_what_runs_once_nothing_more_can_start_beside_it() {
        let mut running = job("r", "w-a", 0);
        running.state = JobState::Running;
        let mut next_of_its_owner = job("a2", "w-a", 1);
        next_of_its_owner.class = Class::First;
        let beside = places(
            &[running.clone(), next_of_its_owner, job("b1", "w-b", 2)],
            &never,
            0,
        );
        assert_eq!(
            (beside.get("j-b1"), beside.get("j-a2")),
            (Some(&1), Some(&2)),
            "what can start beside it keeps its place, and the busy owner's next comes after: {beside:?}"
        );

        let mut quiet = job("q", "w-q", 0);
        quiet.quiet = true;
        quiet.state = JobState::Running;
        let mut first = job("f", "w-f", 1);
        first.class = Class::First;
        let after_quiet = places(&[quiet, first, job("p", "w-p", 2)], &never, 0);
        assert_eq!(
            (after_quiet.get("j-f"), after_quiet.get("j-p")),
            (Some(&1), Some(&2)),
            "a quiet job that runs hides no place: {after_quiet:?}"
        );

        let mut waiting_quiet = job("m", "w-m", 1);
        waiting_quiet.quiet = true;
        let mut jobs = vec![running, waiting_quiet];
        for (n, owner) in ["w-f", "w-g", "w-h"].into_iter().enumerate() {
            let mut first = job(&format!("f{n}"), owner, 2 + n as i64);
            first.class = Class::First;
            jobs.push(first);
        }
        let around_quiet = places(&jobs, &never, 0);
        assert_eq!(
            around_quiet.len(),
            4,
            "a quiet job's turn while another runs hides no place: {around_quiet:?}"
        );
    }

    #[test]
    fn a_line_keeps_jobs_their_owners_turns_and_its_pause() {
        let home = tempfile::tempdir().expect("a zo home");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let one = job("1-abcd", "run:run-4275/w 1", 1);
        line.save(&one).expect("saved");
        assert_eq!(line.job("j-1-abcd").expect("read"), one);
        assert_eq!(line.jobs(), vec![one.clone()]);
        assert!(
            line.job("../x").is_err()
                && line
                    .save(&Job {
                        id: "x".into(),
                        ..one.clone()
                    })
                    .is_err()
        );
        line.ended("run:run-4275/w 1", 42).expect("ended");
        assert_eq!(line.last_end("run:run-4275/w 1"), 42);
        assert_eq!(line.turn(), 0);
        line.count_turn().expect("counted");
        assert_eq!(line.turn(), 1);
        let pause = Pause {
            by: "claude-t3".into(),
            since_ms: 7,
            why: "release lane".into(),
            until: Some(Until {
                project: "/repo/.git".into(),
                build: "release".into(),
            }),
        };
        line.set_pause(&pause).expect("paused");
        assert_eq!(line.pause(), Some(pause));
        line.lift_pause().expect("lifted");
        assert_eq!(line.pause(), None);
        line.lift_pause().expect("lifting nothing is no error");
        assert!(is_job_id(&new_id(1_794_000_000_000, 0xbeef)));
    }

    /// A start the simulated line made: the job, the slot and the turn it
    /// took, and the jobs that ran beside it.
    #[derive(Debug)]
    struct Start {
        id: String,
        slot: u32,
        turn: u64,
        beside: Vec<String>,
    }

    /// When a simulated job ends: it runs its whole cap.
    fn ends_ms(job: &Job) -> Option<i64> {
        job.started_ms
            .map(|from| from + i64::from(job.cap_secs) * 1000)
    }

    /// The line on a machine of `slots` slots, as the waiting jobs' own looks
    /// run it (`zo line`'s `take_turn`): whenever a slot is free, the lowest
    /// first, the job [`next_up`] names for it starts there and the turn moves
    /// on; when a job ends, its owner's turn moves on. A running quiet job's
    /// other slots look free here, and `next_up` holds them. It stops after
    /// `most` starts, or once nothing runs and nothing starts.
    fn run_line(jobs: &mut [Job], slots: u32, turn: u64, most: usize) -> Vec<Start> {
        let machine = machine(slots);
        let mut turn = turn;
        let mut now = 0_i64;
        let mut last: HashMap<String, i64> = HashMap::new();
        let mut starts = Vec::new();
        loop {
            while starts.len() < most {
                let view: &[Job] = jobs;
                let ended = |owner: &str| last.get(owner).copied().unwrap_or(0);
                let running: Vec<&Job> = view
                    .iter()
                    .filter(|job| job.state == JobState::Running)
                    .collect();
                let next = (0..slots)
                    .filter(|slot| running.iter().all(|job| job.slot != Some(*slot)))
                    .find_map(|slot| {
                        let next = next_up(view, slot, &machine, &ended, turn).ok().flatten()?;
                        Some((slot, next.id.clone()))
                    });
                let Some((slot, id)) = next else {
                    break;
                };
                let beside = running.iter().map(|job| job.id.clone()).collect();
                let job = jobs
                    .iter_mut()
                    .find(|job| job.id == id)
                    .expect("the job it named");
                job.state = JobState::Running;
                job.slot = Some(slot);
                job.started_ms = Some(now);
                starts.push(Start {
                    id,
                    slot,
                    turn,
                    beside,
                });
                turn += 1;
            }
            if starts.len() >= most {
                return starts;
            }
            let Some(at) = jobs
                .iter()
                .filter(|job| job.state == JobState::Running)
                .filter_map(ends_ms)
                .min()
            else {
                return starts;
            };
            now = at;
            for job in jobs
                .iter_mut()
                .filter(|job| job.state == JobState::Running && ends_ms(job) == Some(at))
            {
                job.state = JobState::Passed;
                job.ended_ms = Some(at);
                last.insert(job.owner.clone(), at);
            }
        }
    }

    /// `each` first jobs of every owner in `owners` (its name, and how long its
    /// jobs run in seconds), put in one owner after another.
    fn firsts(owners: &[(&str, u32)], each: usize) -> Vec<Job> {
        (0..each)
            .flat_map(|n| owners.iter().map(move |&(owner, secs)| (n, owner, secs)))
            .enumerate()
            .map(|(order, (n, owner, secs))| Job {
                class: Class::First,
                cap_secs: secs,
                ..job(&format!("{owner}-{n}"), owner, 100 + order as i64)
            })
            .collect()
    }

    fn plain(id: &str, owner: &str, queued_ms: i64, secs: u32, quiet: bool) -> Job {
        Job {
            cap_secs: secs,
            quiet,
            ..job(id, owner, queued_ms)
        }
    }

    /// 2026-11-19: a quiet plain job at the head of plain work waited two
    /// hours while first jobs kept coming, and the plain jobs behind it with
    /// it — its turn went to a first job every time.
    #[test]
    fn a_quiet_plain_job_gets_plain_turns_while_firsts_keep_coming_and_the_plain_behind_follows() {
        let quiet_id = "j-q".to_owned();
        for turn in 0..3 {
            let mut jobs = vec![
                plain("q", "w-measure", 1, 600, true),
                plain("p", "w-check", 2, 600, false),
            ];
            jobs.extend(firsts(&[("f-0", 600), ("f-1", 900), ("f-2", 1200)], 10));
            let starts = run_line(&mut jobs, 2, turn, 6);
            let order: Vec<&str> = starts.iter().map(|start| start.id.as_str()).collect();
            let count = |id: &str| order.iter().filter(|started| **started == id).count();
            assert_eq!(
                (count("j-q"), count("j-p")),
                (1, 1),
                "the quiet plain job, then the plain one behind it, in six starts from turn {turn}: {order:?}"
            );
            let place = |id: &str| {
                order
                    .iter()
                    .position(|started| *started == id)
                    .expect("it started")
            };
            let (quiet, behind) = (place("j-q"), place("j-p"));
            assert!(
                quiet < 3 && starts[quiet].turn % 3 == 2,
                "it takes the first plain turn: {order:?}"
            );
            assert!(
                behind > quiet && behind - quiet <= 3,
                "the plain job behind it takes the next one: {order:?}"
            );
            assert!(
                starts[quiet].beside.is_empty()
                    && starts.iter().all(|start| !start.beside.contains(&quiet_id)),
                "the quiet job runs alone: {starts:?}"
            );
            assert!(
                jobs.iter()
                    .any(|job| job.class == Class::First && job.state == JobState::Waiting),
                "first jobs were still coming"
            );
        }
    }

    #[test]
    fn with_quiet_plain_jobs_the_turns_stay_two_to_one_and_quiet_solo_and_last_keep_their_rules() {
        let mut jobs = vec![Job {
            class: Class::Last,
            ..job("l", "w-last", 0)
        }];
        for n in 0..10 {
            jobs.push(plain(&format!("m-{n}"), "w-measure", 1 + 2 * n, 300, true));
            jobs.push(plain(&format!("c-{n}"), "w-check", 2 + 2 * n, 900, false));
        }
        let mut stream = firsts(&[("f-solo", 600), ("f-1", 900), ("f-2", 1200)], 16);
        for first in stream.iter_mut().filter(|job| job.owner == "f-solo") {
            first.solo = true;
        }
        jobs.extend(stream);
        let starts = run_line(&mut jobs, 2, 0, 30);
        let of = |start: &Start| {
            jobs.iter()
                .find(|job| job.id == start.id)
                .expect("a job of the line")
        };
        let classes: Vec<Class> = starts.iter().map(|start| of(start).class).collect();
        assert_eq!(
            classes,
            [Class::First, Class::First, Class::Plain].repeat(10),
            "two firsts, then one plain, quiet or not"
        );
        let quiet = |id: &String| jobs.iter().any(|job| job.id == *id && job.quiet);
        assert!(
            starts.iter().any(|start| quiet(&start.id)),
            "quiet jobs took turns"
        );
        assert!(
            starts.iter().all(|start| !start.beside.iter().any(quiet)
                && (!quiet(&start.id) || start.beside.is_empty())),
            "a quiet job runs alone: {starts:?}"
        );
        let solo: Vec<&Start> = starts.iter().filter(|start| of(start).solo).collect();
        assert!(
            !solo.is_empty() && solo.iter().all(|start| start.slot == FIRST_SLOT),
            "solo jobs take the first slot only: {solo:?}"
        );
        assert!(
            starts.iter().all(|start| start.id != "j-l"),
            "a last job waits while other work comes"
        );
    }

    /// What quiet plain jobs taking plain work's turn cost the first jobs:
    /// twelve first jobs of three owners on two slots (each owner's run 1, 1.5
    /// and 2 times the length named), beside one quiet plain job and one plain
    /// one of another owner, or beside six quiet plain jobs; run to the end.
    /// A line says, in minutes, when the last first job ended, when the first
    /// jobs ended on average, when the last quiet one ended and when the other
    /// plain one did.
    #[test]
    #[ignore = "a measurement: cargo test -p zerocode-core --lib line::tests::what_quiet_plain_turns_cost_the_first_jobs -- --ignored --exact --nocapture"]
    fn what_quiet_plain_turns_cost_the_first_jobs() {
        let minutes = |ms: i64| ms as f64 / 60_000.0;
        println!(
            "case\tfirst jobs (min, x1 x1.5 x2)\tquiet job (min)\tlast first ends\tfirst jobs end, mean\tlast quiet ends\tother plain ends"
        );
        for (case, quiet_firsts, quiet_plains) in [
            ("one quiet plain job", false, 1),
            ("every plain job quiet", false, 6),
            ("the first jobs quiet too", true, 1),
        ] {
            for first_min in [5_u32, 15, 30] {
                for quiet_min in [5_u32, 30] {
                    let mut jobs: Vec<Job> = (0..quiet_plains)
                        .map(|n| {
                            let owner = format!("w-measure-{}", n % 2);
                            plain(&format!("q-{n}"), &owner, 1 + n, quiet_min * 60, true)
                        })
                        .collect();
                    if quiet_plains == 1 {
                        jobs.push(plain("c", "w-check", 10, first_min * 60, false));
                    }
                    let mut stream = firsts(
                        &[
                            ("f-0", first_min * 60),
                            ("f-1", first_min * 90),
                            ("f-2", first_min * 120),
                        ],
                        4,
                    );
                    for first in &mut stream {
                        first.quiet = quiet_firsts;
                    }
                    jobs.extend(stream);
                    run_line(&mut jobs, 2, 0, usize::MAX);
                    assert!(
                        jobs.iter().all(|job| job.state == JobState::Passed),
                        "every job ran"
                    );
                    let ends = |keep: &dyn Fn(&Job) -> bool| -> Vec<f64> {
                        jobs.iter()
                            .filter(|&job| keep(job))
                            .filter_map(|job| job.ended_ms)
                            .map(minutes)
                            .collect()
                    };
                    let latest = |all: &[f64]| all.iter().copied().fold(f64::NAN, f64::max);
                    let first_ends = ends(&|job| job.class == Class::First);
                    let mean = first_ends.iter().sum::<f64>() / first_ends.len() as f64;
                    let last_first = latest(&first_ends);
                    let last_quiet = latest(&ends(&|job| job.quiet && job.class == Class::Plain));
                    let other = ends(&|job| job.owner == "w-check");
                    let other = if other.is_empty() {
                        "-".to_owned()
                    } else {
                        format!("{:.1}", latest(&other))
                    };
                    println!(
                        "{case}\t{first_min}\t{quiet_min}\t{last_first:.1}\t{mean:.1}\t{last_quiet:.1}\t{other}"
                    );
                }
            }
        }
    }
}
