//! Core runtime primitives for the `zo` CLI and supporting crates.
//!
//! This crate owns session persistence, permission evaluation, prompt assembly,
//! MCP plumbing, tool-facing file operations, and the core conversation loop
//! that drives interactive and one-shot turns.

pub mod auto_format;
pub mod background_log;
mod bash;
pub mod bash_validation;
mod bootstrap;
mod compact;
mod config;
pub mod context_compression;
mod conversation;
mod convert_messages;
pub mod file_neighbours;
pub mod file_ops;
pub mod file_read_registry;
pub mod fuzzy_file_picker;
pub mod git_snapshot;
mod hooks;
pub mod image_guard;
pub mod jev_score;
mod jsonl_log;
pub mod lsp_client;
pub mod live_output;
mod mcp;
mod mcp_client;
pub mod mcp_http;
pub mod mcp_http_common;
pub mod mcp_lifecycle_hardened;
mod mcp_limits;
pub mod mcp_oauth;
pub mod mcp_sse;
mod mcp_stdio;
pub mod mcp_ws;
pub mod memory;
pub mod message_stream;
pub mod model_inventory;
pub mod model_catalog;
pub mod model_discovery;
pub mod model_router;
pub mod notifications;
mod oauth;
pub mod permission;
pub mod permission_enforcer;
mod permissions;
mod prompt;
pub mod prompt_cache_breaks;
pub mod request_timings;
pub mod scoreboard;
mod registry_io;
pub mod retry;
pub mod sandbox;
pub mod second_brain;
pub mod secure_fs;
pub mod session_control;
pub mod skill_rank;
pub mod skill_sources;
pub mod skills;
pub mod stale_branch;
pub mod subagent_panes;
pub mod summary_compression;
pub mod task_packet;
pub mod task_registry;
mod team_inbox_digest;
pub mod team_cron_registry;
pub mod todo_progress;
pub mod todo_store;
pub mod tool_cancel;
pub mod trust_resolver;
pub mod verified_state;
pub mod worker_boot;

// Re-export modules from core-types so that `crate::json`, `crate::session`,
// `crate::usage`, `crate::lane_events`, and `crate::sse` still resolve for
// internal consumers within this crate.
pub use core_types::json;
pub use core_types::lane_events;
pub use core_types::session;
pub use core_types::sse;
pub use core_types::usage;

pub use bash::{
    execute_bash, execute_bash_with_tasks, interrupt_foreground_bash, timeout_stderr,
    timeout_stderr_with_background_advice, BashCommandInput, BashCommandOutput,
};
pub use tool_cancel::{
    ToolCancelSignal, ToolCancelWatch, ToolDispatchOutcome, CANCELLED_TOOL_RESULT,
};

#[must_use]
pub fn available_disk_bytes(dir: &std::path::Path) -> Option<u64> {
    bash::available_disk_bytes(dir)
}

#[must_use]
pub fn low_disk_warning(dir: &std::path::Path) -> Option<String> {
    bash::low_disk_warning(dir)
}
pub use bootstrap::{BootstrapPhase, BootstrapPlan};
pub use compact::{
    apply_compaction, compact_session, compact_session_with, compaction_system_prompt,
    distill_session_state, edited_file_paths, estimate_session_tokens, format_compact_summary,
    get_compact_continuation_message, is_edit_result_tool, is_pre_clear_original_of,
    microcompact_clearable_estimate, microcompact_quote, microcompact_session, prepare_compaction,
    should_compact, MicrocompactQuote,
    summary_fabricates_identifiers,
    CompactionConfig, CompactionPlan, CompactionResult, CompactionSummarizer, FocusSummarizer,
    LocalSummarizer, MicrocompactEvent, COMPACTION_SYSTEM_PROMPT, MICROCOMPACT_IMAGE_PLACEHOLDER,
    MICROCOMPACT_PLACEHOLDER,
};
pub use zerocode_core::compact_diff::{
    CompactDiffHunk, CompactDiffLine, CompactDiffLineKind, compact_line_diff,
};
pub use config::{
    conventional_config_home, default_config_home, durable_traces_armed, jev_ledger_dir,
    jev_seat_applies, project_slug,
    relocate_traces_out_of_tree, traces_base,
    zo_global_config_roots,
    zo_project_state_dir, zo_state_base, JEV_LEDGER_DIR,
    CliConfigOverrides, ConfigEntry, ConfigError, ConfigLoader, ConfigSource, HookMatcher,
    HookRule, McpConfigCollection, McpEdit, McpManagedProxyServerConfig, McpOAuthConfig,
    McpRemoteServerConfig, McpSdkServerConfig, McpServerConfig, McpStdioServerConfig, McpTransport,
    McpWebSocketServerConfig, OAuthConfig, ResolvedPermissionMode, RuntimeConfig,
    RuntimeFeatureConfig, RuntimeHookConfig, RuntimePermissionRuleConfig, RuntimePluginConfig,
    RuntimeSecondBrainConfig, RuntimeShipConfig, ScopedMcpServerConfig, UntrustedMcpServer,
    TRUSTED_MCP_SERVERS_FILE, ZO_SETTINGS_SCHEMA_NAME,
    persist_allow_always_rules, remove_mcp_server, trust_mcp_server, write_mcp_server,
};
pub use conversation::{ModelSwitch, SteeringObserver, SteeringQueue, SwitchObserver};
pub use conversation::{
    decide_step_effort, shift_step_effort, EffortStep, RungMove, StepAsk, StepAskContext,
    StepBatch, StepDecision, StepEffortConfig, StepEffortObserver, StepEffortSeat, StepEvent,
    StepJudgment, StepJudgmentRow, StepLabel, StepMove, StepReason, StepRow, StepSignals,
    LABEL_ROW_KIND, NO_IN_TURN_ROAD, ROUTINE_STEPS_FOR_LIGHTER, STEP_JUDGMENT_EVERY,
    STEP_ROW_KIND, STRONG_STEPS_FOR_HEAVIER,
};
pub use conversation::{
    build_design_guidance_reminder, DESIGN_GUIDANCE_REMINDER_PREFIX, PRELUDE_FANNED_OUT_REMINDER,
    ROUTE_HINT_REMINDER_PREFIX,
};
pub use conversation::{AgentNotification, AgentNotificationInbox, AgentNotificationKind};
pub use conversation::{
    auto_compaction_threshold_for_model, auto_compaction_threshold_from_env, bash_result_exited_zero,
    detect_check_command,
    declare_attendance, declared_attendance, env_deadline_extension, env_turn_budgets,
    final_assistant_text, read_only_bash_allow_rules,
    take_verifier_calibration_events, VerifierCalibrationEvent,
    flush_pending_tool_events, prompt_cache_record_to_event, record_non_anthropic_prompt_cache_usage, push_output_block, redacted_thinking_data_to_string, response_to_events, ApiClient, ApiRequest,
    AssistantEvent, AsyncApiClient, Attendance, AutoCompactionEvent, BudgetExhausted,
    ConcurrentDispatchFn, ConversationRuntime, DeepGateConfig, DeepMode,
    DeepOutcome, ExecContract, PromptCacheEvent, ProviderStateBlob, RuntimeError, StaticToolExecutor,
    StreamingTurnError, ToolError, ToolExecutor, ToolTextKind, TurnSummary, AUTO_RETRY_MARKER,
    DEEP_EXEC_MARKER, DEEP_PLAN_MARKER, DEEP_VERIFY_MARKER,
    DEFAULT_STREAMING_CHANNEL_CAPACITY, DEFAULT_TURN_DEADLINE_SECS,
    DEFAULT_TURN_INPUT_TOKEN_BUDGET, DEFAULT_TURN_OUTPUT_TOKEN_BUDGET, STEERING_ECHO_PREFIX,
};
pub use conversation::{parse_peer_message, peer_message_frame, PeerMessage, PEER_MESSAGE_FRAME_OPEN};
/// Provider failure classification carried on [`RuntimeError`]. Re-exported so
/// callers (and tests) can construct/inspect a classified error — notably the
/// `RateLimit` class the quota-fallback turn loop keys off — without reaching
/// past `runtime` into the `api` crate.
pub use api::ProviderErrorClass;
pub use convert_messages::{
    append_wire_reminders, conversation_anchor_ttl, convert_messages, convert_messages_for,
    declare_conversation_anchor_ttl, is_persisted_reminder_text,
    is_reasoning_passport_text, strip_reasoning_passport_label,
    mark_conversation_cache_breakpoints, mark_conversation_cache_breakpoints_short_lived,
    model_handoff_notice, wrap_reminder, ConversationAnchorTtl, ReasoningReplay,
    REASONING_PASSPORT_LABEL,
};
pub use core_types::{format_usd, pricing_for_model};
pub use core_types::{
    ContentBlock, ConversationMessage, CouncilOutcome, IncrementalSseParser, JsonError, JsonValue,
    LaneEvent, LaneEventBlocker, LaneEventName, LaneEventStatus, LaneFailureClass, MemoryEntry,
    MemoryHit, MemoryRetriever, MessageRole, ModelPricing, OutputTokensDetails, Session,
    SessionCompaction, SessionError, SessionFork, SseEvent, TokenUsage, UsageCostEstimate,
    UsageTracker,
};
pub use file_ops::{
    edit_file, glob_search, grep_search, read_file, replace_file_atomic, write_file, EditFileOutput,
    GlobSearchOutput, GrepSearchInput, GrepSearchOutput, ReadFileOutput, SettingsFileLock,
    StructuredPatchHunk, TextFilePayload, WriteFileOutput,
};
pub use file_read_registry::{FileFreshness, FileReadRegistry};
pub use verified_state::{
    VerifiedStateEvent, VerifiedStateLedger, VERIFIED_STATE_REMINDER_PREFIX,
};
pub use hooks::{
    HookAbortOrigin, HookAbortSignal, HookEvent, HookProgressEvent, HookProgressReporter,
    HookRunResult, HookRunner,
};
pub use team_inbox_digest::{
    ensure_session_channel_subscription, team_inbox_manual_ack, team_inbox_snapshot,
    team_inbox_store_root, team_inbox_unread_count, TeamInboxSnapshot, TeamInboxSnapshotRow,
};
pub use mcp::{
    mcp_server_signature, mcp_tool_name, mcp_tool_prefix, normalize_name_for_mcp,
    scoped_mcp_config_hash, unwrap_ccr_proxy_url,
};
pub use mcp_client::{
    McpClientAuth, McpClientBootstrap, McpClientTransport, McpManagedProxyTransport,
    McpRemoteTransport, McpSdkTransport, McpStdioTransport,
};
pub use mcp_http::{connect_mcp_http, McpHttpProcess};
pub use mcp_lifecycle_hardened::{
    McpDegradedReport, McpErrorSurface, McpFailedServer, McpLifecyclePhase, McpLifecycleState,
    McpLifecycleValidator, McpPhaseResult,
};
pub use mcp_sse::{connect_mcp_sse, McpSseProcess};
pub use mcp_stdio::{
    spawn_mcp_stdio_process, InboundEvent, JsonRpcError, JsonRpcId, JsonRpcRequest,
    JsonRpcResponse, ManagedMcpTool, McpDiscoveryClass, McpDiscoveryFailure, McpGetPromptParams,
    McpGetPromptResult, McpInitializeClientInfo, McpInitializeParams, McpInitializeResult,
    McpInitializeServerInfo, McpListPromptsParams, McpListPromptsResult, McpListResourcesParams,
    McpListResourcesResult, McpListToolsParams, McpListToolsResult, McpPrompt, McpPromptArgument,
    McpPromptMessage, McpReadResourceParams, McpReadResourceResult, McpResource,
    McpResourceContents, McpServerManager, McpServerManagerError, McpStdioProcess, McpTool,
    McpToolCallContent, McpToolCallParams, McpToolCallResult, McpToolDiscoveryReport,
    UnsupportedMcpServer,
};
pub use mcp_ws::{connect_mcp_ws, McpWsProcess};
pub use model_inventory::{connected_model_inventory, model_inventory_from_authorized_providers};
pub use model_router::{
    agent_preference_adjustment, agent_stats_for_route, completion_ceiling_for, completion_loop_step,
    spawn_attempt_key, turn_attempt_key,
    orchestration_accuracy,
    preferred_agent_for_route, read_orchestration_accuracy,
    classify_model_tiers, deep_tier_model_matches, default_deep_tier_models, dynamic_deep_tier_models,
    exploration_slot_for_route, ImplRung, ModelBand, ModelTierAssignment,
    fuse_probe_assessment, implementation_route_model_allowed, is_deep_tier_model,
    is_reserved_orchestrator_model,
    is_terminal_outcome_status, parse_probe_response, probe_prompt, ComplexityCalibration,
    ProbeAssessment, ProbeFusion, ProbeFusionEffect, RouteAssessmentProvenance, RouteTaskIntent,
    rubric_task_text, rubric_task_whole, RubricAxis, COMPLEXITY_AXIS, CONFIDENCE_AXIS, DECISION_RUBRIC_VERSION,
    INTENT_AXIS, RISK_AXIS, ROUTING_RUBRIC, RUBRIC_TASK_CHAR_CAP,
    axis_metrics, decision_questions, decision_request, judged_axes, validate_decision,
    AxisMetrics, AxisReading, AxisSample, DecisionAnswer, DecisionRejection, DecisionVerdict, ROUTE_TRUST_FLOOR,
    CALIBRATION_BINS, PROBABILITY_SUM_TOLERANCE,
    read_route_outcome_summary, read_route_outcomes, read_route_outcomes_across_projects,
    recommend_auto_assignments,
    recommend_auto_assignments_with_feedback, recommend_auto_assignments_with_learned_specialty,
    recommend_auto_assignments_with_options,
    recommend_role_fallbacks, recommend_role_fallbacks_with_learned_specialty, record_route_outcome,
    recommended_effort_for,
    route_model, route_model_fallback_candidates,
    resolve_verdict_basis, route_outcome_log_path, summarize_decisions_by_kind,
    summarize_route_outcomes, summarize_route_outcomes_with_canonicalizer, verify_metrics,
    weakest_decision_kind, weighted_feedback_hint_for_route_key, AccuracyReport, AgentRouteStat,
    AssignmentSource,
    AutoAssignmentOptions, AutoAssignmentPlan, BuiltinSubagentProfile, CompletionStep, DecisionKind,
    PlanShape, RouteTaxCall, ROUTE_TAX_ROUTE_KEY,
    OUTCOME_COMPLETED, OUTCOME_FAILED, OUTCOME_STOPPED,
    choose_plan, plan_candidates, plan_evidence_from_records, score_plan, ChoiceReason,
    CostBreakdown, CostTerm, ModelOption,
    ModelPrice, PlanCacheState, PlanCandidate, PlanChoice, PlanContext, PlanEstimate, PlanEvidence,
    PlanPriors, ScoredPlan, SwitchTrigger, VerifyMode,
    DecisionOutcomeStat, EffortCeiling, FreshnessPolicy, LaneRouteMetadata,
    LearnedSpecialtyEntry, LearnedSpecialtyHint, VerdictBasis, VerdictSubject, VerifyMetrics,
    ACCURACY_MIN_DECISIVE, CONFIDENT_DECISIVE_SAMPLES,
    ModelCapability, ModelDescriptor, ModelInventory, ModelStatus, ModelTier, RoleOverride,
    RoleSelector, RouteAudit, RouteAutoClassifierMode, RouteConfidence, RouteContextNeed,
    RouteDecision, RouteDecisionSource, RouteDiversityNeed, RouteFeedbackHint,
    RouteOutputNeed, RouteOutcomeBucket, RouteOutcomeRecord, RouteOutcomeSummary,
    RoutePolicyContext, RouteRequest, RouteRole, RouteShapeKind, RouteSignalSource,
    RouteTaskComplexity, RouteTaskKind, RouteTaskRisk, RouteToolNeed, RouteVerificationNeed,
    RouterMode, RoutingTarget, SmartPolicy, SubagentProfileId, SubagentProfileKind, TiersProvenance,
};
pub use memory::{
    dream_at_cwd, load_lexical_memory_retriever, load_memory_retriever, maybe_auto_dream,
    parse_memory_index, record_auto_dream_failure, record_automation_event, record_observation,
    record_verified_check, render_recalled_memory_section, DreamReport, Dreamer,
    LexicalMemoryRetriever,
};
pub use memory::recall_seat::RecallSeat;
pub use mcp_oauth::open_browser;
pub use second_brain::{SecondBrain, VaultStatus};
pub use oauth::{
    clear_mcp_oauth_token, clear_oauth_credentials, clear_openai_oauth, code_challenge_s256,
    credentials_path, generate_pkce_pair, generate_state, is_mcp_token_expired,
    list_mcp_oauth_servers, load_mcp_oauth_token, load_oauth_credentials, load_openai_oauth,
    loopback_redirect_uri, parse_oauth_callback_query, parse_oauth_callback_request_target,
    save_mcp_oauth_token, save_oauth_credentials, save_openai_oauth, OAuthAuthorizationRequest,
    OAuthCallbackParams, OAuthRefreshRequest, OAuthTokenExchangeRequest, OAuthTokenSet,
    OpenAiOAuthTokens, PkceChallengeMethod, PkceCodePair,
};
pub use permissions::{
    PermissionContext, PermissionMode, PermissionOutcome, PermissionOverride, PermissionPolicy,
    PermissionPromptDecision, PermissionPrompter, PermissionRequest, TemporaryAllowGrant,
};
pub use prompt::{
    discover_skills, load_system_prompt, load_system_prompt_for_main,
    load_system_prompt_for_main_with_mode, load_system_prompt_for_main_with_mode_and_spawn,
    output_style, prepend_bullets, skill_search_roots,
    split_system_with_identity, ContextFile, ProjectContext, PromptBuildError, PromptMode,
    render_reminders, SkillIndexEntry, SkillInvocationMode, SkillTriggers, SkillsIndexRoad,
    SystemPromptBuilder,
    ReminderCandidate, SKILLS_INDEX_HEADING, SKILL_INDEX_BUDGET_TOKENS, MAX_REMINDER_LINES,
    MAX_REMINDER_TOKENS,
    REMINDERS_SECTION_HEADING, REMINDER_LINE_PREFIX,
    frontier_model_name, retarget_prompt_model, CLAUDE_CODE_IDENTITY,
    SYSTEM_PROMPT_DYNAMIC_BOUNDARY,
};
pub use skills::{
    build_skill_recommendation_reminder, recommend_skills, SkillDecision, SkillMatchInput,
    SkillRecommendation, SKILL_RECOMMENDATION_REMINDER_PREFIX,
};
pub use skill_sources::{SkillCandidate, SkillCatalog, SkillSource};
pub use sandbox::{
    build_linux_sandbox_command, detect_container_environment, detect_container_environment_from,
    resolve_sandbox_status, resolve_sandbox_status_for_request, ContainerEnvironment,
    FilesystemIsolationMode, LinuxSandboxCommand, SandboxConfig, SandboxDetectionInputs,
    SandboxRequest, SandboxStatus,
};
mod tool_output_truncation;
pub use tool_output_truncation::{truncate_tool_output, TruncatedOutput, TruncationConfig};
pub mod commit_ledger;
pub mod turn_trace;
pub use turn_trace::{TurnOutcome, TurnRecord};

pub use stale_branch::{
    apply_policy, check_freshness, BranchFreshness, StaleBranchAction, StaleBranchEvent,
    StaleBranchPolicy,
};
pub use task_packet::{validate_packet, TaskPacket, TaskPacketValidationError, ValidatedPacket};

/// Env flag enabling per-phase turn timing logs (set to any value).
pub const PROFILE_TURN_ENV: &str = "ZO_PROFILE_TURN";

/// Whether `ZO_PROFILE_TURN` turn profiling is on. Read per call (no
/// memoization) so long-lived sessions observe changes; every caller sits on
/// a >=50ms slow path where one getenv is noise.
#[must_use]
pub fn turn_profiling_enabled() -> bool {
    std::env::var(PROFILE_TURN_ENV).is_ok()
}
pub use trust_resolver::{TrustConfig, TrustDecision, TrustEvent, TrustPolicy, TrustResolver};
pub use worker_boot::{
    Worker, WorkerEvent, WorkerEventKind, WorkerEventPayload, WorkerFailure, WorkerFailureKind,
    WorkerPromptTarget, WorkerReadySnapshot, WorkerRegistry, WorkerStatus, WorkerTrustResolution,
};

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let guard = LOCK.get_or_init(|| {
        // Compaction-tier assertions must measure the model's window, not the
        // host's. A zo test run launched from inside a Claude Code session
        // inherits `CLAUDE_CODE_AUTO_COMPACT_WINDOW` (Claude Code sets it to
        // 650000 for a 1M-context session), which silently rescales every tier
        // and fails two dozen threshold tests for a reason that is nowhere in
        // the test. Cleared once, here, so hermeticity is a property of the lock
        // rather than something each test has to remember.
        std::env::remove_var("CLAUDE_CODE_AUTO_COMPACT_WINDOW");
        std::sync::Mutex::new(())
    })
    .lock()
    .unwrap_or_else(std::sync::PoisonError::into_inner);
    // The ZeroCode window exports the second-brain vault into every pane it
    // opens, so a test run from inside one would index the developer's REAL
    // vault: recall fixtures would see hits nobody put in them, and a
    // "there is nothing to recall" assertion would fail for a reason that is
    // nowhere in the test. Cleared on every acquisition rather than once at
    // init, because the second-brain tests set it themselves under this lock.
    std::env::remove_var(crate::second_brain::VAULT_ENV);
    guard
}
