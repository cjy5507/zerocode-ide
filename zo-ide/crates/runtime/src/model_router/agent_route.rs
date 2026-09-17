//! Agent-axis routing from outcome evidence (orchestration-accuracy P3).
//!
//! Model routing (which model) is the original axis; this adds the AGENT axis
//! (which fleet agent — claude / codex / gemini — ran the work), learned the
//! same way: from the recorded decisive outcomes. Pure and api-free — the
//! caller injects `agent_of`, a model-id -> agent-label map (the tools layer
//! builds it from the live catalog's `provider_for_model`, the same injection
//! pattern [`summarize_route_outcomes_with_canonicalizer`] uses for the
//! model-canonicalizer). Vicoa runs the agent the user picked; this is the
//! layer that PICKS the agent from what actually worked before — the axis a
//! relay/fleet-manager has no signal for.

use std::collections::BTreeMap;

use super::outcome::{
    confidence_weighted_margin, decisive_outcome, learning_samples, rate, RouteOutcomeRecord,
};
use super::policy::MAX_FEEDBACK_ADJUSTMENT;

/// One agent's decisive tally for a `route_key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRouteStat {
    pub agent: String,
    pub completed: usize,
    pub failed: usize,
}

impl AgentRouteStat {
    #[must_use]
    fn decisive(&self) -> usize {
        self.completed.saturating_add(self.failed)
    }

    /// Decisive success rate `completed / (completed + failed)` in `0.0..=1.0`;
    /// `0.0` when the agent has no decisive samples. Display only — ranking
    /// uses [`AgentRouteStat::score`], never this raw rate.
    #[must_use]
    pub fn success_rate(&self) -> f64 {
        rate(self.completed, self.decisive()).unwrap_or(0.0)
    }

    /// Confidence-weighted margin (`confidence_weighted_margin`, the module's
    /// shared ramp) — what the ranker orders on, so a lucky 2-of-2 cannot
    /// outrank a proven 15-of-16. `0.0` with no decisive samples (never wins).
    #[must_use]
    pub fn score(&self) -> f64 {
        confidence_weighted_margin(self.completed, self.failed).unwrap_or(0.0)
    }
}

/// Per-agent decisive tally for one `route_key`, the agent label taken from
/// `agent_of(selected_model)`. Reuses `decisive_outcome` so a `stopped` run
/// or an infra-class provider fault counts for neither agent, and reads the
/// router's learning samples only — one per attributed attempt, never a fold.
/// Sorted by agent label for a deterministic result.
#[must_use]
pub fn agent_stats_for_route(
    records: &[RouteOutcomeRecord],
    route_key: &str,
    agent_of: impl Fn(&str) -> String,
) -> Vec<AgentRouteStat> {
    let mut by_agent: BTreeMap<String, AgentRouteStat> = BTreeMap::new();
    for record in learning_samples(records) {
        if record.route_key != route_key {
            continue;
        }
        let Some(win) = decisive_outcome(record.status.as_str(), record.provider_error_class.as_deref())
        else {
            continue;
        };
        let agent = agent_of(&record.selected_model);
        let stat = by_agent.entry(agent.clone()).or_insert(AgentRouteStat {
            agent,
            completed: 0,
            failed: 0,
        });
        if win {
            stat.completed = stat.completed.saturating_add(1);
        } else {
            stat.failed = stat.failed.saturating_add(1);
        }
    }
    by_agent.into_values().collect()
}

/// The agent with the best confidence-weighted margin ([`AgentRouteStat::score`])
/// for `route_key`, among agents with at least `min_samples` decisive samples.
/// Ranking on the weighted margin — not a raw rate — is what lets a proven
/// 15-of-16 beat a lucky 2-of-2. `None` when no agent clears the floor OR the
/// top two are tied (no evidence to prefer one over the other) — the caller
/// then keeps its existing default rather than guessing.
///
/// A user-named agent must be honored BEFORE this is consulted (the same pin
/// discipline a user-named model gets); this is the AUTO-selection path only.
/// Not yet wired into the live `assignment` blend — the substrate lands first
/// (mirrors `weighted_feedback_hint_for_route_key`'s deferred-wiring note), so
/// routing behavior stays byte-identical until the blend is explicitly signed
/// off.
#[must_use]
pub fn preferred_agent_for_route(
    records: &[RouteOutcomeRecord],
    route_key: &str,
    min_samples: usize,
    agent_of: impl Fn(&str) -> String,
) -> Option<String> {
    preferred_stat(agent_stats_for_route(records, route_key, agent_of), min_samples).map(|stat| stat.agent)
}

/// The routing nudge the outcome evidence earns for one agent on `route_key`:
/// the preferred agent (see [`preferred_agent_for_route`]) and its
/// confidence-weighted margin scaled to the SAME bound every other feedback
/// adjustment uses (`MAX_FEEDBACK_ADJUSTMENT`), so an agent preference composes
/// with — and can never out-shout — the per-model feedback. `None` when no
/// agent is preferred. The tools layer applies it to every candidate model of
/// that agent (`agent_of(model) == agent`).
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn agent_preference_adjustment(
    records: &[RouteOutcomeRecord],
    route_key: &str,
    min_samples: usize,
    agent_of: impl Fn(&str) -> String,
) -> Option<(String, i16)> {
    let stat = preferred_stat(agent_stats_for_route(records, route_key, agent_of), min_samples)?;
    let max = f64::from(MAX_FEEDBACK_ADJUSTMENT);
    let bounded = (stat.score() * max).round().clamp(-max, max) as i16;
    Some((stat.agent, bounded))
}

/// The shared ranking behind both public entry points: keep agents at or above
/// the sample floor, order by confidence-weighted margin, and return `None` on
/// an ambiguous top-two tie so the caller keeps its default.
fn preferred_stat(stats: Vec<AgentRouteStat>, min_samples: usize) -> Option<AgentRouteStat> {
    let floor = min_samples.max(1);
    let mut eligible: Vec<AgentRouteStat> =
        stats.into_iter().filter(|stat| stat.decisive() >= floor).collect();
    if eligible.is_empty() {
        return None;
    }
    // Best confidence-weighted margin first; on an equal score, more decisive
    // evidence first; then label for determinism.
    eligible.sort_by(|left, right| {
        right
            .score()
            .partial_cmp(&left.score())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.decisive().cmp(&left.decisive()))
            .then_with(|| left.agent.cmp(&right.agent))
    });
    // Ambiguous when the top two share the same score AND the same decisive
    // weight — no evidence to prefer one; keep the caller's default.
    if let [top, second, ..] = eligible.as_slice() {
        let same_score = (top.score() - second.score()).abs() < f64::EPSILON;
        if same_score && top.decisive() == second.decisive() {
            return None;
        }
    }
    eligible.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test `agent_of`: a model-id prefix -> fleet agent label. Real callers
    /// inject the catalog's `provider_for_model`; the tests only need a stable
    /// stub, not the live map.
    fn agent_of(model: &str) -> String {
        if model.starts_with("claude") {
            "claude".to_string()
        } else if model.starts_with("gpt") {
            "codex".to_string()
        } else if model.starts_with("gemini") {
            "gemini".to_string()
        } else {
            "other".to_string()
        }
    }

    fn rec(route_target: &str, model: &str, status: &str) -> RouteOutcomeRecord {
        RouteOutcomeRecord::new("subagent", route_target, model, status)
    }

    #[test]
    fn prefers_the_agent_with_the_better_decisive_rate() {
        let records = vec![
            rec("code-reviewer", "gpt-5.6-sol", "completed"),
            rec("code-reviewer", "gpt-5.6-sol", "completed"),
            rec("code-reviewer", "gpt-5.6-sol", "completed"),
            rec("code-reviewer", "gpt-5.6-sol", "completed"),
            rec("code-reviewer", "claude-opus-5", "completed"),
            rec("code-reviewer", "claude-opus-5", "failed"),
        ];
        assert_eq!(
            preferred_agent_for_route(&records, "subagent:code-reviewer", 2, agent_of).as_deref(),
            Some("codex"),
            "codex 4-0 beats claude 1-1"
        );
    }

    #[test]
    fn an_agent_below_the_sample_floor_is_ignored() {
        let records = vec![
            // codex: only 1 decisive sample (below floor 2) — excluded even at 1.0.
            rec("debugger", "gpt-5.6-sol", "completed"),
            // claude: 4 decisive at 0.75 — the only eligible agent.
            rec("debugger", "claude-opus-5", "completed"),
            rec("debugger", "claude-opus-5", "completed"),
            rec("debugger", "claude-opus-5", "completed"),
            rec("debugger", "claude-opus-5", "failed"),
        ];
        assert_eq!(
            preferred_agent_for_route(&records, "subagent:debugger", 2, agent_of).as_deref(),
            Some("claude")
        );
    }

    /// Ranking by RAW rate lets a lucky 2-of-2 (1.0) outrank a well-evidenced
    /// 15-of-16 (0.94). The module's confidence ramp
    /// (`CONFIDENT_DECISIVE_SAMPLES`, the same one `feedback_adjustment` uses)
    /// must weigh evidence: the agent that has actually proven itself wins
    /// (re-verification finding, 2026-09-10).
    #[test]
    fn a_well_evidenced_agent_outranks_a_lucky_small_sample() {
        let mut records = vec![
            rec("Refactor", "gpt-5.6-sol", "completed"),
            rec("Refactor", "gpt-5.6-sol", "completed"),
        ];
        for _ in 0..15 {
            records.push(rec("Refactor", "claude-opus-5", "completed"));
        }
        records.push(rec("Refactor", "claude-opus-5", "failed"));
        assert_eq!(
            preferred_agent_for_route(&records, "subagent:Refactor", 2, agent_of).as_deref(),
            Some("claude"),
            "15-of-16 is better evidence than a lucky 2-of-2"
        );
    }

    /// The agent nudge is the preferred agent's margin scaled to the shared
    /// feedback bound — one formula, no second constant.
    #[test]
    fn agent_preference_adjustment_scales_the_margin_to_the_feedback_bound() {
        let mut records = vec![
            rec("Refactor", "gpt-5.6-sol", "completed"),
            rec("Refactor", "gpt-5.6-sol", "completed"),
        ];
        for _ in 0..15 {
            records.push(rec("Refactor", "claude-opus-5", "completed"));
        }
        records.push(rec("Refactor", "claude-opus-5", "failed"));
        let (agent, adjustment) =
            agent_preference_adjustment(&records, "subagent:Refactor", 2, agent_of).expect("preferred");
        assert_eq!(agent, "claude");
        // claude: margin (15-1)/16 = 0.875 at full confidence (16 >= 8)
        #[allow(clippy::cast_possible_truncation)]
        let expected = (0.875 * f64::from(MAX_FEEDBACK_ADJUSTMENT)).round() as i16;
        assert_eq!(adjustment, expected);
        assert!(adjustment > 0 && adjustment <= MAX_FEEDBACK_ADJUSTMENT);
        // An ambiguous tie earns no nudge at all.
        let tie = vec![
            rec("Plan", "gpt-5.6-sol", "completed"),
            rec("Plan", "gpt-5.6-sol", "completed"),
            rec("Plan", "claude-opus-5", "completed"),
            rec("Plan", "claude-opus-5", "completed"),
        ];
        assert!(agent_preference_adjustment(&tie, "subagent:Plan", 2, agent_of).is_none());
    }

    #[test]
    fn a_tie_at_the_top_keeps_the_callers_default() {
        let records = vec![
            rec("Plan", "gpt-5.6-sol", "completed"),
            rec("Plan", "gpt-5.6-sol", "completed"),
            rec("Plan", "claude-opus-5", "completed"),
            rec("Plan", "claude-opus-5", "completed"),
        ];
        // Both 2-0 (rate 1.0, 2 decisive) — ambiguous, so no preference.
        assert_eq!(
            preferred_agent_for_route(&records, "subagent:Plan", 2, agent_of),
            None
        );
    }

    #[test]
    fn nothing_above_the_floor_is_none_and_other_route_keys_do_not_leak() {
        let records = vec![
            rec("Explore", "gpt-5.6-sol", "completed"),
            // A different route_key must not contribute.
            rec("code-reviewer", "gpt-5.6-sol", "completed"),
            rec("code-reviewer", "gpt-5.6-sol", "completed"),
        ];
        assert_eq!(
            preferred_agent_for_route(&records, "subagent:Explore", 2, agent_of),
            None,
            "Explore has 1 codex sample, below the floor; code-reviewer must not leak in"
        );
        // A stopped run is neither win nor loss.
        let stats = agent_stats_for_route(
            &[rec("Explore", "gpt-5.6-sol", "stopped")],
            "subagent:Explore",
            agent_of,
        );
        assert!(stats.is_empty(), "a cancelled run gives no decisive sample");
    }

    /// An agent whose attempts all finish but all fail verification must not
    /// hold its ground on completions: each attempt is ONE sample, its
    /// verdict. Counted per receipt, codex sat at 3-3 — the same zero margin
    /// as claude's honest 1-1 — and its bigger pile of receipts then won the
    /// tiebreak; per attempt it is 0-3.
    #[test]
    fn failed_verdicts_speak_for_the_attempts_they_judged() {
        let mut records = Vec::new();
        for attempt in 0..3 {
            let agent = format!("agent-{attempt}");
            records.push(rec("Refactor", "gpt-5.6-sol", "completed").with_attempt(&agent, 1));
            records.push(
                rec("Refactor", "gpt-5.6-sol", "failed")
                    .with_signal(super::super::outcome::VERDICT_SIGNAL)
                    .with_decision(super::super::outcome::DecisionKind::Verify)
                    .with_attempt(&agent, 1),
            );
        }
        records.push(rec("Refactor", "claude-opus-5", "completed"));
        records.push(rec("Refactor", "claude-opus-5", "failed"));

        let stats = agent_stats_for_route(&records, "subagent:Refactor", agent_of);
        let codex = stats.iter().find(|stat| stat.agent == "codex").expect("codex stat");
        assert_eq!((codex.completed, codex.failed), (0, 3), "{stats:?}");
        assert_eq!(
            preferred_agent_for_route(&records, "subagent:Refactor", 2, agent_of).as_deref(),
            Some("claude"),
            "1-1 beats 0-3 once completions stop masking caught failures"
        );
    }
}
