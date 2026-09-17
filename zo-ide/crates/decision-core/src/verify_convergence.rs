//! Semantic convergence for repeated verification rounds.
//!
//! The verification treadmill guard counts *rounds*; the stall detector hashes
//! *identical* failure sets. Both provably miss the worst verification runaway:
//! a repair⇄re-verify oscillation where every round produces a *different*
//! finding set (an adversarial verifier prompted to find problems almost always
//! finds one), so the loop's stop condition — "the verifier reports nothing" —
//! is never reached. The observed pathology is a goal that runs dozens of
//! "final" verification rounds, each followed by a repair that spawns the next
//! round.
//!
//! This module tracks the *content* of successive verification rounds and
//! answers: is more verification still buying anything?
//!
//! - **Converged** — the last round was clean, or several rounds running have
//!   produced no *new blocking* finding. Advisory: the caller may stop
//!   verifying and report honestly (it never overrides an objective red).
//! - **Diverging** — verification is provably not converging: a finding that
//!   was resolved has reappeared (churn — repairs are undoing each other), or
//!   the round cap is reached while new blocking findings still appear (no net
//!   progress). The caller should stop the verify loop and hand the open
//!   findings to the human instead of buying another round.
//!
//! Deliberately conservative: rounds are only folded when the verifier produced
//! *concrete* findings text (a bare rejection carries no discriminating signal
//! and is never folded — mirroring how the stall signature excludes the
//! constant semantic-rejection marker), churn requires an exact
//! normalized-key match, and `Converged` is advice, never a fabricated accept.
//! Pure and total (no IO); serializable so a resumed loop keeps its ledger.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::loop_progress::failure_signature;

/// Consecutive no-new-blocking-finding rounds that count as converged.
pub const CONVERGENCE_QUIET_ROUNDS: u32 = 2;
/// Resolved-then-reappeared findings that count as churn (diverging).
pub const CONVERGENCE_CHURN_LIMIT: u32 = 2;
/// Round cap: reaching it while new blocking findings still appear is
/// "no net progress" (diverging). Chosen below the treadmill guard's soft
/// threshold so the semantic verdict lands first and the round count stays
/// a blunt backstop.
pub const CONVERGENCE_MAX_ROUNDS: u32 = 4;

/// Severity of one verification finding. Ordered so `blocking <= severity`
/// comparisons read naturally (`Low < Medium < High < Critical`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum FindingSeverity {
    Low,
    Medium,
    High,
    Critical,
}

/// One verification finding: a normalized identity key plus severity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Normalized identity ([`failure_signature`] of the text minus any
    /// severity prefix) — severity re-grading alone is not a new finding.
    pub key: u64,
    pub severity: FindingSeverity,
    /// The finding text (severity prefix stripped), retained so a diverging
    /// stop can show the human *which* findings are still open.
    pub text: String,
}

/// Parse one verifier issue line into a [`Finding`].
///
/// Severity comes from an optional case-insensitive `critical:`/`high:`/
/// `medium:`/`low:` prefix (Korean parity: `치명:`/`높음:`/`중간:`/`낮음:`);
/// an unprefixed finding defaults to `Medium` — conservative in both
/// directions (it neither blocks convergence like `High` nor is discounted
/// like `Low`). Blank text is no finding.
#[must_use]
pub fn finding_from_text(text: &str) -> Option<Finding> {
    const PREFIXES: &[(&str, FindingSeverity)] = &[
        ("critical:", FindingSeverity::Critical),
        ("치명:", FindingSeverity::Critical),
        ("high:", FindingSeverity::High),
        ("높음:", FindingSeverity::High),
        ("medium:", FindingSeverity::Medium),
        ("중간:", FindingSeverity::Medium),
        ("low:", FindingSeverity::Low),
        ("낮음:", FindingSeverity::Low),
    ];
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_lowercase();
    let (severity, body) = PREFIXES
        .iter()
        .find(|(prefix, _)| lower.starts_with(prefix))
        .map_or((FindingSeverity::Medium, trimmed), |(prefix, severity)| {
            (*severity, trimmed[prefix.len()..].trim_start())
        });
    if body.is_empty() {
        return None;
    }
    Some(Finding {
        key: failure_signature(&[body.to_string()]),
        severity,
        text: body.to_string(),
    })
}

/// Tuning knobs for the convergence decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConvergencePolicy {
    pub quiet_rounds: u32,
    pub churn_limit: u32,
    pub max_rounds: u32,
    /// Findings at or above this severity block convergence; below it they are
    /// recorded (and reported) but do not keep the loop verifying.
    pub blocking: FindingSeverity,
}

impl Default for ConvergencePolicy {
    fn default() -> Self {
        Self {
            quiet_rounds: CONVERGENCE_QUIET_ROUNDS,
            churn_limit: CONVERGENCE_CHURN_LIMIT,
            max_rounds: CONVERGENCE_MAX_ROUNDS,
            blocking: FindingSeverity::High,
        }
    }
}

/// The verdict after folding one verification round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvergenceVerdict {
    /// Verification has converged — advisory only: stop verifying and report
    /// honestly. Never a fabricated accept (an objective red still blocks).
    Converged(ConvergedReason),
    /// No verdict yet — keep working.
    Continue,
    /// More verification provably cannot converge — stop the verify loop and
    /// hand the open findings to the human.
    Diverging(DivergingReason),
}

/// Why verification is considered converged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvergedReason {
    /// The round produced no findings at all.
    CleanRound,
    /// Several rounds running produced no new blocking finding.
    QuietStreak,
}

/// Why verification is considered diverging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DivergingReason {
    /// Resolved findings reappeared — repairs are undoing each other.
    Churn,
    /// The round cap was reached while new blocking findings still appear.
    NoNetProgress,
}

impl DivergingReason {
    /// Short human label for the stop digest.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Churn => "verification churn: repaired findings keep reappearing",
            Self::NoNetProgress => {
                "no net progress: new blocking findings keep appearing every round"
            }
        }
    }
}

/// Per-finding history inside the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FindingRecord {
    severity: FindingSeverity,
    /// Round the finding was last reported in.
    last_round: u32,
    /// True once a later round omitted it (the repair evidently addressed it).
    resolved: bool,
    /// Truncated text sample for the diverging digest.
    text: String,
}

/// Maximum characters of finding text retained per record (digest sample).
const MAX_RECORD_TEXT: usize = 160;

/// Ledger of verification rounds for one goal/loop. Serializable, mirrors
/// [`ProgressTracker`](crate::loop_progress::ProgressTracker) in spirit: pure
/// state folded turn by turn, consulted for a stop verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ConvergenceLedger {
    rounds: u32,
    seen: BTreeMap<u64, FindingRecord>,
    reopened: u32,
    quiet_streak: u32,
}

impl ConvergenceLedger {
    /// Fold one verification round's findings and return the verdict.
    ///
    /// Callers must only invoke this for turns that actually produced a
    /// verification round with concrete findings text (or an explicit clean
    /// round) — a turn with no verification leaves the ledger untouched.
    pub fn observe_round(
        &mut self,
        findings: &[Finding],
        policy: &ConvergencePolicy,
    ) -> ConvergenceVerdict {
        self.rounds = self.rounds.saturating_add(1);

        // Dedup within the round so a verifier repeating itself in one reply
        // cannot double-count.
        let mut current: BTreeMap<u64, &Finding> = BTreeMap::new();
        for finding in findings {
            current.entry(finding.key).or_insert(finding);
        }

        // Any previously-open finding absent from this round was evidently
        // addressed by the intervening repair: mark it resolved.
        for (key, record) in &mut self.seen {
            if !record.resolved && !current.contains_key(key) {
                record.resolved = true;
            }
        }

        let mut new_blocking = 0u32;
        for (key, finding) in current {
            if let Some(record) = self.seen.get_mut(&key) {
                record.last_round = self.rounds;
                if record.resolved {
                    // A repaired finding is back: the strongest evidence that
                    // repairs are oscillating rather than converging.
                    record.resolved = false;
                    self.reopened = self.reopened.saturating_add(1);
                }
            } else {
                if finding.severity >= policy.blocking {
                    new_blocking = new_blocking.saturating_add(1);
                }
                self.seen.insert(
                    key,
                    FindingRecord {
                        severity: finding.severity,
                        last_round: self.rounds,
                        resolved: false,
                        text: truncate_chars(&finding.text, MAX_RECORD_TEXT),
                    },
                );
            }
        }

        if policy.churn_limit > 0 && self.reopened >= policy.churn_limit {
            return ConvergenceVerdict::Diverging(DivergingReason::Churn);
        }
        if findings.is_empty() {
            return ConvergenceVerdict::Converged(ConvergedReason::CleanRound);
        }
        if new_blocking == 0 {
            self.quiet_streak = self.quiet_streak.saturating_add(1);
        } else {
            self.quiet_streak = 0;
        }
        if policy.quiet_rounds > 0 && self.quiet_streak >= policy.quiet_rounds {
            return ConvergenceVerdict::Converged(ConvergedReason::QuietStreak);
        }
        if policy.max_rounds > 0 && self.rounds >= policy.max_rounds && new_blocking > 0 {
            return ConvergenceVerdict::Diverging(DivergingReason::NoNetProgress);
        }
        ConvergenceVerdict::Continue
    }

    /// Verification rounds folded so far (diagnostics/digests).
    #[must_use]
    pub const fn rounds(&self) -> u32 {
        self.rounds
    }

    /// Text samples of findings still open (unresolved), most recent round
    /// first, capped at `limit` — the "what is still broken" list for a
    /// diverging stop digest.
    #[must_use]
    pub fn unresolved_samples(&self, limit: usize) -> Vec<&str> {
        let mut open: Vec<&FindingRecord> = self
            .seen
            .values()
            .filter(|record| !record.resolved)
            .collect();
        open.sort_by(|a, b| b.last_round.cmp(&a.last_round));
        open.into_iter()
            .take(limit)
            .map(|record| record.text.as_str())
            .collect()
    }
}

/// Char-boundary-safe truncation with an ellipsis, for digest samples.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(texts: &[&str]) -> Vec<Finding> {
        texts
            .iter()
            .filter_map(|text| finding_from_text(text))
            .collect()
    }

    fn policy() -> ConvergencePolicy {
        ConvergencePolicy::default()
    }

    #[test]
    fn severity_prefix_parsing_english_and_korean() {
        assert_eq!(
            finding_from_text("critical: data loss on retry").map(|f| f.severity),
            Some(FindingSeverity::Critical)
        );
        assert_eq!(
            finding_from_text("HIGH: race in shutdown").map(|f| f.severity),
            Some(FindingSeverity::High)
        );
        assert_eq!(
            finding_from_text("낮음: 변수명이 관례와 다름").map(|f| f.severity),
            Some(FindingSeverity::Low)
        );
        // No prefix ⇒ Medium (conservative default), text preserved.
        let plain = finding_from_text("missing null check").expect("finding");
        assert_eq!(plain.severity, FindingSeverity::Medium);
        assert_eq!(plain.text, "missing null check");
        // Severity re-grade of the same body is the SAME finding.
        assert_eq!(
            finding_from_text("high: dangling lock").map(|f| f.key),
            finding_from_text("low: dangling lock").map(|f| f.key)
        );
        // Blank / prefix-only are no findings.
        assert_eq!(finding_from_text("   "), None);
        assert_eq!(finding_from_text("high:"), None);
    }

    #[test]
    fn clean_round_converges_immediately() {
        let mut ledger = ConvergenceLedger::default();
        assert_eq!(
            ledger.observe_round(&[], &policy()),
            ConvergenceVerdict::Converged(ConvergedReason::CleanRound)
        );
    }

    #[test]
    fn fresh_blocking_findings_continue() {
        let mut ledger = ConvergenceLedger::default();
        assert_eq!(
            ledger.observe_round(&findings(&["high: race in worker"]), &policy()),
            ConvergenceVerdict::Continue
        );
        // A different blocking finding next round: still working, still fine.
        assert_eq!(
            ledger.observe_round(&findings(&["high: leak in shutdown"]), &policy()),
            ConvergenceVerdict::Continue
        );
    }

    #[test]
    fn quiet_streak_of_low_severity_rounds_converges() {
        let mut ledger = ConvergenceLedger::default();
        assert_eq!(
            ledger.observe_round(&findings(&["low: nit A"]), &policy()),
            ConvergenceVerdict::Continue
        );
        assert_eq!(
            ledger.observe_round(&findings(&["low: nit B"]), &policy()),
            ConvergenceVerdict::Converged(ConvergedReason::QuietStreak)
        );
    }

    #[test]
    fn reopened_findings_diverge_as_churn() {
        let mut ledger = ConvergenceLedger::default();
        // Round 1: two findings.
        assert_eq!(
            ledger.observe_round(&findings(&["high: X", "high: Y"]), &policy()),
            ConvergenceVerdict::Continue
        );
        // Round 2: both gone (repaired), one new finding.
        assert_eq!(
            ledger.observe_round(&findings(&["high: Z"]), &policy()),
            ConvergenceVerdict::Continue
        );
        // Round 3: the two "repaired" findings are BACK — churn.
        assert_eq!(
            ledger.observe_round(&findings(&["high: X", "high: Y"]), &policy()),
            ConvergenceVerdict::Diverging(DivergingReason::Churn)
        );
    }

    #[test]
    fn round_cap_with_new_blocking_findings_is_no_net_progress() {
        let mut ledger = ConvergenceLedger::default();
        for round in 1..=3u32 {
            assert_eq!(
                ledger.observe_round(&findings(&[&format!("high: issue {round}")]), &policy()),
                ConvergenceVerdict::Continue,
                "round {round}"
            );
        }
        assert_eq!(
            ledger.observe_round(&findings(&["high: issue 4"]), &policy()),
            ConvergenceVerdict::Diverging(DivergingReason::NoNetProgress)
        );
    }

    #[test]
    fn round_cap_without_new_blocking_findings_does_not_diverge() {
        let mut ledger = ConvergenceLedger::default();
        // Rounds 1-3: blocking findings (Continue), round 4+: only Low nits —
        // the cap alone must NOT fire; quiet convergence wins instead.
        for round in 1..=3u32 {
            ledger.observe_round(&findings(&[&format!("high: issue {round}")]), &policy());
        }
        assert_eq!(
            ledger.observe_round(&findings(&["low: nit"]), &policy()),
            ConvergenceVerdict::Continue
        );
        assert_eq!(
            ledger.observe_round(&findings(&["low: other nit"]), &policy()),
            ConvergenceVerdict::Converged(ConvergedReason::QuietStreak)
        );
    }

    #[test]
    fn duplicate_findings_within_a_round_count_once() {
        let mut ledger = ConvergenceLedger::default();
        let round = findings(&["high: same thing", "HIGH: same thing"]);
        assert_eq!(ledger.observe_round(&round, &policy()), ConvergenceVerdict::Continue);
        // The duplicate did not create churn or extra records.
        assert_eq!(ledger.unresolved_samples(10).len(), 1);
    }

    #[test]
    fn resolved_then_absent_findings_never_diverge() {
        // The healthy path: findings get fixed and STAY fixed.
        let mut ledger = ConvergenceLedger::default();
        ledger.observe_round(&findings(&["high: A", "medium: B"]), &policy());
        assert_eq!(
            ledger.observe_round(&[], &policy()),
            ConvergenceVerdict::Converged(ConvergedReason::CleanRound)
        );
        assert!(ledger.unresolved_samples(10).is_empty());
    }

    #[test]
    fn unresolved_samples_list_open_findings_most_recent_first() {
        let mut ledger = ConvergenceLedger::default();
        ledger.observe_round(&findings(&["high: old open"]), &policy());
        ledger.observe_round(&findings(&["high: old open", "high: newer open"]), &policy());
        let samples = ledger.unresolved_samples(10);
        assert_eq!(samples.len(), 2);
        // Both were last seen in round 2; order within a round is by key —
        // just assert containment and cap behavior.
        assert!(samples.contains(&"old open"));
        assert!(samples.contains(&"newer open"));
        assert_eq!(ledger.unresolved_samples(1).len(), 1);
    }

    #[test]
    fn zeroed_policy_knobs_disable_their_rules() {
        let off = ConvergencePolicy {
            quiet_rounds: 0,
            churn_limit: 0,
            max_rounds: 0,
            blocking: FindingSeverity::High,
        };
        let mut ledger = ConvergenceLedger::default();
        ledger.observe_round(&findings(&["high: X"]), &off);
        ledger.observe_round(&findings(&["high: Y"]), &off);
        // Churn scenario that would fire under defaults:
        ledger.observe_round(&findings(&["high: X"]), &off);
        for _ in 0..10 {
            assert_eq!(
                ledger.observe_round(&findings(&["low: nit"]), &off),
                ConvergenceVerdict::Continue
            );
        }
    }

    #[test]
    fn ledger_roundtrips_through_serde() {
        let mut ledger = ConvergenceLedger::default();
        ledger.observe_round(&findings(&["high: X"]), &policy());
        ledger.observe_round(&findings(&["low: Y"]), &policy());
        let json = serde_json::to_string(&ledger).expect("serialize");
        let back: ConvergenceLedger = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ledger, back);
    }
}
