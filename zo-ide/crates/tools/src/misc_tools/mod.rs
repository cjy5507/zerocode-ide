mod agent_tools;
mod audit;
mod config_tools;
pub(crate) mod council;
pub(crate) mod dispatch;
mod fanout_isolation;
mod notebook_tools;
pub mod push_notification;
mod retrieve_output;
mod session_recall;
mod skill_tools;
mod smart_router;
pub use smart_router::jev_summary;
mod specs;

pub(crate) use dispatch::dispatch;
pub(crate) use specs::tool_specs;

// Re-export everything that was previously public from this module.
// These are used by lib.rs tests.
pub(crate) use smart_router::{
    canonicalize_route_model_id, smart_parent_model_for_agent,
};
pub use smart_router::{
    assess_agent_task, assess_turn_complexity, assess_turn_complexity_probed,
    assess_turn_deterministic, assess_turn_orchestration, assess_turn_probed,
    decide_host_prelude, smart_deep_tier_models, smart_deep_tier_models_for, smart_exec_swap,
    smart_setting_defaults, smart_turn_routing_and_inventory_for, smart_turn_routing_for,
    turn_has_write_intent, AgentTaskAssessment,
    DeepTierModelsSetting, HostOrchestration, HostPrelude, PlanShadowSettings, SmartExecSwap,
    SmartSettingDefaults, AssessmentReaders, SmartTurnRouting, TurnOrchestrationHint,
    TurnProbeAssessment,
    merged_settings_root,
};
pub use smart_router::{
    jev_mention_rerank_mode_from, mention_rerank_path, MentionAnswer, MentionAsk, MentionCandidate,
    MentionJudged, MentionLabelRow, MentionRerank, MentionRerankRow, MentionSurface,
    JEV_MENTION_RERANK_SETTING, MENTION_OUTCOME_ANSWERED, MENTION_RERANK_DEADLINE, MENTION_RERANK_FILE,
    MENTION_RUBRIC_VERSION,
};
pub use smart_router::{
    claim_check_path, jev_claim_mode_from, note_claim_turn, ClaimCheckRow, ClaimLabelRow,
    judge_vault_pairs, mark_vault_pair, recorded_vault_pair_proposals,
    PairJudgment, PairLabel, PairRun,
    jev_patch_review_mode_from, note_patch_review_turn, patch_review_path, PatchReviewJudge,
    PatchReviewLabelRow, PatchReviewRow, JEV_PATCH_REVIEW_SETTING, PATCH_REVIEW_DEADLINE,
    PATCH_REVIEW_FILE, PATCH_REVIEW_OUTCOME_ANSWERED,
};
pub use smart_router::{
    file_pick_path, jev_file_pick_mode_from, note_file_pick_turn, FilePickJudge,
    FilePickLabelRow, FilePickRow, JEV_FILE_PICK_SETTING, FILE_PICK_FILE,
    FILE_PICK_OUTCOME_ANSWERED,
};
pub use smart_router::{
    command_guard_path, jev_command_guard_mode_from, jev_tool_text_guard_mode_from,
    note_tool_guard_turn, tool_text_guard_path, CommandGuardLabelRow, CommandGuardRow,
    ToolGuardJudge, ToolTextGuardLabelRow, ToolTextGuardRow, COMMAND_GUARD_FILE,
    JEV_COMMAND_GUARD_SETTING, JEV_TOOL_TEXT_GUARD_SETTING, TOOL_GUARD_OUTCOME_ANSWERED,
    TOOL_TEXT_GUARD_FILE,
};
pub use smart_router::{
    compaction_relevance_path, jev_compaction_mode_from, note_compaction_reread, CompactionJudge,
    CompactionLabelRow, CompactionRow, COMPACTION_JUDGMENT_DEADLINE, COMPACTION_OUTCOME_ANSWERED,
    COMPACTION_RELEVANCE_FILE, JEV_COMPACTION_SETTING,
};
pub use smart_router::{
    basis_points, check_system_one, decision_shadow_mode_from, decision_shadow_path,
    evaluate_decision_labels, note_recall_read, note_route_followed, read_shadow_rows,
    rerank_shadow_mode_from, rerank_shadow_path, route_unseated_by, summarize_decision_shadow,
    task_fingerprint, AxisAgreement, AxisEvaluation, CheckFailure, DecisionShadowMode,
    DecisionShadowRow, DecisionShadowSummary, Judged, JudgedAxis, LabelEvaluation, ProbeCell,
    RerankLabelRow, RerankShadow, RerankShadowRow, ShownNote, RouteLabelRow, SystemOneCheck, BASIS_POINTS,
    DECISION_SHADOW_FILE, DECISION_SHADOW_SETTING, KEY_CHECK_TASK, OUTCOME_ANSWERED,
    RERANK_OUTCOME_ANSWERED, RERANK_OUTCOME_UNORDERABLE, RERANK_SHADOW_FILE,
    RERANK_SHADOW_SETTING, ROUTE_STOOD,
};
pub use smart_router::{
    agent_tool_mode_from, agent_tool_path, jev_decide, AgentToolRow, JevAnswer, JevCaller, JevInvalid,
    JevQuestion, JevShape, JevVerdict, ScoredItem, AGENT_TOOL_FILE, AGENT_TOOL_OUTCOME_ANSWERED,
    AGENT_TOOL_SETTING,
};
pub use smart_router::{
    note_loaded_skill, note_search_answer, skill_search, skill_search_mode_from, SkillSuggestionJudge,
    skill_search_path,
    Chosen, Searched, SkillLabelRow, SkillSearchRow,
    SKILL_OUTCOME_ANSWERED, SKILL_SEARCH_DEADLINE, SKILL_SEARCH_FILE, SKILL_SEARCH_SETTING,
    judge_step_effort_ledger, record_step_event, step_effort_path, step_effort_raised,
    step_effort_word, step_effort_word_in, StepEffortWord, StepJudgmentRow, StepSeat,
    JUDGMENT_ROW_KIND, STEP_EFFORT_FILE, STEP_EFFORT_SETTING, STEP_JUDGMENT_DEADLINE,
};
pub use smart_router::{
    build_plan_shadow, conversation_anchor_ttl_for, conversation_anchor_ttl_from_root,
    model_options_for, model_price_for, plan_priors_for, plan_shadow_path,
    record_plan_shadow, switch_candidates, CACHE_ANCHOR_TTL_ENV,
    PlanShadowActual, PlanShadowCandidate, PlanShadowInputs, PlanShadowRow,
};
#[allow(unused_imports)]
pub(crate) use agent_tools::{
    agent_usage_cost_for_attempt, AgentUsageCost, wait_for_next_agent_completions,
    agent_worker_generation_is_live, cancel_and_salvage_agent_attempt,
    build_provider_client_for_agent,
    agent_permission_policy, allowed_tools_for_subagent, inspection_shell_for_subagent, cancel_and_salvage_agent,
    classify_lane_failure, execute_agent_with_parent_model_and_hooks, execute_agent_with_spawn,
    execute_agent_with_spawn_and_parent_model_and_hooks, final_assistant_text,
    normalize_subagent_type, persist_agent_terminal_state, push_output_block,
    wait_for_agent_completions_cancellable,
    wait_for_agent_completions_until_done, workflow_concurrency_limit,
    agent_activity_snapshot_by_id, AgentActivitySnapshot, AgentInput, AgentJob, AgentOutput,
    SubagentToolExecutor, AGENT_MODEL_ENV,
};
pub use agent_tools::{
    agent_message_source_id,
    agent_store_dir, agent_worker_is_live, mark_session_process, registry_locator_for,
    store_root_for, AgentRegistry, RegistryRecord, background_completion_matches_session,
    clear_background_agent, is_background_agent, mark_background_agent,
    reconcile_dead_agent_worker,
    background_task_completion, notify_background_task_completion,
    loaded_custom_agents, LoadedCustomAgent,
    live_pane_children, PaneChildChannel,
    parent_session_belongs, provider_error_class_from_completion, provider_error_class_metadata,
    reap_orphaned_agents, register_agent_completion_channel,
    stop_agent_for_session, stop_running_agents_since,
    stop_running_agents_since_for_session, stop_running_agents_since_for_strict_session,
    wait_for_agent_completions, AgentCompletion, AgentStopOutcome, AGENT_MESSAGE_STATUS,
    AGENT_STARVED_STATUS,
};
pub(crate) use audit::run_audit;
pub(crate) use config_tools::write_plan_artifact;
pub use config_tools::{
    execute_config, execute_enter_plan_mode, execute_exit_plan_mode, ConfigInput, ConfigOutput,
    ConfigValue, EnterPlanModeInput, ExitPlanModeInput, PlanModeOutput,
};
#[allow(unused_imports)]
pub(crate) use council::{
    CouncilInput, MAX_COUNCIL_CANDIDATES, MAX_COUNCIL_CANDIDATE_CHARS, MAX_COUNCIL_LLM_JUDGE_CALLS,
};
#[allow(unused_imports)]
pub(crate) use notebook_tools::{NotebookCellType, NotebookEditInput, NotebookEditMode};
pub(crate) use retrieve_output::{run_retrieve_tool_output, RetrieveToolOutputInput};
pub(crate) use session_recall::{run_session_recall, SessionRecallInput};
#[cfg(test)]
pub(crate) use skill_tools::execute_skill;
pub use skill_tools::{stranded_proposed_skills, ProposedSkill};
pub(crate) use skill_tools::{
    normalize_skill_slug, parse_skill_frontmatter_field, render_proposed_skill, write_atomic_new,
    write_atomic_replace, SkillDistillInput, SkillInput, SkillLoadInput, SkillReviewInput,
    SkillSearchInput,
};

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use core_types::helper_run::HelperRun;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    epoch_seconds_now, mvp_tool_specs, to_pretty_json, GlobalToolRegistry, SearchableToolSpec,
    ToolContext, ToolError, ToolSpec,
};
use runtime::subagent_panes::SteerReceipt;
use runtime::{lsp_client::LspRegistry, McpDegradedReport, RuntimeHookConfig};

use super::{execute_tool_with_context, from_value, maybe_enforce_permission_check};

// --- Input structs (remaining in misc_tools) ---

#[derive(Debug, Deserialize)]
pub(crate) struct ToolSearchInput {
    pub query: String,
    pub max_results: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SleepInput {
    pub duration_ms: u64,
    /// 런타임이 이미 비차단 `tokio::time::sleep` 으로 대기했음을 표시한다.
    /// live 디스패치(`conversation::run_turn`)는 `sleep_tool_execution_input`
    /// 이 이 플래그를 주입한 뒤 `tokio::time::sleep` 으로 먼저 잔다. set 이면
    /// `execute_sleep` 이 동기 `std::thread::sleep` 을 건너뛰어 ① 대기 2배,
    /// ② `block_in_place` 로 인한 spinner/elapsed freeze 를 막는다. 플래그가
    /// 없는 비-live 경로(테스트·직접 dispatch)에서는 종전대로 동기 슬립한다.
    #[serde(default, rename = "__zo_already_slept")]
    pub already_slept: bool,
}

/// Upper bound on a single `send_to_user` push, applied at the tool boundary
/// (the render block itself never truncates). A verbatim finding/diff can be
/// large, so the cap is generous; past it the message is truncated with a
/// trailing marker and the result flags `truncated: true` so the model knows.
pub(crate) const MAX_SEND_TO_USER_CHARS: usize = 16_000;

/// Upper bound on ONE agent result relayed into another context — the parent's
/// blocking `tool_result`, a background completion re-injected into the parent
/// conversation (zo-cli mirrors this size), and a `sees` edge's payload
/// injected into a sibling's prompt all share it: however the result travels,
/// the receiving context sees at most this much, middle-elided so both the
/// opening framing and the concluding deliverable survive.
pub(crate) const AGENT_RESULT_RELAY_CHARS: usize = 16_000;

/// Input for `send_to_user` (and its legacy aliases `SendUserMessage` /
/// `Brief`). Only `message` is meaningful; the aliases' old `attachments` /
/// `status` fields are tolerated (serde ignores unknown keys) but no longer do
/// anything — attachments are a non-goal for the mid-run push.
#[derive(Debug, Deserialize)]
pub(crate) struct SendToUserInput {
    pub message: String,
}

/// `Jev`'s input (t-6040): which of the three questions, the question itself,
/// and the parts each shape reads — `context` for `ask` and `choose`,
/// `options` for `choose`, `levels` and `items` for `score`. The check is the
/// seat's own ([`JevQuestion::new`]), the same one `zo jev` runs.
#[derive(Debug, Deserialize)]
pub(crate) struct JevInput {
    pub shape: JevShape,
    pub question: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub levels: Vec<String>,
    #[serde(default)]
    pub items: Vec<String>,
}

/// `PushNotification`'s input — Claude Code's shape verbatim: a message and
/// the `proactive` status word. The status is accepted and checked, never
/// used: it is the schema's way of saying the model, not a hook, asked.
#[derive(Debug, Deserialize)]
pub(crate) struct PushNotificationInput {
    pub message: String,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub(crate) struct StructuredOutputInput(BTreeMap<String, Value>);

#[derive(Debug)]
pub(crate) struct AskUserQuestionInput {
    pub question: String,
    /// Short topic chip shown beside the prompt title (e.g. `Auth method`).
    pub header: Option<String>,
    /// Accepts both bare strings and `{label, description}` objects — see
    /// [`runtime::message_stream::QuestionOption`]'s `Deserialize`.
    pub options: Option<Vec<runtime::message_stream::QuestionOption>>,
    /// When `true` the user may pick several options and the answer is
    /// returned as a JSON array; the default (`false`) keeps the single-choice
    /// contract where `answer` is a lone string.
    pub multi_select: bool,
}

impl<'de> Deserialize<'de> for AskUserQuestionInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        normalize_ask_user_question(value).map_err(serde::de::Error::custom)
    }
}

/// Recover a well-formed `{question, header, options}` from the several shapes
/// models actually emit. Beyond the canonical flat form we tolerate:
/// - a harness-style `{ "questions": [ { … } ] }` envelope (take the first),
/// - the entire payload JSON-encoded into the `question` string
///   (`question = "{\"question\": …, \"options\": […]}"`),
/// - prose followed by a trailing JSON array of options dumped into `question`.
///
/// Top-level fields always win; recovered fields only fill gaps. The function is
/// fail-open: anything it cannot interpret is left as-is so a plain question
/// still renders.
fn normalize_ask_user_question(mut value: Value) -> Result<AskUserQuestionInput, String> {
    use runtime::message_stream::QuestionOption;

    // Unwrap a `{ "questions": [ {…} ] }` envelope to its first entry.
    if let Some(first) = value
        .get_mut("questions")
        .and_then(Value::as_array_mut)
        .and_then(|arr| arr.first_mut())
    {
        value = first.take();
    }

    let obj = value
        .as_object()
        .ok_or_else(|| "AskUserQuestion input must be a JSON object".to_string())?;

    let mut question = obj
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut header = obj.get("header").and_then(Value::as_str).map(str::to_string);
    let mut options = obj
        .get("options")
        .and_then(|v| serde_json::from_value::<Vec<QuestionOption>>(v.clone()).ok())
        .filter(|opts| !opts.is_empty());
    // Tolerate both the canonical camelCase `multiSelect` and a snake_case
    // spelling; models emit either. Missing / non-bool defaults to single-select.
    let mut multi_select = bool_field(obj, "multiSelect").or_else(|| bool_field(obj, "multi_select"));

    // The whole payload JSON-encoded into the `question` string.
    if let Ok(inner) = serde_json::from_str::<Value>(question.trim()) {
        if let Some(inner_obj) = inner.as_object() {
            if let Some(inner_q) = inner_obj.get("question").and_then(Value::as_str) {
                if options.is_none() {
                    options = inner_obj
                        .get("options")
                        .and_then(|v| serde_json::from_value::<Vec<QuestionOption>>(v.clone()).ok())
                        .filter(|opts| !opts.is_empty());
                }
                if header.is_none() {
                    header = inner_obj
                        .get("header")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                if multi_select.is_none() {
                    multi_select = bool_field(inner_obj, "multiSelect")
                        .or_else(|| bool_field(inner_obj, "multi_select"));
                }
                question = inner_q.to_string();
            }
        }
    }

    // Prose followed by a trailing JSON array of options dumped into `question`.
    if options.is_none() {
        if let Some(idx) = question.find('[') {
            let (prose, tail) = question.split_at(idx);
            if let Ok(arr) = serde_json::from_str::<Vec<QuestionOption>>(tail.trim()) {
                if !arr.is_empty() {
                    options = Some(arr);
                    question = prose.trim().to_string();
                }
            }
        }
    }

    if question.trim().is_empty() {
        return Err("AskUserQuestion requires a non-empty `question`".to_string());
    }

    Ok(AskUserQuestionInput {
        question,
        header,
        options,
        multi_select: multi_select.unwrap_or(false),
    })
}

/// Read a boolean field, tolerating the stringy `"true"`/`"false"` some models
/// emit instead of a JSON bool. Returns `None` when the key is absent or the
/// value is neither a bool nor a recognisable boolean string.
fn bool_field(obj: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    match obj.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct MemoryWriteInput {
    pub slug: String,
    pub summary: String,
    pub body: String,
    /// Write to the machine-local project overlay (`memory.local/`). This is
    /// the default: it survives compaction and a restart on this machine, but
    /// cannot change durable project knowledge. Set `local: false` only for an
    /// explicit global memory write. Recall merges both stores.
    #[serde(default = "default_memory_local")]
    pub local: bool,
}

const fn default_memory_local() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub(crate) struct RemoteTriggerInput {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Option<Value>,
    #[serde(default)]
    pub body: Option<String>,
}


#[derive(Debug, Deserialize)]
pub(crate) struct MonitorInput {
    pub process_id: Option<String>,
    pub command: Option<String>,
    #[serde(default = "default_monitor_lines")]
    pub lines: usize,
}

fn default_monitor_lines() -> usize {
    50
}

#[derive(Debug, Deserialize)]
pub(crate) struct SendMessageInput {
    pub to: String,
    pub message: String,
    /// Information-topology edge on the steer/resume path: names/ids of
    /// FINISHED agents whose deliverables travel WITH this message, injected
    /// engine-side as the same labeled shared-context section a spawn-time
    /// `sees` edge produces. This is the feedback half of the topology
    /// vocabulary — `SendMessage(to: builder, attach_results: [reviewer])`
    /// relays the reviewer's findings into the builder's resumed context
    /// without the orchestrator re-narrating a byte of them. Orchestrator
    /// only: a sub-agent's upstream-only copy of this tool rejects it.
    #[serde(default, rename = "attach_results")]
    pub attach_results: Vec<String>,
    /// On a resume: re-resolve the agent's harness from today's definition
    /// instead of reloading the stored one (t-2513 §2.4). Off by default,
    /// because a follow-up must not silently become a different agent when a
    /// custom definition was edited between turns — the stored harness is
    /// reloaded and the response says `harness: "stale"` when they differ.
    #[serde(default, rename = "refresh_harness")]
    pub refresh_harness: bool,
    /// Addressing session, filled by dispatch from [`ToolContext`] (hidden
    /// from tool input). A teammate NAME is session-scoped — this is what
    /// stops a same-named worker from another session/tab winning the lookup.
    /// `None` (untracked host) keeps the legacy workspace-global resolution.
    #[serde(skip)]
    pub session_id: Option<String>,
}

/// `ScheduleWakeup`: the model's self-pacing tool. A schedule call names a
/// delay, a reason and the prompt the wakeup turn opens with; `stop: true`
/// on its own ends the loop; `noop` marks the tick that scheduled it quiet.
#[derive(Debug, Deserialize)]
pub(crate) struct ScheduleWakeupInput {
    #[serde(rename = "delaySeconds", default)]
    pub delay_seconds: Option<f64>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub noop: bool,
    #[serde(default)]
    pub stop: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SyntheticOutputInput {
    pub tool_name: String,
    pub output: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StopAgentInput {
    pub agent_id: String,
    pub reason: String,
}

/// Stop one sub-agent this session owns.
///
/// Every guard lives in [`agent_tools::stop_agent_for_session`], which the
/// human-facing viewer already uses: the manifest must carry this exact session
/// id **and** this process id, and this process must still own the live run
/// generation. A legacy manifest without those stamps is refused rather than
/// trusted. So the model gets the same door the operator has — not a wider one.
pub(crate) fn run_stop_agent(
    registry: &agent_tools::AgentRegistry,
    input: &StopAgentInput,
    session_id: Option<&str>,
) -> Result<String, ToolError> {
    let agent_id = input.agent_id.trim();
    if agent_id.is_empty() {
        return Err(ToolError::InvalidInput("agent_id must not be empty".to_string()));
    }
    let reason = input.reason.trim();
    if reason.is_empty() {
        return Err(ToolError::InvalidInput(
            "reason must say why the agent is being stopped".to_string(),
        ));
    }
    // Without a session there is nothing to prove ownership against, and a stop
    // that cannot prove ownership must not happen at all.
    let Some(session_id) = session_id else {
        return Err(ToolError::InvalidInput(
            "this session cannot be identified, so agent ownership cannot be proven".to_string(),
        ));
    };

    let outcome = agent_tools::stop_agent_for_session(registry, agent_id, session_id, reason);
    let (status, detail) = match outcome {
        agent_tools::AgentStopOutcome::Stopped { name } => ("stopped", name),
        agent_tools::AgentStopOutcome::Closed { name } => ("closed", name),
        agent_tools::AgentStopOutcome::AlreadyFinished { name, status } => {
            ("already_finished", format!("{name} ({status})"))
        }
        agent_tools::AgentStopOutcome::NotFound => (
            "not_found",
            "no agent with that id belongs to this session".to_string(),
        ),
        agent_tools::AgentStopOutcome::NotOwned { name } => ("not_owned", name),
        agent_tools::AgentStopOutcome::Unreachable { name } => ("unreachable", name),
        agent_tools::AgentStopOutcome::Failed { name, error } => {
            ("failed", format!("{name}: {error}"))
        }
    };
    to_pretty_json(json!({
        "status": status,
        "agent_id": agent_id,
        "detail": detail,
    }))
}

/// The compact table's columns, in the order a row prints them. One table,
/// read by the header and by every cell, so a renamed column cannot leave the
/// header saying one thing and the rows another.
pub(crate) const ROSTER_COLUMNS: [&str; 8] = [
    "id",
    "name",
    "status",
    "execution",
    "pane",
    "receipt",
    "gen",
    "completion",
];

/// What an empty roster says. A model that asked for the list deserves a
/// sentence, not an empty array it has to interpret.
pub(crate) const NO_AGENTS_LINE: &str = "no agents in this session";

/// The table's stand-in for a fact this row does not have.
const ROSTER_ABSENT_CELL: &str = "-";

/// The `completion` cell of an agent whose result is sitting in the store.
const ROSTER_COMPLETION_WAITING: &str = "waiting";

/// How `ListAgents` renders. JSON is the default because the tool result is
/// read by a model; the table is for a human-facing surface that wants one
/// line per agent.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RosterFormat {
    #[default]
    Json,
    Table,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ListAgentsInput {
    #[serde(default)]
    pub format: RosterFormat,
}

/// List every sub-agent this session owns.
///
/// The rows come from [`agent_tools::session_agent_rows`], which reads the
/// SESSION's registry — the same store, and the same `parentSessionId`
/// ownership test, the Alt+A overview scans. There is no roster of the tool's
/// own to drift from the screen's.
pub(crate) fn run_list_agents(
    registry: &agent_tools::AgentRegistry,
    input: &ListAgentsInput,
    session_id: Option<&str>,
) -> Result<String, ToolError> {
    // Without a session there is nothing to be the owner of, and answering
    // from a cwd-derived store is exactly the drift t-2511 closed.
    let Some(session_id) = session_id else {
        return Err(ToolError::InvalidInput(
            "this session cannot be identified, so its agents cannot be listed".to_string(),
        ));
    };
    let rows = agent_tools::session_agent_rows(registry, session_id);
    match input.format {
        RosterFormat::Json => to_pretty_json(json!({
            "session": session_id,
            "count": rows.len(),
            "note": rows.is_empty().then_some(NO_AGENTS_LINE),
            "agents": rows.iter().map(roster_row_json).collect::<Vec<_>>(),
        })),
        RosterFormat::Table => Ok(roster_table(&rows)),
    }
}

fn roster_row_json(row: &agent_tools::AgentRosterRow) -> serde_json::Value {
    json!({
        "id": row.id,
        "name": row.name,
        "status": row.status,
        "execution": row.execution,
        "pane": row.pane,
        "lastReceipt": row.last_receipt,
        "run_generation": row.run_generation,
        "completion_waiting": row.completion_waiting,
    })
}

/// The compact text shape: a header and one line per agent, columns padded to
/// their widest cell so the eye can run down one.
fn roster_table(rows: &[agent_tools::AgentRosterRow]) -> String {
    if rows.is_empty() {
        return NO_AGENTS_LINE.to_string();
    }
    let cells: Vec<[String; ROSTER_COLUMNS.len()]> = rows.iter().map(roster_row_cells).collect();
    let mut widths = ROSTER_COLUMNS.map(str::len);
    for row in &cells {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let line = |values: &[String]| {
        values
            .iter()
            .zip(widths)
            .map(|(value, width)| format!("{value:<width$}"))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let header = ROSTER_COLUMNS.map(str::to_string);
    std::iter::once(line(&header))
        .chain(cells.iter().map(|row| line(row)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn roster_row_cells(row: &agent_tools::AgentRosterRow) -> [String; ROSTER_COLUMNS.len()] {
    let absent = || ROSTER_ABSENT_CELL.to_string();
    [
        row.id.clone(),
        row.name.clone(),
        row.status.clone(),
        row.execution.to_string(),
        row.pane.clone().unwrap_or_else(absent),
        row.last_receipt.map_or_else(absent, str::to_string),
        row.run_generation.to_string(),
        if row.completion_waiting {
            ROSTER_COMPLETION_WAITING.to_string()
        } else {
            absent()
        },
    ]
}

#[derive(Debug, Deserialize)]
pub(crate) struct SpawnMultiAgentInput {
    #[serde(deserialize_with = "deserialize_agents_lenient")]
    pub agents: Vec<Value>,
    /// Permit-before-spawn window: at most this many sub-agents run at once, so a
    /// flat fan-out can no longer create N OS threads eagerly (BUG-R11/D4). `None`
    /// uses the workflow execution window (`min(16, cores-2)`, the Claude Code
    /// model) — later members queue for a freed slot. Clamped to
    /// `[1, MAX_SPAWN_MULTI_AGENT_AGENTS]`. Lenient like `agents`: models emit
    /// `"3"` (string) or `3.0` (integral float) instead of a bare integer, and a
    /// strict parse rejected the whole fan-out call.
    #[serde(default, deserialize_with = "deserialize_option_usize_lenient")]
    pub concurrency: Option<usize>,
    /// Internal foreground session id stamped onto every spawned member
    /// manifest. Hidden from tool input; dispatch fills it from [`ToolContext`].
    #[serde(skip)]
    pub parent_session_id: Option<String>,
    /// The shape stamped on every member's outcome row when the HOST laid the
    /// batch out — the prelude's read-only lanes. Hidden from tool input.
    /// `None` is a fan-out the MODEL asked for, which the member spawn labels
    /// as parallel lanes of the batch's own width; the two are different
    /// decisions and a scorer must not read them as one.
    #[serde(skip)]
    pub plan_shape: Option<runtime::PlanShape>,
    /// The session-owned agent registry every member is written into
    /// (t-2511). Hidden from tool input; dispatch fills it from [`ToolContext`].
    #[serde(skip)]
    pub registry: Option<std::sync::Arc<agent_tools::AgentRegistry>>,
    /// Internal `tool_use` id of this fan-out call, copied onto every member's
    /// [`AgentInput`] so their manifests attribute to this call's transcript
    /// batch. Hidden from tool input; dispatch fills it from the smuggled
    /// `__zo_tool_call_id`.
    #[serde(skip)]
    pub tool_call_id: Option<String>,
    /// Internal parent-session MCP passthrough, copied onto every member's
    /// [`AgentInput`] (see that field). Hidden from tool input; dispatch fills
    /// it from [`ToolContext`].
    #[serde(skip)]
    pub mcp_passthrough: Option<crate::registry::McpPassthrough>,
    /// Parent's active permission mode, copied onto every member's
    /// [`AgentInput`] so the spawn clamp applies to fan-out members exactly
    /// like single `Agent` spawns. Hidden from tool input; dispatch fills it.
    #[serde(skip)]
    pub parent_permission_mode: Option<runtime::PermissionMode>,
}

/// Hard cap on `agents` per flat `SpawnMultiAgent` call — CC-scale queued
/// breadth (matches the workflow hard ceiling). The cap bounds how many
/// members one call may ENUMERATE; how many run at once is the spawn window
/// below, and real provider concurrency is bounded tighter still by the
/// adaptive per-provider rate governor.
pub(crate) const MAX_SPAWN_MULTI_AGENT_AGENTS: usize = 64;

/// Resolve the effective permit-before-spawn window. Defaults to the workflow
/// execution bound (`min(16, cores-2)`, the Claude Code model): a large
/// fan-out spawns that many members immediately and streams the rest in as
/// slots free up, instead of opening one OS thread per member eagerly
/// (BUG-R11/D4). An explicit `concurrency` request may tighten the window to 1
/// or widen it up to the hard cap — widening never raises real API concurrency,
/// which stays behind the adaptive per-provider governor either way.
pub(crate) fn effective_spawn_window(requested: Option<usize>, agent_count: usize) -> usize {
    let cap = MAX_SPAWN_MULTI_AGENT_AGENTS.min(agent_count.max(1));
    requested.map_or_else(
        || workflow_concurrency_limit().min(cap),
        |window| window.clamp(1, cap),
    )
}
/// Legacy foreground collection window used by compatibility call sites and
/// tests. Production `SpawnMultiAgent` result aggregation waits for terminal
/// completions instead of surfacing `still_running` after this window.
pub(crate) const SPAWN_MULTI_AGENT_WAIT_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Deserialize `agents`, tolerating the stringified-JSON form some models
/// emit (`"[{…}]"`) instead of a real array. See [`coerce_agents`].
fn deserialize_agents_lenient<'de, D>(deserializer: D) -> Result<Vec<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    coerce_agents(value).map_err(serde::de::Error::custom)
}

/// Coerce a raw `agents` JSON value into a list of agent objects.
///
/// Accepts, in increasing order of leniency:
/// - a real array (the schema-correct form),
/// - a single object (treated as a one-agent fan-out),
/// - a JSON-encoded string of either of the above — the failure seen in the
///   wild, where the model serialized the argument to a string and strict
///   serde rejected it with `invalid type: string, expected a sequence`.
fn coerce_agents(value: Value) -> Result<Vec<Value>, String> {
    match value {
        Value::Array(items) => Ok(items),
        Value::Object(_) => Ok(vec![value]),
        Value::String(raw) => {
            let parsed: Value = crate::model_json::parse_model_json(&raw)
                .map_err(|err| format!("`agents` was a string but not valid JSON: {err}"))?;
            match parsed {
                Value::Array(items) => Ok(items),
                object @ Value::Object(_) => Ok(vec![object]),
                _ => Err("`agents` string must encode an array or object".to_string()),
            }
        }
        other => Err(format!(
            "`agents` must be an array of agent objects (got {})",
            json_type_name(&other)
        )),
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Deserialize an optional non-negative integer, tolerating the forms models
/// emit instead of a bare integer. Seen in the wild on `SpawnMultiAgent`'s
/// `concurrency` argument: the model sent `"3"` and strict serde rejected the
/// whole call with `invalid type: string "3", expected usize`.
fn deserialize_option_usize_lenient<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        None => Ok(None),
        Some(value) => coerce_optional_usize(&value).map_err(serde::de::Error::custom),
    }
}

/// Coerce a raw JSON value into an optional non-negative integer.
///
/// Accepts, in increasing order of leniency:
/// - a real non-negative integer (the schema-correct form),
/// - an integral float (`3.0`) — some models emit numbers that way,
/// - a numeric string (`"3"`, `" 4 "`, `"3.0"`) — the failure seen in the
///   wild, mirroring [`coerce_agents`],
/// - `null` or an empty/whitespace string, treated as unset.
fn coerce_optional_usize(value: &Value) -> Result<Option<usize>, String> {
    match value {
        Value::Null => Ok(None),
        Value::Number(number) => number_as_usize(number)
            .map(Some)
            .ok_or_else(|| format!("expected a non-negative integer, got {number}")),
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            trimmed
                .parse::<usize>()
                .ok()
                .or_else(|| {
                    serde_json::from_str::<serde_json::Number>(trimmed)
                        .ok()
                        .as_ref()
                        .and_then(number_as_usize)
                })
                .map(Some)
                .ok_or_else(|| {
                    format!("expected a non-negative integer, got string {trimmed:?}")
                })
        }
        other => Err(format!(
            "expected a non-negative integer, got {}",
            json_type_name(other)
        )),
    }
}

/// A `serde_json::Number` as `usize` when it is an exact non-negative
/// integer — including the integral-float form (`3.0`) models sometimes emit.
/// Floats are round-tripped through their display form instead of an `as`
/// cast so nothing truncates silently.
fn number_as_usize(number: &serde_json::Number) -> Option<usize> {
    if let Some(int) = number.as_u64() {
        return usize::try_from(int).ok();
    }
    let float = number.as_f64()?;
    if float.fract() != 0.0 || !float.is_finite() || float < 0.0 {
        return None;
    }
    format!("{float:.0}").parse::<usize>().ok()
}

// --- Output structs (remaining in misc_tools) ---

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolSearchOutput {
    pub matches: Vec<String>,
    /// Full definitions (description + input schema) of the matched tools —
    /// the payload a deferred-tool lookup exists for. Empty when no match
    /// resolved to a registered definition.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<api::ToolDefinition>,
    pub query: String,
    pub normalized_query: String,
    #[serde(rename = "total_deferred_tools")]
    pub total_deferred_tools: usize,
    #[serde(rename = "pending_mcp_servers")]
    pub pending_mcp_servers: Option<Vec<String>>,
    #[serde(rename = "mcp_degraded", skip_serializing_if = "Option::is_none")]
    pub mcp_degraded: Option<McpDegradedReport>,
}

#[derive(Debug, Serialize)]
struct SleepOutput {
    duration_ms: u64,
    message: String,
}

#[derive(Debug, Serialize)]
struct StructuredOutputResult {
    data: String,
    structured_output: BTreeMap<String, Value>,
}

// --- run_* entry points ---

pub(crate) fn run_skill(input: SkillInput, ctx: &ToolContext) -> Result<String, ToolError> {
    let output = skill_tools::execute_skill(input)?;
    let text = to_pretty_json(&output)?;
    ctx.note_artifact_skill_read(std::path::Path::new(&output.path));
    Ok(text)
}

/// The search's own cwd is the project whose settings, ledger and skill roots
/// it reads — the one the tool context names, or this process's own when a
/// context was built without one.
fn tool_cwd(ctx: &ToolContext) -> std::path::PathBuf {
    ctx.cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

pub(crate) fn run_skill_search(
    input: &SkillSearchInput,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    let output = skill_tools::execute_skill_search(input, &tool_cwd(ctx))?;
    for skill in &output.skills {
        ctx.note_artifact_skill_read(std::path::Path::new(&skill.path));
    }
    to_pretty_json(&output)
}

/// Put an agent's own question to the Jev seat, on the road this project's
/// switch names, and print the verdict — the same struct `zo jev` renders.
pub(crate) fn run_jev(input: &JevInput, ctx: &ToolContext) -> Result<String, ToolError> {
    let question = JevQuestion::new(
        input.shape,
        &input.question,
        input.context.as_deref(),
        &input.options,
        &input.levels,
        &input.items,
    )
    .map_err(|refused| ToolError::InvalidInput(refused.0))?;
    to_pretty_json(jev_decide(&tool_cwd(ctx), JevCaller::Tool, &question))
}

pub(crate) fn run_skill_load(
    input: &SkillLoadInput,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    let output = skill_tools::execute_skill_load(input, &tool_cwd(ctx))?;
    for skill in &output.skills {
        ctx.note_artifact_skill_read(std::path::Path::new(&skill.path));
    }
    to_pretty_json(&output)
}

pub(crate) fn run_skill_distill(input: &SkillDistillInput) -> Result<String, ToolError> {
    to_pretty_json(skill_tools::execute_skill_distill(input)?)
}

pub(crate) fn run_skill_review(input: &SkillReviewInput) -> Result<String, ToolError> {
    to_pretty_json(skill_tools::execute_skill_review(input)?)
}

const BACKGROUND_AGENT_NOTE: &str = "Agent is running in the background; you will be notified when \
    it finishes. Keep making progress on other independent work meanwhile — if it completes while \
    your turn is still running, its result reaches you mid-turn at your next tool boundary as a \
    task notification; if your turn has ended, it arrives as a follow-up message. Do NOT poll the \
    output file and do NOT idle-wait; when nothing is left but waiting, end your turn. To steer it \
    mid-run, or to follow up after it finishes without losing its context, call SendMessage with its \
    name (load the schema via ToolSearch if needed). If it stops being worth its remaining cost, end \
    it with StopAgent and this agentId rather than leaving it running unwatched.";

fn running_background_agent_result(
    manifest: &AgentOutput,
    blocking_wait_timed_out: bool,
) -> Result<String, ToolError> {
    agent_tools::mark_background_agent(manifest.agent_id.clone());
    let note = if blocking_wait_timed_out {
        format!(
            "{BACKGROUND_AGENT_NOTE} The blocking wait timed out, but the agent keeps running."
        )
    } else {
        BACKGROUND_AGENT_NOTE.to_string()
    };
    to_pretty_json(json!({
        "agentId": manifest.agent_id,
        "name": manifest.name,
        "subagentType": manifest.subagent_type,
        "status": "running",
        "background": true,
        "outputFile": manifest.output_file,
        "note": note,
    }))
}

fn finish_blocking_agent_call(
    manifest: &AgentOutput,
    completion: Option<AgentCompletion>,
) -> Result<String, ToolError> {
    if completion
        .as_ref()
        .is_some_and(|completion| completion.status == "still_running")
    {
        // `notify_agent_completion` publishes every spawn through the existing
        // single-consumer channel. Marking this id is the only extra wiring a
        // detached spawn needs; `parent_session_id` already scopes the live
        // manifest consumed by the HUD and agents viewer.
        return running_background_agent_result(manifest, true);
    }
    to_pretty_json(match completion {
        Some(completion) => json!({
            "agentId": manifest.agent_id,
            "name": manifest.name,
            "subagentType": manifest.subagent_type,
            "status": completion.status,
            // Head/tail cap (background re-injection parity): the global
            // envelope truncation is a blind tail cut that loses exactly the
            // agent's conclusion.
            "result": completion
                .result
                .as_deref()
                .map(|r| core_types::text::elide_middle(r, AGENT_RESULT_RELAY_CHARS)),
            "structured": completion.structured,
            "error": completion.error,
            // What the helper cost, spelled exactly as the background
            // notification and the on-screen card spell it. A synchronous
            // `Agent` call is the same delegation; the model should not have
            // to learn two grammars for one fact.
            "summary": completion.run.summary(),
            "outputFile": manifest.output_file,
        }),
        // Defensive fallback for a backend that returns no completion at all:
        // hand back the output-file path and a live brief.
        None => json!({
            "agentId": manifest.agent_id,
            "name": manifest.name,
            "subagentType": manifest.subagent_type,
            "status": "still_running",
            "outputFile": manifest.output_file,
            "live": agent_tools::agent_live_brief(&manifest.manifest_file),
        }),
    })
}

pub(crate) fn run_agent(
    input: AgentInput,
    parent_model: Option<&str>,
    parent_lsp: Option<&LspRegistry>,
    hook_config: Option<&RuntimeHookConfig>,
) -> Result<String, ToolError> {
    // Background mode: detach and return AT spawn time so the main model can
    // keep working (and the user keep chatting) while the agent runs. The real
    // result is PUSHED back into the conversation by the foreground REPL when
    // the agent completes — the host marks this id via `mark_background_agent`;
    // a completion during a live turn is folded in mid-turn at the next
    // tool-result boundary (CC task-notification parity), and one arriving
    // while the host is idle re-injects as a fresh turn. Crucially this is
    // push-based, not poll-based: the `note` tells the model NOT to poll the
    // output file, which is exactly the sleep+cat polling the synchronous path
    // below was introduced to kill.
    if input.background.unwrap_or(false) {
        let manifest = agent_tools::execute_agent_with_parent_model_and_hooks(
            input,
            parent_model,
            parent_lsp,
            hook_config,
        )?;
        return running_background_agent_result(&manifest, false);
    }
    // Block until the sub-agent finishes and return its result inline — like
    // `SpawnMultiAgent` and Claude Code's `Task`. The old behavior returned at
    // spawn time (`status: "running"` + an output-file path), so the model had
    // to poll the file with `sleep`+`cat`; that polling multiplied foreground
    // requests and tripped the shared provider rate limit even though the
    // sub-agent runs on the same account quota.
    let (manifest, completion) =
        agent_tools::execute_agent_blocking(input, parent_model, parent_lsp, hook_config)?;
    finish_blocking_agent_call(&manifest, completion)
}

pub(crate) fn run_tool_search(
    input: &ToolSearchInput,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    to_pretty_json(execute_tool_search(input, ctx))
}

pub(crate) fn run_council(input: &CouncilInput) -> Result<String, ToolError> {
    to_pretty_json(council::execute_council(input)?)
}

pub(crate) fn run_notebook_edit(input: NotebookEditInput) -> Result<String, ToolError> {
    to_pretty_json(notebook_tools::execute_notebook_edit(input)?)
}

pub(crate) fn run_sleep(input: &SleepInput) -> Result<String, ToolError> {
    to_pretty_json(execute_sleep(input)?)
}

/// `send_to_user` (and legacy aliases): push verbatim content to the user
/// mid-run without ending the turn.
///
/// With a live channel the message becomes a `UserNotice` block and the tool
/// returns `{delivered:true}`; with no interactive surface (headless runs,
/// sub-agents whose fresh context carries no channel, or a channel that does
/// not support the push) it degrades to an inline echo — `{delivered:false,
/// message}` — so the content is never silently lost. A sub-agent thus reports
/// the text back to its parent instead of barging into the parent's TUI.
pub(crate) fn run_send_to_user(
    input: SendToUserInput,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    if input.message.trim().is_empty() {
        return Err(ToolError::InvalidInput("'message' must not be empty".into()));
    }

    // Cap at the input boundary (the render block never truncates). Truncate on
    // a char boundary so multi-byte content is never split mid-codepoint.
    let (message, truncated) = if input.message.chars().count() > MAX_SEND_TO_USER_CHARS {
        let kept: String = input.message.chars().take(MAX_SEND_TO_USER_CHARS).collect();
        (
            format!("{kept}\n\n[send_to_user: truncated at {MAX_SEND_TO_USER_CHARS} chars]"),
            true,
        )
    } else {
        (input.message, false)
    };

    match ctx
        .user_question_channel()
        .as_deref()
        .map(|channel| channel.send_to_user(&message))
    {
        Some(Ok(())) => to_pretty_json(json!({
            "delivered": true,
            "truncated": truncated,
            "info": "Pushed to the user; the turn continues.",
        })),
        // No channel, or a channel that cannot push: echo inline so the content
        // survives. Headless / sub-agent runs land here (fresh context, no TUI).
        Some(Err(_)) | None => to_pretty_json(json!({
            "delivered": false,
            "truncated": truncated,
            "message": message,
            "note": "no interactive surface; returned inline",
        })),
    }
}

/// Push a one-line notification toward the person, and report the road.
///
/// The shaping (one line, the table's cap) happens here, once, so every
/// surface delivers the same bytes the receipt shows. The judgement of WHICH
/// road belongs to the surface: only it knows whether a key was just pressed
/// or a window is subscribed. No surface at all is `skipped: nowhere`, the
/// same honest answer `send_to_user` gives, with the message echoed inline so
/// nothing is lost.
pub(crate) fn run_push_notification(
    input: &PushNotificationInput,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    use push_notification::{PushLimits, PushNotice, PushRoad};

    if let Some(status) = input
        .status
        .as_deref()
        .filter(|status| *status != PUSH_STATUS_PROACTIVE)
    {
        return Err(ToolError::InvalidInput(format!(
            "'status' must be \"{PUSH_STATUS_PROACTIVE}\", got {status:?}"
        )));
    }
    let limits = PushLimits::from_env();
    let (body, truncated) = push_notification::shape_message(&input.message, &limits)?;
    let notice = PushNotice {
        title: push_title(ctx),
        body,
    };
    let road = match ctx
        .user_question_channel()
        .as_deref()
        .map(|channel| channel.push_notification(&notice))
    {
        Some(Ok(road)) => road,
        Some(Err(_)) | None => PushRoad::SkippedNowhere,
    };
    to_pretty_json(json!({
        "road": road.as_str(),
        "delivered": road.delivered(),
        "message": notice.body,
        "truncated": truncated,
        "note": push_notification::road_note(road),
    }))
}

/// The only `status` the schema admits — Claude Code's word for "the model
/// asked on its own".
const PUSH_STATUS_PROACTIVE: &str = "proactive";

/// The notice's title: which zo is calling, by the directory it works in —
/// the word a terminal shows above the body (OSC 777) and a person with
/// several panes needs to tell them apart.
fn push_title(ctx: &ToolContext) -> String {
    let here = ctx
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok());
    match here
        .as_deref()
        .and_then(std::path::Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
    {
        Some(name) => format!("zo · {name}"),
        None => "zo".to_string(),
    }
}

pub(crate) fn run_synthetic_output(input: &SyntheticOutputInput) -> Result<String, ToolError> {
    to_pretty_json(json!({
        "tool_name": input.tool_name,
        "output": input.output,
        "injected": true,
    }))
}

#[allow(clippy::too_many_lines)]
pub(crate) fn run_spawn_multi_agent(
    input: &SpawnMultiAgentInput,
    parent_model: Option<&str>,
    parent_lsp: Option<&LspRegistry>,
    hook_config: Option<&RuntimeHookConfig>,
) -> Result<String, ToolError> {
    run_spawn_multi_agent_with_timeout_and_hooks(
        input,
        parent_model,
        parent_lsp,
        SPAWN_MULTI_AGENT_WAIT_TIMEOUT,
        None,
        false,
        hook_config,
    )
}

#[allow(dead_code, clippy::too_many_lines)]
pub(crate) fn run_spawn_multi_agent_with_timeout(
    input: &SpawnMultiAgentInput,
    parent_model: Option<&str>,
    parent_lsp: Option<&LspRegistry>,
    wait_timeout: Duration,
    agent_time_budget: Option<Duration>,
    workflow_member: bool,
) -> Result<String, ToolError> {
    run_spawn_multi_agent_with_timeout_and_hooks(
        input,
        parent_model,
        parent_lsp,
        wait_timeout,
        agent_time_budget,
        workflow_member,
        None,
    )
}

#[allow(clippy::too_many_lines)]
pub(crate) fn run_spawn_multi_agent_with_timeout_and_hooks(
    input: &SpawnMultiAgentInput,
    parent_model: Option<&str>,
    parent_lsp: Option<&LspRegistry>,
    wait_timeout: Duration,
    agent_time_budget: Option<Duration>,
    workflow_member: bool,
    hook_config: Option<&RuntimeHookConfig>,
) -> Result<String, ToolError> {
    if input.agents.is_empty() {
        return Err(ToolError::InvalidInput(
            "agents list must not be empty".to_string(),
        ));
    }
    // Every member is written into — and every id-keyed read below goes
    // through — the spawning session's registry (t-2511).
    let registry = input
        .registry
        .clone()
        .unwrap_or_else(agent_tools::AgentRegistry::unowned_from_cwd);
    if input.agents.len() > MAX_SPAWN_MULTI_AGENT_AGENTS {
        return Err(ToolError::InvalidInput(format!(
            "SpawnMultiAgent accepts at most {MAX_SPAWN_MULTI_AGENT_AGENTS} agents (got {})",
            input.agents.len()
        )));
    }

    // The lane count this batch ran, captured once before any member spawns:
    // the shape stamped on every member's outcome row.
    let fanout_width = input.agents.len();
    let mut spawned: Vec<Value> = Vec::with_capacity(input.agents.len());
    let mut errors: Vec<Value> = Vec::new();
    let mut name_by_id: BTreeMap<String, String> = BTreeMap::new();
    // Phase 4 verdict channel — source #2: the inverse of `name_by_id`,
    // populated as each member spawns (see below), so a LATER member in the
    // batch whose `__zo_route_judged_agent` smuggle names an EARLIER
    // member can resolve that name to that member's attempt (its spawned
    // manifest). A worker spawned in a later chunk than its reviewer is
    // unresolvable (`None`) by construction — the ordering constraint this
    // map encodes.
    let mut attempt_by_name: BTreeMap<String, agent_tools::AgentOutput> = BTreeMap::new();
    let mut completions: Vec<AgentCompletion> = Vec::new();

    // Auto worktree-isolation (tracks 3-3 + 4-3): when the workspace guard is
    // opt-in enabled and this is a real multi-agent fan-out, place each agent in
    // its own git worktree so parallel (and heterogeneous cross-provider) editors
    // never clobber the shared tree, then merge each change-set back after the
    // batch barrier. A git failure leaves the provider unset → the fan-out runs
    // un-isolated (honest fallback), exactly as the workflow engine degrades.
    let auto_isolate =
        fanout_isolation::should_auto_isolate(&input.agents, crate::workspace_guard_enabled());
    let worktree_provider: Option<crate::workflow_tools::worktree::GitWorktreeProvider> =
        auto_isolate
            .then(crate::workflow_tools::worktree::GitWorktreeProvider::new)
            .and_then(Result::ok);
    let heterogeneous = auto_isolate && fanout_isolation::is_heterogeneous(&input.agents);
    let mut isolated_count = 0usize;

    // Permit-before-spawn windowing (BUG-R11/D4): spawn at most `window` agents,
    // then, the instant one finishes, refill its slot with the next pending
    // agent — so a flat fan-out never holds more than `window` live agent
    // threads, yet a slow agent never delays the START of a sibling while a slot
    // is free (no chunk barrier). With the default window (all at once, up to the
    // cap) every agent spawns before the first wait, exactly as before; a smaller
    // `concurrency` opts into tighter bounding without the head-of-line stall.
    let window = effective_spawn_window(input.concurrency, input.agents.len());
    // Ids currently in flight (bounded by `window`); a completed id is removed as
    // its slot is reclaimed. Worktree guards accumulate across the whole run and
    // are merged back once, in spawn order, after every agent has reached a
    // terminal/cancelled state — the flat analog of the per-window teardown,
    // preserving merge ordering.
    let mut in_flight: Vec<String> = Vec::with_capacity(window);
    let mut all_guards: Vec<(
        String,
        Box<dyn crate::workflow_tools::worktree::WorktreeGuard>,
    )> = Vec::new();
    // ONE overall deadline for the whole fan-out, not one per reclaim: a rolling
    // window over N agents reclaims `N - window` times, so a per-call
    // `wait_timeout` would let the total collection budget grow with the agent
    // count. Every reclaim and the final drain share this single deadline (and
    // one shared progress-extension counter), so the public collection budget
    // stays bounded by `wait_timeout` plus at most
    // `SPAWN_DEADLINE_MAX_EXTENSIONS` progress-gated extensions — granted only
    // while a cancellation candidate is demonstrably still working — regardless
    // of fan-out width.
    let mut overall_deadline = std::time::Instant::now() + wait_timeout;
    // An extension buys a *working agent* time to finish; it is not more room to
    // start work. Gating spawns on the extendable deadline handed that borrowed
    // time to fresh workers, which then get cancelled at the drain — so the
    // spawn gate keeps the deadline the caller asked for.
    let spawn_deadline = overall_deadline;
    let mut deadline_extensions: u32 = 0;
    {
        for (idx, agent_val) in input.agents.iter().enumerate() {
            // Never spawn a fresh worker once the overall deadline has elapsed:
            // doing so could push the live-worker count over `window` (a slot
            // freed by a *synthetic* terminal may still back a physically-live
            // worker) and would burn budget past the public collection deadline.
            // Instead record an explicit, ordered timeout error for this input so
            // result cardinality and input order are preserved (aggregation keys
            // both `spawned` and `errors` by `index`).
            if std::time::Instant::now() >= spawn_deadline {
                errors.push(serde_json::json!({
                    "index": idx,
                    "error": "fan-out overall deadline elapsed before this agent was spawned",
                }));
                continue;
            }
            // Reclaim a slot before opening the next one: block (up to the shared
            // overall deadline) until at least one in-flight agent reaches a
            // terminal completion, drain those into `completions`, and free their
            // slots. If the deadline passes with nothing terminal, the oldest
            // agent is cancelled+salvaged to a terminal state BEFORE its slot is
            // reused — so a freed slot never leaves a live worker running, and the
            // live worker count is always <= `window`. Only engages once `window`
            // agents are already live, so the first `window` agents all spawn
            // back-to-back with no wait between them.
            if in_flight.len() >= window {
                reclaim_spawn_slots(
                    &registry,
                    &mut in_flight,
                    &mut completions,
                    &mut overall_deadline,
                    &mut deadline_extensions,
                );

                // `reclaim_spawn_slots` may have blocked all the way to the
                // overall deadline before cancelling the oldest agent. Two hazards
                // remain that the top-of-loop check could not see, because they
                // only become true *after* that blocking wait:
                //
                //   1. The deadline may now have elapsed — spawning here would
                //      burn budget past the public collection deadline.
                //   2. The reclaimed slot was freed by a *cooperative* cancel, so
                //      that worker may still be physically live (its worktree is
                //      still owned by a live editor). Spawning a fresh worker now
                //      would push the real live-worker count over `window`.
                //
                // Recheck both against the authoritative physical-liveness signal
                // and, if either still bars a spawn, record an ordered timeout
                // error for this input instead of spawning — preserving input
                // order and result cardinality (aggregation keys by `index`).
                if spawn_barred_after_reclaim(&in_flight, window, spawn_deadline) {
                    errors.push(serde_json::json!({
                        "index": idx,
                        "error": "fan-out overall deadline elapsed before a worker slot became free",
                    }));
                    continue;
                }
            }
            let prompt = agent_val
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Information-topology edges on the fan-out path. This was pinned to
            // an empty vec, so the one axis that lets an agent read a finished
            // agent's deliverable engine-side was unreachable from the path
            // carrying ~72% of all sub-agent spawns — leaving the parent to
            // re-narrate that deliverable into the next prompt and pay output
            // tokens for text the engine already holds, which is the exact cost
            // the `sees` edge exists to remove.
            let sees = agent_val
                .get("sees")
                .and_then(Value::as_array)
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let subagent_type = agent_val
                .get("subagent_type")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let description = agent_val
                .get("description")
                .and_then(|v| v.as_str())
                .map_or_else(|| format!("parallel-agent-{idx}"), str::to_string);
            let name = agent_val
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            // Per-agent model override (BUG-D5): the model can route each
            // sub-agent explicitly instead of every agent sharing one model.
            // Honored by `resolve_agent_model_selection`; `None` inherits the
            // parent model (CC parity).
            let model = agent_val
                .get("model")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            // Explicit, transcript-visible escape hatch for a cross-provider
            // `model` — reserved for when the user asked for that model (see
            // `AgentInput::allow_cross_provider`).
            let allow_cross_provider = agent_val
                .get("allow_cross_provider")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            // Smart-router WHY, smuggled next to the injected "model" by
            // `apply_smart_models_to_spawn_input` — stamped onto the manifest
            // so the TUI can show the routing decision.
            let route_reason = agent_val
                .get(smart_router::ROUTE_REASON_SMUGGLE_KEY)
                .and_then(|v| v.as_str())
                .map(str::to_string);
            // Resolved smart-route model, smuggled alongside the reason.
            // SECURITY: trusted here ONLY because `apply_smart_models_to_spawn_input`
            // runs first on this same input and scrubs any caller-supplied
            // `__zo_route_model` before (re)inserting a host-computed value —
            // so this is never an untrusted craft. The spawn path then honors it
            // verbatim (already gated to the connected inventory by `route_model`),
            // including a deliberate cross-provider route for a diversity role.
            // `None` inherits the parent model.
            let route_model = agent_val
                .get(smart_router::ROUTE_MODEL_SMUGGLE_KEY)
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let route_fallback_models = agent_val
                .get(smart_router::ROUTE_FALLBACK_MODELS_SMUGGLE_KEY)
                .and_then(|v| v.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str())
                        .map(str::trim)
                        .filter(|model| !model.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            // Recommended effort tier, smuggled alongside the model/reason.
            // SECURITY: trusted here for the same reason as `route_model` —
            // `apply_smart_models_to_spawn_input` scrubs any caller-supplied
            // `__zo_route_effort` before (re)inserting a host-computed
            // value. `EffortLevel` deserializes from its lowercase wire token
            // (`#[serde(rename_all = "lowercase")]`), so this parses the exact
            // string `apply.rs` serialized. `None` = no recommendation.
            let route_effort = agent_val
                .get(smart_router::ROUTE_EFFORT_SMUGGLE_KEY)
                .cloned()
                .and_then(|value| serde_json::from_value::<api::EffortLevel>(value).ok());
            // P3 v2 route-decision metadata (role/complexity/risk/routeSource),
            // smuggled as one JSON object alongside the other route keys.
            // SECURITY: trusted here for the same reason as `route_model` —
            // `apply_smart_models_to_spawn_input` scrubs any caller-supplied
            // `__zo_route_decision_meta` before (re)inserting a
            // host-computed value. Purely descriptive (stamped onto the
            // manifest / route-outcome record, never re-gates a trust
            // boundary), so a missing/malformed field just stays absent.
            let route_decision_meta = agent_val.get(smart_router::ROUTE_DECISION_META_SMUGGLE_KEY);
            let route_role = route_decision_meta
                .and_then(|value| value.get("role"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let route_complexity = route_decision_meta
                .and_then(|value| value.get("complexity"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let route_risk = route_decision_meta
                .and_then(|value| value.get("risk"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let route_source = route_decision_meta
                .and_then(|value| value.get("routeSource"))
                .and_then(Value::as_str)
                .map(str::to_string);
            // Phase 4 verdict channel — source #2: resolve the planner-bound
            // judged worker's NAME (smuggled by
            // `apply_smart_models_to_spawn_input`) to that worker's attempt
            // via `attempt_by_name`, populated as EARLIER members in this
            // same batch spawn (see below) — bound here, as the reviewer
            // spawns, so its verdict names the attempt the worker was spawned
            // as, not a later resume of it. Unresolvable (worker not
            // yet spawned, or the smuggle key is absent) stays `None` —
            // ambiguous binding must never guess.
            let judged_agent = agent_val
                .get(smart_router::ROUTE_JUDGED_AGENT_SMUGGLE_KEY)
                .and_then(Value::as_str)
                .and_then(|worker_name| attempt_by_name.get(worker_name).cloned());

            if prompt.is_empty() {
                errors.push(json!({"index": idx, "error": "prompt is empty"}));
                continue;
            }

            // Per-agent isolated worktree, when auto-isolation engaged. A failed
            // `create` degrades just this agent to the shared cwd (counted for an
            // honest note), never aborting the fan-out.
            let cwd = worktree_provider.as_ref().and_then(|provider| {
                use crate::workflow_tools::worktree::WorktreeProvider as _;
                provider
                    .create(name.as_deref().unwrap_or("agent"))
                    .ok()
                    .map(|guard| {
                        isolated_count += 1;
                        let path = guard.path().to_path_buf();
                        // The guard is parked until the run's end merge-back; its
                        // id is filled in once the spawn returns an agent id.
                        all_guards.push((String::new(), guard));
                        path
                    })
            });
            let guard_slot = cwd.is_some().then(|| all_guards.len() - 1);
            // Captured before `name` moves into `agent_input` below, so a
            // LATER sibling's judged-agent smuggle can resolve to THIS
            // member's agent id once the spawn below succeeds.
            let name_for_judged_lookup = name.clone();

            let agent_input = agent_tools::AgentInput {
                route_probe_confidence: None,
                fork_source: None,
                allow_cross_provider,
                description,
                prompt,
                subagent_type,
                name,
                // Fan-out/workflow member names are engine-generated
                // (`plan:f3`, indexed display names) and must keep the
                // display-slug path; only the `Agent` tool's model-supplied,
                // CC-validated name is carried verbatim as an address.
                addressable_name: None,
                model,
                cwd,
                schema: None,
                workflow_member,
                // Every member of a batch is a lane: its answer is read here,
                // with its siblings', and its pane is released on that read.
                one_shot: true,
                // The batch counted its own lanes; a reader of the outcome
                // ledger must never have to re-count them off the agent store.
                plan_shape: Some(input.plan_shape.unwrap_or(
                    runtime::PlanShape::Parallel {
                        width: u32::try_from(fanout_width).unwrap_or(u32::MAX),
                    },
                )),
                route_tax: None,
                // Fan-out members are always collected synchronously; background
                // detach is a single-`Agent`-tool affordance only.
                background: Some(false),
                parent_permission_mode: input.parent_permission_mode,
                parent_session_id: input.parent_session_id.clone(),
                registry: input.registry.clone(),
                tool_call_id: input.tool_call_id.clone(),
                mcp_passthrough: input.mcp_passthrough.clone(),
                // The per-call `concurrency` now also caps real provider-request
                // concurrency via the adaptive governor, not just OS-thread spawn
                // windowing — so a tighter value genuinely throttles the API.
                api_concurrency: input.concurrency,
                time_budget: agent_time_budget,
                prior_failures: 0,
                route_reason,
                route_model,
                route_fallback_models,
                route_effort,
                route_role,
                route_complexity,
                route_risk,
                route_source,
                judged_agent,
                verify_loop: None,
                sees,
            };
            match agent_tools::execute_agent_with_parent_model_and_hooks(
                agent_input,
                parent_model,
                parent_lsp,
                hook_config,
            ) {
                Ok(output) => {
                    let display_name = output.label.as_deref().unwrap_or(&output.name).to_string();
                    // Tag the parked worktree guard with this agent's id so the
                    // end-of-run merge-back attributes its patch correctly.
                    if let Some(slot) = guard_slot {
                        all_guards[slot].0.clone_from(&output.agent_id);
                    }
                    in_flight.push(output.agent_id.clone());
                    name_by_id.insert(output.agent_id.clone(), display_name.clone());
                    // Phase 4 verdict channel — source #2: make THIS member's
                    // attempt resolvable by name for any LATER sibling's
                    // judged-agent smuggle in the same batch.
                    if let Some(worker_name) = name_for_judged_lookup {
                        attempt_by_name.insert(worker_name, output.clone());
                    }
                    spawned.push(json!({
                        "index": idx,
                        "agentId": output.agent_id,
                        "name": output.name,
                        "label": display_name,
                    }));
                }
                Err(e) => {
                    errors.push(json!({"index": idx, "error": e.to_string()}));
                }
            }
        }
        // Drain the agents still in flight after the last spawn, then guarantee
        // every one of them has reached a terminal/cancelled state *before* any
        // worktree is collected or dropped below. Waiting is bounded by the same
        // shared overall deadline as the reclaims, so the collection budget never
        // grows with the agent count; any agent still live at the deadline is
        // cancelled+salvaged here (not merely observed as `still_running`), so no
        // worktree merge-back/drop can race a live editor and tear a patch.
        drain_or_cancel_remaining_agents(
            &registry,
            &mut in_flight,
            &mut completions,
            &mut overall_deadline,
            &mut deadline_extensions,
        );
        // Merge each isolated agent's change-set back into the main tree in spawn
        // order (same 3-way apply the workflow engine uses), then drop the guards
        // to tear down every worktree at once — but ONLY for workers that have
        // physically exited. A cooperative cancel writes a terminal `stopped`
        // manifest immediately while the worker keeps running until it observes
        // the abort (see `cancel_and_salvage_agent`), so the *generation-bound*
        // `agent_worker_generation_is_live` (backed by the still-registered cancel
        // signal for this exact run generation) is the real exit ack. Collecting
        // or dropping a still-live worker's worktree here would race a live editor
        // and remove a live worktree; those are handed to a background owner that
        // tears them down only after that exact worker generation actually exits,
        // and never merges their late change-set back (quarantine, not apply).
        if let Some(provider) = worktree_provider.as_ref() {
            let mut ready: Vec<(String, Box<dyn crate::workflow_tools::worktree::WorktreeGuard>)> =
                Vec::new();
            for (agent_id, guard) in std::mem::take(&mut all_guards) {
                if !agent_id.is_empty()
                    && agent_tools::agent_worker_generation_is_live(
                        &agent_id,
                        agent_tools::AGENT_INITIAL_RUN_GENERATION,
                    )
                {
                    spawn_deferred_worktree_cleanup(
                        agent_id,
                        agent_tools::AGENT_INITIAL_RUN_GENERATION,
                        guard,
                        DEFERRED_WORKTREE_CLEANUP_POLL,
                    );
                } else {
                    ready.push((agent_id, guard));
                }
            }
            merge_back_fanout_worktrees(provider, &ready, &mut errors);
        }
        drop(all_guards);
    }

    // 결과 합치기 — index 순서 유지를 위해 spawn 시 받은 idx 와 매칭.
    let mut completed_count = 0usize;
    let mut failed_count = 0usize;
    let mut stopped_count = 0usize;
    let mut timed_out_count = 0usize;
    // Fair share of the global tool-output envelope per agent, head/tail
    // preserved: the global cap is a blind tail cut on the ASSEMBLED JSON, so
    // without a per-agent budget one verbose early agent evicted every later
    // agent's result mid-JSON. The floor keeps a wide fan-out from squeezing
    // each agent below usefulness; the ceiling matches the background
    // re-injection cap.
    let per_agent_result_budget = (runtime::TruncationConfig::default().default_max_chars
        / spawned.len().max(1))
    .clamp(3_000, AGENT_RESULT_RELAY_CHARS);
    let agents_results: Vec<Value> = spawned
        .iter()
        .map(|s| {
            let agent_id = s
                .get("agentId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            // Only a terminal completion (completed/failed/stopped) is a real
            // result. A `still_running` placeholder means the collection window
            // elapsed before the worker finished — spawned fan-out agents have
            // no wall-clock deadline of their own, so left alone they run to the
            // iteration/tool-call cap while the parent gets nothing back.
            let terminal = completions.iter().find(|c| {
                c.agent_id == agent_id && agent_tools::agent_output_status_is_terminal(&c.status)
            });
            let (status, result, error) = if let Some(c) = terminal {
                match c.status.as_str() {
                    "completed" => completed_count += 1,
                    "failed" => failed_count += 1,
                    _ => stopped_count += 1,
                }
                (
                    c.status.clone(),
                    c.result.as_deref().map(|r| core_types::text::elide_middle(r, per_agent_result_budget)),
                    c.error.clone(),
                )
            } else {
                // Cancel the overran worker (cooperative stop) and salvage
                // whatever it has streamed (`outputTail`) as a partial result,
                // instead of discarding all its work behind a bare
                // `still_running` with no result. Keep the cancel signal
                // registered: a background worktree-cleanup owner may already be
                // gating teardown on this worker's physical exit, and
                // unregistering here would zo a premature exit ack.
                timed_out_count += 1;
                let partial = agent_tools::cancel_and_salvage_agent_keep_worker_registered(
                    &registry,
                    &agent_id,
                    "agent exceeded spawn collection timeout",
                );
                ("timed_out".to_string(), partial, None)
            };
            // A timed-out agent carries a live brief (phase / current tool /
            // heartbeat) so the summary says how far it got.
            let live = (status == "timed_out")
                .then(|| agent_tools::agent_live_brief_by_id(&registry, &agent_id))
                .flatten();
            json!({
                "index": s.get("index"),
                "agentId": agent_id,
                "name": s.get("name"),
                "status": status,
                "result": result,
                "error": error,
                "live": live,
            })
        })
        .collect();

    let mut summary = json!({
        "status": "completed",
        "spawned_count": spawned.len(),
        "completed": completed_count,
        "failed": failed_count,
        "stopped": stopped_count,
        "timed_out": timed_out_count,
        "error_count": errors.len(),
        "agents": agents_results,
        "errors": errors,
    });
    // Surface auto worktree-isolation (tracks 3-3 + 4-3) so the run is honest
    // about whether parallel editors were sandboxed and merged back. Omitted
    // entirely when isolation did not engage, keeping the common output stable.
    if isolated_count > 0 {
        summary["isolation"] = json!("worktree");
        summary["isolated_agents"] = json!(isolated_count);
        summary["heterogeneous"] = json!(heterogeneous);
    }
    // When the collection window elapsed before an agent finished, we cancelled
    // it and recovered its partial streamed output (if any) into `result`.
    if timed_out_count > 0 {
        summary["note"] = json!(
            "Some agents did not finish within the collection window; they were \
             cancelled and any partial streamed output was recovered into `result`."
        );
    }
    to_pretty_json(summary)
}

fn wait_for_spawned_agent_completions(
    agent_ids: &[String],
    wait_timeout: Duration,
) -> Vec<AgentCompletion> {
    if agent_ids.is_empty() {
        Vec::new()
    } else {
        agent_tools::wait_for_agent_completions(agent_ids, wait_timeout)
    }
}

/// Final drain for the live fan-out scheduler: bring **every** still-in-flight
/// agent to a terminal/cancelled state before the caller collects or drops any
/// worktree.
///
/// First it waits (bounded by the shared overall `deadline`) for agents that
/// finish on their own, draining their terminal completions and clearing them
/// from `in_flight`. When the deadline passes with live stragglers, any
/// straggler whose manifest heartbeat shows fresh activity earns the whole set
/// a bounded deadline extension (shared cap in `extensions_used`, same gate as
/// the mid-loop reclaim) — this was the site that killed a healthy long-running
/// agent mid-work at the fixed wall budget. Only when every straggler is stale,
/// or the extension cap is spent, are the survivors cancelled+salvaged via
/// [`reclaim_cancel_and_collect`], which persists a terminal record and drains
/// it. On return `in_flight` is empty and no listed agent has a live editor, so
/// the subsequent per-agent worktree `collect_patch`/`drop` cannot race a writer
/// and tear a patch. Result cardinality/order is unaffected: aggregation iterates
/// the spawned set, and each agent now carries a real terminal completion.
fn drain_or_cancel_remaining_agents(
    registry: &agent_tools::AgentRegistry,
    in_flight: &mut Vec<String>,
    completions: &mut Vec<AgentCompletion>,
    deadline: &mut std::time::Instant,
    extensions_used: &mut u32,
) {
    loop {
        if in_flight.is_empty() {
            return;
        }
        let remaining = deadline
            .saturating_duration_since(std::time::Instant::now())
            .max(Duration::ZERO);
        let observed = wait_for_spawned_agent_completions(in_flight, remaining);
        let terminal: std::collections::HashSet<String> = observed
            .iter()
            .filter(|c| agent_tools::agent_output_status_is_terminal(&c.status))
            .map(|c| c.agent_id.clone())
            .collect();
        if !terminal.is_empty() {
            completions.extend(
                observed
                    .into_iter()
                    .filter(|c| terminal.contains(&c.agent_id)),
            );
            in_flight.retain(|id| !terminal.contains(id));
        }
        if in_flight.is_empty() {
            return;
        }
        // The deadline elapsed with live stragglers. A straggler that is still
        // demonstrably working keeps the set alive for one more bounded step.
        if reclaim_should_extend_deadline(any_agent_shows_progress(registry, in_flight), *extensions_used) {
            *extensions_used += 1;
            *deadline = std::time::Instant::now() + SPAWN_DEADLINE_EXTENSION_STEP;
            continue;
        }
        // Anything still live is driven terminal here so its worktree is safe
        // to collect. Drain the vector so callers see an empty in-flight set.
        for agent_id in std::mem::take(in_flight) {
            reclaim_cancel_and_collect(registry, &agent_id, completions);
        }
        return;
    }
}

/// Poll slice used when waiting for *any one* in-flight agent to finish. Small
/// enough that a slot is reclaimed promptly after a completion lands, large
/// enough not to busy-spin. The overall wait is still bounded by `wait_timeout`.
const SPAWN_SLOT_RECLAIM_POLL: Duration = Duration::from_millis(25);

/// Progress gate for the shared fan-out deadline. When the deadline elapses
/// while the cancellation candidate is still demonstrably working (manifest
/// heartbeat within this window), the deadline is extended instead of the
/// agent being cancelled: killing a mid-work agent discards its entire
/// deliverable to save wall-clock, and long-thinking models routinely exceed
/// a fixed wall budget while perfectly healthy. Streaming refreshes the
/// heartbeat continuously — reasoning deltas coalesce to one stamp every few
/// seconds, output-tail flushes are sub-second — so a live agent stays far
/// inside this window while a hung one goes stale past it.
const SPAWN_RECLAIM_ACTIVITY_WINDOW: Duration = Duration::from_secs(180);

/// Hard cap on progress extensions, so the total collection budget stays
/// bounded (`wait_timeout + MAX_EXTENSIONS * EXTENSION_STEP`) instead of
/// following an agent that stays busy forever.
const SPAWN_DEADLINE_MAX_EXTENSIONS: u32 = 2;

/// Collection budget each progress extension grants, measured from the moment
/// the extension is decided (not stacked onto the stale deadline).
const SPAWN_DEADLINE_EXTENSION_STEP: Duration = Duration::from_secs(10 * 60);

/// Whether a manifest snapshot is evidence of an agent still working. Pure so
/// both progress signals are testable without a real agent: a heartbeat inside
/// [`SPAWN_RECLAIM_ACTIVITY_WINDOW`], or being blocked in a harness call the
/// worker cannot stamp from — a tool call (stamped on entry and exit only, so a
/// multi-minute build would otherwise read as idle for its whole run) or a
/// provider request that has not streamed a frame. A never-stamped heartbeat
/// with nothing in flight is no evidence at all.
const fn snapshot_shows_progress(
    seconds_since_activity: Option<u64>,
    blocked_in_harness_call: bool,
) -> bool {
    if blocked_in_harness_call {
        return true;
    }
    match seconds_since_activity {
        Some(age) => age <= SPAWN_RECLAIM_ACTIVITY_WINDOW.as_secs(),
        None => false,
    }
}

/// Decide whether an elapsed fan-out deadline is extended instead of the
/// candidate agent(s) being cancelled: only for a candidate that is still
/// making progress, and only while extensions remain against
/// [`SPAWN_DEADLINE_MAX_EXTENSIONS`].
const fn reclaim_should_extend_deadline(candidate_progressing: bool, extensions_used: u32) -> bool {
    candidate_progressing && extensions_used < SPAWN_DEADLINE_MAX_EXTENSIONS
}

/// Whether one specific agent's manifest shows progress right now.
///
/// Both in-flight claims — `current_tool` and the provider-request bracket —
/// are cleared by the worker itself, so a worker that died without reaching a
/// terminal transition leaves its manifest claiming work forever. Taken at face
/// value that corpse reads as progress, wins every extension, and holds the
/// whole fan-out's collection for the full extension budget. The cancel-signal
/// registry — which the worker unregisters at physical exit — is what says the
/// claim is still someone's to make.
fn agent_shows_progress(registry: &agent_tools::AgentRegistry, agent_id: &str) -> bool {
    agent_tools::agent_progress_snapshot(registry, agent_id).is_some_and(|snapshot| {
        let claims_work = snapshot.inside_tool_call || snapshot.awaiting_provider;
        let working = claims_work && agent_tools::agent_worker_is_live(agent_id);
        snapshot_shows_progress(snapshot.seconds_since_activity, working)
    })
}

/// Whether ANY still-in-flight agent shows progress, for the final drain: one
/// demonstrably working straggler is enough to keep waiting for the whole set,
/// because the deadline is shared and a per-agent extension is not expressible.
fn any_agent_shows_progress(registry: &agent_tools::AgentRegistry, in_flight: &[String]) -> bool {
    in_flight.iter().any(|id| agent_shows_progress(registry, id))
}

/// Count the fan-out members that are still *physically* live.
///
/// `in_flight` is the scheduler's slot bookkeeping: an id is removed the moment a
/// slot is reclaimed, even when the reclaim was a *cooperative* cancel whose
/// worker has not yet observed the abort and is still editing its worktree. So
/// `in_flight.len()` undercounts real live workers. This counts the ids whose
/// exact initial-generation worker is still registered (the authoritative
/// physical-liveness signal), which is what the window cap must be measured
/// against before spawning a fresh worker.
fn live_worker_count(in_flight: &[String]) -> usize {
    in_flight
        .iter()
        .filter(|id| {
            agent_tools::agent_worker_generation_is_live(
                id,
                agent_tools::AGENT_INITIAL_RUN_GENERATION,
            )
        })
        .count()
}

/// Whether opening a fresh worker slot is barred right after a reclaim attempt.
///
/// `reclaim_spawn_slots` may block to the overall deadline and then free a slot
/// via a *cooperative* cancel whose worker is still physically live. Spawning
/// then would either burn budget past the public collection deadline or push the
/// real live-worker count over `window` (the reclaimed slot's worker still holds
/// a registered signal). This is the exact decision the execute loop makes after
/// reclaiming; extracted so it can be exercised deterministically. A barred input
/// becomes an ordered timeout error instead of a spawn.
fn spawn_barred_after_reclaim(
    in_flight: &[String],
    window: usize,
    overall_deadline: std::time::Instant,
) -> bool {
    std::time::Instant::now() >= overall_deadline || live_worker_count(in_flight) >= window
}

/// Reclaim at least one in-flight slot for the live fan-out scheduler.
///
/// Blocks until *one or more* of the `in_flight` agents reaches a terminal
/// completion (unlike [`wait_for_spawned_agent_completions`], which drains the
/// whole set): the finished completions are moved into `completions` and their
/// ids are removed from `in_flight`, freeing the slot(s) so the caller can start
/// the next pending agent immediately. This is what turns the old chunk barrier
/// into a rolling window — a slow sibling no longer blocks the next agent's start
/// while a slot is free.
///
/// If the shared `deadline` elapses with nothing terminal, the oldest agent is
/// the cancellation candidate — but a candidate whose manifest heartbeat shows
/// fresh activity is *not* cancelled: the shared deadline is extended (bounded
/// by [`SPAWN_DEADLINE_MAX_EXTENSIONS`], counted in `extensions_used`) and the
/// wait resumes, because cancelling a demonstrably mid-work agent discards its
/// whole deliverable. Only a stale candidate — or one past the extension cap —
/// is **cancelled and salvaged to a terminal state before its slot is freed**:
/// the cancel drives that agent to a persisted terminal record, its completion
/// is drained into `completions`, and only then is its id removed from
/// `in_flight`. A freed slot therefore never leaves a live worker running, so
/// the live worker count stays at or below the window, and the reclaimed
/// agent's worktree is safe to collect (its editor is no longer live).
/// `deadline` is the single overall fan-out deadline shared by every reclaim
/// and the final drain, so the total collection budget stays bounded by
/// `wait_timeout` plus at most the capped extensions, regardless of fan-out
/// width.
/// Test seam: how many deadline extensions any reclaim in this process has
/// granted. A test that must land a sibling's completion only AFTER the
/// extension is recorded waits on this instead of betting a wall-clock sleep
/// against the scheduler — the 2026-09-14 lane flake, where a 150 ms sleep lost
/// to a starved main thread, the completion was already stored when the first
/// poll ran, and `extensions_used` read 0.
#[cfg(test)]
static RECLAIM_EXTENSIONS_GRANTED: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

fn reclaim_spawn_slots(
    registry: &agent_tools::AgentRegistry,
    in_flight: &mut Vec<String>,
    completions: &mut Vec<AgentCompletion>,
    deadline: &mut std::time::Instant,
    extensions_used: &mut u32,
) {
    if in_flight.is_empty() {
        return;
    }
    loop {
        let slice = SPAWN_SLOT_RECLAIM_POLL.min(
            deadline
                .saturating_duration_since(std::time::Instant::now())
                .max(Duration::ZERO),
        );
        let observed = agent_tools::wait_for_agent_completions(in_flight, slice);
        let terminal: std::collections::HashSet<String> = observed
            .iter()
            .filter(|c| agent_tools::agent_output_status_is_terminal(&c.status))
            .map(|c| c.agent_id.clone())
            .collect();
        if !terminal.is_empty() {
            completions.extend(
                observed
                    .into_iter()
                    .filter(|c| terminal.contains(&c.agent_id)),
            );
            in_flight.retain(|id| !terminal.contains(id));
            return;
        }
        if std::time::Instant::now() >= *deadline {
            // Nothing finished within the shared budget. Before cancelling,
            // check whether the candidate (the oldest in-flight agent) is
            // still demonstrably working — if so, grant a bounded extension
            // instead of destroying in-progress work on a fixed wall clock.
            let candidate_progressing =
                in_flight.first().is_some_and(|id| agent_shows_progress(registry, id));
            if reclaim_should_extend_deadline(candidate_progressing, *extensions_used) {
                *extensions_used += 1;
                #[cfg(test)]
                RECLAIM_EXTENSIONS_GRANTED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                *deadline = std::time::Instant::now() + SPAWN_DEADLINE_EXTENSION_STEP;
                continue;
            }
            // Reclaim the oldest slot by driving its agent to a terminal
            // manifest state. The cancel is cooperative, so the physical
            // worker may still be running after this returns; we deliberately
            // keep its cancel signal registered so `agent_worker_is_live`
            // still reports it live and its worktree teardown is deferred
            // until it actually exits. Freeing the scheduler slot here does
            // not breach `live workers <= window`: reclaim only ever runs
            // before the overall deadline, and once the deadline elapses the
            // spawn loop starts no further workers, so the count can only
            // fall.
            let oldest = in_flight.remove(0);
            reclaim_cancel_and_collect(registry, &oldest, completions);
            return;
        }
    }
}

/// Cancel/salvage a single agent whose slot is being reclaimed under deadline
/// pressure, then move its now-terminal completion into `completions`.
///
/// [`agent_tools::cancel_and_salvage_agent_keep_worker_registered`] cooperatively
/// cancels the agent and persists a terminal record (returning real output
/// untouched if the agent had already finished on its own in the race window),
/// but leaves the cancel signal registered so `agent_worker_is_live` keeps
/// reporting the physical worker live until it truly exits — which is what gates
/// deferred worktree teardown. We then drain that terminal completion so the
/// reclaimed agent carries a real terminal status into result aggregation
/// instead of being re-cancelled there. A short bounded wait covers the
/// persist/notify hop; if the record is somehow still not visible we leave the
/// id undrained and aggregation's own straggler path handles it — cardinality
/// and order are preserved either way because aggregation iterates the spawned
/// set, not `completions`.
fn reclaim_cancel_and_collect(
    registry: &agent_tools::AgentRegistry,
    agent_id: &str,
    completions: &mut Vec<AgentCompletion>,
) {
    let _ = agent_tools::cancel_and_salvage_agent_keep_worker_registered(
        registry,
        agent_id,
        "fan-out reclaimed this slot before the agent reached a terminal state",
    );
    let drained = agent_tools::wait_for_agent_completions(
        std::slice::from_ref(&agent_id.to_string()),
        SPAWN_SLOT_RECLAIM_POLL,
    );
    if let Some(terminal) = drained
        .into_iter()
        .find(|c| c.agent_id == agent_id && agent_tools::agent_output_status_is_terminal(&c.status))
    {
        completions.push(terminal);
    }
}

/// Pure fan-out schedule shared with the live scheduler above: start agents in
/// input order, holding at most `window` in flight, and the instant a slot frees
/// (`reclaim`) start the next pending agent into it. Extracted so the ordering
/// invariants — never more than `window` concurrent, and a free slot filled
/// without waiting on unrelated siblings — can be exercised deterministically
/// without real provider threads. `spawn(idx)` returns the started id; `reclaim`
/// removes one or more finished ids from the live set and is only called when the
/// window is full.
#[cfg(test)]
fn drive_fanout_schedule(
    agent_count: usize,
    window: usize,
    mut spawn: impl FnMut(usize, &[String]) -> String,
    mut reclaim: impl FnMut(&mut Vec<String>),
) -> Vec<String> {
    let window = window.max(1);
    let mut in_flight: Vec<String> = Vec::with_capacity(window);
    let mut order: Vec<String> = Vec::with_capacity(agent_count);
    for idx in 0..agent_count {
        if in_flight.len() >= window {
            reclaim(&mut in_flight);
        }
        let id = spawn(idx, &in_flight);
        in_flight.push(id.clone());
        order.push(id);
    }
    order
}

/// Poll slice for the background worktree-cleanup owner while it waits for a
/// still-live worker to physically exit. Matches the reclaim poll cadence.
const DEFERRED_WORKTREE_CLEANUP_POLL: Duration = Duration::from_millis(25);

/// A worktree guard parked in the process-global quarantine, tagged with the
/// exact `(agent_id, generation)` whose physical exit must be observed before the
/// guard may be dropped.
type QuarantinedGuard = (
    String,
    u64,
    Box<dyn crate::workflow_tools::worktree::WorktreeGuard>,
);

/// Process-global quarantine for still-live worktree guards whose dedicated
/// cleanup thread could not be spawned.
///
/// The normal path is a per-guard cleanup thread that drops the guard only after
/// its exact worker generation exits. If `thread::Builder::spawn` *fails* (thread
/// limit reached, etc.), letting the closure drop would tear down a **live**
/// worktree — so the caller instead parks the guard here and a single shared
/// quarantine owner drops it only after the same exact-generation exit ack, or
/// safely retains it until process exit. Never applies patches back.
static QUARANTINED_WORKTREE_GUARDS: OnceLock<std::sync::Mutex<Vec<QuarantinedGuard>>> =
    OnceLock::new();

fn quarantined_worktree_guards() -> &'static std::sync::Mutex<Vec<QuarantinedGuard>> {
    QUARANTINED_WORKTREE_GUARDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Test-only seam: force the next `spawn_deferred_worktree_cleanup` thread spawn
/// to fail, exercising the quarantine handoff without exhausting real OS threads.
#[cfg(test)]
static FORCE_DEFERRED_CLEANUP_SPAWN_FAILURE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Guards the single quarantine drainer thread: true while a drainer is running,
/// so concurrent quarantine calls do not spawn a second one.
static QUARANTINE_DRAINER_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Park a still-live worktree guard in the process-global quarantine and ensure a
/// single shared owner thread is draining it. Used when the per-guard cleanup
/// thread could not be spawned; the guard is dropped only after its exact
/// `(agent_id, generation)` worker exits, never while still live.
fn quarantine_worktree_guard(
    agent_id: String,
    generation: u64,
    guard: Box<dyn crate::workflow_tools::worktree::WorktreeGuard>,
    poll: Duration,
) {
    use std::sync::atomic::Ordering;

    quarantined_worktree_guards()
        .lock()
        .expect("worktree quarantine lock")
        .push((agent_id, generation, guard));

    // Ensure exactly one drainer runs: win the gate before spawning. If another
    // quarantine call already owns the drainer, this parked guard is picked up by
    // that running drainer's next sweep, so no second thread is needed.
    if QUARANTINE_DRAINER_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    // Best-effort: start the single shared drainer. If even this spawn fails,
    // release the gate and leave the guard safely parked in the static (never
    // dropped while live) until a later quarantine call starts a drainer or the
    // process exits.
    let spawned = std::thread::Builder::new()
        .name("zo-fanout-worktree-quarantine".to_string())
        .spawn(move || loop {
            {
                let mut parked = quarantined_worktree_guards()
                    .lock()
                    .expect("worktree quarantine lock");
                let mut still_parked: Vec<QuarantinedGuard> = Vec::new();
                for (id, gen, guard) in std::mem::take(&mut *parked) {
                    if agent_tools::agent_worker_generation_is_live(&id, gen) {
                        still_parked.push((id, gen, guard));
                    } else {
                        // Exact-generation exit ack observed: safe to tear down.
                        drop(guard);
                    }
                }
                *parked = still_parked;
                if parked.is_empty() {
                    // Release the gate while still holding the quarantine lock, so
                    // it is atomic with respect to `quarantine_worktree_guard`,
                    // which pushes under the same lock *before* it attempts the
                    // gate CAS. A guard parked after this drainer's last sweep is
                    // therefore either already visible above (kept in `parked`, so
                    // we would not be here) or lands only after the lock is
                    // released — in which case its pusher wins the CAS and starts a
                    // fresh drainer. No guard is ever stranded without a drainer.
                    QUARANTINE_DRAINER_RUNNING.store(false, Ordering::SeqCst);
                    return;
                }
            }
            std::thread::sleep(poll);
        });

    if spawned.is_err() {
        QUARANTINE_DRAINER_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Tear down a *still-live* fan-out worker's worktree, but **only after that
/// exact worker generation physically exits** — never before, and never on a
/// timeout.
///
/// A cooperative cancel writes a terminal `stopped` manifest immediately, but the
/// worker keeps running (and may keep writing its worktree) until it observes the
/// abort. Dropping the guard runs `git worktree remove`; doing that while the
/// worker is still editing would corrupt or delete a live worktree, so teardown
/// is gated strictly on the real physical-exit ack: `agent_worker_generation_is_live`
/// going false for *this* `(agent_id, generation)`. There is deliberately **no
/// cap**: a worker that never exits keeps its worktree owned here (a bounded,
/// safe quarantine) rather than having a live worktree yanked out from under it —
/// correctness over reclaiming the handle.
///
/// The generation binding defeats the same-id resume ABA: the cancel-signal
/// registry keys each generation separately, so a same-id resume registers a new
/// generation *alongside* the old one. This owner waits on its exact
/// `(agent_id, generation)` and stays independent of the resume — the old
/// generation remains live (and this owner keeps holding the old worktree) until
/// the old worker itself exits and unregisters, never dropped merely because a
/// new generation took the id, and never touching the new generation's worktree.
///
/// This owner **never mutates the shared working tree** (no `apply_patch`). By
/// the time a fan-out worker is cancelled for overrunning the deadline, the tool
/// has already returned its result; merging its late change-set back
/// asynchronously would race later turns and user edits and break the spawn-order
/// merge contract. A timed-out worker's change-set is therefore quarantined in
/// its worktree and dropped, not merged — reported honestly as not-merged rather
/// than applied behind the user's back.
///
/// Deadlock-safety: the owner runs on its own thread and touches only the
/// cancel-signal registry (via `agent_worker_generation_is_live`); it never
/// acquires a per-agent manifest lock, so it cannot invert the manifest →
/// cancel-signal lock order the salvage path uses. Returns the join handle so
/// tests can await deterministic teardown ordering; production callers detach it.
fn spawn_deferred_worktree_cleanup(
    agent_id: String,
    generation: u64,
    guard: Box<dyn crate::workflow_tools::worktree::WorktreeGuard>,
    poll: Duration,
) -> Option<std::thread::JoinHandle<()>> {
    // Share the guard through a handoff cell so that if the cleanup thread cannot
    // be spawned, the caller can recover the *exact same* guard and park it in the
    // quarantine — the guard is owned in exactly one place at all times and is
    // never dropped by a failed `spawn` while its worker is still live.
    let cell: std::sync::Arc<
        std::sync::Mutex<Option<Box<dyn crate::workflow_tools::worktree::WorktreeGuard>>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(Some(guard)));
    let thread_cell = std::sync::Arc::clone(&cell);
    let thread_agent_id = agent_id.clone();

    #[cfg(test)]
    let force_failure = FORCE_DEFERRED_CLEANUP_SPAWN_FAILURE
        .swap(false, std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(test))]
    let force_failure = false;

    let spawn_result = if force_failure {
        Err(std::io::Error::other("forced spawn failure (test seam)"))
    } else {
        std::thread::Builder::new()
            .name("zo-fanout-worktree-cleanup".to_string())
            .spawn(move || {
                while agent_tools::agent_worker_generation_is_live(&thread_agent_id, generation) {
                    std::thread::sleep(poll);
                }
                // Physical-exit ack observed for this exact generation: the
                // worktree is no longer live, so dropping the guard (`git worktree
                // remove`) is safe. No merge-back — the change-set is quarantined,
                // not applied. Take ownership out of the cell so the drop happens
                // exactly once, here.
                let taken = thread_cell.lock().expect("cleanup handoff lock").take();
                drop(taken);
            })
    };

    if let Ok(handle) = spawn_result {
        Some(handle)
    } else {
        // The cleanup thread never ran, so it never took the guard: recover it
        // from the cell and park it in the process-global quarantine, which drops
        // it only after the exact-generation exit ack. Guard ownership moves
        // exactly once, here — never dropped while live.
        if let Some(guard) = cell.lock().expect("cleanup handoff lock").take() {
            quarantine_worktree_guard(agent_id, generation, guard, poll);
        }
        None
    }
}

/// Merge each isolated fan-out agent's change-set back into the main working
/// tree (tracks 3-3 + 4-3), in spawn order, using the same collect-patch →
/// 3-way-apply path the workflow engine uses. Best-effort: a clean worktree
/// contributes nothing, and a patch that fails to collect or apply is recorded
/// as a per-agent error note (left for manual resolution) rather than aborting
/// — sibling change-sets still merge. Guards with no agent id (spawn failed
/// after the worktree was created) are skipped.
fn merge_back_fanout_worktrees(
    provider: &dyn crate::workflow_tools::worktree::WorktreeProvider,
    guards: &[(
        String,
        Box<dyn crate::workflow_tools::worktree::WorktreeGuard>,
    )],
    errors: &mut Vec<Value>,
) {
    for (agent_id, guard) in guards {
        if agent_id.is_empty() {
            continue;
        }
        match guard.collect_patch() {
            Ok(None) => {}
            Ok(Some(patch)) => {
                if let Err(err) = provider.apply_patch(&patch) {
                    errors.push(json!({
                        "agentId": agent_id,
                        "error": format!("worktree merge-back failed: {err}"),
                    }));
                }
            }
            Err(err) => errors.push(json!({
                "agentId": agent_id,
                "error": format!("collecting worktree changes failed: {err}"),
            })),
        }
    }
}
#[cfg(test)]
fn wait_window_label(timeout: Duration) -> String {
    let secs = timeout.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

pub(crate) fn run_config(input: ConfigInput) -> Result<String, ToolError> {
    to_pretty_json(config_tools::execute_config(input)?)
}

pub(crate) fn run_enter_plan_mode(input: EnterPlanModeInput) -> Result<String, ToolError> {
    to_pretty_json(config_tools::execute_enter_plan_mode(input)?)
}

pub(crate) fn run_exit_plan_mode(input: ExitPlanModeInput) -> Result<String, ToolError> {
    to_pretty_json(config_tools::execute_exit_plan_mode(input)?)
}

pub(crate) fn run_structured_output(input: StructuredOutputInput) -> Result<String, ToolError> {
    to_pretty_json(execute_structured_output(input)?)
}

pub(crate) fn run_ask_user_question(
    input: AskUserQuestionInput,
    channel: Option<&dyn crate::UserQuestionChannel>,
) -> Result<String, ToolError> {
    run_ask_user_question_with_terminal_state(input, channel, stdin_is_terminal())
}

fn run_ask_user_question_with_terminal_state(
    input: AskUserQuestionInput,
    channel: Option<&dyn crate::UserQuestionChannel>,
    stdin_is_terminal: bool,
) -> Result<String, ToolError> {
    let options = input.options.unwrap_or_default();
    // Multi-select only applies with a fixed choice list; a free-form prompt is
    // always a single answer, matching how the modal degrades the flag.
    let multi_select = input.multi_select && !options.is_empty();
    let raw: Vec<String> = if let Some(ch) = channel {
        ch.ask(
            &input.question,
            input.header.as_deref(),
            &options,
            multi_select,
        )?
    } else if !stdin_is_terminal {
        return to_pretty_json(json!({
            "question": input.question,
            "status": "unanswered",
            "reason": "non-interactive"
        }));
    } else {
        ask_user_question_stdio(&input.question, &options, multi_select)?
    };

    // Map any numeric picks to their labels; the TUI already returns labels, so
    // this only rewrites the stdio "type 2" form and is a no-op otherwise.
    let resolved: Vec<String> = raw
        .iter()
        .map(|answer| resolve_option_choice(answer, &options))
        .collect();

    // Single-select preserves the historical string `answer`; multi-select
    // returns the full list so the model sees every checked option.
    let answer = if multi_select {
        json!(resolved)
    } else {
        json!(resolved.into_iter().next().unwrap_or_default())
    };

    to_pretty_json(json!({
        "question": input.question,
        "answer": answer,
        "status": "answered"
    }))
}

fn stdin_is_terminal() -> bool {
    use std::io::IsTerminal;

    std::io::stdin().is_terminal()
}

pub(crate) fn run_memory_write(
    input: &MemoryWriteInput,
    ctx: &ToolContext,
) -> Result<String, ToolError> {
    let output = execute_memory_write(input, ctx)?;
    to_pretty_json(output)
}

fn execute_memory_write(input: &MemoryWriteInput, ctx: &ToolContext) -> Result<Value, ToolError> {
    let slug = normalize_memory_slug(&input.slug)?;
    let summary = sanitize_memory_summary(&input.summary)?;
    let body = input.body.trim();
    if body.is_empty() {
        return Err(ToolError::InvalidInput(
            "`body` must not be empty".to_string(),
        ));
    }
    // The model writing this memory is the one the session is running right
    // now, and the shared context cell already carries it (a `/model` switch
    // refreshes every registry clone through it). Reading it here is what keeps
    // the tag truthful without inventing a second place that tracks the model.
    let body = body_with_hand_written_metadata(body, ctx.active_model().as_deref());

    let cwd = ctx
        .cwd
        .clone()
        .map_or_else(std::env::current_dir, Ok)
        .map_err(ToolError::Io)?;
    let outcome = runtime::memory::write_hand_written_memory_entry(
        &cwd,
        input.local,
        &runtime::memory::MemoryWriteRequest {
            slug: slug.clone(),
            summary: summary.clone(),
            body,
        },
    )
    .map_err(|error| ToolError::Io(std::io::Error::other(error)))?;
    let memory_dir = runtime::memory::paths::memory_write_dir(&cwd, input.local);
    let entry_path = memory_dir.join(format!("{slug}.md"));
    let index_path = memory_dir.join("MEMORY.md");
    Ok(json!({
        "status": outcome.as_str(),
        "slug": slug,
        "local": input.local,
        "path": entry_path.display().to_string(),
        "indexPath": index_path.display().to_string()
    }))
}

fn body_with_hand_written_metadata(body: &str, active_model: Option<&str>) -> String {
    let body = strip_memory_metadata_lines(body);
    let written_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs());
    format!(
        "{}

---
{}",
        body.trim(),
        runtime::memory::MemoryClassification::hand_written(
            runtime::memory::MemoryKind::Unknown,
            written_at,
            active_model.and_then(runtime::memory::MemoryModelTag::new),
        )
        .metadata_line()
    )
}

fn sanitize_memory_summary(summary: &str) -> Result<String, ToolError> {
    let summary = summary
        .chars()
        .map(|ch| match ch {
            '[' | ']' | '(' | ')' => ' ',
            ch if ch.is_control() => ' ',
            ch => ch,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if summary.is_empty() {
        return Err(ToolError::InvalidInput(
            "`summary` must not be empty".to_string(),
        ));
    }
    Ok(summary)
}

fn strip_memory_metadata_lines(body: &str) -> String {
    body.lines()
        .filter(|line| !line.trim_start().starts_with("- memory_metadata:"))
        .collect::<Vec<_>>()
        .join("
")
}

fn normalize_memory_slug(input: &str) -> Result<String, ToolError> {
    let stem = input.trim().trim_end_matches(".md");
    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if (ch.is_ascii_whitespace() || matches!(ch, '-' | '_' | '.' | '/'))
            && !previous_dash
            && !slug.is_empty()
        {
            slug.push('-');
            previous_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        return Err(ToolError::InvalidInput(
            "`slug` must contain at least one ASCII letter or digit".to_string(),
        ));
    }
    Ok(slug)
}

/// Resolve a raw response against an options list (numeric index → label).
/// Anything that is not a valid index passes through as a free-form answer.
fn resolve_option_choice(
    response: &str,
    options: &[runtime::message_stream::QuestionOption],
) -> String {
    let trimmed = response.trim();
    if let Ok(idx) = trimmed.parse::<usize>() {
        if idx >= 1 && idx <= options.len() {
            return options[idx - 1].label.clone();
        }
    }
    trimmed.to_string()
}

/// Fallback: direct stdin/stdout I/O when no channel is configured.
///
/// Returns the raw response tokens; the caller maps any numeric picks to their
/// labels uniformly for both the stdio and channel paths. A multi-select prompt
/// accepts a comma-separated list, so several picks come back as several tokens.
fn ask_user_question_stdio(
    question: &str,
    options: &[runtime::message_stream::QuestionOption],
    multi_select: bool,
) -> Result<Vec<String>, ToolError> {
    use std::io::{self, BufRead, Write};

    let stdout = io::stdout();
    let stdin = io::stdin();
    let mut out = stdout.lock();

    writeln!(out, "\n[Question] {question}")?;

    if options.is_empty() {
        write!(out, "Your answer: ")?;
    } else {
        for (i, option) in options.iter().enumerate() {
            writeln!(out, "  {}. {}", i + 1, option.label)?;
            if let Some(description) = option.description.as_deref() {
                writeln!(out, "     {description}")?;
            }
            // The TUI shows previews in a side pane; on a plain terminal there
            // is no pane, so indent each mockup under its option rather than
            // dropping the one thing the option was meant to be compared by.
            if let Some(preview) = option.preview.as_deref() {
                for row in preview.lines() {
                    writeln!(out, "       {row}")?;
                }
            }
        }
        if multi_select {
            write!(
                out,
                "Enter choices (1-{}, comma-separated) or free text: ",
                options.len()
            )?;
        } else {
            write!(out, "Enter choice (1-{}) or free text: ", options.len())?;
        }
    }
    out.flush()?;

    let mut response = String::new();
    stdin.lock().read_line(&mut response)?;

    if multi_select && !options.is_empty() {
        Ok(response
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .collect())
    } else {
        Ok(vec![response.trim().to_string()])
    }
}

pub(crate) fn run_remote_trigger(input: RemoteTriggerInput) -> Result<String, ToolError> {
    crate::http_bridge::run_http(run_remote_trigger_async(input))
}

/// Advisory webhook helper for non-tool callers such as `zo serve`.
///
/// It reuses the `RemoteTrigger` HTTP client and response handling, but fixes the
/// method to POST and serializes `payload` as JSON. Callers should treat errors
/// as best-effort notification failures rather than core turn failures.
pub async fn notify_remote(url: &str, payload: Value) -> Result<String, ToolError> {
    run_remote_trigger_async(RemoteTriggerInput {
        url: url.to_string(),
        method: Some("POST".to_string()),
        headers: Some(json!({ "Content-Type": "application/json" })),
        body: Some(payload.to_string()),
    })
    .await
}

/// Process-wide shared `reqwest::Client` for the `RemoteTrigger` tool.
///
/// Per-request method, headers, and body still vary per call; only the client
/// (and its connection pool / TLS state) is shared. Building one per call
/// re-initialised the TLS backend and discarded the pool every time. Mirrors
/// `web_tools::shared_http_client` and `api::providers::shared_http_client`;
/// `reqwest::Client` is `Arc`-backed so `clone()` is free. The 30s timeout is
/// unchanged.
fn shared_remote_trigger_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new())
        })
        .clone()
}

async fn run_remote_trigger_async(input: RemoteTriggerInput) -> Result<String, ToolError> {
    let method_raw = input.method.clone().unwrap_or_else(|| "GET".to_string());
    let method = reqwest::Method::from_bytes(method_raw.to_uppercase().as_bytes())
        .map_err(|_| ToolError::InvalidInput(format!("unsupported HTTP method: {method_raw}")))?;

    if !matches!(
        method,
        reqwest::Method::GET
            | reqwest::Method::POST
            | reqwest::Method::PUT
            | reqwest::Method::DELETE
            | reqwest::Method::PATCH
            | reqwest::Method::HEAD
    ) {
        return Err(ToolError::InvalidInput(format!(
            "unsupported HTTP method: {method_raw}"
        )));
    }

    let client = shared_remote_trigger_client();
    let mut request = client.request(method.clone(), &input.url);

    if let Some(ref headers) = input.headers {
        if let Some(obj) = headers.as_object() {
            for (key, value) in obj {
                if let Some(val) = value.as_str() {
                    request = request.header(key.as_str(), val);
                }
            }
        }
    }

    if let Some(ref body) = input.body {
        request = request.body(body.clone());
    }

    match request.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            let truncated_body = if body.len() > 8192 {
                // Clamp to a UTF-8 char boundary at or below 8192 bytes —
                // a raw byte slice panics if the cut lands mid-codepoint,
                // which is common for non-ASCII HTTP response bodies.
                let mut end = 8192;
                while end > 0 && !body.is_char_boundary(end) {
                    end -= 1;
                }
                format!(
                    "{}\n\n[response truncated — {} bytes total]",
                    &body[..end],
                    body.len()
                )
            } else {
                body
            };
            to_pretty_json(json!({
                "url": input.url,
                "method": method.as_str(),
                "status_code": status,
                "body": truncated_body,
                "success": (200..300).contains(&status)
            }))
        }
        Err(e) => to_pretty_json(json!({
            "url": input.url,
            "method": method.as_str(),
            "error": e.to_string(),
            "success": false
        })),
    }
}


pub(crate) fn run_monitor(input: MonitorInput) -> Result<String, ToolError> {
    let process_id = input.process_id.or(input.command).unwrap_or_default();
    if process_id.trim().is_empty() {
        return Err(ToolError::InvalidInput(
            "either process_id or command must be provided".into(),
        ));
    }
    let max_lines = input.lines;

    // Attempt to read the background process output file written by
    // `run_in_background` in bash_tools.  The convention is that background
    // process output is stored alongside the process id.
    let output_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let output_file = output_dir.join(format!(".zo-bg-{}.log", process_id.trim()));

    let lines: Vec<String> = if output_file.exists() {
        let content = std::fs::read_to_string(&output_file)
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        content
            .lines()
            .rev()
            .take(max_lines)
            .map(String::from)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        Vec::new()
    };

    to_pretty_json(json!({
        "process_id": process_id,
        "lines": lines,
        "line_count": lines.len(),
        "source": if output_file.exists() { "file" } else { "not_found" },
        "message": if lines.is_empty() {
            format!("No output found for process '{process_id}'. The process may not have started or has no output yet.")
        } else {
            format!("Returning last {} lines from process '{process_id}'", lines.len())
        }
    }))
}

/// Deliver a message to a spawned agent — the interactive-agents contract:
/// a RUNNING agent receives it mid-turn via its steering queue (drained at the
/// next tool-result boundary), and a TERMINAL agent is resumed in the
/// background with its persisted transcript rehydrated and the message as its
/// next user turn. Both replies ride the background-completion channel, so the
/// interactive host re-invokes the parent model with the result.
/// The reserved address of the parent conversation. A sub-agent may send here
/// and nowhere else; it is never matched against spawned agent names.
pub(crate) const MAIN_CONVERSATION_TARGET: &str = "main";

/// Whether `to` addresses the parent conversation rather than a spawned agent.
fn targets_main_conversation(to: &str) -> bool {
    to.trim().eq_ignore_ascii_case(MAIN_CONVERSATION_TARGET)
}

/// A sub-agent's restricted `SendMessage`: push `message` to the PARENT
/// conversation mid-run (the AGENT→MAIN edge). Delivery rides the same
/// process-global channel as completions, so a message and a completion from
/// the same agent can never be reordered relative to each other.
///
/// Not a resume and not a spawn: nothing is re-launched and no permission
/// envelope is re-created, which is why this path is exempt from the
/// full-access grade the agent-resume path is gated on (see the dispatch arm).
fn run_send_message_to_main(
    caller: &crate::context::SubagentIdentity,
    message: &str,
) -> Result<String, ToolError> {
    let delivered = agent_tools::message_main_from_agent(
        &caller.agent_id,
        caller.display(),
        message.to_string(),
    );
    to_pretty_json(json!({
        "to": MAIN_CONVERSATION_TARGET,
        "agentId": caller.agent_id,
        "mode": "notify",
        "delivered": delivered,
        "sentAt": epoch_seconds_now(),
        "info": if delivered {
            "Delivered to the main conversation. If its turn is running it is folded in at the \
             next tool boundary; otherwise it starts a follow-up turn. Any reply comes back as \
             ordinary steering — keep working, do not wait or poll for it."
        } else {
            "This process has no live main conversation to receive the message (headless or \
             detached run). Put anything the parent must see in your final result instead."
        },
    }))
}

/// A SUB-AGENT's copy of `SendMessage` is the upstream edge only. Letting it
/// address a sibling would give sub-agents a peer-to-peer channel with no
/// orchestrator in the loop, and letting it address a terminal agent would
/// let it RESUME workers (a re-spawn with the original permission envelope)
/// — both far past what a delegated worker is authorized to do. Full
/// `SendMessage` therefore stays custom-tool-list-only. The same boundary
/// covers `attach_results`: pulling a sibling's deliverable is a read the
/// ORCHESTRATOR grants, never one a worker takes for itself.
fn run_send_message_as_subagent(
    input: &SendMessageInput,
    caller: &crate::context::SubagentIdentity,
) -> Result<String, ToolError> {
    if !input.attach_results.is_empty() {
        return Err(ToolError::InvalidInput(
            "as a sub-agent you cannot attach other agents' results — report to main and let \
             it coordinate the topology"
                .into(),
        ));
    }
    if targets_main_conversation(&input.to) {
        return run_send_message_to_main(caller, &input.message);
    }
    Err(ToolError::InvalidInput(format!(
        "as a sub-agent you may only send to \"main\" (the conversation that spawned you); \
         '{}' is not an allowed target. You cannot message sibling agents or resume other \
         agents — report what you need to main and let it coordinate.",
        input.to.trim()
    )))
}

/// Feedback edges: resolve attached deliverables engine-side and carry them
/// WITH the message, so relaying one agent's findings into another's context
/// costs the orchestrator zero re-narrated tokens. Fail-loud like the
/// spawn-time edge — a message that claims context it does not carry is
/// worse than an error.
fn send_message_with_attachments(
    store: &agent_tools::AgentRegistry,
    input: &SendMessageInput,
) -> Result<String, ToolError> {
    if input.attach_results.is_empty() {
        return Ok(input.message.clone());
    }
    // Ablation arm: the attachment goes inert and the message travels alone,
    // matching the spawn-time edge's own control behavior.
    if telemetry::attest_ablated(telemetry::HarnessFeature::InformationTopology) {
        return Ok(input.message.clone());
    }
    match agent_tools::resolve_seen_outputs(store, &input.attach_results, input.session_id.as_deref()) {
        Ok(seen) => {
            telemetry::attest_fired(telemetry::HarnessFeature::InformationTopology);
            Ok(agent_tools::prompt_with_seen_context(&input.message, &seen))
        }
        Err(error) => {
            telemetry::attest_failed(
                telemetry::HarnessFeature::InformationTopology,
                "attach_reference_unresolvable",
            );
            Err(error)
        }
    }
}

pub(crate) fn run_send_message(
    registry: Option<std::sync::Arc<agent_tools::AgentRegistry>>,
    input: &SendMessageInput,
    caller: Option<&crate::context::SubagentIdentity>,
    parent_lsp: Option<&LspRegistry>,
    hook_config: Option<&RuntimeHookConfig>,
    mcp_passthrough: Option<crate::registry::McpPassthrough>,
    parent_permission_mode: Option<runtime::PermissionMode>,
) -> Result<String, ToolError> {
    if input.to.trim().is_empty() {
        return Err(ToolError::InvalidInput("'to' must not be empty".into()));
    }
    if input.message.trim().is_empty() {
        return Err(ToolError::InvalidInput(
            "'message' must not be empty".into(),
        ));
    }

    if let Some(caller) = caller {
        return run_send_message_as_subagent(input, caller);
    }

    // `main` is reserved for the upstream edge above. Without this the main
    // model's own `SendMessage(to: "main")` would prefix-match a spawned agent
    // whose name merely starts with "main" and steer it by accident.
    if targets_main_conversation(&input.to) {
        return Err(ToolError::InvalidInput(
            "\"main\" is the reserved address of this conversation — you are already here. Name \
             a spawned agent instead."
                .into(),
        ));
    }

    let store = registry
        .clone()
        .unwrap_or_else(agent_tools::AgentRegistry::unowned_from_cwd);
    let Some(mut manifest) = lookup_agent_manifest(&store, &input.to, input.session_id.as_deref()) else {
        return to_pretty_json(json!({
            "to": input.to,
            "delivered": false,
            "agentStatus": "not_found",
            "sentAt": epoch_seconds_now(),
            "info": format!(
                "No spawned agent matches '{}'; check the name/id or spawn a new Agent.",
                input.to
            ),
            // Appended after the fields older readers know (t-2513 §2.3).
            "receipt": SteerReceipt::Rejected.as_str(),
            "reason": "no such agent",
        }));
    };

    let outgoing_message = send_message_with_attachments(&store, input)?;

    if manifest.status == "running" {
        // Framed so the sub-agent can tell an injected orchestrator message
        // apart from its own task text mid-turn. The route — in-process queue
        // or the pane child's channel — is the manifest's, and the receipt is
        // what actually happened (`docs/design/zo-teammate-lifecycle-contract.md` §1.2).
        let outcome = agent_tools::steer_agent_with_receipt(
            &manifest,
            runtime::peer_message_frame(
                caller.map(crate::context::SubagentIdentity::display),
                &outgoing_message,
            ),
        );
        if outcome.delivered() || !agent_tools::settle_dead_owner_agent(&manifest) {
            return steer_receipt_response(&input.to, &manifest, &outcome);
        }
        if let Some(settled) = lookup_agent_manifest(&store, &manifest.agent_id, None) {
            manifest = settled;
        }
    }

    match agent_tools::resume_agent_with_message(
        registry,
        &manifest,
        &outgoing_message,
        parent_lsp,
        hook_config,
        mcp_passthrough,
        parent_permission_mode,
        input.refresh_harness,
    ) {
        Ok(resumed) => {
            // A resume puts the message in front of the agent's NEXT turn —
            // whether that is a rehydrated thread or a pane child's idle
            // pane — so its receipt is `queued`; the manifest's own record
            // says whether a live pane took it or a new one was cut.
            let receipt = resumed
                .manifest
                .lifecycle
                .last_receipt
                .as_ref()
                .map_or(SteerReceipt::Queued, |record| record.receipt);
            to_pretty_json(json!({
                "to": input.to,
                "agentId": resumed.manifest.agent_id,
                "agentStatus": "running",
                "mode": "resume",
                "delivered": true,
                "sentAt": epoch_seconds_now(),
                "info": "Agent resumed in the background with its prior context intact; its \
                         reply will be delivered to you in a later message. Do NOT poll its \
                         output file.",
                "receipt": receipt.as_str(),
                "execution": resumed.manifest.lifecycle.execution,
                "harness": resumed.harness.as_str(),
            }))
        }
        Err(error) => to_pretty_json(json!({
            "to": input.to,
            "agentId": manifest.agent_id,
            "agentStatus": manifest.status,
            "mode": "resume",
            "delivered": false,
            "sentAt": epoch_seconds_now(),
            "error": error.to_string(),
            "receipt": SteerReceipt::Rejected.as_str(),
            "reason": error.to_string(),
        })),
    }
}

/// The `SendMessage` result for a steer of a RUNNING agent: the old fields,
/// then the receipt and its reason (t-2513 §2.3) — `delivered` stays the
/// boolean older readers know and means `receipt != rejected`.
fn steer_receipt_response(
    to: &str,
    manifest: &agent_tools::AgentOutput,
    outcome: &runtime::subagent_panes::SteerOutcome,
) -> Result<String, ToolError> {
    let mut response = json!({
        "to": to,
        "agentId": manifest.agent_id,
        "agentStatus": "running",
        "mode": "steer",
        "delivered": outcome.delivered(),
        "sentAt": epoch_seconds_now(),
        "info": match outcome.receipt {
            SteerReceipt::Consumed => "Read by the running agent's turn; it is acting on \
                the message now, and its result is still delivered on completion.",
            SteerReceipt::Queued => "Delivered into the running agent's turn; it will see \
                the message at its next tool boundary, and its result is still delivered \
                on completion.",
            SteerReceipt::Rejected => "Agent is marked running but nothing could take the \
                message (no live steering handle in this process, or its pane did not \
                answer). Retry shortly or wait for its completion.",
        },
        "receipt": outcome.receipt.as_str(),
    });
    if let Some(reason) = outcome.reason.as_deref() {
        response["reason"] = json!(reason);
    }
    if let Some(turn_id) = outcome.turn_id {
        response["turnId"] = json!(turn_id);
    }
    to_pretty_json(response)
}

/// Outcome of a host-initiated agent send ([`send_agent_message`]), shaped for
/// direct display (the Alt+A viewer's footer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSendOutcome {
    /// Delivered into the running agent's steering queue; it sees the message
    /// at its next tool boundary.
    Steered { name: String },
    /// The terminal agent was resumed in the background with its prior
    /// context; its reply rides the completion re-injection channel.
    Resumed { name: String },
    /// No spawned agent matches the target.
    NotFound,
    /// The target exists but is not owned by the calling foreground session.
    NotOwned { name: String },
    /// Marked running but no live steering handle in this process, with an
    /// owner that is still live or whose liveness could not be resolved.
    Unreachable { name: String },
    /// The resume attempt failed (e.g. no persisted transcript).
    Failed { name: String, error: String },
}

/// Deliver `message` to the agent named/ided `target` on behalf of the USER —
/// the Alt+A viewer's message box. Same steer/resume semantics as the
/// model-facing `SendMessage` tool, minus session-context inheritance
/// (LSP/MCP passthrough/hooks), which a user-initiated send has no handle on;
/// the resumed agent's builtin toolset does not depend on it.
/// `parent_permission_mode` is the host session's active mode — a resume
/// re-clamps to it, so the fresh-spawn clamp is not escapable via the viewer.
#[must_use]
pub fn send_agent_message(
    registry: &std::sync::Arc<agent_tools::AgentRegistry>,
    target: &str,
    message: &str,
    parent_permission_mode: Option<runtime::PermissionMode>,
) -> AgentSendOutcome {
    send_agent_message_scoped(registry, target, message, None, parent_permission_mode)
}

/// Session-strict host send for the agents viewer. Unlike the model-facing
/// tool, a human selects a concrete row that was filtered to one foreground
/// session, so a race or forged id must not cross that ownership boundary.
#[must_use]
pub fn send_agent_message_for_session(
    registry: &std::sync::Arc<agent_tools::AgentRegistry>,
    target: &str,
    message: &str,
    session_id: &str,
    parent_permission_mode: Option<runtime::PermissionMode>,
) -> AgentSendOutcome {
    send_agent_message_scoped(
        registry,
        target,
        message,
        Some(session_id),
        parent_permission_mode,
    )
}

fn send_agent_message_scoped(
    registry: &std::sync::Arc<agent_tools::AgentRegistry>,
    target: &str,
    message: &str,
    session_id: Option<&str>,
    parent_permission_mode: Option<runtime::PermissionMode>,
) -> AgentSendOutcome {
    let message = message.trim();
    if target.trim().is_empty() || message.is_empty() {
        return AgentSendOutcome::NotFound;
    }
    // The Alt+A viewer already lists only this session's agents, and the user
    // picks a concrete row, so the target is unambiguous: resolve workspace
    // wide (`None`) exactly as before.
    let Some(manifest) = lookup_agent_manifest(registry, target, None) else {
        return AgentSendOutcome::NotFound;
    };
    let name = manifest
        .label
        .clone()
        .unwrap_or_else(|| manifest.name.clone());
    if session_id.is_some()
        && !agent_tools::parent_session_belongs(
            manifest.parent_session_id.as_deref(),
            session_id,
            false,
        )
    {
        return AgentSendOutcome::NotOwned { name };
    }
    if manifest.status == "running" {
        return if agent_tools::steer_agent_with_receipt(
            &manifest,
            format!("[message from the user] {message}"),
        )
        .delivered()
        {
            AgentSendOutcome::Steered { name }
        } else {
            AgentSendOutcome::Unreachable { name }
        };
    }
    match agent_tools::resume_agent_with_message(
        Some(std::sync::Arc::clone(registry)),
        &manifest,
        message,
        None,
        None,
        None,
        parent_permission_mode,
        false,
    ) {
        Ok(_) => AgentSendOutcome::Resumed { name },
        Err(error) => AgentSendOutcome::Failed {
            name,
            error: error.to_string(),
        },
    }
}

/// Drain terminal background-agent completions belonging to `session_id`,
/// clearing their background marks. Hosts WITHOUT an idle re-injection pump
/// (serve — the interactive REPL consumes the completion channel instead)
/// sweep this at the next turn boundary and fold the results into the turn
/// input via [`fold_background_completions_into_input`], so a detached
/// agent's answer is never lost. Agents stamped with a different (or no)
/// session are left marked for their own host to consume.
#[must_use]
pub fn drain_background_completions_for_session(
    registry: &agent_tools::AgentRegistry,
    session_id: &str,
) -> Vec<AgentCompletion> {
    let mut drained = Vec::new();
    for agent_id in agent_tools::background_agent_ids_snapshot() {
        let belongs_to_session = match agent_tools::background_task_session_id(&agent_id) {
            agent_tools::BackgroundTaskSession::Session(task_session_id) => {
                task_session_id == session_id
            }
            agent_tools::BackgroundTaskSession::Unstamped => false,
            agent_tools::BackgroundTaskSession::NotTask => {
                manifest_by_id(registry, &agent_id).is_some_and(|manifest| {
                    manifest.parent_session_id.as_deref() == Some(session_id)
                        && matches!(manifest.status.as_str(), "completed" | "failed")
                })
            }
        };
        if !belongs_to_session {
            continue;
        }
        let Some(completion) = agent_tools::wait_for_agent_completions(
            std::slice::from_ref(&agent_id),
            std::time::Duration::ZERO,
        )
        .into_iter()
        .find(|completion| {
            completion.agent_id == agent_id && completion.status != "still_running"
        }) else {
            continue;
        };
        clear_background_agent(&agent_id);
        drained.push(completion);
    }
    drained
}

/// Retire every background-completion delivery claim owned by `session_id`.
///
/// The workers and their completion-store entries are left alone; only the
/// process-global marker which authorizes interactive re-injection is removed.
/// This is the session-close boundary: a late completion can still be queried
/// explicitly, but it cannot be mistaken for input to a subsequently resumed
/// conversation in the same process. Unstamped tasks/agents fail closed, and a
/// foreign session's claims remain untouched.
#[must_use]
pub fn clear_background_completion_marks_for_session(
    registry: &agent_tools::AgentRegistry,
    session_id: &str,
) -> usize {
    let mut cleared = 0;
    for agent_id in agent_tools::background_agent_ids_snapshot() {
        let belongs_to_session = match agent_tools::background_task_session_id(&agent_id) {
            agent_tools::BackgroundTaskSession::Session(task_session_id) => {
                task_session_id == session_id
            }
            agent_tools::BackgroundTaskSession::Unstamped => false,
            agent_tools::BackgroundTaskSession::NotTask => {
                manifest_by_id(registry, &agent_id).is_some_and(|manifest| {
                    agent_tools::parent_session_belongs(
                        manifest.parent_session_id.as_deref(),
                        Some(session_id),
                        false,
                    )
                })
            }
        };
        if belongs_to_session {
            clear_background_agent(&agent_id);
            cleared += 1;
        }
    }
    cleared
}

/// Body stand-in when a terminal agent produced no final text at all. A stop
/// still has to notify (CC parity: a task-notification fires on EVERY stop),
/// so the message says so explicitly instead of being silently dropped.
pub const AGENT_NOTIFICATION_EMPTY_RESULT: &str = "(no final text)";

/// Longest error text inlined into a notification SUMMARY. The full text still
/// rides the body; this only keeps a multi-KB provider dump out of the header.
const AGENT_NOTIFICATION_SUMMARY_ERROR_CHARS: usize = 200;

/// CC's summary grammar for a terminal agent: `finished`, `failed: {error}`,
/// `was stopped by user`, or a bare/qualified `was stopped`. zo does not track
/// *who* stopped an agent, so a cancellation reason (the foreground Esc path
/// writes "cancelled by foreground turn") is what identifies a user stop.
#[must_use]
pub fn agent_terminal_summary(status: &str, error: Option<&str>) -> String {
    let reason = error
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(|reason| {
            core_types::text::elide_middle(reason, AGENT_NOTIFICATION_SUMMARY_ERROR_CHARS)
        });
    match status {
        "completed" => "finished".to_string(),
        "stopped" => match reason {
            Some(reason) if reason.to_ascii_lowercase().contains("cancel") => {
                "was stopped by user".to_string()
            }
            Some(reason) => format!("was stopped: {reason}"),
            None => "was stopped".to_string(),
        },
        _ => format!(
            "failed: {}",
            reason.unwrap_or_else(|| "Unknown error".to_string())
        ),
    }
}

/// The ONE header every background-agent notification leads with, in every
/// host: the interactive mid-turn fold, the interactive idle follow-up turn,
/// and the headless/serve turn-boundary sweep below. It names the agent's NAME
/// and its ID (either one addresses it) and carries the continuation hint, so
/// the model reaches for a context-preserving resume instead of re-spawning
/// and re-explaining the task.
#[must_use]
pub fn background_agent_notification_header(
    agent_id: &str,
    name: &str,
    status: &str,
    error: Option<&str>,
    run: HelperRun,
) -> String {
    let name = name.trim();
    let summary = agent_terminal_summary(status, error);
    // What the run cost rides the header the way Claude Code writes it —
    // `Done (12 tool uses · 45.3k tokens · 1m 20s)` — so the model can weigh a
    // helper's answer against what it spent without asking for a second
    // report. A run nothing was measured for adds nothing.
    let cost = run
        .summary()
        .map_or_else(String::new, |cost| format!(" ({cost})"));
    format!(
        "[task notification — background agent `{name}` (id: {agent_id}) {summary}{cost}. To \
         follow up without losing its context, use SendMessage with that id/name as `to` to \
         continue this agent]"
    )
}

/// Render drained background completions plus the user's input as ONE turn
/// input — each completion leads with the same
/// [`background_agent_notification_header`] the interactive re-injection uses,
/// so the model reads byte-identical framing on every host. Pure, for
/// testability.
#[must_use]
pub fn fold_background_completions_into_input(
    completions: &[AgentCompletion],
    input: &str,
) -> String {
    if completions.is_empty() {
        return input.to_string();
    }
    let mut sections = Vec::with_capacity(completions.len() + 1);
    for completion in completions {
        let body = completion
            .result
            .as_deref()
            .map(str::trim)
            .filter(|result| !result.is_empty())
            .or_else(|| {
                (completion.status != "completed")
                    .then(|| completion.error.as_deref().map(str::trim))
                    .flatten()
                    .filter(|error| !error.is_empty())
            })
            .unwrap_or(AGENT_NOTIFICATION_EMPTY_RESULT);
        sections.push(format!(
            "{header}\n{body}",
            header = background_agent_notification_header(
                &completion.agent_id,
                &completion.name,
                &completion.status,
                completion.error.as_deref(),
                completion.run,
            ),
            body = core_types::text::elide_middle(body, AGENT_RESULT_RELAY_CHARS),
        ));
    }
    sections.push(input.to_string());
    sections.join("\n\n---\n\n")
}

/// The manifest for one agent id, read straight from its store path (no
/// directory scan — the id IS the file name).
fn manifest_by_id(
    registry: &agent_tools::AgentRegistry,
    agent_id: &str,
) -> Option<agent_tools::AgentOutput> {
    let path = registry.manifest_path(agent_id)?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Locate a spawned agent's manifest by exact name/id first, then by
/// name/id substring. Ties go to the most recently created match, so a
/// re-used agent name in a long session addresses the newest incarnation.
pub(crate) fn lookup_agent_manifest(
    registry: &agent_tools::AgentRegistry,
    target: &str,
    session_id: Option<&str>,
) -> Option<agent_tools::AgentOutput> {
    let target_lower = target.to_ascii_lowercase();
    // One best candidate per rank (see `agent_manifest_match_rank`), each
    // resolved latest-wins; the lowest non-empty rank answers.
    let mut ranked: [Option<agent_tools::AgentOutput>; AGENT_MATCH_RANKS] = Default::default();
    for path in registry.manifest_paths() {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Non-manifest `.json` siblings (`<id>.resume.json`) fail this parse
        // and are skipped.
        let Ok(manifest) = serde_json::from_str::<agent_tools::AgentOutput>(&content) else {
            continue;
        };
        let Some(rank) =
            agent_manifest_match_rank(&manifest, target, &target_lower, session_id)
        else {
            continue;
        };
        let slot = &mut ranked[rank];
        let newer = slot.as_ref().is_none_or(|held| {
            manifest.created_at.parse::<u64>().unwrap_or(0)
                >= held.created_at.parse::<u64>().unwrap_or(0)
        });
        if newer {
            *slot = Some(manifest);
        }
    }
    ranked.into_iter().flatten().next()
}

/// Number of match ranks [`agent_manifest_match_rank`] can return.
const AGENT_MATCH_RANKS: usize = 4;

/// How well `manifest` answers the address `target`, lower = better. `None`
/// means "no match at all".
///
/// CC's addressing semantics, in order:
/// 0. exact NAME inside the addressing session — a teammate name is
///    session-scoped, so a stale same-name worker from another tab can never
///    hijack the send;
/// 1. exact agent id, or an exact name from any session (the id is globally
///    unique, and a name is still a legitimate address when the manifest
///    predates session stamping);
/// 2. substring/prefix fallback inside the session;
/// 3. substring/prefix fallback anywhere.
///
/// Ties inside a rank go to the most recently created manifest, so a re-used
/// name addresses the newest incarnation ("latest wins").
fn agent_manifest_match_rank(
    manifest: &agent_tools::AgentOutput,
    target: &str,
    target_lower: &str,
    session_id: Option<&str>,
) -> Option<usize> {
    let in_session = session_id
        .is_some_and(|session| manifest.parent_session_id.as_deref() == Some(session));
    if manifest.name == target {
        return Some(usize::from(!in_session));
    }
    if manifest.agent_id == target {
        return Some(1);
    }
    if manifest.name.to_ascii_lowercase().contains(target_lower)
        || manifest.agent_id.contains(target)
    {
        return Some(if in_session { 2 } else { 3 });
    }
    None
}

/// The session scheduler's standing, as the receipt names it: a tracked session
/// folds the record into its autonomy engine and opens the turn when idle; an
/// untracked host has nobody to do that, and the receipt says so instead of
/// promising a turn that never comes.
fn wakeup_scheduler_status(session_id: Option<&str>) -> (&'static str, &'static str) {
    if session_id.is_some() {
        (
            "session_idle",
            "the owning session opens a turn with the stored prompt once it is idle and the delay has passed; the delay is bounded by its autonomy limits (autonomy.minModelWakeupSecs..maxModelWakeupSecs)",
        )
    } else {
        (
            "unowned",
            "no session is attached to this tool context, so nothing fires this wakeup; the record is kept for a headless reader",
        )
    }
}

pub(crate) fn run_schedule_wakeup(
    input: &ScheduleWakeupInput,
    session_id: Option<&str>,
) -> Result<String, ToolError> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let (scheduler, scheduler_note) = wakeup_scheduler_status(session_id);
    let reason = input
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty());

    if input.stop {
        let record = crate::wakeup_store::WakeupRecord {
            delay_seconds: 0.0,
            reason: reason.unwrap_or_default().to_string(),
            prompt: String::new(),
            scheduled_at: epoch_seconds_now(),
            session_id: session_id.map(str::to_string),
            noop: input.noop,
            stop: true,
        };
        let written = crate::wakeup_store::write(&cwd, &record)
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        return to_pretty_json(json!({
            "wakeupId": written.id,
            "stopped": true,
            "scheduledAt": record.scheduled_at,
            "stateFile": written.path.display().to_string(),
            "scheduler": scheduler,
            "schedulerNote": scheduler_note,
            "message": "wakeup loop stopped: the pending wakeup is cancelled and no further turn fires"
        }));
    }

    let reason = reason.ok_or_else(|| ToolError::InvalidInput("'reason' must not be empty".into()))?;
    let prompt = input
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .ok_or_else(|| ToolError::InvalidInput("'prompt' must not be empty".into()))?;
    let delay_seconds = input.delay_seconds.ok_or_else(|| {
        ToolError::InvalidInput("'delaySeconds' is required unless 'stop' is true".into())
    })?;
    if delay_seconds < 0.0 {
        return Err(ToolError::InvalidInput(
            "delaySeconds must be non-negative".into(),
        ));
    }

    let record = crate::wakeup_store::WakeupRecord {
        delay_seconds,
        reason: reason.to_string(),
        prompt: prompt.to_string(),
        scheduled_at: epoch_seconds_now(),
        session_id: session_id.map(str::to_string),
        noop: input.noop,
        stop: false,
    };
    let written = crate::wakeup_store::write(&cwd, &record)
        .map_err(|error| ToolError::Execution(error.to_string()))?;

    to_pretty_json(json!({
        "wakeupId": written.id,
        "delaySeconds": delay_seconds,
        "reason": reason,
        "noop": input.noop,
        "promptLength": prompt.len(),
        "scheduledAt": record.scheduled_at,
        "stateFile": written.path.display().to_string(),
        "scheduler": scheduler,
        "schedulerNote": scheduler_note,
        "message": format!(
            "Wakeup '{}' scheduled in {}s: {}",
            written.id, delay_seconds, reason
        )
    }))
}

// --- Implementation functions remaining in misc_tools ---

const MAX_SLEEP_DURATION_MS: u64 = 5_000;

#[allow(clippy::unnecessary_wraps)]
fn execute_sleep(input: &SleepInput) -> Result<SleepOutput, String> {
    let clamped = input.duration_ms.min(MAX_SLEEP_DURATION_MS);
    let message = if input.duration_ms > MAX_SLEEP_DURATION_MS {
        format!(
            "Slept for {clamped}ms (clamped from {} to keep tool budget tight)",
            input.duration_ms
        )
    } else {
        format!("Slept for {clamped}ms")
    };
    // live 경로는 런타임이 이미 `tokio::time::sleep` 으로 비차단 대기했으므로
    // 여기서 다시 동기 슬립하면 대기가 2배가 되고 `block_in_place` 로 turn
    // loop 의 render_tick 이 멈춘다. 플래그가 없을 때(비-live)만 슬립한다.
    if !input.already_slept {
        std::thread::sleep(Duration::from_millis(clamped));
    }
    Ok(SleepOutput {
        duration_ms: clamped,
        message,
    })
}

fn execute_structured_output(
    input: StructuredOutputInput,
) -> Result<StructuredOutputResult, String> {
    if input.0.is_empty() {
        return Err(String::from("structured output payload must not be empty"));
    }
    Ok(StructuredOutputResult {
        data: String::from("Structured output provided successfully"),
        structured_output: input.0,
    })
}

// --- ToolSearch ---

fn execute_tool_search(input: &ToolSearchInput, ctx: &ToolContext) -> ToolSearchOutput {
    let registry = GlobalToolRegistry::builtin().with_context(ctx.clone());
    if let Some(passthrough) = ctx.mcp_passthrough() {
        let _ = registry.set_runtime_tools(passthrough.definitions_snapshot());
    }
    registry.search(&input.query, input.max_results.unwrap_or(5), None, None)
}

pub(crate) fn search_tool_specs(
    query: &str,
    max_results: usize,
    specs: &[SearchableToolSpec],
) -> Vec<String> {
    let lowered = query.to_lowercase();
    if let Some(selection) = lowered.strip_prefix("select:") {
        return selection
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .filter_map(|wanted| {
                let wanted = canonical_tool_token(wanted);
                specs
                    .iter()
                    .find(|spec| canonical_tool_token(&spec.name) == wanted)
                    .map(|spec| spec.name.clone())
            })
            .take(max_results)
            .collect();
    }

    let mut required = Vec::new();
    let mut optional = Vec::new();
    for term in lowered.split_whitespace() {
        if let Some(rest) = term.strip_prefix('+') {
            if !rest.is_empty() {
                required.push(rest);
            }
        } else {
            optional.push(term);
        }
    }
    let terms = if required.is_empty() {
        optional.clone()
    } else {
        required.iter().chain(optional.iter()).copied().collect()
    };

    let mut scored = specs
        .iter()
        .filter_map(|spec| {
            let name = spec.name.to_lowercase();
            let canonical_name = canonical_tool_token(&spec.name);
            let normalized_description = normalize_tool_search_query(&spec.description);
            let haystack = format!(
                "{name} {} {canonical_name}",
                spec.description.to_lowercase()
            );
            let normalized_haystack = format!("{canonical_name} {normalized_description}");
            if required.iter().any(|term| !haystack.contains(term)) {
                return None;
            }

            let mut score = 0_i32;
            for term in &terms {
                let canonical_term = canonical_tool_token(term);
                if haystack.contains(term) {
                    score += 2;
                }
                if name == *term {
                    score += 8;
                }
                if name.contains(term) {
                    score += 4;
                }
                if canonical_name == canonical_term {
                    score += 12;
                }
                if normalized_haystack.contains(&canonical_term) {
                    score += 3;
                }
            }

            if score == 0 && !lowered.is_empty() {
                return None;
            }
            Some((score, spec.name.clone()))
        })
        .collect::<Vec<_>>();

    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    scored
        .into_iter()
        .map(|(_, name)| name)
        .take(max_results)
        .collect()
}

pub(crate) fn normalize_tool_search_query(query: &str) -> String {
    query
        .trim()
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|term| !term.is_empty())
        .map(canonical_tool_token)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn canonical_tool_token(value: &str) -> String {
    let mut canonical = value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if let Some(stripped) = canonical.strip_suffix("tool") {
        canonical = stripped.to_string();
    }
    canonical
}

#[cfg(test)]
mod tests;
