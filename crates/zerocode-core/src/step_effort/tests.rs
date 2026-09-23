//! What the step-effort rule promises, what an answer must be to be one, and
//! what a turn looks like by the time it is read.

use serde_json::json;

use super::*;
use crate::capabilities::agent_capabilities;
use crate::transcript::turns_in;

/// Claude Code's records for one turn: a prompt, then tool calls and their
/// results, in the shapes its transcript writes them.
fn claude_turn(calls: &[(&str, &str, bool)]) -> String {
    let mut lines = vec![
        json!({"type": "user", "message": {"role": "user", "content": "fix the build"}})
            .to_string(),
    ];
    for (at, (name, command, failed)) in calls.iter().enumerate() {
        lines.push(
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": format!("toolu_{at}"), "name": name, "input": {"command": command}}
            ]}})
            .to_string(),
        );
        lines.push(
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": format!("toolu_{at}"), "is_error": failed,
                 "content": if *failed { "error: it failed" } else { "ok" }}
            ]}})
            .to_string(),
        );
    }
    lines.join("\n")
}

fn moves_of(agent: &str) -> TurnMoves {
    agent_capabilities(agent).expect(agent).moves
}

#[test]
fn the_version_is_pinned_to_the_words() {
    // Changing a word of the question without bumping the version turns this
    // red: a judgment read under one wording is not evidence about another.
    assert_eq!(STEP_EFFORT_RUBRIC_VERSION, 1);
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        "f5fce3b7c9cbece9"
    );
}

/// The same call three times running is a loop; a different call between
/// two of them is not. A result the vendor marked as an error is a failure.
#[test]
fn a_turn_is_read_off_its_own_calls() {
    let looping = claude_turn(&[
        ("Bash", "cargo test -p x", true),
        ("Bash", "cargo test -p x", true),
        ("Bash", "cargo test -p x", true),
    ]);
    let signals = signals_of(&turns_in(&looping));
    assert_eq!(signals.repeats, 3);
    assert_eq!(signals.tool_failures, 3);
    assert!(!signals.read_only);
    assert!(signals.stuck());
    assert_eq!(
        signals.repeated.as_deref(),
        Some("Bash · cargo test -p x"),
        "the call as the board draws it"
    );

    let retrying = claude_turn(&[
        ("Bash", "cargo test -p x", true),
        ("Read", "src/lib.rs", false),
        ("Bash", "cargo test -p x", false),
    ]);
    let signals = signals_of(&turns_in(&retrying));
    assert_eq!(signals.repeats, 1);
    assert_eq!(signals.tool_failures, 1);
    assert_eq!(
        signals.repeated, None,
        "one call twice with another between is no run"
    );
    assert!(!signals.stuck());

    let reading = claude_turn(&[("Read", "a.rs", false), ("Grep", "fn main", false)]);
    let signals = signals_of(&turns_in(&reading));
    assert!(signals.read_only);
    assert_eq!(signals.repeats, 1);
    assert!(!signals.stuck());

    // Only the newest turn is read: the loop before the last prompt is over.
    let two_turns = format!("{}\n{}", looping, claude_turn(&[("Read", "a.rs", false)]));
    assert!(signals_of(&turns_in(&two_turns)).read_only);
    assert_eq!(
        signals_of(&[]),
        Signals {
            read_only: true,
            ..Signals::default()
        }
    );
}

/// Codex's rollout spells a call as a `function_call` payload; the same run
/// reads the same way, and `shell` is a writing tool.
#[test]
fn a_codex_turn_reads_the_same_way() {
    let call = |id: &str| {
        json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell",
               "call_id": id, "arguments": "{\"command\":[\"cargo\",\"test\"]}"}})
        .to_string()
    };
    let output = |id: &str| {
        json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": id,
               "output": "exit 101"}})
        .to_string()
    };
    let rollout = [
        json!({"type": "response_item", "payload": {"type": "message", "role": "user",
               "content": [{"type": "input_text", "text": "run the tests"}]}})
        .to_string(),
        call("c1"),
        output("c1"),
        call("c2"),
        output("c2"),
        call("c3"),
        output("c3"),
    ]
    .join("\n");
    let signals = signals_of(&turns_in(&rollout));
    assert_eq!(signals.repeats, 3);
    assert!(!signals.read_only);
    assert!(signals.stuck());
}

/// The rule: up once when stuck, back down once the raise did its work, down
/// on a reading turn only above the summons' word, hold otherwise — and hold
/// under a named stall cause whatever the turn did.
#[test]
fn the_rule_moves_one_rung_and_never_below_the_floor() {
    let claude = moves_of("claude");
    let stuck = Signals {
        repeats: 3,
        ..Signals::default()
    };
    let fine = Signals {
        read_only: false,
        ..Signals::default()
    };
    let reading = Signals {
        read_only: true,
        ..Signals::default()
    };
    let standing =
        |current: Option<&'static str>, floor: Option<&'static str>, raised: bool| Standing {
            current,
            floor,
            raised,
            stall_cause: None,
        };
    let at = |current, floor, raised| standing(current, floor, raised);
    assert_eq!(
        ruled(&stuck, &at(Some("medium"), Some("medium"), false), &claude),
        Move::Raise
    );
    assert_eq!(
        ruled(&stuck, &at(Some("high"), Some("medium"), true), &claude),
        Move::Hold,
        "never twice up"
    );
    assert_eq!(
        ruled(&fine, &at(Some("high"), Some("medium"), true), &claude),
        Move::Lower,
        "back down once it worked"
    );
    assert_eq!(
        ruled(&fine, &at(Some("medium"), Some("medium"), false), &claude),
        Move::Hold
    );
    assert_eq!(
        ruled(&reading, &at(Some("high"), Some("medium"), false), &claude),
        Move::Lower
    );
    assert_eq!(
        ruled(
            &reading,
            &at(Some("medium"), Some("medium"), false),
            &claude
        ),
        Move::Hold,
        "never below the floor"
    );
    assert_eq!(
        ruled(&reading, &at(Some("high"), None, false), &claude),
        Move::Hold,
        "no floor known, no lowering"
    );
    assert_eq!(
        ruled(&reading, &at(None, Some("low"), false), &claude),
        Move::Hold
    );
    // Failures alone are the stuck shape too.
    let failing = Signals {
        tool_failures: 2,
        ..Signals::default()
    };
    assert_eq!(
        ruled(&failing, &at(Some("low"), Some("low"), false), &claude),
        Move::Raise
    );
    // A silence the sweep named holds the effort where it is; one it could
    // not name does not.
    for cause in Cause::ALL {
        let held = Standing {
            stall_cause: Some(cause),
            ..at(Some("medium"), Some("medium"), false)
        };
        let expected = if more_reasoning_helps(cause) {
            Move::Raise
        } else {
            Move::Hold
        };
        assert_eq!(ruled(&stuck, &held, &claude), expected, "{}", cause.word());
    }
    assert!(more_reasoning_helps(Cause::Unknown));
    assert!(!more_reasoning_helps(Cause::QuotaWall));
}

/// The question carries the numbers, the words the row's `sends` names and
/// nothing else; the answer is read as a closed choice.
#[test]
fn the_question_carries_the_state_and_the_answer_is_a_closed_choice() {
    let signals = Signals {
        repeats: 4,
        tool_failures: 1,
        read_only: false,
        repeated: Some("Bash · just test".to_string()),
    };
    let standing = Standing {
        current: Some("medium"),
        floor: Some("medium"),
        raised: false,
        stall_cause: Some(Cause::Unknown),
    };
    let asked = ask(&StepLook {
        agent: "claude",
        signals: &signals,
        standing: &standing,
    });
    let keys: Vec<&str> = asked
        .state
        .as_object()
        .expect("state")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), STATE_KEYS.len());
    for key in STATE_KEYS {
        assert!(keys.contains(&key), "{key}");
    }
    assert_eq!(asked.state["repeated"], "Bash · just test");
    assert_eq!(asked.state["stallCause"], "unknown");
    assert_eq!(asked.state["repeats"], 4);
    assert_eq!(asked.questions[QUESTION]["type"], "choice");
    assert_eq!(
        asked.questions[QUESTION]["criteria"]
            .as_object()
            .map(|all| all.len()),
        Some(3)
    );

    let answer = |chosen: &str| {
        json!({ QUESTION: { "type": "choice", "choice": chosen,
            "probabilities": { "raise": 0.7, "lower": 0.1, "hold": 0.2 }, "confidence": 0.61 } })
    };
    let read = asked.read(&answer("raise")).expect("an answer in shape");
    assert_eq!(read.chosen, Move::Raise);
    assert_eq!(read.confidence, 0.61);
    assert_eq!(
        asked.read(&answer("faster")),
        Err(ChoiceRefusal::UnknownOption)
    );
    assert_eq!(asked.read(&json!({})), Err(ChoiceRefusal::NoAnswer));

    // A repeated call longer than the cap is cut to it before it leaves.
    let long = Signals {
        repeated: Some("x".repeat(STEP_EFFORT_REPEATED_CHAR_CAP * 2)),
        repeats: 3,
        ..Signals::default()
    };
    let asked = ask(&StepLook {
        agent: "codex",
        signals: &long,
        standing: &Standing::default(),
    });
    assert_eq!(
        asked.state["repeated"].as_str().map(str::len),
        Some(STEP_EFFORT_REPEATED_CHAR_CAP)
    );
    assert_eq!(asked.state["effort"], Value::Null);
}

/// A move's label is the next turn's shape.
#[test]
fn a_move_is_graded_by_the_turn_after_it() {
    let stuck = Signals {
        tool_failures: 2,
        ..Signals::default()
    };
    assert_eq!(Followed::of(&stuck), Followed::Stuck);
    assert_eq!(Followed::of(&Signals::default()), Followed::Progressed);
    for mv in Move::ALL {
        assert_eq!(Move::from_word(mv.word()), Some(mv));
    }
    assert_eq!(Move::from_word("faster"), None);
    assert_eq!(Followed::Nothing.word(), "none");
}

/// "The next turn went through" is not a label by itself (t-6342): it happens
/// whatever an answer nobody carried out said, and whatever an answer that
/// only repeated the rule's own move said. A move is graded only where the
/// seat's answer had an effect: it moved the effort away from the rule's
/// move, and the move reached the request or the composer.
#[test]
fn an_effort_move_is_graded_only_where_the_answer_moved_what_was_carried() {
    assert_eq!(move_mark(true, true, true), Ok(true));
    assert_eq!(move_mark(true, true, false), Ok(false));
    for progressed in [true, false] {
        assert_eq!(move_mark(true, false, progressed), Err(NOT_CARRIED));
        assert_eq!(move_mark(false, false, progressed), Err(NOT_CARRIED));
        assert_eq!(move_mark(false, true, progressed), Err(SAME_AS_RULE));
    }
}
