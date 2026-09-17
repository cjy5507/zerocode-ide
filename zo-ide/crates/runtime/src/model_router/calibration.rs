//! Learned complexity calibration: promote a (role, complexity) class one
//! band when the outcome log shows that class keeps failing at its assigned
//! tier.
//!
//! The jacobian-lens lesson applied here: a small linear read over a modest
//! sample (the paper's lens saturates around ~100 prompts) beats a static
//! table — so instead of hand-tuning keyword bands forever, the router
//! learns "tasks this classifier calls `small` under role X keep failing"
//! and floors that class one band up. Bounded on purpose: exactly one band,
//! only upward (an under-modeled task costs quality; an over-modeled one
//! only costs tokens — the same asymmetry as probe fusion), and only on
//! enough decisive samples that the signal is not noise.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use super::outcome::{decisive_outcome, is_terminal_outcome_status, learning_samples, RouteOutcomeRecord};
use super::policy::RouteTaskComplexity;

/// Decisive samples a (role, complexity) class needs before calibration may
/// promote it. Mirrors the spirit of `CONFIDENT_DECISIVE_SAMPLES` — below
/// this, one flaky spawn would flip the class.
pub const CALIBRATION_MIN_SAMPLES: usize = 5;

/// Failure share (failed over decisive) at or above which a class is
/// considered under-provisioned at its current band.
pub const CALIBRATION_FAILURE_SHARE: f32 = 0.5;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ClassOutcome {
    completed: usize,
    failed: usize,
}

/// Per-(role, complexity) learned promotion table, computed from route
/// outcome records. Pure data — no IO, no clock.
#[derive(Debug, Clone, Default)]
pub struct ComplexityCalibration {
    /// Keys are `(role_label, complexity_label)` exactly as the outcome
    /// records carry them (`role: "coding"`, `complexity: "small"`).
    promoted: BTreeMap<(String, String), ClassOutcome>,
}

impl ComplexityCalibration {
    /// An inert table (no records → nothing promotes).
    #[must_use]
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Aggregate decisive outcomes per (role, complexity) class and keep the
    /// classes that clear both the sample floor and the failure share. Reads
    /// the router's learning samples only — one per attributed attempt, never
    /// a fold — so a completion whose work then failed verification counts as
    /// the failure it was, not as a success that hides it.
    #[must_use]
    pub fn compute(records: &[RouteOutcomeRecord]) -> Self {
        let mut classes: BTreeMap<(String, String), ClassOutcome> = BTreeMap::new();
        for record in learning_samples(records) {
            if !is_terminal_outcome_status(&record.status) {
                continue;
            }
            let Some(passed) = decisive_outcome(&record.status, record.provider_error_class.as_deref()) else {
                continue;
            };
            let (Some(role), Some(complexity)) = (record.role.as_deref(), record.complexity.as_deref())
            else {
                continue;
            };
            // `unknown` complexity carries no band to promote; a promoted
            // `large` has no higher band. Skip both up front so the table
            // only ever holds actionable classes.
            if matches!(complexity, "unknown" | "large") {
                continue;
            }
            let entry = classes
                .entry((role.to_ascii_lowercase(), complexity.to_ascii_lowercase()))
                .or_default();
            if passed {
                entry.completed += 1;
            } else {
                entry.failed += 1;
            }
        }
        classes.retain(|_, outcome| {
            let decisive = outcome.completed + outcome.failed;
            decisive >= CALIBRATION_MIN_SAMPLES
                && failure_share(*outcome) >= CALIBRATION_FAILURE_SHARE
        });
        Self { promoted: classes }
    }

    /// Whether any class is promoted (for status surfaces).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.promoted.is_empty()
    }

    /// The promoted classes as `(role, complexity, completed, failed)` rows,
    /// for `/smart doctor`.
    #[must_use]
    pub fn promoted_classes(&self) -> Vec<(String, String, usize, usize)> {
        self.promoted
            .iter()
            .map(|((role, complexity), outcome)| {
                (role.clone(), complexity.clone(), outcome.completed, outcome.failed)
            })
            .collect()
    }

    /// The calibrated complexity for a route: one band above the classified
    /// one when this (role, complexity) class is promoted, otherwise the
    /// classified value unchanged. Only ever raises, only ever one band.
    #[must_use]
    pub fn calibrated_complexity(
        &self,
        role_label: &str,
        complexity: RouteTaskComplexity,
    ) -> RouteTaskComplexity {
        let label = match complexity {
            RouteTaskComplexity::Trivial => "trivial",
            RouteTaskComplexity::Small => "small",
            RouteTaskComplexity::Medium => "medium",
            // No band above Large; Unknown has no evidence class.
            RouteTaskComplexity::Large | RouteTaskComplexity::Unknown => return complexity,
        };
        if !self
            .promoted
            .contains_key(&(role_label.to_ascii_lowercase(), label.to_string()))
        {
            return complexity;
        }
        match complexity {
            RouteTaskComplexity::Trivial => RouteTaskComplexity::Small,
            RouteTaskComplexity::Small => RouteTaskComplexity::Medium,
            RouteTaskComplexity::Medium => RouteTaskComplexity::Large,
            RouteTaskComplexity::Large | RouteTaskComplexity::Unknown => complexity,
        }
    }
}

fn failure_share(outcome: ClassOutcome) -> f32 {
    let decisive = outcome.completed + outcome.failed;
    if decisive == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    {
        outcome.failed as f32 / decisive as f32
    }
}

/// Enumerate every project's route-outcome log under the active config home
/// (`projects/<slug>/state/smart-router/route-outcomes.jsonl`) — the
/// cross-project merge (`JacobianLens.merge()` analog) used by `/smart
/// doctor`, where one project's thin history can borrow its siblings'
/// evidence. Deliberately NOT used on the per-spawn routing path: routing
/// reads exactly one file per batch, and cross-project reads there would
/// multiply IO per fan-out.
pub fn route_outcome_log_paths_across_projects() -> io::Result<Vec<PathBuf>> {
    // Mirror `zo_project_state_dir`'s root resolution exactly: an active
    // `ZO_STATE_DIR` override relocates every project's state, so the
    // cross-project walk must follow it or it would enumerate a root no
    // recorder writes to.
    let root = std::env::var_os(core_types::paths::ZO_STATE_DIR_ENV)
        .filter(|dir| !dir.is_empty())
        .map_or_else(crate::default_config_home, PathBuf::from);
    let projects = root.join("projects");
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(&projects)? {
        let entry = entry?;
        let candidate = entry
            .path()
            .join("state")
            .join("smart-router")
            .join("route-outcomes.jsonl");
        if candidate.is_file() {
            paths.push(candidate);
        }
    }
    paths.sort();
    Ok(paths)
}

/// Read and concatenate route outcomes across every project under the config
/// home. Unreadable files are skipped (best-effort — one corrupt sibling
/// project must not break the merge). Each file is parsed by the same reader
/// the per-project routing path uses, so the two aggregates can never drift
/// on corrupt-line/schema handling.
#[must_use]
pub fn read_route_outcomes_across_projects() -> Vec<RouteOutcomeRecord> {
    let Ok(paths) = route_outcome_log_paths_across_projects() else {
        return Vec::new();
    };
    paths
        .iter()
        .filter_map(|path| super::outcome::read_route_outcomes_from_path(path).ok())
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(role: &str, complexity: &str, status: &str) -> RouteOutcomeRecord {
        RouteOutcomeRecord::new("subagent", "general-purpose", "model-x", status)
            .with_role(Some(role.to_string()))
            .with_complexity(Some(complexity.to_string()))
    }

    #[test]
    fn failing_class_promotes_one_band_only() {
        let records: Vec<_> = (0..3)
            .map(|_| record("coding", "small", "failed"))
            .chain((0..2).map(|_| record("coding", "small", "completed")))
            .collect();
        let calibration = ComplexityCalibration::compute(&records);
        assert_eq!(
            calibration.calibrated_complexity("coding", RouteTaskComplexity::Small),
            RouteTaskComplexity::Medium
        );
        // Other classes and roles stay untouched.
        assert_eq!(
            calibration.calibrated_complexity("coding", RouteTaskComplexity::Medium),
            RouteTaskComplexity::Medium
        );
        assert_eq!(
            calibration.calibrated_complexity("analysis", RouteTaskComplexity::Small),
            RouteTaskComplexity::Small
        );
    }

    #[test]
    fn thin_or_healthy_classes_never_promote() {
        // 4 samples: below the floor even at 100% failure.
        let thin: Vec<_> = (0..4).map(|_| record("coding", "small", "failed")).collect();
        assert!(ComplexityCalibration::compute(&thin).is_empty());
        // 6 samples, 2 failures: failure share below the bar.
        let healthy: Vec<_> = (0..4)
            .map(|_| record("coding", "small", "completed"))
            .chain((0..2).map(|_| record("coding", "small", "failed")))
            .collect();
        assert!(ComplexityCalibration::compute(&healthy).is_empty());
    }

    #[test]
    fn unavailable_verification_does_not_promote_task_complexity() {
        let records: Vec<_> = (0..CALIBRATION_MIN_SAMPLES).flat_map(|attempt| {
            let id = format!("unverified-{attempt}");
            [record("coding", "small", "completed").with_attempt(&id, 0),
             record("coding", "small", "stopped").with_attempt(&id, 0)
                .with_decision(super::super::outcome::DecisionKind::Verify).with_signal("verdict")]
        }).collect();
        assert!(ComplexityCalibration::compute(&records).is_empty());
    }

    #[test]
    fn large_and_unknown_are_never_promoted() {
        let records: Vec<_> = (0..6)
            .map(|_| record("coding", "large", "failed"))
            .chain((0..6).map(|_| record("coding", "unknown", "failed")))
            .collect();
        let calibration = ComplexityCalibration::compute(&records);
        assert!(calibration.is_empty());
        assert_eq!(
            calibration.calibrated_complexity("coding", RouteTaskComplexity::Large),
            RouteTaskComplexity::Large
        );
        assert_eq!(
            calibration.calibrated_complexity("coding", RouteTaskComplexity::Unknown),
            RouteTaskComplexity::Unknown
        );
    }

    /// Three attempts that finished but failed verification, beside two plain
    /// completions: counted per receipt the class read 5 completed / 3 failed
    /// (share 0.375, never promoted) — the completions of the failed work hid
    /// it. Per attempt it is 2 / 3 (share 0.6) and the class is promoted.
    #[test]
    fn a_verified_failure_is_not_hidden_by_its_own_completion() {
        let mut records = Vec::new();
        for attempt in 0..3 {
            let agent = format!("agent-{attempt}");
            records.push(record("coding", "small", "completed").with_attempt(&agent, 1));
            records.push(
                record("coding", "small", "failed")
                    .with_decision(super::super::outcome::DecisionKind::Verify)
                    .with_attempt(&agent, 1),
            );
        }
        records.push(record("coding", "small", "completed"));
        records.push(record("coding", "small", "completed"));

        let calibration = ComplexityCalibration::compute(&records);
        assert_eq!(
            calibration.promoted_classes(),
            vec![("coding".to_string(), "small".to_string(), 2, 3)]
        );
    }

    #[test]
    fn non_terminal_statuses_are_ignored() {
        let records: Vec<_> = (0..10)
            .map(|_| record("coding", "small", "still_running"))
            .collect();
        assert!(ComplexityCalibration::compute(&records).is_empty());
    }
}
