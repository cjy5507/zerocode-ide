//! `zo build` — runs a project's builds as its `.zo/build.toml` orders them,
//! into the records the window's task board reads (`zerocode_core::builds`).
//!
//! One runner holds a project at a time ([`build_run::hold`]). It begins a
//! record for every build it was asked for, so the board shows them waiting
//! their turn, then runs them in the file's order and each one's steps in
//! theirs, each step's output going to the log beside its run's record. A
//! step that fails ends its build, and a build that waits for one that did not
//! pass (`after`) is cancelled. A step that writes nothing for long is said
//! stalled and, past the quiet rule's end, ended ([`build_run::QuietRule`]); a
//! step with a `cap` is ended once it runs past it. A `when = "manual"` step
//! waits for a person: its build stops there, and `--step` goes on from it.
//! The machine runs at most `buildsAtOnce` builds at once, whatever their
//! projects. `zo build stop` stops the runner, as Ctrl-C, a hang-up and
//! SIGTERM do.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use plugins::bounded_child::{self, Group};
use serde_json::json;
use zerocode_core::build_file::{self, BuildSpec, StepSpec, When};
use zerocode_core::build_progress::Reader;
use zerocode_core::build_run::{self, QuietRule};
use zerocode_core::builds::{self, Outcome, Run, Stalled, Usual};

use crate::build_cli::{USAGE, now_ms};

/// How many builds a machine runs at once when its settings do not say.
pub const AT_ONCE_DEFAULT: usize = 2;
/// How often a running step is looked at: its exit, its log, a stop.
const LOOK_EVERY: Duration = Duration::from_secs(1);
/// How long a step that is ended has to go before its group is killed.
const GRACE: Duration = Duration::from_secs(10);
/// How long the `taskkill` that ends a cut step's tree may take on Windows.
#[cfg(windows)]
const TREE_END_WAIT: Duration = Duration::from_secs(5);
/// The machine's slots, under the Zo home: one locked file a running build.
const SLOTS_FOLDER: &str = "build-slots";
/// The exit code a shell gives a child a signal ended: this plus the signal.
#[cfg(unix)]
const SIGNALLED_RC_BASE: i32 = 128;
/// The exit code of a step that could not be started, as a shell gives it.
const NOT_STARTED_RC: i32 = 127;
/// How much of a step's output its reader reads at once, and at most each
/// look; the rest waits for the next look, and all of it for the step's end.
const READ_CHUNK: usize = 64 * 1024;
const READ_MOST: usize = 4 * 1024 * 1024;

/// What a runner keeps to: the quiet rule, how often it looks at a running
/// step, how long a step it ends has to go, and the machine's slots.
#[derive(Debug, Clone)]
pub struct Rules {
    pub quiet: QuietRule,
    pub look_every: Duration,
    pub grace: Duration,
    pub at_once: usize,
    pub slots: PathBuf,
}

impl Rules {
    /// The rules of this machine: its settings' `buildsAtOnce`, its Zo home.
    #[must_use]
    pub fn of_this_machine() -> Self {
        Self {
            quiet: QuietRule::DEFAULT,
            look_every: LOOK_EVERY,
            grace: GRACE,
            at_once: crate::preferences::load()
                .builds_at_once
                .unwrap_or(AT_ONCE_DEFAULT),
            slots: runtime::default_config_home().join(SLOTS_FOLDER),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Verb {
    Run {
        builds: Vec<String>,
        step: Option<String>,
        label: Option<String>,
    },
    Stop {
        builds: Vec<String>,
    },
    Approve {
        builds: Vec<String>,
    },
}

const STOP: &str = "stop";
const APPROVE: &str = "approve";

fn parse(args: &[String]) -> Result<Verb, String> {
    let (verb, words) = match args.split_first() {
        Some((first, rest)) if first == STOP || first == APPROVE => (first.as_str(), rest),
        Some((first, _)) if !first.starts_with('-') => {
            return Err(format!("unknown `zo build` verb `{first}`\n\n{USAGE}"));
        }
        _ => ("", args),
    };
    let (mut builds, mut step, mut label) = (Vec::new(), None, None);
    let mut words = words.iter();
    while let Some(flag) = words.next() {
        let mut value = || {
            words
                .next()
                .cloned()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--build" => builds.push(value()?),
            "--step" if verb.is_empty() => step = Some(value()?),
            "--label" if verb.is_empty() => label = Some(value()?),
            _ => return Err(format!("unknown argument `{flag}`\n\n{USAGE}")),
        }
    }
    if step.is_some() && builds.len() != 1 {
        return Err("--step goes on with one build: name it with one --build".to_owned());
    }
    Ok(match verb {
        STOP => Verb::Stop { builds },
        APPROVE => Verb::Approve { builds },
        _ => Verb::Run {
            builds,
            step,
            label,
        },
    })
}

/// `zo build`, `zo build stop` and `zo build approve`, from `cwd`.
///
/// # Errors
///
/// One sentence a person reads: why nothing ran, or which build did not pass.
pub fn run(args: &[String], cwd: &Path) -> Result<String, String> {
    let verb = parse(args)?;
    let checkout = checkout_of(cwd)?;
    match verb {
        Verb::Run {
            builds,
            step,
            label,
        } => {
            let stop = Arc::new(AtomicBool::new(false));
            stop_on_signals(Arc::clone(&stop));
            Runner::new(&checkout, Rules::of_this_machine(), stop)?.run(
                &builds,
                step.as_deref(),
                label.as_deref(),
            )
        }
        Verb::Stop { builds } => stop(&checkout, &builds),
        Verb::Approve { builds } => approve(&checkout, &builds),
    }
}

/// The root of the checkout `cwd` sits in, where its order file is.
fn checkout_of(cwd: &Path) -> Result<PathBuf, String> {
    cwd.ancestors()
        .find(|folder| zerocode_core::git_dir::of(folder).is_some())
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} is not inside a git checkout", cwd.display()))
}

fn shared_of(checkout: &Path) -> Result<PathBuf, String> {
    zerocode_core::git_dir::shared_of(checkout)
        .ok_or_else(|| format!("{} is not inside a git checkout", checkout.display()))
}

fn listed(commands: &[String]) -> String {
    commands
        .iter()
        .map(|command| format!("  {command}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Turns Ctrl-C, SIGTERM and a hang-up into a stop: the runner ends its step's
/// group and the record says so, rather than the step going on alone.
fn stop_on_signals(stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        runtime.block_on(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                let (Ok(mut term), Ok(mut hangup)) =
                    (signal(SignalKind::terminate()), signal(SignalKind::hangup()))
                else {
                    return;
                };
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                    _ = hangup.recv() => {}
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            stop.store(true, Ordering::SeqCst);
        });
    });
}

/// How one build's run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    Passed,
    Failed,
    /// It waits for a build that did not pass.
    Cancelled,
    /// It waits at a step a person presses.
    Waiting,
    Stopped,
}

impl Ended {
    const fn word(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Waiting => "waits for a person",
            Self::Stopped => "stopped",
        }
    }
}

/// How one step ended.
enum StepEnd {
    Passed,
    Failed(String),
    Stopped,
}

/// Why the runner ended a step before it exited.
enum Cut {
    Stopped,
    Failed(String),
}

struct Runner {
    checkout: PathBuf,
    shared: PathBuf,
    rules: Rules,
    stop: Arc<AtomicBool>,
}

impl Runner {
    fn new(checkout: &Path, rules: Rules, stop: Arc<AtomicBool>) -> Result<Self, String> {
        Ok(Self {
            shared: shared_of(checkout)?,
            checkout: checkout.to_path_buf(),
            rules,
            stop,
        })
    }

    /// Whether this runner is to stop: asked by `zo build stop` or a signal.
    fn stopping(&self) -> bool {
        if build_run::stop_asked(&self.shared) {
            self.stop.store(true, Ordering::SeqCst);
        }
        self.stop.load(Ordering::SeqCst)
    }

    fn save(path: &Path, run: &Run) -> Result<(), String> {
        builds::save(path, run).map_err(|error| error.to_string())
    }

    fn run(
        &self,
        asked: &[String],
        step: Option<&str>,
        label: Option<&str>,
    ) -> Result<String, String> {
        let file = build_file::read(&self.checkout)?;
        let queue = build_run::queue(&file, asked)?;
        let unapproved = build_run::unapproved(&self.shared, &build_run::commands_of(&queue));
        if !unapproved.is_empty() {
            return Err(format!(
                "these commands have not been allowed to run here:\n{}\nallow them with `zo build approve`",
                listed(&unapproved)
            ));
        }
        let sha = git(&self.checkout, &["rev-parse", "HEAD"]).unwrap_or_default();
        let version = label
            .map(str::to_owned)
            .or_else(|| git(&self.checkout, &["describe", "--tags", "--always"]))
            .unwrap_or_default();
        let values = build_run::values(&sha, &version)?;
        for build in &queue {
            for each in &build.steps {
                if let Some(name) = build_run::unfilled(build_run::command_of(each), &values) {
                    return Err(format!(
                        "the step `{}` of `{}` uses `${{{name}}}`, which this checkout cannot fill",
                        each.id, build.id
                    ));
                }
            }
        }
        let Some(_hold) = build_run::hold(&self.shared).map_err(|error| error.to_string())? else {
            return Err(
                "builds are already running in this project; `zo build stop` stops them".to_owned(),
            );
        };
        // An ask left before this runner held the project is not for it.
        let _ = build_run::stop_asked(&self.shared);
        let mut runs = if let Some(step) = step {
            vec![self.resume(queue[0], step)?]
        } else {
            let now = now_ms();
            queue
                .iter()
                .map(|build| self.begin(build, &sha, label.unwrap_or_default(), now))
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut ended: Vec<(&str, Ended)> = Vec::new();
        for (build, (path, run)) in queue.iter().zip(runs.iter_mut()) {
            let after = build.after.as_deref().filter(|after| {
                ended
                    .iter()
                    .any(|(id, how)| id == after && *how != Ended::Passed)
            });
            let how = if self.stopping() {
                run.end(Outcome::Cancelled, String::new());
                Self::save(path, run)?;
                Ended::Stopped
            } else if let Some(after) = after {
                run.end(Outcome::Cancelled, format!("`{after}` did not pass"));
                Self::save(path, run)?;
                Ended::Cancelled
            } else {
                self.run_build(build, path, run, step, &values)?
            };
            println!("{}: {}", build.id, how.word());
            ended.push((build.id.as_str(), how));
        }
        let summary = ended
            .iter()
            .map(|(id, how)| format!("{id} {}", how.word()))
            .collect::<Vec<_>>()
            .join(", ");
        if ended
            .iter()
            .all(|(_, how)| matches!(how, Ended::Passed | Ended::Waiting))
        {
            Ok(summary)
        } else {
            Err(summary)
        }
    }

    /// A new run of `build`, waiting its turn: its plan, its commit and its
    /// log beside its record.
    fn begin(&self, build: &BuildSpec, sha: &str, label: &str, now: i64) -> Result<(PathBuf, Run), String> {
        let run = Run {
            name: build.name.clone(),
            label: label.to_owned(),
            tool: build.tool.clone().unwrap_or_default(),
            sha: sha.to_owned(),
            started_ms: now,
            plan: build_run::plan_of(build),
            log: builds::log_path(&self.shared, &build.id, now)
                .display()
                .to_string(),
            ..Run::default()
        };
        let path = builds::begin(&self.shared, &build.id, &run).map_err(|error| error.to_string())?;
        Ok((path, run))
    }

    /// The run of `build` that waits at `step` for a person, taken up again.
    fn resume(&self, build: &BuildSpec, step: &str) -> Result<(PathBuf, Run), String> {
        let (path, mut run) = builds::running(&self.shared, &build.id).map_err(|error| error.to_string())?;
        if run.waiting.as_deref() != Some(step) || !build.steps.iter().any(|each| each.id == step) {
            return Err(format!("`{}` does not wait at a step `{step}`", build.id));
        }
        run.waiting = None;
        Self::save(&path, &run)?;
        Ok((path, run))
    }

    /// Runs `build`'s steps in order, from `from` when it goes on from a step
    /// a person pressed, while it holds one of the machine's slots.
    fn run_build(
        &self,
        build: &BuildSpec,
        path: &Path,
        run: &mut Run,
        from: Option<&str>,
        values: &BTreeMap<&str, String>,
    ) -> Result<Ended, String> {
        let Some(_slot) = self.slot()? else {
            run.end(Outcome::Cancelled, String::new());
            Self::save(path, run)?;
            return Ok(Ended::Stopped);
        };
        let usual = builds::usual(&self.shared, &build.id);
        let mut pressed = from;
        for step in build
            .steps
            .iter()
            .skip_while(|step| from.is_some_and(|from| step.id != from))
        {
            if step.when == When::Manual && pressed != Some(step.id.as_str()) {
                run.waiting = Some(step.id.clone());
                Self::save(path, run)?;
                return Ok(Ended::Waiting);
            }
            pressed = None;
            let usual = usual.get(&step.id).copied().unwrap_or_default();
            match self.run_step(build, step, path, run, usual, values)? {
                StepEnd::Passed => {}
                StepEnd::Failed(reason) => {
                    run.end(Outcome::Failed, reason);
                    Self::save(path, run)?;
                    return Ok(Ended::Failed);
                }
                StepEnd::Stopped => {
                    run.end(Outcome::Cancelled, String::new());
                    Self::save(path, run)?;
                    return Ok(Ended::Stopped);
                }
            }
        }
        run.end(Outcome::Passed, String::new());
        Self::save(path, run)?;
        Ok(Ended::Passed)
    }

    /// One of the machine's slots, waited for; none when the runner is
    /// stopped while it waits.
    fn slot(&self) -> Result<Option<File>, String> {
        std::fs::create_dir_all(&self.rules.slots).map_err(|error| error.to_string())?;
        loop {
            for slot in 0..self.rules.at_once.max(1) {
                let file = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(self.rules.slots.join(format!("{slot}.lock")))
                    .map_err(|error| error.to_string())?;
                if file.try_lock().is_ok() {
                    return Ok(Some(file));
                }
            }
            if self.stopping() {
                return Ok(None);
            }
            std::thread::sleep(self.rules.look_every);
        }
    }

    fn run_step(
        &self,
        build: &BuildSpec,
        step: &StepSpec,
        path: &Path,
        run: &mut Run,
        usual: Usual,
        values: &BTreeMap<&str, String>,
    ) -> Result<StepEnd, String> {
        let command = build_file::fill(build_run::command_of(step), values);
        let log_path = PathBuf::from(&run.log);
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|error| format!("could not open {}: {error}", log_path.display()))?;
        let _ = writeln!(log, "==> {}: {command}", step.id);
        println!("{} · {}: {command}", build.id, step.id);
        let output = build
            .tool
            .as_deref()
            .and_then(|tool| Reader::of_tool(tool, usual.units))
            .and_then(|reader| Output::open(&log_path, reader));
        run.start_step(&step.id, now_ms());
        run.progress = output.as_ref().map(|output| output.reader.progress());
        Self::save(path, run)?;
        let started = Instant::now();
        let child = log
            .try_clone()
            .and_then(|out| Ok((out, log.try_clone()?)))
            .and_then(|(out, err)| {
                shell(&command)
                    .current_dir(&self.checkout)
                    .stdin(Stdio::null())
                    .stdout(out)
                    .stderr(err)
                    .spawn()
            });
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                let _ = writeln!(log, "<== {} did not start: {error}", step.id);
                run.end_step(&step.id, NOT_STARTED_RC, Some(0), now_ms());
                Self::save(path, run)?;
                return Ok(StepEnd::Failed(format!("`{}` did not start: {error}", step.id)));
            }
        };
        let mut watched = Watched {
            step,
            command: &command,
            usual: usual.secs,
            log_path: &log_path,
            started,
            output,
        };
        let (status, cut) = self.watch(&mut child, &mut watched, path, run)?;
        if let Some(output) = watched.output.as_mut() {
            output.read(usize::MAX);
            run.progress = Some(output.reader.progress());
        }
        let rc = rc_of(status);
        let secs = u32::try_from(started.elapsed().as_secs()).unwrap_or(u32::MAX);
        let _ = writeln!(log, "<== {} rc={rc} {secs}s", step.id);
        run.end_step(&step.id, rc, Some(secs), now_ms());
        Self::save(path, run)?;
        Ok(match cut {
            None if rc == 0 => StepEnd::Passed,
            None => StepEnd::Failed(format!("`{}` exited {rc}", step.id)),
            Some(Cut::Stopped) => StepEnd::Stopped,
            Some(Cut::Failed(reason)) => StepEnd::Failed(reason),
        })
    }

    /// Watches a running step until it exits, or the runner ends it: stopped,
    /// past its cap, or quiet past the rule's end. A step quiet past the
    /// rule's saying is said stalled, and unsaid when it writes again; a step
    /// whose output is read says how far it is.
    fn watch(
        &self,
        child: &mut Child,
        watched: &mut Watched<'_>,
        path: &Path,
        run: &mut Run,
    ) -> Result<(ExitStatus, Option<Cut>), String> {
        let step = watched.step;
        let quiet = self.rules.quiet.limits(watched.usual, step.cap.is_some());
        let cap = step
            .cap
            .as_deref()
            .and_then(build_file::cap_secs)
            .map(|secs| Duration::from_secs(u64::from(secs)));
        let length = |path: &Path| std::fs::metadata(path).map_or(0, |meta| meta.len());
        let (mut heard_length, mut heard_at, mut heard_ms) =
            (length(watched.log_path), Instant::now(), now_ms());
        let cut = loop {
            if let Some(status) = bounded_child::reap_if_exited(child, Some(Group::Kill))
                .map_err(|error| error.to_string())?
            {
                return Ok((status, None));
            }
            if self.stopping() {
                break Cut::Stopped;
            }
            let mut changed = false;
            let heard = length(watched.log_path);
            if heard != heard_length {
                (heard_length, heard_at, heard_ms) = (heard, Instant::now(), now_ms());
                changed |= run.stalled.take().is_some();
            }
            if let Some(output) = watched.output.as_mut() {
                if output.read(READ_MOST) {
                    run.progress = Some(output.reader.progress());
                    changed = true;
                }
            }
            if changed {
                Self::save(path, run)?;
            }
            let silent = heard_at.elapsed();
            if run.stalled.is_none() && silent >= Duration::from_millis(quiet.say_ms) {
                run.stalled = Some(Stalled {
                    step: step.id.clone(),
                    what: watched.command.to_owned(),
                    quiet_since_ms: heard_ms,
                    cap_secs: quiet.end_ms.map_or(0, |end| {
                        u32::try_from(end.div_ceil(1000)).unwrap_or(u32::MAX)
                    }),
                });
                Self::save(path, run)?;
            }
            if quiet
                .end_ms
                .is_some_and(|end| silent >= Duration::from_millis(end))
            {
                break Cut::Failed(format!(
                    "`{}` wrote nothing for {}s",
                    step.id,
                    silent.as_secs()
                ));
            }
            if cap.is_some_and(|cap| watched.started.elapsed() >= cap) {
                break Cut::Failed(format!(
                    "`{}` ran past its cap of {}",
                    step.id,
                    step.cap.as_deref().unwrap_or_default()
                ));
            }
            // The step's exit wakes the look at once, so a step's time is its
            // own; where the platform has no watch on it, the look comes round
            // after its time.
            let next = Instant::now() + self.rules.look_every;
            if bounded_child::observe_exit(child.id(), Some(next)).is_none() {
                std::thread::sleep(self.rules.look_every);
            }
        };
        Ok((end_child(child, self.rules.grace)?, Some(cut)))
    }
}

/// A running step as the runner watches it.
struct Watched<'step> {
    step: &'step StepSpec,
    command: &'step str,
    /// What its passing runs usually took.
    usual: Option<u32>,
    log_path: &'step Path,
    started: Instant,
    output: Option<Output>,
}

/// A running step's output, as its tool's reader reads it from the log.
struct Output {
    file: File,
    reader: Reader,
    buffer: Vec<u8>,
}

impl Output {
    /// Read from the end of `log_path` as it stands, where the step's own
    /// output begins.
    fn open(log_path: &Path, reader: Reader) -> Option<Self> {
        let mut file = File::open(log_path).ok()?;
        file.seek(SeekFrom::End(0)).ok()?;
        Some(Self {
            file,
            reader,
            buffer: vec![0; READ_CHUNK],
        })
    }

    /// Reads what the step wrote since the last read, `most` bytes at most:
    /// whether it moved on.
    fn read(&mut self, most: usize) -> bool {
        let mut moved = false;
        let mut left = most;
        while left > 0 {
            let want = left.min(self.buffer.len());
            match self.file.read(&mut self.buffer[..want]) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    moved |= self.reader.read(&self.buffer[..read]);
                    left -= read;
                }
            }
        }
        moved
    }
}

/// The shell a step's command runs in, as its own process group, so that the
/// runner can end the step and everything it started.
fn shell(command: &str) -> Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let mut shell = Command::new("/bin/sh");
        shell.arg("-c").arg(command).process_group(0);
        shell
    }
    #[cfg(not(unix))]
    {
        let mut shell = Command::new("cmd");
        shell.arg("/C").arg(command);
        shell
    }
}

/// Ends a step the runner cut short: SIGTERM to its group, SIGKILL once it
/// has exited or its grace has passed, then the reap. Windows has no group, so
/// `taskkill /T` ends the step's `cmd` and what it started.
fn end_child(child: &mut Child, grace: Duration) -> Result<ExitStatus, String> {
    #[cfg(unix)]
    if let Some(end) = bounded_child::GroupEnd::begin(child.id(), Group::Terminate { grace }) {
        if let Ok(Some(grace)) = end.first() {
            if bounded_child::observe_exit(child.id(), Some(Instant::now() + grace)).is_none() {
                std::thread::sleep(grace);
            }
            let _ = end.finish();
        }
    }
    #[cfg(windows)]
    end_tree(child.id());
    #[cfg(not(unix))]
    {
        let _ = (child.kill(), grace);
    }
    bounded_child::reap(child).map_err(|error| error.to_string())
}

/// Runs `taskkill /T` on a step's `cmd` and waits for it a bounded time, so a
/// tool that does not answer cannot hold the stop; the `cmd` is killed after
/// it either way.
#[cfg(windows)]
fn end_tree(pid: u32) {
    let Ok(mut taskkill) = runtime::end_process_tree_command(pid).spawn() else {
        return;
    };
    let until = Instant::now() + TREE_END_WAIT;
    while matches!(bounded_child::try_reap(&mut taskkill), Ok(None)) && Instant::now() < until {
        std::thread::sleep(bounded_child::EXIT_LOOK_EVERY);
    }
    let _ = taskkill.kill();
    let _ = bounded_child::reap(&mut taskkill);
}

fn rc_of(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return SIGNALLED_RC_BASE + signal;
        }
    }
    status.code().unwrap_or(-1)
}

fn git(checkout: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(checkout)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}

/// `zo build stop`: the runner that holds the project is asked to stop; a
/// run that waits for a person, or that no runner holds any more, is ended
/// here. A build another script reports is never touched.
fn stop(checkout: &Path, asked: &[String]) -> Result<String, String> {
    let shared = shared_of(checkout)?;
    let file = build_file::read(checkout)?;
    let queue = build_run::queue(&file, asked)?;
    let runner = build_run::held(&shared);
    let (mut ended, mut asked_runner) = (Vec::new(), false);
    for build in queue {
        let Ok((path, mut run)) = builds::running(&shared, &build.id) else {
            continue;
        };
        if runner && run.waiting.is_none() {
            asked_runner = true;
            continue;
        }
        run.end(Outcome::Cancelled, String::new());
        builds::save(&path, &run).map_err(|error| error.to_string())?;
        ended.push(build.id.clone());
    }
    if asked_runner {
        build_run::ask_stop(&shared).map_err(|error| error.to_string())?;
    }
    serde_json::to_string(&json!({ "ended": ended, "runner_asked": asked_runner }))
        .map_err(|error| error.to_string())
}

/// `zo build approve`: the commands of the builds asked for that no one has
/// allowed here are allowed, and listed.
fn approve(checkout: &Path, asked: &[String]) -> Result<String, String> {
    let shared = shared_of(checkout)?;
    let file = build_file::read(checkout)?;
    let commands = build_run::unapproved(
        &shared,
        &build_run::commands_of(&build_run::queue(&file, asked)?),
    );
    build_run::approve(&shared, &commands)?;
    Ok(if commands.is_empty() {
        "every command here was allowed already".to_owned()
    } else {
        format!("allowed:\n{}", listed(&commands))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use zerocode_core::builds::Progress;

    fn checkout_with(order: &str) -> tempfile::TempDir {
        let checkout = tempfile::tempdir().expect("a checkout");
        std::fs::create_dir_all(checkout.path().join(".git")).expect("its git folder");
        std::fs::create_dir_all(checkout.path().join(".zo")).expect("its .zo folder");
        std::fs::write(checkout.path().join(".zo/build.toml"), order).expect("its order");
        checkout
    }

    fn rules(slots: &Path) -> Rules {
        Rules {
            quiet: QuietRule::DEFAULT,
            look_every: Duration::from_millis(20),
            grace: Duration::from_millis(300),
            at_once: 2,
            slots: slots.to_path_buf(),
        }
    }

    #[cfg(unix)]
    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    fn runner(checkout: &Path, rules: Rules) -> Runner {
        Runner::new(checkout, rules, Arc::new(AtomicBool::new(false))).expect("a runner")
    }

    fn newest(checkout: &Path, id: &str) -> Option<Run> {
        let shared = shared_of(checkout).expect("a repository");
        let mut records: Vec<PathBuf> = std::fs::read_dir(builds::folder(&shared).join(id))
            .ok()?
            .map(|entry| entry.expect("an entry").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
            .collect();
        records.sort();
        let text = std::fs::read_to_string(records.last()?).expect("readable");
        Some(serde_json::from_str(&text).expect("a run"))
    }

    fn wait_for(what: &str, ready: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(Instant::now() < until, "waited too long for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    const IN_ORDER: &str = r#"
[[build]]
id = "app"
step = [
  { id = "one", run = "echo first" },
  { id = "two", run = "echo second; exit 3" },
  { id = "three", run = "echo never" },
]

[[build]]
id = "image"
after = "app"
step = [{ id = "build", run = "echo image" }]

[[build]]
id = "lint"
step = [{ id = "check", run = "true" }]
"#;

    #[cfg(unix)]
    #[test]
    fn builds_run_in_order_and_a_failed_step_ends_its_build_and_the_ones_after_it() {
        let checkout = checkout_with(IN_ORDER);
        let slots = tempfile::tempdir().expect("slots");
        approve(checkout.path(), &[]).expect("allowed");

        let said = runner(checkout.path(), rules(slots.path()))
            .run(&[], None, None)
            .expect_err("app failed");

        let app = newest(checkout.path(), "app").expect("app ran");
        assert_eq!(said, "app failed, image cancelled, lint passed");
        assert_eq!(
            (app.outcome, app.reason.as_str()),
            (Some(Outcome::Failed), "`two` exited 3")
        );
        assert_eq!(
            app.done.iter().map(|step| (step.name.as_str(), step.rc)).collect::<Vec<_>>(),
            [("one", 0), ("two", 3)]
        );
        let log = std::fs::read_to_string(&app.log).expect("its log beside its record");
        assert!(log.contains("first") && log.contains("second") && !log.contains("never"), "{log}");
        let image = newest(checkout.path(), "image").expect("image was queued");
        assert_eq!(
            (image.outcome, image.reason.as_str(), image.done.len()),
            (Some(Outcome::Cancelled), "`app` did not pass", 0)
        );
        assert_eq!(newest(checkout.path(), "lint").and_then(|run| run.outcome), Some(Outcome::Passed));
    }

    #[cfg(unix)]
    #[test]
    fn a_manual_step_waits_for_a_person_and_the_run_goes_on_from_it() {
        let checkout = checkout_with(
            r#"
[[build]]
id = "image"
step = [
  { id = "build", run = "echo built" },
  { id = "push", run = "echo pushed", when = "manual" },
  { id = "tag", run = "echo tagged" },
]
"#,
        );
        let slots = tempfile::tempdir().expect("slots");
        approve(checkout.path(), &[]).expect("allowed");

        let first = runner(checkout.path(), rules(slots.path())).run(&[], None, None);
        let waiting = newest(checkout.path(), "image").expect("a run");
        let wrong = runner(checkout.path(), rules(slots.path()))
            .run(&words(&["image"]), Some("tag"), None)
            .expect_err("it does not wait there");
        let pressed = runner(checkout.path(), rules(slots.path())).run(&words(&["image"]), Some("push"), None);
        let ended = newest(checkout.path(), "image").expect("the same run");

        assert_eq!(first, Ok("image waits for a person".to_owned()));
        assert_eq!((waiting.outcome, waiting.waiting.as_deref()), (None, Some("push")));
        assert!(wrong.contains("does not wait at a step `tag`"), "{wrong}");
        assert_eq!(pressed, Ok("image passed".to_owned()));
        assert_eq!(ended.started_ms, waiting.started_ms);
        assert_eq!(
            ended.done.iter().map(|step| step.name.as_str()).collect::<Vec<_>>(),
            ["build", "push", "tag"]
        );
        assert_eq!((ended.outcome, ended.waiting), (Some(Outcome::Passed), None));
    }

    #[cfg(unix)]
    #[test]
    fn commands_no_one_allowed_are_refused_until_a_person_allows_them() {
        let checkout = checkout_with("[[build]]\nid = \"app\"\nstep = [{ id = \"make\", run = \"echo made\" }]\n");
        let slots = tempfile::tempdir().expect("slots");

        let refused = runner(checkout.path(), rules(slots.path()))
            .run(&[], None, None)
            .expect_err("not allowed yet");
        let nothing = newest(checkout.path(), "app");
        let allowed = approve(checkout.path(), &[]).expect("allowed");
        let ran = runner(checkout.path(), rules(slots.path())).run(&[], None, None);

        assert!(refused.contains("  echo made") && refused.contains("zo build approve"), "{refused}");
        assert!(nothing.is_none(), "nothing begun before the yes");
        assert_eq!(allowed, "allowed:\n  echo made");
        assert_eq!(ran, Ok("app passed".to_owned()));
        assert_eq!(approve(checkout.path(), &[]).expect("again"), "every command here was allowed already");
    }

    #[cfg(unix)]
    #[test]
    fn a_step_past_its_cap_is_ended_and_its_build_fails() {
        let checkout = checkout_with(
            "[[build]]\nid = \"slow\"\nstep = [{ id = \"wait\", run = \"sleep 30\", cap = \"1s\" }]\n",
        );
        let slots = tempfile::tempdir().expect("slots");
        approve(checkout.path(), &[]).expect("allowed");
        let began = Instant::now();

        let said = runner(checkout.path(), rules(slots.path())).run(&[], None, None);

        let slow = newest(checkout.path(), "slow").expect("a run");
        assert_eq!(said, Err("slow failed".to_owned()));
        assert!(began.elapsed() < Duration::from_secs(10), "ended at its cap");
        assert_eq!(
            (slow.outcome, slow.reason.as_str()),
            (Some(Outcome::Failed), "`wait` ran past its cap of 1s")
        );
        assert_ne!(slow.done[0].rc, 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_quiet_step_is_said_stalled_then_ended_by_the_rule() {
        let checkout = checkout_with("[[build]]\nid = \"quiet\"\nstep = [{ id = \"wait\", run = \"sleep 30\" }]\n");
        let slots = tempfile::tempdir().expect("slots");
        approve(checkout.path(), &[]).expect("allowed");
        let shared = shared_of(checkout.path()).expect("a repository");
        let mut past = Run {
            started_ms: 1,
            ..Run::default()
        };
        past.end_step("wait", 0, Some(1), 1);
        past.end(Outcome::Passed, String::new());
        builds::begin(&shared, "quiet", &past).expect("a passing run");
        let rules = Rules {
            quiet: QuietRule {
                say_factor: 1,
                end_factor: 3,
                say_floor_ms: 0,
                end_floor_ms: 0,
            },
            ..rules(slots.path())
        };
        let at = checkout.path().to_path_buf();

        let running = std::thread::spawn(move || runner(&at, rules).run(&[], None, None));
        wait_for("the stalled step to be said", || {
            newest(checkout.path(), "quiet").is_some_and(|run| run.stalled.is_some())
        });
        let said = newest(checkout.path(), "quiet").and_then(|run| run.stalled).expect("said");
        let ended = running.join().expect("the runner");

        assert_eq!((said.step.as_str(), said.what.as_str(), said.cap_secs), ("wait", "sleep 30", 3));
        assert_eq!(ended, Err("quiet failed".to_owned()));
        let quiet = newest(checkout.path(), "quiet").expect("a run");
        assert!(quiet.reason.starts_with("`wait` wrote nothing for"), "{}", quiet.reason);
        assert_eq!(quiet.stalled, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_stop_ends_the_running_step_and_cancels_what_waits_its_turn() {
        let checkout = checkout_with(
            "[[build]]\nid = \"long\"\nstep = [{ id = \"wait\", run = \"sleep 30\" }]\n\n\
             [[build]]\nid = \"next\"\nstep = [{ id = \"go\", run = \"true\" }]\n",
        );
        let slots = tempfile::tempdir().expect("slots");
        approve(checkout.path(), &[]).expect("allowed");
        let at = checkout.path().to_path_buf();
        let rules = rules(slots.path());
        let began = Instant::now();

        let running = std::thread::spawn(move || runner(&at, rules).run(&[], None, None));
        wait_for("the step to run", || {
            newest(checkout.path(), "long").is_some_and(|run| run.running.is_some())
        });
        let asked = stop(checkout.path(), &[]).expect("asked");
        let ended = running.join().expect("the runner");

        assert_eq!(asked, r#"{"ended":[],"runner_asked":true}"#);
        assert_eq!(ended, Err("long stopped, next stopped".to_owned()));
        assert!(began.elapsed() < Duration::from_secs(10), "stopped, not waited out");
        for id in ["long", "next"] {
            assert_eq!(newest(checkout.path(), id).and_then(|run| run.outcome), Some(Outcome::Cancelled));
        }
    }

    #[cfg(unix)]
    #[test]
    fn one_runner_a_project_and_a_stop_with_none_ends_what_was_left_running() {
        let checkout = checkout_with("[[build]]\nid = \"app\"\nstep = [{ id = \"make\", run = \"true\" }]\n");
        let slots = tempfile::tempdir().expect("slots");
        approve(checkout.path(), &[]).expect("allowed");
        let shared = shared_of(checkout.path()).expect("a repository");

        let held = build_run::hold(&shared).expect("held").expect("free");
        let refused = runner(checkout.path(), rules(slots.path()))
            .run(&[], None, None)
            .expect_err("one runner a project");
        drop(held);
        // A child another test spawns keeps the lock file open until it execs,
        // so the hold can outlive its holder for a moment.
        wait_for("the hold to be freed", || !build_run::held(&shared));
        builds::begin(&shared, "app", &Run { started_ms: 5, ..Run::default() }).expect("a run left unended");
        let asked = stop(checkout.path(), &[]).expect("ended here");

        assert!(refused.contains("already running"), "{refused}");
        assert_eq!(asked, r#"{"ended":["app"],"runner_asked":false}"#);
        assert_eq!(newest(checkout.path(), "app").and_then(|run| run.outcome), Some(Outcome::Cancelled));
    }

    #[cfg(unix)]
    #[test]
    fn the_machine_runs_no_more_builds_at_once_than_its_slots() {
        let checkout = checkout_with("[[build]]\nid = \"app\"\nstep = [{ id = \"make\", run = \"true\" }]\n");
        let slots = tempfile::tempdir().expect("slots");
        approve(checkout.path(), &[]).expect("allowed");
        let taken = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(slots.path().join("0.lock"))
            .expect("the one slot");
        taken.try_lock().expect("taken by another build");
        let rules = Rules {
            at_once: 1,
            ..rules(slots.path())
        };
        let at = checkout.path().to_path_buf();

        let running = std::thread::spawn(move || runner(&at, rules).run(&[], None, None));
        wait_for("the run to be begun", || newest(checkout.path(), "app").is_some());
        std::thread::sleep(Duration::from_millis(200));
        let waiting = newest(checkout.path(), "app").expect("a run");
        drop(taken);
        let ended = running.join().expect("the runner");

        assert_eq!((waiting.running, waiting.done.len()), (None, 0), "it waits its turn");
        assert_eq!(ended, Ok("app passed".to_owned()));
    }

    #[cfg(unix)]
    #[test]
    fn a_step_of_a_read_tool_says_how_far_it_is_and_its_units_make_the_next_whole() {
        let checkout = checkout_with(
            r#"
[[build]]
id = "app"
tool = "gradle"
step = [{ id = "build", run = "printf '> Task :a\\n> Task :b\\n'; sleep 1; printf '> Task :c\\n'" }]
"#,
        );
        let slots = tempfile::tempdir().expect("slots");
        approve(checkout.path(), &[]).expect("allowed");

        let first = runner(checkout.path(), rules(slots.path())).run(&[], None, None);
        let counted = newest(checkout.path(), "app").expect("a run");
        let at = checkout.path().to_path_buf();
        let rules = rules(slots.path());
        let running = std::thread::spawn(move || runner(&at, rules).run(&[], None, None));
        let halfway = Progress {
            done: 2,
            of: Some(3),
        };
        wait_for("the step to say how far it is", || {
            newest(checkout.path(), "app").is_some_and(|run| run.progress == Some(halfway))
        });
        let second = running.join().expect("the runner");
        let ended = newest(checkout.path(), "app").expect("a run");

        assert_eq!(first, Ok("app passed".to_owned()));
        assert_eq!(
            (counted.done[0].units, counted.progress),
            (Some(3), None),
            "a first run has no whole to say, and leaves its count for the next"
        );
        assert_eq!(second, Ok("app passed".to_owned()));
        assert_eq!((ended.done[0].units, ended.progress), (Some(3), None));
    }

    #[cfg(unix)]
    #[test]
    fn what_cannot_be_asked_is_refused() {
        for (args, said) in [
            (&["--step", "push"][..], "one --build"),
            (&["--build", "a", "--build", "b", "--step", "push"][..], "one --build"),
            (&["stop", "--step", "push"][..], "unknown argument `--step`"),
            (&["--bogus"][..], "unknown argument `--bogus`"),
            (&["deploy"][..], "unknown `zo build` verb `deploy`"),
            (&["--build"][..], "--build requires a value"),
        ] {
            let refused = parse(&words(args)).expect_err("refused");
            assert!(refused.contains(said), "{args:?}: {refused}");
        }
        assert_eq!(
            parse(&words(&["--build", "image", "--step", "push", "--label", "v2"])),
            Ok(Verb::Run {
                builds: words(&["image"]),
                step: Some("push".to_owned()),
                label: Some("v2".to_owned()),
            })
        );
        assert_eq!(parse(&[]), Ok(Verb::Run { builds: Vec::new(), step: None, label: None }));
    }

    // Windows has no process group: the runner ends a stopped step's `cmd` with
    // `taskkill /T`, so what that `cmd` started ends with it. Here the step's
    // `cmd` starts a second one, which writes a beat a second for a minute.
    #[cfg(windows)]
    #[test]
    fn a_stop_ends_what_the_step_started_too() {
        let checkout = checkout_with("[[build]]\nid = \"long\"\nstep = [{ id = \"beat\", run = \"cmd /C beat.cmd\" }]\n");
        let beat = "@echo off\r\nfor /L %%n in (1,1,60) do (\r\n  echo beat>>beat.txt\r\n  ping -n 2 127.0.0.1 >nul\r\n)\r\n";
        std::fs::write(checkout.path().join("beat.cmd"), beat).expect("the second cmd's script");
        let slots = tempfile::tempdir().expect("slots");
        approve(checkout.path(), &[]).expect("allowed");
        let beats = checkout.path().join("beat.txt");
        let heard = || std::fs::metadata(&beats).map_or(0, |meta| meta.len());
        let at = checkout.path().to_path_buf();
        let rules = rules(slots.path());

        let running = std::thread::spawn(move || runner(&at, rules).run(&[], None, None));
        wait_for("the second cmd to beat", || heard() > 0);
        let first = heard();
        wait_for("its next beat", || heard() > first);
        let asked = stop(checkout.path(), &[]).expect("asked");
        let ended = running.join().expect("the runner");
        // A beat already on its way when the tree was ended may still land.
        std::thread::sleep(Duration::from_secs(2));
        let after_stop = heard();
        std::thread::sleep(Duration::from_secs(3));

        assert_eq!(asked, r#"{"ended":[],"runner_asked":true}"#);
        assert_eq!(ended, Err("long stopped".to_owned()));
        assert_eq!(heard(), after_stop, "the cmd the step started beats on after the stop");
        assert_eq!(newest(checkout.path(), "long").and_then(|run| run.outcome), Some(Outcome::Cancelled));
    }
}
