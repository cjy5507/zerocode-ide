//! `zo auto-dream` is the idle pass a session runs in a process of its own:
//! the real binary curates the evidence where the session keeps it, outside
//! the checkout, and answers with one JSON line.

#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

fn auto_dream(root: &Path, cwd: &Path) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zo"));
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    let output = command
        .args(["auto-dream", "--cwd"])
        .arg(cwd)
        .args(["--session", "idle-session", "--model", "test-model"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .env("HOME", root.join("home"))
        .env("ZO_CONFIG_HOME", root.join("config"))
        .env("ZO_STATE_DIR", root.join("state"))
        .env("ZO_DISABLE_MODEL_DISCOVERY", "1")
        .env("ZO_DISABLE_KEYCHAIN", "1")
        .env("ZO_DISABLE_EXTERNAL_CREDENTIALS", "1")
        .env("ZO_CLAUDE_HOME", "")
        .env("ZO_CODEX_HOME", "")
        .output()
        .expect("zo should start");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("the reply is UTF-8")
}

fn one_line(stdout: &str) -> Value {
    assert_eq!(stdout.lines().count(), 1, "one reply line: {stdout:?}");
    serde_json::from_str(stdout.trim()).expect("the reply is JSON")
}

#[test]
fn the_real_binary_curates_the_relocated_evidence_and_then_says_until_when_it_waits() {
    let root = tempfile::tempdir().expect("a test root");
    let cwd = root.path().join("workspace");
    for dir in [&cwd, &root.path().join("home"), &root.path().join("config")] {
        fs::create_dir_all(dir).expect("the test folders");
    }
    let dream = root
        .path()
        .join("state/projects")
        .join(runtime::project_slug(&cwd))
        .join("state/.zo/dream");
    fs::create_dir_all(&dream).expect("the relocated dream folder");
    let mut lines = String::new();
    for session in ["session-1", "session-2"] {
        let observation = runtime::memory::verified_check_observation(session, Some("cargo test -p acme"))
            .expect("a check command makes an observation");
        lines.push_str(&serde_json::to_string(&observation).expect("an observation serialises"));
        lines.push('\n');
    }
    fs::write(dream.join("seen.jsonl"), lines).expect("the evidence");

    let ran = one_line(&auto_dream(root.path(), &cwd));
    assert_eq!(ran["outcome"], "ran", "{ran}");
    assert_eq!(ran["promoted"].as_array().map(Vec::len), Some(1), "{ran}");

    let stamped: u64 = fs::read_to_string(dream.join(".last_auto_dream"))
        .expect("a pass that ran stamps the shared marker")
        .trim()
        .parse()
        .expect("the marker holds unix seconds");
    let waits = one_line(&auto_dream(root.path(), &cwd));
    assert_eq!(waits["outcome"], "not_due", "{waits}");
    assert_eq!(waits["untilMs"].as_u64(), Some((stamped + 6 * 60 * 60) * 1000), "{waits}");
}
