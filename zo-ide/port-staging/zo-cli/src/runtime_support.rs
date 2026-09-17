use std::collections::BTreeMap;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use api::sync_bridge::run_blocking;
use api::{
    AuthRoute, AuthSource, ContentBlockDelta, MessageRequest, OutputContentBlock, PromptCache,
    ProviderClient, ProviderKind, StreamEvent as ApiStreamEvent, ToolChoice,
    resolve_startup_auth_source,
};
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ConcurrentDispatchFn, ConfigLoader, ConversationRuntime,
    PermissionMode, ProviderStateBlob, RuntimeError, Session, TokenUsage,
};
use tools::GlobalToolRegistry;

use crate::conversation_support::{convert_messages, permission_policy};
use crate::render::{MarkdownStreamState, TerminalRenderer};
use crate::response_events::{push_output_block, push_prompt_cache_record, response_to_events};
use crate::tool_formatting::format_tool_call_start;
use crate::{
    AllowedToolSet, BuiltRuntime, CliToolExecutor, DisallowedToolSet, RuntimePluginState,
    cli_tool_executor::{check_disallowed_tools, parse_tool_input_json},
    session::build_runtime_plugin_state_with_loader,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupAuthPolicy {
    /// Require provider credentials while constructing the runtime. Used by
    /// headless/automation paths so auth failures fail before work starts.
    Require,
    /// Let the interactive TUI open even when Claude auth cannot refresh. The
    /// client is built without credentials; model requests still fail until the
    /// user runs `/login` or switches to an authenticated provider.
    AllowUnauthenticated,
}

impl StartupAuthPolicy {
    const fn allows_unauthenticated(self) -> bool {
        matches!(self, Self::AllowUnauthenticated)
    }
}

/// Process-lifetime flags installed before the CLI runtime is built. Interactive
/// runtime rebuilds read the same snapshot, so startup restrictions cannot
/// disappear after a model, permission, or context change.
#[derive(Clone, Default)]
pub(crate) struct CliRuntimeOverrides {
    pub(crate) disallowed_tools: Option<DisallowedToolSet>,
    pub(crate) max_turns: Option<usize>,
    pub(crate) max_tool_calls: Option<usize>,
    pub(crate) system_prompt: Option<String>,
    pub(crate) append_system_prompt: Option<String>,
}

static CLI_RUNTIME_OVERRIDES: OnceLock<Mutex<CliRuntimeOverrides>> = OnceLock::new();

pub(crate) fn install_cli_runtime_overrides(overrides: CliRuntimeOverrides) {
    *CLI_RUNTIME_OVERRIDES
        .get_or_init(|| Mutex::new(CliRuntimeOverrides::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = overrides;
}

fn cli_runtime_overrides() -> CliRuntimeOverrides {
    CLI_RUNTIME_OVERRIDES
        .get_or_init(|| Mutex::new(CliRuntimeOverrides::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Build a runtime and apply an extended-thinking budget to its API client.
/// `cwd` pins runtime feature discovery to the session workspace rather than
/// the process-wide cwd.
#[allow(dead_code, clippy::too_many_arguments)]
pub(crate) fn build_runtime_with_thinking_for(
    cwd: &Path,
    session: Session,
    session_id: &str,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    thinking: Option<api::ThinkingConfig>,
    named_effort: Option<api::EffortLevel>,
    effort_band_ceiling: Option<api::EffortLevel>,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    build_runtime_with_thinking_for_auth_policy(
        cwd,
        session,
        session_id,
        model,
        system_prompt,
        enable_tools,
        emit_output,
        allowed_tools,
        permission_mode,
        thinking,
        named_effort,
        effort_band_ceiling,
        None,
        StartupAuthPolicy::Require,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_runtime_with_thinking_for_auth_policy(
    cwd: &Path,
    session: Session,
    session_id: &str,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    thinking: Option<api::ThinkingConfig>,
    named_effort: Option<api::EffortLevel>,
    effort_band_ceiling: Option<api::EffortLevel>,
    tasks: Option<runtime::task_registry::TaskRegistry>,
    startup_auth_policy: StartupAuthPolicy,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    let loader = ConfigLoader::default_for(cwd);
    // Stage-tagged like the LiveCli constructor (see `stage_tag` there): a
    // startup error must name the stage it escaped from or a rare failure
    // (the 2026-08-10 `Io(EINVAL)` flake) dies unattributable.
    let runtime_config = loader
        .load()
        .map_err(|error| format!("startup stage config-load: {error:?}"))?;
    apply_custom_providers_env(&runtime_config);
    apply_model_wire_env();
    spawn_session_retention_cleanup(&runtime_config);
    spawn_orphaned_agent_reap();
    let runtime_plugin_state =
        build_runtime_plugin_state_with_loader(cwd, &loader, &runtime_config, tasks)
            .map_err(|error| format!("startup stage plugin-state: {error:?}"))?;
    build_runtime_with_plugin_state_auth_policy(
        session,
        session_id,
        model,
        system_prompt,
        enable_tools,
        emit_output,
        allowed_tools,
        permission_mode,
        runtime_plugin_state,
        thinking,
        named_effort,
        effort_band_ceiling,
        startup_auth_policy,
    )
}

#[allow(dead_code, clippy::too_many_arguments)]
pub(crate) fn build_runtime_with_plugin_state(
    session: Session,
    session_id: &str,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    runtime_plugin_state: RuntimePluginState,
    thinking: Option<api::ThinkingConfig>,
    named_effort: Option<api::EffortLevel>,
    effort_band_ceiling: Option<api::EffortLevel>,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    build_runtime_with_plugin_state_auth_policy(
        session,
        session_id,
        model,
        system_prompt,
        enable_tools,
        emit_output,
        allowed_tools,
        permission_mode,
        runtime_plugin_state,
        thinking,
        named_effort,
        effort_band_ceiling,
        StartupAuthPolicy::Require,
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn build_runtime_with_plugin_state_auth_policy(
    session: Session,
    session_id: &str,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    runtime_plugin_state: RuntimePluginState,
    thinking: Option<api::ThinkingConfig>,
    named_effort: Option<api::EffortLevel>,
    effort_band_ceiling: Option<api::EffortLevel>,
    startup_auth_policy: StartupAuthPolicy,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    let RuntimePluginState {
        feature_config,
        tool_registry,
        plugin_registry,
        prompt_commands,
        memory_retriever,
        mcp_state,
        lsp_state,
    } = runtime_plugin_state;
    let overrides = cli_runtime_overrides();
    let mut system_prompt = overrides
        .system_prompt
        .clone()
        .map_or(system_prompt, |replacement| vec![replacement]);
    if let Some(addition) = overrides.append_system_prompt.clone() {
        system_prompt.push(addition);
    }
    let disallowed_tools = overrides.disallowed_tools.clone();
    tool_registry.context().set_active_model(&model);
    tool_registry.context().set_session_id(session_id);
    // Record the session permission mode on the shared context so the file-tool
    // workspace-boundary relaxation can see it. The foreground `tool_registry`
    // carries no `PermissionEnforcer` (tool gating is enforced at the runtime
    // layer below via `policy` + the prompter), so without this a full-access
    // user would be wrongly denied an outside `read_file`/`write_file` with
    // "escapes workspace boundary". A live Shift+Tab / `/permission` switch
    // refreshes it through the same shared cell (see `apply_permission_change`).
    tool_registry.context().set_permission_mode(permission_mode);
    plugin_registry
        .initialize()
        .map_err(|error| format!("startup stage plugin-initialize: {error:?}"))?;
    let policy = permission_policy(permission_mode, &feature_config, &tool_registry)
        .map_err(|error| format!("startup stage permission-policy: {error:?}"))?;
    // Derive context_window and model-family context policy from the actual
    // selected model so compaction thresholds match the real model limits (not
    // the optional feature_config.model() which may be None/200k default).
    let selected_model = model.clone();
    let effective_context_window = api::context_window_for_model(&selected_model);
    let tool_executor = CliToolExecutor::new(
        allowed_tools.clone(),
        disallowed_tools.clone(),
        emit_output,
        tool_registry.clone(),
        mcp_state.clone(),
    );
    // Shared per-turn allowed-tools slot: the concurrent dispatch closure below
    // must enforce the same one-turn restriction as the serial executor, or
    // concurrency-safe tools would bypass the gate entirely.
    let turn_allowed_slot = tool_executor.shared_turn_allowed_tools();
    let mut runtime = ConversationRuntime::new_with_context_window(
        session,
        build_claude_runtime_client(
            session_id,
            model,
            enable_tools,
            emit_output,
            allowed_tools.clone(),
            tool_registry.clone(),
            permission_mode,
            thinking,
            named_effort,
            effort_band_ceiling,
            startup_auth_policy,
        )
        .map_err(|error| format!("startup stage claude-runtime-client: {error:?}"))?,
        tool_executor,
        policy,
        system_prompt,
        &feature_config,
        effective_context_window,
    );
    runtime.set_context_model(&selected_model);
    if let Some(max_turns) = overrides.max_turns {
        runtime.set_max_iterations(max_turns);
    }
    if let Some(max_tool_calls) = overrides.max_tool_calls {
        runtime.set_max_tool_calls(max_tool_calls);
    }
    // Turn-level events (turn_completed with token counts, tool execution
    // audits) flow to the same global OTLP exporter as the HTTP events.
    if let Some(tracer) = api::otlp::session_tracer_from_env(session_id) {
        runtime = runtime.with_session_tracer(tracer);
    }
    runtime.set_auto_compaction_enabled(feature_config.auto_compact_enabled());
    runtime.set_memory_retriever(memory_retriever);
    // Wire up parallel tool execution: concurrency-safe tools (Read,
    // Glob, Grep, …) will run via spawn_blocking instead of serially.
    let dispatch_registry = tool_registry.clone();
    let dispatch_allowed = allowed_tools;
    let dispatch_disallowed = disallowed_tools;
    let dispatch_mcp = mcp_state.clone();
    let concurrent_dispatch: ConcurrentDispatchFn =
        std::sync::Arc::new(move |tool_name: &str, input: &str| {
            check_disallowed_tools(dispatch_disallowed.as_ref(), tool_name)?;
            if let Some(ref allowed) = dispatch_allowed {
                if !allowed.contains(tool_name) {
                    return Err(runtime::ToolError::new(format!(
                        "tool `{tool_name}` is not enabled by the current --allowedTools setting"
                    )));
                }
            }
            crate::cli_tool_executor::check_turn_allowed_tools(&turn_allowed_slot, tool_name)?;
            let input_str = if tool_name == "TaskList" && input.trim().is_empty() {
                "{}"
            } else {
                input
            };
            let value = parse_tool_input_json(tool_name, input_str)?;
            if dispatch_registry.has_runtime_tool(tool_name) {
                let Some(ref mcp) = dispatch_mcp else {
                    return Err(runtime::ToolError::new(format!(
                        "runtime tool `{tool_name}` unavailable without MCP servers"
                    )));
                };
                let mut state = mcp
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Same meta-tool handling as the serial CliToolExecutor path, so
                // `ListMcpResourcesTool` & friends don't hit `call_tool` and fail
                // as "unknown MCP tool" when dispatched concurrently/long-running.
                state.dispatch_runtime_tool(tool_name, value)
            } else {
                dispatch_registry
                    .execute(tool_name, &value)
                    .map_err(|e| runtime::ToolError::new(e.to_string()))
            }
        });
    runtime.set_concurrent_dispatch(concurrent_dispatch);
    install_subagent_mcp_passthrough(&tool_registry, mcp_state.clone());
    // Plugin tools spawn a blocking subprocess; mark them long-running so they
    // dispatch via spawn_blocking and never freeze the TUI render loop.
    runtime.set_long_running_tools(tool_registry.plugin_tool_names());
    // MCP / runtime tools do blocking network RPC. Flag them long-running via a
    // *live* predicate rather than a snapshot: the registry's runtime-tool set is
    // refreshed on mid-session `tools/list_changed`, and the predicate shares
    // that registry's `Arc`, so newly announced MCP tools are covered too.
    // Without this they dispatch via `block_in_place`, whose synchronous RPC
    // freezes the whole TUI render loop (spinner + timer), not just the stream.
    let long_running_registry = tool_registry.clone();
    runtime.set_long_running_predicate(std::sync::Arc::new(move |name: &str| {
        long_running_registry.has_runtime_tool(name)
    }));

    if emit_output {
        runtime = runtime.with_hook_progress_reporter(Box::new(CliHookProgressReporter));
    }
    Ok(BuiltRuntime::new(
        runtime,
        feature_config,
        plugin_registry,
        prompt_commands,
        mcp_state,
        lsp_state,
    ))
}

/// Install the sub-agent MCP passthrough on the registry: spawned agents
/// (Agent/SpawnMultiAgent/Workflow) advertise this session's MCP tools and
/// route their calls back through the same `dispatch_runtime_tool` seam the
/// foreground uses. The definitions side shares the registry's live
/// runtime-tools Arc, so a mid-session `tools/list_changed` refresh reaches
/// later spawns too. A session without MCP servers installs nothing.
fn install_subagent_mcp_passthrough(
    tool_registry: &GlobalToolRegistry,
    mcp_state: Option<std::sync::Arc<std::sync::Mutex<crate::session::RuntimeMcpState>>>,
) {
    let Some(passthrough_mcp) = mcp_state else {
        return;
    };
    let passthrough_registry = tool_registry.clone();
    tool_registry.install_subagent_mcp_passthrough(std::sync::Arc::new(
        move |tool_name: &str, input: &serde_json::Value| {
            if !passthrough_registry.has_runtime_tool(tool_name) {
                return Err(format!("unknown MCP tool `{tool_name}`"));
            }
            passthrough_mcp
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .dispatch_runtime_tool(tool_name, input.clone())
                .map_err(|error| error.to_string())
        },
    ));
}

pub(crate) struct CliHookProgressReporter;

impl runtime::HookProgressReporter for CliHookProgressReporter {
    fn on_event(&mut self, event: &runtime::HookProgressEvent) {
        match event {
            runtime::HookProgressEvent::Started {
                event,
                tool_name,
                command,
            } => eprintln!(
                "[hook {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            ),
            runtime::HookProgressEvent::Completed {
                event,
                tool_name,
                command,
            } => eprintln!(
                "[hook done {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            ),
            runtime::HookProgressEvent::Cancelled {
                event,
                tool_name,
                command,
            } => eprintln!(
                "[hook cancelled {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            ),
        }
    }
}

/// Pure predicate: is this process attached to a human text terminal that can
/// host a blocking approval prompt? True only when all three standard fds are
/// terminals.
///
/// The prompt is read from stdin and drawn on stderr, so those two must be
/// TTYs or the run would block on / write into a redirected fd. stdout is
/// required too because a redirected stdout (e.g. `zo -p ... > out.txt`)
/// is the signature of a non-interactive automation run: the human is not
/// watching a live terminal, so we must auto-deny rather than block on a
/// prompt they cannot answer. Requiring all three keeps the "non-TTY one-shot
/// never prompts or blocks" contract intact regardless of which single fd is
/// redirected.
fn interactive_terminal(stdin_tty: bool, stdout_tty: bool, stderr_tty: bool) -> bool {
    stdin_tty && stdout_tty && stderr_tty
}

/// Terminal prompter for the text one-shot path (`run_turn_capturing` drives
/// the deep-gate-aware streaming dispatcher, whose residual prompts arrive on
/// the async [`runtime::permission::PermissionPrompter`] contract).
/// Interactive only when stdin, stdout, and stderr are all terminals; the
/// approval UI is written to stderr so it never corrupts machine-readable
/// stdout, and a redirected stdout (`zo -p ... > out.txt`) marks an
/// automation run, which resolves every residual prompt to a safe deny
/// without touching stdin. The y/N read runs on the blocking pool so it never
/// stalls the render drain. `y` maps to `AllowOnce`: the y/N prompt never
/// installed a durable rule, and `Allow` on the async contract grants one for
/// the whole session.
pub(crate) struct TerminalPermissionPrompter {
    current_mode: PermissionMode,
    interactive: bool,
}

impl TerminalPermissionPrompter {
    pub(crate) fn new(current_mode: PermissionMode) -> Self {
        let interactive = interactive_terminal(
            io::stdin().is_terminal(),
            io::stdout().is_terminal(),
            io::stderr().is_terminal(),
        );
        Self {
            current_mode,
            interactive,
        }
    }

    /// The non-interactive state a redirected fd produces, constructed
    /// directly so tests never depend on the harness's real fds.
    #[cfg(test)]
    pub(crate) fn non_interactive_for_test(current_mode: PermissionMode) -> Self {
        Self {
            current_mode,
            interactive: false,
        }
    }
}

impl runtime::permission::PermissionPrompter for TerminalPermissionPrompter {
    fn decide<'a>(
        &'a self,
        request: runtime::permission::PermissionRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        runtime::permission::PermissionDecision,
                        runtime::permission::PermissionError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        use runtime::permission::PermissionDecision;
        let interactive = self.interactive;
        let mode = self.current_mode;
        Box::pin(async move {
            if !interactive {
                return Ok(PermissionDecision::Deny);
            }
            let decision = tokio::task::spawn_blocking(move || {
                // Interactive approval UI goes to stderr so machine-readable
                // stdout is never mixed with the prompt (mirrors the sync
                // prompter). The async request deliberately carries a
                // display-safe input summary, never the full payload.
                let mut err = io::stderr();
                let _ = writeln!(err);
                let _ = writeln!(err, "Permission approval required");
                let _ = writeln!(err, "  Tool             {}", request.tool);
                let _ = writeln!(err, "  Current mode     {}", mode.as_str());
                let _ = writeln!(err, "  Reason           {}", request.reasoning);
                let _ = writeln!(err, "  Input            {}", request.input_summary);
                let _ = write!(err, "Approve this tool call? [y/N]: ");
                let _ = err.flush();

                let mut response = String::new();
                match io::stdin().read_line(&mut response) {
                    // EOF or a read error on an interactive stdin: deny rather
                    // than looping or hanging.
                    Ok(0) | Err(_) => PermissionDecision::Deny,
                    Ok(_) => {
                        let normalized = response.trim().to_ascii_lowercase();
                        if matches!(normalized.as_str(), "y" | "yes") {
                            PermissionDecision::AllowOnce
                        } else {
                            PermissionDecision::Deny
                        }
                    }
                }
            })
            .await
            .unwrap_or(PermissionDecision::Deny);
            Ok(decision)
        })
    }
}

pub(crate) struct AnthropicRuntimeClient {
    client: ProviderClient,
    session_id: String,
    model: String,
    auth_route: AuthRoute,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    tool_registry: GlobalToolRegistry,
    /// Extended-thinking budget applied to every request. Mirrors the
    /// budget the TUI path injects via `LiveAsyncApiClient`, so the headless
    /// `run_turn` / `--print` JSON paths honor `/effort` too (previously they
    /// hardcoded `thinking: None`, dropping the budget).
    thinking: Option<api::ThinkingConfig>,
    /// Named effort preset carried separately from the legacy token budget.
    named_effort: Option<api::EffortLevel>,
    /// `Some(ceiling)` when `named_effort` is Smart's dynamic-band floor
    /// (Xhigh) rather than a static pin — mirrors
    /// `LiveAsyncApiClient::effort_band_ceiling` (`runtime_bridge.rs`) so the
    /// headless `-p`/serve sync path resolves the same band the TUI does.
    effort_band_ceiling: Option<api::EffortLevel>,
    /// Provider-neutral telemetry seam. The Anthropic client emits its own
    /// HTTP-request spans / `message_usage` analytics from inside the api
    /// crate, but the OpenAI/Gemini/xAI/Ollama clients do not. This tracer
    /// lets the shared `stream()` loop record `api_request` / `api_error`
    /// spans and `message_usage` for those providers so non-Anthropic
    /// operators get the same request-level telemetry. `None` unless OTLP
    /// export is enabled via env (the same gate the Anthropic path uses).
    session_tracer: Option<api::SessionTracer>,
}

/// Outcome of one non-Anthropic streaming request, fed to the neutral
/// telemetry seam. Mirrors the success/failure split the Anthropic client
/// records internally so OTLP counters (`zo_code.api_request` with
/// `outcome=success|error`) tally identically across providers.
enum NeutralRequestOutcome {
    /// The stream completed; carries the cumulative token usage observed on
    /// the closing `message_delta` (zeroed if the provider sent none).
    Succeeded { usage: TokenUsage },
    /// The request failed before or during streaming.
    Failed { error: String, retryable: bool },
}

/// A representative request path label for a provider's chat endpoint, used
/// as the `path` attribute on neutral request spans. The exact route varies
/// per provider (and per streaming vs non-streaming), but the label only has
/// to be stable and human-legible for the trace; it is not used for routing.
fn neutral_request_path(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Anthropic => "/v1/messages",
        ProviderKind::Google => "/v1beta/models:streamGenerateContent",
        ProviderKind::Ollama => "/api/chat",
        // OpenAI (Responses/chat) and xAI both speak the OpenAI-compatible
        // wire; the chat-completions route is the legible default.
        ProviderKind::OpenAi | ProviderKind::Xai => "/v1/chat/completions",
    }
}

/// Record one non-Anthropic request's lifecycle on the provider-neutral
/// telemetry seam: a `request_started` span, then either a success span plus
/// a `message_usage` analytics event, or a failure span. This is the seam
/// equivalent of what `AnthropicClient::send_with_retry` / `send_message`
/// emit internally; emitting it here (and only for non-Anthropic providers,
/// to avoid double-counting) is what gives GPT/Gemini operators request-level
/// telemetry. Pure over its inputs so it is unit-testable against a
/// `MemoryTelemetrySink`-backed tracer.
fn emit_neutral_request_telemetry(
    tracer: &api::SessionTracer,
    provider: ProviderKind,
    model: &str,
    outcome: &NeutralRequestOutcome,
) {
    use serde_json::{Map, Value};

    let path = neutral_request_path(provider);
    tracer.record_http_request_started(1, "POST", path, Map::new());
    match outcome {
        NeutralRequestOutcome::Succeeded { usage } => {
            tracer.record_http_request_succeeded(1, "POST", path, 200, None, Map::new());
            tracer.record_analytics(
                api::AnalyticsEvent::new("api", "message_usage")
                    .with_property("request_id", Value::Null)
                    .with_property("model", Value::String(model.to_string()))
                    .with_property("total_tokens", Value::from(usage.total_tokens()))
                    .with_property("input_tokens", Value::from(usage.input_tokens))
                    .with_property("output_tokens", Value::from(usage.output_tokens)),
            );
        }
        NeutralRequestOutcome::Failed { error, retryable } => {
            tracer.record_http_request_failed(
                1,
                "POST",
                path,
                error.clone(),
                *retryable,
                Map::new(),
            );
        }
    }
}

/// Where a resolved [`AuthSource`] came from. Drives whether the process-wide
/// memo is final or a recoverable fallback: a Claude Code keychain token that
/// was unusable at startup can become valid again mid-session (the api layer
/// now refreshes an expired keychain token itself, and the desktop app may
/// too), so anything tagged [`AuthOrigin::Fallback`] re-checks the keychain on
/// every resolve instead of pinning the stale choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthOrigin {
    /// `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` — fixed for the process.
    Env,
    /// A valid Claude Code keychain session token (carries `user:inference`).
    Keychain,
    /// Saved `zo login` OAuth or the Claude CLI bridge, reached only because
    /// the keychain was unavailable. Re-attempt keychain recovery each resolve.
    Fallback,
}

/// Process-wide auth memo. Besides the credential and its origin, a
/// keychain-origin entry remembers its hard expiry so the turn boundary can
/// refresh it *proactively* (Claude Code parity) instead of waiting for a 401.
#[derive(Debug, Clone)]
struct CachedClaudeAuth {
    auth: AuthSource,
    origin: AuthOrigin,
    /// Unix ms expiry of the resolved bearer when its origin records one. The
    /// proactive keychain probe consumes it for `Keychain` origins; a
    /// fallback's expiry is re-read from the credentials file each turn by
    /// `oauth_refresh_needed`, and env credentials never expire under us.
    expires_at_ms: Option<u64>,
}

static CACHED_AUTH: OnceLock<Mutex<Option<CachedClaudeAuth>>> = OnceLock::new();

fn resolve_and_cache_claude_auth() -> Result<AuthSource, Box<dyn std::error::Error>> {
    let cache = CACHED_AUTH.get_or_init(|| Mutex::new(None));
    let mut guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Cache hit. Env/keychain origins are final. A `Fallback` memo means the
    // keychain was unusable at startup — retry it now (the api layer refreshes
    // an expired keychain token in place) so recovery needs no restart.
    if let Some(cached) = guard.as_ref() {
        if cached.origin != AuthOrigin::Fallback {
            return Ok(cached.auth.clone());
        }
        if let Some(session) = api::read_claude_code_keychain_session() {
            let recovered = AuthSource::BearerToken(session.access_token);
            *guard = Some(CachedClaudeAuth {
                auth: recovered.clone(),
                origin: AuthOrigin::Keychain,
                expires_at_ms: session.expires_at_ms,
            });
            AuthSource::cache_resolved(&recovered);
            return Ok(recovered);
        }
        return Ok(cached.auth.clone());
    }

    let resolved = resolve_fresh_claude_auth()?;
    let auth = resolved.auth.clone();
    AuthSource::cache_resolved(&auth);
    *guard = Some(resolved);
    Ok(auth)
}

/// Map the api layer's resolution origin onto the CLI memo's recovery policy:
/// a keychain session is final until its own expiry, saved OAuth stays a
/// fallback (re-probe the keychain every turn), env credentials are
/// process-fixed.
fn cli_auth_origin(origin: api::ClaudeAuthOrigin) -> AuthOrigin {
    match origin {
        api::ClaudeAuthOrigin::Keychain => AuthOrigin::Keychain,
        api::ClaudeAuthOrigin::SavedOauth => AuthOrigin::Fallback,
        api::ClaudeAuthOrigin::Env => AuthOrigin::Env,
    }
}

/// Resolve auth from scratch, tagging the origin so the memo knows whether the
/// result is final or a recoverable fallback. Zo is an OAuth-subscription
/// tool first, so the api chain runs managed OAuth before env keys:
/// 1) Claude Code keychain (refreshing an expired token in place)
/// 2) saved `zo login` OAuth (refreshing)
/// 3) env `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN`
/// 4) the runtime-config bridge (custom `.zo` OAuth config), which also
///    produces the canonical missing-credentials error.
fn resolve_fresh_claude_auth() -> Result<CachedClaudeAuth, Box<dyn std::error::Error>> {
    if let Some(resolved) = api::resolve_claude_auth_fresh_detailed() {
        if resolved.origin == api::ClaudeAuthOrigin::SavedOauth {
            warn_if_saved_oauth_lacks_inference();
        }
        return Ok(CachedClaudeAuth {
            auth: resolved.auth,
            origin: cli_auth_origin(resolved.origin),
            expires_at_ms: resolved.expires_at_ms,
        });
    }

    let auth = resolve_claude_cli_auth_source()?;
    Ok(CachedClaudeAuth {
        auth,
        origin: AuthOrigin::Fallback,
        expires_at_ms: None,
    })
}

/// Warn when the saved `zo login` token can't do inference, so a keychain-less
/// fallback doesn't silently 403 every turn across every session/project. The
/// `claude.ai` subscription flow grants `user:inference`; the old
/// `platform.claude.com` console flow did not. An empty scope list means a token
/// saved before scopes were persisted — also worth a re-login. The global
/// credentials file is shared by all projects, so one `zo login` fixes them
/// all at once.
fn warn_if_saved_oauth_lacks_inference() {
    let Ok(Some(token)) = runtime::load_oauth_credentials() else {
        return;
    };
    if !token.scopes.iter().any(|scope| scope == "user:inference") {
        eprintln!(
            "\x1b[33mZo login token lacks the user:inference scope — run `zo login` again \
             to use the claude.ai subscription flow (otherwise every turn 403s).\x1b[0m"
        );
    }
}

/// Overwrite the process-wide auth memo (and the api-layer subagent cache) after
/// a mid-session refresh, so later runtime rebuilds and spawned subagents use the
/// fresh token rather than the expired snapshot resolved at startup. The origin
/// is recorded so a keychain recovery is treated as final while a saved-OAuth
/// refresh stays a `Fallback` (still eligible for later keychain recovery).
fn update_cached_claude_auth(auth: &AuthSource, origin: AuthOrigin, expires_at_ms: Option<u64>) {
    if let Some(lock) = CACHED_AUTH.get() {
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(CachedClaudeAuth {
            auth: auth.clone(),
            origin,
            expires_at_ms,
        });
    }
    AuthSource::cache_resolved(auth);
}

/// Refresh the long-lived OAuth bearer this many seconds before its hard expiry,
/// matching the api-layer startup buffer (`OAUTH_EXPIRY_BUFFER_SECS`).
const OAUTH_REFRESH_BUFFER_SECS: u64 = 60;

/// Pure predicate: should the interactive client refresh its saved-OAuth bearer
/// now? Only for the `zo login` path — env-managed auth (API key / bearer)
/// never expires under us, and a missing or comfortably-future expiry needs no
/// refresh.
fn oauth_refresh_needed(env_managed: bool, expires_at: Option<u64>, now: u64) -> bool {
    !env_managed
        && expires_at
            .is_some_and(|expires_at| expires_at <= now.saturating_add(OAUTH_REFRESH_BUFFER_SECS))
}

/// Best-effort mid-session OAuth refresh for the long-lived interactive client.
///
/// The TUI builds the client once at startup and reuses it across turns; the
/// resolved OAuth access token is a bare bearer snapshot and the request path
/// never refreshes it, so once it crosses `expires_at` every request fails with a
/// 401 until the process restarts. Called at the interactive turn boundary
/// (before the per-turn `client()` clone is taken): when the saved `zo login`
/// token is within the refresh buffer, refresh it (reusing the startup refresh
/// path, which also re-persists the new token set) and swap the live client's
/// bearer. No-op for env-key / non-OAuth auth or a still-fresh token. The network
/// refresh runs on a blocking thread to avoid a nested-runtime panic inside the
/// async turn; a refresh failure is swallowed (the existing 401 message still
/// guides the user to `zo login`).
pub(crate) async fn refresh_oauth_if_near_expiry(client: &mut AnthropicRuntimeClient) {
    // OAuth-backed non-Anthropic clients (Gemini Code Assist, ChatGPT) capture
    // their bearer at construction and never refresh per-request — unlike the
    // Anthropic client, whose `set_auth` swaps the bearer in place below. Rotate
    // them by rebuilding from `build_provider_client`, which re-runs the
    // provider's own loader (`load_fresh_oauth` / `load_fresh_openai_oauth`);
    // each loader refreshes and re-persists a near-expiry token. Generic — no
    // per-provider token handling here, so a stale Gemini token no longer drops
    // the session mid-run with no recovery.
    if matches!(
        &client.client,
        ProviderClient::GeminiCodeAssist(_) | ProviderClient::ChatGpt(_)
    ) {
        if client.client.oauth_rebuild_needed() {
            rebuild_oauth_client(client).await;
        }
        return;
    }
    if !matches!(&client.client, ProviderClient::Anthropic(_))
        || client.auth_route == AuthRoute::ApiKey
    {
        return;
    }

    // First, the keychain lane. Two cases share one blocking probe:
    // - `Fallback` memo: the keychain was unusable at startup and we run on a
    //   weaker token (the saved `zo login` OAuth may lack `user:inference`
    //   and 403 every turn) — re-read it each turn; the api layer refreshes an
    //   expired keychain token in place, so recovery no longer depends on the
    //   desktop app and needs no restart.
    // - `Keychain` memo nearing its recorded expiry: refresh *proactively*,
    //   exactly like Claude Code, instead of letting the next request 401.
    // A final, comfortably-fresh keychain memo skips the probe entirely, so
    // the hot turn path doesn't shell out to `security`.
    let recovered = tokio::task::spawn_blocking(keychain_recovery_session)
        .await
        .ok()
        .flatten();
    if let Some(session) = recovered {
        let auth = AuthSource::BearerToken(session.access_token);
        update_cached_claude_auth(&auth, AuthOrigin::Keychain, session.expires_at_ms);
        client.set_auth(auth);
        return;
    }

    // OAuth-first: env credentials only "manage" the session when they are
    // what resolution actually picked (memo origin `Env`) — their mere
    // presence in the environment must not freeze a saved-OAuth bearer that
    // outranked them.
    let env_managed = cached_auth_origin().is_some_and(|origin| origin == AuthOrigin::Env);
    let refreshed = tokio::task::spawn_blocking(move || {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let expires_at = runtime::load_oauth_credentials()
            .ok()
            .flatten()
            .and_then(|credentials| credentials.expires_at);
        if !oauth_refresh_needed(env_managed, expires_at, now) {
            return None;
        }
        api::resolve_claude_auth_fresh_detailed()
    })
    .await
    .ok()
    .flatten();

    if let Some(resolved) = refreshed {
        update_cached_claude_auth(
            &resolved.auth,
            cli_auth_origin(resolved.origin),
            resolved.expires_at_ms,
        );
        client.set_auth(resolved.auth);
    }
}

/// Rebuild an OAuth-backed non-Anthropic provider client so an expired or
/// near-expiry bearer rotates. The provider's loader runs a blocking token
/// refresh, so this hops to a blocking thread (avoiding the nested-runtime panic
/// inside the async turn). A rebuild failure is swallowed — the existing client
/// (and its 401, if any) still guides the user to re-login.
async fn rebuild_oauth_client(client: &mut AnthropicRuntimeClient) {
    let session_id = client.session_id.clone();
    let model = client.model.clone();
    let auth_route = client.auth_route;
    let rebuilt = tokio::task::spawn_blocking(move || {
        build_provider_client(&session_id, &model, auth_route, None).ok()
    })
    .await
    .ok()
    .flatten();
    if let Some(rebuilt) = rebuilt {
        client.client = rebuilt;
    }
}

/// The origin recorded in the process-wide auth memo, if any.
fn cached_auth_origin() -> Option<AuthOrigin> {
    let guard = CACHED_AUTH
        .get()?
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.as_ref().map(|cached| cached.origin)
}

/// Probe the Claude Code keychain when the memo warrants it: always while on a
/// [`AuthOrigin::Fallback`] (recover the moment the keychain is usable again),
/// and for a [`AuthOrigin::Keychain`] memo only once its recorded expiry is
/// inside the refresh buffer (the api re-read then refreshes the expired blob
/// in place). `None` when the memo is env-pinned, comfortably fresh, or the
/// keychain stays unavailable — keeping the per-turn path from spawning
/// `security` needlessly.
fn keychain_recovery_session() -> Option<api::KeychainSession> {
    let cache = CACHED_AUTH.get()?;
    let should_probe = {
        let guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.as_ref() {
            Some(cached) => match cached.origin {
                AuthOrigin::Fallback => true,
                AuthOrigin::Keychain => {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
                        .unwrap_or(u64::MAX);
                    keychain_refresh_due(cached.expires_at_ms, now_ms)
                }
                AuthOrigin::Env => false,
            },
            None => false,
        }
    };
    if !should_probe {
        return None;
    }
    api::read_claude_code_keychain_session()
}

/// Pure gate for the proactive keychain refresh: due once the recorded expiry
/// is within the shared refresh buffer. An unrecorded expiry never schedules a
/// probe (the reactive 401 path still covers it).
fn keychain_refresh_due(expires_at_ms: Option<u64>, now_ms: u64) -> bool {
    expires_at_ms.is_some_and(|expires_at_ms| {
        now_ms.saturating_add(OAUTH_REFRESH_BUFFER_SECS.saturating_mul(1000)) >= expires_at_ms
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderRoute {
    Anthropic,
    Xai,
    OpenAiDefault,
    OpenAiCustom(&'static str),
    Google,
    Ollama,
}

pub(crate) fn catalog_provider_for_model(model: &str) -> Option<ProviderKind> {
    runtime::model_catalog::ModelCatalog::load()
        .ok()
        .and_then(|catalog| catalog.provider_for_model(model.trim()))
}

pub(crate) fn catalog_auth_route_for_model(model: &str) -> AuthRoute {
    runtime::model_catalog::ModelCatalog::load()
        .ok()
        .and_then(|catalog| catalog.auth_route_for_model(model.trim()))
        .unwrap_or(AuthRoute::Auto)
}

fn provider_kind_for_model(model: &str) -> ProviderKind {
    let trimmed = model.trim();
    let lower = trimmed.to_ascii_lowercase();

    if let Some(provider) = catalog_provider_for_model(trimmed) {
        return provider;
    }

    if let Some(entry) = api::provider_catalog().iter().find(|entry| {
        entry.alias == lower || entry.canonical_model_id.eq_ignore_ascii_case(trimmed)
    }) {
        return entry.provider;
    }

    if lower.starts_with("claude")
        || lower.contains("opus")
        || lower.contains("sonnet")
        || lower.contains("haiku")
    {
        return ProviderKind::Anthropic;
    }
    if lower.starts_with("gpt")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
        || lower.starts_with("codex")
        || lower.starts_with("openai")
    {
        return ProviderKind::OpenAi;
    }
    if lower.starts_with("gemini") {
        return ProviderKind::Google;
    }
    if lower.starts_with("grok") {
        return ProviderKind::Xai;
    }

    api::detect_provider_kind(model)
}

fn provider_route_for_model(model: &str) -> ProviderRoute {
    match provider_kind_for_model(model) {
        ProviderKind::Anthropic => ProviderRoute::Anthropic,
        ProviderKind::Xai => ProviderRoute::Xai,
        ProviderKind::OpenAi => custom_openai_provider_name_for_model(model)
            .map_or(ProviderRoute::OpenAiDefault, ProviderRoute::OpenAiCustom),
        ProviderKind::Google => ProviderRoute::Google,
        ProviderKind::Ollama => ProviderRoute::Ollama,
    }
}

fn custom_openai_provider_name_for_model(model: &str) -> Option<&'static str> {
    let trimmed = model.trim();
    let lower = trimmed.to_ascii_lowercase();

    // Match the API crate's precedence: built-in registry rows always win over
    // custom-provider collisions, so a custom entry for e.g. `gpt-5` must not
    // make an official OpenAI model look like a custom route.
    if api::provider_catalog().iter().any(|entry| {
        entry.alias == lower || entry.canonical_model_id.eq_ignore_ascii_case(trimmed)
    }) {
        return None;
    }

    let canonical = api::resolve_model_alias(trimmed);
    api::custom_provider_catalog()
        .into_iter()
        .find(|(_, models)| {
            models.iter().any(|served| {
                served.eq_ignore_ascii_case(trimmed)
                    || served.eq_ignore_ascii_case(canonical.as_str())
            })
        })
        .map(|(name, _)| name)
}

fn build_provider_client(
    session_id: &str,
    model: &str,
    auth_route: AuthRoute,
    anthropic_auth: Option<AuthSource>,
) -> Result<ProviderClient, Box<dyn std::error::Error>> {
    let client = if let Some(provider_kind) = catalog_provider_for_model(model) {
        ProviderClient::from_provider_kind_with_auth_route_and_anthropic_auth(
            provider_kind,
            auth_route,
            anthropic_auth,
        )?
    } else {
        ProviderClient::from_model_with_auth_route_and_anthropic_auth(
            model,
            auth_route,
            anthropic_auth,
        )?
    };

    if let ProviderClient::Anthropic(client) = client {
        let mut client = client
            .with_base_url(api::read_base_url())
            .with_prompt_cache(PromptCache::new(session_id));

        // The OAuth beta header is first-party-only; Bedrock/Vertex gateways
        // replace the auth chain entirely and would reject it.
        if client.auth().bearer_token().is_some() && !api::cloud_gateway_active() {
            client = client.with_beta("oauth-2025-04-20");
        }
        // Server-side clear_tool_uses defaults on; the environment flag is an opt-out.
        client = client.with_env_context_editing();
        // OTLP export (CC monitoring parity): when enabled via env, HTTP
        // request events flow to the process-global exporter.
        if let Some(tracer) = api::otlp::session_tracer_from_env(session_id) {
            client = client.with_session_tracer(tracer);
        }

        return Ok(ProviderClient::Anthropic(client));
    }

    // Pin the ChatGPT prompt-cache scope to the zo session id (Anthropic gets
    // the same via `PromptCache::new(session_id)` above): without it the scope
    // defaults to a random per-client id, and every provider-route model swap
    // or OAuth rotation rebuilds the client — rolling the provider cache key
    // mid-session.
    Ok(client.with_cache_scope(session_id))
}

/// Mirror the merged settings `providers` array into the env var the `api` crate
/// reads for OpenAI-compatible custom providers (Ollama / LM Studio / `DeepSeek`
/// / Kimi / Qwen / …). `api` cannot depend on runtime config, so this is the
/// single bridge, and it runs before any provider client is built.
///
/// The runtime is rebuilt several times per session (`/model`, `/resume`, every
/// headless turn), so this must re-publish rather than short-circuit on "the
/// variable is already set": the value it would find is usually zo's own boot
/// snapshot, and honoring that would revert a `/connect` merge or a
/// `/providers` delete made since. [`custom_provider_env`] draws the line
/// between zo's own seed and a genuine operator export, which is still never
/// rewritten.
fn apply_custom_providers_env(config: &runtime::RuntimeConfig) {
    if crate::custom_provider_env::operator_override_active() {
        api::refresh_custom_providers_from_env();
        return;
    }
    // `None` means "no providers configured" — publish the empty array anyway
    // so removing the last provider is reflected on the next rebuild.
    let json = config
        .custom_providers_json()
        .unwrap_or_else(|| "[]".to_string());
    if let Err(error) = crate::custom_provider_env::publish(&json) {
        eprintln!("[zo] failed to refresh custom provider catalog from settings: {error}");
    }
}

/// Mirror the settings-declared model wire ids into the env var the `api` crate
/// reads for its model catalog. Same bridge rationale as
/// [`apply_custom_providers_env`]: `api` cannot depend on runtime config, and
/// this must run before any provider client is built.
///
/// Re-publishes on every runtime rebuild for the same reason that one does — a
/// `/model` add or edit lands in settings and would otherwise not be served
/// until the next restart.
fn apply_model_wire_env() {
    let declared = runtime::model_catalog::ModelCatalog::load()
        .ok()
        .and_then(|catalog| catalog.catalog_overlay_json());
    if let Err(error) = crate::model_wire_env::publish(declared.as_deref()) {
        eprintln!("[zo] failed to publish the declared model catalog from settings: {error}");
    }
}

/// Retire expired session transcripts (CC-parity `cleanupPeriodDays`, default
/// 30) on a detached thread so boot never waits on the sweep. Once per
/// process: `/resume` and model handoffs rebuild the runtime, and re-sweeping
/// on each rebuild would just re-walk an already-clean tree.
/// Removed-transcript count above which one boot sweep is flagged as likely a
/// restored backup or moved config home rather than organic aging.
const LARGE_SWEEP_WARN: usize = 200;

/// Settle phantom `running` agent manifests whose owning zo process died
/// (crash, kill, `/restart`) — without this every store reader (HUD live
/// rows, stop paths) shows them running forever. Same once-per-process
/// detached-thread idiom as the retention sweep below: boot never waits on
/// the `ps` probe or the store walk.
fn spawn_orphaned_agent_reap() {
    use std::sync::OnceLock;
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::thread::spawn(|| {
            let reaped = tools::reap_orphaned_agents();
            if reaped > 0 {
                eprintln!(
                    "[zo] agent store: settled {reaped} orphaned running manifest(s) whose owning process exited"
                );
            }
        });
    });
}

/// Boot-time delay for the retention sweep so its store-wide walk never
/// competes with the interactive warm-up for disk/CPU on low-spec machines.
const STARTUP_SWEEP_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

fn spawn_session_retention_cleanup(config: &runtime::RuntimeConfig) {
    use std::sync::OnceLock;
    static ONCE: OnceLock<()> = OnceLock::new();
    let Some(days) = config.session_retention_days() else {
        return; // cleanupPeriodDays: 0 — retention disabled
    };
    ONCE.get_or_init(|| {
        std::thread::spawn(move || {
            // The sweep walks every project's session store — real IO on a
            // cold or low-spec machine. Hold it back until the interactive
            // warm-up (first paint, first prompt) is past; a session shorter
            // than the delay simply defers the sweep to the next boot.
            std::thread::sleep(STARTUP_SWEEP_DELAY);
            // Age out content-addressed tool-output artifacts on the same
            // retention horizon. Only the workflow retention path retires
            // artifacts by hash, so ordinary tool-output artifacts would
            // otherwise accumulate under `.zo/artifacts` without bound. Reuse
            // `days` as the max age (best-effort; failures are ignored).
            let pruned_artifacts =
                tools::prune_artifact_files_older_than(std::time::Duration::from_secs(
                    u64::from(days).saturating_mul(24 * 60 * 60),
                ));
            if pruned_artifacts > 0 {
                eprintln!(
                    "[zo] artifact retention: removed {pruned_artifacts} stale artifact file(s) (older than {days}d)"
                );
            }
            let report = runtime::session_control::cleanup_expired_sessions(days);
            if !report.is_empty() {
                // Boot-time stderr reaches zo.log on the TUI path and the
                // terminal on headless runs; silent when nothing expired.
                eprintln!(
                    "[zo] session retention: removed {} transcript(s), {} pref file(s), {} stale lock(s), {} empty dir(s) (~{} MB, older than {days}d)",
                    report.removed_sessions,
                    report.removed_prefs,
                    report.removed_locks,
                    report.removed_dirs,
                    report.reclaimed_bytes / (1024 * 1024),
                );
                // A sweep this large on one boot is almost always a restored
                // backup or a repointed ZO_CONFIG_HOME whose mtimes predate
                // the cutoff — not organic aging. Call it out loudly so a
                // surprised user can set cleanupPeriodDays: 0 and recover from
                // backup before the next boot sweeps the rest.
                if report.removed_sessions >= LARGE_SWEEP_WARN {
                    eprintln!(
                        "[zo] warning: that retention sweep removed {} transcripts at once — if this was a restored backup or a moved config home, set `cleanupPeriodDays: 0` to stop further deletion",
                        report.removed_sessions,
                    );
                }
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn build_claude_runtime_client(
    session_id: &str,
    model: String,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    tool_registry: GlobalToolRegistry,
    permission_mode: PermissionMode,
    thinking: Option<api::ThinkingConfig>,
    named_effort: Option<api::EffortLevel>,
    effort_band_ceiling: Option<api::EffortLevel>,
    startup_auth_policy: StartupAuthPolicy,
) -> Result<AnthropicRuntimeClient, Box<dyn std::error::Error>> {
    AnthropicRuntimeClient::new(
        session_id,
        model,
        enable_tools,
        emit_output,
        allowed_tools,
        tool_registry,
        permission_mode,
        thinking,
        named_effort,
        effort_band_ceiling,
        startup_auth_policy,
    )
}

fn resolve_startup_auth<R>(
    startup_auth_policy: StartupAuthPolicy,
    resolve_auth: R,
) -> Result<AuthSource, Box<dyn std::error::Error>>
where
    R: FnOnce() -> Result<AuthSource, Box<dyn std::error::Error>>,
{
    match resolve_auth() {
        Ok(auth) => Ok(auth),
        Err(error) if startup_auth_policy.allows_unauthenticated() => {
            eprintln!(
                "[zo] Claude auth unavailable at startup: {error}. Opening TUI unauthenticated; run `/login claude` before sending Anthropic requests."
            );
            Ok(AuthSource::None)
        }
        Err(error) => Err(error),
    }
}

impl AnthropicRuntimeClient {
    pub(crate) fn client(&self) -> ProviderClient {
        self.client.clone()
    }

    pub(crate) fn provider_kind(&self) -> ProviderKind {
        self.client.provider_kind()
    }

    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub(crate) const fn auth_route(&self) -> AuthRoute {
        self.auth_route
    }

    pub(crate) fn enable_tools(&self) -> bool {
        self.enable_tools
    }

    pub(crate) fn tool_registry(&self) -> GlobalToolRegistry {
        self.tool_registry.clone()
    }

    pub(crate) fn set_model(&mut self, model: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.set_model_with_auth_resolver(model, resolve_and_cache_claude_auth)
    }

    fn set_model_with_auth_resolver<R>(
        &mut self,
        model: &str,
        resolve_auth: R,
    ) -> Result<(), Box<dyn std::error::Error>>
    where
        R: FnOnce() -> Result<AuthSource, Box<dyn std::error::Error>>,
    {
        let provider_kind = provider_kind_for_model(model);
        let auth_route = catalog_auth_route_for_model(model);
        let provider_route_changed =
            provider_route_for_model(&self.model) != provider_route_for_model(model);
        let auth_route_changed = self.auth_route != auth_route;
        if provider_kind != self.provider_kind() || provider_route_changed || auth_route_changed {
            let auth = if provider_kind == ProviderKind::Anthropic
                && auth_route == AuthRoute::Auto
            {
                Some(resolve_auth()?)
            } else {
                None
            };
            self.client = build_provider_client(&self.session_id, model, auth_route, auth)?;
            self.auth_route = auth_route;
        }
        self.model = model.to_string();
        Ok(())
    }

    /// Swap the underlying client's auth (mid-session OAuth refresh). Only the
    /// auth field changes — the OAuth beta header, base URL, and prompt cache set
    /// at construction are preserved. The per-turn `LiveAsyncApiClient` is built
    /// from a fresh `client()` clone, so updating it here before the next turn's
    /// clone is taken propagates the new bearer to that turn's requests.
    pub(crate) fn set_auth(&mut self, auth: AuthSource) {
        let route_matches = match self.auth_route {
            AuthRoute::Auto => true,
            AuthRoute::OAuth => auth.bearer_token().is_some(),
            AuthRoute::ApiKey => auth.api_key().is_some() && auth.bearer_token().is_none(),
        };
        if route_matches {
            if let ProviderClient::Anthropic(client) = &mut self.client {
                client.set_auth(auth);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        session_id: &str,
        model: String,
        enable_tools: bool,
        emit_output: bool,
        allowed_tools: Option<AllowedToolSet>,
        tool_registry: GlobalToolRegistry,
        permission_mode: PermissionMode,
        thinking: Option<api::ThinkingConfig>,
        named_effort: Option<api::EffortLevel>,
        effort_band_ceiling: Option<api::EffortLevel>,
        startup_auth_policy: StartupAuthPolicy,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_auth_resolver(
            session_id,
            model,
            enable_tools,
            emit_output,
            allowed_tools,
            tool_registry,
            permission_mode,
            thinking,
            named_effort,
            effort_band_ceiling,
            startup_auth_policy,
            resolve_and_cache_claude_auth,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_auth_resolver<R>(
        session_id: &str,
        model: String,
        enable_tools: bool,
        emit_output: bool,
        allowed_tools: Option<AllowedToolSet>,
        tool_registry: GlobalToolRegistry,
        _permission_mode: PermissionMode,
        thinking: Option<api::ThinkingConfig>,
        named_effort: Option<api::EffortLevel>,
        effort_band_ceiling: Option<api::EffortLevel>,
        startup_auth_policy: StartupAuthPolicy,
        resolve_auth: R,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        R: FnOnce() -> Result<AuthSource, Box<dyn std::error::Error>>,
    {
        // Only resolve Claude auth when the main model actually routes to
        // Anthropic. A non-Anthropic main (GPT/Gemini/…) never uses this
        // credential, yet resolving it shells out to the macOS keychain — and
        // may block on a token refresh — on the synchronous startup path,
        // needlessly delaying when MCP discovery is spawned. Mirrors
        // `set_model_with_auth_resolver`, which already gates the resolve on the
        // Anthropic provider for mid-session model swaps.
        let auth_route = catalog_auth_route_for_model(&model);
        let auth = if provider_kind_for_model(&model) == ProviderKind::Anthropic
            && auth_route == AuthRoute::Auto
        {
            Some(
                resolve_startup_auth(startup_auth_policy, resolve_auth)
                    .map_err(|error| format!("startup stage resolve-startup-auth: {error:?}"))?,
            )
        } else {
            None
        };
        let client = build_provider_client(session_id, &model, auth_route, auth)
            .map_err(|error| format!("startup stage build-provider-client: {error:?}"))?;

        Ok(Self {
            client,
            session_id: session_id.to_string(),
            model,
            auth_route,
            enable_tools,
            emit_output,
            allowed_tools,
            tool_registry,
            thinking,
            named_effort,
            effort_band_ceiling,
            // Same env gate as the Anthropic client's internal tracer; the
            // shared `stream()` loop uses this to emit request spans / usage
            // for the non-Anthropic providers (which carry no internal tracer).
            session_tracer: api::otlp::session_tracer_from_env(session_id),
        })
    }
}

fn resolve_claude_cli_auth_source() -> Result<AuthSource, Box<dyn std::error::Error>> {
    Ok(resolve_startup_auth_source(|| {
        let cwd = crate::current_cli_cwd().map_err(api::ApiError::from)?;
        let config = ConfigLoader::default_for(&cwd).load().map_err(|error| {
            api::ApiError::Auth(format!("failed to load runtime OAuth config: {error}"))
        })?;
        // No OAuth config in `.zo` is the norm — default to the Claude Code
        // subscription application (the same one `zo login` uses), so an
        // expired saved token can always refresh instead of dying on a
        // "runtime OAuth config is missing" error Claude Code would never show.
        Ok(Some(
            config
                .oauth()
                .cloned()
                .unwrap_or_else(api::claude_code_oauth_config),
        ))
    })?)
}

/// Re-resolve the Claude bearer after a 401 through the OAuth-first chain —
/// keychain (the api layer refreshes an expired keychain token in place, the
/// most common reason a mid-turn bearer lapsed), then the saved `zo login`
/// OAuth (refreshing via the token endpoint), then env credentials as the last
/// resort. `None` when every lane fails. The resolves do nested `block_on`s
/// for the token round-trips, so they run on a blocking thread — never call
/// this chain directly on the async turn task. Also updates the process-wide
/// cached auth so sub-agents inherit the new token.
///
/// Recovery hook for a long turn whose cached bearer lapsed mid-flight: the
/// request path uses a bare snapshot and never refreshes per request, so a
/// crossed expiry otherwise 401s every request until the process restarts.
pub(crate) async fn refresh_claude_oauth() -> Option<AuthSource> {
    tokio::task::spawn_blocking(|| {
        // Recovery path: the memoized keychain session is exactly what just
        // lapsed/401'd, so drop it before re-resolving.
        api::invalidate_claude_code_keychain_cache();
        let resolved = api::resolve_claude_auth_fresh_detailed()?;
        update_cached_claude_auth(
            &resolved.auth,
            cli_auth_origin(resolved.origin),
            resolved.expires_at_ms,
        );
        Some(resolved.auth)
    })
    .await
    .ok()
    .flatten()
}

/// stdout wrapper for the text one-shot path: strips the SGR/ANSI escapes the
/// markdown renderer and tool formatter emit when the output is machine-bound
/// (`NO_COLOR` non-empty, or stdout is not a TTY), so a piped or `NO_COLOR`
/// consumer gets clean text while an interactive terminal keeps the existing
/// colored UX. JSON/NDJSON never reach here — they route to `io::sink`.
pub(crate) struct StripAnsiWriter<W: Write> {
    inner: W,
    strip: bool,
}

impl<W: Write> StripAnsiWriter<W> {
    /// Wrap `inner`; strips ANSI per write when `strip` is true.
    pub(crate) fn new(inner: W, strip: bool) -> Self {
        Self { inner, strip }
    }
}

impl<W: Write> Write for StripAnsiWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.strip {
            // Each caller write is a complete rendered chunk (markdown
            // push/flush, one formatted tool call, or one non-streaming block),
            // so no escape sequence is split across writes and per-chunk
            // stripping is safe.
            let text = String::from_utf8_lossy(buf);
            let stripped = zo_cli::util::ansi::strip_ansi(&text);
            self.inner.write_all(stripped.as_bytes())?;
            Ok(buf.len())
        } else {
            self.inner.write(buf)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl ApiClient for AnthropicRuntimeClient {
    #[allow(clippy::too_many_lines)] // cohesive sync streaming loop
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        // Per-turn wire-model override (refusal → Opus 4.8 fallback). Only the
        // wire model id and its `max_tokens` change; `self.client`/`provider`
        // routing below stays on the bound Anthropic client — the fallback
        // target is Anthropic, same as the refused Fable.
        let selected_model = request
            .model_override
            .as_deref()
            .unwrap_or(&self.model);
        let wire_model = api::wire_model_id(selected_model);
        let tools = self
            .enable_tools
            .then(|| {
                crate::filter_tool_specs(
                    &self.tool_registry,
                    self.allowed_tools.as_ref(),
                )
            });
        // Reconcile history against the advertised toolset (see runtime_bridge):
        // a stored tool_use naming a tool no longer offered would 400 the
        // OpenAI-compatible path after a toolset shrink or model switch.
        let known: std::collections::BTreeSet<String> =
            tools.iter().flatten().map(|def| def.name.clone()).collect();
        let reconciled = runtime::session::reconcile_tool_history(&request.messages, &known);
        // `effort_override` is a floor (deep-gate escalation): raise the budget,
        // never lower it, then derive both the legacy `thinking.budget_tokens`
        // and the adaptive effort level from the floored value.
        let configured_budget = self
            .thinking
            .as_ref()
            .and_then(|t| t.budget_tokens)
            .filter(|&budget| budget > 0);
        let effective_budget =
            api::effort_budget_with_floor(configured_budget, request.effort_override);
        // Reminders ride the newest user message (see runtime_bridge) so the
        // system blocks and cached history stay byte-identical across turns.
        let mut messages = convert_messages(&reconciled);
        runtime::append_wire_reminders(&mut messages, &request.wire_reminders);
        // Rolling conversation-prefix breakpoints, same as runtime_bridge:
        // without them only the system blocks cache and every call re-bills
        // the full transcript as uncached input.
        runtime::mark_conversation_cache_breakpoints(&mut messages);
        let thinking = effective_budget.map_or_else(
            || self.thinking.clone(),
            |b| Some(api::ThinkingConfig::enabled(b)),
        );
        let effort = crate::session::runtime_bridge::effort_with_budget_floor(
            self.named_effort,
            effective_budget,
            self.effort_band_ceiling,
        );
        let effort_band_ceiling = self.effort_band_ceiling;
        let message_request = MessageRequest {
            model: wire_model.clone(),
            max_tokens: crate::max_tokens_for_model(&wire_model),
            messages,
            system: (!request.system_prompt.is_empty()).then(|| {
                let joined = request.system_prompt.join("\n\n");
                crate::session::runtime_bridge::split_system_with_identity(&joined)
            }),
            tools,
            tool_choice: self.enable_tools.then_some(ToolChoice::Auto),
            stream: true,
            thinking,
            output_config: None,
            effort,
            effort_band_ceiling,
        };

        // `stream` is a sync trait method entered from both plain sync entry
        // points (`zo -p ..`) and worker threads of the TUI session
        // runtime — the shared bridge re-enters the ambient runtime when one
        // exists instead of nesting a fresh one (which panics with "Cannot
        // start a runtime from within a runtime").
        let provider = self.provider_kind();
        let model = self.model.clone();
        // The Anthropic client emits its own request spans / usage from inside
        // the api crate; emitting here too would double-count. The neutral seam
        // therefore covers only the non-Anthropic providers, whose clients carry
        // no internal tracer (see `session_tracer` field doc).
        let neutral_tracer = (provider != ProviderKind::Anthropic)
            .then(|| self.session_tracer.clone())
            .flatten();
        let result: Result<Vec<AssistantEvent>, RuntimeError> = run_blocking(async {
            let mut stream = self
                .client
                .stream_message(&message_request)
                .await
                .map_err(|error| RuntimeError::from_api_error(&error))?;
            // Text one-shot stdout is the only branch that writes the
            // markdown-renderer / tool-formatter ANSI to a real fd; strip those
            // escapes when the output is machine-bound (`NO_COLOR` non-empty, or
            // stdout is not a TTY) so a piped/NO_COLOR consumer gets clean text,
            // while an interactive terminal keeps the existing colored UX. The
            // JSON/NDJSON and TUI paths route to `sink` here, so they are
            // unaffected.
            let strip_color = crate::render::no_color_env() || !io::stdout().is_terminal();
            let mut stdout = StripAnsiWriter {
                inner: io::stdout(),
                strip: strip_color,
            };
            let mut sink = io::sink();
            let out: &mut dyn Write = if self.emit_output && !crate::tui_active() {
                &mut stdout
            } else {
                &mut sink
            };
            let renderer = TerminalRenderer::new();
            let mut markdown_stream = MarkdownStreamState::default();
            let mut events = Vec::new();
            // Tool-use blocks keyed by content-block index. Parallel calls
            // (OpenAI Responses backend) interleave across indices, so a single
            // slot would splice their arguments into one malformed call.
            let mut pending_tools: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
            // Thinking (text, signature) accumulated across delta events, keyed by
            // content-block index and flushed on the block stop so headless-path
            // reasoning is stored and replayed verbatim on the next Anthropic
            // request. (`redacted_thinking` needs no buffer — it arrives complete
            // via `push_output_block` on `content_block_start`.)
            let mut pending_thinking: BTreeMap<u32, (String, Option<String>)> = BTreeMap::new();
            let mut saw_stop = false;

            while let Some(event) = stream
                .next_event()
                .await
                .map_err(|error| RuntimeError::from_api_error(&error))?
            {
                match event {
                    ApiStreamEvent::MessageStart(start) => {
                        if let Some(signature) = &start.message.thought_signature {
                            events.push(AssistantEvent::ProviderState(
                                ProviderStateBlob::gemini_thought_signature(signature.clone()),
                            ));
                        }
                        // ChatGPT/Codex reasoning-replay payload — the headless
                        // `zo -p` path streams through here too, so without
                        // this a headless turn's history would drop the same
                        // replay data the TUI path carries.
                        if let Some(replay) = &start.message.reasoning_replay {
                            events.push(AssistantEvent::ReasoningReplay(replay.clone()));
                        }
                        // Streaming `message_start` carries no content blocks; a
                        // non-streaming payload would, and those blocks are
                        // already complete, so emit them directly.
                        for block in start.message.content {
                            match block {
                                OutputContentBlock::ToolUse { id, name, input } => {
                                    events.push(AssistantEvent::ToolUse {
                                        id,
                                        name,
                                        input: input.to_string(),
                                    });
                                }
                                other => {
                                    push_output_block(other, out, &mut events, &mut None, true)?;
                                }
                            }
                        }
                    }
                    ApiStreamEvent::ContentBlockStart(start) => match start.content_block {
                        OutputContentBlock::ToolUse { id, name, input } => {
                            // The streaming start ships an empty-object
                            // placeholder; the real arguments arrive as
                            // input_json_delta. Key by index so parallel calls
                            // accumulate independently.
                            let buffered =
                                if input.as_object().is_some_and(serde_json::Map::is_empty) {
                                    String::new()
                                } else {
                                    input.to_string()
                                };
                            pending_tools.insert(start.index, (id, name, buffered));
                        }
                        other => {
                            push_output_block(other, out, &mut events, &mut None, true)?;
                        }
                    },
                    ApiStreamEvent::ContentBlockDelta(delta) => match delta.delta {
                        ContentBlockDelta::TextDelta { text } => {
                            if !text.is_empty() {
                                if let Some(rendered) = markdown_stream.push(&renderer, &text) {
                                    write!(out, "{rendered}")
                                        .and_then(|()| out.flush())
                                        .map_err(|error| RuntimeError::new(error.to_string()))?;
                                }
                                events.push(AssistantEvent::TextDelta(text));
                            }
                        }
                        ContentBlockDelta::InputJsonDelta { partial_json } => {
                            // Accumulate into the tool block for *this* index so
                            // parallel calls don't splice their arguments.
                            if let Some((_, _, input)) = pending_tools.get_mut(&delta.index) {
                                input.push_str(&partial_json);
                            }
                        }
                        ContentBlockDelta::ThinkingDelta { thinking } => {
                            pending_thinking.entry(delta.index).or_default().0.push_str(&thinking);
                        }
                        ContentBlockDelta::SignatureDelta { signature } => {
                            pending_thinking.entry(delta.index).or_default().1 = Some(signature);
                        }
                    },
                    ApiStreamEvent::ContentBlockStop(stop) => {
                        if let Some(rendered) = markdown_stream.flush(&renderer) {
                            write!(out, "{rendered}")
                                .and_then(|()| out.flush())
                                .map_err(|error| RuntimeError::new(error.to_string()))?;
                        }
                        // A block index is either a thinking block or a tool.
                        if let Some((thinking, signature)) = pending_thinking.remove(&stop.index) {
                            events.push(AssistantEvent::Thinking { thinking, signature });
                        }
                        if let Some((id, name, input)) = pending_tools.remove(&stop.index) {
                            writeln!(out, "\n{}", format_tool_call_start(&name, &input))
                                .and_then(|()| out.flush())
                                .map_err(|error| RuntimeError::new(error.to_string()))?;
                            events.push(AssistantEvent::ToolUse { id, name, input });
                        }
                    }
                    ApiStreamEvent::MessageDelta(delta) => {
                        if let Some(signature) = &delta.delta.thought_signature {
                            events.push(AssistantEvent::ProviderState(
                                ProviderStateBlob::gemini_thought_signature(signature.clone()),
                            ));
                        }
                        if let Some(replay) = &delta.delta.reasoning_replay {
                            events.push(AssistantEvent::ReasoningReplay(replay.clone()));
                        }
                        events.push(AssistantEvent::Usage(delta.usage.token_usage()));
                        // Surface the stop reason so the conversation loop can
                        // tell a natural end from an output-limit truncation
                        // (`max_tokens`) and continue the turn instead of ending
                        // it without a deliverable. The headless `zo -p` path
                        // streams through here, so without this the truncation
                        // recovery never engages.
                        if let Some(reason) = delta
                            .delta
                            .stop_reason
                            .as_deref()
                            .filter(|reason| !reason.is_empty())
                        {
                            events.push(AssistantEvent::StopReason(reason.to_string()));
                        }
                    }
                    ApiStreamEvent::MessageStop(_) => {
                        saw_stop = true;
                        if let Some(rendered) = markdown_stream.flush(&renderer) {
                            write!(out, "{rendered}")
                                .and_then(|()| out.flush())
                                .map_err(|error| RuntimeError::new(error.to_string()))?;
                        }
                        runtime::flush_pending_tool_events(&mut events, &mut pending_tools);
                        events.push(AssistantEvent::MessageStop);
                    }
                }
            }

            runtime::flush_pending_tool_events(&mut events, &mut pending_tools);

            if let ProviderClient::Anthropic(client) = &self.client {
                push_prompt_cache_record(client, &mut events);
            }

            if !saw_stop
                && events.iter().any(|event| {
                    matches!(event, AssistantEvent::TextDelta(text) if !text.is_empty())
                        || matches!(event, AssistantEvent::ToolUse { .. })
                })
            {
                events.push(AssistantEvent::MessageStop);
            }

            if events
                .iter()
                .any(|event| matches!(event, AssistantEvent::MessageStop))
            {
                runtime::record_non_anthropic_prompt_cache_usage(
                    self.session_id.as_str(),
                    provider,
                    &message_request,
                    &mut events,
                );
                return Ok(events);
            }

            let response = self
                .client
                .send_message(&MessageRequest {
                    stream: false,
                    ..message_request.clone()
                })
                .await
                .map_err(|error| RuntimeError::from_api_error(&error))?;
            let mut events = response_to_events(response, out)?;
            if let ProviderClient::Anthropic(client) = &self.client {
                push_prompt_cache_record(client, &mut events);
            }
            runtime::record_non_anthropic_prompt_cache_usage(
                self.session_id.as_str(),
                provider,
                &message_request,
                &mut events,
            );
            Ok(events)
        });

        // Provider-neutral telemetry seam: GPT/Gemini/xAI/Ollama clients carry
        // no internal tracer, so the only place their request outcome and usage
        // are visible is here, at the shared streaming boundary. Mirror what the
        // Anthropic client records internally (request span + usage / error
        // span) so non-Anthropic operators get request-level telemetry too.
        if let Some(tracer) = &neutral_tracer {
            let outcome = match &result {
                Ok(events) => NeutralRequestOutcome::Succeeded {
                    usage: latest_usage_from_events(events),
                },
                Err(error) => NeutralRequestOutcome::Failed {
                    error: error.to_string(),
                    retryable: runtime_error_is_retryable(error),
                },
            };
            emit_neutral_request_telemetry(tracer, provider, &model, &outcome);
        }

        result
    }
}

/// The cumulative token usage carried by the last `Usage` event of a turn.
/// The provider streams a running total on each `message_delta`, so the final
/// one is the turn's authoritative count; absent any usage event (some
/// non-streaming fallbacks) this is the zero usage.
fn latest_usage_from_events(events: &[AssistantEvent]) -> TokenUsage {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            AssistantEvent::Usage(usage) => Some(*usage),
            _ => None,
        })
        .unwrap_or_default()
}

/// Whether a failed request should be tallied as retryable on the neutral
/// telemetry seam, matching the api crate's own retry classification: rate
/// limits and transient transport faults are retryable, everything else
/// (auth, context overflow, schema/protocol, safety, explicit non-retryable)
/// is terminal. An unclassified error is treated as non-retryable.
fn runtime_error_is_retryable(error: &RuntimeError) -> bool {
    matches!(
        error.provider_error_class(),
        Some(api::ProviderErrorClass::RateLimit { .. } | api::ProviderErrorClass::Transient)
    )
}

#[cfg(test)]
mod oauth_refresh_tests {
    use super::{
        AnthropicRuntimeClient, NeutralRequestOutcome, OAUTH_REFRESH_BUFFER_SECS,
        build_runtime_with_thinking_for, emit_neutral_request_telemetry, latest_usage_from_events,
        catalog_provider_for_model, oauth_refresh_needed, provider_kind_for_model,
        refresh_oauth_if_near_expiry, runtime_error_is_retryable,
    };
    use api::{
        AnthropicClient, AuthRoute, AuthSource, InputMessage, MessageRequest, OpenAiCompatClient,
        OpenAiCompatConfig, ProviderClient, ProviderKind,
    };
    use runtime::{AssistantEvent, PermissionMode, RuntimeError, Session, TokenUsage};
    use std::path::PathBuf;
    use tools::GlobalToolRegistry;

    struct EnvVarGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let original = std::env::var_os(key);
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zo-runtime-support-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn refresh_gate_targets_only_near_expiry_saved_oauth() {
        let now = 1_000_000;
        // env-managed auth (API key / bearer) never expires under us.
        assert!(!oauth_refresh_needed(true, Some(now), now));
        assert!(!oauth_refresh_needed(true, Some(now - 100), now));
        // no recorded expiry -> nothing to refresh.
        assert!(!oauth_refresh_needed(false, None, now));
        // comfortably in the future -> no refresh.
        assert!(!oauth_refresh_needed(false, Some(now + 3600), now));
        // just outside the buffer -> no refresh.
        assert!(!oauth_refresh_needed(
            false,
            Some(now + OAUTH_REFRESH_BUFFER_SECS + 1),
            now
        ));
        // exactly at the buffer edge -> refresh.
        assert!(oauth_refresh_needed(
            false,
            Some(now + OAUTH_REFRESH_BUFFER_SECS),
            now
        ));
        // within the buffer -> refresh.
        assert!(oauth_refresh_needed(false, Some(now + 5), now));
        // already expired -> refresh.
        assert!(oauth_refresh_needed(false, Some(now - 10), now));
    }

    #[test]
    fn refresh_oauth_if_near_expiry_skips_non_anthropic_rebuild_for_fresh_saved_token() {
        let _env_lock = crate::test_env_lock();
        let config_home = temp_dir("fresh-google-oauth");
        let _config_home = EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = EnvVarGuard::set("ZO_HOME", None);
        let _adapter_gate = EnvVarGuard::set(api::NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        let _disable_external = EnvVarGuard::set("ZO_DISABLE_EXTERNAL_CREDENTIALS", None);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_secs();
        api::oauth_store::save_google_code_assist_oauth(&core_types::OAuthTokenSet {
            access_token: "stored-fresh-google-token".to_string(),
            refresh_token: Some("google-refresh".to_string()),
            expires_at: Some(now + 3600),
            scopes: Vec::new(),
        })
        .expect("save google oauth");

        let mut client = AnthropicRuntimeClient {
            client: ProviderClient::GeminiCodeAssist(api::GeminiCodeAssistClient::new(
                "live-google-token",
            )),
            session_id: "fresh-google-oauth-test".to_string(),
            model: "gemini-3-flash-preview".to_string(),
            auth_route: AuthRoute::Auto,
            enable_tools: false,
            emit_output: false,
            allowed_tools: None,
            tool_registry: GlobalToolRegistry::builtin(),
            thinking: None,
            named_effort: None,
            effort_band_ceiling: None,
            session_tracer: None,
        };
        let before = format!("{:?}", client.client);
        assert!(before.contains("live-google-token"));

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(refresh_oauth_if_near_expiry(&mut client));

        let after = format!("{:?}", client.client);
        assert_eq!(
            after, before,
            "fresh non-Anthropic OAuth must not rebuild the live client before every turn"
        );
        assert!(!after.contains("stored-fresh-google-token"));
        std::fs::remove_dir_all(config_home).ok();
    }

    #[test]
    fn refresh_oauth_if_near_expiry_rebuilds_non_anthropic_when_saved_token_expired() {
        let _env_lock = crate::test_env_lock();
        let config_home = temp_dir("expired-google-oauth");
        let _config_home = EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = EnvVarGuard::set("ZO_HOME", None);
        let _adapter_gate = EnvVarGuard::set(api::NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        let _disable_external = EnvVarGuard::set("ZO_DISABLE_EXTERNAL_CREDENTIALS", None);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_secs();
        api::oauth_store::save_google_code_assist_oauth(&core_types::OAuthTokenSet {
            access_token: "stored-expired-google-token".to_string(),
            // No refresh token: provider loader reuses the expired bearer without
            // network, making this a deterministic call-site test for rebuild.
            refresh_token: None,
            expires_at: Some(now.saturating_sub(1)),
            scopes: Vec::new(),
        })
        .expect("save expired google oauth");

        let mut client = AnthropicRuntimeClient {
            client: ProviderClient::GeminiCodeAssist(api::GeminiCodeAssistClient::new(
                "live-google-token",
            )),
            session_id: "expired-google-oauth-test".to_string(),
            model: "gemini-3-flash-preview".to_string(),
            auth_route: AuthRoute::Auto,
            enable_tools: false,
            emit_output: false,
            allowed_tools: None,
            tool_registry: GlobalToolRegistry::builtin(),
            thinking: None,
            named_effort: None,
            effort_band_ceiling: None,
            session_tracer: None,
        };

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(refresh_oauth_if_near_expiry(&mut client));

        let after = format!("{:?}", client.client);
        assert!(
            after.contains("stored-expired-google-token"),
            "expired non-Anthropic OAuth should rebuild the live client: {after}"
        );
        std::fs::remove_dir_all(config_home).ok();
    }

    #[test]
    fn provider_routing_detects_explicit_model_families_without_credentials() {
        assert_eq!(
            provider_kind_for_model("opus"),
            api::ProviderKind::Anthropic
        );
        assert_eq!(
            provider_kind_for_model("claude-sonnet-4-6"),
            api::ProviderKind::Anthropic
        );
        assert_eq!(
            provider_kind_for_model("gpt-5.5"),
            api::ProviderKind::OpenAi
        );
        assert_eq!(
            provider_kind_for_model("gpt-5.5-2026-04-23"),
            api::ProviderKind::OpenAi
        );
        assert_eq!(
            provider_kind_for_model("gemini-3.1-pro-preview"),
            api::ProviderKind::Google
        );
        assert_eq!(provider_kind_for_model("grok-3"), api::ProviderKind::Xai);
    }

    #[test]
    fn provider_routing_uses_catalog_for_unfamiliar_model_id() {
        let _env_lock = crate::test_env_lock();
        let config_home = temp_dir("catalog-provider-routing");
        let _config_home = EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = EnvVarGuard::set("ZO_HOME", None);
        let mut catalog = runtime::model_catalog::ModelCatalog::load().unwrap();
        catalog
            .add(
                runtime::model_catalog::CatalogProvider::Google,
                "future-flash-2027",
                "Future Flash",
            )
            .unwrap();

        assert_eq!(
            catalog_provider_for_model("future-flash-2027"),
            Some(api::ProviderKind::Google)
        );
        assert_eq!(
            provider_kind_for_model("future-flash-2027"),
            api::ProviderKind::Google
        );
        catalog
            .add(
                runtime::model_catalog::CatalogProvider::Openai,
                "future-flash-2027",
                "OpenAI Future Flash",
            )
            .unwrap();
        let google = catalog.selection_token(
            runtime::model_catalog::CatalogProvider::Google,
            "future-flash-2027",
        );
        let openai = catalog.selection_token(
            runtime::model_catalog::CatalogProvider::Openai,
            "future-flash-2027",
        );
        assert_eq!(provider_kind_for_model(&google), api::ProviderKind::Google);
        assert_eq!(provider_kind_for_model(&openai), api::ProviderKind::OpenAi);
        assert_eq!(api::wire_model_id(&google), "future-flash-2027");
        assert_eq!(api::wire_model_id(&openai), "future-flash-2027");
        std::fs::remove_dir_all(config_home).ok();
    }

    #[test]
    fn provider_routing_rebuilds_client_when_model_crosses_provider() {
        let _env_lock = crate::test_env_lock();
        let _openai_base = EnvVarGuard::set("OPENAI_BASE_URL", Some("http://localhost:8080/v1"));
        let _openai_key = EnvVarGuard::set("OPENAI_API_KEY", None);
        let mut client = AnthropicRuntimeClient {
            client: ProviderClient::Anthropic(AnthropicClient::from_auth(AuthSource::None)),
            session_id: "provider-routing-test".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            auth_route: AuthRoute::Auto,
            enable_tools: false,
            emit_output: false,
            allowed_tools: None,
            tool_registry: GlobalToolRegistry::builtin(),
            thinking: None,
            named_effort: None,
            effort_band_ceiling: None,
            session_tracer: None,
        };

        client
            .set_model_with_auth_resolver("gpt-5.5", || {
                Ok::<AuthSource, Box<dyn std::error::Error>>(AuthSource::None)
            })
            .expect("OpenAI-compatible client should build from custom base URL");
        assert_eq!(client.model(), "gpt-5.5");
        assert_eq!(client.provider_kind(), api::ProviderKind::OpenAi);

        client
            .set_model_with_auth_resolver("claude-haiku", || {
                Ok::<AuthSource, Box<dyn std::error::Error>>(AuthSource::None)
            })
            .expect("Anthropic client should rebuild when switching back");
        assert_eq!(client.model(), "claude-haiku");
        assert_eq!(client.provider_kind(), api::ProviderKind::Anthropic);
    }

    #[test]
    fn provider_routing_does_not_resolve_anthropic_auth_for_non_anthropic_target() {
        let _env_lock = crate::test_env_lock();
        let _openai_base = EnvVarGuard::set("OPENAI_BASE_URL", Some("http://localhost:8080/v1"));
        let _openai_key = EnvVarGuard::set("OPENAI_API_KEY", None);
        let mut client = AnthropicRuntimeClient {
            client: ProviderClient::Anthropic(AnthropicClient::from_auth(AuthSource::None)),
            session_id: "provider-routing-test".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            auth_route: AuthRoute::Auto,
            enable_tools: false,
            emit_output: false,
            allowed_tools: None,
            tool_registry: GlobalToolRegistry::builtin(),
            thinking: None,
            named_effort: None,
            effort_band_ceiling: None,
            session_tracer: None,
        };

        client
            .set_model_with_auth_resolver("gpt-5.5", || {
                panic!("switching to a non-Anthropic provider must not require Claude auth")
            })
            .expect("OpenAI-compatible client should build from custom base URL");

        assert_eq!(client.model(), "gpt-5.5");
        assert_eq!(client.provider_kind(), api::ProviderKind::OpenAi);
    }

    #[test]
    fn boot_does_not_resolve_anthropic_auth_for_non_anthropic_main() {
        // Booting with a non-Anthropic main model must not resolve Claude auth.
        // That read shells out to the macOS keychain (and can block on a token
        // refresh) on the synchronous startup path, before MCP discovery is even
        // spawned — so a GPT/Gemini main should never pay it.
        let _env_lock = crate::test_env_lock();
        let _openai_base = EnvVarGuard::set("OPENAI_BASE_URL", Some("http://localhost:8080/v1"));
        let _openai_key = EnvVarGuard::set("OPENAI_API_KEY", None);

        let client = AnthropicRuntimeClient::new_with_auth_resolver(
            "boot-auth-gate-test",
            "gpt-5.5".to_string(),
            false,
            false,
            None,
            GlobalToolRegistry::builtin(),
            PermissionMode::ReadOnly,
            None,
            None,
            None,
            super::StartupAuthPolicy::AllowUnauthenticated,
            || panic!("booting a non-Anthropic main must not resolve Claude auth"),
        )
        .expect("OpenAI-compatible client should build from custom base URL");

        assert_eq!(client.model(), "gpt-5.5");
        assert_eq!(client.provider_kind(), api::ProviderKind::OpenAi);
    }

    #[test]
    fn provider_routing_rebuilds_when_same_provider_auth_route_changes() {
        let _env_lock = crate::test_env_lock();
        let config_home = temp_dir("same-provider-auth-route");
        let _config_home = EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = EnvVarGuard::set("ZO_HOME", None);
        let _openai_key = EnvVarGuard::set("OPENAI_API_KEY", Some("test-openai-key"));
        let mut catalog = runtime::model_catalog::ModelCatalog::load().unwrap();
        let row = catalog
            .rows(&[runtime::model_catalog::CatalogProvider::Openai], false)
            .into_iter()
            .find(|row| row.id == "gpt-5.6-sol")
            .unwrap();
        catalog
            .edit_with_auth_route(
                &row,
                row.provider,
                &row.id,
                &row.display_name,
                AuthRoute::ApiKey,
            )
            .unwrap();

        let expires_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_secs()
            + 3600;
        api::oauth_store::save_openai_oauth(&core_types::OpenAiOAuthTokens {
            access_token: "oauth-token".to_string(),
            refresh_token: None,
            expires_at: Some(expires_at),
            account_id: Some("acct".to_string()),
            scopes: Vec::new(),
        })
        .expect("save OpenAI OAuth");
        let oauth_client = ProviderClient::from_provider_kind_with_auth_route(
            ProviderKind::OpenAi,
            AuthRoute::OAuth,
        )
        .expect("saved OpenAI OAuth should build ChatGPT client");
        assert!(matches!(&oauth_client, ProviderClient::ChatGpt(_)));

        let mut client = AnthropicRuntimeClient {
            client: oauth_client,
            session_id: "same-provider-auth-route".to_string(),
            model: "gpt-5.6-sol".to_string(),
            auth_route: AuthRoute::OAuth,
            enable_tools: false,
            emit_output: false,
            allowed_tools: None,
            tool_registry: GlobalToolRegistry::builtin(),
            thinking: None,
            named_effort: None,
            effort_band_ceiling: None,
            session_tracer: None,
        };
        client
            .set_model_with_auth_resolver("gpt-5.6-sol", || {
                panic!("same-provider OpenAI rebuild must not resolve Anthropic auth")
            })
            .unwrap();
        assert_eq!(client.auth_route(), AuthRoute::ApiKey);
        assert!(matches!(client.client, ProviderClient::OpenAi(_)));
        std::fs::remove_dir_all(config_home).ok();
    }

    #[test]
    fn provider_routing_preserves_client_when_provider_is_unchanged() {
        let mut client = AnthropicRuntimeClient {
            client: ProviderClient::OpenAi(OpenAiCompatClient::new(
                "",
                OpenAiCompatConfig::openai(),
            )),
            session_id: "provider-routing-test".to_string(),
            model: "gpt-5".to_string(),
            auth_route: AuthRoute::Auto,
            enable_tools: false,
            emit_output: false,
            allowed_tools: None,
            tool_registry: GlobalToolRegistry::builtin(),
            thinking: None,
            named_effort: None,
            effort_band_ceiling: None,
            session_tracer: None,
        };

        client
            .set_model_with_auth_resolver("gpt-5.5", || {
                panic!("same-provider model switch must not resolve Anthropic auth")
            })
            .expect("same-provider switch should not rebuild credentials");
        assert_eq!(client.model(), "gpt-5.5");
        assert_eq!(client.provider_kind(), api::ProviderKind::OpenAi);
    }

    #[test]
    fn provider_routing_rebuilds_when_openai_kind_switches_to_custom_provider() {
        let _env_lock = crate::test_env_lock();
        let config_home = temp_dir("chatgpt-to-custom-provider");
        let _config_home = EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = EnvVarGuard::set("ZO_HOME", None);
        let _disable_external = EnvVarGuard::set("ZO_DISABLE_EXTERNAL_CREDENTIALS", None);
        let _openai_key = EnvVarGuard::set("OPENAI_API_KEY", None);
        let _custom_providers_env = EnvVarGuard::set(api::CUSTOM_PROVIDERS_ENV, None);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_secs();
        api::oauth_store::save_openai_oauth(&core_types::OpenAiOAuthTokens {
            access_token: "stored-openai-token".to_string(),
            refresh_token: Some("openai-refresh".to_string()),
            expires_at: Some(now + 3600),
            account_id: Some("acct".to_string()),
            scopes: Vec::new(),
        })
        .expect("save openai oauth");
        api::refresh_custom_providers_from_json(
            r#"[{"name":"deepseek","base_url":"https://api.deepseek.com",
                "models":["deepseek-chat","deepseek-reasoner"],"requires_auth":false}]"#,
        )
        .expect("load custom provider catalog");

        let mut client = AnthropicRuntimeClient {
            client: ProviderClient::from_model_with_anthropic_auth("gpt-5.5", None)
                .expect("saved OpenAI OAuth should build ChatGPT client"),
            session_id: "provider-routing-test".to_string(),
            model: "gpt-5.5".to_string(),
            auth_route: AuthRoute::Auto,
            enable_tools: false,
            emit_output: false,
            allowed_tools: None,
            tool_registry: GlobalToolRegistry::builtin(),
            thinking: None,
            named_effort: None,
            effort_band_ceiling: None,
            session_tracer: None,
        };
        assert!(matches!(client.client, ProviderClient::ChatGpt(_)));

        client
            .set_model_with_auth_resolver("deepseek-chat", || {
                panic!("switching to a non-Anthropic custom provider must not resolve Claude auth")
            })
            .expect("custom OpenAI-compatible provider should rebuild away from ChatGPT OAuth");

        assert_eq!(client.model(), "deepseek-chat");
        assert_eq!(client.provider_kind(), api::ProviderKind::OpenAi);
        assert!(
            matches!(client.client, ProviderClient::OpenAi(_)),
            "DeepSeek must use the OpenAI-compatible custom-provider client, not ChatGPT OAuth"
        );

        api::refresh_custom_providers_from_json("[]").expect("clear custom provider catalog");
        let _ = std::fs::remove_dir_all(config_home);
    }

    #[test]
    fn build_runtime_with_thinking_for_discovers_prompt_commands_from_supplied_cwd() {
        // The env lock also pins ZO_DISABLE_KEYCHAIN, keeping runtime
        // construction off the developer's real Claude Code keychain (which
        // would outrank the dummy env key under OAuth-first resolution).
        let _env_lock = crate::test_env_lock();
        let _api_key = EnvVarGuard::set("ANTHROPIC_API_KEY", Some("test-dummy-key"));
        let config_home = temp_dir("config-home");
        let _config_home = EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 temp path")),
        );
        let cwd = temp_dir("workspace");
        let commands_dir = cwd.join(".zo").join("commands");
        std::fs::create_dir_all(&commands_dir).expect("commands dir");
        std::fs::write(
            commands_dir.join("review.md"),
            "---\ndescription: Review from supplied cwd\n---\n\nReview $ARGUMENTS\n",
        )
        .expect("write prompt command");

        let runtime = build_runtime_with_thinking_for(
            &cwd,
            Session::new(),
            "session-cwd-runtime",
            "claude-sonnet-4-6".to_string(),
            Vec::new(),
            false,
            false,
            None,
            PermissionMode::ReadOnly,
            None,
            None,
            None,
        )
        .expect("runtime should build from supplied cwd");

        let command = runtime
            .prompt_command("review")
            .expect("prompt command should be discovered from supplied cwd");
        assert_eq!(
            command.description.as_deref(),
            Some("Review from supplied cwd")
        );
        std::fs::remove_dir_all(cwd).ok();
        std::fs::remove_dir_all(config_home).ok();
    }

    // Keychain blob evaluation (expiry buffer, scope rules, refresh + write
    // back) is owned by `api::providers::anthropic::keychain` and tested there;
    // here only the CLI-side proactive scheduling gate remains.
    #[test]
    fn keychain_proactive_refresh_due_only_inside_buffer() {
        let now_ms: u64 = 1_000_000_000;
        let buffer_ms = OAUTH_REFRESH_BUFFER_SECS * 1000;
        // Unrecorded expiry never schedules a probe.
        assert!(!super::keychain_refresh_due(None, now_ms));
        // Comfortably fresh -> no probe.
        assert!(!super::keychain_refresh_due(
            Some(now_ms + buffer_ms + 1),
            now_ms
        ));
        // At the buffer edge and inside it -> probe (refresh proactively).
        assert!(super::keychain_refresh_due(
            Some(now_ms + buffer_ms),
            now_ms
        ));
        assert!(super::keychain_refresh_due(Some(now_ms + 5_000), now_ms));
        // Already expired -> probe.
        assert!(super::keychain_refresh_due(Some(now_ms - 1), now_ms));
    }

    #[test]
    fn non_anthropic_cache_usage_updates_prompt_cache_stats_and_break_events() {
        let _env_lock = crate::test_env_lock();
        let config_home = temp_dir("non-anthropic-prompt-cache");
        let _config_home = EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = EnvVarGuard::set("ZO_HOME", None);
        let session_id = "openai-cache-stats-test";
        let request = MessageRequest {
            model: "gpt-5.5".to_string(),
            max_tokens: 128,
            messages: vec![InputMessage::user_text("same prompt")],
            system: None,
            tools: None,
            tool_choice: None,
            stream: true,
            thinking: None,
            output_config: None,
            effort: None,
            effort_band_ceiling: None,
        };
        let mut first = vec![AssistantEvent::Usage(TokenUsage {
            input_tokens: 100,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 6_000,
            output_tokens: 10,
        })];
        runtime::record_non_anthropic_prompt_cache_usage(
            session_id,
            ProviderKind::OpenAi,
            &request,
            &mut first,
        );
        assert!(
            !first
                .iter()
                .any(|event| matches!(event, AssistantEvent::PromptCache(_))),
            "first observation updates stats but has no previous read to compare"
        );

        let mut second = vec![AssistantEvent::Usage(TokenUsage {
            input_tokens: 100,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 1_000,
            output_tokens: 10,
        })];
        runtime::record_non_anthropic_prompt_cache_usage(
            session_id,
            ProviderKind::OpenAi,
            &request,
            &mut second,
        );

        assert!(second.iter().any(|event| matches!(
            event,
            AssistantEvent::PromptCache(event) if event.unexpected && event.token_drop == 5_000
        )));
        let stats = api::PromptCache::new(session_id).stats();
        assert_eq!(stats.tracked_requests, 2);
        assert_eq!(stats.total_cache_read_input_tokens, 7_000);
        assert_eq!(stats.last_cache_read_input_tokens, Some(1_000));
        std::fs::remove_dir_all(config_home).ok();
    }

    #[test]
    fn latest_usage_from_events_returns_last_usage_total() {
        // The provider streams a running usage total; the seam must read the
        // final one (not the first, not the default) so a non-Anthropic turn's
        // recorded usage matches the model's actual consumption.
        let events = vec![
            AssistantEvent::Usage(TokenUsage {
                input_tokens: 5,
                output_tokens: 1,
                ..TokenUsage::default()
            }),
            AssistantEvent::TextDelta("hi".to_string()),
            AssistantEvent::Usage(TokenUsage {
                input_tokens: 40,
                output_tokens: 12,
                cache_read_input_tokens: 8,
                ..TokenUsage::default()
            }),
            AssistantEvent::MessageStop,
        ];
        let usage = latest_usage_from_events(&events);
        assert_eq!(usage.input_tokens, 40);
        assert_eq!(usage.output_tokens, 12);
        assert_eq!(usage.cache_read_input_tokens, 8);
        // No usage event at all -> zero (non-streaming fallback path).
        assert_eq!(
            latest_usage_from_events(&[AssistantEvent::MessageStop]),
            TokenUsage::default()
        );
    }

    #[test]
    fn runtime_error_retryable_classification_matches_provider_class() {
        use std::time::Duration;
        assert!(runtime_error_is_retryable(
            &RuntimeError::with_provider_error_class(
                "429",
                api::ProviderErrorClass::account_rate_limit(Some(Duration::from_secs(2))),
            )
        ));
        assert!(runtime_error_is_retryable(
            &RuntimeError::with_provider_error_class("blip", api::ProviderErrorClass::Transient)
        ));
        assert!(!runtime_error_is_retryable(
            &RuntimeError::with_provider_error_class(
                "401",
                api::ProviderErrorClass::AuthExpired
            )
        ));
        // An unclassified error is terminal for telemetry purposes.
        assert!(!runtime_error_is_retryable(&RuntimeError::new("opaque")));
    }

    /// A non-Anthropic turn must surface a request span *and* usage analytics
    /// through the provider-neutral seam — the regression this group fixes
    /// (GPT/Gemini previously got zero request-level telemetry because the
    /// tracer was wired only into the Anthropic client).
    #[test]
    fn non_anthropic_turn_records_request_span_and_usage_via_neutral_seam() {
        use std::sync::Arc;

        let sink = Arc::new(api::MemoryTelemetrySink::default());
        let tracer = api::SessionTracer::new("gpt-session", sink.clone());

        // Drive the exact success path `stream()` takes for a non-Anthropic
        // provider: derive usage from the turn's events, then emit.
        let events = vec![
            AssistantEvent::TextDelta("answer".to_string()),
            AssistantEvent::Usage(TokenUsage {
                input_tokens: 100,
                output_tokens: 25,
                ..TokenUsage::default()
            }),
            AssistantEvent::MessageStop,
        ];
        let outcome = NeutralRequestOutcome::Succeeded {
            usage: latest_usage_from_events(&events),
        };
        emit_neutral_request_telemetry(
            &tracer,
            api::ProviderKind::OpenAi,
            "gpt-5.5",
            &outcome,
        );

        let recorded = sink.events();
        // A request span was opened (started) and closed (succeeded).
        assert!(
            recorded.iter().any(|event| matches!(
                event,
                api::TelemetryEvent::HttpRequestStarted { method, .. } if method == "POST"
            )),
            "non-Anthropic turn must emit an api_request_started span: {recorded:?}"
        );
        assert!(
            recorded.iter().any(|event| matches!(
                event,
                api::TelemetryEvent::HttpRequestSucceeded { status: 200, .. }
            )),
            "non-Anthropic turn must emit an api_request success span: {recorded:?}"
        );
        // The usage analytics event carries the model's real token totals.
        let usage_event = recorded.iter().find_map(|event| match event {
            api::TelemetryEvent::Analytics(analytics) if analytics.action == "message_usage" => {
                Some(analytics)
            }
            _ => None,
        });
        let usage_event = usage_event.expect("non-Anthropic turn must emit message_usage analytics");
        assert_eq!(
            usage_event.properties.get("total_tokens"),
            Some(&serde_json::Value::from(125u32))
        );
        assert_eq!(
            usage_event.properties.get("model"),
            Some(&serde_json::Value::String("gpt-5.5".to_string()))
        );
    }

    #[test]
    fn neutral_seam_records_failure_span_with_retryable_flag() {
        use std::sync::Arc;
        use std::time::Duration;

        let sink = Arc::new(api::MemoryTelemetrySink::default());
        let tracer = api::SessionTracer::new("gemini-session", sink.clone());

        let error = RuntimeError::with_provider_error_class(
            "429 rate limited",
            api::ProviderErrorClass::account_rate_limit(Some(Duration::from_secs(3))),
        );
        let outcome = NeutralRequestOutcome::Failed {
            error: error.to_string(),
            retryable: runtime_error_is_retryable(&error),
        };
        emit_neutral_request_telemetry(
            &tracer,
            api::ProviderKind::Google,
            "gemini-3-flash-preview",
            &outcome,
        );

        let recorded = sink.events();
        assert!(
            recorded.iter().any(|event| matches!(
                event,
                api::TelemetryEvent::HttpRequestFailed { retryable: true, .. }
            )),
            "rate-limited non-Anthropic request must emit a retryable api_error span: {recorded:?}"
        );
        // A failure never fabricates a usage analytics event.
        assert!(
            !recorded.iter().any(|event| matches!(
                event,
                api::TelemetryEvent::Analytics(analytics) if analytics.action == "message_usage"
            )),
            "a failed request must not record usage: {recorded:?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn startup_auth_policy_requires_auth_by_default() {
        let result = resolve_startup_auth(StartupAuthPolicy::Require, || {
            Err(Box::new(io::Error::other("invalid_grant")) as Box<dyn std::error::Error>)
        });

        assert!(result.is_err());
    }

    #[test]
    fn startup_auth_policy_allows_interactive_unauthenticated_startup() {
        let result = resolve_startup_auth(StartupAuthPolicy::AllowUnauthenticated, || {
            Err(Box::new(io::Error::other("invalid_grant")) as Box<dyn std::error::Error>)
        })
        .expect("interactive startup should fall back to unauthenticated client");

        assert!(matches!(result, AuthSource::None));
    }

    #[test]
    fn startup_auth_policy_preserves_successful_auth() {
        let result = resolve_startup_auth(StartupAuthPolicy::AllowUnauthenticated, || {
            Ok(AuthSource::None)
        })
        .expect("successful auth should pass through");

        assert!(matches!(result, AuthSource::None));
    }
}

#[cfg(test)]
mod headless_permission_tests {
    use super::TerminalPermissionPrompter;
    use runtime::permission::{
        PermissionDecision, PermissionPrompter, PermissionRequest, RiskLevel,
    };
    use runtime::PermissionMode;

    fn sample_request() -> PermissionRequest {
        PermissionRequest {
            tool: "bash".to_string(),
            input_summary: "ls -la".to_string(),
            input_hash: "0".repeat(64),
            reasoning: "tool 'bash' requires approval to run while mode is read-only".to_string(),
            choices: Vec::new(),
            risk_level: RiskLevel::Medium,
        }
    }

    fn decide_blocking(
        prompter: &TerminalPermissionPrompter,
        request: PermissionRequest,
    ) -> PermissionDecision {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(prompter.decide(request))
            .expect("terminal prompter never errors")
    }

    /// Contract (a): a non-interactive one-shot must resolve permission
    /// requests to a safe deny without reading stdin (no blocking prompt).
    /// If `decide` fell through to the interactive branch it would block on
    /// `stdin.read_line`; returning at all proves the short-circuit fires.
    #[test]
    fn non_interactive_prompter_denies_without_blocking() {
        let prompter =
            TerminalPermissionPrompter::non_interactive_for_test(PermissionMode::ReadOnly);
        assert_eq!(
            decide_blocking(&prompter, sample_request()),
            PermissionDecision::Deny,
            "non-interactive session must never auto-approve",
        );
    }

    /// Contract (d): the deny holds regardless of how permissive the active
    /// mode is — mode rules are the policy's job; residual prompts with no
    /// human attached always resolve to deny.
    #[test]
    fn non_interactive_deny_holds_in_permissive_mode() {
        let prompter =
            TerminalPermissionPrompter::non_interactive_for_test(PermissionMode::DangerFullAccess);
        assert_eq!(
            decide_blocking(&prompter, sample_request()),
            PermissionDecision::Deny,
            "must deny in non-interactive session",
        );
    }

    /// Contract (b): the interactive predicate is true for exactly one point of
    /// the 3-fd TTY cube — all three terminals. Every other combination (any
    /// single redirected fd, including the `zo -p ... > out.txt` case where
    /// only stdout is redirected) must be non-interactive so automation never
    /// gets a blocking prompt.
    #[test]
    fn interactive_only_when_all_three_fds_are_terminals() {
        for stdin_tty in [false, true] {
            for stdout_tty in [false, true] {
                for stderr_tty in [false, true] {
                    let expected = stdin_tty && stdout_tty && stderr_tty;
                    assert_eq!(
                        super::interactive_terminal(stdin_tty, stdout_tty, stderr_tty),
                        expected,
                        "stdin={stdin_tty} stdout={stdout_tty} stderr={stderr_tty} \
                         must be interactive only when all three are TTYs",
                    );
                }
            }
        }
    }

    /// Contract (c): the regression at the heart of this fix — stdout redirected
    /// while stdin and stderr stay TTYs (`zo -p ... > out.txt`) must NOT be
    /// interactive. Under the old `stdin && stderr` rule this was true and the
    /// run would draw a prompt on stderr and block on `stdin.read_line`.
    #[test]
    fn redirected_stdout_is_not_interactive() {
        assert!(
            !super::interactive_terminal(true, false, true),
            "stdin+stderr TTY but stdout redirected must be non-interactive",
        );
    }

    /// Contract (c) end-to-end: the non-interactive state a redirected-stdout
    /// run produces must deny without ever reading stdin — the deny arm
    /// returns before the blocking-pool prompt is even spawned.
    #[test]
    fn non_interactive_state_auto_denies_without_reading_stdin() {
        let prompter =
            TerminalPermissionPrompter::non_interactive_for_test(PermissionMode::ReadOnly);
        assert_eq!(
            decide_blocking(&prompter, sample_request()),
            PermissionDecision::Deny,
            "redirected-stdout automation must never auto-approve",
        );
    }
}

/// Regression coverage for the text one-shot stdout color seam: the same
/// assistant-markdown and tool-formatting ANSI must survive to a TTY without
/// `NO_COLOR`, and must be fully stripped for a machine-bound sink.
#[cfg(test)]
mod headless_color_tests {
    use super::StripAnsiWriter;
    use std::io::Write;

    /// An input mixing the two real producers on this path: a
    /// markdown-renderer SGR run and a `format_tool_call_start`-style
    /// colored tool header.
    fn ansi_sample() -> &'static str {
        "\x1b[1mhello\x1b[0m world\n\x1b[38;5;245m╭─ \x1b[1;36mbash\x1b[0m\n"
    }

    /// Plain matrix: when the seam decides to strip (`NO_COLOR` or non-TTY),
    /// every escape is removed and only the visible text reaches stdout.
    #[test]
    fn strip_true_removes_all_ansi() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut writer = StripAnsiWriter {
                inner: &mut buf,
                strip: true,
            };
            write!(writer, "{}", ansi_sample()).unwrap();
            writer.flush().unwrap();
        }
        let out = String::from_utf8(buf).unwrap();
        assert!(
            !out.contains('\u{1b}'),
            "no ESC byte must survive stripping: {out:?}"
        );
        assert_eq!(out, "hello world\n╭─ bash\n", "visible text is preserved");
    }

    /// Colored matrix: when the seam keeps colors (TTY without `NO_COLOR`),
    /// the bytes pass through untouched so the interactive UX is preserved.
    #[test]
    fn strip_false_preserves_bytes_exactly() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut writer = StripAnsiWriter {
                inner: &mut buf,
                strip: false,
            };
            write!(writer, "{}", ansi_sample()).unwrap();
            writer.flush().unwrap();
        }
        assert_eq!(
            buf,
            ansi_sample().as_bytes(),
            "colored path is byte-identical to input"
        );
    }

    /// The stripping wrapper must report the whole input as consumed even
    /// though it writes fewer bytes, so `write!`/`write_all` do not loop or
    /// error on the shrunk output.
    #[test]
    fn strip_reports_full_input_consumed() {
        let mut buf: Vec<u8> = Vec::new();
        let mut writer = StripAnsiWriter {
            inner: &mut buf,
            strip: true,
        };
        let input = ansi_sample().as_bytes();
        let n = writer.write(input).unwrap();
        assert_eq!(n, input.len(), "must claim the full buffer as written");
    }

    /// A chunk with no escapes is untouched under either mode — guards the
    /// `strip_ansi` fast path and the pass-through branch alike.
    #[test]
    fn plain_text_unchanged_both_modes() {
        for strip in [true, false] {
            let mut buf: Vec<u8> = Vec::new();
            {
                let mut writer = StripAnsiWriter {
                    inner: &mut buf,
                    strip,
                };
                writeln!(writer, "plain, no escapes").unwrap();
            }
            assert_eq!(String::from_utf8(buf).unwrap(), "plain, no escapes\n");
        }
    }
}
