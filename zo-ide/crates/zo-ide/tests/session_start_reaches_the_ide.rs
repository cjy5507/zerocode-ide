//! Every conversation the TUI swaps in is announced to the IDE (t-56480).
//!
//! The IDE learns which conversation a pane holds from one envelope,
//! `SessionStart` (`HookReporter::session_start`), and the TUI sent it from
//! its boot alone: `/resume`, `/new` and `/clear` swapped the conversation
//! without a word. The window went on holding the conversation it heard last,
//! wrote that one into its pane record, and its next restart resumed it in the
//! pane (2026-10-15: a `/resume` in a zo pane, then the window's restart
//! brought back the conversation from before it).
//!
//! The swaps run on a real session in a real terminal, so this reads the
//! source, the way `agent_completion_host_wiring` pins its seam: each road
//! that puts a conversation in the pane says so through the one announcement,
//! and the announcement carries that conversation's own facts.

/// One method of `App`, from its head to the next method's.
fn body<'a>(source: &'a str, head: &str) -> &'a str {
    let start = source
        .find(head)
        .unwrap_or_else(|| panic!("`{head}` left tui/app.rs"));
    let rest = &source[start..];
    let end = rest[head.len()..]
        .find("\n    fn ")
        .map_or(rest.len(), |at| at + head.len());
    &rest[..end]
}

#[test]
fn every_conversation_the_tui_swaps_in_is_announced_to_the_ide() {
    let source = include_str!("../src/tui/app.rs");
    for road in [
        "fn resume_session(&mut self, id: &str)",
        "fn start_fresh_session(&mut self, name: Option<&str>, clear_terminal: bool)",
        "fn boot(&mut self)",
    ] {
        assert!(
            body(source, road).contains("self.announce_session();"),
            "`{road}` puts a conversation in the pane without telling the IDE: \
             the window keeps the one it heard last, and its next restart \
             resumes that one here"
        );
    }
    let announcing = body(source, "fn announce_session(&self)");
    for fact in [
        "reporter.session_start(",
        "session.handle.id",
        "session.handle.path",
        "session.cwd",
        "session.model",
    ] {
        assert!(
            announcing.contains(fact),
            "the announcement lost `{fact}`:\n{announcing}"
        );
    }
    assert_eq!(
        source.matches("reporter.session_start(").count(),
        1,
        "a second road announces a conversation by hand, and the two can say \
         different things"
    );
}
