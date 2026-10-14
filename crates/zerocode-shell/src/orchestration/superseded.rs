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
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::agent_teams::Host;

/// One pane a restore road brought back.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CameBack {
    agent: String,
    /// Where its conversation is written, when the road knew; else the
    /// pane's own report says ([`Host::provider_session`]).
    transcript: Option<String>,
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

/// A restore road brought `agent`'s conversation — written at `transcript`,
/// when the road knows — back into `term`, its process started at
/// `launched_ms`.
pub(crate) fn came_back(term: u32, agent: &str, transcript: Option<&str>, launched_ms: i64) {
    panes()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .insert(
            term,
            CameBack {
                agent: agent.to_string(),
                transcript: transcript.map(str::to_string),
                launched_ms,
                hand: crate::human_input::line_of(term).1,
            },
        );
}

/// The terminal is gone, or its one look is taken: nothing of it is left to
/// move.
pub(crate) fn forget(term: u32) {
    panes()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .remove(&term);
}

/// One beat: every pane written down whose turn ended at rest, with nothing
/// else on its way to it, is read once its conversation has written a
/// record since it came back — and moved when that record ran a release the
/// catalog superseded.
pub(super) fn sweep(host: &dyn Host) {
    // Copied out, so nothing is typed under the lock.
    let written: Vec<(u32, CameBack)> = panes()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .iter()
        .map(|(term, pane)| (*term, pane.clone()))
        .collect();
    if written.is_empty() {
        return;
    }
    // The panes words are on their way to (t-52566): a line typed now would
    // wait behind them and land inside the turn they begin.
    let owed = super::continuations_owed()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clone();
    let on_their_way = host.prompts_on_their_way();
    for (term, pane) in written {
        if !host.pane_exists(term) {
            forget(term);
            continue;
        }
        // A person's hand since it came back makes the pane theirs, model
        // and all.
        if crate::human_input::line_of(term).1 != pane.hand {
            forget(term);
            continue;
        }
        if !super::turn_rests(host, term) || owed.contains(&term) || on_their_way.contains(&term) {
            continue;
        }
        // Until the window has read a lineup for the agent, nothing says
        // which releases stand; the pane waits for the read.
        if !super::lineup_held(&pane.agent) {
            continue;
        }
        let Some(raw) = pane
            .transcript
            .clone()
            .or_else(|| host.provider_session(term)?.transcript_path)
            .and_then(|path| zerocode_core::transcript::tail_lines(Path::new(&path)))
            .map(|lines| lines.join("\n"))
        else {
            continue;
        };
        // Nothing written since it came back: nothing yet says what it runs.
        let Some(ran) = zerocode_core::transcript::model_written_since(&raw, pane.launched_ms)
        else {
            continue;
        };
        forget(term);
        let Some(standing) = super::current_release(&pane.agent, &ran) else {
            continue;
        };
        let line = zerocode_core::capabilities::agent_capabilities(&pane.agent)
            .and_then(|caps| caps.moves.model)
            .and_then(|road| road.line(&standing));
        let done = match line {
            Some(line) => {
                if host.point(term, &line, true).is_some() {
                    format!("{line} went to its composer")
                } else {
                    format!("the pane could not be addressed, so its next resume names {standing}")
                }
            }
            None => format!(
                "its CLI takes no model line between turns, so its next resume names {standing}"
            ),
        };
        if let Some(root) = super::BLACKBOX.get() {
            crate::note_window_event(
                root,
                &format!(
                    "orchestration: terminal {term} ran {ran} since it came back, which the \
                     catalog superseded; {done}"
                ),
            );
        }
    }
}
