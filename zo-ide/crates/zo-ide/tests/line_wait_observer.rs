//! `zo line wait <job>` only watches a job it was not started for (t-118403).
//! The waiter `zo line add` started runs the job; a wait a worker's pane starts
//! beside it, to learn how the job ended, never does: a runner that a pane's
//! restart takes with it leaves its job lost (2026-11-19: j-…6b95 at the
//! window's restart at 21:24). The real binary, each test on a line of its own
//! in a temporary zo home (`ZO_HOME`) — never the machine's line.

#![cfg(unix)]

use std::cell::RefCell;
use std::fs;
use std::io::{BufRead as _, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use zerocode_core::line::{Job, JobState, Line, DISK_FLOOR_DEFAULT_GIB};

/// How long a step waits for the line before the test says what did not happen.
const PATIENCE: Duration = Duration::from_secs(60);
/// The cap of every job here: a job a failed test leaves behind ends by it.
const CAP: &str = "90s";
/// Rounds of a race a pane's wait won in some rounds before t-118403.
const ROUNDS: u32 = 10;
/// How often a waiting `zo line wait` looks at the line (`line_cli::LOOK_EVERY`).
const LOOK: Duration = Duration::from_millis(500);
/// Long enough for a wait just started to have looked at the line more than once.
const SETTLE: Duration = Duration::from_millis(1200);
/// A job's command that holds its runner until the file named after it (`$0`) is there.
const HOLD: &str = r#"while [ ! -e "$0" ]; do sleep 0.05; done"#;
/// A job's command that writes who ran it to the file named after it: the runner's
/// pid, its parent's pid and its process group.
const WHO_RAN: &str = r#"echo "$PPID $(ps -o ppid= -p $PPID) $(ps -o pgid= -p $PPID)" >> "$0""#;

/// One line in a temporary zo home, and the folder its jobs run in.
struct Fixture {
    root: tempfile::TempDir,
    line: Line,
    /// Files that end a job's `HOLD`: all made when the test ends, whatever happened.
    gates: RefCell<Vec<PathBuf>>,
}

impl Fixture {
    /// A line of its own — or none, said on stderr, when this disk is below the
    /// line's floor: nothing starts there, by the line's own rule.
    fn new() -> Option<Self> {
        let root = tempfile::tempdir().expect("a folder");
        for folder in ["zo", "work", "user"] {
            fs::create_dir_all(root.path().join(folder)).expect("folders");
        }
        let stat = nix::sys::statvfs::statvfs(root.path()).expect("the disk's free space");
        let free = u128::from(stat.blocks_available()).saturating_mul(u128::from(stat.fragment_size()));
        if free < u128::from(DISK_FLOOR_DEFAULT_GIB + 1) << 30 {
            eprintln!("skipped: {free} bytes free, below the line's floor of {DISK_FLOOR_DEFAULT_GIB} GiB — nothing would start");
            return None;
        }
        let line = Line::at(&root.path().join("zo"));
        Some(Self { root, line, gates: RefCell::new(Vec::new()) })
    }

    fn cwd(&self) -> PathBuf {
        self.root.path().join("work")
    }

    /// `zo` on this line only: zo's home, the person's home and nothing else of this process's environment.
    fn zo(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zo"));
        command.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        command
            .args(args)
            .current_dir(self.cwd())
            .env("HOME", self.root.path().join("user"))
            .env("ZO_HOME", self.root.path().join("zo"))
            .stdin(Stdio::null());
        command
    }

    /// `zo line add --owner <owner> -- sh -c <script> <arg>`: the job's id.
    fn add(&self, owner: &str, script: &str, arg: &str) -> String {
        let output = self
            .zo(&["line", "add", "--owner", owner, "--cap", CAP, "--", "sh", "-c", script, arg])
            .output()
            .expect("zo line add");
        assert!(output.status.success(), "zo line add: {}", String::from_utf8_lossy(&output.stderr));
        let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).expect("the receipt");
        receipt["job"].as_str().expect("the job's id").to_owned()
    }

    /// `zo line <words>`, which must say it did what it was asked.
    fn act(&self, words: &[&str]) {
        let mut args = vec!["line"];
        args.extend_from_slice(words);
        let output = self.zo(&args).output().expect("zo line");
        assert!(output.status.success(), "zo line {words:?}: {}", String::from_utf8_lossy(&output.stderr));
    }

    /// `zo line wait <job>`, started here: a pane's wait for the job's end.
    fn wait(&self, id: &str) -> Child {
        self.zo(&["line", "wait", id]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("zo line wait")
    }

    fn job(&self, id: &str) -> Job {
        self.line.job(id).expect("the job's record")
    }

    fn runs(&self, id: &str) {
        until(&format!("{id} runs"), || self.job(id).state == JobState::Running);
    }

    /// A file that ends a `HOLD` job, made when the test ends at the latest.
    fn gate(&self, name: &str) -> String {
        self.gates.borrow_mut().push(self.cwd().join(name));
        name.to_owned()
    }

    fn open(&self, gate: &str) {
        fs::write(self.cwd().join(gate), "").expect("the gate");
    }

    /// Who ran a `WHO_RAN` job, a line each run: runner, its parent, its group.
    fn who_ran(&self, file: &str) -> Vec<[u32; 3]> {
        let text = fs::read_to_string(self.cwd().join(file)).unwrap_or_default();
        text.lines()
            .map(|line| {
                let words: Vec<u32> = line.split_whitespace().map(|word| word.parse().expect("a number")).collect();
                words.try_into().unwrap_or_else(|words| panic!("three numbers, not {words:?} in {text:?}"))
            })
            .collect()
    }
}

impl Drop for Fixture {
    /// Every job ends before the folder goes, so no waiter of this line outlives the test.
    fn drop(&mut self) {
        for gate in &*self.gates.borrow() {
            let _ = fs::write(gate, "");
        }
        for job in self.line.jobs() {
            let verb = match job.state {
                JobState::Waiting => "withdraw",
                JobState::Running => "stop",
                _ => continue,
            };
            let _ = self.zo(&["line", verb, &job.id]).output();
        }
        let until = Instant::now() + Duration::from_secs(20);
        while self.line.jobs().iter().any(|job| !job.state.ended()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        // A waiter looks every 500 ms: one more look sees its job ended.
        std::thread::sleep(Duration::from_millis(700));
    }
}

/// The process `zo line add` started for job `id`: the one `zo line wait <id>`
/// of this binary standing before a test starts its own.
fn waiter_of(id: &str) -> u32 {
    let mut found = Vec::new();
    until(&format!("the waiter of {id} stands"), || {
        found = waits_on(id);
        !found.is_empty()
    });
    assert_eq!(found.len(), 1, "one waiter for {id}: {found:?}");
    found[0]
}

/// Kills a waiter of this line: only a `zo line wait <id>` of this binary.
fn kill_waiter(pid: u32, id: &str) {
    assert!(waits_on(id).contains(&pid), "{pid} is no waiter of {id}");
    kill(Pid::from_raw(i32::try_from(pid).expect("a pid")), Signal::SIGKILL).expect("killed");
    until(&format!("waiter {pid} is gone"), || !alive(pid));
}

/// Waits until `done`, at most `PATIENCE`, and says `what` did not happen when it does not.
fn until(what: &str, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < until, "{what}: not in {} s", PATIENCE.as_secs());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The pids of the `zo line wait <id>` processes: the job's id is this test line's own.
fn waits_on(id: &str) -> Vec<u32> {
    let output = Command::new("pgrep").args(["-f", &format!("line wait {id}$")]).output().expect("pgrep");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .filter(|pid| ps(*pid, "command").is_some_and(|command| command.ends_with(&format!("/zo line wait {id}"))))
        .collect()
}

/// One column of `ps` for a process, or none when it is gone.
fn ps(pid: u32, column: &str) -> Option<String> {
    let output = Command::new("ps").args(["-o", &format!("{column}="), "-p", &pid.to_string()]).output().ok()?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !value.is_empty()).then_some(value)
}

fn alive(pid: u32) -> bool {
    kill(Pid::from_raw(i32::try_from(pid).expect("a pid")), None).is_ok()
}

/// A child's exit code, waited for at most `PATIENCE`.
fn exit_of(child: &mut Child) -> i32 {
    let until = Instant::now() + PATIENCE;
    loop {
        if let Some(status) = child.try_wait().expect("its status") {
            return status.code().expect("an exit code");
        }
        if Instant::now() > until {
            let _ = child.kill();
            panic!("process {} did not end in {} s", child.id(), PATIENCE.as_secs());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Red → green 1 (t-118403): a job that waits its turn with the waiter `add`
/// started and a pane's `zo line wait` beside it runs once, in that waiter,
/// round after round; the wait exits with the job's code. As a pane's wait
/// does, the wait starts at another moment of the waiter's look each round,
/// and the turn comes when the paused line is let go.
#[test]
fn a_wait_beside_the_waiter_add_started_never_runs_the_job() {
    let Some(fixture) = Fixture::new() else { return };
    for round in 0..ROUNDS {
        fixture.act(&["pause"]);
        let ran = format!("ran-{round}");
        let job = fixture.add(&format!("o-{round}"), WHO_RAN, &ran);
        let waiter = waiter_of(&job);
        std::thread::sleep(LOOK * round / ROUNDS);
        let mut observer = fixture.wait(&job);
        std::thread::sleep(SETTLE);
        fixture.act(&["resume"]);
        let code = exit_of(&mut observer);
        let runs = fixture.who_ran(&ran);
        assert_eq!(runs.len(), 1, "round {round}: the job ran once: {runs:?}");
        let [runner, ..] = runs[0];
        assert_ne!(runner, observer.id(), "round {round}: 관찰자가 러너가 됨 — the pane's `zo line wait` ran the job");
        assert_eq!(runner, waiter, "round {round}: the waiter `zo line add` started ran the job");
        assert_eq!(fixture.job(&job).pid, Some(runner), "round {round}: the record names its runner");
        assert_eq!(code, 0, "round {round}: the wait exits with the job's code");
    }
}

/// Red → green 3 (t-118403, as the coordinator decided on 11-20): when the
/// waiter `add` started is gone before the job's turn, a pane's wait does not
/// run the job, and no waiter is started in its place — one would run it with
/// the environment of whichever process looked. The look ends the job as lost,
/// the wait exits with its code, and the job's log says how the pane that put
/// it in puts it in again. Before t-118403 the wait ran it as its own child; at
/// 69c371b78 the wait started a new waiter that ran it.
#[test]
fn a_wait_on_a_job_whose_waiter_is_gone_ends_it_and_runs_nothing() {
    let Some(fixture) = Fixture::new() else { return };
    let gate = fixture.gate("go");
    let first = fixture.add("p", HOLD, &gate);
    fixture.runs(&first);
    let job = fixture.add("p", WHO_RAN, "ran");
    kill_waiter(waiter_of(&job), &job);
    let mut observer = fixture.wait(&job);
    std::thread::sleep(SETTLE);
    fixture.open(&gate);
    let code = exit_of(&mut observer);
    until("the first job ends", || fixture.job(&first).state.ended());
    // Long enough for a runner, had the line kept one, to have run the job.
    std::thread::sleep(SETTLE);
    let runs = fixture.who_ran("ran");
    assert!(
        runs.iter().all(|[runner, ..]| *runner != observer.id()),
        "관찰자가 러너가 됨 — the pane's `zo line wait` ran the job: {runs:?}"
    );
    assert!(runs.is_empty(), "the job ran after its waiter was gone, in a waiter some look started for it: {runs:?}");
    assert_eq!((fixture.job(&job).state, code), (JobState::Lost, 1), "the job ended lost, and the wait with its code");
    let log = fs::read_to_string(fixture.line.log_path(&job)).expect("its log");
    assert!(log.contains(&format!("`zo line retry {job}` from the pane that put it in")), "{log}");
}

/// Decision 2 of t-118403, as decided on 11-20: a job whose waiter is gone
/// before its turn, with no wait watching it, stays waiting no more — the next
/// look ends it as lost and its owner's later job runs. Before, it held its
/// owner's later jobs, and the line's slots that chose it, for good.
#[test]
fn a_waiting_job_whose_waiter_is_gone_does_not_hold_the_line() {
    let Some(fixture) = Fixture::new() else { return };
    let gate = fixture.gate("go");
    let first = fixture.add("q", HOLD, &gate);
    fixture.runs(&first);
    let orphan = fixture.add("q", r#"echo orphan >> "$0""#, "ran");
    kill_waiter(waiter_of(&orphan), &orphan);
    fixture.open(&gate);
    until("the first job ends", || fixture.job(&first).state.ended());
    let behind = fixture.add("q", r#"echo behind >> "$0""#, "ran");
    until(&format!("{orphan} has no waiter and stays waiting; {behind} behind it never starts"), || {
        fixture.job(&behind).state.ended()
    });
    let ran = fs::read_to_string(fixture.cwd().join("ran")).expect("what ran");
    assert_eq!(ran, "behind\n", "the job whose waiter was gone did not run; the one behind it did");
    assert_eq!(fixture.job(&orphan).state, JobState::Lost);
}

/// Red → green 2 (t-118403): a pane's wait exits with the job's code however
/// the job ended — passed, failed, withdrawn, stopped, lost — the code its
/// receipt (`<job>.rc`) says when it ran.
#[test]
fn a_wait_exits_with_the_jobs_code_however_the_job_ended() {
    let Some(fixture) = Fixture::new() else { return };
    // Two jobs that run, one in each of the line's two slots: a stop ends one — its command leaves with
    // 0, yet the job did not pass — and the other's runner is killed.
    let stopped = fixture.add("e", "trap 'exit 0' TERM; while :; do sleep 0.05; done", "-");
    let free = fixture.gate("free");
    let lost = fixture.add("f", HOLD, &free);
    fixture.runs(&stopped);
    fixture.runs(&lost);
    let runner = fixture.job(&lost).pid.expect("its runner");
    let mut watched = vec![("stopped", stopped.clone(), 1, fixture.wait(&stopped)), ("lost", lost.clone(), 1, fixture.wait(&lost))];
    std::thread::sleep(SETTLE);
    fixture.act(&["stop", &stopped]);
    kill_waiter(runner, &lost);
    // Three that wait behind their owner's first job: two their command ends, one taken out before it ran.
    let gate = fixture.gate("go");
    let first = fixture.add("c", HOLD, &gate);
    let passes = fixture.add("c", "exit 0", "-");
    let fails = fixture.add("c", "exit 3", "-");
    let withdrawn = fixture.add("c", "exit 0", "-");
    for (how, id, code) in [("passed", &passes, 0), ("failed", &fails, 3), ("withdrawn", &withdrawn, 1)] {
        watched.push((how, id.clone(), code, fixture.wait(id)));
    }
    fixture.runs(&first);
    std::thread::sleep(SETTLE);
    fixture.act(&["withdraw", &withdrawn]);
    fixture.open(&gate);
    for (how, id, expected, wait) in &mut watched {
        let code = exit_of(wait);
        assert_eq!(code, *expected, "{how}: the wait exits with the job's code ({:?})", fixture.job(id.as_str()));
        if let Ok(receipt) = fs::read_to_string(fixture.line.log_path(id.as_str()).with_extension("rc")) {
            assert_eq!(receipt.trim(), code.to_string(), "{how}: the wait's code is the receipt's");
        }
    }
    let states: Vec<JobState> = [&passes, &fails, &withdrawn, &stopped, &lost].iter().map(|id| fixture.job(id).state).collect();
    assert_eq!(states, [JobState::Passed, JobState::Failed, JobState::Withdrawn, JobState::Stopped, JobState::Lost]);
}

/// Red → green 4 (t-118403): `zo line run` still runs its job here — its
/// output on this terminal, its exit the command's — and a pane's wait beside
/// it, started at another moment of its look each round, never takes the job
/// from it when the paused line is let go.
#[test]
fn zo_line_run_still_runs_its_job_in_its_own_terminal() {
    let Some(fixture) = Fixture::new() else { return };
    let alone = fixture
        .zo(&["line", "run", "--owner", "g", "--cap", CAP, "--", "sh", "-c", "echo out; exit 4"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("zo line run");
    let pid = alone.id();
    let output = alone.wait_with_output().expect("its output");
    assert_eq!(output.status.code(), Some(4), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "out\n");
    let job = fixture.job(&id_of(&String::from_utf8_lossy(&output.stderr)));
    assert_eq!((job.state, job.rc, job.pid), (JobState::Failed, Some(4), Some(pid)));

    for round in 0..ROUNDS {
        fixture.act(&["pause"]);
        let owner = format!("h-{round}");
        let ran = format!("ran-{owner}");
        let mut run = fixture
            .zo(&["line", "run", "--owner", &owner, "--cap", CAP, "--", "sh", "-c", WHO_RAN, &ran])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("zo line run");
        let mut said = BufReader::new(run.stderr.take().expect("its stderr"));
        let mut first_line = String::new();
        said.read_line(&mut first_line).expect("its first line");
        std::thread::sleep(LOOK * round / ROUNDS);
        let mut observer = fixture.wait(&id_of(&first_line));
        std::thread::sleep(SETTLE);
        fixture.act(&["resume"]);
        let (code, watched) = (exit_of(&mut run), exit_of(&mut observer));
        drop(said);
        let runs = fixture.who_ran(&ran);
        assert_eq!(runs.len(), 1, "round {round}: the job ran once: {runs:?}");
        assert_ne!(runs[0][0], observer.id(), "round {round}: 관찰자가 러너가 됨 — the pane's `zo line wait` took `zo line run`'s job");
        assert_eq!(runs[0][0], run.id(), "round {round}: `zo line run` ran its own job");
        assert_eq!((code, watched), (0, 0), "round {round}: both exit with the job's code");
    }
}

/// The job `zo line run` names on its first line of stderr: `zo line: <job> · cap … · <log>`.
fn id_of(said: &str) -> String {
    said.strip_prefix("zo line: ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no job named in {said:?}"))
        .to_owned()
}
