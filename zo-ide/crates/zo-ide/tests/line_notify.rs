//! `zo line add --notify <address>`: when the job ends, the line tells the
//! ledger through `zerocode-orc send`, run with the environment of the process
//! that put the job in, in the run that process's pane stood in (`--run`,
//! t-119309). `send` changes the ledger, so the window takes it only
//! with `--retry-request` — before t-118403 the line named none, the window
//! refused every end it was told, and the line threw the refusal away: workers
//! waited for letters that never came (t-116447, 2026-11-19). A fake
//! `zerocode-orc` first on PATH writes how it was called; nothing reaches the
//! real ledger. The real binary, on a line of its own in a temporary zo home,
//! with no disk floor (`FLOOR_ENV`): the tests run whatever this disk holds.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use zerocode_core::line::{JobState, Line};
use zerocode_core::shell_rule::kill::WINDOW_PID_ENV;

/// The variable that names the line's disk floor in GiB, `0` for none: below
/// 17 GiB free these tests said `skipped:` and passed in no time (t-119309).
const FLOOR_ENV: &str = "ZO_LINE_DISK_FLOOR_GIB";
/// How long a test waits for the line before it says what did not happen.
const PATIENCE: Duration = Duration::from_secs(60);
/// The pane the fake sees in the environment of the process that put the job in.
const PANE: &str = "term-77";
/// The run `PANE` stands in, as the fake's `run-current` answers it there.
const PUT_RUN: &str = "run-put";
/// The pane of a process that only looks at the line (`zo line wait`).
const LOOKER: &str = "term-88";
/// The run `LOOKER` stands in: another run than the one the job was put in from.
const LOOK_RUN: &str = "run-other";
/// The window's board, whose buttons run `zo line` from no pane and in no run.
const BOARD: &str = "";
/// A job's command that holds its runner until the file `free` is there, a
/// minute and a half at most: a job whose runner is killed is left running it.
const HOLD: &str = "i=0; while [ ! -e free ] && [ $i -lt 900 ]; do sleep 0.1; i=$((i+1)); done";
/// What the window says to a `send` that names no retry (`orchestration::needs_a_retry_name`), word
/// for word as the real `zerocode-orc` printed it to the line's call on 2026-11-20.
const REFUSAL: &str = "orchestration: send changes the ledger, so it needs --retry-request <id> — a name you can repeat, unchanged, if you never hear how it went. Reads do not need one.";
/// A fake `zerocode-orc`. With `$ORC_BIND` set to `gone` it answers nothing at all, as when no window is
/// there. Asked `line-notify` (t-131644), it writes its arguments one a line and the pane it was called
/// from to `$ORC_SEEN.bound`, then answers as a window too old to know the verb (`$ORC_BIND` is
/// `unknown`), does not answer for half a minute (`silent`), or binds — a second later when `$ORC_BIND`
/// is `slow`: the job, the address and the run its `--run` names, else `$ORC_RUN`, refusing as the
/// window does when there is none, and writing the second it answered in to `$ORC_SEEN.answered`.
/// Asked `run-current`, it answers the run `$ORC_RUN` names, or that the pane stands in none, as the
/// window does, and writes nothing. Any other verb it writes, its arguments one a line and the pane it
/// was called from, to `$ORC_SEEN`; it refuses as the window does when `$ORC_REFUSE` says so.
const FAKE_ORC: &str = r#"#!/bin/sh
if [ "${ORC_BIND:-}" = gone ]; then printf '%s\n' 'zerocode-orc: no window answers here' >&2; exit 1; fi
if [ "${1:-}" = line-notify ]; then
  { for arg in "$@"; do printf '%s\n' "$arg"; done; printf 'pane=%s\n' "${ZEROCODE_PANE_KEY:-}"; } > "$ORC_SEEN.bound"
  case "${ORC_BIND:-}" in
    unknown) printf '%s\n' 'orchestration: unknown verb: line-notify — try `help`' >&2; exit 1 ;;
    silent) exec sleep 30 ;;
    slow) sleep 1 ;;
  esac
  job= to= run="${ORC_RUN:-}"
  while [ $# -gt 0 ]; do
    case "$1" in --job) job=$2; shift ;; --to) to=$2; shift ;; --run) run=$2; shift ;; esac
    shift
  done
  if [ -z "$run" ]; then printf '%s\n' 'orchestration: no run in use — run-create or run-use first' >&2; exit 1; fi
  date +%s > "$ORC_SEEN.answered"
  printf '{"job":"%s","to":"%s","run":"%s","project":"/p/.git"}\n' "$job" "$to" "$run"
  exit 0
fi
if [ "${1:-}" = run-current ]; then
  if [ -n "${ORC_RUN:-}" ]; then printf '{"runId":"%s","name":"fake"}\n' "$ORC_RUN"; exit 0; fi
  printf 'orchestration: no run in use — run-create or run-use first\n' >&2; exit 1
fi
{ for arg in "$@"; do printf '%s\n' "$arg"; done; printf 'pane=%s\n' "${ZEROCODE_PANE_KEY:-}"; } > "$ORC_SEEN.part"
mv "$ORC_SEEN.part" "$ORC_SEEN"
if [ -n "${ORC_REFUSE:-}" ]; then printf '%s\n' "$ORC_REFUSE" >&2; exit 1; fi
printf '{"messageId":"m-1"}\n'
"#;

struct Fixture {
    root: tempfile::TempDir,
    line: Line,
}

impl Fixture {
    /// A line of its own with the fake first on PATH.
    fn new() -> Self {
        let root = tempfile::tempdir().expect("a folder");
        for folder in ["zo", "work", "user", "bin"] {
            fs::create_dir_all(root.path().join(folder)).expect("folders");
        }
        let fake = root.path().join("bin/zerocode-orc");
        fs::write(&fake, FAKE_ORC).expect("the fake");
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("runnable");
        let line = Line::at(&root.path().join("zo"));
        Self { root, line }
    }

    /// `zo` from the pane `pane` (the board's: none), standing in that pane's run, on this line with no
    /// disk floor, with the fake first on PATH writing to `seen`.
    fn zo(&self, pane: &str, seen: &str) -> Command {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.root.path().join("bin")];
        paths.extend(std::env::split_paths(&path));
        let mut command = Command::new(env!("CARGO_BIN_EXE_zo"));
        command
            .env_clear()
            .env("PATH", std::env::join_paths(paths).expect("a PATH"))
            .env("HOME", self.root.path().join("user"))
            .env("ZO_HOME", self.root.path().join("zo"))
            .env(FLOOR_ENV, "0")
            .env("ORC_SEEN", self.seen(seen))
            .current_dir(self.root.path().join("work"))
            .stdin(Stdio::null());
        if !pane.is_empty() {
            command.env("ZEROCODE_PANE_KEY", pane);
        }
        let run = match pane {
            PANE => Some(PUT_RUN),
            LOOKER => Some(LOOK_RUN),
            _ => None,
        };
        if let Some(run) = run {
            command.env("ORC_RUN", run);
        }
        command
    }

    /// `zo line pause` from the pane that puts jobs in: what is put in after it waits.
    fn pause(&self) {
        let paused = self.zo(PANE, "pause").args(["line", "pause"]).output().expect("zo line pause");
        assert!(paused.status.success(), "zo line pause: {}", String::from_utf8_lossy(&paused.stderr));
    }

    /// Job `id`'s record as zo wrote it: the fields a newer zo writes too.
    fn record(&self, id: &str) -> serde_json::Value {
        let text = fs::read_to_string(self.line.job_path(id)).expect("its record");
        serde_json::from_str(&text).expect("a record")
    }

    /// `zo line add --notify <address> -- sh -c <script>` from a pane: the job's id.
    fn add(&self, notify: &str, script: &str, seen: &str, refuse: bool) -> String {
        self.add_under(None, notify, script, seen, refuse)
    }

    /// [`Self::add`] from a pane of the window whose pid is `window` (`WINDOW_PID_ENV`), or of a
    /// window the environment does not name.
    fn add_under(&self, window: Option<u32>, notify: &str, script: &str, seen: &str, refuse: bool) -> String {
        let mut command = self.zo(PANE, seen);
        command.args(["line", "add", "--owner", seen, "--cap", "90s", "--notify", notify, "--", "sh", "-c", script]);
        if refuse {
            command.env("ORC_REFUSE", REFUSAL);
        }
        if let Some(window) = window {
            command.env(WINDOW_PID_ENV, window.to_string());
        }
        let output = command.output().expect("zo line add");
        assert!(output.status.success(), "zo line add: {}", String::from_utf8_lossy(&output.stderr));
        let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).expect("the receipt");
        receipt["job"].as_str().expect("the job's id").to_owned()
    }

    fn seen(&self, name: &str) -> PathBuf {
        self.root.path().join(format!("seen-{name}"))
    }

    /// The words the fake was asked to bind a job's end with (`line-notify`), as `zo line add` asked
    /// it before it answered.
    fn bound(&self, seen: &str) -> Vec<String> {
        let path = self.root.path().join(format!("seen-{seen}.bound"));
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("zo line add asked the window to bind nothing ({})", path.display()));
        text.lines().map(str::to_owned).collect()
    }

    /// The second, since the epoch, the fake answered a binding in.
    fn answered(&self, seen: &str) -> i64 {
        let path = self.root.path().join(format!("seen-{seen}.answered"));
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("the window bound no end ({})", path.display()));
        text.trim().parse().unwrap_or_else(|_| panic!("a second, not {text:?}"))
    }

    /// `zo line wait <job>` from another pane: a look that may end the job, and
    /// tell its end through the fake, writing to `seen`.
    fn watch(&self, id: &str, seen: &str) -> std::process::Child {
        self.zo(LOOKER, seen)
            .args(["line", "wait", id])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("zo line wait")
    }

    /// Kills the process job `id`'s record names — its waiter, or its runner —
    /// after reading its command: only a `zo line wait` of this line's job.
    fn kill_named(&self, id: &str) {
        let pid = self.line.job(id).expect("its record").pid.expect("a process named");
        let output = Command::new("ps").args(["-o", "command=", "-p", &pid.to_string()]).output().expect("ps");
        let command = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        assert!(command.ends_with(&format!("/zo line wait {id}")), "{pid} is no waiter of {id}: {command}");
        let pid = Pid::from_raw(i32::try_from(pid).expect("a pid"));
        kill(pid, Signal::SIGKILL).expect("killed");
        let until = Instant::now() + PATIENCE;
        while kill(pid, None).is_ok() {
            assert!(Instant::now() < until, "{pid} still stands");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits until job `id` is in `state`, and says `why` when it is not in time.
    fn until(&self, id: &str, state: JobState, why: &str) {
        let until = Instant::now() + PATIENCE;
        while !self.line.job(id).is_ok_and(|job| job.state == state) {
            assert!(Instant::now() < until, "{id} is not {state:?} in {} s: {why}", PATIENCE.as_secs());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The words the fake was called with, once the job has ended and the line has told.
    fn told(&self, id: &str, seen: &str) -> Vec<String> {
        let until = Instant::now() + PATIENCE;
        loop {
            let ended = self.line.job(id).is_ok_and(|job| job.state.ended());
            if let (true, Ok(text)) = (ended, fs::read_to_string(self.seen(seen))) {
                return text.lines().map(str::to_owned).collect();
            }
            assert!(Instant::now() < until, "{id} ended, and the line called no zerocode-orc in {} s", PATIENCE.as_secs());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// What job `id`'s record says of a tell that did not go (`notify_failed`), once the line has
    /// written it; says what did not happen when it is not there in time.
    fn marked(&self, id: &str) -> serde_json::Value {
        let until = Instant::now() + PATIENCE;
        loop {
            let record = self.record(id);
            if let Some(failed) = record.get("notify_failed") {
                return failed.clone();
            }
            assert!(
                Instant::now() < until,
                "{id}'s record never says its end was not told, in {} s: {record}",
                PATIENCE.as_secs()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Waits until the process job `id`'s record names — the waiter that ran it — has exited:
    /// whatever it was going to write is written.
    fn settled(&self, id: &str) {
        let pid = self.line.job(id).expect("its record").pid.expect("a process named");
        let pid = Pid::from_raw(i32::try_from(pid).expect("a pid"));
        let until = Instant::now() + PATIENCE;
        while kill(pid, None).is_ok() {
            assert!(Instant::now() < until, "{pid}, which ran {id}, still stands");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// The pid of a process that has ended and been reaped: a window that is gone.
fn a_pid_that_ended() -> u32 {
    let mut child = Command::new("true").spawn().expect("`true` starts");
    let pid = child.id();
    child.wait().expect("`true` ends");
    pid
}

/// The value after `flag` in `words`.
fn value<'a>(words: &'a [String], flag: &str) -> Option<&'a str> {
    words.iter().position(|word| word == flag).and_then(|at| words.get(at + 1)).map(String::as_str)
}

/// The line tells the address `--notify` named how the job ended — `send`, a
/// status, the job and its end in the body — under the name a change needs,
/// the same each time it would be asked again: the job's. A run's address is
/// told in that run. The send has the pane of the process that put the job in.
#[test]
fn the_line_tells_how_a_job_ended_under_the_name_a_ledger_change_needs() {
    let fixture = Fixture::new();
    let job = fixture.add("worker:w-7", "exit 3", "worker", false);
    let words = fixture.told(&job, "worker");
    assert_eq!(words.first().map(String::as_str), Some("send"), "{words:?}");
    assert_eq!((value(&words, "--to"), value(&words, "--type")), (Some("worker:w-7"), Some("status")), "{words:?}");
    let body = value(&words, "--body").expect("a body");
    assert!(body.starts_with(&format!("zo line {job}: ")) && body.contains("failed (rc 3,"), "{body}");
    assert!(
        value(&words, "--retry-request").is_some_and(|name| name.contains(&job)),
        "the window refuses a send that names no --retry-request (`{REFUSAL}`), and the line named none: {words:?}"
    );
    assert!(words.contains(&format!("pane={PANE}")), "the send has the pane that put the job in: {words:?}");

    let job = fixture.add("run:run-9", "exit 0", "run", false);
    let words = fixture.told(&job, "run");
    assert_eq!((value(&words, "--to"), value(&words, "--run")), (Some("run:run-9"), Some("run-9")), "{words:?}");
    assert!(value(&words, "--retry-request").is_some_and(|name| name.contains(&job)), "{words:?}");
    assert_eq!(fixture.line.job(&job).expect("its record").state, JobState::Passed);
}

/// A send that does not go — the window refused it, or there is no
/// `zerocode-orc` to run — no longer vanishes: the job's log says, as its last
/// line, whom the line could not tell and what was said.
#[test]
fn a_send_that_does_not_go_says_so_in_the_jobs_log() {
    let fixture = Fixture::new();
    let job = fixture.add("worker:w-7", "echo built", "refused", true);
    fixture.told(&job, "refused");
    let log = fixture.line.log_path(&job);
    let until = Instant::now() + PATIENCE;
    let last = loop {
        let text = fs::read_to_string(&log).unwrap_or_default();
        let last = text.lines().last().unwrap_or_default().to_owned();
        if last.starts_with("zo line: ") {
            break last;
        }
        assert!(Instant::now() < until, "the job's log never says the send did not go: {text:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(last.contains("worker:w-7") && last.contains("did not go") && last.contains(REFUSAL), "{last}");
    let text = fs::read_to_string(&log).expect("its log");
    assert!(text.starts_with("built\n"), "the job's own output first: {text:?}");
}

/// Red → green (t-131644, I-line-notify-1): a tell that does not go is written in the job's
/// record too, not only as its log's last line — when, why, and whether the window that gave the
/// pane that put the job in its seat (`WINDOW_PID_ENV`) had ended by then. A window restart mints
/// every seat anew, and the waiter of a job put in before it tells under a seat the new window
/// refuses: on 2026-11-25 and 26 three ends were told to nobody, and their workers waited. The
/// window reads this mark to tell such an end itself. A send that goes leaves none.
#[test]
fn a_send_that_does_not_go_is_written_in_the_jobs_record() {
    let fixture = Fixture::new();
    let job = fixture.add_under(Some(a_pid_that_ended()), "worker:w-7", "exit 0", "gone", true);
    let failed = fixture.marked(&job);
    assert!(failed["why"].as_str().is_some_and(|why| why.contains(REFUSAL)), "{failed}");
    assert_eq!(failed["window_gone"], true, "the window the putting pane came from had ended: {failed}");
    let ended = fixture.record(&job)["ended_ms"].as_i64().expect("its end");
    assert!(failed["at_ms"].as_i64().is_some_and(|at| at >= ended), "told after it ended: {failed}");

    let job = fixture.add_under(Some(std::process::id()), "worker:w-7", "exit 0", "standing", true);
    assert_eq!(fixture.marked(&job)["window_gone"], false, "the window it names, this test, stands");

    let job = fixture.add("worker:w-7", "exit 0", "unnamed", true);
    assert_eq!(fixture.marked(&job)["window_gone"], false, "no window named, none said gone");

    let job = fixture.add("worker:w-7", "exit 0", "went", false);
    fixture.told(&job, "went");
    fixture.settled(&job);
    let record = fixture.record(&job);
    assert!(record.get("notify_failed").is_none(), "a send that went left a mark: {record}");
}

/// Red → green (t-131644): the line's letter carries the name its job's end is told under as its
/// subject — the one the ledger's letter about the same end carries when the line could not tell
/// it — so the window does not tell an end the line's own letter already told, even one that
/// landed after the line heard it had not.
#[test]
fn the_lines_letter_names_its_jobs_end_as_its_subject() {
    let fixture = Fixture::new();
    let job = fixture.add("worker:w-7", "exit 0", "subject", false);
    let words = fixture.told(&job, "subject");
    let name = format!("zo-line-{job}");
    assert_eq!(value(&words, "--subject"), Some(name.as_str()), "{words:?}");
    assert_eq!(value(&words, "--retry-request"), Some(name.as_str()), "{words:?}");
}

/// Red → green (t-131644, F1 of the review t-131831): `zo line add --notify` binds the job's end to
/// the pane that puts it in, before the job runs — `line-notify`, under the job's own name, from that
/// pane — so the ledger can tell that end itself when a window restart leaves the line no pane to
/// tell it from. A worker is told in the run the window answered for the pane, and a run's chair in
/// that run.
#[test]
fn zo_line_add_binds_its_jobs_end_to_the_pane_that_puts_it_in() {
    let fixture = Fixture::new();
    let job = fixture.add("worker:w-7", "exit 0", "bound", false);
    let name = format!("zo-line-notify-{job}");
    let pane = format!("pane={PANE}");
    assert_eq!(
        fixture.bound("bound"),
        ["line-notify", "--job", job.as_str(), "--to", "worker:w-7", "--retry-request", name.as_str(), pane.as_str()],
        "the bind asked"
    );
    assert_eq!(fixture.record(&job)["notify_run"], PUT_RUN, "the run the window answered: {}", fixture.record(&job));
    fixture.told(&job, "bound");
    fixture.settled(&job);

    let job = fixture.add("run:run-9", "exit 0", "chair", false);
    let words = fixture.bound("chair");
    assert_eq!((value(&words, "--to"), value(&words, "--run")), (Some("run:run-9"), Some("run-9")), "{words:?}");
    assert!(fixture.record(&job).get("notify_run").is_none(), "{}", fixture.record(&job));
    fixture.told(&job, "chair");
    fixture.settled(&job);
}

/// Red → green (t-131644, F1 of the review t-131848): `zo line add` puts its job in after the window
/// bound its end — the job is queued from the moment the binding came back. The ledger tells only an
/// end bound no later than its job was put in: a pane reads a job's id off the line once the job is
/// in it, so a binding filed after that is another pane's. A window that takes a second to bind shows
/// the order: the job was queued in the second the window answered, or later.
#[test]
fn zo_line_add_puts_its_job_in_after_its_end_is_bound() {
    let fixture = Fixture::new();
    let output = fixture
        .zo(PANE, "slow")
        .env("ORC_BIND", "slow")
        .args(["line", "add", "--owner", "slow", "--cap", "90s", "--notify", "worker:w-7", "--", "sh", "-c", "exit 0"])
        .output()
        .expect("zo line add");
    assert!(output.status.success(), "zo line add: {}", String::from_utf8_lossy(&output.stderr));
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).expect("the receipt");
    let job = receipt["job"].as_str().expect("the job's id").to_owned();
    let answered = fixture.answered("slow");
    let queued = fixture.record(&job)["queued_ms"].as_i64().expect("when it was queued");
    assert!(
        queued >= answered * 1_000,
        "{job} was queued at {queued} ms, before the window bound its end in second {answered}"
    );
    fixture.told(&job, "slow");
    fixture.settled(&job);
}

/// Red → green (t-131644): a window that cannot bind a job's end — too old to know `line-notify`, not
/// there at all, or not answering — costs `zo line add` one warning line and a few seconds at most,
/// never the put: the job goes in, and its run is asked as before (`run-current`) of a window that
/// answered at all.
#[test]
fn a_window_that_cannot_bind_costs_a_put_one_warning_line_and_not_the_put() {
    let fixture = Fixture::new();
    for (how, run) in [("unknown", Some(PUT_RUN)), ("gone", None), ("silent", None)] {
        let began = Instant::now();
        let output = fixture
            .zo(PANE, how)
            .env("ORC_BIND", how)
            .args(["line", "add", "--owner", how, "--cap", "90s", "--notify", "worker:w-7", "--", "sh", "-c", "exit 0"])
            .output()
            .expect("zo line add");
        let took = began.elapsed();
        let said = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{how}: zo line add: {said}");
        let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).expect("the receipt");
        let job = receipt["job"].as_str().expect("the job's id").to_owned();
        let warnings: Vec<&str> = said.lines().filter(|line| line.starts_with("zo line: warning")).collect();
        assert_eq!(warnings.len(), 1, "{how}: one warning line, not {}: {said}", warnings.len());
        assert!(warnings[0].contains(&job) && warnings[0].contains("did not bind"), "{how}: {said}");
        assert!(took < Duration::from_secs(20), "{how}: zo line add waited {took:?} on a window that cannot bind");
        let record = fixture.record(&job);
        assert_eq!(record["notify_run"].as_str(), run, "{how}: {record}");
        if how == "gone" {
            fixture.marked(&job);
        } else {
            fixture.told(&job, how);
        }
        fixture.settled(&job);
    }
}

/// t-131644: a retry is a job of its own. `zo line retry` copies the first job's record, and an
/// end the first could not tell is the first's: the retry starts with no such mark, or the window
/// would tell the retry's end once more for it.
#[test]
fn a_retry_carries_no_mark_of_an_end_its_first_could_not_tell() {
    let fixture = Fixture::new();
    let first = fixture.add_under(Some(a_pid_that_ended()), "worker:w-7", "exit 3", "untold", true);
    fixture.marked(&first);
    let output = fixture.zo(BOARD, "again").args(["line", "retry", &first]).output().expect("zo line retry");
    assert!(output.status.success(), "zo line retry: {}", String::from_utf8_lossy(&output.stderr));
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).expect("the receipt");
    let again = receipt["job"].as_str().expect("the job's id").to_owned();
    let record = fixture.record(&again);
    assert!(record.get("notify_failed").is_none(), "the retry carries its first's mark: {record}");
    // Its own end, told from the board, before the line's folder goes.
    fixture.told(&again, "again");
    fixture.settled(&again);
}

/// The line tells how a job it ended came to its end, too (t-118403, 11-20):
/// a running job whose runner was gone, and a waiting job whose waiter was
/// gone before its turn — both lost, told once, by the look that ended them,
/// from that look's pane. Before, a job the line ended told no one.
#[test]
fn a_job_the_line_ends_tells_its_address_too() {
    let fixture = Fixture::new();
    let running = fixture.add("worker:w-7", HOLD, "running", false);
    fixture.until(&running, JobState::Running, "its turn came");
    // The only job on the line: once its runner is gone, this wait's look is the one that ends it.
    fixture.kill_named(&running);
    let mut looks = fixture.watch(&running, "lost-running");
    let words = fixture.told(&running, "lost-running");
    let body = value(&words, "--body").expect("a body");
    assert!(body.starts_with(&format!("zo line {running}: ")) && body.contains("— lost"), "{body}");
    assert!(value(&words, "--retry-request").is_some_and(|name| name.contains(&running)), "{words:?}");
    assert!(words.contains(&format!("pane={LOOKER}")), "told from the pane that looked: {words:?}");
    let _ = looks.wait();

    // Put in only now, on a paused line: before, its waiter's own looks could end the running job first
    // and tell it in its stead (t-119176).
    fixture.pause();
    let waiting = fixture.add("worker:w-7", "exit 0", "waiting", false);
    fixture.kill_named(&waiting);
    let mut looks = fixture.watch(&waiting, "lost-waiting");
    fixture.until(&waiting, JobState::Lost, "its waiter was gone before its turn");
    let words = fixture.told(&waiting, "lost-waiting");
    let body = value(&words, "--body").expect("a body");
    assert!(body.starts_with(&format!("zo line {waiting}: ")) && body.contains("— lost"), "{body}");
    let _ = looks.wait();
    fs::write(fixture.root.path().join("work/free"), "").expect("the running job's command let go");
}

/// Red → green (t-119309, I-t118403-8): a job the line ends is told in the run
/// the pane that put it in stood in, whoever looked. `zerocode-orc send` finds
/// a `worker:` or `pane:` address in the run its caller stands in, so the look
/// of a pane of another run told a worker it could not find — the ledger said
/// 「unknown worker」 and the job's log kept it. The pane that puts a job in is
/// asked its run once (`run-current`), the record keeps it (`notify_run`), and
/// every tell names it with `--run`. The looking pane signs as itself.
#[test]
fn a_job_a_look_from_another_run_ended_is_told_in_the_run_it_was_put_in_from() {
    let fixture = Fixture::new();
    fixture.pause();
    let job = fixture.add("worker:w-7", "exit 0", "put", false);
    fixture.kill_named(&job);
    let mut looks = fixture.watch(&job, "looked");
    fixture.until(&job, JobState::Lost, "its waiter was gone before its turn");
    let words = fixture.told(&job, "looked");
    assert!(words.contains(&format!("pane={LOOKER}")), "told from the pane that looked: {words:?}");
    assert_eq!(
        value(&words, "--run"),
        Some(PUT_RUN),
        "told in the run of the pane that put the job in, not in the looker's {LOOK_RUN}: {words:?}"
    );
    assert_eq!(value(&words, "--to"), Some("worker:w-7"), "{words:?}");
    assert_eq!(fixture.record(&job)["notify_run"], PUT_RUN, "the record keeps the run: {}", fixture.record(&job));
    let _ = looks.wait();
}

/// t-119309: the board's `retry` button runs `zo line retry` from the window,
/// which stands in no run; the job it puts in again keeps the run the first
/// was put in from, and its end is told there — the address is that run's.
#[test]
fn a_retry_from_the_board_keeps_the_run_its_job_was_put_in_from() {
    let fixture = Fixture::new();
    let first = fixture.add("worker:w-7", "exit 3", "first", false);
    let words = fixture.told(&first, "first");
    assert_eq!(value(&words, "--run"), Some(PUT_RUN), "the waiter tells in the run the job was put in from: {words:?}");
    let output = fixture.zo(BOARD, "again").args(["line", "retry", &first]).output().expect("zo line retry");
    assert!(output.status.success(), "zo line retry: {}", String::from_utf8_lossy(&output.stderr));
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).expect("the receipt");
    let again = receipt["job"].as_str().expect("the job's id").to_owned();
    assert_eq!(fixture.record(&again)["notify_run"], PUT_RUN, "the retry kept its run: {}", fixture.record(&again));
    let words = fixture.told(&again, "again");
    assert_eq!(value(&words, "--run"), Some(PUT_RUN), "the retry's end is told in that run too: {words:?}");
    assert!(words.contains(&"pane=".to_owned()), "told by the retry's waiter, from the board's environment: {words:?}");
}
