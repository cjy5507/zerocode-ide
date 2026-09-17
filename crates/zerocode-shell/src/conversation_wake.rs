//! One conversation, one process: what a resume door asks before it spawns.
//!
//! A conversation re-entered twice is two processes on one transcript. Zo's
//! writer lease refuses the second one, and that refusal reached the person as
//! an error over a conversation that was already coming back; Claude keeps no
//! lease, so both of its processes write. The window's earlier defence judged
//! in the webview, door by door, and one door could not see another's wake in
//! flight: on 2026-09-16 a sidebar row resumed the zo session its own
//! activation was restoring — `term 4 resumed zo`, then `term 5 refused zo:
//! the pane exited (code 1) before publishing its events channel` — five times
//! that day.
//!
//! So it is judged here, once, on the road every door takes
//! (`resume_session`). A conversation is held by a live pane that reported it
//! and whose agent still holds the terminal, or by a wake that has claimed it
//! and not yet settled. A door that finds it held spawns nothing and is told
//! which pane holds it; the webview keeps no second copy of the rule.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use zerocode_core::{ConversationKey, ProviderSession};

use crate::{AppState, ShellStateExt as _, TermId};

/// A resume door's answer: the pane it opened, or — `standing`, with nothing
/// spawned — the pane already holding the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct ConversationWake {
    pub(crate) term: TermId,
    pub(crate) standing: bool,
}

impl ConversationWake {
    pub(crate) const fn opened(term: TermId) -> Self {
        Self {
            term,
            standing: false,
        }
    }

    pub(crate) const fn standing(term: TermId) -> Self {
        Self {
            term,
            standing: true,
        }
    }
}

/// Wakes that have claimed their conversation and not yet settled.
///
/// A claim is taken before anything of its wake happens — the keychain write,
/// the trust mark, the spawn — and dropped on every road out of the wake: the
/// spawn that failed, and the one whose pane `pane_sessions` answers for from
/// then on.
pub(crate) struct WakeClaims {
    waking: Mutex<Vec<(ConversationKey, TermId)>>,
}

/// The window's one set: one process, one set of panes — the same reason the
/// keychain's write lock is process-wide.
pub(crate) static WAKES: WakeClaims = WakeClaims::new();

impl WakeClaims {
    pub(crate) const fn new() -> Self {
        Self {
            waking: Mutex::new(Vec::new()),
        }
    }

    /// Claim `wanted` for the wake opening `term`, or answer the pane that
    /// already holds it: a wake in flight first, then the live pane `holder`
    /// names. Both are asked under the claim's own lock, so two doors asking
    /// at once cannot both be told the conversation is free.
    pub(crate) fn claim(
        &self,
        wanted: ConversationKey,
        term: TermId,
        holder: impl FnOnce(&ConversationKey) -> Option<TermId>,
    ) -> Result<WakeClaim<'_>, TermId> {
        let mut waking = self.waking.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, first)) = waking.iter().find(|(held, _)| *held == wanted) {
            return Err(*first);
        }
        if let Some(standing) = holder(&wanted) {
            return Err(standing);
        }
        waking.push((wanted.clone(), term));
        Ok(WakeClaim {
            claims: self,
            wanted,
            term,
        })
    }
}

/// One wake's hold on its conversation. See [`WakeClaims`].
pub(crate) struct WakeClaim<'a> {
    claims: &'a WakeClaims,
    wanted: ConversationKey,
    term: TermId,
}

impl Drop for WakeClaim<'_> {
    fn drop(&mut self) {
        self.claims
            .waking
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(held, term)| !(*held == self.wanted && *term == self.term));
    }
}

/// The live panes whose reported conversation is `wanted`, lowest first.
///
/// Pure over the two records a pane's conversation lives in — the session it
/// reported and the agent this window knows it as — so the match is asserted
/// without a window. Whether each one's agent still holds its terminal is the
/// caller's question: it takes the terminal's lock, and these two are released
/// before it is asked.
pub(crate) fn panes_naming(
    wanted: &ConversationKey,
    sessions: &HashMap<TermId, ProviderSession>,
    agents: &HashMap<TermId, &'static str>,
) -> Vec<TermId> {
    let mut naming: Vec<TermId> = sessions
        .iter()
        .filter(|(term, session)| {
            agents
                .get(term)
                .and_then(|agent| zerocode_core::conversation_key(agent, session))
                .is_some_and(|held| held == *wanted)
        })
        .map(|(term, _)| *term)
        .collect();
    naming.sort_unstable();
    naming
}

/// Claim the conversation `session` names for the wake opening `term`, or
/// answer the pane already holding it.
///
/// `Ok(None)` for a record that names no conversation this window can key —
/// an agent it does not drive, an id that is not one; the resume command
/// refuses that record in its own words. A pane whose agent a person quit
/// back to its shell holds nothing: reopening that conversation is exactly
/// what its tab's menu is for.
pub(crate) fn claim_for_wake(
    state: &AppState,
    agent: &str,
    session: &ProviderSession,
    term: TermId,
) -> Result<Option<WakeClaim<'static>>, TermId> {
    let Some(wanted) = zerocode_core::conversation_key(agent, session) else {
        return Ok(None);
    };
    WAKES
        .claim(wanted, term, |wanted| {
            let naming = {
                let sessions = state.pane_sessions();
                let agents = state.agent_terms();
                panes_naming(wanted, &sessions, &agents)
            };
            naming
                .into_iter()
                .find(|held| !crate::agent_tools_runtime::shell_in_front_of(state, *held))
        })
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::SessionKey;

    const ZO: &str = "zo";
    const HELD: &str = "session-1789537852228-0";

    fn session(id: &str) -> ProviderSession {
        ProviderSession {
            key: SessionKey::SessionId,
            id: id.to_string(),
            transcript_path: None,
        }
    }

    fn key(agent: &str, id: &str) -> ConversationKey {
        zerocode_core::conversation_key(agent, &session(id)).expect("a conversation")
    }

    /// 2026-09-16 17:50:02, as the window lived it: switching to the workspace
    /// woke its stored zo pane (term 3), and the row the person had pressed
    /// asked for the same conversation while that wake still waited for its
    /// events channel. The second `zo --resume` (term 4) died at the lease.
    #[test]
    fn a_door_that_asks_while_the_conversation_is_waking_is_told_which_pane_it_wakes_in() {
        let claims = WakeClaims::new();
        let restoring = claims
            .claim(key(ZO, HELD), 3, |_| None)
            .expect("the restore's wake claims the conversation");
        let pressed = claims.claim(key(ZO, &format!(" {HELD} ")), 4, |_| {
            panic!("a wake in flight answers before any pane is asked")
        });
        assert_eq!(
            pressed.err(),
            Some(3),
            "the row's wake reached a second spawn"
        );
        let other = claims.claim(key(ZO, "session-1789537852228-1"), 5, |_| None);
        assert!(
            other.is_ok(),
            "another conversation is not held by this one"
        );
        drop(other);
        drop(restoring);
        assert!(
            claims.claim(key(ZO, HELD), 6, |_| None).is_ok(),
            "a wake that ended holds nothing — its failure must not lock the \
             conversation out"
        );
    }

    /// Once the wake settles, `pane_sessions` answers: the live pane that
    /// reported the conversation holds it, whichever agent's panes also carry
    /// an id spelled the same way do not, and the answer is the same pane every
    /// time.
    #[test]
    fn a_live_pane_that_reported_the_conversation_holds_it() {
        let sessions = HashMap::from([
            (7, session(HELD)),
            (3, session(&format!("{HELD}\n"))),
            (9, session("session-1789537852228-1")),
            (11, session(HELD)),
        ]);
        let agents = HashMap::from([(7, ZO), (3, ZO), (9, ZO), (11, "claude")]);
        assert_eq!(
            panes_naming(&key(ZO, HELD), &sessions, &agents),
            vec![3, 7],
            "the panes holding this conversation, lowest first"
        );
        assert_eq!(
            panes_naming(&key(ZO, HELD), &sessions, &HashMap::new()),
            Vec::<TermId>::new(),
            "a pane this window knows no agent for names no conversation"
        );

        let claims = WakeClaims::new();
        let quit = [3];
        let answer = claims.claim(key(ZO, HELD), 12, |wanted| {
            panes_naming(wanted, &sessions, &agents)
                .into_iter()
                .find(|held| !quit.contains(held))
        });
        assert_eq!(
            answer.err(),
            Some(7),
            "a pane whose agent is still in front holds the conversation"
        );
        assert!(
            claims
                .claim(key(ZO, HELD), 13, |wanted| {
                    panes_naming(wanted, &sessions, &agents)
                        .into_iter()
                        .find(|held| ![3, 7].contains(held))
                })
                .is_ok(),
            "panes whose agents were quit back to their shells hold nothing"
        );
    }

    /// What the webview reads: the term, and whether anything was spawned.
    #[test]
    fn the_answer_says_whether_this_door_opened_the_pane() {
        assert_eq!(
            serde_json::to_value(ConversationWake::opened(4)).expect("json"),
            serde_json::json!({ "term": 4, "standing": false })
        );
        assert_eq!(
            serde_json::to_value(ConversationWake::standing(3)).expect("json"),
            serde_json::json!({ "term": 3, "standing": true })
        );
    }
}
