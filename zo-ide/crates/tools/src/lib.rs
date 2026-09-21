//! `tools` crate root.
//!
//! Production surface lives in dedicated modules; this file is the public
//! API + crate-internal re-export hub only.
//!
//! ```text
//! lib.rs (this file)        ── module dispatch + pub/pub(crate) use
//!   ├── context.rs          ── ToolContext + UserQuestionChannel
//!   ├── aliases.rs          ── canonical_tool_name + TOOL_NAME_ALIASES
//!   ├── registry.rs         ── ToolSpec + GlobalToolRegistry + mvp_tool_specs
//!   ├── dispatch.rs         ── execute_tool + dispatch_tool_inner + helpers
//!   ├── preflight.rs        ── workspace test branch divergence guard
//!   └── error.rs            ── ToolError
//! ```
//!
//! Tool-family handlers (`bash_tools`, `file_tools`, `web_tools`, ...) are
//! unchanged — only the cross-cutting glue that used to live inline was
//! extracted as part of Phase 3.1 of the refactor.

mod aliases;
mod artifacts;
mod bash_redirect;
mod bash_tools;
mod codegraph_tools;
mod computer_tools;
mod artifact_tools;
mod context;
mod dispatch;
pub mod capability;
pub mod error;
mod fanout;
mod file_tools;
mod image_tools;
mod file_write_lease;
mod gateway;
mod hunk_attribution;
mod http_bridge;
mod mcp_tools;
mod misc_tools;
mod model_json;
mod plan_mode_v2;
mod preflight;
mod registry;
mod repl_kernel;
mod task_tools;
mod team_inbox_store;
mod team_tools;
mod tool_digest;
pub mod wakeup_store;
mod web_tools;
mod worker_tools;
mod workflow_tools;
mod workspace_scope_guard;
mod workspace_checkpoint;
mod worktree_tools;

// Public API.
pub use tool_digest::{configure_tool_digest, ToolDigestMode, TOOL_DIGEST_ENV};
pub use aliases::canonical_tool_name;
pub use computer_tools::COMPUTER_SHIM;
pub use artifacts::ARTIFACT_STORE_ENV;
pub use team_inbox_store::STORE_ENV as TEAM_INBOX_STORE_ENV;
pub use artifacts::{
    prune_artifact_files_older_than, read_artifact, store_artifact, ArtifactKind, ArtifactRef,
};
pub use context::{ForkSource, SubagentIdentity, ToolContext, TurnAgentPolicy, UserQuestionChannel};
pub use dispatch::{
    bound_tool_error, bound_tool_error_text, bound_tool_output_text, enforce_permission_check,
    execute_tool,
};
pub use error::ToolError;
pub use fanout::{
    clarify_intent, decompose_for_fanout, decompose_for_fanout_with_timeout,
    decompose_for_fanout_with_timeout_and_hooks, decompose_for_fanout_with_width,
    run_fanout_spawn, run_fanout_spawn_with_timeout,
    diagnose_lens_labels, fanout_analysis, prelude_evidence, run_diagnose_fanout,
    run_fanout_spawn_with_timeout_and_hooks, run_self_consistency_fanout, FanoutMode,
    FanoutSubtask, IntentTriage, AUTO_FANOUT_AGENT_TIMEOUT, fanout_width_for,
    AUTO_FANOUT_DECOMPOSE_TIMEOUT, MAX_FANOUT_SUBTASKS, MIN_FANOUT_SUBTASKS, SELF_CONSISTENCY_K,
};
pub use gateway::{
    summarize_invocations, AuditDenial, AuditSummary, RouteDecisionRecord, ToolErrorKind,
    ToolFamily, ToolInvocation, ToolInvocationRequest, ToolInvocationResult, ToolPolicyCheck,
    ToolPolicyDecision, ToolResultMetadata,
};
pub use runtime::live_output;
pub use hunk_attribution::{
    AttributionLine, AttributionLineKind, AttributionOrigin, AttributionStatus, AttributedHunk,
    HunkAttributionLedger, ReviewHunkError, apply_reverse_patch,
};
pub use misc_tools::agent_store_dir;
pub use misc_tools::jev_summary;
pub use misc_tools::{
    basis_points, check_system_one, decision_shadow_mode_from, decision_shadow_path,
    evaluate_decision_labels, read_shadow_rows, rerank_shadow_mode_from, rerank_shadow_path,
    summarize_decision_shadow, task_fingerprint, AxisAgreement, AxisEvaluation, CheckFailure,
    DecisionShadowMode, DecisionShadowRow, DecisionShadowSummary, Judged, JudgedAxis,
    LabelEvaluation, ProbeCell, RerankShadow, RerankShadowRow, SystemOneCheck, BASIS_POINTS,
    DECISION_SHADOW_FILE, DECISION_SHADOW_SETTING, KEY_CHECK_TASK, OUTCOME_ANSWERED,
    RERANK_OUTCOME_ANSWERED, RERANK_OUTCOME_UNORDERABLE, RERANK_SHADOW_FILE,
    RERANK_SHADOW_SETTING,
    merged_settings_root,
};
pub use misc_tools::{
    note_loaded_skill, note_search_answer, skill_search, skill_search_mode_from,
    skill_search_path,
    Chosen, Searched, SkillLabelRow, SkillSearchRow,
    SKILL_OUTCOME_ANSWERED, SKILL_SEARCH_DEADLINE, SKILL_SEARCH_FILE, SKILL_SEARCH_SETTING,
    judge_step_effort_ledger, record_step_event, step_effort_path, step_effort_raised,
    step_effort_word, step_effort_word_in, StepEffortWord, StepJudgmentRow, StepSeat,
    JUDGMENT_ROW_KIND, STEP_EFFORT_FILE, STEP_EFFORT_SETTING, STEP_JUDGMENT_DEADLINE,
};
pub use misc_tools::{
    build_plan_shadow, conversation_anchor_ttl_for, conversation_anchor_ttl_from_root,
    model_options_for, model_price_for, plan_priors_for, plan_shadow_path,
    record_plan_shadow, switch_candidates, CACHE_ANCHOR_TTL_ENV,
    PlanShadowActual, PlanShadowCandidate, PlanShadowInputs, PlanShadowRow,
};
pub use misc_tools::{
    mark_session_process, registry_locator_for, store_root_for, AgentRegistry, RegistryRecord,
};
pub use misc_tools::{stranded_proposed_skills, ProposedSkill};
pub use misc_tools::{loaded_custom_agents, LoadedCustomAgent};
pub use misc_tools::{live_pane_children, PaneChildChannel};
pub use misc_tools::{
    send_agent_message, send_agent_message_for_session, AgentSendOutcome,
};
pub use misc_tools::{
    agent_terminal_summary, background_agent_notification_header,
    clear_background_completion_marks_for_session, drain_background_completions_for_session,
    fold_background_completions_into_input, AGENT_NOTIFICATION_EMPTY_RESULT,
};
pub use misc_tools::{
    assess_agent_task, assess_turn_complexity, assess_turn_complexity_probed,
    assess_turn_deterministic, assess_turn_orchestration, assess_turn_probed,
    decide_host_prelude, turn_has_write_intent, AgentTaskAssessment, HostPrelude,
    AssessmentReaders, TurnOrchestrationHint, TurnProbeAssessment,
};
pub use misc_tools::{
    smart_deep_tier_models, smart_deep_tier_models_for, smart_exec_swap, smart_setting_defaults,
    smart_turn_routing_and_inventory_for, smart_turn_routing_for, DeepTierModelsSetting,
    HostOrchestration, PlanShadowSettings,
    SmartExecSwap, SmartSettingDefaults, SmartTurnRouting,
};
pub use misc_tools::ToolSearchOutput;
pub use misc_tools::{
    agent_message_source_id,
    agent_worker_is_live, background_completion_matches_session, clear_background_agent,
    execute_config, execute_enter_plan_mode, execute_exit_plan_mode, is_background_agent,
    reconcile_dead_agent_worker,
    mark_background_agent,
    background_task_completion, notify_background_task_completion, notify_remote,
    parent_session_belongs,
    register_agent_completion_channel,
    reap_orphaned_agents, stop_agent_for_session, stop_running_agents_since,
    stop_running_agents_since_for_session,
    stop_running_agents_since_for_strict_session, wait_for_agent_completions, AgentCompletion,
    AgentStopOutcome,
    ConfigInput, ConfigOutput, ConfigValue, EnterPlanModeInput, ExitPlanModeInput, PlanModeOutput,
    AGENT_MESSAGE_STATUS, AGENT_STARVED_STATUS, provider_error_class_from_completion,
    provider_error_class_metadata,
};
pub use misc_tools::push_notification::{
    push_road, road_note, shape_message, KeyboardPresence, PushLimits, PushNotice, PushRoad,
    PushSurface, PUSH_NOTIFICATION_TOOL,
};
pub use registry::{
    deferred_tool_manifest_section, mvp_tool_specs, GlobalToolRegistry, RuntimeToolDefinition,
    ToggleableTool, ToggleableToolSource, ToolManifestEntry, ToolRegistry, ToolSource, ToolSpec,
};
pub use team_tools::{ensure_team_inbox_store, host_post_team_inbox_update};
pub use workflow_tools::{
    event_log_terminal_status, event_phase_statuses, event_timeline_lines, read_event_log,
    request_foreground_workflow_cancel, EventPhase, WorkflowEventKind, WorkflowEventRecord,
};
pub use workspace_checkpoint::{
    render_workspace_checkpoint_list, render_workspace_restore_summary, WorkspaceCheckpoint,
    WorkspaceCheckpointFile, WorkspaceFileSnapshot, WorkspaceRestoreSkippedPath,
    WorkspaceRestoreSummary, WorkspaceSnapshotSkip,
    MAX_CHECKPOINT_FILE_BYTES, MAX_WORKSPACE_CHECKPOINTS,
};

/// The tools one subagent type is allowed, as its harness would be given them.
///
/// Public because a sub-agent no longer has to be a THREAD of this process: a
/// teammate pane is a whole zo run, and it has to install the same allow-list
/// its in-process twin would have had. An `Explore` that could write files
/// because it happened to get a screen would be a different agent from the
/// one its parent asked for.
#[must_use]
pub fn allowed_tools_for_subagent(subagent_type: &str) -> std::collections::BTreeSet<String> {
    misc_tools::allowed_tools_for_subagent(subagent_type)
}

/// Whether this role requires the host's local inspection shell constraint.
#[must_use]
pub fn inspection_shell_for_subagent(subagent_type: &str) -> bool {
    misc_tools::inspection_shell_for_subagent(subagent_type)
}

/// Compact capability tag for the static toolset behind a subagent type.
#[must_use]
pub fn subagent_toolset_class(subagent_type: &str) -> &'static str {
    let tools = misc_tools::allowed_tools_for_subagent(subagent_type);
    let builtin = runtime::BuiltinSubagentProfile::all()
        .iter()
        .any(|profile| profile.key().eq_ignore_ascii_case(subagent_type));
    if !builtin {
        "custom"
    } else if tools.contains("Config") || tools.contains("Sleep") {
        "full"
    } else if tools.contains("edit_file") || tools.contains("write_file") {
        "edit"
    } else {
        "read-only"
    }
}

// Crate-internal helpers — `*_tools.rs` siblings reach these through
// `crate::xxx` so the re-export here is what keeps the existing call sites
// working without a per-file path rewrite. `permission_mode_from_plugin`
// and `normalize_shell_command` are only touched from `#[cfg(test)]`, so
// silence the unused-import warning that fires on a non-test build.
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use aliases::permission_mode_from_plugin;
pub(crate) use dispatch::{
    epoch_seconds_now, execute_tool_with_context, from_value, maybe_enforce_permission_check,
    to_pretty_json,
};
pub(crate) use file_write_lease::{
    acquire as acquire_write_lease, release_all_for_owner as release_write_leases, LeaseOutcome,
};
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use preflight::normalize_shell_command;
pub(crate) use preflight::workspace_test_branch_preflight;
pub(crate) use registry::SearchableToolSpec;
pub(crate) use workspace_scope_guard::{workspace_guard_enabled, workspace_scope_guard};

#[cfg(test)]
mod tests;
