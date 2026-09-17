use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::{Stream, StreamExt};
use runtime::message_stream::{
    BlockId, BlockIdGen, PermissionDecision as RenderPermissionDecision, RenderBlock, SystemLevel,
    ToolCallId, ToolCallStatus, ToolPreview,
};
use runtime::permission::ChannelPrompter;
use runtime::{
    ConversationRuntime, PermissionMode, RouteOutcomeRecord, StreamingTurnError, TurnSummary,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tools::{
    clarify_intent, decompose_for_fanout_with_timeout_and_hooks, diagnose_lens_labels,
    request_foreground_workflow_cancel, run_diagnose_fanout,
    run_fanout_spawn_with_timeout_and_hooks, run_self_consistency_fanout,
    stop_running_agents_since_for_session, AgentCompletion, FanoutMode, IntentTriage,
    AUTO_FANOUT_AGENT_TIMEOUT, AUTO_FANOUT_DECOMPOSE_TIMEOUT, SELF_CONSISTENCY_K,
};

use super::agent_notice::{
    agent_completion_is_agent_message, agent_completion_is_auth_failure,
    agent_completion_is_internal,
    agent_completion_is_rate_limit_failure, agent_completion_is_starvation_notice,
    deliver_agent_message_mid_turn, deliver_background_agent_completion_mid_turn,
    format_agent_completion, queue_agent_message,
    requeue_undelivered_agent_notifications, suppress_mismatched_background_task_completion,
};
use super::freshness::{FreshnessDomain, SessionFreshness};
use super::live_cli_commands::{
    apply_clipboard_payload, copy_payload, read_clipboard_payload, ClipboardPayload,
};
use super::loop_arms;
use super::permission_bridge::{
    RemotePermissionResolution, run_permission_pump_with_remote,
};
use super::runtime_bridge::LiveAsyncApiClient;
use super::tui_loop::{PendingClipboardCopy, TuiLoopError, TuiTerminal, TurnOutcome};
use super::turn_harness::TurnHarness;
use super::user_question_bridge::install_tui_user_question_channel;
use super::LiveCli;
use zo_cli::tui::app::{AppAction, ClipboardCopyTarget};
use zo_cli::tui::hud::AgentTaskSummary;
use zo_cli::tui::render_schedule::{
    ANIMATION_TICK_INTERVAL, STREAM_FRAME_INTERVAL, StreamFrameGate,
};
use zo_cli::tui::sidebar::GitStatusSnapshot;
use zo_cli::tui::modals::Effort;
use zo_cli::tui::modals::workflow_viewer::WorkflowView;
use zo_cli::tui::workflow_progress::AgentRowsSnapshot;
use zo_cli::tui::{AgentCommand, App};

pub(crate) mod session_hud;

use session_hud::{
    apply_live_hud_snapshot, load_live_hud_snapshot, read_live_hud_snapshot,
    refresh_live_hud_snapshot, spawn_agent_rows_snapshot, spawn_changed_files_snapshot,
    spawn_live_hud_snapshot, spawn_workflow_view_snapshot, LiveHudSnapshot,
};

#[derive(Clone, Debug)]
pub(crate) struct AutoFanoutPlan {
    parent_model: Option<String>,
    hook_config: runtime::RuntimeHookConfig,
    session_goal: Option<String>,
    session_id: String,
    /// Breadth fan-out (`FanoutParallel`) vs a non-breadth turn.
    breadth: bool,
    /// True when this prelude is the real host pre-spawn fast path, not just
    /// semantic triage that may still fall back to the model-led turn.
    host_prespawn: bool,
    /// True when the prelude should run cheap intent triage before choosing
    /// whether to spawn agents or fall back.
    semantic_triage: bool,
    /// The turn's deterministic difficulty verdict — what decides HOW MANY
    /// agents a decompose fan-out spawns (`fanout::fanout_width_for`), and
    /// whether it spawns any at all. Classified with the same table the
    /// per-agent router uses, so the host's resource allocation and the
    /// model's routing speak one taxonomy.
    complexity: runtime::RouteTaskComplexity,
}

impl AutoFanoutPlan {
    /// Build the optional pre-turn collaboration prelude from the turn's
    /// [`RouteHint`], or `None` when the model should decide without host work.
    ///
    /// The host only pre-spawns agents on the route hint's fast-path (explicit
    /// parallel request or ultracode). A semantic-triage-only plan may still run
    /// cheap intent routing and then fall back to the model-led turn, so the two
    /// states stay separate for audit/UI accuracy.
    fn from_hint(cli: &LiveCli, hint: &super::auto_fanout::RouteHint) -> Option<Self> {
        let host_prespawn = hint.should_host_prespawn();
        let semantic_triage = hint.should_run_semantic_triage();
        if !host_prespawn && !semantic_triage {
            return None;
        }
        // Fail-closed permission gate: `SpawnMultiAgent` requires
        // `DangerFullAccess`, so never auto-spawn agents in a stricter mode.
        if cli.permission_mode != PermissionMode::DangerFullAccess {
            return None;
        }
        let session_goal = cli
            .session_goal
            .as_deref()
            .map(str::trim)
            .filter(|goal| !goal.is_empty())
            .map(ToOwned::to_owned);
        // Spawn policy: inherit the active model; `/smart` may route spawned roles.
        Some(Self {
            parent_model: cli
                .runtime
                .api_client()
                .tool_registry()
                .context()
                .spawn_parent_model(),
            hook_config: cli.runtime.feature_config.hooks().clone(),
            session_goal,
            session_id: cli.session.id.clone(),
            breadth: hint.is_breadth(),
            host_prespawn,
            semantic_triage,
            complexity: hint.complexity(),
        })
    }
}

fn estimated_fanout_context_tokens(session_tokens: usize, system_prompt: &[String]) -> usize {
    system_prompt.iter().fold(session_tokens, |acc, section| {
        acc.saturating_add(section.len() / 4 + 4)
    })
}

fn route_reminder_for_hint(hint: &super::auto_fanout::RouteHint) -> Option<String> {
    // The host-consumes-the-turn gate lives in `model_reminder`: it suppresses
    // the nudge only for a breadth pre-spawn (which fans out and carries the
    // intent), so a non-breadth ultracode pre-spawn — which usually falls back
    // to the model-led turn — keeps its shape guidance and any prior-failure note.
    hint.model_reminder()
}

fn fanout_decomposition_input(user_input: &str, session_goal: Option<&str>) -> String {
    let Some(goal) = session_goal.map(str::trim).filter(|goal| !goal.is_empty()) else {
        return user_input.to_string();
    };
    format!("Session goal: {goal}\n\nUser turn:\n{user_input}")
}

/// Score the discovered active skills against this turn and build the advisory
/// recommendation reminder, or `None` when nothing actionable matches. Re-scans
/// `cwd` each turn so newly added/approved skills are picked up without a
/// restart; proposed skills are excluded by discovery before scoring.
fn build_turn_skill_reminder(cwd: &std::path::Path, user_input: &str) -> Option<String> {
    let skills = runtime::discover_skills(cwd);
    if skills.is_empty() {
        return None;
    }
    let recommendations = runtime::recommend_skills(
        &runtime::SkillMatchInput {
            user_text: user_input,
            touched_paths: &[],
        },
        &skills,
    );
    runtime::build_skill_recommendation_reminder(&recommendations)
}

/// Return `true` when it is wire-valid to inject the synthetic
/// `assistant(tool_use) + tool_result` pair into `session` right now — i.e.
/// immediately BEFORE `push_user_text` adds the real user turn.
///
/// Two invariants must hold for the resulting sequence to be accepted by all
/// three backends (Anthropic, OpenAI-compat, Gemini):
///
/// 1. **Non-empty session**: the pair must not lead the conversation — Anthropic
///    and most providers reject a first message with `role:"assistant"`.
/// 2. **Last message is user-wire**: the session's current last message must
///    serialize as wire role `"user"` (i.e. its stored role is `User` or `Tool`)
///    so the new `assistant(tool_use)` message is the *next* role in proper
///    user→assistant alternation. A stored `Assistant` role as the last message
///    would produce two consecutive assistant-wire messages, which the Anthropic
///    API rejects.
///
/// When either invariant fails the caller must fall back to the evidence prepend
/// so the analysis still reaches the model without breaking the wire sequence.
pub(crate) fn fanout_evidence_injection_is_wire_safe(session: &runtime::Session) -> bool {
    let Some(last) = session.messages.last() else {
        // Empty session → injecting assistant first would lead the conversation.
        return false;
    };
    // Only user-wire roles (User or Tool) satisfy alternation before a new
    // assistant block. System and Assistant both violate the constraint.
    matches!(
        last.role,
        runtime::session::MessageRole::User | runtime::session::MessageRole::Tool
    )
}

fn push_synthetic_fanout_evidence_pair(
    session: &mut runtime::Session,
    tool_use: runtime::ConversationMessage,
    tool_result: runtime::ConversationMessage,
) -> Result<(), runtime::SessionError> {
    push_synthetic_fanout_evidence_pair_with_persist(session, tool_use, tool_result, |session| {
        let path = session.persistence_path().map(std::path::Path::to_path_buf);
        if let Some(path) = path {
            session.save_to_path(path)
        } else {
            Ok(())
        }
    })
}

fn push_synthetic_fanout_evidence_pair_with_persist(
    session: &mut runtime::Session,
    tool_use: runtime::ConversationMessage,
    tool_result: runtime::ConversationMessage,
    persist: impl FnOnce(&runtime::Session) -> Result<(), runtime::SessionError>,
) -> Result<(), runtime::SessionError> {
    let original_len = session.messages.len();
    let original_updated_at_ms = session.updated_at_ms;
    session.updated_at_ms = current_time_millis_for_synthetic_pair();
    {
        let messages = Arc::make_mut(&mut session.messages);
        messages.push(tool_use);
        messages.push(tool_result);
    }

    if let Err(error) = persist(session) {
        Arc::make_mut(&mut session.messages).truncate(original_len);
        session.updated_at_ms = original_updated_at_ms;
        return Err(error);
    }

    Ok(())
}

fn current_time_millis_for_synthetic_pair() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

enum AutoFanoutPrelude {
    Ready(String),
    /// Completed pre-analysis delivered as a microcompact-clearable synthetic
    /// `SpawnMultiAgent` tool-result rather than a permanent user-message
    /// prepend. `user_input` is the ORIGINAL turn input (kept in its own user
    /// message); `evidence` is the consume-not-rederive analysis body.
    ReadyWithEvidence {
        user_input: String,
        evidence: String,
    },
    Cancelled,
}

enum AutoFanoutResult {
    DecomposeFailed,
    SpawnFailed {
        roles: Vec<String>,
        error: String,
    },
    Completed {
        roles: Vec<String>,
        summary: String,
    },
    /// Self-consistency mode (P2): the same clarified question was answered by
    /// `k` independent agents and reconciled through the Council. The `answer`
    /// is the already-synthesized majority (or honest unreconciled list), so the
    /// consumer treats it as a single completed evidence block.
    SelfConsistent {
        answer: String,
    },
}

type AutoFanoutTask = JoinHandle<AutoFanoutResult>;

#[derive(Debug, Clone)]
enum AutoFanoutProgress {
    LaunchingAgents { roles: Vec<String> },
}

/// Upper bound on the pre-turn OAuth refresh before we start the turn on the
/// current bearer anyway. Long enough for a real token refresh / loadCodeAssist
/// round-trip, short enough that a hung endpoint can't wedge the UI; a genuine
/// expiry still surfaces downstream as a normal 401.
const PRETURN_REFRESH_BUDGET: Duration = Duration::from_secs(8);

/// Mark the preflight frame dirty and paint it when the shared frame budget is
/// ready. `force` is reserved for the animation tick, a rebuilt viewport, and
/// semantic UI transitions; key bursts coalesce here exactly as they do in the
/// main app loop. Every paint reports its cost and completion time so a slow TTY
/// cannot collapse the gate back to one full draw per event.
fn request_pumped_preflight_draw<W>(
    app: &mut App,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<W>>,
    frame_gate: &mut StreamFrameGate,
    pending_draw: &mut bool,
    force: bool,
) -> Result<(), TuiLoopError>
where
    W: std::io::Write,
{
    *pending_draw = true;
    if force
        || frame_gate
            .on_stream_update(std::time::Instant::now())
            .draws_now()
    {
        let draw_started = std::time::Instant::now();
        app.draw_frame(terminal)?;
        frame_gate.note_draw_cost(draw_started.elapsed());
        frame_gate.note_stream_draw(std::time::Instant::now());
        *pending_draw = false;
    }
    Ok(())
}

/// Apply one terminal event while an awaited preflight future owns the turn.
/// Key and paste state is consumed immediately; only redundant paints inside a
/// key burst wait for the next 33 ms animation tick.
fn handle_pumped_preflight_event<W, R>(
    app: &mut App,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<W>>,
    maybe_event: Option<Result<Event, io::Error>>,
    frame_gate: &mut StreamFrameGate,
    pending_draw: &mut bool,
    clipboard_write: &mut Option<PendingClipboardCopy>,
    resize_terminal: &mut R,
) -> Result<bool, TuiLoopError>
where
    W: std::io::Write,
    R: FnMut(
        &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<W>>,
        zo_cli::tui::TerminalMode,
    ) -> Result<bool, TuiLoopError>,
{
    match maybe_event {
        Some(Ok(Event::Key(key))) => {
            if app.active_prompt().is_some()
                && key.kind == KeyEventKind::Press
                && matches!(key.code, KeyCode::Up | KeyCode::Down)
            {
                app.move_permission_selection(matches!(key.code, KeyCode::Up));
            } else if let Some(decision) = key_to_permission_decision(&key, app) {
                if let Some(prompt) = app.take_active_prompt() {
                    let _ = prompt.responder.send(decision);
                }
            } else {
                match app.handle_key(key)? {
                    AppAction::OpenWorkflowViewer => {
                        crate::session::tui_loop::open_workflow_viewer(app);
                    }
                    AppAction::OpenAgentInViewer(agent_id) => {
                        crate::session::tui_loop::open_workflow_viewer_focused(app, &agent_id);
                    }
                    AppAction::ClipboardCopyBlock(text) if clipboard_write.is_none() => {
                        *clipboard_write = Some(PendingClipboardCopy::new(text));
                    }
                    _ => {}
                }
            }
            request_pumped_preflight_draw(
                app,
                terminal,
                frame_gate,
                pending_draw,
                false,
            )?;
        }
        Some(Ok(Event::Paste(text))) => {
            app.handle_paste_owned(text);
            request_pumped_preflight_draw(
                app,
                terminal,
                frame_gate,
                pending_draw,
                false,
            )?;
        }
        Some(Ok(Event::Mouse(mouse))) => {
            let is_scroll = matches!(
                mouse.kind,
                crossterm::event::MouseEventKind::ScrollUp
                    | crossterm::event::MouseEventKind::ScrollDown
            );
            let action = app.handle_mouse(mouse)?;
            let draw = match action {
                AppAction::OpenWorkflowViewer => {
                    crate::session::tui_loop::open_workflow_viewer(app);
                    Some(true)
                }
                AppAction::OpenAgentInViewer(agent_id) => {
                    crate::session::tui_loop::open_workflow_viewer_focused(app, &agent_id);
                    Some(true)
                }
                AppAction::ClipboardCopyBlock(text) => {
                    if clipboard_write.is_none() {
                        *clipboard_write = Some(PendingClipboardCopy::new(text));
                    }
                    Some(true)
                }
                AppAction::Redraw => Some(false),
                _ if is_scroll => Some(false),
                _ => None,
            };
            if let Some(force) = draw {
                request_pumped_preflight_draw(
                    app,
                    terminal,
                    frame_gate,
                    pending_draw,
                    force,
                )?;
            }
        }
        Some(Ok(Event::Resize(..))) => {
            let rebuilt = resize_terminal(terminal, app.terminal_mode())?;
            request_pumped_preflight_draw(
                app,
                terminal,
                frame_gate,
                pending_draw,
                rebuilt,
            )?;
        }
        Some(Ok(_)) => {}
        Some(Err(error)) => return Err(TuiLoopError::Io(error)),
        None => return Ok(false),
    }
    Ok(true)
}

/// Drive `fut` to completion while keeping `Thinking` animated and interactive.
///
/// This covers every await before `drive_turn` opens its streaming `select!`:
/// checkpoint settle, OAuth refresh, route assessment, and skill discovery.
/// Polling the terminal stream here is as important as polling the animation
/// tick; otherwise input sits in Crossterm's queue and the composer appears
/// frozen until the first model block starts streaming.
async fn pump_draw_until<W, S, R, F, T>(
    app: &mut App,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<W>>,
    events: &mut S,
    clipboard_write: &mut Option<PendingClipboardCopy>,
    mut resize_terminal: R,
    fut: F,
) -> Result<T, TuiLoopError>
where
    W: std::io::Write,
    S: Stream<Item = Result<Event, io::Error>> + Unpin,
    R: FnMut(
        &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<W>>,
        zo_cli::tui::TerminalMode,
    ) -> Result<bool, TuiLoopError>,
    F: std::future::Future<Output = T>,
{
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(ANIMATION_TICK_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut events_open = true;
    let mut pending_draw = false;
    let mut frame_gate =
        StreamFrameGate::new_ready(std::time::Instant::now(), STREAM_FRAME_INTERVAL);
    loop {
        zo_cli::tui::watchdog::beat();
        tokio::select! {
            output = &mut fut => {
                if pending_draw {
                    request_pumped_preflight_draw(
                        app,
                        terminal,
                        &mut frame_gate,
                        &mut pending_draw,
                        true,
                    )?;
                }
                return Ok(output);
            }
            maybe_event = events.next(), if events_open => {
                events_open = handle_pumped_preflight_event(
                    app,
                    terminal,
                    maybe_event,
                    &mut frame_gate,
                    &mut pending_draw,
                    clipboard_write,
                    &mut resize_terminal,
                )?;
            }
            _ = tick.tick() => {
                app.advance_tick();
                request_pumped_preflight_draw(
                    app,
                    terminal,
                    &mut frame_gate,
                    &mut pending_draw,
                    true,
                )?;
            }
        }
    }
}

async fn pump_blocking_with_draw<W, S, R, T, F>(
    app: &mut App,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<W>>,
    events: &mut S,
    clipboard_write: &mut Option<PendingClipboardCopy>,
    resize_terminal: R,
    work: F,
) -> Result<T, TuiLoopError>
where
    W: std::io::Write,
    S: Stream<Item = Result<Event, io::Error>> + Unpin,
    R: FnMut(
        &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<W>>,
        zo_cli::tui::TerminalMode,
    ) -> Result<bool, TuiLoopError>,
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    pump_draw_until(
        app,
        terminal,
        events,
        clipboard_write,
        resize_terminal,
        tokio::task::spawn_blocking(work),
    )
    .await?
    .map_err(|error| TuiLoopError::Turn(format!("blocking preflight failed: {error}")))
}

/// The turn prelude seam: disable input, begin the visible `Thinking` activity,
/// and draw one frame so the spinner is live BEFORE any preflight work (the
/// checkpoint settle, OAuth refresh, and route probe that follow).
///
/// This owns the turn's single begin. `drive_turn` also calls
/// [`App::begin_turn_with_generation`], but that is idempotent once a turn is
/// active, so the later call is a no-op and the activity is created exactly
/// once. On a draw failure it restores input and disarms the activity — the
/// same guard `drive_turn` uses — so a failed prelude never strands a permanent
/// spinner. Generic over the backend writer so the seam is exercised headlessly.
fn begin_turn_prelude<W: std::io::Write>(
    app: &mut App,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<W>>,
    turn_generation: u64,
) -> Result<(), TuiLoopError> {
    app.disable_input();
    app.begin_turn_with_generation(turn_generation);
    if let Err(error) = app.draw_frame(terminal) {
        app.enable_input();
        app.abort_turn();
        return Err(error.into());
    }
    Ok(())
}

/// Adopt the previous turn's in-flight code checkpoint OFF the UI thread while
/// the just-started spinner keeps animating.
///
/// The tree-hash `join` can fork `git write-tree` over a whole worktree
/// (seconds on a big or cold repo), so joining it inline on the UI thread is
/// exactly the follow-up-turn stall this slice removes. The join is moved onto
/// a blocking task and driven through [`pump_draw_until`]; the settled hash is
/// applied exactly once, here, before this turn's tools can edit files — a tree
/// still being hashed would otherwise smear the new edits into the previous
/// turn's snapshot.
async fn settle_pending_checkpoint_pumped(
    cli: &mut LiveCli,
    app: &mut App,
    terminal: &mut TuiTerminal,
    events: &mut EventStream,
    clipboard_write: &mut Option<PendingClipboardCopy>,
) -> Result<(), TuiLoopError> {
    let Some((turn_number, handle)) = cli.take_pending_code_checkpoint() else {
        return Ok(());
    };
    let settle = tokio::task::spawn_blocking(move || handle.join().ok().flatten());
    let tree_hash = pump_draw_until(
        app,
        terminal,
        events,
        clipboard_write,
        crate::session::tui_loop::on_terminal_resize,
        settle,
    )
    .await?
    .ok()
    .flatten();
    cli.adopt_settled_checkpoint(turn_number, tree_hash);
    Ok(())
}

fn routed_auth_route(model: &str) -> api::AuthRoute {
    crate::runtime_support::catalog_auth_route_for_model(model)
}

fn routed_provider_kind(model: &str) -> api::ProviderKind {
    crate::runtime_support::catalog_provider_for_model(model)
        .unwrap_or_else(|| api::detect_provider_kind(model))
}

fn build_routed_provider_client(
    model: &str,
    auth_route: api::AuthRoute,
    session_id: &str,
) -> Result<api::ProviderClient, api::ApiError> {
    let client = if let Some(provider) = crate::runtime_support::catalog_provider_for_model(model) {
        api::ProviderClient::from_provider_kind_with_auth_route(provider, auth_route)?
    } else {
        api::ProviderClient::from_model_with_auth_route(model, auth_route)?
    };
    // Keep ChatGPT prompt-cache keys on the zo session across deep-lane,
    // implementer, and quota-fallback client rebuilds.
    Ok(client.with_cache_scope(session_id))
}

/// Build the ordered VERIFY candidate clients for this turn's deep gate,
/// top-ranked first, each `(client, model)`. The models come from
/// [`super::smart_settings::route_deep_verify_candidates`] (configured
/// deep-tier order under Architect; Smart Router order under Classic), so a
/// candidate that is itself hard rate-limited fails over to the next
/// different-provider candidate. The
/// top candidate reuses the cached [`api::ProviderClient`] on `cli` (the common
/// no-rate-limit path re-resolves nothing); the rarely-needed lower candidates
/// build their client on the spot. Any candidate whose provider client cannot be
/// constructed is skipped rather than aborting the list. Empty when Smart is off
/// or the pool offers no cross-model verifier — the deep gate then runs its
/// native same-model verify.
fn deep_verify_candidate_clients(
    cli: &mut LiveCli,
    anchor_model: &str,
    deep_tier_models: &[String],
    leg_effort: api::EffortLevel,
) -> Vec<(Arc<dyn runtime::AsyncApiClient>, String)> {
    let session_model = cli.runtime.api_client().model().to_string();
    // Anchor the Verifier route on the model whose work is being judged: the
    // session main normally, the Architect contract's implementer when one is
    // installed this turn — so cross-provider diversity means "different
    // provider than the IMPLEMENTER", not "different than the orchestrator".
    // A candidate equal to the session's native model is dropped: the deep
    // gate's native VERIFY fallback already runs on that client, so building a
    // duplicate would only re-resolve credentials for the same behavior.
    let mut targets =
        super::smart_settings::route_deep_verify_candidates(anchor_model, deep_tier_models);
    targets.retain(|target| *target != session_model);
    if targets.is_empty() {
        cli.deep_verify_provider = None;
        return Vec::new();
    }
    let mut resolved = Vec::with_capacity(targets.len());
    for target in targets {
        let auth_route = routed_auth_route(&target);
        // Reuse the cached provider for the top candidate only when both the
        // model and its catalog-selected credential route are unchanged.
        let provider = match cli.deep_verify_provider.as_ref() {
            Some((model, cached_route, client))
                if *model == target && *cached_route == auth_route =>
            {
                client.clone()
            }
            _ => match build_routed_provider_client(&target, auth_route, &cli.session.id) {
                Ok(client) => {
                    if resolved.is_empty() {
                        cli.deep_verify_provider =
                            Some((target.clone(), auth_route, client.clone()));
                    }
                    client
                }
                Err(_) => continue,
            },
        };
        resolved.push((provider, target, auth_route));
    }

    let provider_kinds = resolved
        .iter()
        .map(|(_, model, _)| routed_provider_kind(model))
        .collect::<Vec<_>>();
    // Kept in lockstep with the runtime's
    // `ConversationRuntime::verifier_rate_limit_retry_cap`: both layers hand a
    // 429 over as soon as the walk has somewhere to go, so the inner provider
    // ladder can never burn minutes underneath the outer cap. The escape is a
    // later different-provider candidate OR the terminal native fallback; only
    // when the deep-tier invariant bars that fallback too is there nowhere to
    // go, and the leg rides the window out on both ladders.
    let native_fallback_available = !super::smart_settings::architect_deep_lanes_enabled()
        || runtime::is_deep_tier_model(&session_model, deep_tier_models);
    let mut clients = Vec::with_capacity(resolved.len());
    for (idx, (provider, target, auth_route)) in resolved.into_iter().enumerate() {
        let has_cross_provider_fallback = provider_kinds
            .iter()
            .skip(idx + 1)
            .any(|candidate| *candidate != provider_kinds[idx]);
        let provider = if has_cross_provider_fallback || native_fallback_available {
            provider.with_rate_limit_fail_fast()
        } else {
            provider
        };
        let client = deep_verify_leg_client(cli, provider, target.clone(), auth_route, leg_effort);
        clients.push((client, target));
    }
    clients
}

/// How deep this turn's VERIFY leg can go, as far as TURN ENTRY can know.
///
/// The deep gate resolves the real depth at verify time from two inputs: this
/// turn's routing band — which this host installs itself
/// ([`runtime::ConversationRuntime::set_verify_band`]), so it is exactly
/// reproducible here — and the diff it is about to judge, which does not exist
/// yet. The measured diff can move the gate's verdict in BOTH directions (a
/// small green diff demotes even a `Medium` band to one spec lens; large
/// churn, security paths, touched tests, or a red objective force `Full`), so
/// this mirror answers a narrower question than the gate: which depth is a
/// SAFE prediction from the band alone, before any diff exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifyLegDepth {
    /// The band permits the deep gate's single-lens verify.
    SingleLens,
    /// The band forces the full three-lens verify.
    Full,
}

/// The band-only PREDICTION of the deep gate's depth cut, made before the
/// diff exists. Deliberately more conservative than `deep_gate::verify_depth`
/// itself: the gate can demote a `Medium` (or `Trivial`) band to one lens
/// once it has MEASURED a small green diff, but this mirror decides how much
/// reasoning the leg's client is built with, and a `Medium` verb guess spans
/// everything from a one-line fix to a twelve-file feature. Predicting one
/// lens there would run a ballooned diff's full three-lens verify on a
/// discounted client — reasoning lost exactly where it matters. `Small` at
/// `Low`/`Medium` risk is the one band whose diff prior is tight enough to
/// predict a single lens; erring toward `Full` everywhere else only ever
/// costs speed, never reasoning.
const fn verify_leg_depth_for_band(
    complexity: runtime::RouteTaskComplexity,
    risk: runtime::RouteTaskRisk,
) -> VerifyLegDepth {
    match (complexity, risk) {
        (
            runtime::RouteTaskComplexity::Small,
            runtime::RouteTaskRisk::Low | runtime::RouteTaskRisk::Medium,
        ) => VerifyLegDepth::SingleLens,
        _ => VerifyLegDepth::Full,
    }
}

/// The effort this turn's VERIFY leg runs at.
///
/// `xhigh` for a full-depth leg: one focused judgment gates the whole turn,
/// so "one smart verification beats several mediocre ones". A single-lens leg
/// runs at `high` instead: that lens is the spec-only read — is the
/// deliverable what was asked for — and its quality saturates well below max
/// reasoning. Measured first on design turns (the win came from the guidance
/// reminder plus the existence of the lens, not from how hard the verifier
/// thought), and the lens is the same spec-only read whatever the intent, so
/// the discount follows the DEPTH, not the intent. The design kill switch
/// (`ZO_DESIGN_GUIDANCE=off`) therefore no longer touches effort: it still
/// downgrades the intent — killing the guidance reminder and the design
/// verify floor — but a single-lens leg keeps `high` on the saturation
/// argument alone.
const fn deep_verify_leg_effort(depth: VerifyLegDepth) -> api::EffortLevel {
    match depth {
        VerifyLegDepth::SingleLens => api::EffortLevel::High,
        VerifyLegDepth::Full => api::EffortLevel::Xhigh,
    }
}

/// Wrap a resolved verifier [`api::ProviderClient`] in a [`LiveAsyncApiClient`]
/// with the same tool wiring as the main turn, at this turn's
/// [`deep_verify_leg_effort`]. Shared by [`deep_verify_async_client`] and
/// [`deep_verify_candidate_clients`] so every VERIFY leg — primary or ranked
/// fallback — is built identically.
fn deep_verify_leg_client(
    cli: &LiveCli,
    provider: api::ProviderClient,
    target: String,
    auth_route: api::AuthRoute,
    leg_effort: api::EffortLevel,
) -> Arc<dyn runtime::AsyncApiClient> {
    let api_client = cli.runtime.api_client();
    Arc::new(LiveAsyncApiClient::new(
        provider,
        target,
        auth_route,
        api_client.enable_tools(),
        // Session-global advertisement only: the one-turn allowed-tools
        // restriction is enforced by the executor's shared slot, not the wire
        // (see the main turn build site — cache-stability rationale).
        cli.allowed_tools.clone(),
        api_client.tool_registry(),
        // Thinking budgets are main-model-specific (None). The leg effort is
        // resolved once per turn by `deep_verify_leg_effort` — xhigh for a
        // full-depth verify, high for a predicted single-lens read. Models
        // without the level are clamped by the api layer. No dynamic band —
        // the verifier is a fixed independent check, not a Smart-mode main
        // turn.
        None,
        Some(leg_effort),
        None,
    ))
}

/// Build this turn's Architect execution contract: when `smart.policy=architect`,
/// the session main model belongs to the configured deep-tier PLAN/VERIFY pool
/// and this turn is generative ([`turn_is_contract_eligible`] — keyword write
/// intent OR a resolved `Design`/`Implementation` read), non-trivial turns force
/// the read-only PLAN phase first. `smart.execSwap` controls whether EXEC legs run
/// on the router's Coding-role implementer for this classified turn; otherwise
/// they stay on the session client with direct edits allowed as explicit owner
/// intent. `None` — chat/analysis turns, classic policy,
/// a main model outside the pool, no routable implementer, or an explicit
/// reserved `coding` pin — keeps every leg native, the pre-contract behavior.
///
/// Pool membership ([`runtime::is_deep_tier_model`] over
/// `smart.deepTierModels`) is the SAME test the PLAN and VERIFY lanes below
/// use. It deliberately replaced the catalog-derived
/// `is_reserved_orchestrator_model`: that hardcoded set made `/tier add <model>`
/// a half-feature — the deep lanes honoured the edited pool while this gate
/// did not, so putting a model the user considers a planner in the pool
/// silently failed to arm any plan/implement split for it.
///
/// The implementer client carries the same tool wiring and per-turn effort
/// band as the main turn (it does the turn's real work); thinking budgets are
/// main-model-specific and deliberately not carried over. The raw
/// [`api::ProviderClient`] is cached on `cli` keyed by model id so credentials
/// are not re-resolved every turn (same pattern as the verify/quota caches).
/// Whether this turn is generative enough to deserve an Architect contract.
///
/// The keyword tables answer first and stay authoritative when they fire. When
/// they do NOT, the resolved intent read is consulted: `만들어줘`,
/// `디자인해줘`, and every other generative phrasing outside the ASCII verb
/// tables is invisible to `turn_has_write_intent`, which silently dropped the
/// contract (no plan-first, no implementer swap) on exactly the turns that
/// produce the most code. A trusted probe supplies that read; the deterministic
/// assessment stays neutral.
///
/// `Analysis`/`Other`/absent intent leaves the gate exactly where it was.
fn turn_is_contract_eligible(user_input: &str, intent: runtime::RouteTaskIntent) -> bool {
    tools::turn_has_write_intent(user_input)
        || matches!(
            intent,
            runtime::RouteTaskIntent::Design | runtime::RouteTaskIntent::Implementation
        )
}

fn exec_contract_for_turn(
    cli: &mut LiveCli,
    user_input: &str,
    assessment: tools::TurnProbeAssessment,
    exec_swap: tools::SmartExecSwap,
    deep_tier_models: &[String],
    turn_effort: Option<api::EffortLevel>,
    turn_effort_ceiling: Option<api::EffortLevel>,
) -> Option<runtime::ExecContract> {
    let complexity = assessment.complexity;
    let main_model = cli.runtime.api_client().model().to_string();
    if !runtime::is_deep_tier_model(&main_model, deep_tier_models)
        || !turn_is_contract_eligible(user_input, assessment.intent)
    {
        cli.exec_impl_provider = None;
        return None;
    }
    let Some(impl_model) =
        super::smart_settings::route_exec_impl_model(&main_model, deep_tier_models)
    else {
        cli.exec_impl_provider = None;
        return None;
    };
    let impl_client = if should_install_exec_implementer(exec_swap, complexity) {
        let auth_route = routed_auth_route(&impl_model);
        let cached_provider = cli.exec_impl_provider.as_ref().and_then(
            |(model, cached_route, client)| {
                (*model == impl_model && *cached_route == auth_route).then(|| client.clone())
            },
        );
        let provider = if let Some(client) = cached_provider {
            Some(client)
        } else if let Ok(client) =
            build_routed_provider_client(&impl_model, auth_route, &cli.session.id)
        {
            cli.exec_impl_provider = Some((impl_model.clone(), auth_route, client.clone()));
            Some(client)
        } else {
            // An unbuildable implementer (missing credential, an auth route the
            // provider does not support) costs the SWAP, not the contract: the
            // planner still plans first and the deep lanes still run. Returning
            // `None` here used to drop `plan_first` too, so one unreachable
            // provider silently downgraded the turn all the way to pre-contract
            // behavior.
            cli.exec_impl_provider = None;
            None
        };
        provider.map(|provider| {
            let api_client = cli.runtime.api_client();
            Arc::new(LiveAsyncApiClient::new(
                provider,
                impl_model.clone(),
                auth_route,
                api_client.enable_tools(),
                // Session-global advertisement only — the per-turn restriction
                // lives in the executor slot (cache-stability, see main site).
                cli.allowed_tools.clone(),
                api_client.tool_registry(),
                None,
                turn_effort,
                turn_effort_ceiling,
            )) as Arc<dyn runtime::AsyncApiClient>
        })
    } else {
        cli.exec_impl_provider = None;
        None
    };
    Some(runtime::ExecContract {
        impl_client,
        impl_model,
        plan_first: plan_first_for(complexity),
    })
}

/// Sync the executor's shared turn-allowed-tools slot (the actual enforcement
/// point for a one-turn `allowed-tools` override) with the CLI's current
/// override. Called at every turn start — including when the override is
/// `None`, which self-heals any slot a mid-turn error path left armed — and
/// again at turn completion after the override is cleared. `pub(crate)` so
/// the slash-dispatch contract test pins this exact production wiring: the
/// gate is the only enforcement (the wire advertisement no longer shrinks),
/// so an unwired arm would silently fail open.
pub(crate) fn arm_turn_allowed_tools(cli: &LiveCli) {
    if let Some(runtime) = cli.runtime.runtime.as_ref() {
        runtime
            .tool_executor()
            .set_turn_allowed_tools(cli.turn_allowed_tools.clone());
    }
}

/// Whether the Architect contract runs its read-only PLAN phase before the
/// first EXEC leg for a turn of this difficulty.
///
/// Plan before handing work over: an implementer that starts without the
/// planner's plan is just a cheaper model doing the whole task alone, which is
/// the opposite of the contract. `Trivial`/`Small` skip it — a plan phase costs
/// a round trip that a one-file edit does not earn — and `Unknown` stays out
/// because it is the classifier's "no signal" value, not a band below `Medium`.
///
/// This used to require `Large`, which never intersected the swap band the old
/// `execSwap=easy` default armed (`Trivial` only): every turn got at most one
/// half of the contract.
fn plan_first_for(complexity: runtime::RouteTaskComplexity) -> bool {
    matches!(
        complexity,
        runtime::RouteTaskComplexity::Medium | runtime::RouteTaskComplexity::Large
    )
}

fn should_install_exec_implementer(
    exec_swap: tools::SmartExecSwap,
    complexity: runtime::RouteTaskComplexity,
) -> bool {
    exec_swap.arms_for(complexity)
}

fn exec_contract_arms_edit_gate(contract: Option<&runtime::ExecContract>) -> bool {
    contract.is_some_and(runtime::ExecContract::exec_swap_enabled)
}

/// Build this turn's cross-provider quota-fallback client: the different-
/// provider route wrapped in a [`LiveAsyncApiClient`] with the same tool wiring
/// as the main turn (it continues the turn, so it needs the full tool set on
/// the wire). `None` when Smart is off, `smart.quotaFallback` is off, the route
/// resolves to no cross-provider peer (single-provider pool), or the other
/// provider's client cannot be constructed — the turn then has no fallback and
/// a quota-exhausted turn fails as before. Mirrors [`deep_verify_async_client`],
/// including caching the raw [`api::ProviderClient`] on `cli` keyed by model id
/// so credentials are not re-resolved on every turn. Thinking/effort ride the
/// provider default (like the verifier): a cross-provider continuation should
/// not carry a main-model-specific budget.
/// The interactive turn's `(wall_clock_deadline, output_token_budget,
/// input_token_budget)` circuit breakers, each `None` when disabled. Thin
/// alias over [`runtime::env_turn_budgets`] — the parsing, defaults, and env
/// overrides (`ZO_TURN_DEADLINE_SECS` / `ZO_TURN_OUTPUT_TOKEN_BUDGET` /
/// `ZO_TURN_INPUT_TOKEN_BUDGET`, `0` disables) live in the runtime so the
/// headless, serve, and sub-agent hosts apply the identical bounds.
fn interactive_turn_budgets() -> (Option<std::time::Duration>, Option<u32>, Option<u32>) {
    runtime::env_turn_budgets()
}

pub(crate) fn quota_fallback_async_client(
    cli: &mut LiveCli,
) -> Option<(Arc<dyn runtime::AsyncApiClient>, String)> {
    let main_model = cli.runtime.api_client().model().to_string();
    let Some(target) = super::smart_settings::route_quota_fallback_model(&main_model) else {
        cli.quota_fallback_provider = None;
        return None;
    };
    let auth_route = routed_auth_route(&target);
    let provider = match cli.quota_fallback_provider.as_ref() {
        Some((model, cached_route, client))
            if *model == target && *cached_route == auth_route =>
        {
            client.clone()
        }
        _ => {
            let Ok(client) =
                build_routed_provider_client(&target, auth_route, &cli.session.id)
            else {
                cli.quota_fallback_provider = None;
                return None;
            };
            cli.quota_fallback_provider =
                Some((target.clone(), auth_route, client.clone()));
            client
        }
    };
    let api_client = cli.runtime.api_client();
    let client: Arc<dyn runtime::AsyncApiClient> = Arc::new(LiveAsyncApiClient::new(
        provider,
        target.clone(),
        auth_route,
        api_client.enable_tools(),
        // Session-global advertisement only — the per-turn restriction lives
        // in the executor slot (cache-stability, see the main turn build site).
        cli.allowed_tools.clone(),
        api_client.tool_registry(),
        None,
        None,
        None,
    ));
    Some((client, target))
}

/// The deterministic (probe-free) reading of a turn: complexity plus the
/// neutral `Other` intent fallback shared with headless turns.
fn deterministic_turn_assessment(user_input: &str) -> tools::TurnProbeAssessment {
    tools::assess_turn_deterministic(user_input)
}

/// This turn's complexity AND design/implementation intent, for the effort band
/// and the design-guidance reminder.
///
/// Effort mode is not a probe gate: [`tools::assess_turn_probed`] alone decides
/// whether the keyword verdict is worth a second opinion (`Trivial`/`Small`,
/// or a `Medium` reached by length alone). That keeps pinned-effort turns able
/// to resolve the model-owned design axis without probing ordinary
/// verb-matched implementation work. When a probe clears its confidence gate,
/// its intent remains authoritative over the neutral deterministic seed.
///
/// The probe is a blocking round-trip, so it goes to the blocking pool rather
/// than holding the turn's async task: the composer and spinner stay live while
/// it resolves. A join failure is visible under `ZO_ROUTE_DEBUG` and degrades
/// to the deterministic verdict.
async fn turn_assessment(user_input: &str, model: &str) -> tools::TurnProbeAssessment {
    let text = user_input.to_string();
    let model = model.to_string();
    match tokio::task::spawn_blocking(move || tools::assess_turn_probed(&text, &model)).await {
        Ok(assessment) => assessment,
        Err(error) => {
            telemetry::attest_failed(
                telemetry::HarnessFeature::RoutingProbe,
                "blocking_join_failure",
            );
            if std::env::var("ZO_ROUTE_DEBUG").is_ok() {
                eprintln!("[ROUTE_PROBE] result=blocking_join_failure error={error}");
            }
            deterministic_turn_assessment(user_input)
        }
    }
}

/// Build this turn's design-guidance reminder, or `None` when it does not
/// apply. Armed by [`runtime::RouteTaskIntent::Design`] only when the model
/// probe resolves that deliverable intent, independent of effort mode.
///
/// The skill mandate is conditional on this session actually having a
/// frontend-design skill discoverable: telling the model to load a skill that
/// does not exist would burn a tool call and undermine the rest of the
/// reminder.
pub(super) fn build_turn_design_reminder(
    cwd: &std::path::Path,
    intent: runtime::RouteTaskIntent,
) -> Option<String> {
    // Both bail-outs are attested SEPARATELY, because the two zero-reminder
    // worlds have opposite diagnoses: `intent_not_design` accumulating over a
    // whole session means no design turn ever arrived (fine, and the proof that
    // the intent axis is the thing to look at), while `guidance_disabled` means
    // a design turn DID arrive and a switch swallowed it. Conflating them is
    // how `grep -c 'zo:design-guidance'` came to read zero for weeks with
    // nothing anywhere reporting why.
    if intent != runtime::RouteTaskIntent::Design {
        telemetry::attest_declined(
            telemetry::HarnessFeature::DesignGuidance,
            "intent_not_design",
        );
        return None;
    }
    // Checked after the intent gate on purpose: an ablation decline then
    // counts design turns whose reminder was actually suppressed, rather than
    // every turn in the run — the former is the number the experiment needs.
    if telemetry::attest_ablated(telemetry::HarnessFeature::DesignGuidance) {
        return None;
    }
    if !super::smart_settings::design_guidance_enabled() {
        telemetry::attest_declined(
            telemetry::HarnessFeature::DesignGuidance,
            "guidance_disabled",
        );
        return None;
    }
    telemetry::attest_fired(telemetry::HarnessFeature::DesignGuidance);
    let has_frontend_design_skill = runtime::discover_skills(cwd)
        .iter()
        .any(|skill| skill.name.to_ascii_lowercase().contains("frontend-design"));
    Some(runtime::build_design_guidance_reminder(
        has_frontend_design_skill,
    ))
}

/// The intent this turn installs on the runtime for the deep gate's VERIFY leg
/// ([`runtime::ConversationRuntime::set_verify_intent`]).
///
/// This is the kill-switch seam. `ZO_DESIGN_GUIDANCE=off` must disable the
/// VERIFY design lens exactly as it disables the reminder and effort-band cap,
/// and the runtime deliberately reads no env of its own — so a switched-off
/// `Design` turn is downgraded to `Other` here, at the single call that crosses
/// into the runtime, and the deep gate then behaves byte-for-byte as it did
/// before the design lens existed.
pub(super) fn verify_intent_for_turn(
    intent: runtime::RouteTaskIntent,
) -> runtime::RouteTaskIntent {
    if intent == runtime::RouteTaskIntent::Design
        && !super::smart_settings::design_guidance_enabled()
    {
        return runtime::RouteTaskIntent::Other;
    }
    intent
}

#[allow(clippy::option_option)] // cascade carries an optional reason string
struct TurnEscalation {
    grind_armed: bool,
    cascade: Option<Option<String>>,
    cascade_armed: bool,
    grind_reminder: Option<String>,
    cascade_directive_allowed: bool,
    // Pre-escalation band inputs, retained to record the full decision; the
    // caller consumes only the escalated `turn_effort`/`turn_effort_ceiling`.
    #[allow(dead_code)]
    named_effort: Option<api::EffortLevel>,
    #[allow(dead_code)]
    band_ceiling: Option<api::EffortLevel>,
    complexity: runtime::RouteTaskComplexity,
    turn_effort: Option<api::EffortLevel>,
    turn_effort_ceiling: Option<api::EffortLevel>,
}

/// This turn's intelligence/effort decision for `run_live_turn_with_images`:
/// grind/cascade escalation arming, the exclusive directive reminder, and the
/// effort band, resolved into a single `TurnEscalation`. Pure given its inputs
/// (bar `ZO_ROUTE_DEBUG` logging); the caller performs the one-shot
/// `cli.cascade_armed.take()` and the `cascade_ran_last_turn` write and hands in
/// the resulting `cascade`.
#[allow(clippy::option_option)] // cascade carries an optional reason string
fn resolve_turn_escalation(
    grind_streak: u32,
    effort: Option<Effort>,
    cascade: Option<Option<String>>,
    assessment: tools::TurnProbeAssessment,
) -> TurnEscalation {
    let complexity = assessment.complexity;
    // Grind escalation: after N consecutive budget-exhausted turns, raise
    // intelligence for this turn — effort floored at xhigh plus a
    // strategy-review directive — instead of re-running the same approach at
    // the same effort. The directive reminder is set-or-cleared with the other
    // per-turn runtime slots below.
    let grind_armed = super::grind_escalation::armed(grind_streak);
    let grind_reminder = super::grind_escalation::reminder(grind_streak);
    if std::env::var("ZO_ROUTE_DEBUG").is_ok() {
        eprintln!("[GRIND] entry streak={grind_streak} armed={grind_armed}");
    }
    let cascade_armed = cascade.is_some();
    // At most ONE escalation directive per turn — grind-armed > cascade >
    // grind-checkpoint. Without this, a streak-1 grind checkpoint ("report
    // and ask, do NOT resume heavy execution") and the cascade directive
    // ("re-derive and repair") would fire together and give the model
    // contradictory guidance. The effort floor and the cascade's wire-model
    // escalation are unaffected — only the directive text is exclusive.
    let grind_reminder = if cascade_armed && !grind_armed {
        None
    } else {
        grind_reminder
    };
    let cascade_directive_allowed = cascade_armed && !grind_armed;
    // Smart dynamic effort band: a trusted trivial/small probe may authorize a
    // lower band (low|medium ..= xhigh); medium+ turns keep the heavy
    // xhigh..=max band unchanged. `complexity` and its provenance arrive
    // already resolved by the caller, so this stays a pure mapping. TUI turns
    // only (headless keeps the static band).
    // Escalation (grind/cascade) is applied AFTER and overrides any downshift
    // with its xhigh floor.
    // Keyword-only verdicts may raise the floor for `Large`, but only a trusted
    // probe may lower it. A resolved non-Large `Design` intent independently
    // caps the measured ceiling at `xhigh`.
    let smart_band = (effort == Some(Effort::Smart))
        .then(|| {
            super::smart_settings::smart_turn_effort_band_for_complexity(
                complexity,
                assessment.intent,
                assessment.provenance,
            )
        })
        .flatten();
    if let (Some((floor, ceiling)), true) =
        (smart_band, std::env::var("ZO_ROUTE_DEBUG").is_ok())
    {
        eprintln!("[BAND] smart dynamic floor={floor:?} ceiling={ceiling:?}");
    }
    let (named_effort, band_ceiling) = match smart_band {
        Some((floor, ceiling)) => (Some(floor), Some(ceiling)),
        None => (
            effort.and_then(Effort::level),
            effort.and_then(Effort::band_ceiling),
        ),
    };
    let (turn_effort, turn_effort_ceiling) = super::grind_escalation::effective_turn_effort(
        grind_armed || cascade_armed,
        named_effort,
        band_ceiling,
    );
    TurnEscalation {
        grind_armed,
        cascade,
        cascade_armed,
        grind_reminder,
        cascade_directive_allowed,
        named_effort,
        band_ceiling,
        complexity,
        turn_effort,
        turn_effort_ceiling,
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)] // cohesive turn-lifecycle sequence
pub(crate) async fn run_live_turn_with_images(
    cli: &mut LiveCli,
    app: &mut App,
    terminal: &mut TuiTerminal,
    events: &mut EventStream,
    render_tx: &mpsc::Sender<RenderBlock>,
    cmd_rx: &mut mpsc::Receiver<AgentCommand>,
    turn_generation: u64,
    remote_approval: Option<crate::remote_control::RemoteShared>,
    user_input: String,
    images: Vec<(String, String)>,
    agent_rx: &mut mpsc::UnboundedReceiver<AgentCompletion>,
    freshness: &SessionFreshness,
    clipboard_write: &mut Option<PendingClipboardCopy>,
) -> Result<TurnOutcome, TuiLoopError> {
    let active_session_id = cli.session.id.clone();
    zo_cli::tui::watchdog::set_phase(
        zo_cli::tui::watchdog::Phase::PreTurnSetup,
    );
    // Turn prelude: make the turn visible (disable input, begin `Thinking`, draw
    // one frame) BEFORE any preflight work. The checkpoint settle, the OAuth
    // refresh, and the route probe below all run after this, so the spinner is
    // live from the instant the user's block appears — not only once a probe or
    // checkpoint join returns, which was the reported first-/follow-up-Enter
    // stall.
    begin_turn_prelude(app, terminal, turn_generation)?;
    // Adopt the previous turn's post-turn checkpoint OFF the UI thread now that
    // the spinner is live, before this turn's tools can edit files. Replaces the
    // synchronous `settle_code_checkpoint()` the submit path used to run before
    // the turn, which blocked the UI thread on the tree hash.
    settle_pending_checkpoint_pumped(cli, app, terminal, events, clipboard_write).await?;
    let hook_abort_signal = runtime::HookAbortSignal::new();
    cli.runtime.set_hook_abort_signal(hook_abort_signal.clone());
    let hook_abort_monitor = crate::HookAbortMonitor::spawn(hook_abort_signal);
    sync_turn_permission_policy(cli)?;
    // The TUI reuses one long-lived runtime across turns, so re-sync the
    // session-goal and model-handoff reminders here. Surgical so it preserves
    // the base prompt and any post-compaction reminder, and never duplicates.
    cli.apply_session_system_reminders();
    // Refresh an about-to-expire OAuth bearer before this turn's client clone is
    // taken. The long-lived interactive client captures the bearer once at
    // startup and the request path never refreshes it, so a long session would
    // otherwise 401 mid-stream once the token lapses. No-op for env-key auth or a
    // still-fresh token.
    if let Some(runtime) = cli.runtime.runtime.as_mut() {
        // This refresh runs *before* drive_turn's streaming select! loop, so a
        // slow token endpoint / loadCodeAssist round-trip would otherwise freeze
        // the whole TUI (no spinner, no input) until it returns — the reported
        // "TUI freezes ~25s every turn" on OAuth providers (Gemini/ChatGPT rebuild
        // the client every turn here). Pump the render tick so the spinner stays
        // live, and bound the wait so a hung endpoint can't stall the turn.
        let refresh =
            crate::runtime_support::refresh_oauth_if_near_expiry(runtime.api_client_mut());
        zo_cli::tui::watchdog::set_phase(
            zo_cli::tui::watchdog::Phase::OauthRefresh,
        );
        let _ = pump_draw_until(
            app,
            terminal,
            events,
            clipboard_write,
            crate::session::tui_loop::on_terminal_resize,
            tokio::time::timeout(PRETURN_REFRESH_BUDGET, refresh),
        )
        .await?;
        zo_cli::tui::watchdog::set_phase(
            zo_cli::tui::watchdog::Phase::PreTurnSetup,
        );
    }
    // G20: surface any mid-session MCP `tools/list_changed` before this turn's
    // request is assembled, so the model sees each server's current tool set.
    // The long-lived manager buffered the notifications during prior turns' tool
    // calls; refreshing here writes through the shared registry `Arc` so the
    // `live_client`/request builder below picks up the new definitions. The
    // headless `-p` path rebuilds MCP per turn (fresh discovery) and needs no
    // poll, so this lives on the long-lived TUI turn entry only.
    if let Some(mcp_state) = cli.runtime.mcp_state.clone() {
        let specs = {
            let registry = cli.runtime.api_client().tool_registry();
            super::mcp_runtime::refresh_runtime_tools_on_inbound(&mcp_state, &registry);
            // Carry the freshly-discovered MCP tools' annotation-derived
            // permission requirements into the live policy. Without this, a tool
            // discovered after startup is absent from `tool_requirements` and
            // defaults to DangerFullAccess, so a read-only fetch like context7 is
            // denied inside the ReadOnly PLAN/VERIFY sub-turns of `/goal` (and
            // `/loop` while a goal is active) even though it only reads.
            registry.permission_specs(None).ok()
        };
        if let (Some(specs), Some(runtime)) = (specs, cli.runtime.try_runtime_mut()) {
            runtime.refresh_tool_requirements(specs);
        }
    }
    // Confidence cascade: the previous turn's verbalized low-confidence
    // readout armed an escalation for THIS turn. One-shot (taken here); the
    // effort floor rides the same ladder as grind, the directive and the
    // optional same-provider Deep wire-model escalation are installed with
    // the other per-turn runtime slots below.
    let cascade_enabled = super::confidence_cascade::cascade_enabled();
    let cascade = if cascade_enabled { cli.cascade_armed.take() } else { None };
    cli.cascade_ran_last_turn = cascade.is_some();
    // One effort-independent reading of this turn feeds three decisions: the
    // effort band, the Architect contract gate, and the design-guidance
    // reminder. It is probed at most once so they cannot disagree and no
    // decision pays for a second round-trip.
    // Pump the spinner while the (possibly blocking) route probe resolves so the
    // `Thinking` state the prelude started stays animated instead of freezing
    // until the probe returns; the probe's assessment is preserved as the pump's
    // output.
    let turn_assessment = pump_draw_until(
        app,
        terminal,
        events,
        clipboard_write,
        crate::session::tui_loop::on_terminal_resize,
        turn_assessment(&user_input, &cli.model),
    )
    .await?;
    // This turn's intelligence/effort decision (grind/cascade arming, exclusive
    // directive reminder, effort band). The cascade one-shot take and the
    // `cascade_ran_last_turn` write stay above so this stays a pure mapping.
    let TurnEscalation {
        grind_armed,
        cascade,
        cascade_armed,
        grind_reminder,
        cascade_directive_allowed,
        named_effort: _,
        band_ceiling: _,
        complexity,
        turn_effort,
        turn_effort_ceiling,
    } = resolve_turn_escalation(cli.grind_streak, cli.effort, cascade, turn_assessment);
    // Resolve the cascade's Deep-tier wire-model escalation before the
    // mutable runtime borrow below. `None` (Smart off / nothing same-provider
    // routes above the main model) leaves the escalation on effort alone.
    let cascade_model = cascade_armed
        .then(|| {
            let main_model = cli.runtime.api_client().model().to_string();
            super::smart_settings::route_cascade_escalation_model(&main_model)
        })
        .flatten();
    // A one-turn `allowed-tools` override (custom prompt/slash command) is
    // enforced at EXECUTION time through the executor's shared slot — never on
    // the wire advertisement. Shrinking `tools` for one turn and restoring it
    // the next would change the first prompt-cache block twice per scoped
    // command, cold-rewriting the whole cached prefix each time ("tool
    // definitions changed" in the cache diagnostics). The advertisement stays
    // on the session-global set, byte-stable across turns.
    arm_turn_allowed_tools(cli);
    let live_client = TurnHarness::build_live_client(
        &cli.runtime,
        cli.allowed_tools.clone(),
        cli.thinking_config(),
        turn_effort,
        turn_effort_ceiling,
    );
    // The configured PLAN/VERIFY pool is the single membership test for every
    // Architect lane below — the contract gate, the deep PLAN client, and the
    // VERIFY candidates. Read once per turn entry so a `/tier` edit lands on
    // the next turn and all three lanes always agree on who counts as a planner.
    let deep_tier_models = super::smart_settings::configured_deep_tier_models();
    // Architect execution contract: implementation turns PLAN first from the
    // Medium band up. The merged `smart.execSwap` policy controls only whether
    // this classified EXEC uses the routed Coding-role implementer or the
    // session's main client.
    let exec_contract = exec_contract_for_turn(
        cli,
        &user_input,
        turn_assessment,
        tools::smart_exec_swap(),
        &deep_tier_models,
        turn_effort,
        turn_effort_ceiling,
    );
    // Cross-model deep verify: when Smart resolves the Verifier role to a
    // different model than the main one, install a verifier client so the deep
    // gate's always-on VERIFY legs judge the diff with an independent model
    // instead of self-reviewing. Re-derived every turn entry — and cleared when
    // routing yields nothing — so a model switch or `/smart off` can never
    // leave a stale verifier installed. With EXEC swapping enabled, the verify
    // route anchors on the IMPLEMENTER; otherwise it anchors on the main model
    // that wrote the diff.
    let verify_anchor = exec_contract
        .as_ref()
        .filter(|contract| contract.exec_swap_enabled())
        .map_or_else(|| cli.runtime.api_client().model().to_string(), |contract| contract.impl_model.clone());
    // The VERIFY leg's effort, resolved from the SAME band this turn installs
    // on the runtime below (`set_verify_band`), so the leg's client and the
    // gate's own depth decision read one turn. The intent is installed
    // alongside (`set_verify_intent`) for the gate's design verify floor; it
    // no longer moves effort.
    let turn_orchestration = tools::assess_turn_orchestration(&user_input);
    let verify_intent = verify_intent_for_turn(turn_assessment.intent);
    let verify_leg_effort =
        deep_verify_leg_effort(verify_leg_depth_for_band(complexity, turn_orchestration.risk));
    let deep_verify_candidates = deep_verify_candidate_clients(
        cli,
        &verify_anchor,
        &deep_tier_models,
        verify_leg_effort,
    );
    // PLAN/VERIFY stay on the configured deep-tier pool regardless of EXEC
    // policy or turn difficulty. A pool-member session plans natively; another
    // session borrows the first available configured client for PLAN as well.
    let deep_tier_only = super::smart_settings::architect_deep_lanes_enabled();
    let native_is_deep = runtime::is_deep_tier_model(
        cli.runtime.api_client().model(),
        &deep_tier_models,
    );
    let deep_plan_client = (deep_tier_only && !native_is_deep)
        .then(|| deep_verify_candidates.first().cloned())
        .flatten();
    // The foreground edit gate belongs only to a live EXEC swap. Difficulty-
    // gated native EXEC and turns without a contract are explicit owner intent
    // to let the session model edit directly.
    let reserved_edit_gate = exec_contract_arms_edit_gate(exec_contract.as_ref());
    // Cross-provider quota fallback: when the main model's subscription/quota
    // window is exhausted mid-turn, the runtime swaps to this different-provider
    // client for the rest of the turn instead of failing. Re-derived every turn
    // entry — and cleared when routing yields nothing or `/smart quota-fallback
    // off` — so a model switch can never leave a stale fallback installed.
    let quota_fallback_client = quota_fallback_async_client(cli);
    // The wait band holds the turn on the main model for a quota window that
    // resets soon, checked before any fallback swap. Read every turn entry (like
    // the fallback client) so a `/smart` edit takes effect next turn; NOT gated
    // on a fallback client existing — waiting is valid with no peer too.
    let quota_wait_band = super::smart_settings::quota_wait_band();
    // Runaway circuit breaker: bound each interactive instruction by wall-clock
    // and cumulative output tokens. Re-applied every turn (like the deadline
    // sub-agents get) so a stale bound from a prior turn never fires and an env
    // change takes effect next turn. Both default ON (generous) with an env
    // override; `0` disables either. A tripped bound stops the turn gracefully
    // (work preserved, resumable with "계속") instead of a silent multi-hour burn.
    let (turn_deadline, turn_token_budget, turn_input_budget) = interactive_turn_budgets();
    if let Some(runtime) = cli.runtime.try_runtime_mut() {
        let orchestration = &turn_orchestration;
        runtime.set_verify_band(complexity, orchestration.risk);
        // Same per-turn slot, same overwrite discipline: the resolved intent the
        // deep gate's VERIFY leg reads, with the design kill switch applied
        // here so the runtime never has to know about it. Resolved above with
        // the leg's effort so the two can never disagree.
        runtime.set_verify_intent(verify_intent);
        // Install this turn's agent-delegation policy so the `Agent` dispatch
        // guard can fold a wasteful same-model simple-implementation spawn to
        // inline (headless/serve install the same policy via
        // `TurnHarness::install_turn_agent_policy`). All-SSOT, overwritten per
        // turn like `verify_band`.
        runtime
            .tool_executor_mut()
            .tool_registry_mut()
            .context()
            .set_turn_agent_policy(Some(tools::TurnAgentPolicy {
                user_complexity: complexity,
                user_shape: orchestration.shape,
                user_need_count: orchestration.need_count,
                user_requested_delegation: orchestration.user_requested_delegation,
            }));
        runtime.set_exec_contract(exec_contract);
        runtime.set_reserved_edit_gate(reserved_edit_gate);
        runtime.set_deep_plan_client(deep_plan_client);
        runtime.set_deep_tier_only(deep_tier_only);
        runtime.set_deep_tier_models(deep_tier_models);
        runtime.set_deep_verify_candidates(deep_verify_candidates);
        runtime.set_quota_fallback_client(quota_fallback_client);
        runtime.set_quota_wait_band(quota_wait_band);
        // Set-or-clear so a prior grinding turn's strategy directive never
        // lingers into a turn that is not grinding.
        runtime.replace_transient_system_reminder_by_prefix(
            super::grind_escalation::GRIND_ESCALATION_REMINDER_PREFIX,
            grind_reminder.as_deref(),
        );
        // Confidence-cascade slots: the standing marker contract is taught
        // only until its persisted transcript copy exists (~700 constant chars
        // re-billed every turn otherwise; compaction erasing the copy re-arms
        // teaching), while the escalated turn's directive and the Deep
        // wire-model escalation stay set-or-cleared per turn.
        runtime.install_reminder_until_persisted(
            super::confidence_cascade::CONFIDENCE_CONTRACT_REMINDER_PREFIX,
            cascade_enabled
                .then(super::confidence_cascade::contract_reminder)
                .as_deref(),
        );
        let cascade_directive = cascade_directive_allowed
            .then(|| {
                cascade.as_ref().map(|reason| {
                    super::confidence_cascade::escalation_reminder(reason.as_deref())
                })
            })
            .flatten();
        runtime.replace_transient_system_reminder_by_prefix(
            super::confidence_cascade::CASCADE_ESCALATION_REMINDER_PREFIX,
            cascade_directive.as_deref(),
        );
        runtime.set_escalation_model_override(cascade_model.clone());
        match turn_deadline {
            Some(budget) => runtime.set_deadline(std::time::Instant::now() + budget),
            None => runtime.clear_deadline(),
        }
        // Progress-gated deadline extensions are an interactive-host affordance
        // only — sub-agents keep their deadline as a hard straggler bound. Read
        // per turn so an env retune takes effect without a rebuild.
        runtime.set_deadline_extension(runtime::env_deadline_extension());
        runtime.set_turn_output_token_budget(turn_token_budget);
        runtime.set_turn_input_token_budget(turn_input_budget);
    }

    // Stage 4 — adaptive routing. Build the cheap, model-free route hint once,
    // before the `&mut` runtime borrow.
    let route_hint = super::auto_fanout::build_route_hint(
        &user_input,
        cli.effort,
        estimated_fanout_context_tokens(cli.runtime.estimated_tokens(), &cli.system_prompt),
    )
    // Escalate one step if the previous turn failed in a way escalation helps
    // (WI-B). A no-op when the last turn succeeded or escalation already stopped.
    .escalate(cli.route_escalation);
    // The host pre-spawns agents only on the hint's fast-path; semantic-triage
    // preludes are tracked separately because they may fall back to the model-led
    // turn without ever launching agents. The decomposition/wait happens inside
    // `drive_turn`, after the TUI event stream is live, so typing, scrolling,
    // and Ctrl+C keep working while pre-analysis runs.
    let auto_fanout_plan = AutoFanoutPlan::from_hint(cli, &route_hint);
    // Audit: record what the host decided so "why did it (not) spawn?" is
    // answerable. Env-guarded to match the existing `ZO_PROFILE_TURN`
    // diagnostic convention (the cli has no log/tracing dependency); the route
    // *logic* itself is unit-tested on `build_route_hint`.
    if std::env::var("ZO_ROUTE_DEBUG").is_ok() {
        eprintln!(
            "[ROUTE] shape={} canonical={} intent={} confidence={:.2} host_prespawn={} semantic_triage={} reasons={:?}",
            route_hint.shape.as_str(),
            route_hint.canonical_shape_kind().label(),
            turn_assessment.intent.as_str(),
            route_hint.confidence,
            auto_fanout_plan
                .as_ref()
                .is_some_and(|plan| plan.host_prespawn),
            auto_fanout_plan
                .as_ref()
                .is_some_and(|plan| plan.semantic_triage),
            route_hint.reasons,
        );
    }
    // Surface the host's route decision to the user as a one-line banner for any
    // non-Solo turn (principle ①). The host classifies every turn, but the
    // decision was previously visible only via the `ZO_ROUTE_DEBUG` eprintln
    // above or an `/audit` count — never in the live TUI. This is deterministic
    // and independent of whether the model narrates; pushed before `drive_turn`
    // so it renders at turn start, ahead of the model's own (authoritative)
    // routing narration. Solo turns stay clean (no banner for direct handling).
    if let Some(line) = route_hint.user_hint_line(
        auto_fanout_plan
            .as_ref()
            .is_some_and(|plan| plan.host_prespawn),
    ) {
        app.push_block(RenderBlock::System {
            id: BlockIdGen::default().next(),
            level: SystemLevel::Info,
            text: line,
        });
    }
    // Surface the grind escalation as a one-line banner: an invisible effort
    // bump would leave the user wondering why this turn thinks longer, and the
    // whole point is to visibly change approach rather than silently retry.
    if grind_armed {
        app.push_block(RenderBlock::System {
            id: BlockIdGen::default().next(),
            level: SystemLevel::Warn,
            text: format!(
                "[grind] {} consecutive budget-exhausted turns — escalating this turn: effort floored at xhigh + strategy review",
                cli.grind_streak
            ),
        });
    }
    // Same visibility principle for the confidence cascade: an invisible
    // model/effort escalation would leave the user wondering why this turn
    // runs differently.
    if cascade_armed {
        app.push_block(RenderBlock::System {
            id: BlockIdGen::default().next(),
            level: SystemLevel::Warn,
            text: match &cascade_model {
                Some(model) => format!(
                    "[cascade] previous turn self-reported low confidence — escalating this turn to {model} (effort floored at xhigh)"
                ),
                None => "[cascade] previous turn self-reported low confidence — escalating this turn: effort floored at xhigh".to_string(),
            },
        });
    }
    // Surface the hint to the model unless the host will actually consume the
    // turn (a breadth pre-spawn); the gate lives in `model_reminder`. A
    // non-breadth ultracode pre-spawn runs the triage but usually defers to the
    // model-led turn, so it keeps its shape reminder. Always set-or-clear so a
    // prior turn's hint never lingers.
    let route_reminder = route_reminder_for_hint(&route_hint);

    // Skill auto-routing (Codex-style implicit invocation): score this turn's
    // text against discovered active skills' trigger metadata and, when one
    // clearly fits, nudge the model to load it via the `Skill` tool. Advisory
    // only — never force-loads a body, never bypasses the proposed-skill gate.
    // Computed before the `&mut runtime` borrow; re-scanned per turn so newly
    // added/approved skills are picked up without a restart. Discovery performs
    // filesystem I/O, so keep it off the async UI thread and pump frames until
    // the blocking task returns.
    let skill_cwd = cli.cwd.clone();
    let skill_input = user_input.clone();
    let skill_reminder = pump_blocking_with_draw(
        app,
        terminal,
        events,
        clipboard_write,
        crate::session::tui_loop::on_terminal_resize,
        move || build_turn_skill_reminder(&skill_cwd, &skill_input),
    )
    .await?;
    if std::env::var("ZO_SKILL_DEBUG").is_ok() {
        eprintln!("[SKILL] reminder={skill_reminder:?}");
    }

    // Design guidance: armed only by the model-owned resolved intent. Like the
    // route/skill reminders it rides the newest user wire message, so it costs
    // nothing in prefix-cache invalidation and is cleared at the next turn's
    // start.
    let design_reminder = build_turn_design_reminder(&cli.cwd, turn_assessment.intent);

    // Captured before the `&mut runtime` borrow below: a per-turn label so this
    // turn's tool invocations and its route decision share a `turn_id` in the
    // audit (WI-C).
    let route_turn_label = format!("turn-{}", cli.runtime.session().messages.len());
    let checkpoint_turn_index = cli
        .runtime
        .session()
        .messages
        .iter()
        .filter(|message| message.role == runtime::MessageRole::User)
        .count()
        .saturating_add(1);
    let Some(runtime) = cli.runtime.runtime.as_mut() else {
        return Err(TuiLoopError::Turn("runtime not available".to_string()));
    };
    runtime.replace_transient_system_reminder_by_prefix(
        super::auto_fanout::ROUTE_HINT_REMINDER_PREFIX,
        route_reminder.as_deref(),
    );
    // Install (or clear) the per-turn skill recommendation the same way as the
    // route hint, so a prior turn's nudge never lingers.
    runtime.replace_transient_system_reminder_by_prefix(
        runtime::SKILL_RECOMMENDATION_REMINDER_PREFIX,
        skill_reminder.as_deref(),
    );
    // Same set-or-clear discipline as the two above: a design turn followed by
    // "now run the tests" must not still be carrying design guidance.
    runtime.replace_transient_system_reminder_by_prefix(
        runtime::DESIGN_GUIDANCE_REMINDER_PREFIX,
        design_reminder.as_deref(),
    );
    // Record the route decision as a structured audit event (the
    // `ZO_ROUTE_DEBUG` eprintln above stays an optional console echo), and
    // stamp the active turn so subsequent tool invocations join it (WI-C).
    {
        let context = runtime.tool_executor_mut().tool_registry_mut().context();
        context.begin_workspace_checkpoint(checkpoint_turn_index);
        context.set_active_turn_id(Some(route_turn_label));
        context.record_route_decision(tools::RouteDecisionRecord {
            shape: route_hint.shape.as_str().to_string(),
            canonical_shape: route_hint.canonical_shape_kind().label().to_string(),
            intent: turn_assessment.intent.as_str().to_string(),
            confidence: route_hint.confidence,
            host_prespawn: auto_fanout_plan
                .as_ref()
                .is_some_and(|plan| plan.host_prespawn),
            semantic_triage: auto_fanout_plan
                .as_ref()
                .is_some_and(|plan| plan.semantic_triage),
            reasons: route_hint
                .reasons
                .iter()
                .map(|reason| (*reason).to_string())
                .collect(),
            turn_id: None,
        });
    }
    install_tui_user_question_channel(
        runtime.tool_executor_mut().tool_registry_mut(),
        render_tx.clone(),
        runtime::message_stream::BlockIdGen::default(),
    );
    // `drive_turn` OWNS the runtime for the turn (it moves it onto a spawned
    // task), so hand it the slot rather than a borrow; it restores the slot
    // before returning. The `runtime` binding above is done being used (its borrow
    // ended at `install_tui_user_question_channel`), so reborrowing the slot here
    // is clean.
    // `Box::pin`: this turn future sits just over clippy's `large_futures`
    // threshold after the per-turn runtime grew (the quota-fallback client +
    // cooldown state on `ConversationRuntime`), so heap-allocate it rather than
    // carry a 16 KB future on the stack of every awaiting frame.
    let outcome = Box::pin(drive_turn(
        &mut cli.runtime.runtime,
        live_client,
        user_input,
        images,
        app,
        terminal,
        events,
        render_tx.clone(),
        cmd_rx,
        turn_generation,
        remote_approval,
        agent_rx,
        active_session_id,
        auto_fanout_plan,
        freshness,
        clipboard_write,
    ))
    .await;
    zo_cli::tui::watchdog::set_phase(zo_cli::tui::watchdog::Phase::PostTurn);
    hook_abort_monitor.stop();
    if let Some(runtime) = cli.runtime.runtime.as_mut() {
        let context = runtime.tool_executor_mut().tool_registry_mut().context();
        if let Err(error) = context.finish_workspace_checkpoint() {
            eprintln!("warning: failed to finalize workspace checkpoint: {error}");
        }
    }
    // Clear any one-turn `allowed-tools` override set by a custom prompt/slash
    // command so the tool restriction applies to exactly this turn and never
    // leaks into the next one — on both the success and failure paths. The
    // executor's shared slot (the actual enforcement point) is cleared with it.
    cli.turn_allowed_tools = None;
    arm_turn_allowed_tools(cli);
    // Drain any "always allow" rules granted during the turn so they can be
    // persisted to the project's local settings. `drive_turn` restored the
    // runtime slot before returning; the only way it is `None` here is a turn-task
    // panic, in which case there are no rules to drain.
    let granted_rules = cli
        .runtime
        .runtime
        .as_mut()
        .map(ConversationRuntime::take_granted_permission_rules)
        .unwrap_or_default();

    match outcome {
        Ok(outcome) => {
            // A budget-exhausted turn returns `Ok` (graceful stop) but did NOT
            // converge: feed the escalation ladder and grind streak instead of
            // resetting them — the unconditional reset here is exactly how the
            // exhaust→"계속"→exhaust grind stayed invisible to WI-B routing.
            // A genuinely clean turn resets both (WI-B).
            if std::env::var("ZO_ROUTE_DEBUG").is_ok() {
                eprintln!(
                    "[GRIND] exit summary_present={} budget={:?}",
                    outcome.summary.is_some(),
                    outcome
                        .summary
                        .as_ref()
                        .and_then(|summary| summary.budget_exhausted)
                );
            }
            match outcome
                .summary
                .as_ref()
                .and_then(|summary| summary.budget_exhausted)
            {
                Some(kind) => cli.record_turn_budget_exhausted(kind),
                None => cli.clear_turn_failure(),
            }
            cli.persist_appended_session_offloaded()
                .await
                .map_err(|error| TuiLoopError::Turn(error.to_string()))?;
            record_confidence_cascade(cli, &outcome);
            record_deep_verdict_outcomes(cli, &outcome);
            persist_granted_rules(&granted_rules);
            Ok(outcome)
        }
        Err(error) => {
            // Remember the failure so the next turn's route can escalate (WI-B).
            cli.record_turn_failure(&error.to_string());
            Err(error)
        }
    }
}

fn sync_turn_permission_policy(cli: &mut LiveCli) -> Result<(), TuiLoopError> {
    if cli.permission_mode != runtime::PermissionMode::DangerFullAccess {
        return Ok(());
    }

    let policy = crate::conversation_support::permission_policy(
        cli.permission_mode,
        &cli.runtime.feature_config,
        &cli.runtime.api_client().tool_registry(),
    )
    .map_err(|error| TuiLoopError::Turn(error.clone()))?;
    if let Some(runtime) = cli.runtime.try_runtime_mut() {
        runtime.set_permission_policy(policy);
    }
    Ok(())
}

/// Scan the finished turn's final assistant text for the verbalized
/// confidence readout and arm the cascade for the next turn on `low`. An
/// escalated turn never immediately re-arms (one escalation per low streak —
/// see `LiveCli::cascade_ran_last_turn`); the directive's report contract
/// surfaces persistent uncertainty to the user instead.
fn record_confidence_cascade(cli: &mut LiveCli, outcome: &TurnOutcome) {
    if !super::confidence_cascade::cascade_enabled() {
        cli.cascade_armed = None;
        return;
    }
    let Some(summary) = outcome.summary.as_ref() else {
        return;
    };
    let final_text = summary.assistant_messages.iter().rev().find_map(|message| {
        let text = message
            .blocks
            .iter()
            .filter_map(|block| match block {
                core_types::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        (!text.trim().is_empty()).then_some(text)
    });
    let Some(text) = final_text else { return };
    let readout = super::confidence_cascade::parse_turn_confidence(&text);
    if super::confidence_cascade::should_arm(readout) && !cli.cascade_ran_last_turn {
        cli.cascade_armed = Some(super::confidence_cascade::parse_confidence_reason(&text));
    }
}

/// Phase 4 verdict channel — source #1: the deep-gate VERIFY sub-turn that runs
/// on (almost) every edited main turn. Records TWO outcomes when a VERIFY leg
/// actually ran this turn:
///
/// (i) the JUDGED main turn itself, under a new `targetKind:"main"` /
///     `target:"turn"` route (`route_key` `"main:turn"`), `selectedModel` the
///     MAIN session model, `signal:"verdict"` — the quality signal the
///     outcome store had ZERO of before this phase (§1.3 of the routing
///     plan). Recorded ONLY when the leg produced a usable verdict
///     (`VerifierParse::Json | Salvaged`) — an ambiguous/unparseable leg
///     (`Empty | Unparseable | Timeout`) is never attributed to the main
///     turn's quality, per the "ambiguous verdicts are never recorded"
///     doctrine.
/// (ii) the VERIFY leg's own run, under `targetKind:"deep-verify"` /
///     `target:"leg"` (`route_key` `"deep-verify:leg"`), `selectedModel` the
///     verifier model (the cross-model verifier when one is installed via
///     `set_deep_verify_candidates`, else the main model — VERIFY always runs on
///     SOME model). This is a normal did-run record (no `signal`), letting
///     the outcome store learn which verifier models actually complete
///     VERIFY legs versus time out or produce unusable output — and is what
///     `route_deep_verify_candidates`' feedback hint reads back. Recorded
///     whenever a leg ran, whether or not it yielded a usable verdict (an
///     unusable result is itself a valid "this run was bad" signal about the
///     verifier).
///
/// No VERIFY leg at all this turn (`deep_verifier_parse: None` — a no-edit
/// chat turn, or no deep gate installed) records nothing. Both records are
/// always terminal (`completed`/`failed`, never `still_running` — the shared
/// P3 doctrine guard in `runtime::record_route_outcome` would skip-write
/// anything else).
fn record_deep_verdict_outcomes(cli: &LiveCli, outcome: &TurnOutcome) {
    record_deep_verdict_outcomes_for(&cli.model, &cli.cwd, outcome);
}

/// Core of [`record_deep_verdict_outcomes`], split out so unit tests can
/// exercise the gating logic directly against a temp `cwd` without
/// constructing a full [`LiveCli`].
fn record_deep_verdict_outcomes_for(main_model: &str, cwd: &std::path::Path, outcome: &TurnOutcome) {
    let Some(summary) = outcome.summary.as_ref() else {
        return;
    };
    let Some(parse) = summary.deep_verifier_parse else {
        return;
    };
    let verifier_model = summary
        .deep_verifier_model
        .clone()
        .unwrap_or_else(|| main_model.to_string());
    let usable = matches!(
        parse,
        decision_core::deep_lane::VerifierParse::Json
            | decision_core::deep_lane::VerifierParse::Salvaged
    );

    // (ii) the VERIFY leg's own run — always recorded once a leg ran, whether
    // or not it produced a usable verdict.
    let leg_record = RouteOutcomeRecord::new(
        "deep-verify",
        "leg",
        api::resolve_model_alias(&verifier_model),
        if usable { "completed" } else { "failed" },
    );
    let _ = runtime::record_route_outcome(cwd, &leg_record);

    // (i) the judged main turn — only when the leg produced a usable verdict.
    if !usable {
        return;
    }
    let Some(passed) = summary.deep_verification else {
        return;
    };
    let turn_record = RouteOutcomeRecord::new(
        "main",
        "turn",
        api::resolve_model_alias(main_model),
        if passed { "completed" } else { "failed" },
    )
    .with_signal("verdict")
    // Strict pass/fail judgement — same convention/weight as
    // `tools::workflow_tools::engine::attribution::VerdictKind::PassFail`
    // (a cross-crate type, so the constant is mirrored here rather than
    // shared — see that type's doc for the full pass_fail/preference scale).
    .with_signal_weight(Some(1.0))
    // P1 pair attribution: (implementation model = this record's
    // `selected_model`, verification model = the deep-verify leg's model).
    // `Some` only when a cross-model verifier actually ran — a native
    // same-model verify leaves `deep_verifier_model` `None`, so no pair is
    // recorded. Canonicalized the same way this record's `selected_model` and
    // the verify-leg's `selected_model` are (P3 write-time canonicalization),
    // so `/smart doctor`'s pair table groups model aliases together.
    .with_verifier_model(
        summary
            .deep_verifier_model
            .as_deref()
            .map(api::resolve_model_alias),
    );
    let _ = runtime::record_route_outcome(cwd, &turn_record);
}

/// Persist newly-granted "always allow" rules to the project's local settings
/// (`.zo/settings.local.json`) so they survive across sessions. Best-effort:
/// any I/O / parse failure is silently skipped — the rule is still active for
/// the current session via the live policy.
fn persist_granted_rules(rules: &[String]) {
    if rules.is_empty() {
        return;
    }
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let dir = cwd.join(".zo");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("settings.local.json");
    let mut root = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let Some(obj) = root.as_object_mut() else {
        return;
    };
    let perms = obj
        .entry("permissions")
        .or_insert_with(|| serde_json::json!({}));
    if !perms.is_object() {
        *perms = serde_json::json!({});
    }
    let Some(perms_obj) = perms.as_object_mut() else {
        return;
    };
    let allow = perms_obj
        .entry("allow")
        .or_insert_with(|| serde_json::json!([]));
    if !allow.is_array() {
        *allow = serde_json::json!([]);
    }
    let Some(arr) = allow.as_array_mut() else {
        return;
    };
    for rule in rules {
        let value = serde_json::Value::String(rule.clone());
        if !arr.contains(&value) {
            arr.push(value);
        }
    }
    if let Ok(pretty) = serde_json::to_string_pretty(&root) {
        if let Err(err) = std::fs::write(&path, pretty) {
            eprintln!(
                "[zo] warning: failed to persist permission allow-rules to {}: {err}",
                path.display()
            );
        }
    }
}

/// The value a spawned turn task hands back: the runtime it owned for the turn
/// (so the session survives a Ctrl+C — the task returns it rather than losing it)
/// plus the turn's result. See [`drive_turn`].
type TurnHandleOutput<T> = (
    ConversationRuntime<crate::AnthropicRuntimeClient, T>,
    Result<TurnSummary, StreamingTurnError>,
);

/// Poll the turn task's shared abort flag so a Ctrl+C (set on the flag by
/// `hook_abort_monitor`, or by the render loop's recovery path) cancels the
/// in-flight turn promptly. Resolves once the flag is set; the 25 ms cadence
/// keeps cancel latency imperceptible without a busy-wait. Used as the cancel arm
/// of the spawned turn's `select!` in [`drive_turn`].
async fn wait_until_aborted(signal: &runtime::HookAbortSignal) {
    while !signal.is_aborted() {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub(crate) async fn drive_turn<T>(
    runtime_slot: &mut Option<ConversationRuntime<crate::AnthropicRuntimeClient, T>>,
    live_client: Arc<LiveAsyncApiClient>,
    user_input: String,
    images: Vec<(String, String)>,
    app: &mut App,
    terminal: &mut TuiTerminal,
    events: &mut EventStream,
    render_tx: mpsc::Sender<RenderBlock>,
    cmd_rx: &mut mpsc::Receiver<AgentCommand>,
    turn_generation: u64,
    remote_approval: Option<crate::remote_control::RemoteShared>,
    agent_rx: &mut mpsc::UnboundedReceiver<AgentCompletion>,
    active_session_id: String,
    auto_fanout_plan: Option<AutoFanoutPlan>,
    freshness: &SessionFreshness,
    clipboard_write: &mut Option<PendingClipboardCopy>,
) -> Result<TurnOutcome, TuiLoopError>
where
    T: runtime::ToolExecutor + Send + 'static,
{
    // Borrow the runtime for the pre-turn setup and the Smart prelude. The
    // turn itself takes OWNERSHIP only just before it is spawned (see the
    // `tokio::spawn` below), so until then the runtime stays in the caller's slot:
    // an early return from the prelude (e.g. a cancelled pre-analysis, or a draw
    // error) leaves the slot intact for the next turn. After the turn finishes the
    // recovery point restores the slot; only a panic in the turn task can leave it
    // empty, which the caller's entry guard then reports cleanly.
    let runtime = runtime_slot
        .as_mut()
        .ok_or_else(|| TuiLoopError::Turn("runtime not available".to_string()))?;
    runtime.set_async_api_client(live_client);
    // Steering handle captured before the streaming future borrows `runtime`.
    // Mid-turn `AgentCommand::Steer` pushes here; the turn drains it at each
    // tool-result boundary so the agent adjusts course within this turn.
    let steering = runtime.steering_handle();
    // Agent-notification inbox, same lifetime dance: background agents that
    // complete while THIS turn is live are staged here and folded into the
    // turn at its next tool-result boundary (CC task-notification parity), so
    // the main model keeps working through completions instead of ending its
    // turn to receive them. Leftovers are re-queued after the turn (below).
    let agent_notifications = runtime.agent_notification_inbox();
    // HUD ctx/cost are now driven by real `RenderBlock::Usage` snapshots emitted
    // from the streaming loop (see `App::record_live_usage`), so no pre-turn
    // base estimate is needed here.
    // `run_live_turn_with_images`'s prelude already began this turn's `Thinking`
    // activity before preflight; this is the idempotent no-op guard for
    // `drive_turn`'s own entry (`begin_turn_with_generation` no-ops once a turn
    // is active), keeping it self-contained without a duplicate begin.
    begin_turn_prelude(app, terminal, turn_generation)?;

    // Whether a Smart host prelude ran this turn. Used below to advance the
    // activity label off the prelude's final note once the main model takes
    // over, so it doesn't sit frozen on e.g. "Smart: continuing with the main
    // model" until the first content delta arrives.
    let ran_fanout_prelude = auto_fanout_plan.is_some();
    let user_input = if let Some(plan) = auto_fanout_plan {
        let prelude = match maybe_apply_auto_fanout_live(
            app,
            terminal,
            AutoFanoutLiveChannels {
                events: &mut *events,
                commands: cmd_rx,
                agent_completions: agent_rx,
                turn_generation,
            },
            user_input,
            plan,
            freshness,
            clipboard_write,
        )
        .await
        {
            Ok(prelude) => prelude,
            Err(error) => {
                app.enable_input();
                app.abort_turn();
                return Err(error);
            }
        };
        match prelude {
            AutoFanoutPrelude::Ready(user_input) => user_input,
            AutoFanoutPrelude::ReadyWithEvidence {
                user_input,
                evidence,
            } => {
                // The host actually fanned out this turn and its findings are now
                // in context. Enforce (not just nudge) the host-XOR-model spawn
                // invariant (BUG-D2): replace the original "consider delegating"
                // route reminder with one that tells the model to build on the
                // pre-analysis rather than launch a second, duplicate fan-out.
                runtime.replace_transient_system_reminder_by_prefix(
                    super::auto_fanout::ROUTE_HINT_REMINDER_PREFIX,
                    Some(super::auto_fanout::PRELUDE_FANNED_OUT_REMINDER),
                );
                // Deliver the completed pre-analysis as a synthetic, microcompact-
                // clearable `SpawnMultiAgent` tool-result pair (assistant ToolUse +
                // matching tool_result) BEFORE the turn pushes the user input. The
                // tool-result body is not an edit result, so microcompact can clear
                // it once it ages past the recent tail — unlike a permanent user-
                // message prepend, which re-bills as cache_read every later turn.
                // The turn then runs on the ORIGINAL user input untouched.
                //
                // Wire-safety guard (Option A): the synthetic assistant(tool_use)
                // must not lead the conversation (empty session) and must not
                // follow a prior assistant message (two consecutive assistant-wire
                // turns). Check before attempting the push; fall back to the
                // combined prepend when the sequence would be malformed.
                if fanout_evidence_injection_is_wire_safe(runtime.session()) {
                    let id = format!("auto_fanout_{}", runtime.session().messages.len());
                    let tool_use = runtime::ConversationMessage::assistant(vec![
                        runtime::ContentBlock::ToolUse {
                            id: id.clone(),
                            name: "SpawnMultiAgent".to_string(),
                            input: r#"{"reason":"parallel pre-analysis"}"#.to_string(),
                        },
                    ]);
                    let tool_result = runtime::ConversationMessage::tool_result(
                        id,
                        "SpawnMultiAgent",
                        evidence.clone(),
                        false,
                    );
                    let session = runtime.session_mut();
                    match push_synthetic_fanout_evidence_pair(session, tool_use, tool_result) {
                        // Synthetic pair seated cleanly: run the turn on the
                        // untouched original input.
                        Ok(()) => user_input,
                        // A push failed (e.g. persistence I/O). Never break the
                        // turn: fall back to the old combined prepend so the
                        // analysis still reaches the model in this turn's input.
                        Err(_) => format!("{user_input}\n\n---\n{evidence}"),
                    }
                } else {
                    // Prepend path: evidence reaches the model in the user turn
                    // itself rather than via a clearable tool-result, but the
                    // wire sequence is always valid.
                    user_input + "\n\n---\n" + &evidence
                }
            }
            AutoFanoutPrelude::Cancelled => {
                app.enable_input();
                app.abort_turn();
                return Ok(TurnOutcome { summary: None });
            }
        }
    } else {
        user_input
    };

    // Advance the label off the prelude's final note ("…continuing with the main
    // model", a synthesizing line, etc.) the moment the main turn starts, and —
    // via `set_turn_activity`'s `mark_event` — refresh the stall clock so the
    // hand-off doesn't read as "no output". The model stream then overwrites this
    // with its own reasoning/text action on the first delta.
    if ran_fanout_prelude {
        app.set_turn_activity(zo_cli::tui::blocks::reasoning::ZO_REVEAL_VERBS[0]);
    }

    let (prompter, prompter_rx) = ChannelPrompter::new(4);
    let prompter_arc: Arc<dyn runtime::permission::PermissionPrompter> = Arc::new(prompter);
    let pump_render_tx = render_tx.clone();
    let pump_ids = BlockIdGen::default();
    let (remote_resolution_tx, mut remote_resolution_rx) = mpsc::unbounded_channel();
    let pump_handle: JoinHandle<()> = tokio::spawn(async move {
        let _ = run_permission_pump_with_remote(
            prompter_rx,
            pump_render_tx,
            pump_ids,
            remote_approval,
            Some(remote_resolution_tx),
        )
        .await;
    });

    let copy_session_snapshot = runtime.session().clone();
    // Run the turn on its OWN task so a heavy synchronous segment inside it
    // (request assembly, large-message construction, mid-turn compaction, big
    // tool-input parsing) can never starve the render / event / spinner arms of
    // the `select!` below — the root fix for the mid-stream "output stops then
    // resumes" freeze. Polling the turn inline meant one long synchronous stretch
    // held this `select!` task and starved `render_tick`; spawning hands the turn
    // to another worker thread, so the render loop only ever waits on channels.
    //
    // The task OWNS `runtime` and hands it back on completion, so the session
    // survives a Ctrl+C: the abort flag (shared with `hook_abort_monitor`, and set
    // by the recovery path below) makes the task's inner `select!` drop the
    // in-flight turn — the same instant stream-kill as the old inline `drop` — and
    // return the runtime with a `Cancelled` result.
    // The prelude is past (its early-return paths left the slot intact); take
    // OWNERSHIP now so the runtime can move onto the spawned turn task. The
    // `as_mut` borrow above proved the slot is `Some` and nothing clears it in
    // between, so this cannot fail.
    let mut runtime = runtime_slot
        .take()
        .expect("runtime slot was Some at the top of drive_turn");
    let abort_signal = runtime.hook_abort_signal();
    // Cloned before the runtime moves onto the turn task. Unlike the abort
    // signal this is NOT re-created per turn: it is a monotonic epoch, so there
    // is nothing to reset and a stale clone can never cancel a later tool.
    let tool_cancel_signal = runtime.tool_cancel_signal();
    let task_abort = abort_signal.clone();
    let user_cancel_requested = Arc::new(AtomicBool::new(false));
    let task_user_cancel_requested = Arc::clone(&user_cancel_requested);
    let mut turn_handle: JoinHandle<(
        ConversationRuntime<crate::AnthropicRuntimeClient, T>,
        Result<TurnSummary, StreamingTurnError>,
    )> = tokio::spawn(async move {
        let result = tokio::select! {
            biased;
            // `/deep` routes through the deep-lane gate; otherwise the ordinary turn.
            outcome = runtime.run_turn_streaming_maybe_deep(
                user_input,
                images,
                render_tx,
                prompter_arc,
            ) => {
                // A retry/provider future can resolve in the same poll that the
                // host requests cancellation. The stop request is authoritative:
                // re-check it after the turn future resolves so a provider error
                // cannot win the `select!` race and become actionable evidence.
                if task_user_cancel_requested.load(Ordering::SeqCst) {
                    Err(runtime.cancel_streaming_turn_by_user("turn cancelled by user"))
                } else if task_abort.is_aborted() {
                    Err(runtime.cancel_streaming_turn_by_host(
                        "turn aborted because the interactive host stopped",
                    ))
                } else {
                    outcome
                }
            },
            () = wait_until_aborted(&task_abort) => {
                if task_user_cancel_requested.load(Ordering::SeqCst) {
                    Err(runtime.cancel_streaming_turn_by_user("turn cancelled by user"))
                } else {
                    Err(runtime.cancel_streaming_turn_by_host(
                        "turn aborted because the interactive host stopped",
                    ))
                }
            },
        };
        (runtime, result)
    });
    // Tick at ~30 fps (33 ms) so the spinner/HUD animate on the *same* frame
    // grid as stream-driven redraws and the prose pacer.
    // Previously the tick ran at 20 fps (50 ms) while stream draws ran at 30 fps
    // (33 ms): the two cadences beat against each other, so frames landed at an
    // irregular 33/50 ms spacing that reads as micro-stutter during streaming.
    // A single 30 fps grid makes every frame evenly spaced. The draw path is
    // cheap (only visible blocks are rendered), so 30 fps idle is still light.
    let mut render_tick = tokio::time::interval(ANIMATION_TICK_INTERVAL);
    render_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut summary: Option<Result<TurnSummary, StreamingTurnError>> = None;
    // Raw output of the turn task, captured by the `joined` arm of the render loop
    // when the handle completes (the turn finished, or self-cancelled from within
    // via the abort poll). `None` after the loop means the loop exited *before*
    // the turn finished (Ctrl+C / Quit / event-stream end / a render error); the
    // recovery block below then aborts the task and awaits the runtime back.
    let mut turn_output: Option<Result<TurnHandleOutput<T>, tokio::task::JoinError>> = None;
    let mut cancelled = false;
    let mut auth_failure_reported = false;
    let mut rate_limit_failure_reported = false;
    // A Ctrl+V clipboard read in flight. The read is blocking (process spawn +
    // base64 of a multi-MB image), so it runs on a blocking thread and its
    // result is applied by the `clipboard_read` arm below — never inline in the
    // event arm, which would stall this `select!` and freeze the spinner/stream.
    let mut clipboard_read: Option<tokio::task::JoinHandle<Option<ClipboardPayload>>> = None;
    // `clipboard_write` is owned by the outer session loop, so it survives this
    // streaming turn and continues to collapse duplicate copy actions after the
    // turn hands control back to the idle prompt.
    let mut live_hud_snapshot: Option<JoinHandle<LiveHudSnapshot>> = None;
    let mut changed_files_snapshot: Option<JoinHandle<Option<GitStatusSnapshot>>> = None;
    let mut workflow_view_snapshot: Option<JoinHandle<Option<WorkflowView>>> = None;
    let mut agents_rows_snapshot: Option<JoinHandle<AgentRowsSnapshot>> = None;
    // Freeze probe (ZO_PROFILE_TURN): log when the render tick is starved. The
    // turn now runs on its own task, so the only thing that can still starve this
    // loop is a heavy synchronous segment ON the render side — e.g. one expensive
    // `app.draw`. The probe pinpoints any such residual stall.
    let profile_turn = runtime::turn_profiling_enabled();
    let mut last_render_tick = std::time::Instant::now();
    let mut frame_gate = StreamFrameGate::new_ready(
        std::time::Instant::now(),
        STREAM_FRAME_INTERVAL,
    );

    // Render / input / event loop. The turn runs on `turn_handle` (spawned
    // above), so no synchronous segment inside the turn can starve these arms.
    // Wrapping the loop in an `async` block lets a render error (`?`) or the
    // event-stream IO error fall through to `loop_result`; the single recovery
    // point just below then reclaims `runtime` from the task on EVERY exit — not
    // only the clean ones — so the slot is always restored before this returns.
    zo_cli::tui::watchdog::set_phase(zo_cli::tui::watchdog::Phase::TurnRender);
    let loop_result: Result<(), TuiLoopError> = async {
    loop {
        // Liveness heartbeat for the freeze watchdog (shared with the idle
        // loop): a stalled beat here means the mid-turn render task is wedged.
        zo_cli::tui::watchdog::beat();
        tokio::select! {
            joined = &mut turn_handle => {
                // The turn task finished (normal completion, or self-cancelled
                // from within via the abort poll). Capture its output — which
                // carries `runtime` back — and let the single recovery point
                // below restore the caller's slot.
                turn_output = Some(joined);
                drain_into_app(app);
                while let Ok(resolution) = remote_resolution_rx.try_recv() {
                    apply_remote_permission_resolution(app, resolution);
                }
                // Do not paint one last active-spinner frame after the provider
                // future has completed. The shutdown path below drains any
                // remaining ready blocks, clears turn_activity, and then draws
                // the definitive settled frame.
                break;
            }

            maybe_block = app.recv_block() => {
                // Wake on render-channel traffic, not only on the timer tick.
                // Absorb a bounded frame's worth of blocks immediately, but
                // *throttle the actual draw* to ~30 fps: a fast stream can push
                // tens-to-hundreds of token blocks per second, and redrawing a
                // long transcript on every one floods the terminal with full-
                // screen repaints faster than the emulator can apply them —
                // the "context fills up → stutter + torn/garbled frames" the
                // user sees. Coalescing draws to a min interval keeps the
                // newest content visible without ever exceeding the terminal's
                // paint throughput; `render_tick` paints any frame this skipped,
                // so nothing sits buffered.
                if let Some(block) = maybe_block {
                    let _ = app.drain_ready_blocks_with_first(block);
                    if frame_gate.on_stream_update(std::time::Instant::now()).draws_now() {
                        // Drive the streaming pacer on this block-driven repaint,
                        // not only on the 33 ms render tick: `drain_ready_blocks_*`
                        // buffers arrived prose into the pacer, so without a drip
                        // here a fast stream would only advance on the tick and
                        // read as stepped. `drip_stream` is wall-clock based and
                        // reveals exactly the elapsed-time budget, so it never
                        // dumps a whole burst — the tail still spreads across
                        // frames. Throttled by the shared frame gate so a fast
                        // burst can't redraw faster than the emulator paints.
                        app.drip_stream();
                        let draw_started = std::time::Instant::now();
                        app.draw_frame(terminal)?;
                        // Feed the measured draw cost back into the gate: on a
                        // terminal that paints slower than the frame grid
                        // (Apple Terminal.app), the stream cadence stretches so
                        // this loop keeps most of its time for the input arms
                        // instead of saturating on blocked tty writes.
                        frame_gate.note_draw_cost(draw_started.elapsed());
                        frame_gate.note_stream_draw(std::time::Instant::now());
                        if profile_turn {
                            last_render_tick = std::time::Instant::now();
                        }
                    }
                    // A burst of block arrivals can chain back-to-back draws on
                    // this `select!` task; without a yield the `render_tick` arm
                    // never gets polled between them, so a heavy per-frame draw
                    // (e.g. a long streamed answer) reads as a freeze. Yielding
                    // here caps starvation to a single frame: the spinner keeps
                    // ticking and input stays responsive even mid-stream.
                    tokio::task::yield_now().await;
                }
            }

            Some(resolution) = remote_resolution_rx.recv() => {
                // The prompt block is enqueued before its remote event is
                // published. Drain it first so even a very fast phone answer
                // dismisses the matching modal rather than racing ahead of it.
                drain_into_app(app);
                apply_remote_permission_resolution(app, resolution);
                app.draw_frame(terminal)?;
            }

            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        // ↑/↓ move the permission cursor (consume + redraw); the
                        // accelerators and Enter resolve via key_to_permission_decision.
                        if app.active_prompt().is_some()
                            && key.kind == KeyEventKind::Press
                            && matches!(key.code, KeyCode::Up | KeyCode::Down)
                        {
                            app.move_permission_selection(matches!(key.code, KeyCode::Up));
                            app.draw_frame(terminal)?;
                        } else if let Some(decision) = key_to_permission_decision(&key, app) {
                            if let Some(prompt) = app.take_active_prompt() {
                                let _ = prompt.responder.send(decision);
                            }
                        } else {
                            match app.handle_key(key)? {
                                AppAction::Quit => {
                                    user_cancel_requested.store(true, Ordering::SeqCst);
                                    abort_signal.abort();
                                    cancelled = true;
                                    break;
                                }
                                AppAction::ClipboardPaste => {
                                    // Offload the blocking read to a blocking
                                    // thread and return to the `select!` at
                                    // once; the `clipboard_read` arm applies the
                                    // result when it lands, so the spinner and
                                    // streaming never stall. A second Ctrl+V
                                    // while one read is in flight is ignored.
                                    if clipboard_read.is_none() {
                                        clipboard_read = Some(tokio::task::spawn_blocking(
                                            read_clipboard_payload,
                                        ));
                                    }
                                }
                                AppAction::ClipboardCopy(target) => {
                                    if clipboard_write.is_none() {
                                        *clipboard_write = copy_runtime_session_to_clipboard(
                                            &copy_session_snapshot,
                                            target,
                                        );
                                    }
                                }
                                AppAction::ClipboardCopyBlock(text) => {
                                    if clipboard_write.is_none() {
                                        *clipboard_write = Some(PendingClipboardCopy::new(text));
                                    }
                                }
                                // The live workflow viewer is read-only and safe
                                // to open *while* a workflow streams — that is the
                                // whole point (watch the agents run). The render
                                // tick below keeps it refreshed.
                                AppAction::OpenWorkflowViewer => {
                                    crate::session::tui_loop::open_workflow_viewer(app);
                                }
                                AppAction::OpenAgentInViewer(agent_id) => {
                                    crate::session::tui_loop::open_workflow_viewer_focused(
                                        app, &agent_id,
                                    );
                                }
                                // RewindCheckpoint is intentionally ignored
                                // mid-turn: rewinding conversation + code
                                // while a turn is still streaming would be
                                // unsafe. It only acts between turns.
                                AppAction::Submit(_)
                                | AppAction::ConnectApiKey { .. }
                                | AppAction::ConnectCustomProvider(_)
                                | AppAction::ManageProvider(_)
                                | AppAction::SelectModel(_)
                                | AppAction::SelectPermission(_)
                                | AppAction::SelectSession(_)
                                | AppAction::ToggleTool { .. }
                                | AppAction::SaveSmartSettings(_)
                                | AppAction::DeepTier(_)
                                | AppAction::Editor
                                | AppAction::RewindCheckpoint
                                | AppAction::ConfirmRewind
                                | AppAction::OpenRewindViewer
                                | AppAction::RewindTo(_)
                                | AppAction::AckTeamInboxUpdate(_)
                                | AppAction::IncludeTeamInboxUpdate(_)
                                | AppAction::RefreshTeamInboxViewer
                                | AppAction::Redraw
                                | AppAction::TerminalResized
                                | AppAction::None => {}
                            }
                        }
                        // Coalesce keystroke echoes through the shared frame
                        // gate, mirroring the idle loop. Mid-turn every draw is
                        // at its heaviest (streaming transcript + HUD), and a
                        // terminal that paints slower than keys repeat backed
                        // the event queue up behind unconditional per-key full
                        // draws — the "typing hangs while streaming" input lag.
                        // A deferred echo lands on the next render tick.
                        if frame_gate.on_stream_update(std::time::Instant::now()).draws_now() {
                            let draw_started = std::time::Instant::now();
                            app.draw_frame(terminal)?;
                            frame_gate.note_draw_cost(draw_started.elapsed());
                            frame_gate.note_stream_draw(std::time::Instant::now());
                        }
                    }
                    Some(Ok(Event::Paste(text))) => {
                        app.handle_paste_owned(text);
                        // Same coalescing as keystrokes: the pasted text lands
                        // in state immediately, the repaint shares the frame
                        // budget.
                        if frame_gate.on_stream_update(std::time::Instant::now()).draws_now() {
                            let draw_started = std::time::Instant::now();
                            app.draw_frame(terminal)?;
                            frame_gate.note_draw_cost(draw_started.elapsed());
                            frame_gate.note_stream_draw(std::time::Instant::now());
                        }
                    }
                    Some(Ok(Event::Mouse(mouse))) => {
                        // Only redraw for scroll events; ignore mouse
                        // move/click to avoid event-flood freeze.
                        let is_scroll = matches!(
                            mouse.kind,
                            crossterm::event::MouseEventKind::ScrollUp
                                | crossterm::event::MouseEventKind::ScrollDown
                        );
                        let action = app.handle_mouse(mouse)?;
                        if matches!(action, AppAction::OpenWorkflowViewer) {
                            crate::session::tui_loop::open_workflow_viewer(app);
                            app.draw_frame(terminal)?;
                        } else if let AppAction::OpenAgentInViewer(agent_id) = &action {
                            crate::session::tui_loop::open_workflow_viewer_focused(app, agent_id);
                            app.draw_frame(terminal)?;
                        } else if let AppAction::ClipboardCopyBlock(text) = action {
                            if clipboard_write.is_none() {
                                *clipboard_write = Some(PendingClipboardCopy::new(text));
                            }
                            app.draw_frame(terminal)?;
                        } else if matches!(action, AppAction::Redraw) {
                            // Hover repaints ride the same frame budget as the
                            // wheel arm below: a pointer crossing rows flips the
                            // hover target once per row, so an ungated draw per
                            // flip stacks full mid-stream frames exactly like an
                            // inertial scroll does. `render_tick` lands the final
                            // hover state.
                            if frame_gate.on_stream_update(std::time::Instant::now()).draws_now() {
                                let draw_started = std::time::Instant::now();
                                app.draw_frame(terminal)?;
                                frame_gate.note_draw_cost(draw_started.elapsed());
                            }
                        } else if is_scroll {
                            // Coalesce wheel repaints. Mid-turn the loop is
                            // already streaming tokens (and may be compacting),
                            // so each draw is heavy; a macOS trackpad's inertial
                            // scroll fires dozens–hundreds of ScrollUp/Down
                            // events per flick. Drawing on every one backs the
                            // events up faster than they paint — the "wheel lags
                            // / UI tears on big context" freeze. The scroll
                            // offset is already accumulated, so repaint at most
                            // once per the shared stream frame interval; `render_tick`
                            // lands the final frame.
                            if frame_gate.on_stream_update(std::time::Instant::now()).draws_now() {
                                let draw_started = std::time::Instant::now();
                                app.draw_frame(terminal)?;
                                frame_gate.note_draw_cost(draw_started.elapsed());
                            }
                        }
                    }
                    Some(Ok(Event::Resize(..))) => {
                        // Re-fit the inline viewport first: its height is fixed
                        // at construction, so without this the live region keeps
                        // the launch height forever.
                        let rebuilt = crate::session::tui_loop::on_terminal_resize(
                            terminal,
                            app.terminal_mode(),
                        )?;
                        // Coalesce resize repaints through the same stream
                        // frame gate as wheel events: a drag-resize fires many
                        // intermediate widths, and each new width invalidates
                        // the whole transcript layout (full re-wrap +
                        // re-highlight), so drawing every event tears the UI
                        // mid-stream. `render_tick` lands the final geometry.
                        // A rebuilt viewport is the exception — its band is
                        // blank until something paints it.
                        if rebuilt
                            || frame_gate.on_stream_update(std::time::Instant::now()).draws_now()
                        {
                            let draw_started = std::time::Instant::now();
                            app.draw_frame(terminal)?;
                            frame_gate.note_draw_cost(draw_started.elapsed());
                        }
                    }
                    Some(Ok(_)) => {
                        // Focus changes, etc. — defer to render_tick
                        // to avoid event-flood freeze.
                    }
                    Some(Err(err)) => return Err(TuiLoopError::Io(err)),
                    None => break,
                }
            }

            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    cmd if !command_targets_turn(&cmd, turn_generation) => {}
                    AgentCommand::CancelTurn | AgentCommand::RemoteCancelTurn { .. } => {
                        user_cancel_requested.store(true, Ordering::SeqCst);
                        abort_signal.abort();
                        // Single Ctrl+C interrupts the in-flight turn. Set the
                        // shared abort flag so the spawned turn task's `select!`
                        // drops the in-flight HTTP stream and tool dispatch at
                        // once and hands the runtime back (the recovery below also
                        // sets it, as a backstop). Output already streamed stays
                        // visible; we surface an explicit interrupt marker so the
                        // turn doesn't end silently. The app is *not* quit (that
                        // needs a double Ctrl+C, which sets `should_quit` before
                        // sending `Quit`).
                        cancelled = true;
                        let stopped_agents = stop_visible_agents(app);
                        drain_into_app(app);
                        app.push_block(RenderBlock::System {
                            id: BlockIdGen::default().next(),
                            level: SystemLevel::Warn,
                            text: interrupt_message("turn cancelled (Ctrl+C)", stopped_agents),
                        });
                        app.draw_frame(terminal)?;
                        break;
                    }
                    AgentCommand::CancelTool => {
                        // Esc once, mid-turn, with a tool in flight. Bump the
                        // epoch and nothing else: no `user_cancel_requested`, no
                        // `abort_signal.abort()`, no `break`. The turn task keeps
                        // running and resumes from the synthetic tool result the
                        // runtime settles for each cancelled call, so the model
                        // sees the cancellation and picks another approach —
                        // instead of the user losing the turn and re-typing.
                        //
                        // No transcript note here: the cancelled tool card flips
                        // to `⊘` and its result body says so, which is feedback
                        // at the right altitude. A system line would double it.
                        tool_cancel_signal.cancel_running_tools();
                    }
                    AgentCommand::Quit => {
                        user_cancel_requested.store(true, Ordering::SeqCst);
                        abort_signal.abort();
                        cancelled = true;
                        stop_visible_agents(app);
                        break;
                    }
                    AgentCommand::Steer(text) | AgentCommand::RemoteSteer { text, .. } => {
                        // Queue the steer for the turn's next tool-result
                        // boundary — the MAIN model, and only the main model
                        // (CC parity). A poisoned lock just drops it
                        // (best-effort); the turn loop emits its own
                        // "⤷ steering" echo when it actually folds the
                        // message in, so no echo here.
                        //
                        // Deliberately NOT fanned out to running sub-agents:
                        // a mid-turn message is addressed to the main
                        // conversation, and the host broadcasting it to every
                        // worker made agents act on asides they were never
                        // meant to see (live report: a model-choice remark
                        // cancelled/steered a running fixture agent). Reaching
                        // a specific agent is an EXPLICIT act — the agents
                        // viewer's message box (Ctrl+G → m) or the model
                        // relaying via SendMessage — mirroring how CC only
                        // delivers to the main turn.
                        if let Ok(mut queue) = steering.lock() {
                            queue.push(text);
                        }
                    }
                }
            }

            Some(completion) = agent_rx.recv() => {
                if suppress_mismatched_background_task_completion(
                    &completion,
                    &active_session_id,
                ) {
                    continue;
                }
                // AGENT→MAIN: a still-RUNNING sub-agent pushed a message
                // upstream. It is not a completion, so it must skip the tree
                // flip, the failure dedup and the background re-injection —
                // branch first. Staged into the same inbox completions use, so
                // this turn folds it at its next tool boundary in the order it
                // arrived relative to any completion.
                if agent_completion_is_agent_message(&completion) {
                    if deliver_agent_message_mid_turn(app, &agent_notifications, &completion) {
                        app.draw_frame(terminal)?;
                    }
                    continue;
                }
                // Internal plumbing (decompose) never surfaces; the fan-out
                // controller owns its user-facing messaging. Skip before the
                // auth/rate-limit dedup so it cannot consume a "first failure"
                // slot that a real agent failure should claim.
                if agent_completion_is_internal(&completion) {
                    continue;
                }
                // W9-3: starvation notices render as a one-shot warning line
                // and must not reach the tree flip (the agent is still
                // running) or consume an auth/rate-limit failure dedup slot.
                if agent_completion_is_starvation_notice(&completion) {
                    let (level, text) = format_agent_completion(&completion);
                    app.push_block(RenderBlock::System {
                        id: BlockIdGen::default().next(),
                        level,
                        text,
                    });
                    app.draw_frame(terminal)?;
                    continue;
                }
                if agent_completion_is_auth_failure(&completion) {
                    if auth_failure_reported {
                        continue;
                    }
                    auth_failure_reported = true;
                } else if agent_completion_is_rate_limit_failure(&completion) {
                    if rate_limit_failure_reported {
                        continue;
                    }
                    rate_limit_failure_reported = true;
                }
                // Live `⎿ Done` flip on the transcript's agent tree, in true
                // completion order, while the batch is still collecting. An
                // absorbed `completed` event needs no extra system line — the
                // tree row *is* the notification (CC parity).
                let absorbed = app.note_agent_completion_display(
                    &completion.agent_id,
                    &completion.name,
                    &completion.status,
                    completion.output_tokens,
                );
                // A background agent that finished while THIS turn is live:
                // stage its result for MID-TURN delivery — the turn folds it
                // in at its next tool-result boundary, so the main model
                // receives it while still working (CC task-notification
                // parity) instead of only after ending its turn. If the turn
                // never reaches another boundary, the post-turn leftover
                // drain below re-queues it as a follow-up turn. The `⎿ Done`
                // row above is its visible notice, so skip the bare system
                // line.
                if deliver_background_agent_completion_mid_turn(
                    app,
                    &agent_notifications,
                    &completion,
                    &active_session_id,
                ) {
                    app.draw_frame(terminal)?;
                    continue;
                }
                if !(absorbed && completion.status == "completed") {
                    let (level, text) = format_agent_completion(&completion);
                    app.push_block(RenderBlock::System {
                        id: BlockIdGen::default().next(),
                        level,
                        text,
                    });
                }
                app.draw_frame(terminal)?;
            }

            // A clipboard read finished on its blocking thread. Apply it on the
            // main thread (cheap) and redraw. The guard keeps this arm inert
            // until a Ctrl+V actually spawned a read; the `pending()` fallback
            // means even if the guard were ever bypassed the arm would simply
            // never resolve rather than panic on an `unwrap`.
            join_result = async {
                match clipboard_read.as_mut() {
                    Some(handle) => handle.await,
                    None => std::future::pending().await,
                }
            }, if clipboard_read.is_some() => {
                clipboard_read = None;
                if let Ok(Some(payload)) = join_result {
                    apply_clipboard_payload(app, payload);
                    app.draw_frame(terminal)?;
                }
            }

            () = async {
                if let Some(write) = clipboard_write.as_mut() {
                    write.wait_until_ready().await;
                }
            }, if clipboard_write.is_some() => {
                // This arm waits only for the native helper result. The OSC 52
                // fallback and transcript report run after it wins, never in a
                // cancellable future.
                let failure = clipboard_write
                    .take()
                    .expect("clipboard write must still be present")
                    .finish();
                if let Some(text) = failure {
                    app.push_block(RenderBlock::System {
                        id: BlockIdGen::default().next(),
                        level: SystemLevel::Error,
                        text,
                    });
                }
                app.draw_frame(terminal)?;
            }

            _ = render_tick.tick() => {
                if profile_turn {
                    let gap = last_render_tick.elapsed().as_millis();
                    if gap > 150 {
                        eprintln!(
                            "[FREEZE-PROBE] render tick starved {gap}ms — a synchronous segment on the render task (e.g. a heavy app.draw) held the select! thread; the turn itself now runs off-thread"
                        );
                    }
                }
                app.advance_tick();
                let drained = app.drain_ready_blocks();
                let mut refresh_redraw = false;
                if let Some(snapshot) =
                    loop_arms::take_finished_snapshot(&mut live_hud_snapshot).await
                {
                    // A delegating main turn is not idle while its agents make
                    // progress. Mirror the auto-fanout loop and reset the stall
                    // clock from agent token flow (computed before the snapshot is
                    // moved), so the spinner reads "Delegating · \u{2191} N agent
                    // tokens" instead of flipping to a false "no output" badge
                    // after STALL_THRESHOLD_SECS.
                    let agent_tokens = agent_token_total(&snapshot.agents);
                    app.update_hud_live_snapshot(
                        snapshot.running,
                        snapshot.todos,
                        snapshot.agents,
                        snapshot.workflow,
                    );
                    if agent_tokens > 0 {
                        app.update_turn_tokens(0, agent_tokens);
                    }
                    refresh_redraw = true;
                }
                if let Some(Some(snapshot)) =
                    loop_arms::take_finished_snapshot(&mut changed_files_snapshot).await
                {
                    app.set_changed_files(snapshot.files, snapshot.total);
                    refresh_redraw = true;
                }
                if live_hud_snapshot.is_none()
                    && freshness.begin_scan(FreshnessDomain::Agents, Instant::now())
                {
                    live_hud_snapshot = Some(spawn_live_hud_snapshot(
                        app.agent_manifest_started_after(),
                        app.agent_manifest_session_id().map(str::to_string),
                    ));
                }
                if changed_files_snapshot.is_none()
                    && freshness.begin_scan(FreshnessDomain::Workspace, Instant::now())
                {
                    changed_files_snapshot = Some(spawn_changed_files_snapshot(
                        app.hud_cwd(),
                        freshness,
                    ));
                }
                if workflow_view_snapshot.as_ref().is_some_and(JoinHandle::is_finished) {
                    if let Some(handle) = workflow_view_snapshot.take() {
                        let view = handle.await.unwrap_or(None);
                        app.apply_workflow_viewer_snapshot(view);
                        refresh_redraw = true;
                    }
                }
                if app.workflow_viewer_refresh_due() && workflow_view_snapshot.is_none() {
                    if let Some((started_after, session_id)) = app.workflow_viewer_snapshot_scope() {
                        workflow_view_snapshot = Some(spawn_workflow_view_snapshot(
                            started_after,
                            session_id,
                        ));
                    }
                }
                if agents_rows_snapshot.as_ref().is_some_and(JoinHandle::is_finished) {
                    if let Some(handle) = agents_rows_snapshot.take() {
                        let snapshot = handle.await.unwrap_or_default();
                        app.apply_agents_viewer_snapshot(snapshot);
                        refresh_redraw = true;
                    }
                }
                if app.agents_viewer_refresh_due() && agents_rows_snapshot.is_none() {
                    if let Some((started_after, session_id, include_history)) =
                        app.agents_viewer_snapshot_scope()
                    {
                        agents_rows_snapshot = Some(spawn_agent_rows_snapshot(
                            started_after,
                            session_id,
                            include_history,
                        ));
                    }
                }
                // Keep the live workflow/agents viewers animating while
                // snapshots are read on the HUD runtime above.
                let workflow_redraw = app.tick_workflow_viewer() || app.tick_agents_viewer();
                let tick_stream_work = drained > 0 || app.turn_activity().is_some() || app.stream_pending();
                let tick_has_work = tick_stream_work || refresh_redraw || workflow_redraw;
                let tick_now = std::time::Instant::now();
                let decision = if tick_stream_work {
                    frame_gate.on_stream_tick(tick_now, tick_has_work)
                } else {
                    frame_gate.on_tick(tick_now, tick_has_work)
                };
                if decision.draws_now() {
                    let draw_started = std::time::Instant::now();
                    app.draw_frame(terminal)?;
                    frame_gate.note_draw_cost(draw_started.elapsed());
                    if app.turn_activity().is_some() || app.stream_pending() {
                        frame_gate.note_stream_draw(std::time::Instant::now());
                    }
                }
                if profile_turn {
                    last_render_tick = std::time::Instant::now();
                }
            }
        }
    }
        // Reached only via `break` (turn done / cancel / event-stream end). A
        // render error leaves earlier through `?`/`return Err`, straight to
        // `loop_result`.
        Ok(())
    }
    .await;

    // Single recovery point: reclaim `runtime` from the turn task on EVERY exit
    // and restore the caller's slot, so the session survives a cancel or a render
    // error exactly as it survives a clean completion.
    match turn_output {
        // Turn finished and handed the runtime back (normal or self-cancelled).
        Some(Ok((returned, result))) => {
            *runtime_slot = Some(returned);
            summary = Some(result);
        }
        // The turn task panicked: the runtime is gone. Leave the slot empty (the
        // caller's entry guard reports "runtime not available" on the next turn)
        // and surface the panic rather than masking it.
        Some(Err(join_error)) => {
            pump_handle.abort();
            return propagate_started_turn_error(
                app,
                TuiLoopError::Turn(format!("turn task panicked: {join_error}")),
            );
        }
        // The loop exited before the turn finished (Ctrl+C / Quit / event-stream
        // end / a render error): wind the task down — the abort flag makes its
        // inner `select!` drop the in-flight turn and return the runtime — then
        // restore the slot. Any render error stashed in `loop_result` is surfaced
        // only after the runtime is safely back.
        None => {
            if user_cancel_requested.load(Ordering::SeqCst) {
                abort_signal.abort();
            } else {
                abort_signal.abort_host();
            }
            match turn_handle.await {
                Ok((returned, _result)) => *runtime_slot = Some(returned),
                Err(join_error) => {
                    pump_handle.abort();
                    return propagate_started_turn_error(
                        app,
                        TuiLoopError::Turn(format!("turn task panicked: {join_error}")),
                    );
                }
            }
        }
    }
    if let Err(error) = loop_result {
        pump_handle.abort();
        return propagate_started_turn_error(app, error);
    }

    finish_turn(
        app,
        terminal,
        live_hud_snapshot,
        changed_files_snapshot,
        workflow_view_snapshot,
        &steering,
        &agent_notifications,
        pump_handle,
        summary,
        cancelled,
        freshness,
    )
    .await
}

fn propagate_started_turn_error(
    app: &mut App,
    error: TuiLoopError,
) -> Result<TurnOutcome, TuiLoopError> {
    app.enable_input();
    app.end_turn();
    Err(error)
}

#[allow(clippy::too_many_arguments)]
async fn finish_turn(
    app: &mut App,
    terminal: &mut TuiTerminal,
    live_hud_snapshot: Option<JoinHandle<LiveHudSnapshot>>,
    changed_files_snapshot: Option<JoinHandle<Option<GitStatusSnapshot>>>,
    workflow_view_snapshot: Option<JoinHandle<Option<WorkflowView>>>,
    steering: &runtime::SteeringQueue,
    agent_notifications: &runtime::AgentNotificationInbox,
    pump_handle: JoinHandle<()>,
    summary: Option<Result<TurnSummary, StreamingTurnError>>,
    cancelled: bool,
    freshness: &SessionFreshness,
) -> Result<TurnOutcome, TuiLoopError> {
    loop_arms::abort_snapshot(live_hud_snapshot);
    loop_arms::abort_snapshot(workflow_view_snapshot);
    // Steers the turn never reached a boundary to fold stay visible in the
    // message queue and auto-submit as their own turns next (CC parity).
    // Discard the runtime-side copies so the next turn's first boundary does
    // not deliver them a second time.
    if let Ok(mut pending_steers) = steering.lock() {
        pending_steers.clear();
    }
    // Background completions staged for mid-turn delivery that the turn never
    // reached a tool-result boundary to fold (or that arrived after the last
    // one): re-queue as follow-up turns so no result is ever lost. The
    // pop-time coalesce then folds a batch into one combined turn as before.
    let _ = requeue_undelivered_agent_notifications(app, agent_notifications);
    // Apply any final render frames that were already queued but not yet
    // consumed by the capped per-frame drain. This catches the common trailing
    // `TextDelta { done: true }` before `end_turn()` removes the spinner.
    let _ = app.drain_ready_blocks_to_idle();
    app.enable_input();
    app.end_turn();
    // Always paint the no-spinner frame immediately. Waiting for the final HUD
    // snapshot (or for the outer loop's next draw) leaves the old animated row
    // visible after the provider turn has already settled.
    app.draw_frame(terminal)?;
    let final_hud = refresh_live_hud_snapshot(app).await;
    let mut final_status = false;
    if let Some(handle) = changed_files_snapshot {
        if let Ok(Some(snapshot)) = handle.await {
            app.set_changed_files(snapshot.files, snapshot.total);
            final_status = true;
        }
    }
    if freshness.begin_scan(FreshnessDomain::Workspace, Instant::now()) {
        if let Ok(Some(snapshot)) = spawn_changed_files_snapshot(app.hud_cwd(), freshness).await {
            app.set_changed_files(snapshot.files, snapshot.total);
            final_status = true;
        }
    }
    if final_hud || final_status {
        app.draw_frame(terminal)?;
    }
    pump_handle.abort();
    let _ = pump_handle.await;

    match summary {
        Some(Ok(summary)) => Ok(TurnOutcome {
            summary: Some(summary),
        }),
        Some(Err(StreamingTurnError::Cancelled)) => Ok(TurnOutcome { summary: None }),
        Some(Err(err)) => Err(TuiLoopError::Turn(err.to_string())),
        None if cancelled => Ok(TurnOutcome { summary: None }),
        None => Ok(TurnOutcome { summary: None }),
    }
}

fn copy_runtime_session_to_clipboard(
    session: &runtime::Session,
    target: ClipboardCopyTarget,
) -> Option<PendingClipboardCopy> {
    let all = matches!(target, ClipboardCopyTarget::All);
    copy_payload(session, all).map(PendingClipboardCopy::new)
}

/// Open the live workflow viewer mid-turn (Ctrl+O). Reads the engine's progress
/// snapshot; when there is no *active* workflow it pushes a one-line notice — the
/// same feedback the idle path gives — because the render tick only *refreshes*
/// an already-open viewer, it cannot open one, so without this branch the key
/// would silently no-op.
pub(crate) fn drain_into_app(app: &mut App) {
    let _ = app.drain_ready_blocks_to_idle();
}

pub(crate) fn key_to_permission_decision(
    key: &KeyEvent,
    app: &App,
) -> Option<RenderPermissionDecision> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    let prompt = app.active_prompt()?;
    match key.code {
        KeyCode::Char('y' | 'Y') => Some(RenderPermissionDecision::AllowOnce),
        KeyCode::Char('a' | 'A') => Some(RenderPermissionDecision::AllowAlways),
        KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(RenderPermissionDecision::Deny),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(RenderPermissionDecision::Deny)
        }
        // Enter resolves the row the ↑↓ cursor is on (defaults to Deny).
        KeyCode::Enter => zo_cli::tui::blocks::permission::decision_for_selected(
            prompt,
            app.permission_selected(),
        ),
        _ => None,
    }
}

fn apply_remote_permission_resolution(app: &mut App, resolution: RemotePermissionResolution) {
    if !app.dismiss_permission_prompt(resolution.block_id) {
        return;
    }
    let label = match resolution.decision {
        RenderPermissionDecision::AllowOnce => "allow once",
        RenderPermissionDecision::AllowAlways => "always allow",
        RenderPermissionDecision::Deny => "deny",
        RenderPermissionDecision::DenyAlways => "always deny",
    };
    app.push_block(RenderBlock::System {
        id: next_synthetic_block_id(),
        level: SystemLevel::Info,
        text: format!("Permission prompt resolved remotely: {label}."),
    });
}

/// Push a one-line system status straight into the transcript and redraw, so the
/// auto-fan-out's progress shows immediately — its blocking work runs *before*
/// `drive_turn`, so the render channel that loop drains is not pumping yet.
fn fanout_note(app: &mut App, terminal: &mut TuiTerminal, text: String) {
    app.push_block(RenderBlock::System {
        id: BlockIdGen::default().next(),
        level: SystemLevel::Info,
        text,
    });
    let _ = app.draw_frame(terminal);
}

fn next_synthetic_block_id() -> BlockId {
    static NEXT_ID: AtomicU64 = AtomicU64::new(u64::MAX / 3);
    BlockId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

fn upsert_fanout_progress(
    app: &mut App,
    terminal: &mut TuiTerminal,
    id: BlockId,
    phase: &str,
    agents: &[AgentTaskSummary],
    running: u16,
) {
    upsert_fanout_progress_text(
        app,
        terminal,
        id,
        format_fanout_progress_text(phase, agents, running),
    );
}

fn upsert_fanout_progress_text(
    app: &mut App,
    terminal: &mut TuiTerminal,
    id: BlockId,
    text: String,
) {
    app.upsert_system_block(id, SystemLevel::Info, text);
    let _ = app.draw_frame(terminal);
}

fn format_fanout_progress_text(phase: &str, agents: &[AgentTaskSummary], running: u16) -> String {
    let terminal = agents
        .iter()
        .filter(|agent| matches!(agent.status.as_str(), "completed" | "failed" | "stopped"))
        .count();
    let failed = agents
        .iter()
        .filter(|agent| matches!(agent.status.as_str(), "failed"))
        .count();
    let stopped = agents
        .iter()
        .filter(|agent| matches!(agent.status.as_str(), "stopped"))
        .count();
    let total = agents.len();
    let active = total.saturating_sub(terminal);
    let percent = fanout_progress_percent(phase, terminal, total);
    let left = 100_u8.saturating_sub(percent);
    let token_total = agent_token_total(agents);
    let token_line = if token_total > 0 {
        format!(
            "~{} agent output tokens",
            zo_cli::tui::spinner::format_tokens(token_total)
        )
    } else {
        "agent output pending (waiting for usage)".to_string()
    };

    let mut lines = vec![
        format!(
            "{SMART_PRE_ANALYSIS_LABEL}: {phase} · {} · {percent}% complete · {left}% left",
            fanout_phase_label(phase)
        ),
        format!("- tokens: {token_line}"),
    ];
    if agents.is_empty() {
        lines.insert(1, "- agents: waiting for decomposition result".to_string());
        return lines.join("\n");
    }
    lines.insert(
        1,
        format!(
            "- agents: {terminal}/{total} terminal, {active} active ({running} running), {failed} failed, {stopped} stopped"
        ),
    );
    if active > 0 {
        lines.insert(
            2,
            format!(
                "- remaining: waiting for {active} agent {}",
                pluralize("result", active)
            ),
        );
    }
    if let Some(model_summary) = summarize_agent_models(agents) {
        lines.insert(
            2 + usize::from(active > 0),
            format!("- models: {model_summary}"),
        );
    }

    // Per-agent rows are now rendered by the Claude-Code-style agent tree under
    // the synthetic spawn ToolCall (see `maybe_apply_auto_fanout_live`); this
    // block stays a compact phase/percent/tokens/counts summary above it so the
    // two surfaces do not duplicate each other.
    lines.join("\n")
}

fn format_live_fanout_activity(
    agents: &[AgentTaskSummary],
    running: u16,
    spawned_total: usize,
) -> String {
    let running = usize::from(running);
    // `total` is the spawned fleet size, which is FIXED for the run — NOT the live
    // list length. `list_running_agents_since` drops a terminal agent after a grace
    // window, so `agents.len()` shrinks as agents finish; deriving the denominator
    // from it made the counter read 0/4 → 0/3 instead of 1/4 (the completed agent
    // left the list before it was ever counted as done). `spawned_total` comes from
    // the workflow manifest (`agent_ids`, fixed for the run); fall back to the live
    // length only when no manifest is available yet (the very first frames).
    let total = spawned_total.max(running).max(agents.len());
    let terminal = total.saturating_sub(running);
    let active = running;
    let percent = fanout_progress_percent("running", terminal, total);
    let left = 100_u8.saturating_sub(percent);

    if active == 0 {
        return format!(
            "{SMART_PRELUDE_LABEL}: {terminal}/{total} complete · {percent}% · {left}% left · finalizing agents"
        );
    }

    format!(
        "{SMART_PRELUDE_LABEL}: {terminal}/{total} complete · {percent}% · {left}% left · {active} pre-analysis {} active ({running} running)",
        pluralize("agent", active)
    )
}

fn format_fanout_launch_activity(roles: &[String]) -> String {
    let total = roles.len();
    format!(
        "{SMART_PRELUDE_LABEL}: 0/{total} complete · 0% · 100% left · {total} pre-analysis {} launching",
        pluralize("agent", total)
    )
}

fn format_fanout_launch_progress_text(roles: &[String]) -> String {
    let total = roles.len();
    let mut lines = vec![
        format!(
            "{SMART_PRE_ANALYSIS_LABEL}: launching · {} · 0% complete · 100% left",
            fanout_phase_label("launching")
        ),
        format!("- agents: 0/{total} terminal, {total} queued, 0 failed, 0 stopped"),
        "- tokens: agent output pending (waiting for launch)".to_string(),
    ];
    if let Some(preview) = summarize_role_names(roles) {
        lines.push(format!("- roles: {preview}"));
    }
    lines.join("\n")
}

fn fanout_phase_label(phase: &str) -> &'static str {
    match phase {
        "decomposing" => "routing",
        "launching" | "running" => "step 1/2",
        "completed" | "closed" => "step 2/2",
        _ => "step ?/2",
    }
}

fn fanout_progress_percent(phase: &str, terminal: usize, total: usize) -> u8 {
    if matches!(phase, "completed" | "closed") {
        return 100;
    }
    if total == 0 {
        return 0;
    }
    let percent = terminal.saturating_mul(100) / total;
    u8::try_from(percent.min(100)).unwrap_or(100)
}

fn close_fanout_collection_snapshot(snapshot: &mut LiveHudSnapshot) -> &'static str {
    let had_active = snapshot.running > 0
        || snapshot
            .agents
            .iter()
            .any(|agent| !agent_status_is_terminal(&agent.status));
    if !had_active {
        snapshot.running = 0;
        return "completed";
    }

    for agent in &mut snapshot.agents {
        if !agent_status_is_terminal(&agent.status) {
            agent.status = "stopped".to_string();
            agent.current_tool = None;
            agent.current_phase = Some("collection window closed".to_string());
        }
    }
    snapshot.running = 0;
    "closed"
}

fn agent_status_is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "stopped")
}

fn format_fanout_collection_closed_without_snapshot_text() -> String {
    format!(
        "{SMART_PRE_ANALYSIS_LABEL}: closed · {} · 100% complete · 0% left\n- agents: collection window closed; continuing with the main model\n- tokens: agent output pending (waiting for usage)",
        fanout_phase_label("closed")
    )
}

fn agent_token_total(agents: &[AgentTaskSummary]) -> u32 {
    agents
        .iter()
        .flat_map(|agent| agent.token_history.iter().copied())
        .fold(0u32, u32::saturating_add)
}

fn summarize_agent_models(agents: &[AgentTaskSummary]) -> Option<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for agent in agents {
        let model = zo_cli::tui::workflow_progress::short_model(agent.model.as_str());
        let model = model.trim();
        if !model.is_empty() {
            *counts.entry(model.to_string()).or_default() += 1;
        }
    }
    if counts.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    for (model, count) in counts.iter().take(4) {
        if *count == 1 {
            parts.push(model.clone());
        } else {
            parts.push(format!("{model} x{count}"));
        }
    }
    if counts.len() > 4 {
        parts.push(format!("+{} more models", counts.len() - 4));
    }
    Some(parts.join(", "))
}

const SMART_PRELUDE_LABEL: &str = "Smart";
const SMART_PRE_ANALYSIS_LABEL: &str = "Smart pre-analysis";
const SMART_SELF_CONSISTENCY_ACTIVITY: &str = "Smart: self-consistency vote completed";
const SMART_SELF_CONSISTENCY_NOTE: &str =
    "Smart self-consistency: independent answers reconciled by majority vote";
const SMART_TRIAGE_SELECTED_PREVIEW: &str =
    "Smart prelude · semantic triage selected agent pre-analysis";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AutoFanoutPreludeLabels {
    tool_label: &'static str,
    input_summary: &'static str,
    initial_activity: &'static str,
    initial_note: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SmartPreludeKind {
    Fanout,
    Triage,
}

impl SmartPreludeKind {
    const fn from_host_prespawn(host_prespawn: bool) -> Self {
        if host_prespawn { Self::Fanout } else { Self::Triage }
    }

    const fn opens_tool_call_immediately(self) -> bool {
        matches!(self, Self::Fanout)
    }

    const fn labels(self) -> AutoFanoutPreludeLabels {
        match self {
            Self::Fanout => AutoFanoutPreludeLabels {
                tool_label: "SpawnMultiAgent",
                input_summary: "Smart prelude · auto fan-out pre-analysis",
                initial_activity: "Smart: preparing parallel pre-analysis",
                initial_note: "Smart: preparing parallel pre-analysis...",
            },
            Self::Triage => AutoFanoutPreludeLabels {
                tool_label: "SemanticTriage",
                input_summary: "Smart prelude · choosing collaboration route",
                initial_activity: "Smart: choosing collaboration route",
                initial_note: "Smart: choosing collaboration route...",
            },
        }
    }
}

fn auto_fanout_opens_tool_call_immediately(host_prespawn: bool) -> bool {
    SmartPreludeKind::from_host_prespawn(host_prespawn).opens_tool_call_immediately()
}

fn auto_fanout_prelude_labels(host_prespawn: bool) -> AutoFanoutPreludeLabels {
    SmartPreludeKind::from_host_prespawn(host_prespawn).labels()
}

fn start_auto_fanout_tool_call(
    app: &mut App,
    progress_block_id: BlockId,
    tool_label: &'static str,
    input_summary: &'static str,
) -> String {
    let fanout_call_id = format!("auto-fanout-{}", progress_block_id.0);
    app.push_block(RenderBlock::ToolCall {
        id: next_synthetic_block_id(),
        tool_call_id: ToolCallId(fanout_call_id.clone()),
        name: tool_label.to_string(),
        summary: String::new(),
        preview: ToolPreview::Generic {
            name: tool_label.to_string(),
            input_summary: input_summary.to_string(),
        },
        status: ToolCallStatus::Running,
    });
    app.begin_agent_batch_with_label(&fanout_call_id, Some(SMART_PRELUDE_LABEL));
    fanout_call_id
}

struct AutoFanoutLiveChannels<'a> {
    events: &'a mut EventStream,
    commands: &'a mut mpsc::Receiver<AgentCommand>,
    agent_completions: &'a mut mpsc::UnboundedReceiver<AgentCompletion>,
    turn_generation: u64,
}

/// Stage 4 of automatic multi-agent fan-out. Returns `user_input` augmented with
/// a parallel pre-analysis block when the heuristic gate fires. The important
/// UX invariant: this runs after `drive_turn` has opened the TUI event stream,
/// so the user can still type, scroll, and cancel while decomposition/agents run.
#[allow(clippy::too_many_lines)] // single decompose→spawn→collect live sequence; stages share TUI state
async fn maybe_apply_auto_fanout_live(
    app: &mut App,
    terminal: &mut TuiTerminal,
    channels: AutoFanoutLiveChannels<'_>,
    user_input: String,
    plan: AutoFanoutPlan,
    freshness: &SessionFreshness,
    clipboard_write: &mut Option<PendingClipboardCopy>,
) -> Result<AutoFanoutPrelude, TuiLoopError> {
    let AutoFanoutLiveChannels {
        events,
        commands: cmd_rx,
        agent_completions: agent_rx,
        turn_generation,
    } = channels;
    zo_cli::tui::watchdog::set_phase(
        zo_cli::tui::watchdog::Phase::FanoutPrelude,
    );
    let progress_block_id = next_synthetic_block_id();
    // Host pre-spawn renders the same Claude-Code-style agent tree the
    // model-invoked `SpawnMultiAgent` path does. Semantic-triage-only preludes
    // use a neutral status note and defer the synthetic ToolCall/agent tree
    // until triage actually chooses an agent branch. Once opened, that row owns
    // the tree and the existing batch machinery
    // (`refresh_agent_batch` via the live HUD snapshot + `note_agent_completion_display`)
    // fills it with per-agent rows and `⎿ Done` flips. The `upsert_fanout_progress`
    // text below stays as a compact tokens/percent summary above the tree.
    let prelude_labels = auto_fanout_prelude_labels(plan.host_prespawn);
    let mut fanout_call_id = if auto_fanout_opens_tool_call_immediately(plan.host_prespawn) {
        Some(start_auto_fanout_tool_call(
            app,
            progress_block_id,
            prelude_labels.tool_label,
            prelude_labels.input_summary,
        ))
    } else {
        None
    };
    app.set_turn_activity(prelude_labels.initial_activity);
    fanout_note(app, terminal, prelude_labels.initial_note.to_string());

    let cancelled = Arc::new(AtomicBool::new(false));
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    // Straggler-reap boundary for THIS fan-out: only agents created from here
    // on belong to the collection window. The reap below once used the
    // session-start boundary (`app.agent_manifest_started_after()`), which
    // swept every still-running agent in the session — including a detached
    // background agent the model spawned on an EARLIER turn and was expecting
    // a mid-turn completion notification from (live report: a 16-minute E2E
    // agent force-stopped by the next turn's triage window closing).
    let fanout_started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let mut task = spawn_auto_fanout_task(
        user_input.clone(),
        plan,
        Arc::clone(&cancelled),
        progress_tx,
    );

    let mut prelude_steers = Vec::new();
    let mut live_hud_snapshot: Option<JoinHandle<LiveHudSnapshot>> = None;
    let mut changed_files_snapshot: Option<JoinHandle<Option<GitStatusSnapshot>>> = None;
    let mut workflow_view_snapshot: Option<JoinHandle<Option<WorkflowView>>> = None;
    let mut agents_rows_snapshot: Option<JoinHandle<AgentRowsSnapshot>> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await; // consume the immediate first tick
    // Wheel-repaint coalescing state, mirroring the main event loops: start in
    // the past so the first scroll paints immediately.
    let mut wheel_frame_gate = StreamFrameGate::new_ready(
        std::time::Instant::now(),
        ANIMATION_TICK_INTERVAL,
    );
    let fanout = loop {
        zo_cli::tui::watchdog::beat();
        tokio::select! {
            joined = &mut task => {
                break joined.unwrap_or(AutoFanoutResult::DecomposeFailed);
            }
            maybe_event = events.next() => {
                if handle_auto_fanout_event(
                    app,
                    terminal,
                    maybe_event,
                    &cancelled,
                    &task,
                    &mut wheel_frame_gate,
                    clipboard_write,
                )? {
                    return Ok(AutoFanoutPrelude::Cancelled);
                }
            }
            Some(cmd) = cmd_rx.recv() => {
                if handle_auto_fanout_command(
                    app,
                    terminal,
                    cmd,
                    turn_generation,
                    &mut prelude_steers,
                    &cancelled,
                    &task,
                )? {
                    return Ok(AutoFanoutPrelude::Cancelled);
                }
            }
            Some(completion) = agent_rx.recv() => {
                handle_auto_fanout_agent_completion(app, terminal, &completion)?;
            }
            Some(progress) = progress_rx.recv() => {
                handle_auto_fanout_progress(
                    app,
                    terminal,
                    progress_block_id,
                    &mut fanout_call_id,
                    progress,
                );
            }
            () = async {
                if let Some(write) = clipboard_write.as_mut() {
                    write.wait_until_ready().await;
                }
            }, if clipboard_write.is_some() => {
                let failure = clipboard_write
                    .take()
                    .expect("clipboard write must still be present")
                    .finish();
                if let Some(text) = failure {
                    app.push_block(RenderBlock::System {
                        id: BlockIdGen::default().next(),
                        level: SystemLevel::Error,
                        text,
                    });
                }
                app.draw_frame(terminal)?;
            }
            _ = tick.tick() => {
                app.advance_tick();
                let mut refresh_redraw = false;
                if let Some(snapshot) =
                    loop_arms::take_finished_snapshot(&mut live_hud_snapshot).await
                {
                    if snapshot.running > 0 {
                        let spawned_total =
                            snapshot.workflow.as_ref().map_or(0, |w| w.total_agents);
                        app.set_turn_activity(format_live_fanout_activity(
                            &snapshot.agents,
                            snapshot.running,
                            spawned_total,
                        ));
                    }
                    let agent_tokens = agent_token_total(&snapshot.agents);
                    if agent_tokens > 0 {
                        app.update_turn_tokens(0, agent_tokens);
                    }
                    upsert_fanout_progress(
                        app,
                        terminal,
                        progress_block_id,
                        "running",
                        &snapshot.agents,
                        snapshot.running,
                    );
                    app.update_hud_live_snapshot(
                        snapshot.running,
                        snapshot.todos,
                        snapshot.agents,
                        snapshot.workflow,
                    );
                    refresh_redraw = true;
                }
                if let Some(Some(snapshot)) =
                    loop_arms::take_finished_snapshot(&mut changed_files_snapshot).await
                {
                    app.set_changed_files(snapshot.files, snapshot.total);
                    refresh_redraw = true;
                }
                if live_hud_snapshot.is_none()
                    && freshness.begin_scan(FreshnessDomain::Agents, Instant::now())
                {
                    live_hud_snapshot = Some(spawn_live_hud_snapshot(
                        app.agent_manifest_started_after(),
                        app.agent_manifest_session_id().map(str::to_string),
                    ));
                }
                if changed_files_snapshot.is_none()
                    && freshness.begin_scan(FreshnessDomain::Workspace, Instant::now())
                {
                    changed_files_snapshot = Some(spawn_changed_files_snapshot(
                        app.hud_cwd(),
                        freshness,
                    ));
                }
                if workflow_view_snapshot.as_ref().is_some_and(JoinHandle::is_finished) {
                    if let Some(handle) = workflow_view_snapshot.take() {
                        let view = handle.await.unwrap_or(None);
                        app.apply_workflow_viewer_snapshot(view);
                        refresh_redraw = true;
                    }
                }
                if app.workflow_viewer_refresh_due() && workflow_view_snapshot.is_none() {
                    if let Some((started_after, session_id)) = app.workflow_viewer_snapshot_scope() {
                        workflow_view_snapshot = Some(spawn_workflow_view_snapshot(
                            started_after,
                            session_id,
                        ));
                    }
                }
                if agents_rows_snapshot.as_ref().is_some_and(JoinHandle::is_finished) {
                    if let Some(handle) = agents_rows_snapshot.take() {
                        let snapshot = handle.await.unwrap_or_default();
                        app.apply_agents_viewer_snapshot(snapshot);
                        refresh_redraw = true;
                    }
                }
                if app.agents_viewer_refresh_due() && agents_rows_snapshot.is_none() {
                    if let Some((started_after, session_id, include_history)) =
                        app.agents_viewer_snapshot_scope()
                    {
                        agents_rows_snapshot = Some(spawn_agent_rows_snapshot(
                            started_after,
                            session_id,
                            include_history,
                        ));
                    }
                }
                let tick_stream_work = app.turn_activity().is_some() || app.stream_pending();
                let tick_has_work = refresh_redraw
                    || tick_stream_work
                    || app.tick_workflow_viewer()
                    || app.tick_agents_viewer();
                let tick_now = std::time::Instant::now();
                let decision = if tick_stream_work {
                    wheel_frame_gate.on_stream_tick(tick_now, tick_has_work)
                } else {
                    wheel_frame_gate.on_tick(tick_now, tick_has_work)
                };
                if decision.draws_now() {
                    app.draw_frame(terminal)?;
                    if tick_stream_work {
                        wheel_frame_gate.note_stream_draw(std::time::Instant::now());
                    }
                }
            }
        }
    };
    loop_arms::abort_snapshot(live_hud_snapshot);
    if changed_files_snapshot.is_some() {
        loop_arms::abort_snapshot(changed_files_snapshot);
        freshness.mark_dirty(FreshnessDomain::Workspace);
    }
    loop_arms::abort_snapshot(workflow_view_snapshot);
    // The join branch can win the select race while a just-sent `LaunchingAgents`
    // progress message is still queued. Drain pending progress before deciding
    // whether a lazy semantic-triage ToolCall/agent batch exists to snapshot and
    // finish.
    while let Ok(progress) = progress_rx.try_recv() {
        handle_auto_fanout_progress(
            app,
            terminal,
            progress_block_id,
            &mut fanout_call_id,
            progress,
        );
    }
    // The join branch can also win while per-agent completion notices are
    // queued. Drain them before loading the final HUD snapshot so fast
    // auto-fanout agents seed visible rows even when the manifest file-set cache
    // has not expired yet.
    while let Ok(completion) = agent_rx.try_recv() {
        handle_auto_fanout_agent_completion(app, terminal, &completion)?;
    }
    // Collection window closed — proactively reap any straggler so no agent
    // thread outlives the fan-out (BUG-R3). The per-agent deadline set on spawn
    // is the self-bound; this is the immediate stop. The final transcript block
    // must also move out of `running` before the main model takes over; if a
    // manifest still reads non-terminal (for example a racy or unstamped row),
    // close it locally for this visible snapshot instead of leaving stale
    // `running · waiting for agent result` text above `continuing with the main
    // model`. Scoped to agents created by THIS fan-out (`fanout_started_at`):
    // detached background agents from earlier turns are deliberately out of
    // reach — they outlive turns by design and report via the completion inbox.
    let _ = stop_running_agents_since_for_session(
        fanout_started_at,
        app.agent_manifest_session_id(),
        "Smart collection window closed",
    );
    if fanout_call_id.is_some() {
        if let Some(mut snapshot) = load_live_hud_snapshot(app).await {
            let phase = close_fanout_collection_snapshot(&mut snapshot);
            let agent_tokens = agent_token_total(&snapshot.agents);
            if agent_tokens > 0 {
                app.update_turn_tokens(0, agent_tokens);
            }
            upsert_fanout_progress(
                app,
                terminal,
                progress_block_id,
                phase,
                &snapshot.agents,
                snapshot.running,
            );
            apply_live_hud_snapshot(app, snapshot);
            app.draw_frame(terminal)?;
        } else {
            upsert_fanout_progress_text(
                app,
                terminal,
                progress_block_id,
                format_fanout_collection_closed_without_snapshot_text(),
            );
        }

        // Seal the agent tree: the synthetic spawn row flips to the
        // `N agents finished (ctrl+g for details)` header, matching the
        // model-invoked SpawnMultiAgent path once its collection window closes.
        if let Some(fanout_call_id) = &fanout_call_id {
            app.finish_agent_batch(fanout_call_id);
            app.draw_frame(terminal)?;
        }
    }

    let user_input = fold_prelude_steers(user_input, &prelude_steers);
    finalize_fanout_prelude(app, terminal, fanout, user_input, fanout_call_id.is_some())
}

/// Value-mapping tail of `maybe_apply_auto_fanout_live`: turns a resolved
/// `AutoFanoutResult` (and the empty-analysis case) into the `AutoFanoutPrelude`
/// the caller returns, emitting the matching turn-activity/fan-out notes. Never
/// errors today; the `Result` mirrors the caller's signature. `has_fanout_call`
/// is the caller's `fanout_call_id.is_some()`.
#[allow(clippy::unnecessary_wraps)] // Result mirrors the caller's signature
fn finalize_fanout_prelude(
    app: &mut App,
    terminal: &mut TuiTerminal,
    fanout: AutoFanoutResult,
    user_input: String,
    has_fanout_call: bool,
) -> Result<AutoFanoutPrelude, TuiLoopError> {
    let (roles, summary) = match fanout {
        AutoFanoutResult::Completed { roles, summary } => (roles, summary),
        AutoFanoutResult::DecomposeFailed => {
            if has_fanout_call {
                app.set_turn_activity("Smart: continuing with the main model");
                fanout_note(
                    app,
                    terminal,
                    "Smart: could not decompose work; continuing with the main model".to_string(),
                );
            } else {
                app.set_turn_activity("Smart: continuing with the main model");
                fanout_note(
                    app,
                    terminal,
                    "Smart: continuing with the main model".to_string(),
                );
            }
            return Ok(AutoFanoutPrelude::Ready(user_input));
        }
        AutoFanoutResult::SpawnFailed { roles, error } => {
            app.set_turn_activity("Smart: continuing after parallel-agent failure");
            fanout_note(
                app,
                terminal,
                format!(
                    "Smart: failed to run parallel agents ({} subtasks); continuing with the main model: {}",
                    roles.len(),
                    concise_error(&error)
                ),
            );
            return Ok(AutoFanoutPrelude::Ready(user_input));
        }
        AutoFanoutResult::SelfConsistent { answer } => {
            // P2 self-consistency: the answer is already reconciled by the
            // Council. Deliver it as evidence (microcompact-clearable, consume-
            // not-rederive) just like a decompose fan-out, with no further split.
            app.set_turn_activity(SMART_SELF_CONSISTENCY_ACTIVITY);
            fanout_note(app, terminal, SMART_SELF_CONSISTENCY_NOTE.to_string());
            return Ok(AutoFanoutPrelude::ReadyWithEvidence {
                user_input,
                evidence: build_self_consistency_evidence(&answer),
            });
        }
    };
    let Some(analysis) = format_fanout_analysis(&summary, &roles) else {
        app.set_turn_activity("Smart: continuing after empty pre-analysis");
        fanout_note(
            app,
            terminal,
            "Smart: no usable agent results; continuing with the main model".to_string(),
        );
        return Ok(AutoFanoutPrelude::Ready(user_input));
    };
    app.set_turn_activity(format!(
        "Smart: {} pre-analysis {} completed",
        roles.len(),
        pluralize("agent", roles.len())
    ));
    fanout_note(
        app,
        terminal,
        format!(
            "Smart pre-analysis: {} agents completed; synthesizing with the main model",
            roles.len()
        ),
    );
    Ok(AutoFanoutPrelude::ReadyWithEvidence {
        user_input,
        evidence: build_fanout_evidence(&analysis, roles.len()),
    })
}

/// Build the microcompact-clearable evidence body for a completed auto fan-out.
///
/// Unlike the old user-message prepend, this string does NOT carry the user
/// input and instructs the model to CONSUME the parallel pre-analysis as an
/// evidence base rather than re-derive it ("verify and synthesize"). It is
/// delivered as a synthetic `SpawnMultiAgent` tool-result whose body
/// microcompact can clear once it ages past the recent tail.
fn build_fanout_evidence(analysis: &str, n: usize) -> String {
    format!(
        "[Smart pre-analysis]\nZo split this request into {n} independent subtasks and \
         ran them through parallel sub-agents. Use this completed parallel pre-analysis as an \
         evidence base; do NOT re-run or re-derive it — build on it.\n\n{analysis}"
    )
}

/// Build the evidence body for a completed self-consistency vote (P2). The
/// `answer` is already reconciled by the Council, so the model is told to build
/// on it rather than re-run the analysis — same consume-not-rederive contract
/// and microcompact-clearable delivery as [`build_fanout_evidence`].
fn build_self_consistency_evidence(answer: &str) -> String {
    format!(
        "[Smart self-consistency]\nZo answered this request with several independent \
         sub-agents and reconciled them by majority vote. Use this reconciled result as an \
         evidence base; do NOT re-run or re-derive it — build on it.\n\n{answer}"
    )
}

fn fold_prelude_steers(mut user_input: String, prelude_steers: &[String]) -> String {
    if prelude_steers.is_empty() {
        return user_input;
    }
    user_input.push_str("\n\n---\n[Input received during pre-analysis]\n");
    for steer in prelude_steers {
        user_input.push_str("- ");
        user_input.push_str(steer.trim());
        user_input.push('\n');
    }
    user_input
}

fn spawn_auto_fanout_task(
    user_input: String,
    plan: AutoFanoutPlan,
    cancelled: Arc<AtomicBool>,
    progress_tx: mpsc::UnboundedSender<AutoFanoutProgress>,
) -> AutoFanoutTask {
    // Decompose + run off the async runtime. A cancellation flag prevents a
    // late decompose result from spawning agents after the user interrupts.
    tokio::task::spawn_blocking(move || {
        // P2 — intent triage BEFORE decomposing. A cheap 1-shot agent clarifies
        // the user's actual intent (vague asks no longer get split blindly) and
        // picks the fan-out mode. On any failure/low confidence it returns None
        // and we fall back to the raw input + the existing decompose path, so
        // this never breaks or blocks the turn beyond its short budget.
        let triage = if cancelled.load(Ordering::Relaxed) {
            None
        } else {
            clarify_intent(
                &user_input,
                plan.breadth,
                plan.parent_model.as_deref(),
                Some(&plan.hook_config),
                Some(&plan.session_id),
            )
        };
        // The text fed to decompose/self-consistency: the clarified intent when
        // triage is confident, else the raw user input (conservative fallback).
        let effective_intent = triage
            .as_ref()
            .filter(|t| t.confidence >= 0.4 && !t.intent.trim().is_empty())
            .map_or_else(|| user_input.clone(), |t| t.intent.clone());
        let mode = trusted_triage_mode(triage.as_ref());

        // Centralised, side-effect-free routing decision (unit-tested exhaustively
        // in `fanout_branch`): which host action this triage verdict selects given
        // whether the turn is a breadth fan-out. The invariant a non-breadth turn
        // must get right — only `diagnose` engages the host; `self_consistency`,
        // `decompose`, `solo`, and an absent triage all defer to the model-led
        // turn — lives in that one pure fn instead of scattered branch ordering.
        match fanout_branch(mode, plan.breadth) {
            // Solo, or a non-breadth non-diagnose verdict: no fan-out — defer to
            // the single model-led turn (this is the cost-saver: no agents spawned,
            // and a non-breadth pipeline keeps its plan→implement→verify flow).
            FanoutBranch::Fallback => AutoFanoutResult::DecomposeFailed,

            // Diagnose: an adversarial root-cause fan-out — one independent finder
            // per diagnostic lens, each refuting its own hypothesis (simulate /
            // reproduce) before reporting. The combined findings become the model's
            // pre-analysis, so it cross-checks competing hypotheses instead of
            // committing to one unverified guess — the method that cracks bugs a
            // single linear turn keeps surface-fixing. The triage LLM picks this for
            // hard/recurring bugs from meaning; the parent model is inherited, so it
            // runs on any provider. Reachable on breadth AND non-breadth turns.
            FanoutBranch::Diagnose => {
                // The triage round took several seconds; bail before spawning if the
                // user interrupted meanwhile (mirrors the decompose-path check).
                if cancelled.load(Ordering::Relaxed) {
                    return AutoFanoutResult::DecomposeFailed;
                }
                let roles = diagnose_lens_labels();
                let _ = progress_tx.send(AutoFanoutProgress::LaunchingAgents {
                    roles: roles.clone(),
                });
                // Finders get the VERBATIM bug report, not the one-sentence
                // clarified intent: reproducing a hard bug needs the stack trace,
                // error text, file names, and repro steps the compression strips.
                // The clarified intent only steers decompose/self-consistency
                // framing, not diagnosis.
                match run_diagnose_fanout(
                    &user_input,
                    plan.parent_model.as_deref(),
                    Some(&plan.hook_config),
                    Some(&plan.session_id),
                ) {
                    Some(summary) => AutoFanoutResult::Completed { roles, summary },
                    // Diagnosis spawn failed: fall back to a single turn rather than
                    // the breadth decompose (this was a bug, not splittable work).
                    None => AutoFanoutResult::DecomposeFailed,
                }
            }

            // Self-consistency (breadth only): answer the SAME clarified question
            // with k independent agents and reconcile via the Council (the CC-style
            // majority vote). On failure, fall through to the decompose path so the
            // turn still benefits from parallel pre-analysis when possible.
            FanoutBranch::SelfConsistency => {
                if cancelled.load(Ordering::Relaxed) {
                    return AutoFanoutResult::DecomposeFailed;
                }
                let _ = progress_tx.send(AutoFanoutProgress::LaunchingAgents {
                    roles: (0..SELF_CONSISTENCY_K)
                        .map(|i| format!("candidate {i}"))
                        .collect(),
                });
                if let Some(answer) = run_self_consistency_fanout(
                    &effective_intent,
                    plan.parent_model.as_deref(),
                    SELF_CONSISTENCY_K,
                    Some(&plan.hook_config),
                    Some(&plan.session_id),
                ) {
                    return AutoFanoutResult::SelfConsistent { answer };
                }
                run_decompose_fanout(&effective_intent, &plan, &cancelled, &progress_tx)
            }

            // Breadth decompose-and-spawn.
            FanoutBranch::Decompose => {
                run_decompose_fanout(&effective_intent, &plan, &cancelled, &progress_tx)
            }
        }
    })
}

/// Which host action a triage verdict selects, given whether the turn is a
/// breadth fan-out. Pure (no side effects) so the branch ordering — the part a
/// non-breadth turn must get right — is exhaustively unit-testable without a
/// live triage agent: a non-breadth turn engages the host ONLY for `Diagnose`;
/// `SelfConsistency`/`Decompose`/`Solo`/no-triage all fall back to the model-led
/// turn. A breadth turn additionally runs self-consistency and decompose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FanoutBranch {
    /// Defer to the single model-led turn (no host spawn).
    Fallback,
    /// Adversarial root-cause diagnose fan-out (breadth or non-breadth).
    Diagnose,
    /// k-way self-consistency council (breadth only).
    SelfConsistency,
    /// Breadth decompose-and-spawn (breadth only).
    Decompose,
}

fn trusted_triage_mode(triage: Option<&IntentTriage>) -> Option<FanoutMode> {
    triage
        .filter(|triage| triage.confidence >= 0.4)
        .map(|triage| triage.mode)
}

fn fanout_branch(mode: Option<FanoutMode>, breadth: bool) -> FanoutBranch {
    // Diagnose always fans out; SelfConsistency/Decompose only under breadth, and
    // an unclassified breadth turn defaults to Decompose. Everything else — Solo,
    // and any non-breadth mode — falls through to Fallback (no fan-out).
    match mode {
        Some(FanoutMode::Diagnose) => FanoutBranch::Diagnose,
        Some(FanoutMode::SelfConsistency) if breadth => FanoutBranch::SelfConsistency,
        Some(FanoutMode::Decompose) | None if breadth => FanoutBranch::Decompose,
        _ => FanoutBranch::Fallback,
    }
}

/// Breadth decompose-and-spawn: split the clarified intent into independent
/// subtasks and run them as a pre-analysis fan-out. Shared by the `Decompose`
/// branch and the self-consistency fall-through. Returns `DecomposeFailed` when
/// the split yields fewer than two subtasks or the user cancels.
fn run_decompose_fanout(
    effective_intent: &str,
    plan: &AutoFanoutPlan,
    cancelled: &AtomicBool,
    progress_tx: &mpsc::UnboundedSender<AutoFanoutProgress>,
) -> AutoFanoutResult {
    let fanout_input = fanout_decomposition_input(effective_intent, plan.session_goal.as_deref());
    // Decompose on the active provider's model too — not just the spawned
    // sub-agents — so a non-Anthropic session never dials a Claude id out.
    let subtasks = decompose_for_fanout_with_timeout_and_hooks(
        &fanout_input,
        plan.parent_model.as_deref(),
        AUTO_FANOUT_DECOMPOSE_TIMEOUT,
        Some(&plan.hook_config),
        Some(&plan.session_id),
        plan.complexity,
    );
    if cancelled.load(Ordering::Relaxed) || subtasks.len() < 2 {
        return AutoFanoutResult::DecomposeFailed;
    }
    let roles: Vec<String> = subtasks
        .iter()
        .map(|subtask| subtask.role.clone())
        .collect();
    let _ = progress_tx.send(AutoFanoutProgress::LaunchingAgents {
        roles: roles.clone(),
    });
    let summary = match run_fanout_spawn_with_timeout_and_hooks(
        &subtasks,
        plan.parent_model.as_deref(),
        AUTO_FANOUT_AGENT_TIMEOUT,
        // Give each pre-analysis agent a hard wall-clock deadline equal to
        // the collection window so it self-terminates instead of becoming
        // an orphan thread once the foreground stops collecting (BUG-R3).
        Some(AUTO_FANOUT_AGENT_TIMEOUT),
        Some(&plan.hook_config),
        Some(&plan.session_id),
    ) {
        Ok(summary) => summary,
        Err(error) => {
            return AutoFanoutResult::SpawnFailed {
                roles,
                error: error.to_string(),
            };
        }
    };
    AutoFanoutResult::Completed { roles, summary }
}

fn handle_auto_fanout_progress(
    app: &mut App,
    terminal: &mut TuiTerminal,
    progress_block_id: BlockId,
    fanout_call_id: &mut Option<String>,
    progress: AutoFanoutProgress,
) {
    match progress {
        AutoFanoutProgress::LaunchingAgents { roles } => {
            if fanout_call_id.is_none() {
                *fanout_call_id = Some(start_auto_fanout_tool_call(
                    app,
                    progress_block_id,
                    "SpawnMultiAgent",
                    SMART_TRIAGE_SELECTED_PREVIEW,
                ));
            }
            let role_label = summarize_roles(&roles);
            app.set_turn_activity(format_fanout_launch_activity(&roles));
            fanout_note(
                app,
                terminal,
                format!(
                    "Smart: launching {} pre-analysis {}{}",
                    roles.len(),
                    pluralize("agent", roles.len()),
                    role_label
                ),
            );
            upsert_fanout_progress_text(
                app,
                terminal,
                progress_block_id,
                format_fanout_launch_progress_text(&roles),
            );
        }
    }
}

fn handle_auto_fanout_agent_completion(
    app: &mut App,
    terminal: &mut TuiTerminal,
    completion: &AgentCompletion,
) -> Result<(), TuiLoopError> {
    // AGENT→MAIN: a running agent's mid-run message. The main turn has not
    // started yet during the fan-out prelude, so there is no inbox to fold it
    // into — queue it as a follow-up turn (same framing) instead of letting the
    // completion-shaped handling below mistake it for a finished agent.
    if agent_completion_is_agent_message(completion) {
        queue_agent_message(app, completion);
        return Ok(());
    }
    // W9-3: starvation notices are live warnings — render directly, skipping
    // the activity label and the completion tree flip.
    if agent_completion_is_starvation_notice(completion) {
        let (level, text) = format_agent_completion(completion);
        app.push_block(RenderBlock::System {
            id: BlockIdGen::default().next(),
            level,
            text,
        });
        app.draw_frame(terminal)?;
        return Ok(());
    }

    app.set_turn_activity(auto_fanout_activity_for_completion(completion));
    // The decompose step's transient activity is useful, but its persistent
    // completion block is internal noise — and the benign collection-window reap
    // would otherwise render a scary "stopped" warning. Spawned pre-analysis
    // agents still surface.
    if !agent_completion_is_internal(completion) {
        push_live_agent_completion(app, terminal, completion)?;
    }
    Ok(())
}

fn auto_fanout_activity_for_completion(completion: &AgentCompletion) -> &'static str {
    if completion.name == "decompose" {
        if completion.status == "completed" {
            "Smart: preparing parallel pre-analysis agents"
        } else {
            "Smart: decomposition failed; continuing"
        }
    } else if completion.status == "completed" {
        "Smart: collecting pre-analysis results"
    } else {
        "Smart: collecting agent error details"
    }
}

fn summarize_roles(roles: &[String]) -> String {
    summarize_role_names(roles).map_or_else(String::new, |preview| format!(" ({preview})"))
}

fn summarize_role_names(roles: &[String]) -> Option<String> {
    if roles.is_empty() {
        return None;
    }
    let preview = roles
        .iter()
        .take(3)
        .map(|role| role.trim())
        .filter(|role| !role.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if preview.is_empty() {
        return None;
    }
    let suffix = roles.len().saturating_sub(3);
    if suffix == 0 {
        Some(preview)
    } else {
        Some(format!("{preview}, +{suffix} more"))
    }
}

fn pluralize(word: &str, count: usize) -> String {
    if count == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

fn concise_error(error: &str) -> String {
    const MAX_CHARS: usize = 160;
    let trimmed = error.trim();
    match trimmed.char_indices().nth(MAX_CHARS) {
        Some((byte_idx, _)) => format!("{}...", &trimmed[..byte_idx]),
        None => trimmed.to_string(),
    }
}

fn handle_auto_fanout_event(
    app: &mut App,
    terminal: &mut TuiTerminal,
    maybe_event: Option<Result<Event, io::Error>>,
    cancelled: &AtomicBool,
    task: &AutoFanoutTask,
    wheel_frame_gate: &mut StreamFrameGate,
    clipboard_write: &mut Option<PendingClipboardCopy>,
) -> Result<bool, TuiLoopError> {
    match maybe_event {
        Some(Ok(Event::Key(key))) => {
            if is_ctrl_c(&key) {
                cancel_auto_fanout(cancelled, task);
                let stopped_agents = stop_visible_agents(app);
                app.set_turn_activity("Cancelling Smart pre-analysis");
                app.push_block(RenderBlock::System {
                    id: next_synthetic_block_id(),
                    level: SystemLevel::Warn,
                    text: interrupt_message("Smart pre-analysis cancelled (Ctrl+C)", stopped_agents),
                });
                app.draw_frame(terminal)?;
                return Ok(true);
            }
            if let Some(decision) = key_to_permission_decision(&key, app) {
                if let Some(prompt) = app.take_active_prompt() {
                    let _ = prompt.responder.send(decision);
                }
            } else {
                let action = app.handle_key(key)?;
                if handle_auto_fanout_key_action(
                    &action,
                    app,
                    cancelled,
                    task,
                    clipboard_write,
                ) {
                    return Ok(true);
                }
            }
            app.draw_frame(terminal)?;
        }
        Some(Ok(Event::Paste(text))) => {
            app.handle_paste_owned(text);
            app.draw_frame(terminal)?;
        }
        Some(Ok(Event::Mouse(mouse))) => {
            let is_scroll = matches!(
                mouse.kind,
                crossterm::event::MouseEventKind::ScrollUp
                    | crossterm::event::MouseEventKind::ScrollDown
            );
            let action = app.handle_mouse(mouse)?;
            if matches!(action, AppAction::OpenWorkflowViewer) {
                crate::session::tui_loop::open_workflow_viewer(app);
                app.draw_frame(terminal)?;
            } else if let AppAction::OpenAgentInViewer(agent_id) = &action {
                crate::session::tui_loop::open_workflow_viewer_focused(app, agent_id);
                app.draw_frame(terminal)?;
            } else if let AppAction::ClipboardCopyBlock(text) = action {
                if clipboard_write.is_none() {
                    *clipboard_write = Some(PendingClipboardCopy::new(text));
                }
                app.draw_frame(terminal)?;
            } else if matches!(action, AppAction::Redraw) {
                // Hover repaints share the wheel arm's frame budget: crossing
                // rows flips the hover target once per row, so an ungated draw
                // per flip stacks full frames while the fan-out streams. The
                // 50 ms tick lands the final hover state.
                if wheel_frame_gate
                    .on_stream_update(std::time::Instant::now())
                    .draws_now()
                {
                    app.draw_frame(terminal)?;
                }
            } else if is_scroll {
                // Coalesce wheel repaints like the other event loops: the
                // scroll offset is already accumulated in `handle_mouse`, and
                // the fan-out loop's 50 ms tick (turn activity is always set
                // here) lands the final frame, so skipping a paint never
                // strands the scroll position.
                if wheel_frame_gate
                    .on_stream_update(std::time::Instant::now())
                    .draws_now()
                {
                    app.draw_frame(terminal)?;
                }
            }
        }
        Some(Ok(Event::Resize(..))) => {
            // Re-fit the inline viewport to the new height before repainting;
            // ratatui pins it to the height it was constructed with.
            let rebuilt =
                crate::session::tui_loop::on_terminal_resize(terminal, app.terminal_mode())?;
            // Coalesce resize repaints like the mid-turn loop: a drag-resize
            // fires many intermediate widths, and each new width invalidates the
            // whole transcript layout (full re-wrap + re-highlight), so painting
            // every event spends the budget on geometry the user is still
            // dragging past. The 50 ms tick lands the final size. A rebuilt
            // viewport paints immediately — its band is blank until it does.
            if rebuilt
                || wheel_frame_gate
                    .on_stream_update(std::time::Instant::now())
                    .draws_now()
            {
                app.draw_frame(terminal)?;
            }
        }
        Some(Ok(_)) => {}
        Some(Err(err)) => return Err(TuiLoopError::Io(err)),
        None => {
            cancel_auto_fanout(cancelled, task);
            stop_visible_agents(app);
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_ctrl_c(key: &KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && matches!(key.code, KeyCode::Char('c'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn handle_auto_fanout_key_action(
    action: &AppAction,
    app: &mut App,
    cancelled: &AtomicBool,
    task: &AutoFanoutTask,
    clipboard_write: &mut Option<PendingClipboardCopy>,
) -> bool {
    match action {
        AppAction::Quit => {
            cancel_auto_fanout(cancelled, task);
            stop_visible_agents(app);
            true
        }
        AppAction::OpenWorkflowViewer => {
            crate::session::tui_loop::open_workflow_viewer(app);
            false
        }
        AppAction::OpenAgentInViewer(agent_id) => {
            crate::session::tui_loop::open_workflow_viewer_focused(app, agent_id);
            false
        }
        AppAction::ClipboardCopyBlock(text) => {
            if clipboard_write.is_none() {
                *clipboard_write = Some(PendingClipboardCopy::new(text.clone()));
            }
            false
        }
        // During the prelude there is no model stream yet. Plain text entered
        // with Enter becomes a queued `AgentCommand::Steer` in the command
        // handler; slash commands stay queued on the app for the next outer loop.
        AppAction::ClipboardPaste
        | AppAction::ClipboardCopy(_)
        | AppAction::Submit(_)
        | AppAction::ConnectApiKey { .. }
        | AppAction::ConnectCustomProvider(_)
        | AppAction::ManageProvider(_)
        | AppAction::SelectModel(_)
        | AppAction::SelectPermission(_)
        | AppAction::SelectSession(_)
        | AppAction::ToggleTool { .. }
        | AppAction::SaveSmartSettings(_)
        | AppAction::DeepTier(_)
        | AppAction::Editor
        | AppAction::RewindCheckpoint
        | AppAction::ConfirmRewind
        | AppAction::OpenRewindViewer
        | AppAction::RewindTo(_)
        | AppAction::AckTeamInboxUpdate(_)
        | AppAction::IncludeTeamInboxUpdate(_)
        | AppAction::RefreshTeamInboxViewer
        | AppAction::Redraw
        | AppAction::TerminalResized
        | AppAction::None => false,
    }
}

fn command_targets_turn(command: &AgentCommand, turn_generation: u64) -> bool {
    match command {
        AgentCommand::RemoteCancelTurn {
            turn_generation: command_generation,
        }
        | AgentCommand::RemoteSteer {
            turn_generation: command_generation,
            ..
        } => *command_generation == turn_generation,
        AgentCommand::CancelTurn
        | AgentCommand::CancelTool
        | AgentCommand::Quit
        | AgentCommand::Steer(_) => true,
    }
}

fn handle_auto_fanout_command(
    app: &mut App,
    terminal: &mut TuiTerminal,
    cmd: AgentCommand,
    turn_generation: u64,
    prelude_steers: &mut Vec<String>,
    cancelled: &AtomicBool,
    task: &AutoFanoutTask,
) -> Result<bool, TuiLoopError> {
    match cmd {
        cmd if !command_targets_turn(&cmd, turn_generation) => Ok(false),
        AgentCommand::CancelTurn
        | AgentCommand::Quit
        | AgentCommand::RemoteCancelTurn { .. } => {
            cancel_auto_fanout(cancelled, task);
            let stopped_agents = stop_visible_agents(app);
            app.push_block(RenderBlock::System {
                id: BlockIdGen::default().next(),
                level: SystemLevel::Warn,
                text: interrupt_message("Smart pre-analysis cancelled (Ctrl+C)", stopped_agents),
            });
            app.draw_frame(terminal)?;
            Ok(true)
        }
        AgentCommand::Steer(text) | AgentCommand::RemoteSteer { text, .. } => {
            // Folding into the upcoming turn's input *is* the delivery —
            // clear the message's pending "queued" entry so it does not also
            // run as its own turn afterwards.
            app.remove_queued_message_matching(&text);
            prelude_steers.push(text);
            Ok(false)
        }
        // Smart pre-analysis runs before the turn exists, so there is no tool
        // dispatch to cancel yet. Swallow it rather than escalating to a
        // fan-out cancel: Esc-once must never end anything the user did not
        // aim it at.
        AgentCommand::CancelTool => Ok(false),
    }
}

fn push_live_agent_completion(
    app: &mut App,
    terminal: &mut TuiTerminal,
    completion: &AgentCompletion,
) -> Result<(), TuiLoopError> {
    // Completion-order `⎿ Done` flip on the transcript agent tree; an absorbed
    // `completed` event needs no extra system line (the tree row is the
    // notification — CC parity). Failures still surface as system lines.
    let absorbed = app.note_agent_completion_display(
        &completion.agent_id,
        &completion.name,
        &completion.status,
        completion.output_tokens,
    );
    if !(absorbed && completion.status == "completed") {
        let (level, text) = format_agent_completion(completion);
        app.push_block(RenderBlock::System {
            id: BlockIdGen::default().next(),
            level,
            text,
        });
    }
    app.draw_frame(terminal)?;
    Ok(())
}

fn cancel_auto_fanout(cancelled: &AtomicBool, task: &AutoFanoutTask) {
    cancelled.store(true, Ordering::Relaxed);
    task.abort();
}

fn stop_visible_agents(app: &mut App) -> usize {
    // A running `Workflow` tool executes on a `spawn_blocking` worker that the
    // dropped turn future can't abort, so also signal its phase loop to stop
    // spawning new phases (BUG-D6). A no-op when no workflow is running; the
    // flag is cleared at the next workflow start.
    request_foreground_workflow_cancel();
    let started_after = app.agent_manifest_started_after();
    let stopped = stop_running_agents_since_for_session(
        started_after,
        app.agent_manifest_session_id(),
        "cancelled by foreground turn",
    );
    // Always re-read after a stop request, even when `stopped == 0`.
    // The manifests may already be terminal (for example after an external
    // cancel or a previous stop), while the HUD still has the last live rows.
    // Rendering before this refresh is the "chat says stopped, sidebar still
    // says running" mismatch users see.
    apply_live_hud_snapshot(
        app,
        read_live_hud_snapshot(started_after, app.agent_manifest_session_id()),
    );
    stopped
}

fn interrupt_message(action: &str, stopped_agents: usize) -> String {
    if stopped_agents == 0 {
        return format!("⊘ Interrupted — {action}.");
    }
    format!("⊘ Interrupted — {action}; stopped {stopped_agents} background agent(s).")
}

/// Parse the `SpawnMultiAgent` summary into a labelled analysis block, pairing
/// each agent's result with its subtask role by index. Returns `None` when no
/// agent produced a usable result (the caller then proceeds single-agent).
fn format_fanout_analysis(summary: &str, roles: &[String]) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(summary).ok()?;
    let agents = parsed.get("agents")?.as_array()?;
    let mut sections = Vec::with_capacity(agents.len());
    let mut any_result = false;
    for agent in agents {
        let idx = agent
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .and_then(|i| usize::try_from(i).ok())
            .unwrap_or(0);
        let role = roles.get(idx).map_or("subtask", String::as_str);
        let result = agent
            .get("result")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty());
        if let Some(result) = result {
            any_result = true;
            sections.push(format!("### {role}\n{result}"));
        } else {
            let status = agent
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            sections.push(format!("### {role}\n(no result; status: {status})"));
        }
    }
    any_result.then(|| sections.join("\n\n"))
}

#[cfg(test)]
mod tests;
