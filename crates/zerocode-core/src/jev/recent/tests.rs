use serde_json::json;

use super::*;
use crate::jev::summary::{ANSWERED, CONTROL};

#[test]
fn the_last_requests_come_newest_first_and_the_judges_notes_are_not_among_them() {
    let rows = [
        json!({"at": 1, "outcome": ANSWERED, "chosen": "tab"}),
        json!({"at": 2, "transition": "rise", "rows": 20}),
        json!({"at": 3, "outcome": CONTROL, "probe": {}}),
        json!({"at": 4, "outcome": ANSWERED, "chosen": "split"}),
        json!({"at": 5, "outcome": "no_key"}),
    ];
    let listed = recent(&rows, 2);
    assert_eq!(
        listed.iter().map(|one| one.at).collect::<Vec<_>>(),
        [5, 4],
        "newest first, and only what was asked"
    );
    assert_eq!(listed[0].outcome, "no_key");
    assert_eq!(
        listed[0].answered,
        Value::Null,
        "a refusal answered nothing"
    );
    assert_eq!(listed[1].answered, json!("split"));
    assert!(
        recent(&rows, 0).is_empty(),
        "nothing asked for is nothing listed"
    );
    assert_eq!(
        recent(&rows, 10).len(),
        3,
        "a ledger shorter than the ask is the whole ledger"
    );
}

#[test]
fn what_was_answered_is_read_in_the_shape_the_seat_wrote_it() {
    let routing = json!({
        "at": 1, "outcome": ANSWERED, "routeUse": "applied", "task": "68212a1194a4e327",
        "jev": {
            "complexity": {"choice": "small", "probabilities": {}, "confidence": 0.26},
            "risk": {"choice": "low", "probabilities": {}, "confidence": 0.9},
        },
    });
    let recall = json!({
        "at": 2, "outcome": ANSWERED, "applied": true, "candidates": 8,
        "judged": {"recalled": ["a", "b"], "moved": 7, "top_changed": true, "dropped": []},
    });
    let screen = json!({
        "at": 3, "outcome": ANSWERED, "chosen": "mark:7", "confidence": 0.97,
        "pressed": true, "routeUse": "applied", "flow": "press the digit 7", "candidates": 12,
    });
    let one = |row: Value| recent(&[row], 1).remove(0);

    let routing = one(routing);
    assert_eq!(
        routing.answered,
        json!({"complexity": "small", "risk": "low"})
    );
    assert_eq!(routing.applied, Some(true));
    assert_eq!(
        routing.asked,
        json!({"task": "68212a1194a4e327"})
            .as_object()
            .cloned()
            .unwrap()
    );

    let recall = one(recall);
    assert_eq!(
        recall.answered,
        json!({"moved": 7, "top_changed": true}),
        "the numbers of a reorder, not its lists of names"
    );
    assert_eq!(recall.applied, Some(true));
    assert_eq!(recall.asked.get("candidates"), Some(&json!(8)));

    let screen = one(screen);
    assert_eq!(screen.answered, json!("mark:7"));
    assert_eq!(screen.confidence, Some(0.97));
    assert_eq!(screen.applied, Some(true));
    assert_eq!(screen.asked.get("flow"), Some(&json!("press the digit 7")));
}

#[test]
fn whether_an_answer_was_acted_on_is_read_off_whichever_word_the_seat_spelled() {
    let word = |row: Value| recent(&[row], 1).remove(0).applied;
    assert_eq!(
        word(json!({"at": 1, "outcome": ANSWERED, "routeUse": "applied"})),
        Some(true)
    );
    assert_eq!(
        word(json!({"at": 1, "outcome": ANSWERED, "routeUse": "fallback"})),
        Some(false)
    );
    assert_eq!(
        word(json!({"at": 1, "outcome": ANSWERED, "routeUse": "shadow", "pressed": true})),
        Some(false),
        "the word outranks the boolean beside it"
    );
    assert_eq!(
        word(json!({"at": 1, "outcome": ANSWERED, "applied": false})),
        Some(false)
    );
    assert_eq!(
        word(json!({"at": 1, "outcome": ANSWERED, "pressed": true})),
        Some(true)
    );
    assert_eq!(
        word(json!({"at": 1, "outcome": ANSWERED, "chosen": "claude"})),
        None
    );
    assert_eq!(word(json!({"at": 1, "outcome": "not_consented"})), None);
}

#[test]
fn a_label_row_written_later_is_read_onto_the_request_it_names() {
    let key = "dp-4674@1789702286717";
    let rows = [
        json!({"at": 1, "outcome": ANSWERED, "stall": key, "dispatch": "dp-4674", "worker": "w-4673",
               "chosen": "waiting_on_person", "confidence": 0.7}),
        json!({"at": 2, "outcome": ANSWERED, "stall": "dp-9@2", "chosen": "crashed"}),
        json!({"at": 9, "label": key, "run": "run-1", "worker": "w-4673", "dispatch": "dp-4674",
               "followed": "worker_done", "agreed": false, "afterMs": 1031531}),
    ];
    let listed = recent(&rows, 5);
    assert_eq!(listed.len(), 2, "a label row is not a request");
    let labelled = listed
        .iter()
        .find(|one| one.at == 1)
        .expect("the labelled request");
    assert_eq!(labelled.followed.as_deref(), Some("worker_done"));
    assert_eq!(labelled.agreed, Some(false));
    let alone = listed.iter().find(|one| one.at == 2).expect("the other");
    assert_eq!(
        alone.followed, None,
        "a request no label names has no label"
    );
    assert_eq!(alone.agreed, None);
}

#[test]
fn the_stall_seats_cause_is_its_answer() {
    let rows =
        [json!({"at": 1, "outcome": ANSWERED, "cause": "waiting_on_person", "confidence": 0.7})];
    let one = recent(&rows, 1).remove(0);
    assert_eq!(one.answered, json!("waiting_on_person"));
    assert_eq!(one.confidence, Some(0.7));
}

#[test]
fn a_mark_on_the_request_row_itself_is_read_the_same_way() {
    let rows = [json!({"at": 1, "outcome": ANSWERED, "chosen": "claude", "agreed": true})];
    assert_eq!(recent(&rows, 1)[0].agreed, Some(true));
}

#[test]
fn the_asked_facts_are_a_line_not_the_ledger() {
    let long = "x".repeat(ASKED_VALUE_CHARS + 5);
    let rows = [json!({
        "at": 1, "outcome": ANSWERED,
        "flow": long, "options": ["a", "b", "c"], "panes": 2, "worker": "w-1",
        "probabilities": {"a": 0.5, "b": 0.5}, "requestBytes": 3123, "mode": "on",
    })];
    let asked = recent(&rows, 1).remove(0).asked;
    let flow = asked.get("flow").and_then(Value::as_str).expect("the goal");
    assert_eq!(
        flow.chars().count(),
        ASKED_VALUE_CHARS + 1,
        "cut, with the mark after"
    );
    assert!(flow.ends_with(crate::jev::CUT_MARK));
    assert_eq!(
        asked.get("options"),
        Some(&json!(3)),
        "a list is its length"
    );
    assert_eq!(asked.get("panes"), Some(&json!(2)));
    assert_eq!(asked.get("worker"), Some(&json!("w-1")));
    for not_a_fact in ["probabilities", "requestBytes", "mode"] {
        assert!(
            !asked.contains_key(not_a_fact),
            "{not_a_fact} is the ledger's, not the line's"
        );
    }
    assert!(
        ASKED_KEYS
            .iter()
            .all(|key| !matches!(*key, "probabilities" | "requestBytes" | "mode")),
        "the table names facts, not bookkeeping"
    );
}

#[test]
fn every_decision_key_names_its_canonical_spelling_first() {
    for key in DECISION_KEYS {
        assert_eq!(key.spellings().next(), Some(key.canonical));
        assert!(!key.canonical.is_empty());
    }
}
