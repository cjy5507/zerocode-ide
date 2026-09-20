//! Smart Model Router core.
//!
//! Routing policy stays pure: callers provide a source-guarded usable model
//! inventory and routing preferences. The sibling outcome module only persists
//! aggregate-safe route results for later diagnostics; it never selects models,
//! reads settings, builds provider clients, or performs network discovery.

mod accuracy;
mod agent_route;
mod attempt;
mod assignment;
mod calibration;
mod completion;
mod decision;
mod inventory;
mod learned;
mod outcome;
mod plan;
mod policy;
mod probe;
mod selector;
mod target;
mod tiering;

#[cfg(test)]
mod tests;

pub use attempt::{spawn_attempt_key, turn_attempt_key};
pub use assignment::{
    recommend_auto_assignments, recommend_auto_assignments_with_feedback,
    recommend_auto_assignments_with_learned_specialty, recommend_auto_assignments_with_options,
    recommend_role_fallbacks, recommend_role_fallbacks_with_learned_specialty, AssignmentConfidence,
    AssignmentSource, AutoAssignmentOptions, AutoAssignmentPlan, InventorySummary, TargetAssignment,
};
pub use inventory::{
    ModelDescriptor, ModelInventory, ModelSource, UsableModel, UsableModelInventory,
};
pub use tiering::{classify_model_tiers, ImplRung, ModelBand, ModelTierAssignment};
pub use plan::{
    choose_plan, plan_candidates, plan_evidence_from_records, score_plan,
    CacheState as PlanCacheState, ChoiceReason,
    CostBreakdown, CostTerm, ModelOption, ModelPrice, PlanCandidate, PlanChoice, PlanContext,
    PlanEstimate, PlanEvidence, PlanPriors, ScoredPlan, SwitchTrigger, VerifyMode,
};
pub use accuracy::{
    orchestration_accuracy, read_orchestration_accuracy, AccuracyReport, ACCURACY_MIN_DECISIVE,
};
pub use agent_route::{
    agent_preference_adjustment, agent_stats_for_route, preferred_agent_for_route, AgentRouteStat,
};
pub use completion::{completion_ceiling_for, completion_loop_step, CompletionStep};
pub use decision::{
    axis_metrics, decision_questions, decision_request, judged_axes, validate_decision,
    AxisMetrics, AxisReading, AxisSample, DecisionAnswer, DecisionRejection, DecisionVerdict, ROUTE_TRUST_FLOOR,
    CALIBRATION_BINS, PROBABILITY_SUM_TOLERANCE,
};
pub use learned::{LearnedSpecialtyEntry, LearnedSpecialtyHint};
pub use outcome::{
    is_terminal_outcome_status, read_route_outcome_summary, read_route_outcomes,
    record_route_outcome, resolve_verdict_basis, route_outcome_log_path,
    summarize_decisions_by_kind, summarize_route_outcomes,
    summarize_route_outcomes_with_canonicalizer, verify_metrics, weakest_decision_kind,
    weighted_feedback_hint_for_route_key, DecisionKind, DecisionOutcomeStat, PlanShape,
    RouteOutcomeBucket, RouteOutcomeRecord, RouteOutcomeSummary, VerdictBasis, VerdictSubject,
    VerifyMetrics, RouteTaxCall, CONFIDENT_DECISIVE_SAMPLES, ROUTE_TAX_ROUTE_KEY,
    OUTCOME_COMPLETED, OUTCOME_FAILED, OUTCOME_STOPPED,
};
pub use policy::{
    deep_tier_model_matches, default_deep_tier_models, dynamic_deep_tier_models,
    escalate_complexity, exploration_slot_for_route,
    implementation_escalation_allowed, implementation_route_model_allowed, is_deep_tier_model, is_reserved_orchestrator_model,
    recommended_effort_for,
    route_model, route_model_fallback_candidates, EffortCeiling, FreshnessPolicy, LaneRouteMetadata, ModelCapability, ModelStatus,
    ModelTier, RouteAudit, RouteAutoClassifierMode, RouteConfidence, RouteContextNeed,
    RouteDecision, RouteDecisionSource, RouteDiversityNeed, RouteFeedbackHint,
    RouteOutputNeed, RoutePolicyContext, RouteRequest, RouterMode, RouteShapeKind,
    RouteSignalSource, RouteTaskComplexity, RouteTaskKind, RouteTaskRisk, RouteToolNeed,
    RouteVerificationNeed, SmartPolicy, TiersProvenance,
};
pub use calibration::{
    read_route_outcomes_across_projects, route_outcome_log_paths_across_projects,
    ComplexityCalibration, CALIBRATION_FAILURE_SHARE, CALIBRATION_MIN_SAMPLES,
};
pub use probe::{
    fuse_probe_assessment, parse_probe_response, probe_prompt, rubric_task_text, rubric_task_whole,
    ProbeAssessment, ProbeFusion, ProbeFusionEffect, RouteAssessmentProvenance, RouteTaskIntent, RubricAxis,
    COMPLEXITY_AXIS, CONFIDENCE_AXIS, DECISION_RUBRIC_VERSION, INTENT_AXIS, RISK_AXIS,
    ROUTING_RUBRIC, RUBRIC_TASK_CHAR_CAP,
};
pub use selector::{RoleOverride, RoleSelector};
pub use target::{
    BuiltinSubagentProfile, RouteRole, RoutingTarget, SubagentProfileId, SubagentProfileKind,
};
