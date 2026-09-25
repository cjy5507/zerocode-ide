mod agent_tool;
mod apply;
mod canonical;
mod compaction_seat;
mod decision_report;
mod decision_shadow;
mod evidence;
mod file_pick;
mod infer;
mod jev_gate;
#[cfg(test)]
mod jev_mock;
#[cfg(test)]
mod replay_support;
mod mention_rerank;
mod metadata;
mod patch_review;
mod claim_check;
mod vault_pairs;
pub mod jev_summary;
mod plan_shadow;
mod planner;
mod probe_exec;
mod probe_gate;
mod settings;
mod shadow_ledger;
mod rerank_shadow;
#[cfg(test)]
mod roads_tests;
mod route_label;
#[cfg(test)]
mod question_discovery;
#[cfg(test)]
mod routing_replay;
mod skill_search;
mod shape;
mod shape_words;
mod tool_guard;
mod step_effort;
mod turn;
mod turn_reads;

#[cfg(test)]
mod tests;

pub use agent_tool::{
    agent_tool_path, decide as jev_decide, AgentToolRow, JevAnswer, JevCaller, JevInvalid,
    JevQuestion, JevShape, JevVerdict, ScoredItem, AGENT_TOOL_FILE, AGENT_TOOL_OUTCOME_ANSWERED,
};
pub(crate) use apply::{
    apply_smart_models_to_spawn_input_with_auto_types, route_source_label, smart_parent_model_for_agent,
    smart_parent_model_for_agent_with_auto_type,
    ROUTE_DECISION_META_SMUGGLE_KEY, ROUTE_EFFORT_SMUGGLE_KEY,
    ROUTE_FALLBACK_MODELS_SMUGGLE_KEY, ROUTE_JUDGED_AGENT_SMUGGLE_KEY,
    ROUTE_MODEL_SMUGGLE_KEY, ROUTE_REASON_SMUGGLE_KEY,
};
pub(crate) use canonical::canonicalize_route_model_id;
pub use decision_report::{
    basis_points, evaluate_decision_labels, summarize_decision_shadow, AxisAgreement,
    AxisEvaluation, DecisionShadowSummary, LabelEvaluation, BASIS_POINTS,
};
pub use decision_shadow::{
    check_system_one, decision_shadow_path, note_route_followed, route_unseated_by, CheckFailure,
    DecisionShadowRow, JudgedAxis, ProbeCell, RouteLabelRow, SystemOneCheck, DECISION_SHADOW_FILE,
    KEY_CHECK_TASK, OUTCOME_ANSWERED, ROUTE_STOOD,
};
pub use probe_exec::task_fingerprint;
pub use rerank_shadow::{
    note_recall_read, rerank_shadow_path, Judged, RerankLabelRow, RerankShadow, RerankShadowRow, ShownNote,
    RERANK_OUTCOME_ANSWERED, RERANK_OUTCOME_UNORDERABLE, RERANK_SHADOW_FILE,
};
pub use compaction_seat::{
    compaction_relevance_path, note_compaction_reread, CompactionJudge, CompactionLabelRow,
    CompactionRow, COMPACTION_JUDGMENT_DEADLINE, COMPACTION_OUTCOME_ANSWERED,
    COMPACTION_RELEVANCE_FILE,
};
pub use patch_review::{
    note_patch_review_turn, patch_review_path, PatchReviewJudge, PatchReviewLabelRow, PatchReviewRow,
    PATCH_REVIEW_DEADLINE, PATCH_REVIEW_FILE, PATCH_REVIEW_OUTCOME_ANSWERED,
};
pub use claim_check::{claim_check_path, note_claim_turn, ClaimCheckRow, ClaimLabelRow};
pub use vault_pairs::{
    judge_vault_pairs, mark_vault_pair, recorded_vault_pair_proposals,
    PairJudgment, PairLabel, PairRun,
};
pub use file_pick::{
    file_pick_path, note_file_pick_turn, FilePickJudge, FilePickLabelRow, FilePickRow,
    FILE_PICK_FILE, FILE_PICK_OUTCOME_ANSWERED,
};
pub use tool_guard::{
    command_guard_path, note_tool_guard_turn, tool_text_guard_path, CommandGuardLabelRow, CommandGuardRow,
    ToolGuardJudge, ToolTextGuardLabelRow, ToolTextGuardRow, COMMAND_GUARD_FILE, TOOL_GUARD_OUTCOME_ANSWERED,
    TOOL_TEXT_GUARD_FILE,
};
pub use mention_rerank::{
    mention_rerank_path, MentionAnswer, MentionAsk, MentionCandidate, MentionJudged, MentionLabelRow,
    MentionRerank, MentionRerankRow, MentionSurface, MENTION_OUTCOME_ANSWERED, MENTION_RERANK_DEADLINE,
    MENTION_RERANK_FILE, MENTION_RUBRIC_VERSION,
};
pub use skill_search::{
    note_loaded_skill, note_search_answer, search as skill_search,
    skill_search_path, Chosen, Searched, SkillLabelRow, SkillSearchRow, SkillSuggestionJudge, SKILL_OUTCOME_ANSWERED,
    SKILL_SEARCH_DEADLINE, SKILL_SEARCH_FILE,
};
pub use shadow_ledger::read_shadow_rows;
pub use step_effort::{
    judge_ledger as judge_step_effort_ledger, record_step_event, step_effort_path, step_effort_raised,
    step_effort_word, step_effort_word_in, StepEffortWord, StepJudgmentRow, StepSeat,
    JUDGMENT_ROW_KIND, STEP_EFFORT_FILE, STEP_EFFORT_SETTING, STEP_JUDGMENT_DEADLINE,
};
pub use plan_shadow::{
    build_plan_shadow, model_options_for, model_price_for, plan_priors_for, plan_shadow_path,
    record_plan_shadow, switch_candidates,
    PlanShadowActual, PlanShadowCandidate, PlanShadowInputs, PlanShadowRow,
};
pub(crate) use settings::live_agent_model_policy;
pub use settings::{
    agent_tool_mode_from, decision_shadow_mode_from, jev_compaction_mode_from,
    jev_claim_mode_from, jev_command_guard_mode_from, jev_file_pick_mode_from, jev_tool_text_guard_mode_from,
    jev_mention_rerank_mode_from, jev_patch_review_mode_from, rerank_shadow_mode_from,
    skill_search_mode_from, DecisionShadowMode, AGENT_TOOL_SETTING, DECISION_SHADOW_SETTING,
    JEV_COMMAND_GUARD_SETTING, JEV_COMPACTION_SETTING, JEV_FILE_PICK_SETTING, JEV_MENTION_RERANK_SETTING,
    JEV_PATCH_REVIEW_SETTING, JEV_TOOL_TEXT_GUARD_SETTING,
    RERANK_SHADOW_SETTING, SKILL_SEARCH_SETTING,
    conversation_anchor_ttl_for, conversation_anchor_ttl_from_root, CACHE_ANCHOR_TTL_ENV,
    smart_deep_tier_models, smart_deep_tier_models_for, smart_exec_swap, smart_setting_defaults,
    smart_turn_routing_and_inventory_for, smart_turn_routing_for, DeepTierModelsSetting,
    HostOrchestration, PlanShadowSettings,
    SmartExecSwap, SmartSettingDefaults, SmartTurnRouting,
    merged_settings_root,
};
pub use turn::{
    assess_agent_task, assess_turn_complexity, assess_turn_complexity_probed,
    assess_turn_deterministic, assess_turn_orchestration, assess_turn_probed, decide_host_prelude,
    turn_has_write_intent, AgentTaskAssessment, HostPrelude, TurnOrchestrationHint,
    AssessmentReaders, TurnProbeAssessment,
};

pub(crate) fn agent_task_has_write_intent(description: &str, prompt: &str) -> bool {
    infer::task_has_write_intent(description, prompt)
}
