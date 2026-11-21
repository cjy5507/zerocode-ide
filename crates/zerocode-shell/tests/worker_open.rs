#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const OPEN_GUARD_ENV: &str = "ZEROCODE_OPEN_GUARD";
const MIRROR_DIR_ENV: &str = "ZEROCODE_MIRROR_DIR";

struct FakeOpen {
    _dir: tempfile::TempDir,
    bin: PathBuf,
    log: PathBuf,
    mirror_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedCall {
    guard_env: String,
    args: Vec<String>,
}

impl FakeOpen {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("fake-open");
        let log = dir.path().join("calls.log");
        let mirror_dir = dir.path().join("mirror-dir");
        fs::create_dir_all(&mirror_dir).expect("mirror dir");

        let script = format!(
            r#"#!/bin/sh
log='{log}'
{{
  printf 'CALL_BEGIN\n'
  printf 'GUARD=%s\n' "${{{OPEN_GUARD_ENV}:-unset}}"
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
            log = log.display()
        );
        fs::write(&bin, script).expect("write fake open");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod fake open");

        Self {
            _dir: dir,
            bin,
            log,
            mirror_dir,
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
            .env_remove(zerocode_core::agent_teams::TEAM_ID_VAR)
            .env_remove(zerocode_core::agent_teams::TEAM_PANE_VAR)
            .env_remove(zerocode_core::agent_teams::TEAM_TOKEN_VAR)
            .env_remove("ZEROCODE_AGENT_TEAM_TOKEN_FILE")
            .env_remove("ZEROCODE_HOOK_PORT")
            .env_remove("ZEROCODE_HOOK_TOKEN")
            .env(OPEN_GUARD_ENV, "1")
            .env(MIRROR_DIR_ENV, &self.mirror_dir);
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
    assert_eq!(
        fake.calls(),
        vec![RecordedCall {
            guard_env: "unset".to_string(),
            args: vec!["-g".to_string(), url.to_string()],
        }]
    );
}

#[test]
fn worker_open_skips_duplicate_url_ignoring_query_and_fragment() {
    let fake = FakeOpen::new();
    let first = "https://auth.example.com/authorize?state=first&code=aaa#one";
    let second = "https://auth.example.com/authorize?state=second&code=bbb#two";

    let out1 = fake.run_mirror(&[first]);
    assert!(out1.status.success());

    let out2 = fake.run_mirror(&[second]);
    assert!(
        out2.status.success(),
        "duplicate skip must exit 0, got {:?}: {}",
        out2.status.code(),
        String::from_utf8_lossy(&out2.stderr)
    );
    assert_eq!(
        fake.calls(),
        vec![RecordedCall {
            guard_env: "unset".to_string(),
            args: vec!["-g".to_string(), first.to_string()],
        }],
        "second call with same scheme/host/path must not invoke real open"
    );

    let stderr = String::from_utf8_lossy(&out2.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "skipped duplicate must write one stderr line: {stderr:?}"
    );
    assert!(
        lines[0].contains("https://auth.example.com/authorize"),
        "stderr line must name normalized url: {stderr:?}"
    );
    assert!(
        !lines[0].contains("state=second") && !lines[0].contains("code=bbb"),
        "stderr line must not leak query parameters: {stderr:?}"
    );
}

#[test]
fn worker_open_allows_distinct_path_and_host() {
    let fake = FakeOpen::new();
    let urls = [
        "https://auth.example.com/one?x=1",
        "https://auth.example.com/two?x=1",
        "https://other.example.com/one?x=1",
    ];
    for url in urls {
        let out = fake.run_mirror(&[url]);
        assert!(out.status.success());
    }
    let recorded: Vec<Vec<String>> = fake.calls().into_iter().map(|call| call.args).collect();
    assert_eq!(
        recorded,
        vec![
            vec!["-g".to_string(), urls[0].to_string()],
            vec!["-g".to_string(), urls[1].to_string()],
            vec!["-g".to_string(), urls[2].to_string()],
        ]
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
fn worker_open_keeps_existing_background_flag_and_dedupes_repeat() {
    let fake = FakeOpen::new();
    let url1 = "https://auth.example.com/bg?try=1";
    let url2 = "https://auth.example.com/bg?try=2";

    let out1 = fake.run_mirror(&["--background", url1]);
    assert!(out1.status.success());
    let out2 = fake.run_mirror(&["--background", url2]);
    assert!(out2.status.success());

    assert_eq!(
        fake.calls(),
        vec![RecordedCall {
            guard_env: "unset".to_string(),
            args: vec!["--background".to_string(), url1.to_string()],
        }]
    );
    assert_eq!(String::from_utf8_lossy(&out2.stderr).lines().count(), 1);
}

#[test]
fn worker_open_burst_across_six_concurrent_processes_opens_once() {
    let fake = FakeOpen::new();
    let children: Vec<_> = (0..6)
        .map(|nth| {
            let url = format!("https://auth.example.com/burst?attempt={nth}");
            fake.mirror_command(&[&url])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spawn concurrent open")
        })
        .collect();

    let mut skipped = 0usize;
    for child in children {
        let out = child.wait_with_output().expect("wait child");
        assert!(
            out.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        if !out.stderr.is_empty() {
            assert_eq!(String::from_utf8_lossy(&out.stderr).lines().count(), 1);
            skipped += 1;
        }
    }

    assert_eq!(
        fake.calls().len(),
        1,
        "exactly one process in the burst should call real open"
    );
    assert_eq!(skipped, 5, "the other five processes should report skip");
}

fn install_open_shim(dir: &Path, mirror: &Path, real_open: &Path) -> PathBuf {
    let shims = dir.join("shims");
    fs::create_dir_all(&shims).expect("shims dir");
    let shim = shims.join(zerocode_core::worker_open::SHIM_NAME);
    let body = zerocode_core::worker_open::open_shim_script(mirror, real_open);
    fs::write(&shim, body).expect("write open shim");
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("chmod open shim");
    shims
}

fn clean_open_command(program: impl AsRef<std::ffi::OsStr>, shims: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env("PATH", format!("{}:/bin", shims.display()))
        .env_remove(zerocode_core::agent_teams::TEAM_ID_VAR)
        .env_remove(zerocode_core::agent_teams::TEAM_PANE_VAR)
        .env_remove(zerocode_core::agent_teams::TEAM_TOKEN_VAR)
        .env_remove(zerocode_core::agent_teams::TEAM_TOKEN_FILE_VAR)
        .env_remove(OPEN_GUARD_ENV)
        .env_remove(MIRROR_DIR_ENV)
        .env_remove("ZEROCODE_HOOK_PORT")
        .env_remove("ZEROCODE_HOOK_TOKEN");
    cmd
}

#[test]
fn human_panes_bypass_mirror_and_pass_open_through_unchanged() {
    let fake = FakeOpen::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let mirror_called = dir.path().join("mirror-was-called");
    let poison_mirror = dir.path().join("poison-mirror");
    let poison_body = format!(
        "#!/bin/sh\nprintf 'called\\n' >> '{}'\nexit 99\n",
        mirror_called.display()
    );
    fs::write(&poison_mirror, poison_body).expect("write poison mirror");
    fs::set_permissions(&poison_mirror, fs::Permissions::from_mode(0o755))
        .expect("chmod poison mirror");

    let shims = install_open_shim(dir.path(), &poison_mirror, &fake.bin);
    let token_file = dir.path().join("team-token");
    fs::write(&token_file, "secret").expect("token file");
    let url = "https://auth.example.com/authorize?state=human";

    let scenarios: Vec<(&str, Vec<(&str, String)>)> = vec![
        ("plain-shell", vec![]),
        (
            "human-leader-pane",
            vec![
                (zerocode_core::agent_teams::TEAM_ID_VAR, "team-1".into()),
                (
                    zerocode_core::agent_teams::TEAM_PANE_VAR,
                    zerocode_core::agent_teams::LEADER_PANE.into(),
                ),
                (
                    zerocode_core::agent_teams::TEAM_TOKEN_FILE_VAR,
                    token_file.to_string_lossy().into_owned(),
                ),
            ],
        ),
        (
            "inherited-pane-without-team-token",
            vec![
                (zerocode_core::agent_teams::TEAM_ID_VAR, "team-1".into()),
                (zerocode_core::agent_teams::TEAM_PANE_VAR, "%5".into()),
            ],
        ),
    ];

    for (label, extra_env) in &scenarios {
        for attempt in 0..2 {
            let mut cmd = clean_open_command(shims.join("open"), &shims);
            cmd.arg(url)
                .env(MIRROR_DIR_ENV, &fake.mirror_dir)
                .env("FAKE_OPEN_STDOUT", format!("stdout-{label}-{attempt}"))
                .env("FAKE_OPEN_STDERR", format!("stderr-{label}-{attempt}"))
                .env("FAKE_OPEN_EXIT", "5");
            for (name, value) in extra_env {
                cmd.env(name, value);
            }
            let out = cmd.output().expect("run human open");
            assert_eq!(out.status.code(), Some(5), "scenario={label}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                format!("stdout-{label}-{attempt}")
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stderr),
                format!("stderr-{label}-{attempt}")
            );
        }
    }

    assert!(
        !mirror_called.exists(),
        "human panes must never invoke zerocode-mirror"
    );
    let expected = vec![
        RecordedCall {
            guard_env: "unset".to_string(),
            args: vec![url.to_string()],
        };
        scenarios.len() * 2
    ];
    assert_eq!(fake.calls(), expected);
}

#[test]
fn worker_panes_route_through_shim_and_dedupe_across_panes() {
    let fake = FakeOpen::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let shims = install_open_shim(dir.path(), mirror_path(), &fake.bin);
    let token_file = dir.path().join("team-token");
    fs::write(&token_file, "secret").expect("token file");

    let first_url = "https://auth.example.com/oauth?state=one#a";
    let second_url = "https://auth.example.com/oauth?state=two#b";

    // Worker pane %2 (token file): resolves bare `open` via PATH.
    let out1 = clean_open_command("open", &shims)
        .arg(first_url)
        .env(MIRROR_DIR_ENV, &fake.mirror_dir)
        .env(zerocode_core::agent_teams::TEAM_ID_VAR, "team-1")
        .env(zerocode_core::agent_teams::TEAM_PANE_VAR, "%2")
        .env(zerocode_core::agent_teams::TEAM_TOKEN_FILE_VAR, &token_file)
        .output()
        .expect("run worker %2 open");
    assert!(out1.status.success());

    // Worker pane %3 (legacy token var) sharing the window's ZEROCODE_MIRROR_DIR.
    let out2 = clean_open_command("open", &shims)
        .arg(second_url)
        .env(MIRROR_DIR_ENV, &fake.mirror_dir)
        .env(zerocode_core::agent_teams::TEAM_ID_VAR, "team-1")
        .env(zerocode_core::agent_teams::TEAM_PANE_VAR, "%3")
        .env(zerocode_core::agent_teams::TEAM_TOKEN_VAR, "legacy-secret")
        .output()
        .expect("run worker %3 open");
    assert!(out2.status.success());
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert_eq!(stderr2.lines().count(), 1, "{stderr2:?}");
    assert!(stderr2.contains("https://auth.example.com/oauth"));

    // Non-URL and -a calls inside worker pane pass through unchanged.
    for args in [&["-a", "Calculator"][..], &["/Users/dev/doc.md"]] {
        let out = clean_open_command("open", &shims)
            .args(args)
            .env(MIRROR_DIR_ENV, &fake.mirror_dir)
            .env(zerocode_core::agent_teams::TEAM_ID_VAR, "team-1")
            .env(zerocode_core::agent_teams::TEAM_PANE_VAR, "%2")
            .env(zerocode_core::agent_teams::TEAM_TOKEN_FILE_VAR, &token_file)
            .output()
            .expect("run worker non-url open");
        assert!(out.status.success());
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
                args: vec!["-a".to_string(), "Calculator".to_string()],
            },
            RecordedCall {
                guard_env: "unset".to_string(),
                args: vec!["/Users/dev/doc.md".to_string()],
            },
        ]
    );
}

#[test]
fn worker_open_shim_falls_back_to_real_open_when_mirror_is_missing() {
    let fake = FakeOpen::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let missing_mirror = dir.path().join("gone-mirror");
    let shims = install_open_shim(dir.path(), &missing_mirror, &fake.bin);
    let token_file = dir.path().join("team-token");
    fs::write(&token_file, "secret").expect("token file");

    let url = "https://auth.example.com/fallback?x=1";
    let out = clean_open_command("open", &shims)
        .arg(url)
        .env(MIRROR_DIR_ENV, &fake.mirror_dir)
        .env(zerocode_core::agent_teams::TEAM_ID_VAR, "team-1")
        .env(zerocode_core::agent_teams::TEAM_PANE_VAR, "%2")
        .env(zerocode_core::agent_teams::TEAM_TOKEN_FILE_VAR, &token_file)
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
