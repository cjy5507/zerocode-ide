//! The plan scorer in shadow — the tools-layer half of `runtime::plan`.
//!
//! At a turn's start the host already knows the difficulty, the current model
//! and effort, who is watching and which models are connected. This module
//! turns those into the scorer's inputs (catalog priors, model options with
//! the efforts the turn's band allows, the verified record, the warm prefixes),
//! prices every `(model, effort, shape, verify)` candidate, and writes what
//! the scorer WOULD have chosen next to what the turn actually did — to
//! `state/smart-router/plan-shadow.jsonl`, never to the learning store.
//! Nothing here changes a decision: the shadow ledger is the evidence a later
//! phase reads before the scorer is allowed to decide
//! (`docs/design/zo-autonomous-routing-review-20260915.md` §5 P1).

use std::io;
use std::path::{Path, PathBuf};

use runtime::{
    choose_plan, plan_candidates, plan_evidence_from_records, score_plan, ModelBand, ModelOption,
    ModelPrice, PlanCacheState, PlanCandidate, PlanContext, PlanPriors, RouteOutcomeRecord,
    RouteTaskComplexity, ScoredPlan, ModelInventory, SwitchTrigger,
};
use serde::{Deserialize, Serialize};

use super::canonical::canonicalize_route_model_id;
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};

/// The plan shadow's ledger file, under the shared shadow-ledger directory.
pub const PLAN_SHADOW_FILE: &str = "plan-shadow.jsonl";
/// How many scored candidates a row keeps in full: the ranked head, plus the
/// best same-model plan when it did not make the head. The rest is a count.
const PLAN_SHADOW_HEAD: usize = 8;

/// The catalog's plan priors for a complexity, as the scorer's input. `None`
/// when the catalog lists no work turn for the label — the scorer then does
/// not run, rather than run on a number nobody wrote down.
#[must_use]
pub fn plan_priors_for(complexity: RouteTaskComplexity) -> Option<PlanPriors> {
    let table = api::plan_priors();
    let turn = table.work_turn(complexity.as_label())?;
    if turn.output_tokens == 0 || turn.requests == 0 || turn.duration_ms == 0 {
        return None;
    }
    Some(PlanPriors {
        pass_top: percent(table.pass_percent_top),
        pass_second: percent(table.pass_percent_second),
        pass_rest: percent(table.pass_percent_rest),
        output_tokens: turn.output_tokens,
        requests: turn.requests,
        duration_ms: turn.duration_ms,
        judge_output_tokens: table.judge_output_tokens,
        judge_duration_ms: table.judge_duration_ms,
        objective_check_ms: table.objective_check_ms,
        classify_output_tokens: table.classify_output_tokens,
        classify_ms: table.classify_ms,
        lane_brief_tokens: table.lane_brief_tokens,
        handover_tokens: table.handover_tokens,
    })
}

fn percent(value: u8) -> f64 {
    f64::from(value.min(100)) / 100.0
}

/// The models the scorer may choose, read off the connected inventory: every
/// model that stands in a band (a superseded release serves nothing), with
/// the efforts the turn's band — `floor..=ceiling` on the one ladder — allows
/// and the model accepts. A model outside the band at every rung carries no
/// effort and runs at its default.
#[must_use]
pub fn model_options_for(
    inventory: &ModelInventory,
    floor: Option<api::EffortLevel>,
    ceiling: Option<api::EffortLevel>,
) -> Vec<ModelOption> {
    let low: usize = floor.map_or(0, api::EffortLevel::rung);
    let high: usize = ceiling.map_or(api::EffortLevel::LADDER.len() - 1, api::EffortLevel::rung);
    inventory
        .models()
        .iter()
        .filter(|model| model.band() != ModelBand::Superseded)
        .map(|model| ModelOption {
            id: model.id().to_string(),
            band: model.band(),
            efforts: api::EffortLevel::LADDER
                .iter()
                .copied()
                .filter(|level| (low..=high).contains(&level.rung()))
                .filter(|level| api::model_accepts_effort(model.id(), *level))
                .map(|level| level.label().to_string())
                .collect(),
        })
        .collect()
}

/// The shared price table as the scorer reads one: `api::model_price` in
/// `runtime`'s vocabulary.
///
/// Two structs, four identical rates. The scorer's core owns no table — it is
/// handed a price function so it can be tested on made-up numbers — and the
/// table owns no scorer, so the two meet here, in the layer that already knows
/// both. A model no row names stays `None`: unpriced is not free
/// (`docs/design/zo-autonomous-routing-review-20260915.md` §4.2).
#[must_use]
pub fn model_price_for(model: &str) -> Option<ModelPrice> {
    api::model_price(model).map(|price| ModelPrice {
        input: price.input,
        cache_read: price.cache_read,
        cache_write: price.cache_write,
        output: price.output,
    })
}

/// What the turn actually did — the four decisions as the host made them,
/// in the scorer's vocabulary, so a row can say whether the scorer agreed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanShadowActual {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// A [`runtime::PlanShape`] label.
    pub shape: String,
    /// A [`runtime::VerifyMode`] label.
    pub verify: String,
}

/// One scored candidate as the ledger keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanShadowCandidate {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub shape: String,
    pub verify: String,
    pub p_verified: f64,
    pub confidence: f64,
    pub t_verified_ms: u64,
    pub tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub interventions: f64,
}

impl PlanShadowCandidate {
    fn from_scored(plan: &ScoredPlan) -> Self {
        Self {
            model: plan.candidate.model.clone(),
            effort: plan.candidate.effort.clone(),
            shape: plan.candidate.shape.label(),
            verify: plan.candidate.verify.as_str().to_string(),
            p_verified: plan.estimate.p_verified,
            confidence: plan.estimate.confidence,
            t_verified_ms: plan.estimate.t_verified_ms,
            tokens: plan.estimate.tokens(),
            cost_usd: plan.estimate.cost_usd(),
            interventions: plan.estimate.interventions,
        }
    }

    fn matches(&self, actual: &PlanShadowActual) -> bool {
        self.model == actual.model
            && self.shape == actual.shape
            && self.verify == actual.verify
            && (self.effort.is_none() || actual.effort.is_none() || self.effort == actual.effort)
    }
}

/// One turn of the shadow ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanShadowRow {
    pub recorded_at: u64,
    pub session_id: String,
    /// The turn's attempt key when the runtime had minted one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    pub complexity: String,
    pub current_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_effort: Option<String>,
    pub attended: bool,
    pub objective_check: bool,
    pub context_tokens: u64,
    /// How many candidates were scored in all.
    pub candidate_count: usize,
    /// The ranked head (see `PLAN_SHADOW_HEAD`) plus the best same-model plan.
    pub candidates: Vec<PlanShadowCandidate>,
    /// Index into `candidates` of the scorer's pick, and why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub actual: PlanShadowActual,
    /// Whether the scorer's pick is the plan the turn ran.
    pub agreed: bool,
    /// Absent on the row a turn start files. A [`SwitchTrigger`] label on a
    /// row filed because the wire moved — `/model`, the quota or refusal
    /// fallback, the overload demotion, a deep-gate leg — where `actual` is
    /// the model the switch went to, `current_model` the one it left, and
    /// `context_tokens` what the new model had to read
    /// (`docs/design/zo-autonomous-routing-review-20260915.md` §5 P2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
}

/// The models a switch may be scored against. A forced switch — a quota
/// wall, a refusal, a shed tier — could not have stayed, so the model it
/// left is not a candidate; a quota wall takes its whole provider with it,
/// since the wall is the account's window, not one model's. Every other door
/// keeps the full set: the person's or the leg's choice is compared with
/// what the scorer would have done, staying included.
#[must_use]
pub fn switch_candidates(
    trigger: SwitchTrigger,
    from: &str,
    models: &[ModelOption],
) -> Vec<ModelOption> {
    if !trigger.forced() {
        return models.to_vec();
    }
    let walled_provider = (trigger == SwitchTrigger::Quota).then(|| api::detect_provider_kind(from));
    models
        .iter()
        .filter(|model| model.id != from)
        .filter(|model| {
            walled_provider.is_none_or(|walled| api::detect_provider_kind(&model.id) != walled)
        })
        .cloned()
        .collect()
}

/// Everything a row is built from, gathered by the caller.
pub struct PlanShadowInputs<'a> {
    pub session_id: &'a str,
    pub attempt: Option<&'a str>,
    pub recorded_at: u64,
    pub ctx: PlanContext<'a>,
    pub models: &'a [ModelOption],
    pub priors: &'a PlanPriors,
    pub records: &'a [RouteOutcomeRecord],
    pub price: &'a dyn Fn(&str) -> Option<ModelPrice>,
    pub cache: &'a [PlanCacheState],
    pub actual: PlanShadowActual,
    /// `None` for a turn-start row; the door for a switch row. The caller
    /// has already narrowed `models` with [`switch_candidates`].
    pub trigger: Option<SwitchTrigger>,
}

/// Score the turn's alternatives and lay the result next to what the turn
/// did. Pure apart from its inputs: the caller reads the store and the
/// inventory, this only computes.
#[must_use]
pub fn build_plan_shadow(inputs: PlanShadowInputs<'_>) -> PlanShadowRow {
    let band_of = |model: &str| {
        inputs
            .models
            .iter()
            .find(|option| option.id == model)
            .map_or(ModelBand::Rest, |option| option.band)
    };
    let evidence = |candidate: &PlanCandidate| {
        plan_evidence_from_records(inputs.records, candidate, canonicalize_route_model_id)
    };
    let scored: Vec<ScoredPlan> = plan_candidates(&inputs.ctx, inputs.models)
        .into_iter()
        .map(|candidate| {
            let estimate = score_plan(
                &candidate,
                &inputs.ctx,
                inputs.priors,
                band_of,
                evidence,
                inputs.price,
                inputs.cache,
            );
            ScoredPlan { candidate, estimate }
        })
        .collect();
    let choice = choose_plan(&scored, &inputs.ctx);

    // Keep the ranked head and, if it fell outside, the best same-model plan
    // and the scorer's pick — the rows a reader compares by hand.
    let mut order: Vec<usize> = (0..scored.len()).collect();
    order.sort_by(|a, b| rank(&scored[*a]).partial_cmp(&rank(&scored[*b])).unwrap_or(std::cmp::Ordering::Equal));
    let mut kept: Vec<usize> = order.iter().copied().take(PLAN_SHADOW_HEAD).collect();
    if let Some(best_stay) = order
        .iter()
        .copied()
        .find(|index| scored[*index].candidate.model == inputs.ctx.current_model)
    {
        if !kept.contains(&best_stay) {
            kept.push(best_stay);
        }
    }
    if let Some(choice) = choice {
        if !kept.contains(&choice.index) {
            kept.push(choice.index);
        }
    }
    let candidates: Vec<PlanShadowCandidate> = kept
        .iter()
        .map(|index| PlanShadowCandidate::from_scored(&scored[*index]))
        .collect();
    let chosen = choice.and_then(|choice| kept.iter().position(|index| *index == choice.index));
    let agreed = chosen.is_some_and(|index| candidates[index].matches(&inputs.actual));

    PlanShadowRow {
        recorded_at: inputs.recorded_at,
        session_id: inputs.session_id.to_string(),
        attempt: inputs.attempt.map(str::to_string),
        complexity: inputs.ctx.complexity.as_label().to_string(),
        current_model: inputs.ctx.current_model.to_string(),
        current_effort: inputs.ctx.current_effort.map(str::to_string),
        attended: inputs.ctx.attended,
        objective_check: inputs.ctx.objective_check_available,
        context_tokens: inputs.ctx.context_tokens,
        candidate_count: scored.len(),
        candidates,
        chosen,
        reason: choice.map(|choice| format!("{:?}", choice.reason)),
        actual: inputs.actual,
        agreed,
        trigger: inputs.trigger.map(|trigger| trigger.as_str().to_string()),
    }
}

/// Cheaper first, a missing price last — the scorer's own order.
fn rank(plan: &ScoredPlan) -> (u8, f64, u64, u64) {
    match plan.estimate.cost_usd() {
        Some(usd) => (0, usd, plan.estimate.t_verified_ms, plan.estimate.tokens()),
        None => (1, 0.0, plan.estimate.t_verified_ms, plan.estimate.tokens()),
    }
}

/// Where a project's shadow ledger lives.
#[must_use]
pub fn plan_shadow_path(cwd: &Path) -> PathBuf {
    shadow_ledger_path(cwd, PLAN_SHADOW_FILE)
}

/// Append one row to the project's shadow ledger.
pub fn record_plan_shadow(cwd: &Path, row: &PlanShadowRow) -> io::Result<()> {
    record_plan_shadow_at_path(&plan_shadow_path(cwd), row, SHADOW_LEDGER_MAX_BYTES)
}

/// Append one row at `path` with the shared shadow-ledger discipline
/// ([`append_shadow_row`]): one line, one `O_APPEND` write, the newer half
/// kept past `max_bytes`.
pub fn record_plan_shadow_at_path(path: &Path, row: &PlanShadowRow, max_bytes: u64) -> io::Result<()> {
    append_shadow_row(path, row, max_bytes)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use runtime::{ModelDescriptor, PlanShape, VerifyMode};

    #[test]
    fn the_shipped_catalog_prices_every_complexity_the_scorer_can_be_asked_about() {
        for complexity in [
            RouteTaskComplexity::Trivial,
            RouteTaskComplexity::Small,
            RouteTaskComplexity::Medium,
            RouteTaskComplexity::Large,
            RouteTaskComplexity::Unknown,
        ] {
            let priors = plan_priors_for(complexity).unwrap_or_else(|| panic!("{complexity:?}"));
            assert!(priors.requests > 0 && priors.output_tokens > 0 && priors.duration_ms > 0);
            assert!(priors.pass_top >= priors.pass_second && priors.pass_second >= priors.pass_rest);
            assert!(priors.pass_top <= 1.0);
        }
        // The medium turn is the measured one (r50), not a scaled estimate.
        assert_eq!(plan_priors_for(RouteTaskComplexity::Medium).unwrap().output_tokens, 7_551);
    }

    #[test]
    fn model_options_take_the_bands_efforts_the_model_accepts_and_skip_superseded_releases() {
        // The inventory stamps bands once, from catalog signals: the older
        // release of a lineage reads as superseded and serves no plan.
        let inventory = ModelInventory::new(
            "claude-opus-5",
            vec![
                ModelDescriptor::new("claude-opus-5", "anthropic", "claude").release_rank(2),
                ModelDescriptor::new("claude-opus-4-8", "anthropic", "claude").release_rank(1),
            ],
        );
        let superseded: Vec<&str> = inventory
            .models()
            .iter()
            .filter(|model| model.band() == ModelBand::Superseded)
            .map(ModelDescriptor::id)
            .collect();
        assert_eq!(superseded, vec!["claude-opus-4-8"], "fixture: the older release is superseded");
        let options = model_options_for(&inventory, Some(api::EffortLevel::High), Some(api::EffortLevel::Max));
        assert!(options.iter().all(|option| option.id != "claude-opus-4-8"));
        assert_eq!(options.len(), inventory.models().len() - superseded.len());
        let opus = options.iter().find(|option| option.id == "claude-opus-5").expect("the current release");
        let stamped = inventory.models().iter().find(|model| model.id() == "claude-opus-5").unwrap().band();
        assert_eq!(opus.band, stamped);
        // Every listed effort sits inside high..=max on the one ladder and
        // is one the catalog says the model takes.
        assert!(!opus.efforts.is_empty());
        for label in &opus.efforts {
            let level = api::parse_effort_level(label).expect("a ladder label");
            assert!((api::EffortLevel::High.rung()..=api::EffortLevel::Max.rung()).contains(&level.rung()));
            assert!(api::model_accepts_effort("claude-opus-5", level));
        }
        assert!(!opus.efforts.iter().any(|label| label == "low" || label == "ultra"));
    }

    fn priors() -> PlanPriors {
        plan_priors_for(RouteTaskComplexity::Medium).expect("shipped priors")
    }

    fn actual(model: &str) -> PlanShadowActual {
        PlanShadowActual {
            model: model.to_string(),
            effort: Some("high".to_string()),
            shape: PlanShape::Solo.label(),
            verify: VerifyMode::ModelJudge.as_str().to_string(),
        }
    }

    fn context(current: &str) -> PlanContext<'_> {
        PlanContext {
            complexity: RouteTaskComplexity::Medium,
            current_model: current,
            current_effort: Some("high"),
            context_tokens: 40_000,
            pinned_model: None,
            pinned_effort: None,
            attended: false,
            objective_check_available: false,
            independent_slices: None,
            max_lanes: 4,
            verify_ceiling: 3,
            min_p_verified: 0.5,
            switch_margin: 0.15,
        }
    }

    #[test]
    fn a_shadow_row_keeps_the_ranked_head_the_stay_plan_and_the_pick_and_says_whether_it_agreed() {
        let models = vec![
            ModelOption { id: "a".into(), band: ModelBand::Top, efforts: vec!["high".into(), "max".into()] },
            ModelOption { id: "b".into(), band: ModelBand::Second, efforts: vec!["high".into()] },
            ModelOption { id: "c".into(), band: ModelBand::Rest, efforts: vec!["high".into()] },
        ];
        let priors = priors();
        let price = |_: &str| Some(ModelPrice { input: 3.0, cache_read: 0.3, cache_write: 3.75, output: 15.0 });
        let row = build_plan_shadow(PlanShadowInputs {
            session_id: "session-1",
            attempt: Some("session-1@4"),
            recorded_at: 1,
            ctx: context("a"),
            models: &models,
            priors: &priors,
            records: &[],
            price: &price,
            cache: &[],
            actual: actual("a"),
            trigger: None,
        });
        // 3 models × (2+1+1 efforts) × 2 shapes (solo, delegate) × 1 verify (model judge).
        assert_eq!(row.candidate_count, 8);
        assert!(row.candidates.len() <= PLAN_SHADOW_HEAD + 2);
        let chosen = row.chosen.expect("a pick");
        assert!(chosen < row.candidates.len());
        assert!(row.candidates.iter().any(|candidate| candidate.model == "a"), "the stay plan is kept");
        assert_eq!(row.attempt.as_deref(), Some("session-1@4"));
        assert_eq!(row.complexity, "medium");
        // With flat prices and no evidence, staying on the warm-less current
        // model is still the bar; the pick is a solo model-judge plan on `a`
        // or a cheaper alternative past the margin — either way the row says
        // whether it matched what the turn did.
        let pick = &row.candidates[chosen];
        assert_eq!(row.agreed, pick.matches(&row.actual));
        let line = serde_json::to_string(&row).unwrap();
        let back: PlanShadowRow = serde_json::from_str(&line).unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn the_shadow_ledger_appends_one_line_and_keeps_its_newer_half_past_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan-shadow.jsonl");
        let models = vec![ModelOption { id: "a".into(), band: ModelBand::Top, efforts: vec![] }];
        let priors = priors();
        let price = |_: &str| None;
        let row = |n: u64| {
            build_plan_shadow(PlanShadowInputs {
                session_id: "s",
                attempt: None,
                recorded_at: n,
                ctx: context("a"),
                models: &models,
                priors: &priors,
                records: &[],
                price: &price,
                cache: &[],
                actual: actual("a"),
                trigger: None,
            })
        };
        for n in 1..=6 {
            record_plan_shadow_at_path(&path, &row(n), u64::MAX).unwrap();
        }
        let lines = fs::read_to_string(&path).unwrap().lines().count();
        assert_eq!(lines, 6);
        // A tiny cap: the next append first cuts the file to its newer half.
        record_plan_shadow_at_path(&path, &row(7), 1).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let rows: Vec<PlanShadowRow> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows.iter().map(|row| row.recorded_at).collect::<Vec<_>>(), vec![4, 5, 6, 7]);
    }

    /// A forced switch drops the model it left — and, for a quota wall, the
    /// whole provider behind it — while a person's or a leg's switch keeps
    /// the full set so the row can say whether staying would have won. The
    /// row itself carries the door.
    #[test]
    fn a_forced_switch_shrinks_the_candidate_set_and_the_row_names_its_door() {
        let models = vec![
            ModelOption { id: "claude-opus-5".into(), band: ModelBand::Top, efforts: vec!["high".into()] },
            ModelOption { id: "claude-sonnet-5".into(), band: ModelBand::Second, efforts: vec!["high".into()] },
            ModelOption { id: "gpt-5.6-sol".into(), band: ModelBand::Second, efforts: vec!["high".into()] },
        ];
        let ids = |options: &[ModelOption]| options.iter().map(|o| o.id.clone()).collect::<Vec<_>>();
        // The quota wall is the account's: every Anthropic model goes with it.
        assert_eq!(ids(&switch_candidates(SwitchTrigger::Quota, "claude-opus-5", &models)), vec!["gpt-5.6-sol"]);
        // A refusal or a shed tier takes only the model that failed.
        assert_eq!(
            ids(&switch_candidates(SwitchTrigger::Refusal, "claude-opus-5", &models)),
            vec!["claude-sonnet-5", "gpt-5.6-sol"]
        );
        assert_eq!(
            ids(&switch_candidates(SwitchTrigger::Starvation, "claude-opus-5", &models)),
            vec!["claude-sonnet-5", "gpt-5.6-sol"]
        );
        // The person and the legs are compared against the full set.
        for trigger in [SwitchTrigger::Person, SwitchTrigger::PlanLeg, SwitchTrigger::VerifyLeg, SwitchTrigger::ExecLeg] {
            assert_eq!(switch_candidates(trigger, "claude-opus-5", &models), models, "{trigger:?}");
        }

        let priors = priors();
        let price = |_: &str| Some(ModelPrice { input: 3.0, cache_read: 0.3, cache_write: 3.75, output: 15.0 });
        let narrowed = switch_candidates(SwitchTrigger::Quota, "claude-opus-5", &models);
        let row = build_plan_shadow(PlanShadowInputs {
            session_id: "session-1",
            attempt: Some("session-1@4"),
            recorded_at: 1,
            ctx: context("claude-opus-5"),
            models: &narrowed,
            priors: &priors,
            records: &[],
            price: &price,
            cache: &[],
            actual: PlanShadowActual {
                model: "gpt-5.6-sol".to_string(),
                effort: None,
                shape: PlanShape::Solo.label(),
                verify: VerifyMode::None.as_str().to_string(),
            },
            trigger: Some(SwitchTrigger::Quota),
        });
        assert_eq!(row.trigger.as_deref(), Some("quota"));
        // With the walled model gone there is no stay bar: the pick is the
        // best of what is left, and it is the fallback the turn took.
        assert_eq!(row.reason.as_deref(), Some("SwitchByMargin"));
        assert!(row.candidates.iter().all(|c| c.model == "gpt-5.6-sol"), "{:?}", row.candidates);
        let line = serde_json::to_string(&row).unwrap();
        assert!(line.contains("\"trigger\":\"quota\""), "{line}");
        let back: PlanShadowRow = serde_json::from_str(&line).unwrap();
        assert_eq!(back, row);
        // A turn-start row keeps the column off the wire.
        let turn = build_plan_shadow(PlanShadowInputs {
            session_id: "s",
            attempt: None,
            recorded_at: 1,
            ctx: context("claude-opus-5"),
            models: &models,
            priors: &priors,
            records: &[],
            price: &price,
            cache: &[],
            actual: actual("claude-opus-5"),
            trigger: None,
        });
        assert!(!serde_json::to_string(&turn).unwrap().contains("trigger"));
    }
}
