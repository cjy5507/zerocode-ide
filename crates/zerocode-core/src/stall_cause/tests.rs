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
    // Version 5 carries the screen's lines and the record's turns as fields
    // of their own where version 4 carried each as one text (t-9469): what
    // the question reads changed shape, so the series starts again.
    assert_eq!(STALL_CAUSE_RUBRIC_VERSION, 5);
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        "557d21525cc1dc79"
    );
}

/// The screen and the record are fields, not one text each (t-9469): the
/// screen a list of its lines, top to bottom, and the record a list of its
/// turns, oldest first, each its role and its words — so what the judgment
/// reads is the shape the pane had, and no line is a sentence to split.
#[test]
fn the_screen_and_the_record_are_lists_of_their_own_parts() {
    let transcript = vec![SUMMARY.to_string(), INTERRUPTED.to_string()];
    let asked = ask(&a_look("✻ Worked for 27m 0s\n\n❯ ", &transcript)).expect("asks");
    assert_eq!(
        asked.state[STATE_KEYS[2]],
        json!(["✻ Worked for 27m 0s", "", "❯"]),
        "one entry a line, blank lines inside kept, trailing blanks off"
    );
    let turns = asked.state[STATE_KEYS[3]].as_array().expect("the turns");
    assert_eq!(turns.len(), 3, "{turns:?}");
    for turn in turns {
        let mut keys: Vec<&str> = turn
            .as_object()
            .expect("a turn")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut expected = TURN_KEYS.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected, "a turn's keys are the fingerprint's");
    }
    assert_eq!(
        turns[2],
        json!({ TURN_KEYS[0]: "user", TURN_KEYS[1]: "[Request interrupted by user]" })
    );
    for key in STATE_KEYS.iter().chain(&TURN_KEYS) {
        assert!(
            INSTRUCTIONS.contains(&format!("`{key}`")),
            "the question names what it reads: {key}"
        );
    }
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
    let asked = ask(&a_look(&screen, &[])).expect("a screen asks");
    let tail: Vec<String> = asked.state[STATE_KEYS[2]]
        .as_array()
        .expect("the screen's lines")
        .iter()
        .map(|line| line.as_str().expect("a line").to_string())
        .collect();

    assert!(
        tail.ends_with(&[
            "✻ Worked for 27m 0s · done 2:11 AM".to_string(),
            String::new(),
            "❯".to_string()
        ]),
        "{tail:?}"
    );
    assert!(
        tail.iter().all(|line| line.trim_end() == line),
        "a line kept its trailing blanks"
    );
    // Each line as the door clears it (`/state/screen/*`), and the lines
    // together as they count against the cap.
    let cleared: Vec<(String, usize)> = tail
        .iter()
        .map(|line| clear_text(line, Cap::Bytes(STALL_SCREEN_BYTE_CAP)))
        .collect();
    assert!(
        cleared.iter().any(|(_, withheld)| *withheld > 0),
        "the fixture's lines are ones the door withholds"
    );
    let kept = cleared
        .iter()
        .map(|(line, _)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        kept.len() <= STALL_SCREEN_BYTE_CAP,
        "{} bytes once cleared",
        kept.len()
    );
    assert!(
        cleared
            .iter()
            .all(|(line, _)| !line.ends_with(crate::jev::CUT_MARK)),
        "the door cut a line the tail kept"
    );
    // Not a short tail kept safe by being short: the cap is filled with as
    // many withheld lines as it holds, less the three lines of the footer.
    let holds = STALL_SCREEN_BYTE_CAP / (WITHHELD_LINE.len() + "\n".len());
    assert!(tail.len() >= holds - 3, "{} lines", tail.len());
}

/// The record's end in the board's own words, one entry a turn: what was
/// said, what was called, what came back, and a person's interruption — each
/// its role and its words, never the agent's reasoning.
#[test]
fn the_transcript_tail_is_the_boards_turns_one_record_each() {
    let result = r#"{"type":"user","uuid":"r","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"Diff in src/a.rs\n  1 file would be reformatted"}]}}"#;
    let transcript = vec![
        SUMMARY.to_string(),
        result.to_string(),
        INTERRUPTED.to_string(),
    ];
    let turns = |transcript: &[String]| -> Vec<Value> {
        ask(&a_look("", transcript)).expect("a record asks").state[STATE_KEYS[3]]
            .as_array()
            .expect("the record's turns")
            .clone()
    };
    let tail = turns(&transcript);
    let turn = |role: &str, words: &str| json!({ TURN_KEYS[0]: role, TURN_KEYS[1]: words });
    assert_eq!(
        tail,
        [
            turn("assistant", "All runs had no extra invokes and no errors."),
            turn("tool", "Bash · just fmt-check"),
            turn(
                "tool_result",
                "Diff in src/a.rs 1 file would be reformatted"
            ),
            turn("user", "[Request interrupted by user]"),
        ]
    );
    assert!(
        !json!(tail).to_string().contains("the gate is green"),
        "reasoning was sent"
    );

    // A long record keeps its newest turns within the cap, its words counted
    // whole as the door counts them.
    let many: Vec<String> = (0..500)
        .map(|turn| {
            format!(
                r#"{{"type":"assistant","uuid":"a{turn}","message":{{"role":"assistant","content":[{{"type":"text","text":"turn {turn} {}"}}]}}}}"#,
                "가".repeat(80)
            )
        })
        .collect();
    let tail = turns(&many);
    let words: Vec<&str> = tail
        .iter()
        .map(|turn| turn[TURN_KEYS[1]].as_str().expect("words"))
        .collect();
    assert!(
        words.join("\n").len() <= STALL_TRANSCRIPT_BYTE_CAP,
        "{}",
        words.join("\n").len()
    );
    assert!(
        words
            .last()
            .is_some_and(|line| line.starts_with("turn 499 "))
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
            mark(cause, Followed::Nothing).is_ok(),
            cause != Cause::Unknown,
            "{}",
            cause.word()
        );
    }
    assert_eq!(mark(Cause::QuotaWall, Followed::Resumed), Ok(true));
    assert_eq!(
        mark(Cause::WaitingOnOwnCliQuestion, Followed::Mail),
        Ok(true)
    );
    assert_eq!(
        mark(Cause::FinishedWithoutReport, Followed::WorkerDone),
        Ok(false)
    );
}

/// The eleven marks this machine's ledger held against the seat (2026-09-23):
/// every answer read a command still running, and every worker then reported
/// on its own — nobody had to do anything, which is exactly what that cause
/// says. The label asks one question of both sides — did the silence need
/// its coordinator's hand — so a long tool the coordinator had to mail about
/// is still a wrong reading, and a finished worker that reported only once
/// it was asked is a right one.
#[test]
fn a_long_tool_that_ends_in_the_workers_own_report_agrees() {
    assert_eq!(mark(Cause::LongRunningTool, Followed::WorkerDone), Ok(true));
    assert_eq!(mark(Cause::LongRunningTool, Followed::Nothing), Ok(true));
    assert_eq!(mark(Cause::HumanTookOver, Followed::WorkerDone), Ok(true));
    assert_eq!(mark(Cause::LongRunningTool, Followed::Mail), Ok(false));
    assert_eq!(
        mark(Cause::LongRunningTool, Followed::WorkerStop),
        Ok(false)
    );
    assert_eq!(mark(Cause::FinishedWithoutReport, Followed::Mail), Ok(true));
    assert_eq!(mark(Cause::TransientApiError, Followed::Resumed), Ok(true));
    assert_eq!(
        mark(Cause::TransientApiError, Followed::WorkerDone),
        Ok(false)
    );
    // A side that says nothing leaves no mark, and names itself instead.
    assert_eq!(
        mark(Cause::Unknown, Followed::WorkerDone),
        Err(Cause::Unknown.word())
    );
    assert_eq!(
        mark(Cause::QuotaWall, Followed::WorkerDied),
        Err(Followed::WorkerDied.word())
    );
}

/// Every cause and every thing that can follow a silence is read against the
/// one question, or says in so many words that it is not — so a variant added
/// to either list without a place in the table is a red test rather than a
/// label that quietly marks nothing.
#[test]
fn the_label_table_places_every_cause_and_every_follow_up() {
    let placed = |cause: Cause| mark(cause, Followed::Nothing).is_ok();
    assert_eq!(
        Cause::ALL
            .into_iter()
            .filter(|cause| !placed(*cause))
            .collect::<Vec<_>>(),
        [Cause::Unknown]
    );
    for followed in Followed::ALL {
        assert_eq!(
            mark(Cause::LongRunningTool, followed).is_ok(),
            followed != Followed::WorkerDied,
            "{}",
            followed.word()
        );
    }
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
    assert_eq!(mark(Cause::AuthFailure, Followed::WorkerStop), Ok(true));
    assert_eq!(mark(Cause::AuthFailure, Followed::WorkerDone), Ok(false));
}

/// A classifier's decline is its own answer (t-6747): the pause box it can
/// stand behind waits for a key like a question box does, but no question of
/// the work's is on it, and what ends it is a coordinator's hand — another
/// model, or the attempt handed over. The answer is a label only: the
/// ledger's `classifier_declined` news is told on the table's two witnesses
/// and never on this.
#[test]
fn a_classifier_decline_is_offered_and_labeled_as_needing_a_hand() {
    let look = StallLook {
        agent: "claude",
        quiet_ms: 180_000,
        screen: "Session paused\nDetails: [cyber]\n❯ 1. Switch to Opus 4.8\n  2. Edit prompt and retry",
        transcript: &[],
    };
    let asked = ask(&look).expect("a screen with words is a question");
    let offered = asked.questions["cause"]["criteria"]
        .as_object()
        .expect("criteria");
    let means = offered["classifier_decline"]
        .as_str()
        .expect("the decline is offered");
    for measured in [
        "safeguards flagged this message",
        "Session paused",
        "Edit prompt and retry",
    ] {
        assert!(
            means.contains(measured),
            "the criterion quotes the words this machine showed: {measured}"
        );
    }
    assert_eq!(
        Cause::from_word("classifier_decline"),
        Some(Cause::ClassifierDecline)
    );
    assert_eq!(Cause::ClassifierDecline.predicts(), Some(Hand::Needed));
    assert_eq!(
        mark(Cause::ClassifierDecline, Followed::WorkerStop),
        Ok(true)
    );
    assert_eq!(
        mark(Cause::ClassifierDecline, Followed::WorkerDone),
        Ok(false)
    );
}
