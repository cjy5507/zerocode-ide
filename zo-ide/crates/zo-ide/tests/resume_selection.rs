//! Resume selection uses conversations, not a quota of recently touched files.

use std::fs::{self, File, FileTimes};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::process::Command;

struct Fixture {
    root: TempDir,
    cwd: PathBuf,
    sessions: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        let sessions = root.path().join("store/sessions");
        for path in [&cwd, &sessions, &root.path().join("config"), &root.path().join("state")] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(root.path().join("config/settings.json"), json!({
            "smart": {"autoClassifier":"off", "orchestration":"model"},
            "mcpServers": {}
        }).to_string()).unwrap();
        Self { root, cwd, sessions }
    }

    fn session(&self, id: &str, prompts: Vec<Vec<Value>>, age: u64) -> PathBuf {
        let path = self.sessions.join(format!("{id}.jsonl"));
        let mut lines = vec![json!({"type":"session_meta", "version":1,
            "session_id":id, "created_at_ms":1, "updated_at_ms":1}).to_string()];
        for (index, blocks) in prompts.into_iter().enumerate() {
            lines.push(json!({"type":"message", "turn_index":index,
                "message":{"role":"user", "blocks":blocks}}).to_string());
        }
        fs::write(&path, lines.join("\n") + "\n").unwrap();
        File::options().write(true).open(&path).unwrap().set_times(
            FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(age))
        ).unwrap();
        path
    }

    async fn run(&self, args: &[&str]) -> String {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zo"));
        command.env_clear();
        for key in ["PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
            if let Some(value) = std::env::var_os(key) { command.env(key, value); }
        }
        command.args(args).current_dir(&self.cwd).stdin(Stdio::null())
            .env("ZO_CONFIG_HOME", self.root.path().join("config"))
            .env("ZO_SESSION_ROOT", self.root.path().join("store"))
            .env("ZO_STATE_DIR", self.root.path().join("state"))
            .env("ZO_DISABLE_MODEL_DISCOVERY", "1")
            .env("ZO_DISABLE_KEYCHAIN", "1")
            .env("ZO_DISABLE_EXTERNAL_CREDENTIALS", "1")
            .env("ZO_CLAUDE_HOME", "").env("ZO_CODEX_HOME", "")
            .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9")
            .env("ANTHROPIC_API_KEY", "test-dummy-key")
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(30), command.output())
            .await.expect("resume diagnostic did not finish").unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    }
}

fn text(value: &str) -> Value { json!({"type":"text", "text":value}) }

fn image() -> Value {
    json!({"type":"image", "source":{"type":"base64", "media_type":"image/png", "data":"iVBORw0KGgo="}})
}

#[tokio::test]
async fn continue_and_latest_skip_a_new_empty_session() {
    let fixture = Fixture::new();
    fixture.session("conversation-with-work", vec![vec![text("Continue the implementation")]], 100);
    fixture.session("empty-window", vec![], 1);
    for selector in [vec!["--continue"], vec!["--resume", "latest"]] {
        let mut args = selector;
        args.extend(["--status", "--model", "claude-sonnet-4-6", "--permission-mode", "read-only"]);
        let output = fixture.run(&args).await;
        assert!(output.contains("session conversation-with-work"), "{output}");
    }
}

#[tokio::test]
async fn empty_and_unreadable_heads_do_not_use_up_the_resume_page() {
    let fixture = Fixture::new();
    fixture.session("conversation-behind-empty-heads", vec![vec![text("Keep this task")]], 100);
    for index in 0..25 {
        fixture.session(&format!("empty-{index}"), vec![], index + 1);
    }
    fs::write(fixture.sessions.join("unreadable.jsonl"), "not a transcript\n").unwrap();
    let output = fixture.run(&["--resume"]).await;
    assert!(output.contains("conversation-behind-empty-heads"), "{output}");
}

#[tokio::test]
async fn an_image_prompt_is_resumable_without_a_text_caption() {
    let fixture = Fixture::new();
    fixture.session("image-conversation", vec![vec![image()], vec![text("Explain the image")]], 1);
    let output = fixture.run(&["--resume"]).await;
    assert!(output.contains("image-conversation"), "{output}");
}

#[tokio::test]
async fn whitespace_before_a_real_prompt_does_not_hide_the_conversation() {
    let fixture = Fixture::new();
    fixture.session("conversation-after-whitespace", vec![vec![text(" \n\t")], vec![text("Actual task")]], 1);
    let output = fixture.run(&["--resume"]).await;
    assert!(output.contains("conversation-after-whitespace"), "{output}");
}

#[tokio::test]
async fn an_explicit_empty_session_id_remains_openable() {
    let fixture = Fixture::new();
    fixture.session("explicit-empty", vec![], 1);
    let output = fixture.run(&["--resume", "explicit-empty", "--status", "--model", "claude-sonnet-4-6", "--permission-mode", "read-only"]).await;
    assert!(output.contains("session explicit-empty"), "{output}");
}
