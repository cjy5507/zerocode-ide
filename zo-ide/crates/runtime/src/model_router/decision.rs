//! A typed routing judgment beside the probe — the pure core of the decision
//! shadow.
//!
//! The probe (`probe.rs`) asks a Fast-tier chat model to write a small JSON
//! verdict. A System One model (`api::SystemOneClient`) answers the same rubric
//! as typed choices instead: for each axis, one of the tokens the rubric
//! offers, a probability for every one of them, and a confidence. This module
//! turns [`ROUTING_RUBRIC`] into those questions, validates an answer against
//! them, and measures any reader against human labels — without I/O, so the
//! shadow executor and the label evaluation share one arithmetic.
//!
//! Nothing here routes. The shadow records the judgment beside the probe's and
//! fusion never reads a [`DecisionVerdict`]
//! (`docs/design/jev-decision-shadow-20260917.md` §3).
//!
//! `confidence` is not a probability of being right: it is a statistic of the
//! returned distribution's shape. It is kept as it came, and the apply stage
//! reads exactly one cut of it ([`ROUTE_TRUST_FLOOR`]): whether the answer is
//! more likely than not, which is the one band that means the same thing
//! whatever the statistic's calibration.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use api::{SystemOneQuestion, SystemOneQuestionKind, SystemOneRequest, SystemOneResponse};
use serde::{Deserialize, Serialize};
use zerocode_core::jev::noul::{self, NoulRefusal};
use zerocode_core::jev::questions::{
    fold_intent, Contrast, ROUTER_INTENT_OTHER, ROUTING_COMPLEXITY_ID, ROUTING_COMPLEXITY_LEVELS,
    ROUTING_COMPLEXITY_QUESTION, ROUTING_FACTS, ROUTING_FACT_RETRY, ROUTING_INTENTS, ROUTING_INTENT_ID,
    ROUTING_INTENT_QUESTION, ROUTING_REASONING, ROUTING_REASONING_ID, ROUTING_REASONING_QUESTION,
    ROUTING_RISK_ID, ROUTING_RISK_LEVELS, ROUTING_RISK_QUESTION, ROUTING_STATE_FACTS, ROUTING_STATE_TASK,
};
use zerocode_core::jev::{Band, ROUTING};

use super::outcome::rate;
use super::policy::{RouteConfidence, RouteTaskComplexity, RouteTaskRisk};
use super::probe::{
    ProbeAssessment, RouteTaskIntent, RubricAxis, COMPLEXITY_AXIS, INTENT_AXIS, RISK_AXIS, ROUTING_RUBRIC,
};

/// How far a choice answer's probabilities may sum from one: rounding in
/// transit, not a second distribution.
pub const PROBABILITY_SUM_TOLERANCE: f64 = 0.01;

/// Equal-width bins the expected calibration error is measured over.
pub const CALIBRATION_BINS: u8 = 10;

/// The rubric axes a typed judgment answers, in rubric order: every axis that
/// judges the task rather than reports on its reader.
pub fn judged_axes() -> impl Iterator<Item = &'static RubricAxis> {
    ROUTING_RUBRIC.iter().filter(|axis| !axis.self_report)
}

/// The questions a typed judgment is asked: one choice per judged axis, its
/// criteria the axis's tokens and what the rubric says they mean. Built once —
/// every call asks the same words.
#[must_use]
pub fn decision_questions() -> &'static BTreeMap<String, SystemOneQuestion> {
    static QUESTIONS: OnceLock<BTreeMap<String, SystemOneQuestion>> = OnceLock::new();
    QUESTIONS.get_or_init(|| {
        judged_axes()
            .map(|axis| {
                let criteria = axis.tokens.iter().map(|token| (*token, axis.description(token)));
                (axis.name.to_string(), SystemOneQuestion::choice(axis.question, criteria))
            })
            .collect()
    })
}

/// The request for one task: `task_text` (already cut by
/// [`super::probe::rubric_task_text`]) as the state, the rubric's questions.
#[must_use]
pub fn decision_request<'a>(model: &'a str, task_text: &'a str) -> SystemOneRequest<'a, str> {
    SystemOneRequest { state: task_text, model, questions: decision_questions() }
}

/// One axis of a validated answer: the choice in the router's own vocabulary,
/// its position among the axis's tokens, and the whole distribution in token
/// order.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionAnswer<T> {
    pub choice: T,
    pub position: usize,
    pub probabilities: Vec<f64>,
    pub confidence: f64,
}

/// A typed judgment that passed every check in [`validate_decision`].
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionVerdict {
    pub complexity: DecisionAnswer<RouteTaskComplexity>,
    pub risk: DecisionAnswer<RouteTaskRisk>,
    pub intent: DecisionAnswer<RouteTaskIntent>,
}

/// The least any axis of a verdict may be sure of before the apply stage lets
/// the verdict move a route: more likely than not.
///
/// Not a calibration claim. The verdict's confidence is a statistic of its
/// distribution's shape, and the only cut of it that means one thing whether
/// or not it is calibrated is a half — the chosen token holding more of the
/// distribution than every other token together. Under it the verdict is a
/// guess the reader itself does not back, and the router's `Low` band is the
/// word for that: trusted for nothing, the deterministic assessment stands.
/// At or over it the verdict has the `Medium` authority every applied verdict
/// had before this cut — intent read, risk raised but never lowered,
/// complexity raised by at most one band — because that authority was chosen
/// for a reader whose confidence is not accuracy, and that has not changed.
///
/// Measured on this machine's 25 answered routing rows (2026-09-20): the
/// judgment agreed with the chat probe on 53% of all axes and on 77% of the
/// axes it was at least this sure of; the rows it was under it on include a
/// `large` at 0.07 the probe called `trivial`, which the old unconditional
/// `Medium` would have raised a route on.
pub const ROUTE_TRUST_FLOOR: f64 = 0.5;

impl DecisionVerdict {
    /// The authority the apply stage hands this verdict: `Medium` when every
    /// axis is at least [`ROUTE_TRUST_FLOOR`] sure, `Low` otherwise. Never
    /// `High`: lowering a route's complexity is the one move a verdict whose
    /// confidence is not accuracy is not given.
    #[must_use]
    pub fn route_confidence(&self) -> RouteConfidence {
        let sure = self
            .readings()
            .iter()
            .all(|reading| reading.confidence >= ROUTE_TRUST_FLOOR);
        if sure { RouteConfidence::Medium } else { RouteConfidence::Low }
    }
}

/// One axis of a verdict without its enum — what a ledger writes and what the
/// metrics read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisReading<'a> {
    pub axis: &'static RubricAxis,
    pub position: usize,
    pub probabilities: &'a [f64],
    pub confidence: f64,
}

impl DecisionVerdict {
    /// Every axis the verdict answers, in the order it was validated.
    #[must_use]
    pub fn readings(&self) -> [AxisReading<'_>; 3] {
        fn reading<'a, T>(axis: &'static RubricAxis, answer: &'a DecisionAnswer<T>) -> AxisReading<'a> {
            AxisReading {
                axis,
                position: answer.position,
                probabilities: &answer.probabilities,
                confidence: answer.confidence,
            }
        }
        [
            reading(&COMPLEXITY_AXIS, &self.complexity),
            reading(&RISK_AXIS, &self.risk),
            reading(&INTENT_AXIS, &self.intent),
        ]
    }
}

/// Why an answer was thrown away, naming the axis. Every one of these is the
/// ledger's `schema` failure: an answer that breaks any rule is discarded
/// whole, never partly used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionRejection {
    /// An asked question has no answer.
    MissingAnswer(&'static str),
    /// The answer is not shaped as a choice answer.
    NotAChoice(&'static str),
    /// `choice` is not one of the tokens the question offered.
    ChoiceOutsideCriteria(&'static str),
    /// `probabilities` does not name exactly the offered tokens.
    ProbabilityKeys(&'static str),
    /// A probability outside `[0, 1]`.
    ProbabilityRange(&'static str),
    /// The probabilities do not sum to one within [`PROBABILITY_SUM_TOLERANCE`].
    ProbabilitySum(&'static str),
    /// `confidence` outside `[0, 1]`.
    ConfidenceRange(&'static str),
    /// An asked Score has an answer that is not score-shaped.
    NotAScore(&'static str),
    /// A score's position is not on its own levels.
    ScoreOutsideLevels(&'static str),
    /// A fact's answer is not a Noul in `[0, 1]`, by the Noul's own rule.
    Noul(&'static str, NoulRefusal),
}

/// Check a response against the questions [`decision_questions`] asked and
/// read it into the router's vocabulary.
///
/// # Errors
/// The first rule the answer breaks.
pub fn validate_decision(response: &SystemOneResponse) -> Result<DecisionVerdict, DecisionRejection> {
    Ok(DecisionVerdict {
        complexity: read_answer(response, &COMPLEXITY_AXIS, RouteTaskComplexity::from_label)?,
        risk: read_answer(response, &RISK_AXIS, RouteTaskRisk::from_label)?,
        intent: read_answer(response, &INTENT_AXIS, RouteTaskIntent::from_label)?,
    })
}

/// One axis's answer, checked against the question that axis asked.
fn read_answer<T>(
    response: &SystemOneResponse,
    axis: &RubricAxis,
    read: fn(&str) -> Option<T>,
) -> Result<DecisionAnswer<T>, DecisionRejection> {
    let name = axis.name;
    let answer = response
        .choice_answer(name)
        .ok_or(DecisionRejection::MissingAnswer(name))?
        .ok()
        .filter(|answer| answer.kind == SystemOneQuestionKind::Choice)
        .ok_or(DecisionRejection::NotAChoice(name))?;
    let position = axis
        .position(&answer.choice)
        .ok_or(DecisionRejection::ChoiceOutsideCriteria(name))?;
    let choice = read(axis.tokens[position]).ok_or(DecisionRejection::ChoiceOutsideCriteria(name))?;
    if answer.probabilities.len() != axis.tokens.len() {
        return Err(DecisionRejection::ProbabilityKeys(name));
    }
    let probabilities = axis
        .tokens
        .iter()
        .map(|token| answer.probabilities.get(*token).copied().ok_or(DecisionRejection::ProbabilityKeys(name)))
        .collect::<Result<Vec<f64>, _>>()?;
    if !probabilities.iter().all(|probability| (0.0..=1.0).contains(probability)) {
        return Err(DecisionRejection::ProbabilityRange(name));
    }
    if (probabilities.iter().sum::<f64>() - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
        return Err(DecisionRejection::ProbabilitySum(name));
    }
    if !(0.0..=1.0).contains(&answer.confidence) {
        return Err(DecisionRejection::ConfidenceRange(name));
    }
    Ok(DecisionAnswer { choice, position, probabilities, confidence: answer.confidence })
}

// ---- the routing seat's second version (t-6346) ---------------------------
//
// Version 1 asked the probe's rubric as three Choices (above, and still what
// the step governor asks). The routing seat now asks the core catalog's
// questions (`zerocode_core::jev::questions`) — two Scores, two contrastive
// Choices and the facts — in one request, and code turns the answers into
// the router's words and the authority they route with. Nothing here asks a
// model what to route to: every question is a fact about the task.

/// The routing seat's questions, built once from the core catalog: every
/// call asks the same words, and those words are the catalog's alone.
#[must_use]
pub fn routing_questions() -> &'static BTreeMap<String, SystemOneQuestion> {
    static QUESTIONS: OnceLock<BTreeMap<String, SystemOneQuestion>> = OnceLock::new();
    QUESTIONS.get_or_init(|| {
        let contrasted = |question: &str, options: &mut dyn Iterator<Item = &'static Contrast>| {
            SystemOneQuestion::contrastive_choice(
                question,
                options.map(|option| (option.word, option.what, option.not_for, option.examples)),
            )
        };
        let mut asked = BTreeMap::from([
            (
                ROUTING_COMPLEXITY_ID.to_string(),
                SystemOneQuestion::score(ROUTING_COMPLEXITY_QUESTION, ROUTING_COMPLEXITY_LEVELS),
            ),
            (ROUTING_RISK_ID.to_string(), SystemOneQuestion::score(ROUTING_RISK_QUESTION, ROUTING_RISK_LEVELS)),
            (
                ROUTING_INTENT_ID.to_string(),
                contrasted(ROUTING_INTENT_QUESTION, &mut ROUTING_INTENTS.iter().map(|intent| &intent.option)),
            ),
            (
                ROUTING_REASONING_ID.to_string(),
                contrasted(ROUTING_REASONING_QUESTION, &mut ROUTING_REASONING.iter()),
            ),
        ]);
        asked.extend(ROUTING_FACTS.iter().map(|fact| {
            (fact.id.to_string(), SystemOneQuestion::noul(fact.instructions, fact.yes, fact.no))
        }));
        asked
    })
}

/// What code knows about a task beside its words — the facts the state
/// carries and a question reads by path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RoutingFacts {
    /// An earlier attempt at this same task already failed (a spawn's
    /// `prior_failures`). A person's turn is never marked: nothing says which
    /// earlier turn it retries.
    pub retry_of_failed_attempt: bool,
}

/// The routing seat's state for one task: the task as it stands — the door
/// withholds and cuts it by its own pointer — and the facts, under the
/// catalog's keys.
#[must_use]
pub fn routing_state(task: &str, facts: RoutingFacts) -> serde_json::Value {
    serde_json::json!({
        ROUTING_STATE_TASK: task,
        ROUTING_STATE_FACTS: { ROUTING_FACT_RETRY: facts.retry_of_failed_attempt },
    })
}

/// The routing seat's request for one task's state.
#[must_use]
pub fn routing_request<'a>(model: &'a str, state: &'a serde_json::Value) -> SystemOneRequest<'a, serde_json::Value> {
    SystemOneRequest { state, model, questions: routing_questions() }
}

/// A Score answer read along its levels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelReading {
    /// The position along the levels, which can fall between two.
    pub score: f64,
    /// The level the position rounds to — the one a route reads (the vendor's
    /// guide rounds a score; no line fitted to one version sits here).
    pub level: usize,
    /// Every level's probability, the low end first.
    pub probabilities: Vec<f64>,
    pub confidence: f64,
}

/// A Choice answer read on the options it was offered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionReading {
    pub chosen: String,
    /// Every offered option's probability, by option.
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// Every answer of one routing judgment as it came — what the ledger keeps
/// for a later reader to weigh (t-6324 P7), and what code reads the route
/// from ([`Self::verdict`], [`Self::assessment`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingReading {
    pub complexity: LevelReading,
    pub risk: LevelReading,
    pub intent: OptionReading,
    pub reasoning: OptionReading,
    /// Each fact's probability of yes, by the catalog's id.
    pub facts: BTreeMap<String, f64>,
}

/// Check a response against the questions [`routing_questions`] asked, and
/// read every answer. An answer that breaks any rule is discarded whole.
///
/// # Errors
/// The first rule the answer breaks, in the catalog's order.
pub fn validate_routing(response: &SystemOneResponse) -> Result<RoutingReading, DecisionRejection> {
    let answers = serde_json::Value::Object(response.answers.clone().into_iter().collect());
    let intents: Vec<&'static str> = ROUTING_INTENTS.iter().map(|intent| intent.option.word).collect();
    let kinds: Vec<&'static str> = ROUTING_REASONING.iter().map(|kind| kind.word).collect();
    Ok(RoutingReading {
        complexity: read_level(response, ROUTING_COMPLEXITY_ID, ROUTING_COMPLEXITY_LEVELS.len())?,
        risk: read_level(response, ROUTING_RISK_ID, ROUTING_RISK_LEVELS.len())?,
        intent: read_option(response, ROUTING_INTENT_ID, &intents)?,
        reasoning: read_option(response, ROUTING_REASONING_ID, &kinds)?,
        facts: ROUTING_FACTS
            .iter()
            .map(|fact| {
                noul::read(&answers, fact.id)
                    .map(|yes| (fact.id.to_string(), yes))
                    .map_err(|refusal| DecisionRejection::Noul(fact.id, refusal))
            })
            .collect::<Result<_, _>>()?,
    })
}

/// One Score answer, checked against the `levels` it was offered.
fn read_level(response: &SystemOneResponse, id: &'static str, levels: usize) -> Result<LevelReading, DecisionRejection> {
    let answer = response
        .score_answer(id)
        .ok_or(DecisionRejection::MissingAnswer(id))?
        .ok()
        .filter(|answer| answer.kind == SystemOneQuestionKind::Score)
        .ok_or(DecisionRejection::NotAScore(id))?;
    if answer.probabilities.len() != levels {
        return Err(DecisionRejection::ProbabilityKeys(id));
    }
    let probabilities = (0..levels)
        .map(|level| answer.probabilities.get(&level.to_string()).copied().ok_or(DecisionRejection::ProbabilityKeys(id)))
        .collect::<Result<Vec<f64>, _>>()?;
    check_distribution(id, &probabilities, answer.confidence)?;
    #[allow(clippy::cast_precision_loss)]
    let last = (levels - 1) as f64;
    if !(0.0..=last).contains(&answer.score) {
        return Err(DecisionRejection::ScoreOutsideLevels(id));
    }
    // In `0.0..=last` after the check above, so the cast neither truncates a
    // meaningful part nor loses a sign.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let level = answer.score.round() as usize;
    Ok(LevelReading { score: answer.score, level, probabilities, confidence: answer.confidence })
}

/// One Choice answer, checked against the `options` it was offered.
fn read_option(
    response: &SystemOneResponse,
    id: &'static str,
    options: &[&'static str],
) -> Result<OptionReading, DecisionRejection> {
    let answer = response
        .choice_answer(id)
        .ok_or(DecisionRejection::MissingAnswer(id))?
        .ok()
        .filter(|answer| answer.kind == SystemOneQuestionKind::Choice)
        .ok_or(DecisionRejection::NotAChoice(id))?;
    if !options.contains(&answer.choice.as_str()) {
        return Err(DecisionRejection::ChoiceOutsideCriteria(id));
    }
    if answer.probabilities.len() != options.len()
        || !options.iter().all(|option| answer.probabilities.contains_key(*option))
    {
        return Err(DecisionRejection::ProbabilityKeys(id));
    }
    let shares: Vec<f64> = answer.probabilities.values().copied().collect();
    check_distribution(id, &shares, answer.confidence)?;
    Ok(OptionReading { chosen: answer.choice, probabilities: answer.probabilities, confidence: answer.confidence })
}

/// A distribution's rules, one place for both primitives: every share in
/// `[0, 1]`, the sum one within the wire's rounding (the core's tolerance for
/// that many options), and the confidence in `[0, 1]`.
fn check_distribution(id: &'static str, shares: &[f64], confidence: f64) -> Result<(), DecisionRejection> {
    if !shares.iter().all(|share| (0.0..=1.0).contains(share)) {
        return Err(DecisionRejection::ProbabilityRange(id));
    }
    let tolerance = zerocode_core::jev::choice::probability_sum_tolerance(shares.len());
    if (shares.iter().sum::<f64>() - 1.0).abs() > tolerance {
        return Err(DecisionRejection::ProbabilitySum(id));
    }
    if !(0.0..=1.0).contains(&confidence) {
        return Err(DecisionRejection::ConfidenceRange(id));
    }
    Ok(())
}

impl RoutingReading {
    /// The judgment in the router's words, as answered: complexity and risk
    /// at the level each rounds to, the intent folded into the router's four
    /// with its probabilities. What a ledger lays beside the probe's answer.
    #[must_use]
    pub fn verdict(&self) -> DecisionVerdict {
        DecisionVerdict {
            complexity: level_answer(&COMPLEXITY_AXIS, &self.complexity, RouteTaskComplexity::from_label)
                .unwrap_or(RouteTaskComplexity::Unknown),
            risk: level_answer(&RISK_AXIS, &self.risk, RouteTaskRisk::from_label).unwrap_or(RouteTaskRisk::Unknown),
            intent: self.folded_intent(),
        }
    }

    /// The ten intents' probabilities summed into the router's four, and the
    /// router word that holds the most — a judgment torn between reviewing
    /// and investigating is sure it is analysis.
    fn folded_intent(&self) -> DecisionAnswer<RouteTaskIntent> {
        let mut probabilities = vec![0.0; INTENT_AXIS.tokens.len()];
        for intent in &ROUTING_INTENTS {
            if let (Some(position), Some(share)) =
                (INTENT_AXIS.position(intent.folds_to), self.intent.probabilities.get(intent.option.word))
            {
                probabilities[position] += share;
            }
        }
        // From the chosen option's own fold, moved only by a strictly larger
        // share, so a tie keeps what the judgment chose.
        let chosen = fold_intent(&self.intent.chosen).unwrap_or(ROUTER_INTENT_OTHER);
        let mut position = INTENT_AXIS.position(chosen).unwrap_or_default();
        for (candidate, share) in probabilities.iter().enumerate() {
            if *share > probabilities[position] {
                position = candidate;
            }
        }
        DecisionAnswer {
            choice: RouteTaskIntent::from_label(INTENT_AXIS.tokens[position]).unwrap_or_default(),
            position,
            probabilities,
            confidence: self.intent.confidence,
        }
    }

    /// The band the complexity answer's own confidence falls in — the seat's
    /// lines ([`zerocode_core::jev::ConfidenceBands::ROUTED`]), read on the
    /// axis the route moves. A reading outside `[0, 1]` never passed its
    /// checks, and abstains.
    #[must_use]
    pub fn band(&self) -> Band {
        ROUTING.band_of(self.complexity.confidence).unwrap_or(Band::Abstain)
    }

    /// What the apply stage routes on, or `None` when the judgment abstains
    /// and the chat probe is to be asked instead (t-6346: the probe only in
    /// the abstain band).
    ///
    /// The band is the authority, in the words the router's fusion already
    /// reads for the probe: acting alone is `High` (one band either way),
    /// wanting a confirmation is `Medium` (up only — the keyword tables are
    /// the confirmation a lower band does not get). Risk and intent speak
    /// only past their own abstain line: a timid risk raises nothing
    /// (`Unknown`), a timid intent is the router's neutral `Other`.
    #[must_use]
    pub fn assessment(&self) -> Option<ProbeAssessment> {
        let confidence = match self.band() {
            Band::Act => RouteConfidence::High,
            Band::Confirm => RouteConfidence::Medium,
            Band::Abstain => return None,
        };
        let speaks = |confidence: f64| ROUTING.band_of(confidence).is_some_and(|band| band != Band::Abstain);
        let verdict = self.verdict();
        Some(ProbeAssessment {
            complexity: verdict.complexity.choice,
            risk: if speaks(self.risk.confidence) { verdict.risk.choice } else { RouteTaskRisk::Unknown },
            confidence,
            intent: if speaks(self.intent.confidence) { verdict.intent.choice } else { RouteTaskIntent::Other },
        })
    }

    /// Whether the fact `id` reads true on its own yes line
    /// ([`zerocode_core::jev::questions::Fact::yes_from_permille`]); `None`
    /// for a fact the seat never asked.
    #[must_use]
    pub fn holds(&self, id: &str) -> Option<bool> {
        let fact = ROUTING_FACTS.iter().find(|fact| fact.id == id)?;
        let yes = self.facts.get(id)?;
        Some(*yes >= f64::from(fact.yes_from_permille) / 1_000.0)
    }
}

/// One Score reading in an ordered axis's words: the token at its level.
fn level_answer<T>(
    axis: &RubricAxis,
    reading: &LevelReading,
    read: fn(&str) -> Option<T>,
) -> DecisionAnswer<Option<T>> {
    DecisionAnswer {
        choice: axis.tokens.get(reading.level).copied().and_then(read),
        position: reading.level,
        probabilities: reading.probabilities.clone(),
        confidence: reading.confidence,
    }
}

impl<T> DecisionAnswer<Option<T>> {
    /// The answer with `absent` where its token did not read.
    fn unwrap_or(self, absent: T) -> DecisionAnswer<T> {
        DecisionAnswer {
            choice: self.choice.unwrap_or(absent),
            position: self.position,
            probabilities: self.probabilities,
            confidence: self.confidence,
        }
    }
}

/// One labelled judgment on one axis, as positions in the axis's token order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisSample<'a> {
    pub label: usize,
    pub predicted: usize,
    /// The reader's whole distribution in token order, when it gives one. A
    /// typed judgment does; the probe states a single token.
    pub probabilities: Option<&'a [f64]>,
}

/// How one reader did on one axis against the labels.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisMetrics {
    pub samples: usize,
    pub correct: usize,
    /// `confusion[label][predicted]`, in the axis's token order.
    pub confusion: Vec<Vec<usize>>,
    /// Predictions below their label on an ordered axis — a risk
    /// under-estimated, a task called easier than it is. `None` on an axis
    /// whose tokens are not a scale.
    pub below_label: Option<usize>,
    /// Samples that carried a distribution: the population of the two scores
    /// below.
    pub scored: usize,
    /// Mean multi-class Brier score over the scored samples.
    pub brier: Option<f64>,
    /// Expected calibration error of the predicted token's probability over
    /// [`CALIBRATION_BINS`] equal-width bins, over the scored samples.
    pub calibration_error: Option<f64>,
}

impl AxisMetrics {
    #[must_use]
    pub fn accuracy(&self) -> Option<f64> {
        rate(self.correct, self.samples)
    }

    #[must_use]
    pub fn below_label_rate(&self) -> Option<f64> {
        self.below_label.and_then(|below| rate(below, self.samples))
    }
}

/// Measure one reader on one axis. The probe and a typed judgment go through
/// this same function; the probe simply carries no distribution, so its Brier
/// score and calibration error stay `None`.
///
/// A sample whose positions or distribution do not fit the axis is not
/// evidence about it and is left out.
#[must_use]
pub fn axis_metrics(axis: &RubricAxis, samples: &[AxisSample<'_>]) -> AxisMetrics {
    let classes = axis.tokens.len();
    let fits = |sample: &&AxisSample<'_>| {
        sample.label < classes
            && sample.predicted < classes
            && sample.probabilities.is_none_or(|distribution| distribution.len() == classes)
    };
    let mut metrics = AxisMetrics {
        samples: 0,
        correct: 0,
        confusion: vec![vec![0; classes]; classes],
        below_label: axis.ordered.then_some(0),
        scored: 0,
        brier: None,
        calibration_error: None,
    };
    let mut brier_sum = 0.0;
    // Per bin: how many predictions came out right, and the probability they
    // were stated with, both summed. A bin's weighted gap
    // `(n_b / n) · |right_b / n_b − stated_b / n_b|` is `|right_b − stated_b| / n`,
    // so the sums are all the error needs.
    let mut bins = vec![(0.0f64, 0.0f64); usize::from(CALIBRATION_BINS)];
    for sample in samples.iter().filter(fits) {
        let correct = sample.label == sample.predicted;
        metrics.samples += 1;
        metrics.correct += usize::from(correct);
        metrics.confusion[sample.label][sample.predicted] += 1;
        if let Some(below) = metrics.below_label.as_mut() {
            *below += usize::from(sample.predicted < sample.label);
        }
        let Some(distribution) = sample.probabilities else {
            continue;
        };
        metrics.scored += 1;
        brier_sum += distribution
            .iter()
            .enumerate()
            .map(|(position, probability)| (probability - f64::from(position == sample.label)).powi(2))
            .sum::<f64>();
        let stated = distribution[sample.predicted];
        let bin = &mut bins[calibration_bin(stated)];
        bin.0 += f64::from(correct);
        bin.1 += stated;
    }
    let per_scored = rate(1, metrics.scored);
    metrics.brier = per_scored.map(|share| brier_sum * share);
    metrics.calibration_error =
        per_scored.map(|share| bins.iter().map(|(right, stated)| (right - stated).abs()).sum::<f64>() * share);
    metrics
}

/// The equal-width bin a probability falls in; `1.0` belongs to the last.
fn calibration_bin(probability: f64) -> usize {
    let bins = f64::from(CALIBRATION_BINS);
    // In `0.0..=bins` after the clamp, so the cast neither truncates a
    // meaningful part nor loses a sign.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bin = (probability.clamp(0.0, 1.0) * bins).floor() as usize;
    bin.min(usize::from(CALIBRATION_BINS) - 1)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::super::probe::{
        probe_prompt, rubric_task_text, CONFIDENCE_AXIS, DECISION_RUBRIC_VERSION, RUBRIC_TASK_CHAR_CAP,
    };
    use super::*;

    fn response(answers: serde_json::Value) -> SystemOneResponse {
        let mut body = json!({ "model": "jev-latest", "usage": { "input_tokens": 300, "output_tokens": 0 } });
        body["answers"] = answers;
        serde_json::from_value(body).expect("a contract-shaped response")
    }

    fn choice(token: &str, probabilities: serde_json::Value, confidence: f64) -> serde_json::Value {
        let mut answer = json!({ "type": "choice", "choice": token, "confidence": confidence });
        answer["probabilities"] = probabilities;
        answer
    }

    fn valid_answers() -> serde_json::Value {
        json!({
            "complexity": choice("large", json!({"trivial": 0.05, "small": 0.05, "medium": 0.2, "large": 0.7}), 0.64),
            "risk": choice("high", json!({"low": 0.1, "medium": 0.2, "high": 0.6, "critical": 0.1}), 0.41),
            "intent": choice("design", json!({"design": 0.9, "implementation": 0.05, "analysis": 0.03, "other": 0.02}), 0.88),
        })
    }

    #[test]
    fn the_questions_are_the_rubric_axes_that_judge_the_task() {
        let questions = decision_questions();
        let asked: Vec<&str> = questions.keys().map(String::as_str).collect();
        let mut expected: Vec<&str> = judged_axes().map(|axis| axis.name).collect();
        expected.sort_unstable();
        assert_eq!(
            ROUTING_RUBRIC.iter().filter(|axis| axis.self_report).count() + expected.len(),
            ROUTING_RUBRIC.len(),
            "every axis is either judged or a self-report"
        );
        assert_eq!(asked, expected);
        assert!(!questions.contains_key(CONFIDENCE_AXIS.name), "a typed judgment reports a distribution instead");
        for axis in judged_axes() {
            let question = &questions[axis.name];
            assert_eq!(question.kind, SystemOneQuestionKind::Choice);
            assert_eq!(question.instructions, axis.question);
            let api::SystemOneCriteria::Named(criteria) = &question.criteria else {
                panic!("a judged axis offers named options: {}", axis.name);
            };
            let keys: Vec<&str> = criteria.keys().map(String::as_str).collect();
            let mut tokens = axis.tokens.to_vec();
            tokens.sort_unstable();
            assert_eq!(keys, tokens, "criteria keys are the axis tokens: {}", axis.name);
            for token in axis.tokens {
                assert_eq!(criteria[*token].as_deref(), axis.description(token), "{}:{token}", axis.name);
            }
        }
        // A verdict reads back exactly the axes that were asked.
        let verdict = validate_decision(&response(valid_answers())).expect("valid");
        let mut read: Vec<&str> = verdict.readings().iter().map(|reading| reading.axis.name).collect();
        read.sort_unstable();
        assert_eq!(read, expected);
        for reading in verdict.readings() {
            assert_eq!(reading.probabilities.len(), reading.axis.tokens.len(), "{}", reading.axis.name);
        }
        let request = decision_request("jev-latest", "state");
        assert_eq!(request.state, "state");
        assert!(std::ptr::eq(request.questions, questions));
    }

    /// The rubric's words are pinned to [`DECISION_RUBRIC_VERSION`]: change a
    /// token, a description, a question, the probe framing or the task cut,
    /// and this goes red until the version is bumped and the digest re-pinned.
    #[test]
    fn the_rubric_version_names_these_exact_words() {
        let mut hasher = Sha256::new();
        hasher.update(probe_prompt("", "").as_bytes());
        hasher.update(serde_json::to_vec(decision_questions()).expect("questions serialize"));
        hasher.update(rubric_task_text("description", "prompt").as_bytes());
        hasher.update(RUBRIC_TASK_CHAR_CAP.to_le_bytes());
        let digest = format!("{:x}", hasher.finalize());
        assert_eq!(
            (DECISION_RUBRIC_VERSION, &digest[..16]),
            (1, "3163e6fbd84adff6"),
            "the rubric's words changed: bump DECISION_RUBRIC_VERSION and re-pin this digest"
        );
    }

    #[test]
    fn a_verdict_moves_a_route_only_when_every_axis_is_more_likely_than_not() {
        let sure = validate_decision(&response(valid_answers())).expect("valid");
        assert!((sure.risk.confidence - 0.41).abs() < f64::EPSILON, "the fixture's least sure axis");
        assert_eq!(sure.route_confidence(), RouteConfidence::Low, "one axis under a half is a guess");
        let mut answers = valid_answers();
        answers["risk"] = choice("high", json!({"low": 0.1, "medium": 0.2, "high": 0.6, "critical": 0.1}), 0.5);
        let at_the_line = validate_decision(&response(answers)).expect("valid");
        assert_eq!(at_the_line.route_confidence(), RouteConfidence::Medium, "at the line is over it");
        let mut answers = valid_answers();
        for axis in ["complexity", "risk", "intent"] {
            answers[axis]["confidence"] = json!(0.99);
        }
        assert_eq!(
            validate_decision(&response(answers)).expect("valid").route_confidence(),
            RouteConfidence::Medium,
            "never High: a verdict is not given the move that lowers a route"
        );
    }

    #[test]
    fn a_valid_answer_reads_into_the_routers_vocabulary() {
        let verdict = validate_decision(&response(valid_answers())).expect("a valid answer");
        assert_eq!(verdict.complexity.choice, RouteTaskComplexity::Large);
        assert_eq!(verdict.complexity.position, 3);
        assert_eq!(verdict.complexity.probabilities, vec![0.05, 0.05, 0.2, 0.7]);
        assert!((verdict.complexity.confidence - 0.64).abs() < f64::EPSILON);
        assert_eq!(verdict.risk.choice, RouteTaskRisk::High);
        assert_eq!(verdict.risk.position, 2);
        assert_eq!(verdict.intent.choice, RouteTaskIntent::Design);
        assert_eq!(verdict.intent.probabilities, vec![0.9, 0.05, 0.03, 0.02]);
        // Rounding in transit is not a broken distribution.
        let mut rounded = valid_answers();
        rounded["risk"]["probabilities"] = json!({"low": 0.1, "medium": 0.2, "high": 0.6, "critical": 0.105});
        assert!(validate_decision(&response(rounded)).is_ok());
        // An extra answer nobody asked for is not read, and not a failure.
        let mut extra = valid_answers();
        extra["mood"] = json!({"type": "choice", "choice": "calm"});
        assert!(validate_decision(&response(extra)).is_ok());
    }

    #[test]
    fn every_broken_rule_discards_the_whole_answer() {
        let cases: Vec<(&str, serde_json::Value, DecisionRejection)> = vec![
            ("missing", json!(null), DecisionRejection::MissingAnswer("risk")),
            ("score answer", json!({"type": "score", "score": 3.2, "confidence": 0.5}), DecisionRejection::NotAChoice("risk")),
            ("no type", json!({"choice": "high", "probabilities": {"low": 0.1, "medium": 0.2, "high": 0.6, "critical": 0.1}, "confidence": 0.4}), DecisionRejection::NotAChoice("risk")),
            ("string probability", choice("high", json!({"low": "0.1", "medium": 0.2, "high": 0.6, "critical": 0.1}), 0.4), DecisionRejection::NotAChoice("risk")),
            ("choice outside criteria", choice("severe", json!({"low": 0.1, "medium": 0.2, "high": 0.6, "critical": 0.1}), 0.4), DecisionRejection::ChoiceOutsideCriteria("risk")),
            ("choice in another case", choice("HIGH", json!({"low": 0.1, "medium": 0.2, "high": 0.6, "critical": 0.1}), 0.4), DecisionRejection::ChoiceOutsideCriteria("risk")),
            ("a key missing", choice("high", json!({"low": 0.2, "medium": 0.2, "high": 0.6}), 0.4), DecisionRejection::ProbabilityKeys("risk")),
            ("a key extra", choice("high", json!({"low": 0.1, "medium": 0.2, "high": 0.6, "critical": 0.1, "severe": 0.0}), 0.4), DecisionRejection::ProbabilityKeys("risk")),
            ("a key renamed", choice("high", json!({"low": 0.1, "medium": 0.2, "high": 0.6, "severe": 0.1}), 0.4), DecisionRejection::ProbabilityKeys("risk")),
            ("negative probability", choice("high", json!({"low": -0.1, "medium": 0.3, "high": 0.7, "critical": 0.1}), 0.4), DecisionRejection::ProbabilityRange("risk")),
            ("probability above one", choice("high", json!({"low": 0.0, "medium": 0.0, "high": 1.2, "critical": -0.2}), 0.4), DecisionRejection::ProbabilityRange("risk")),
            ("sum short of one", choice("high", json!({"low": 0.1, "medium": 0.1, "high": 0.6, "critical": 0.1}), 0.4), DecisionRejection::ProbabilitySum("risk")),
            ("confidence above one", choice("high", json!({"low": 0.1, "medium": 0.2, "high": 0.6, "critical": 0.1}), 1.5), DecisionRejection::ConfidenceRange("risk")),
            ("confidence below zero", choice("high", json!({"low": 0.1, "medium": 0.2, "high": 0.6, "critical": 0.1}), -0.01), DecisionRejection::ConfidenceRange("risk")),
        ];
        for (case, risk, rejection) in cases {
            let mut answers = valid_answers();
            if risk.is_null() {
                answers.as_object_mut().expect("an object").remove("risk");
            } else {
                answers["risk"] = risk;
            }
            assert_eq!(validate_decision(&response(answers)), Err(rejection), "{case}");
        }
    }

    fn close(actual: Option<f64>, expected: f64) -> bool {
        actual.is_some_and(|actual| (actual - expected).abs() < 1e-9)
    }

    #[test]
    fn the_metrics_of_a_known_distribution() {
        // Complexity tokens: trivial, small, medium, large.
        let sure_right = [0.0, 0.0, 0.0, 1.0];
        let split = [0.0, 0.0, 0.5, 0.5];
        let sure_wrong = [1.0, 0.0, 0.0, 0.0];
        let samples = [
            // Right and certain: Brier 0; bin 9 (p = 1.0), correct.
            AxisSample { label: 3, predicted: 3, probabilities: Some(&sure_right) },
            // Right at a coin flip: Brier 0.5; bin 5 (p = 0.5), correct.
            AxisSample { label: 2, predicted: 2, probabilities: Some(&split) },
            // Wrong and certain, and BELOW the label: Brier 2; bin 9, wrong.
            AxisSample { label: 3, predicted: 0, probabilities: Some(&sure_wrong) },
            // A reader with no distribution, above its label.
            AxisSample { label: 1, predicted: 2, probabilities: None },
        ];
        let metrics = axis_metrics(&COMPLEXITY_AXIS, &samples);
        assert_eq!(metrics.samples, 4);
        assert_eq!(metrics.correct, 2);
        assert!(close(metrics.accuracy(), 0.5));
        assert_eq!(metrics.confusion[3][3], 1);
        assert_eq!(metrics.confusion[2][2], 1);
        assert_eq!(metrics.confusion[3][0], 1);
        assert_eq!(metrics.confusion[1][2], 1);
        assert_eq!(metrics.confusion.iter().flatten().sum::<usize>(), 4);
        assert_eq!(metrics.below_label, Some(1), "only the certain miss sits below its label");
        assert!(close(metrics.below_label_rate(), 0.25));
        assert_eq!(metrics.scored, 3);
        // (0 + 0.5 + 2) / 3.
        assert!(close(metrics.brier, 2.5 / 3.0), "{:?}", metrics.brier);
        // Bin 9: two samples at p = 1.0, one right → |0.5 − 1.0| × 2/3.
        // Bin 5: one sample at p = 0.5, right → |1.0 − 0.5| × 1/3.
        assert!(close(metrics.calibration_error, 0.5), "{:?}", metrics.calibration_error);
    }

    #[test]
    fn a_reader_without_distributions_scores_accuracy_only_and_an_unordered_axis_has_no_below() {
        let samples = [
            AxisSample { label: 0, predicted: 0, probabilities: None },
            AxisSample { label: 1, predicted: 0, probabilities: None },
        ];
        let intent = axis_metrics(&INTENT_AXIS, &samples);
        assert_eq!(intent.samples, 2);
        assert!(close(intent.accuracy(), 0.5));
        assert_eq!(intent.below_label, None, "intent is not a scale");
        assert_eq!(intent.below_label_rate(), None);
        assert_eq!((intent.scored, intent.brier, intent.calibration_error), (0, None, None));
        let risk = axis_metrics(&RISK_AXIS, &samples);
        assert_eq!(risk.below_label, Some(1), "calling a medium risk low is an under-estimate");
        let empty = axis_metrics(&RISK_AXIS, &[]);
        assert_eq!((empty.accuracy(), empty.below_label_rate()), (None, None));
        // A sample whose positions or distribution do not fit the axis is not
        // evidence about it.
        let short = [0.5, 0.5];
        let unfit = [
            AxisSample { label: 9, predicted: 0, probabilities: None },
            AxisSample { label: 0, predicted: 0, probabilities: Some(&short) },
        ];
        assert_eq!(axis_metrics(&RISK_AXIS, &unfit).samples, 0);
    }

    // ---- the routing seat's second version (t-6346) ----------------------

    use zerocode_core::jev::questions::{
        ROUTING_COMPLEXITY_ID, ROUTING_COMPLEXITY_LEVELS, ROUTING_COMPLEXITY_QUESTION, ROUTING_FACTS,
        ROUTING_INTENTS, ROUTING_INTENT_ID, ROUTING_INTENT_QUESTION, ROUTING_REASONING,
        ROUTING_REASONING_ID, ROUTING_REASONING_QUESTION, ROUTING_RISK_ID, ROUTING_RISK_LEVELS,
        ROUTING_RISK_QUESTION,
    };

    /// A Score answer spread over four levels as the contract writes one: the
    /// position is the levels weighted by their probabilities, on the wire's
    /// grid.
    fn level_answer(probabilities: [f64; 4], confidence: f64) -> serde_json::Value {
        #[allow(clippy::cast_precision_loss)]
        let score: f64 = probabilities.iter().enumerate().map(|(level, share)| level as f64 * share).sum();
        json!({
            "type": "score",
            "score": (score * 100.0).round() / 100.0,
            "confidence": confidence,
            "legend": {"0": "a", "1": "b", "2": "c", "3": "d"},
            "probabilities": {
                "0": probabilities[0], "1": probabilities[1], "2": probabilities[2], "3": probabilities[3],
            },
        })
    }

    /// A choice over `options` that gives `chosen` the share `leading` and
    /// splits the rest evenly.
    fn option_answer(options: &[&str], chosen: &str, leading: f64, confidence: f64) -> serde_json::Value {
        #[allow(clippy::cast_precision_loss)]
        let rest = (1.0 - leading) / (options.len() - 1) as f64;
        let probabilities: serde_json::Map<String, serde_json::Value> = options
            .iter()
            .map(|option| ((*option).to_string(), json!(if *option == chosen { leading } else { rest })))
            .collect();
        json!({"type": "choice", "choice": chosen, "probabilities": probabilities, "confidence": confidence})
    }

    fn intents() -> Vec<&'static str> {
        ROUTING_INTENTS.iter().map(|intent| intent.option.word).collect()
    }

    fn kinds() -> Vec<&'static str> {
        ROUTING_REASONING.iter().map(|kind| kind.word).collect()
    }

    /// Every answer of a second-version judgment: a medium task (level 2, sure
    /// enough to act), a low risk, a debugging task, a search, and every fact
    /// a clear no but a plan asked for.
    fn routing_answers() -> serde_json::Value {
        let mut answers = json!({
            ROUTING_COMPLEXITY_ID: level_answer([0.02, 0.06, 0.88, 0.04], 0.9),
            ROUTING_RISK_ID: level_answer([0.1, 0.8, 0.07, 0.03], 0.7),
            ROUTING_INTENT_ID: option_answer(&intents(), "debugging", 0.82, 0.8),
            ROUTING_REASONING_ID: option_answer(&kinds(), "search", 0.7, 0.66),
        });
        for fact in &ROUTING_FACTS {
            answers[fact.id] = json!({"type": "noul", "noul": 0.05});
        }
        answers["plan_first"] = json!({"type": "noul", "noul": 0.81});
        answers
    }

    #[test]
    fn the_routing_questions_are_the_catalogs_asked_in_one_request() {
        let asked = routing_questions();
        assert_eq!(asked.len(), 4 + ROUTING_FACTS.len(), "two scores, two choices and every fact, one request");
        for (id, question, levels) in [
            (ROUTING_COMPLEXITY_ID, ROUTING_COMPLEXITY_QUESTION, ROUTING_COMPLEXITY_LEVELS),
            (ROUTING_RISK_ID, ROUTING_RISK_QUESTION, ROUTING_RISK_LEVELS),
        ] {
            assert_eq!(asked[id].kind, SystemOneQuestionKind::Score, "{id} is ordered");
            assert_eq!(asked[id].instructions, question);
            assert_eq!(asked[id].criteria.levels().map(<[String]>::to_vec), Some(levels.map(str::to_string).to_vec()));
        }
        for (id, question, mut options) in [
            (ROUTING_INTENT_ID, ROUTING_INTENT_QUESTION, intents()),
            (ROUTING_REASONING_ID, ROUTING_REASONING_QUESTION, kinds()),
        ] {
            assert_eq!(asked[id].kind, SystemOneQuestionKind::Choice);
            assert_eq!(asked[id].instructions, question);
            assert!(matches!(asked[id].criteria, api::SystemOneCriteria::Contrastive(_)), "{id} contrasts its options");
            options.sort_unstable();
            assert_eq!(asked[id].criteria.options().collect::<Vec<_>>(), options);
        }
        for fact in &ROUTING_FACTS {
            assert_eq!(asked[fact.id].kind, SystemOneQuestionKind::Noul, "{}", fact.id);
            assert_eq!(asked[fact.id].instructions, fact.instructions);
        }
        // The probe's axes are named by the catalog's ids, so a ledger row
        // lays the two readers side by side under one spelling.
        assert_eq!(
            [COMPLEXITY_AXIS.name, RISK_AXIS.name, INTENT_AXIS.name],
            [ROUTING_COMPLEXITY_ID, ROUTING_RISK_ID, ROUTING_INTENT_ID]
        );
        let state = routing_state("fix the login", RoutingFacts { retry_of_failed_attempt: true });
        assert_eq!(state, json!({"task": "fix the login", "facts": {"retry_of_failed_attempt": true}}));
        let request = routing_request("jev-latest", &state);
        assert!(std::ptr::eq(request.questions, asked));
        assert_eq!(request.state, &state);
    }

    /// Complexity is a Score now: its answer is read as a position along the
    /// levels, the level it rounds to is the router's band, and version 1's
    /// reader refuses the same answer, since a Score is not a Choice.
    /// The catalog folds into the router's own four words, spelled where the
    /// router spells them: a word only one side knew would fold an intent
    /// into nothing.
    #[test]
    fn the_catalogs_router_words_are_the_routers_own() {
        let router: Vec<&str> = RouteTaskIntent::ALL.iter().map(|intent| intent.as_str()).collect();
        assert_eq!(router, zerocode_core::jev::questions::ROUTER_INTENTS.to_vec());
        assert_eq!(INTENT_AXIS.tokens.to_vec(), router);
        assert_eq!(ROUTING_COMPLEXITY_LEVELS.len(), COMPLEXITY_AXIS.tokens.len(), "a level per band");
        assert_eq!(ROUTING_RISK_LEVELS.len(), RISK_AXIS.tokens.len(), "a level per risk");
    }

    #[test]
    fn a_score_answer_for_complexity_is_read() {
        let reading = validate_routing(&response(routing_answers())).expect("a valid answer");
        assert_eq!(reading.complexity.level, 2);
        assert_eq!(reading.complexity.probabilities, vec![0.02, 0.06, 0.88, 0.04]);
        assert!((reading.complexity.score - 1.94).abs() < 1e-9);
        assert!((reading.complexity.confidence - 0.9).abs() < f64::EPSILON);
        let verdict = reading.verdict();
        assert_eq!(verdict.complexity.choice, RouteTaskComplexity::Medium);
        assert_eq!(verdict.complexity.position, 2);
        assert_eq!(verdict.risk.choice, RouteTaskRisk::Medium);
        assert_eq!(verdict.intent.choice, RouteTaskIntent::Implementation, "debugging folds into implementation");
        assert_eq!(reading.intent.chosen, "debugging");
        assert_eq!(reading.reasoning.chosen, "search");
        assert_eq!(
            validate_decision(&response(routing_answers())),
            Err(DecisionRejection::NotAChoice("complexity")),
            "the probe rubric's reader asks for a Choice"
        );
    }

    /// The band the complexity answer's own confidence falls in decides the
    /// authority it routes with — the seat's own lines (600‰ / 850‰): act
    /// alone (the router's High: one band either way), confirm (Medium: up
    /// only, the tables stand against a lower band), or abstain — no
    /// assessment, and the caller asks the chat probe instead.
    #[test]
    fn a_verdict_acts_by_its_band() {
        for (confidence, band, authority) in [
            (0.97, Band::Act, Some(RouteConfidence::High)),
            (0.85, Band::Act, Some(RouteConfidence::High)),
            (0.849, Band::Confirm, Some(RouteConfidence::Medium)),
            (0.6, Band::Confirm, Some(RouteConfidence::Medium)),
            (0.599, Band::Abstain, None),
            (0.1, Band::Abstain, None),
        ] {
            let mut answers = routing_answers();
            answers[ROUTING_COMPLEXITY_ID]["confidence"] = json!(confidence);
            let reading = validate_routing(&response(answers)).expect("valid");
            assert_eq!(reading.band(), band, "{confidence}");
            assert_eq!(reading.assessment().map(|assessment| assessment.confidence), authority, "{confidence}");
        }
    }

    /// An axis whose own answer is under the abstain line routes nothing — the
    /// intent stays the router's neutral `Other`, the risk moves nothing —
    /// while the verdict the ledger keeps is still what was answered.
    #[test]
    fn an_axis_under_its_abstain_line_says_nothing_while_the_ledger_keeps_what_it_said() {
        let mut answers = routing_answers();
        answers[ROUTING_INTENT_ID]["confidence"] = json!(0.4);
        answers[ROUTING_RISK_ID] = level_answer([0.0, 0.1, 0.2, 0.7], 0.3);
        let reading = validate_routing(&response(answers)).expect("valid");
        let assessment = reading.assessment().expect("complexity acts");
        assert_eq!(assessment.intent, RouteTaskIntent::Other);
        assert_eq!(assessment.risk, RouteTaskRisk::Unknown, "a timid risk raises nothing");
        assert_eq!(assessment.complexity, RouteTaskComplexity::Medium);
        let verdict = reading.verdict();
        assert_eq!(verdict.intent.choice, RouteTaskIntent::Implementation);
        assert_eq!(verdict.risk.choice, RouteTaskRisk::Critical);
    }

    /// The ten intents fold into the router's four with their probabilities:
    /// a judgment torn between reviewing and investigating is sure it is
    /// analysis.
    #[test]
    fn an_intent_folds_with_its_probability_into_the_routers_four() {
        let mut answers = routing_answers();
        let mut torn = option_answer(&intents(), "review_or_verification", 0.45, 0.35);
        torn["probabilities"]["investigation"] = json!(0.4);
        let rest = (1.0 - 0.45 - 0.4) / 8.0;
        for word in intents() {
            if word != "review_or_verification" && word != "investigation" {
                torn["probabilities"][word] = json!(rest);
            }
        }
        answers[ROUTING_INTENT_ID] = torn;
        let verdict = validate_routing(&response(answers)).expect("valid").verdict();
        assert_eq!(verdict.intent.choice, RouteTaskIntent::Analysis);
        let analysis = INTENT_AXIS.position("analysis").expect("a router word");
        assert!(verdict.intent.probabilities[analysis] > 0.85, "{:?}", verdict.intent.probabilities);
        assert!((verdict.intent.probabilities.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert_eq!(verdict.intent.probabilities.len(), INTENT_AXIS.tokens.len());
    }

    /// A fact reads true from its own yes line (the seat's table), and a fact
    /// the seat never asked reads as nothing.
    #[test]
    fn a_fact_reads_true_from_its_own_line() {
        let line = ROUTING_FACTS.iter().find(|fact| fact.id == "plan_first").expect("asked").yes_from_permille;
        for (yes, holds) in [(0.81, true), (f64::from(line) / 1_000.0, true), (f64::from(line) / 1_000.0 - 0.01, false), (0.05, false)] {
            let mut answers = routing_answers();
            answers["plan_first"] = json!({"type": "noul", "noul": yes});
            let reading = validate_routing(&response(answers)).expect("valid");
            assert_eq!(reading.holds("plan_first"), Some(holds), "{yes}");
        }
        let reading = validate_routing(&response(routing_answers())).expect("valid");
        assert_eq!(reading.holds("a fact nobody asked"), None);
        assert_eq!(reading.holds("changes_code"), Some(false));
    }

    #[test]
    fn every_broken_routing_rule_discards_the_whole_answer() {
        let cases: Vec<(&str, &str, serde_json::Value, DecisionRejection)> = vec![
            ("missing score", ROUTING_COMPLEXITY_ID, json!(null), DecisionRejection::MissingAnswer("complexity")),
            ("a choice where a score was asked", ROUTING_RISK_ID, option_answer(&["a", "b"], "a", 0.9, 0.9), DecisionRejection::NotAScore("risk")),
            ("a score past the last level", ROUTING_RISK_ID, json!({"type": "score", "score": 3.5, "confidence": 0.5, "legend": {}, "probabilities": {"0": 0.25, "1": 0.25, "2": 0.25, "3": 0.25}}), DecisionRejection::ScoreOutsideLevels("risk")),
            ("a level missing", ROUTING_RISK_ID, json!({"type": "score", "score": 1.0, "confidence": 0.5, "legend": {}, "probabilities": {"0": 0.5, "1": 0.5, "2": 0.0}}), DecisionRejection::ProbabilityKeys("risk")),
            ("levels short of one", ROUTING_RISK_ID, json!({"type": "score", "score": 1.0, "confidence": 0.5, "legend": {}, "probabilities": {"0": 0.2, "1": 0.2, "2": 0.2, "3": 0.2}}), DecisionRejection::ProbabilitySum("risk")),
            ("an intent nobody offered", ROUTING_INTENT_ID, json!({"type": "choice", "choice": "chitchat", "probabilities": {}, "confidence": 0.5}), DecisionRejection::ChoiceOutsideCriteria("intent")),
            ("a kind with a missing key", ROUTING_REASONING_ID, option_answer(&kinds()[1..], "search", 0.7, 0.6), DecisionRejection::ProbabilityKeys("reasoning")),
            ("a fact unanswered", "needs_measurement", json!(null), DecisionRejection::Noul("needs_measurement", zerocode_core::jev::noul::NoulRefusal::NoAnswer)),
            ("a fact past one", "refers_to_earlier", json!({"type": "noul", "noul": 1.2}), DecisionRejection::Noul("refers_to_earlier", zerocode_core::jev::noul::NoulRefusal::OutOfRange)),
        ];
        for (case, id, answer, rejection) in cases {
            let mut answers = routing_answers();
            if answer.is_null() {
                answers.as_object_mut().expect("an object").remove(id);
            } else {
                answers[id] = answer;
            }
            assert_eq!(validate_routing(&response(answers)), Err(rejection), "{case}");
        }
    }
}
