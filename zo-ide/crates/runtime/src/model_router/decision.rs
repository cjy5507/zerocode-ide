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

use super::outcome::rate;
use super::policy::{RouteConfidence, RouteTaskComplexity, RouteTaskRisk};
use super::probe::{RouteTaskIntent, RubricAxis, COMPLEXITY_AXIS, INTENT_AXIS, RISK_AXIS, ROUTING_RUBRIC};

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
}
