//! What the stall question promises, what an answer must be to be one, and
//! what the screen and the record look like by the time they are asked.

use serde_json::json;

use super::*;
use crate::jev::Cap;
use crate::jev::door::{WITHHELD_LINE, clear_text};

/// An answer in the contract's shape: `chosen` with the rest spread evenly.
fn answered(chosen: &str, confidence: f64) -> Value {
    // The rest share what the chosen one leaves, however many causes stand.
    let rest = 0.3 / (Cause::ALL.len() - 1) as f64;
    let probabilities: Map<String, Value> = Cause::ALL
        .iter()
        .map(|cause| {
            let share = if cause.word() == chosen { 0.7 } else { rest };
            (cause.word().to_string(), json!(share))
        })
        .collect();
    json!({
        QUESTION: {
            "type": "choice",
            "choice": chosen,
            "probabilities": Value::Object(probabilities),
            "confidence": confidence,
        }
    })
}

fn a_look<'a>(screen: &'a str, transcript: &'a [String]) -> StallLook<'a> {
    StallLook {
        agent: "claude",
        quiet_ms: 312_400,
        screen,
        transcript,
    }
}

/// The record Claude Code writes when a person presses Esc on a turn — the
/// words of 139 records on this machine.
const INTERRUPTED: &str = r#"{"type":"user","uuid":"i","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#;
const SUMMARY: &str = r#"{"type":"assistant","uuid":"s","message":{"role":"assistant","content":[{"type":"thinking","thinking":"the gate is green, now report"},{"type":"text","text":"All runs had no extra invokes and no errors."},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"just fmt-check"}}]}}"#;

#[test]
fn the_question_offers_every_cause_and_nothing_else() {
    let transcript = vec![SUMMARY.to_string()];
    let asked = ask(&a_look("❯ ", &transcript)).expect("a screen and a record ask");

    // Both sides sorted: which order a `serde_json::Map` hands its keys back in
    // is a property of the build, not of this question. `zerocode-shell` and
    // `zerocode-hookd` ask serde_json for `preserve_order`, and in a workspace
    // build Cargo's feature unification hands it to this crate too — so the
    // same code answers in insertion order there and in sorted order when this
    // crate is built alone. What the question promises is every cause and
    // nothing else.
    let mut offered: Vec<&str> = asked.questions[QUESTION]["criteria"]
        .as_object()
        .expect("the criteria")
        .keys()
        .map(String::as_str)
        .collect();
    offered.sort_unstable();
    let mut expected: Vec<&str> = Cause::ALL.iter().map(|cause| cause.word()).collect();
    expected.sort_unstable();
    assert_eq!(offered, expected);
    assert_eq!(asked.questions[QUESTION]["type"], "choice");
    assert_eq!(asked.questions[QUESTION]["instructions"], INSTRUCTIONS);

    let mut keys: Vec<&str> = asked
        .state
        .as_object()
        .expect("the state")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    let mut state_keys = STATE_KEYS.to_vec();
    state_keys.sort_unstable();
    assert_eq!(keys, state_keys);
    assert_eq!(asked.state["agent"], "claude");
    assert_eq!(asked.state["quietSeconds"], 312, "whole seconds");
    for cause in Cause::ALL {
        assert_eq!(Cause::from_word(cause.word()), Some(cause));
    }
    assert_eq!(Cause::from_word("quota"), None);
}

#[test]
fn a_silence_with_nothing_to_read_asks_nothing() {
    assert!(ask(&a_look("\n   \n\n", &[])).is_none());
    // A record with no turn in it says nothing either.
    let meta = vec![r#"{"type":"system","subtype":"turn_duration","durationMs":1}"#.to_string()];
    assert!(ask(&a_look("", &meta)).is_none());
    assert!(ask(&a_look("❯", &meta)).is_some(), "a screen alone asks");
}

#[test]
fn an_answer_is_read_only_through_the_causes_offered() {
    let transcript = vec![SUMMARY.to_string()];
    let asked = ask(&a_look("❯ ", &transcript)).expect("asks");

    let good = answered("finished_without_report", 0.64);
    let choice = asked.read(&good).expect("a well formed answer");
    assert_eq!(choice.cause, Cause::FinishedWithoutReport);
    assert_eq!(choice.probabilities.len(), Cause::ALL.len());
    assert!((choice.confidence - 0.64).abs() < f64::EPSILON);

    let mut invented = good.clone();
    invented[QUESTION]["choice"] = json!("rate_limited");
    assert_eq!(asked.read(&invented), Err(ChoiceRefusal::UnknownOption));

    let mut short = good.clone();
    short[QUESTION]["probabilities"]
        .as_object_mut()
        .expect("probabilities")
        .remove("unknown");
    assert_eq!(asked.read(&short), Err(ChoiceRefusal::Keys));

    let mut unbalanced = good.clone();
    unbalanced[QUESTION]["probabilities"]["unknown"] = json!(0.9);
    assert_eq!(asked.read(&unbalanced), Err(ChoiceRefusal::NotOne));

    let mut wild = good;
    wild[QUESTION]["confidence"] = json!(1.5);
    assert_eq!(asked.read(&wild), Err(ChoiceRefusal::OutOfRange));

    assert_eq!(asked.read(&json!({})), Err(ChoiceRefusal::NoAnswer));
}

#[test]
fn the_version_is_pinned_to_the_words() {
    // Changing a word of the question without bumping the version turns this
    // red: a judgment read under one wording is not evidence about another.
    assert_eq!(STALL_CAUSE_RUBRIC_VERSION, 2);
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        "f75306aaa8d3cf97"
    );
}

/// The screen's newest lines are kept, blank ends are dropped, and what is
/// kept fits the cap even after the door has put its mark on every line that
/// may carry a credential — so the door never has to cut the newest words.
#[test]
fn the_screen_tail_keeps_the_newest_lines_and_fits_its_cap_once_cleared() {
    let mut screen = String::new();
    for _ in 0..2_000 {
        // A line the door withholds, shorter than the mark it grows into.
        screen.push_str("export X  \n");
    }
    screen.push_str("✻ Worked for 27m 0s · done 2:11 AM   \n\n❯ \n\n\n");
    let tail = screen_tail(&screen);

    assert!(
        tail.ends_with("✻ Worked for 27m 0s · done 2:11 AM\n\n❯"),
        "{tail}"
    );
    assert!(!tail.contains("  \n"), "a line kept its trailing blanks");
    let (cleared, withheld) = clear_text(&tail, Cap::Uncut);
    assert!(
        withheld > 0,
        "the fixture's lines are ones the door withholds"
    );
    assert!(
        cleared.len() <= STALL_SCREEN_BYTE_CAP,
        "{} bytes once cleared",
        cleared.len()
    );
    let (cut, _) = clear_text(&tail, Cap::Bytes(STALL_SCREEN_BYTE_CAP));
    assert_eq!(cut, cleared, "the door cut what the tail kept");
    // Not a short tail kept safe by being short: the cap is filled with as
    // many withheld lines as it holds, less the three lines of the footer.
    let holds = STALL_SCREEN_BYTE_CAP / (WITHHELD_LINE.len() + "\n".len());
    assert!(
        tail.lines().count() >= holds - 3,
        "{} lines",
        tail.lines().count()
    );
}

/// The record's end in the board's own words, one line a turn: what was said,
/// what was called, what came back, and a person's interruption — never the
/// agent's reasoning.
#[test]
fn the_transcript_tail_is_the_boards_turns_one_line_each() {
    let result = r#"{"type":"user","uuid":"r","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"Diff in src/a.rs\n  1 file would be reformatted"}]}}"#;
    let transcript = vec![
        SUMMARY.to_string(),
        result.to_string(),
        INTERRUPTED.to_string(),
    ];
    let tail = transcript_tail(&transcript);
    let lines: Vec<&str> = tail.lines().collect();
    assert_eq!(
        lines,
        [
            "assistant: All runs had no extra invokes and no errors.",
            "tool: Bash · just fmt-check",
            "tool_result: Diff in src/a.rs 1 file would be reformatted",
            "user: [Request interrupted by user]",
        ]
    );
    assert!(!tail.contains("the gate is green"), "reasoning was sent");

    // A long record keeps its newest turns within the cap.
    let many: Vec<String> = (0..500)
        .map(|turn| {
            format!(
                r#"{{"type":"assistant","uuid":"a{turn}","message":{{"role":"assistant","content":[{{"type":"text","text":"turn {turn} {}"}}]}}}}"#,
                "가".repeat(80)
            )
        })
        .collect();
    let tail = transcript_tail(&many);
    assert!(tail.len() <= STALL_TRANSCRIPT_BYTE_CAP, "{}", tail.len());
    assert!(
        tail.lines()
            .last()
            .is_some_and(|line| line.starts_with("assistant: turn 499 "))
    );
}

/// Every cause that leads somewhere names where; `unknown` leaves no mark.
/// A cause added to the table without a follow-up here would let a label row
/// carry no `agreed` mark for it, silently, and the judge would never see
/// that seat's answers to it.
#[test]
fn every_cause_but_unknown_names_what_follows_it() {
    for cause in Cause::ALL {
        assert_eq!(
            expected_followed(cause).is_some(),
            cause != Cause::Unknown,
            "{}",
            cause.word()
        );
    }
    assert_eq!(expected_followed(Cause::QuotaWall), Some(Followed::Resumed));
    assert_eq!(
        expected_followed(Cause::WaitingOnOwnCliQuestion),
        Some(Followed::Mail)
    );
    assert_eq!(
        expected_followed(Cause::FinishedWithoutReport),
        Some(Followed::WorkerDone)
    );
}

/// A dead login is its own answer, offered by the word a ledger row keeps and
/// labeled by what a coordinator does about it — not waited out like a
/// transient error, not mailed like a question box (t-5498: `Token refresh
/// failed: 401` read as `unknown` 0.95 under the first rubric).
#[test]
fn a_dead_login_is_offered_and_labeled_as_the_end_of_the_attempt() {
    let look = StallLook {
        agent: "opencode",
        quiet_ms: 180_000,
        screen: "Token refresh failed: 401\n❯",
        transcript: &[],
    };
    let asked = ask(&look).expect("a screen with words is a question");
    let offered = asked.questions["cause"]["criteria"]
        .as_object()
        .expect("criteria");
    assert!(offered.contains_key("auth_failure"), "{offered:?}");
    assert!(
        offered["auth_failure"]
            .as_str()
            .is_some_and(|means| means.contains("Token refresh failed: 401")),
        "the criterion quotes the words this machine's pane showed"
    );
    assert_eq!(Cause::from_word("auth_failure"), Some(Cause::AuthFailure));
    assert_eq!(
        expected_followed(Cause::AuthFailure),
        Some(Followed::WorkerStop)
    );
}
