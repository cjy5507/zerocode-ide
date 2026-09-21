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

use crate::conversation_support::permission_policy;
use crate::render::{MarkdownStreamState, TerminalRenderer};
use crate::response_events::{push_output_block, push_prompt_cache_record, response_to_events};
use crate::tool_formatting::format_tool_call_start;
use crate::{
    AllowedToolSet, BuiltRuntime, CliToolExecutor, DisallowedToolSet, RuntimePluginState,
    cli_tool_executor::{
        SharedTurnAllowedTools, check_disallowed_tools, parse_tool_input_json,
    },
    session::build_runtime_plugin_state_with_loader,
};

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

const PROFILE_BOOT_ENV: &str = "ZO_PROFILE_BOOT";
const PROFILE_BOOT_COMPONENTS_ENV: &str = "ZO_PROFILE_BOOT_COMPONENTS";
const PROFILE_PREFIX_ENV: &str = "ZO_PROFILE_PREFIX";

fn profile_env_enabled(name: &str) -> bool {
    std::env::var_os(name).is_some()
}

pub(crate) fn log_boot_stage(label: &str, started: std::time::Instant) {
    if profile_env_enabled(PROFILE_BOOT_ENV) {
        eprintln!("[BOOT-STAGE] {label}={}us", started.elapsed().as_micros());
    }
}

/// Break the opaque plugin-state build into its two independently callable
/// read-only inputs when explicitly profiling. This probe runs only under the
/// capture script's environment switch; ordinary startup never repeats work.
fn profile_boot_components(
    cwd: &Path,
    loader: &ConfigLoader,
    runtime_config: &runtime::RuntimeConfig,
    model: &str,
) {
    if !profile_env_enabled(PROFILE_BOOT_COMPONENTS_ENV) {
        return;
    }

    let plugin_started = std::time::Instant::now();
    let plugin_result = (|| {
        let registry = crate::build_plugin_manager(cwd, loader, runtime_config).plugin_registry()?;
        let _ = registry.aggregated_hooks()?;
        let _ = registry.aggregated_tools()?;
        Ok::<_, plugins::PluginError>(())
    })();
    eprintln!(
        "[BOOT-COMPONENT] plugin-catalog={}us ok={}",
        plugin_started.elapsed().as_micros(),
        plugin_result.is_ok()
    );

    let retriever_started = std::time::Instant::now();
    let retriever_loaded = runtime::load_memory_retriever(cwd, Some(model)).is_some();
    eprintln!(
        "[BOOT-COMPONENT] retriever-load={}us loaded={retriever_loaded}",
        retriever_started.elapsed().as_micros()
    );
}

fn estimated_tokens_for_chars(chars: usize) -> usize {
    chars.div_ceil(4)
}

fn serialized_tool_chars(
    name: &str,
    description: Option<&str>,
    input_schema: &serde_json::Value,
) -> usize {
    serde_json::to_string(&serde_json::json!({
        "name": name,
        "description": description,
        "input_schema": input_schema,
    }))
    .map_or(0, |wire| wire.chars().count())
}

fn profile_prefix_inventory(
    tool_registry: &GlobalToolRegistry,
    allowed_tools: Option<&AllowedToolSet>,
    system_prompt: &[String],
) {
    if !profile_env_enabled(PROFILE_PREFIX_ENV) {
        return;
    }

    for (index, section) in system_prompt.iter().enumerate() {
        let chars = section.chars().count();
        let heading = section.lines().next().unwrap_or_default();
        eprintln!(
            "[PREFIX-SECTION] {}",
            serde_json::json!({
                "index": index,
                "heading": heading,
                "chars": chars,
                "estimated_tokens": estimated_tokens_for_chars(chars),
            })
        );
    }

    for definition in tool_registry.definitions(allowed_tools) {
        let chars = serialized_tool_chars(
            &definition.name,
            definition.description.as_deref(),
            &definition.input_schema,
        );
        eprintln!(
            "[PREFIX-TOOL] {}",
            serde_json::json!({
                "name": definition.name,
                "chars": chars,
                "estimated_tokens": estimated_tokens_for_chars(chars),
            })
        );
    }
}

fn profile_mcp_inventory(
    runtime_config: &runtime::RuntimeConfig,
    tool_registry: &GlobalToolRegistry,
) {
    if !profile_env_enabled(PROFILE_PREFIX_ENV) {
        return;
    }

    let definitions = tool_registry.runtime_tool_definitions();
    for server in runtime_config.mcp().servers().keys() {
        let prefix = runtime::mcp_tool_prefix(server);
        let mut chars = 0usize;
        let mut tools = 0usize;
        for definition in definitions.iter().filter(|tool| tool.name.starts_with(&prefix)) {
            chars = chars.saturating_add(serialized_tool_chars(
                &definition.name,
                definition.description.as_deref(),
                &definition.input_schema,
            ));
            tools = tools.saturating_add(1);
        }
        eprintln!(
            "[PREFIX-MCP] {}",
            serde_json::json!({
                "server": server,
                "tools": tools,
                "chars": chars,
                "estimated_tokens": estimated_tokens_for_chars(chars),
                "wire_tools": definitions.iter().filter(|tool| {
                    tool.name.starts_with(&prefix)
                        && tool_registry
                            .definitions(None)
                            .iter()
                            .any(|wire| wire.name == tool.name)
                }).count(),
            })
        );
    }
}

/// The thread-safe execution path installed on every live conversation.
///
/// Keep this as a named seam rather than an anonymous closure: the runtime
/// sends every ordinary tool call here (plain/JSON, TUI, and IDE alike), while
/// [`CliToolExecutor`] is only the fallback when no concurrent dispatcher is
/// installed. Executor-level tests must exercise the production seam, not the
/// fallback beside it.
#[allow(clippy::too_many_arguments)] // one gate per argument, in the order the serial executor checks them
fn dispatch_concurrent_tool(
    tool_registry: &GlobalToolRegistry,
    allowed_tools: Option<&AllowedToolSet>,
    disallowed_tools: Option<&DisallowedToolSet>,
    turn_allowed_tools: &SharedTurnAllowedTools,
    mcp_state: Option<&std::sync::Arc<std::sync::Mutex<crate::session::RuntimeMcpState>>>,
    remote_mcp: Option<&crate::remote_mcp::RemoteMcp>,
    tool_name: &str,
    input: &str,
) -> Result<String, runtime::ToolError> {
    // `CapabilityInvoke` is an address, not a privilege. Unwrap before every
    // session/turn gate and re-enter this production dispatcher with the inner
    // name, exactly like the serial fallback does. The live runtime routes all
    // ordinary calls through this seam once `ConcurrentDispatchFn` is installed;
    // handling the wrapper only in `CliToolExecutor` therefore left plain/JSON,
    // TUI, and IDE sessions advertising a door they never actually reached.
    if tool_name == tools::capability::CAPABILITY_INVOKE {
        let value = parse_tool_input_json(tool_name, input)?;
        let call = tools::capability::unwrap_capability_invoke(&value)
            .map_err(runtime::ToolError::new)?;
        return dispatch_concurrent_tool(
            tool_registry,
            allowed_tools,
            disallowed_tools,
            turn_allowed_tools,
            mcp_state,
            remote_mcp,
            &call.name,
            &call.input,
        );
    }
    check_disallowed_tools(disallowed_tools, tool_name)?;
    if tool_name == crate::autonomy::wakeup::TOOL_NAME {
        let value = parse_tool_input_json(tool_name, input)?;
        return crate::autonomy::wakeup::execute(value);
    }
    if let Some(allowed) = allowed_tools {
        if !allowed.contains(tool_name) {
            return Err(runtime::ToolError::new(format!(
                "tool `{tool_name}` is not enabled by the current --allowedTools setting"
            )));
        }
    }
    crate::cli_tool_executor::check_turn_allowed_tools(turn_allowed_tools, tool_name)?;
    let input = if tool_name == "TaskList" && input.trim().is_empty() {
        "{}"
    } else {
        input
    };
    let value = parse_tool_input_json(tool_name, input)?;
    if tool_registry.has_runtime_tool(tool_name) {
        // A pane child's parent answers first (t-2513 §2.1): its MCP tools
        // are the parent's, and the parent's runtime is where they live.
        if let Some(remote) = remote_mcp {
            return remote.dispatch(tool_name, value);
        }
        let Some(mcp_state) = mcp_state else {
            return Err(runtime::ToolError::new(format!(
                "runtime tool `{tool_name}` unavailable without MCP servers"
            )));
        };
        let mut state = mcp_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Same meta-tool handling as the serial CliToolExecutor path, so
        // `ListMcpResourcesTool` & friends don't hit `call_tool` and fail as
        // "unknown MCP tool" when dispatched concurrently/long-running.
        state.dispatch_runtime_tool(tool_name, value)
    } else {
        tool_registry
            .execute(tool_name, &value)
            .map_err(runtime::ToolError::from)
    }
}

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

/// The session-wide deny set from the launch flags (`--no-spawn`,
/// `--disallowedTools`), or `None` when nothing was turned off.
///
/// Read by [`crate::filter_tool_specs`] so a turned-off tool is not
/// **advertised**. It is safe to consult a process-global here precisely
/// because these are launch flags: the set is fixed for the session, and the
/// advertised tool list must not change between two requests of one
/// conversation or the cached prefix behind it is re-billed.
pub(crate) fn disallowed_tool_names() -> Option<DisallowedToolSet> {
    cli_runtime_overrides().disallowed_tools
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_runtime_with_thinking(
    cwd: &Path,
    // `--mcp-config` 로 지정된 설정 파일. `None` 이면 워크스페이스 기본 설정.
    // 이 인자가 생기기 전에는 호출부가 이 함수의 앞부분만 복사해 갔고, 그
    // 복사본에는 아래 네 부수효과와 단계 태그 에러가 빠져 있었다.
    mcp_config: Option<&Path>,
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
    remote_mcp: Option<runtime::subagent_panes::McpRoute>,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    let runtime_started = std::time::Instant::now();
    let loader = match mcp_config {
        Some(path) => ConfigLoader::default_for(cwd).with_mcp_config(path),
        None => ConfigLoader::default_for(cwd),
    };
    // Stage-tagged like the LiveCli constructor (see `stage_tag` there): a
    // startup error must name the stage it escaped from or a rare failure
    // (the 2026-08-10 `Io(EINVAL)` flake) dies unattributable.
    let config_started = std::time::Instant::now();
    let runtime_config = loader
        .load()
        .map_err(|error| format!("startup stage config-load: {error:?}"))?;
    log_boot_stage("config-load", config_started);
    profile_boot_components(cwd, &loader, &runtime_config, &model);
    apply_custom_providers_env(&runtime_config);
    apply_model_wire_env();
    // Before the memory retriever is built below: the corpus scan resolves the
    // vault from the environment, so a settings-only vault has to be published
    // by now or the first session of a fresh install would recall nothing.
    runtime::second_brain::publish_vault_from_config(&runtime_config);
    spawn_session_retention_cleanup(&runtime_config);
    let plugin_state_started = std::time::Instant::now();
    let runtime_plugin_state =
        build_runtime_plugin_state_with_loader(cwd, &loader, &runtime_config, tasks, Some(&model))
            .map_err(|error| format!("startup stage plugin-state: {error:?}"))?;
    log_boot_stage("plugin-state", plugin_state_started);
    profile_mcp_inventory(&runtime_config, &runtime_plugin_state.tool_registry);
    let built = build_runtime_with_plugin_state(
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
        remote_mcp,
    )?;
    log_boot_stage("runtime-ready", runtime_started);
    Ok(built)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
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
    remote_mcp: Option<runtime::subagent_panes::McpRoute>,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    let RuntimePluginState {
        feature_config,
        tool_registry,
        plugin_registry,
        memory_retriever,
        recall_seat,
        mcp_state,
        lsp_state,
    } = runtime_plugin_state;
    // A pane child's parent-session MCP tools (t-2513 §2.1): advertised as
    // this session's runtime tools, and answered by the parent over its
    // channel — the same schemas and the same runtime an inline child gets
    // through the in-process passthrough.
    let remote_mcp = remote_mcp.and_then(|route| {
        if let Err(error) = tool_registry.set_runtime_tools(crate::remote_mcp::RemoteMcp::definitions(&route)) {
            eprintln!("[zo] the parent's MCP tools could not be registered: {error}");
        }
        crate::remote_mcp::RemoteMcp::from_route(&route, &runtime::subagent_panes::Limits::load())
            .map(std::sync::Arc::new)
    });
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
    let plugin_initialize_started = std::time::Instant::now();
    plugin_registry
        .initialize()
        .map_err(|error| format!("startup stage plugin-initialize: {error:?}"))?;
    log_boot_stage("plugin-initialize", plugin_initialize_started);
    let permission_started = std::time::Instant::now();
    let policy = permission_policy(permission_mode, &feature_config, &tool_registry)
        .map_err(|error| format!("startup stage permission-policy: {error:?}"))?;
    log_boot_stage("permission-policy", permission_started);
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
    )
    .with_remote_mcp(remote_mcp.clone());
    // Shared per-turn allowed-tools slot: the concurrent dispatch closure below
    // must enforce the same one-turn restriction as the serial executor, or
    // concurrency-safe tools would bypass the gate entirely.
    let turn_allowed_slot = tool_executor.shared_turn_allowed_tools();
    profile_prefix_inventory(&tool_registry, allowed_tools.as_ref(), &system_prompt);
    let client_started = std::time::Instant::now();
    let client = build_claude_runtime_client(
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
            )
    .map_err(|error| format!("startup stage claude-runtime-client: {error:?}"))?;
    log_boot_stage("  client-only", client_started);
    let runtime_started = std::time::Instant::now();
    let mut runtime = ConversationRuntime::new_with_context_window(
        session,
        client,
        tool_executor,
        policy,
        system_prompt,
        &feature_config,
        effective_context_window,
    );
    log_boot_stage("  runtime-only", runtime_started);
    log_boot_stage("runtime-client", client_started);
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
    runtime.set_recall_seat(recall_seat);
    // Wire up parallel tool execution: concurrency-safe tools (Read,
    // Glob, Grep, …) will run via spawn_blocking instead of serially.
    let dispatch_registry = tool_registry.clone();
    let dispatch_allowed = allowed_tools;
    let dispatch_disallowed = disallowed_tools;
    let dispatch_mcp = mcp_state.clone();
    let dispatch_remote_mcp = remote_mcp;
    let concurrent_dispatch: ConcurrentDispatchFn =
        std::sync::Arc::new(move |tool_name: &str, input: &str| {
            dispatch_concurrent_tool(
                &dispatch_registry,
                dispatch_allowed.as_ref(),
                dispatch_disallowed.as_ref(),
                &turn_allowed_slot,
                dispatch_mcp.as_ref(),
                dispatch_remote_mcp.as_deref(),
                tool_name,
                input,
            )
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
        plugin_registry,
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
    /// A credential file under the IDE-provided `CLAUDE_CONFIG_DIR`.
    ManagedFile,
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
    /// Identity of the managed file when [`AuthOrigin::ManagedFile`] answered.
    managed_file_stamp: Option<api::ManagedCredentialsStamp>,
}

static CACHED_AUTH: OnceLock<Mutex<Option<CachedClaudeAuth>>> = OnceLock::new();

fn resolve_and_cache_claude_auth() -> Result<AuthSource, Box<dyn std::error::Error>> {
    let cache = CACHED_AUTH.get_or_init(|| Mutex::new(None));
    let mut guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Cache hit. Env/keychain origins are final. A managed-file origin is final
    // only while its cheap metadata stamp is unchanged. A `Fallback` memo means
    // the keychain was unusable at startup — retry it now so recovery needs no
    // restart.
    if let Some(cached) = guard.as_ref() {
        match cached.origin {
            AuthOrigin::Env | AuthOrigin::Keychain => return Ok(cached.auth.clone()),
            AuthOrigin::ManagedFile => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
                    .unwrap_or(u64::MAX);
                if managed_file_cache_current(cached)
                    && !keychain_refresh_due(cached.expires_at_ms, now_ms)
                {
                    return Ok(cached.auth.clone());
                }
                api::invalidate_claude_code_keychain_cache();
            }
            AuthOrigin::Fallback => {
                let managed_file_stamp = api::managed_claude_credentials_stamp();
                if let Some(session) = api::read_claude_code_keychain_session() {
                    let recovered = AuthSource::BearerToken(session.access_token);
                    *guard = Some(CachedClaudeAuth {
                        auth: recovered.clone(),
                        origin: if managed_file_stamp.is_some() {
                            AuthOrigin::ManagedFile
                        } else {
                            AuthOrigin::Keychain
                        },
                        expires_at_ms: session.expires_at_ms,
                        managed_file_stamp,
                    });
                    AuthSource::cache_resolved(&recovered);
                    return Ok(recovered);
                }
                return Ok(cached.auth.clone());
            }
        }
    }

    let resolved = resolve_fresh_claude_auth()?;
    let auth = resolved.auth.clone();
    AuthSource::cache_resolved(&auth);
    *guard = Some(resolved);
    Ok(auth)
}

/// 이 자격이 어디서 왔는가 — 사람이 `/status` 에서, 창이 상태바에서 읽는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountOrigin {
    /// IDE 가 판에 건넨 계정 폴더(런치 env 또는 `auth.reload`).
    IdeManaged,
    /// `zo login` 으로 이 도구가 직접 받은 것.
    OwnLogin,
    /// 이 기계의 Claude Code 키체인 세션.
    Keychain,
    /// 판이 태어날 때의 환경 변수 — Claude 는 API 키, OpenAI 는 창이 판에
    /// 건넨 `CODEX_HOME`.
    Env,
}

impl AccountOrigin {
    /// 프레임과 카드가 쓰는 이름.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::IdeManaged => "ide-managed",
            Self::OwnLogin => "own-login",
            Self::Keychain => "keychain",
            Self::Env => "env",
        }
    }
}

/// 한 프로바이더의 계정 — 라벨과 출처.
///
/// 라벨은 창이 `auth.reload` 에 실어 보낸 표시 이름이다. 창이 말해 주지 않았으면
/// `None`: 이메일이나 계정 id 를 여기서 지어내지 않는다(설계 §2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountFacts {
    pub provider: &'static str,
    pub label: Option<String>,
    pub origin: AccountOrigin,
}

/// 한 프로바이더가 지금 어느 계정으로 말하는가. 아무 자격도 못 찾으면 `None` —
/// 모르는 것을 "로그인됨" 으로 그리지 않는다.
#[must_use]
pub fn account_facts(provider: api::ManagedProvider) -> Option<AccountFacts> {
    let label = api::managed_account::label(provider);
    let origin = match provider {
        api::ManagedProvider::Anthropic => {
            if api::managed_account::claude_config_dir().is_some() {
                Some(AccountOrigin::IdeManaged)
            } else {
                match api::latest_claude_auth_origin()? {
                    api::ClaudeAuthOrigin::ManagedFile => Some(AccountOrigin::IdeManaged),
                    api::ClaudeAuthOrigin::Keychain => Some(AccountOrigin::Keychain),
                    api::ClaudeAuthOrigin::SavedOauth => Some(AccountOrigin::OwnLogin),
                    api::ClaudeAuthOrigin::Env => Some(AccountOrigin::Env),
                }
            }
        }
        // 어디서 빌린 계정인지 그대로 말한다: 창이 고르거나 창에서 찾아낸
        // 계정은 관리, 판이 태어날 때의 `CODEX_HOME` 은 환경, 아무것도 없으면
        // 제 로그인. 「자주 만료된다」 는 신고는 이 칸이 비어 있어서 아무도
        // 어느 계정으로 말하는지 볼 수 없었기 때문이다(t-5777).
        api::ManagedProvider::OpenAi => match api::oauth_store::openai_oauth_source()? {
            api::oauth_store::OpenAiAuthSource::CodexHome(
                api::managed_account::CodexHomeSource::Channel
                | api::managed_account::CodexHomeSource::IdeManaged,
            ) => Some(AccountOrigin::IdeManaged),
            api::oauth_store::OpenAiAuthSource::CodexHome(
                api::managed_account::CodexHomeSource::Env,
            ) => Some(AccountOrigin::Env),
            api::oauth_store::OpenAiAuthSource::OwnLogin => Some(AccountOrigin::OwnLogin),
        },
        // Google 은 창과 zo 가 **한 파일**을 나눠 쓴다: 창의 로그인이 zo 의
        // credentials.json 에 직접 쓰므로 "IDE 관리" 와 "자기 로그인" 을 파일만
        // 보고는 가를 수 없다. 창이 이름을 실어 보냈을 때만 관리로 말한다.
        api::ManagedProvider::Google => {
            if label.is_some() {
                Some(AccountOrigin::IdeManaged)
            } else if api::google_code_assist_oauth_present() {
                Some(AccountOrigin::OwnLogin)
            } else {
                None
            }
        }
    }?;
    Some(AccountFacts {
        provider: provider.slug(),
        label,
        origin,
    })
}

/// 이 모델이 말을 거는 계정의 갈래. 계정 저장소가 없는 프로바이더
/// (xAI·Ollama)는 `None` 이고, 그 세션의 카드에는 계정 줄이 없다.
#[must_use]
pub fn managed_provider_for(kind: api::ProviderKind) -> Option<api::ManagedProvider> {
    match kind {
        api::ProviderKind::Anthropic => Some(api::ManagedProvider::Anthropic),
        api::ProviderKind::OpenAi => Some(api::ManagedProvider::OpenAi),
        api::ProviderKind::Google => Some(api::ManagedProvider::Google),
        api::ProviderKind::Xai | api::ProviderKind::Ollama => None,
    }
}

/// 이 모델이 지금 어느 계정으로 말하는가.
#[must_use]
pub fn account_facts_for_model(model: &str) -> Option<AccountFacts> {
    account_facts(managed_provider_for(
        crate::status_format::provider_for_model(model),
    )?)
}

/// 이 판이 자격을 찾을 수 있는 프로바이더 전부, 선언 순서대로.
#[must_use]
pub fn all_account_facts() -> Vec<AccountFacts> {
    api::ManagedProvider::all()
        .iter()
        .filter_map(|provider| account_facts(*provider))
        .collect()
}

/// Drop the process-wide Claude auth memo so the next resolve reads the
/// credential source again.
///
/// The channel's `auth.reload` calls this when the IDE hands this pane a
/// different account: the memo pins `Env`/`Keychain` origins for the life of
/// the process by design, and a switch is precisely the event that rule was
/// never meant to outlive.
pub(crate) fn forget_cached_claude_auth() {
    if let Some(lock) = CACHED_AUTH.get() {
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

fn managed_file_cache_current(cached: &CachedClaudeAuth) -> bool {
    let Some(expected) = cached.managed_file_stamp.as_ref() else {
        return false;
    };
    api::managed_claude_credentials_stamp().as_ref() == Some(expected)
}

/// Map the api layer's resolution origin onto the CLI memo's recovery policy:
/// a keychain session is final until its own expiry, saved OAuth stays a
/// fallback (re-probe the keychain every turn), env credentials are
/// process-fixed.
fn cli_auth_origin(origin: api::ClaudeAuthOrigin) -> AuthOrigin {
    match origin {
        api::ClaudeAuthOrigin::ManagedFile => AuthOrigin::ManagedFile,
        api::ClaudeAuthOrigin::Keychain => AuthOrigin::Keychain,
        api::ClaudeAuthOrigin::SavedOauth => AuthOrigin::Fallback,
        api::ClaudeAuthOrigin::Env => AuthOrigin::Env,
    }
}

/// Resolve auth from scratch, tagging the origin so the memo knows whether the
/// result is final or a recoverable fallback. Zo is an OAuth-subscription
/// tool first, so the api chain runs managed OAuth before env keys:
/// 1) IDE-managed Claude file, otherwise the Claude Code keychain (refreshing)
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
            managed_file_stamp: resolved.managed_file_stamp,
        });
    }

    let auth = resolve_claude_cli_auth_source()?;
    Ok(CachedClaudeAuth {
        auth,
        origin: AuthOrigin::Fallback,
        expires_at_ms: None,
        managed_file_stamp: None,
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
fn update_cached_claude_auth(
    auth: &AuthSource,
    origin: AuthOrigin,
    expires_at_ms: Option<u64>,
    managed_file_stamp: Option<api::ManagedCredentialsStamp>,
) {
    if let Some(lock) = CACHED_AUTH.get() {
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(CachedClaudeAuth {
            auth: auth.clone(),
            origin,
            expires_at_ms,
            managed_file_stamp,
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
            // Cleared only after the rebuild: a swallowed failure leaves the
            // switch outstanding so the next turn tries again rather than
            // spending the whole session on the account the person left.
            let provider = match &client.client {
                ProviderClient::GeminiCodeAssist(_) => api::ManagedProvider::Google,
                _ => api::ManagedProvider::OpenAi,
            };
            api::managed_account::clear_reload_pending(provider);
        }
        return;
    }
    if !matches!(&client.client, ProviderClient::Anthropic(_))
        || client.auth_route == AuthRoute::ApiKey
    {
        return;
    }

    // A switch the IDE pushed over the channel: the memo was already dropped
    // by the handler, so this resolves the NEW account's file and hands the
    // long-lived client its bearer before the per-turn clone. Unlike the stamp
    // path below it does not depend on the memo's origin — a reload can also
    // arrive while zo runs on its own login.
    if api::managed_account::reload_pending(api::ManagedProvider::Anthropic) {
        api::managed_account::clear_reload_pending(api::ManagedProvider::Anthropic);
        if let Ok(Some(auth)) =
            tokio::task::spawn_blocking(|| resolve_and_cache_claude_auth().ok()).await
        {
            client.set_auth(auth);
        }
        return;
    }

    // IDE-managed credentials are checked once per turn by metadata. Only a
    // changed `(mtime, len)` (or approaching expiry) re-reads the secret; the
    // long-lived client then adopts that bearer before its per-turn clone.
    if cached_auth_origin() == Some(AuthOrigin::ManagedFile) {
        if let Ok(Some(auth)) =
            tokio::task::spawn_blocking(|| resolve_and_cache_claude_auth().ok()).await
        {
            client.set_auth(auth);
        }
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
    if let Some((session, origin, managed_file_stamp)) = recovered {
        let auth = AuthSource::BearerToken(session.access_token);
        update_cached_claude_auth(&auth, origin, session.expires_at_ms, managed_file_stamp);
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
            resolved.managed_file_stamp,
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
fn keychain_recovery_session() -> Option<(
    api::KeychainSession,
    AuthOrigin,
    Option<api::ManagedCredentialsStamp>,
)> {
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
                AuthOrigin::Env | AuthOrigin::ManagedFile => false,
            },
            None => false,
        }
    };
    if !should_probe {
        return None;
    }
    let managed_file_stamp = api::managed_claude_credentials_stamp();
    api::read_claude_code_keychain_session().map(|session| {
        let origin = if managed_file_stamp.is_some() {
            AuthOrigin::ManagedFile
        } else {
            AuthOrigin::Keychain
        };
        (session, origin, managed_file_stamp)
    })
}

/// Pure gate for the proactive keychain refresh: due once the recorded expiry
/// is within the shared refresh buffer. An unrecorded expiry never schedules a
/// probe (the reactive 401 path still covers it).
fn keychain_refresh_due(expires_at_ms: Option<u64>, now_ms: u64) -> bool {
    expires_at_ms.is_some_and(|expires_at_ms| {
        now_ms.saturating_add(OAUTH_REFRESH_BUFFER_SECS.saturating_mul(1000)) >= expires_at_ms
    })
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

/// 모델 이름 → provider. 카탈로그가 먼저, 그다음 별칭표, 마지막이 접두어다.
///
/// 크레이트 안에 드러나 있다: `/status` 카드가 "지금 모델의 사용량"을 물으려면
/// 같은 판별이 필요하고([`crate::usage::for_provider`]), 두 번째 판별표를 두면
/// 카드와 실제 요청이 서로 다른 provider 를 가리키는 날이 온다.
pub(crate) fn provider_kind_for_model(model: &str) -> ProviderKind {
    let trimmed = model.trim();
    let lower = trimmed.to_ascii_lowercase();

    // The catalog first, in the order `build_provider_client` asks it: a bare
    // id it names stays first-party even when a connected gateway lists it
    // too, and it claims nothing for a gateway's own slash id.
    if let Some(provider) = catalog_provider_for_model(trimmed) {
        return provider;
    }

    if let Some(entry) = api::provider_catalog().iter().find(|entry| {
        entry.alias == lower || entry.canonical_model_id.eq_ignore_ascii_case(trimmed)
    }) {
        return entry.provider;
    }

    // A model a configured custom provider declares — or an explicit
    // `<provider>/<model>` naming one — rides the OpenAI-compatible wire to
    // that provider, before any prefix reading of the id (a gateway's
    // `anthropic/claude-x` is not an Anthropic model).
    if api::custom_provider_for_model(trimmed).is_some() {
        return ProviderKind::OpenAi;
    }

    // An id the catalog does not name yet — `claude-opus-9`, `gpt-7-vega` —
    // still routes by the words the catalog does know: any Anthropic family
    // alias it contains, or an OpenAI lineup word (`api::is_openai_lineup_word`).
    if lower.starts_with("claude") || contains_family_alias_of(&lower, ProviderKind::Anthropic) {
        return ProviderKind::Anthropic;
    }
    if api::is_openai_lineup_word(&lower) || lower.starts_with("openai") {
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

/// Whether `lower` carries one of `provider`'s family aliases from the shipped
/// catalog (`opus`, `sonnet`, `haiku`, `fable`, …) — the words a person types,
/// which is what an unknown release is most likely spelled with.
fn contains_family_alias_of(lower: &str, provider: ProviderKind) -> bool {
    api::builtin_provider_catalog()
        .iter()
        .filter(|entry| entry.provider == provider)
        .any(|entry| lower.contains(&entry.alias.to_ascii_lowercase()))
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

/// Load `cwd`'s settings and publish its `providers` table, for a process
/// that builds no runtime (`zo models`): without it the listing would show
/// only what an operator exported by hand.
pub(crate) fn publish_custom_providers_from_settings(cwd: &Path) {
    if let Ok(config) = ConfigLoader::default_for(cwd).load() {
        apply_custom_providers_env(&config);
    }
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
/// until the next restart. A rebuild is a connection (a session start, a
/// `/resume`, a model handoff), so the quiet sources are asked again here.
fn apply_model_wire_env() {
    connect_model_catalog();
}

/// Notices the catalog layer wants a person to see once — a family alias that
/// moved, moves a `notify` policy withheld, a settings migration. Queued here
/// because the layer runs before any renderer exists; the front-end drains it
/// at its first opportunity ([`drain_catalog_notices`]).
static CATALOG_NOTICES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn push_catalog_notice(text: impl Into<String>) {
    let text = text.into();
    let mut notices = CATALOG_NOTICES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !notices.contains(&text) {
        notices.push(text);
    }
}

#[must_use]
pub fn drain_catalog_notices() -> Vec<String> {
    std::mem::take(
        &mut *CATALOG_NOTICES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// The alias moves announced on the previous publish ([`alias_moves_key`]),
/// so a move is announced once rather than on every runtime rebuild — and
/// not on every refresh either: the overlay's bytes carry each row's fetch
/// stamp, so keyed on them a refresh that found the same models read as a
/// new answer, and a status line that publishes per paint said the same five
/// moves on every keystroke (2026-09-08).
static LAST_ANNOUNCED_MOVES: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// What a publish would announce, as one stable key: every alias move and
/// withheld candidate, sorted, without the fetch stamp the overlay rows
/// carry. `None` when there is nothing to say.
#[must_use]
pub(crate) fn alias_moves_key(overlay: &runtime::model_discovery::Overlay) -> Option<String> {
    let mut lines: Vec<String> = overlay
        .alias_updates
        .iter()
        .map(|update| format!("{}→{}←{}", update.alias, update.to, update.from))
        .chain(
            overlay
                .alias_candidates
                .iter()
                .map(|update| format!("?{}→{}←{}", update.alias, update.to, update.from)),
        )
        .collect();
    if lines.is_empty() {
        return None;
    }
    lines.sort();
    Some(lines.join("\n"))
}

/// The words a move is announced with. An alias minted for a family the
/// shipped catalog never named has no "was".
#[must_use]
pub(crate) fn alias_move_words(update: &runtime::model_discovery::AliasUpdate) -> String {
    if update.from.is_empty() {
        format!(
            "model catalog: {} → {} (new alias, discovered)",
            update.alias, update.to
        )
    } else {
        format!(
            "model catalog: {} → {} (was {}, discovered)",
            update.alias, update.to, update.from
        )
    }
}

/// Mirror the settings-declared and discovered model rows into the env vars
/// the `api` crate reads for its model catalog. `api` cannot depend on
/// runtime config, and this must run before any provider client is built —
/// and before the persisted model alias is resolved, so a family alias lands
/// on the release discovery found rather than the one the binary shipped with.
///
/// Re-publishes on every runtime rebuild for the same reason
/// `apply_custom_providers_env` does — a `/model` add or edit lands in
/// settings and would otherwise not be served until the next restart — and
/// picks up a background discovery refresh that completed since the last call.
#[allow(clippy::must_use_candidate)] // every runtime rebuild calls this for the effect; the answer is for `zo models`
pub fn publish_model_catalog() -> Option<crate::model_wire_env::Published> {
    let declared = runtime::model_catalog::ModelCatalog::load()
        .ok()
        .and_then(|catalog| catalog.catalog_overlay_json());
    let policy = runtime::model_discovery::UpdatePolicy::load();
    let discovered = runtime::model_discovery::current();
    let overlay = discovered
        .as_deref()
        .map(|catalog| runtime::model_discovery::overlay(catalog, policy))
        .unwrap_or_default();
    let published = crate::model_wire_env::publish(declared.as_deref(), overlay.json.as_deref())
        .unwrap_or_else(|error| {
            eprintln!("[zo] failed to publish the model catalog: {error}");
            None
        });
    let ceilings = discovered
        .as_deref()
        .and_then(|catalog| runtime::model_discovery::effort_ceilings_json(catalog, policy));
    if let Err(error) = crate::model_wire_env::publish_effort_ceilings(ceilings.as_deref()) {
        eprintln!("[zo] failed to publish discovered effort ceilings: {error}");
    }

    announce_alias_moves(&overlay);
    LAST_PUBLISHED_DISCOVERY.store(
        discovered.as_deref().map_or(0, |catalog| catalog.fetched_at),
        std::sync::atomic::Ordering::Relaxed,
    );
    published
}

/// A connection: publish, and ask again the sources that have gone quiet
/// for the TTL (t-3054). The three connections are a runtime build (session
/// start, `/resume`, a handoff), the `/model` picker opening, and `zo
/// models`. Publishing alone — an alias resolved for the status line, a
/// launch contract checked — spawns no refresh: it used to, and with a
/// keyless source always due, every paint refreshed the cache.
#[allow(clippy::must_use_candidate)]
pub fn connect_model_catalog() -> Option<crate::model_wire_env::Published> {
    let published = publish_model_catalog();
    spawn_model_discovery_refresh(
        runtime::model_discovery::UpdatePolicy::load(),
        runtime::model_discovery::current().as_deref(),
    );
    published
}

/// `fetched_at` of the discovery snapshot the last publish layered in — so
/// the `/model` picker can tell a refresh that finished since from one that
/// is already live.
static LAST_PUBLISHED_DISCOVERY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Whether a discovery refresh thread is running now — one at a time; a
/// session start and a picker open during it wait for its answer instead of
/// asking the providers twice.
static DISCOVERY_REFRESH_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The `/model` picker is opening: make a refresh that finished since the
/// last publish live (the picker reads the published catalog), and ask
/// again the sources whose answer is older than the connection TTL. The
/// picker itself shows what is live now; the refresh lands for the next
/// open, the way a session start's does.
pub fn model_picker_opening() {
    let current = runtime::model_discovery::current();
    let published = LAST_PUBLISHED_DISCOVERY.load(std::sync::atomic::Ordering::Relaxed);
    if current.as_deref().is_some_and(|catalog| catalog.fetched_at > published) {
        // A connection: the publish makes the refresh live, and spawns the
        // next one for the sources still due.
        connect_model_catalog();
        return;
    }
    spawn_model_discovery_refresh(
        runtime::model_discovery::UpdatePolicy::load(),
        current.as_deref(),
    );
}

/// Say once what this publish moved — or, under `notify`, what it withheld.
/// Keyed on the MOVES ([`alias_moves_key`]), so a runtime rebuild — or a
/// refresh that found the same models under a newer fetch stamp — says
/// nothing.
fn announce_alias_moves(overlay: &runtime::model_discovery::Overlay) {
    let moves = alias_moves_key(overlay);
    let changed = {
        let mut last = LAST_ANNOUNCED_MOVES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = *last != moves;
        last.clone_from(&moves);
        changed
    };
    if !changed {
        return;
    }
    // Once per answer, not once per launch: the last moves a person was told
    // about are kept beside the discovery cache, so the same move is announced
    // the first time it is published and never again until it changes.
    let announced_path = runtime::model_discovery::cache_path().with_file_name("announced.json");
    let current = moves.unwrap_or_default();
    if std::fs::read_to_string(&announced_path).ok().as_deref() == Some(current.as_str()) {
        return;
    }
    if let Some(parent) = announced_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&announced_path, &current);
    for update in &overlay.alias_updates {
        push_catalog_notice(alias_move_words(update));
    }
    for update in &overlay.alias_candidates {
        push_catalog_notice(format!(
            "model catalog: {} could move to {} (now {}) — /model, or set modelUpdatePolicy to auto",
            update.alias, update.to, update.from
        ));
    }
}

/// Refresh the discovery cache on a detached thread when any source's answer
/// is missing, failed, or past the connection TTL (t-3054: every session
/// start and every `/model` open asks the providers that have gone quiet
/// for an hour). One thread at a time: it writes the cache and installs the
/// snapshot, and the next catalog publish (a runtime rebuild, the next
/// picker open, or the next launch) makes it live — the bridge writes
/// process environment, which stays on the thread that owns the runtime.
fn spawn_model_discovery_refresh(
    policy: runtime::model_discovery::UpdatePolicy,
    current: Option<&runtime::model_discovery::DiscoveredCatalog>,
) {
    use std::sync::atomic::Ordering;
    if policy == runtime::model_discovery::UpdatePolicy::Pinned
        || std::env::var_os("ZO_DISABLE_MODEL_DISCOVERY").is_some()
    {
        return;
    }
    let now = runtime::model_discovery::now_secs();
    let ttl = runtime::model_discovery::ttl_secs();
    if runtime::model_discovery::due_sources(current, now, ttl).is_empty() {
        return;
    }
    if DISCOVERY_REFRESH_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("zo-model-discovery".to_string())
        .spawn(move || {
            refresh_model_discovery_cache(now, ttl);
            DISCOVERY_REFRESH_RUNNING.store(false, Ordering::Release);
        });
    if let Err(error) = spawned {
        DISCOVERY_REFRESH_RUNNING.store(false, Ordering::Release);
        eprintln!("[zo] model discovery thread failed to start: {error}");
    }
}

/// The body of the refresh thread: ask the due sources, cache, and say on
/// stderr (the log file, in an interactive run) what was learned and which
/// source refused.
fn refresh_model_discovery_cache(now: u64, ttl: u64) {
    let catalog = runtime::model_discovery::discover_due(now, ttl);
    let found = catalog.models.len();
    let failed: Vec<String> = catalog
        .reports
        .iter()
        .filter(|report| !report.ok && !report.detail.starts_with("skipped"))
        .map(|report| format!("{}: {}", report.provider, report.detail))
        .collect();
    match runtime::model_discovery::install(catalog) {
        Ok(()) => eprintln!("[zo] model discovery: {found} model(s) cached"),
        Err(error) => eprintln!("[zo] model discovery: could not write the cache: {error}"),
    }
    for line in failed {
        eprintln!("[zo] model discovery: {line}");
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
/// rows, stop paths) shows them running forever. Once per store root in this
/// process (a `/resume` onto the same root sweeps nothing twice), on a
/// detached thread like the retention sweep: boot never waits on the `ps`
/// probe or the store walk. Reads through the SESSION's registry — its root
/// and adopted mirrors — never a store derived from the process cwd.
pub(crate) fn spawn_orphaned_agent_reap(registry: &std::sync::Arc<tools::AgentRegistry>) {
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    static SWEPT: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let fresh = SWEPT
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(registry.root().to_path_buf());
    if !fresh {
        return;
    }
    let registry = std::sync::Arc::clone(registry);
    std::thread::spawn(move || {
        let reaped = tools::reap_orphaned_agents(&registry);
        if reaped > 0 {
            eprintln!(
                "[zo] agent store: settled {reaped} orphaned running manifest(s) whose owning process exited"
            );
        }
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
    )
}

/// 자격증명이 없어도 런타임은 선다. TUI 가 열려야 `/login` 을 칠 수 있고,
/// 파이프 경로도 같은 오류 문구를 사람이 읽는 자리에 남기는 편이 낫다 —
/// 모델 요청은 여전히 실패한다.
fn resolve_startup_auth<R>(resolve_auth: R) -> AuthSource
where
    R: FnOnce() -> Result<AuthSource, Box<dyn std::error::Error>>,
{
    resolve_auth().unwrap_or_else(|error| {
        eprintln!(
            "[zo] Claude auth unavailable at startup: {error}. Opening TUI unauthenticated; run `/login claude` before sending Anthropic requests."
        );
        AuthSource::None
    })
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
        let auth_route = catalog_auth_route_for_model(&model);
        let auth = if provider_kind_for_model(&model) == ProviderKind::Anthropic
            && auth_route == AuthRoute::Auto
        {
            Some(resolve_startup_auth(resolve_auth))
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
            resolved.managed_file_stamp,
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

impl<W: Write> Write for StripAnsiWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.strip {
            // Each caller write is a complete rendered chunk (markdown
            // push/flush, one formatted tool call, or one non-streaming block),
            // so no escape sequence is split across writes and per-chunk
            // stripping is safe.
            let text = String::from_utf8_lossy(buf);
            let stripped = crate::util::ansi::strip_ansi(&text);
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
        let provider = self.provider_kind();
        let tools = self
            .enable_tools
            .then(|| {
                crate::filter_tool_specs(
                    &self.tool_registry,
                    self.allowed_tools.as_ref(),
                )
            });
        // Anthropic accepts every currently resolvable history name even when
        // its schema is deferred from this request. Strict providers retain the
        // advertised-only guard that prevents invalid function history.
        let resolvable = self.tool_registry.resolvable_tool_names();
        let advertised: std::collections::BTreeSet<String> =
            tools.iter().flatten().map(|def| def.name.clone()).collect();
        // Only while tools ride the request (see runtime_bridge): a request
        // that advertises no tools cannot carry tool blocks, so the
        // tools-disabled leg keeps the rewrite-everything contract.
        let known = if tools.is_some() && provider == ProviderKind::Anthropic {
            &resolvable
        } else {
            &advertised
        };
        let reconciled = runtime::session::reconcile_tool_history(
            &request.messages,
            known,
            &resolvable,
        );
        // `effort_override` is a floor (deep-gate escalation): raise the budget,
        // never lower it, then derive both the legacy `thinking.budget_tokens`
        // and the adaptive effort level from the floored value.
        let configured_budget = self
            .thinking
            .as_ref()
            .and_then(|t| t.budget_tokens)
            .filter(|&budget| budget > 0);
        // The step governor's effort for THIS request, when it decided one,
        // else the turn's — the same reading the TUI client takes.
        let requested = crate::session::runtime_bridge::request_effort(
            &request,
            self.named_effort,
            self.effort_band_ceiling,
            configured_budget,
        );
        let effective_budget =
            api::effort_budget_with_floor(requested.budget, request.effort_override);
        // Reminders ride the newest user message (see runtime_bridge) so the
        // system blocks and cached history stay byte-identical across turns.
        let mut messages = runtime::convert_messages_for(
            &reconciled,
            runtime::ReasoningReplay::for_model(&wire_model),
        );
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
            requested.named,
            effective_budget,
            requested.band_ceiling,
        );
        let effort_band_ceiling = requested.band_ceiling;
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
        AnthropicRuntimeClient, AuthOrigin, CACHED_AUTH, CachedClaudeAuth, NeutralRequestOutcome,
        OAUTH_REFRESH_BUFFER_SECS, catalog_provider_for_model, emit_neutral_request_telemetry,
        latest_usage_from_events, oauth_refresh_needed, provider_kind_for_model,
        refresh_oauth_if_near_expiry, resolve_and_cache_claude_auth,
        runtime_error_is_retryable,
    };
    use api::{
        AuthRoute, InputMessage, MessageRequest, ProviderClient, ProviderKind,
    };
    use runtime::{AssistantEvent, PermissionMode, RuntimeError, TokenUsage};
    
    use tools::GlobalToolRegistry;

    #[test]
    fn managed_claude_credentials_are_reloaded_after_the_file_changes() {
        let _env_lock = crate::test_env_lock();
        let config_home = crate::support::temp_dir("managed-claude-reload");
        let managed_home = crate::support::temp_dir("managed-claude-account");
        let credentials = managed_home.join(".credentials.json");
        let _config_home = crate::support::EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = crate::support::EnvVarGuard::set("ZO_HOME", None);
        let _claude_home = crate::support::EnvVarGuard::set(
            "CLAUDE_CONFIG_DIR",
            Some(managed_home.to_str().expect("utf8 managed home")),
        );
        let _legacy_claude_home = crate::support::EnvVarGuard::set("ZO_CLAUDE_HOME", Some(""));
        let _disable_keychain =
            crate::support::EnvVarGuard::set("ZO_DISABLE_KEYCHAIN", Some("1"));
        let _api_key = crate::support::EnvVarGuard::set("ANTHROPIC_API_KEY", None);
        let _auth_token = crate::support::EnvVarGuard::set("ANTHROPIC_AUTH_TOKEN", None);
        api::invalidate_claude_code_keychain_cache();
        *CACHED_AUTH
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        std::fs::write(
            &credentials,
            r#"{"claudeAiOauth":{"accessToken":"first","scopes":["user:inference"]}}"#,
        )
        .expect("first managed credentials");
        let first = resolve_and_cache_claude_auth().expect("first managed auth");
        assert_eq!(first.bearer_token(), Some("first"));

        std::fs::write(
            &credentials,
            r#"{"claudeAiOauth":{"accessToken":"second-token","scopes":["user:inference"]}}"#,
        )
        .expect("switched managed credentials");
        let second = resolve_and_cache_claude_auth().expect("switched managed auth");

        assert_eq!(second.bearer_token(), Some("second-token"));
        std::fs::remove_dir_all(config_home).ok();
        std::fs::remove_dir_all(managed_home).ok();
    }

    /// 채널이 민 계정 전환은 **핀을 뚫는다**. 그 핀은 "프로세스가 도는 동안
    /// 자격은 안 바뀐다" 는 가정이고, 전환은 바로 그 가정이 깨지는 사건이다.
    #[test]
    fn a_pushed_switch_unpins_the_memo_and_follows_the_new_account() {
        let _env_lock = crate::test_env_lock();
        let config_home = crate::support::temp_dir("pushed-switch-config");
        let switched = crate::support::temp_dir("pushed-switch-account");
        let credentials = switched.join(".credentials.json");
        std::fs::write(
            &credentials,
            r#"{"claudeAiOauth":{"accessToken":"the-switched-account","scopes":["user:inference"]}}"#,
        )
        .expect("switched account credentials");
        let _config_home = crate::support::EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = crate::support::EnvVarGuard::set("ZO_HOME", None);
        // The pane was LAUNCHED under a different account, and that env can no
        // longer be changed from outside — the whole reason the reload carries
        // values rather than a bare "reload yourself".
        let _launched_with = crate::support::EnvVarGuard::set("CLAUDE_CONFIG_DIR", Some(""));
        let _legacy_claude_home = crate::support::EnvVarGuard::set("ZO_CLAUDE_HOME", Some(""));
        let _disable_keychain =
            crate::support::EnvVarGuard::set("ZO_DISABLE_KEYCHAIN", Some("1"));
        let _api_key = crate::support::EnvVarGuard::set("ANTHROPIC_API_KEY", None);
        let _auth_token = crate::support::EnvVarGuard::set("ANTHROPIC_AUTH_TOKEN", None);
        api::managed_account::clear();
        let cache = CACHED_AUTH.get_or_init(|| std::sync::Mutex::new(None));
        *cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(CachedClaudeAuth {
            auth: api::AuthSource::BearerToken("the-account-we-started-on".to_string()),
            origin: AuthOrigin::Keychain,
            expires_at_ms: Some(u64::MAX),
            managed_file_stamp: None,
        });
        assert_eq!(
            resolve_and_cache_claude_auth()
                .expect("the pinned memo")
                .bearer_token(),
            Some("the-account-we-started-on")
        );

        // What `auth.reload` does, in the order it does it.
        api::managed_account::apply(
            api::ManagedProvider::Anthropic,
            &api::ManagedAccountUpdate {
                label: Some("work".to_string()),
                claude_config_dir: Some(switched.clone()),
                codex_home: None,
            },
        );
        api::managed_account::note_reload(api::ManagedProvider::Anthropic);
        api::invalidate_claude_code_keychain_cache();
        super::forget_cached_claude_auth();

        let resolved = resolve_and_cache_claude_auth().expect("the switched account's auth");

        assert_eq!(resolved.bearer_token(), Some("the-switched-account"));
        assert_eq!(
            super::account_facts(api::ManagedProvider::Anthropic),
            Some(super::AccountFacts {
                provider: "anthropic",
                label: Some("work".to_string()),
                origin: super::AccountOrigin::IdeManaged,
            })
        );
        api::managed_account::clear();
        *cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        std::fs::remove_dir_all(config_home).ok();
        std::fs::remove_dir_all(switched).ok();
    }

    /// 카드의 출처 칸은 어느 계정으로 말하는지 **그대로** 말한다: 창이 판에
    /// 건넨 `CODEX_HOME` 은 환경, 창에서 찾아낸 홈은 관리, 아무것도 없으면 제
    /// 로그인. 이 칸이 비어 있던 동안 사람은 zo 가 죽은 제 로그인으로 말하는
    /// 것을 「토큰이 또 만료됐다」 로만 볼 수 있었다(t-5777).
    #[test]
    fn the_openai_card_names_which_account_it_borrowed() {
        let _env_lock = crate::test_env_lock();
        api::managed_account::clear();
        let home = crate::support::temp_dir("openai-origin");
        let handed = home.join("handed");
        std::fs::create_dir_all(&handed).expect("a codex home");
        std::fs::write(
            handed.join("auth.json"),
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"at","refresh_token":"rt","account_id":"acct"}}"#,
        )
        .expect("seed the handed-off login");
        let _config_home = crate::support::EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(home.to_str().expect("utf8 config home")),
        );
        let _zo_home = crate::support::EnvVarGuard::set("ZO_HOME", None);

        // ② 판이 태어날 때의 `CODEX_HOME`.
        let _codex_home = crate::support::EnvVarGuard::set(
            api::managed_account::CODEX_HOME_ENV,
            Some(handed.to_str().expect("utf8 codex home")),
        );
        assert_eq!(
            super::account_facts(api::ManagedProvider::OpenAi).map(|facts| facts.origin),
            Some(super::AccountOrigin::Env)
        );

        // ① 채널이 실어 온 계정 — 창이 고른 것.
        api::managed_account::apply(
            api::ManagedProvider::OpenAi,
            &api::ManagedAccountUpdate {
                label: Some("personal".to_string()),
                claude_config_dir: None,
                codex_home: Some(handed.clone()),
            },
        );
        assert_eq!(
            super::account_facts(api::ManagedProvider::OpenAi),
            Some(super::AccountFacts {
                provider: "openai",
                label: Some("personal".to_string()),
                origin: super::AccountOrigin::IdeManaged,
            })
        );
        api::managed_account::clear();

        // 아무 codex 홈도 없고 제 저장소만 있으면 제 로그인이다.
        let _no_codex_home =
            crate::support::EnvVarGuard::set(api::managed_account::CODEX_HOME_ENV, None);
        api::oauth_store::save_openai_oauth(&core_types::OpenAiOAuthTokens {
            access_token: "own-at".to_string(),
            refresh_token: Some("own-rt".to_string()),
            expires_at: Some(0),
            account_id: Some("own-account".to_string()),
            scopes: Vec::new(),
        })
        .expect("zo's own login saves");
        assert_eq!(
            super::account_facts(api::ManagedProvider::OpenAi).map(|facts| facts.origin),
            Some(super::AccountOrigin::OwnLogin)
        );
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn keychain_origin_remains_process_pinned_until_its_refresh_boundary() {
        let _env_lock = crate::test_env_lock();
        let cache = CACHED_AUTH.get_or_init(|| std::sync::Mutex::new(None));
        *cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(CachedClaudeAuth {
            auth: api::AuthSource::BearerToken("pinned-keychain-token".to_string()),
            origin: AuthOrigin::Keychain,
            expires_at_ms: Some(u64::MAX),
            managed_file_stamp: None,
        });

        let resolved = resolve_and_cache_claude_auth().expect("cached keychain auth");

        assert_eq!(resolved.bearer_token(), Some("pinned-keychain-token"));
        *cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
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
        let config_home = crate::support::temp_dir("fresh-google-oauth");
        let _config_home = crate::support::EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = crate::support::EnvVarGuard::set("ZO_HOME", None);
        let _adapter_gate = crate::support::EnvVarGuard::set(api::NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        let _disable_external = crate::support::EnvVarGuard::set("ZO_DISABLE_EXTERNAL_CREDENTIALS", None);
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
        let config_home = crate::support::temp_dir("expired-google-oauth");
        let _config_home = crate::support::EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = crate::support::EnvVarGuard::set("ZO_HOME", None);
        let _adapter_gate = crate::support::EnvVarGuard::set(api::NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        let _disable_external = crate::support::EnvVarGuard::set("ZO_DISABLE_EXTERNAL_CREDENTIALS", None);
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
        let config_home = crate::support::temp_dir("catalog-provider-routing");
        let _config_home = crate::support::EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = crate::support::EnvVarGuard::set("ZO_HOME", None);
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

    /// A router connected in the window's settings may list the very ids a
    /// person runs first-party (`AgentRouter` serves Claude ids). The bare id
    /// keeps its provider in the one judgement the startup auth choice, the
    /// tool reconciliation and `/status` read, and the client is built for
    /// that same provider; `<router>/<id>` is the opt-in, and the router's
    /// own slash ids go to the router — judged and built alike.
    #[test]
    fn a_router_listing_a_first_party_id_does_not_take_the_bare_id() {
        let _env_lock = crate::test_env_lock();
        let config_home = crate::support::temp_dir("router-first-party-id");
        let _config_home = crate::support::EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = crate::support::EnvVarGuard::set("ZO_HOME", None);
        let _custom = crate::support::EnvVarGuard::set(api::CUSTOM_PROVIDERS_ENV, None);
        let bare = api::provider_catalog()
            .iter()
            .find(|entry| entry.provider == ProviderKind::Anthropic)
            .expect("an Anthropic catalog row")
            .canonical_model_id;
        let slashed = format!("anthropic/{bare}");
        api::refresh_custom_providers_from_json(&format!(
            r#"[{{"name":"testrouter","base_url":"http://127.0.0.1:9/v1",
                 "models":["{bare}","{slashed}"],"requires_auth":false}}]"#
        ))
        .expect("custom providers");
        let built = |model: &str, auth: Option<api::AuthSource>| {
            super::build_provider_client("router-test", model, AuthRoute::Auto, auth)
                .map(|client| client.provider_kind())
                .map_err(|error| error.to_string())
        };

        assert_eq!(provider_kind_for_model(bare), ProviderKind::Anthropic);
        assert_eq!(
            built(bare, Some(api::AuthSource::ApiKey("test-key".to_string()))),
            Ok(ProviderKind::Anthropic)
        );
        let explicit = api::format_provider_model_ref("testrouter", bare);
        assert_eq!(provider_kind_for_model(&explicit), ProviderKind::OpenAi);
        assert_eq!(built(&explicit, None), Ok(ProviderKind::OpenAi));
        assert_eq!(provider_kind_for_model(&slashed), ProviderKind::OpenAi);
        assert_eq!(built(&slashed, None), Ok(ProviderKind::OpenAi));

        api::refresh_custom_providers_from_json("[]").expect("clear");
        std::fs::remove_dir_all(config_home).ok();
    }


    #[test]
    fn boot_does_not_resolve_anthropic_auth_for_non_anthropic_main() {
        // Booting with a non-Anthropic main model must not resolve Claude auth.
        // That read shells out to the macOS keychain (and can block on a token
        // refresh) on the synchronous startup path, before MCP discovery is even
        // spawned — so a GPT/Gemini main should never pay it.
        let _env_lock = crate::test_env_lock();
        let _openai_base = crate::support::EnvVarGuard::set("OPENAI_BASE_URL", Some("http://localhost:8080/v1"));
        let _openai_key = crate::support::EnvVarGuard::set("OPENAI_API_KEY", None);

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
            || panic!("booting a non-Anthropic main must not resolve Claude auth"),
        )
        .expect("OpenAI-compatible client should build from custom base URL");

        assert_eq!(client.model(), "gpt-5.5");
        assert_eq!(client.provider_kind(), api::ProviderKind::OpenAi);
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
        let config_home = crate::support::temp_dir("non-anthropic-prompt-cache");
        let _config_home = crate::support::EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = crate::support::EnvVarGuard::set("ZO_HOME", None);
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
            output_tokens_details: None,
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
            output_tokens_details: None,
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

    /// 2026-09-08, 「타자칠때마다 이게 계속 나타남」: the same five alias moves
    /// were announced on every keystroke. The announcement was keyed on the
    /// overlay's bytes, and every row carries its fetch stamp — so a refresh
    /// that found the very same models read as a new answer. The key is the
    /// moves themselves: same moves, whatever the stamp, is nothing to say.
    #[test]
    fn the_same_alias_moves_under_a_newer_fetch_stamp_are_not_news() {
        let update = |alias: &str, to: &str, from: &str| runtime::model_discovery::AliasUpdate {
            provider: "openai".to_string(),
            alias: alias.to_string(),
            from: from.to_string(),
            to: to.to_string(),
        };
        let first = runtime::model_discovery::Overlay {
            json: Some(r#"{"models":[{"source":"discovered via chatgpt-backend at 1788827309"}]}"#.to_string()),
            new_models: Vec::new(),
            alias_updates: vec![update("openai-latest", "gpt-6-astra", "gpt-5.6-sol"), update("astra", "gpt-6-astra", "")],
            alias_candidates: vec![update("gemini-flash", "gemini-3.8-flash", "gemini-3.6-flash")],
        };
        let refreshed = runtime::model_discovery::Overlay {
            json: Some(r#"{"models":[{"source":"discovered via chatgpt-backend at 1788827387"}]}"#.to_string()),
            new_models: Vec::new(),
            // The same moves in another order: still the same answer.
            alias_updates: vec![update("astra", "gpt-6-astra", ""), update("openai-latest", "gpt-6-astra", "gpt-5.6-sol")],
            alias_candidates: first.alias_candidates.clone(),
        };
        assert_ne!(first.json, refreshed.json, "the stamp moved");
        let key = alias_moves_key(&first).expect("moves to announce");
        assert_eq!(alias_moves_key(&refreshed).as_deref(), Some(key.as_str()));
        assert_eq!(
            key,
            "?gemini-flash→gemini-3.8-flash←gemini-3.6-flash\nastra→gpt-6-astra←\nopenai-latest→gpt-6-astra←gpt-5.6-sol"
        );
        // A different move is news; no move is nothing.
        let moved = runtime::model_discovery::Overlay {
            alias_updates: vec![update("openai-latest", "gpt-6-nova", "gpt-6-astra")],
            ..first.clone()
        };
        assert_ne!(alias_moves_key(&moved), Some(key));
        assert_eq!(alias_moves_key(&runtime::model_discovery::Overlay::default()), None);
        // And an alias minted for a family the shipped catalog never named
        // says so instead of "(was , discovered)".
        assert_eq!(
            alias_move_words(&update("astra", "gpt-6-astra", "")),
            "model catalog: astra → gpt-6-astra (new alias, discovered)"
        );
        assert_eq!(
            alias_move_words(&update("openai-latest", "gpt-6-astra", "gpt-5.6-sol")),
            "model catalog: openai-latest → gpt-6-astra (was gpt-5.6-sol, discovered)"
        );
    }

    /// The other half of the same morning: every publish spawned a refresh,
    /// and the status line publishes once per paint (it resolves the model
    /// alias through `cli_args`), so with a keyless source always due the
    /// cache was rewritten 33 times a second. A refresh belongs to a
    /// CONNECTION — a runtime build, the picker opening, the session start
    /// — and a bare publish spawns none.
    #[test]
    fn only_a_connection_spawns_a_discovery_refresh() {
        let source = include_str!("runtime_support.rs");
        let (shipped, _) = source.split_once("#[cfg(test)]").expect("a test boundary");
        let body_of = |name: &str| {
            let start = shipped.find(name).unwrap_or_else(|| panic!("missing {name}"));
            let rest = &shipped[start..];
            let end = rest.find("\n}\n").map_or(rest.len(), |at| at + 3);
            &rest[..end]
        };
        assert!(
            !body_of("pub fn publish_model_catalog(").contains("spawn_model_discovery_refresh("),
            "a bare publish spawned a refresh"
        );
        assert!(body_of("pub fn connect_model_catalog(").contains("spawn_model_discovery_refresh("));
        assert!(body_of("fn apply_model_wire_env(").contains("connect_model_catalog()"));
        assert!(body_of("pub fn model_picker_opening(").contains("connect_model_catalog()"));
        assert!(
            include_str!("session/plain_session.rs")
                .contains("fn preferences_behind_the_catalog() -> crate::preferences::Preferences {\n    // A session start is a connection: the quiet sources are asked again.\n    crate::runtime_support::connect_model_catalog();"),
            "the session start no longer connects"
        );
        for (file, text) in [
            ("cli_args.rs", include_str!("cli_args.rs")),
            ("launch_contract.rs", include_str!("launch_contract.rs")),
        ] {
            assert!(
                text.contains("publish_model_catalog()") && !text.contains("connect_model_catalog()"),
                "{file} is not a connection and must only publish"
            );
        }
        assert_eq!(
            shipped.matches("spawn_model_discovery_refresh(").count(),
            3,
            "the refresh is spawned from the two connection roads and its own definition"
        );
    }

    /// Production installs `dispatch_concurrent_tool` on every plain/JSON,
    /// TUI, and IDE runtime. Finding a deferred schema is useful only if that
    /// exact seam accepts the stable on-wire address and reaches the selected
    /// tool instead of treating the address itself as an unsupported builtin.
    #[test]
    fn production_dispatch_reaches_a_tool_selected_through_capability_invoke() {
        let registry = GlobalToolRegistry::builtin();
        let turn_allowed_tools = std::sync::Arc::new(std::sync::Mutex::new(None));

        let found = dispatch_concurrent_tool(
            &registry,
            None,
            None,
            &turn_allowed_tools,
            None,
            None,
            "ToolSearch",
            r#"{"query":"select:Agent"}"#,
        )
        .expect("ToolSearch should select Agent");
        let found: serde_json::Value = serde_json::from_str(&found).expect("search JSON");
        assert_eq!(found["matches"], serde_json::json!(["Agent"]));

        // Deliberately omit Agent's required arguments: reaching Agent's input
        // decoder gives a deterministic marker without launching a real model.
        let error = dispatch_concurrent_tool(
            &registry,
            None,
            None,
            &turn_allowed_tools,
            None,
            None,
            tools::capability::CAPABILITY_INVOKE,
            r#"{"name":"Agent","input":{}}"#,
        )
        .expect_err("empty Agent input should be rejected after routing");
        let message = error.to_string();
        assert!(
            !message.contains("unsupported tool"),
            "CapabilityInvoke never reached Agent: {message}"
        );
        assert!(
            message.contains("description") || message.contains("prompt"),
            "Agent's decoder should name a required field: {message}"
        );

        let missing = dispatch_concurrent_tool(
            &registry,
            None,
            None,
            &turn_allowed_tools,
            None,
            None,
            tools::capability::CAPABILITY_INVOKE,
            "{}",
        )
        .expect_err("an unselected capability has no address");
        assert!(missing.to_string().contains("ToolSearch"), "{missing}");

        let nested = dispatch_concurrent_tool(
            &registry,
            None,
            None,
            &turn_allowed_tools,
            None,
            None,
            tools::capability::CAPABILITY_INVOKE,
            r#"{"name":"CapabilityInvoke"}"#,
        )
        .expect_err("CapabilityInvoke cannot invoke itself");
        assert!(nested.to_string().contains("cannot invoke itself"), "{nested}");

        let wrapper_only: AllowedToolSet = [tools::capability::CAPABILITY_INVOKE.to_string()]
            .into_iter()
            .collect();
        let denied = dispatch_concurrent_tool(
            &registry,
            Some(&wrapper_only),
            None,
            &turn_allowed_tools,
            None,
            None,
            tools::capability::CAPABILITY_INVOKE,
            r#"{"name":"Agent","input":{}}"#,
        )
        .expect_err("the address must not grant its inner capability");
        assert!(
            denied.to_string().contains("Agent")
                && denied.to_string().contains("--allowedTools"),
            "the inner name must re-enter session gates: {denied}"
        );
    }

    #[test]
    fn startup_auth_resolution_falls_back_to_unauthenticated() {
        let result = resolve_startup_auth(|| {
            Err(Box::new(io::Error::other("invalid_grant")) as Box<dyn std::error::Error>)
        });

        assert!(matches!(result, AuthSource::None));
    }

    #[test]
    fn startup_auth_resolution_preserves_successful_auth() {
        let result = resolve_startup_auth(|| Ok(AuthSource::None));

        assert!(matches!(result, AuthSource::None));
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
