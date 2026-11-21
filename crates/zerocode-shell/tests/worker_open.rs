#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use zerocode_core::worker_open;

struct FakeOpen {
    _dir: tempfile::TempDir,
    bin_dir: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedCall {
    guard_env: String,
    args: Vec<String>,
}

impl FakeOpen {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin_dir = dir.path().join("fake-bin");
        fs::create_dir_all(&bin_dir).expect("fake bin dir");
        let bin = bin_dir.join(worker_open::SHIM_NAME);
        let log = dir.path().join("calls.log");

        let script = format!(
            r#"#!/bin/sh
log='{log}'
{{
  printf 'CALL_BEGIN\n'
  printf 'GUARD=%s\n' "${{{marker}:-unset}}"
  for arg in "$@"; do
    printf 'ARG=%s\n' "$arg"
  done
  printf 'CALL_END\n'
}} >> "$log"
if [ -n "${{FAKE_OPEN_STDOUT:-}}" ]; then
  printf '%s' "$FAKE_OPEN_STDOUT"
fi
if [ -n "${{FAKE_OPEN_STDERR:-}}" ]; then
  printf '%s' "$FAKE_OPEN_STDERR" >&2
fi
exit "${{FAKE_OPEN_EXIT:-0}}"
"#,
            log = log.display(),
            marker = worker_open::MARKER,
        );
        fs::write(&bin, script).expect("write fake open");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod fake open");

        Self {
            _dir: dir,
            bin_dir,
            bin,
            log,
        }
    }

    fn calls(&self) -> Vec<RecordedCall> {
        let Ok(text) = fs::read_to_string(&self.log) else {
            return Vec::new();
        };
        let mut calls = Vec::new();
        let mut current: Option<RecordedCall> = None;
        for line in text.lines() {
            if line == "CALL_BEGIN" {
                current = Some(RecordedCall {
                    guard_env: String::new(),
                    args: Vec::new(),
                });
            } else if line == "CALL_END" {
                if let Some(call) = current.take() {
                    calls.push(call);
                }
            } else if let Some(guard) = line.strip_prefix("GUARD=") {
                if let Some(call) = current.as_mut() {
                    call.guard_env = guard.to_string();
                }
            } else if let Some(arg) = line.strip_prefix("ARG=")
                && let Some(call) = current.as_mut()
            {
                call.args.push(arg.to_string());
            }
        }
        calls
    }

    fn mirror_command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zerocode-mirror"));
        cmd.arg(&self.bin)
            .args(args)
            .env_clear()
            .env("PATH", "/bin")
            .env(worker_open::MARKER, "1");
        cmd
    }

    fn run_mirror(&self, args: &[&str]) -> Output {
        self.mirror_command(args)
            .output()
            .expect("run zerocode-mirror")
    }
}

fn mirror_path() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_zerocode-mirror"))
}

fn install_worker_open_shim(dir: &Path, mirror: &Path, real_open: &Path) -> PathBuf {
    let worker_dir = dir.join("worker-open");
    fs::create_dir_all(&worker_dir).expect("worker-open dir");
    let shim = worker_dir.join(worker_open::SHIM_NAME);
    let body = worker_open::open_shim_script(mirror, real_open);
    fs::write(&shim, body).expect("write open shim");
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("chmod open shim");
    worker_dir
}

/// Build a command with ONLY the 11 environment variables Codex forwards to
/// stdio MCP servers after `env_clear()` (`codex-rs/rmcp-client/src/utils.rs`).
fn codex_stripped_command(program: &str, path_value: &str, tmpdir: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear()
        .env("HOME", "/Users/dev")
        .env("LOGNAME", "dev")
        .env("PATH", path_value)
        .env("SHELL", "/bin/zsh")
        .env("USER", "dev")
        .env("__CF_USER_TEXT_ENCODING", "0x1F5:0x0:0x0")
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .env("TERM", "xterm-256color")
        .env("TMPDIR", tmpdir)
        .env("TZ", "UTC");
    cmd
}

#[test]
fn worker_open_adds_background_flag_and_hides_guard_env() {
    assert!(mirror_path().is_file());
    let fake = FakeOpen::new();
    let url = "https://auth.example.com/authorize?state=first#frag";
    let out = fake.run_mirror(&[url]);
    assert!(
        out.status.success(),
        "status={:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fake.calls(),
        vec![RecordedCall {
            guard_env: "unset".to_string(),
            args: vec!["-g".to_string(), url.to_string()],
        }]
    );
}

#[test]
fn worker_open_repeats_same_url_in_background_without_dedupe() {
    let fake = FakeOpen::new();
    let mirror_dir = tempfile::tempdir().expect("mirror tempdir");
    let first = "https://auth.example.com/authorize?state=first&code=aaa#one";
    let second = "https://auth.example.com/authorize?state=second&code=bbb#two";

    let out1 = fake
        .mirror_command(&[first])
        .env("ZEROCODE_MIRROR_DIR", mirror_dir.path())
        .output()
        .expect("run first open");
    assert!(out1.status.success());
    assert!(out1.stderr.is_empty());

    let out2 = fake
        .mirror_command(&[second])
        .env("ZEROCODE_MIRROR_DIR", mirror_dir.path())
        .output()
        .expect("run second open");
    assert!(out2.status.success());
    assert!(
        out2.stderr.is_empty(),
        "repeated web open must not be skipped or emit a skip notice: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    assert_eq!(
        fake.calls(),
        vec![
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec!["-g".to_string(), first.to_string()],
            },
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec!["-g".to_string(), second.to_string()],
            },
        ],
        "every retry must still open in background so the latest PKCE tab works"
    );
    assert!(
        fs::read_dir(mirror_dir.path())
            .expect("read mirror dir")
            .next()
            .is_none(),
        "worker_open must not create lock or state files in ZEROCODE_MIRROR_DIR"
    );
}

#[test]
fn worker_open_leaves_non_url_and_app_targeted_calls_unchanged() {
    let fake = FakeOpen::new();
    let cases: &[&[&str]] = &[
        &["/Users/dev/notes.txt"],
        &["-a", "Calculator"],
        &["-a", "Safari", "https://auth.example.com/authorize"],
        &["file:///Users/dev/index.html"],
    ];

    for (index, args) in cases.iter().enumerate() {
        let out = fake
            .mirror_command(args)
            .env("FAKE_OPEN_STDOUT", format!("out-{index}"))
            .env("FAKE_OPEN_STDERR", format!("err-{index}"))
            .env("FAKE_OPEN_EXIT", "7")
            .output()
            .expect("run non-url open");
        assert_eq!(out.status.code(), Some(7));
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("out-{index}"));
        assert_eq!(String::from_utf8_lossy(&out.stderr), format!("err-{index}"));
    }

    let expected: Vec<RecordedCall> = cases
        .iter()
        .map(|args| RecordedCall {
            guard_env: "unset".to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
        })
        .collect();
    assert_eq!(fake.calls(), expected);
}

#[test]
fn worker_open_keeps_existing_background_flag_on_repeated_calls() {
    let fake = FakeOpen::new();
    let mirror_dir = tempfile::tempdir().expect("mirror tempdir");
    let url1 = "https://auth.example.com/bg?try=1";
    let url2 = "https://auth.example.com/bg?try=2";

    let out1 = fake
        .mirror_command(&["--background", url1])
        .env("ZEROCODE_MIRROR_DIR", mirror_dir.path())
        .output()
        .expect("run first bg open");
    assert!(out1.status.success());
    assert!(out1.stderr.is_empty());

    let out2 = fake
        .mirror_command(&["--background", url2])
        .env("ZEROCODE_MIRROR_DIR", mirror_dir.path())
        .output()
        .expect("run second bg open");
    assert!(out2.status.success());
    assert!(
        out2.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&out2.stderr)
    );

    assert_eq!(
        fake.calls(),
        vec![
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec!["--background".to_string(), url1.to_string()],
            },
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec!["--background".to_string(), url2.to_string()],
            },
        ]
    );
}

#[test]
fn codex_stripped_env_opens_web_url_in_background_via_worker_shim() {
    let fake = FakeOpen::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let worker_dir = install_worker_open_shim(dir.path(), mirror_path(), &fake.bin);
    let path_value = format!(
        "{}:{}:/bin",
        worker_dir.display(),
        fake.bin_dir.display()
    );

    let first_url = "https://auth.example.com/oauth?state=one#a";
    let second_url = "https://auth.example.com/oauth?state=two#b";

    for args in [
        &[first_url][..],
        &[second_url][..],
        &["-g", first_url][..],
        &["-a", "Calculator"][..],
        &["-a", "Safari", first_url][..],
        &["/Users/dev/doc.md"][..],
    ] {
        let out = codex_stripped_command("open", &path_value, dir.path())
            .args(args)
            .output()
            .expect("run open in codex-stripped env");
        assert!(
            out.status.success(),
            "args={args:?} status={:?} stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.stderr.is_empty(),
            "args={args:?} unexpected stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    assert_eq!(
        fake.calls(),
        vec![
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec!["-g".to_string(), first_url.to_string()],
            },
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec!["-g".to_string(), second_url.to_string()],
            },
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec!["-g".to_string(), first_url.to_string()],
            },
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec!["-a".to_string(), "Calculator".to_string()],
            },
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec!["-a".to_string(), "Safari".to_string(), first_url.to_string()],
            },
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec!["/Users/dev/doc.md".to_string()],
            },
        ]
    );
}

#[test]
fn human_pane_path_without_worker_dir_runs_real_open_without_mirror() {
    let fake = FakeOpen::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let shared_shims = dir.path().join("shims");
    fs::create_dir_all(&shared_shims).expect("shared shims dir");
    let mirror_called = dir.path().join("mirror-was-called");
    let poison_mirror = dir.path().join("poison-mirror");
    let poison_body = format!(
        "#!/bin/sh\nprintf 'called\\n' >> '{}'\nexit 99\n",
        mirror_called.display()
    );
    fs::write(&poison_mirror, poison_body).expect("write poison mirror");
    fs::set_permissions(&poison_mirror, fs::Permissions::from_mode(0o755))
        .expect("chmod poison mirror");
    let _worker_dir = install_worker_open_shim(&shared_shims, &poison_mirror, &fake.bin);

    // Human pane PATH includes shared shims and fake open dir, never worker-open.
    let human_path = format!(
        "{}:{}:/bin",
        shared_shims.display(),
        fake.bin_dir.display()
    );
    let url = "https://auth.example.com/authorize?state=human";

    for attempt in 0..2 {
        let out = Command::new("open")
            .arg(url)
            .env_clear()
            .env("PATH", &human_path)
            .env("FAKE_OPEN_STDOUT", format!("stdout-{attempt}"))
            .env("FAKE_OPEN_STDERR", format!("stderr-{attempt}"))
            .env("FAKE_OPEN_EXIT", "5")
            .output()
            .expect("run human open");
        assert_eq!(out.status.code(), Some(5));
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("stdout-{attempt}"));
        assert_eq!(String::from_utf8_lossy(&out.stderr), format!("stderr-{attempt}"));
    }

    assert!(
        !mirror_called.exists(),
        "human panes must never invoke zerocode-mirror"
    );
    assert_eq!(
        fake.calls(),
        vec![
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec![url.to_string()],
            },
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec![url.to_string()],
            },
        ]
    );
}

#[test]
fn worker_open_shim_falls_back_to_real_open_when_mirror_is_missing() {
    let fake = FakeOpen::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let missing_mirror = dir.path().join("gone-mirror");
    let worker_dir = install_worker_open_shim(dir.path(), &missing_mirror, &fake.bin);
    let path_value = format!(
        "{}:{}:/bin",
        worker_dir.display(),
        fake.bin_dir.display()
    );

    let url = "https://auth.example.com/fallback?x=1";
    let out = codex_stripped_command("open", &path_value, dir.path())
        .arg(url)
        .output()
        .expect("run fallback open");
    assert!(out.status.success());
    assert_eq!(
        fake.calls(),
        vec![RecordedCall {
            guard_env: "unset".to_string(),
            args: vec![url.to_string()],
        }]
    );
}
