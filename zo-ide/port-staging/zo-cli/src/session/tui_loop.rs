//! TUI integration loop driving `ConversationRuntime::run_turn_streaming`.
//!
//! L7c wired the one-turn TUI path. L8 lifts terminal and app lifetime
//! to the whole interactive session so multiple turns run inside one
//! persistent alternate-screen session.

#[cfg(test)]
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime};

use api::{ProviderKind, context_window_for_model, detect_provider_kind, resolve_model_alias};
use commands::{RemoteAction, SlashCommand};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use runtime::message_stream::{ActiveModel, BlockIdGen, RenderBlock, SystemLevel};
use runtime::{ContentBlock, TurnSummary};
use tokio::sync::mpsc;

use super::LiveCli;
use crate::formatting::{format_auto_compaction_notice, report_row};
use crate::remote_control::{PromptMode, RemoteInbox, RemoteManager, TurnPhase};
#[cfg(test)]
use zo_cli::tui::TodoChecklistStatus;
use zo_cli::tui::modals::{DeepTierView, RemoteOnboardingView, RemotePendingPair};
use zo_cli::tui::app::{
    AppAction, ClipboardCopyTarget, ScheduledWakeHud, WakeSource,
};
use zo_cli::tui::hud::{
    McpHudStatus, SessionIdentity, load_todo_items_for_hud, todo_store_path_for_hud,
};
use zo_cli::tui::stderr_redirect::{self, StderrRedirectGuard};
use zo_cli::tui::{
    AgentCommand, AgentTaskSummary, App, HudState, LspStatusItem, PermissionMode, SecurityPosture,
    TerminalMode, Theme, TodoChecklistItem,
};
use tools::{request_foreground_workflow_cancel, stop_running_agents_since_for_strict_session};

use tools::AgentCompletion;

use super::agent_notice::{
    agent_completion_is_agent_message, agent_completion_is_auth_failure,
    agent_completion_is_internal,
    agent_completion_is_rate_limit_failure, coalesce_agent_result_messages,
    format_agent_completion, queue_agent_message, reinject_background_agent_completion,
    suppress_mismatched_background_task_completion,
};
use super::live_cli_commands::{
    ClipboardWrite, copy_payload, handle_clipboard_paste, start_clipboard_write,
};
use super::freshness::{FreshnessDomain, FreshnessWatcher, SessionFreshness};
use super::slash_dispatch::{
    handle_persistent_slash, handle_ship_at, push_report, seed_transcript_from_session,
};
use super::turn_controller::run_live_turn_with_images;
use super::turn_harness::{AutomationPermissionGate, DeepGateRestore, TurnHarness};

/// Bounded channel capacity for `RenderBlock`s flowing from the agent
/// task to the TUI. Keep this comfortably above one provider/SSE burst so
/// terminal draw cost does not immediately backpressure the awaited parser send
/// and pause reqwest body reads. The bound still prevents unbounded growth if
/// the TUI is genuinely wedged.
const RENDER_CHANNEL_CAPACITY: usize = 1024;

/// Bounded channel capacity for `AgentCommand`s flowing from the TUI
/// back to the agent task.
const COMMAND_CHANNEL_CAPACITY: usize = 8;

fn session_start_payload(session_id: &str, cwd: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "source": "startup",
        "cwd": cwd,
        "session_id": session_id,
    })
}

fn session_end_payload(session_id: &str, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "reason": reason,
    })
}

/// Build the transcript image blocks that echo the images attached to a
/// just-submitted user message.
///
/// Pasted images are held as base64 (ready for the Anthropic API), but
/// [`RenderBlock::Image`] renders raw bytes, so each payload is base64-decoded
/// here. An image whose payload is not valid base64 is skipped — it still
/// reached the model via `run_live_turn_with_images`, but bytes we cannot
/// decode cannot be drawn. Kept pure (modulo the id counter) so the echo logic
/// is unit-tested without driving the event loop.
fn attached_image_blocks(images: &[(String, String)], ids: &BlockIdGen) -> Vec<RenderBlock> {
    use base64::Engine as _;
    images
        .iter()
        .filter_map(|(media_type, b64)| {
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .ok()
                .map(|data| RenderBlock::Image {
                    id: ids.next(),
                    data,
                    media_type: media_type.clone(),
                })
        })
        .collect()
}

fn reseed_transcript_after_auto_compaction(
    app: &mut App,
    ids: &BlockIdGen,
    session: &runtime::Session,
    removed_message_count: usize,
) {
    if app.terminal_mode().is_inline() {
        // Native scrollback is append-only. The completed turns were already
        // emitted at their ownership-moving `end_turn` seams, so rebuilding the
        // compacted session here would print those turns a second time.
        push_report(
            app,
            ids,
            SystemLevel::Info,
            format_auto_compaction_notice(removed_message_count),
        );
        return;
    }
    app.clear_transcript();
    seed_transcript_from_session(app, ids, session);
    push_report(
        app,
        ids,
        SystemLevel::Info,
        format_auto_compaction_notice(removed_message_count),
    );
}

/// Errors surfaced by the TUI driver loop.
#[derive(Debug, thiserror::Error)]
pub enum TuiLoopError {
    /// Terminal backend or event-stream I/O failure.
    #[error("terminal io: {0}")]
    Io(#[from] io::Error),

    /// Failed to construct or draw the [`App`].
    #[error("tui: {0}")]
    Tui(String),

    /// The agent task surfaced a streaming-turn error.
    #[error("turn: {0}")]
    Turn(String),
}

impl From<zo_cli::tui::TuiError> for TuiLoopError {
    fn from(error: zo_cli::tui::TuiError) -> Self {
        Self::Tui(error.to_string())
    }
}

/// A single in-flight clipboard write.
///
/// Each TUI loop owns one of these at a time. Repeated copy actions while the
/// native helper is still running are intentionally ignored, preventing a burst
/// of clicks from queuing `pbcopy` processes and repaint work behind one another.
pub(crate) struct PendingClipboardCopy {
    write: ClipboardWrite,
}

impl PendingClipboardCopy {
    pub(crate) fn new(text: String) -> Self {
        Self {
            write: start_clipboard_write(text),
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.write.is_ready()
    }

    pub(crate) async fn wait_until_ready(&mut self) {
        self.write.wait_until_ready().await;
    }

    /// Report only what the user cannot infer: a *failed* copy.
    ///
    /// A successful copy is already evident from the clipboard itself, so it
    /// stays off the transcript — the confirmation line was pure noise on a
    /// surface reserved for the conversation.
    pub(crate) fn finish(self) -> Option<String> {
        self.write.finish().err().map(|error| {
            format!(
                "Clipboard error: {error}. Use your terminal's mouse-selection override and copy shortcut."
            )
        })
    }
}

/// Outcome of one streaming turn.
#[derive(Debug)]
pub struct TurnOutcome {
    /// `Some` if the turn completed normally; `None` if the user
    /// cancelled or the loop was terminated.
    pub summary: Option<TurnSummary>,
}

/// Resolve the boot theme.
///
/// [`Theme::pi`] is the source of truth. `.zo/design/tokens.json` (relative to
/// the session cwd) overrides it when present — it lets operators retune roles
/// without recompiling — and the loader treats that file as a *partial*
/// override, so any key it omits falls back to the built-in palette rather than
/// to black.
///
/// The path is `.zo/`, not `.forge/`. A `.forge/design/tokens.json` left over
/// from before the rename is silently ignored, which is the intended outcome:
/// its palette predates the Pi one and adopting it would quietly undo the theme
/// the binary ships with. Move such a file to `.zo/design/` only after porting
/// its values.
pub(crate) fn boot_theme(terminal_background: Option<(u8, u8, u8)>) -> Theme {
    let tokens = crate::current_cli_cwd()
        .unwrap_or_default()
        .join(".zo")
        .join("design")
        .join("tokens.json");
    if tokens.is_file() {
        if let Ok(theme) = Theme::load_for_terminal(&tokens, terminal_background) {
            return theme;
        }
    }
    // No tokens file: the built-in palette must still honor the terminal's
    // color policy — most importantly, a non-empty `NO_COLOR` neutralizes the
    // palette on this fallback boot path too, not only when a tokens file
    // happens to exist.
    Theme::pi().for_current_terminal_with_background(terminal_background)
}

/// Load input history + slash/mention frecency into `app`. Slash commands and
/// prompt history live in the per-user data dir (the same across projects);
/// `@`-file mention frecency is project-scoped (under `<project>/.zo/`) so a
/// global store cannot pollute ranking across unrelated workspaces. All three
/// hints/recorders are inert until attached, so this is the single wiring point
/// shared by the local REPL and the remote-attach TUI.
pub(crate) fn load_input_frecency(app: &mut App, project_cwd: &std::path::Path) {
    if let Some(home) = std::env::var_os("HOME") {
        let data_dir = std::path::PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("zo-cli");
        if let Ok(history) = zo_cli::tui::History::load(data_dir.join("history.jsonl")) {
            app.set_history(history);
        }
        if let Ok(command_history) =
            zo_cli::tui::CommandHistory::load(data_dir.join("command-history.jsonl"))
        {
            app.set_command_history(command_history);
        }
    }
    let mention_history_path = project_cwd.join(".zo").join("mention-history.jsonl");
    if let Ok(mention_history) = zo_cli::tui::CommandHistory::load(&mention_history_path) {
        app.set_mention_history(mention_history);
    }
}

fn flush_inline_transcript_at_shutdown<B: ratatui::backend::Backend>(
    app: &mut App,
    terminal: &mut ratatui::Terminal<B>,
    result: &mut Result<(), TuiLoopError>,
) where
    B::Error: std::fmt::Display,
{
    app.finalize_inline_transcript();
    // `insert_before` may move an inline viewport's absolute screen origin.
    // Shutdown does not need another composed frame after that move: restore
    // clears the live viewport immediately. Emitting the queue directly keeps
    // the temporary zero-origin insertion buffer and the absolute viewport
    // buffer in separate operations instead of re-entering the full renderer
    // between them.
    if let Err(error) = app.flush_finalized_inline_transcript(terminal) {
        if result.is_ok() {
            *result = Err(error.into());
        }
    }
}

/// Boot-phase wall-clock timings captured in `run_repl` and carried into the
/// TUI loop, where the one-line `[boot]` summary is emitted only *after* the
/// stderr redirect is live (an earlier `eprintln!` would paint over the
/// terminal instead of landing in `~/.zo/logs/zo.log`).
///
/// `started` anchors the total; the two `Duration`s are the notable pre-TUI
/// sub-phases and need not sum to the total — the runtime build also spans
/// resume/checkpoint work the summary does not itemize, and the total includes
/// the tokio-runtime build between the two capture points.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BootPhaseTimings {
    pub started: Instant,
    pub runtime_build: Duration,
    pub status_context: Duration,
}

/// Format the single boot-timing log line. Pure (no clock reads, no output) so
/// it is unit-tested directly; the caller supplies `to_first_frame` / `total` from
/// its own [`Instant`]s and emits the result via `eprintln!` once stderr is
/// redirected to the log file.
fn format_boot_timing_line(
    runtime_build: Duration,
    status_context: Duration,
    to_first_frame: Duration,
    total: Duration,
) -> String {
    format!(
        "[boot] runtime_build={}ms status_context={}ms to_first_frame={}ms total={}ms",
        runtime_build.as_millis(),
        status_context.as_millis(),
        to_first_frame.as_millis(),
        total.as_millis(),
    )
}

/// Drive the full interactive REPL inside one persistent TUI session.
// Top-level orchestration entry point (channels, terminal init, watchdog, event
// loop) already at the line limit; the fail-open background probe adds one
// necessary line, and splitting the cohesive setup to save it would not clarify.
#[expect(clippy::too_many_lines)]
pub async fn run_repl_session(
    cli: &mut LiveCli,
    startup_elapsed: Duration,
    terminal_mode: TerminalMode,
    // Boot-time workspace snapshot for the startup screen, gathered by
    // `run_repl` under the startup loader — building it here would fork `git`
    // and re-walk the config chain on the TUI thread's first frames.
    startup_status: Option<crate::StatusContext>,
    boot: BootPhaseTimings,
) -> Result<(), TuiLoopError> {
    // Anchor `to_first_frame` at the loop's entry so it measures the TUI-side
    // latency (terminal init + app build + first draw) distinctly from the
    // pre-TUI phases `boot` already carries.
    let repl_entry = Instant::now();
    // Persist this session's harness attest evidence on EVERY exit route
    // (quit, error, panic) — `/refine` aggregates these rows across sessions.
    let _attest_evidence = crate::session::refine::AttestPersistGuard::new(cli.session.id.clone());
    let (render_tx, render_rx) = mpsc::channel::<RenderBlock>(RENDER_CHANNEL_CAPACITY);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<AgentCommand>(COMMAND_CHANNEL_CAPACITY);
    let mut agent_rx = tools::register_agent_completion_channel();
    let freshness_watcher = FreshnessWatcher::start(&cli.cwd);
    let freshness = SessionFreshness::new(&freshness_watcher, &cli.cwd);
    // Capture the pre-session git baseline before the terminal (and the freeze
    // watchdog) exist: the snapshot forks `git`, and on a slow or contended
    // repo that stall would otherwise land on the TUI loop's first beats. The
    // Changes panel diffs against this so it only shows files modified during
    // this session, not pre-existing dirt.
    let sidebar_baseline = crate::current_cli_cwd().ok().and_then(|cwd| {
        let _ = freshness.begin_scan(FreshnessDomain::Workspace, Instant::now());
        freshness
            .workspace_status()
            .snapshot(&cwd, Arc::new(AtomicBool::new(false)))
            .ok()
    });
    // Fail-open background probe before raw mode: any failure keeps the existing theme.
    let terminal_background = zo_cli::tui::term::detect_background();
    // `_stderr_guard` must stay alive for the whole session so stderr remains
    // redirected to the log file. After `restore_terminal`, it drops naturally
    // and restores the original stderr fd.
    let (mut terminal, _stderr_guard) = init_terminal(terminal_mode)?;
    set_terminal_session_title(&mut terminal, cli.runtime.session().name.as_deref());
    // Stderr now points at the log file, so the freeze watchdog's verdict lands
    // in `~/.zo/logs/zo.log`. Spawn it once the terminal (and the
    // redirect) are live.
    spawn_freeze_watchdog();
    let theme = boot_theme(terminal_background);
    let ids = BlockIdGen::default();
    let mut app = App::new(theme, render_rx, cmd_tx.clone());
    app.set_terminal_mode(terminal_mode);
    let mut remote = RemoteManager::new(cmd_tx, render_tx.clone(), ids.clone());
    app.set_agent_manifest_started_after(cli.agent_manifest_started_after);
    app.set_agent_manifest_session_id(cli.session.id.clone());
    // This REPL consumes the agent-completion channel (`agent_rx` above) and
    // re-injects a detached agent's result as a fresh turn, so an `Agent` call
    // that omits `background` may default to detached here — the user keeps
    // chatting while the agent runs. Headless `-p`, `serve`, and spawned
    // sub-agents never register this consumer, so their contexts keep the
    // blocking default and no result is ever silently lost.
    if let Some(runtime) = cli.runtime.runtime.as_mut() {
        runtime
            .tool_executor_mut()
            .tool_registry_mut()
            .context()
            .set_background_agent_default(true);
    }
    install_status_line_poller(cli, &mut app);
    install_scheduled_wakeup_poller(cli, &mut app, freshness.clone());
    install_workspace_status_poller(&mut app, freshness.clone());

    // 부트 시점에 세션이 이미 메시지를 갖고 있으면(현재 유일 경로: `/restart`의
    // ZO_RESTART_RESUME 핸드오프가 run_repl에서 resume_session_fast로 스왑)
    // 트랜스크립트를 시딩한다. 다른 resume 표면(/resume·SelectSession·rewind)은
    // 전부 디스패치 시점에 명시 시딩하지만 부트 경로만 이게 없어서, 재시작 후
    // 대화는 이어지는데 화면은 빈 트랜스크립트로 뜨는 UX 구멍이 있었다.
    if !cli.runtime.session().messages.is_empty() {
        seed_transcript_from_session(&mut app, &ids, cli.runtime.session());
    }

    // Interactive default: reactive auto-verify (DeepMode::Reactive). Say
    // "improve X" and the harness runs the turn normally, then — only if it
    // edited files — verifies the diff with the adversarial verifier and retries
    // on failure, one-shot. Do not auto-detect a heavyweight project command here:
    // `cargo test` before or immediately after ordinary chat made the first output
    // feel frozen. Users can opt into an objective command with `/auto <command>`.
    // `/auto off` disables it; `ZO_AUTO_VERIFY=0` opts out at startup. Headless
    // `-p`, `serve`, and spawned sub-agents never go through here.
    let auto_opt_out = std::env::var("ZO_AUTO_VERIFY")
        .map(|value| value == "0" || value.eq_ignore_ascii_case("off"))
        .unwrap_or(false);
    if !auto_opt_out && cli.runtime.deep_gate().is_none() {
        cli.runtime.set_deep_gate(Some(runtime::DeepGateConfig {
            mode: runtime::DeepMode::Reactive,
            check_command: None,
            max_attempts: 2,
        }));
    }

    // Root the runtime's durable traces (`.zo/dream/`, `.zo/turns/`) at the
    // session's stable workspace, not the live process cwd. `EnterWorktree`
    // chdirs the process mid-session; without this, trace producers would write
    // into the worktree's `.zo/` while the auto-dream consumer reads
    // `cli.cwd`, silently breaking the loop.
    cli.runtime.set_workspace_cwd(cli.cwd.clone());

    // Load input history + slash/mention frecency. Shared with the remote-attach
    // TUI so both interactive paths float recently-used commands/files.
    load_input_frecency(&mut app, &cli.cwd);

    app.enable_input();
    app.set_startup_screen(cli.startup_screen(Some(startup_elapsed), startup_status.as_ref()));
    if let Some(snapshot) = &sidebar_baseline {
        app.capture_sidebar_baseline(snapshot);
    }
    sync_app_context(cli, &mut app);
    // One-time (per user, marker-file backed) smart-AUTO default banner: shown
    // only when `smart.enabled` is absent from `settings.json` — i.e. smart is
    // on via the new default rather than an explicit choice. Injected through
    // the same System-notice path every other boot report uses, and only on
    // this interactive REPL boot — headless `-p`, `serve`, and spawned
    // sub-agents never enter this function, so they stay quiet.
    if let Some(banner) = crate::session::smart_settings::smart_default_banner_notice() {
        push_report(&mut app, &ids, SystemLevel::Info, banner);
    }
    // Resume history and boot notices stay in the live transcript so they sit
    // directly above the composer; the per-frame overflow seal moves only what
    // does not fit into native scrollback. The old full drain here emptied the
    // viewport on every resumed session.
    app.draw_frame(&mut terminal)?;

    // One boot-timing summary, emitted here because the stderr redirect (inside
    // `init_terminal` above) is now live, so the line lands in `~/.zo/logs/zo.log`
    // rather than painting over the freshly drawn first frame. This is the first
    // moment `to_first_frame` and `total` are both known.
    eprintln!(
        "{}",
        format_boot_timing_line(
            boot.runtime_build,
            boot.status_context,
            repl_entry.elapsed(),
            boot.started.elapsed(),
        )
    );

    // CC parity: SessionStart fires once per interactive session, before the
    // first prompt is accepted (and SessionEnd below, after the loop exits).
    // Use the explicit Option instead of BuiltRuntime's Deref: the inner runtime
    // can be taken during interactive shutdown flows (for example `/resume`).
    if let Some(ref rt) = cli.runtime.runtime {
        let cwd = crate::current_cli_cwd()
            .ok()
            .map(|cwd| cwd.display().to_string());
        let payload = session_start_payload(&cli.session.id, cwd.as_deref());
        rt.fire_lifecycle_hook(runtime::HookEvent::SessionStart, &payload);
    }

    // Box::pin: the session-loop future carries the whole turn state machine
    // and sits just past clippy's large-futures threshold; one heap pin per
    // session keeps it off the caller's stack frame.
    let mut result = Box::pin(run_session_loop(
        cli,
        &mut app,
        &mut terminal,
        SessionLoopChannels {
            render_tx: &render_tx,
            cmd_rx: &mut cmd_rx,
            agent_rx: &mut agent_rx,
        },
        &mut remote,
        &ids,
        &freshness,
    ))
    .await;
    app.set_render_observer(None);
    if remote.is_active() {
        let _ = remote.stop().await;
    }

    if terminal_mode.is_inline() {
        flush_inline_transcript_at_shutdown(&mut app, &mut terminal, &mut result);
    }

    // Single TUI SessionEnd site by construction: all in-loop quit paths return
    // here after finish_tui_session performs its non-hook teardown/persistence.
    // Keep this before terminal restore so hooks still observe the live TUI end.
    if let Some(ref rt) = cli.runtime.runtime {
        let payload = session_end_payload(
            &cli.session.id,
            if result.is_ok() { "exit" } else { "error" },
        );
        rt.fire_lifecycle_hook(runtime::HookEvent::SessionEnd, &payload);
    }

    let restore_result = restore_terminal(&mut terminal, terminal_mode);
    match (result, restore_result) {
        (Err(err), _) | (Ok(()), Err(err)) => Err(err),
        (Ok(()), Ok(())) => Ok(()),
    }
}

struct SessionLoopChannels<'a> {
    render_tx: &'a mpsc::Sender<RenderBlock>,
    cmd_rx: &'a mut mpsc::Receiver<AgentCommand>,
    agent_rx: &'a mut mpsc::UnboundedReceiver<AgentCompletion>,
}

/// An OAuth sign-in still waiting on the browser, and the handle needed to
/// retire it when the user starts another one. Held only so a second `/login`
/// can free the callback port the first one owns.
struct PendingLogin {
    cancel: crate::auth::LoginCancel,
    handle: std::thread::JoinHandle<()>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubmissionOrigin {
    Local,
    Remote,
}

fn dispatches_local_slash(origin: SubmissionOrigin, input: &str) -> bool {
    origin == SubmissionOrigin::Local && input.starts_with('/')
}

#[allow(clippy::too_many_lines)] // cohesive async session event loop
async fn run_session_loop(
    cli: &mut LiveCli,
    app: &mut App,
    terminal: &mut TuiTerminal,
    channels: SessionLoopChannels<'_>,
    remote: &mut RemoteManager,
    ids: &BlockIdGen,
    freshness: &SessionFreshness,
) -> Result<(), TuiLoopError> {
    let SessionLoopChannels {
        render_tx,
        cmd_rx,
        agent_rx,
    } = channels;
    // Keep one terminal input reader for the entire session. Dropping a pending
    // crossterm EventStream wakes its background reader without joining it, so
    // recreating one whenever an outer select arm wins can race the replacement.
    let mut events = crossterm::event::EventStream::new();
    // The turn-0 rewind checkpoint used to be captured here; it forks `git`
    // and stalled the loop's first beats, so `run_repl` now captures it under
    // the startup loader before the TUI exists.

    // Pin the running binary's on-disk identity NOW, before any redeploy can
    // race the first HUD build. Later HUD builds compare against this baseline
    // and surface a `/restart` warning once a new build lands on disk.
    zo_cli::tui::stale_binary::init();

    // Connect MCP servers OFF the startup path: discovery (which can stall on a
    // slow `npx`/remote server) runs on a background thread and splices each
    // server's tools into the live registry as it finishes, so the TUI is
    // interactive immediately instead of blocking on the discovery budget.
    cli.start_mcp_discovery_in_background();

    // Subscribe this session to the team inbox `digest` channel so an autonomous
    // loop's overnight notices surface in the morning first-turn digest injection
    // (which only considers joined channels). Best-effort and fail-open: no store
    // yet means nothing to join, so a session that never touches the inbox pays
    // nothing.
    cli.seed_digest_subscription();

    let mut auth_failure_reported = false;
    let mut rate_limit_failure_reported = false;
    // Post-turn verify warnings (independent / competing-hypotheses) are produced
    // on spawned tasks so the lens LLM round-trips never block the input pump; the
    // `verify_rx` arm of the idle `select!` below receives a finished warning and
    // pushes it once `run` is dropped and `app` is borrowable. Bounded so a burst
    // of rapid turns cannot grow it without limit.
    let (verify_tx, mut verify_rx) = mpsc::channel::<String>(8);
    // `/login <provider>` runs its browser round-trip on a plain thread (no
    // ambient runtime, so the flow's own `block_on` stays safe) and reports the
    // outcome here. The provider is carried alongside the result because the
    // report names it and the thread outlives the dispatch that started it.
    // One slot: a second sign-in cannot start while one holds the callback port.
    let (login_tx, mut login_rx) = mpsc::channel::<(String, Result<(), String>)>(1);
    // The sign-in that currently owns the loopback callback port, if any. A
    // second `/login` cancels it rather than failing on "callback port is
    // already in use" — an abandoned browser round-trip otherwise held the port
    // for the full three-minute deadline.
    let mut pending_login: Option<PendingLogin> = None;
    // A single slot deliberately collapses repeated copy actions until the
    // native helper has completed. Never replace a live operation: dropping its
    // JoinHandle would detach an already-running `pbcopy` process.
    let mut clipboard_write: Option<PendingClipboardCopy> = None;
    // Tracks whether the slash-command menu was open last iteration, so file-based
    // commands are re-scanned once on the rising edge (see below) rather than on
    // every keystroke.
    let mut slash_menu_open = false;

    loop {
        if clipboard_write
            .as_ref()
            .is_some_and(PendingClipboardCopy::is_ready)
        {
            let failure = clipboard_write
                .take()
                .expect("ready clipboard write must still be present")
                .finish();
            if let Some(text) = failure {
                push_report(app, ids, SystemLevel::Error, text);
                app.draw_frame(terminal)?;
            }
        }
        while let Ok(completion) = agent_rx.try_recv() {
            process_idle_agent_completion(
                app,
                ids,
                &completion,
                &cli.session.id,
                &mut auth_failure_reported,
                &mut rate_limit_failure_reported,
            );
        }
        app.enable_input();
        queue_due_loop_prompts(cli, app, ids);
        queue_due_wakeup_prompts(cli, app, ids);
        // File-based slash commands (`.zo/commands/*.md`) are discovered once at
        // runtime build. Re-scan the moment the slash menu opens so a command
        // added mid-session shows up without `/reload`. Rising-edge only, so
        // there is no disk I/O while idle or typing an ordinary message.
        let slash_menu_now = app
            .input()
            .lines()
            .first()
            .is_some_and(|line| line.trim_start().starts_with('/'));
        if slash_menu_now && !slash_menu_open {
            cli.runtime.prompt_commands = commands::discover_prompt_commands(&cli.cwd);
        }
        slash_menu_open = slash_menu_now;
        sync_app_context(cli, app);
        // Auto-submit any messages queued while input was disabled. The
        // user typed these mid-turn (Enter while a turn was in-flight), or a
        // session-local `/goal`/`/loop` controller queued follow-up work. Surface
        // them as their own Submit turns in FIFO order so automation actually
        // drains instead of waiting for a manual keystroke.
        let mut drain_due_after_wait = false;
        // The loop id of a `/loop`-owned run dispatched THIS iteration, so its
        // completed turn can charge output tokens against the loop's optional
        // `--token-budget` (mirrors how `goal_turn_pending` carries goal ownership).
        let mut loop_turn_id: Option<String> = None;
        // A background-agent completion that woke the idle `select!` below. The
        // arm consumes it from the single-consumer channel, so it cannot be left
        // for the loop-top drain — it is processed once after the `select!`
        // returns (when the `run`/timer futures are dropped and `app` is free).
        let mut woke_completion: Option<AgentCompletion> = None;
        // A spawned post-turn verify panel's warning that woke the idle `select!`.
        let mut woke_verify: Option<String> = None;
        // A background `/login` that finished while the prompt sat idle.
        let mut woke_login: Option<(String, Result<(), String>)> = None;
        // A remote prompt or local-only pairing notice that woke the idle prompt.
        // It is handled only after `app.run` is dropped so the App borrow is free.
        let mut woke_remote: Option<RemoteInbox> = None;
        // Preserve the source through submit dispatch: remote text may be sent
        // to the model, but it must never enter local-only slash handling.
        let mut submission_origin = SubmissionOrigin::Local;
        // Whether a submission this iteration is the HUMAN speaking (direct
        // idle-prompt input always is; a queued message only if it is neither
        // goal-owned, loop-owned, nor an agent-result reinjection). A user
        // submission acknowledges the goal's unattended checkpoint — an
        // actively-supervised goal never auto-pauses.
        let mut user_originated_submit = true;
        let mut action = if let Some(queued) = app.pop_next_queued_message() {
            // A batch of background completions that queued during the previous
            // turn pops as ONE combined follow-up turn. Without this, N
            // completions dispatch N consecutive near-identical alarm turns,
            // each re-answered by the model. Only agent-result entries fold;
            // user/goal/loop messages keep their FIFO slots.
            let queued = if queued.agent_result.is_some() {
                coalesce_agent_result_messages(queued, app.drain_queued_agent_results())
            } else {
                queued
            };
            user_originated_submit =
                !queued.goal_owned && queued.loop_id.is_none() && queued.agent_result.is_none();
            // Latch goal ownership for THIS turn from the popped message's tag,
            // overriding any dispatch-time latch. A user message typed ahead of a
            // goal prompt pops first (FIFO) with `goal_owned = false`, so it can
            // no longer consume the goal's verifier verdict; the goal prompt pops
            // later with `goal_owned = true` and is attributed correctly.
            cli.goal_turn_pending = queued.goal_owned;
            // Loop pop-gate: a `/loop`-owned run that was stopped/paused (or whose
            // fixed-count budget is spent) after it was queued is dropped here,
            // without dispatching a turn — what makes `/loop` stoppable mid-flight
            // instead of fire-and-forget. A non-loop message has `loop_id = None`.
            if let Some(loop_id) = queued.loop_id.as_deref() {
                if cli.begin_loop_turn(loop_id) == super::automation::LoopTurnGate::Skip {
                    cli.goal_turn_pending = false;
                    continue;
                }
                loop_turn_id = Some(loop_id.to_string());
            }
            app.stage_queued_images_for_submit(queued.images);
            app.stage_queued_agent_result_for_submit(queued.agent_result);
            app.draw_frame(terminal)?;
            AppAction::Submit(queued.text)
        } else {
            // Back at the prompt: the badge must show exactly what is still
            // attached. A local slash command submits and clears the composer
            // without consuming pasted images, so without this the image is
            // staged but invisible — and rides the next unrelated turn.
            app.resync_image_badge();
            // CC parity: fire the `Notification` hook once when the prompt
            // sits idle for [`NOTIFICATION_IDLE_SECS`] with no user action.
            // The run future is pinned so polling continues across the timer
            // arm without losing input state; with no Notification hooks
            // configured the fire is a no-op.
            let run = app.run_with_events(terminal, &mut events);
            tokio::pin!(run);
            let idle = tokio::time::sleep(std::time::Duration::from_secs(NOTIFICATION_IDLE_SECS));
            tokio::pin!(idle);
            // Wake the idle prompt at whichever fires first: the next `/loop`
            // interval OR the next due `ScheduleWakeup`. Without folding wakeups
            // in here, a scheduled wakeup would only fire on the next keystroke —
            // the "5분 뒤 alarm never comes" bug. (The arm still keys off
            // `loop_due_in.is_some()`, so a pending wakeup alone arms the timer.)
            let loop_due_in = earliest_due(
                cli.next_loop_due_in(Instant::now()),
                super::wakeups::next_wakeup_due_in(
                    super::wakeups::now_epoch_secs(),
                    Some(&cli.session.id),
                ),
            );
            let loop_due = tokio::time::sleep(loop_due_in.unwrap_or(Duration::from_secs(60 * 60)));
            tokio::pin!(loop_due);
            let mut idle_fired = false;
            loop {
                tokio::select! {
                    // A local action wins when it and a remote prompt become
                    // ready in the same poll. Without `biased`, Tokio rotates
                    // ready branches and remote input can overtake a local Enter.
                    biased;
                    action = &mut run => break action?,
                    // A background agent finished while the prompt sat idle.
                    // `app.run` only returns on a real user action, so without
                    // this arm the result would not surface until the next
                    // keystroke. Stash and break; the result is processed below
                    // (re-injected as a queued follow-up turn) once `run` is
                    // dropped and `app` is borrowable again.
                    maybe = agent_rx.recv() => {
                        woke_completion = maybe;
                        break AppAction::Redraw;
                    }
                    Some(warning) = verify_rx.recv() => {
                        // A spawned post-turn verify panel finished; surface its
                        // warning once `run` is dropped (stash, like the agent arm).
                        woke_verify = Some(warning);
                        break AppAction::Redraw;
                    }
                    Some(outcome) = login_rx.recv() => {
                        // A background `/login` finished. Without this arm the
                        // result would sit in the channel until the next
                        // keystroke, which is exactly the "browser came back but
                        // zo says nothing" report.
                        woke_login = Some(outcome);
                        break AppAction::Redraw;
                    }
                    Some(inbox) = remote.next_inbox() => {
                        // Remote prompts use the bounded session-local inbox;
                        // pairing notices use the render channel so they also
                        // surface during live turns. Stash the prompt until
                        // `app.run` is dropped so the App borrow remains single-owner.
                        woke_remote = Some(inbox);
                        break AppAction::Redraw;
                    }
                    () = async {
                        if let Some(write) = clipboard_write.as_mut() {
                            write.wait_until_ready().await;
                        }
                    }, if clipboard_write.is_some() => {
                        // Only the native helper result crosses this cancellable
                        // arm. `finish` remains below the select, after `run` is
                        // dropped, so OSC 52 output is emitted exactly once.
                        break AppAction::Redraw;
                    }
                    () = &mut loop_due, if loop_due_in.is_some() => {
                        drain_due_after_wait = true;
                        break AppAction::Redraw;
                    }
                    () = &mut idle, if !idle_fired => {
                        idle_fired = true;
                        if let Some(ref rt) = cli.runtime.runtime {
                            rt.fire_lifecycle_hook(
                                runtime::HookEvent::Notification,
                                &serde_json::json!({
                                    "notification_type": "idle",
                                    "idle_seconds": NOTIFICATION_IDLE_SECS,
                                }),
                            );
                        }
                    }
                }
            }
        };
        // A spawned post-turn verify panel's warning woke the idle `select!`:
        // `run` is dropped now, so `app` is borrowable — push it.
        if let Some(warning) = woke_verify.take() {
            push_report(app, ids, SystemLevel::Warn, warning);
            app.draw_frame(terminal)?;
        }
        // A background `/login` landed while the prompt was idle. The credential
        // chain caches per process, so a fresh sign-in only takes effect once the
        // bindings are rebuilt — `sync_app_context` is what makes the new
        // account usable without a restart.
        if let Some((provider, result)) = woke_login.take() {
            let (level, message) =
                crate::session::slash_dispatch::login_outcome_report(&provider, &result);
            app.follow_latest();
            push_report(app, ids, level, message);
            sync_app_context(cli, app);
            // No modal is opened here on purpose. Reopening the provider manager
            // when the sign-in lands means a modal appears by itself long after
            // the user dismissed the one they were in — it reads as a dismissal
            // that did not take, and the modal has to be closed twice. The report
            // above is the outcome; `/providers` shows the refreshed list on
            // demand. (`login` drops the return flag for the same reason.)
            app.draw_frame(terminal)?;
        }
        // Process a completion that woke the idle `select!` (consumed from the
        // channel there, so the loop-top drain won't see it). `run` is dropped,
        // so `app` is free; the resulting queued turn drains on the next pass.
        if let Some(completion) = woke_completion.take() {
            process_idle_agent_completion(
                app,
                ids,
                &completion,
                &cli.session.id,
                &mut auth_failure_reported,
                &mut rate_limit_failure_reported,
            );
        }
        if let Some(inbox) = woke_remote.take() {
            match inbox {
                RemoteInbox::Prompt { text, mode } => match mode {
                    PromptMode::New | PromptMode::Queue => {
                        // The host-side inbox is the queue of record. A remote
                        // queue command accepted during a live turn waits here
                        // and becomes its own turn after all local queued work.
                        user_originated_submit = true;
                        submission_origin = SubmissionOrigin::Remote;
                        action = AppAction::Submit(text);
                    }
                    PromptMode::Steer => {
                        // Steers are routed directly to the live turn's command
                        // channel and must never arrive through the idle inbox.
                        push_report(
                            app,
                            ids,
                            SystemLevel::Warn,
                            "Zo Remote\n  Ignored            stale steer outside a live turn",
                        );
                    }
                },
            }
        }
        if drain_due_after_wait {
            queue_due_loop_prompts(cli, app, ids);
            queue_due_wakeup_prompts(cli, app, ids);
            sync_app_context(cli, app);
        }
        match action {
            AppAction::Quit => {
                finish_tui_session(cli)?;
                return Ok(());
            }
            AppAction::ConnectApiKey { provider, api_key } => {
                use crate::session::slash_dispatch::{ConnectReport, connect_preset_with_api_key};
                app.follow_latest();
                let (level, message) = match connect_preset_with_api_key(&provider, &api_key) {
                    ConnectReport::Info(message) => (SystemLevel::Info, message),
                    ConnectReport::Warn(message) => (SystemLevel::Warn, message),
                    ConnectReport::Error(message) => (SystemLevel::Error, message),
                };
                push_report(app, ids, level, message);
                sync_app_context(cli, app);
                app.draw_frame(terminal)?;
            }
            AppAction::ConnectCustomProvider(draft) => {
                use crate::session::slash_dispatch::{
                    ConnectReport, CustomProviderRequest, ProviderTokenLimits,
                    connect_custom_provider,
                };
                app.follow_latest();
                let (level, message) = match connect_custom_provider(&CustomProviderRequest {
                    name: &draft.name,
                    base_url: &draft.base_url,
                    auth_env: draft.auth_env.as_deref(),
                    api_key: draft.api_key.as_deref(),
                    models: &draft.models,
                    token_limits: ProviderTokenLimits {
                        context_window: draft.context_window,
                        max_output_tokens: draft.max_output_tokens,
                    },
                    include_usage: draft.include_usage,
                    supports_reasoning_effort: draft.supports_reasoning_effort,
                    edit_existing: draft.edit_existing,
                }) {
                    ConnectReport::Info(message) => (SystemLevel::Info, message),
                    ConnectReport::Warn(message) => (SystemLevel::Warn, message),
                    ConnectReport::Error(message) => (SystemLevel::Error, message),
                };
                push_report(app, ids, level, message);
                // Saving a form that the manager opened returns to the list, so
                // several providers can be registered without retyping the
                // command each time.
                if app.take_provider_manager_return() {
                    use crate::session::slash_dispatch::{
                        account_rows, manager_rows, settings_path_display,
                    };
                    match manager_rows() {
                        Ok(rows) => app.open_provider_manager_modal(
                            account_rows(),
                            rows,
                            settings_path_display(),
                        ),
                        Err(error) => push_report(
                            app,
                            ids,
                            SystemLevel::Warn,
                            format!("Providers: failed to re-read settings: {error}"),
                        ),
                    }
                }
                sync_app_context(cli, app);
                app.draw_frame(terminal)?;
            }
            AppAction::ManageProvider(action) => {
                use crate::session::slash_dispatch::{
                    ConnectReport, account_rows, apply_provider_action, edit_prefill, manager_rows,
                };
                use zo_cli::tui::modals::ProviderManagerAction;
                app.follow_latest();
                match &action {
                    // Both hand off to the wizard; the flag brings the user back
                    // to the list once the form resolves either way.
                    // Add opens the full picker — OAuth accounts, cloud presets,
                    // local servers, and the custom wizard — so "add" means the
                    // same thing here as the old bare `/connect` did.
                    ProviderManagerAction::Add => {
                        app.mark_provider_manager_return();
                        crate::session::slash_dispatch::open_add_provider_picker(app);
                    }
                    ProviderManagerAction::Edit { name } => match edit_prefill(name) {
                        Some(prefill) => {
                            app.mark_provider_manager_return();
                            app.open_provider_edit_modal(&prefill);
                        }
                        None => push_report(
                            app,
                            ids,
                            SystemLevel::Warn,
                            format!("{name}: not registered — nothing to edit."),
                        ),
                    },
                    _ => {
                        if let Some(outcome) = apply_provider_action(&action) {
                            let (level, message) = match outcome.report {
                                ConnectReport::Info(message) => (SystemLevel::Info, message),
                                ConnectReport::Warn(message) => (SystemLevel::Warn, message),
                                ConnectReport::Error(message) => (SystemLevel::Error, message),
                            };
                            push_report(app, ids, level, message);
                            // The manager stays open across mutations, so it is
                            // re-listed in place rather than reopened.
                            if outcome.refresh_list {
                                match manager_rows() {
                                    Ok(rows) => {
                                        app.refresh_provider_manager_modal(account_rows(), rows);
                                    }
                                    Err(error) => push_report(
                                        app,
                                        ids,
                                        SystemLevel::Warn,
                                        format!("Providers: failed to re-read settings: {error}"),
                                    ),
                                }
                            }
                        }
                    }
                }
                sync_app_context(cli, app);
                app.draw_frame(terminal)?;
            }
            AppAction::SelectModel(model) => {
                let report = cli.apply_model_change(&model.alias);
                if let Err(err) = cli.persist_session() {
                    eprintln!("[zo] warning: failed to persist session after model change: {err}");
                }
                app.follow_latest();
                push_report(app, ids, SystemLevel::Info, report);
                sync_app_context(cli, app);
                app.draw_frame(terminal)?;
            }
            AppAction::SelectPermission(mode) => {
                app.follow_latest();
                // Switching permission mode swaps the live runtime's permission
                // policy in place (it does not rebuild the runtime). If building
                // the new policy fails, surface a warning and keep the session
                // alive — a failed Shift+Tab cycle must never tear the whole TUI
                // down to the shell — and roll the App plan-gate back below.
                match cli.apply_permission_change(mode.as_str()) {
                    Ok(report) => {
                        // The runtime change committed, so the plan-gate mutation
                        // the Shift+Tab cycle made in `keys.rs` is now valid;
                        // discard its rollback snapshot.
                        app.take_plan_cycle_rollback();
                        // The App's plan flag is authoritative for "user selected
                        // Plan"; Plan enforces read-only, so mirror it onto the
                        // runtime-facing LiveCli flag that drives the per-turn
                        // Plan contract. A plain read-only stop clears it.
                        cli.set_plan_selected(app.plan_mode_active());
                        if let Err(err) = cli.persist_session() {
                            eprintln!("[zo] warning: failed to persist session after permission change: {err}");
                        }
                        push_report(app, ids, SystemLevel::Info, report);
                    }
                    Err(error) => {
                        // Transaction failed: roll the App plan-gate state back to
                        // exactly what it was before the Shift+Tab cycle mutated
                        // it (no-op for paths that did not arm a rollback), so the
                        // UI Plan flag never diverges from the runtime and
                        // `plan_selected` is left unchanged.
                        if let Some(rollback) = app.take_plan_cycle_rollback() {
                            app.restore_plan_mode_snapshot(rollback);
                        }
                        push_report(
                            app,
                            ids,
                            SystemLevel::Warn,
                            format!("Permission\n{}", report_row("Not changed", &error)),
                        );
                    }
                }
                sync_app_context(cli, app);
                app.draw_frame(terminal)?;
            }
            AppAction::ToggleTool { name, enabled } => {
                if let Err(error) = apply_tool_toggle(cli, &name, enabled) {
                    push_report(
                        app,
                        ids,
                        SystemLevel::Warn,
                        format!("Tools\n  Not saved         {error}"),
                    );
                }
                sync_app_context(cli, app);
                app.draw_frame(terminal)?;
            }
            AppAction::SaveSmartSettings(commit) => {
                match crate::session::smart_settings::apply_smart_settings_commit(&commit) {
                    Ok(message) => push_report(app, ids, SystemLevel::Info, message),
                    Err(error) => push_report(
                        app,
                        ids,
                        SystemLevel::Warn,
                        format!("Smart Router\n  Not saved         {error}"),
                    ),
                }
                sync_app_context(cli, app);
                app.draw_frame(terminal)?;
            }
            AppAction::DeepTier(action) => {
                let mut result = super::smart_settings::execute_deep_tier_command(&cli.cwd, &action);
                let view = tools::smart_deep_tier_models_for(&cli.cwd).map(|setting| DeepTierView {
                    models: setting.models,
                    configured: setting.configured,
                });
                if view.is_none() && result.is_ok() {
                    result = Err("Deep-tier pool: could not reload merged settings".to_string());
                }
                app.apply_deep_tier_update(view, result);
                app.draw_frame(terminal)?;
            }
            AppAction::Submit(input) => {
                let trimmed = input.trim().to_string();
                // Allow an image-only submission through: a queued entry may
                // carry pasted images with no text (staged back onto the app
                // just above), and dropping it here would silently discard them.
                if trimmed.is_empty() && !app.has_pending_images() {
                    continue;
                }
                // A synthesized auto-continue prompt pops as an ordinary queued
                // message (no goal/loop/agent tag), but it is NOT the human
                // speaking: it must neither acknowledge a goal checkpoint (an
                // unattended chain would look actively supervised) nor start a
                // fresh auto-continue chain.
                let human_submit = user_originated_submit
                    && !trimmed.starts_with(super::grind_escalation::AUTO_CONTINUE_MARKER);
                if human_submit {
                    cli.goal_controller.acknowledge_user_input();
                    cli.auto_continue_chain = 0;
                }
                app.append_history(&trimmed);
                let dispatch_local_slash = dispatches_local_slash(submission_origin, &trimmed);
                if dispatch_local_slash && matches!(trimmed.as_str(), "/exit" | "/quit") {
                    finish_tui_session(cli)?;
                    return Ok(());
                }

                let parsed_slash = if dispatch_local_slash {
                    SlashCommand::parse(&trimmed)
                } else {
                    Ok(None)
                };
                match parsed_slash {
                    // Zo Remote owns async gateway lifecycle and local-only
                    // credential output, so it cannot use the synchronous slash
                    // dispatcher shared by headless modes.
                    Ok(Some(SlashCommand::Remote {
                        action: RemoteAction::Open,
                    })) => {
                        app.open_remote_onboarding_modal(remote_onboarding_view(remote));
                        app.draw_frame(terminal)?;
                    }
                    Ok(Some(SlashCommand::Remote { action })) => {
                        handle_remote_action(cli, app, terminal, ids, remote, action).await?;
                        sync_app_context(cli, app);
                        app.draw_frame(terminal)?;
                    }
                    // `/ship` owns one foreground transaction, but its gates can run for
                    // minutes. Await a blocking worker and repaint each progress line so
                    // subprocess work does not stall the Tokio runtime or leave a detached
                    // gate-to-commit pipeline.
                    Ok(Some(SlashCommand::Ship { message })) => {
                        app.follow_latest();
                        app.set_render_observer(None);
                        push_report(
                            app,
                            ids,
                            SystemLevel::Info,
                            "Ship\n  Status            starting foreground flow",
                        );
                        app.draw_frame(terminal)?;
                        let cwd = cli.cwd.clone();
                        let (progress_tx, mut progress_rx) =
                            tokio::sync::mpsc::unbounded_channel();
                        let mut task = tokio::task::spawn_blocking(move || {
                            handle_ship_at(&cwd, &message, |progress| {
                                let _ = progress_tx.send(progress);
                            })
                        });
                        let result = loop {
                            tokio::select! {
                                biased;
                                Some(progress) = progress_rx.recv() => {
                                    push_report(app, ids, SystemLevel::Info, progress);
                                    app.draw_frame(terminal)?;
                                }
                                result = &mut task => {
                                    break result.map_err(|error| {
                                        TuiLoopError::Tui(format!("ship worker failed: {error}"))
                                    })?;
                                }
                            }
                        };
                        while let Ok(progress) = progress_rx.try_recv() {
                            push_report(app, ids, SystemLevel::Info, progress);
                        }
                        let level = if result.success {
                            SystemLevel::Info
                        } else {
                            SystemLevel::Error
                        };
                        push_report(app, ids, level, result.report);
                        refresh_remote_snapshot(cli, app, remote);
                        sync_app_context(cli, app);
                        app.draw_frame(terminal)?;
                    }
                    // `/compact` runs an API-backed summary round-trip. The
                    // synchronous dispatch chain would block this drive-loop task
                    // for the entire summary stream — the spinner, reveal and
                    // input all freeze. Special-case it here so the round-trip
                    // await-suspends and the TUI keeps repainting, then surface
                    // the report exactly as the dispatch funnel would.
                    Ok(Some(SlashCommand::Compact { instructions })) => {
                        app.follow_latest();
                        match cli
                            .compact_report_streaming(instructions, render_tx, ids)
                            .await
                        {
                            Ok(report) => push_report(app, ids, SystemLevel::Info, report),
                            Err(error) => return Err(TuiLoopError::Tui(error.to_string())),
                        }
                        sync_app_context(cli, app);
                        app.draw_frame(terminal)?;
                    }
                    Ok(Some(command)) => {
                        // The user just ran a command; make its confirmation
                        // visible even if they had scrolled up. Without this a
                        // slash command (e.g. /goal) appends its report
                        // off-screen and looks like a no-op.
                        app.follow_latest();
                        app.set_render_observer(None);
                        let session_name_before = cli.runtime.session().name.clone();
                        let should_quit = handle_persistent_slash(cli, app, ids, command);
                        refresh_remote_snapshot(cli, app, remote);
                        let should_quit = should_quit?;
                        if cli.runtime.session().name != session_name_before {
                            set_terminal_session_title(
                                terminal,
                                cli.runtime.session().name.as_deref(),
                            );
                        }
                        sync_app_context(cli, app);
                        // Draws whatever the command rendered — including the
                        // `Restarting…` acknowledgement for a restart, which the
                        // persist below must not precede.
                        app.draw_frame(terminal)?;
                        // `/memory` records a file to edit; the host owns the
                        // terminal, so suspend the TUI here and run $EDITOR.
                        if let Some(path) = app.take_pending_editor_file() {
                            edit_instruction_file(cli, app, ids, terminal, &path).await?;
                        }
                        // `/dump` records a transcript artifact to view; same
                        // contract, but read-only: $PAGER (or $EDITOR for
                        // `/dump edit`) and nothing to reload afterwards.
                        if let Some(view) = app.take_pending_transcript_view() {
                            if view.edit {
                                run_editor_on_path(terminal, &view.path).await;
                            } else {
                                run_pager_on_path(terminal, &view.path).await;
                            }
                        }
                        // `/login <provider>` records a sign-in to run. A plain
                        // thread, not `spawn_blocking`: the flow calls
                        // `block_on` internally and an ambient runtime handle
                        // would make that a nested-runtime hazard. The UI keeps
                        // painting while the browser round-trip is outstanding.
                        if let Some(provider) = app.take_pending_oauth_login() {
                            let tx = login_tx.clone();
                            // Retiring the previous sign-in happens on the NEW
                            // thread, never here: signalling is instant but the
                            // old wait may be mid-token-exchange, and joining it
                            // on the render thread would reintroduce exactly the
                            // freeze this design removed.
                            let superseded = pending_login.take();
                            let cancel = crate::auth::LoginCancel::default();
                            let handle = std::thread::spawn({
                                let cancel = cancel.clone();
                                move || {
                                    if let Some(superseded) = superseded {
                                        superseded.cancel.cancel();
                                        let _ = superseded.handle.join();
                                    }
                                    let result = crate::auth::run_login_provider_cancellable(
                                        &provider, &cancel,
                                    )
                                    .map_err(|error| error.to_string());
                                    // Blocking send from a non-async thread; the
                                    // single slot is free unless a previous
                                    // outcome is still unread, in which case
                                    // waiting is right — the report must not be
                                    // dropped.
                                    let _ = tx.blocking_send((provider, result));
                                }
                            });
                            pending_login = Some(PendingLogin { cancel, handle });
                        }
                        if should_quit {
                            // A restart returns `should_quit` too, but with a
                            // re-exec plan recorded (`pending_restart`). Persist
                            // NOW — after the `Restarting…` acknowledgement drawn
                            // above is visible — then hand off to `run_repl`'s
                            // exec. A save failure aborts the restart with the
                            // session intact instead of execing into a stale or
                            // empty child: clear the intent so `run_repl` never
                            // execs, restore input, surface the error, and stay
                            // in the loop. A plain exit records no plan and tears
                            // down immediately.
                            if cli.pending_restart.is_some() {
                                match cli.persist_session() {
                                    Ok(()) => {
                                        finish_tui_session_after_persist(cli);
                                        return Ok(());
                                    }
                                    Err(error) => {
                                        cli.pending_restart = None;
                                        app.enable_input();
                                        push_report(
                                            app,
                                            ids,
                                            SystemLevel::Error,
                                            format!(
                                                "Restart\n  Not restarted    could not save the session: {error}"
                                            ),
                                        );
                                        app.draw_frame(terminal)?;
                                    }
                                }
                            } else {
                                finish_tui_session(cli)?;
                                return Ok(());
                            }
                        }
                    }
                    Ok(None) => {
                        app.dismiss_startup_screen();
                        // A re-injected background agent result renders as an
                        // agent-authored card, not a `You` message — but the
                        // same `trimmed` text still submits to the model below,
                        // so the model reads the agent's result unchanged.
                        if let Some(meta) = app.take_pending_agent_result() {
                            app.push_block(RenderBlock::AgentResult {
                                id: ids.next(),
                                label: meta.label,
                                status: meta.status,
                                body: trimmed.clone(),
                            });
                        } else {
                            app.push_block(RenderBlock::UserMessage {
                                id: ids.next(),
                                text: trimmed.clone(),
                            });
                        }
                        let images: Vec<(String, String)> = app
                            .take_pending_images()
                            .into_iter()
                            .map(|img| (img.media_type, img.data))
                            .collect();
                        // Echo each attached image into the transcript so the
                        // user sees what they pasted. The same bytes also go to
                        // the model below; without this the image is sent but
                        // invisible in the chat.
                        for block in attached_image_blocks(&images, ids) {
                            app.push_block(block);
                        }
                        app.draw_frame(terminal)?;
                        // The previous turn's in-flight checkpoint is now
                        // adopted inside `run_live_turn_with_images`'s prelude,
                        // OFF the UI thread and after `Thinking` is drawn, so the
                        // tree-hash join no longer stalls this submit path before
                        // the turn becomes visible.
                        let loop_gate_restore = install_automation_plan_gate(cli, &trimmed);
                        // Turn-scoped read-only + propose-only gate for an
                        // unattended `/loop`/`/goal` schedule turn (a user-typed or
                        // `--allow-writes` turn is a no-op). Installed alongside the
                        // plan gate and restored symmetrically after the turn.
                        let perm_gate = install_automation_permission_gate(cli, &trimmed);
                        let remote_approval = remote.approval_shared();
                        let turn_generation = remote.set_turn(TurnPhase::Running);
                        // Kept for the failure path below: a first-iteration turn
                        // failure rolls the undelivered user message back out of the
                        // transcript (the runtime cannot leave an orphan message the
                        // next request would re-send), so without this the text is
                        // simply gone — and the failures most likely to land here are
                        // exactly the ones worth resubmitting (a capacity refusal).
                        let submitted_text = trimmed.clone();
                        let outcome = Box::pin(run_live_turn_with_images(
                            cli,
                            app,
                            terminal,
                            &mut events,
                            render_tx,
                            cmd_rx,
                            turn_generation,
                            remote_approval,
                            trimmed,
                            images,
                            agent_rx,
                            freshness,
                            &mut clipboard_write,
                        ))
                        .await;
                        remote.set_turn(TurnPhase::Idle);
                        restore_automation_permission_gate(cli, perm_gate);
                        restore_automation_plan_gate(cli, loop_gate_restore);
                        sync_app_context(cli, app);
                        // One-shot transcript notice the first time a redeploy
                        // is detected: the sidebar badge truncates at narrow
                        // widths and is easy to miss mid-stream, while the turn
                        // boundary is exactly when the user reads results and
                        // can act on /restart. Latched in `stale_binary`, so it
                        // never nags twice.
                        if let Some(info) = zo_cli::tui::stale_binary::take_newly_stale()
                        {
                            push_report(app, ids, SystemLevel::Warn, info.transcript_notice());
                        }
                        // Capture the post-turn worktree as a checkpoint so a
                        // later Esc-Esc can rewind this turn's code edits in
                        // lockstep with its conversation messages.
                        cli.capture_code_checkpoint();
                        let outcome = match outcome {
                            Ok(o) => o,
                            Err(err) => {
                                // The visible prelude may have started before a
                                // setup failure. Always tear it down here so a
                                // failed probe/client build cannot leave the
                                // composer disabled or a stale Thinking row.
                                app.abort_turn();
                                app.enable_input();
                                // The turn errored before producing a summary, so
                                // the goal advance (which consumes the ownership
                                // latch) never runs. Clear the latch here, or a
                                // goal-owned turn that errors would leak `pending`
                                // onto the next unrelated direct-submit turn and
                                // mis-attribute it to the goal.
                                cli.goal_turn_pending = false;
                                push_report(app, ids, SystemLevel::Error, format!("{err}"));
                                // Put the prompt back in the composer so a failure
                                // the user can act on (switch model, wait, retry)
                                // costs one keypress instead of retyping. Only when
                                // the composer is empty: anything typed while the
                                // turn ran is newer and must not be clobbered.
                                if app.input_text().trim().is_empty() {
                                    app.set_input_text(&submitted_text);
                                }
                                app.draw_frame(terminal)?;
                                continue;
                            }
                        };
                        if let Some(summary) = outcome.summary {
                            if let Some(event) = &summary.auto_compaction {
                                if event.removed_message_count > 0 {
                                    app.set_render_observer(None);
                                    reseed_transcript_after_auto_compaction(
                                        app,
                                        ids,
                                        cli.runtime.session(),
                                        event.removed_message_count,
                                    );
                                    refresh_remote_snapshot(cli, app, remote);
                                } else {
                                    push_report(
                                        app,
                                        ids,
                                        SystemLevel::Info,
                                        format_auto_compaction_notice(event.removed_message_count),
                                    );
                                }
                            }
                            maybe_auto_open_review_diff(cli, app, &summary);
                            // Charge a `/loop`-owned turn's output tokens against the
                            // loop's optional `--token-budget`, so a bounded recurring
                            // loop stops at its token ceiling on the next pop-gate.
                            if let Some(loop_id) = loop_turn_id.as_deref() {
                                cli.charge_loop_output(loop_id, summary.turn_output_tokens);
                                // Budget graceful-stop → loop pause: a turn that hit a
                                // turn budget (iteration cap / deadline / tool-call
                                // budget) instead of a natural stop pauses the loop and
                                // records the "awaiting your decision" note into the team
                                // inbox digest, so an unattended loop never silently burns
                                // budget across ticks. Recoverable: the user resumes with
                                // `/loop resume` after deciding.
                                if let Some(kind) = summary.budget_exhausted {
                                    if cli.pause_loop_for_budget(loop_id) {
                                        let label =
                                            super::automation::budget_exhausted_kind_label(kind);
                                        cli.record_automation_digest(&format!(
                                            "loop {loop_id} paused: {label} exhausted — awaiting your decision"
                                        ));
                                        push_report(
                                            app,
                                            ids,
                                            SystemLevel::Warn,
                                            format!(
                                                "Loop {loop_id} paused — {label} exhausted; recorded to team inbox digest. Resume with /loop resume {loop_id}."
                                            ),
                                        );
                                    }
                                }
                                // Stop the loop if its `--until` completion check now passes,
                                // or if it has stalled (same failure repeated) — surface the
                                // stall so the user knows why the loop stopped firing.
                                if let Some(notice) = cli.check_loop_until_after_turn(loop_id).await {
                                    push_report(app, ids, SystemLevel::Warn, notice);
                                }
                            }
                            // Captured before the goal advance consumes the latch:
                            // a goal-owned turn folds the independent panel into its
                            // completion gate, so the all-turns warning below runs
                            // only for non-goal turns (no double panel).
                            let was_goal_owned = cli.goal_turn_pending;
                            handle_goal_advance(
                                cli,
                                app,
                                ids,
                                summary.deep_verification,
                                summary.verification_issues.clone(),
                                summary.turn_output_tokens,
                            )
                            .await;
                            // All-turns extension: a non-goal UltraCode edit turn gets
                            // the same independent spec/regression/security panel,
                            // surfaced as a non-blocking warning. A non-goal UltraCode
                            // turn with NO diff (a solo diagnosis/decision) instead gets
                            // the competing-hypotheses panel (principle ②) — mutually
                            // exclusive (the diff panel returns None on a no-diff turn),
                            // so a turn never runs both.
                            // `deep_verification == Some(true)` means the
                            // reactive gate (or a goal verify leg) already
                            // semantically verified this turn's change with an
                            // independent model — a second panel re-judging the
                            // same diff is the exact attention-burning double
                            // verification we are dismantling (one smart
                            // verification per change). Recording the hash (not
                            // just skipping) makes the exemption durable: a
                            // later Smart turn over the still-dirty worktree
                            // must not re-judge the same diff either.
                            if summary.deep_verification == Some(true) {
                                cli.mark_worktree_diff_verified();
                            }
                            if !was_goal_owned && summary.deep_verification != Some(true) {
                                // Run the post-turn verify panels OFF this thread:
                                // spawn the snapshotted future and let the
                                // `verify_rx` arm push the warning when it lands.
                                // Doing the 2–3 lens round-trips inline here froze
                                // the input pump after every Ultracode turn (the
                                // "answer ends → input echoes one char then bursts"
                                // bug); spawning keeps the composer live.
                                if let Some(fut) = cli.post_turn_verify_future() {
                                    let tx = verify_tx.clone();
                                    tokio::spawn(async move {
                                        if let Some(warning) = fut.await {
                                            let _ = tx.send(warning).await;
                                        }
                                    });
                                }
                            }
                            // Auto-continue: a budget-stopped turn that was
                            // making real progress resumes on its own instead
                            // of waiting for the user to type "계속" — bounded
                            // by the chain cap, the fresh-progress requirement,
                            // and the grind-streak ladder, so it can never
                            // become an unattended infinite loop. `/loop`- and
                            // goal-owned turns keep their own schedulers.
                            if let Some(kind) = summary.budget_exhausted {
                                if loop_turn_id.is_none() && !was_goal_owned {
                                    let cap = super::grind_escalation::auto_continue_cap();
                                    if super::grind_escalation::should_auto_continue(
                                        kind,
                                        summary.progress_tool_results(),
                                        cli.auto_continue_chain,
                                        cap,
                                        cli.grind_streak,
                                        super::grind_escalation::grind_escalation_threshold(),
                                    ) {
                                        match app.queue_message(
                                            super::grind_escalation::auto_continue_prompt(),
                                        ) {
                                            Ok(()) => {
                                                cli.auto_continue_chain += 1;
                                                push_report(
                                                    app,
                                                    ids,
                                                    SystemLevel::Info,
                                                    format!(
                                                        "[budget] auto-continuing ({}/{}) — progress detected; finishing the remaining work",
                                                        cli.auto_continue_chain,
                                                        cap.unwrap_or(0)
                                                    ),
                                                );
                                            }
                                            Err(error) => push_report(
                                                app,
                                                ids,
                                                SystemLevel::Warn,
                                                format!(
                                                    "[budget] auto-continue not queued ({error})"
                                                ),
                                            ),
                                        }
                                    }
                                }
                            }
                        } else {
                            // A cancelled turn (Ctrl+C / Esc) returns no summary, so
                            // the goal advance never consumes the ownership latch.
                            // Clear it here, or a cancelled goal-owned turn would
                            // leak `pending` onto the next direct-submit user turn
                            // (empty queue → no pop-time re-stamp) and let it be
                            // mis-attributed to the goal (a possible false success).
                            cli.goal_turn_pending = false;
                        }
                        app.draw_frame(terminal)?;
                    }
                    Err(error) => {
                        push_report(app, ids, SystemLevel::Error, error.to_string());
                        app.draw_frame(terminal)?;
                    }
                }
            }
            AppAction::SelectSession(session_id) => {
                let remote_was_active = remote.is_active();
                if remote_was_active {
                    app.set_render_observer(None);
                    let _ = remote.stop().await;
                }
                let report = cli
                    .resume_session_fast(Some(&session_id))
                    .map_err(|error| TuiLoopError::Tui(error.to_string()))?;
                app.reset_session_view();
                app.set_agent_manifest_started_after(cli.agent_manifest_started_after);
                app.set_agent_manifest_session_id(cli.session.id.clone());
                install_status_line_poller(cli, app);
                seed_transcript_from_session(app, ids, cli.runtime.session());
                push_report(app, ids, SystemLevel::Info, report);
                if remote_was_active {
                    push_report(
                        app,
                        ids,
                        SystemLevel::Info,
                        "Zo Remote\n  Status            stopped\n  Reason            session changed\n  Credentials       revoked",
                    );
                }
                freshness.mark_dirty(FreshnessDomain::Workspace);
                sync_app_context(cli, app);
                app.draw_frame(terminal)?;
            }
            AppAction::ClipboardPaste => {
                handle_clipboard_paste(app).await;
                app.draw_frame(terminal)?;
            }
            AppAction::ClipboardCopy(target) => {
                if clipboard_write.is_none() {
                    if let Some(write) = copy_session_to_clipboard(cli.runtime.session(), target) {
                        clipboard_write = Some(write);
                    } else {
                        push_report(
                            app,
                            ids,
                            SystemLevel::Warn,
                            "Nothing to copy yet — wait for the assistant's reply or type a message first.",
                        );
                        app.draw_frame(terminal)?;
                    }
                }
            }
            AppAction::ClipboardCopyBlock(text) => {
                if clipboard_write.is_none() {
                    clipboard_write = Some(PendingClipboardCopy::new(text));
                }
            }
            AppAction::Editor => {
                if let Some(content) = launch_editor(terminal).await {
                    if !content.trim().is_empty() {
                        app.set_input_text(&content);
                        app.draw_frame(terminal)?;
                    }
                }
                app.draw_frame(terminal)?;
            }
            AppAction::RewindCheckpoint => {
                // Gate the destructive Esc-Esc rewind behind an explicit y/n.
                // An Esc that denies a permission prompt and a follow-up
                // Esc-Esc rewind share the same key, so a reflexive double-tap
                // could otherwise silently discard the latest turn's code
                // edits and conversation. Show what would be reverted and wait
                // for confirmation — the rewind itself runs on ConfirmRewind.
                // Adopt the (possibly in-flight) checkpoint of the turn that
                // just ended, so the preview shows the state Esc-Esc would
                // actually restore.
                cli.settle_code_checkpoint();
                let files = cli.preview_rewind();
                app.open_rewind_confirm(rewind_confirm_lines(&files));
                app.draw_frame(terminal)?;
            }
            AppAction::ConfirmRewind => {
                app.set_render_observer(None);
                rewind_checkpoint(cli, app, ids, freshness);
                refresh_remote_snapshot(cli, app, remote);
                app.draw_frame(terminal)?;
            }
            AppAction::OpenRewindViewer => {
                open_rewind_viewer(cli, app, ids);
                app.draw_frame(terminal)?;
            }
            AppAction::OpenWorkflowViewer => {
                open_workflow_viewer(app);
                app.draw_frame(terminal)?;
            }
            AppAction::OpenAgentInViewer(agent_id) => {
                open_workflow_viewer_focused(app, &agent_id);
                app.draw_frame(terminal)?;
            }
            AppAction::RewindTo(index) => {
                app.set_render_observer(None);
                rewind_to_snapshot(cli, app, ids, index, freshness);
                refresh_remote_snapshot(cli, app, remote);
                app.draw_frame(terminal)?;
            }
            AppAction::AckTeamInboxUpdate(update_id) => {
                if let Err(error) = runtime::team_inbox_manual_ack(&cli.cwd, &cli.session.id, &update_id) {
                    push_report(
                        app,
                        ids,
                        SystemLevel::Warn,
                        format!("Team inbox
  Not acked        {error}"),
                    );
                }
                app.apply_team_inbox_snapshot(runtime::team_inbox_snapshot(&cli.cwd, &cli.session.id, 50));
                sync_app_context(cli, app);
                app.draw_frame(terminal)?;
            }
            AppAction::IncludeTeamInboxUpdate(text) => {
                let current = app.input().text();
                if current.trim().is_empty() {
                    app.set_input_text(&text);
                } else {
                    app.set_input_text(&format!("{} {}", current.trim_end(), text));
                }
                app.draw_frame(terminal)?;
            }
            AppAction::RefreshTeamInboxViewer => {
                app.apply_team_inbox_snapshot(runtime::team_inbox_snapshot(&cli.cwd, &cli.session.id, 50));
                sync_app_context(cli, app);
                app.draw_frame(terminal)?;
            }
            AppAction::Redraw => {
                app.draw_frame(terminal)?;
            }
            AppAction::TerminalResized => {
                on_terminal_resize(terminal, app.terminal_mode())?;
                app.draw_frame(terminal)?;
            }
            AppAction::None => {}
        }
    }
}

fn remote_onboarding_view(remote: &RemoteManager) -> RemoteOnboardingView {
    let overview = remote.overview();
    let turn_state = match overview.status.turn {
        TurnPhase::Idle => "idle",
        TurnPhase::Running => "running",
    };
    RemoteOnboardingView {
        running: overview.running,
        url: overview.origin,
        device_count: overview.status.devices,
        pending_count: overview.status.pending,
        controller: overview.status.controller_name,
        turn_state: turn_state.to_string(),
        pending_pairs: overview
            .status
            .pending_pairs
            .into_iter()
            .map(|pair| RemotePendingPair {
                device_name: pair.device_name,
                comparison_code: pair.comparison_code,
            })
            .collect(),
    }
}

fn remote_session_title(cwd: &Path, name: Option<&str>) -> String {
    if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) {
        return format!("Zo · {name}");
    }
    cwd.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map_or_else(|| "Zo session".to_string(), |name| format!("Zo · {name}"))
}

fn refresh_remote_snapshot(cli: &LiveCli, app: &mut App, remote: &RemoteManager) {
    let title = remote_session_title(&cli.cwd, cli.runtime.session().name.as_deref());
    remote.set_session_info(&cli.session.id, &title);
    remote.replace_snapshot(app.transcript().blocks());
    app.set_render_observer(remote.observer());
}

async fn handle_remote_action(
    cli: &LiveCli,
    app: &mut App,
    terminal: &mut TuiTerminal,
    ids: &BlockIdGen,
    remote: &mut RemoteManager,
    action: RemoteAction,
) -> Result<(), TuiLoopError> {
    app.follow_latest();
    // QR offer secrets, comparison decisions, and credential lifecycle
    // reports must never be projected to an already connected device.
    app.set_render_observer(None);
    let report = match action {
        RemoteAction::Open => unreachable!("bare /remote opens the TUI modal before lifecycle dispatch"),
        RemoteAction::Start => {
            let title = remote_session_title(&cli.cwd, cli.runtime.session().name.as_deref());
            let snapshot = RemoteManager::project_snapshot(app.transcript().blocks());
            let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
            let start = remote.start(
                cli.session.id.clone(),
                title,
                snapshot,
                progress_tx,
            );
            tokio::pin!(start);
            loop {
                tokio::select! {
                    report = &mut start => break report,
                    Some(message) = progress_rx.recv() => {
                        push_report(app, ids, SystemLevel::Info, message);
                        app.draw_frame(terminal)?;
                    }
                }
            }
        }
        RemoteAction::Status => Ok(remote.status_report()),
        RemoteAction::Qr => remote.qr_report(),
        RemoteAction::Rotate => remote.rotate(),
        RemoteAction::Stop => Ok(remote.stop().await),
        RemoteAction::Approve { code } => remote.approve(&code),
        RemoteAction::Deny { code } => remote.deny(&code),
    };
    let (level, message) = match report {
        Ok(message) => (SystemLevel::Info, message),
        Err(message) => (SystemLevel::Warn, message),
    };
    push_report(app, ids, level, message);
    app.set_render_observer(remote.observer());
    Ok(())
}

fn finish_tui_session_after_persist(cli: &mut LiveCli) {
    stop_agents_for_session_close(
        cli.agent_manifest_started_after,
        &cli.session.id,
        "parent session closed",
    );
}

fn finish_tui_session(cli: &mut LiveCli) -> Result<(), TuiLoopError> {
    finish_tui_session_after_persist(cli);
    // Lifecycle SessionEnd is emitted once by run_tui_session after this loop
    // returns; keep finish_tui_session focused on in-loop teardown duties.
    cli.persist_session()
        .map_err(|error| TuiLoopError::Tui(error.to_string()))
}

fn stop_agents_for_session_close(started_after_secs: u64, session_id: &str, reason: &str) -> usize {
    // The agent store is workspace-global; never use an unscoped stop here.
    // Closing one TUI tab/session must only terminate agents whose manifests
    // were stamped with this session id and created after the session's agent
    // scope began.
    request_foreground_workflow_cancel();
    stop_running_agents_since_for_strict_session(started_after_secs, session_id, reason)
}

fn install_automation_plan_gate(cli: &mut LiveCli, input: &str) -> DeepGateRestore {
    TurnHarness::install_automation_plan_gate_if_needed(input, &mut cli.runtime)
}

fn restore_automation_plan_gate(cli: &mut LiveCli, restore: DeepGateRestore) {
    TurnHarness::restore_deep_gate(&mut cli.runtime, restore);
}

fn install_automation_permission_gate(cli: &mut LiveCli, input: &str) -> AutomationPermissionGate {
    TurnHarness::install_automation_permission_gate_if_needed(input, &mut cli.runtime)
}

fn restore_automation_permission_gate(cli: &mut LiveCli, gate: AutomationPermissionGate) {
    TurnHarness::restore_automation_permission_gate(&mut cli.runtime, gate);
}

fn queue_due_loop_prompts(cli: &mut LiveCli, app: &mut App, ids: &BlockIdGen) {
    let prompts = cli.drain_due_loop_prompts(Instant::now());
    for prompt in prompts {
        match app.queue_loop_message(prompt.text, prompt.loop_id.clone()) {
            Ok(()) => push_report(
                app,
                ids,
                SystemLevel::Info,
                format!(
                    "Loop due\n  Id               {}\n  Status           queued scheduled run",
                    prompt.loop_id
                ),
            ),
            Err(error) => push_report(
                app,
                ids,
                SystemLevel::Warn,
                format!(
                    "Loop due\n  Id               {}\n  Status           not queued ({error})",
                    prompt.loop_id
                ),
            ),
        }
    }
}

/// Drain due `ScheduleWakeup` requests (`.zo/wakeups/*.json`) and queue each
/// one's prompt as a fresh user turn — the consumer half the tool was missing
/// (it wrote files nothing ever read, so "wake me in N seconds" never fired).
/// The file is removed only after the prompt is queued, so a full queue simply
/// leaves it for the next pass (no lost wakeup). Mirrors `queue_due_loop_prompts`.
fn queue_due_wakeup_prompts(cli: &LiveCli, app: &mut App, ids: &BlockIdGen) {
    let due = super::wakeups::scan_due_wakeups(
        super::wakeups::now_epoch_secs(),
        Some(&cli.session.id),
    );
    for wakeup in due {
        let reason = wakeup.reason.clone();
        match app.queue_message(wakeup.prompt) {
            Ok(()) => {
                super::wakeups::consume_wakeup_file(&wakeup.file);
                app.clear_scheduled_file_wake();
                let detail = if reason.trim().is_empty() {
                    "Wakeup due\n  Status           queued scheduled prompt".to_string()
                } else {
                    format!(
                        "Wakeup due\n  Reason           {reason}\n  Status           queued scheduled prompt"
                    )
                };
                push_report(app, ids, SystemLevel::Info, detail);
            }
            // Queue is full — leave the file in place so it fires on a later pass.
            Err(_) => break,
        }
    }
}

/// The sooner of two optional countdowns — used to wake the idle prompt at the
/// next `/loop` interval or the next `ScheduleWakeup`, whichever comes first.
fn earliest_due(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

async fn handle_goal_advance(
    cli: &mut LiveCli,
    app: &mut App,
    ids: &BlockIdGen,
    semantic: Option<bool>,
    verifier_issues: Vec<String>,
    output_tokens: u32,
) {
    match cli
        .advance_goal_after_turn(semantic, verifier_issues, output_tokens)
        .await
    {
        super::automation::GoalAdvance::Idle => {}
        super::automation::GoalAdvance::Done(report) => {
            push_report(app, ids, SystemLevel::Info, report);
            // Clear the synthetic `Goal:` todo now that the goal reached a
            // terminal state on its own (succeeded / failed / unverified). The
            // headless `run_goal_prompt_until_stop` path already does this; the
            // interactive TUI must too, or the todo lingers `in_progress`
            // forever. Reuses the single-owner clear in `live_cli_commands`.
            // Only surface a status line when an item was actually removed, to
            // avoid transcript noise on goals that never seeded a todo.
            // (Not done on `Idle`: that fires every no-active-goal turn and
            // would also strip a merely-paused goal's todo.)
            let cleared = LiveCli::goal_todo_clear_line();
            if cleared.contains("removed goal item") {
                push_report(app, ids, SystemLevel::Info, format!("Goal todo\n{cleared}"));
            }
        }
        super::automation::GoalAdvance::Queue { report, prompt } => {
            push_report(app, ids, SystemLevel::Warn, report);
            if let Err(error) = app.queue_goal_message(prompt) {
                push_report(
                    app,
                    ids,
                    SystemLevel::Error,
                    format!("Goal repair\n  Status           not queued ({error})"),
                );
            }
        }
        super::automation::GoalAdvance::Pause(report) => {
            // Auto-paused at an unattended checkpoint: surface loudly but keep
            // the goal (and its `Goal:` todo) intact — `/goal resume` continues.
            push_report(app, ids, SystemLevel::Warn, report);
        }
    }
}

fn apply_tool_toggle(
    cli: &mut LiveCli,
    name: &str,
    enabled: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let registry = cli.runtime.api_client().tool_registry();
    let mut disabled = registry.disabled_tool_names();
    if enabled {
        disabled.remove(name);
    } else {
        disabled.insert(name.to_string());
    }
    super::tool_toggles::save_disabled_tool_names(&cli.cwd, &disabled)?;
    registry.set_disabled_tools(disabled);
    Ok(())
}

fn copy_session_to_clipboard(
    session: &runtime::Session,
    target: ClipboardCopyTarget,
) -> Option<PendingClipboardCopy> {
    let all = matches!(target, ClipboardCopyTarget::All);
    copy_payload(session, all).map(PendingClipboardCopy::new)
}

/// Build the body lines for the Esc-Esc rewind confirmation card from the
/// dry-run file list. Names the tracked files that would be reverted (capped
/// for height) so the user sees the blast radius before confirming.
fn rewind_confirm_lines(files: &[PathBuf]) -> Vec<String> {
    const MAX_LISTED: usize = 8;
    let mut lines = vec![
        "Rewind the latest turn?".to_string(),
        "Restores the worktree to the previous checkpoint and drops".to_string(),
        "that turn's conversation. This undoes the model's edits.".to_string(),
        String::new(),
    ];
    if files.is_empty() {
        lines.push("  Code   no tracked file changes to revert".to_string());
    } else {
        lines.push(format!(
            "  Code   {} file(s) will be reverted:",
            files.len()
        ));
        for path in files.iter().take(MAX_LISTED) {
            lines.push(format!("    • {}", path.display()));
        }
        if files.len() > MAX_LISTED {
            lines.push(format!("    … and {} more", files.len() - MAX_LISTED));
        }
    }
    lines.push(String::new());
    lines.push("  [y] rewind     [n] cancel".to_string());
    lines
}

/// Esc-Esc combined rewind: drop the previous turn's conversation messages
/// *and* restore the worktree to the matching code snapshot in one step.
///
/// Reuses the existing `rewind_turns` + `SnapshotStack` machinery via
/// [`LiveCli::rewind_last_checkpoint`]; this only reseeds the transcript
/// view, persists the trimmed session, and reports the outcome honestly
/// (including a partial result when the code restore is blocked by a user
/// edit). It does not build a new restore engine.
fn rewind_checkpoint(
    cli: &mut LiveCli,
    app: &mut App,
    ids: &BlockIdGen,
    freshness: &SessionFreshness,
) {
    use super::live_cli::CodeRewindOutcome;

    let report = cli.rewind_last_checkpoint();
    if report.is_noop() {
        push_report(
            app,
            ids,
            SystemLevel::Info,
            "Rewind\n  Nothing to rewind — already at the earliest checkpoint.",
        );
        return;
    }

    // Reseed the transcript from the (now trimmed) session and persist so
    // the rewind survives a restart.
    app.reset_session_view();
    seed_transcript_from_session(app, ids, cli.runtime.session());
    if let Err(err) = cli.persist_session() {
        eprintln!("[zo] warning: failed to persist session after rewind: {err}");
    }
    freshness.mark_dirty(FreshnessDomain::Workspace);

    let conv = if report.messages_removed > 0 {
        format!(
            "  Conversation     rewound {} message(s)",
            report.messages_removed
        )
    } else {
        "  Conversation     already at the earliest turn".to_string()
    };
    let level = if matches!(report.code, CodeRewindOutcome::Blocked { .. }) {
        SystemLevel::Warn
    } else {
        SystemLevel::Info
    };
    let code = match report.code {
        CodeRewindOutcome::Restored { turn } => {
            format!("  Code             restored to turn {turn}")
        }
        CodeRewindOutcome::NoRepo => {
            "  Code             unchanged (not a git repository)".to_string()
        }
        CodeRewindOutcome::NoEarlierState => {
            "  Code             unchanged (no earlier snapshot)".to_string()
        }
        CodeRewindOutcome::Blocked { reason } => {
            format!("  Code             not restored — {reason}")
        }
    };
    push_report(
        app,
        ids,
        level,
        format!("Rewind checkpoint\n{conv}\n{code}"),
    );
}

/// Build the snapshot timeline from the git snapshot stack and open the
/// interactive rewind viewer (Ctrl+R). Each row carries that turn's line
/// deltas and parsed diff so the modal is self-contained once open.
pub(crate) fn open_rewind_viewer(cli: &mut LiveCli, app: &mut App, ids: &BlockIdGen) {
    use zo_cli::tui::modals::diff_viewer::parse_unified_diff;
    use zo_cli::tui::modals::rewind_viewer::{RewindRow, RewindViewerModal};

    let Some(stack) = cli.snapshot_stack.as_ref() else {
        push_report(
            app,
            ids,
            SystemLevel::Info,
            "Rewind\n  Not a git repository — nothing to rewind.",
        );
        return;
    };

    let entries = stack.entries();
    if entries.len() < 2 {
        push_report(
            app,
            ids,
            SystemLevel::Info,
            "Rewind\n  Nothing to rewind yet — only the baseline checkpoint exists.",
        );
        return;
    }

    // Newest-first: the current worktree state at the top of the timeline.
    let rows: Vec<RewindRow> = entries
        .iter()
        .rev()
        .map(|entry| {
            let stat = stack.diff_stat(entry.index).unwrap_or_default();
            let (added, removed) = stat
                .iter()
                .fold((0, 0), |(a, r), delta| (a + delta.added, r + delta.removed));
            let diff_text = stack.turn_diff(entry.index).unwrap_or_default();
            RewindRow {
                index: entry.index,
                turn_number: entry.turn_number,
                is_current: entry.is_current,
                added,
                removed,
                file_count: stat.len(),
                views: parse_unified_diff(&diff_text),
            }
        })
        .collect();

    app.open_rewind_viewer(RewindViewerModal::new(rows));
}

/// Open the live workflow progress viewer (Ctrl+O). Reads the engine's progress
/// snapshot + per-agent manifests into a [`WorkflowView`]; while the modal is
/// open, `App`'s tick loop re-polls it so the tree stays live. With no workflow
/// doc it falls back to the per-agent manifest pager (live or finished agents).
pub(crate) fn open_workflow_viewer(app: &mut App) {
    use zo_cli::tui::modals::workflow_viewer::WorkflowViewerModal;
    use zo_cli::tui::workflow_progress;

    match workflow_progress::read_view_since(
        app.agent_manifest_started_after(),
        app.agent_manifest_session_id(),
    ) {
        Some(view) => app.open_workflow_viewer(WorkflowViewerModal::new(view)),
        // No live workflow doc — open the agents viewer, which shows live AND
        // finished sub-agents (or a clear empty state when none ran). So
        // Ctrl+O always lands on a real surface instead of a dead-end "no
        // active workflow yet" notice once the agents have finished.
        None => app.open_agents_viewer(),
    }
}

/// Open the live workflow viewer focused on a specific agent id — the click
/// target behind [`AppAction::OpenAgentInViewer`]. Same snapshot read as
/// [`open_workflow_viewer`], but the modal opens pre-selected to `agent_id`.
/// When the id is not in the freshly-read snapshot (the agent finished between
/// click and read, or there is no workflow doc), it drops to the per-agent
/// manifest pager rather than opening the modal on the wrong (default-selected)
/// agent — so a click never focuses an agent the user did not click.
pub(crate) fn open_workflow_viewer_focused(app: &mut App, agent_id: &str) {
    use zo_cli::tui::modals::workflow_viewer::WorkflowViewerModal;
    use zo_cli::tui::workflow_progress;

    match workflow_progress::read_view_since(
        app.agent_manifest_started_after(),
        app.agent_manifest_session_id(),
    ) {
        Some(view) => {
            let mut modal = WorkflowViewerModal::new(view);
            // Only open the focused modal when the clicked agent actually
            // exists in the freshly-read view. On a miss (the agent finished
            // between the click and this read, or the view is doc-less) the
            // modal would otherwise keep its default `(0, 0)` selection — which
            // highlights the FIRST agent, i.e. the WRONG one — so fall back to
            // the agents viewer instead, exactly like the aggregate path
            // below; it has no live gate, so a just-finished agent is there.
            if modal.select_agent_by_id(agent_id) {
                app.open_workflow_viewer(modal);
            } else {
                app.open_agents_viewer_focused(agent_id);
            }
        }
        None => app.open_agents_viewer_focused(agent_id),
    }
}

/// Rewind the worktree to the snapshot the viewer selected. Code-only: the
/// conversation is left intact (use `/undo` for a combined rewind), so this
/// just restores files, invalidates workspace freshness, and reports.
fn rewind_to_snapshot(
    cli: &mut LiveCli,
    app: &mut App,
    ids: &BlockIdGen,
    index: usize,
    freshness: &SessionFreshness,
) {
    let Some(stack) = cli.snapshot_stack.as_mut() else {
        return;
    };
    match stack.rewind_to(index) {
        Ok(result) => {
            let redo = stack.redo_depth();
            freshness.mark_dirty(FreshnessDomain::Workspace);
            push_report(
                app,
                ids,
                SystemLevel::Info,
                format!(
                    "Rewind\n  Code             restored to turn {}\n  Redo             {redo} snapshot(s) available (/redo)\n  Conversation     unchanged — /undo rewinds code + chat together",
                    result.restored_turn
                ),
            );
        }
        Err(error) => {
            push_report(
                app,
                ids,
                SystemLevel::Warn,
                format!("Rewind\n  Not restored — {error}"),
            );
        }
    }
}

/// Suspend the TUI, run `program` on `path`, then restore the active terminal
/// strategy. Fullscreen temporarily leaves/re-enters the alternate screen;
/// inline stays on the primary screen. Both restore TUI mouse reporting so
/// click-to-expand and app-managed wheel history still work after the external
/// program exits.
///
/// This is the single suspend/spawn/restore seam for `$EDITOR`
/// ([`run_editor_on_path`]) and `$PAGER` ([`run_pager_on_path`]).
async fn run_program_on_path<B: ratatui::backend::Backend + std::io::Write>(
    terminal: &mut ratatui::Terminal<B>,
    program: &str,
    path: &Path,
) -> bool {
    use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
    use std::io::Write;

    let inline = INLINE_TERMINAL_ACTIVE.load(std::sync::atomic::Ordering::Relaxed);
    // Hand the terminal over only once the writer thread has finished with it.
    // The escapes below go straight to stdout, so a frame still queued would be
    // written after them — an alternate-screen diff painted into the editor's
    // session, and a queued `Begin` freezing the editor in synchronized mode.
    Write::flush(terminal.backend_mut()).ok();
    zo_cli::tui::term::frame_writer::drain_active(TERMINAL_HANDOFF_DRAIN);
    crossterm::terminal::disable_raw_mode().ok();
    if inline {
        crossterm::execute!(std::io::stdout(), DisableMouseCapture).ok();
    } else {
        crossterm::execute!(
            std::io::stdout(),
            DisableMouseCapture,
            crossterm::terminal::LeaveAlternateScreen
        )
        .ok();
    }
    std::io::stdout().flush().ok();

    let status = tokio::process::Command::new(program)
        .arg(path)
        .status()
        .await
        .ok();

    if inline {
        crossterm::execute!(std::io::stdout(), EnableMouseCapture).ok();
    } else {
        crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            EnableMouseCapture
        )
        .ok();
    }
    crossterm::terminal::enable_raw_mode().ok();
    terminal.clear().ok();

    status.is_some_and(|s| s.success())
}

/// Run `$EDITOR`/`$VISUAL`/`vi` on `path` behind the shared suspend/restore
/// seam ([`run_program_on_path`]). Shared by the Ctrl+E input composer
/// ([`launch_editor`]), the `/memory` instruction-file editor
/// ([`edit_instruction_file`]), and `/dump edit`.
async fn run_editor_on_path<B: ratatui::backend::Backend + std::io::Write>(
    terminal: &mut ratatui::Terminal<B>,
    path: &Path,
) -> bool {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());
    run_program_on_path(terminal, &editor, path).await
}

/// Run `$PAGER` (default `less`) on `path` behind the shared suspend/restore
/// seam ([`run_program_on_path`]) — the `/dump` read-only transcript viewer,
/// which is what gives the dump real `/pattern` search.
async fn run_pager_on_path<B: ratatui::backend::Backend + std::io::Write>(
    terminal: &mut ratatui::Terminal<B>,
    path: &Path,
) -> bool {
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    run_program_on_path(terminal, &pager, path).await
}

/// Open a throwaway temp file in `$EDITOR` and return its saved contents — the
/// Ctrl+E "compose in editor" path. The temp file is always removed.
async fn launch_editor<B: ratatui::backend::Backend + std::io::Write>(
    terminal: &mut ratatui::Terminal<B>,
) -> Option<String> {
    let tmp_path = std::env::temp_dir().join(format!("zo-editor-{}.md", std::process::id()));
    let saved = run_editor_on_path(terminal, &tmp_path).await;
    let content = saved
        .then(|| std::fs::read_to_string(&tmp_path).ok())
        .flatten();
    let _ = std::fs::remove_file(&tmp_path);
    content
}

/// Open the project `context.md` instruction file in `$EDITOR` for the
/// `/memory` command, creating it if absent, then reload context so the edits
/// take effect this session.
async fn edit_instruction_file<B: ratatui::backend::Backend + std::io::Write>(
    cli: &mut LiveCli,
    app: &mut App,
    ids: &BlockIdGen,
    terminal: &mut ratatui::Terminal<B>,
    path: &Path,
) -> Result<(), TuiLoopError> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, "").ok();
    }
    if run_editor_on_path(terminal, path).await {
        match cli.reload_context() {
            Ok(report) => push_report(app, ids, SystemLevel::Info, report),
            Err(error) => push_report(
                app,
                ids,
                SystemLevel::Warn,
                format!("Memory\n  Context reload failed — {error}"),
            ),
        }
    }
    // Generic over the backend `B` (see the fn signature), so it can't use the
    // `CrosstermBackend`-typed `draw_frame`; this single post-$EDITOR redraw
    // skips the 2026 wrap, which is harmless outside high-frequency streaming.
    app.draw(terminal)?;
    Ok(())
}

/// Encode the live MCP server statuses into the HUD's per-server rows.
/// Shared by the full HUD rebuild ([`build_hud_state`]) and the idle-tick
/// poller ([`install_mcp_status_poller`]) so the two surfaces cannot drift.
///
/// `try_lock` (not `lock`) on the render path with stale-while-revalidate:
/// background discovery holds this lock while a slow server hand-shakes, and a
/// blocking lock here would freeze the TUI. A `WouldBlock` keeps the last known
/// list instead of flashing MCP to zero mid-handshake. See
/// `server_statuses_stale_while_revalidate`.
pub(crate) fn encoded_mcp_hud_rows(
    state: &Arc<std::sync::Mutex<super::mcp_runtime::RuntimeMcpState>>,
) -> Vec<String> {
    super::mcp_runtime::server_statuses_stale_while_revalidate(state)
        .into_iter()
        .map(|status| match status.kind {
            super::mcp_runtime::McpServerStatusKind::Discovering => {
                McpHudStatus::discovering(status.name).encode()
            }
            super::mcp_runtime::McpServerStatusKind::Ready => {
                McpHudStatus::ready(status.name).encode()
            }
            super::mcp_runtime::McpServerStatusKind::AuthPending => {
                McpHudStatus::auth_pending(
                    status.name,
                    status
                        .message
                        .unwrap_or_else(|| "waiting for browser auth".to_string()),
                )
                .encode()
            }
            super::mcp_runtime::McpServerStatusKind::Failed => McpHudStatus::failed(
                status.name,
                status
                    .message
                    .unwrap_or_else(|| "discovery failed".to_string()),
            )
            .encode(),
        })
        .collect()
}

/// (Re-)arm the idle-tick MCP status poller with the CURRENT runtime's state
/// handle. Re-installed on every [`sync_app_context`] because `/resume`,
/// `/session switch`, and context reloads replace the runtime (and its
/// `mcp_state` Arc) — a poller captured once at startup would keep reading the
/// dead runtime's state forever.
fn install_mcp_status_poller(cli: &LiveCli, app: &mut App) {
    let mcp_state = cli.runtime.mcp_state.clone();
    app.set_mcp_status_poller(Box::new(move || {
        mcp_state.as_ref().map(encoded_mcp_hud_rows)
    }));
}

fn sync_app_context(cli: &LiveCli, app: &mut App) {
    let tool_registry = cli.runtime.api_client().tool_registry();
    let tool_context = tool_registry.context();
    app.set_background_process_count(
        tool_context
            .tasks
            .live_background_process_count(Some(&cli.session.id)),
    );
    let now_epoch = super::wakeups::now_epoch_secs();
    // Live `/loop` deadlines are held by this `LiveCli`'s in-memory controller;
    // project-persisted loops reload paused, so no sibling can poll and fire one.
    let loop_wake = cli.next_loop_wake(Instant::now()).map(|(due_in, reason)| {
        let rounded_seconds = due_in
            .as_secs()
            .saturating_add(u64::from(due_in.subsec_nanos() > 0));
        ScheduledWakeHud {
            due_at_epoch: now_epoch.saturating_add(rounded_seconds),
            reason,
            source: WakeSource::Loop,
        }
    });
    app.set_scheduled_loop_wake(loop_wake);
    app.refresh_scheduled_wakeup();
    let hud = build_hud_state(cli);
    app.set_prompt_commands(cli.runtime.prompt_commands.clone());
    sync_mcp_prompt_commands(cli, app);
    app.set_hud_state(hud);
    install_mcp_status_poller(cli, app);
}

/// Merge live-discovered MCP prompts into the palette as synthesized
/// prompt-command entries (`/mcp__server__prompt`). Version-gated so the
/// steady-state cost is one `try_lock` + integer compare per tick; a lock
/// held by background discovery simply defers the merge to the next tick.
fn sync_mcp_prompt_commands(cli: &LiveCli, app: &mut App) {
    let Some(mcp_state) = cli.runtime.mcp_state.as_ref() else {
        return;
    };
    let Ok(state) = mcp_state.try_lock() else {
        return;
    };
    let version = state.prompts_version();
    if app.mcp_prompts_version() == Some(version) {
        return;
    }
    let defs = state
        .prompts_snapshot()
        .into_iter()
        .map(|entry| commands::PromptCommandDef {
            description: entry
                .prompt
                .description
                .clone()
                .or_else(|| entry.prompt.title.clone())
                .or_else(|| Some(format!("MCP prompt from `{}`", entry.server))),
            argument_hint: mcp_prompt_argument_hint(&entry.prompt.arguments),
            model: None,
            effort: None,
            body: String::new(),
            allowed_tools: Vec::new(),
            path: std::path::PathBuf::from(format!("mcp://{}", entry.server)),
            name: entry.command,
        })
        .collect();
    drop(state);
    app.set_mcp_prompt_commands(version, defs);
}

/// `<name>` for required arguments, `[name]` for optional ones — the same
/// shapes the slash hint uses to decide fill-and-wait vs run-immediately.
fn mcp_prompt_argument_hint(arguments: &[runtime::McpPromptArgument]) -> Option<String> {
    if arguments.is_empty() {
        return None;
    }
    let rendered = arguments
        .iter()
        .map(|argument| {
            if argument.required.unwrap_or(false) {
                format!("<{}>", argument.name)
            } else {
                format!("[{}]", argument.name)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Some(rendered)
}

/// Install (or re-install after a session switch) the custom status line
/// poller: the App ticks it ~1/s with the live [`HudState`], the session-side
/// runner debounces and runs the configured command off-thread, and the
/// freshest cached line lands in `HudState::status_line`.
fn install_status_line_poller(cli: &LiveCli, app: &mut App) {
    let runner = std::sync::Arc::clone(&cli.status_line);
    let session_id = cli.session.id.clone();
    let transcript_path = cli.session.path.clone();
    let project_dir = cli.cwd.clone();
    app.set_status_line_poller(Box::new(move |hud| {
        runner.poll(&super::status_line::StatusLineInput {
            session_id: session_id.clone(),
            transcript_path: transcript_path.clone(),
            model_alias: hud.model.alias.clone(),
            model_display: hud.model.display_name.clone(),
            cwd: hud.cwd.clone(),
            project_dir: project_dir.clone(),
            cost_usd: hud.cost_usd,
            ctx_used: hud.ctx_used,
            ctx_limit: hud.ctx_limit,
            ctx_new_input: hud.ctx_new_input,
            ctx_cached: hud.ctx_cached,
        })
    }));
}

fn install_scheduled_wakeup_poller(
    cli: &LiveCli,
    app: &mut App,
    freshness: SessionFreshness,
) {
    let tool_context = cli.runtime.api_client().tool_registry().context().clone();
    app.set_scheduled_wakeup_poller(Box::new(move || {
        if !freshness.begin_scan(FreshnessDomain::Wakeups, Instant::now()) {
            return None;
        }
        let now_epoch = super::wakeups::now_epoch_secs();
        let session_id = tool_context.session_id();
        Some(
            super::wakeups::next_wakeup_info(now_epoch, session_id.as_deref()).map(|info| {
                ScheduledWakeHud {
                    due_at_epoch: now_epoch.saturating_add(info.due_in.as_secs()),
                    reason: info.reason,
                    source: WakeSource::Wakeup,
                }
            }),
        )
    }));
}

fn install_workspace_status_poller(app: &mut App, freshness: SessionFreshness) {
    app.set_workspace_status_poller(Box::new(move |cwd| {
        if !freshness.begin_scan(FreshnessDomain::Workspace, Instant::now()) {
            return None;
        }
        let source = freshness.workspace_status();
        let should_interrupt = freshness.dirty_flag(FreshnessDomain::Workspace);
        let cwd = cwd.to_path_buf();
        Some(tokio::task::spawn_blocking(move || {
            source.snapshot(&cwd, should_interrupt).ok()
        }))
    }));
}

fn provider_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "claude",
        ProviderKind::Xai => "xai",
        ProviderKind::OpenAi => "openai",
        ProviderKind::Google => "google",
        ProviderKind::Ollama => "ollama",
    }
}

#[allow(clippy::too_many_lines)]
fn build_hud_state(cli: &LiveCli) -> HudState {
    let resolved_model = resolve_model_alias(&cli.model);
    let permission_mode = match cli.permission_mode {
        runtime::PermissionMode::ReadOnly => PermissionMode::ReadOnly,
        runtime::PermissionMode::WorkspaceWrite | runtime::PermissionMode::Prompt => {
            PermissionMode::Workspace
        }
        runtime::PermissionMode::Allow | runtime::PermissionMode::DangerFullAccess => {
            PermissionMode::All
        }
    };
    let (git_branch, changed_files) = hud_git_snapshot(cli.cwd.as_path());
    let sandbox_status =
        runtime::resolve_sandbox_status(cli.runtime.feature_config.sandbox(), cli.cwd.as_path());
    let security_posture = security_posture_from_sandbox(&sandbox_status);
    let session = cli.runtime.session();
    let (bash_count, read_count, edit_count) = summarize_tool_results(session);
    let todo_items = todo_items();
    let todo_summary = todo_summary(&todo_items);
    let (ctx_estimate, current_usage, cumulative_usage) = cli
        .runtime
        .runtime
        .as_ref()
        .map(|runtime| {
            let usage = runtime.usage();
            (
                runtime.estimated_tokens() as u64,
                usage.current_turn_usage(),
                usage.cumulative_usage(),
            )
        })
        .unwrap_or_default();
    // Context occupancy and billing usage are different dimensions.
    // `cumulative_usage.total_tokens()` grows across the whole session
    // and can exceed the model context window, so using it for HUD ctx
    // produced impossible displays like `1.4M / 1.0M 137%`. Keep cost on
    // cumulative usage, but context on the *latest turn's* provider count
    // (input + cache read/write) — the real window occupancy. Fall back to
    // the local chars/4 estimate only before the first billed turn so the
    // ledger still shows a non-zero figure immediately.
    // The new/cached breakdown is withheld while it belongs to an earlier model
    // or an earlier run (see `LiveCli::hud_cache_split_is_stale`) — prompt caches
    // are per provider and per model, so showing one model's cache reads under
    // another's name is what reads as a cache carried across vendors.
    let context_usage =
        hud_context_usage(ctx_estimate, current_usage, cli.hud_cache_split_is_stale());
    let pricing = runtime::pricing_for_model(&cli.model);
    let cost_usd = cumulative_usage
        .estimate_cost_usd_with_pricing(
            pricing.unwrap_or_else(runtime::ModelPricing::default_sonnet_tier),
        )
        .total_cost_usd();

    let ctx_limit_tokens = context_window_for_model(&cli.model);
    // The live runtime's threshold is authoritative for "compacts at": it is
    // the exact value the compaction gate enforces, folding in the model-family
    // policy, the env override, AND the settings `autoCompactThresholdPercent`
    // override, which the model-only free function below cannot see. Fall back
    // to the env-or-policy derivation only before a runtime exists.
    let compact_threshold = cli.runtime.runtime.as_ref().map_or_else(
        || {
            u64::from(runtime::auto_compaction_threshold_for_model(
                Some(&cli.model),
                ctx_limit_tokens,
            ))
        },
        |runtime| u64::from(runtime.auto_compaction_input_tokens_threshold()),
    );
    let provider = provider_label(detect_provider_kind(&resolved_model));
    let mcp_servers = cli
        .runtime
        .mcp_state
        .as_ref()
        .map_or_else(Vec::new, encoded_mcp_hud_rows);
    let lsp_servers = lsp_status_items(cli);
    // One scan is the source of truth: the headline `⚡ N agents` count is derived
    // from the same live list (running/pending rows), so the count and the
    // expanded tree can never disagree — and both use the status-aware freshness
    // gate instead of a raw mtime window that drops agents mid model-turn.
    let agents = list_running_agents_since(
        cli.agent_manifest_started_after,
        Some(cli.session.id.as_str()),
    );

    HudState {
        session_identity: SessionIdentity::named(&session.session_id, session.name.as_deref()),
        model: ActiveModel {
            provider,
            alias: cli.model.clone(),
            display_name: resolved_model.clone(),
            context_limit: u32::try_from(ctx_limit_tokens).unwrap_or(u32::MAX),
        },
        turn_fallback_model: None,
        quota_fallback_model: None,
        ctx_used: context_usage.used,
        ctx_limit: ctx_limit_tokens,
        ctx_new_input: context_usage.new_input,
        ctx_cached: context_usage.cached,
        compact_threshold,
        cost_usd,
        cost_approx: pricing.is_none(),
        cwd: cli.cwd.clone(),
        git_branch,
        perm_mode: permission_mode,
        security_posture,
        effort: cli.effort,
        // Set only when `smart.execSwap` armed for the most recent turn's
        // classified difficulty and built an implementer client. Native EXEC
        // turns therefore keep the plain main-model anchor.
        architect_impl: cli
            .exec_impl_provider
            .as_ref()
            .map(|(model, _, _)| model.clone()),
        mcp_servers,
        bash_count,
        read_count,
        edit_count,
        changed_files,
        todo_summary,
        todo_items,
        automation_lines: cli.automation_hud_lines(),
        lsp_servers,
        running_agents: running_count(&agents),
        agents,
        workflow: zo_cli::tui::workflow_progress::read_summary_since(
            cli.agent_manifest_started_after,
            Some(&cli.session.id),
        ),
        last_tool: None,
        rate_limit: None,
        // Cross-provider quota rows, re-derived from the process-global
        // `api::quota` state on every rebuild — atomics plus one small mutex
        // read, cheap enough for the HUD cadence. This is the sidebar's single
        // source: it already prefers a measured reading over the response
        // headers over a 429 guess.
        //
        // The kick below is what fills the measured half. It is throttled to a
        // cadence inside `quota_probe` and never awaits, so calling it from a
        // rebuild path costs an atomic load in the common case; without it the
        // account's real headroom is only ever known after a request has
        // already been spent.
        provider_quotas: {
            api::quota_probe::refresh_measured_quotas_soon();
            api::quota::provider_quota_views()
        },
        auth_origin: api::latest_claude_auth_origin(),
        status_line: cli.status_line.current(),
        team_inbox_unread: runtime::team_inbox_unread_count(&cli.cwd, &cli.session.id),
        // Throttled + latched (see `stale_binary`): once the running binary is
        // replaced on disk this stays `Some` and the sidebar shows a `/restart`
        // warning. Cheap to call every HUD build.
        stale_binary: zo_cli::tui::stale_binary::check(),
        // App replaces this from the exact foreground runtime's session-scoped
        // atomic handle; HUD construction never scans or locks TaskRegistry.
        background_tasks: 0,
        // App replaces this from its cached scheduler sources.
        scheduled_wake: None,
        block_focused: false,
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct HudContextUsage {
    used: u64,
    new_input: u64,
    cached: u64,
}

fn hud_context_usage(
    ctx_estimate: u64,
    usage: runtime::TokenUsage,
    split_stale: bool,
) -> HudContextUsage {
    let provider_ctx = u64::from(usage.context_tokens());
    HudContextUsage {
        // Window occupancy still holds across a switch — only the attribution
        // of the new/cached breakdown to a model does not.
        used: if provider_ctx > 0 {
            provider_ctx
        } else {
            ctx_estimate
        },
        new_input: if split_stale {
            0
        } else {
            u64::from(
                usage
                    .input_tokens
                    .saturating_add(usage.cache_creation_input_tokens),
            )
        },
        cached: if split_stale {
            0
        } else {
            u64::from(usage.cache_read_input_tokens)
        },
    }
}

/// Cached branch + changed-file count for the HUD, refreshed off-thread.
///
/// `build_hud_state` runs on the TUI select! thread at loop entry and on every
/// context resync (each loop iteration, turn boundaries, modal actions). It
/// used to call `status_context` — a full config re-discovery plus
/// `ProjectContext::discover_with_git`, i.e. blocking `git` subprocesses and an
/// instruction-file walk on the render thread. When `git status` stalled
/// (index lock held by a concurrent session/build, cold disk on a low-spec
/// machine) the whole event loop froze for the duration — the main-thread
/// `posix_spawn`/`poll`/`pthread_join` stacks the freeze watchdog kept
/// capturing. The HUD needs exactly two facts (branch, changed count), so they
/// are served from this snapshot and refreshed on the dedicated HUD runtime;
/// the render thread never forks `git`.
struct HudGitSnapshot {
    /// Workspace the snapshot was taken in; a mismatch (session moved cwd)
    /// invalidates it rather than showing another workspace's branch.
    cwd: PathBuf,
    branch: Option<String>,
    changed_files: usize,
    refreshed_at: Instant,
}

static HUD_GIT_SNAPSHOT: std::sync::Mutex<Option<HudGitSnapshot>> = std::sync::Mutex::new(None);
/// One refresh in flight at a time: a slow `git status` must coalesce
/// followers, not stack duplicate forks behind itself.
static HUD_GIT_REFRESH_INFLIGHT: AtomicBool = AtomicBool::new(false);
/// Steady-state staleness bound for the HUD facts. Resyncs inside the window
/// reuse the snapshot; the first one past it triggers a background refresh.
const HUD_GIT_REFRESH_INTERVAL: Duration = Duration::from_secs(3);

fn hud_git_snapshot(cwd: &Path) -> (Option<String>, usize) {
    let cached = {
        let guard = HUD_GIT_SNAPSHOT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.as_ref().and_then(|snapshot| {
            (snapshot.cwd == cwd).then(|| {
                (
                    snapshot.branch.clone(),
                    snapshot.changed_files,
                    snapshot.refreshed_at,
                )
            })
        })
    };
    let refresh_due = cached
        .as_ref()
        .is_none_or(|(_, _, refreshed_at)| refreshed_at.elapsed() >= HUD_GIT_REFRESH_INTERVAL);
    if refresh_due
        && !HUD_GIT_REFRESH_INFLIGHT.swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        let cwd = cwd.to_path_buf();
        super::turn_controller::session_hud::hud_runtime().spawn_blocking(move || {
            /// Clears the in-flight flag on every exit path (including an
            /// unwind out of the parsers) so one failed refresh can never
            /// freeze the snapshot forever.
            struct InflightReset;
            impl Drop for InflightReset {
                fn drop(&mut self) {
                    HUD_GIT_REFRESH_INFLIGHT.store(false, std::sync::atomic::Ordering::Release);
                }
            }
            let _reset = InflightReset;
            let status = read_hud_git_status(&cwd);
            let (_, branch) = crate::git_helpers::parse_git_status_metadata(status.as_deref());
            let changed_files =
                crate::git_helpers::parse_git_workspace_summary(status.as_deref()).changed_files;
            *HUD_GIT_SNAPSHOT
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(HudGitSnapshot {
                cwd,
                branch,
                changed_files,
                refreshed_at: Instant::now(),
            });
        });
    }
    cached.map_or((None, 0), |(branch, changed_files, _)| {
        (branch, changed_files)
    })
}

/// The same bounded `git status` read the system-prompt discovery uses,
/// `--no-optional-locks` included so the HUD poll can never contend on an
/// index lock a real git operation holds.
fn read_hud_git_status(cwd: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args([
            "--no-optional-locks",
            "status",
            "--short",
            "--branch",
            "--",
            ":(exclude)target",
            ":(exclude)node_modules",
            ":(exclude).build",
        ])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn security_posture_from_sandbox(status: &runtime::SandboxStatus) -> SecurityPosture {
    if !status.enabled {
        return SecurityPosture::SandboxOff;
    }
    if status.fallback_reason.is_some() {
        return SecurityPosture::SandboxBlocked;
    }
    SecurityPosture::SandboxActive
}

/// A *terminal* agent (completed/failed/stopped) stays in the live tree only this
/// long after its last manifest write, so the view stays focused on active work.
const TERMINAL_GRACE_SECS: u64 = 8;

/// A *running* agent writes its manifest only on tool calls and at terminal —
/// during a long model turn it is silent — so mtime is NOT a short liveness
/// signal. Past this backstop it is treated as an abandoned worker. Match the
/// foreground multi-agent wait window: real agents can be quiet for many minutes
/// and should not disappear from the HUD while the parent is still waiting for
/// their actual result.
const RUNNING_STALE_SECS: u64 = 20 * 60;
/// Once a running/pending agent passes [`RUNNING_STALE_SECS`] it has gone quiet
/// (no tool call, no stream chunk, no phase write). Rather than drop it — which
/// left the HUD reading `agents 0/1` with a blank inline row, as if nothing had
/// happened — keep surfacing it as `stalled Nm ago` up to this hard ceiling so
/// a hung sub-agent is visibly hung. Past this it is treated as truly abandoned
/// and finally drops, so an old zombie manifest never lingers forever.
const STALLED_SURFACE_LIMIT_SECS: u64 = 40 * 60;
/// CC parity: the `Notification` hook also fires after this much prompt idle
/// (Claude Code notifies at 60s of waiting for input). One shot per wait —
/// the timer resets when the user acts and a new wait begins.
const NOTIFICATION_IDLE_SECS: u64 = 60;

const MAX_AGENT_SUMMARIES: usize = 32;
const MAX_AGENT_MANIFEST_READS: usize = 128;

/// Headline count for the HUD's `⚡ N agents`: the genuinely-running (running /
/// pending) rows in a [`list_running_agents`] result, excluding the brief terminal
/// grace. Derived from the same list the tree shows, so the two never disagree —
/// and, unlike the old 15s mtime window, an agent in a long model turn is still
/// counted (its `status` is `running` even while its manifest is quiet), instead
/// of the count dropping to 0 mid-turn.
pub(crate) fn running_count(agents: &[AgentTaskSummary]) -> u16 {
    let running = agents
        .iter()
        .filter(|a| !matches!(a.status.as_str(), "completed" | "failed" | "stopped"))
        .count();
    u16::try_from(running).unwrap_or(u16::MAX)
}

/// The single source of truth for live agents: parses each manifest's content to
/// return a `(name, status, elapsed_secs, …)` summary, status-aware-gated for
/// freshness. The sidebar tree shows these rows and [`running_count`] derives the
/// `⚡ N agents` headline from the same list, so count and tree never disagree.
///
/// Cost: one `read_to_string + serde_json::from_str` per manifest, capped at 32
/// entries to avoid stalls when manifests grow quickly. Call frequency is low
/// enough (`App::update_hud_live_snapshot` every ~3s, mid-turn around 500ms)
/// that it should not disturb the render loop.
#[allow(clippy::too_many_lines)] // cohesive manifest scan + freshness filter; splitting obscures it
pub(crate) fn list_running_agents_since(
    started_after: u64,
    session_id: Option<&str>,
) -> Vec<AgentTaskSummary> {
    let Ok(store) = tools::agent_store_dir() else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let now_secs = epoch_secs_from_system_time(now);
    let mut summaries: Vec<AgentTaskSummary> = Vec::new();

    let manifests = zo_cli::tui::agent_manifests::newest_first_cached(&store);
    for (reads, (path, modified)) in manifests.iter().enumerate() {
        if summaries.len() >= MAX_AGENT_SUMMARIES || reads >= MAX_AGENT_MANIFEST_READS {
            break;
        }
        let mut age_secs = manifest_age_secs(now, *modified);

        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if !manifest_created_after(&value, started_after) {
            continue;
        }
        // The live HUD scopes strictly to the current session: an unstamped
        // manifest (legacy, or a benchmark/headless agent that never carried a
        // session id) must NOT bleed into this session's sidebar, so pass
        // `allow_unstamped = false`. This is the cross-session agent-sharing fix.
        if !zo_cli::tui::agent_session_filter::manifest_belongs_to_session(
            &value, session_id, false,
        ) {
            continue;
        }
        // The HUD fleet is the single in-memory source the sidebar, the inline
        // fan-out panel, and the viewers' spawn-window fallback all read, so
        // dropping the internal classifier here hides it from every one of them
        // at once — and from the running/total counters derived from this list.
        if zo_cli::tui::agent_session_filter::manifest_is_internal_classifier(&value) {
            continue;
        }
        let reconciled_dead_worker = value
            .get("agentId")
            .and_then(serde_json::Value::as_str)
            .is_some_and(tools::reconcile_dead_agent_worker);
        if reconciled_dead_worker {
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            let Ok(reconciled) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            value = reconciled;
            age_secs = 0;
        }
        let name = value
            .get("label")
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.get("name").and_then(serde_json::Value::as_str))
            .unwrap_or("agent")
            .to_string();
        let status = value
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("running")
            .to_string();
        // Status-aware freshness gate. The manifest is written only on tool calls
        // and at terminal, so a *running* agent in a long model turn is silent and
        // mtime is NOT its liveness signal — keep running/pending visible until the
        // worker is almost certainly dead (RUNNING_STALE_SECS), and drop a terminal
        // one after a brief grace (TERMINAL_GRACE_SECS). This is what stops live
        // agents from vanishing from the HUD mid model-turn.
        // Drop, or keep-and-surface-as-stalled, based on how long the manifest has
        // been quiet (see `agent_freshness`) — with the in-process worker
        // registry as the final word before dropping a running agent.
        let Some(stalled) = agent_freshness_with_liveness(&status, age_secs, || {
            value
                .get("agentId")
                .and_then(serde_json::Value::as_str)
                .is_some_and(tools::agent_worker_is_live)
        }) else {
            continue;
        };
        let elapsed_secs = agent_elapsed_secs(&value, now_secs, age_secs);
        let token_history = value
            .get("tokenHistory")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_u64().map(|n| u32::try_from(n).unwrap_or(u32::MAX)))
                    .collect::<Vec<u32>>()
            })
            .unwrap_or_default();
        let current_tool = value
            .get("currentTool")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let current_phase = value
            .get("currentPhase")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        // Last streamed chars (rolling `outputTail` buffer) — surfaced as a dim
        // `⤷ …` sub-line under each agent's row so the inline / pinned tree shows
        // *what* an agent is producing, not just its current tool.
        let output_tail = value
            .get("outputTail")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|tail| !tail.is_empty())
            .map(str::to_string);
        let last_activity_at = value
            .get("lastActivityAt")
            .and_then(serde_json::Value::as_u64);
        let model = value
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map_or_else(String::new, |model| model.trim().to_string());
        let route_reason = value
            .get("routeReason")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map(str::to_string);
        let id = value
            .get("agentId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let tool_call_id = value
            .get("toolCallId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        let subagent_type = value
            .get("subagentType")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let tool_calls = value
            .get("toolCalls")
            .and_then(serde_json::Value::as_u64)
            .and_then(|count| usize::try_from(count).ok());
        let tokens = token_history.iter().map(|n| u64::from(*n)).sum();
        let created_at = value
            .get("createdAt")
            .and_then(serde_json::Value::as_u64)
            .or_else(|| value.get("created_at").and_then(serde_json::Value::as_u64));
        // A stalled agent overrides its (now stale) current tool/phase with a
        // `stalled Nm ago` activity so the inline tree, sidebar and HUD all read it
        // as visibly hung — `activity_label()` surfaces the phase once the tool is
        // cleared. The minute figure is the idle gap (`age_secs`), not total runtime.
        let (current_tool, current_phase) = if stalled {
            (None, Some(format!("stalled {}m ago", age_secs / 60)))
        } else {
            (current_tool, current_phase)
        };
        summaries.push(AgentTaskSummary {
            id,
            tool_call_id,
            name,
            status,
            model,
            elapsed_secs,
            token_history,
            current_tool,
            current_phase,
            last_activity_at,
            subagent_type,
            tool_calls,
            tokens,
            created_at,
            output_tail,
            route_reason,
        });
    }
    summaries
}

fn manifest_created_after(value: &serde_json::Value, started_after: u64) -> bool {
    if started_after == 0 {
        return true;
    }
    let created = value
        .get("createdAt")
        .or_else(|| value.get("created_at"))
        .and_then(|value| match value {
            serde_json::Value::Number(number) => number.as_u64(),
            serde_json::Value::String(text) => text.trim().parse::<u64>().ok(),
            _ => None,
        });
    // Legacy manifests without a created timestamp cannot be safely attributed
    // to the current session, so keep them out of the live HUD.
    created.is_some_and(|created| created >= started_after)
}

fn manifest_age_secs(now: SystemTime, modified: SystemTime) -> u64 {
    now.duration_since(modified)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn epoch_secs_from_system_time(time: SystemTime) -> u64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn agent_elapsed_secs(value: &serde_json::Value, now_secs: u64, modified_age_secs: u64) -> u64 {
    let created = value
        .get("createdAt")
        .or_else(|| value.get("created_at"))
        .and_then(parse_epoch_field);
    let completed = value
        .get("completedAt")
        .or_else(|| value.get("completed_at"))
        .and_then(parse_epoch_field);

    match (created, completed) {
        (Some(start), Some(end)) => end.saturating_sub(start),
        (Some(start), None) => now_secs.saturating_sub(start),
        _ => modified_age_secs,
    }
}

fn parse_epoch_field(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn agent_stale_limit_secs(status: &str) -> u64 {
    if matches!(status, "completed" | "failed" | "stopped") {
        TERMINAL_GRACE_SECS
    } else {
        RUNNING_STALE_SECS
    }
}

/// Liveness decision for one manifest given its idle `age_secs` (seconds since
/// the file was last written). `None` ⇒ drop it from the live list; `Some(false)`
/// ⇒ keep it, fresh; `Some(true)` ⇒ keep it but surface it as **stalled**.
///
/// A terminal manifest drops on its short grace ([`TERMINAL_GRACE_SECS`]). A
/// running/pending one stays fresh until [`RUNNING_STALE_SECS`], is then surfaced
/// as stalled up to [`STALLED_SURFACE_LIMIT_SECS`] — so a hung sub-agent reads as
/// visibly hung instead of vanishing into `agents 0/1` — and only drops past that
/// hard ceiling, so an abandoned zombie can't linger forever.
fn agent_freshness(status: &str, age_secs: u64) -> Option<bool> {
    let is_terminal = matches!(status, "completed" | "failed" | "stopped");
    let drop_limit = if is_terminal {
        agent_stale_limit_secs(status)
    } else {
        STALLED_SURFACE_LIMIT_SECS
    };
    if age_secs > drop_limit {
        return None;
    }
    Some(!is_terminal && age_secs > agent_stale_limit_secs(status))
}

/// [`agent_freshness`] with an in-process liveness rescue: a *running*
/// manifest can go quiet past the drop window through one long tool call
/// (a cold `cargo build` writes nothing for 40+ minutes), and dropping it
/// makes a working agent vanish from the HUD. Sub-agents run in-process, so
/// when the mtime verdict says "drop" we ask the worker registry instead —
/// a live worker keeps the row, surfaced as stalled; a dead one drops as
/// before. Terminal manifests never take the rescue.
fn agent_freshness_with_liveness(
    status: &str,
    age_secs: u64,
    worker_is_live: impl FnOnce() -> bool,
) -> Option<bool> {
    agent_freshness(status, age_secs).or_else(|| {
        let is_terminal = matches!(status, "completed" | "failed" | "stopped");
        (!is_terminal && worker_is_live()).then_some(true)
    })
}

fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

fn summarize_tool_results(session: &runtime::Session) -> (u32, u32, u32) {
    let mut bash: u32 = 0;
    let mut read: u32 = 0;
    let mut edit: u32 = 0;
    for message in session.messages.iter() {
        for block in &message.blocks {
            if let ContentBlock::ToolResult { tool_name, .. } = block {
                if contains_ascii_ci(tool_name, "bash") {
                    bash += 1;
                } else if contains_ascii_ci(tool_name, "read")
                    || contains_ascii_ci(tool_name, "grep")
                    || contains_ascii_ci(tool_name, "glob")
                {
                    read += 1;
                } else if contains_ascii_ci(tool_name, "edit")
                    || contains_ascii_ci(tool_name, "write")
                {
                    edit += 1;
                }
            }
        }
    }
    (bash, read, edit)
}

fn maybe_auto_open_review_diff(cli: &LiveCli, app: &mut App, summary: &TurnSummary) {
    let Some(threshold) = cli.runtime.feature_config.review().auto_after_edits() else {
        return;
    };
    if !should_auto_open_review_diff(summary, threshold) {
        return;
    }
    let diff_text = git_diff_head_in(&cli.cwd);
    let files = zo_cli::tui::modals::diff_viewer::parse_unified_diff(&diff_text);
    if !files.is_empty() {
        app.open_diff_viewer(files);
    }
}

fn should_auto_open_review_diff(summary: &TurnSummary, threshold: u32) -> bool {
    threshold > 0 && turn_edit_write_count(summary) >= threshold
}

fn turn_edit_write_count(summary: &TurnSummary) -> u32 {
    let count = summary
        .tool_results
        .iter()
        .flat_map(|message| &message.blocks)
        .filter(|block| {
            matches!(
                block,
                ContentBlock::ToolResult { tool_name, .. }
                    if contains_ascii_ci(tool_name, "edit")
                        || contains_ascii_ci(tool_name, "write")
            )
        })
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn git_diff_head_in(cwd: &Path) -> String {
    std::process::Command::new("git")
        .current_dir(cwd)
        .args(["--no-optional-locks", "diff", "HEAD", "--no-color"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

fn todo_summary(items: &[TodoChecklistItem]) -> Option<String> {
    zo_cli::tui::hud::active_todo_summary(items)
}

pub(crate) fn todo_items() -> Vec<TodoChecklistItem> {
    let cwd = crate::current_cli_cwd().ok();
    let Some(store_path) = todo_store_path_for_hud(cwd.as_deref()) else {
        return Vec::new();
    };
    load_todo_items_for_hud(&store_path)
}

fn lsp_status_items(cli: &LiveCli) -> Vec<LspStatusItem> {
    let mut servers = cli
        .runtime
        .lsp_state
        .as_ref()
        .map_or_else(Vec::new, |state| {
            let guard = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard
                .registry
                .list_servers()
                .into_iter()
                .map(|server| LspStatusItem {
                    language: server.language,
                    status: server.status.to_string(),
                })
                .collect()
        });
    servers.sort_by(|left, right| left.language.cmp(&right.language));
    servers
}

/// Set when `init_terminal` pushed Kitty keyboard-enhancement flags, so
/// `restore_terminal` knows to pop them without a second capability probe.
static KEYBOARD_ENHANCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Set only while the primary-screen inline terminal owns stdout. Panic
/// recovery consults it so it never emits `LeaveAlternateScreen` for a session
/// that did not enter the alternate screen.
static INLINE_TERMINAL_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// The concrete ratatui terminal the live TUI drives.
///
/// Frames go through [`zo_cli::tui::term::FrameWriter`], which buffers a whole
/// frame in memory and hands it to a writer thread. Two properties matter here:
/// one frame is one write, so a slow terminal cannot paint a half-finished
/// cell-diff (the old `BufWriter` did this with a 256 KiB capacity, and split
/// anything larger); and the write itself happens off the async main task, so a
/// terminal that stops draining no longer blocks input, the spinner, or Ctrl+C
/// — every freeze captured since 2026-08-02 was exactly that stall.
pub(crate) type TuiTerminal = Terminal<CrosstermBackend<zo_cli::tui::term::FrameWriter>>;

/// How long a handoff of the terminal — viewport rebuild, teardown — waits for
/// queued frames to reach it. Generous enough for any healthy terminal, short
/// enough that a wedged one delays an exit by a beat instead of hanging it.
const TERMINAL_HANDOFF_DRAIN: std::time::Duration = std::time::Duration::from_millis(750);

#[cfg(not(test))]
fn set_terminal_session_title(terminal: &mut TuiTerminal, name: Option<&str>) {
    use std::io::IsTerminal;

    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return;
    };
    if io::stdout().is_terminal() {
        let _ = crossterm::execute!(
            terminal.backend_mut(),
            crossterm::terminal::SetTitle(format!("Zo · {name}"))
        );
    }
}

#[cfg(test)]
fn set_terminal_session_title(_terminal: &mut TuiTerminal, _name: Option<&str>) {}

/// Spawn the freeze watchdog: a background thread that samples the TUI event
/// loops' liveness heartbeat (`zo_cli::tui::watchdog`) and writes a
/// verdict to the log when the main loop stalls. This turns the intermittent
/// "TUI freezes when I go to type again" report into a *fact* — it says whether
/// zo's own async loop wedged (a zo-side hang) or kept running while the
/// screen froze (a terminal-side / xterm-parser stall). Idempotent; opt out
/// with `ZO_FREEZE_WATCHDOG=0`.
fn spawn_freeze_watchdog() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::Relaxed) {
        return;
    }
    if std::env::var("ZO_FREEZE_WATCHDOG")
        .map(|value| value == "0" || value.eq_ignore_ascii_case("off"))
        .unwrap_or(false)
    {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("zo-freeze-watchdog".to_string())
        .spawn(|| {
            use zo_cli::tui::watchdog::{beat_count, phase_label};
            // 1 s sampling; report once the loop has been silent this many
            // consecutive samples. 5 s is far above any legitimate per-frame or
            // between-turn synchronous segment, so a trip means a real stall.
            const POLL: Duration = Duration::from_secs(1);
            const STALL_SECS: u64 = 5;
            let mut last_beat = beat_count();
            let mut stalled_secs: u64 = 0;
            let mut reported = false;
            loop {
                std::thread::sleep(POLL);
                // Only watch while the TUI holds the terminal. During a suspended
                // sub-shell ($EDITOR, `/memory`) the loops legitimately stop
                // beating; flagging that would be a false alarm.
                if !crate::tui_active() {
                    last_beat = beat_count();
                    stalled_secs = 0;
                    reported = false;
                    continue;
                }
                let beat = beat_count();
                if beat == last_beat {
                    stalled_secs += 1;
                    if stalled_secs >= STALL_SECS && !reported {
                        let pid = std::process::id();
                        match capture_freeze_stack(pid) {
                            // Read the sample we just took instead of asserting
                            // whose stall this is: a render-loop write to a
                            // terminal that stopped draining freezes the beat
                            // exactly like a wedged loop does, and calling that
                            // a "ZO-SIDE hang" sent the last investigation
                            // looking for a deadlock that was not there.
                            Some(path) => eprintln!(
                                "[FREEZE-WATCHDOG] zo main TUI event loop has not advanced for ~{stalled_secs}s (beat={beat}, pid={pid}, phase=\"{}\"). Stack says: {} Sample: {}",
                                phase_label(),
                                std::fs::read_to_string(&path)
                                    .map(|report| zo_cli::tui::watchdog::classify_freeze_sample(&report))
                                    // A capture we could not even read back is
                                    // an unread capture, not a read one whose
                                    // leaves tied.
                                    .unwrap_or(zo_cli::tui::watchdog::FreezeVerdict::UnreadableCapture)
                                    .sentence(),
                                path.display(),
                            ),
                            None => eprintln!(
                                "[FREEZE-WATCHDOG] zo main TUI event loop has not advanced for ~{stalled_secs}s (beat={beat}, pid={pid}, phase=\"{}\"). No stack was captured, so whose stall this is stays unknown. Capture it with: lldb -p {pid} -o 'thread backtrace all' -o detach -o quit   (or: sample {pid} 5 -f /tmp/zo-freeze.sample)",
                                phase_label(),
                            ),
                        }
                        reported = true;
                    }
                } else {
                    if reported {
                        eprintln!(
                            "[FREEZE-WATCHDOG] zo main TUI event loop resumed after ~{stalled_secs}s stall (beat {last_beat} \u{2192} {beat})."
                        );
                    }
                    last_beat = beat;
                    stalled_secs = 0;
                    reported = false;
                }
            }
        });
}

/// Auto-capture a full-process stack sample when the freeze watchdog trips, so a
/// zo-side hang names *what* is blocking without the user having to attach a
/// debugger by hand — the static analysis ruled out every obvious blocking path,
/// so the live stack is the only thing that can root-cause the remaining stall.
///
/// Runs the platform stack sampler with a hard timeout (the sampler must never
/// itself wedge the watchdog thread) and writes to a timestamped file under the
/// same `~/.zo/logs/` dir the verdict log lives in. Returns the path on
/// success, `None` if no sampler is available or it failed — the caller then
/// falls back to printing the manual capture command. Best-effort: never panics.
#[cfg(target_os = "macos")]
fn capture_freeze_stack(pid: u32) -> Option<std::path::PathBuf> {
    use std::process::{Command, Stdio};

    let out_path = freeze_stack_path();
    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // `sample <pid> <secs>`: a built-in macOS profiler that does not need
    // ptrace permission on the developer's own process. 2 s is long enough to
    // catch the stuck frame, short enough not to delay the verdict.
    let status = Command::new("/usr/bin/sample")
        .arg(pid.to_string())
        .arg("2")
        .arg("-file")
        .arg(&out_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    (status.success() && out_path.exists()).then_some(out_path)
}

/// Linux fallback: capture kernel-reported per-thread stacks from `/proc`, which
/// needs no extra tooling. Best-effort; returns the file path on success.
#[cfg(target_os = "linux")]
fn capture_freeze_stack(pid: u32) -> Option<std::path::PathBuf> {
    use std::fmt::Write as _;

    let task_dir = format!("/proc/{pid}/task");
    let mut dump = String::new();
    for entry in std::fs::read_dir(&task_dir).ok()? {
        let Ok(entry) = entry else { continue };
        let tid = entry.file_name();
        let stack_path = entry.path().join("stack");
        let comm = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        let stack = std::fs::read_to_string(&stack_path)
            .unwrap_or_else(|_| "(kernel stack unavailable; run with CONFIG_STACKTRACE)\n".into());
        let _ = write!(
            dump,
            "--- thread {} ({}) ---\n{}\n",
            tid.to_string_lossy(),
            comm.trim(),
            stack
        );
    }
    if dump.is_empty() {
        return None;
    }
    let out_path = freeze_stack_path();
    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&out_path, dump).ok()?;
    Some(out_path)
}

/// Other platforms have no zero-config sampler wired up; fall back to the manual
/// capture hint.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn capture_freeze_stack(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

/// Timestamped sample path next to the verdict log, so repeated trips in one
/// session never overwrite each other.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn freeze_stack_path() -> std::path::PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    stderr_redirect::default_log_path()
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf)
        .join(format!("zo-freeze-{secs}.sample"))
}

pub(crate) fn init_terminal(
    mode: TerminalMode,
) -> Result<(TuiTerminal, Option<StderrRedirectGuard>), TuiLoopError> {
    // Arm the fatal-signal restore at the one choke point both modes pass
    // through, so a `kill` or a closed window during the session cannot hand the
    // shell back in raw + alternate-screen mode.
    spawn_terminal_restoring_signal_handler();
    match mode {
        TerminalMode::Fullscreen => init_fullscreen_terminal(),
        TerminalMode::Inline => init_inline_terminal(),
    }
}

fn init_fullscreen_terminal() -> Result<(TuiTerminal, Option<StderrRedirectGuard>), TuiLoopError> {
    use crossterm::event::{EnableBracketedPaste, EnableFocusChange, EnableMouseCapture};
    use crossterm::execute;
    use crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};

    // Redirect stderr before raw mode so later `eprintln!` output (retries,
    // MCP reconnects, panic traces) cannot paint directly over ratatui frames.
    // Activation failure is non-fatal; with read-only filesystems or similar
    // constraints, the TUI still runs but loses that display protection.
    let stderr_guard = StderrRedirectGuard::activate(stderr_redirect::default_log_path()).ok();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Mouse capture must start enabled: collapsed rows, hover affordances,
    // wheel scrolling, and the scrollbar all depend on receiving mouse events.
    // EnableFocusChange lets crossterm emit FocusGained/FocusLost so the TUI
    // can redraw after the user switches windows.
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    )?;
    // Opt into the Kitty keyboard protocol when the terminal advertises
    // support so modified keys (Shift+Enter / Ctrl+Enter for multiline,
    // disambiguated Esc) arrive as distinct Press events. No-op on
    // terminals that don't support it (e.g. Apple Terminal). We push only
    // DISAMBIGUATE_ESCAPE_CODES — that stays on Press-only events (no
    // REPORT_EVENT_TYPES), and `App::handle_key` already drops any
    // non-Press events defensively. JediTerm (IntelliJ) is excluded even
    // when it reports support, because its incomplete implementation breaks
    // key input once the flags are pushed (see `keyboard_enhancement_disabled`).
    if !zo_cli::tui::term::TermProfile::current().kitty_keyboard_disabled
        && matches!(
            crossterm::terminal::supports_keyboard_enhancement(),
            Ok(true)
        )
    {
        use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
        if execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
        {
            KEYBOARD_ENHANCED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    crate::mark_tui_thread();
    crate::TUI_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    let backend = CrosstermBackend::new(zo_cli::tui::term::FrameWriter::stdout());
    let terminal = Terminal::new(backend).map_err(TuiLoopError::Io)?;
    Ok((terminal, stderr_guard))
}

fn init_inline_terminal() -> Result<(TuiTerminal, Option<StderrRedirectGuard>), TuiLoopError> {
    use crossterm::event::{EnableBracketedPaste, EnableFocusChange, EnableMouseCapture};
    use crossterm::execute;
    use crossterm::terminal::enable_raw_mode;
    use ratatui::{TerminalOptions, Viewport};

    let stderr_guard = StderrRedirectGuard::activate(stderr_redirect::default_log_path()).ok();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Inline stays on the primary screen, but Zo owns pointer events while it
    // is active: transcript cards remain clickable and the wheel goes through
    // the same app-managed history path as keyboard scrolling. There is no
    // mouse mode toggle or HUD chip; these interactions work from frame one.
    execute!(
        stdout,
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    )?;
    if !zo_cli::tui::term::TermProfile::current().kitty_keyboard_disabled
        && matches!(
            crossterm::terminal::supports_keyboard_enhancement(),
            Ok(true)
        )
    {
        use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
        if execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
        {
            KEYBOARD_ENHANCED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    crate::mark_tui_thread();
    crate::TUI_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    INLINE_TERMINAL_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    let viewport_rows = zo_cli::tui::inline_viewport_height(
        crossterm::terminal::size().map_or(0, |(_, rows)| rows),
    );
    // Best-effort: a terminal that will not report its cursor just starts where
    // the prompt already is, which is the old behavior rather than a failure.
    let _ = anchor_inline_viewport_to_bottom(&mut stdout, viewport_rows);
    let backend = CrosstermBackend::new(zo_cli::tui::term::FrameWriter::stdout());
    let terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(viewport_rows),
        },
    )
    .map_err(TuiLoopError::Io)?;
    Ok((terminal, stderr_guard))
}

/// Put the live viewport against the bottom of the screen on a clean page.
///
/// ratatui anchors an inline viewport wherever the cursor happens to be. Launch
/// from a prompt halfway up the terminal and the composer sits halfway up too,
/// with dead space below it. Moving the cursor down is enough to pin it to the
/// bottom, but on its own it leaves whatever the shell had printed stranded
/// above a band of blank rows — the gap between the old output and the new
/// viewport cannot be filled, because the rows in between were never written.
///
/// Erasing the screen first is what makes the launch read as a clean page. It is
/// `CSI 2J`, which clears the display only: everything the shell printed stays
/// in scrollback and scrolls back into view, which is the whole reason this mode
/// stays on the primary screen.
fn anchor_inline_viewport_to_bottom(
    stdout: &mut impl io::Write,
    viewport_rows: u16,
) -> io::Result<()> {
    use crossterm::execute;
    use crossterm::terminal::{Clear, ClearType};

    execute!(stdout, Clear(ClearType::All))?;
    move_cursor_to_inline_anchor(stdout, viewport_rows)
}

/// Park the cursor on the row an inline viewport of `viewport_rows` must start
/// on to end flush with the screen bottom. Split out of
/// [`anchor_inline_viewport_to_bottom`] because the resize path repositions
/// without erasing: `CSI 2J` is what makes a *launch* read as a clean page, but
/// on a resize it would wipe the conversation off the visible screen (scrollback
/// keeps it, yet the reader watches their history vanish).
fn move_cursor_to_inline_anchor(
    stdout: &mut impl io::Write,
    viewport_rows: u16,
) -> io::Result<()> {
    use crossterm::cursor::MoveTo;
    use crossterm::execute;

    let (_, screen_rows) = crossterm::terminal::size()?;
    execute!(stdout, MoveTo(0, screen_rows.saturating_sub(viewport_rows)))?;
    stdout.flush()
}

/// Rebuild the live inline viewport when the terminal's size changes.
///
/// ratatui fixes an inline viewport's height at `Terminal` construction.
/// `Terminal::resize` re-reads the *stored* `Viewport::Inline(height)` and only
/// repositions that fixed band, and the stored value has no setter — so a window
/// the user drags taller leaves the live region stuck at its launch height with
/// the rest of the screen never painted (the reported "전체 창을 사용하지 않아 /
/// 고정된 크기"), and a window dragged shorter pushes the composer off the bottom
/// edge. Matching the new size therefore means building a new `Terminal`.
///
/// A width-only change rebuilds too. The emulator rewraps the rows the old
/// frame painted, so the previous input bar and status line survive at the old
/// wrap width while the next `draw` paints a second copy at the new one — the
/// reported double input-bar ghost in embedded IDE terminals, which resize
/// width-first while the pane height stays put.
///
/// Returns whether the viewport was rebuilt, so the caller can force a full
/// repaint instead of a cell diff against a buffer that no longer exists.
fn resync_inline_viewport(terminal: &mut TuiTerminal) -> Result<bool, TuiLoopError> {
    use ratatui::{TerminalOptions, Viewport};

    let (screen_cols, screen_rows) = crossterm::terminal::size()?;
    let desired = zo_cli::tui::inline_viewport_height(screen_rows);
    let current = terminal.get_frame().area();
    if current.height == desired && current.width == screen_cols {
        return Ok(false);
    }

    // Wait for the outgoing terminal's queued frames to actually reach stdout
    // before a second writer exists over it: `flush` now only means "frame
    // complete", so without this the direct escapes below could interleave with
    // a frame still going out, and the old writer's Drop could paint stale
    // cells on top of the rebuilt viewport.
    let _ = io::Write::flush(terminal.backend_mut());
    if !zo_cli::tui::term::frame_writer::drain_active(TERMINAL_HANDOFF_DRAIN) {
        // The terminal is not consuming. Rebuilding now would put two writers
        // on one fd and block this thread on the direct escapes below — the
        // freeze the writer thread exists to prevent, moved onto the resize
        // path. Keep the current viewport and let a later resize try again.
        zo_cli::tui::term::frame_writer::request_full_redraw();
        return Ok(false);
    }
    let mut stdout = io::stdout();
    move_cursor_to_inline_anchor(&mut stdout, desired)?;
    // Erase from the new anchor to the screen bottom before the rebuilt
    // viewport paints. The old frame's rows do not belong to the new band —
    // left in place they show through as a ghost copy of the input bar wherever
    // the new frame's wrapping differs. Scrollback and the history above the
    // anchor stay untouched (the launch path's `CSI 2J` stays launch-only for
    // exactly that reason — see `move_cursor_to_inline_anchor`).
    {
        use crossterm::execute;
        use crossterm::terminal::{Clear, ClearType};
        execute!(stdout, Clear(ClearType::FromCursorDown))?;
    }
    let backend = CrosstermBackend::new(zo_cli::tui::term::FrameWriter::stdout());
    *terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(desired),
        },
    )
    .map_err(TuiLoopError::Io)?;
    Ok(true)
}

/// Resize hook shared by every event loop: re-fit an inline viewport to the new
/// terminal height, then report whether a repaint must happen *now*. A rebuild
/// cannot wait for the next coalescing tick — until it paints, the new viewport
/// band is blank.
pub(crate) fn on_terminal_resize(
    terminal: &mut TuiTerminal,
    mode: TerminalMode,
) -> Result<bool, TuiLoopError> {
    match mode {
        TerminalMode::Inline => resync_inline_viewport(terminal),
        TerminalMode::Fullscreen => Ok(false),
    }
}

pub(crate) fn restore_terminal(
    terminal: &mut TuiTerminal,
    mode: TerminalMode,
) -> Result<(), TuiLoopError> {
    match mode {
        TerminalMode::Fullscreen => restore_fullscreen_terminal(terminal),
        TerminalMode::Inline => restore_inline_terminal(terminal),
    }
}

fn restore_fullscreen_terminal(terminal: &mut TuiTerminal) -> Result<(), TuiLoopError> {
    use crossterm::event::{DisableBracketedPaste, DisableFocusChange, DisableMouseCapture};
    use crossterm::execute;
    use crossterm::terminal::{EndSynchronizedUpdate, LeaveAlternateScreen, disable_raw_mode};

    crate::TUI_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    // Pop Kitty keyboard-enhancement flags if we pushed them, before the
    // rest of the teardown, so the terminal's key reporting is restored.
    if KEYBOARD_ENHANCED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        use crossterm::event::PopKeyboardEnhancementFlags;
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    // Best-effort: try every restoration step even if an earlier one
    // fails, so the terminal is never left in a half-restored state
    // (e.g. raw mode disabled but still in alternate screen). `End`
    // synchronized-update first, so a teardown that interrupts a frame
    // mid-`draw` can never leave the terminal frozen in CSI ?2026 mode.
    let raw = disable_raw_mode();
    let exec = execute!(
        terminal.backend_mut(),
        EndSynchronizedUpdate,
        DisableMouseCapture,
        LeaveAlternateScreen,
        DisableBracketedPaste,
        DisableFocusChange
    );
    let cursor = terminal.show_cursor();
    // The teardown escapes ride the frame queue like everything else, so hand
    // the terminal back only once they have actually been written.
    let _ = io::Write::flush(terminal.backend_mut());
    zo_cli::tui::term::frame_writer::drain_active(TERMINAL_HANDOFF_DRAIN);
    raw.map_err(TuiLoopError::Io)?;
    exec.map_err(TuiLoopError::Io)?;
    cursor.map_err(TuiLoopError::Io)?;
    Ok(())
}

fn restore_inline_terminal(terminal: &mut TuiTerminal) -> Result<(), TuiLoopError> {
    use crossterm::event::{DisableBracketedPaste, DisableFocusChange, DisableMouseCapture};
    use crossterm::execute;
    use crossterm::terminal::{EndSynchronizedUpdate, disable_raw_mode};
    use ratatui::layout::Position;

    crate::TUI_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    INLINE_TERMINAL_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    if KEYBOARD_ENHANCED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        use crossterm::event::PopKeyboardEnhancementFlags;
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }

    // `Terminal::clear` understands inline viewport geometry: it erases the
    // live rows without touching the transcript already inserted above them.
    // Put the shell cursor on the first cleared row, directly below scrollback.
    let viewport_top = terminal.get_frame().area().y;
    let clear = terminal.clear();
    let position = terminal.set_cursor_position(Position::new(0, viewport_top));
    let raw = disable_raw_mode();
    // Defensive and idempotent: if an external program or interrupted
    // fullscreen transition left mouse reporting enabled, never hand that mode
    // back to the shell.
    let exec = execute!(
        terminal.backend_mut(),
        EndSynchronizedUpdate,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange
    );
    let cursor = terminal.show_cursor();
    let _ = io::Write::flush(terminal.backend_mut());
    zo_cli::tui::term::frame_writer::drain_active(TERMINAL_HANDOFF_DRAIN);
    clear.map_err(TuiLoopError::Io)?;
    position.map_err(TuiLoopError::Io)?;
    raw.map_err(TuiLoopError::Io)?;
    exec.map_err(TuiLoopError::Io)?;
    cursor.map_err(TuiLoopError::Io)?;
    Ok(())
}

/// Restore the terminal when the process is killed by a signal, then exit with
/// the conventional `128 + signo`.
///
/// [`emergency_restore`] already covers panics and top-level errors, but nothing
/// covered *signals*: a `kill`, a closed terminal window, or an SSH hang-up tore
/// the process down with raw mode and the alternate screen still active, so the
/// shell inherited a corrupted terminal that only `reset` fixed.
///
/// `SIGINT` is deliberately not claimed. Raw mode disables `ISIG`, so Ctrl+C
/// reaches the app as a `KeyEvent` (see `tui/app/keys.rs`'s double-tap handler)
/// and no `SIGINT` is delivered while the TUI owns the terminal; claiming it here
/// would also force-exit past the graceful `tokio::signal::ctrl_c()` shutdowns in
/// `serve` and headless. `SIGTERM`/`SIGHUP` are the two that had no handler at
/// all.
///
/// Best-effort throughout: a signal stream that fails to register is skipped
/// rather than failing terminal setup, and [`emergency_restore`] itself no-ops
/// unless the TUI is active, so arming this before the terminal is taken is safe.
#[cfg(unix)]
fn spawn_terminal_restoring_signal_handler() {
    use tokio::signal::unix::{SignalKind, signal};

    // `tokio::signal` registers with the current runtime's signal driver, so one
    // must exist. Both TUI entry points run inside the async session; the guard
    // keeps a future synchronous caller from panicking here.
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    // Carry each signal's number alongside its kind so the exit code needs no
    // reverse lookup (and no `SignalKind` comparison).
    for (kind, signo) in [(SignalKind::terminate(), 15), (SignalKind::hangup(), 1)] {
        let Ok(mut stream) = signal(kind) else {
            continue;
        };
        tokio::spawn(async move {
            if stream.recv().await.is_some() {
                emergency_restore();
                std::process::exit(128 + signo);
            }
        });
    }
}

#[cfg(not(unix))]
fn spawn_terminal_restoring_signal_handler() {}

/// Restore the terminal from a panic or top-level error path, where the live
/// [`TuiTerminal`] handle has already been dropped (stack unwinding) or is
/// otherwise unreachable. Writes directly to `io::stdout()` and is best-effort —
/// every step runs even if an earlier one fails, so the shell is never handed
/// back a half-restored terminal (e.g. raw mode off but still in alt-screen).
///
/// Unlike the two byte-identical teardown blocks this replaced in `main.rs`, it
/// also pops the Kitty keyboard-enhancement flags via [`KEYBOARD_ENHANCED`].
/// `main.rs` cannot reach that private latch, so a panic used to leak the pushed
/// flags into the user's cooked shell. Idempotent: a no-op unless
/// [`crate::TUI_ACTIVE`] is set, which it then clears up front (matching
/// [`restore_terminal`]) so the freeze watchdog does not false-alarm during
/// teardown.
pub(crate) fn emergency_restore() {
    use crossterm::event::{DisableBracketedPaste, DisableFocusChange, DisableMouseCapture};
    use crossterm::execute;
    use crossterm::terminal::{EndSynchronizedUpdate, LeaveAlternateScreen, disable_raw_mode};

    if !crate::TUI_ACTIVE.swap(false, std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    let inline = INLINE_TERMINAL_ACTIVE.swap(false, std::sync::atomic::Ordering::Relaxed);
    // Whatever the writer thread still holds was composed for a screen that is
    // about to stop existing. Unwinding drops the terminal after this hook runs
    // and its `Drop` would flush that backlog on top of the restore below, so
    // drop it here instead — including any `Begin` that would re-strand the
    // terminal in synchronized mode behind the defensive `End`.
    zo_cli::tui::term::frame_writer::abandon_queued_frames();
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    // Pop Kitty keyboard-enhancement flags if `init_terminal` pushed them, so
    // the terminal's key reporting is restored. `restore_terminal` does this
    // through the ratatui backend; here there is no live handle, so write the
    // pop straight to stdout.
    if KEYBOARD_ENHANCED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        use crossterm::event::PopKeyboardEnhancementFlags;
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    // `End` synchronized-update first as a defensive clear: on a terminal that
    // `TermProfile` gates 2026 on, `App::draw_frame` may have flushed a `Begin`
    // whose paired `End` was skipped by a panic/error mid-frame (see
    // `tui/app/render.rs`). Emitting `End` here resets synchronized mode so the
    // shell never inherits a frozen terminal. No-op on terminals without 2026.
    if inline {
        let _ = execute!(
            stdout,
            EndSynchronizedUpdate,
            DisableMouseCapture,
            DisableBracketedPaste,
            DisableFocusChange,
            crossterm::cursor::MoveToColumn(0),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown)
        );
    } else {
        let _ = execute!(
            stdout,
            EndSynchronizedUpdate,
            DisableMouseCapture,
            LeaveAlternateScreen,
            DisableBracketedPaste,
            DisableFocusChange
        );
    }
    // If stderr was redirected to a log file, restore the original fd so the
    // panic traceback / error message reaches the user's console.
    let _ = stderr_redirect::restore_stderr_if_active();
}

fn push_agent_completion(app: &mut App, ids: &BlockIdGen, completion: &AgentCompletion) {
    let (level, text) = format_agent_completion(completion);
    app.push_block(RenderBlock::System {
        id: ids.next(),
        level,
        text,
    });
}

/// Handle one agent completion drained from the broadcast channel while the REPL
/// is between turns: drop internal plumbing, honor the auth/rate-limit
/// first-failure dedup latches, flip the transcript's `⎿ Done` row, and — for a
/// **background** agent — re-inject its result as a queued follow-up turn instead
/// of a bare system line. Shared by the loop-top drain and the idle `select!`
/// wake arm so a completion is handled identically however the REPL observed it
/// (the channel is single-consumer, so each completion reaches exactly one site).
fn process_idle_agent_completion(
    app: &mut App,
    ids: &BlockIdGen,
    completion: &AgentCompletion,
    active_session_id: &str,
    auth_failure_reported: &mut bool,
    rate_limit_failure_reported: &mut bool,
) {
    if suppress_mismatched_background_task_completion(completion, active_session_id) {
        return;
    }
    // AGENT→MAIN, idle half: a still-RUNNING sub-agent pushed a message
    // upstream while the prompt sat idle. It queues as its own follow-up turn
    // with the same framing the mid-turn fold uses. Branch before every
    // completion-shaped step below — the agent has not stopped, so the tree
    // flip, the failure dedup and the background re-injection must not see it.
    if agent_completion_is_agent_message(completion) {
        queue_agent_message(app, completion);
        return;
    }
    // Internal plumbing (decompose/triage) never surfaces a notice; the fan-out
    // controller narrates that step itself. Skip before the dedup so it cannot
    // claim a real agent failure's "first failure" slot.
    if agent_completion_is_internal(completion) {
        return;
    }
    if agent_completion_is_auth_failure(completion) {
        if *auth_failure_reported {
            return;
        }
        *auth_failure_reported = true;
    } else if agent_completion_is_rate_limit_failure(completion) {
        if *rate_limit_failure_reported {
            return;
        }
        *rate_limit_failure_reported = true;
    }
    // Flip the transcript's agent-tree row first (completion order). A
    // `completed` event the tree absorbed needs no extra system line — the
    // `⎿ Done` row *is* the notification (CC parity).
    let absorbed = app.note_agent_completion_display(
        &completion.agent_id,
        &completion.name,
        &completion.status,
        completion.output_tokens,
    );
    // A background agent's result is fed back to the model as its own queued
    // turn; the `⎿ Done` row above is its visible notice, so skip the bare line.
    if reinject_background_agent_completion(app, completion, active_session_id) {
        return;
    }
    if absorbed && completion.status == "completed" {
        return;
    }
    push_agent_completion(app, ids, completion);
}

#[cfg(test)]
mod tests;
