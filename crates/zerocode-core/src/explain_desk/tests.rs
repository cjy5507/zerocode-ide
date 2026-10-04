//! What each event of a pane's life does to the requests that went to it
//! (t-32787). One event in, the answer out; no clock and no pane.

use super::*;
use crate::explain::{ACTIVE_MAX, why};

fn held(id: &str, term: Option<Term>, state: State) -> Held {
    Held {
        id: id.to_string(),
        term,
        agent: "claude".to_string(),
        state,
        made_ms: 1_000,
        prompt: None,
    }
}

fn idle(agent: &str) -> PaneFacts<'_> {
    PaneFacts {
        present: true,
        agent: Some(agent),
        ..PaneFacts::default()
    }
}

#[test]
fn a_pane_is_decided_by_what_is_a_persons_before_what_is_busy() {
    let nothing = PaneFacts::default();
    assert_eq!(decide(&nothing), Decision::Refuse(why::NO_PANE));
    assert_eq!(
        decide(&PaneFacts {
            present: true,
            ..nothing
        }),
        Decision::Refuse(why::NO_AGENT)
    );
    assert_eq!(decide(&idle("codex")), Decision::Send);
    assert_eq!(
        decide(&PaneFacts {
            busy: true,
            ..idle("codex")
        }),
        Decision::Wait
    );
    assert_eq!(
        decide(&PaneFacts {
            draft: true,
            ..idle("codex")
        }),
        Decision::Refuse(why::HOLDS_A_DRAFT)
    );
    assert_eq!(
        decide(&PaneFacts {
            parked: true,
            ..idle("codex")
        }),
        Decision::Refuse(why::PARKED)
    );
    // A person's words and a person's question beat waiting: the door never
    // queues itself behind them.
    assert_eq!(
        decide(&PaneFacts {
            busy: true,
            draft: true,
            ..idle("codex")
        }),
        Decision::Refuse(why::HOLDS_A_DRAFT)
    );
    assert_eq!(
        decide(&PaneFacts {
            busy: true,
            parked: true,
            draft: true,
            ..idle("codex")
        }),
        Decision::Refuse(why::PARKED)
    );
}

#[test]
fn a_pane_holds_one_request_and_the_table_holds_sixteen() {
    let mut desk = Desk::default();
    assert!(desk.is_empty());
    assert_eq!(desk.admit(held("a", Some(7), State::Waiting)), Ok(()));
    assert_eq!(
        desk.admit(held("b", Some(7), State::Waiting)),
        Err(why::IN_FLIGHT)
    );
    assert_eq!(desk.admit(held("c", Some(8), State::Waiting)), Ok(()));
    // One-shots have no pane, so they do not collide with each other.
    assert_eq!(desk.admit(held("d", None, State::Running)), Ok(()));
    assert_eq!(desk.admit(held("e", None, State::Running)), Ok(()));
    assert_eq!(desk.len(), 4);
    for n in 0..ACTIVE_MAX {
        let term = 100 + u32::try_from(n).unwrap_or_default();
        let _ = desk.admit(held(&format!("f{n}"), Some(term), State::Asked));
    }
    assert_eq!(desk.len(), ACTIVE_MAX);
    assert_eq!(
        desk.admit(held("late", Some(999), State::Waiting)),
        Err(why::TOO_MANY)
    );
}

#[test]
fn a_waiting_request_is_released_once_with_its_words_when_its_pane_is_between_turns() {
    let mut desk = Desk::default();
    let mut waiting = held("w", Some(7), State::Waiting);
    waiting.prompt = Some("the words".to_string());
    assert_eq!(desk.admit(waiting), Ok(()));
    assert_eq!(desk.admit(held("other", Some(8), State::Waiting)), Ok(()));

    assert_eq!(
        desk.release_waiting(9),
        None,
        "another pane's turn end releases nothing"
    );
    let released = desk.release_waiting(7);
    assert_eq!(released.as_ref().map(|one| one.id.as_str()), Some("w"));
    assert_eq!(released.as_ref().map(|one| one.state), Some(State::Sent));
    assert_eq!(
        released.as_ref().and_then(|one| one.prompt.as_deref()),
        Some("the words")
    );
    assert_eq!(desk.release_waiting(7), None, "released once");
    assert_eq!(desk.get("w").map(|one| one.state), Some(State::Sent));
    assert_eq!(
        desk.get("w").and_then(|one| one.prompt.as_deref()),
        None,
        "the desk keeps no copy of the words"
    );
    assert_eq!(desk.get("other").map(|one| one.state), Some(State::Waiting));
}

#[test]
fn a_page_from_the_asked_pane_after_the_request_finishes_it_and_nothing_else_does() {
    let mut desk = Desk::default();
    assert_eq!(desk.admit(held("asked", Some(7), State::Asked)), Ok(()));
    assert_eq!(desk.admit(held("waits", Some(8), State::Waiting)), Ok(()));
    assert_eq!(desk.admit(held("sent", Some(5), State::Sent)), Ok(()));
    assert_eq!(
        desk.page_published(8, 5_000),
        None,
        "a request that has asked nothing is not answered"
    );
    assert_eq!(
        desk.page_published(9, 5_000),
        None,
        "another pane's page is not its page"
    );
    assert_eq!(
        desk.page_published(7, 999),
        None,
        "a page from before the request is not its answer"
    );
    assert_eq!(desk.page_published(7, 5_000), Some("asked".to_string()));
    assert_eq!(desk.get("asked"), None, "finished means gone");
    assert_eq!(desk.page_published(7, 6_000), None);
    assert_eq!(
        desk.page_published(5, 2_000),
        Some("sent".to_string()),
        "an agent quick enough to publish before its delivery is confirmed"
    );
}

#[test]
fn a_turn_that_ends_without_a_page_fails_only_a_request_that_was_asked() {
    let mut desk = Desk::default();
    assert_eq!(desk.admit(held("asked", Some(7), State::Asked)), Ok(()));
    assert_eq!(desk.admit(held("sent", Some(8), State::Sent)), Ok(()));
    assert_eq!(desk.admit(held("waits", Some(9), State::Waiting)), Ok(()));
    assert_eq!(
        desk.turn_ended(8),
        None,
        "the turn that ended was not the one that took the words"
    );
    assert_eq!(desk.turn_ended(9), None);
    assert_eq!(desk.turn_ended(7), Some("asked".to_string()));
    assert_eq!(desk.turn_ended(7), None);
    assert_eq!(desk.len(), 2);
}

#[test]
fn a_pane_that_goes_takes_its_requests_with_it() {
    let mut desk = Desk::default();
    assert_eq!(desk.admit(held("a", Some(7), State::Asked)), Ok(()));
    assert_eq!(desk.admit(held("b", Some(8), State::Asked)), Ok(()));
    assert_eq!(desk.admit(held("once", None, State::Running)), Ok(()));
    assert_eq!(desk.pane_gone(7), vec!["a".to_string()]);
    assert_eq!(desk.pane_gone(7), Vec::<String>::new());
    assert_eq!(
        desk.len(),
        2,
        "the other pane's request and the one-shot stand"
    );
}

#[test]
fn a_request_moves_forward_and_leaves_when_it_finishes() {
    let mut desk = Desk::default();
    assert_eq!(desk.admit(held("a", Some(7), State::Sent)), Ok(()));
    assert!(desk.move_to("a", State::Asked));
    assert!(!desk.move_to("nobody", State::Asked));
    assert_eq!(desk.get("a").map(|one| one.state), Some(State::Asked));
    assert_eq!(desk.finish("a").map(|one| one.id), Some("a".to_string()));
    assert_eq!(desk.finish("a"), None);
    assert!(desk.is_empty());
}

#[test]
fn the_wire_words_of_the_states_are_the_ones_the_window_reads() {
    assert_eq!(
        State::ALL.map(State::word),
        ["waiting", "sent", "asked", "running", "ready", "failed"]
    );
}
