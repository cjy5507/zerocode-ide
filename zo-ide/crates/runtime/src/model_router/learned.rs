//! Phase 6: learned specialty hint — verdict-weighted, time-decayed,
//! win-rate-vs-same-role-peers score per (role, canonical model).
//!
//! The hardcoded `cold_start_specialty_seed` family table (`policy.rs`) is a
//! COLD-START PRIOR only — this module is the learned signal that operationally
//! replaces it as real outcome data
//! accrues, via `policy::effective_specialty_adjustment`'s confidence blend.
//!
//! This module stays a PURE function of its inputs (no clock read, no I/O,
//! no dependency on `api`/`tools`) — `now` and the canonicalizer are
//! injected, mirroring `outcome::weighted_feedback_hint_for_route_key`'s
//! seam exactly. It reuses that fn's underlying machinery
//! (`outcome::recency_weight`, `outcome::decisive_outcome`) rather than
//! duplicating the half-life/decisive-classification math.

use std::collections::BTreeMap;

use super::outcome::{decisive_outcome, learning_samples, recency_weight, RouteOutcomeRecord};
use super::target::{RouteRole, SubagentProfileId};

/// Minimum weighted decisive samples a (role, model) pair needs before it
/// contributes an entry at all. Below this floor the pair is simply ABSENT
/// from the hint — indistinguishable from zero-data, so
/// `effective_specialty_adjustment` falls back to the cold-start seed
/// unchanged.
///
/// Derived as half [`CONFIDENCE_RAMP_SAMPLES`] rather than written as its own
/// number: the property that matters is that an entry can exist at partial —
/// never full — confidence, and that the weakest entry admitted carries
/// exactly half weight (`ramp/2 / ramp = 0.5`). Stated as a literal, that
/// relationship held only by coincidence, and either constant could move
/// without the other noticing.
///
/// Applied to the model's OWN samples and, symmetrically, to its pooled
/// PEERS' — a win rate relative to a peer pool thinner than what we require of
/// the model itself would let a single near-fully-decayed peer record
/// (weight ~1e-8) hand out a maximal adjustment.
const MIN_WEIGHTED_DECISIVE_SAMPLES: f64 = CONFIDENCE_RAMP_SAMPLES / 2.0;

const _: () = {
    assert!(
        MIN_WEIGHTED_DECISIVE_SAMPLES > 0.0,
        "an entry needs some evidence behind it"
    );
    assert!(
        MIN_WEIGHTED_DECISIVE_SAMPLES < CONFIDENCE_RAMP_SAMPLES,
        "the weakest admitted entry must be at PARTIAL confidence — at or above \
         the ramp it would enter fully trusted on its first qualifying sample"
    );
};

/// Confidence-ramp sample count as an integer — the ONE authority both ramp
/// forms derive from: [`CONFIDENCE_RAMP_SAMPLES`] (the f64 the ramp divides
/// by) and `policy::RUNG_ADMISSION_MIN_CONFIDENCE_PERMILLE` (the admission
/// gate expressed as "the ramp minus one sample's worth of decay").
pub(super) const CONFIDENCE_RAMP_SAMPLE_COUNT: i16 = 8;

/// Confidence-ramp denominator: `min(weighted_samples, this) / this`. Mirrors
/// `outcome::CONFIDENT_DECISIVE_SAMPLES`'s shape (same "8 decisive reaches
/// full trust" sizing rationale) but is a SEPARATE constant — the learned-
/// specialty ramp and the plain feedback ramp are independent knobs even
/// though they currently share a value.
const CONFIDENCE_RAMP_SAMPLES: f64 = CONFIDENCE_RAMP_SAMPLE_COUNT as f64;

/// A verdict record (`signal == "verdict"`) counts double a bare run
/// completion — a judgement about the WORK is stronger evidence of routing
/// quality than "the process merely finished" (mirrors
/// `workflow_tools::engine::attribution::VerdictKind`'s own weighting
/// rationale, applied here at the aggregate level).
const VERDICT_WEIGHT_MULTIPLIER: f64 = 2.0;

/// Bound on the fully-learned (role, model) specialty adjustment — the value
/// `LearnedSpecialtyEntry::model_adjustment` is clamped to. Exceeds the
/// cold-start seed (`policy::cold_start_specialty_seed`'s `±60`) so real
/// per-role win-rate data can eventually dominate the seed once confidence
/// ramps to 1. This bounded blend stays under the tier gate (300) and the
/// capability gate (1000) under every stacking with the outcome-feedback
/// term — the SCORE never crosses either gate. Overturning a tier label
/// works through rung MEMBERSHIP instead: a twice-gated landslide
/// ([`LearnedSpecialtyEntry::earns_rung_admission`]) is admitted into the
/// next rung up at tier-score par (`policy::tier_match_bonus_for` with
/// [`super::policy::TiersProvenance::Learned`] provenance) and then competes
/// on this bounded blend like any member.
pub(super) const LEARNED_SPECIALTY_BOUND: i16 = 90;

/// One (role, model) pair's learned signal: a fully-scaled adjustment
/// (`±LEARNED_SPECIALTY_BOUND`, as if confidence were 1.0) plus a SEPARATE
/// confidence ramp (permille, i.e. `confidence_permille / 1000` = the `c` in
/// `seed × (1 − c) + learned × c`). Kept as plain `i16` fields (no `f32/f64`)
/// so [`LearnedSpecialtyHint`] — and everything that embeds it
/// (`RoutePolicyContext`) — stays `Eq`, not just `PartialEq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LearnedSpecialtyEntry {
    pub model_adjustment: i16,
    pub confidence_permille: i16,
}

impl LearnedSpecialtyEntry {
    /// Whether this entry's evidence earns the model candidacy in the next
    /// rung up: a near-full-ramp confidence AND an extreme POSITIVE win-rate
    /// lead. Directional by design — a losing streak demotes within the rung
    /// via the blend's sign, it never grants membership anywhere — so the
    /// `/smart doctor` marker and the admission waiver
    /// (`policy::learned_tier_exempt_from_selector`) read the SAME predicate
    /// and can never disagree about what is armed.
    #[must_use]
    pub fn earns_rung_admission(&self) -> bool {
        self.confidence_permille >= super::policy::RUNG_ADMISSION_MIN_CONFIDENCE_PERMILLE
            && self.model_adjustment >= super::policy::RUNG_ADMISSION_MIN_ADJUSTMENT
    }
}

/// Injected, plain-data learned-specialty hint (engine purity — see
/// `RoutePolicyContext::learned_specialty`'s doc). Empty/default is the
/// BYTE-IDENTICAL zero-data case: every `entry_for` lookup returns `None`, so
/// `policy::effective_specialty_adjustment` falls through to the cold-start
/// seed exactly as if this module did not exist.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LearnedSpecialtyHint {
    entries: Vec<(RouteRole, String, LearnedSpecialtyEntry)>,
}

impl LearnedSpecialtyHint {
    #[must_use]
    pub fn disabled() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Learned entry for an EXACT (role, canonical model id) pair, or `None`
    /// when no eligible history exists for it — the c=0 fallback case.
    #[must_use]
    pub fn entry_for(&self, role: RouteRole, model_id: &str) -> Option<LearnedSpecialtyEntry> {
        self.entries
            .iter()
            .find(|(entry_role, entry_model, _)| *entry_role == role && entry_model == model_id)
            .map(|(_, _, entry)| *entry)
    }

    /// Every computed `(role, canonical model id, entry)` triple, for
    /// observability surfaces (`/smart doctor`'s P7 learned-specialty table)
    /// that need to enumerate the whole learned table rather than look up one
    /// (role, model) pair. Routing itself only ever uses [`Self::entry_for`];
    /// this exists purely for read-only reporting.
    #[must_use]
    pub fn entries(&self) -> &[(RouteRole, String, LearnedSpecialtyEntry)] {
        &self.entries
    }

    /// Builder for tests/direct injection — mirrors
    /// [`super::policy::RouteFeedbackHint::with_model_adjustment`]'s pattern.
    /// Both fields are clamped to their documented ranges so a caller cannot
    /// construct an out-of-bound entry by hand.
    #[must_use]
    pub fn with_entry(
        mut self,
        role: RouteRole,
        model_id: impl Into<String>,
        model_adjustment: i16,
        confidence_permille: i16,
    ) -> Self {
        self.entries.push((
            role,
            model_id.into(),
            LearnedSpecialtyEntry {
                model_adjustment: model_adjustment.clamp(-LEARNED_SPECIALTY_BOUND, LEARNED_SPECIALTY_BOUND),
                confidence_permille: confidence_permille.clamp(0, 1000),
            },
        ));
        self
    }

    /// Compute the hint from raw outcome records. `now` (epoch seconds) and
    /// `canonicalize_model` are INJECTED — no hidden clock read, no `api`
    /// dependency — mirroring `outcome::weighted_feedback_hint_for_route_key`'s
    /// seam so the tools-layer caller (`smart_router::apply`) passes the SAME
    /// `canonicalize_route_model_id` closure it already uses for the plain
    /// feedback summary.
    #[must_use]
    pub fn compute(
        records: &[RouteOutcomeRecord],
        now: u64,
        canonicalize_model: impl Fn(&str) -> String,
    ) -> Self {
        let per_role_model = weighted_tallies(records, now, canonicalize_model);

        // Pass 2: group by role so each model's peers are its SAME-role
        // siblings only.
        let mut by_role: BTreeMap<RouteRole, Vec<(String, f64, f64)>> = BTreeMap::new();
        for ((role, model), (won, lost)) in per_role_model {
            by_role.entry(role).or_default().push((model, won, lost));
        }

        let mut entries = Vec::new();
        for (role, models) in by_role {
            for (model, won, lost) in &models {
                let own_weighted = won + lost;
                if own_weighted < MIN_WEIGHTED_DECISIVE_SAMPLES {
                    continue;
                }
                let (peer_won, peer_lost) = models
                    .iter()
                    .filter(|(other_model, _, _)| other_model != model)
                    .fold((0.0_f64, 0.0_f64), |(pw, pl), (_, w, l)| (pw + w, pl + l));
                let peer_total = peer_won + peer_lost;
                if peer_total < MIN_WEIGHTED_DECISIVE_SAMPLES {
                    // The peer pool must clear the SAME weighted floor as the
                    // model's own history — a relative score is meaningless
                    // with (next to) nothing to be relative TO.
                    continue;
                }
                let own_rate = won / own_weighted;
                let peer_rate = peer_won / peer_total;
                let relative = (own_rate - peer_rate).clamp(-1.0, 1.0);
                let model_adjustment =
                    round_f64_to_i16(relative * f64::from(LEARNED_SPECIALTY_BOUND));
                let ramp = (own_weighted.min(CONFIDENCE_RAMP_SAMPLES) / CONFIDENCE_RAMP_SAMPLES)
                    .clamp(0.0, 1.0);
                let confidence_permille = round_f64_to_i16(ramp * 1000.0);
                entries.push((
                    role,
                    model.clone(),
                    LearnedSpecialtyEntry { model_adjustment, confidence_permille },
                ));
            }
        }
        Self { entries }
    }
}

/// Pass 1 of the learned signal: weighted (win, loss) sums per (role,
/// canonical model), pooled across every `route_key` that shares a role —
/// learned specialty is a ROLE-level signal (same-role peers), not scoped to
/// one subagent target the way the plain feedback hint is. One sample per
/// attributed attempt, never a fold — the same learning samples every other
/// learner reads (`learning_samples`).
///
/// Shared by [`LearnedSpecialtyHint::compute`] and [`learned_rate`], so the
/// rate the challenger arm holds a newcomer to is the incumbent's record as
/// the router itself weighs it — the same decay, the same verdict weight,
/// the same pin exclusion — and never a second reading of the same rows.
fn weighted_tallies(
    records: &[RouteOutcomeRecord],
    now: u64,
    canonicalize_model: impl Fn(&str) -> String,
) -> BTreeMap<(RouteRole, String), (f64, f64)> {
    let mut per_role_model: BTreeMap<(RouteRole, String), (f64, f64)> = BTreeMap::new();
    for record in learning_samples(records) {
        if is_pin_availability_noise(&record) {
            continue;
        }
        let Some(role) = effective_role_for_record(&record) else {
            continue;
        };
        let Some(win) =
            decisive_outcome(record.status.as_str(), record.provider_error_class.as_deref())
        else {
            continue;
        };
        let weight = record_weight(&record, now);
        let model = canonicalize_model(&record.selected_model);
        let entry = per_role_model.entry((role, model)).or_insert((0.0, 0.0));
        if win {
            entry.0 += weight;
        } else {
            entry.1 += weight;
        }
    }
    per_role_model
}

/// The weighted win rate of `model_id` for `role` — the incumbent's own
/// record, as the learner weighs it — or `None` while the pair has fewer
/// weighted decisive samples than the learner itself requires before it
/// says anything (`MIN_WEIGHTED_DECISIVE_SAMPLES`).
///
/// This is the line a challenger's standing must clear before its evidence
/// may move the role's model (`zerocode_core::jev::challenger::Standing::passes`):
/// read here, from the same tallies and the same floor as the specialty
/// hint, because a bar spelled by another reader would be another bar. A
/// pair under the floor has no rate, and a challenger is held to nothing it
/// cannot be measured against.
#[must_use]
pub fn learned_rate(
    records: &[RouteOutcomeRecord],
    now: u64,
    role: RouteRole,
    model_id: &str,
    canonicalize_model: impl Fn(&str) -> String,
) -> Option<f64> {
    let model = canonicalize_model(model_id);
    let (won, lost) = weighted_tallies(records, now, canonicalize_model).remove(&(role, model))?;
    let own_weighted = won + lost;
    (own_weighted >= MIN_WEIGHTED_DECISIVE_SAMPLES).then(|| won / own_weighted)
}

/// `records` with `routeSource == "pin"` measure AVAILABILITY (a config
/// override was reachable and ran), not the AUTO selector's judgement of
/// which model best fits a role — the live-data audit's largest bucket (195+
/// samples) was exactly this: a past pin's residue, not a quality signal.
/// Excluded entirely (weight 0) from the learned-specialty aggregate, in
/// BOTH the model's own count and its peers' — EXCEPT a verdict record,
/// which keeps full weight regardless of `routeSource`: a judgement is about
/// the WORK itself, independent of how the model doing the work got picked.
fn is_pin_availability_noise(record: &RouteOutcomeRecord) -> bool {
    record.route_source.as_deref() == Some("pin") && record.signal.as_deref() != Some("verdict")
}

/// Combined sample weight for one decisive record: P3 recency half-life
/// (reused verbatim from [`recency_weight`]) × a verdict multiplier ×the
/// record's own `signalWeight` (e.g. a council/preference verdict already
/// down-weighted to 0.5 by `attribution::VerdictKind::Preference`).
/// `signal_weight` absent (the common run-completion case) multiplies by 1.0
/// — a no-op.
fn record_weight(record: &RouteOutcomeRecord, now: u64) -> f64 {
    let mut weight = recency_weight(record.recorded_at, now);
    if record.signal.as_deref() == Some("verdict") {
        weight *= VERDICT_WEIGHT_MULTIPLIER;
    }
    weight *= f64::from(record.signal_weight.unwrap_or(1.0));
    weight
}

/// A record's role for learned-specialty attribution, in priority order:
///
/// 1. The v2 `role` field (`RouteOutcomeRecord::role`), parsed via
///    [`RouteRole::from_key`] — stamped by every P3/P4 recorder whenever the
///    smart router actually computed a route decision for this record (spawn
///    completions with a genuine override, and verdict attribution reading
///    the SAME manifest field back). This covers the overwhelming majority
///    of NEW-schema records, so no fallback mapping is needed for them.
/// 2. For a `"subagent"`-kind record still missing `role` (a pre-v2 record,
///    or a spawn whose route resolved to the parent model with no fallback
///    candidates to smuggle either — see `apply::smart_model_for_fields`'s
///    early-return conditions) — map the record's `target` (the resolved
///    subagent type) through [`SubagentProfileId::parse`] +
///    `route_role_hint()`, which ALREADY exists in this crate for exactly
///    this purpose (no new `subagent_type`→role table to invent or duplicate).
///    This recovers every BUILTIN subagent profile (which the live-data
///    audit showed dominates real usage: `code-reviewer`, `debugger`,
///    `Explore`, `Plan`, `Verification`, `deep-research`, …) — a `custom:*`
///    subagent type has no such mapping and returns `None` here, same as a
///    genuinely unknown one.
///
/// Everything else — most notably `"main"`-kind records (the ad-hoc-turn and
/// deep-gate-VERIFY verdict sources judge the MAIN turn, not any one
/// specialist role) and `"deep-verify"`-kind leg records — has no sound role
/// mapping at all: a main turn is not itself a `RouteRole`, and inventing one
/// would misattribute a whole-turn judgement to a single specialist's
/// win-rate. Such records are simply excluded from this role-scoped signal
/// (`None`), which is the deliberately SIMPLER of the two sound options this
/// phase's investigation surfaced — versus building a second, separate
/// `subagent_type`→role table that would only duplicate
/// `target::SubagentProfileId::route_role_hint` for a small minority of
/// historical rows.
fn effective_role_for_record(record: &RouteOutcomeRecord) -> Option<RouteRole> {
    if let Some(role) = record.role.as_deref().and_then(RouteRole::from_key) {
        return Some(role);
    }
    if record.target_kind == "subagent" {
        return SubagentProfileId::parse(&record.target).and_then(|profile| profile.route_role_hint());
    }
    None
}

/// `value` is bounded by construction at every call site (a `relative` ratio
/// in `[-1, 1]` scaled by `LEARNED_SPECIALTY_BOUND`, or a ramp in `[0, 1]`
/// scaled by 1000) — safely within `i16` range either way.
#[allow(clippy::cast_possible_truncation)]
fn round_f64_to_i16(value: f64) -> i16 {
    value.round() as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn record(
        route_key_target: &str,
        model: &str,
        status: &str,
        role: Option<&str>,
        route_source: Option<&str>,
        signal: Option<&str>,
        signal_weight: Option<f32>,
        recorded_at: u64,
    ) -> RouteOutcomeRecord {
        let mut rec = RouteOutcomeRecord::new("subagent", route_key_target, model, status)
            .with_role(role.map(str::to_string))
            .with_route_source(route_source.map(str::to_string))
            .with_signal_weight(signal_weight);
        if let Some(signal) = signal {
            rec = rec.with_signal(signal);
        }
        rec.recorded_at = recorded_at;
        rec
    }

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn zero_records_produce_an_empty_hint() {
        let hint = LearnedSpecialtyHint::compute(&[], NOW, ToString::to_string);
        assert!(hint.is_empty());
        assert_eq!(hint.entry_for(RouteRole::Coding, "any-model"), None);
    }

    #[test]
    fn below_eligibility_floor_contributes_no_entry() {
        // 3 weighted decisive samples for model-a (< the 4.0 floor) plus a
        // peer with plenty of history — model-a must NOT get an entry yet.
        let records = vec![
            record("code-reviewer", "model-a", "completed", Some("reviewer"), Some("auto"), None, None, NOW),
            record("code-reviewer", "model-a", "completed", Some("reviewer"), Some("auto"), None, None, NOW),
            record("code-reviewer", "model-a", "completed", Some("reviewer"), Some("auto"), None, None, NOW),
            record("code-reviewer", "model-b", "completed", Some("reviewer"), Some("auto"), None, None, NOW),
            record("code-reviewer", "model-b", "failed", Some("reviewer"), Some("auto"), None, None, NOW),
            record("code-reviewer", "model-b", "completed", Some("reviewer"), Some("auto"), None, None, NOW),
            record("code-reviewer", "model-b", "completed", Some("reviewer"), Some("auto"), None, None, NOW),
        ];
        let hint = LearnedSpecialtyHint::compute(&records, NOW, ToString::to_string);
        assert_eq!(hint.entry_for(RouteRole::Reviewer, "model-a"), None, "under the 4.0 weighted floor");
    }

    #[test]
    fn eligible_pair_wins_against_a_weaker_peer() {
        // model-a: 5-0 (all wins); model-b: 1-4 (mostly losses) — same role,
        // pooled peers. model-a's win rate (1.0) exceeds the peer rate
        // (model-b alone: 0.2), so model-a gets a strongly POSITIVE entry;
        // model-b (peer = model-a alone, rate 1.0) gets a strongly NEGATIVE
        // one — both fully confident (5 raw decisive >= the 8-sample ramp
        // cap only partially, so confidence < 1000 but > 500).
        let mut records = Vec::new();
        for _ in 0..5 {
            records.push(record("coder", "model-a", "completed", Some("coding"), Some("auto"), None, None, NOW));
        }
        records.push(record("coder", "model-b", "completed", Some("coding"), Some("auto"), None, None, NOW));
        for _ in 0..4 {
            records.push(record("coder", "model-b", "failed", Some("coding"), Some("auto"), None, None, NOW));
        }

        let hint = LearnedSpecialtyHint::compute(&records, NOW, ToString::to_string);
        let a = hint.entry_for(RouteRole::Coding, "model-a").expect("model-a eligible");
        let b = hint.entry_for(RouteRole::Coding, "model-b").expect("model-b eligible");
        assert!(a.model_adjustment > 0, "model-a beat its only peer: {a:?}");
        assert!(b.model_adjustment < 0, "model-b lost to its only peer: {b:?}");
        // own_rate 1.0 vs peer_rate 0.2 -> relative 0.8 -> round(0.8 * 90) = 72
        // (own_rate 0.2 vs peer_rate 1.0 -> relative -0.8 -> -72 for model-b).
        assert_eq!(a.model_adjustment, 72);
        assert_eq!(b.model_adjustment, -72);
        // 5 weighted decisive samples: ramp = 5/8 = 625 permille.
        assert_eq!(a.confidence_permille, 625);
        assert_eq!(b.confidence_permille, 625);
    }

    #[test]
    fn pin_run_records_are_excluded_but_pin_verdicts_keep_full_weight() {
        // 6 "pin" run-completions for model-a would otherwise swamp the
        // aggregate with an availability signal, not a quality one — they
        // must contribute NOTHING. A single verdict record with
        // routeSource=pin, by contrast, must count at full (2x-verdict)
        // weight despite also being a pin.
        let mut records = Vec::new();
        for _ in 0..6 {
            records.push(record("coder", "model-a", "completed", Some("coding"), Some("pin"), None, None, NOW));
        }
        // Peer model-b has a mixed (50/50) plain-auto history to compare
        // against, so a fresh full-weight win for model-a can score above it.
        for _ in 0..2 {
            records.push(record("coder", "model-b", "completed", Some("coding"), Some("auto"), None, None, NOW));
        }
        for _ in 0..2 {
            records.push(record("coder", "model-b", "failed", Some("coding"), Some("auto"), None, None, NOW));
        }
        let hint = LearnedSpecialtyHint::compute(&records, NOW, ToString::to_string);
        assert_eq!(
            hint.entry_for(RouteRole::Coding, "model-a"),
            None,
            "pin run records must not accumulate any weighted samples"
        );

        // Now give model-a 2 pin-sourced VERDICT wins (2x each = 4.0 weighted
        // — clears the eligibility floor on verdicts alone).
        records.push(record("coder", "model-a", "completed", Some("coding"), Some("pin"), Some("verdict"), Some(1.0), NOW));
        records.push(record("coder", "model-a", "completed", Some("coding"), Some("pin"), Some("verdict"), Some(1.0), NOW));
        let hint_with_verdicts = LearnedSpecialtyHint::compute(&records, NOW, ToString::to_string);
        let a = hint_with_verdicts
            .entry_for(RouteRole::Coding, "model-a")
            .expect("pin-sourced verdicts must count at full weight");
        assert!(a.model_adjustment > 0);
    }

    #[test]
    fn verdict_records_count_double_a_plain_run() {
        // 2 verdict completions (weight 2.0 each = 4.0 weighted) reach the
        // SAME eligibility/confidence as 4 plain run completions (weight 1.0
        // each) would.
        let verdict_records = vec![
            record("Plan", "model-a", "completed", Some("analysis"), Some("auto"), Some("verdict"), Some(1.0), NOW),
            record("Plan", "model-a", "completed", Some("analysis"), Some("auto"), Some("verdict"), Some(1.0), NOW),
            record("Plan", "model-b", "failed", Some("analysis"), Some("auto"), Some("verdict"), Some(1.0), NOW),
            record("Plan", "model-b", "failed", Some("analysis"), Some("auto"), Some("verdict"), Some(1.0), NOW),
        ];
        let plain_records = vec![
            record("Plan", "model-a", "completed", Some("analysis"), Some("auto"), None, None, NOW),
            record("Plan", "model-a", "completed", Some("analysis"), Some("auto"), None, None, NOW),
            record("Plan", "model-a", "completed", Some("analysis"), Some("auto"), None, None, NOW),
            record("Plan", "model-a", "completed", Some("analysis"), Some("auto"), None, None, NOW),
            record("Plan", "model-b", "failed", Some("analysis"), Some("auto"), None, None, NOW),
            record("Plan", "model-b", "failed", Some("analysis"), Some("auto"), None, None, NOW),
            record("Plan", "model-b", "failed", Some("analysis"), Some("auto"), None, None, NOW),
            record("Plan", "model-b", "failed", Some("analysis"), Some("auto"), None, None, NOW),
        ];

        let from_verdicts = LearnedSpecialtyHint::compute(&verdict_records, NOW, ToString::to_string);
        let from_plain = LearnedSpecialtyHint::compute(&plain_records, NOW, ToString::to_string);
        assert_eq!(
            from_verdicts.entry_for(RouteRole::Analysis, "model-a"),
            from_plain.entry_for(RouteRole::Analysis, "model-a"),
            "2 verdicts (2x weight) must equal 4 plain runs"
        );
    }

    #[test]
    fn half_life_decay_reduces_confidence_of_stale_only_evidence() {
        let thirty_days_ago = NOW - 30 * 86_400;
        let mut fresh = Vec::new();
        let mut stale = Vec::new();
        for _ in 0..8 {
            fresh.push(record("coder", "model-a", "completed", Some("coding"), Some("auto"), None, None, NOW));
            fresh.push(record("coder", "model-b", "failed", Some("coding"), Some("auto"), None, None, NOW));
            stale.push(record("coder", "model-a", "completed", Some("coding"), Some("auto"), None, None, thirty_days_ago));
            stale.push(record("coder", "model-b", "failed", Some("coding"), Some("auto"), None, None, thirty_days_ago));
        }
        let fresh_hint = LearnedSpecialtyHint::compute(&fresh, NOW, ToString::to_string);
        let stale_hint = LearnedSpecialtyHint::compute(&stale, NOW, ToString::to_string);
        let fresh_entry = fresh_hint.entry_for(RouteRole::Coding, "model-a").expect("fresh eligible");
        assert_eq!(fresh_entry.confidence_permille, 1000, "8 fresh decisive reaches full confidence");

        match stale_hint.entry_for(RouteRole::Coding, "model-a") {
            None => {} // decayed under the eligibility floor entirely — decay "worked".
            Some(stale_entry) => assert!(
                stale_entry.confidence_permille < fresh_entry.confidence_permille,
                "30-day-old-only evidence must not reach the same confidence as fresh evidence"
            ),
        }
    }

    #[test]
    fn role_falls_back_to_the_builtin_subagent_type_mapping_when_the_v2_field_is_absent() {
        // Pre-v2 (or role-less) record: no `role` field, but `target` is the
        // builtin `code-reviewer` profile key, which maps to Reviewer via
        // `SubagentProfileId::route_role_hint` — no new table invented.
        let mut records = Vec::new();
        for _ in 0..5 {
            records.push(record("code-reviewer", "model-a", "completed", None, Some("auto"), None, None, NOW));
        }
        for _ in 0..5 {
            records.push(record("code-reviewer", "model-b", "failed", None, Some("auto"), None, None, NOW));
        }
        let hint = LearnedSpecialtyHint::compute(&records, NOW, ToString::to_string);
        assert!(
            hint.entry_for(RouteRole::Reviewer, "model-a").is_some(),
            "builtin subagent target must map to Reviewer without an explicit role field"
        );
    }

    #[test]
    fn unmappable_target_kind_is_excluded_from_the_role_scoped_signal() {
        // "main"/"turn" records (the ad-hoc and deep-gate verdict sources)
        // have no role field and no sound role mapping — must be excluded,
        // not misattributed to some guessed role.
        let mut records = Vec::new();
        for _ in 0..10 {
            records.push(
                RouteOutcomeRecord::new("main", "turn", "model-a", "completed")
                    .with_signal("verdict")
                    .with_signal_weight(Some(1.0)),
            );
        }
        let hint = LearnedSpecialtyHint::compute(&records, NOW, ToString::to_string);
        assert!(hint.is_empty(), "main-turn records carry no learnable role signal");
    }

    #[test]
    fn no_peer_history_yields_no_entry_even_with_ample_self_history() {
        // model-a alone has 10 decisive samples but no same-role peer at
        // all — nothing to be "relative to", so no entry either.
        let mut records = Vec::new();
        for _ in 0..10 {
            records.push(record("coder", "model-a", "completed", Some("coding"), Some("auto"), None, None, NOW));
        }
        let hint = LearnedSpecialtyHint::compute(&records, NOW, ToString::to_string);
        assert_eq!(hint.entry_for(RouteRole::Coding, "model-a"), None);
    }

    #[test]
    fn a_nearly_decayed_peer_pool_yields_no_entry() {
        // Regression: the peer floor used to be `> 0.0`, so a SINGLE peer
        // failure from a year ago (recency weight ~1e-8, its own entry long
        // gone) made peer_rate exactly 0.0 and handed the winner a maximal
        // +90 adjustment on no real comparison at all — and since the ratio
        // is decay-invariant, that verdict would then freeze forever. The
        // peer pool must clear the SAME >=4.0 weighted floor as the model's
        // own history.
        let a_year_ago = NOW - 365 * 86_400;
        let mut records = Vec::new();
        for _ in 0..8 {
            records.push(record("coder", "model-a", "completed", Some("coding"), Some("auto"), None, None, NOW));
        }
        records.push(record("coder", "model-b", "failed", Some("coding"), Some("auto"), None, None, a_year_ago));
        let hint = LearnedSpecialtyHint::compute(&records, NOW, ToString::to_string);
        assert_eq!(
            hint.entry_for(RouteRole::Coding, "model-a"),
            None,
            "a near-zero-weight peer pool is not a comparison"
        );
    }

    #[test]
    fn canonicalizer_merges_historical_model_id_fragments() {
        let records = vec![
            record("coder", "claude-opus-4-8", "completed", Some("coding"), Some("auto"), None, None, NOW),
            record("coder", "claude-opus-4.8", "completed", Some("coding"), Some("auto"), None, None, NOW),
            record("coder", "claude-opus-4.8", "completed", Some("coding"), Some("auto"), None, None, NOW),
            record("coder", "claude-opus-4.8", "completed", Some("coding"), Some("auto"), None, None, NOW),
            record("coder", "model-b", "failed", Some("coding"), Some("auto"), None, None, NOW),
            record("coder", "model-b", "failed", Some("coding"), Some("auto"), None, None, NOW),
            record("coder", "model-b", "failed", Some("coding"), Some("auto"), None, None, NOW),
            record("coder", "model-b", "failed", Some("coding"), Some("auto"), None, None, NOW),
        ];
        let merge_dot_to_dash = |model: &str| {
            if model == "claude-opus-4.8" { "claude-opus-4-8".to_string() } else { model.to_string() }
        };
        let hint = LearnedSpecialtyHint::compute(&records, NOW, merge_dot_to_dash);
        let merged = hint.entry_for(RouteRole::Coding, "claude-opus-4-8").expect("merged bucket eligible");
        // Expressed from the constants, not as `500`: the property is that the
        // weakest admissible entry sits at exactly half the ramp, which is why
        // the sample floor is DERIVED from the ramp. A literal here would keep
        // passing if either constant moved on its own and quietly broke it.
        let floor_ratio = MIN_WEIGHTED_DECISIVE_SAMPLES / CONFIDENCE_RAMP_SAMPLES;
        assert!(
            (f64::from(merged.confidence_permille) / 1000.0 - floor_ratio).abs() < f64::EPSILON,
            "a bucket at exactly the sample floor carries the floor's confidence: \
             {} permille vs ratio {floor_ratio}",
            merged.confidence_permille
        );
        assert!(
            (floor_ratio - 0.5).abs() < f64::EPSILON,
            "and the floor is half the ramp — the shape the derivation exists to hold"
        );
    }

    /// model-a finishes every attempt and fails verification on every one.
    /// Per receipt that read as 4 wins beside 8 weighted losses (rate 1/3,
    /// −60 against a perfect peer); per attempt only the verdicts speak —
    /// rate 0, the full negative `LEARNED_SPECIALTY_BOUND`.
    #[test]
    fn a_completion_whose_work_failed_verification_earns_no_win() {
        let mut records = Vec::new();
        for attempt in 0..4 {
            let agent = format!("agent-{attempt}");
            records.push(
                record("coder", "model-a", "completed", Some("coding"), Some("auto"), None, None, NOW)
                    .with_attempt(&agent, 1),
            );
            records.push(
                record("coder", "model-a", "failed", Some("coding"), Some("auto"), Some("verdict"), Some(1.0), NOW)
                    .with_decision(super::super::outcome::DecisionKind::Verify)
                    .with_attempt(&agent, 1),
            );
        }
        for _ in 0..4 {
            records.push(record("coder", "model-b", "completed", Some("coding"), Some("auto"), None, None, NOW));
        }

        let hint = LearnedSpecialtyHint::compute(&records, NOW, ToString::to_string);
        let a = hint.entry_for(RouteRole::Coding, "model-a").expect("model-a eligible");
        assert_eq!(a.model_adjustment, -LEARNED_SPECIALTY_BOUND, "{a:?}");
    }

    #[test]
    fn with_entry_clamps_out_of_bound_inputs() {
        let hint = LearnedSpecialtyHint::default().with_entry(RouteRole::Coding, "model-a", 500, 5000);
        let entry = hint.entry_for(RouteRole::Coding, "model-a").expect("entry present");
        assert_eq!(entry.model_adjustment, LEARNED_SPECIALTY_BOUND);
        assert_eq!(entry.confidence_permille, 1000);
    }
}
