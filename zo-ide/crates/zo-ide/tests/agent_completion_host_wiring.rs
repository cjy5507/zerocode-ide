//! Structural host seam for the interactive completion pump.
//!
//! The behavioral delivery contract lives beside the pump. This test pins the
//! deliberately tiny TUI integration separately so an idle completion cannot
//! regress into a queue that nobody selects, and so future TUI refactors do not
//! render the card without publishing it through the IDE turn frame.

#[test]
fn interactive_tui_wakes_and_runs_agent_followups_through_a_normal_turn() {
    let source = include_str!("../src/tui/app.rs");

    assert!(
        source.contains("start_agent_completion_pump"),
        "the interactive host never starts its one completion consumer"
    );
    assert_eq!(
        source.matches("recv_followup()").count(),
        1,
        "the idle loop owns exactly one completion wake arm"
    );
    assert!(
        source.contains("agent_completion_pump.begin_turn"),
        "a live turn never exposes its AgentNotificationInbox to the pump"
    );
    assert!(
        source.contains("agent_completion_pump.finish_turn"),
        "turn-tail notifications are never requeued after the runtime returns"
    );
    assert!(
        source.contains("followup.render_block(&turn_scaffold.ids)"),
        "the follow-up is not rendered with the existing AgentResult card"
    );
    assert!(
        source.contains("turn_scaffold.publish(&agent_result)"),
        "the AgentResult card bypasses the IDE turn-frame channel"
    );
}
