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
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use zerocode_core::{ConversationKey, ProviderSession};

use crate::{AppState, ShellStateExt as _, TermId};

/// A resume door's answer: the pane it opened, or — `standing`, with nothing
/// spawned — the pane already holding the conversation, or — `finished`, with
/// nothing spawned — the finished worker whose conversation it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ConversationWake {
    pub(crate) term: TermId,
    pub(crate) standing: bool,
    /// The model the opened pane's launch names, when it names one — what
    /// the window shows for the pane until its agent says (t-46882): a
    /// conversation that last ran a superseded release would otherwise wear
    /// that release's name, read off its transcript, while the CLI runs the
    /// one the launch named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    /// The finished worker whose conversation a road that read it off a
    /// stored record asked for (t-57717): nothing was started, and the door
    /// stands a plain shell in its place without carrying the conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) finished: Option<String>,
}

impl ConversationWake {
    pub(crate) const fn opened(term: TermId) -> Self {
        Self {
            term,
            standing: false,
            model: None,
            finished: None,
        }
    }

    pub(crate) const fn standing(term: TermId) -> Self {
        Self {
            term,
            standing: true,
            model: None,
            finished: None,
        }
    }

    /// Nothing opened: the conversation is finished worker `worker`'s.
    pub(crate) const fn finished(term: TermId, worker: String) -> Self {
        Self {
            term,
            standing: false,
            model: None,
            finished: Some(worker),
        }
    }

    /// The same answer, saying the model the pane's launch named.
    pub(crate) fn naming(self, model: Option<String>) -> Self {
        Self { model, ..self }
    }
}

/// The road a conversation's wake came by (t-57717): the door's own word for
/// why it is waking this conversation, which every line the wake writes
/// carries. The webview names it; a word this window does not know is no
/// road at all.
///
/// Four of them read the conversation off the window's stored record of a
/// workspace, and none of those is a person choosing it by name: the record
/// may hold a conversation that is no longer anybody's to come back. On
/// 2026-10-15 and 16 three finished workers' conversations came back that way
/// — `term 33 resumed claude <id> in …/t-54563` — each on the first visit
/// to its checkout after a restart, with no road in the line to say how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeRoad {
    /// The boot putting back the workspace in front.
    RestoreFront,
    /// The boot putting back, behind the front, a workspace whose
    /// conversations were running when the window went.
    RestoreBehind,
    /// The first visit to a workspace since the window started.
    Visit,
    /// A sleeping tab put on the stage.
    Stage,
    /// A person visiting a stored conversation's row in the sidebar.
    /// Finished work is read without starting its former agent again.
    Inspect,
    /// A person explicitly asking to reopen a conversation, such as its
    /// tab's 「이 대화 다시 열기」.
    Reopen,
    /// A door that named no road.
    Unnamed,
}

impl WakeRoad {
    /// Every road a door names, in its wire spelling.
    pub(crate) const NAMED: [WakeRoad; 6] = [
        Self::RestoreFront,
        Self::RestoreBehind,
        Self::Visit,
        Self::Stage,
        Self::Inspect,
        Self::Reopen,
    ];

    /// The road `word` names, as the webview spells it.
    pub(crate) fn named(word: Option<&str>) -> Self {
        Self::NAMED
            .into_iter()
            .find(|road| word == Some(road.word()))
            .unwrap_or(Self::Unnamed)
    }

    /// Its spelling — on the wire, and in every line the wake writes.
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::RestoreFront => "restore-front",
            Self::RestoreBehind => "restore-behind",
            Self::Visit => "visit",
            Self::Stage => "stage",
            Self::Inspect => "inspect",
            Self::Reopen => "reopen",
            Self::Unnamed => "none",
        }
    }

    /// Whether the conversation was read off the window's stored record of a
    /// workspace or its history, rather than explicitly asked to resume.
    pub(crate) const fn reads_a_stored_record(self) -> bool {
        matches!(
            self,
            Self::RestoreFront | Self::RestoreBehind | Self::Visit | Self::Stage | Self::Inspect
        )
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
    /// Rung whenever a wake settles, for a door waiting on it ([`Self::claim_settled`]).
    settled: Condvar,
    /// How many times a door has gone to sleep on [`Self::settled`] — what a
    /// test reads to know a door is truly waiting before it lets the wake go.
    #[cfg(test)]
    sleeps: std::sync::atomic::AtomicUsize,
}

/// How long a door waits for a wake already in flight on its conversation
/// to settle before it is told that wake's pane holds it (t-76434): a zo
/// resume's own channel waits twenty seconds, and the last window's leftover
/// it may end first five more (`pane_leftovers::BOOT_PLAN`).
pub(crate) const WAKE_IN_FLIGHT_PATIENCE: Duration = Duration::from_secs(30);

#[cfg(test)]
thread_local! {
    /// This thread's patience instead of [`WAKE_IN_FLIGHT_PATIENCE`], for a
    /// test whose door asks from inside the very wake it waits on — the
    /// ledger's reseat cutting a pane on the test's own thread — so the wait
    /// can only end at its deadline, and thirty seconds of it proves nothing
    /// fifty milliseconds does not (t-80605).
    pub(crate) static WAKE_IN_FLIGHT_PATIENCE_HERE: std::cell::Cell<Option<Duration>> =
        const { std::cell::Cell::new(None) };
}

fn wake_in_flight_patience() -> Duration {
    #[cfg(test)]
    if let Some(here) = WAKE_IN_FLIGHT_PATIENCE_HERE.with(std::cell::Cell::get) {
        return here;
    }
    WAKE_IN_FLIGHT_PATIENCE
}

/// The window's one set: one process, one set of panes — the same reason the
/// keychain's write lock is process-wide.
pub(crate) static WAKES: WakeClaims = WakeClaims::new();

impl WakeClaims {
    pub(crate) const fn new() -> Self {
        Self {
            waking: Mutex::new(Vec::new()),
            settled: Condvar::new(),
            #[cfg(test)]
            sleeps: std::sync::atomic::AtomicUsize::new(0),
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
        self.claim_settled(wanted, term, holder, Duration::ZERO)
    }

    /// [`Self::claim`], but a wake in flight on `wanted` is waited out for up
    /// to `patience` first (t-76434): the door is told that wake's pane holds
    /// the conversation only if it still does once the wake settles. On
    /// 2026-10-26 a restored leaf asked for the zo coordinator while term 4's
    /// wake was opening it, was told term 4 held it and stood a plain shell;
    /// half a second later term 4 died at a leftover's writer lease, and the
    /// conversation stood nowhere.
    pub(crate) fn claim_settled(
        &self,
        wanted: ConversationKey,
        term: TermId,
        holder: impl FnOnce(&ConversationKey) -> Option<TermId>,
        patience: Duration,
    ) -> Result<WakeClaim<'_>, TermId> {
        let until = Instant::now() + patience;
        let mut waking = self.waking.lock().unwrap_or_else(PoisonError::into_inner);
        while let Some(first) = waking
            .iter()
            .find(|(held, _)| *held == wanted)
            .map(|(_, first)| *first)
        {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(first);
            }
            // Counted under the lock the wait gives up, so a test that sees the
            // count and then takes the lock knows the door is asleep.
            #[cfg(test)]
            self.sleeps
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            waking = self
                .settled
                .wait_timeout(waking, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
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
        self.claims.settled.notify_all();
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
/// answer the pane already holding it: a wake in flight, or the live pane
/// `holder` names — the window's [`holding_pane`], or a test window's own
/// table (t-7812).
///
/// `Ok(None)` for a record that names no conversation this window can key —
/// an agent it does not drive, an id that is not one; the resume command
/// refuses that record in its own words. A pane whose agent a person quit
/// back to its shell holds nothing: reopening that conversation is exactly
/// what its tab's menu is for.
pub(crate) fn claim_among(
    agent: &str,
    session: &ProviderSession,
    term: TermId,
    holder: impl FnOnce(&ConversationKey) -> Option<TermId>,
) -> Result<Option<WakeClaim<'static>>, TermId> {
    let Some(wanted) = zerocode_core::conversation_key(agent, session) else {
        return Ok(None);
    };
    WAKES
        .claim_settled(wanted, term, holder, wake_in_flight_patience())
        .map(Some)
}

/// The live pane holding `wanted`, if one does: a pane that reported it and
/// whose agent still holds the terminal. The one reading both roads ask —
/// a resume door here, and the ledger's reseat through its host (t-7812).
pub(crate) fn holding_pane(state: &AppState, wanted: &ConversationKey) -> Option<TermId> {
    let naming = {
        let sessions = state.pane_sessions();
        let agents = state.agent_terms();
        panes_naming(wanted, &sessions, &agents)
    };
    naming
        .into_iter()
        .find(|held| !crate::agent_tools_runtime::shell_in_front_of(state, *held))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
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

    /// t-76434, 2026-10-26 as the window lived it: term 3's leaf asked for
    /// the zo coordinator while term 4's wake was opening it. A door that
    /// waits on a wake in flight is told that wake's pane holds the
    /// conversation only if it does once the wake settles: a wake refused at
    /// its spawn leaves it free, and the door that waited opens it itself; a
    /// wake that opened it answers its pane. With no patience the door is
    /// told at once, as before.
    #[test]
    fn a_door_waiting_on_a_refused_wake_opens_the_conversation_itself() {
        static CLAIMS: WakeClaims = WakeClaims::new();
        let patience = Duration::from_secs(5);
        let refused = CLAIMS
            .claim(key(ZO, HELD), 4, |_| None)
            .expect("term 4's wake claims the conversation");
        assert_eq!(
            CLAIMS.claim(key(ZO, HELD), 3, |_| None).err(),
            Some(4),
            "no patience is told at once"
        );
        let asked = Arc::new(AtomicBool::new(false));
        let asleep = CLAIMS.sleeps.load(SeqCst);
        let leaf = std::thread::spawn({
            let asked = Arc::clone(&asked);
            move || {
                CLAIMS
                    .claim_settled(
                        key(ZO, HELD),
                        3,
                        |_| {
                            asked.store(true, SeqCst);
                            None
                        },
                        patience,
                    )
                    .map(|claim| claim.term)
            }
        });
        door_asleep(&CLAIMS, asleep, &leaf);
        assert!(
            !asked.load(SeqCst),
            "the door asked who holds the conversation while the wake was still in flight"
        );
        drop(refused);
        assert_eq!(
            leaf.join().expect("the leaf's door"),
            Ok(3),
            "the leaf stood a plain shell over a wake that was refused"
        );
        assert!(
            asked.load(SeqCst),
            "the door never asked who holds the conversation once the wake settled"
        );

        let opened = CLAIMS
            .claim(key(ZO, HELD), 5, |_| None)
            .expect("term 5's wake claims it");
        let asked = Arc::new(AtomicBool::new(false));
        let asleep = CLAIMS.sleeps.load(SeqCst);
        let leaf = std::thread::spawn({
            let asked = Arc::clone(&asked);
            move || {
                CLAIMS
                    .claim_settled(
                        key(ZO, HELD),
                        6,
                        |_| {
                            asked.store(true, SeqCst);
                            Some(5)
                        },
                        patience,
                    )
                    .map(|claim| claim.term)
            }
        });
        door_asleep(&CLAIMS, asleep, &leaf);
        assert!(
            !asked.load(SeqCst),
            "the second door asked who holds the conversation while the wake was still in flight"
        );
        drop(opened);
        assert_eq!(
            leaf.join().expect("the second leaf's door"),
            Err(5),
            "a wake that opened the conversation holds it"
        );
        assert!(
            asked.load(SeqCst),
            "the second door never asked who holds the conversation once the wake settled"
        );
    }

    /// Waits, without sleeping, until `leaf` is asleep on `claims`' settle
    /// signal past `before` sleeps — a door that skipped the wait answers at
    /// once, so it ends the thread instead and the assertion names that. The
    /// deadline only keeps a hung door from hanging the run.
    fn door_asleep<T>(claims: &WakeClaims, before: usize, leaf: &std::thread::JoinHandle<T>) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while claims.sleeps.load(SeqCst) == before && !leaf.is_finished() {
            assert!(Instant::now() < deadline, "the door never began to wait");
            std::thread::yield_now();
        }
        assert!(
            claims.sleeps.load(SeqCst) > before && !leaf.is_finished(),
            "the door did not wait for the wake in flight: it answered before the wake settled"
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
        // t-57717: a finished worker's conversation off a stored record —
        // nothing opened, and the worker named for the shell's one line.
        assert_eq!(
            serde_json::to_value(ConversationWake::finished(5, "w-57717".to_string()))
                .expect("json"),
            serde_json::json!({ "term": 5, "standing": false, "finished": "w-57717" })
        );
    }

    /// t-57717: the road a door names, as the webview spells it — a word this
    /// window does not know is no road — and the stored-record and inspection
    /// roads that decline a finished worker's conversation. Explicit reopening
    /// remains the person's choice.
    #[test]
    fn a_door_names_its_road_and_only_a_stored_records_roads_decline() {
        let spelled: Vec<&str> = WakeRoad::NAMED.into_iter().map(WakeRoad::word).collect();
        assert_eq!(
            spelled,
            [
                "restore-front",
                "restore-behind",
                "visit",
                "stage",
                "inspect",
                "reopen"
            ]
        );
        for road in WakeRoad::NAMED {
            assert_eq!(WakeRoad::named(Some(road.word())), road);
            assert_eq!(
                road.reads_a_stored_record(),
                road != WakeRoad::Reopen,
                "{}",
                road.word()
            );
        }
        for unknown in [None, Some("boot"), Some(""), Some("Visit")] {
            let road = WakeRoad::named(unknown);
            assert_eq!(road, WakeRoad::Unnamed, "{unknown:?}");
            assert_eq!(road.word(), "none");
            assert!(!road.reads_a_stored_record());
        }
    }
}
