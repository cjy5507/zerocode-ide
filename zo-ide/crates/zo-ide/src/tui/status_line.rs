//! `statusLine`: the person's own first footer row.
//!
//! Claude Code's setting, read the same way: a command — the string itself,
//! or an object's `command` — run by `sh -c` with a JSON account of the
//! session on its stdin; the first line it prints stands where the model row
//! would, after the permission mark, which always stays. A repository's own
//! `.zo/settings.json` cannot set it without the person's opt-in
//! (`runtime::config`), so a cloned project never runs a command here.
//!
//! The command runs on a thread of its own, never on the key or frame path:
//! a frame reads the last line it printed, and a refresh asked while one is
//! still running is dropped rather than queued, so a slow script costs
//! nothing but staleness.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// The setting's key, as Claude Code spells it.
pub const STATUS_LINE_KEY: &str = "statusLine";

/// What the command is told about the session, as JSON on its stdin.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub session_id: String,
    pub cwd: String,
    pub model: String,
    pub version: &'static str,
    pub permission_mode: &'static str,
    pub context_left_percent: Option<u8>,
}

impl Facts {
    fn json(&self) -> String {
        serde_json::json!({
            "hook_event_name": "Status",
            "session_id": self.session_id,
            "cwd": self.cwd,
            "model": { "id": self.model, "display_name": self.model },
            "workspace": { "current_dir": self.cwd },
            "version": self.version,
            "permission_mode": self.permission_mode,
            "context_left_percent": self.context_left_percent,
        })
        .to_string()
    }
}

/// The configured command, and the last line it printed.
#[derive(Debug)]
pub struct StatusLine {
    command: String,
    latest: Arc<Mutex<Option<Arc<str>>>>,
    running: Arc<AtomicBool>,
    /// The first run has been asked for.
    started: AtomicBool,
}

impl StatusLine {
    /// The command `cwd`'s settings name, or `None` when they name none.
    #[must_use]
    pub fn configured(cwd: &Path) -> Option<Self> {
        let config = runtime::ConfigLoader::default_for(cwd).load().ok()?;
        let value = config.get(STATUS_LINE_KEY)?;
        let command = value
            .as_str()
            .or_else(|| value.as_object().and_then(|object| object.get("command")).and_then(|command| command.as_str()))?
            .trim()
            .to_string();
        (!command.is_empty()).then(|| Self::new(command))
    }

    fn new(command: String) -> Self {
        Self {
            command,
            latest: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicBool::new(false)),
            started: AtomicBool::new(false),
        }
    }

    /// The first run, asked by the first frame; every later frame pays one
    /// atomic load.
    pub fn start_once(&self, facts: impl FnOnce() -> Facts) {
        if self.started.load(Ordering::Acquire) {
            return;
        }
        if !self.started.swap(true, Ordering::AcqRel) {
            self.refresh(&facts());
        }
    }

    /// Run the command again with `facts`, off this thread. Dropped when the
    /// last run has not finished.
    pub fn refresh(&self, facts: &Facts) {
        if self.running.swap(true, Ordering::AcqRel) {
            return;
        }
        let command = self.command.clone();
        let input = facts.json();
        let latest = Arc::clone(&self.latest);
        let running = Arc::clone(&self.running);
        let spawned = std::thread::Builder::new().name("zo-status-line".into()).spawn(move || {
            if let Some(line) = run(&command, &input) {
                *latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(line);
            }
            running.store(false, Ordering::Release);
        });
        if spawned.is_err() {
            self.running.store(false, Ordering::Release);
        }
    }

    /// The last line the command printed, if it has printed one.
    #[must_use]
    pub fn text(&self) -> Option<Arc<str>> {
        self.latest.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// `sh -c command` with `input` on stdin: its first non-empty line, escapes
/// and control bytes taken out — the footer draws the words, not a script's
/// colours or cursor moves.
fn run(command: &str, input: &str) -> Option<Arc<str>> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }
    let output = child.wait_with_output().ok()?;
    let printed = String::from_utf8_lossy(&output.stdout);
    let line = printed.lines().map(crate::util::ansi::sanitize_inline).find(|line| !line.trim().is_empty())?;
    Some(Arc::from(line.trim_end()))
}

#[cfg(test)]
mod tests {
    use super::{Facts, StatusLine};

    #[test]
    fn the_first_printed_line_stands_and_its_escapes_do_not() {
        let line = super::run("printf '\\033[1;32mmain\\033[0m · 94%%\\nsecond\\n'", "{}").expect("a line");
        assert_eq!(&*line, "main · 94%");
    }

    #[test]
    fn the_command_reads_the_session_on_its_stdin() {
        let facts = Facts { model: "claude-opus-5".into(), ..Facts::default() };
        let line = super::run("grep -o 'claude-opus-5' | head -1", &facts.json()).expect("a line");
        assert_eq!(&*line, "claude-opus-5");
    }

    #[test]
    fn a_refresh_lands_its_line_for_the_next_frame() {
        let status = StatusLine::new("echo ready".into());
        status.refresh(&Facts::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while status.text().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(status.text().as_deref(), Some("ready"));
    }
}
