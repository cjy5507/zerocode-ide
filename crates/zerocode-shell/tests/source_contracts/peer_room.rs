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
