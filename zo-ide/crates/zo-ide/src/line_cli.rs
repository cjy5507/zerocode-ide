//! `zo line` — the machine's build line (`zerocode_core::line`): any project's
//! heavy command waits its turn and runs in one of the machine's build slots,
//! the same slots `zo build` runs in. `run` waits and runs the command here,
//! its output on this terminal and in the job's log, and exits as it did — the
//! place `cargo-turn.sh` held; `add` puts it in and returns at once, and a
//! waiter of its own runs it and tells the ledger how it ended. The rest look
//! and act: `list`, `log`, `retry`, `withdraw`, `front`, `stop`, `pause`,
//! `resume`, `forget` and `restore` — the board's buttons run these.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead as _, BufReader, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use zerocode_core::line::{self, Class, Hold, Job, JobState, Line, Machine, Pause, Until};

use crate::build_cli::now_ms;

pub const USAGE: &str = "\
zo line run  [--what <words>] [--task <t-id>] [--first | --last] [--solo] [--quiet] [--cap <30m|900s>]
             [--owner <name>] -- <command...>
zo line add  (the same flags) [--notify <address>] -- <command...>
zo line wait <job>
zo line list [--all]
zo line log <job>
zo line retry <job> · zo line withdraw <job> · zo line front <job> · zo line stop <job>
zo line pause [--until-build <build>] [--why <words>] · zo line resume
zo line forget <job> · zo line restore <job>

  One line for the whole machine: each owner's jobs run in the order they
  were put in and never two at a time; the owner whose last job ended longest
  ago goes first; --first jobs go before plain ones (two to one) and --last
  ones only fill a slot nothing else wants. --solo takes the first slot only
  (a whole suite), --quiet runs alone (a measurement). Nothing starts while
  the line is paused or the disk is below its floor. `run` exits as its
  command did; `add` prints the job and its cap and returns, and the job's
  end is told to --notify through the ledger; `wait` waits for an added job's
  end and exits as its command did. A put whose command outran that cap last
  time is warned; a job the line ends (its cap, a stop) says so as its log's
  last line. Records live under zo's home (line/).
";

/// How often a waiting job looks for its turn, and a running one for a stop.
const LOOK_EVERY: Duration = Duration::from_millis(500);
/// How long a stopped command has to go before its group is killed.
const GRACE: Duration = Duration::from_secs(10);
/// A forgotten job's record waits this long in the trash.
const TRASH_KEPT_MS: i64 = 24 * 60 * 60 * 1000;
/// The lines of a log `zo line log` prints.
const LOG_TAIL_LINES: usize = 200;
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
    List { all: bool },
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
        "list" => match rest {
            [] => Verb::List { all: false },
            [all] if all == "--all" => Verb::List { all: true },
            _ => return Err(format!("unknown argument for `zo line list`\n\n{USAGE}")),
        },
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
    let machine = MachineRules { slots, at_once, floor_gib: line::DISK_FLOOR_DEFAULT_GIB, look_every: LOOK_EVERY };
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
            let job = put_in(&line, &put, cwd)?;
            spawn_waiter(&job.id)?;
            let places = line::places(&line.jobs(), &|owner| line.last_end(owner), line.turn());
            put_receipt(&line, &job, json!({ "job": job.id, "place": places.get(&job.id) }))
        }
        Verb::Wait(id) => run_job(&line, &machine, &id, false).map(Ended::Ran),
        Verb::List { all } => list(&line, cwd, all),
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
                ..old
            };
            line.save(&again).map_err(|error| error.to_string())?;
            spawn_waiter(&again.id)?;
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
    let now = now_ms();
    let job = Job {
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
    };
    line.save(&job).map_err(|error| error.to_string())?;
    Ok(job)
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

/// Starts `zo line wait <id>` on its own, outside this process's group, so it
/// goes on when the terminal that put the job in closes.
fn spawn_waiter(id: &str) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = Command::new(me);
    command
        .args(["line", "wait", id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command.spawn().map(|_| ()).map_err(|error| error.to_string())
}

/// The machine the line runs on.
struct MachineRules {
    slots: PathBuf,
    at_once: u32,
    floor_gib: u64,
    look_every: Duration,
}

/// Waits for job `id`'s turn, runs it, and records how it ended — or, when
/// another process already runs it (the waiter `add` started), waits for that
/// end. Returns the command's exit code (a withdrawn or stopped job's is not 0).
fn run_job(line: &Line, machine: &MachineRules, id: &str, foreground: bool) -> Result<i32, String> {
    std::fs::create_dir_all(&machine.slots).map_err(|error| error.to_string())?;
    let held = loop {
        match take_turn(line, machine, id)? {
            Turn::Mine(held) => break held,
            Turn::Ended(rc) => return Ok(rc),
            Turn::Wait => std::thread::sleep(machine.look_every),
        }
    };
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

/// One look, under the line's lock: is it job `id`'s turn, and is a slot
/// free for it? Its turn and a slot it can take make it running.
fn take_turn(line: &Line, machine: &MachineRules, id: &str) -> Result<Turn, String> {
    let _lock = lock(line)?;
    let mut jobs = line.jobs();
    mark_lost(line, &mut jobs);
    let Some(mine) = jobs.iter().find(|job| job.id == id).cloned() else {
        return Err(format!("the line holds no job {id}"));
    };
    match mine.state {
        JobState::Waiting if mine.stop_asked => {
            let mut job = mine;
            job.state = JobState::Withdrawn;
            job.ended_ms = Some(now_ms());
            line.save(&job).map_err(|error| error.to_string())?;
            return Ok(Turn::Ended(1));
        }
        JobState::Waiting => {}
        // Another process runs it: its end is the answer (a dead runner's job is lost above).
        JobState::Running => return Ok(Turn::Wait),
        _ => return Ok(Turn::Ended(mine.rc.unwrap_or(1))),
    }
    let pause = lift_ended_pause(line);
    let (free, taken) = slot_files(machine)?;
    let running_slots: Vec<u32> = jobs.iter().filter(|job| job.state == JobState::Running).filter_map(|job| job.slot).collect();
    let state = Machine {
        taken: taken.into_iter().filter(|slot| !running_slots.contains(slot)).collect(),
        slots: machine.at_once,
        disk_low: disk_low(line.root(), machine.floor_gib),
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
/// owner may go on.
fn mark_lost(line: &Line, jobs: &mut [Job]) {
    for job in jobs.iter_mut().filter(|job| job.state == JobState::Running) {
        if job.pid.is_some_and(|pid| !alive(pid)) {
            job.state = JobState::Lost;
            job.ended_ms = Some(now_ms());
            let _ = line.save(job);
            let _ = line.ended(&job.owner, now_ms());
        }
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

/// Free disk under `folder` below `floor_gib`.
fn disk_low(folder: &Path, floor_gib: u64) -> bool {
    free_bytes(folder).is_some_and(|free| free < floor_gib.saturating_mul(1 << 30))
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
    let log = File::create(&log_path).map_err(|error| error.to_string())?;
    // The same open file as the command's: what the line writes under its output follows it, and is
    // not written over by it.
    let closing = log.try_clone().map_err(|error| error.to_string())?;
    let mut command = command_of(&job);
    command.current_dir(&job.checkout).stdin(Stdio::null());
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
    let receipt = if job.state == JobState::Passed { 0 } else if rc == 0 { 1 } else { rc };
    let _ = std::fs::write(line.log_path(&job.id).with_extension("rc"), format!("{receipt}\n"));
    {
        let _lock = lock(line)?;
        line.save(&job).map_err(|error| error.to_string())?;
        line.ended(&job.owner, now_ms()).map_err(|error| error.to_string())?;
    }
    line.prune();
    purge_trash(line, now_ms());
    notify(&job, started.elapsed());
    Ok(receipt)
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
/// `zerocode-orc send` on this machine's PATH. A send that cannot go is left.
fn notify(job: &Job, took: Duration) {
    let Some(to) = job.notify.as_deref() else {
        return;
    };
    let what = if job.what.is_empty() { job.command.join(" ") } else { job.what.clone() };
    let body = format!(
        "zo line {}: {what} — {} (rc {}, {}s)",
        job.id,
        state_word(job.state),
        job.rc.unwrap_or(-1),
        took.as_secs()
    );
    let mut send = Command::new("zerocode-orc");
    send.args(["send", "--to", to, "--type", "status", "--body", &body]);
    if let Some(run) = to.strip_prefix("run:") {
        send.args(["--run", run]);
    }
    let _ = send
        .current_dir(&job.checkout)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

const fn state_word(state: JobState) -> &'static str {
    match state {
        JobState::Waiting => "waiting",
        JobState::Running => "running",
        JobState::Passed => "passed",
        JobState::Failed => "failed",
        JobState::Capped => "ended past its cap",
        JobState::Stopped => "stopped",
        JobState::Withdrawn => "withdrawn",
        JobState::Lost => "lost",
    }
}

/// `zo line list`: this project's jobs (every project's with `--all`), each
/// waiting one with its place.
fn list(line: &Line, cwd: &Path, all: bool) -> Result<Ended, String> {
    let project = project_of(cwd);
    let jobs = line.jobs();
    let places = line::places(&jobs, &|owner| line.last_end(owner), line.turn());
    let shown: Vec<serde_json::Value> = jobs
        .iter()
        .filter(|job| all || job.project == project)
        .map(|job| json!({ "job": job, "place": places.get(&job.id) }))
        .collect();
    said(&json!({ "paused": line.pause(), "jobs": shown }))
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
        MachineRules { slots: slots.to_path_buf(), at_once: 2, floor_gib: 0, look_every: Duration::from_millis(20) }
    }

    #[cfg(unix)]
    fn put(line: &Line, cwd: &Path, owner: &str, command: &[&str]) -> Job {
        put_in(line, &Put { owner: Some(owner.to_owned()), command: words(command), ..Put::default() }, cwd).expect("put in")
    }

    /// A second waiter on a job another runner holds (a person's `zo line wait`
    /// beside the waiter `add` started) waits for its end and takes its code;
    /// the job runs once.
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
}
