//! Portable benchmark/eval decision core for the Zo meta-harness.
//!
//! Holds the decision state machines — task/failure classification and
//! final-verdict synthesis ([`decision`]) and the deep-lane plan/verifier
//! parsing ([`deep_lane`]). Extracted from `core-types` so any crate can reuse
//! the logic without depending on `compat-harness`, and so `core-types` stays
//! a pure shared-types crate.

pub mod checkpoint;
pub mod decision;
pub mod deep_lane;
pub mod dreamer;
pub mod failure_triage;
pub mod goal_contract;
pub mod goal_gate;
pub mod loop_budget;
pub mod loop_fanout;
pub mod loop_progress;
pub mod loop_termination;
pub mod rubric_grade;
pub mod spec_literal;
pub mod strategy_pivot;
pub mod turn_continuity;
pub mod verify_convergence;

pub use checkpoint::{
    CheckpointAction, CheckpointLedger, CheckpointPolicy, CHECKPOINT_EVERY_OUTPUT_TOKENS,
    CHECKPOINT_EVERY_TURNS, CHECKPOINT_EVERY_WALL_SECS, CHECKPOINT_MAX_UNACKED,
};
pub use decision::{
    decide_final, BenchmarkLane, FailureClass, FairnessStatus, FinalDecision, ObjectiveGate,
    RunVerdict, VerifierDecision,
};
// Re-export the pure deep-lane policy seams so runtime and harness code cannot
// drift into separate accept/retry implementations.
pub use deep_lane::{
    decide, decide_with_progress, failures_match, fold_verification_attempt, parse_lens_verifier,
    parse_verifier, validate_plan, DeepDecision, PlanVerdict, VerificationAttempt, VerifierParse,
    VerifierVerdict, MAX_SUMMARY_CHARS, REQUIRED_PLAN_SECTIONS,
};
pub use dreamer::{
    curate, curate_with_existing_summaries, lessons_from_turns, slug_for, CurationPlan,
    LessonKind, LessonObservation, PromotedLesson, PromotionPolicy, SkipReason, SkippedLesson,
    TurnDigest,
};
pub use failure_triage::{
    triage_failures, BlockTracker, BlockedNeed, FailureTriage, BLOCK_ESCALATION_THRESHOLD,
};
pub use goal_contract::{pin_goal_criterion, screen_goal, AmbiguityCue, GoalAmbiguity};
pub use goal_gate::{decide_goal_completion, GoalCompletion};
pub use loop_budget::{BudgetExhaustion, BudgetLedger, LoopBudget};
pub use loop_fanout::{fold_lens_verdicts, ConsensusPolicy, LensVerdict};
pub use loop_progress::{
    failure_signature, CriteriaProgress, Progress, ProgressTracker, StallKind, STALL_THRESHOLD,
};
pub use loop_termination::{decide_loop_termination, DoneReason, LoopTermination};
pub use rubric_grade::parse_rubric_grade;
pub use spec_literal::{
    apply_case_fixes, detect_case_mismatched_literals, has_candidate_spec_literals, CaseMismatch,
};
pub use strategy_pivot::{PivotLedger, StallResponse, GOAL_PIVOT_BUDGET};
// The live turn loops' stop policy: which guards may end a turn outright and
// which must first spend an advisory round. Shared so the runtime cannot drift
// into a second copy of the cost-breaker-vs-heuristic rule.
pub use turn_continuity::{
    decide_turn_continuity, signal_is_heuristic, TurnContinuity, TurnStopSignal,
};
pub use verify_convergence::{
    finding_from_text, ConvergedReason, ConvergenceLedger, ConvergencePolicy, ConvergenceVerdict,
    DivergingReason, Finding, FindingSeverity, CONVERGENCE_CHURN_LIMIT, CONVERGENCE_MAX_ROUNDS,
    CONVERGENCE_QUIET_ROUNDS,
};
