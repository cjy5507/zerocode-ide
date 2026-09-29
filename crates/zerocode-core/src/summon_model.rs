//! Which model a summons runs on, when nobody named one (t-14437).
//!
//! The person's words (2026-09-29): the model and effort a worker runs on
//! are Jev's to judge, for THIS work, among whatever the agent's CLI takes
//! today — a model released this morning included. So a summons that leaves
//! `--model` open asks one closed choice: every model the agent's lineup
//! holds today ([`crate::summon_difficulty::lineup`], folded ones out), each
//! with what its provider's classifier makes of it, the effort it would run
//! at, whether it is new, how much of its quota is spent, and what this
//! ledger saw it do — plus `abstain`, which leaves the ladder's default.
//!
//! The effort is not a second question: the difficulty seat's answer names
//! the ladder's effort, and the chosen model takes it inside what it
//! accepts. Code filters the answer, never the judge: a model outside the
//! offered set is refused by the reader, and an agent that cannot run it
//! never had it offered.
//!
//! A model this ledger has no record of cannot be chosen on evidence, so a
//! share of easy and ordinary summonses tries one first ([`challenger`]); the
//! work it carries is the record the next question reads.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::jev::{Cap, choice};
use crate::summon_difficulty::lineup::{self, Candidate, Lineup, Seen};
use crate::summon_difficulty::{Look, SPEC_CHAR_CAP, TITLE_CHAR_CAP};

/// The one question's name.
pub const QUESTION: &str = "summon_model";
/// The option that leaves the ladder's default in place.
pub const ABSTAIN: &str = "abstain";
/// The version of the words below; bump it when any of them changes.
pub const RUBRIC_VERSION: u32 = 1;
/// The fewest models that make a choice: one model beside `abstain` is a
/// yes-or-no about a default, not a pick.
pub const FEWEST_MODELS: usize = 2;
/// The row key a receipt names the chosen effort under.
pub const EFFORT_KEY: &str = "chosenEffort";
/// The row key that marks a summons a challenger's turn launched.
pub const CHALLENGE_KEY: &str = "challenge";

const INSTRUCTIONS: &str = "A worker is about to be summoned to carry out the work in `title` and `spec` (the first 400 characters). Choose the model it runs on. `models` holds every model this agent's command line accepts today, each under the `id` its option names: where its provider's own classifier ranks it (`band`: `top` plans and verifies, `second` takes hard implementation, `rest` ordinary and easy work; `rungs` the implementation work it serves; both null when unranked), the `effort` it would run at, whether it arrived recently (`new`), how much of its provider's quota is spent (`quotaSpentPercent` of its `quotaWindow`, both null when unread), and this ledger's record of the summonses it carried here: how many (`summoned`), how many have `ended`, how many of those passed on the first attempt (`passedFirstTry`), and the median rework rounds and tokens of the ended ones (`medianReworkRounds`, `medianTokens`). A record built on a handful of summonses is weak evidence, and a model with none has not been shown either way. `attempt` counts earlier summonses of this task, `failures` its consecutive failures, and `retryOf` says whether this summons replaces an ended attempt. Choose the least costly model that will carry this work well on the first attempt. Choose `abstain` when nothing here tells the models apart for this work.";
const OPTION_MEANS: &str = "The worker runs on {model}; its facts are its entry in `models`.";
const ABSTAIN_MEANS: &str = "Nothing here tells the models apart for this work; the ladder's default for its difficulty runs.";
const STATE_KEYS: [&str; 6] = ["title", "spec", "attempt", "failures", "retryOf", "models"];
const MODEL_KEYS: [&str; 12] = [
    "id",
    "band",
    "rungs",
    "effort",
    "new",
    "quotaSpentPercent",
    "quotaWindow",
    "summoned",
    "ended",
    "passedFirstTry",
    "medianReworkRounds",
    "medianTokens",
];

/// What this ledger saw one model do: every summons that ran on it and the
/// outcome of those whose work ended — read off the outcome rows the
/// summons seats already keep, never kept a second time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRecord {
    pub summoned: usize,
    pub ended: usize,
    pub passed_first_try: usize,
    pub median_rework_rounds: Option<u64>,
    pub median_tokens: Option<u64>,
}

/// `agent`'s records by model, folded from outcome rows (`executionModel`
/// beside [`crate::summon_difficulty::outcomes::KEY`]); `canonical` names the
/// model a launched word meant, so an alias and its release are one record.
/// A summons two seats both labeled counts once.
#[must_use]
pub fn records<'a>(
    rows: impl IntoIterator<Item = &'a Value>,
    agent: &str,
    canonical: impl Fn(&str) -> String,
) -> BTreeMap<String, ModelRecord> {
    use crate::summon_difficulty::outcomes::KEY;
    // One outcome per summons: the newest row either seat wrote for it.
    let mut by_summons: BTreeMap<(&str, &str), &Value> = BTreeMap::new();
    for row in rows {
        if row["agent"].as_str() != Some(agent) || row.get(KEY).is_none() {
            continue;
        }
        let (Some(run), Some(dispatch)) = (row["run"].as_str(), row["dispatch"].as_str()) else {
            continue;
        };
        by_summons.insert((run, dispatch), row);
    }
    let mut records: BTreeMap<String, ModelRecord> = BTreeMap::new();
    let mut rework: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut tokens: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for row in by_summons.into_values() {
        let Some(model) = row["executionModel"].as_str().map(&canonical) else {
            continue;
        };
        let outcome = &row[KEY];
        let record = records.entry(model.clone()).or_default();
        record.summoned += 1;
        let Some(first) = outcome["firstAttemptSuccess"].as_bool() else {
            continue;
        };
        record.ended += 1;
        record.passed_first_try += usize::from(first);
        if let Some(rounds) = outcome["reworkRounds"].as_u64() {
            rework.entry(model.clone()).or_default().push(rounds);
        }
        if let Some(spent) = outcome["tokens"].as_u64() {
            tokens.entry(model).or_default().push(spent);
        }
    }
    for (model, mut held) in rework {
        held.sort_unstable();
        if let Some(record) = records.get_mut(&model) {
            record.median_rework_rounds = crate::jev::summary::percentile(&held, 0.50);
        }
    }
    for (model, mut held) in tokens {
        held.sort_unstable();
        if let Some(record) = records.get_mut(&model) {
            record.median_tokens = crate::jev::summary::percentile(&held, 0.50);
        }
    }
    records
}

/// One model offered, with every fact its entry carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub band: Option<lineup::Band>,
    pub rungs: BTreeSet<lineup::Rung>,
    pub effort: String,
    pub fresh: bool,
    pub quota_spent_percent: Option<u8>,
    pub quota_window: Option<&'static str>,
    pub record: ModelRecord,
}

/// Every live model of `lineup`, each at `ladder` effort brought inside what
/// it accepts; `quota` reads a model's gauge the way the quota gate does.
#[must_use]
pub fn options(
    agent: &str,
    lineup: &Lineup,
    seen: Option<&Seen>,
    records: &BTreeMap<String, ModelRecord>,
    ladder: &str,
    quota: impl Fn(&str) -> Option<(u8, &'static str)>,
    now_ms: i64,
) -> Vec<ModelOption> {
    lineup
        .models
        .iter()
        .filter(|model| !model.folded())
        .map(|model| {
            let (spent, window) = quota(&model.id).unzip();
            ModelOption {
                id: model.id.clone(),
                band: model.band,
                rungs: model.rungs.clone(),
                effort: model.effort_for(ladder),
                fresh: seen.is_some_and(|seen| seen.fresh(agent, &model.id, now_ms)),
                quota_spent_percent: spent,
                quota_window: window,
                record: records.get(&model.id).cloned().unwrap_or_default(),
            }
        })
        .collect()
}

/// What the window holds for one agent's model question — today's lineup,
/// the book of models seen, and this ledger's records — peeked, never
/// fetched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub lineup: Lineup,
    pub seen: Seen,
    pub records: BTreeMap<String, ModelRecord>,
}

/// The model question's ledger half, recorded after the summons opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shadow {
    /// The question, for a seat that only records to ask off the beat.
    pub ask: ModelAsk,
    /// An acting request's receipt; its presence prevents a second request.
    pub receipt: Option<Value>,
    /// A challenger's turn launched the summons: the answer is recorded, not
    /// carried out, and its outcome grades no answer.
    pub challenge: bool,
}

/// One question and the models its answer is judged against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelAsk {
    pub state: Value,
    pub questions: Value,
    offered: Vec<(String, String)>,
}

/// A validated answer: a model and the effort it runs at, or `None` for
/// `abstain`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPick {
    pub chosen: Option<(String, String)>,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// The question for `look` over `options`; `None` under [`FEWEST_MODELS`].
#[must_use]
pub fn ask(look: &Look, options: &[ModelOption]) -> Option<ModelAsk> {
    if options.len() < FEWEST_MODELS {
        return None;
    }
    let mut criteria = Map::new();
    let mut models = Vec::new();
    for option in options {
        criteria.insert(
            option.id.clone(),
            Value::from(OPTION_MEANS.replace("{model}", &option.id)),
        );
        let record = &option.record;
        models.push(Value::Object(Map::from_iter(
            MODEL_KEYS.map(String::from).into_iter().zip([
                Value::from(option.id.as_str()),
                serde_json::to_value(option.band).unwrap_or_default(),
                if option.band.is_some() {
                    serde_json::to_value(&option.rungs).unwrap_or_default()
                } else {
                    Value::Null
                },
                Value::from(option.effort.as_str()),
                Value::from(option.fresh),
                Value::from(option.quota_window.and(option.quota_spent_percent)),
                Value::from(option.quota_spent_percent.and(option.quota_window)),
                Value::from(record.summoned),
                Value::from(record.ended),
                Value::from(record.passed_first_try),
                Value::from(record.median_rework_rounds),
                Value::from(record.median_tokens),
            ]),
        )));
    }
    criteria.insert(ABSTAIN.to_string(), Value::from(ABSTAIN_MEANS));
    let state = Value::Object(Map::from_iter(
        STATE_KEYS.map(String::from).into_iter().zip([
            Value::from(crate::jev::brief_shape(&look.title, Cap::Chars(TITLE_CHAR_CAP)).0),
            Value::from(crate::jev::brief_shape(&look.spec, Cap::Chars(SPEC_CHAR_CAP)).0),
            Value::from(look.attempt),
            Value::from(look.failures),
            Value::from(look.retry_of),
            Value::Array(models),
        ]),
    ));
    Some(ModelAsk {
        state,
        questions: choice::asked(QUESTION, INSTRUCTIONS, criteria),
        offered: options
            .iter()
            .map(|option| (option.id.clone(), option.effort.clone()))
            .collect(),
    })
}

impl ModelAsk {
    /// What `answers` says, read against the models offered and `abstain`.
    ///
    /// # Errors
    /// The first rule the answer broke.
    pub fn read(&self, answers: &Value) -> Result<ModelPick, choice::ChoiceRefusal> {
        let words: BTreeSet<String> = self
            .offered
            .iter()
            .map(|(model, _)| model.clone())
            .chain([ABSTAIN.to_string()])
            .collect();
        let read = choice::read(answers, QUESTION, &words)?;
        Ok(ModelPick {
            chosen: self
                .offered
                .iter()
                .find(|(model, _)| *model == read.chosen)
                .cloned(),
            probabilities: read.probabilities,
            confidence: read.confidence,
        })
    }

    /// The models this question offered, each at its effort.
    #[must_use]
    pub fn offered(&self) -> &[(String, String)] {
        &self.offered
    }
}

/// The words that define the question, for the version's fingerprint.
#[must_use]
pub fn rubric_words() -> String {
    [
        INSTRUCTIONS,
        OPTION_MEANS,
        ABSTAIN_MEANS,
        &STATE_KEYS.join(","),
        &MODEL_KEYS.join(","),
    ]
    .join("\n")
}

/// The model a summons at `row`'s difficulty tries instead of its default
/// when it is a challenger's turn — the `summonses`-th of this agent's here,
/// one in [`lineup::CHALLENGE_ONE_IN`] — among the row's candidates with
/// fewer than [`lineup::CHALLENGE_MIN_SAMPLES`] ended summonses, the least
/// tried first. Only [`lineup::CHALLENGED`] difficulties take a turn.
#[must_use]
pub fn challenger<'a>(
    row: &'a lineup::Row,
    records: &BTreeMap<String, ModelRecord>,
    summonses: usize,
) -> Option<&'a Candidate> {
    if !summonses.is_multiple_of(lineup::CHALLENGE_ONE_IN)
        || !lineup::CHALLENGED.contains(&row.difficulty)
    {
        return None;
    }
    let ended = |model: &str| records.get(model).map_or(0, |record| record.ended);
    row.candidates
        .iter()
        .filter(|candidate| ended(&candidate.model) < lineup::CHALLENGE_MIN_SAMPLES)
        .min_by_key(|candidate| ended(&candidate.model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summon_difficulty::LADDER;
    use crate::summon_difficulty::lineup::{Row, Source};
    use serde_json::json;

    const NOW: i64 = 1_790_000_000_000;

    fn look() -> Look {
        Look {
            title: "task".into(),
            spec: "가".repeat(SPEC_CHAR_CAP + 1),
            attempt: 0,
            failures: 0,
            retry_of: false,
        }
    }

    fn lineup() -> Lineup {
        Lineup::from_catalog(
            &json!({"models": [
                {"provider": "claude", "id": "model-a", "builtin": true,
                 "band": "second", "rungs": ["hard"], "efforts": ["low", "medium", "high", "max"]},
                {"provider": "claude", "id": "model-b", "builtin": false,
                 "band": "rest", "rungs": ["easy", "medium"], "efforts": ["low", "medium", "high"]},
                {"provider": "claude", "id": "model-old", "builtin": true, "band": "superseded"},
            ], "aliases": [{"alias": "model-alias", "canonical": "model-a"}]}),
            "claude",
        )
        .unwrap()
    }

    fn outcome(
        agent: &str,
        model: &str,
        dispatch: &str,
        first: Option<bool>,
        tokens: i64,
    ) -> Value {
        json!({
            "run": "run-1", "dispatch": dispatch, "requestAt": 1, "agent": agent,
            "executionModel": model,
            crate::summon_difficulty::outcomes::KEY: {
                "firstAttemptSuccess": first, "reworkRounds": u64::from(first == Some(false)),
                "tokens": tokens,
            },
        })
    }

    #[test]
    fn a_record_is_folded_from_the_outcome_rows_already_kept() {
        let rows = [
            outcome("claude", "model-alias", "dp-1", Some(true), 100),
            outcome("claude", "model-a", "dp-2", Some(false), 300),
            outcome("claude", "model-a", "dp-2", Some(false), 300),
            outcome("claude", "model-a", "dp-3", None, 0),
            outcome("codex", "model-a", "dp-4", Some(true), 1),
        ];
        let held = lineup();
        let records = records(rows.iter(), "claude", |model| {
            held.canonical(model).to_string()
        });
        let a = records.get("model-a");
        assert!(a.is_some(), "the release's record: {records:?}");
        let a = a.unwrap();
        assert_eq!(
            a.summoned, 3,
            "an alias is its release; one summons counted once"
        );
        assert_eq!(a.ended, 2);
        assert_eq!(a.passed_first_try, 1);
        assert!(a.median_tokens.is_some());
        assert!(!records.contains_key("model-alias"));
    }

    #[test]
    fn every_live_model_is_offered_at_the_ladders_effort_inside_what_it_accepts() {
        let mut seen = Seen::default();
        seen.observe("claude", &lineup(), NOW);
        let options = options(
            "claude",
            &lineup(),
            Some(&seen),
            &BTreeMap::new(),
            LADDER[2].2,
            |model| (model == "model-b").then_some((40, "weekly")),
            NOW,
        );
        let ids: Vec<&str> = options.iter().map(|option| option.id.as_str()).collect();
        assert_eq!(
            ids,
            ["model-a", "model-b"],
            "folded models are nobody's choice"
        );
        let b = &options[1];
        assert_eq!(
            b.effort, "high",
            "max is not accepted; the nearest below is"
        );
        assert!(b.fresh, "a discovered model is new");
        assert_eq!(
            (b.quota_spent_percent, b.quota_window),
            (Some(40), Some("weekly"))
        );
        assert!(!options[0].fresh, "a shipped model is not");
    }

    #[test]
    fn the_question_offers_every_model_and_abstain_and_reads_only_those() {
        let options = options(
            "claude",
            &lineup(),
            None,
            &BTreeMap::new(),
            LADDER[1].2,
            |_| None,
            NOW,
        );
        let asked = ask(&look(), &options).expect("two models make a choice");
        let criteria = asked.questions[QUESTION]["criteria"].as_object().unwrap();
        assert_eq!(criteria.len(), options.len() + 1);
        assert!(criteria.contains_key(ABSTAIN));
        assert_eq!(
            asked.state["spec"].as_str().unwrap().chars().count(),
            SPEC_CHAR_CAP,
            "the spec's head only"
        );
        let models = asked.state["models"].as_array().unwrap();
        assert_eq!(models[0].as_object().unwrap().len(), MODEL_KEYS.len());
        let answer = |word: &str| {
            let mut probabilities = Map::new();
            for key in criteria.keys() {
                probabilities.insert(
                    key.clone(),
                    json!(if key == word { 0.9 } else { 0.1 / 2.0 }),
                );
            }
            json!({QUESTION: {
                "type": "choice", "choice": word, "probabilities": probabilities, "confidence": 0.9,
            }})
        };
        let pick = asked.read(&answer("model-b")).unwrap();
        assert_eq!(
            pick.chosen,
            Some(("model-b".to_string(), LADDER[1].2.to_string())),
            "the model and the effort it runs at"
        );
        assert_eq!(
            asked.read(&answer(ABSTAIN)).unwrap().chosen,
            None,
            "abstain is the default"
        );
        let mut outside = answer("model-b");
        outside[QUESTION]["choice"] = json!("model-z");
        assert!(
            asked.read(&outside).is_err(),
            "a model outside the offer is refused"
        );
        assert!(
            ask(&look(), &options[..1]).is_none(),
            "one model is no choice"
        );
    }

    #[test]
    fn the_version_is_pinned_to_the_words() {
        assert_eq!(RUBRIC_VERSION, 1);
        assert_eq!(
            crate::jev::rubric_fingerprint(rubric_words),
            "5980c9992eeabb71"
        );
    }

    #[test]
    fn a_challengers_turn_tries_the_least_tried_new_model_at_easy_and_ordinary_work() {
        let row = |difficulty: &'static str| Row {
            difficulty,
            model: "model-a".into(),
            effort: "medium".into(),
            from: Source::Lineup,
            effort_clamped: None,
            candidates: vec![
                Candidate {
                    model: "model-b".into(),
                    effort: "medium".into(),
                    fresh: true,
                },
                Candidate {
                    model: "model-c".into(),
                    effort: "medium".into(),
                    fresh: true,
                },
            ],
        };
        let mut records = BTreeMap::new();
        records.insert(
            "model-b".to_string(),
            ModelRecord {
                ended: 1,
                ..ModelRecord::default()
            },
        );
        let turn = lineup::CHALLENGE_ONE_IN * 3;
        let low = row(LADDER[0].0);
        assert_eq!(
            challenger(&low, &records, turn).map(|c| c.model.as_str()),
            Some("model-c"),
            "the least tried"
        );
        assert!(
            challenger(&low, &records, turn + 1).is_none(),
            "one in CHALLENGE_ONE_IN"
        );
        assert!(
            challenger(&row(LADDER[2].0), &records, turn).is_none(),
            "hard work takes no turn"
        );
        records.insert(
            "model-c".to_string(),
            ModelRecord {
                ended: lineup::CHALLENGE_MIN_SAMPLES,
                ..ModelRecord::default()
            },
        );
        assert_eq!(
            challenger(&low, &records, turn).map(|c| c.model.as_str()),
            Some("model-b"),
            "a model with a record no longer takes the turn"
        );
        records.insert(
            "model-b".to_string(),
            ModelRecord {
                ended: lineup::CHALLENGE_MIN_SAMPLES,
                ..ModelRecord::default()
            },
        );
        assert!(challenger(&low, &records, turn).is_none());
    }
}
