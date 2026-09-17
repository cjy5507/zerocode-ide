use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use commands::{GoalCommand, LoopCommand, handle_plugins_slash_command};
use runtime::{CompactionConfig, ConfigLoader, Session};
use serde_json::{Value, json};
use tools::ConfigOutput;

use super::live_cli::{LiveCli, SessionHandle};
use super::live_cli_pickers::{prompt_model_picker, prompt_permissions_picker};
use super::report_services;
use crate::formatting::{
    format_compact_report, format_model_delegation_rows, format_model_report,
    format_model_switch_report, format_permissions_report, format_permissions_switch_report,
    format_resume_report, render_resume_usage, report_row,
};
use crate::{
    create_managed_session_handle, normalize_permission_mode,
    permission_mode_from_label, redact_for_share, render_export_text, render_repl_help,
    render_session_list, resolve_export_path, resolve_session_reference,
};
use zo_cli::tui::modals::Effort;

mod repl_dispatch;

fn plan_mode_enabled_in_current_worktree() -> io::Result<bool> {
    let cwd = env::current_dir()?;
    let path = cwd.join(".zo").join("settings.local.json");
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(value
        .get("permissions")
        .and_then(Value::as_object)
        .and_then(|permissions| permissions.get("defaultMode"))
        .and_then(Value::as_str)
        == Some("plan"))
}

fn custom_provider_catalog_supports_model(
    model: &str,
    catalog: &[(&'static str, Vec<String>)],
) -> bool {
    let requested = model.trim().to_lowercase();
    !requested.is_empty()
        && catalog.iter().any(|(_, models)| {
            models
                .iter()
                .any(|configured| configured.trim().eq_ignore_ascii_case(&requested))
        })
}

fn is_supported_runtime_model(model: &str) -> bool {
    if custom_provider_catalog_supports_model(model, &api::custom_provider_catalog()) {
        return true;
    }

    let m = model.to_lowercase();
    // Anthropic
    m.contains("claude") || m.contains("opus") || m.contains("sonnet") || m.contains("haiku")
    // OpenAI
    || m.contains("gpt") || m.contains("o1") || m.contains("o3") || m.contains("o4")
    || m.starts_with("openai")
    // Google
    || m.contains("gemini")
    // xAI
    || m.contains("grok")
    // Ollama / local
    || m.contains("llama") || m.contains("mistral") || m.contains("qwen")
    || m.contains("deepseek") || m.contains("codestral")
    // Catch-all: any slash-separated provider:model format
    || m.contains('/')
}

fn unsupported_model_report(model: &str) -> String {
    format!(
        "Model '{model}' is not recognized.\n\
         \nSupported semantic aliases:\n\
           Anthropic:  {}, opus, sonnet, haiku\n\
           OpenAI:     {}, {}\n\
           Google:     {}, gemini-pro\n\
           xAI:        {}\n\
           Custom/local: provider/model\n\
         \nUse /model <alias> or an explicit provider/model pin.",
        api::ANTHROPIC_LATEST_MODEL_ALIAS,
        api::OPENAI_LATEST_MODEL_ALIAS,
        api::OPENAI_FAST_MODEL_ALIAS,
        api::GOOGLE_LATEST_MODEL_ALIAS,
        api::XAI_LATEST_MODEL_ALIAS,
    )
}

fn render_repl_help_with_prompt_commands(prompt_commands: &[commands::PromptCommandDef]) -> String {
    use std::fmt::Write as _;

    let mut help = render_repl_help();
    if prompt_commands.is_empty() {
        return help;
    }

    help.push_str("\n\nProject prompt commands\n");
    for command in prompt_commands {
        let _ = writeln!(
            help,
            "  {:<20} {}",
            super::slash_dispatch::prompt_command_usage(command),
            command.summary()
        );
    }
    help
}

const GOAL_TODO_PREFIX: &str = "Goal: ";

fn goal_todo_store_path() -> io::Result<PathBuf> {
    // Use the shared resolver so the synthetic `Goal:` todo lands in the same
    // primary store `TodoWrite` writes and the HUD/compaction read -- not a bare
    // `cwd/.zo-todos.json` that diverges from `zo_state_base`.
    Ok(runtime::todo_store::primary_store(
        &crate::current_cli_cwd()?
    ))
}

fn todo_items_from_path(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
}

fn sync_goal_todo(goal: &str) -> io::Result<()> {
    let path = goal_todo_store_path()?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let mut items = todo_items_from_path(&path);
    items.retain(|item| {
        !item
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.starts_with(GOAL_TODO_PREFIX))
    });
    items.insert(
        0,
        json!({
            "content": format!("{GOAL_TODO_PREFIX}{goal}"),
            "activeForm": format!("Working on: {goal}"),
            "status": "in_progress",
        }),
    );
    let rendered = serde_json::to_string_pretty(&items).map_err(io::Error::other)?;
    fs::write(path, rendered)
}

fn clear_goal_todo() -> io::Result<bool> {
    let path = goal_todo_store_path()?;
    if !path.exists() {
        return Ok(false);
    }
    let mut items = todo_items_from_path(&path);
    let before = items.len();
    items.retain(|item| {
        !item
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.starts_with(GOAL_TODO_PREFIX))
    });
    if items.len() == before {
        return Ok(false);
    }
    let rendered = serde_json::to_string_pretty(&items).map_err(io::Error::other)?;
    fs::write(path, rendered)?;
    Ok(true)
}

pub(crate) fn config_string_value(output: &ConfigOutput) -> Result<Option<String>, io::Error> {
    let rendered = serde_json::to_value(output).map_err(io::Error::other)?;
    Ok(rendered
        .get("value")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned))
}

fn sanitize_session_name(raw: &str) -> Result<String, io::Error> {
    let normalized = raw
        .trim()
        .chars()
        .map(|ch| match ch {
            'a'..='z' | '0'..='9' | '-' | '_' => ch,
            'A'..='Z' => ch.to_ascii_lowercase(),
            _ => '-',
        })
        .collect::<String>();
    let collapsed = normalized
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session name must contain at least one alphanumeric character",
        ));
    }
    if collapsed.eq_ignore_ascii_case("latest") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session name 'latest' is reserved",
        ));
    }
    Ok(collapsed)
}

pub(crate) fn last_copy_payload(session: &runtime::Session) -> Option<String> {
    session
        .messages
        .iter()
        .rev()
        .find_map(text_payload_for_message)
        .or_else(|| last_non_text_block_payload(session))
}

fn text_payload_for_message(message: &runtime::ConversationMessage) -> Option<String> {
    let texts: Vec<&str> = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            runtime::ContentBlock::Text { text } if !text.trim().is_empty() => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n\n"))
}

fn last_non_text_block_payload(session: &runtime::Session) -> Option<String> {
    session.messages.iter().rev().find_map(|message| {
        message.blocks.iter().rev().find_map(|block| match block {
            runtime::ContentBlock::Text { text } if !text.trim().is_empty() => {
                Some(text.clone())
            }
            runtime::ContentBlock::ToolResult { output, .. } if !output.trim().is_empty() => {
                Some(output.clone())
            }
            runtime::ContentBlock::ToolUse { input, .. } if !input.trim().is_empty() => {
                Some(input.clone())
            }
            runtime::ContentBlock::Text { .. }
            | runtime::ContentBlock::ToolResult { .. }
            | runtime::ContentBlock::ToolUse { .. }
            | runtime::ContentBlock::Thinking { .. }
            | runtime::ContentBlock::RedactedThinking { .. } => None,
            runtime::ContentBlock::Image { media_type, .. } => {
                Some(format!("[image: {media_type}]"))
            }
        })
    })
}

pub(crate) fn copy_payload(session: &runtime::Session, all: bool) -> Option<String> {
    if all {
        Some(render_export_text(session))
    } else {
        last_copy_payload(session)
    }
}

/// A written `/share` artifact: where the transcript landed and how big it
/// is. Returned by [`write_share_artifact`] so the TUI and non-TUI Share
/// handlers can both report a consistent result line.
pub(crate) struct ShareArtifact {
    pub(crate) path: PathBuf,
    pub(crate) char_count: usize,
}

/// Write the current transcript to `<cwd>/.zo/share/<session-id>.txt` and
/// return where it landed. This is the single source of truth shared by the
/// non-TUI `LiveCli::Share` command and the TUI `/share` handler — both produce
/// the same local artifact. A bare `/share` stops here (local file only);
/// `/share gist` additionally uploads a redacted copy via
/// [`upload_share_to_gist`].
pub(crate) fn write_share_artifact(
    session_id: &str,
    session: &runtime::Session,
) -> Result<ShareArtifact, io::Error> {
    write_share_artifact_in(&env::current_dir()?, session_id, session)
}

/// Core of [`write_share_artifact`], parameterised on the base directory so
/// it is testable without mutating the process working directory. Writes
/// `<base>/.zo/share/<session-id>.txt`.
fn write_share_artifact_in(
    base: &Path,
    session_id: &str,
    session: &runtime::Session,
) -> Result<ShareArtifact, io::Error> {
    let share_dir = base.join(".zo").join("share");
    fs::create_dir_all(&share_dir)?;
    let path = share_dir.join(format!("{session_id}.txt"));
    let text = render_export_text(session);
    fs::write(&path, &text)?;
    Ok(ShareArtifact {
        path,
        char_count: text.len(),
    })
}

/// A one-line, loud warning shown *before* any bytes leave for a `/share gist`
/// upload. The user has already opted in by typing `gist`; this explains the
/// residual risk so the consent is informed.
pub(crate) fn share_gist_warning(char_count: usize) -> String {
    format!(
        "Uploading {char_count} chars to a GitHub gist. Read first:\n  \
         · A secret gist is UNLISTED, not private — anyone with the link can read it.\n  \
         · It may still contain secrets (tool output, .env, tokens); redaction reduces but cannot guarantee removal.\n  \
         · It is attributed to your active gh account.\n  \
         · Revoke any time with /unshare <id>."
    )
}

/// Upload the current transcript to a *secret* GitHub gist via the already
/// authenticated `gh` CLI and return the gist URL. The body is redacted on the
/// way out ([`redact_for_share`]); `render_export_text` and the local artifact
/// are left verbatim. The gist id is persisted to `.zo/share/<id>.gist` so
/// `/unshare` can revoke it later (best-effort — a write failure is non-fatal).
///
/// There is no hosting backend, no new dependency, and crucially **no
/// `--public`** — gists default to secret.
pub(crate) fn upload_share_to_gist(
    session_id: &str,
    session: &runtime::Session,
) -> Result<String, io::Error> {
    if Command::new("gh")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "GitHub CLI (gh) is not installed — install it to upload a share gist.",
        ));
    }

    let body = redact_for_share(&render_export_text(session));
    let url = run_gist_create(session_id, &body)?;

    // Persist the gist id for /unshare. The URL's last path segment is the id.
    if let Some(id) = url.rsplit('/').next().filter(|segment| !segment.is_empty()) {
        let _ = persist_gist_id(session_id, id);
    }
    Ok(url)
}

/// Argv for `gh gist create`, reading the body from stdin (`-`). Factored out
/// so a unit test can assert the upload stays SECRET — there is deliberately no
/// `--public` flag, and gh defaults to a secret gist.
fn gist_create_args(session_id: &str) -> Vec<String> {
    vec![
        "gist".to_string(),
        "create".to_string(),
        "-".to_string(),
        "-f".to_string(),
        format!("{session_id}.txt"),
        "-d".to_string(),
        format!("zo session {session_id}"),
    ]
}

/// Pipe `body` into `gh gist create -` and return the trimmed URL it prints.
fn run_gist_create(session_id: &str, body: &str) -> Result<String, io::Error> {
    let args = gist_create_args(session_id);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = gh_command(&arg_refs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(body.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "gh gist create failed: {}",
            stderr.trim()
        )));
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() {
        return Err(io::Error::other("gh gist create returned no URL"));
    }
    Ok(url)
}

/// Delete a previously created share gist by id (the revoke half of `/share
/// gist`). Removes the persisted id file on success.
pub(crate) fn delete_share_gist(id: &str) -> Result<(), io::Error> {
    let output = gh_command(&["gist", "delete", id, "--yes"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "gh gist delete failed: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

/// A `gh` invocation hardened like [`runtime::bash`]: `GIT_TERMINAL_PROMPT=0`
/// and `GCM_INTERACTIVE=never` keep a credential prompt off `/dev/tty` (it would
/// corrupt the ratatui frame and hang), and stdin defaults to null so no child
/// can reach the controlling terminal. The upload path overrides stdin to pipe
/// the gist body in.
fn gh_command(args: &[&str]) -> Command {
    let mut command = Command::new("gh");
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null());
    command
}

/// Persist a gist id to `.zo/share/<session-id>.gist` (plaintext id).
fn persist_gist_id(session_id: &str, id: &str) -> Result<(), io::Error> {
    let share_dir = env::current_dir()?.join(".zo").join("share");
    fs::create_dir_all(&share_dir)?;
    fs::write(share_dir.join(format!("{session_id}.gist")), id)
}

/// How a clipboard write actually reached the clipboard. Surfaced to the
/// user so they know whether the remote-capable OSC 52 path was used.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ClipboardSink {
    /// A native helper (`pbcopy`, `wl-copy`, `xclip`, `xsel`, `clip`).
    Command(&'static str),
    /// The OSC 52 terminal escape — reaches the *local* terminal's
    /// clipboard even across SSH / tmux.
    Osc52,
}

impl ClipboardSink {
    /// Short human description for the "Copied …" confirmation line.
    pub(crate) fn describe(self) -> String {
        match self {
            ClipboardSink::Command(program) => format!("via {program}"),
            ClipboardSink::Osc52 => "via terminal (OSC 52 — works over SSH/tmux)".to_string(),
        }
    }
}

/// Copy `text` to the clipboard.
///
/// Strategy:
/// 1. Inside an SSH session a native helper would target the *remote*
///    machine's clipboard (useless to the user sitting at their local
///    terminal), so skip straight to OSC 52.
/// 2. Otherwise try native helpers first — no size limit, no dependency on
///    terminal OSC 52 support.
/// 3. Fall back to the OSC 52 escape, which most modern terminal emulators
///    honour and which crosses SSH / tmux to reach the local clipboard.
///
/// OSC 52 is only emitted when stdout is a terminal, so piped output
/// (`--output-format json`, redirected files) is never corrupted by the
/// escape bytes.
pub(crate) fn write_to_clipboard(text: &str) -> Result<ClipboardSink, io::Error> {
    let command_result = (!clipboard_targets_ssh()).then(|| write_via_command(text));
    finish_clipboard_write(text, command_result)
}

/// One native clipboard write running on Tokio's blocking pool.
///
/// The worker sends only its completed command result. An event loop can await
/// [`Self::wait_until_ready`] in `select!` without terminal side effects, then
/// call [`Self::finish`] exactly once after that branch wins. This preserves the
/// ordering of an OSC 52 fallback with terminal draws.
pub(crate) struct ClipboardWrite {
    text: String,
    state: ClipboardWriteState,
}

enum ClipboardWriteState {
    Pending(tokio::sync::mpsc::Receiver<Result<&'static str, io::Error>>),
    Ready(Option<Result<&'static str, io::Error>>),
}

impl ClipboardWrite {
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self.state, ClipboardWriteState::Ready(_))
    }

    /// Await the native helper only; this method never writes to the terminal.
    pub(crate) async fn wait_until_ready(&mut self) {
        let result = match &mut self.state {
            ClipboardWriteState::Pending(receiver) => receiver
                .recv()
                .await
                .unwrap_or_else(|| Err(io::Error::other("clipboard worker stopped"))),
            ClipboardWriteState::Ready(_) => return,
        };
        self.state = ClipboardWriteState::Ready(Some(result));
    }

    /// Finish a ready operation and perform the terminal-bound fallback.
    pub(crate) fn finish(self) -> Result<ClipboardSink, io::Error> {
        let command_result = match self.state {
            ClipboardWriteState::Ready(command_result) => command_result,
            ClipboardWriteState::Pending(_) => {
                return Err(io::Error::other("clipboard helper is not ready"));
            }
        };
        finish_clipboard_write(&self.text, command_result)
    }
}

/// Start an interactive clipboard write without waiting for a native helper.
pub(crate) fn start_clipboard_write(text: String) -> ClipboardWrite {
    let state = if clipboard_targets_ssh() {
        ClipboardWriteState::Ready(None)
    } else {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let payload = text.clone();
        tokio::task::spawn_blocking(move || {
            let _ = tx.blocking_send(write_via_command(&payload));
        });
        ClipboardWriteState::Pending(rx)
    };
    ClipboardWrite { text, state }
}

/// Inside an SSH session a native helper targets the *remote* machine's
/// clipboard (useless to the user at their local terminal), so both write paths
/// skip straight to OSC 52 (which crosses SSH/tmux to the local clipboard).
fn clipboard_targets_ssh() -> bool {
    env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some()
}

/// Shared tail for both clipboard write paths: take a successful native-helper
/// result, else fall back to the OSC 52 escape (only when stdout is a terminal,
/// so piped output is never corrupted). `command_result` is `None` when the
/// helper was skipped (SSH), `Some(Ok)` on success, `Some(Err)` on failure.
fn finish_clipboard_write(
    text: &str,
    command_result: Option<Result<&'static str, io::Error>>,
) -> Result<ClipboardSink, io::Error> {
    let command_error = match command_result {
        Some(Ok(program)) => return Ok(ClipboardSink::Command(program)),
        // No helper installed, or one ran but failed — try OSC 52 next.
        Some(Err(error)) => Some(error),
        None => None,
    };

    if io::stdout().is_terminal() {
        write_osc52(text)?;
        return Ok(ClipboardSink::Osc52);
    }

    // No terminal to emit OSC 52 into; surface the most informative error.
    Err(command_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no clipboard helper found and stdout is not a terminal (OSC 52 unavailable)",
        )
    }))
}

/// Try each platform clipboard helper in turn, returning the program name
/// that succeeded. Errors only if every candidate is missing or fails.
fn write_via_command(text: &str) -> Result<&'static str, io::Error> {
    let candidates: Vec<(&'static str, Vec<&'static str>)> = if cfg!(target_os = "macos") {
        vec![("pbcopy", vec![]), ("/usr/bin/pbcopy", vec![])]
    } else if cfg!(target_os = "windows") {
        vec![("clip", vec![])]
    } else {
        vec![
            ("wl-copy", vec![]),
            ("xclip", vec!["-selection", "clipboard"]),
            ("xsel", vec!["--clipboard", "--input"]),
        ]
    };

    let mut last_not_found = true;
    let mut attempted = Vec::new();
    for (program, args) in candidates {
        attempted.push(if args.is_empty() {
            program.to_string()
        } else {
            format!("{program} {}", args.join(" "))
        });
        let spawn = Command::new(program)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawn {
            Ok(child) => {
                last_not_found = false;
                child
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        let status = child.wait()?;
        if status.success() {
            return Ok(program);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        if last_not_found {
            format!(
                "no supported clipboard command found (tried: {})",
                attempted.join(", ")
            )
        } else {
            format!("clipboard command failed (tried: {})", attempted.join(", "))
        },
    ))
}

/// Emit the OSC 52 clipboard-set escape to the terminal. Inside tmux the
/// sequence is wrapped in a DCS passthrough (requires
/// `set -g allow-passthrough on`, which is OFF by default in tmux 3.3+) with
/// the inner ESC doubled.
fn write_osc52(text: &str) -> Result<(), io::Error> {
    let sequence = osc52_sequence(
        &base64_encode(text.as_bytes()),
        zo_cli::tui::term::TermProfile::current().in_tmux,
    );
    let mut out = io::stdout().lock();
    out.write_all(sequence.as_bytes())?;
    out.flush()
}

/// Build the OSC 52 set-clipboard escape for an already base64-encoded
/// payload. When `in_tmux` is set the sequence is wrapped in a tmux DCS
/// passthrough with the inner ESC doubled.
fn osc52_sequence(payload_b64: &str, in_tmux: bool) -> String {
    if in_tmux {
        format!("\u{1b}Ptmux;\u{1b}\u{1b}]52;c;{payload_b64}\u{07}\u{1b}\\")
    } else {
        format!("\u{1b}]52;c;{payload_b64}\u{07}")
    }
}

/// Read text from the system clipboard. Returns `None` if the clipboard
/// is empty or contains non-text data.
pub(crate) fn read_text_from_clipboard() -> Option<String> {
    let candidates: Vec<(&str, Vec<&str>)> = if cfg!(target_os = "macos") {
        vec![("pbpaste", vec![])]
    } else if cfg!(target_os = "windows") {
        vec![("powershell", vec!["-Command", "Get-Clipboard"])]
    } else {
        vec![
            ("wl-paste", vec!["--no-newline"]),
            ("xclip", vec!["-selection", "clipboard", "-o"]),
            ("xsel", vec!["--clipboard", "--output"]),
        ]
    };

    for (program, args) in candidates {
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            return if text.is_empty() { None } else { Some(text) };
        }
    }
    None
}

/// Clipboard image data with its media type and base64-encoded content.
pub(crate) struct ClipboardImage {
    pub media_type: String,
    /// Base64-encoded image data, ready for the Anthropic API.
    pub data: String,
}

/// Standard base64 encoding (RFC 4648 §4) with padding.
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let b =
            (u32::from(bytes[i]) << 16) | (u32::from(bytes[i + 1]) << 8) | u32::from(bytes[i + 2]);
        output.push(TABLE[((b >> 18) & 0x3F) as usize] as char);
        output.push(TABLE[((b >> 12) & 0x3F) as usize] as char);
        output.push(TABLE[((b >> 6) & 0x3F) as usize] as char);
        output.push(TABLE[(b & 0x3F) as usize] as char);
        i += 3;
    }
    match bytes.len() - i {
        1 => {
            let b = u32::from(bytes[i]) << 16;
            output.push(TABLE[((b >> 18) & 0x3F) as usize] as char);
            output.push(TABLE[((b >> 12) & 0x3F) as usize] as char);
            output.push('=');
            output.push('=');
        }
        2 => {
            let b = (u32::from(bytes[i]) << 16) | (u32::from(bytes[i + 1]) << 8);
            output.push(TABLE[((b >> 18) & 0x3F) as usize] as char);
            output.push(TABLE[((b >> 12) & 0x3F) as usize] as char);
            output.push(TABLE[((b >> 6) & 0x3F) as usize] as char);
            output.push('=');
        }
        _ => {}
    }
    output
}

/// Try to read image data from the system clipboard. Returns `None` if the
/// clipboard does not contain an image.
#[allow(clippy::too_many_lines)] // platform-specific clipboard image read
pub(crate) fn read_image_from_clipboard() -> Option<ClipboardImage> {
    if cfg!(target_os = "macos") {
        // Try pngpaste first (brew install pngpaste) — fastest path.
        if let Ok(out) = Command::new("pngpaste")
            .arg("-")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            if out.status.success() && !out.stdout.is_empty() {
                return Some(ClipboardImage {
                    media_type: "image/png".to_string(),
                    data: base64_encode(&out.stdout),
                });
            }
        }
        // Fallback: osascript writes clipboard PNG to a temp file.
        // This avoids the `clipboard info` check which breaks on
        // non-English macOS locales due to AppleScript guillemet
        // parsing differences.
        let tmp = std::env::temp_dir().join("zo_clipboard_paste.png");
        let script = format!(
            "try\n\
             set theFile to POSIX file \"{}\"\n\
             set fd to open for access theFile with write permission\n\
             set eof fd to 0\n\
             write (the clipboard as \u{00AB}class PNGf\u{00BB}) to fd\n\
             close access fd\n\
             on error\n\
             return \"no image\"\n\
             end try",
            tmp.display()
        );
        let result = Command::new("osascript")
            .args(["-e", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if result.status.success() {
            let stdout_text = String::from_utf8_lossy(&result.stdout);
            if stdout_text.trim() == "no image" {
                return None;
            }
            let raw = std::fs::read(&tmp).ok()?;
            let _ = std::fs::remove_file(&tmp);
            if !raw.is_empty() {
                return Some(ClipboardImage {
                    media_type: "image/png".to_string(),
                    data: base64_encode(&raw),
                });
            }
        }
        None
    } else if cfg!(target_os = "linux") {
        // xclip can output image/png from clipboard.
        let output = Command::new("xclip")
            .args(["-selection", "clipboard", "-t", "image/png", "-o"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if output.status.success() && !output.stdout.is_empty() {
            return Some(ClipboardImage {
                media_type: "image/png".to_string(),
                data: base64_encode(&output.stdout),
            });
        }
        // wl-paste for Wayland.
        let output = Command::new("wl-paste")
            .args(["--type", "image/png"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if output.status.success() && !output.stdout.is_empty() {
            Some(ClipboardImage {
                media_type: "image/png".to_string(),
                data: base64_encode(&output.stdout),
            })
        } else {
            None
        }
    } else {
        // Windows: PowerShell can extract clipboard images.
        let script = r"
Add-Type -AssemblyName System.Windows.Forms
$img = [System.Windows.Forms.Clipboard]::GetImage()
if ($img -ne $null) {
    $ms = New-Object System.IO.MemoryStream
    $img.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    [System.Console]::OpenStandardOutput().Write($ms.ToArray(), 0, $ms.Length)
}
";
        let output = Command::new("powershell")
            .args(["-NoProfile", "-Command", script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if output.status.success() && !output.stdout.is_empty() {
            Some(ClipboardImage {
                media_type: "image/png".to_string(),
                data: base64_encode(&output.stdout),
            })
        } else {
            None
        }
    }
}

/// A payload pulled off the system clipboard by [`read_clipboard_payload`].
pub(crate) enum ClipboardPayload {
    Image(ClipboardImage),
    Text(String),
}

/// Read the system clipboard (image first, then text).
///
/// This is **blocking** I/O — it spawns `osascript` / `xclip` / `PowerShell` and
/// base64-encodes multi-MB image bytes — so it must never run directly on the
/// async event loop, where it would pin the executor thread and freeze the TUI
/// (spinner stalls, streaming stutters: the classic "down" state). Callers offload it
/// with `spawn_blocking`: the idle loop awaits [`handle_clipboard_paste`], the
/// mid-turn loop drives it through a dedicated `select!` arm.
pub(crate) fn read_clipboard_payload() -> Option<ClipboardPayload> {
    read_image_from_clipboard()
        .map(ClipboardPayload::Image)
        .or_else(|| read_text_from_clipboard().map(ClipboardPayload::Text))
}

/// Apply a clipboard payload to the app. Cheap and main-thread only — it just
/// records the attachment / inserts text — so it never blocks the loop.
pub(crate) fn apply_clipboard_payload(
    app: &mut zo_cli::tui::App,
    payload: ClipboardPayload,
) {
    match payload {
        ClipboardPayload::Image(img) => {
            if let Err(error) = app.push_clipboard_image(img.media_type, img.data) {
                app.report_queue_limit_error(error);
            }
        }
        ClipboardPayload::Text(text) => app.handle_paste_owned(text),
    }
}

/// Read the system clipboard and feed it into the TUI app on Ctrl+V.
///
/// The read is blocking (process spawn + base64 of large image data), so it is
/// offloaded to a blocking thread via `spawn_blocking` and awaited — the async
/// caller yields instead of pinning the executor thread, so a large image paste
/// no longer hard-freezes the runtime. Used by the idle loop (no turn is
/// streaming there); the mid-turn loop runs `read_clipboard_payload` through its
/// own `select!` arm so the spinner keeps animating during the read.
pub(crate) async fn handle_clipboard_paste(app: &mut zo_cli::tui::App) {
    if let Ok(Some(payload)) = tokio::task::spawn_blocking(read_clipboard_payload).await {
        apply_clipboard_payload(app, payload);
    }
}

pub(crate) fn open_in_desktop(path: &Path) -> Result<(), io::Error> {
    let path_str = path.to_string_lossy().to_string();
    let candidates: Vec<(&str, Vec<String>)> = if cfg!(target_os = "macos") {
        vec![("open", vec![path_str])]
    } else if cfg!(target_os = "windows") {
        vec![(
            "cmd",
            vec![
                "/C".to_string(),
                "start".to_string(),
                String::new(),
                path_str,
            ],
        )]
    } else {
        vec![("xdg-open", vec![path_str])]
    };

    let mut last_not_found = true;
    for (program, args) in candidates {
        match Command::new(program).args(&args).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(_) => last_not_found = false,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        if last_not_found {
            "no supported desktop opener command found"
        } else {
            "desktop opener command failed"
        },
    ))
}

fn security_secret_scan(scope: Option<&str>) -> String {
    let target = scope.unwrap_or(".");
    let output = Command::new("rg")
        .args([
            "-n",
            "--hidden",
            "--glob",
            "!.git",
            "(BEGIN [A-Z ]*PRIVATE KEY|AKIA[0-9A-Z]{16}|api[_-]?key|secret|password\\s*=|token\\s*=)",
            target,
        ])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.trim().is_empty() {
                "  Secrets scan      no matches found".to_string()
            } else {
                let matches = stdout.lines().take(10).collect::<Vec<_>>().join("\n  ");
                format!("  Secrets scan      potential matches\n  {matches}")
            }
        }
        Ok(output) if output.status.code() == Some(1) => {
            "  Secrets scan      no matches found".to_string()
        }
        Ok(_) | Err(_) => "  Secrets scan      unavailable (rg not installed)".to_string(),
    }
}

pub(crate) fn config_bool_value(output: &ConfigOutput) -> Result<Option<bool>, io::Error> {
    let rendered = serde_json::to_value(output).map_err(io::Error::other)?;
    Ok(rendered.get("value").and_then(Value::as_bool))
}

pub(crate) fn last_thinkback_lines(session: &runtime::Session) -> Vec<String> {
    let mut lines = Vec::new();
    for message in session.messages.iter().rev().take(6).rev() {
        let role = match message.role {
            runtime::MessageRole::System => "system",
            runtime::MessageRole::User => "user",
            runtime::MessageRole::Assistant => "assistant",
            runtime::MessageRole::Tool => "tool",
        };
        for block in &message.blocks {
            match block {
                runtime::ContentBlock::Text { text } => {
                    let summary = text.lines().next().unwrap_or("").trim();
                    if !summary.is_empty() {
                        lines.push(format!("{role}: {summary}"));
                    }
                }
                runtime::ContentBlock::ToolUse { name, .. } => {
                    lines.push(format!("{role}: tool use {name}"));
                }
                runtime::ContentBlock::ToolResult {
                    tool_name,
                    is_error,
                    ..
                } => {
                    lines.push(format!(
                        "{role}: tool result {tool_name} ({})",
                        if *is_error { "error" } else { "ok" }
                    ));
                }
                runtime::ContentBlock::Image { media_type, .. } => {
                    lines.push(format!("{role}: [image: {media_type}]"));
                }
                // Reasoning blocks are internal; omit from the transcript summary.
                runtime::ContentBlock::Thinking { .. }
                | runtime::ContentBlock::RedactedThinking { .. } => {}
            }
        }
    }
    lines.truncate(8);
    lines
}

impl LiveCli {
    fn run_prompt_command(
        &mut self,
        command: &commands::PromptCommandDef,
        args: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(model) = command.model.as_deref() {
            eprintln!("{}", self.apply_model_change(model));
            self.persist_session()?;
        }
        if let Some(effort) = command.effort.as_deref() {
            if let Some(preset) = Effort::from_token(effort) {
                let warning = self.set_effort(preset);
                eprintln!("Prompt command effort: {}", preset.canonical());
                if let Some(warning) = warning {
                    eprintln!("{warning}");
                }
            } else if let Ok(custom) = effort.parse::<u32>() {
                let warning = self.set_effort_budget(custom);
                eprintln!("Prompt command effort: {custom}");
                if let Some(warning) = warning {
                    eprintln!("{warning}");
                }
            } else {
                eprintln!(
                    "Prompt command\n  Command          /{}\n  Invalid effort   \"{effort}\"\n  Source           {}",
                    command.name,
                    command.path.display()
                );
                return Ok(());
            }
        }
        // Restrict the tool set for this turn to the command's `allowed-tools`
        // frontmatter, mirroring the TUI slash-dispatch path. Without this the
        // headless / `-p` path silently ran a read-only-declared command with the
        // full write/bash tool set — the restriction dropped exactly where it
        // matters most (automation/CI). Reuses the same normalizer as
        // `--allowed-tools`; an invalid spec aborts before the turn.
        if !command.allowed_tools.is_empty() {
            match crate::cli_args::normalize_allowed_tools(&command.allowed_tools) {
                Ok(turn_allowed) => self.turn_allowed_tools = turn_allowed,
                Err(error) => {
                    eprintln!(
                        "Prompt command\n  Command          /{}\n  Invalid tools    {error}\n  Source           {}",
                        command.name,
                        command.path.display()
                    );
                    return Ok(());
                }
            }
        }
        eprintln!(
            "Prompt command\n  Command          /{}\n  Source           {}",
            command.name,
            command.path.display()
        );
        self.run_turn(&command.render_prompt(args))
    }

    fn print_status(&self) {
        println!("{}", self.status_report());
    }

    pub(crate) fn goal_status_report(&self) -> String {
        self.goal_controller.status_report()
    }

    pub(crate) fn goal_todo_sync_line(goal: &str) -> String {
        match sync_goal_todo(goal) {
            Ok(()) => "  Todo             synced as top item".to_string(),
            Err(error) => format!("  Todo             not synced ({error})"),
        }
    }

    pub(crate) fn goal_todo_clear_line() -> String {
        match clear_goal_todo() {
            Ok(true) => "  Todo             removed goal item".to_string(),
            Ok(false) => "  Todo             no goal item found".to_string(),
            Err(error) => format!("  Todo             not synced ({error})"),
        }
    }

    pub(crate) fn handle_goal_command_repl(
        &mut self,
        command: GoalCommand,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match command {
            GoalCommand::Status => println!("{}", self.goal_status_report()),
            GoalCommand::Start { goal, options } => {
                let goal_text = goal.clone();
                let (report, prompt) = self.start_goal_controller(goal, options);
                // `None` prompt = the ambiguity gate held the goal back (a
                // started goal always has an action prompt): print only the
                // clarify report — no todo, no plan banner, no turn.
                let Some(prompt) = prompt else {
                    println!("{report}");
                    return Ok(());
                };
                let todo_line = Self::goal_todo_sync_line(&goal_text);
                println!(
                    "{report}\n{todo_line}\n  Plan             required before execution\n  Status           executing first goal turn now"
                );
                self.run_goal_prompt_until_stop(prompt)?;
            }
            GoalCommand::Verify => {
                println!("{}", self.verify_goal_controller());
            }
            GoalCommand::Pause => println!("{}", self.pause_goal_controller()),
            GoalCommand::Resume => {
                let (report, prompt) = self.resume_goal_controller();
                println!("{report}");
                if let Some(prompt) = prompt {
                    self.run_goal_prompt_until_stop(prompt)?;
                }
            }
            GoalCommand::Clear => {
                let todo_line = Self::goal_todo_clear_line();
                println!("{}\n{todo_line}", self.clear_goal_controller());
            }
            GoalCommand::History => println!("{}", self.goal_controller.history_report()),
            GoalCommand::Edit { goal } => {
                let todo_line = Self::goal_todo_sync_line(&goal);
                println!("{}\n{todo_line}", self.edit_goal_controller(goal));
            }
        }
        Ok(())
    }

    fn run_goal_prompt_until_stop(
        &mut self,
        mut prompt: String,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            // Every prompt this loop runs is goal-owned (the goal action prompt or
            // a queued repair). Latch ownership for THIS turn here — the headless
            // path has no message queue to carry the `goal_owned` tag — so the
            // advance below attributes the turn to the goal.
            self.goal_turn_pending = true;
            let output_tokens = self.run_turn_capturing(&prompt)?;
            // The headless sync `run_turn` path does not route through the deep
            // gate, so it produces no semantic verdict; pass `None` (the goal
            // gate then relies on deterministic validators, and honestly reports
            // "unverified" at the cap for a goal that has none). The output-token
            // count is charged against the goal's token budget / stall ledger.
            match self.advance_goal_after_turn_blocking(None, output_tokens) {
                super::automation::GoalAdvance::Idle => {
                    // Goal no longer active: drop the synthetic `Goal:` todo so
                    // it never lingers in_progress.
                    let _ = clear_goal_todo();
                    break;
                }
                super::automation::GoalAdvance::Done(report) => {
                    println!("{report}");
                    // Goal reached a terminal state on its own (succeeded /
                    // failed / unverified). Clear the synthetic `Goal:` todo --
                    // previously only `/goal clear` did, so an auto-completed
                    // goal left its todo stuck in_progress forever.
                    let _ = clear_goal_todo();
                    break;
                }
                super::automation::GoalAdvance::Queue {
                    report,
                    prompt: next,
                } => {
                    println!("{report}");
                    prompt = next;
                }
                super::automation::GoalAdvance::Pause(report) => {
                    // Auto-paused at an unattended checkpoint. Headless has no
                    // later "resume" affordance inside this invocation, so stop
                    // the run loop; the goal (and its todo) stays paused for a
                    // future `/goal resume`.
                    println!("{report}");
                    break;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn handle_loop_command_repl(
        &mut self,
        command: LoopCommand,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match self.handle_loop_controller_command(command) {
            super::automation::LoopCommandResult::Report(report) => println!("{report}"),
            super::automation::LoopCommandResult::Queue { report, prompts } => {
                println!("{report}");
                for queued in prompts {
                    // Route each run through the same pop-time gate as the TUI so
                    // the turn cap is charged via the decision-core ledger (and a
                    // stopped loop is honored). Headless is synchronous, so a run
                    // can't be interrupted mid-turn, but the gate keeps run_count
                    // and the loop's terminal status consistent across paths.
                    if self.begin_loop_turn(&queued.loop_id)
                        == super::automation::LoopTurnGate::Skip
                    {
                        break;
                    }
                    self.run_turn(&queued.text)?;
                    // Stop the loop if its `--until` completion check now passes,
                    // or if it has stalled on the same failure with no progress
                    // (synchronous headless path — the validators run inline).
                    if let Some(until) =
                        self.loop_controller.loop_until_validators(&queued.loop_id)
                    {
                        let report = super::automation::run_validators(&self.cwd, &until, None);
                        if report.ok {
                            self.loop_controller.complete_loop(&queued.loop_id);
                        } else {
                            match self
                                .loop_controller
                                .observe_loop_stall(&queued.loop_id, &report.objective_failures)
                            {
                                super::automation::LoopStallVerdict::Continue => {}
                                super::automation::LoopStallVerdict::Stalled => {
                                    self.loop_controller.stall_loop(&queued.loop_id);
                                }
                                super::automation::LoopStallVerdict::Blocked(need) => {
                                    self.loop_controller.block_loop(&queued.loop_id, need);
                                    // Escalations must reach the human even headless.
                                    println!(
                                        "Loop {} stopped — `--until` blocked; needs {}. Next: {}",
                                        queued.loop_id,
                                        need.label(),
                                        need.remedy()
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn set_model(&mut self, model: Option<String>) -> Result<bool, Box<dyn std::error::Error>> {
        let model = if let Some(model) = model {
            model
        } else if let Some(picked) = prompt_model_picker(&self.model)? {
            picked
        } else {
            println!(
                "{}",
                format_model_report(
                    &self.model,
                    self.runtime.session().messages.len(),
                    self.runtime.usage().turns(),
                )
            );
            return Ok(false);
        };

        let previous = self.model.clone();
        let report = self.apply_model_change(&model);
        println!("{report}");
        let changed = self.model != previous;
        Ok(changed)
    }

    pub(crate) fn apply_model_change(&mut self, model: &str) -> String {
        let model = crate::cli_args::resolve_model_alias(model);

        if model == self.model {
            return format_model_report(
                &self.model,
                self.runtime.session().messages.len(),
                self.runtime.usage().turns(),
            );
        }

        if !is_supported_runtime_model(&model) {
            return unsupported_model_report(&model);
        }

        let previous = self.model.clone();
        let message_count = self.runtime.session().messages.len();
        let handoff_memory = super::live_cli::model_handoff_system_reminder(
            &previous,
            &model,
            self.runtime.session().messages.as_ref(),
            self.session_goal.as_deref(),
        );
        if let Some(rt) = self.runtime.runtime.as_mut() {
            if let Err(error) = rt.api_client_mut().set_model(&model) {
                return format!("Failed to switch model to '{model}': {error}");
            }
            // Re-derive the context window, full-compaction threshold, and
            // model-family cheap-trim policy for the new model. The runtime
            // otherwise keeps the *startup* model's limits/policy, so switching
            // to Opus (1M) would still compact at GPT's 258k → 219k (far too
            // early), and its microcompact trigger would remain tuned for the
            // old family. The HUD already tracks the new model; this keeps the
            // actual compaction triggers in lockstep with it.
            rt.set_context_model(&model);
            // Propagate the new model to the tool-dispatch context so post-switch
            // `SpawnMultiAgent` / `Agent` inherit it. `active_model` is an
            // `Arc<Mutex>` shared cell, so setting it on the executor's registry
            // clone also updates the concurrent-dispatch closure's clone — the
            // one that actually serves live tool calls (see
            // `ToolContext::active_model`). Without this the sub-agents keep
            // spawning on the startup model.
            rt.tool_executor_mut()
                .tool_registry_mut()
                .context()
                .set_active_model(&model);
        }
        // The runtime retargeted its own copy inside `set_context_model`; keep
        // the host's copy in lockstep so a later `/reload`-free rebuild does not
        // reintroduce the previous model's `Model family:` line.
        runtime::retarget_prompt_model(&mut self.system_prompt, &model);
        self.model.clone_from(&model);
        // The last request's cache split belongs to the model we just left.
        self.disown_hud_cache_split();
        // Every `apply_model_change` caller is a user naming a model (flag,
        // slash command, picker, /fast tier flip), so the session model is now
        // an explicit pin — of the FOREGROUND turn. Delegation keeps routing
        // per role; see `LiveCli::set_model_user_pinned`.
        self.set_model_user_pinned(true);
        crate::session::slash_dispatch::add_model_to_history(&model);
        self.model_handoff_memory = handoff_memory;
        self.apply_session_system_reminders();
        let persistence_error = self.persist_session_preferences().err();
        let mut report = format_model_switch_report(&previous, &model, message_count);
        // State the pin's scope where it is set. Silence here is what made the
        // old behavior invisible: users read "/model X" as "everything runs on
        // X" and never learned delegation had stopped routing.
        if self.model_user_pinned() && crate::session::smart_settings::smart_routing_enabled() {
            report.push('\n');
            report.push_str(&format_model_delegation_rows());
        }
        if let Some(error) = persistence_error {
            let _ = write!(
                report,
                "\n  Warning          model preference was not saved: {error}"
            );
        }
        report
    }

    /// `/fast [on|off|status]` — toggle the active OpenAI model's priority
    /// service tier. Fast mode is orthogonal to reasoning effort: it asks the
    /// backend to prioritise serving while the model thinks just as hard. The
    /// API capability catalog owns each family's base/fast spelling; this CLI
    /// only applies the pair it receives.
    pub(crate) fn toggle_fast(&mut self, mode: Option<&str>) -> String {
        let current = self.model.to_ascii_lowercase();
        let Some((base_id, fast_id)) = fast_variant_pair(&current) else {
            return [
                "Fast".to_string(),
                report_row("Status", "unavailable"),
                report_row("Active model", &self.model),
                report_row(
                    "Note",
                    "priority serving is unavailable for this catalog model — choose an OpenAI model with a verified fast tier",
                ),
            ]
            .join("\n");
        };
        let currently_on = api::openai_fast_tier_enabled(&current);
        match mode.map(str::trim).filter(|m| !m.is_empty()) {
            Some("on") => {
                if currently_on {
                    return format!("Fast mode is already on — {base_id} requests use priority serving.");
                }
                let report = self.apply_model_change(&fast_id);
                if report.starts_with("Failed") {
                    return report;
                }
                format!("Fast mode on — {base_id} now uses priority serving (~1.5x faster, higher credit rate). Reasoning effort is unchanged.")
            }
            Some("off") => {
                if !currently_on {
                    return format!("Fast mode is already off — {base_id} uses standard serving.");
                }
                let report = self.apply_model_change(&base_id);
                if report.starts_with("Failed") {
                    return report;
                }
                format!("Fast mode off — standard serving restored for {base_id}.")
            }
            None | Some("status") => {
                let state = if currently_on { "on" } else { "off" };
                [
                    "Fast".to_string(),
                    report_row("Status", state),
                    report_row("Usage", "/fast [on|off]"),
                    report_row(
                        "Effect",
                        format!(
                            "priority serving for {base_id} (~1.5x faster); reasoning effort unchanged"
                        ),
                    ),
                ]
                .join("\n")
            }
            Some(other) => [
                "Fast".to_string(),
                report_row("Unknown option", format!("\"{other}\"")),
                report_row("Usage", "/fast [on|off]"),
            ]
            .join("\n"),
        }
    }

    fn set_permissions(
        &mut self,
        mode: Option<String>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let mode = if let Some(mode) = mode {
            mode
        } else if let Some(picked) = prompt_permissions_picker(self.permission_mode.as_str())? {
            picked
        } else {
            println!(
                "{}",
                format_permissions_report(self.permission_mode.as_str())
            );
            return Ok(false);
        };

        let changed = normalize_permission_mode(&mode)
            .is_some_and(|normalized| normalized != self.permission_mode.as_str());
        let report = self.apply_permission_change(&mode)?;
        println!("{report}");
        Ok(changed)
    }

    pub(crate) fn apply_permission_change(
        &mut self,
        mode: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let normalized = normalize_permission_mode(mode).ok_or_else(|| {
            format!(
                "unsupported permission mode '{mode}'. Use read-only, workspace-write, or danger-full-access."
            )
        })?;

        if normalized == self.permission_mode.as_str() {
            return Ok(format_permissions_report(normalized));
        }

        let previous = self.permission_mode.as_str().to_string();
        let next_mode = permission_mode_from_label(normalized);
        // Do all fallible work BEFORE committing any state. Building the policy
        // can fail; if it did after we had already advanced `self.permission_mode`
        // or the shared tool-context cell, LiveCli would sit on the new mode while
        // the live runtime kept the old policy — a split brain that the caller's
        // App-side rollback cannot repair. Compute from `next_mode` (a local),
        // then commit `self.permission_mode`, the tool-context cell, and the
        // runtime policy together only once the build succeeds.
        //
        // The commit swaps ONLY the permission policy on the live runtime — the
        // policy is the single mode-dependent component. The old path rebuilt the
        // whole runtime (LSP re-spawn with a 5s-per-server init budget, MCP/plugin
        // re-init, old-runtime teardown) synchronously on the TUI event-loop
        // thread, which froze the screen on every Shift+Tab cycle.
        let policy = crate::conversation_support::permission_policy(
            next_mode,
            &self.runtime.feature_config,
            &self.runtime.api_client().tool_registry(),
        )?;

        // The runtime policy swap is part of the commit, so the live runtime
        // MUST be available before we mutate anything. `try_runtime_mut` returns
        // None while the runtime is taken (e.g. a turn in flight); committing
        // the mode and tool-context cell and then silently skipping
        // `set_permission_policy` would leave LiveCli and the runtime split. Bail
        // out here — before any mutation — so the commit stays all-or-nothing.
        if self.runtime.try_runtime_mut().is_none() {
            return Err(format!(
                "cannot change permission mode to {normalized}: the live runtime is unavailable (a turn may be in progress). Retry once it completes."
            )
            .into());
        }

        // All fallible/availability checks have passed; commit atomically.
        self.permission_mode = next_mode;
        // Refresh the shared tool-context permission mode too, so the file-tool
        // workspace-boundary relaxation tracks the switch live (the registry
        // carries no enforcer; the boundary reads this cell). Without it, a
        // Shift+Tab into danger-full-access would still deny outside `read_file`
        // until the next runtime rebuild.
        self.runtime
            .api_client()
            .tool_registry()
            .context()
            .set_permission_mode(self.permission_mode);
        self.runtime
            .try_runtime_mut()
            .expect("runtime availability was verified above before committing")
            .set_permission_policy(policy);
        Ok(format_permissions_switch_report(&previous, normalized))
    }

    fn clear_session(&mut self, confirm: bool) -> Result<bool, Box<dyn std::error::Error>> {
        let report = self.clear_session_report(confirm)?;
        println!("{report}");
        Ok(confirm)
    }

    pub(crate) fn clear_session_report(
        &mut self,
        confirm: bool,
    ) -> Result<String, Box<dyn std::error::Error>> {
        if !confirm {
            return Ok(
                "clear: confirmation required; run /clear --confirm to start a fresh session."
                    .to_string(),
            );
        }

        let previous_session = self.session.clone();
        let session_state = Session::new();
        self.session =
            create_managed_session_handle(&session_state.session_id, self.session_scope)?;
        // A fresh `/clear` session must also get a fresh per-session todo
        // store. Without this, `ZO_TODO_STORE` keeps pointing at the
        // previous session's `.todos.json`, and the next HUD/live-panel poll
        // resurrects the old `Updated Plan` in the new session.
        super::live_cli::scope_todo_store_to_session(&self.session.path, true);
        // Swap only the session on the live runtime (same fast path as
        // `resume_session_fast`). The old path rebuilt the entire runtime —
        // LSP re-spawn, MCP/plugin re-init, old-runtime teardown — synchronously
        // on the TUI event-loop thread, freezing the screen on every `/new`.
        if let Some(rt) = self.runtime.runtime.as_mut() {
            rt.replace_session(session_state.with_persistence_path(self.session.path.clone()));
            rt.tool_executor_mut()
                .tool_registry_mut()
                .context()
                .set_session_id(&self.session.id);
            // read-before-edit 레지스트리는 아래 refresh_agent_manifest_scope →
            // refresh_file_reads_scope가 새 세션 사이드카로 rebind하며 교체한다.
            // 여기서 직접 clear()하면 안 된다: 이 시점의 레지스트리는 아직
            // 이전 세션 사이드카에 바인드돼 있어, clear의 write-through가
            // 이전 세션의 신선도 기준선을 지워 버린다(/clear는 이전 세션을
            // /resume 가능하게 보존하는 명령이다).
        } else {
            return Err("runtime not available".into());
        }
        self.refresh_agent_manifest_scope();
        Ok(format!(
            "Session cleared\n  Mode             fresh session\n  Previous session {}\n  Resume previous  /resume {}\n  Preserved model  {}\n  Permission mode  {}\n  New session      {}\n  Session file     {}",
            previous_session.id,
            previous_session.id,
            self.model,
            self.permission_mode.as_str(),
            self.session.id,
            self.session.path.display(),
        ))
    }

    pub(crate) fn new_session_report(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        self.clear_session_report(true)
    }

    fn print_cost(&self) {
        println!("{}", self.cost_report());
    }

    fn resume_session(
        &mut self,
        session_path: Option<&str>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let report = self.resume_session_report(session_path)?;
        println!("{report}");
        Ok(session_path.is_some())
    }

    pub(crate) fn resume_session_report(
        &mut self,
        session_path: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let session_ref = match session_path {
            Some(r) => r.to_string(),
            None => {
                // Auto-resolve to latest; if no sessions exist, show usage.
                match resolve_session_reference("latest") {
                    Ok(_) => "latest".to_string(),
                    Err(_) => return Ok(render_resume_usage()),
                }
            }
        };

        let handle = resolve_session_reference(&session_ref)?;
        let session = Session::load_from_path(&handle.path)?;
        let message_count = session.messages.len();
        let session_id = session.session_id.clone();
        let runtime = self.build_runtime(
            session,
            &handle.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        // The rehydrated tracker's last turn came out of the loaded transcript,
        // written by a run whose model this one need not match.
        self.disown_hud_cache_split();
        self.session = SessionHandle {
            id: session_id,
            path: handle.path,
        };
        // Follow the todo store to the resumed session so its restored checklist
        // shows (and new todos land beside the resumed transcript, not in the
        // previous session's store). `fresh = false`: keep the resumed todos.
        super::live_cli::scope_todo_store_to_session(&self.session.path, false);
        self.refresh_agent_manifest_scope();
        Ok(format_resume_report(
            &self.session.path.display().to_string(),
            message_count,
            self.runtime.usage().turns(),
        ))
    }

    /// Fast resume that swaps only the session without rebuilding
    /// MCP/LSP/plugin state. Avoids the 10s+ blocking MCP discovery.
    pub(crate) fn resume_session_fast(
        &mut self,
        session_path: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let session_ref = match session_path {
            Some(r) => r.to_string(),
            None => match resolve_session_reference("latest") {
                Ok(_) => "latest".to_string(),
                Err(_) => return Ok(render_resume_usage()),
            },
        };

        let handle = resolve_session_reference(&session_ref)?;
        let session = Session::load_from_path(&handle.path)?;
        let message_count = session.messages.len();
        let session_id = session.session_id.clone();
        if let Some(rt) = self.runtime.runtime.as_mut() {
            rt.replace_session(session);
            rt.tool_executor_mut()
                .tool_registry_mut()
                .context()
                .set_session_id(&session_id);
            // read-before-edit 레지스트리는 아래 refresh_agent_manifest_scope →
            // refresh_file_reads_scope가 재개 세션의 사이드카로 rebind하며
            // 복원한다(다른 세션의 기록은 rebind가 교체하므로 누출 불가).
            // 과거의 수동 clear는 "그 사이 바뀌었을 수 있다"는 보수적
            // 기준선이었지만, 사이드카가 마지막 관측 해시를 보존하므로 바뀐
            // 파일만 정확히 ModifiedSinceRead에 걸린다 — 재읽기 전면 강요보다
            // 엄밀히 낫다. 직접 clear()하면 이전 세션 사이드카(아직 바인드
            // 중)에 빈 상태가 write-through되는 부작용도 있다.
        } else {
            return Err("runtime not available".into());
        }
        // Unlike the full resume above, this path swaps the session WITHOUT
        // rebuilding the runtime, so the usage tracker keeps the outgoing
        // session's last turn — a split belonging to another transcript
        // entirely. It must not be shown against the resumed one.
        self.disown_hud_cache_split();
        self.session = SessionHandle {
            id: session_id,
            path: handle.path,
        };
        // Follow the todo store to the resumed session so its restored checklist
        // shows (and new todos land beside the resumed transcript, not in the
        // previous session's store). `fresh = false`: keep the resumed todos.
        super::live_cli::scope_todo_store_to_session(&self.session.path, false);
        self.refresh_agent_manifest_scope();
        Ok(format_resume_report(
            &self.session.path.display().to_string(),
            message_count,
            self.runtime.usage().turns(),
        ))
    }

    fn print_config(section: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", report_services::config_report(section)?);
        Ok(())
    }

    fn print_memory() -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", report_services::memory_report()?);
        Ok(())
    }

    /// `/dream` — manually force a between-sessions memory curation pass now,
    /// bypassing the startup throttle. The Dreamer also runs automatically at
    /// startup; this is the on-demand escape hatch. Promotes only lessons that
    /// were repeated across distinct sessions *and* verified, then prints what
    /// it wrote and what it skipped (with reasons).
    fn run_dream(&self) -> Result<(), Box<dyn std::error::Error>> {
        let report = runtime::dream_at_cwd(&self.cwd)?;
        println!("Dream");
        println!("  Result           {}", report.summary_line());
        for applied in &report.applied {
            println!(
                "  + {:<16} {} ({})",
                "promoted",
                applied.slug,
                applied.outcome.as_str()
            );
        }
        if report.applied.is_empty() {
            println!("  (no new lessons met the promotion bar)");
        }
        Ok(())
    }

    pub(crate) fn print_agents(args: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", report_services::agents_report(args)?);
        Ok(())
    }

    pub(crate) fn print_inbox(&self, args: Option<&str>) {
        println!(
            "{}",
            report_services::inbox_command(&self.cwd, &self.session.id, args)
        );
    }

    pub(crate) fn print_mcp(args: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", report_services::mcp_report(args)?);
        Ok(())
    }

    pub(crate) fn print_skills(args: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", report_services::skills_report(args)?);
        Ok(())
    }

    fn print_diff() -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", report_services::diff_report()?);
        Ok(())
    }

    fn print_version() {
        println!("{}", report_services::version_report());
    }

    fn export_session(
        &self,
        requested_path: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let export_path = resolve_export_path(requested_path, self.runtime.session())?;
        let text = render_export_text(self.runtime.session());
        crate::write_atomic(&export_path, text.as_bytes())?;
        println!(
            "Export\n  Result           wrote transcript\n  File             {}\n  Messages         {}",
            export_path.display(),
            self.runtime.session().messages.len(),
        );
        Ok(())
    }

    fn handle_session_command(
        &mut self,
        action: Option<&str>,
        target: Option<&str>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let (report, changed) = self.session_command_report(action, target)?;
        println!("{report}");
        Ok(changed)
    }

    pub(crate) fn session_command_report(
        &mut self,
        action: Option<&str>,
        target: Option<&str>,
    ) -> Result<(String, bool), Box<dyn std::error::Error>> {
        match action {
            None | Some("list") => Ok((render_session_list(&self.session.id)?, false)),
            Some("switch") => {
                let Some(target) = target else {
                    return Ok(("Usage: /session switch <session-id>".to_string(), false));
                };
                let handle = resolve_session_reference(target)?;
                let session = Session::load_from_path(&handle.path)?;
                let message_count = session.messages.len();
                let session_id = session.session_id.clone();
                let runtime = self.build_runtime(
                    session,
                    &handle.id,
                    self.model.clone(),
                    self.system_prompt.clone(),
                    true,
                    true,
                    self.allowed_tools.clone(),
                    self.permission_mode,
                    None,
                )?;
                self.replace_runtime(runtime)?;
                // Another session's transcript, so its last turn's cache split
                // is not this model's to show.
                self.disown_hud_cache_split();
                self.session = SessionHandle {
                    id: session_id,
                    path: handle.path,
                };
                super::live_cli::scope_todo_store_to_session(&self.session.path, false);
                self.refresh_agent_manifest_scope();
                Ok((
                    format!(
                        "Session switched\n  Active session   {}\n  File             {}\n  Messages         {}",
                        self.session.id,
                        self.session.path.display(),
                        message_count,
                    ),
                    true,
                ))
            }
            Some("fork") => {
                let forked = self.runtime.fork_session(target.map(ToOwned::to_owned));
                let parent_session_id = self.session.id.clone();
                let handle = create_managed_session_handle(&forked.session_id, self.session_scope)?;
                let branch_name = forked
                    .fork
                    .as_ref()
                    .and_then(|fork| fork.branch_name.clone());
                let forked = forked.with_persistence_path(handle.path.clone());
                let message_count = forked.messages.len();
                forked.save_to_path(&handle.path)?;
                let runtime = self.build_runtime(
                    forked,
                    &handle.id,
                    self.model.clone(),
                    self.system_prompt.clone(),
                    true,
                    true,
                    self.allowed_tools.clone(),
                    self.permission_mode,
                    None,
                )?;
                self.replace_runtime(runtime)?;
                self.session = handle;
                super::live_cli::scope_todo_store_to_session(&self.session.path, false);
                self.refresh_agent_manifest_scope();
                Ok((
                    format!(
                        "Session forked\n  Parent session   {}\n  Active session   {}\n  Branch           {}\n  File             {}\n  Messages         {}",
                        parent_session_id,
                        self.session.id,
                        branch_name.as_deref().unwrap_or("(unnamed)"),
                        self.session.path.display(),
                        message_count,
                    ),
                    true,
                ))
            }
            Some(other) => Ok((
                format!(
                    "Unknown /session action '{other}'. Use /session list, /session switch <session-id>, or /session fork [branch-name]."
                ),
                false,
            )),
        }
    }

    fn handle_plugins_command(
        &mut self,
        action: Option<&str>,
        target: Option<&str>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        let loader = ConfigLoader::default_for(&cwd);
        let runtime_config = loader.load()?;
        let mut manager = crate::build_plugin_manager(&cwd, &loader, &runtime_config);
        let result = handle_plugins_slash_command(action, target, &mut manager)?;
        println!("{}", result.message);
        if result.reload_runtime {
            self.reload_runtime_features()?;
        }
        Ok(false)
    }

    fn reload_runtime_features(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let runtime = self.build_runtime(
            self.runtime.session().clone(),
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        self.persist_session()
    }

    fn compact(&mut self, instructions: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
        let report = self.compact_report(instructions)?;
        println!("{report}");
        Ok(())
    }

    pub(crate) fn compact_report(
        &mut self,
        instructions: Option<String>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // `/compact <focus>` threads the user's focus directive into the API
        // summary request so the high-quality 8-section summary preserves detail
        // about the focus above all else (Claude Code "Compact Instructions"
        // parity). Both bare and focused compaction take the API-first path with
        // a deterministic local fallback; only the focus string differs. This
        // replaces the old branch that routed `/compact <focus>` straight to the
        // local extractor — which made the summary *worse* the more specific the
        // request got.
        let config = CompactionConfig::default();
        let focus = instructions
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let result = self.runtime.compact(config, focus.as_deref());
        let removed = result.removed_message_count;
        let kept = result.compacted_session.messages.len();
        let skipped = removed == 0;
        // Apply the compaction in place on the LIVE runtime rather than
        // rebuilding it: `build_runtime` + `replace_runtime` re-spawned
        // LSP/MCP/plugins and tore down the old runtime synchronously, which
        // compaction never needs (nothing on disk changed). See
        // `ConversationRuntime::apply_manual_compaction`.
        self.runtime.apply_manual_compaction(result);
        self.persist_session()?;
        Ok(format_compact_report(removed, kept, skipped))
    }

    /// Streaming sibling of [`Self::compact_report`] for the interactive
    /// `/compact` command. Both bare `/compact` and `/compact <focus>` run an
    /// API-backed summary routed through the async client so the network
    /// round-trip await-suspends instead of blocking the drive-loop task (which
    /// froze the spinner, reveal and input for the whole summary stream). A
    /// `/compact <focus>` directive is threaded into that summary request so the
    /// summary prioritizes the focus, with the deterministic local summarizer
    /// kept only as the API-failure fallback. The rebuild-and-replace tail is
    /// identical to [`Self::compact_report`].
    pub(crate) async fn compact_report_streaming(
        &mut self,
        instructions: Option<String>,
        render_tx: &tokio::sync::mpsc::Sender<runtime::message_stream::RenderBlock>,
        id_gen: &runtime::message_stream::BlockIdGen,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let config = CompactionConfig::default();
        let focus = instructions
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // Install a fresh async client so the summary round-trip await-suspends
        // rather than blocking this task; the focus (if any) rides into the
        // summary system prompt.
        self.ensure_async_api_client();
        let result = self
            .runtime
            .compact_streaming(config, render_tx, id_gen, focus.as_deref())
            .await;
        let removed = result.removed_message_count;
        let kept = result.compacted_session.messages.len();
        let skipped = removed == 0;
        // Apply in place on the LIVE runtime: this is the post-summary "second
        // freeze" fix. The former `build_runtime` + `replace_runtime` re-spawned
        // LSP servers (up to 5s each) and tore down the old runtime's LSP/MCP
        // synchronously, blocking this drive-loop task after the (already-async)
        // summary. The in-place swap is pure in-memory mutation.
        self.runtime.apply_manual_compaction(result);
        self.persist_session()?;
        Ok(format_compact_report(removed, kept, skipped))
    }
}

/// Base/priority pair owned by the API capability catalog.
fn fast_variant_pair(model: &str) -> Option<(String, String)> {
    api::openai_fast_variant_pair(model)
}

fn run_pr_comments(pr_ref: Option<&str>) -> Result<String, io::Error> {
    // Auto-detect PR number from current branch if not specified
    let pr_number = if let Some(n) = pr_ref {
        n.to_string()
    } else {
        let out = Command::new("gh")
            .args(["pr", "view", "--json", "number", "-q", ".number"])
            .output()
            .map_err(io::Error::other)?;
        if !out.status.success() {
            return Ok("pr-comments\n  Error            no PR found for current branch\n  Hint             /pr-comments <number> or push branch first".to_string());
        }
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    let out = Command::new("gh")
        .args(["pr", "view", &pr_number, "--comments"])
        .output()
        .map_err(io::Error::other)?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Ok(format!(
            "pr-comments #{pr_number}\n  Error            {}",
            stderr.lines().next().unwrap_or("unknown")
        ));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        return Ok(format!(
            "pr-comments #{pr_number}\n  Result           no comments"
        ));
    }

    Ok(format!("pr-comments #{pr_number}\n{stdout}"))
}

#[cfg(test)]
mod tests;
