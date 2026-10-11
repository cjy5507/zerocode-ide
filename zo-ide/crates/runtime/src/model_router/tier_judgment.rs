//! A judged tier order — where each model of a provider stands, as the Jev
//! seat read it off the catalog facts and the recorded outcomes (t-45961).
//!
//! The classifier ([`super::tiering`]) orders a provider's models by one fact
//! rule when nothing better stands. A judgment replaces that order for the
//! models it covers; a model it does not cover (one discovered after the
//! judgment) takes a provisional place by the facts until the next judgment.

use std::collections::BTreeMap;

use api::{SystemOneQuestion, SystemOneResponse};
use serde_json::{json, Value};
use zerocode_core::summon_difficulty::{lineup::CHALLENGE_MIN_SAMPLES, LADDER};

use super::inventory::ModelDescriptor;
use super::policy::{EffortCeiling, ModelTier, TiersProvenance};
use super::outcome::{is_terminal_outcome_status, RouteOutcomeRecord, OUTCOME_COMPLETED, OUTCOME_FAILED};
use super::tiering::{class_word, classify_model_tiers_with, FactRule, ModelBand, ModelTierAssignment};

/// Who placed a model in its provider's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum TierSource {
    /// The Jev seat's judgment covered it.
    Judged,
    /// No judgment stood for its provider: the fact rule placed it.
    #[default]
    FactRule,
    /// Its provider was judged, but not this model — it arrived after the
    /// judgment, and the facts gave it a place until the next one.
    Provisional,
}

impl TierSource {
    /// The word a report prints (`zo models --json`, `tierFrom`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Judged => "jev",
            Self::FactRule => "facts",
            Self::Provisional => "provisional",
        }
    }
}

/// One judged model: its place on the judged capability scale and how sure
/// the judgment was, both in thousandths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JudgedPlace {
    pub score_permille: u16,
    pub confidence_permille: u16,
}

/// One judgment over a lineup: the places it gave, by model id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TierJudgment {
    pub places: BTreeMap<String, JudgedPlace>,
}

/// [`classify_model_tiers`](super::tiering::classify_model_tiers) with a judgment applied where one stands.
#[must_use]
pub fn classify_model_tiers_judged(
    models: &[ModelDescriptor],
    judgment: Option<&TierJudgment>,
) -> Vec<ModelTierAssignment> {
    classify_model_tiers_with(models, judgment)
}

/// One model's place in its provider's order, and who gave it.
#[derive(Debug, Clone, Copy)]
pub(super) struct Place<'a> {
    pub model: &'a ModelDescriptor,
    pub source: TierSource,
    pub confidence_permille: Option<u16>,
}

/// One provider's models (`by_facts`, already in the fact rule's order) in
/// the order a judgment gives them.
///
/// - A provider the judgment covers none of keeps the fact order.
/// - The head of the fact order stays at the head when it reaches the Deep
///   tier: planning and verifying stand on that declaration, and the
///   judgment orders the implementers below it.
/// - The judged implementers stand by their judged score, the higher first,
///   a tie kept in the fact order.
/// - A model the judgment does not cover — discovered after it — is placed
///   by the facts: just above the first model already placed that the fact
///   rule ranks below it, else last. It is provisional until a judgment
///   covers it.
pub(super) fn judged_order<'a>(
    by_facts: Vec<&'a ModelDescriptor>,
    rule: &FactRule,
    judgment: Option<&TierJudgment>,
) -> Vec<Place<'a>> {
    let judged = |model: &ModelDescriptor| judgment.and_then(|judgment| judgment.places.get(model.id()).copied());
    if !by_facts.iter().any(|model| judged(model).is_some()) {
        return by_facts
            .into_iter()
            .map(|model| Place { model, source: TierSource::FactRule, confidence_permille: None })
            .collect();
    }
    let place = |model: &'a ModelDescriptor| match judged(model) {
        Some(found) => Place { model, source: TierSource::Judged, confidence_permille: Some(found.confidence_permille) },
        None => Place { model, source: TierSource::Provisional, confidence_permille: None },
    };
    let mut order: Vec<Place<'a>> = Vec::new();
    let mut implementers: Vec<&'a ModelDescriptor> = Vec::new();
    for (index, model) in by_facts.into_iter().enumerate() {
        if index == 0 && model.has_tier(ModelTier::Deep) {
            order.push(place(model));
        } else {
            implementers.push(model);
        }
    }
    let (mut covered, provisional): (Vec<&ModelDescriptor>, Vec<&ModelDescriptor>) =
        implementers.into_iter().partition(|model| judged(model).is_some());
    covered.sort_by_key(|model| std::cmp::Reverse(judged(model).map_or(0, |found| found.score_permille)));
    let mut below: Vec<Place<'a>> = covered.into_iter().map(place).collect();
    for model in provisional {
        let key = rule.key(model);
        let at = below.iter().position(|placed| key > rule.key(placed.model)).unwrap_or(below.len());
        below.insert(at, place(model));
    }
    order.extend(below);
    order
}

/// The level above the summons' ladder: the work a provider's top model is
/// kept for.
pub const TIER_TOP_LEVEL: &str = "Planning the lineup's hardest work and verifying it.";

/// The levels a model's place is scored on, the low end first: the work the
/// summons' difficulty ladder describes, rung by rung ([`LADDER`]), then
/// planning and verifying. Each is a situation, since a score is judged one
/// level at a time.
#[must_use]
pub fn tier_levels() -> [&'static str; 4] {
    [LADDER[0].1, LADDER[1].1, LADDER[2].1, TIER_TOP_LEVEL]
}

/// The question asked of each model; `{at}` is its place in the state.
pub const TIER_INSTRUCTIONS: &str = "Which work can `{at}` be trusted with? Judge it beside the other models of `models`, one provider's lineup, from its catalog facts and the outcomes recorded for it: `class` is the class the provider declared (`classFrom` says whether the provider declared it or it was derived), `pricePerMillionTokens` its list price in US dollars, `contextWindow` its window in tokens, `releaseOrder` higher for a newer release, `effortCeiling` the deepest reasoning it accepts, and `outcomes` the runs it finished here.";

/// The keys of the tier seat's state, and of each model in it.
pub const TIER_STATE_KEYS: [&str; 2] = ["provider", "models"];
pub const TIER_MODEL_KEYS: [&str; 8] =
    ["id", "class", "classFrom", "pricePerMillionTokens", "contextWindow", "releaseOrder", "effortCeiling", "outcomes"];

/// The id of the question asked of the model at `index` of the state.
#[must_use]
pub fn tier_question_id(index: usize) -> String {
    format!("place_{index}")
}

/// Every word the tier question asks with, in one text: what its version
/// ([`zerocode_core::jev::questions::MODEL_TIERS_RUBRIC_VERSION`]) is pinned
/// to.
#[must_use]
pub fn tier_rubric_words() -> String {
    std::iter::once(TIER_INSTRUCTIONS)
        .chain(tier_levels())
        .chain(TIER_STATE_KEYS)
        .chain(TIER_MODEL_KEYS)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The questions for a lineup of `count` models: one score per model.
#[must_use]
pub fn tier_questions(count: usize) -> BTreeMap<String, SystemOneQuestion> {
    (0..count)
        .map(|index| {
            let at = format!("models[{index}]");
            (tier_question_id(index), SystemOneQuestion::score(&TIER_INSTRUCTIONS.replace("{at}", &at), tier_levels()))
        })
        .collect()
}

/// What a model's recorded work says, for the judgment: how many runs ended
/// here, how many completed and how many failed, and their mean size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TierOutcomes {
    pub runs: u64,
    pub completed: u64,
    pub failed: u64,
    pub mean_output_tokens: Option<u64>,
    pub mean_minutes: Option<u64>,
}

/// How many more recorded runs, completions or failures move a lineup's
/// fingerprint — and so ask its judgment again: as many as make a record
/// here ([`CHALLENGE_MIN_SAMPLES`]).
fn outcome_step() -> u64 {
    u64::try_from(CHALLENGE_MIN_SAMPLES).unwrap_or(u64::MAX).max(1)
}

fn effort_word(ceiling: EffortCeiling) -> &'static str {
    match ceiling {
        EffortCeiling::High => "high",
        EffortCeiling::Xhigh => "xhigh",
        EffortCeiling::Max => "max",
        EffortCeiling::Ultra => "ultra",
    }
}

fn provenance_word(provenance: TiersProvenance) -> &'static str {
    match provenance {
        TiersProvenance::ProviderDeclared => "provider",
        TiersProvenance::ColdStartPrior => "effort ceiling",
        TiersProvenance::Fallback => "guess",
        TiersProvenance::Learned => "learned",
    }
}

/// US dollars per million tokens from a descriptor's millionths.
fn usd(micro_usd: u64) -> Value {
    // A list price is a few hundred dollars at most, and a million is a
    // million: both exact in an f64.
    #[allow(clippy::cast_precision_loss)]
    let dollars = micro_usd as f64 / super::inventory::MICRO_USD_PER_USD as f64;
    json!(dollars)
}

/// One provider's lineup as the tier seat sends it: every model's catalog
/// facts and its recorded outcomes. `exact` is false for the lineup's
/// fingerprint, which counts outcomes in steps ([`outcome_step`]) and leaves
/// their means out, so a lineup is judged again when its record moves by a
/// step and not on every run.
fn lineup_state(
    provider: &str,
    models: &[&ModelDescriptor],
    outcomes: &dyn Fn(&str) -> Option<TierOutcomes>,
    exact: bool,
) -> Value {
    let step = if exact { 1 } else { outcome_step() };
    let rows: Vec<Value> = models
        .iter()
        .map(|model| {
            let recorded = outcomes(model.id()).map(|seen| {
                let mut row = json!({
                    "runs": seen.runs / step,
                    "completed": seen.completed / step,
                    "failed": seen.failed / step,
                });
                if exact {
                    row["meanOutputTokens"] = json!(seen.mean_output_tokens);
                    row["meanMinutes"] = json!(seen.mean_minutes);
                }
                row
            });
            json!({
                "id": model.id(),
                "class": class_word(model),
                "classFrom": provenance_word(model.tiers_provenance_value()),
                "pricePerMillionTokens": model.list_price_value().map(|price| json!({
                    "input": usd(price.input_micro_usd),
                    "output": usd(price.output_micro_usd),
                })),
                "contextWindow": model.context_window_value(),
                "releaseOrder": model.release_rank_value(),
                "effortCeiling": effort_word(model.effort_ceiling_value()),
                "outcomes": recorded,
            })
        })
        .collect();
    json!({ "provider": provider, "models": rows })
}

/// The state the tier seat sends for one provider's lineup.
#[must_use]
pub fn tier_state(provider: &str, models: &[&ModelDescriptor], outcomes: &dyn Fn(&str) -> Option<TierOutcomes>) -> Value {
    lineup_state(provider, models, outcomes, true)
}

/// The lineup's fingerprint: its facts and its record counted in steps,
/// under the question's version. A judgment stands for the fingerprint it
/// was asked under; a new fingerprint asks again.
#[must_use]
pub fn tier_fingerprint(
    provider: &str,
    models: &[&ModelDescriptor],
    outcomes: &dyn Fn(&str) -> Option<TierOutcomes>,
) -> String {
    let state = lineup_state(provider, models, outcomes, false);
    zerocode_core::jev::fingerprint_of(&format!(
        "{}\n{state}",
        zerocode_core::jev::questions::MODEL_TIERS_RUBRIC_VERSION
    ))
}

/// Each provider's lineup as the tier seat asks it: its models in the fact
/// rule's order (`by_facts`, the classifier's assignments over `models`),
/// superseded releases left out.
#[must_use]
pub fn tier_lineups_of<'a>(
    models: &'a [ModelDescriptor],
    by_facts: &[ModelTierAssignment],
) -> Vec<(String, Vec<&'a ModelDescriptor>)> {
    let mut lineups: Vec<(String, Vec<(usize, &'a ModelDescriptor)>)> = Vec::new();
    for entry in by_facts.iter().filter(|entry| entry.band != ModelBand::Superseded) {
        let Some(model) = models.iter().find(|model| model.id() == entry.id) else {
            continue;
        };
        match lineups.iter_mut().find(|(provider, _)| *provider == entry.provider) {
            Some((_, lineup)) => lineup.push((entry.rank, model)),
            None => lineups.push((entry.provider.clone(), vec![(entry.rank, model)])),
        }
    }
    lineups
        .into_iter()
        .map(|(provider, mut lineup)| {
            lineup.sort_by_key(|(rank, _)| *rank);
            (provider, lineup.into_iter().map(|(_, model)| model).collect())
        })
        .collect()
}

/// Milliseconds in a minute, for a run's mean length.
const MS_PER_MINUTE: u64 = 60_000;

/// What each model's recorded runs say — `records` are zo's route outcomes,
/// every project's — for the tier seat's state: the runs that ended, how
/// many completed, how many failed on the model's own account (a provider's
/// error is not the model's), and their mean output and length.
#[must_use]
pub fn tier_outcomes_of(records: &[RouteOutcomeRecord]) -> BTreeMap<String, TierOutcomes> {
    #[derive(Default)]
    struct Sums {
        seen: TierOutcomes,
        tokens: u64,
        millis: u64,
        timed: u64,
    }
    let mut sums: BTreeMap<String, Sums> = BTreeMap::new();
    for record in records.iter().filter(|record| is_terminal_outcome_status(&record.status)) {
        let sum = sums.entry(record.selected_model.clone()).or_default();
        sum.seen.runs += 1;
        if record.status == OUTCOME_COMPLETED {
            sum.seen.completed += 1;
        }
        if record.status == OUTCOME_FAILED && record.provider_error_class.is_none() {
            sum.seen.failed += 1;
        }
        sum.tokens = sum.tokens.saturating_add(record.output_tokens);
        if let Some(millis) = record.duration_ms {
            sum.millis = sum.millis.saturating_add(millis);
            sum.timed += 1;
        }
    }
    sums.into_iter()
        .map(|(model, sum)| {
            let mut seen = sum.seen;
            seen.mean_output_tokens = sum.tokens.checked_div(seen.runs);
            seen.mean_minutes = sum.millis.checked_div(sum.timed).map(|millis| millis / MS_PER_MINUTE);
            (model, seen)
        })
        .collect()
}

/// Why an answer was not taken as a judgment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TierRefusal {
    /// An answer that is not a score, or a score or confidence off the scale.
    Schema(String),
    /// No model of the lineup was answered.
    Empty,
}

/// Thousandths in a whole: the unit a judged place and its confidence are
/// written in.
const PERMILLE_PER_WHOLE: f64 = 1000.0;

/// A share in thousandths.
fn permille(share: f64) -> u16 {
    // Clamped to 0..=1 first: the product is a whole number in 0..=1000.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let permille = (share.clamp(0.0, 1.0) * PERMILLE_PER_WHOLE).round() as u16;
    permille
}

/// Read a response to [`tier_questions`] over `ids` (the state's models, in
/// its order) as a judgment. A model whose answer is missing is left out —
/// it stands by the facts, provisionally; an answer that is not a score on
/// the scale refuses the whole response.
///
/// # Errors
/// [`TierRefusal`] as above.
pub fn read_tier_answers(response: &SystemOneResponse, ids: &[&str]) -> Result<TierJudgment, TierRefusal> {
    let top = f64::from(u8::try_from(tier_levels().len() - 1).unwrap_or(u8::MAX));
    let mut places = BTreeMap::new();
    for (index, id) in ids.iter().enumerate() {
        let Some(answer) = response.score_answer(&tier_question_id(index)) else {
            continue;
        };
        let answer = answer.map_err(|error| TierRefusal::Schema(format!("{id}: {error}")))?;
        if !(0.0..=top).contains(&answer.score) || !(0.0..=1.0).contains(&answer.confidence) {
            return Err(TierRefusal::Schema(format!("{id}: score {} confidence {}", answer.score, answer.confidence)));
        }
        places.insert(
            (*id).to_string(),
            JudgedPlace { score_permille: permille(answer.score / top), confidence_permille: permille(answer.confidence) },
        );
    }
    if places.is_empty() {
        return Err(TierRefusal::Empty);
    }
    Ok(TierJudgment { places })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_router::policy::ModelCapability;
    use crate::model_router::ModelSource;

    fn model(id: &str, tiers: [ModelTier; 2], release: u32) -> ModelDescriptor {
        ModelDescriptor::new(id, "p", id)
            .source(ModelSource::EnabledBuiltinProvider)
            .capabilities([ModelCapability::Default, ModelCapability::Coding])
            .tiers(tiers)
            .tiers_provenance(TiersProvenance::ProviderDeclared)
            .effort_ceiling(EffortCeiling::Max)
            .release_rank(release)
            .context_window(200_000)
    }

    fn recorded(runs: u64) -> TierOutcomes {
        TierOutcomes { runs, completed: runs, failed: 0, mean_output_tokens: Some(1_000), mean_minutes: Some(3) }
    }

    /// The state carries each model's facts and record under the keys the
    /// question names — no word a person wrote — and the fingerprint moves
    /// when a model arrives or its record crosses a step, not on every run.
    #[test]
    fn a_lineup_is_sent_as_facts_and_counts_and_judged_again_when_they_move() {
        let a = model("a", [ModelTier::Balanced, ModelTier::Strong], 1).list_price_micro_usd(3_000_000, 15_000_000);
        let b = model("b", [ModelTier::Fast, ModelTier::Balanced], 2);
        let runs = |count: u64| move |id: &str| (id == "a").then(|| recorded(count));
        let state = tier_state("p", &[&a, &b], &runs(3));
        assert_eq!(state["provider"], "p");
        let first = &state["models"][0];
        for key in TIER_MODEL_KEYS {
            assert!(first.get(key).is_some(), "{key} in {first}");
        }
        assert_eq!(first["class"], "balanced");
        assert_eq!(first["classFrom"], "provider");
        assert_eq!(first["pricePerMillionTokens"]["output"], 15.0);
        assert_eq!(first["outcomes"]["runs"], 3);
        assert_eq!(state["models"][1]["class"], "fast");
        assert!(state["models"][1]["outcomes"].is_null(), "no record is no record, never zero runs");

        let step = outcome_step();
        let at = |count: u64, models: &[&ModelDescriptor]| tier_fingerprint("p", models, &runs(count));
        assert_eq!(at(0, &[&a, &b]), at(step - 1, &[&a, &b]), "inside a step: the judgment stands");
        assert_ne!(at(step - 1, &[&a, &b]), at(step, &[&a, &b]), "a step of record asks again");
        let c = model("c", [ModelTier::Balanced, ModelTier::Strong], 3);
        assert_ne!(at(0, &[&a, &b]), at(0, &[&a, &b, &c]), "a model arrived");
        assert_eq!(tier_questions(3).len(), 3);
        assert!(tier_questions(3)[&tier_question_id(2)].instructions.contains("`models[2]`"));
    }

    fn response(answers: &serde_json::Value) -> SystemOneResponse {
        serde_json::from_value(json!({
            "model": "jev-test",
            "answers": answers,
            "usage": {"input_tokens": 100, "output_tokens": 10},
        }))
        .expect("a response in the contract's shape")
    }

    fn score(score: f64, confidence: f64) -> serde_json::Value {
        json!({
            "type": "score", "score": score, "confidence": confidence,
            "legend": {"0": "a", "1": "b", "2": "c", "3": "d"},
            "probabilities": {"0": 0.1, "1": 0.2, "2": 0.3, "3": 0.4},
        })
    }

    /// A score is read as the model's place on the scale, in thousandths,
    /// beside the answer's confidence; a model the response does not answer
    /// is left to the facts; a score off the scale refuses the response.
    #[test]
    fn an_answer_is_read_as_places_and_an_unanswered_model_is_left_to_the_facts() {
        let judged = read_tier_answers(&response(&json!({ "place_0": score(1.5, 0.8) })), &["a", "b"]).expect("judged");
        assert_eq!(judged.places.get("a"), Some(&JudgedPlace { score_permille: 500, confidence_permille: 800 }));
        assert!(!judged.places.contains_key("b"));
        assert_eq!(read_tier_answers(&response(&json!({})), &["a"]), Err(TierRefusal::Empty));
        assert!(matches!(
            read_tier_answers(&response(&json!({ "place_0": score(4.0, 0.8) })), &["a"]),
            Err(TierRefusal::Schema(_))
        ));
    }

    /// A model's record counts the runs that ended, its completions and the
    /// failures that were its own — a provider's error is not — with the
    /// mean output and length of its runs; a run still going is no record.
    #[test]
    fn a_models_record_counts_its_own_failures_and_its_means() {
        let run = |model: &str, status: &str, tokens: u64, millis: Option<u64>| RouteOutcomeRecord {
            output_tokens: tokens,
            duration_ms: millis,
            ..RouteOutcomeRecord::new("subagent", "Coding", model, status)
        };
        let mut provider_down = run("a", OUTCOME_FAILED, 0, None);
        provider_down.provider_error_class = Some("overloaded".to_string());
        let records = [
            run("a", OUTCOME_COMPLETED, 3_000, Some(120_000)),
            run("a", OUTCOME_FAILED, 1_000, Some(60_000)),
            provider_down,
            run("b", "still_running", 9_000, None),
        ];
        let outcomes = tier_outcomes_of(&records);
        let a = outcomes["a"];
        assert_eq!((a.runs, a.completed, a.failed), (3, 1, 1), "{a:?}");
        assert_eq!(a.mean_output_tokens, Some(4_000 / 3));
        assert_eq!(a.mean_minutes, Some(u64::midpoint(120_000, 60_000) / MS_PER_MINUTE));
        assert!(!outcomes.contains_key("b"), "a run still going is no record");
    }

    /// The tier question's words are pinned to its version: a changed level,
    /// instruction or key changes the fingerprint, and the version must move
    /// with it.
    #[test]
    fn the_version_is_pinned_to_the_words() {
        assert_eq!(zerocode_core::jev::questions::MODEL_TIERS_RUBRIC_VERSION, 1);
        assert_eq!(zerocode_core::jev::rubric_fingerprint(tier_rubric_words), "e839b8f9036e0740");
    }
}
