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

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
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

impl WakeClaims {
    /// Whether a wake is opening `wanted` in `term` right now: the wake's
    /// spawn stands before its record is written, and what the process says
    /// in between is that wake's own conversation (t-68069).
    pub(crate) fn waking_in(&self, wanted: &ConversationKey, term: TermId) -> bool {
        self.waking
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|(held, waking)| *waking == term && held == wanted)
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

/// What the window says of a conversation a pane reported on its own
/// (t-68069). A door stands behind the conversation it opened from before the
/// process starts: a wake's claim until its record is written, or for the two
/// roads with no claim on the pane (the launch door's named conversation, the
/// ledger's reseat) the record itself, written before the spawn. So a report
/// neither answers for is one no door asked for: a "continue" typed in a shell,
/// a picker inside the CLI, a selector in the launch args. The CLI has already
/// chosen by then; what the window can do is say which, in the pane's head and
/// its log, and in the ledger when a worker sits there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportedWord {
    /// Another live pane holds this conversation: two processes on one
    /// transcript. A worker's report of it is refused by the ledger
    /// (`worker_session_reported`).
    HeldBy(TermId),
    /// It re-entered a conversation nobody named, and nobody else holds it.
    Picked,
}

impl ReportedWord {
    /// Whether the seated worker's coordinator is told by this word. A
    /// conversation another pane holds is told already, when another worker
    /// carries it, in the ledger's own refusal (`worker_session_reported`);
    /// "nobody named it" would be wrong of it besides (t-68069 P3-4).
    pub(crate) const fn tells_the_coordinator(self) -> bool {
        matches!(self, Self::Picked)
    }

    /// The word on the wire to the pane's head.
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::HeldBy(_) => "held",
            Self::Picked => "picked",
        }
    }

    /// The one line the window's log keeps of it, by the id's first letters.
    pub(crate) fn line(self, term: TermId, agent: &str, session_id: &str) -> String {
        let short = zerocode_core::session_id_prefix(session_id);
        match self {
            Self::HeldBy(holder) => format!(
                "term {term} reported {agent} {short}, which term {holder} holds: two processes on \
                 one transcript"
            ),
            Self::Picked => {
                format!("term {term} re-entered {agent} {short}, which no door named")
            }
        }
    }
}

/// What the report of `term` says against the pane's own record. `resumed` is
/// only asked for a report that differs (`hook::session_start_resumed_parsed`
/// reads the payload; a repeat costs nothing on the hook's hot road). A pane's
/// holder is only ever another pane: its own record differs by then, and a wake
/// still opening this conversation in this very pane has nothing to say.
pub(crate) fn word_on_report(
    agent: &str,
    term: TermId,
    record: Option<&ProviderSession>,
    reported: &ProviderSession,
    resumed: impl FnOnce() -> bool,
    holder: impl FnOnce(&ConversationKey) -> Option<TermId>,
) -> Option<ReportedWord> {
    let wanted = zerocode_core::conversation_key(agent, reported)?;
    if record
        .and_then(|held| zerocode_core::conversation_key(agent, held))
        .as_ref()
        == Some(&wanted)
        || WAKES.waking_in(&wanted, term)
    {
        return None;
    }
    match holder(&wanted) {
        Some(term) => Some(ReportedWord::HeldBy(term)),
        None => resumed().then_some(ReportedWord::Picked),
    }
}

/// A pane this window started, as the folder judgment reads it.
#[derive(Debug, Clone)]
struct PaneLaunch {
    /// The agent the launch started, before any report has said so.
    agent: Option<&'static str>,
    /// Where it started. On a platform with no way to ask where a process
    /// stands (`system_runtime::process_cwds`), the only folder there is until
    /// the agent's first report.
    folder: PathBuf,
}

/// Where each agent pane of this window was started, from before its process
/// exists until the pane is forgotten (t-68069). Written by each road that
/// starts an agent, because a pane whose agent has not reported yet is a
/// neighbour too: `pane_sessions` does not know it.
pub(crate) struct PaneLaunches {
    panes: Mutex<BTreeMap<TermId, PaneLaunch>>,
}

/// The window's one book, for the same reason as [`WAKES`].
pub(crate) static PANE_LAUNCHES: PaneLaunches = PaneLaunches::new();

/// What the folder judgment asks of the window: what it reads off the pane
/// tables and the terminals, asked with this book's lock already let go when
/// it may wait on a terminal.
pub(crate) trait FolderPanes {
    /// The panes whose agent the window knows of, reported or launched.
    fn agent_panes(&self) -> HashMap<TermId, &'static str>;
    /// Where `term` stands now, when the window can tell: its process's
    /// working directory, or the checkout its seat names.
    fn pane_folder(&self, term: TermId) -> Option<PathBuf>;
    /// Whether the shell, not the agent, is in front in `term`.
    fn shell_in_front(&self, term: TermId) -> bool;
}

impl FolderPanes for AppState {
    fn agent_panes(&self) -> HashMap<TermId, &'static str> {
        self.agent_terms().clone()
    }

    fn pane_folder(&self, term: TermId) -> Option<PathBuf> {
        let cwd = self.pane_cwds().get(&term).map(PathBuf::from);
        cwd.or_else(|| crate::agent_tools_runtime::pane_checkout(self, term))
    }

    fn shell_in_front(&self, term: TermId) -> bool {
        crate::agent_tools_runtime::shell_in_front_of(self, term)
    }
}

/// A pane written into the book before its process exists. Dropped without
/// [`Self::opened`], which is every road where the process never started, the
/// line is taken back: a launch that failed is nobody's neighbour.
#[must_use]
pub(crate) struct OpeningPane<'a> {
    book: &'a PaneLaunches,
    term: TermId,
    opened: bool,
}

impl OpeningPane<'_> {
    /// The process started: the line stays until the pane is forgotten.
    pub(crate) fn opened(mut self) {
        self.opened = true;
    }
}

impl Drop for OpeningPane<'_> {
    fn drop(&mut self) {
        if !self.opened {
            self.book.gone(self.term);
        }
    }
}

impl PaneLaunches {
    pub(crate) const fn new() -> Self {
        Self {
            panes: Mutex::new(BTreeMap::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<TermId, PaneLaunch>> {
        self.panes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `term` is opening `agent` in `folder`.
    pub(crate) fn opening(
        &self,
        term: TermId,
        agent: Option<&'static str>,
        folder: &Path,
    ) -> OpeningPane<'_> {
        self.lock().insert(
            term,
            PaneLaunch {
                agent,
                folder: folder.to_path_buf(),
            },
        );
        OpeningPane {
            book: self,
            term,
            opened: false,
        }
    }

    /// The pane is gone; its number may be minted again.
    pub(crate) fn gone(&self, term: TermId) {
        self.lock().remove(&term);
    }
}

/// The pane of `agent` already in the folder a new pane is about to open in,
/// lowest first, and the new pane written down (t-68069). Beside one, an
/// agent's own "continue" may take that pane's conversation, so the launch door
/// starts a new one instead and says so; zo's writer lease would refuse it, but
/// Claude keeps no lease and both of its processes would write.
///
/// The judgment and the writing happen under one lock, so of two panes opening
/// at once, the second sees the first whether or not the first has reported.
/// A pane whose current folder the window cannot tell is judged by where it was
/// started. One whose folder it knows neither way is nobody's neighbour: saying
/// "this folder" of a pane in another project cost a person the conversation
/// they meant to continue, every launch, on a platform that cannot read a
/// process's folder.
pub(crate) fn neighbour_in_folder<'a>(
    book: &'a PaneLaunches,
    window: &impl FolderPanes,
    term: TermId,
    agent: &'static str,
    root: &Path,
) -> (Option<TermId>, OpeningPane<'a>) {
    let mut standing: Vec<(TermId, Option<PathBuf>)> = Vec::new();
    let opening = {
        let mut panes = book.lock();
        let agents = window.agent_panes();
        standing.extend(
            agents
                .iter()
                .filter(|(_, held)| **held == agent)
                .map(|(pane, _)| (*pane, panes.get(pane).map(|held| held.folder.clone()))),
        );
        standing.extend(
            panes
                .iter()
                .filter(|(pane, held)| held.agent == Some(agent) && !agents.contains_key(pane))
                .map(|(pane, held)| (*pane, Some(held.folder.clone()))),
        );
        panes.insert(
            term,
            PaneLaunch {
                agent: Some(agent),
                folder: root.to_path_buf(),
            },
        );
        OpeningPane {
            book,
            term,
            opened: false,
        }
    };
    standing.sort_unstable_by_key(|(pane, _)| *pane);
    let neighbour = standing
        .into_iter()
        .find(|(pane, started)| {
            window
                .pane_folder(*pane)
                .or_else(|| started.clone())
                .is_some_and(|folder| zerocode_core::git_dir::same_folder(&folder, root))
                && !window.shell_in_front(*pane)
        })
        .map(|(pane, _)| pane);
    (neighbour, opening)
}

/// What a launch naming no conversation runs on (t-68069).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct UnnamedLaunch {
    pub(crate) args: Vec<String>,
    /// The pane a selector the person saved stood aside for.
    pub(crate) beside: Option<TermId>,
    /// A selector the person saved did not ride.
    pub(crate) stood_aside: bool,
}

/// The launch args a launch naming no conversation runs on. With no neighbour a
/// person's launch keeps the saved selector, theirs to have: the head says
/// which conversation it took once the CLI reports it ([`word_on_report`]).
///
/// A restored leaf never keeps one. It names no conversation, so it has none to
/// come back to, and the conversation a "continue" would take there may be the
/// one a leaf later in the same restore names: nothing about that leaf is a
/// pane yet, so no judgment of panes can see it. It comes back new, as a pane
/// whose own conversation was never written does (`fresh_command`), and every
/// named leaf keeps its own whatever order the restore runs in.
pub(crate) fn unnamed_launch_args(
    scrubbed: Vec<String>,
    planned: &[String],
    restored_leaf: bool,
    neighbour: Option<TermId>,
) -> UnnamedLaunch {
    let saved = scrubbed != planned;
    if !saved {
        return UnnamedLaunch {
            args: scrubbed,
            beside: None,
            stood_aside: false,
        };
    }
    if restored_leaf || neighbour.is_some() {
        return UnnamedLaunch {
            args: scrubbed,
            beside: neighbour,
            stood_aside: true,
        };
    }
    UnnamedLaunch {
        args: planned.to_vec(),
        beside: None,
        stood_aside: false,
    }
}

/// What a vault row hears when the ledger's reseat is bringing the conversation
/// back: no pane exists yet to send the person to (t-68069 P3-6).
pub(crate) const VAULT_RESEATING: &str = "이 대화는 잠든 워커가 다시 여는 중이라 새로 열지 않았습니다 — 잠시 뒤 그 워커의 판에서 볼 수 있습니다";

/// The record a vault row re-enters, keyed the way panes report it (t-68069),
/// for the agents this window can key. Everyone else's stays unjudged: an
/// agent the window does not drive reports no conversation to compare with.
pub(crate) fn vault_record(agent: &str, session_id: &str) -> Option<ProviderSession> {
    let kind = zerocode_core::AgentKind::from_slug(agent)?;
    let record = ProviderSession {
        key: zerocode_core::session_key_of(kind)?,
        id: session_id.to_string(),
        transcript_path: None,
    };
    zerocode_core::conversation_key(agent, &record).map(|_| record)
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

    /// t-68069 ②: what a pane's own report says. A door's record and a repeat
    /// say nothing and ask nothing; a conversation another pane holds is said
    /// whatever the agent said of it; one the agent says it re-entered, nobody
    /// else holding it, is said once; a new one is a new conversation.
    #[test]
    fn a_report_no_door_asked_for_is_said_once() {
        let free = |_: &ConversationKey| None;
        let mine = session(HELD);
        assert_eq!(
            word_on_report(
                ZO,
                7,
                Some(&mine),
                &mine,
                || panic!("a report matching the pane's record asks nothing"),
                |_| panic!("a report matching the pane's record asks nothing"),
            ),
            None,
            "the door's own record"
        );
        let other = session("session-1789537852228-1");
        let held = word_on_report(ZO, 8, None, &other, || false, |_| Some(7));
        assert_eq!(held, Some(ReportedWord::HeldBy(7)));
        assert!(
            !held.expect("the word").tells_the_coordinator(),
            "a conversation another pane holds was told as one nobody named"
        );
        let picked = word_on_report(ZO, 8, Some(&mine), &other, || true, free);
        assert_eq!(picked, Some(ReportedWord::Picked));
        assert!(picked.expect("the word").tells_the_coordinator());
        assert_eq!(
            word_on_report(ZO, 8, Some(&mine), &other, || false, free),
            None,
            "a new conversation is nobody's pick"
        );
        assert_eq!(
            word_on_report("nobody", 8, None, &other, || true, free),
            None
        );
    }

    /// t-68069 P2-1: a wake's process may report before the wake writes its
    /// record: zo publishes its channel, then says `source: "resume"`. That is
    /// the door's own conversation, in the pane the wake is opening. The same
    /// conversation reported by another pane is not.
    #[test]
    fn a_report_inside_its_own_wake_says_nothing() {
        const WAKING: &str = "session-1789537852228-77";
        const TERM: TermId = 680_690;
        let free = |_: &ConversationKey| None;
        let reported = session(WAKING);
        let claim = WAKES
            .claim(key(ZO, WAKING), TERM, |_| None)
            .expect("the wake's claim");
        assert_eq!(
            word_on_report(ZO, TERM, None, &reported, || true, free),
            None,
            "the wake's own process was told as a conversation nobody named"
        );
        assert_eq!(
            word_on_report(ZO, TERM + 1, None, &reported, || true, free),
            Some(ReportedWord::Picked),
            "another pane on the conversation being woken has something to say"
        );
        drop(claim);
        assert_eq!(
            word_on_report(ZO, TERM, None, &reported, || true, free),
            Some(ReportedWord::Picked),
            "a settled wake answers for nothing: by then its record does"
        );
    }

    /// A window as the folder judgment reads it.
    #[derive(Default)]
    struct Panes {
        agents: HashMap<TermId, &'static str>,
        folders: HashMap<TermId, PathBuf>,
        shells: Vec<TermId>,
    }

    impl FolderPanes for Panes {
        fn agent_panes(&self) -> HashMap<TermId, &'static str> {
            self.agents.clone()
        }
        fn pane_folder(&self, term: TermId) -> Option<PathBuf> {
            self.folders.get(&term).cloned()
        }
        fn shell_in_front(&self, term: TermId) -> bool {
            self.shells.contains(&term)
        }
    }

    const CLAUDE: &str = "claude";
    const NEW_PANE: TermId = 50;

    fn neighbour_of(book: &PaneLaunches, panes: &Panes, root: &Path) -> Option<TermId> {
        let (neighbour, opening) = neighbour_in_folder(book, panes, NEW_PANE, CLAUDE, root);
        drop(opening);
        neighbour
    }

    /// t-68069 P2-2: a window that cannot say where a pane stands now (every
    /// pane, on a platform with no `process_cwds`) judges it by where it was
    /// started, and one it knows neither way is nobody's neighbour: a person's
    /// saved "continue" is not lost to a pane in another project.
    #[test]
    fn a_pane_whose_folder_is_unknown_is_judged_by_where_it_started() {
        let here = tempfile::tempdir().expect("a folder");
        let there = tempfile::tempdir().expect("another");
        let book = PaneLaunches::new();
        let mut panes = Panes::default();
        panes.agents.insert(3, CLAUDE);

        assert_eq!(
            neighbour_of(&book, &panes, here.path()),
            None,
            "a pane the window knows nothing of the folder of was taken for this folder's"
        );

        book.opening(3, Some(CLAUDE), there.path()).opened();
        assert_eq!(
            neighbour_of(&book, &panes, here.path()),
            None,
            "a pane started in another project was taken for this folder's"
        );
        assert_eq!(
            neighbour_of(&book, &panes, there.path()),
            Some(3),
            "a pane started here, whose folder the window cannot read now"
        );

        panes.folders.insert(3, here.path().to_path_buf());
        assert_eq!(
            neighbour_of(&book, &panes, here.path()),
            Some(3),
            "where it stands now answers first"
        );
        assert_eq!(neighbour_of(&book, &panes, there.path()), None);
    }

    /// t-68069 P2-2: only a pane whose agent is this agent, and is in front.
    /// Lowest first, and a forgotten pane is nobody.
    #[test]
    fn a_neighbour_is_the_same_agent_still_in_front() {
        let here = tempfile::tempdir().expect("a folder");
        let book = PaneLaunches::new();
        let mut panes = Panes::default();
        for term in [9, 4, 7] {
            book.opening(term, Some(CLAUDE), here.path()).opened();
            panes.agents.insert(term, CLAUDE);
        }
        assert_eq!(neighbour_of(&book, &panes, here.path()), Some(4));
        panes.shells.push(4);
        assert_eq!(neighbour_of(&book, &panes, here.path()), Some(7));
        panes.agents.insert(7, "codex");
        assert_eq!(neighbour_of(&book, &panes, here.path()), Some(9));
        book.gone(9);
        panes.agents.remove(&9);
        assert_eq!(neighbour_of(&book, &panes, here.path()), None);
    }

    /// t-68069 P3-7: a link and the folder it names are one folder; `lsof`
    /// answers with the real path while the window opened the link. A path
    /// that resolves to nothing is compared as spelled.
    #[cfg(unix)]
    #[test]
    fn a_link_and_its_folder_are_one_folder() {
        let dir = tempfile::tempdir().expect("a folder");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("the real one");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("the link");
        let book = PaneLaunches::new();
        let mut panes = Panes::default();
        panes.agents.insert(3, CLAUDE);
        panes.folders.insert(3, real.clone());
        assert_eq!(neighbour_of(&book, &panes, &link), Some(3));

        let gone = dir.path().join("gone");
        panes.folders.insert(3, gone.clone());
        assert_eq!(neighbour_of(&book, &panes, &gone), Some(3));
        assert_eq!(neighbour_of(&book, &panes, &real), None);
    }

    /// t-68069 P2-3: a launch is written down with its judgment, so the pane
    /// that has not reported anything yet is a neighbour; a launch that never
    /// started is taken back.
    #[test]
    fn a_pane_opening_is_a_neighbour_before_it_reports_and_a_failed_one_is_not() {
        let here = tempfile::tempdir().expect("a folder");
        let book = PaneLaunches::new();
        let panes = Panes::default();
        let (first, opening) = neighbour_in_folder(&book, &panes, 3, CLAUDE, here.path());
        assert_eq!(first, None);
        let (second, failed) = neighbour_in_folder(&book, &panes, 4, CLAUDE, here.path());
        assert_eq!(second, Some(3), "the first pane had not reported yet");
        opening.opened();
        drop(failed);
        assert_eq!(neighbour_of(&book, &panes, here.path()), Some(3));
        assert!(
            !book.lock().contains_key(&4),
            "a launch that never started stayed a neighbour"
        );
        assert_eq!(
            neighbour_in_folder(&book, &panes, 5, "codex", here.path()).0,
            None,
            "another agent's pane is not this agent's neighbour"
        );
    }

    /// t-68069 ②, P2-3: a saved selector stands aside beside a neighbour and on
    /// a restored leaf. No selector has nothing to stand aside.
    #[test]
    fn a_saved_selector_stands_aside_only_beside_a_neighbour_or_on_a_restored_leaf() {
        let plan = |line: &[&str]| line.iter().map(ToString::to_string).collect::<Vec<_>>();
        let (base, saved) = (plan(&["--yolo"]), plan(&["--yolo", "--continue"]));
        let fresh = |beside| UnnamedLaunch {
            args: base.clone(),
            beside,
            stood_aside: beside.is_some(),
        };
        assert_eq!(
            unnamed_launch_args(base.clone(), &base, false, Some(4)),
            UnnamedLaunch {
                stood_aside: false,
                beside: None,
                args: base.clone()
            },
        );
        assert_eq!(
            unnamed_launch_args(base.clone(), &saved, false, Some(4)),
            fresh(Some(4)),
            "beside a neighbour a new conversation starts"
        );
        assert_eq!(
            unnamed_launch_args(base.clone(), &saved, true, None),
            UnnamedLaunch {
                stood_aside: true,
                ..fresh(None)
            },
            "a restored leaf names nothing to come back to"
        );
        assert_eq!(
            unnamed_launch_args(base.clone(), &saved, false, None),
            UnnamedLaunch {
                args: saved.clone(),
                beside: None,
                stood_aside: false
            },
            "alone in the folder, the person's setting stands"
        );
    }

    /// t-68069 ②: a vault row is judged by the record a pane reports, and the
    /// live pane holding it answers; an agent the window cannot key stays out.
    #[test]
    fn a_vault_row_is_judged_like_a_pane_and_opens_nothing_over_one() {
        static CLAIMS: WakeClaims = WakeClaims::new();
        let record = vault_record(ZO, HELD).expect("a zo conversation");
        assert_eq!(
            Some(key(ZO, HELD)),
            zerocode_core::conversation_key(ZO, &record)
        );
        assert_eq!(
            CLAIMS
                .claim(key(ZO, HELD), 9, |wanted| {
                    (*wanted == key(ZO, HELD)).then_some(3)
                })
                .err(),
            Some(3)
        );
        assert_eq!(vault_record("nobody", HELD), None);
        assert_eq!(vault_record(ZO, "  "), None);

        let door = include_str!("cmd/fs.rs");
        let door = &door[door.find("fn resume_vault_session(").expect("the door")..];
        for word in [
            "conversation_wake::vault_record",
            "claim_among",
            "pane_sessions()",
        ] {
            assert!(door.contains(word), "the vault's door lost {word}");
        }
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
