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

use zerocode_core::line::{JobState, Line, DISK_FLOOR_DEFAULT_GIB};

/// How long a test waits for the line before it says what did not happen.
const PATIENCE: Duration = Duration::from_secs(60);
/// The pane the fake sees in the environment of the process that put the job in.
const PANE: &str = "term-77";
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

    /// `zo line add --notify <address> -- sh -c <script>` from a pane: the job's id.
    fn add(&self, notify: &str, script: &str, seen: &str, refuse: bool) -> String {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.root.path().join("bin")];
        paths.extend(std::env::split_paths(&path));
        let mut command = Command::new(env!("CARGO_BIN_EXE_zo"));
        command
            .env_clear()
            .env("PATH", std::env::join_paths(paths).expect("a PATH"))
            .env("HOME", self.root.path().join("user"))
            .env("ZO_HOME", self.root.path().join("zo"))
            .env("ZEROCODE_PANE_KEY", PANE)
            .env("ORC_SEEN", self.seen(seen))
            .args(["line", "add", "--owner", seen, "--cap", "90s", "--notify", notify, "--", "sh", "-c", script])
            .current_dir(self.root.path().join("work"))
            .stdin(Stdio::null());
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
