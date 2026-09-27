//! The claim seat's shared half on its own: the words its version is pinned
//! to, what code settles before any question, and the state a request
//! carries (t-11349 moved it here; zo's own cases still run on it through
//! zo's reader of its turns).

use super::*;

/// A shell result of the turn, as a host reads one into evidence.
fn shell<'a>(command: &str, output: &'a str, nonzero: bool) -> Evidence<&'a str> {
    Evidence {
        command: Some(command.to_string()),
        output,
        is_error: false,
        nonzero,
    }
}

/// The version is pinned to the words zo pinned it to (t-9469): moving the
/// words moved no word.
#[test]
fn the_version_is_pinned_to_the_same_words() {
    assert_eq!(CLAIM_RUBRIC_VERSION, 1);
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        "864bb8ebe6e00fe8"
    );
}

/// Code settles what it can before a question: a named command that never
/// ran is unsupported, a pass over a non-zero exit is contradicted, and lines
/// that match reach the one request as its state.
#[test]
fn code_settles_first_and_the_rest_reach_the_state() {
    let ran = [shell("cargo check", r#"{"stdout":"ok"}"#, false)];
    assert_eq!(
        scan(&ran, "`cargo test` passed.")[0].code,
        CodeVerdict::Unsupported
    );
    let failed = [shell("cargo test", r#"{"stdout":"failed"}"#, true)];
    assert_eq!(
        scan(&failed, "`cargo test` passed.")[0].code,
        CodeVerdict::Contradicted
    );
    let green = [shell(
        "cargo test",
        r#"{"stdout":"test result: ok"}"#,
        false,
    )];
    let claims = scan(&green, "`cargo test` passed.");
    assert_eq!(claims[0].code, CodeVerdict::NeedsReading);
    assert_eq!(claims[0].evidence, "test result: ok");
    let sent = state(&claims);
    assert_eq!(sent["claims"][0]["id"], "C1");
    assert_eq!(sent["evidence"]["C1"], "test result: ok");
    let asked = questions(&claims, |instructions, options| {
        (instructions.to_string(), options.len())
    });
    assert_eq!(
        asked.get("C1").map(|(_, options)| *options),
        Some(CLAIM_CRITERIA.len())
    );
}

/// The claim seat's label, golden (t-11349) — the one zo and a window pane
/// both write ([`ClaimWaiting::label`]). The judgment is the turn's verdict;
/// today's rule is "always supports" (the seat's baseline); hindsight is the
/// person's next turn. Each case: the verdict, whether it was the model's
/// alone, what the next turn opened with, and the marks it earns.
#[test]
fn the_claim_label_golden_table() {
    struct Case {
        verdict: &'static str,
        compared: bool,
        next: &'static str,
        agreed: Option<bool>,
        baseline_agreed: Option<bool>,
        hindsight: Option<&'static str>,
    }
    let failed = FAILURE_OPENINGS[4];
    let cases = [
        // An alert the next turn bore out: the judgment right where today's
        // rule — "it passed" — was wrong.
        Case {
            verdict: CONTRADICTS,
            compared: true,
            next: failed,
            agreed: Some(true),
            baseline_agreed: Some(false),
            hindsight: Some(NEXT_PERSON_FAILED),
        },
        // An alert the next turn did not bear out: the judgment wrong,
        // today's rule right.
        Case {
            verdict: CONTRADICTS,
            compared: true,
            next: "thanks, next task",
            agreed: Some(false),
            baseline_agreed: Some(true),
            hindsight: Some(NEXT_PERSON_CONTINUED),
        },
        // Supported and it held: both right.
        Case {
            verdict: SUPPORTS,
            compared: true,
            next: "thanks, next task",
            agreed: Some(true),
            baseline_agreed: Some(true),
            hindsight: Some(NEXT_PERSON_CONTINUED),
        },
        // Supported and it did not hold: both wrong.
        Case {
            verdict: SUPPORTS,
            compared: true,
            next: failed,
            agreed: Some(false),
            baseline_agreed: Some(false),
            hindsight: Some(NEXT_PERSON_FAILED),
        },
        // Saying nothing raises no alert: wrong where the turn failed.
        Case {
            verdict: SAYS_NOTHING,
            compared: true,
            next: failed,
            agreed: Some(false),
            baseline_agreed: Some(false),
            hindsight: Some(NEXT_PERSON_FAILED),
        },
        // A verdict code settled keeps its hindsight and earns the model
        // no mark.
        Case {
            verdict: CONTRADICTS,
            compared: false,
            next: failed,
            agreed: None,
            baseline_agreed: None,
            hindsight: Some(NEXT_PERSON_FAILED),
        },
        // A turn a host note opens is not the person's word: no label.
        Case {
            verdict: SUPPORTS,
            compared: true,
            next: "[zo:resume] carrying on",
            agreed: None,
            baseline_agreed: None,
            hindsight: None,
        },
    ];
    for case in cases {
        let waiting = ClaimWaiting {
            judged: 7,
            verdict: Some(case.verdict.to_string()),
            failure: next_person_failed(case.next),
            confidence: Some(0.9),
            compared: case.compared,
        };
        let label = waiting.label(1);
        assert_eq!(
            label.as_ref().map(|row| row.hindsight.as_str()),
            case.hindsight,
            "{} / {}",
            case.verdict,
            case.next
        );
        let Some(label) = label else { continue };
        assert_eq!(
            (label.agreed, label.baseline_agreed),
            (case.agreed, case.baseline_agreed),
            "{} / {}",
            case.verdict,
            case.next
        );
        assert_eq!(
            label.not_compared.as_deref(),
            (!case.compared).then_some(NOT_MODEL_COMPARISON)
        );
        assert_eq!((label.kind.as_str(), label.label.as_str()), ("label", "7"));
    }
}
