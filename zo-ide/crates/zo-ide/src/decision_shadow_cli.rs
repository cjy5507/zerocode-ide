//! `zo decision-shadow eval` — the routing probe and its typed twin, scored
//! against labels a person wrote — and `zo decision-shadow check`, whether the
//! key the twin would use is one System One answers.
//!
//! The decision shadow (`smart.decisionShadow`) records, per probed task, what
//! the probe said and what the System One judgment said — by fingerprint,
//! never by text. `eval` joins a labels file to that ledger and prints the
//! measurements `runtime::axis_metrics` makes of each reader
//! (`docs/design/jev-decision-shadow-20260917.md` §2.5); the labels are the
//! person's, never generated here. `check` asks the twin's own question once
//! (`tools::check_system_one`) — what the window's settings run behind their
//! "연결 확인". Neither writes anything or opens a session.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use runtime::AxisMetrics;
use serde_json::{Value, json};

pub const USAGE: &str = "\
zo decision-shadow eval --labels <file.jsonl> [--cwd <dir>] [--json]
zo decision-shadow check [--json]

  eval: score the routing probe and the typed judgment recorded beside it
  (smart.decisionShadow) against labels a person wrote. One task per line:
  its \"prompt\", its \"description\" when it had one, and a label per axis
  under the axis's name (complexity, risk, intent). A label joins the ledger
  of <cwd> by the task's fingerprint, so the words must be the task as the
  probe read it; the latest row where both readers answered is scored.
  Prints each axis's accuracy, the share answered below the label (complexity
  called easier, risk called lower), Brier score and calibration error for
  the judgment's distribution, and both confusion matrices.

  check: put the judgment's own questions about one fixed task (no words of
  yours) to TypeSafe's System One with the key zo would use — TYPESAFE_API_KEY,
  or the key saved in ZeroCode's settings — and print the model that answered
  and how long it took, or the token of why none did (no_key, unauthorized,
  timeout, …). Bills one small request; exits 1 when nothing answered.
";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    Eval { labels: PathBuf, cwd: Option<PathBuf>, json: bool },
    Check { json: bool },
}

fn parse(args: &[String]) -> Result<Request, String> {
    let verb = match args.first().map(String::as_str) {
        Some(verb @ ("eval" | "check")) => verb,
        Some("-h" | "--help") | None => return Err(USAGE.to_string()),
        Some(other) => return Err(format!("unknown `zo decision-shadow` verb `{other}`\n\n{USAGE}")),
    };
    let mut labels = None;
    let mut cwd = None;
    let mut json = false;
    let mut words = args[1..].iter();
    while let Some(flag) = words.next() {
        let mut value = || words.next().cloned().ok_or_else(|| format!("{flag} requires a value"));
        match (verb, flag.as_str()) {
            ("eval", "--labels") => labels = Some(PathBuf::from(value()?)),
            ("eval", "--cwd") => cwd = Some(PathBuf::from(value()?)),
            (_, "--json") => json = true,
            (_, other) => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }
    if verb == "check" {
        return Ok(Request::Check { json });
    }
    let labels = labels.ok_or_else(|| format!("--labels is required\n\n{USAGE}"))?;
    Ok(Request::Eval { labels, cwd, json })
}

/// What `zo decision-shadow …` prints, and whether it is an answer: a check
/// that nothing answered still prints its reason, and exits non-zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub text: String,
    pub answered: bool,
}

/// Run `zo decision-shadow …` from `cwd`: the report, or one sentence saying
/// why there is none.
///
/// # Errors
/// A malformed command line, an unreadable labels file, or a line in it that
/// is not a labelled task.
pub fn run(args: &[String], cwd: &Path) -> Result<Report, String> {
    match parse(args)? {
        Request::Eval { labels, cwd: workspace, json } => {
            let text = std::fs::read_to_string(&labels)
                .map_err(|error| format!("could not read {}: {error}", labels.display()))?;
            let workspace = workspace.as_deref().unwrap_or(cwd);
            let rows: Vec<tools::DecisionShadowRow> = tools::read_shadow_rows(&tools::decision_shadow_path(workspace));
            let evaluation = tools::evaluate_decision_labels(&text, &rows)?;
            Ok(Report {
                text: if json { render_json(&evaluation).to_string() } else { render_text(&evaluation) },
                answered: true,
            })
        }
        Request::Check { json } => {
            let check = api::sync_bridge::run_blocking(tools::check_system_one());
            Ok(Report {
                text: if json { render_check_json(&check).to_string() } else { render_check_text(&check) },
                answered: check.outcome.is_ok(),
            })
        }
    }
}

fn elapsed_ms(check: &tools::SystemOneCheck) -> u64 {
    u64::try_from(check.elapsed.as_millis()).unwrap_or(u64::MAX)
}

fn render_check_json(check: &tools::SystemOneCheck) -> Value {
    match &check.outcome {
        Ok(model) => json!({
            "answered": true,
            "model": model,
            "elapsedMs": elapsed_ms(check),
            "retries": check.retries,
        }),
        Err(failure) => json!({
            "answered": false,
            "failure": failure.ledger_token(),
            "elapsedMs": elapsed_ms(check),
            "retries": check.retries,
        }),
    }
}

fn render_check_text(check: &tools::SystemOneCheck) -> String {
    match &check.outcome {
        Ok(model) => format!(
            "System One answered: {model} in {} ms ({} retries)",
            elapsed_ms(check),
            check.retries
        ),
        Err(failure) => format!("System One did not answer: {}", failure.ledger_token()),
    }
}

fn render_json(evaluation: &tools::LabelEvaluation) -> Value {
    let reader = |metrics: &AxisMetrics| {
        json!({
            "samples": metrics.samples,
            "correct": metrics.correct,
            "accuracy": metrics.accuracy(),
            "belowLabel": metrics.below_label,
            "belowLabelRate": metrics.below_label_rate(),
            "scored": metrics.scored,
            "brier": metrics.brier,
            "calibrationError": metrics.calibration_error,
            "confusion": metrics.confusion,
        })
    };
    json!({
        "labels": evaluation.labels,
        "matched": evaluation.matched,
        "calibrationBins": runtime::CALIBRATION_BINS,
        "axes": evaluation.axes.iter().map(|axis| json!({
            "axis": axis.axis.name,
            "tokens": axis.axis.tokens,
            "probe": reader(&axis.probe),
            "judgment": reader(&axis.judgment),
        })).collect::<Vec<_>>(),
    })
}

fn render_text(evaluation: &tools::LabelEvaluation) -> String {
    let mut out = format!(
        "{} labelled tasks · {} scored (both readers answered on the ledger) · {} not scored\n",
        evaluation.labels,
        evaluation.matched,
        evaluation.labels - evaluation.matched
    );
    for axis in &evaluation.axes {
        // A scale reads lowest first; the other axes are only a set.
        let separator = if axis.axis.ordered { " < " } else { " | " };
        let _ = writeln!(out, "\n{} ({})", axis.axis.name, axis.axis.tokens.join(separator));
        let _ = writeln!(
            out,
            "  {:<9} {:>5} {:>9} {:>12} {:>7} {:>12}",
            "reader", "n", "accuracy", "below label", "Brier", "calibration"
        );
        for (name, metrics) in [("probe", &axis.probe), ("judgment", &axis.judgment)] {
            let below = if axis.axis.ordered { share(metrics.below_label_rate()) } else { "n/a".to_string() };
            let _ = writeln!(
                out,
                "  {name:<9} {:>5} {:>9} {below:>12} {:>7} {:>12}",
                metrics.samples,
                share(metrics.accuracy()),
                score(metrics.brier),
                score(metrics.calibration_error),
            );
        }
        let _ = writeln!(out, "  confusion (row = label, column = answer)");
        for (name, metrics) in [("probe", &axis.probe), ("judgment", &axis.judgment)] {
            let rows = metrics
                .confusion
                .iter()
                .map(|row| format!("[{}]", row.iter().map(usize::to_string).collect::<Vec<_>>().join(" ")))
                .collect::<Vec<_>>()
                .join(" ");
            let _ = writeln!(out, "    {name:<9} {rows}");
        }
    }
    out
}

fn share(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_string(), |value| format!("{:.2}%", value * 100.0))
}

fn score(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_string(), |value| format!("{value:.3}"))
}

#[cfg(test)]
mod tests {
    use super::{parse, run, Request, USAGE};

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_command_line_is_two_verbs_and_their_flags_all_in_the_usage() {
        let request = parse(&words("eval --labels labels.jsonl --cwd /tmp/w --json")).expect("parses");
        let Request::Eval { labels, json, .. } = request else { panic!("an eval") };
        assert_eq!(labels.to_str(), Some("labels.jsonl"));
        assert!(json);
        assert_eq!(parse(&words("check --json")), Ok(Request::Check { json: true }));
        assert_eq!(parse(&words("check")), Ok(Request::Check { json: false }));
        for word in ["eval", "check", "--labels", "--cwd", "--json"] {
            assert!(USAGE.contains(word), "{word} is parsed but not in USAGE");
        }
        for axis in runtime::decision_questions().keys() {
            assert!(USAGE.contains(axis.as_str()), "the usage names every judged axis: {axis}");
        }
        assert!(parse(&words("eval")).unwrap_err().contains("--labels is required"));
        assert!(parse(&words("score --labels x")).unwrap_err().contains("unknown `zo decision-shadow` verb"));
        assert!(parse(&words("eval --labels x --verbose")).unwrap_err().contains("unknown argument `--verbose`"));
        assert!(
            parse(&words("check --labels x")).unwrap_err().contains("unknown argument `--labels`"),
            "a check reads no labels"
        );
        assert_eq!(parse(&[]).unwrap_err(), USAGE);
    }

    #[test]
    fn eval_scores_a_labels_file_against_the_workspaces_ledger() {
        let _lock = crate::test_env_lock();
        let root = tempfile::tempdir().expect("a root");
        let previous = std::env::var_os(core_types::paths::ZO_STATE_DIR_ENV);
        std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, root.path().join("state"));
        let outcome = std::panic::catch_unwind(|| {
            let workspace = root.path().join("workspace");
            let ledger = tools::decision_shadow_path(&workspace);
            std::fs::create_dir_all(ledger.parent().expect("a directory")).expect("ledger dir");
            // A turn's probe reads no description, only the prompt.
            let task = "delete the production database";
            let fingerprint = format!("{:016x}", tools::task_fingerprint("", task));
            let row = serde_json::json!({
                "at": 1, "task": fingerprint, "rubricVersion": 1, "model": "jev-latest",
                "outcome": tools::OUTCOME_ANSWERED, "elapsedMs": 180, "retries": 0, "cached": false, "inputTokens": 90,
                "probe": {"complexity": "small", "risk": "medium", "confidence": "high", "intent": "implementation"},
                "jev": {
                    "complexity": {"choice": "small", "probabilities": {"trivial": 0.0, "small": 1.0, "medium": 0.0, "large": 0.0}, "confidence": 0.9},
                    "risk": {"choice": "critical", "probabilities": {"low": 0.0, "medium": 0.0, "high": 0.0, "critical": 1.0}, "confidence": 0.9},
                    "intent": {"choice": "implementation", "probabilities": {"design": 0.0, "implementation": 1.0, "analysis": 0.0, "other": 0.0}, "confidence": 0.9}},
            });
            std::fs::write(&ledger, format!("{row}\n")).expect("ledger");
            let labels = root.path().join("labels.jsonl");
            std::fs::write(
                &labels,
                format!(
                    "{}\n",
                    serde_json::json!({"prompt": task, "complexity": "small", "risk": "critical"})
                ),
            )
            .expect("labels");

            let args = |extra: &str| words(&format!("eval --labels {} --cwd {} {extra}", labels.display(), workspace.display()));
            let report = run(&args(""), root.path()).expect("a report");
            assert!(report.answered);
            let text = report.text;
            assert!(text.starts_with("1 labelled tasks · 1 scored"), "{text}");
            assert!(text.contains("risk (low < medium < high < critical)"), "{text}");
            // The probe called a critical risk medium: wrong, and below its label.
            let risk_probe = text.lines().skip_while(|line| !line.starts_with("risk")).nth(2).expect("the probe line");
            assert!(risk_probe.contains("0.00%") && risk_probe.contains("100.00%"), "{risk_probe}");

            let json: serde_json::Value =
                serde_json::from_str(&run(&args("--json"), root.path()).expect("json").text).expect("parses");
            assert_eq!(json["matched"], 1);
            let risk = json["axes"].as_array().expect("axes").iter().find(|axis| axis["axis"] == "risk").expect("risk");
            assert_eq!(risk["probe"]["belowLabel"], 1);
            assert_eq!(risk["judgment"]["correct"], 1);
            assert_eq!(risk["judgment"]["brier"], 0.0);
        });
        match previous {
            Some(value) => std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, value),
            None => std::env::remove_var(core_types::paths::ZO_STATE_DIR_ENV),
        }
        if let Err(payload) = outcome {
            std::panic::resume_unwind(payload);
        }
    }
}
