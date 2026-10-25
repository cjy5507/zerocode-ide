//! t-57705: the surface tells the window which panes the person closed while
//! their agent kept running, and which came back into a tab — the one fact
//! behind a coordinator seat that follows its conversation out of a closed
//! pane. Only the surface knows it, so the report is pinned where it is
//! written: one helper, one command, and one call beside each road that puts
//! a pane on the detached list or takes it off again.

use super::support::{block_after, shipped_backend, window_source};

#[test]
fn the_surface_says_which_panes_were_closed_with_their_agent_running() {
    let window = window_source();
    let helper = block_after(window, "function noteAgentDetachment(term, detached) {");
    assert!(
        helper.contains("invoke(\"note_term_detached\", { term, detached })"),
        "the helper no longer reports through the window's command:\n{helper}"
    );
    // Each road reports in the same breath as it writes the surface's own
    // list, so the two cannot disagree about a pane.
    for (opens, written) in [
        (
            "function closePaneLeaf(tab, going, hand = handNow()) {",
            "detachedAgents.set(going, tab.worktree);\n    noteAgentDetachment(going, true);",
        ),
        (
            "function adoptStrandedTerm(term, worktree, road) {",
            "detachedAgents.set(term, worktree);\n  noteAgentDetachment(term, true);",
        ),
        (
            "async function focusAgentPane(worktree, tabId, term, agent = null) {",
            "detachedAgents.delete(term);\n      noteAgentDetachment(term, false);",
        ),
    ] {
        let road = block_after(window, opens);
        assert_eq!(
            road.matches("noteAgentDetachment(").count(),
            1,
            "`{opens}` reports the detached list more than once, or not at all:\n{road}"
        );
        assert!(
            road.contains(written),
            "`{opens}` no longer reports beside the line that writes the list:\n{road}"
        );
    }

    // The command the helper names is registered, and keeps the fact where
    // the beat's host reads it.
    let backend = shipped_backend();
    assert!(
        backend.contains("            note_term_detached,\n"),
        "note_term_detached is not registered in generate_handler!"
    );
    assert!(
        block_after(
            backend,
            "pub(crate) fn note_term_detached(term: u32, detached: bool) -> bool {"
        )
        .contains("crate::orchestration::note_term_detached(term, detached)"),
        "the command no longer writes the fact the beat reads"
    );
}
