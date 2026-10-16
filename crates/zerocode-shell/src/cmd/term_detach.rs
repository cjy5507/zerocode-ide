//! The window's surface saying the person closed a pane while the agent in it
//! kept running, or brought that pane back into a tab (t-57705).
//!
//! Closing a pane that holds an agent does not end the agent — the person's
//! contract of 2026-08-21: the split goes, the session runs on, and the row
//! on its workspace card is the way back (`closePaneLeaf`, `focusAgentPane`
//! in the surface). Only the surface knows which panes stand that way, and
//! one road behind the window needs to: a coordinator's seat follows its
//! conversation out of a pane the person closed (`orchestration::chair_for`).
//! A window whose surface never says so behaves as it always did.
//!
//! A lock and a set, so it waits on nothing.

/// Write down whether the pane at `term` is closed with its agent running
/// (`detached`) or back in a tab. Answers whether the fact changed.
#[tauri::command(async)]
pub(crate) fn note_term_detached(term: u32, detached: bool) -> bool {
    crate::orchestration::note_term_detached(term, detached)
}
