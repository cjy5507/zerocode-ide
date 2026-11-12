//! A project's room: where the wake budget is wired, held by its text (review
//! t-105562). The budget's own tests pass whether or not the mail pass asks
//! it, and whether or not the person's input commands renew it.

use super::support::block_after;

const ORCHESTRATION: &str = include_str!("../../src/orchestration.rs");
const TERMINAL: &str = include_str!("../../src/cmd/terminal.rs");

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

/// Each command the person's typing, committed text and paste arrive by reads
/// whose hand made the call, and renews the room's budget only for the
/// person's — never for an agent's press of a window button.
#[test]
fn the_persons_input_commands_renew_the_room_only_for_the_persons_hand() {
    for command in [
        "pub(crate) fn term_key(",
        "pub(crate) fn term_text(",
        "pub(crate) fn term_paste(",
    ] {
        let body = block_after(TERMINAL, command);
        assert!(
            body.contains("hand_said(request.headers())"),
            "`{command}` no longer reads whose hand made the call"
        );
        let renews = body
            .find("renews_the_room(hand,")
            .unwrap_or_else(|| panic!("`{command}` no longer asks renews_the_room"));
        let renewed = body
            .find("peer_room::person_at(term)")
            .unwrap_or_else(|| panic!("`{command}` never renews the room's budget"));
        assert!(
            renews < renewed,
            "`{command}` renews before it asks whose hand"
        );
    }
}
