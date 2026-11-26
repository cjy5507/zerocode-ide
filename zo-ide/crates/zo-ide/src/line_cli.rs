//! `zo line` — the machine's build line (`zerocode_core::line`): any project's
//! heavy command waits its turn and runs in one of the machine's build slots,
//! the same slots `zo build` runs in. `run` waits and runs the command here,
//! its output on this terminal and in the job's log, and exits as it did — the
//! place `cargo-turn.sh` held; `add` puts it in and returns at once, and a
//! waiter of its own runs it and tells the ledger how it ended. The rest look
//! and act: `list`, `log`, `retry`, `withdraw`, `front`, `stop`, `pause`,
//! `resume`, `forget` and `restore` — the board's buttons run these. Cargo
//! builds in its slot's own folder, kept from job to job (`slot_target`).

mod slot_target;

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead as _, BufReader, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use zerocode_core::agent_shims;
use zerocode_core::line::{self, Class, Hold, Job, JobState, Line, Machine, NotifyFailed, Pause, Until};
use zerocode_core::orchestration::{line_notify, WORKER_ADDRESS_PREFIX};
use zerocode_core::process_tree::{self, Ran};
use zerocode_core::shell_rule::kill::WINDOW_PID_ENV;

use crate::build_cli::now_ms;

pub const USAGE: &str = "\
zo line run  [--what <words>] [--task <t-id>] [--first | --last] [--solo] [--quiet] [--cap <30m|900s>]
             [--owner <name>] -- <command...>
zo line add  (the same flags) [--notify <address>] -- <command...>
zo line wait <job>
zo line list [--all] [--history]
zo line log <job>
zo line retry <job> · zo line withdraw <job> · zo line front <job> · zo line stop <job>
zo line pause [--until-build <build>] [--why <words>] · zo line resume
zo line forget <job> · zo line restore <job>

  One line for the whole machine: each owner's jobs run in the order they
  were put in and never two at a time; the owner whose last job ended longest
  ago goes first; --first jobs go before plain ones (two to one, --quiet ones
  too) and --last ones only fill a slot nothing else wants. --solo takes the
  first slot only (a whole suite); --quiet runs alone (a measurement): from
  its turn nothing new starts until the machine is empty. Nothing starts while
  the line is paused or the disk is below its floor; near it, the folders of
  idle slots go first. The floor is 16 GiB free, or the whole GiB that
  ZO_LINE_DISK_FLOOR_GIB names in the environment of the zo that put the job
  in (0 for none). Cargo builds in the slot's own folder (zo's home,
  build-slots/targets/<slot>), the job's workspace crates removed from it
  before and after, so only registry crates carry over; a job that names its
  CARGO_TARGET_DIR builds there, untouched. `run` exits as its
  command did; `add` prints the job and its cap and returns, a waiter of its
  own runs the job, and the job's end is told to --notify through the ledger;
  `wait` waits for a job's end and exits with its code: it runs nothing — the
  job's waiter does. A put whose command outran that cap last
  time is warned; a job the line ends (its cap, a stop) says so as its log's
  last line. `list` shows this project's waiting and running jobs and the ten
  that ended last, and counts the older ones; --history lists every job of
  this project, --all every job of every project. `wait <job>` on a job that
  has ended exits with its code too. Records live under zo's home (line/).
";

/// How often a waiting job looks for its turn, and a running one for a stop.
const LOOK_EVERY: Duration = Duration::from_millis(500);
/// How long a stopped command has to go before its group is killed.
const GRACE: Duration = Duration::from_secs(10);
/// A forgotten job's record waits this long in the trash.
const TRASH_KEPT_MS: i64 = 24 * 60 * 60 * 1000;
/// The folder of the files a job's waiter holds locked while it lives (`waiter_path`).
const WAITERS: &str = "waiters";
/// The variable that tells a `zo line wait` it is the waiter the line started for the job it
/// names: the one process that runs that job (`start_waiter`).
const WAITER_ENV: &str = "ZO_LINE_WAITER";
/// How long a process that started a waiter waits for it to hold its lock.
const WAITER_START: Duration = Duration::from_secs(2);
/// How often that process looks whether it does.
const WAITER_LOOK: Duration = Duration::from_millis(10);
/// The variable that names the line's disk floor in whole GiB, `0` for none
/// (`floor_gib`). A job's waiter has the environment of the `zo` that put the
/// job in, so the floor is that one's. The line's own integration tests give
/// their line none: below 17 GiB free they said `skipped:` and passed in no
/// time (t-119309).
const FLOOR_ENV: &str = "ZO_LINE_DISK_FLOOR_GIB";
/// How long after a job is put in its waiter may still be taking its lock:
/// `add` waits `WAITER_START` for it and writes the record, and on a very busy
/// machine the lock stands only after that (`end_orphans`).
const WAITER_GRACE_MS: i64 = 60_000;
/// The lines of a log `zo line log` prints.
const LOG_TAIL_LINES: usize = 200;
/// How a ledger address names a run's chair: `run:<run>`, found in that run.
const RUN_ADDRESS: &str = "run:";
/// How much of what a failed `zerocode-orc send` said the job's log keeps.
const SAID_CHARS: usize = 400;
/// The machine's slot folder under zo's home — `zo build`'s.
const SLOTS_FOLDER: &str = "build-slots";
const TRASH: &str = "trash";

/// What `zo line` ends with: a receipt to print, or the exit of a command it ran.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    Said(String),
    Ran(i32),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Put {
    what: String,
    task: Option<String>,
    class: Class,
    solo: bool,
    quiet: bool,
    cap_secs: Option<u32>,
    owner: Option<String>,
    notify: Option<String>,
    command: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Verb {
    Run(Put),
    Add(Put),
    Wait(String),
    List { all: bool, history: bool },
    Log(String),
    Retry(String),
    Withdraw(String),
    Front(String),
    Stop(String),
    Pause { until_build: Option<String>, why: String },
    Resume,
    Forget(String),
    Restore(String),
}

fn parse(args: &[String]) -> Result<Verb, String> {
    let Some((verb, rest)) = args.split_first() else {
        return Err(USAGE.to_owned());
    };
    let one = |what: &str| -> Result<String, String> {
        match rest {
            [job] if line::is_job_id(job) => Ok(job.clone()),
            [job] => Err(format!("`{job}` is no job id (j-…)")),
            _ => Err(format!("`zo line {what}` takes one job id")),
        }
    };
    Ok(match verb.as_str() {
        "run" | "add" => {
            let put = parse_put(rest, verb == "add")?;
            if verb == "run" { Verb::Run(put) } else { Verb::Add(put) }
        }
        "wait" => Verb::Wait(one("wait")?),
        "list" => {
            let (mut all, mut history) = (false, false);
            for flag in rest {
                match flag.as_str() {
                    "--all" => all = true,
                    "--history" => history = true,
                    _ => return Err(format!("unknown argument for `zo line list`\n\n{USAGE}")),
                }
            }
            Verb::List { all, history }
        }
        "log" => Verb::Log(one("log")?),
        "retry" => Verb::Retry(one("retry")?),
        "withdraw" => Verb::Withdraw(one("withdraw")?),
        "front" => Verb::Front(one("front")?),
        "stop" => Verb::Stop(one("stop")?),
        "forget" => Verb::Forget(one("forget")?),
        "restore" => Verb::Restore(one("restore")?),
        "resume" if rest.is_empty() => Verb::Resume,
        "pause" => {
            let (mut until_build, mut why) = (None, String::new());
            let mut words = rest.iter();
            while let Some(flag) = words.next() {
                let mut value = || words.next().cloned().ok_or_else(|| format!("{flag} requires a value"));
                match flag.as_str() {
                    "--until-build" => until_build = Some(value()?),
                    "--why" => why = value()?,
                    _ => return Err(format!("unknown argument `{flag}`\n\n{USAGE}")),
                }
            }
            Verb::Pause { until_build, why }
        }
        other => return Err(format!("unknown `zo line` verb `{other}`\n\n{USAGE}")),
    })
}

fn parse_put(args: &[String], adds: bool) -> Result<Put, String> {
    let mut put = Put::default();
    let mut words = args.iter();
    while let Some(flag) = words.next() {
        if flag == "--" {
            put.command = words.by_ref().cloned().collect();
            break;
        }
        let mut value = || words.next().cloned().ok_or_else(|| format!("{flag} requires a value"));
        match flag.as_str() {
            "--what" => put.what = value()?,
            "--task" => put.task = Some(value()?),
            "--first" => put.class = Class::First,
            "--last" => put.class = Class::Last,
            "--solo" => put.solo = true,
            "--quiet" => put.quiet = true,
            "--cap" => put.cap_secs = Some(cap_secs(&value()?)?),
            "--owner" => put.owner = Some(value()?),
            "--notify" if adds => put.notify = Some(value()?),
            _ => return Err(format!("unknown argument `{flag}`\n\n{USAGE}")),
        }
    }
    if put.command.is_empty() || put.command.iter().all(|word| word.trim().is_empty()) {
        return Err("name the command after `--`".to_owned());
    }
    Ok(put)
}

/// `30m`, `900s`, `2h` or a number of seconds.
fn cap_secs(word: &str) -> Result<u32, String> {
    let (number, unit) = word.split_at(word.find(|c: char| !c.is_ascii_digit()).unwrap_or(word.len()));
    let number: u32 = number.parse().map_err(|_| format!("--cap takes 30m, 900s or 2h, not `{word}`"))?;
    let secs = match unit {
        "" | "s" => number,
        "m" => number.saturating_mul(60),
        "h" => number.saturating_mul(3600),
        _ => return Err(format!("--cap takes 30m, 900s or 2h, not `{word}`")),
    };
    if secs == 0 {
        return Err("--cap must be above zero".to_owned());
    }
    Ok(secs)
}

/// `zo line …` from `cwd`.
///
/// # Errors
///
/// One sentence a person reads: why nothing was done.
pub fn run(args: &[String], cwd: &Path) -> Result<Ended, String> {
    let verb = parse(args)?;
    let floor_gib = floor_gib(std::env::var_os(FLOOR_ENV).as_deref())?;
    let home = line::zo_home().ok_or_else(|| "no home folder to keep the line in".to_owned())?;
    let line = Line::at(&home);
    line.make().map_err(|error| error.to_string())?;
    let slots = home.join(SLOTS_FOLDER);
    let at_once = u32::try_from(
        crate::preferences::load()
            .builds_at_once
            .unwrap_or(crate::build_runner::AT_ONCE_DEFAULT)
            .max(1),
    )
    .unwrap_or(1);
    let machine = MachineRules {
        slots,
        at_once,
        floor_gib,
        look_every: LOOK_EVERY,
        env: |name| std::env::var_os(name),
        free: free_bytes,
        start_waiter,
    };
    match verb {
        Verb::Run(put) => {
            let job = put_in(&line, &put, cwd)?;
            // The job's name, cap and log on stderr: the command's own output goes to stdout as it runs.
            eprintln!("zo line: {} · cap {} · {}", job.id, cap_words(job.cap_secs), line.log_path(&job.id).display());
            if let Some(warning) = cap_warning(&line, &job) {
                eprintln!("{warning}");
            }
            run_job(&line, &machine, &job.id, true).map(Ended::Ran)
        }
        Verb::Add(put) => {
            let job = put_with_waiter(&line, &machine, bound_to_this_pane(job_of(&put, cwd)))?;
            let places = line::places(&line.jobs(), &|owner| line.last_end(owner), line.turn());
            put_receipt(&line, &job, json!({ "job": job.id, "place": places.get(&job.id) }))
        }
        Verb::Wait(id) => wait(&line, &machine, &id, std::env::var_os(WAITER_ENV).as_deref()).map(Ended::Ran),
        Verb::List { all, history } => listing(&line, cwd, all, history),
        Verb::Log(id) => log_tail(&line, &id),
        Verb::Retry(id) => {
            let old = line.job(&id).map_err(|error| error.to_string())?;
            let now = now_ms();
            let again = Job {
                id: line::new_id(now, salt(now)),
                state: JobState::Waiting,
                queued_ms: now,
                started_ms: None,
                ended_ms: None,
                rc: None,
                slot: None,
                pid: None,
                retry_of: Some(old.id.clone()),
                stop_asked: false,
                ended_by: None,
                // An end the first could not tell is the first's: the window tells it once (t-131644).
                notify_failed: None,
                ..old
            };
            // A new job's end, bound by the pane that puts it in again, in the run its first was told in.
            let again = put_with_waiter(&line, &machine, bound_to_this_pane(again))?;
            // A retry keeps its job's cap: a job that ended past it is told so before it runs into it again.
            put_receipt(&line, &again, json!({ "job": again.id, "retry_of": id }))
        }
        Verb::Withdraw(id) => change(&line, &id, |job| {
            if job.state != JobState::Waiting {
                return Err(format!("{id} is not waiting; `zo line stop` stops a running job"));
            }
            job.state = JobState::Withdrawn;
            job.ended_ms = Some(now_ms());
            job.ended_by = Some(who_asks());
            Ok(())
        }),
        Verb::Front(id) => change(&line, &id, |job| {
            if job.state != JobState::Waiting {
                return Err(format!("{id} is not waiting"));
            }
            job.class = Class::First;
            Ok(())
        }),
        Verb::Stop(id) => change(&line, &id, |job| match job.state {
            JobState::Running | JobState::Waiting => {
                job.stop_asked = true;
                job.ended_by = Some(who_asks());
                Ok(())
            }
            _ => Err(format!("{id} has ended")),
        }),
        Verb::Pause { until_build, why } => {
            let until = until_build.map(|build| Until { project: project_of(cwd), build });
            let pause = Pause { by: who_asks(), since_ms: now_ms(), why, until };
            line.set_pause(&pause).map_err(|error| error.to_string())?;
            said(&json!({ "paused": true }))
        }
        Verb::Resume => {
            line.lift_pause().map_err(|error| error.to_string())?;
            said(&json!({ "paused": false }))
        }
        Verb::Forget(id) => forget(&line, &id),
        Verb::Restore(id) => restore(&line, &id),
    }
}

fn said(value: &serde_json::Value) -> Result<Ended, String> {
    serde_json::to_string(value).map(Ended::Said).map_err(|error| error.to_string())
}

/// The chance part of a job id put in at `now`: the process, the moment, and
/// how many ids this process made before it. Two jobs one process puts in
/// within one millisecond would otherwise share an id, and the second's record
/// would take the first's place.
fn salt(now: i64) -> u32 {
    static MADE: AtomicU32 = AtomicU32::new(0);
    std::process::id() ^ u32::try_from(now & 0xffff).unwrap_or(0) ^ MADE.fetch_add(1, Ordering::Relaxed)
}

/// Who acts: the worker or run the ledger named in this process's
/// environment, else the person.
fn who_asks() -> String {
    ["ZEROCODE_WORKER", "ZEROCODE_RUN"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.trim().is_empty()))
        .unwrap_or_else(|| "person".to_owned())
}

/// The project a folder belongs to: its repository's shared git folder, or the
/// folder itself outside a repository.
fn project_of(cwd: &Path) -> String {
    cwd.ancestors()
        .find_map(zerocode_core::git_dir::shared_of)
        .unwrap_or_else(|| cwd.to_path_buf())
        .display()
        .to_string()
}

fn put_in(line: &Line, put: &Put, cwd: &Path) -> Result<Job, String> {
    let job = job_of(put, cwd);
    line.save(&job).map_err(|error| error.to_string())?;
    Ok(job)
}

/// Puts `job` in with a waiter of its own, under the line's lock: the waiter
/// holds its lock before any look sees the job, and the job's record names it
/// (`pid`) from the start. A waiter that cannot start puts nothing in.
fn put_with_waiter(line: &Line, machine: &MachineRules, mut job: Job) -> Result<Job, String> {
    let _lock = lock(line)?;
    job.pid = Some((machine.start_waiter)(line, &job.id)?);
    line.save(&job).map_err(|error| error.to_string())?;
    Ok(job)
}

/// `job`, its end bound by the window to this process's pane when the ledger
/// could tell it at its address — a worker, a run's chair (`line-notify`,
/// t-131644): a window restart leaves the line no pane to tell an end from, and
/// the ledger tells only an end its pane bound. Told in the run this pane
/// stands in when its address is one another run cannot find — a worker, a
/// pane (`notify_run`, t-119309): the run the binding answers, else
/// `run-current`'s. A window that cannot bind costs one warning line on
/// stderr and the patience at most, never the put; one that does not answer at
/// all is not asked its run as well. Asked before the line's lock is taken,
/// and the job is queued from when the asking is over: the ledger tells only
/// an end bound no later than its job was put in, since another pane can read
/// the job's id off the line once it is in it (review t-131848).
fn bound_to_this_pane(mut job: Job) -> Job {
    let Some(to) = job.notify.clone() else {
        return job;
    };
    // One deadline for every question the put asks the window, the end of one that runs out included.
    let until = Instant::now() + ASK_PATIENCE;
    let in_a_run = to.starts_with(RUN_ADDRESS);
    // Whether this pane's run is still to be asked: an address another run cannot find, no run yet.
    let mut ask_run = !in_a_run && job.notify_run.is_none();
    if in_a_run || to.starts_with(WORKER_ADDRESS_PREFIX) {
        match bind(&job.id, &to, job.notify_run.as_deref(), until) {
            Ok(run) => {
                if !in_a_run {
                    job.notify_run = Some(run);
                }
                ask_run = false;
            }
            Err(unbound) => {
                eprintln!(
                    "zo line: warning — the window did not bind {}'s end to this pane ({}); if it \
                     restarts before the job ends, the ledger will not tell {to} how it ended",
                    job.id,
                    unbound.why
                );
                ask_run &= unbound.answered;
            }
        }
    }
    if ask_run {
        job.notify_run = run_of_this_pane(until);
    }
    job.queued_ms = now_ms();
    job
}

/// How long `zo line add` waits for the window, all told (t-131644): the
/// binding, the `run-current` it may ask after it, and ending a question that
/// ran out — on Windows `taskkill`'s own wait — under one deadline, so a
/// refusal late in it leaves the next question only what is left (review
/// t-131848). A window that hangs would hold every put. On 2026-11-26
/// `zerocode-orc run-current` took at most 44 ms in five tries, at a load
/// average of 8 to 12; a window that is not there answers at once.
const ASK_PATIENCE: Duration = Duration::from_secs(5);

/// Why the window did not bind a job's end, and whether it answered at all.
struct Unbound {
    why: String,
    answered: bool,
}

/// Asks the window to bind job `id`'s end at `to` to this process's pane, in
/// `run` when one is named, by `until`: the run the binding is told in, or
/// why not.
fn bind(id: &str, to: &str, run: Option<&str>, until: Instant) -> Result<String, Unbound> {
    let mut asked = ledger_command();
    asked.args(bind_args(id, to, run)).stdout(Stdio::piped()).stderr(Stdio::piped());
    match process_tree::run_bounded_until(process_tree::as_a_question(&mut asked), until) {
        Ran::Ended { code: Some(0), stdout, .. } => stdout
            .as_deref()
            .and_then(|said| serde_json::from_str::<serde_json::Value>(said).ok())
            .and_then(|said| said["run"].as_str().filter(|run| !run.is_empty()).map(str::to_owned))
            .ok_or_else(|| Unbound { why: "the window answered no run".to_owned(), answered: true }),
        Ran::Ended { code, stderr, .. } => Err(Unbound {
            why: format!(
                "exit {} — {}",
                code.map_or_else(|| "by a signal".to_owned(), |code| code.to_string()),
                first_line(stderr.unwrap_or_default().as_bytes())
            ),
            answered: true,
        }),
        Ran::TimedOut => Err(Unbound {
            why: format!("no answer within the put's {} s", ASK_PATIENCE.as_secs()),
            answered: false,
        }),
        Ran::NotStarted(error) => Err(Unbound { why: format!("zerocode-orc did not start — {error}"), answered: true }),
    }
}

/// The words `zo line add` binds job `id`'s end with: the address the line
/// will tell it at, the run — a chair's own, else the one `run` names — and the
/// job's own name, which the ledger finds the binding by.
fn bind_args(id: &str, to: &str, run: Option<&str>) -> Vec<String> {
    let name = line::bound_as(id);
    let mut args: Vec<String> =
        [line_notify::VERB, "--job", id, "--to", to, "--retry-request", name.as_str()].map(str::to_owned).into();
    if let Some(run) = to.strip_prefix(RUN_ADDRESS).or(run) {
        args.extend(["--run".to_owned(), run.to_owned()]);
    }
    args
}

/// The job a put makes, waiting from now.
fn job_of(put: &Put, cwd: &Path) -> Job {
    let now = now_ms();
    Job {
        id: line::new_id(now, salt(now)),
        project: project_of(cwd),
        checkout: cwd.display().to_string(),
        owner: put.owner.clone().unwrap_or_else(who_asks),
        what: put.what.clone(),
        task: put.task.clone(),
        class: put.class,
        quiet: put.quiet,
        solo: put.solo,
        cap_secs: put.cap_secs.unwrap_or(line::CAP_DEFAULT_SECS),
        command: put.command.clone(),
        notify: put.notify.clone(),
        notify_run: None,
        notify_failed: None,
        state: JobState::Waiting,
        queued_ms: now,
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

/// A warning for a job put in under a cap its command has already outrun: the
/// newest ended run of the same command in the same project ended past a cap
/// no larger than this one, or ran longer than this one allows. A command the
/// line never ran has nothing to go by, so the receipt names the cap instead.
fn cap_warning(line: &Line, job: &Job) -> Option<String> {
    let before = line
        .jobs()
        .into_iter()
        .filter(|old| old.id != job.id && old.project == job.project && old.command == job.command)
        .filter(|old| matches!(old.state, JobState::Passed | JobState::Failed | JobState::Capped))
        .max_by_key(|old| old.ended_ms.unwrap_or(old.queued_ms))?;
    let outran = if before.state == JobState::Capped && before.cap_secs >= job.cap_secs {
        format!("ended past its cap of {} as {}", cap_words(before.cap_secs), before.id)
    } else {
        let ran_secs = before.ended_ms.zip(before.started_ms).map(|(end, start)| (end - start) / 1000)?;
        if ran_secs <= i64::from(job.cap_secs) {
            return None;
        }
        format!("ran {ran_secs} s as {}", before.id)
    };
    Some(format!(
        "zo line: warning — {} runs under a cap of {}, and the same command {outran}; a longer --cap gives it room",
        job.id,
        cap_words(job.cap_secs)
    ))
}

/// What `add` and `retry` say back: the job, its cap, and the warning when its
/// command has outrun that cap before (on stderr too, for a reader of stdout
/// that keeps only the job).
fn put_receipt(line: &Line, job: &Job, mut receipt: serde_json::Value) -> Result<Ended, String> {
    receipt["cap"] = json!(cap_words(job.cap_secs));
    if let Some(warning) = cap_warning(line, job) {
        eprintln!("{warning}");
        receipt["warning"] = json!(warning);
    }
    said(&receipt)
}

/// Changes job `id` under the line's lock.
fn change(line: &Line, id: &str, act: impl FnOnce(&mut Job) -> Result<(), String>) -> Result<Ended, String> {
    let _lock = lock(line)?;
    let mut job = line.job(id).map_err(|error| error.to_string())?;
    act(&mut job)?;
    line.save(&job).map_err(|error| error.to_string())?;
    said(&json!({ "job": job.id, "state": job.state, "class": job.class, "stop_asked": job.stop_asked }))
}

fn lock(line: &Line) -> Result<File, String> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(line.lock_path())
        .map_err(|error| error.to_string())?;
    file.lock().map_err(|error| error.to_string())?;
    Ok(file)
}

/// Starts `zo line wait <id>` as job `id`'s waiter, on its own, and returns its
/// pid once the waiter holds its lock — or after `WAITER_START`, when it has not
/// taken it by then. Its environment names the job (`WAITER_ENV`): it is the one
/// `zo line wait` that runs the job. On unix `sh` starts it in the background
/// and leaves at once, so the waiter's parent is the machine's first process
/// and its group is none of the asker's: the pane whose `zo line add` or
/// `zo line retry` started it takes no waiter along when it restarts. The
/// waiter, and the job it runs, have the environment of that pane.
fn start_waiter(line: &Line, id: &str) -> Result<u32, String> {
    let held = waiter_path(line, id);
    if let Some(folder) = held.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    let me = std::env::current_exe().map_err(|error| error.to_string())?;
    let pid = spawn_waiter(&me, id)?;
    let until = Instant::now() + WAITER_START;
    while !waiter_stands(&held) && Instant::now() < until {
        std::thread::sleep(WAITER_LOOK);
    }
    Ok(pid)
}

#[cfg(unix)]
fn spawn_waiter(me: &Path, id: &str) -> Result<u32, String> {
    use std::os::unix::process::CommandExt as _;
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(r#""$0" line wait "$1" </dev/null >/dev/null 2>&1 & echo $!"#)
        .arg(me)
        .arg(id)
        .env(WAITER_ENV, id)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .output()
        .map_err(|error| error.to_string())?;
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|_| format!("no waiter started for {id}"))
}

/// Windows has no `sh` to leave it with: the waiter starts as this process's child.
#[cfg(not(unix))]
fn spawn_waiter(me: &Path, id: &str) -> Result<u32, String> {
    Command::new(me)
        .args(["line", "wait", id])
        .env(WAITER_ENV, id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|child| child.id())
        .map_err(|error| error.to_string())
}

/// The file job `id`'s waiter holds locked while it lives: what tells the line
/// that the waiter stands, whatever process takes its pid after it.
fn waiter_path(line: &Line, id: &str) -> PathBuf {
    line.root().join(WAITERS).join(format!("{id}.lock"))
}

/// Whether a waiter holds the lock at `path`. Opened for writing, as every lock
/// of the line is: std leaves a lock on a file opened only for reading free to
/// fail, and a probe that failed would read a standing waiter as gone.
fn waiter_stands(path: &Path) -> bool {
    OpenOptions::new()
        .write(true)
        .open(path)
        .is_ok_and(|file| matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
}

/// The machine the line runs on.
struct MachineRules {
    slots: PathBuf,
    at_once: u32,
    floor_gib: u64,
    look_every: Duration,
    /// The environment a job runs in: this process's.
    env: fn(&str) -> Option<OsString>,
    /// The free bytes of the disk a folder is on.
    free: fn(&Path) -> Option<u64>,
    /// Starts a job's waiter on its own and returns its pid (`start_waiter`).
    start_waiter: fn(&Line, &str) -> Result<u32, String>,
}

/// `zo line wait <job>`: waits for the job's end and exits with its code. The
/// waiter the line started for the job — `started_for`, its `WAITER_ENV`,
/// names the job — runs it when its turn comes. Any other wait only watches,
/// a worker's pane's too: a runner a pane's restart takes along leaves its job
/// lost (t-118403).
fn wait(line: &Line, machine: &MachineRules, id: &str, started_for: Option<&OsStr>) -> Result<i32, String> {
    if started_for == Some(OsStr::new(id)) {
        return be_the_waiter(line, machine, id);
    }
    loop {
        if let Some(code) = watch(line, id)? {
            return Ok(code);
        }
        std::thread::sleep(machine.look_every);
    }
}

/// One look of a wait that runs nothing: the job's code once it has ended.
fn watch(line: &Line, id: &str) -> Result<Option<i32>, String> {
    let mut ended = Vec::new();
    let code = look(line, id, &mut ended).map(|(_lock, _, job)| job.state.ended().then(|| code_of(&job)));
    tell_ended(line, &ended);
    code
}

/// Job `id`'s waiter: it holds the job's waiter lock while it lives, has the
/// record name it, runs the job when its turn comes, and lets the lock go once
/// the job has ended.
fn be_the_waiter(line: &Line, machine: &MachineRules, id: &str) -> Result<i32, String> {
    let path = waiter_path(line, id);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    let held = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| error.to_string())?;
    held.lock().map_err(|error| error.to_string())?;
    {
        let _lock = lock(line)?;
        if let Ok(mut job) = line.job(id) {
            if job.state == JobState::Waiting && job.pid != Some(std::process::id()) {
                job.pid = Some(std::process::id());
                line.save(&job).map_err(|error| error.to_string())?;
            }
        }
    }
    let rc = run_job(line, machine, id, false);
    drop(held);
    let _ = std::fs::remove_file(&path);
    rc
}

/// Waits for job `id`'s turn, runs it, and records how it ended — or, when
/// another process already runs it, waits for that end: `zo line run`, and the
/// waiter the line started for the job. Returns the job's code (`code_of`).
fn run_job(line: &Line, machine: &MachineRules, id: &str, foreground: bool) -> Result<i32, String> {
    std::fs::create_dir_all(&machine.slots).map_err(|error| error.to_string())?;
    let held = loop {
        match take_turn(line, machine, id)? {
            Turn::Mine(held) => break held,
            Turn::Ended(rc) => return Ok(rc),
            Turn::Wait => {
                slot_target::sweep(&machine.slots);
                std::thread::sleep(machine.look_every);
            }
        }
    };
    slot_target::sweep(&machine.slots);
    let job = line.job(id).map_err(|error| error.to_string())?;
    let rc = execute(line, machine, job, foreground);
    drop(held);
    rc
}

enum Turn {
    Mine(Vec<File>),
    Ended(i32),
    Wait,
}

/// What every look at the line starts with, under its lock (held until the
/// returned file drops): running jobs whose runner is gone are lost, and
/// waiting jobs whose waiter is gone are ended. The jobs it ends go to `ended`,
/// to be told once the lock is let go (`tell_ended`). The lock, the line's jobs
/// and job `id`.
fn look(line: &Line, id: &str, ended: &mut Vec<Job>) -> Result<(File, Vec<Job>, Job), String> {
    let held = lock(line)?;
    let mut jobs = line.jobs();
    mark_lost(line, &mut jobs, ended);
    end_orphans(line, &mut jobs, ended);
    let mine = jobs
        .iter()
        .find(|job| job.id == id)
        .cloned()
        .ok_or_else(|| format!("the line holds no job {id}"))?;
    Ok((held, jobs, mine))
}

/// One look at job `id`'s turn (`turn_of`); the jobs the look ended are told
/// after it, the line's lock let go.
fn take_turn(line: &Line, machine: &MachineRules, id: &str) -> Result<Turn, String> {
    let mut ended = Vec::new();
    let turn = turn_of(line, machine, id, &mut ended);
    tell_ended(line, &ended);
    turn
}

/// One look, under the line's lock: is it job `id`'s turn, and is a slot
/// free for it? Its turn and a slot it can take make it running.
fn turn_of(line: &Line, machine: &MachineRules, id: &str, ended: &mut Vec<Job>) -> Result<Turn, String> {
    let (_lock, jobs, mine) = look(line, id, ended)?;
    match mine.state {
        JobState::Waiting if mine.stop_asked => {
            let mut job = mine;
            job.state = JobState::Withdrawn;
            job.ended_ms = Some(now_ms());
            line.save(&job).map_err(|error| error.to_string())?;
            return Ok(Turn::Ended(1));
        }
        JobState::Waiting => {}
        // Another process runs it: its end is the answer (a dead runner's job is lost in `look`).
        JobState::Running => return Ok(Turn::Wait),
        _ => return Ok(Turn::Ended(code_of(&mine))),
    }
    let pause = lift_ended_pause(line);
    let (free, taken) = slot_files(machine)?;
    let running_slots: Vec<u32> = jobs.iter().filter(|job| job.state == JobState::Running).filter_map(|job| job.slot).collect();
    let disk_free = (machine.free)(line.root());
    if slot_target::near_floor(disk_free, machine.floor_gib) {
        // No job runs in a slot this process holds: its folder can go, and is removed outside the lock.
        let idle: Vec<u32> = free.iter().map(|(slot, _)| *slot).collect();
        slot_target::let_idle_go(&machine.slots, &idle, now_ms());
    }
    let state = Machine {
        taken: taken.into_iter().filter(|slot| !running_slots.contains(slot)).collect(),
        slots: machine.at_once,
        disk_low: disk_low(disk_free, machine.floor_gib),
        paused: pause.is_some(),
    };
    let turn = line.turn();
    let ended = |owner: &str| line.last_end(owner);
    let mut free = free;
    let order: Vec<u32> = free.iter().map(|(slot, _)| *slot).collect();
    for slot in order {
        let next = match line::next_up(&jobs, slot, &state, &ended, turn) {
            Ok(next) => next.map(|job| job.id.clone()),
            Err(Hold::QuietWanted | Hold::Paused | Hold::DiskLow) => None,
        };
        if next.as_deref() != Some(id) {
            continue;
        }
        // A quiet job holds every slot it can, and runs only when it has them all.
        let keep: Vec<File> = if mine.quiet {
            if free.len() < machine.at_once as usize {
                return Ok(Turn::Wait);
            }
            free.drain(..).map(|(_, file)| file).collect()
        } else {
            let at = free.iter().position(|(held, _)| *held == slot).unwrap_or(0);
            vec![free.remove(at).1]
        };
        let mut job = mine;
        job.state = JobState::Running;
        job.slot = Some(slot);
        job.started_ms = Some(now_ms());
        job.pid = Some(std::process::id());
        line.save(&job).map_err(|error| error.to_string())?;
        line.count_turn().map_err(|error| error.to_string())?;
        return Ok(Turn::Mine(keep));
    }
    Ok(Turn::Wait)
}

/// A slot this process took, its lock held until the file drops.
type HeldSlot = (u32, File);

/// The slots this process could take now (held until dropped), and those it
/// could not.
fn slot_files(machine: &MachineRules) -> Result<(Vec<HeldSlot>, Vec<u32>), String> {
    let (mut free, mut taken) = (Vec::new(), Vec::new());
    for slot in 0..machine.at_once {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(machine.slots.join(format!("{slot}.lock")))
            .map_err(|error| error.to_string())?;
        if file.try_lock().is_ok() {
            free.push((slot, file));
        } else {
            taken.push(slot);
        }
    }
    Ok((free, taken))
}

/// A pause that waits for a build lifts once that build's newest run has ended.
fn lift_ended_pause(line: &Line) -> Option<Pause> {
    let pause = line.pause()?;
    if let Some(until) = &pause.until {
        let ended = newest_run_ended(Path::new(&until.project), &until.build, pause.since_ms);
        if ended {
            let _ = line.lift_pause();
            return None;
        }
    }
    Some(pause)
}

/// Whether the build `build` of the repository whose shared git folder is
/// `shared` has a run, begun at or after `since_ms`, that ended.
fn newest_run_ended(shared: &Path, build: &str, since_ms: i64) -> bool {
    let Ok(entries) = std::fs::read_dir(zerocode_core::builds::folder(shared).join(build)) else {
        return false;
    };
    let mut records: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    records.sort();
    let Some(newest) = records.last() else {
        return false;
    };
    std::fs::read(newest)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<zerocode_core::builds::Run>(&bytes).ok())
        .is_some_and(|run| run.outcome.is_some() && run.started_ms + 60_000 >= since_ms)
}

/// Running jobs whose runner is gone are lost: their slot is free and their
/// owner may go on. A runner is gone when its pid stands no more, or, for a job
/// the line started a waiter for, when no waiter holds the job's lock — another
/// process may have the runner's pid by then. The jobs it ends go to `ended`.
fn mark_lost(line: &Line, jobs: &mut [Job], ended: &mut Vec<Job>) {
    for job in jobs.iter_mut().filter(|job| job.state == JobState::Running) {
        if job.pid.is_some_and(|pid| !alive(pid)) || waiter_gone(line, &job.id) {
            job.state = JobState::Lost;
            job.ended_ms = Some(now_ms());
            let _ = line.save(job);
            let _ = line.ended(&job.owner, now_ms());
            ended.push(job.clone());
        }
    }
}

/// A job the line started a waiter for, whose waiter holds its lock no more.
fn waiter_gone(line: &Line, id: &str) -> bool {
    let path = waiter_path(line, id);
    path.exists() && !waiter_stands(&path)
}

/// Waiting jobs whose waiter is gone — killed, or the machine restarted — are
/// ended: lost, or withdrawn when a stop was asked of them. A job no waiter
/// stands for held its owner's later jobs, and every slot the line chose it
/// for, for good. The line starts no waiter in its place: that one would run
/// the job with the environment of whichever process looked, not of the pane
/// that put it in. Its log says so, and how that pane puts it in again.
///
/// A waiter is gone when the record names it (`pid`), its lock is free, and
/// either no process has its pid or the job was put in `WAITER_GRACE_MS` ago — a
/// waiter that holds no lock by then is not coming, whatever process has its
/// pid now. A job whose record names no waiter is left as it is: one put in
/// before v1.1.168, whose waiter holds no lock to tell, and one of
/// `zo line run`, which runs in its own terminal. The jobs it ends go to
/// `ended`.
fn end_orphans(line: &Line, jobs: &mut [Job], ended: &mut Vec<Job>) {
    let now = now_ms();
    for job in jobs.iter_mut().filter(|job| job.state == JobState::Waiting) {
        let Some(waiter) = job.pid else {
            continue;
        };
        let late = now.saturating_sub(job.queued_ms) >= WAITER_GRACE_MS;
        if waiter_stands(&waiter_path(line, &job.id)) || (alive(waiter) && !late) {
            continue;
        }
        job.state = if job.stop_asked { JobState::Withdrawn } else { JobState::Lost };
        job.ended_ms = Some(now);
        if line.save(job).is_err() {
            continue;
        }
        let _ = line.ended(&job.owner, now);
        let path = line.log_path(&job.id);
        if let Ok(log) = OpenOptions::new().create(true).append(true).open(&path) {
            let words = format!(
                "zo line: waiter {waiter} was gone before the job's turn, so the line ended the job — \
                 `zo line retry {}` from the pane that put it in runs it again with that pane's environment",
                job.id
            );
            close_log(&log, &path, &words);
        }
        ended.push(job.clone());
    }
}

/// Tells each job a look ended, when it named where (`notify`): after the
/// look, the line's lock let go, since a send may take seconds. A job's end is
/// told once, by the look that ended it.
fn tell_ended(line: &Line, ended: &[Job]) {
    for job in ended {
        let ran = job.started_ms.zip(job.ended_ms).map_or(0, |(start, end)| end.saturating_sub(start));
        notify(line, job, Duration::from_millis(u64::try_from(ran).unwrap_or(0)));
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // Signal 0 asks whether the process stands; EPERM says it does, as another user's.
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(error) => error == nix::errno::Errno::EPERM,
    }
}

/// Windows has no signal 0 here: a runner that died there leaves its job
/// running until its cap passes in the record (`zo line stop` ends it).
#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    true
}

/// The disk floor in GiB that `value` of `FLOOR_ENV` names: the line's default
/// when it is unset or empty, `0` for none. A word that is no whole number of
/// GiB is refused, never read as the default — a floor someone set and the
/// line ignored would hold or start jobs nobody expected.
fn floor_gib(value: Option<&OsStr>) -> Result<u64, String> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(line::DISK_FLOOR_DEFAULT_GIB);
    };
    value.to_str().and_then(|word| word.parse().ok()).ok_or_else(|| {
        format!("{FLOOR_ENV} takes a whole number of GiB, 0 for no floor — not `{}`", value.to_string_lossy())
    })
}

/// `free` bytes of disk below `floor_gib`.
fn disk_low(free: Option<u64>, floor_gib: u64) -> bool {
    free.is_some_and(|free| free < floor_gib.saturating_mul(1 << 30))
}

#[cfg(unix)]
fn free_bytes(folder: &Path) -> Option<u64> {
    let stat = nix::sys::statvfs::statvfs(folder).ok()?;
    // The block count is 32 bits wide on macOS and 64 on Linux; u128 takes both.
    let bytes = u128::from(stat.blocks_available()).saturating_mul(u128::from(stat.fragment_size()));
    Some(u64::try_from(bytes).unwrap_or(u64::MAX))
}

#[cfg(not(unix))]
fn free_bytes(_folder: &Path) -> Option<u64> {
    None
}

/// Runs the job's command, its output to the job's log (and to this terminal
/// when it runs in the foreground), ended by a stop or its cap.
fn execute(line: &Line, machine: &MachineRules, mut job: Job, foreground: bool) -> Result<i32, String> {
    let log_path = line.log_path(&job.id);
    // Added to, not written over: a line the job's log took before it ran (a new waiter) stays.
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| error.to_string())?;
    // The same open file as the command's: what the line writes under its output follows it, and is
    // not written over by it.
    let closing = log.try_clone().map_err(|error| error.to_string())?;
    let mut command = command_of(&job);
    // A `zo line wait` the job's command starts is no waiter the line started.
    command.current_dir(&job.checkout).stdin(Stdio::null()).env_remove(WAITER_ENV);
    let ready = job.slot.and_then(|slot| slot_target::ready(&machine.slots, slot, Path::new(&job.checkout), &machine.env, &log));
    if let Some(ready) = &ready {
        ready.apply(&mut command);
    }
    let mut tees = Vec::new();
    let mut child = if foreground {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        tees.extend(tee(child.stdout.take(), &log, false));
        tees.extend(tee(child.stderr.take(), &log, true));
        child
    } else {
        let err = log.try_clone().map_err(|error| error.to_string())?;
        command.stdout(Stdio::from(log)).stderr(Stdio::from(err));
        command.spawn().map_err(|error| error.to_string())?
    };
    let started = Instant::now();
    let cap = Duration::from_secs(u64::from(job.cap_secs));
    let mut ended_as = None;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if started.elapsed() > cap {
            ended_as = Some(JobState::Capped);
            break end(&mut child)?;
        }
        if line.job(&job.id).is_ok_and(|now| now.stop_asked) {
            ended_as = Some(JobState::Stopped);
            break end(&mut child)?;
        }
        std::thread::sleep(machine.look_every);
    };
    let rc = crate::build_runner::rc_of(status);
    let fresh = line.job(&job.id).unwrap_or(job.clone());
    job.ended_by = fresh.ended_by;
    job.stop_asked = fresh.stop_asked;
    if let Some(words) = ended_as.and_then(|ended| ended_by_the_line(&job, ended, started.elapsed())) {
        // The copies finish the output the ended group left in its pipes first, so the closing line
        // stands last; a pipe some escaped process still holds is not waited on past the grace.
        let until = Instant::now() + GRACE;
        while tees.iter().any(|copy| !copy.is_finished()) && Instant::now() < until {
            std::thread::sleep(machine.look_every);
        }
        close_log(&closing, &log_path, &words);
        if foreground {
            eprintln!("{words}");
        }
    }
    job.state = ended_as.unwrap_or(if rc == 0 { JobState::Passed } else { JobState::Failed });
    job.rc = Some(rc);
    job.ended_ms = Some(now_ms());
    // The receipt beside the log (`<job>.rc`): what a worker names as a pass condition's evidence, the
    // exit code the ledger reads (`orchestration::evidence`). A stopped or withdrawn job says its own.
    let receipt = code_of(&job);
    let _ = std::fs::write(line.log_path(&job.id).with_extension("rc"), format!("{receipt}\n"));
    {
        let _lock = lock(line)?;
        line.save(&job).map_err(|error| error.to_string())?;
        line.ended(&job.owner, now_ms()).map_err(|error| error.to_string())?;
    }
    line.prune();
    purge_trash(line, now_ms());
    notify(line, &job, started.elapsed());
    if let Some(ready) = ready {
        ready.done();
    }
    Ok(receipt)
}

/// The code an ended job says, the same in its receipt (`<job>.rc`) and to
/// every `zo line wait`: 0 for a job that passed, its command's code otherwise,
/// and 1 when that was 0 — a command a stop or its cap ended left with 0 — or
/// there was none (withdrawn, lost).
fn code_of(job: &Job) -> i32 {
    if job.state == JobState::Passed {
        0
    } else {
        job.rc.filter(|rc| *rc != 0).unwrap_or(1)
    }
}

/// The command a job names: one word with spaces runs through the shell, more
/// words run as they are.
fn command_of(job: &Job) -> Command {
    if let [one] = job.command.as_slice() {
        return crate::build_runner::shell(one);
    }
    let mut command = Command::new(&job.command[0]);
    command.args(&job.command[1..]);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
}

fn end(child: &mut Child) -> Result<std::process::ExitStatus, String> {
    crate::build_runner::end_child(child, GRACE)
}

/// The line's own last word in the log of a command it ended: a log read on
/// its own, or a receipt's bare code beside it, would pass for a failure of
/// the command. A command that ended by itself says its own.
fn ended_by_the_line(job: &Job, ended: JobState, took: Duration) -> Option<String> {
    let secs = took.as_secs();
    match ended {
        JobState::Capped => Some(format!(
            "zo line: ended past its cap of {} after {secs} s — the line stopped the command; the command did not fail",
            cap_words(job.cap_secs)
        )),
        JobState::Stopped => Some(format!(
            "zo line: stopped by {} after {secs} s — the command did not end by itself",
            job.ended_by.as_deref().unwrap_or("its owner")
        )),
        _ => None,
    }
}

/// Writes `words` as the log's last line, on a line of its own.
fn close_log(mut log: &File, path: &Path, words: &str) {
    let mut last = [0_u8; 1];
    // An empty log has no last byte to read: it ends no line of output either.
    let ends_a_line = File::open(path)
        .and_then(|mut file| {
            file.seek(SeekFrom::End(-1))?;
            file.read_exact(&mut last)
        })
        .is_err()
        || last[0] == b'\n';
    let _ = writeln!(log, "{}{words}", if ends_a_line { "" } else { "\n" });
}

/// A cap in the words `--cap` reads: `2h`, `30m`, `90s`.
fn cap_words(secs: u32) -> String {
    match secs {
        0 => "0s".to_owned(),
        _ if secs.is_multiple_of(3600) => format!("{}h", secs / 3600),
        _ if secs.is_multiple_of(60) => format!("{}m", secs / 60),
        _ => format!("{secs}s"),
    }
}

/// Copies a child's output to the job's log and this terminal, line by line.
fn tee(output: Option<impl io::Read + Send + 'static>, log: &File, to_err: bool) -> Option<std::thread::JoinHandle<()>> {
    let (Some(output), Ok(mut log)) = (output, log.try_clone()) else {
        return None;
    };
    Some(std::thread::spawn(move || {
        let mut reader = BufReader::new(output);
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line).is_ok_and(|read| read > 0) {
            let _ = log.write_all(&line);
            if to_err {
                let _ = io::stderr().write_all(&line);
            } else {
                let _ = io::stdout().write_all(&line);
            }
            line.clear();
        }
    }))
}

/// Tells the ledger how the job ended, when it named where: through
/// `zerocode-orc send`, with the environment of the process that tells — the
/// job's waiter, which has the one of the process that put the job in, or the
/// look that ended the job (`tell_ended`) — in the run the address is found in
/// (`send_args`). `send` changes the ledger, so it goes under a name the window
/// can take again unchanged, the job's: without one the window refused every
/// end the line told (t-118403). A send that does not go is left: the job's
/// log says so as its last line, and its record says it too (`untold`).
fn notify(line: &Line, job: &Job, took: Duration) {
    let Some(to) = job.notify.as_deref() else {
        return;
    };
    let mut send = ledger_command();
    send.args(send_args(job, to, took));
    let said = send
        .current_dir(&job.checkout)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let why = match said {
        Ok(said) if said.status.success() => return,
        Ok(said) => format!("exit {} — {}", crate::build_runner::rc_of(said.status), first_line(&said.stderr)),
        Err(error) => format!("zerocode-orc did not start — {error}"),
    };
    let path = line.log_path(&job.id);
    if let Ok(log) = OpenOptions::new().create(true).append(true).open(&path) {
        close_log(&log, &path, &format!("zo line: telling {to} how the job ended did not go: {why}"));
    }
    untold(line, &job.id, why, teller_window_gone());
}

/// Writes in job `id`'s record that its end was not told (t-131644), under the
/// line's lock, over the record as it stands now: when, `why`, and whether the
/// window whose seat the teller signed with had ended. On 2026-11-25 and 26
/// three ends a restart had left without a seat were refused and kept only in
/// their logs, and the workers waiting on them waited on. The window reads
/// this to tell such an end itself. A record that cannot be read or written
/// keeps what the log says.
fn untold(line: &Line, id: &str, why: String, window_gone: bool) {
    let Ok(_lock) = lock(line) else {
        return;
    };
    let Ok(mut job) = line.job(id) else {
        return;
    };
    job.notify_failed = Some(NotifyFailed { at_ms: now_ms(), why, window_gone });
    let _ = line.save(&job);
}

/// Whether the window this process's seat came from — the one its environment
/// names (`WINDOW_PID_ENV`), whose pane put the job in or looked — has ended:
/// a restart mints every seat anew, so a seat whose window is gone is one the
/// window refuses (t-131644). A process no window names says none is gone.
fn teller_window_gone() -> bool {
    std::env::var(WINDOW_PID_ENV)
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok())
        .is_some_and(|pid| !alive(pid))
}

/// The words `notify` gives `zerocode-orc` to tell `to` how `job` ended. The
/// window finds a worker or a pane in the run its caller stands in, and the
/// look that ends a job may stand in another run, or in none: the send names
/// the run the address is found in — a run's own, or the one the pane that put
/// the job in stood in (`notify_run`, t-119309). The sender signs as itself.
fn send_args(job: &Job, to: &str, took: Duration) -> Vec<String> {
    let what = if job.what.is_empty() { job.command.join(" ") } else { job.what.clone() };
    let body = format!(
        "zo line {}: {what} — {} (rc {}, {}s)",
        job.id,
        job.state.word(),
        job.rc.unwrap_or(-1),
        took.as_secs()
    );
    // The end's name, as its retry name and as its subject: the window finds a letter about this end
    // by it before it tells the end itself (t-131644).
    let name = line::told_as(&job.id);
    let mut args: Vec<String> = [
        "send", "--to", to, "--type", "status", "--body", body.as_str(), "--retry-request", name.as_str(), "--subject",
        name.as_str(),
    ]
    .map(str::to_owned)
    .into();
    if let Some(run) = to.strip_prefix(RUN_ADDRESS).or(job.notify_run.as_deref()) {
        args.extend(["--run".to_owned(), run.to_owned()]);
    }
    args
}

/// The ledger's command as this machine runs it.
fn ledger_command() -> Command {
    let endpoint = std::env::var_os(zerocode_core::hook::ENDPOINT_ENV);
    ledger_command_on(std::env::var_os("PATH").as_deref(), endpoint.as_deref().map(Path::new), cfg!(windows))
}

/// The ledger's command on a machine whose `PATH` and window endpoint are these:
/// the one zo's Agent tool runs (`zerocode_core::agent_shims`). On Windows that
/// is the window's `.ps1` through Windows `PowerShell` — a bare `zerocode-orc`
/// finds only an `.exe` there, and the window installs none (I-t118403-W1).
/// Found nowhere, the bare name, whose failure to start the job's log says.
fn ledger_command_on(path: Option<&OsStr>, endpoint: Option<&Path>, windows: bool) -> Command {
    match agent_shims::find_orchestration_program(path, endpoint, windows) {
        Some(program) => agent_shims::orchestration_command(&program, windows),
        None => Command::new(agent_shims::ORCHESTRATION_COMMAND),
    }
}

/// The run this process's pane stands in, as `zerocode-orc run-current` says:
/// none when it stands in none, or the ledger cannot be asked by `until` —
/// what is left of the put's `ASK_PATIENCE`.
fn run_of_this_pane(until: Instant) -> Option<String> {
    let mut asked = ledger_command();
    asked.arg("run-current").stdout(Stdio::piped()).stderr(Stdio::null());
    let Ran::Ended { code: Some(0), stdout: Some(said), .. } =
        process_tree::run_bounded_until(process_tree::as_a_question(&mut asked), until)
    else {
        return None;
    };
    run_named(said.as_bytes())
}

/// The run `run-current`'s answer names.
fn run_named(answer: &[u8]) -> Option<String> {
    let answer: serde_json::Value = serde_json::from_slice(answer).ok()?;
    answer["runId"].as_str().filter(|run| !run.is_empty()).map(str::to_owned)
}

/// The first line a command said on stderr, at most `SAID_CHARS` characters.
fn first_line(said: &[u8]) -> String {
    let text = String::from_utf8_lossy(said);
    let first = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("it said nothing");
    first.chars().take(SAID_CHARS).collect()
}

/// How many of the jobs that ended a list shows unasked: the last ones, where
/// a script finds the code of the job it just waited on.
const LIST_ENDED_SHOWN: usize = 10;

/// `zo line list`: this project's jobs that wait or run, each waiting one
/// with its place, and the ten that ended last; the older ended ones are
/// counted (t-115139 Z6). An agent asks the line whether it is busy or how
/// its job ended, and the project's whole history came back — 399 jobs,
/// 245 KB of one-line JSON a shell result cut. `--history` lists every job
/// of this project, and `--all` every job of every project as it always has:
/// a script finds an old job's code there by its id (the independent review).
fn listing(line: &Line, cwd: &Path, all: bool, history: bool) -> Result<Ended, String> {
    let history = history || all;
    let project = project_of(cwd);
    let jobs = line.jobs();
    let places = line::places(&jobs, &|owner| line.last_end(owner), line.turn());
    let ours: Vec<&Job> = jobs.iter().filter(|job| all || job.project == project).collect();
    let mut ended: Vec<&Job> = ours.iter().copied().filter(|job| job.state.ended()).collect();
    ended.sort_by_key(|job| std::cmp::Reverse(job.ended_ms.unwrap_or(job.queued_ms)));
    let older = if history { 0 } else { ended.len().saturating_sub(LIST_ENDED_SHOWN) };
    let last_ended: Vec<&str> = ended.iter().take(LIST_ENDED_SHOWN).map(|job| job.id.as_str()).collect();
    let shown: Vec<serde_json::Value> = ours
        .iter()
        .filter(|job| history || !job.state.ended() || last_ended.contains(&job.id.as_str()))
        .map(|job| listed(job, places.get(&job.id).copied()))
        .collect();
    let mut answer = json!({ "paused": line.pause(), "jobs": shown });
    if older > 0 {
        answer["olderEndedJobs"] = json!(older);
        answer["note"] = json!(format!(
            "{older} older ended jobs are not listed; `zo line list --history` lists every job, `zo line wait <job>` exits with one's code"
        ));
    }
    said(&answer)
}

/// One job as `zo line list` shows it, with its place when it waits. A waiting
/// job's pid is its waiter's — the process that runs it when its turn comes —
/// and is listed as `waiter`, beside the job: a job's own `pid` is its runner's.
fn listed(job: &Job, place: Option<usize>) -> serde_json::Value {
    let mut job = job.clone();
    let waiter = if job.state == JobState::Waiting { job.pid.take() } else { None };
    let mut row = json!({ "job": job, "place": place });
    if let Some(waiter) = waiter {
        row["waiter"] = json!(waiter);
    }
    row
}

/// `zo line log <job>`: the log's last lines.
fn log_tail(line: &Line, id: &str) -> Result<Ended, String> {
    let _ = line.job(id).map_err(|error| error.to_string())?;
    let mut file = File::open(line.log_path(id)).map_err(|_| format!("{id} has no log yet"))?;
    let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let from = length.saturating_sub(64 * 1024);
    file.seek(SeekFrom::Start(from)).map_err(|error| error.to_string())?;
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|error| error.to_string())?;
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(LOG_TAIL_LINES);
    Ok(Ended::Said(lines[start..].join("\n")))
}

/// `zo line forget <job>`: an ended job's record and log go to the trash for a day.
fn forget(line: &Line, id: &str) -> Result<Ended, String> {
    let _lock = lock(line)?;
    let job = line.job(id).map_err(|error| error.to_string())?;
    if !job.state.ended() {
        return Err(format!("{id} has not ended; stop or withdraw it first"));
    }
    let trash = line.root().join(TRASH);
    std::fs::create_dir_all(&trash).map_err(|error| error.to_string())?;
    std::fs::write(trash.join(format!("{id}.forgotten")), now_ms().to_string()).map_err(|error| error.to_string())?;
    let _ = std::fs::rename(line.log_path(id), trash.join(format!("{id}.log")));
    std::fs::rename(line.job_path(id), trash.join(format!("{id}.json"))).map_err(|error| error.to_string())?;
    said(&json!({ "forgotten": id }))
}

/// `zo line restore <job>`: brings a forgotten job back while the trash keeps it.
fn restore(line: &Line, id: &str) -> Result<Ended, String> {
    let _lock = lock(line)?;
    let trash = line.root().join(TRASH);
    let record = trash.join(format!("{id}.json"));
    if !record.is_file() || line.job_path(id).exists() {
        return Err(format!("the trash keeps no job {id}"));
    }
    let _ = std::fs::rename(trash.join(format!("{id}.log")), line.log_path(id));
    std::fs::rename(&record, line.job_path(id)).map_err(|error| error.to_string())?;
    let _ = std::fs::remove_file(trash.join(format!("{id}.forgotten")));
    said(&json!({ "restored": id }))
}

fn purge_trash(line: &Line, now: i64) {
    let trash = line.root().join(TRASH);
    let Ok(entries) = std::fs::read_dir(&trash) else {
        return;
    };
    for note in entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "forgotten"))
    {
        let at: i64 = std::fs::read_to_string(&note).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(now);
        if now.saturating_sub(at) >= TRASH_KEPT_MS {
            let _ = std::fs::remove_file(note.with_extension("json"));
            let _ = std::fs::remove_file(note.with_extension("log"));
            let _ = std::fs::remove_file(&note);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_put_reads_its_flags_and_takes_the_words_after_the_dashes_as_its_command() {
        let put = parse(&words(&[
            "add", "--what", "결제 취소 · 통합 시험", "--task", "t-1203", "--first", "--solo", "--cap", "15m",
            "--notify", "run:run-4275", "--", "./gradlew", ":payment:integrationTest", "--tests", "*IT",
        ]))
        .expect("an add");
        assert_eq!(
            put,
            Verb::Add(Put {
                what: "결제 취소 · 통합 시험".to_owned(),
                task: Some("t-1203".to_owned()),
                class: Class::First,
                solo: true,
                quiet: false,
                cap_secs: Some(900),
                owner: None,
                notify: Some("run:run-4275".to_owned()),
                command: words(&["./gradlew", ":payment:integrationTest", "--tests", "*IT"]),
            })
        );
        for (args, says) in [
            (&["run", "--what", "x"][..], "name the command after `--`"),
            (&["run", "--notify", "run:r", "--", "make"][..], "unknown argument `--notify`"),
            (&["run", "--cap", "soon", "--", "make"][..], "not `soon`"),
            (&["run", "--cap", "0", "--", "make"][..], "above zero"),
            (&["retry", "abc"][..], "no job id"),
            (&["retry"][..], "takes one job id"),
            (&["deploy"][..], "unknown `zo line` verb"),
            (&[][..], "zo line run"),
        ] {
            let refused = parse(&words(args)).expect_err("refused");
            assert!(refused.contains(says), "{args:?}: {refused}");
        }
        assert_eq!(cap_secs("2h"), Ok(7200));
        assert_eq!(cap_secs("90"), Ok(90));
        assert_eq!(parse(&words(&["pause", "--until-build", "release"])), Ok(Verb::Pause { until_build: Some("release".to_owned()), why: String::new() }));
    }

    #[cfg(unix)]
    fn rules(slots: &Path) -> MachineRules {
        MachineRules {
            slots: slots.to_path_buf(),
            at_once: 2,
            floor_gib: 0,
            look_every: Duration::from_millis(20),
            // The job names its own build folder unless a test says otherwise: these runs make none.
            env: |name| (name == "CARGO_TARGET_DIR").then(|| OsString::from("own")),
            free: |_| None,
            // No process starts here: a test that wants a waiter given names the one it gets.
            start_waiter: |_, _| Err("a test starts no waiter".to_owned()),
        }
    }

    #[cfg(unix)]
    fn put(line: &Line, cwd: &Path, owner: &str, command: &[&str]) -> Job {
        put_in(line, &Put { owner: Some(owner.to_owned()), command: words(command), ..Put::default() }, cwd).expect("put in")
    }

    /// A second runner on a job another runner holds (a waiter of an older zo,
    /// which takes the turn, beside the one `add` started) waits for its end
    /// and takes its code; the job runs once.
    #[cfg(unix)]
    #[test]
    fn a_second_waiter_waits_for_the_running_jobs_end_and_takes_its_code() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let machine = rules(&home.path().join(SLOTS_FOLDER));
        let job = put(&line, cwd.path(), "w-1", &["sh", "-c", "echo once >> ran; sleep 1; exit 3"]);
        let runner = {
            let (line, id) = (line.clone(), job.id.clone());
            let machine = rules(&home.path().join(SLOTS_FOLDER));
            std::thread::spawn(move || run_job(&line, &machine, &id, false))
        };
        while line.job(&job.id).expect("its record").state == JobState::Waiting {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(run_job(&line, &machine, &job.id, false), Ok(3), "the second waiter takes the job's code");
        assert_eq!(runner.join().expect("the runner"), Ok(3));
        let ran = std::fs::read_to_string(cwd.path().join("ran")).expect("the job's mark");
        assert_eq!(ran, "once\n", "the job ran once");
    }

    /// The pid a test's `start_waiter` gives: a number in the record, no process.
    const GIVEN: u32 = 4_242;

    /// The lock a job's waiter holds while it lives, taken here as the waiter would.
    #[cfg(unix)]
    fn hold_waiter_lock(line: &Line, id: &str) -> File {
        let path = waiter_path(line, id);
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("folders");
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(&path).expect("the waiter's file");
        file.lock().expect("held");
        file
    }

    #[cfg(unix)]
    fn named(line: &Line, id: &str, pid: u32) {
        change(line, id, |job| {
            job.pid = Some(pid);
            Ok(())
        })
        .expect("named");
    }

    /// A `zo line wait` the line did not start for a job never takes its turn,
    /// though the turn is the job's and a slot is free; the waiter the line
    /// started runs it, once, and both end with its code (t-118403).
    #[cfg(unix)]
    #[test]
    fn a_wait_the_line_did_not_start_for_a_job_never_takes_its_turn() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let machine = rules(&home.path().join(SLOTS_FOLDER));
        let job = put(&line, cwd.path(), "w-1", &["echo ran >> ran"]);
        let waiter = hold_waiter_lock(&line, &job.id);
        named(&line, &job.id, std::process::id());
        let watcher = {
            let (line, id) = (line.clone(), job.id.clone());
            let machine = rules(&home.path().join(SLOTS_FOLDER));
            std::thread::spawn(move || wait(&line, &machine, &id, None))
        };
        std::thread::sleep(machine.look_every * 10);
        let waits = line.job(&job.id).expect("its record");
        assert_eq!((waits.state, waits.slot), (JobState::Waiting, None), "the wait took no turn");
        drop(waiter);
        assert_eq!(wait(&line, &machine, &job.id, Some(OsStr::new(&job.id))), Ok(0), "the waiter runs the job");
        assert_eq!(watcher.join().expect("the wait"), Ok(0), "the wait takes the job's code");
        let ran = std::fs::read_to_string(cwd.path().join("ran")).expect("the job's mark");
        assert_eq!(ran, "ran\n", "the job ran once");
        assert!(!waiter_path(&line, &job.id).exists(), "the waiter let its lock go");
    }

    /// A job put in a minute ago, as the line counts it.
    #[cfg(unix)]
    fn put_long_ago(line: &Line, id: &str) {
        change(line, id, |job| {
            job.queued_ms = now_ms() - WAITER_GRACE_MS - 1_000;
            Ok(())
        })
        .expect("put in long ago");
    }

    /// A waiting job whose waiter is gone is ended at the next look, and no
    /// waiter is started in its place: lost, or withdrawn when a stop was asked
    /// of it. Gone means its lock is free and its pid stands no more — or the job
    /// was put in a minute ago, though another process has the pid by then. The
    /// job's log says how to put it in again from the pane that put it in.
    #[cfg(unix)]
    #[test]
    fn a_waiting_job_whose_waiter_holds_no_lock_is_ended_though_its_pid_stands() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let reused = put(&line, cwd.path(), "w-1", &["true"]);
        // Its waiter left the file, unlocked; its pid names this test now, a process that stands.
        drop(hold_waiter_lock(&line, &reused.id));
        let taken = std::process::id();
        named(&line, &reused.id, taken);
        put_long_ago(&line, &reused.id);
        let stopped = put(&line, cwd.path(), "w-2", &["true"]);
        let mut child = std::process::Command::new("true").spawn().expect("a process");
        let dead = child.id();
        child.wait().expect("it ended");
        named(&line, &stopped.id, dead);
        change(&line, &stopped.id, |job| {
            job.stop_asked = true;
            Ok(())
        })
        .expect("a stop asked");
        assert_eq!(watch(&line, &reused.id), Ok(Some(1)), "a wait on it ends with its code");
        let ended: Vec<(JobState, Option<u32>)> =
            [&reused, &stopped].iter().map(|job| line.job(&job.id).expect("a record")).map(|job| (job.state, job.pid)).collect();
        assert_eq!(ended, [(JobState::Lost, Some(taken)), (JobState::Withdrawn, Some(dead))], "no new waiter is named");
        let log = std::fs::read_to_string(line.log_path(&reused.id)).expect("its log");
        assert!(log.contains(&format!("waiter {taken} was gone before the job's turn")), "{log}");
        assert!(log.contains(&format!("`zo line retry {}` from the pane that put it in", reused.id)), "{log}");
    }

    /// A job put in a moment ago whose waiter stands but holds no lock yet is
    /// not ended: `add` writes the record after waiting `WAITER_START` for the
    /// lock, and on a busy machine the lock stands only later.
    #[cfg(unix)]
    #[test]
    fn a_job_whose_waiter_is_still_taking_its_lock_is_not_ended() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let job = put(&line, cwd.path(), "w-1", &["true"]);
        named(&line, &job.id, std::process::id());
        assert_eq!(watch(&line, &job.id), Ok(None), "the job still waits");
        assert_eq!(line.job(&job.id).expect("its record").state, JobState::Waiting);
        assert!(!line.log_path(&job.id).exists(), "no log says the line ended it");
    }

    /// A job whose waiter holds its lock keeps it, whatever its pid and however
    /// long it waits; a job whose record names no waiter — put in before
    /// v1.1.168, or by `zo line run` — is left as it is: nothing tells whether
    /// its runner stands.
    #[cfg(unix)]
    #[test]
    fn a_standing_waiter_keeps_its_job_and_a_job_naming_none_is_left() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let kept = put(&line, cwd.path(), "w-1", &["true"]);
        let _waiter = hold_waiter_lock(&line, &kept.id);
        named(&line, &kept.id, 1);
        put_long_ago(&line, &kept.id);
        let none = put(&line, cwd.path(), "w-2", &["true"]);
        put_long_ago(&line, &none.id);
        assert_eq!(watch(&line, &none.id), Ok(None));
        let left: Vec<(JobState, Option<u32>)> =
            [&kept, &none].iter().map(|job| line.job(&job.id).expect("a record")).map(|job| (job.state, job.pid)).collect();
        assert_eq!(left, [(JobState::Waiting, Some(1)), (JobState::Waiting, None)]);
        assert!(!line.log_path(&kept.id).exists() && !line.log_path(&none.id).exists(), "no log says the line ended them");
    }

    /// A running job whose waiter holds its lock no more is lost, though another
    /// process has the waiter's pid; one whose waiter holds it runs on, and one
    /// `zo line run` runs, which has no waiter, goes by its runner's pid.
    #[cfg(unix)]
    #[test]
    fn a_running_job_whose_waiter_is_gone_is_lost_though_its_pid_stands() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let running = |job: &mut Job| {
            job.state = JobState::Running;
            job.pid = Some(std::process::id());
            Ok(())
        };
        let gone = put(&line, cwd.path(), "w-1", &["true"]);
        change(&line, &gone.id, running).expect("running");
        drop(hold_waiter_lock(&line, &gone.id));
        let standing = put(&line, cwd.path(), "w-2", &["true"]);
        change(&line, &standing.id, running).expect("running");
        let _waiter = hold_waiter_lock(&line, &standing.id);
        let here = put(&line, cwd.path(), "w-3", &["true"]);
        change(&line, &here.id, running).expect("running");
        assert_eq!(watch(&line, &here.id), Ok(None));
        let states: Vec<JobState> = [&gone, &standing, &here].iter().map(|job| line.job(&job.id).expect("a record").state).collect();
        assert_eq!(states, [JobState::Lost, JobState::Running, JobState::Running]);
    }

    /// One code for an ended job, in its receipt and to every wait: 0 when it
    /// passed, its command's otherwise, and 1 for a command a stop or its cap
    /// ended with 0, and for a job that never ran or whose runner is gone.
    #[test]
    fn an_ended_jobs_code_is_one_for_its_receipt_and_every_wait() {
        let ended = |state, rc| Job { state, rc, ..job_of(&Put { command: words(&["true"]), ..Put::default() }, Path::new("/")) };
        for (state, rc, code) in [
            (JobState::Passed, Some(0), 0),
            (JobState::Failed, Some(3), 3),
            (JobState::Stopped, Some(0), 1),
            (JobState::Stopped, Some(143), 143),
            (JobState::Capped, Some(0), 1),
            (JobState::Withdrawn, None, 1),
            (JobState::Lost, None, 1),
        ] {
            assert_eq!(code_of(&ended(state, rc)), code, "{state:?} {rc:?}");
        }
    }

    /// `zo line add` names the waiter it started in the job's record from the
    /// start; when no waiter can start, nothing is put in — a job no waiter
    /// stands for would wait for good.
    #[cfg(unix)]
    #[test]
    fn an_added_job_names_its_waiter_and_one_no_waiter_starts_for_is_not_put_in() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let put = Put { command: words(&["true"]), ..Put::default() };
        let starts = MachineRules { start_waiter: |_, _| Ok(GIVEN), ..rules(&home.path().join(SLOTS_FOLDER)) };
        let job = put_with_waiter(&line, &starts, job_of(&put, cwd.path())).expect("put in");
        assert_eq!(line.job(&job.id).expect("its record").pid, Some(GIVEN));
        let fails = rules(&home.path().join(SLOTS_FOLDER));
        assert!(put_with_waiter(&line, &fails, job_of(&put, cwd.path())).is_err());
        assert_eq!(line.jobs().len(), 1, "nothing else was put in");
    }

    /// The send that tells a job's end names the run its address is found in:
    /// a run's own, or, for a worker or a pane, the run the pane that put the job
    /// in stood in — whoever tells it (t-119309). A job put in from no run, or
    /// by a zo before it, names none, as before.
    #[test]
    fn a_tell_names_the_run_its_address_is_found_in() {
        let put = Put { what: "검사".to_owned(), command: words(&["true"]), ..Put::default() };
        let run_of = |args: &[String]| args.iter().position(|word| word == "--run").map(|at| args[at + 1].clone());
        for (to, put_from, told_in) in [
            ("worker:w-7", Some("run-put"), Some("run-put")),
            ("pane:team-1/%3", Some("run-put"), Some("run-put")),
            ("worker:w-7", None, None),
            ("run:run-9", None, Some("run-9")),
            ("run:run-9", Some("run-put"), Some("run-9")),
        ] {
            let job = Job { notify: Some(to.to_owned()), notify_run: put_from.map(str::to_owned), ..job_of(&put, Path::new("/")) };
            let args = send_args(&job, to, Duration::from_secs(3));
            assert_eq!(run_of(&args).as_deref(), told_in, "{to} put in from {put_from:?}: {args:?}");
            assert_eq!(args[..5], ["send", "--to", to, "--type", "status"], "{args:?}");
            assert_eq!(args[6], format!("zo line {}: 검사 — waiting (rc -1, 3s)", job.id));
            assert_eq!(args[7..9], ["--retry-request".to_owned(), format!("zo-line-{}", job.id)]);
            assert_eq!(args[9..11], ["--subject".to_owned(), format!("zo-line-{}", job.id)], "t-131644");
        }
    }

    /// Red → green (t-119309, I-t118403-W1): zo's line runs the ledger's command
    /// as zo's Agent tool does. On Windows that is the `.ps1` the window installs,
    /// found on PATH or beside the window's endpoint, through Windows `PowerShell`:
    /// a bare `zerocode-orc` finds only a `zerocode-orc.exe` there, which the
    /// window does not install, so no end the line told on Windows went anywhere.
    /// Elsewhere the command itself; found nowhere, the bare name, whose failure to
    /// start the job's log says.
    #[test]
    fn the_line_runs_the_ledgers_command_as_zos_agent_tool_does() {
        let root = tempfile::tempdir().expect("a folder");
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin).expect("folders");
        for name in ["zerocode-orc", "zerocode-orc.ps1", "zerocode-orc.cmd"] {
            std::fs::write(bin.join(name), "a fixture command; never run here").expect("written");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(bin.join("zerocode-orc"), std::fs::Permissions::from_mode(0o755)).expect("runnable");
        }
        let ps1 = bin.join("zerocode-orc.ps1");
        let on_windows = ledger_command_on(Some(bin.as_os_str()), None, true);
        assert_eq!(on_windows.get_program(), "powershell.exe", "a bare name finds no `.cmd` or `.ps1` on Windows");
        let said: Vec<&OsStr> = on_windows.get_args().collect();
        let policy = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"].map(OsStr::new);
        assert_eq!(said, [&policy[..], &[ps1.as_os_str()][..]].concat());
        let elsewhere = ledger_command_on(Some(bin.as_os_str()), None, false);
        assert_eq!((elsewhere.get_program(), elsewhere.get_args().count()), (bin.join("zerocode-orc").as_os_str(), 0));
        let endpoint = root.path().join("application/agent-hooks/endpoint.env");
        let beside = zerocode_core::agent_shims::orchestration_program(&endpoint, true).expect("a place");
        std::fs::create_dir_all(beside.parent().expect("a folder")).expect("folders");
        std::fs::write(&beside, "a fixture command; never run here").expect("written");
        let empty = root.path().join("empty");
        let found = ledger_command_on(Some(empty.as_os_str()), Some(endpoint.as_path()), true);
        assert_eq!(found.get_args().last(), Some(beside.as_os_str()), "nothing on PATH: the one beside the window's endpoint");
        let none = ledger_command_on(None, None, true);
        assert_eq!((none.get_program(), none.get_args().count()), (OsStr::new("zerocode-orc"), 0));
    }

    /// The words a tell gives the ledger on Windows reach the window's `.ps1`
    /// whole through `PowerShell`'s `-File`: the flags as words, and a body with
    /// quotes, `$(`, a backtick, `%PATH%`, a line break, Korean and a dash. Run
    /// with `--ignored` where `pwsh` is on PATH; `pwsh` on this Mac stands in for
    /// Windows `PowerShell`, and how Windows quotes the command line it starts
    /// (`CreateProcess`) is not seen here (t-119309).
    #[cfg(unix)]
    #[test]
    #[ignore = "needs pwsh on PATH: cargo test -- --ignored"]
    fn the_words_a_tell_gives_powershell_reach_the_script_whole() {
        let root = tempfile::tempdir().expect("a folder");
        std::fs::write(root.path().join("zerocode-orc.ps1"), "[System.IO.File]::WriteAllLines($env:ORC_SEEN, [string[]]$args)\n")
            .expect("the fake");
        let seen = root.path().join("seen");
        let put = Put { what: "검사 \"quoted\" $(not run) `tick` %PATH%\nsecond line — 끝".to_owned(), command: words(&["true"]), ..Put::default() };
        let job = Job { notify_run: Some("run-put".to_owned()), ..job_of(&put, Path::new("/")) };
        let told = send_args(&job, "worker:w-7", Duration::from_secs(1));
        let windows = ledger_command_on(Some(root.path().as_os_str()), None, true);
        let status = Command::new("pwsh").args(windows.get_args()).args(&told).env("ORC_SEEN", &seen).status().expect("pwsh runs");
        assert!(status.success(), "{status}");
        let got = std::fs::read_to_string(&seen).expect("what the script got");
        assert_eq!(got, format!("{}\n", told.join("\n")), "{told:?}");
    }

    /// The run a pane stands in is the one `run-current` names; an answer that
    /// names none says the pane stands in no run.
    #[test]
    fn the_run_a_pane_stands_in_is_the_one_run_current_names() {
        assert_eq!(run_named(br#"{"runId":"run-94975","name":"x","seated":false}"#), Some("run-94975".to_owned()));
        for said in [&b""[..], b"no json", br#"{"runId":""}"#, br#"{"runId":7}"#, br#"["run-1"]"#] {
            assert_eq!(run_named(said), None, "{}", String::from_utf8_lossy(said));
        }
    }

    /// What a failed `zerocode-orc send` said, as the job's log keeps it: its
    /// first line that says anything, cut short.
    #[test]
    fn a_failed_sends_words_are_its_first_line_cut_short() {
        assert_eq!(first_line(b"\n  orchestration: refused\nmore\n"), "orchestration: refused");
        assert_eq!(first_line(b""), "it said nothing");
        assert_eq!(first_line("가".repeat(SAID_CHARS + 5).as_bytes()).chars().count(), SAID_CHARS);
    }

    /// `zo line list` shows a waiting job's pid as its waiter's, beside the job;
    /// a running job's pid stays its runner's, in the job (t-118403).
    #[test]
    fn a_list_names_a_waiting_jobs_waiter_apart_from_a_runner() {
        let mut job = job_of(&Put { command: words(&["true"]), ..Put::default() }, Path::new("/"));
        job.pid = Some(GIVEN);
        let row = listed(&job, Some(1));
        assert_eq!(row["waiter"], GIVEN, "{row}");
        assert!(row["job"].get("pid").is_none(), "{row}");
        job.state = JobState::Running;
        let row = listed(&job, None);
        assert_eq!(row["job"]["pid"], GIVEN, "{row}");
        assert!(row.get("waiter").is_none(), "{row}");
    }

    /// A job runs when its turn comes, its output in its log, and its record
    /// says how it ended; a failed one says its code; a stop ends a running one.
    #[cfg(unix)]
    #[test]
    fn a_job_runs_in_its_turn_and_its_record_and_log_say_how_it_went() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let machine = rules(&home.path().join(SLOTS_FOLDER));

        let ok = put(&line, cwd.path(), "w-1", &["echo built; pwd"]);
        assert_eq!(run_job(&line, &machine, &ok.id, false), Ok(0));
        let ran = line.job(&ok.id).expect("its record");
        assert_eq!((ran.state, ran.rc), (JobState::Passed, Some(0)));
        let log = std::fs::read_to_string(line.log_path(&ok.id)).expect("its log");
        assert!(log.contains("built"), "{log}");
        assert!(line.last_end("w-1") > 0, "its owner's turn moved on");

        let bad = put(&line, cwd.path(), "w-1", &["sh", "-c", "exit 3"]);
        assert_eq!(run_job(&line, &machine, &bad.id, false), Ok(3));
        assert_eq!(line.job(&bad.id).expect("its record").state, JobState::Failed);

        let sleeper = put(&line, cwd.path(), "w-2", &["sleep 30"]);
        let id = sleeper.id.clone();
        let stopper = {
            let line = line.clone();
            std::thread::spawn(move || {
                for _ in 0..200 {
                    if line.job(&id).is_ok_and(|job| job.state == JobState::Running) {
                        let _ = change(&line, &id, |job| {
                            job.stop_asked = true;
                            Ok(())
                        });
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            })
        };
        let rc = run_job(&line, &machine, &sleeper.id, false).expect("ran");
        stopper.join().expect("stopped");
        assert_ne!(rc, 0);
        assert_eq!(line.job(&sleeper.id).expect("its record").state, JobState::Stopped);
        let log = std::fs::read_to_string(line.log_path(&sleeper.id)).expect("its log");
        let last = log.lines().last().unwrap_or_default();
        assert!(last.starts_with("zo line: stopped by its owner after "), "the log says the line ended it: {log:?}");
    }

    /// A job its cap ends says so as its log's last line, on a line of its
    /// own after the output it left, run in the background or here; its
    /// record says capped and its receipt keeps the code the ledger reads.
    #[cfg(unix)]
    #[test]
    fn a_capped_jobs_log_ends_with_the_lines_word_that_the_command_did_not_fail() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let machine = rules(&home.path().join(SLOTS_FOLDER));
        for foreground in [false, true] {
            let put = Put {
                owner: Some(format!("w-{foreground}")),
                cap_secs: Some(1),
                command: words(&["sh", "-c", "echo begun; printf partial; sleep 30"]),
                ..Put::default()
            };
            let job = put_in(&line, &put, cwd.path()).expect("put in");
            let rc = run_job(&line, &machine, &job.id, foreground).expect("ran");
            assert_ne!(rc, 0, "a capped job does not pass");
            assert_eq!(line.job(&job.id).expect("its record").state, JobState::Capped);
            let receipt = std::fs::read_to_string(line.log_path(&job.id).with_extension("rc")).expect("its receipt");
            assert_eq!(receipt, format!("{rc}\n"), "the receipt stays the bare code");
            let log = std::fs::read_to_string(line.log_path(&job.id)).expect("its log");
            let lines: Vec<&str> = log.lines().collect();
            assert_eq!(lines[..2], ["begun", "partial"], "the output first, its unended line ended (foreground {foreground}): {log:?}");
            assert!(
                lines[2].starts_with("zo line: ended past its cap of 1s after ") && lines[2].ends_with("the command did not fail"),
                "foreground {foreground}: {log:?}"
            );
            assert_eq!(lines.len(), 3, "the line's word stands last: {log:?}");
        }
    }

    /// Jobs one process puts in within one millisecond keep an id each: a
    /// shared id let the second record take the first's place (the stacked
    /// v1.1.143 gate saw a warning name the wrong run).
    #[test]
    fn jobs_one_process_puts_in_at_once_keep_an_id_and_a_record_each() {
        let now = now_ms();
        assert_ne!(line::new_id(now, salt(now)), line::new_id(now, salt(now)));
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let put = Put { command: words(&["true"]), ..Put::default() };
        let ids: std::collections::BTreeSet<String> =
            (0..64).map(|_| put_in(&line, &put, cwd.path()).expect("put in").id).collect();
        assert_eq!(ids.len(), 64, "every put has its own id");
        assert_eq!(line.jobs().len(), 64, "and its own record");
    }

    #[test]
    fn a_cap_reads_back_in_the_words_cap_takes() {
        for secs in [90, 900, 1800, 3600, 3660, 7200] {
            assert_eq!(cap_secs(&cap_words(secs)), Ok(secs), "{secs}");
        }
        assert_eq!(cap_words(1800), "30m");
        assert_eq!(cap_words(7200), "2h");
        assert_eq!(cap_words(3660), "61m");
    }

    /// A put whose command outran the cap it gets is warned in its receipt;
    /// one with room, a command the line never ran, or another command is not;
    /// every receipt names the cap.
    #[test]
    fn a_put_is_warned_when_the_same_command_outran_the_cap_it_gets() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let gate = Put { command: words(&["sh", "-c", "just verify"]), ..Put::default() };
        let put = |put: &Put| put_in(&line, put, cwd.path()).expect("put in");
        let ended = |job: &Job, state: JobState, started_ms: i64, ran_secs: u32| {
            change(&line, &job.id, |job| {
                job.state = state;
                job.started_ms = Some(started_ms);
                job.ended_ms = Some(started_ms + i64::from(ran_secs) * 1000);
                Ok(())
            })
            .expect("ended");
        };
        let receipt = |job: &Job| -> serde_json::Value {
            let Ok(Ended::Said(text)) = put_receipt(&line, job, json!({ "job": job.id })) else {
                panic!("a receipt");
            };
            serde_json::from_str(&text).expect("json")
        };

        let first = put(&gate);
        assert_eq!(cap_warning(&line, &first), None, "a command the line never ran has nothing to go by");
        assert_eq!(receipt(&first)["cap"], cap_words(line::CAP_DEFAULT_SECS));
        assert!(receipt(&first).get("warning").is_none());
        let long_ago = now_ms() - 10_000_000;
        ended(&first, JobState::Capped, long_ago, line::CAP_DEFAULT_SECS);

        let again = put(&gate);
        let warned = receipt(&again);
        let warning = warned["warning"].as_str().expect("warned");
        assert!(warning.contains(&first.id) && warning.contains("ended past its cap"), "{warning}");
        assert!(warning.contains(&format!("a cap of {}", cap_words(line::CAP_DEFAULT_SECS))), "{warning}");

        let roomy = put(&Put { cap_secs: Some(line::CAP_DEFAULT_SECS * 2), ..gate.clone() });
        assert_eq!(cap_warning(&line, &roomy), None, "a run capped at a smaller cap says nothing of a larger one");
        ended(&roomy, JobState::Passed, long_ago + 5_000_000, line::CAP_DEFAULT_SECS + 60);
        let warning = cap_warning(&line, &put(&gate)).expect("the newest run outran the default");
        assert!(warning.contains(&roomy.id) && warning.contains(&format!("ran {} s", line::CAP_DEFAULT_SECS + 60)), "{warning}");

        let other = put(&Put { command: words(&["sh", "-c", "just lint"]), ..Put::default() });
        assert_eq!(cap_warning(&line, &other), None, "another command's runs say nothing of this one");
    }

    #[cfg(unix)]
    #[test]
    fn a_withdrawn_job_never_runs_a_pause_holds_the_line_and_a_retry_runs_the_same_command_again() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let machine = rules(&home.path().join(SLOTS_FOLDER));

        let out = put(&line, cwd.path(), "w-1", &["touch ran-out"]);
        change(&line, &out.id, |job| {
            job.state = JobState::Withdrawn;
            Ok(())
        })
        .expect("withdrawn");
        assert_eq!(run_job(&line, &machine, &out.id, false), Ok(1));
        assert!(!cwd.path().join("ran-out").exists(), "a withdrawn job runs nothing");

        line.set_pause(&Pause { by: "person".into(), since_ms: now_ms(), why: String::new(), until: None }).expect("paused");
        let held = put(&line, cwd.path(), "w-2", &["touch ran-held"]);
        assert!(matches!(take_turn(&line, &machine, &held.id), Ok(Turn::Wait)), "nothing starts while paused");
        line.lift_pause().expect("resumed");
        assert_eq!(run_job(&line, &machine, &held.id, false), Ok(0));
        assert!(cwd.path().join("ran-held").exists());

        let first = line.job(&held.id).expect("the run");
        let again = Job { id: line::new_id(now_ms() + 1, 7), state: JobState::Waiting, retry_of: Some(first.id.clone()),
            started_ms: None, ended_ms: None, rc: None, slot: None, pid: None, ..first };
        line.save(&again).expect("put in again");
        assert_eq!(run_job(&line, &machine, &again.id, false), Ok(0));
        assert_eq!(line.job(&again.id).expect("again").retry_of.as_deref(), Some(held.id.as_str()));

        forget(&line, &held.id).expect("forgotten");
        assert!(line.job(&held.id).is_err());
        restore(&line, &held.id).expect("restored");
        assert!(line.job(&held.id).is_ok());
    }

    #[cfg(unix)]
    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("folders");
        std::fs::write(path, text).expect("written");
    }

    /// A checkout of one cargo workspace with one crate, `foo-bar`, in a repository of its own.
    #[cfg(unix)]
    fn workspace() -> tempfile::TempDir {
        let checkout = tempfile::tempdir().expect("a checkout");
        let root = checkout.path();
        std::fs::create_dir_all(root.join(".git")).expect("a repository");
        write(&root.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/foo-bar\"]\nresolver = \"2\"\n");
        write(&root.join("crates/foo-bar/Cargo.toml"), "[package]\nname = \"foo-bar\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        write(&root.join("crates/foo-bar/src/lib.rs"), "pub fn one() {}\n");
        checkout
    }

    /// A job put in from a cargo workspace builds in its slot's folder, without incremental state: the
    /// workspace crates another checkout left there are gone before it starts, its own are gone after it,
    /// and a registry crate's build stays for the next job. Its log says where it built.
    #[cfg(unix)]
    #[test]
    fn a_job_from_a_workspace_builds_in_its_slots_folder_its_workspace_crates_removed_before_and_after() {
        let home = tempfile::tempdir().expect("a zo home");
        let checkout = workspace();
        let line = Line::at(home.path());
        line.make().expect("folders");
        let slots = home.path().join(SLOTS_FOLDER);
        let machine = MachineRules { env: |_| None, ..rules(&slots) };
        for slot in [0, 1] {
            let folder = slot_target::folder(&slots, slot);
            write(&folder.join("debug/.fingerprint/foo-bar-1111/lib-foo_bar"), "another checkout's");
            write(&folder.join("debug/.fingerprint/serde-3333/lib-serde"), "a registry crate's");
        }
        let job = put(&line, checkout.path(), "w-1", &[
            "echo \"dir=$CARGO_TARGET_DIR inc=$CARGO_INCREMENTAL\"; ls \"$CARGO_TARGET_DIR/debug/.fingerprint\"; \
             mkdir -p \"$CARGO_TARGET_DIR/debug/.fingerprint/foo-bar-9999\"",
        ]);
        assert_eq!(run_job(&line, &machine, &job.id, false), Ok(0));
        let slot = line.job(&job.id).expect("its record").slot.expect("its slot");
        let folder = slot_target::folder(&slots, slot);
        let log = std::fs::read_to_string(line.log_path(&job.id)).expect("its log");
        let lines: Vec<&str> = log.lines().collect();
        assert!(lines[0].starts_with(&format!("zo line: cargo builds in {} — ", folder.display())), "{log}");
        assert_eq!(lines[1..], [format!("dir={} inc=0", folder.display()).as_str(), "serde-3333"], "{log}");
        let records: Vec<String> = std::fs::read_dir(folder.join("debug/.fingerprint"))
            .expect("the folder's records")
            .map(|entry| entry.expect("a record").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(records, ["serde-3333"], "the job's own workspace crate is gone after it");
    }

    /// A job whose environment names its own `CARGO_TARGET_DIR` — the release lane's gates, lane-env —
    /// builds there: the line sets nothing on it and makes no folder for it.
    #[cfg(unix)]
    #[test]
    fn a_job_that_names_its_own_build_folder_gets_nothing_from_the_line() {
        let home = tempfile::tempdir().expect("a zo home");
        let checkout = workspace();
        let line = Line::at(home.path());
        line.make().expect("folders");
        let slots = home.path().join(SLOTS_FOLDER);
        let machine = MachineRules { env: |name| (name == "CARGO_TARGET_DIR").then(|| OsString::from("/lane/target")), ..rules(&slots) };
        let job = put(&line, checkout.path(), "w-1", &["echo \"dir=${CARGO_TARGET_DIR-unset} inc=${CARGO_INCREMENTAL-unset}\""]);
        assert_eq!(run_job(&line, &machine, &job.id, false), Ok(0));
        let own = |name: &str| std::env::var(name).unwrap_or_else(|_| "unset".to_owned());
        let log = std::fs::read_to_string(line.log_path(&job.id)).expect("its log");
        assert_eq!(log, format!("dir={} inc={}\n", own("CARGO_TARGET_DIR"), own("CARGO_INCREMENTAL")), "the runner's own, as it was");
        assert!(!slots.join("targets").exists(), "no folder is made");
    }

    /// Near its disk floor the line lets the folders of idle slots go before a job starts — the job still
    /// runs above the floor.
    #[cfg(unix)]
    #[test]
    fn near_the_disk_floor_idle_slots_folders_go_before_a_job_starts() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let slots = home.path().join(SLOTS_FOLDER);
        let machine = MachineRules { floor_gib: 16, free: |_| Some(17 << 30), ..rules(&slots) };
        for slot in [0, 1] {
            write(&slot_target::folder(&slots, slot).join("debug/deps/libserde-3333.rlib"), "");
        }
        let job = put(&line, cwd.path(), "w-1", &["echo ran"]);
        assert_eq!(run_job(&line, &machine, &job.id, false), Ok(0));
        let left: Vec<_> = std::fs::read_dir(slots.join("targets")).expect("the folders' folder").collect();
        assert!(left.is_empty(), "both idle slots' folders went: {left:?}");
    }

    /// The line's floor is its default unless `ZO_LINE_DISK_FLOOR_GIB` names
    /// another in whole GiB, 0 for none; a word that is no such number is
    /// refused, not read as the default (t-119309). No floor holds nothing
    /// back, a full disk too.
    #[test]
    fn a_floor_the_environment_names_is_the_lines_and_a_word_that_is_none_is_refused() {
        assert_eq!(floor_gib(None), Ok(line::DISK_FLOOR_DEFAULT_GIB));
        assert_eq!(floor_gib(Some(OsStr::new(""))), Ok(line::DISK_FLOOR_DEFAULT_GIB), "set empty is unset");
        assert_eq!(floor_gib(Some(OsStr::new("0"))), Ok(0));
        assert_eq!(floor_gib(Some(OsStr::new("24"))), Ok(24));
        for word in ["-1", "16GiB", "1.5", " 8", "lots"] {
            let refused = floor_gib(Some(OsStr::new(word))).expect_err(word);
            assert!(refused.contains(FLOOR_ENV) && refused.contains(&format!("`{word}`")), "{refused}");
        }
        assert!(!disk_low(Some(0), 0), "no floor holds nothing back, whatever the disk holds");
        assert!(disk_low(Some((1 << 30) - 1), 1));
        assert!(USAGE.contains(FLOOR_ENV), "{USAGE}");
    }

    /// `zo line list` answers what an agent asks it — is the line busy, how
    /// did my job end — and not the project's whole history (t-115139 Z6):
    /// 399 jobs came back as 245 KB of one-line JSON. The waiting and running
    /// jobs and the ten that ended last are listed; the older ones are counted.
    #[cfg(unix)]
    #[test]
    fn a_list_shows_the_live_jobs_and_the_last_ended_and_counts_the_rest() {
        let home = tempfile::tempdir().expect("a zo home");
        let cwd = tempfile::tempdir().expect("a folder");
        let line = Line::at(home.path());
        line.make().expect("folders");
        let mut ended = Vec::new();
        for n in 0..14_i64 {
            let job = put(&line, cwd.path(), &format!("w-{n}"), &["true"]);
            change(&line, &job.id, |record| {
                record.state = JobState::Passed;
                record.rc = Some(0);
                record.ended_ms = Some(1_000 + n);
                Ok(())
            })
            .expect("ended");
            ended.push(job.id);
        }
        let waiting = put(&line, cwd.path(), "w-live", &["true"]);
        let Ok(Ended::Said(text)) = listing(&line, cwd.path(), false, false) else {
            panic!("a listing");
        };
        let listed: serde_json::Value = serde_json::from_str(&text).expect("json");
        let ids: Vec<&str> = listed["jobs"]
            .as_array()
            .expect("jobs")
            .iter()
            .filter_map(|row| row["job"]["id"].as_str())
            .collect();
        assert_eq!(ids.len(), 11, "the waiting job and the ten that ended last: {ids:?}");
        assert!(ids.contains(&waiting.id.as_str()), "{ids:?}");
        assert!(ended[4..].iter().all(|id| ids.contains(&id.as_str())), "{ids:?}");
        assert_eq!(listed["olderEndedJobs"], 4, "the older ones are counted: {listed}");
        assert_eq!(listed["paused"], serde_json::Value::Null, "{listed}");
    }

    /// `zo line list --all` still lists every job of every project (t-115139
    /// Z6, the independent review's case): a script finds an old job's code
    /// there by its id, and the list of ten ended jobs left out the one that
    /// ended with 7. This project's own list says how to reach that code.
    #[cfg(unix)]
    #[test]
    fn a_list_of_every_project_still_holds_every_ended_job() {
        let home = tempfile::tempdir().expect("a zo home");
        let (one, two) = (tempfile::tempdir().expect("a folder"), tempfile::tempdir().expect("another"));
        let line = Line::at(home.path());
        line.make().expect("folders");
        let mut ids = Vec::new();
        for n in 0..16_i64 {
            let cwd = if n < 12 { one.path() } else { two.path() };
            let job = put(&line, cwd, &format!("w-{}", n % 4), &["true"]);
            change(&line, &job.id, |record| {
                record.state = if n == 0 { JobState::Failed } else { JobState::Passed };
                record.rc = Some(if n == 0 { 7 } else { 0 });
                record.ended_ms = Some(1_000 + n);
                Ok(())
            })
            .expect("ended");
            ids.push(job.id);
        }
        let Ok(Ended::Said(text)) = listing(&line, one.path(), true, false) else {
            panic!("a listing");
        };
        let every: serde_json::Value = serde_json::from_str(&text).expect("json");
        let rows = every["jobs"].as_array().expect("jobs");
        assert_eq!(rows.len(), 16, "every job of both projects: {every}");
        let first = rows.iter().find(|row| row["job"]["id"] == ids[0].as_str()).expect("the oldest job is listed");
        assert_eq!(first["job"]["rc"], 7, "{first}");
        assert!(every.get("olderEndedJobs").is_none(), "{every}");

        let Ok(Ended::Said(text)) = listing(&line, one.path(), false, false) else {
            panic!("a listing");
        };
        let ours: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(ours["olderEndedJobs"], 2, "{ours}");
        let note = ours["note"].as_str().expect("a note");
        assert!(note.contains("zo line wait <job>") && note.contains("--history"), "{note}");
        assert!(USAGE.contains("--all every job of every project"), "{USAGE}");
    }
}
