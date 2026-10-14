//! A ledger pane a restore brought back on a superseded release is moved to
//! the release that stands for it today, at the first turn end that shows it
//! (t-46882).
//!
//! The person, 10-11 16:5x: 「페이블 5.1이 있는데 5를 왜」. After a window
//! restart a coordinator's conversation came back as `claude --resume` with
//! no `--model`, and the CLI ran the selection its session had saved —
//! `claude-fable-5`, superseded by `claude-fable-5-1` in the catalog the
//! ledger itself summons from. The restore roads now name the release that
//! stands ([`super::current_release`]); this is their second half, for a
//! pane that came back before the window held a lineup to ask (the lineup is
//! read off the beat at boot, and a resume does not wait for it).
//!
//! Every road that brings a ledger pane back writes the pane down here
//! ([`came_back`]): a door's wake of a sleeper or a run's coordinator, and
//! the ledger's own reseat. A person's own tab is never written down. The
//! beat ([`sweep`]) then reads each written pane once its turn has ended at
//! rest: the model its transcript's newest record since the pane came back
//! says it ran. A release the catalog superseded is moved by the line the
//! agent's capability row names for a model (`TurnMoves::model`, `/model
//! <id>` for Claude Code and zo), once, through the guarded door every other
//! producer types through ([`Host::point`]). A row whose model only a person
//! can move (Codex's picker) is left to the next resume, which names the
//! release. Three things it never does:
//!
//! - **move a person's choice.** A pane a person's hand has reached since it
//!   came back is theirs (`human_input`), and a worker the person took over
//!   is not written down by the reseat that brings it back as taken over.
//! - **move a current release, or guess one.** Only what the lineup says
//!   stands for a superseded release is typed.
//! - **move a pane twice.** One look decides a pane, whatever it decided.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::agent_teams::Host;

/// One pane a restore road brought back.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CameBack {
    agent: String,
    /// When the pane's process started: a transcript record older than this
    /// was written by the conversation before the restart.
    launched_ms: i64,
    /// The person's hand count on the pane's line when it came back
    /// ([`crate::human_input::line_of`]); another count is a hand since.
    hand: Option<u64>,
}

fn panes() -> &'static Mutex<HashMap<u32, CameBack>> {
    static PANES: OnceLock<Mutex<HashMap<u32, CameBack>>> = OnceLock::new();
    PANES.get_or_init(Mutex::default)
}

/// A restore road brought `agent`'s conversation back into `term`, its
/// process started at `launched_ms`.
pub(crate) fn came_back(term: u32, agent: &str, launched_ms: i64) {
    panes()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .insert(
            term,
            CameBack {
                agent: agent.to_string(),
                launched_ms,
                hand: crate::human_input::line_of(term).1,
            },
        );
}

/// The terminal is gone: nothing of it is left to move.
pub(crate) fn forget(term: u32) {
    panes()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .remove(&term);
}

/// One beat: every pane written down whose turn ended at rest is read once.
pub(super) fn sweep(_host: &dyn Host, _now_ms: i64) {}
