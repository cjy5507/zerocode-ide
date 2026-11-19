//! `zo line add --notify <address>`: when the job ends, the line tells the
//! ledger through `zerocode-orc send`, run with the environment of the process
//! that put the job in. `send` changes the ledger, so the window takes it only
//! with `--retry-request` — before t-118403 the line named none, the window
//! refused every end it was told, and the line threw the refusal away: workers
//! waited for letters that never came (t-116447, 2026-11-19). A fake
//! `zerocode-orc` first on PATH writes how it was called; nothing reaches the
//! real ledger. The real binary, on a line of its own in a temporary zo home.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use zerocode_core::line::{JobState, Line, DISK_FLOOR_DEFAULT_GIB};

/// How long a test waits for the line before it says what did not happen.
const PATIENCE: Duration = Duration::from_secs(60);
/// The pane the fake sees in the environment of the process that put the job in.
const PANE: &str = "term-77";
/// The pane of a process that only looks at the line (`zo line wait`).
const LOOKER: &str = "term-88";
/// A job's command that holds its runner until the file `free` is there, a
/// minute and a half at most: a job whose runner is killed is left running it.
const HOLD: &str = "i=0; while [ ! -e free ] && [ $i -lt 900 ]; do sleep 0.1; i=$((i+1)); done";
/// What the window says to a `send` that names no retry (`orchestration::needs_a_retry_name`), word
/// for word as the real `zerocode-orc` printed it to the line's call on 2026-11-20.
const REFUSAL: &str = "orchestration: send changes the ledger, so it needs --retry-request <id> — a name you can repeat, unchanged, if you never hear how it went. Reads do not need one.";
/// A fake `zerocode-orc`: it writes its arguments, one a line, and the pane it was
/// called from to `$ORC_SEEN`; it refuses as the window does when `$ORC_REFUSE` says so.
const FAKE_ORC: &str = r#"#!/bin/sh
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
    /// A line of its own with the fake first on PATH — or none, said on stderr,
    /// when this disk is below the line's floor, where nothing starts.
    fn new() -> Option<Self> {
        let root = tempfile::tempdir().expect("a folder");
        for folder in ["zo", "work", "user", "bin"] {
            fs::create_dir_all(root.path().join(folder)).expect("folders");
        }
        let stat = nix::sys::statvfs::statvfs(root.path()).expect("the disk's free space");
        let free = u128::from(stat.blocks_available()).saturating_mul(u128::from(stat.fragment_size()));
        if free < u128::from(DISK_FLOOR_DEFAULT_GIB + 1) << 30 {
            eprintln!("skipped: {free} bytes free, below the line's floor of {DISK_FLOOR_DEFAULT_GIB} GiB — nothing would start");
            return None;
        }
        let fake = root.path().join("bin/zerocode-orc");
        fs::write(&fake, FAKE_ORC).expect("the fake");
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("runnable");
        let line = Line::at(&root.path().join("zo"));
        Some(Self { root, line })
    }

    /// `zo` from the pane `pane`, on this line, with the fake first on PATH writing to `seen`.
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
            .env("ZEROCODE_PANE_KEY", pane)
            .env("ORC_SEEN", self.seen(seen))
            .current_dir(self.root.path().join("work"))
            .stdin(Stdio::null());
        command
    }

    /// `zo line add --notify <address> -- sh -c <script>` from a pane: the job's id.
    fn add(&self, notify: &str, script: &str, seen: &str, refuse: bool) -> String {
        let mut command = self.zo(PANE, seen);
        command.args(["line", "add", "--owner", seen, "--cap", "90s", "--notify", notify, "--", "sh", "-c", script]);
        if refuse {
            command.env("ORC_REFUSE", REFUSAL);
        }
        let output = command.output().expect("zo line add");
        assert!(output.status.success(), "zo line add: {}", String::from_utf8_lossy(&output.stderr));
        let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).expect("the receipt");
        receipt["job"].as_str().expect("the job's id").to_owned()
    }

    fn seen(&self, name: &str) -> PathBuf {
        self.root.path().join(format!("seen-{name}"))
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
    let Some(fixture) = Fixture::new() else { return };
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
    let Some(fixture) = Fixture::new() else { return };
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

/// The line tells how a job it ended came to its end, too (t-118403, 11-20):
/// a running job whose runner was gone, and a waiting job whose waiter was
/// gone before its turn — both lost, told once, by the look that ended them,
/// from that look's pane. Before, a job the line ended told no one.
#[test]
fn a_job_the_line_ends_tells_its_address_too() {
    let Some(fixture) = Fixture::new() else { return };
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
    let paused = fixture.zo(PANE, "pause").args(["line", "pause"]).output().expect("zo line pause");
    assert!(paused.status.success(), "zo line pause: {}", String::from_utf8_lossy(&paused.stderr));
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
