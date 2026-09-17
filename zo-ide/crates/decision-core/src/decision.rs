//! Spec-aligned benchmark decision taxonomy and the decision matrix.
//!
//! [`crate::deep_lane`] decides what the *retry loop* does after one attempt
//! (accept / retry / give up). This module decides the *final benchmark
//! verdict* for a recorded run, following the Zo Meta-Harness decision
//! matrix: fairness × objective gate × verifier parse mode × verifier decision
//! × artifact preservation → exactly one of accepted / rejected / inconclusive
//! / invalid / blocked, plus a precise failure class.
//!
//! It is pure and total: every input combination maps to one defined verdict,
//! so the harness, the scorer, and the report generator share one source of
//! truth instead of re-deriving the policy in shell and drifting apart.

use crate::deep_lane::VerifierParse;

/// The benchmark lane a task is scored under.
///
/// Lanes are never averaged together — the spec forbids comparing a fast-lane
/// win against a deep-lane loss. Adding a lane is a data concern (a lane policy
/// entry plus fixtures); this enum only fixes the closed set of lane *names*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BenchmarkLane {
    Fast,
    Deep,
    RiskyRefactor,
    Debug,
    Migration,
    Review,
}

impl BenchmarkLane {
    /// Every lane, in spec declaration order.
    pub const ALL: [Self; 6] = [
        Self::Fast,
        Self::Deep,
        Self::RiskyRefactor,
        Self::Debug,
        Self::Migration,
        Self::Review,
    ];

    /// Canonical wire token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Deep => "deep",
            Self::RiskyRefactor => "risky-refactor",
            Self::Debug => "debug",
            Self::Migration => "migration",
            Self::Review => "review",
        }
    }

    /// Parse a lane from its wire token. Accepts `_` for `-` in `risky_refactor`.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        let t = token.trim();
        if t == "risky_refactor" {
            return Some(Self::RiskyRefactor);
        }
        Self::ALL.into_iter().find(|lane| lane.as_str() == t)
    }
}

/// Whether the objective gate (test command + diff hygiene) ran and what it
/// found. Classified separately from the verifier's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectiveGate {
    /// Tests passed and the diff stayed within policy.
    Green,
    /// Tests failed or the diff violated policy.
    Red,
    /// The gate never executed (e.g. the agent produced no candidate).
    NotRun,
    /// The gate ran but its result cannot be trusted (e.g. fixture mutated).
    Invalid,
}

impl ObjectiveGate {
    /// Every state, for exhaustive iteration.
    pub const ALL: [Self; 4] = [Self::Green, Self::Red, Self::NotRun, Self::Invalid];

    /// Canonical wire token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Red => "red",
            Self::NotRun => "not_run",
            Self::Invalid => "invalid",
        }
    }

    /// Parse from a wire token.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        let t = token.trim();
        Self::ALL.into_iter().find(|gate| gate.as_str() == t)
    }
}

/// The verifier's semantic decision, classified separately from how its output
/// parsed. A verifier can produce parseable output yet carry no decision
/// signal — that is [`VerifierDecision::Unknown`], not a rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerifierDecision {
    Accept,
    Reject,
    Unknown,
}

impl VerifierDecision {
    /// Every decision, for exhaustive iteration.
    pub const ALL: [Self; 3] = [Self::Accept, Self::Reject, Self::Unknown];

    /// Canonical wire token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
            Self::Unknown => "unknown",
        }
    }

    /// Parse from a wire token.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        let t = token.trim();
        Self::ALL.into_iter().find(|d| d.as_str() == t)
    }

    /// Derive a decision from the verifier's `accepted` flag and how its output
    /// parsed. An explicit accept is always [`Self::Accept`]. A non-acceptance
    /// is a real [`Self::Reject`] only when the parse mode carried a decision
    /// signal (strict JSON or a salvaged ACCEPT/REJECT token); an empty,
    /// malformed, or timed-out verifier yields no signal → [`Self::Unknown`].
    #[must_use]
    pub const fn from_verdict(accepted: bool, parse: VerifierParse) -> Self {
        if accepted {
            Self::Accept
        } else {
            match parse {
                VerifierParse::Json | VerifierParse::Salvaged => Self::Reject,
                VerifierParse::Empty
                | VerifierParse::Unparseable
                | VerifierParse::Timeout
                | VerifierParse::BudgetExhausted => Self::Unknown,
            }
        }
    }
}

/// Whether the fairness contract for a run is trustworthy enough to compare.
/// Only [`FairnessStatus::Valid`] runs may enter a leaderboard denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FairnessStatus {
    Valid,
    Invalid,
    Partial,
    Unknown,
}

impl FairnessStatus {
    /// Every status, for exhaustive iteration.
    pub const ALL: [Self; 4] = [Self::Valid, Self::Invalid, Self::Partial, Self::Unknown];

    /// Canonical wire token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::Partial => "partial",
            Self::Unknown => "unknown",
        }
    }

    /// Parse from a wire token.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        let t = token.trim();
        Self::ALL.into_iter().find(|s| s.as_str() == t)
    }
}

/// The final, leaderboard-facing verdict for one recorded run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FinalDecision {
    Accepted,
    Rejected,
    Inconclusive,
    Invalid,
    Blocked,
}

impl FinalDecision {
    /// Every state, for exhaustive iteration.
    pub const ALL: [Self; 5] = [
        Self::Accepted,
        Self::Rejected,
        Self::Inconclusive,
        Self::Invalid,
        Self::Blocked,
    ];

    /// Canonical wire token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Inconclusive => "inconclusive",
            Self::Invalid => "invalid",
            Self::Blocked => "blocked",
        }
    }

    /// Parse from a wire token.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        let t = token.trim();
        Self::ALL.into_iter().find(|d| d.as_str() == t)
    }
}

/// Precise failure classification. The spec forbids collapsing every failure
/// into "model failed"; these 23 classes name the actual cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    ModelOutputWrong,
    SpecMiningFailure,
    ContextPackingFailure,
    PlanningFailure,
    EditApplicationFailure,
    TestFailure,
    VerifierSemanticReject,
    VerifierContractFailure,
    VerifierMissing,
    VerifierTimeout,
    PermissionDenied,
    ProviderTimeout,
    ProviderError,
    FirstModelCallTimeout,
    RunnerCrash,
    ArtifactPreservationFailed,
    FairnessContractInvalid,
    FixtureInvalid,
    PromptMismatch,
    IntendedPathViolation,
    DirtyDiff,
    BenchmarkHarnessBug,
    UnknownFailure,
}

impl FailureClass {
    /// Every class, in spec declaration order.
    pub const ALL: [Self; 23] = [
        Self::ModelOutputWrong,
        Self::SpecMiningFailure,
        Self::ContextPackingFailure,
        Self::PlanningFailure,
        Self::EditApplicationFailure,
        Self::TestFailure,
        Self::VerifierSemanticReject,
        Self::VerifierContractFailure,
        Self::VerifierMissing,
        Self::VerifierTimeout,
        Self::PermissionDenied,
        Self::ProviderTimeout,
        Self::ProviderError,
        Self::FirstModelCallTimeout,
        Self::RunnerCrash,
        Self::ArtifactPreservationFailed,
        Self::FairnessContractInvalid,
        Self::FixtureInvalid,
        Self::PromptMismatch,
        Self::IntendedPathViolation,
        Self::DirtyDiff,
        Self::BenchmarkHarnessBug,
        Self::UnknownFailure,
    ];

    /// Canonical wire token (the spec's `snake_case` failure name).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ModelOutputWrong => "model_output_wrong",
            Self::SpecMiningFailure => "spec_mining_failure",
            Self::ContextPackingFailure => "context_packing_failure",
            Self::PlanningFailure => "planning_failure",
            Self::EditApplicationFailure => "edit_application_failure",
            Self::TestFailure => "test_failure",
            Self::VerifierSemanticReject => "verifier_semantic_reject",
            Self::VerifierContractFailure => "verifier_contract_failure",
            Self::VerifierMissing => "verifier_missing",
            Self::VerifierTimeout => "verifier_timeout",
            Self::PermissionDenied => "permission_denied",
            Self::ProviderTimeout => "provider_timeout",
            Self::ProviderError => "provider_error",
            Self::FirstModelCallTimeout => "first_model_call_timeout",
            Self::RunnerCrash => "runner_crash",
            Self::ArtifactPreservationFailed => "artifact_preservation_failed",
            Self::FairnessContractInvalid => "fairness_contract_invalid",
            Self::FixtureInvalid => "fixture_invalid",
            Self::PromptMismatch => "prompt_mismatch",
            Self::IntendedPathViolation => "intended_path_violation",
            Self::DirtyDiff => "dirty_diff",
            Self::BenchmarkHarnessBug => "benchmark_harness_bug",
            Self::UnknownFailure => "unknown_failure",
        }
    }

    /// Parse from a wire token.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        let t = token.trim();
        Self::ALL.into_iter().find(|fc| fc.as_str() == t)
    }
}

/// Output of [`decide_final`]: the final verdict, an optional precise failure
/// class (`None` only when accepted), and whether the run may enter a
/// leaderboard denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunVerdict {
    pub decision: FinalDecision,
    pub failure: Option<FailureClass>,
    pub leaderboard_eligible: bool,
}

/// Apply the Zo Meta-Harness decision matrix to one recorded run.
///
/// The five inputs are the axes the spec classifies independently. The output
/// is exactly one final state plus a precise failure class. [`FinalDecision::Blocked`]
/// is never produced here: a blocked run never reaches the objective gate, so
/// the harness sets `Blocked` *before* calling this — `decide_final` scores runs
/// that actually executed.
///
/// The body encodes the ten rows of the spec's "Decision Matrix" section as an
/// exhaustive match; each row is pinned by a `matrix_rowN` unit test below.
#[must_use]
pub fn decide_final(
    fairness: FairnessStatus,
    objective: ObjectiveGate,
    parse: VerifierParse,
    decision: VerifierDecision,
    artifacts_preserved: bool,
) -> RunVerdict {
    use FailureClass as F;
    use FinalDecision as D;

    // Row 1: an invalid/partial/unknown fairness contract means the comparison
    // itself cannot be trusted — never leaderboard-eligible.
    if !matches!(fairness, FairnessStatus::Valid) {
        return RunVerdict {
            decision: D::Invalid,
            failure: Some(F::FairnessContractInvalid),
            leaderboard_eligible: false,
        };
    }

    // Fairness is valid from here on: the run is leaderboard-eligible whatever
    // the candidate outcome — accepted/rejected/inconclusive/invalid all count
    // toward their own denominators; only invalid *fairness* is excluded.
    let verdict = |decision, failure| RunVerdict {
        decision,
        failure,
        leaderboard_eligible: true,
    };

    match objective {
        // Row 10: the gate never ran — not scoreable as a candidate.
        ObjectiveGate::NotRun => verdict(D::Invalid, Some(F::BenchmarkHarnessBug)),
        // The gate ran but is untrustworthy (e.g. fixture mutated mid-run).
        ObjectiveGate::Invalid => verdict(D::Invalid, Some(F::FixtureInvalid)),
        // Row 2: objective red rejects regardless of the verifier.
        ObjectiveGate::Red => verdict(D::Rejected, None),
        // Objective green: the verifier parse mode and decision decide the rest.
        ObjectiveGate::Green => match parse {
            // strict_valid
            VerifierParse::Json => match decision {
                // Row 3 vs Row 9: a strict accept demands preserved artifacts.
                VerifierDecision::Accept => {
                    if artifacts_preserved {
                        verdict(D::Accepted, None)
                    } else {
                        verdict(D::Invalid, Some(F::ArtifactPreservationFailed))
                    }
                }
                // Row 4: a strict, in-contract rejection is a real failure.
                VerifierDecision::Reject => verdict(D::Rejected, Some(F::VerifierSemanticReject)),
                // Strict parse but no decision signal is a contract failure.
                VerifierDecision::Unknown => {
                    verdict(D::Inconclusive, Some(F::VerifierContractFailure))
                }
            },
            // Row 8: salvage_valid is never a strict accept → inconclusive.
            VerifierParse::Salvaged => verdict(D::Inconclusive, Some(F::VerifierContractFailure)),
            // Row 5: malformed
            VerifierParse::Unparseable => {
                verdict(D::Inconclusive, Some(F::VerifierContractFailure))
            }
            // Row 6: missing
            VerifierParse::Empty => verdict(D::Inconclusive, Some(F::VerifierMissing)),
            // Row 7: timeout — budget exhaustion is the same "did not
            // finish" row for the fairness matrix (the deep-loop retry
            // policy distinguishes them, this ledger does not).
            VerifierParse::Timeout | VerifierParse::BudgetExhausted => {
                verdict(D::Inconclusive, Some(F::VerifierTimeout))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(
        fairness: FairnessStatus,
        objective: ObjectiveGate,
        parse: VerifierParse,
        decision: VerifierDecision,
        accepted: bool,
    ) -> RunVerdict {
        decide_final(fairness, objective, parse, decision, accepted)
    }

    // --- Decision Matrix, row by row (spec lines 261-270) ---

    #[test]
    fn matrix_row1_invalid_fairness_is_invalid_and_not_eligible() {
        let r = v(
            FairnessStatus::Invalid,
            ObjectiveGate::Green,
            VerifierParse::Json,
            VerifierDecision::Accept,
            true,
        );
        assert_eq!(r.decision, FinalDecision::Invalid);
        assert_eq!(r.failure, Some(FailureClass::FairnessContractInvalid));
        assert!(!r.leaderboard_eligible);
    }

    #[test]
    fn matrix_row2_objective_red_is_rejected() {
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::Red,
            VerifierParse::Json,
            VerifierDecision::Accept,
            true,
        );
        assert_eq!(r.decision, FinalDecision::Rejected);
        assert!(r.leaderboard_eligible);
    }

    #[test]
    fn matrix_row3_green_strict_accept_with_artifacts_is_accepted() {
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::Green,
            VerifierParse::Json,
            VerifierDecision::Accept,
            true,
        );
        assert_eq!(r.decision, FinalDecision::Accepted);
        assert_eq!(r.failure, None);
    }

    #[test]
    fn matrix_row4_green_strict_reject_is_semantic_reject() {
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::Green,
            VerifierParse::Json,
            VerifierDecision::Reject,
            true,
        );
        assert_eq!(r.decision, FinalDecision::Rejected);
        assert_eq!(r.failure, Some(FailureClass::VerifierSemanticReject));
    }

    #[test]
    fn matrix_row5_green_malformed_is_inconclusive_contract() {
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::Green,
            VerifierParse::Unparseable,
            VerifierDecision::Unknown,
            true,
        );
        assert_eq!(r.decision, FinalDecision::Inconclusive);
        assert_eq!(r.failure, Some(FailureClass::VerifierContractFailure));
    }

    #[test]
    fn matrix_row6_green_missing_is_inconclusive_missing() {
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::Green,
            VerifierParse::Empty,
            VerifierDecision::Unknown,
            true,
        );
        assert_eq!(r.decision, FinalDecision::Inconclusive);
        assert_eq!(r.failure, Some(FailureClass::VerifierMissing));
    }

    #[test]
    fn matrix_row7_green_timeout_is_inconclusive_timeout() {
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::Green,
            VerifierParse::Timeout,
            VerifierDecision::Unknown,
            true,
        );
        assert_eq!(r.decision, FinalDecision::Inconclusive);
        assert_eq!(r.failure, Some(FailureClass::VerifierTimeout));
    }

    #[test]
    fn matrix_row8_green_salvage_is_inconclusive_not_accepted() {
        // Even with a salvaged ACCEPT signal, salvage is never a strict accept.
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::Green,
            VerifierParse::Salvaged,
            VerifierDecision::Accept,
            true,
        );
        assert_eq!(r.decision, FinalDecision::Inconclusive);
        assert_ne!(r.decision, FinalDecision::Accepted);
    }

    #[test]
    fn matrix_row9_green_strict_accept_missing_artifacts_is_invalid() {
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::Green,
            VerifierParse::Json,
            VerifierDecision::Accept,
            false,
        );
        assert_eq!(r.decision, FinalDecision::Invalid);
        assert_eq!(r.failure, Some(FailureClass::ArtifactPreservationFailed));
    }

    #[test]
    fn matrix_row10_objective_not_run_is_invalid() {
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::NotRun,
            VerifierParse::Empty,
            VerifierDecision::Unknown,
            true,
        );
        assert_eq!(r.decision, FinalDecision::Invalid);
    }

    // --- Required Policy Tests (spec lines 612-628) that this module owns ---

    #[test]
    fn policy_malformed_verifier_is_not_candidate_failure() {
        // objective green + malformed verifier must be inconclusive, not rejected.
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::Green,
            VerifierParse::Unparseable,
            VerifierDecision::Reject,
            true,
        );
        assert_eq!(r.decision, FinalDecision::Inconclusive);
        assert_ne!(r.decision, FinalDecision::Rejected);
    }

    #[test]
    fn policy_strict_reject_blocks_acceptance() {
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::Green,
            VerifierParse::Json,
            VerifierDecision::Reject,
            true,
        );
        assert_ne!(r.decision, FinalDecision::Accepted);
    }

    #[test]
    fn policy_invalid_fairness_cannot_enter_leaderboard() {
        for fairness in [
            FairnessStatus::Invalid,
            FairnessStatus::Partial,
            FairnessStatus::Unknown,
        ] {
            let r = v(
                fairness,
                ObjectiveGate::Green,
                VerifierParse::Json,
                VerifierDecision::Accept,
                true,
            );
            assert!(!r.leaderboard_eligible, "{fairness:?} must be excluded");
        }
    }

    #[test]
    fn policy_missing_artifact_makes_accept_invalid() {
        let r = v(
            FairnessStatus::Valid,
            ObjectiveGate::Green,
            VerifierParse::Json,
            VerifierDecision::Accept,
            false,
        );
        assert_eq!(r.decision, FinalDecision::Invalid);
    }

    #[test]
    fn policy_green_tests_alone_do_not_accept() {
        // objective green but a non-strict verifier never yields acceptance.
        for parse in [
            VerifierParse::Empty,
            VerifierParse::Timeout,
            VerifierParse::Unparseable,
            VerifierParse::Salvaged,
        ] {
            let r = v(
                FairnessStatus::Valid,
                ObjectiveGate::Green,
                parse,
                VerifierDecision::Accept,
                true,
            );
            assert_ne!(
                r.decision,
                FinalDecision::Accepted,
                "{parse:?} must not accept on a green gate alone"
            );
        }
    }

    #[test]
    fn policy_accepted_requires_strict_verifier_accept() {
        let accepted = v(
            FairnessStatus::Valid,
            ObjectiveGate::Green,
            VerifierParse::Json,
            VerifierDecision::Accept,
            true,
        );
        assert_eq!(accepted.decision, FinalDecision::Accepted);
    }

    // --- Verifier decision derivation ---

    #[test]
    fn verdict_accept_is_accept_regardless_of_parse() {
        for parse in [
            VerifierParse::Json,
            VerifierParse::Salvaged,
            VerifierParse::Empty,
            VerifierParse::Unparseable,
            VerifierParse::Timeout,
        ] {
            assert_eq!(
                VerifierDecision::from_verdict(true, parse),
                VerifierDecision::Accept
            );
        }
    }

    #[test]
    fn verdict_non_accept_is_reject_only_with_signal() {
        assert_eq!(
            VerifierDecision::from_verdict(false, VerifierParse::Json),
            VerifierDecision::Reject
        );
        assert_eq!(
            VerifierDecision::from_verdict(false, VerifierParse::Salvaged),
            VerifierDecision::Reject
        );
        for parse in [
            VerifierParse::Empty,
            VerifierParse::Unparseable,
            VerifierParse::Timeout,
        ] {
            assert_eq!(
                VerifierDecision::from_verdict(false, parse),
                VerifierDecision::Unknown,
                "{parse:?} carries no decision signal"
            );
        }
    }

    // --- enum round-trips: every wire token parses back to its variant ---

    #[test]
    fn lane_token_round_trip() {
        for lane in BenchmarkLane::ALL {
            assert_eq!(BenchmarkLane::from_token(lane.as_str()), Some(lane));
        }
        assert_eq!(
            BenchmarkLane::from_token("risky_refactor"),
            Some(BenchmarkLane::RiskyRefactor)
        );
        assert_eq!(BenchmarkLane::from_token("nope"), None);
    }

    #[test]
    fn failure_class_token_round_trip() {
        for fc in FailureClass::ALL {
            assert_eq!(FailureClass::from_token(fc.as_str()), Some(fc));
        }
        assert_eq!(FailureClass::ALL.len(), 23);
    }

    #[test]
    fn state_enum_token_round_trips() {
        for d in FinalDecision::ALL {
            assert_eq!(FinalDecision::from_token(d.as_str()), Some(d));
        }
        for g in ObjectiveGate::ALL {
            assert_eq!(ObjectiveGate::from_token(g.as_str()), Some(g));
        }
        for d in VerifierDecision::ALL {
            assert_eq!(VerifierDecision::from_token(d.as_str()), Some(d));
        }
        for s in FairnessStatus::ALL {
            assert_eq!(FairnessStatus::from_token(s.as_str()), Some(s));
        }
    }

    #[test]
    fn verifier_parse_spec_mode_is_stable() {
        assert_eq!(VerifierParse::Json.spec_mode(), "strict_valid");
        assert_eq!(VerifierParse::Salvaged.spec_mode(), "salvage_valid");
        assert_eq!(VerifierParse::Unparseable.spec_mode(), "malformed");
        assert_eq!(VerifierParse::Empty.spec_mode(), "missing");
        assert_eq!(VerifierParse::Timeout.spec_mode(), "timeout");
    }
}
