//! The judge's promotions on a synthetic corpus, counted the same way on the code
//! before t-26584's evidence rules and after them (t-26584).
//!
//! Each scenario is one seat's ledger with a known truth: `should_rise` says
//! whether a rise is right. The corpus is built here, from no one's ledger.
//! It uses only `judge_seat` and `judge_seat_in`, so the same file builds on
//! both sides of the change.
//!
//! Prints: wrongly raised seats (a rise the truth says is wrong), recall's
//! rule-side control apart from them, the marks a right rise needs, and the
//! judge's time per judgment in milliseconds.

use std::time::Instant;

use serde_json::{Value, json};
use zerocode_core::jev::promote::{Verdict, judge_seat, judge_seat_in};
use zerocode_core::jev::{CHALLENGER, JevUse, PLACEMENT, RECALL};

/// How one scenario's labels are written.
#[derive(Clone, Copy)]
struct Shape {
    /// The label's source word, or `None` for a label that names none.
    source: Option<&'static str>,
    /// Whether each mark carries the current judgment's cost and latency.
    priced: bool,
    /// The candidate's cost per decision, relative to the current one (permille over 1000).
    cost_scale_permille: u64,
    /// The line the seat acts from, when the seat reads one.
    line: Option<u16>,
    /// Whether the ledger carries a pinned human bundle (the pin row is written
    /// the same way the calibrator writes it).
    bundled: bool,
}

/// Requests the seat's window is read on, each answered and named as the
/// seat's labels name a request.
fn requests(seat: &JevUse, count: usize) -> Vec<Value> {
    (0..count)
        .map(|at| {
            let mut row = json!({
                "at": at,
                "outcome": "answered",
                "elapsedMs": 100,
                "inputTokens": 100,
                "rubricVersion": seat.rubric_version,
                "confidence": 0.9,
            });
            for key in seat.request_name {
                row[*key] = json!(at);
            }
            row
        })
        .collect()
}

/// `marks` label rows of `seat`, written after `start`, graded the way `shape` says.
fn marks(seat: &JevUse, start: usize, marks: usize, shape: Shape) -> Vec<Value> {
    (0..marks)
        .map(|n| {
            let name = if seat.request_name.is_empty() {
                json!(n)
            } else {
                json!(vec![n.to_string(); seat.request_name.len()].join(":"))
            };
            // The candidate says no to as many facts as the seat's record must hold
            // before its agreement may speak (t-6342: a label that never says no
            // is not evidence), and yes to the rest. The current judgment is right
            // on half of them.
            let agreed = n >= seat.negatives_wanted.unwrap_or(0);
            let mut row = json!({
                "at": start + n,
                "label": name,
                "requestAt": n,
                "agreed": agreed,
                "baselineAgreed": n % 2 == 0,
                "confidence": 0.9,
            });
            if let Some(source) = shape.source {
                row["labelSource"] = json!(source);
                row["labelAuthor"] = json!("eval");
            }
            if shape.priced {
                let tokens = 1_000;
                row["inputTokens"] = json!(tokens * shape.cost_scale_permille / 1_000);
                row["currentInputTokens"] = json!(tokens);
                row["elapsedMs"] = json!(100);
                row["currentElapsedMs"] = json!(100);
            }
            row
        })
        .collect()
}

/// The pin row a calibrator writes and the person rows it pins (none, when the
/// shape carries no bundle).
fn bundle(seat: &JevUse, shape: Shape) -> Vec<Value> {
    if !shape.bundled {
        return Vec::new();
    }
    // The person rows are written beside the seat's rows; the pin names their hash.
    let rows: Vec<Value> = (0..40)
        .map(|n| {
            json!({
                "at": 1_000_000 + n,
                "label": format!("person-{n}"),
                "labelSource": "person",
                "agreed": true,
            })
        })
        .collect();
    let mut judged: Vec<(String, bool)> = rows
        .iter()
        .map(|row| (row["label"].as_str().unwrap_or_default().to_string(), true))
        .collect();
    judged.sort();
    let hash = eval_hash(&judged);
    let mut all = rows;
    all.push(json!({"kind": "bundle_pin", "seat": seat.id, "bundleSha256": hash}));
    all
}

/// The same hash the judge computes over the person rows, reproduced here so
/// the corpus can pin them (the eval builds on both sides of the change).
fn eval_hash(judged: &[(String, bool)]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (label, agreed) in judged {
        hasher.update(label.as_bytes());
        hasher.update([b'\t', if *agreed { b'1' } else { b'0' }, b'\n']);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A ledger of `seat` under `shape` with `count` marks.
fn ledger(seat: &JevUse, shape: Shape, count: usize) -> Vec<Value> {
    let window = 100;
    let mut rows = requests(seat, window);
    rows.extend(marks(seat, window, count, shape));
    rows.extend(bundle(seat, shape));
    rows
}

/// The judge's verdict for a ledger, with the calibrated line when the shape has one.
fn judge(seat: &JevUse, rows: &[Value], shape: Shape) -> Option<Verdict> {
    let verdict = if seat.reads_act_line {
        judge_seat_in(seat, rows, shape.line)?.verdict
    } else {
        judge_seat(seat, rows)?.verdict
    };
    Some(verdict)
}

/// One scenario: its name, the seat, its shape, and whether a rise is right.
struct Scenario {
    name: &'static str,
    seat: &'static JevUse,
    shape: Shape,
    should_rise: bool,
}

fn scenarios() -> Vec<Scenario> {
    let executed = Shape {
        source: Some("executed"),
        priced: true,
        cost_scale_permille: 1_000,
        line: None,
        bundled: false,
    };
    vec![
        Scenario {
            name: "executed labels, priced",
            seat: &RECALL,
            shape: executed,
            should_rise: true,
        },
        Scenario {
            name: "proxy labels, priced",
            seat: &RECALL,
            shape: Shape {
                source: Some("proxy"),
                ..executed
            },
            should_rise: false,
        },
        Scenario {
            name: "unsourced labels, priced",
            seat: &RECALL,
            shape: Shape {
                source: None,
                ..executed
            },
            should_rise: false,
        },
        Scenario {
            name: "executed labels, no cost or latency pairs",
            seat: &RECALL,
            shape: Shape {
                priced: false,
                ..executed
            },
            should_rise: false,
        },
        // The cost line reads a seat whose current side is a model (t-26584,
        // decision 1a): the challenger's is the incumbent's design.
        Scenario {
            name: "model-side seat, executed labels, cost +1.5%",
            seat: &CHALLENGER,
            shape: Shape {
                cost_scale_permille: 1_015,
                ..executed
            },
            should_rise: false,
        },
        Scenario {
            name: "act-line seat, executed, no calibration",
            seat: &PLACEMENT,
            shape: Shape {
                line: None,
                ..executed
            },
            should_rise: false,
        },
        Scenario {
            name: "act-line seat, calibrated, no pinned bundle",
            seat: &PLACEMENT,
            shape: Shape {
                line: Some(500),
                bundled: false,
                ..executed
            },
            should_rise: false,
        },
        Scenario {
            name: "act-line seat, calibrated, pinned bundle",
            seat: &PLACEMENT,
            shape: Shape {
                line: Some(500),
                bundled: true,
                ..executed
            },
            should_rise: true,
        },
    ]
}

/// Recall's own control, apart from the eight: its current side is its own
/// order, a rule that spends no tokens, so its cost is not read (t-26584,
/// decision 1a) and the +1.5% that holds the model side raises it. On the
/// code before that decision the cost line held it.
fn rule_side_control() -> Scenario {
    Scenario {
        name: "rule-side control: executed labels, cost +1.5%",
        seat: &RECALL,
        shape: Shape {
            cost_scale_permille: 1_015,
            ..scenarios()[0].shape
        },
        should_rise: true,
    }
}

fn main() {
    let cases = scenarios();
    let mut wrong = 0_usize;
    let mut judgments = 0_usize;
    let started = Instant::now();
    for scenario in &cases {
        let rows = ledger(scenario.seat, scenario.shape, 60);
        let verdict = judge(scenario.seat, &rows, scenario.shape);
        let rose = matches!(verdict, Some(Verdict::Rise));
        let wrongly = rose && !scenario.should_rise;
        wrong += usize::from(wrongly);
        judgments += 1;
        println!(
            "{:<50} verdict={:<28} should_rise={:<5} wrongly_raised={}",
            scenario.name,
            format!("{verdict:?}"),
            scenario.should_rise,
            wrongly
        );
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    println!("wrongly raised seats: {wrong} of {} scenarios", cases.len());
    let control = rule_side_control();
    let verdict = judge(
        control.seat,
        &ledger(control.seat, control.shape, 60),
        control.shape,
    );
    let rose = matches!(verdict, Some(Verdict::Rise));
    println!(
        "{:<50} verdict={:<28} should_rise={:<5} as_intended={}",
        control.name,
        format!("{verdict:?}"),
        control.should_rise,
        rose == control.should_rise
    );
    // Marks a right rise needs: the smallest count the judge rises on, scanned
    // from one (the same shape, the same window).
    let needed = (1..=200).find(|count| {
        let shape = Shape {
            line: None,
            ..scenarios()[0].shape
        };
        matches!(
            judge(&RECALL, &ledger(&RECALL, shape, *count), shape),
            Some(Verdict::Rise)
        )
    });
    println!(
        "marks a right rise needs (executed, priced): {}",
        needed.map_or_else(|| "none up to 200".to_string(), |n| n.to_string())
    );
    let per_judgment_ms = elapsed_ms / judgments.max(1) as f64;
    println!(
        "judge time: {per_judgment_ms:.3} ms per judgment ({judgments} judgments, {elapsed_ms:.3} ms total)"
    );
}
