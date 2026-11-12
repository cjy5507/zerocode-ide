//! A project's room: where the wake budget is wired, held by its text (review
//! t-105562). The budget's own tests pass whether or not the mail pass asks
//! it, and whether or not the person's input commands renew it.

use super::support::block_after;

const ORCHESTRATION: &str = include_str!("../../src/orchestration.rs");
const TERMINAL: &str = include_str!("../../src/cmd/terminal.rs");
const TRANSACTION: &str = include_str!("../../src/prompt_transaction.rs");

/// The mail pass asks the room's budget before any road points at a pane —
/// the channel notice, the hook's shelf, and the composer below them.
#[test]
fn the_mail_pass_asks_the_rooms_budget_before_any_road_points() {
    let pass = block_after(ORCHESTRATION, "fn point_at_waiting_mail(");
    let asked = pass
        .find("peer_room::held_back(")
        .expect("the mail pass no longer asks the room's wake budget");
    for road in [
        "notice_without_composer(",
        "orchestration_pointer_mailbox::park(",
    ] {
        let at = pass
            .find(road)
            .unwrap_or_else(|| panic!("`{road}` is gone from the mail pass"));
        assert!(asked < at, "the wake budget is asked after `{road}`");
    }
}

/// Each command the person's typing, committed text and paste arrive by
/// renews the room's budget through the one door, with the call's own
/// headers; that door reads whose hand made the call before it renews — so
/// an agent's press of a window button renews nothing.
#[test]
fn the_persons_input_commands_renew_the_room_only_for_the_persons_hand() {
    for command in [
        "pub(crate) fn term_key(",
        "pub(crate) fn term_text(",
        "pub(crate) fn term_paste(",
    ] {
        let body = block_after(TERMINAL, command);
        assert!(
            body.contains("renew_the_room(request.headers(), term,"),
            "`{command}` no longer renews the room through its own headers"
        );
    }
    let door = block_after(TRANSACTION, "pub(crate) fn renew_the_room(");
    let read = door
        .find("hand_said(headers)")
        .expect("the door no longer reads whose hand made the call");
    let judged = door
        .find("renews_the_room(hand,")
        .expect("the door no longer asks renews_the_room");
    let renewed = door
        .find("peer_room::person_at(term)")
        .expect("the door never renews the room's budget");
    assert!(
        read < judged && judged < renewed,
        "the door renews before it asks whose hand"
    );
}

const PEER_ROOM: &str = include_str!("../../src/orchestration/peer_room.rs");
const INDEX: &str = include_str!("../../../../ui/index.html");
const CATALOGS: &str = include_str!("../../../../ui/shell-i18n.js");
const GUIDE: &str = include_str!("../../../../skills/orchestration/SKILL.md");

/// How often a room letter may wake a pane is one number, the window's
/// `WAKES_PER_HAND`, and every text that says it — the setting's hint in five
/// languages and the guide's room section — says that number (review
/// t-105562: the count was written in six places beside the constant).
#[test]
fn every_text_that_says_how_often_a_room_wakes_a_pane_says_the_constant() {
    let wakes: usize = PEER_ROOM
        .split_once("pub(crate) const WAKES_PER_HAND: usize = ")
        .and_then(|(_, rest)| rest.split_once(';'))
        .and_then(|(value, _)| value.trim().parse().ok())
        .expect("WAKES_PER_HAND is gone from orchestration/peer_room.rs");
    // The words each text says the number with. A new number is a new row
    // here and a new word in every text — which is the point.
    let said: &[(usize, [&str; 6])] = &[(
        3,
        [
            "세 번",
            "three times",
            "3回",
            "三次",
            "tres veces",
            "three times",
        ],
    )];
    let words = said
        .iter()
        .find(|(count, _)| *count == wakes)
        .map(|(_, words)| words)
        .unwrap_or_else(|| {
            panic!(
                "WAKES_PER_HAND is {wakes}: say it in the setting's five languages and the guide"
            )
        });
    let hint = INDEX
        .split_once("data-i18n=\"settings.harness.peerMailHint\"")
        .and_then(|(_, rest)| rest.split_once("</span>"))
        .map(|(hint, _)| hint)
        .expect("the setting's hint is gone from ui/index.html");
    assert!(
        hint.contains(words[0]),
        "the Korean hint does not say {}",
        words[0]
    );
    let catalogs: Vec<&str> = CATALOGS
        .lines()
        .filter(|line| line.contains("\"settings.harness.peerMailHint\":"))
        .collect();
    assert_eq!(catalogs.len(), 4, "the hint is not in the four catalogs");
    for (line, word) in catalogs.iter().zip(&words[1..5]) {
        assert!(
            line.contains(word),
            "a catalog's hint does not say {word}: {line}"
        );
    }
    let room = GUIDE
        .split_once("## Writing to another agent pane")
        .map(|(_, rest)| rest)
        .expect("the guide's room section is gone");
    assert!(
        room.contains(words[5]),
        "the guide's room section does not say {}",
        words[5]
    );
}
