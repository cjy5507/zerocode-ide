//! What the decision shadow's ledger says: the doctor's summary, and the
//! probe and the typed judgment measured against human labels.
//!
//! Both read `decision_shadow.rs`'s rows and nothing else. Shares are integer
//! basis points, as the doctor's other ledgers report theirs; the label
//! metrics are `runtime::axis_metrics`, the one arithmetic both readers are
//! measured with (`docs/design/jev-decision-shadow-20260917.md` §2.5).

use std::collections::{BTreeMap, HashMap};

use runtime::{axis_metrics, AxisMetrics, AxisSample, RubricAxis};
use serde::Deserialize;

use zerocode_core::jev::door::Refused;

use super::decision_shadow::{DecisionShadowRow, ProbeCell};
use super::probe_exec::task_fingerprint;

/// Hundredths of a percent in a whole share.
pub const BASIS_POINTS: u64 = 10_000;

/// A share in basis points, integer-exact; `None` for an empty population.
#[must_use]
pub fn basis_points(part: usize, whole: usize) -> Option<u64> {
    let part = u64::try_from(part).ok()?;
    let whole = u64::try_from(whole).ok().filter(|whole| *whole > 0)?;
    part.checked_mul(BASIS_POINTS).map(|scaled| scaled / whole)
}

/// One judged axis: over the rows where both the probe and the typed judgment
/// answered, how many named the same token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisAgreement {
    pub axis: &'static RubricAxis,
    pub compared: usize,
    pub agreed: usize,
}

/// The decision shadow's ledger, counted.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionShadowSummary {
    pub rows: usize,
    pub answered: usize,
    /// Rows whose judgment went over the wire — the latency population.
    pub called: usize,
    /// Rows that asked and did not answer, by outcome token, most frequent
    /// first.
    pub failures: Vec<(String, usize)>,
    /// Rows the Jev door refused before anything was sent, by refusal token,
    /// most frequent first. Not failures: nothing was asked.
    pub refused: Vec<(String, usize)>,
    /// Nearest-rank percentiles of the calls' elapsed milliseconds.
    pub latency_p50_ms: Option<u64>,
    pub latency_p95_ms: Option<u64>,
    /// Every judged axis, in rubric order.
    pub agreement: Vec<AxisAgreement>,
    /// Input tokens the ledger's calls billed.
    pub input_tokens: u64,
    /// What the billed tokens on priced models cost.
    pub cost_usd: Option<f64>,
    /// Billed tokens on rows whose model no price row names.
    pub unpriced_input_tokens: u64,
}

/// Count a ledger.
#[must_use]
pub fn summarize_decision_shadow(rows: &[DecisionShadowRow]) -> DecisionShadowSummary {
    let mut failures: BTreeMap<&str, usize> = BTreeMap::new();
    let mut refused: BTreeMap<&str, usize> = BTreeMap::new();
    let mut elapsed: Vec<u64> = Vec::new();
    let mut agreement: Vec<AxisAgreement> =
        runtime::judged_axes().map(|axis| AxisAgreement { axis, compared: 0, agreed: 0 }).collect();
    let (mut answered, mut input_tokens, mut unpriced_input_tokens) = (0, 0u64, 0u64);
    let mut cost_usd: Option<f64> = None;
    for row in rows {
        if row.answered() {
            answered += 1;
        } else if row.refused() && row.outcome != Refused::NoKey.token() {
            // `no_key` stays a failure, as this report counted it before the
            // door asked for it: the doctor's reader already words it there.
            *refused.entry(row.outcome.as_str()).or_default() += 1;
        } else {
            *failures.entry(row.outcome.as_str()).or_default() += 1;
        }
        if row.called() {
            elapsed.push(row.elapsed_ms);
        }
        if let Some(jev) = &row.jev {
            for tally in &mut agreement {
                if let (Some(probe), Some(judged)) = (row.probe.token(tally.axis), jev.get(tally.axis.name)) {
                    tally.compared += 1;
                    tally.agreed += usize::from(probe == judged.choice);
                }
            }
        }
        let Some(tokens) = row.input_tokens else {
            continue;
        };
        input_tokens = input_tokens.saturating_add(tokens);
        match row.model.as_deref().and_then(api::systemone_rate) {
            Some(rate) => *cost_usd.get_or_insert(0.0) += rate.input_cost_usd(tokens),
            None => unpriced_input_tokens = unpriced_input_tokens.saturating_add(tokens),
        }
    }
    DecisionShadowSummary {
        rows: rows.len(),
        answered,
        called: elapsed.len(),
        failures: most_frequent_first(failures),
        refused: most_frequent_first(refused),
        latency_p50_ms: runtime::scoreboard::percentile(&elapsed, LATENCY_MEDIAN_PERCENT),
        latency_p95_ms: runtime::scoreboard::percentile(&elapsed, LATENCY_TAIL_PERCENT),
        agreement,
        input_tokens,
        cost_usd,
        unpriced_input_tokens,
    }
}

/// Token counts, most frequent first and alphabetical among equals.
fn most_frequent_first(counts: BTreeMap<&str, usize>) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> =
        counts.into_iter().map(|(token, count)| (token.to_string(), count)).collect();
    counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    counts
}

/// The percentiles the doctor reads a shadow's latency at.
const LATENCY_MEDIAN_PERCENT: u32 = 50;
const LATENCY_TAIL_PERCENT: u32 = 95;

/// One judged axis measured against the labels, for both readers.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisEvaluation {
    pub axis: &'static RubricAxis,
    pub probe: AxisMetrics,
    pub judgment: AxisMetrics,
}

/// Both readers against a labels file.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelEvaluation {
    /// Labelled tasks in the file.
    pub labels: usize,
    /// Labelled tasks with a ledger row where both readers answered.
    pub matched: usize,
    pub axes: Vec<AxisEvaluation>,
}

/// One labelled task on one axis where both readers answered, as positions in
/// the axis's token order.
struct ScoredTask {
    label: usize,
    probe: usize,
    choice: usize,
    /// The judgment's distribution, in token order.
    distribution: Vec<f64>,
}

/// One line of a labels file: the task as the probe read it, and its labels by
/// axis name.
#[derive(Debug, Deserialize)]
struct LabelLine {
    #[serde(default)]
    description: String,
    prompt: String,
    #[serde(flatten)]
    labels: BTreeMap<String, String>,
}

/// Measure the probe and the typed judgment against `labels_jsonl`: one task
/// per line, `prompt` (and `description` when the task had one) exactly as the
/// probe read it, and a label per judged axis under the axis's name. A label
/// is joined to the ledger by the task's fingerprint; the latest row where both
/// readers answered is the one measured.
///
/// # Errors
/// A line that is not a labelled task, named by its line number.
pub fn evaluate_decision_labels(
    labels_jsonl: &str,
    rows: &[DecisionShadowRow],
) -> Result<LabelEvaluation, String> {
    // The latest row per task where both readers answered.
    let mut latest: HashMap<&str, &DecisionShadowRow> = HashMap::new();
    for row in rows.iter().filter(|row| row.jev.is_some() && matches!(row.probe, ProbeCell::Answered(_))) {
        let entry = latest.entry(row.task.as_str()).or_insert(row);
        if row.at >= entry.at {
            *entry = row;
        }
    }
    let axes: Vec<&'static RubricAxis> = runtime::judged_axes().collect();
    let mut scored: Vec<Vec<ScoredTask>> = std::iter::repeat_with(Vec::new).take(axes.len()).collect();
    let (mut labels, mut matched) = (0, 0);
    for (index, line) in labels_jsonl.lines().enumerate().filter(|(_, line)| !line.trim().is_empty()) {
        let number = index + 1;
        let label: LabelLine = serde_json::from_str(line).map_err(|error| format!("line {number}: {error}"))?;
        for name in label.labels.keys() {
            if !axes.iter().any(|axis| axis.name == name) {
                let offered = axes.iter().map(|axis| axis.name).collect::<Vec<_>>().join(", ");
                return Err(format!("line {number}: `{name}` is not a judged axis ({offered})"));
            }
        }
        labels += 1;
        let task = format!("{:016x}", task_fingerprint(&label.description, &label.prompt));
        let row = latest.get(task.as_str());
        matched += usize::from(row.is_some());
        for (axis, samples) in axes.iter().zip(&mut scored) {
            let Some(token) = label.labels.get(axis.name) else {
                continue;
            };
            let position = axis.position(token).ok_or_else(|| {
                format!("line {number}: `{token}` is not a {} label ({})", axis.name, axis.tokens.join("|"))
            })?;
            let Some(row) = row else {
                continue;
            };
            let probe = row.probe.token(axis).and_then(|token| axis.position(token));
            let judged = row.jev.as_ref().and_then(|jev| jev.get(axis.name));
            let (Some(probe), Some(judged)) = (probe, judged) else {
                continue;
            };
            let Some(choice) = axis.position(&judged.choice) else {
                continue;
            };
            let distribution: Option<Vec<f64>> =
                axis.tokens.iter().map(|token| judged.probabilities.get(*token).copied()).collect();
            if let Some(distribution) = distribution {
                samples.push(ScoredTask { label: position, probe, choice, distribution });
            }
        }
    }
    let axes = axes
        .into_iter()
        .zip(&scored)
        .map(|(axis, samples)| {
            let probe: Vec<AxisSample<'_>> = samples
                .iter()
                .map(|task| AxisSample { label: task.label, predicted: task.probe, probabilities: None })
                .collect();
            let judgment: Vec<AxisSample<'_>> = samples
                .iter()
                .map(|task| AxisSample {
                    label: task.label,
                    predicted: task.choice,
                    probabilities: Some(task.distribution.as_slice()),
                })
                .collect();
            AxisEvaluation { axis, probe: axis_metrics(axis, &probe), judgment: axis_metrics(axis, &judgment) }
        })
        .collect();
    Ok(LabelEvaluation { labels, matched, axes })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use runtime::{COMPLEXITY_AXIS, INTENT_AXIS, RISK_AXIS};
    use serde_json::json;

    use super::super::decision_shadow::{DecisionShadowRow, OUTCOME_ANSWERED};
    use super::super::probe_exec::task_fingerprint;
    use super::*;

    /// A row as the executor writes one, from its JSON — the ledger's own shape.
    fn row(value: serde_json::Value) -> DecisionShadowRow {
        serde_json::from_value(value).expect("a ledger row")
    }

    fn probe(complexity: &str, risk: &str, intent: &str) -> serde_json::Value {
        json!({"complexity": complexity, "risk": risk, "confidence": "high", "intent": intent})
    }

    fn judgment(complexity: &str, risk: &str, intent: &str) -> serde_json::Value {
        let axis = |choice: &str, tokens: &[&str]| {
            let probabilities: BTreeMap<&str, f64> =
                tokens.iter().map(|token| (*token, if *token == choice { 1.0 } else { 0.0 })).collect();
            json!({"choice": choice, "probabilities": probabilities, "confidence": 0.9})
        };
        json!({
            "complexity": axis(complexity, COMPLEXITY_AXIS.tokens),
            "risk": axis(risk, RISK_AXIS.tokens),
            "intent": axis(intent, INTENT_AXIS.tokens),
        })
    }

    fn answered(task: &str, at: u64, elapsed: u64, probe: serde_json::Value, jev: serde_json::Value) -> DecisionShadowRow {
        let mut line = json!({
            "at": at, "task": task, "rubricVersion": 1, "model": "jev-latest", "outcome": OUTCOME_ANSWERED,
            "elapsedMs": elapsed, "retries": 0, "cached": false, "inputTokens": 1_000_000,
        });
        line["probe"] = probe;
        line["jev"] = jev;
        row(line)
    }

    /// A row the door refused asked nothing: it is neither an answer missed nor
    /// a call, and the report names it apart.
    #[test]
    fn a_refused_row_is_counted_apart_from_failures_and_calls() {
        let rows = vec![
            row(json!({"at": 1, "task": "a", "rubricVersion": 1, "outcome": "not_consented", "elapsedMs": 0,
                "retries": 0, "cached": false, "requests": 0, "redactedLines": 0, "probe": "timeout"})),
            row(json!({"at": 2, "task": "b", "rubricVersion": 1, "outcome": "budget", "elapsedMs": 0,
                "retries": 0, "cached": false, "requests": 0, "redactedLines": 0, "probe": "timeout"})),
            row(json!({"at": 3, "task": "c", "rubricVersion": 1, "outcome": "not_consented", "elapsedMs": 0,
                "retries": 0, "cached": false, "requests": 0, "redactedLines": 0, "probe": "timeout"})),
            row(json!({"at": 4, "task": "d", "rubricVersion": 1, "outcome": "timeout", "elapsedMs": 1_500,
                "retries": 0, "cached": false, "requests": 1, "redactedLines": 1, "probe": "timeout"})),
        ];
        let summary = summarize_decision_shadow(&rows);
        assert_eq!((summary.rows, summary.answered, summary.called), (4, 0, 1));
        assert_eq!(summary.failures, vec![("timeout".to_string(), 1)]);
        assert_eq!(summary.refused, vec![("not_consented".to_string(), 2), ("budget".to_string(), 1)]);
        assert_eq!((summary.latency_p50_ms, summary.latency_p95_ms), (Some(1_500), Some(1_500)));
    }

    #[test]
    fn a_ledger_counts_into_shares_latency_agreement_and_cost() {
        let rows = vec![
            answered("a", 1, 100, probe("large", "high", "design"), judgment("large", "high", "analysis")),
            answered("b", 2, 300, probe("small", "low", "design"), judgment("medium", "low", "design")),
            // Recalled: answered, but no call, no bill, no latency.
            row(json!({"at": 3, "task": "b", "rubricVersion": 1, "model": "jev-latest", "outcome": OUTCOME_ANSWERED,
                "elapsedMs": 0, "retries": 0, "cached": true, "probe": probe("small", "low", "design"),
                "jev": judgment("medium", "low", "design")})),
            // A timeout: a call, no answer.
            row(json!({"at": 4, "task": "c", "rubricVersion": 1, "outcome": "timeout", "elapsedMs": 8_000,
                "retries": 0, "cached": false, "probe": "provider_failure"})),
            // No key: neither a call nor an answer.
            row(json!({"at": 5, "task": "d", "rubricVersion": 1, "outcome": "no_key", "elapsedMs": 0,
                "retries": 0, "cached": false, "probe": probe("medium", "low", "other")})),
            row(json!({"at": 6, "task": "e", "rubricVersion": 1, "outcome": "timeout", "elapsedMs": 8_000,
                "retries": 0, "cached": false, "probe": probe("medium", "low", "other")})),
            // An unpriced model still counts its tokens.
            row(json!({"at": 7, "task": "f", "rubricVersion": 1, "model": "jev-next", "outcome": "schema",
                "elapsedMs": 200, "retries": 1, "cached": false, "inputTokens": 500, "probe": probe("medium", "low", "other")})),
        ];
        let summary = summarize_decision_shadow(&rows);
        assert_eq!((summary.rows, summary.answered, summary.called), (7, 3, 5));
        assert_eq!(
            summary.failures,
            vec![("timeout".to_string(), 2), ("no_key".to_string(), 1), ("schema".to_string(), 1)]
        );
        assert!(summary.refused.is_empty());
        // Calls: 100, 300, 8000, 8000, 200 → p50 = 300, p95 = 8000.
        assert_eq!((summary.latency_p50_ms, summary.latency_p95_ms), (Some(300), Some(8_000)));
        let by_axis: Vec<(&str, usize, usize)> =
            summary.agreement.iter().map(|agreement| (agreement.axis.name, agreement.compared, agreement.agreed)).collect();
        assert_eq!(by_axis, vec![("complexity", 3, 1), ("risk", 3, 3), ("intent", 3, 2)]);
        assert_eq!(summary.input_tokens, 2_000_500);
        assert_eq!(summary.unpriced_input_tokens, 500);
        let rate = api::systemone_rate("jev-latest").expect("the table prices jev-latest");
        let expected = rate.input_cost_usd(2_000_000);
        assert!(summary.cost_usd.is_some_and(|cost| (cost - expected).abs() < 1e-12), "{:?}", summary.cost_usd);

        let empty = summarize_decision_shadow(&[]);
        assert_eq!((empty.rows, empty.latency_p50_ms, empty.cost_usd), (0, None, None));
        assert!(empty.agreement.iter().all(|agreement| agreement.compared == 0));
        assert_eq!(basis_points(1, 3), Some(3_333));
        assert_eq!(basis_points(2, 2), Some(BASIS_POINTS));
        assert_eq!(basis_points(0, 0), None);
    }

    #[test]
    fn labels_join_the_ledger_by_fingerprint_and_score_both_readers() {
        let hex = |description: &str, prompt: &str| format!("{:016x}", task_fingerprint(description, prompt));
        let rows = vec![
            // An older row for the first task, superseded by the later one.
            answered(&hex("", "rename a label"), 1, 50, probe("large", "high", "design"), judgment("large", "high", "design")),
            answered(&hex("", "rename a label"), 2, 50, probe("small", "low", "implementation"), judgment("trivial", "low", "implementation")),
            answered(&hex("wide", "migrate every crate"), 3, 50, probe("medium", "medium", "implementation"), judgment("large", "high", "implementation")),
            // A labelled task whose judgment never answered is not matched.
            row(json!({"at": 4, "task": hex("", "rotate the keys"), "rubricVersion": 1, "outcome": "timeout",
                "elapsedMs": 8000, "retries": 0, "cached": false, "probe": probe("small", "low", "other")})),
        ];
        let labels = [
            json!({"prompt": "rename a label", "complexity": "trivial", "risk": "low", "intent": "implementation"}),
            json!({"description": "wide", "prompt": "migrate every crate", "complexity": "large", "risk": "high"}),
            json!({"prompt": "rotate the keys", "complexity": "small", "risk": "critical"}),
            json!({"prompt": "never probed", "risk": "low"}),
        ]
        .iter()
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");

        let evaluation = evaluate_decision_labels(&format!("{labels}\n\n"), &rows).expect("labels parse");
        assert_eq!((evaluation.labels, evaluation.matched), (4, 2));
        let axis = |name: &str| evaluation.axes.iter().find(|axis| axis.axis.name == name).expect("an axis");

        let complexity = axis("complexity");
        // Probe: small vs trivial (wrong, above), medium vs large (wrong, below).
        assert_eq!((complexity.probe.samples, complexity.probe.correct), (2, 0));
        assert_eq!(complexity.probe.below_label, Some(1));
        assert_eq!(complexity.probe.brier, None, "the probe states no distribution");
        // Judgment: trivial vs trivial, large vs large, both certain.
        assert_eq!((complexity.judgment.samples, complexity.judgment.correct), (2, 2));
        assert_eq!(complexity.judgment.brier, Some(0.0));
        assert_eq!(complexity.judgment.calibration_error, Some(0.0));

        let risk = axis("risk");
        assert_eq!((risk.probe.samples, risk.probe.correct, risk.probe.below_label), (2, 1, Some(1)));
        assert_eq!((risk.judgment.samples, risk.judgment.correct), (2, 2));
        // Only the first task labels intent.
        let intent = axis("intent");
        assert_eq!((intent.probe.samples, intent.judgment.samples), (1, 1));
        assert_eq!(intent.probe.below_label, None);
        assert_eq!(evaluation.axes.len(), runtime::decision_questions().len());
    }

    #[test]
    fn a_labels_file_that_does_not_parse_names_its_line() {
        let rows: Vec<DecisionShadowRow> = Vec::new();
        for (labels, needle) in [
            ("{\"prompt\": \"x\", \"risk\": \"severe\"}", "line 1"),
            ("{\"prompt\": \"x\"}\n{\"prompt\": \"y\", \"complexty\": \"large\"}", "line 2"),
            ("not json", "line 1"),
            ("{\"risk\": \"low\"}", "line 1"),
        ] {
            let error = evaluate_decision_labels(labels, &rows).expect_err(labels);
            assert!(error.contains(needle), "{error}");
        }
        let error = evaluate_decision_labels("{\"prompt\": \"x\", \"risk\": \"severe\"}", &rows).expect_err("a token");
        assert!(error.contains("severe") && error.contains(&RISK_AXIS.tokens.join("|")), "{error}");
    }
}
