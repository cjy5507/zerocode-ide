//! Orchestration accuracy report (orchestration-accuracy P4).
//!
//! The dashboard's data: assembled from the SAME outcome store the router
//! learns from, so "how accurate is my orchestration" is answered by the exact
//! evidence that drives it — the number a relay/fleet-manager (Vicoa) cannot
//! show, because it keeps no such evidence. Every rate is `Option`: `None` when
//! there is not yet enough evidence to state it, never a fabricated `0`.

use std::io;
use std::path::Path;

use serde::Serialize;

use super::outcome::{
    rate, read_route_outcomes, summarize_decisions_by_kind, verify_metrics,
    weakest_decision_kind, DecisionKind, RouteOutcomeRecord,
};

/// Sample floor the report's weakest-kind call uses — the module's documented
/// `>= 2 decisive samples` minimum before any outcome nudges a decision
/// (`RouteOutcomeBucket::feedback_adjustment`), named ONCE so the zo CLI
/// surface and the window's board card ask the same question.
pub const ACCURACY_MIN_DECISIVE: usize = 2;

/// The orchestration accuracy picture (P4). Serializes camelCase so the window
/// card reads `firstTrySuccess`, `weakestDecision`, … straight off the wire.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccuracyReport {
    /// Total decision records the report is built from.
    pub total_decisions: usize,
    /// `completed / (completed + failed)` across the decisions that produced
    /// work — first-try success, the headline "how often is orchestration right
    /// the first time". `fold` decisions are left out: a folded lane is a spawn
    /// whose output added nothing, so it has no first try to succeed at, and
    /// counting its `completed` status here let wasted spawns raise the
    /// headline (design §8). `fold_rate` carries them.
    pub first_try_success: Option<f64>,
    /// Verify catches / completed work (folds excluded, as above) — how often
    /// landed work still needed a fix.
    /// A caught defect is what triggers a repair round (the workflow loop's
    /// fixer, the verify-by-default retry, or a person), so the verify catch IS
    /// the rework signal; a separate rework record would count the same run
    /// twice. The record's `reworked` flag stays reserved for an explicit mark.
    pub rework_rate: Option<f64>,
    /// Verify decisions that caught a defect / all verify decisions.
    pub verify_catch_rate: Option<f64>,
    /// Verify decisions settled by model judgement alone / all verify decisions
    /// — the honest-risk gauge (how much of "verify passed" is only an opinion).
    pub model_only_verify_rate: Option<f64>,
    /// `fold` decisions / total decisions — wasted spawns folded away.
    pub fold_rate: Option<f64>,
    /// The decision kind with the lowest decisive success rate above the floor
    /// (its canonical label), or `None` when none clears the floor — where
    /// orchestration is going wrong.
    pub weakest_decision: Option<String>,
}

/// Build the accuracy report from decision-outcome records. `min_decisive` is
/// the sample floor for the weakest-kind call (the caller's confidence gate);
/// the rate fields use their own natural denominators. Reuses the P1 per-kind
/// aggregation and the P2 verify metrics — one assembly point, no re-derived
/// counting.
#[must_use]
pub fn orchestration_accuracy(
    records: &[RouteOutcomeRecord],
    min_decisive: usize,
) -> AccuracyReport {
    // A classify row is the tax a decision paid, not a decision: it is out of
    // every count here (a plan's total cost sums it through the attempt key).
    let stats: Vec<_> = summarize_decisions_by_kind(records)
        .into_iter()
        .filter(|stat| stat.decision != DecisionKind::Classify.as_str())
        .collect();
    let total_decisions: usize = stats.iter().map(|stat| stat.total).sum();
    let is_fold = |stat: &&super::outcome::DecisionOutcomeStat| stat.decision == DecisionKind::Fold.as_str();
    // Work decisions: everything but folds. A fold has no first try to
    // succeed at, so it belongs in `fold_rate` alone.
    let work = || stats.iter().filter(|stat| !is_fold(stat));
    let completed: usize = work().map(|stat| stat.completed).sum();
    let failed: usize = work().map(|stat| stat.failed).sum();
    let decisive = completed.saturating_add(failed);
    let fold_total: usize = stats.iter().filter(is_fold).map(|stat| stat.total).sum();
    let verify = verify_metrics(records);

    AccuracyReport {
        total_decisions,
        first_try_success: rate(completed, decisive),
        rework_rate: rate(verify.caught, completed),
        verify_catch_rate: verify.catch_rate(),
        model_only_verify_rate: verify.model_only_rate(),
        fold_rate: rate(fold_total, total_decisions),
        weakest_decision: weakest_decision_kind(&stats, min_decisive)
            .map(|stat| stat.decision.clone()),
    }
}

/// Read the project's outcome store and build its accuracy report — the reader
/// half the surface (CLI scoreboard / window card) calls.
pub fn read_orchestration_accuracy(cwd: &Path, min_decisive: usize) -> io::Result<AccuracyReport> {
    Ok(orchestration_accuracy(&read_route_outcomes(cwd)?, min_decisive))
}

#[cfg(test)]
mod tests {
    use super::super::outcome::VerdictBasis;
    use super::*;

    fn dec(target: &str, status: &str, kind: DecisionKind) -> RouteOutcomeRecord {
        RouteOutcomeRecord::new("subagent", target, "m", status).with_decision(kind)
    }

    #[test]
    fn a_classify_row_is_the_tax_of_deciding_and_moves_no_rate() {
        let work = || {
            vec![
                dec("Plan", "completed", DecisionKind::Model),
                dec("Plan", "failed", DecisionKind::Model),
            ]
        };
        let without = orchestration_accuracy(&work(), 1);
        let mut with_tax = work();
        with_tax.push(
            RouteOutcomeRecord::new("main", "probe", "fast", "failed").with_decision(DecisionKind::Classify),
        );
        with_tax.push(
            RouteOutcomeRecord::new("main", "decompose", "fast", "completed")
                .with_decision(DecisionKind::Classify),
        );
        let report = orchestration_accuracy(&with_tax, 1);
        assert_eq!(report.total_decisions, without.total_decisions);
        assert_eq!(report.first_try_success, without.first_try_success);
        assert_eq!(report.fold_rate, without.fold_rate);
        assert_eq!(report.weakest_decision, without.weakest_decision);
        assert_ne!(report.weakest_decision.as_deref(), Some(DecisionKind::Classify.as_str()));
    }

    #[test]
    fn accuracy_report_assembles_every_axis_from_the_store() {
        let records = vec![
            // model: 3 completed, 1 failed -> 3/4
            dec("Plan", "completed", DecisionKind::Model),
            dec("Plan", "completed", DecisionKind::Model),
            dec("Plan", "completed", DecisionKind::Model),
            dec("Plan", "failed", DecisionKind::Model),
            // verify: 2 completed (model-basis), 1 failed (caught) -> catch 1/3
            RouteOutcomeRecord::new("main", "turn", "m", "completed")
                .with_decision(DecisionKind::Verify)
                .with_verdict_basis(VerdictBasis::Model),
            RouteOutcomeRecord::new("main", "turn", "m", "completed")
                .with_decision(DecisionKind::Verify)
                .with_verdict_basis(VerdictBasis::Model),
            RouteOutcomeRecord::new("main", "turn", "m", "failed")
                .with_decision(DecisionKind::Verify)
                .with_verdict_basis(VerdictBasis::Model),
            // fold: 2 completed (one carries the reserved reworked mark) -> fold 2/9,
            // and NOT two first-try successes: a folded lane produced nothing.
            dec("fold", "completed", DecisionKind::Fold).with_reworked(true),
            dec("fold", "completed", DecisionKind::Fold),
        ];

        let report = orchestration_accuracy(&records, 2);

        assert_eq!(report.total_decisions, 9);
        // work decisions only: completed = 3+2 = 5, decisive = 7 -> 5/7
        assert!((report.first_try_success.unwrap() - 5.0 / 7.0).abs() < 1e-9);
        // rework = verify catches 1 / completed work 5 (the reserved reworked mark is not counted)
        assert!((report.rework_rate.unwrap() - 1.0 / 5.0).abs() < 1e-9);
        // verify caught 1 / 3 verify
        assert!((report.verify_catch_rate.unwrap() - 1.0 / 3.0).abs() < 1e-9);
        // all 3 verify are model-basis -> 3/3
        assert!((report.model_only_verify_rate.unwrap() - 1.0).abs() < 1e-9);
        // fold 2 / total 9
        assert!((report.fold_rate.unwrap() - 2.0 / 9.0).abs() < 1e-9);
        // weakest: verify 0.667 < model 0.75 < fold 1.0
        assert_eq!(report.weakest_decision.as_deref(), Some("verify"));
    }

    #[test]
    fn an_empty_store_states_no_fabricated_zeroes() {
        let report = orchestration_accuracy(&[], 2);
        assert_eq!(report.total_decisions, 0);
        assert_eq!(report.first_try_success, None);
        assert_eq!(report.rework_rate, None);
        assert_eq!(report.verify_catch_rate, None);
        assert_eq!(report.model_only_verify_rate, None);
        assert_eq!(report.fold_rate, None);
        assert_eq!(report.weakest_decision, None);
    }
}
