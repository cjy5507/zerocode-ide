use std::collections::BTreeMap;
use std::path::Path;

use runtime::{PlanCandidate, PlanCohort, PlanContext, PlanShape, ScoredPlan, VerifyMode};
use serde::{Deserialize, Serialize};

use super::plan_shadow::{PlanShadowActual, PlanShadowInputs};
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};

pub const PLAN_RECEIPTS_FILE: &str = "plan-receipts.jsonl";
const MIN_SAMPLES: usize = 12;
const MAX_AGE_MS: u64 = 28 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanRunReceipt {
    pub version: u8,
    pub at_ms: u64,
    pub attempt: String,
    pub cohort: PlanCohort,
    pub plan: PlanShadowActual,
    pub verified: Option<bool>,
    pub duration_ms: u64,
    pub total_tokens: Option<u64>,
    pub total_usd: Option<f64>,
    pub held: Vec<String>,
}

impl PlanRunReceipt {
    fn matches(&self, candidate: &PlanCandidate, cohort: &PlanCohort, now_ms: u64) -> bool {
        self.version == 1 && !self.attempt.trim().is_empty()
            && self.cohort == *cohort && self.held.is_empty()
            && self.at_ms <= now_ms && now_ms - self.at_ms <= MAX_AGE_MS
            && self.plan.model == candidate.model && self.plan.effort == candidate.effort
            && self.plan.shape == candidate.shape.label()
            && self.plan.verify == candidate.verify.as_str()
            && self.verified.is_some() && self.duration_ms > 0
            && self.total_tokens.is_some_and(|tokens| tokens > 0)
            && self.total_usd.is_some_and(|cost| cost.is_finite() && cost > 0.0)
    }
}

pub fn record_plan_receipt(cwd: &Path, receipt: &PlanRunReceipt) -> std::io::Result<()> {
    append_shadow_row(&shadow_ledger_path(cwd, PLAN_RECEIPTS_FILE), receipt, SHADOW_LEDGER_MAX_BYTES)
}

pub fn read_plan_receipts(cwd: &Path) -> std::io::Result<Vec<PlanRunReceipt>> {
    let file = shadow_ledger_path(cwd, PLAN_RECEIPTS_FILE);
    let Some(contents) = runtime::secure_fs::read_regular_file_absolute_no_follow_bounded(
        &file, SHADOW_LEDGER_MAX_BYTES * 2,
    )? else { return Ok(Vec::new()); };
    contents.lines().filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(std::io::Error::other)).collect()
}

#[must_use]
pub fn choose_measured_plan(inputs: &PlanShadowInputs<'_>, receipts: &[PlanRunReceipt]) -> Option<PlanCandidate> {
    let cohort = inputs.ctx.cohort?;
    if inputs.trigger.is_some() || !inputs.ctx.objective_check_available { return None; }
    let now_ms = inputs.recorded_at.checked_mul(1_000)?;
    let mut unique = BTreeMap::new();
    for receipt in receipts {
        match unique.entry(receipt.attempt.as_str()) {
            std::collections::btree_map::Entry::Vacant(entry) => { entry.insert(Some(receipt)); }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if entry.get().is_some_and(|previous| previous != receipt) { entry.insert(None); }
            }
        }
    }
    let mut scored = Vec::new();
    for candidate in runtime::plan_candidates(&inputs.ctx, inputs.models) {
        if candidate.verify != VerifyMode::Objective || candidate.shape != PlanShape::Solo { continue; }
        let sample: Vec<_> = unique.values().filter_map(|receipt| *receipt)
            .filter(|receipt| receipt.matches(&candidate, cohort, now_ms)).collect();
        if sample.len() < MIN_SAMPLES { continue; }
        let passes = sample.iter().filter(|receipt| receipt.verified == Some(true)).count();
        let pass_rate = f64::from(u32::try_from(passes).ok()?) / f64::from(u32::try_from(sample.len()).ok()?);
        if pass_rate < inputs.ctx.min_p_verified { continue; }
        let mut estimate = runtime::score_plan(
            &candidate, &inputs.ctx, inputs.priors,
            |model| inputs.models.iter().find(|option| option.id == model)
                .map_or(runtime::ModelBand::Rest, |option| option.band),
            |_| None, inputs.price, inputs.cache,
        );
        let total_usd = sample.iter().try_fold(0.0, |total, receipt| Some(total + receipt.total_usd?))?;
        let total_tokens = sample.iter().try_fold(0_u64, |total, receipt| total.checked_add(receipt.total_tokens?))?;
        let total_ms = sample.iter().try_fold(0_u64, |total, receipt| total.checked_add(receipt.duration_ms))?;
        let count = u64::try_from(sample.len()).ok()?;
        let observed_usd = total_usd / f64::from(u32::try_from(count).ok()?);
        if !observed_usd.is_finite() { continue; }
        let switch = if candidate.model == inputs.ctx.current_model && candidate.effort.as_deref() == inputs.ctx.current_effort {
            runtime::CostTerm::default()
        } else { estimate.cost.switch };
        estimate.cost = runtime::CostBreakdown {
            work: runtime::CostTerm { tokens: total_tokens / count, usd: Some(observed_usd) },
            switch,
            ..runtime::CostBreakdown::default()
        };
        estimate.p_verified = pass_rate;
        estimate.confidence = 1.0;
        estimate.t_verified_ms = total_ms / count;
        estimate.interventions = 0.0;
        scored.push(ScoredPlan { candidate, estimate });
    }
    let current = scored.iter().find(|plan| plan.candidate.model == inputs.ctx.current_model
        && plan.candidate.effort.as_deref() == inputs.ctx.current_effort)?;
    let chosen = runtime::choose_plan(&scored, &inputs.ctx)?;
    let candidate = &scored[chosen.index];
    if candidate.candidate == current.candidate { return None; }
    let margin = inputs.ctx.switch_margin.clamp(0.0, 1.0);
    (candidate.estimate.cost_usd()? < current.estimate.cost_usd()? * (1.0 - margin))
        .then(|| candidate.candidate.clone())
}

#[must_use]
pub fn plan_pins_allow(ctx: &PlanContext<'_>, candidate: &PlanCandidate) -> bool {
    ctx.pinned_model.is_none_or(|model| model == candidate.model)
        && ctx.pinned_effort.is_none_or(|effort| candidate.effort.as_deref() == Some(effort))
        && candidate.shape == PlanShape::Solo && candidate.verify == VerifyMode::Objective
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cohort() -> PlanCohort {
        PlanCohort::new("main", "turn", runtime::RouteRole::Coding,
            runtime::RouteTaskComplexity::Medium, runtime::RouteTaskRisk::Low).unwrap()
    }

    fn receipts() -> Vec<PlanRunReceipt> {
        ["current", "cheaper"].into_iter().flat_map(|model| (0..12).map(move |index| PlanRunReceipt {
            version: 1, at_ms: 900_000, attempt: format!("{model}@{index}"), cohort: cohort(),
            plan: PlanShadowActual { model: model.into(), effort: Some("high".into()), shape: "solo".into(), verify: "objective".into() },
            verified: Some(true), duration_ms: 10_000, total_tokens: Some(5_000),
            total_usd: Some(if model == "current" { 1.0 } else { 0.2 }), held: Vec::new(),
        })).collect()
    }

    fn select(receipts: &[PlanRunReceipt], pin: Option<&str>) -> Option<PlanCandidate> {
        let cohort = cohort();
        let models: Vec<_> = ["current", "cheaper"].into_iter().map(|model| runtime::ModelOption {
            id: model.into(), band: runtime::ModelBand::Top, efforts: vec!["high".into()],
        }).collect();
        let priors = super::super::plan_shadow::plan_priors_for(runtime::RouteTaskComplexity::Medium).unwrap();
        let ctx = PlanContext {
            cohort: Some(&cohort), complexity: runtime::RouteTaskComplexity::Medium,
            current_model: "current", current_effort: Some("high"), context_tokens: 100,
            pinned_model: pin, pinned_effort: Some("high"), attended: true,
            objective_check_available: true, independent_slices: None, max_lanes: 1,
            verify_ceiling: 1, min_p_verified: 0.9, switch_margin: 0.1,
        };
        let price = |_: &str| Some(runtime::ModelPrice { input: 1.0, cache_read: 0.1, cache_write: 1.0, output: 1.0 });
        choose_measured_plan(&PlanShadowInputs {
            session_id: "session", attempt: Some("session@1"), recorded_at: 1_000,
            ctx, models: &models, priors: &priors, records: &[], price: &price, cache: &[],
            actual: receipts[0].plan.clone(), trigger: None, category: None,
        }, receipts)
    }

    #[test]
    fn measured_verified_cohort_can_choose_then_demote_and_recover() {
        let mut samples = receipts();
        assert_eq!(select(&samples, None).unwrap().model, "cheaper");
        for receipt in samples.iter_mut().filter(|row| row.plan.model == "cheaper").take(2) {
            receipt.verified = Some(false);
        }
        assert!(select(&samples, None).is_none());
        assert_eq!(select(&receipts(), None).unwrap().model, "cheaper");
    }

    #[test]
    fn pins_missing_costs_and_different_cohorts_never_authorize_a_switch() {
        assert!(select(&receipts(), Some("current")).is_none());
        let mutations: [fn(&mut PlanRunReceipt); 6] = [
            |row: &mut PlanRunReceipt| row.total_usd = None,
            |row: &mut PlanRunReceipt| row.cohort.risk = "high".into(),
            |row: &mut PlanRunReceipt| row.plan.effort = None,
            |row: &mut PlanRunReceipt| row.plan.verify = "model-judge".into(),
            |row: &mut PlanRunReceipt| row.held.push("incomplete".into()),
            |row: &mut PlanRunReceipt| row.at_ms = 1_100_000,
        ];
        for mutate in mutations {
            let mut samples = receipts();
            samples.iter_mut().filter(|row| row.plan.model == "cheaper").for_each(mutate);
            assert!(select(&samples, None).is_none());
        }
    }

    #[test]
    fn repeated_or_conflicting_attempts_cannot_inflate_evidence() {
        let samples = receipts();
        let mut repeated: Vec<_> = samples.iter().filter(|row| row.plan.model == "current").cloned().collect();
        repeated.extend(std::iter::repeat_n(samples[12].clone(), 12));
        assert!(select(&repeated, None).is_none());
        let mut conflicting = samples.clone();
        let mut changed = samples[12].clone();
        changed.verified = Some(false);
        conflicting.push(changed);
        assert!(select(&conflicting, None).is_none());
    }
}
