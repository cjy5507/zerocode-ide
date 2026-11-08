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
//! — two firsts, then one plain, so plain work is not starved — and a `last` job
//! only fills a slot nothing else wants. A `solo` job (a whole suite, an e2e
//! run) takes the first slot only; a `quiet` job (a measurement) runs alone,
//! and while it waits for the machine nothing new starts beside it. Nothing
//! starts while the line is paused or the disk is below its floor. The window
//! reads the records and never writes them: its buttons run `zo line`.

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
    serde_json::from_slice(&bytes).map_err(io::Error::other)
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

/// Two first jobs, then a plain one when both wait; a last job only when no other.
fn pick_ordered<'jobs>(
    pick: &Pick<'jobs, '_>,
    fits: &dyn Fn(&Job) -> bool,
    turn: u64,
) -> Option<&'jobs Job> {
    let first = pick(Class::First, fits);
    let plain = pick(Class::Plain, fits);
    match (first, plain) {
        (Some(first), Some(plain)) => Some(if turn % 3 == 2 && !plain.quiet {
            plain
        } else {
            first
        }),
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
/// over and over as if each one before it had started. Ended and running
/// jobs have no place.
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
    while let Ok(Some(next)) = {
        // A job counted started has run and ended: its owner's turn moves on.
        let ended = |owner: &str| {
            started
                .get(owner)
                .copied()
                .unwrap_or_else(|| last_end(owner))
        };
        next_up(&left, FIRST_SLOT, &machine, &ended, turn).map(|job| job.cloned())
    } {
        places.insert(next.id.clone(), places.len() + 1);
        tick += 1;
        started.insert(next.owner.clone(), i64::MAX / 2 + tick);
        left.retain(|job| job.id != next.id);
        turn += 1;
    }
    places
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
