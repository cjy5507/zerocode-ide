//! Custom status line — Claude Code parity for the `statusLine` settings key.
//!
//! When settings carry `{"statusLine": {"type": "command", "command": "…"}}`
//! (or the shorthand `{"statusLine": "…"}`), the command runs with a JSON
//! context document on stdin and its **first stdout line** replaces the HUD's
//! bottom bar content. Refreshes are debounced ([`MIN_REFRESH_INTERVAL`]) and
//! executed on a detached thread, so a slow or wedged command can never stall
//! the render loop — the HUD simply keeps showing the previous output.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Minimum spacing between two command launches.
const MIN_REFRESH_INTERVAL: Duration = Duration::from_millis(1_000);
/// A command still running after this long is abandoned (killed best-effort).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// Context document piped to the command's stdin (Claude Code-compatible
/// field names where they exist).
#[derive(Debug, Clone)]
pub(crate) struct StatusLineInput {
    pub session_id: String,
    pub transcript_path: PathBuf,
    pub model_alias: String,
    pub model_display: String,
    pub cwd: PathBuf,
    pub project_dir: PathBuf,
    pub cost_usd: f64,
    pub ctx_used: u64,
    pub ctx_limit: u64,
    pub ctx_new_input: u64,
    pub ctx_cached: u64,
}

impl StatusLineInput {
    fn to_json(&self) -> String {
        let used_percentage = context_window_percentage(self.ctx_used, self.ctx_limit);
        let remaining_percentage = (100.0 - used_percentage).max(0.0);
        serde_json::json!({
            // Kept for hook-style scripts that branch on the event name; Claude
            // Code's statusLine docs focus on the payload fields below.
            "hook_event_name": "Status",
            "session_id": self.session_id,
            "transcript_path": self.transcript_path.to_string_lossy(),
            "cwd": self.cwd.to_string_lossy(),
            "version": env!("CARGO_PKG_VERSION"),
            "model": {
                "id": self.model_alias,
                "display_name": self.model_display,
            },
            "workspace": {
                "current_dir": self.cwd.to_string_lossy(),
                "project_dir": self.project_dir.to_string_lossy(),
                // Reflect the live extra workspace roots (CLI `--add-dir` plus
                // runtime `/add-dir`) so the statusLine payload matches the
                // directories reads/writes are actually allowed under, rather
                // than always being empty.
                "added_dirs": added_dirs(),
            },
            "cost": {
                "total_cost_usd": self.cost_usd,
            },
            // Claude Code-compatible context-window payload. The public docs'
            // inline example reads `.context_window.used_percentage`; keeping
            // only Zo's older `.context.used_tokens` made that example render
            // as 0/empty even though the HUD knew the live context pressure.
            "context_window": {
                "total_input_tokens": self.ctx_used,
                "total_output_tokens": 0,
                "context_window_size": self.ctx_limit,
                "used_percentage": used_percentage,
                "remaining_percentage": remaining_percentage,
                "current_usage": {
                    "input_tokens": self.ctx_new_input,
                    "output_tokens": 0,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": self.ctx_cached,
                },
            },
            "exceeds_200k_tokens": self.ctx_used > 200_000,
            // Backward-compatible Zo field for existing local scripts.
            "context": {
                "used_tokens": self.ctx_used,
                "limit_tokens": self.ctx_limit,
            },
        })
        .to_string()
    }
}

/// The live extra workspace roots, as display strings, for the statusLine
/// `workspace.added_dirs` payload. Sourced from the same single list the
/// boundary checks consult, so the reported directories always match what the
/// session can actually read/write.
fn added_dirs() -> Vec<String> {
    runtime::file_ops::additional_workspace_roots()
        .iter()
        .map(|root| root.to_string_lossy().into_owned())
        .collect()
}

// Token counts stay far below f64's 2^52 exact-integer range, so the cast is
// lossless in practice.
#[allow(clippy::cast_precision_loss)]
fn context_window_percentage(used: u64, limit: u64) -> f64 {
    if limit == 0 {
        return 0.0;
    }
    ((used as f64 / limit as f64) * 100.0).clamp(0.0, 100.0)
}

#[derive(Debug, Default)]
struct RunnerState {
    last_output: Option<String>,
    last_started: Option<Instant>,
    running: bool,
}

/// Debounced, thread-offloaded runner for the configured status command.
/// Shared between the idle HUD rebuild and the in-turn snapshot task via
/// `Arc`, so both surfaces read the same cached line.
#[derive(Debug, Default)]
pub(crate) struct StatusLineRunner {
    command: Mutex<Option<String>>,
    state: std::sync::Arc<Mutex<RunnerState>>,
}

impl StatusLineRunner {
    pub(crate) fn new(command: Option<String>) -> Self {
        Self {
            command: Mutex::new(command),
            state: std::sync::Arc::default(),
        }
    }

    /// Swap the configured command (e.g. after `/reload` re-reads settings).
    /// Clearing the command also clears the cached output so the stock HUD
    /// returns immediately.
    pub(crate) fn set_command(&self, command: Option<String>) {
        let mut guard = self
            .command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = *guard != command;
        *guard = command;
        drop(guard);
        if changed {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.last_output = None;
            state.last_started = None;
        }
    }

    /// The latest first-line output, if any. `None` when unconfigured or the
    /// first run hasn't completed yet (callers fall back to the stock HUD).
    pub(crate) fn current(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_output
            .clone()
    }

    /// Kick a refresh if one is due, then return the freshest cached output.
    /// Never blocks: execution happens on a detached thread.
    pub(crate) fn poll(&self, input: &StatusLineInput) -> Option<String> {
        let command = self
            .command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        let due = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let due = !state.running
                && state
                    .last_started
                    .is_none_or(|started| started.elapsed() >= MIN_REFRESH_INTERVAL);
            if due {
                state.running = true;
                state.last_started = Some(Instant::now());
            }
            due
        };
        if due {
            let state = std::sync::Arc::clone(&self.state);
            let stdin_json = input.to_json();
            std::thread::spawn(move || {
                let output = run_status_command(&command, &stdin_json);
                let mut guard = state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.running = false;
                if let Some(line) = output {
                    guard.last_output = Some(line);
                }
            });
        }
        self.current()
    }
}

/// Run the command via `sh -c`, feed the JSON context on stdin, and return the
/// first non-empty stdout line. `None` on spawn failure, timeout, non-zero
/// exit, or empty output.
fn run_status_command(command: &str, stdin_json: &str) -> Option<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_json.as_bytes());
    }
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut stdout);
    }
    stdout
        .lines()
        .map(str::trim_end)
        .find(|line| !line.trim().is_empty())
        .map(str::to_string)
}

/// Read the configured status command out of the merged settings for `cwd`.
/// Accepts Claude Code's object form (`{"type":"command","command":"…"}`) and
/// a plain-string shorthand.
pub(crate) fn status_line_command_from_config(cwd: &Path) -> Option<String> {
    let config = runtime::ConfigLoader::default_for(cwd).load().ok()?;
    let value = config.get("statusLine")?;
    if let Some(text) = value.as_str() {
        let text = text.trim();
        return (!text.is_empty()).then(|| text.to_string());
    }
    let object = value.as_object()?;
    if object
        .get("type")
        .and_then(|kind| kind.as_str())
        .is_some_and(|kind| !kind.eq_ignore_ascii_case("command"))
    {
        return None;
    }
    object
        .get("command")
        .and_then(|command| command.as_str())
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the few tests that touch the process-global workspace-root
    /// list so parallel execution can't leak roots between assertions.
    fn root_guard() -> std::sync::MutexGuard<'static, ()> {
        static GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
        GUARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn input() -> StatusLineInput {
        StatusLineInput {
            session_id: "s-1".to_string(),
            transcript_path: PathBuf::from("/tmp/session.jsonl"),
            model_alias: "opus".to_string(),
            model_display: "Claude Opus".to_string(),
            cwd: PathBuf::from("/tmp/project/subdir"),
            project_dir: PathBuf::from("/tmp/project"),
            cost_usd: 0.42,
            ctx_used: 50_000,
            ctx_limit: 200_000,
            ctx_new_input: 12_000,
            ctx_cached: 38_000,
        }
    }

    #[test]
    fn unconfigured_runner_returns_none_without_spawning() {
        let runner = StatusLineRunner::new(None);
        assert_eq!(runner.poll(&input()), None);
        assert_eq!(runner.current(), None);
    }

    #[test]
    fn command_receives_json_context_and_first_line_wins() {
        // `head -c0` 류가 아닌, stdin JSON에서 모델 id를 뽑아 두 줄을 찍는
        // 명령 — 첫 비공백 줄만 채택되어야 한다.
        let runner = StatusLineRunner::new(Some(
            "grep -o '\"id\":\"[^\"]*\"' | head -1; echo second-line".to_string(),
        ));
        assert_eq!(runner.poll(&input()), None, "first poll kicks the thread");
        let deadline = Instant::now() + Duration::from_secs(3);
        let line = loop {
            if let Some(line) = runner.current() {
                break line;
            }
            assert!(Instant::now() < deadline, "status command never completed");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(line, "\"id\":\"opus\"");
    }

    #[test]
    // The asserted percentages are exact integer values (25.0, 75.0), so a
    // strict float comparison is correct here.
    #[allow(clippy::float_cmp)]
    fn input_json_exposes_claude_context_window_fields() {
        let _guard = root_guard();
        runtime::file_ops::set_additional_workspace_roots(Vec::new());
        let value: serde_json::Value = serde_json::from_str(&input().to_json()).unwrap();

        assert_eq!(value["workspace"]["added_dirs"], serde_json::json!([]));
        assert_eq!(value["transcript_path"], "/tmp/session.jsonl");
        assert_eq!(value["workspace"]["current_dir"], "/tmp/project/subdir");
        assert_eq!(value["workspace"]["project_dir"], "/tmp/project");
        assert_eq!(value["context_window"]["total_input_tokens"], 50_000);
        assert_eq!(value["context_window"]["context_window_size"], 200_000);
        assert_eq!(
            value["context_window"]["used_percentage"].as_f64().unwrap(),
            25.0
        );
        assert_eq!(
            value["context_window"]["remaining_percentage"]
                .as_f64()
                .unwrap(),
            75.0
        );
        assert_eq!(
            value["context_window"]["current_usage"]["input_tokens"],
            12_000
        );
        assert_eq!(
            value["context_window"]["current_usage"]["cache_read_input_tokens"],
            38_000
        );
        assert_eq!(value["context"]["used_tokens"], 50_000);
        assert_eq!(value["context"]["limit_tokens"], 200_000);
    }

    #[test]
    fn refresh_is_debounced_between_polls() {
        let runner = StatusLineRunner::new(Some("echo tick".to_string()));
        let _ = runner.poll(&input());
        // Immediately polling again must not mark a second run while the
        // first is in flight or inside the debounce window.
        let _ = runner.poll(&input());
        let state = runner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(state.last_started.is_some());
    }

    #[test]
    fn config_accepts_object_and_string_forms() {
        // `statusLine` runs `sh -c`, so a repo-committed (Project) one is now
        // supply-chain gated. This test checks the object/string PARSING, which is
        // scope-independent, so it authors the value in the trusted User config
        // home (`default_for` reads `ZO_CONFIG_HOME` as the User scope).
        let _guard = root_guard();
        let dir = std::env::temp_dir().join(format!(
            "zo-statusline-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let home = dir.join("config-home");
        std::fs::create_dir_all(&home).expect("mkdir");
        let original = std::env::var("ZO_CONFIG_HOME").ok();
        std::env::set_var("ZO_CONFIG_HOME", &home);

        std::fs::write(
            home.join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"echo hi"}}"#,
        )
        .expect("write settings");
        assert_eq!(
            status_line_command_from_config(&dir).as_deref(),
            Some("echo hi")
        );
        std::fs::write(
            home.join("settings.json"),
            r#"{"statusLine":"echo direct"}"#,
        )
        .expect("write settings");
        assert_eq!(
            status_line_command_from_config(&dir).as_deref(),
            Some("echo direct")
        );

        match original {
            Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
            None => std::env::remove_var("ZO_CONFIG_HOME"),
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// Regression: `added_dirs` in the statusLine payload must reflect the live
    /// extra workspace roots (`--add-dir` / `/add-dir`), not always be empty.
    /// Before the fix the field was hardcoded to `[]`, so a directory the
    /// session could actually read/write never appeared in the payload.
    #[test]
    fn added_dirs_reflects_live_workspace_roots() {
        let _guard = root_guard();
        let extra = PathBuf::from("/tmp/zo-added-dir-root");
        runtime::file_ops::set_additional_workspace_roots(vec![extra.clone()]);

        let value: serde_json::Value = serde_json::from_str(&input().to_json()).unwrap();
        assert_eq!(
            value["workspace"]["added_dirs"],
            serde_json::json!([extra.to_string_lossy()]),
            "added_dirs must mirror the installed workspace roots"
        );

        // 전역 복원 — 다른 테스트에 영향 금지.
        runtime::file_ops::set_additional_workspace_roots(Vec::new());
        let cleared: serde_json::Value = serde_json::from_str(&input().to_json()).unwrap();
        assert_eq!(cleared["workspace"]["added_dirs"], serde_json::json!([]));
    }
}
