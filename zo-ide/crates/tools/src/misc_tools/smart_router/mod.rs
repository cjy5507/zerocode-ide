mod apply;
mod canonical;
mod decision_report;
mod decision_shadow;
mod evidence;
mod infer;
mod jev_gate;
mod metadata;
pub mod jev_summary;
mod plan_shadow;
mod planner;
mod probe_exec;
mod probe_gate;
mod settings;
mod shadow_ledger;
mod rerank_shadow;
mod skill_search;
mod shape;
mod shape_words;
mod step_effort;
mod turn;

#[cfg(test)]
mod tests;

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
    check_system_one, decision_shadow_path, CheckFailure, DecisionShadowRow, JudgedAxis, ProbeCell,
    SystemOneCheck, DECISION_SHADOW_FILE, KEY_CHECK_TASK, OUTCOME_ANSWERED,
};
pub use probe_exec::task_fingerprint;
pub use rerank_shadow::{
    rerank_shadow_path, Judged, RerankShadow, RerankShadowRow, RERANK_OUTCOME_ANSWERED,
    RERANK_OUTCOME_UNORDERABLE, RERANK_SHADOW_FILE,
};
pub use skill_search::{
    note_loaded_skill, note_search_answer, search as skill_search,
    skill_search_path, Chosen, Searched, SkillLabelRow, SkillSearchRow, SKILL_OUTCOME_ANSWERED,
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
    decision_shadow_mode_from, rerank_shadow_mode_from, skill_search_mode_from,
    DecisionShadowMode, DECISION_SHADOW_SETTING, RERANK_SHADOW_SETTING, SKILL_SEARCH_SETTING,
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
