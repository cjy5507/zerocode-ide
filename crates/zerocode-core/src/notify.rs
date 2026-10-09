//! When the machine may ring, and what it says.
//!
//! Measured from Orca **1.4.169**. An agent that needs somebody rings an OS
//! notification (out/main/index.js:133984-134019); one that finished rings a
//! completion. Every rule here exists to keep the ringing from becoming noise,
//! because a notification that fires too often is one the person turns off —
//! and then the agent that genuinely needs them waits forever.
//!
//! - **Cooldown, per worktree** (`NOTIFICATION_COOLDOWN_MS = 5000`,
//!   main:134119, keyed :134325-134334). Per worktree and not global: one busy
//!   workspace must not silence every other one.
//! - **Suppression at the screen being watched** (:134435): the pane's
//!   worktree is the active one AND the window has focus. A notification about
//!   the thing in front of you is an interruption about nothing.
//! - **A quiet moment before "finished"** (`HOOK_DONE_QUIET_MS = 1500`,
//!   index-ftls8Hg_.js:15711, :15961-15981): agents end a turn and immediately
//!   start the next; ringing on every Done would ring mid-conversation. The
//!   ring is armed, and fires only if the state is still Done when the quiet
//!   ends.
//!
//! The DECISIONS live here, pure and testable; the shell owns the clock, the
//! focus fact, and the OS call.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::hook::HookState;
use crate::lane::LaneState;

/// Per-worktree quiet period between rings.
pub const COOLDOWN_MS: i64 = 5_000;

/// How long a `Done` must stand before it counts as finished.
pub const DONE_QUIET_MS: i64 = 1_500;

/// How long an attention stop (막힘·질문) must stand before it rings.
pub const ATTENTION_QUIET_MS: i64 = 10_000;

/// How long a finished turn must stand before it rings (끝남).
pub const FINISHED_QUIET_MS: i64 = 60_000;

/// The shortest turn that earns a finish ring under [`FinishRing::Long`].
pub const LONG_TURN_MS: i64 = 60_000;

/// RED STUB — today's behaviour: no wait at all.
#[must_use]
pub const fn quiet_ms(_ring: Ring) -> i64 {
    0
}

/// Which finishes ring (`알림` → 끝남). Per device, in the settings card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FinishRing {
    /// No finish rings, and no finish is listed as waiting.
    Off,
    /// Only a turn that ran for at least [`LONG_TURN_MS`] rings.
    #[default]
    Long,
    /// Every finish that stands its quiet rings.
    Always,
}

impl FinishRing {
    /// Every mode, in the order the settings card offers them.
    pub const ALL: [Self; 3] = [Self::Off, Self::Long, Self::Always];

    /// The word the settings file keeps for this mode.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Long => "long",
            Self::Always => "always",
        }
    }

    /// The mode a settings word names, if it names one.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.word() == word)
    }

    /// RED STUB — today's behaviour: every finish rings.
    #[must_use]
    pub fn earns(self, _turn_ms: Option<i64>) -> bool {
        true
    }
}

/// How long a turn ran, when its start is known. RED STUB.
#[must_use]
pub const fn turn_ms(_turn_started_at_ms: Option<i64>, _ended_at_ms: i64) -> Option<i64> {
    None
}

/// When the turn a pane is in began, after one report. RED STUB.
#[must_use]
pub const fn turn_started_after(
    _prior: Option<(HookState, Option<i64>)>,
    _next: HookState,
    _now_ms: i64,
) -> Option<i64> {
    None
}

/// Whether the pane still shows a finish nobody has looked at. RED STUB.
#[must_use]
pub const fn finish_mark_after(
    _prior_mark: bool,
    _next: HookState,
    _prior: Option<HookState>,
    _interrupted: bool,
    _session_boundary: bool,
) -> bool {
    false
}

/// What a pane is waiting on the person for (`나를 기다림`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    /// A permission or a stop nobody else can clear.
    Blocked,
    /// A question the agent asked.
    Question,
    /// A finished turn nobody has looked at.
    Finished,
}

/// The facts the waiting list reads about one pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub state: HookState,
    /// Whether the stop carries a question.
    pub asking: bool,
    /// Whether the finish mark stands.
    pub mark: bool,
    /// How long the turn ran, when it is known.
    pub turn_ms: Option<i64>,
}

/// What a pane waits on the person for, under a finish mode. RED STUB.
#[must_use]
pub const fn waiting_for(_look: Look, _mode: FinishRing) -> Option<Waiting> {
    None
}

/// Rings that wait for their stop to keep standing. Each armed ring fires at
/// most once, and only when its stop still stands at the due time.
#[derive(Debug)]
pub struct Quiet<K> {
    armed: HashMap<K, Armed>,
}

#[derive(Debug, Clone, Copy)]
struct Armed {
    ring: Ring,
    stamp: i64,
}

impl<K> Default for Quiet<K> {
    fn default() -> Self {
        Self {
            armed: HashMap::new(),
        }
    }
}

impl<K: Eq + std::hash::Hash> Quiet<K> {
    /// Arm `ring` for the stop stamped `stamp`, at `now_ms`.
    pub fn arm(&mut self, key: K, ring: Ring, stamp: i64, _now_ms: i64) {
        self.armed.insert(key, Armed { ring, stamp });
    }

    /// Cancel any armed ring for `key`.
    pub fn cancel(&mut self, key: &K) {
        self.armed.remove(key);
    }

    /// RED STUB — today's behaviour: the ring fires at once, whatever the
    /// clock and the stop say.
    pub fn settle(&mut self, key: &K, _now_ms: i64, _standing: Option<i64>) -> Option<Ring> {
        self.armed.remove(key).map(|armed| armed.ring)
    }
}

/// Which kind of ring a state earns. `Working` earns none — progress is what
/// the board is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ring {
    /// The agent stopped and a person is the only thing that will move it.
    Attention,
    /// The agent finished its turn.
    Completion,
    /// The agent asked for the person on purpose — zo's `PushNotification`
    /// (t-2943), arriving as a `notify` frame on the window road. It rings
    /// like attention (it is the agent pulling a person toward something to
    /// act on) and wears the words the agent chose.
    Push,
}

impl Ring {
    /// Every kind, in the order the notify seat's ledger lists them.
    pub const ALL: [Self; 3] = [Self::Attention, Self::Completion, Self::Push];

    /// The word a ledger row keeps for this kind (t-6043).
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Attention => "attention",
            Self::Completion => "completion",
            Self::Push => "push",
        }
    }

    /// The kind a ledger word names, if it names one.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ring| ring.word() == word)
    }
}

/// The ring a hook state means, if any.
pub const fn ring_of(state: HookState) -> Option<Ring> {
    match state {
        HookState::NeedsAttention => Some(Ring::Attention),
        HookState::Done => Some(Ring::Completion),
        // `Idle` rings NOTHING, and that is the whole reason it is not spelled
        // `Done`. It is observed — the agent's process left the terminal — and
        // the person who observed it first is the person who quit the agent.
        // Telling them their agent finished, on the lock screen, seconds after
        // they typed the thing that ended it, is a notification that reports
        // their own keystroke back to them.
        HookState::Working | HookState::Idle => None,
    }
}

/// The ring a lane state means, if any.
///
/// The same two stops as [`ring_of`], read off the states a supervised session
/// actually publishes: a lane that stopped on a permission gate or on something
/// it cannot resolve alone needs a person, and a lane whose process ended
/// finished its work. `Streaming` and `Idle` ring nothing — progress is what the
/// board is for, and an idle lane is not asking for anything.
///
/// `Exited` rings IMMEDIATELY, with no [`DONE_QUIET_MS`] wait. The quiet exists
/// because a hook-reporting agent ends a turn and starts the next one within
/// the second, so a `Done` is usually mid-conversation and worth re-checking.
/// An exited lane is a dead process: nothing can supersede it, and waiting
/// would only delay the one ring that is certainly final.
pub const fn ring_of_lane(state: LaneState) -> Option<Ring> {
    match state {
        LaneState::AwaitingPermission | LaneState::Blocked => Some(Ring::Attention),
        LaneState::Exited => Some(Ring::Completion),
        LaneState::Streaming | LaneState::Idle => None,
    }
}

/// What the notification says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// The three words a ring can wear, named once so no second spelling of any of
/// them can appear.
///
/// Orca derives all three from one expression
/// (`main/ipc/notification-options.ts:60-65`): `blocked`/`waiting` earn
/// [`VERB_ATTENTION`], and a `done` splits on whether the person stopped it —
/// [`VERB_STOPPED`] if so, [`VERB_FINISHED`] otherwise.
pub const VERB_ATTENTION: &str = "needs input";
/// A turn the agent ended itself.
pub const VERB_FINISHED: &str = "finished";
/// A turn a PERSON ended. Orca renames the ring rather than silencing it —
/// somebody who pressed Ctrl+C still gets told the pane came to rest, they are
/// just not told the agent finished something.
pub const VERB_STOPPED: &str = "stopped";
/// The agent's own words, sent on purpose: the title says who is talking and
/// the body is what they said.
pub const VERB_PUSH: &str = "says";

/// The word this ring wears.
///
/// `interrupted` is only ever consulted for a completion, which is Orca's own
/// shape: the `done` arm is the only one that reads it, so an interrupted
/// pane that is WAITING still says it needs input.
#[must_use]
pub const fn verb(ring: Ring, interrupted: bool) -> &'static str {
    match ring {
        Ring::Attention => VERB_ATTENTION,
        Ring::Completion if interrupted => VERB_STOPPED,
        Ring::Completion => VERB_FINISHED,
        Ring::Push => VERB_PUSH,
    }
}

/// The ring a verb names — the verb table read backwards. `interrupted`
/// is lost on the way back, as it is only a completion's one bit
/// ([`verb`]): both `finished` and `stopped` name a completion. `None` for a
/// word that is no verb of this table.
///
/// One reading for every replay that rebuilds a ring from a seed's verb
/// (the notify seat's, the question search's): a second match on the four
/// words would be a second table.
#[must_use]
pub fn from_verb(verb: &str) -> Option<Ring> {
    match verb {
        VERB_ATTENTION => Some(Ring::Attention),
        VERB_FINISHED | VERB_STOPPED => Some(Ring::Completion),
        VERB_PUSH => Some(Ring::Push),
        _ => None,
    }
}

/// Build the notice. Title is Orca's own sentence shape —
/// "`{worktree} - {agent} needs input|finished|stopped`"
/// (`main/ipc/notification-options.ts:56-71`) — and the body is the agent's
/// question when it asked one, else its last words (:134009-134019). Empty body
/// stays empty: inventing a body would put words in the agent's mouth on the
/// lock screen.
///
/// `interrupted` rides beside `ring` rather than at the end because the two of
/// them together decide one thing — the verb — and Orca reads them in one
/// expression for that reason.
pub fn notice(
    worktree: &str,
    agent: &str,
    ring: Ring,
    interrupted: bool,
    ask: Option<&str>,
    said: Option<&str>,
) -> Notice {
    let verb = verb(ring, interrupted);
    let place = if worktree.is_empty() { "?" } else { worktree };
    let body = match ring {
        // The question beats the last answer: what the person must READ to
        // unblock the agent is the question.
        Ring::Attention => ask.or(said),
        // A push has no question; its body is the message the agent wrote.
        Ring::Completion | Ring::Push => said,
    };
    Notice {
        title: format!("{place} - {agent} {verb}"),
        body: body.unwrap_or_default().to_string(),
    }
}

/// Several notices the notify seat held (`crate::jev::NOTIFY_BATCH`), folded
/// into the one the person is told at their next hand on the window
/// (t-6043).
///
/// One notice stands as it is. More than one wear the first's title with
/// how many stand behind it, and every title as the body, one per line —
/// the OS shows a title and a few lines, and "api - claude finished +2"
/// over three titles is what tells somebody what waited for them without
/// putting words in any agent's mouth. `None` when nothing was held.
#[must_use]
pub fn batched(held: &[Notice]) -> Option<Notice> {
    let (first, rest) = held.split_first()?;
    if rest.is_empty() {
        return Some(first.clone());
    }
    Some(Notice {
        title: format!("{} +{}", first.title, rest.len()),
        body: held
            .iter()
            .map(|notice| notice.title.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    })
}

/// The lane bell's memory: which state each lane last rang for, and the
/// decision that follows from a fresh look.
///
/// The DECISION lives here — pure, behind behaviour tests — because a review
/// defeated three successive textual gates over the same inline logic: a
/// substring survived a discarded block, a line-equality pin survived inside a
/// nested unused block, and each defeat kept the required text while changing
/// what ran. Logic that a unit test can call cannot be defeated by
/// re-spelling; what remains in the shell is two lines of glue.
#[derive(Debug, Default)]
pub struct LaneBell {
    last: HashMap<crate::lane::LaneId, LaneState>,
}

impl LaneBell {
    /// One fresh look at a lane: what the registry says NOW, not what an
    /// event snapshot said when it was queued — snapshots queue, and
    /// bookkeeping from them walks this memory backwards under thread
    /// interleaving (a re-ring for a gate the person already answered).
    ///
    /// `None` means the registry no longer holds the lane: it was closed, the
    /// memory goes, and nothing rings for a lane nobody can reach.
    ///
    /// An `Exited` lane STAYS in the memory until [`Self::forget`] — it used
    /// to be dropped immediately, and a straggling duplicate event for the
    /// dead lane then looked like a first appearance and rang the same
    /// completion twice. The registry keeps exited lanes on the rail until
    /// they are closed, so the memory keeping them too is the same bound.
    pub fn observe(&mut self, id: crate::lane::LaneId, current: Option<LaneState>) -> Option<Ring> {
        let Some(now) = current else {
            self.last.remove(&id);
            return None;
        };
        let moved = self.last.insert(id, now) != Some(now);
        if moved { ring_of_lane(now) } else { None }
    }

    /// The lane was closed by hand: its memory goes with it.
    pub fn forget(&mut self, id: crate::lane::LaneId) {
        self.last.remove(&id);
    }
}

/// The cooldown's memory: when each worktree last rang.
#[derive(Debug, Default)]
pub struct RingLedger {
    last: HashMap<String, i64>,
}

impl RingLedger {
    /// May this worktree ring now? Records the ring when it says yes.
    ///
    /// Asked LAST, after suppression: a suppressed ring must not spend the
    /// cooldown, or working at the active screen would silence the same
    /// worktree's genuine ring five seconds after you look away.
    pub fn may_ring(&mut self, worktree: &str, now_ms: i64) -> bool {
        if let Some(&rang) = self.last.get(worktree)
            && now_ms.saturating_sub(rang) < COOLDOWN_MS
        {
            return false;
        }
        self.last.insert(worktree.to_string(), now_ms);
        true
    }
}

/// Is this ring about the screen the person is already watching?
///
/// Both facts at once: the pane's worktree is the active one AND the window
/// has focus. Active-but-unfocused still rings — the person is in another app
/// and cannot see the pane, however active its worktree is.
pub fn suppressed(pane_worktree: &str, active_worktree: &str, window_focused: bool) -> bool {
    window_focused && !pane_worktree.is_empty() && pane_worktree == active_worktree
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verb_reads_back_to_the_ring_that_wore_it() {
        for ring in Ring::ALL {
            for interrupted in [false, true] {
                assert_eq!(
                    from_verb(verb(ring, interrupted)),
                    Some(ring),
                    "{ring:?} interrupted={interrupted}"
                );
            }
        }
        assert_eq!(from_verb("working"), None);
    }

    /// Working rings nothing — progress is the board's job, and a ring per
    /// tool call is how notifications get turned off.
    #[test]
    fn only_the_stops_ring() {
        assert_eq!(ring_of(HookState::Working), None);
        assert_eq!(ring_of(HookState::NeedsAttention), Some(Ring::Attention));
        assert_eq!(ring_of(HookState::Done), Some(Ring::Completion));
    }

    /// And the observed stop rings nothing at all, which is the one behaviour
    /// that makes it a separate state from `Done`.
    ///
    /// An agent that finished a turn is news. An agent that is no longer there
    /// is not: the window learned it by watching the terminal, a second or so
    /// after the person watching that same terminal typed the thing that
    /// ended it.
    #[test]
    fn a_pane_whose_agent_left_does_not_ring() {
        assert_eq!(ring_of(HookState::Idle), None);
    }

    /// A lane rings for the same reasons a hook does — the supervised path
    /// must not be a quieter product than the hook path.
    #[test]
    fn the_lane_states_ring_like_the_hook_states() {
        assert_eq!(ring_of_lane(LaneState::Idle), None);
        assert_eq!(ring_of_lane(LaneState::Streaming), None);
        assert_eq!(
            ring_of_lane(LaneState::AwaitingPermission),
            Some(Ring::Attention)
        );
        assert_eq!(ring_of_lane(LaneState::Blocked), Some(Ring::Attention));
        // A dead process, not a turn that might continue: no quiet to wait out.
        assert_eq!(ring_of_lane(LaneState::Exited), Some(Ring::Completion));
    }

    /// The bell follows a CHANGE the registry can still vouch for — not an
    /// event, not a snapshot, and never the same state twice. These are
    /// behaviour tests because three successive textual gates over the same
    /// inline logic were each defeated while keeping their required text;
    /// a decision a test can CALL cannot be defeated by re-spelling.
    #[test]
    fn the_bell_follows_the_change_and_not_the_update() {
        use crate::lane::LaneId;
        let mut bell = LaneBell::default();
        let lane = LaneId::random();

        // First appearance in a ringing state rings: an agent that opens
        // straight into a permission gate is exactly the one nobody watches.
        assert_eq!(
            bell.observe(lane, Some(LaneState::AwaitingPermission)),
            Some(Ring::Attention)
        );
        // The same state again — a title update, a repaint — is not a change.
        assert_eq!(
            bell.observe(lane, Some(LaneState::AwaitingPermission)),
            None
        );
        assert_eq!(
            bell.observe(lane, Some(LaneState::AwaitingPermission)),
            None
        );
        // Moving between the two attention states IS a change and rings.
        assert_eq!(
            bell.observe(lane, Some(LaneState::Blocked)),
            Some(Ring::Attention)
        );
        // Progress rings nothing.
        assert_eq!(bell.observe(lane, Some(LaneState::Streaming)), None);
        // The end rings once —
        assert_eq!(
            bell.observe(lane, Some(LaneState::Exited)),
            Some(Ring::Completion)
        );
        // — and a straggling duplicate for the dead lane is NOT a first
        // appearance. The old memory dropped Exited entries immediately, and
        // this exact case rang the same completion twice.
        assert_eq!(bell.observe(lane, Some(LaneState::Exited)), None);

        // Gone from the registry means closed: the memory goes, nothing
        // rings, and the next occupant of a fresh id starts clean.
        assert_eq!(bell.observe(lane, None), None);
        let reborn = LaneId::random();
        assert_eq!(bell.observe(reborn, Some(LaneState::Idle)), None);
        assert_eq!(
            bell.observe(reborn, Some(LaneState::Exited)),
            Some(Ring::Completion)
        );
        // forget() is close_lane's half of the same rule.
        bell.forget(reborn);
        assert_eq!(
            bell.observe(reborn, Some(LaneState::Exited)),
            Some(Ring::Completion),
            "a forgotten lane's state is a first appearance again"
        );
    }

    /// The cooldown is per WORKTREE: one busy workspace must not silence the
    /// others.
    #[test]
    fn one_busy_workspace_does_not_silence_the_others() {
        let mut rings = RingLedger::default();
        assert!(rings.may_ring("/w/a", 1_000));
        // Same worktree, inside the window: quiet.
        assert!(!rings.may_ring("/w/a", 2_000));
        // A DIFFERENT worktree in the same moment: rings.
        assert!(rings.may_ring("/w/b", 2_000));
        // The window passes and the first speaks again.
        assert!(rings.may_ring("/w/a", 1_000 + COOLDOWN_MS));
        // A refused ring did not reset the clock: the allowed ring at 6000
        // is what the next window counts from.
        assert!(!rings.may_ring("/w/a", 1_000 + COOLDOWN_MS + 1));
    }

    /// Suppressed only at the screen being watched — both facts at once.
    #[test]
    fn the_screen_you_are_watching_never_rings() {
        assert!(suppressed("/w/a", "/w/a", true));
        // Same worktree, window in the background: the person cannot see it.
        assert!(!suppressed("/w/a", "/w/a", false));
        // Another worktree, however focused the window: they are not looking
        // at this pane.
        assert!(!suppressed("/w/b", "/w/a", true));
        // A pane whose worktree nobody recorded matches nothing.
        assert!(!suppressed("", "", true));
    }

    /// The words are Orca's sentence, and the question beats the last answer
    /// when the agent is stuck on one.
    #[test]
    fn the_notice_says_who_where_and_what_it_wants() {
        let stuck = notice(
            "api",
            "claude",
            Ring::Attention,
            false,
            Some("rm -rf를 실행할까요?"),
            Some("마지막 답변"),
        );
        assert_eq!(stuck.title, "api - claude needs input");
        assert_eq!(stuck.body, "rm -rf를 실행할까요?");

        // No question: the last words stand in.
        let stuck = notice(
            "api",
            "claude",
            Ring::Attention,
            false,
            None,
            Some("말한 것"),
        );
        assert_eq!(stuck.body, "말한 것");

        let done = notice(
            "api",
            "codex",
            Ring::Completion,
            false,
            None,
            Some("테스트 통과"),
        );
        assert_eq!(done.title, "api - codex finished");
        assert_eq!(done.body, "테스트 통과");

        // Nothing to say is an empty body, not an invented one.
        let quiet = notice("api", "codex", Ring::Completion, false, None, None);
        assert_eq!(quiet.body, "");
        // And a worktree nobody named does not render as an empty gap.
        assert_eq!(
            notice("", "codex", Ring::Completion, false, None, None).title,
            "? - codex finished"
        );
    }

    /// 사람이 멈춘 턴은 **울리되 다른 낱말을 입는다** — 침묵이 아니다
    /// (`notification-options.ts:60-65`). 그리고 그 낱말은 완료에만 붙는다:
    /// 인터럽트된 채 질문에 서 있는 판은 여전히 입력을 기다린다.
    #[test]
    fn a_turn_a_person_stopped_rings_with_a_different_word() {
        let stopped = notice(
            "api",
            "claude",
            Ring::Completion,
            true,
            None,
            Some("절반쯤 하다 멈췄습니다"),
        );
        assert_eq!(stopped.title, "api - claude stopped");
        // 낱말만 바뀌고 몸통은 그대로다 — 알림이 죽는 것이 아니다.
        assert_eq!(stopped.body, "절반쯤 하다 멈췄습니다");

        // 주의를 구하는 링은 깃발을 아예 읽지 않는다.
        assert_eq!(
            notice(
                "api",
                "claude",
                Ring::Attention,
                true,
                Some("계속할까요?"),
                None
            )
            .title,
            "api - claude needs input"
        );

        // 세 낱말은 상수 하나씩이고, 결정은 순수 함수 하나다.
        assert_eq!(verb(Ring::Attention, false), VERB_ATTENTION);
        assert_eq!(verb(Ring::Attention, true), VERB_ATTENTION);
        assert_eq!(verb(Ring::Completion, false), VERB_FINISHED);
        assert_eq!(verb(Ring::Completion, true), VERB_STOPPED);
    }

    /// Held rings fold into one notice: one stands as it is, several wear
    /// the first's title with a count and list every title (t-6043).
    #[test]
    fn held_rings_fold_into_one_notice() {
        assert_eq!(batched(&[]), None);
        let one = notice(
            "api",
            "claude",
            Ring::Completion,
            false,
            None,
            Some("green"),
        );
        assert_eq!(batched(std::slice::from_ref(&one)), Some(one.clone()));
        let two = notice("web", "codex", Ring::Attention, false, Some("merge?"), None);
        let three = notice("api", "zo", Ring::Push, false, None, Some("done"));
        let folded = batched(&[one, two, three]).expect("three held");
        assert_eq!(folded.title, "api - claude finished +2");
        assert_eq!(
            folded.body,
            "api - claude finished\nweb - codex needs input\napi - zo says"
        );
    }

    /// The three kinds have one word each, and the word reads back.
    #[test]
    fn a_ring_kind_has_one_word_that_reads_back() {
        for ring in Ring::ALL {
            assert_eq!(Ring::from_word(ring.word()), Some(ring));
        }
        assert_eq!(Ring::from_word("bell"), None);
    }

    /// A push the agent sent on purpose (zo `PushNotification`, t-2943) wears
    /// its own verb and the agent's own words — never a question, never an
    /// interrupt reading, and never an invented body.
    #[test]
    fn a_push_says_what_the_agent_wrote() {
        let push = notice(
            "t-2943",
            "zo",
            Ring::Push,
            false,
            Some("a question it never asked"),
            Some("Build is green; merge when you are back"),
        );
        assert_eq!(push.title, "t-2943 - zo says");
        assert_eq!(push.body, "Build is green; merge when you are back");
        assert_eq!(
            verb(Ring::Push, true),
            VERB_PUSH,
            "a push reads no interrupt flag"
        );
        assert_eq!(
            notice("t-2943", "zo", Ring::Push, false, None, None).body,
            ""
        );
    }

    // The clock in every test below is a number the test chose. Nothing sleeps:
    // a ring that would fire after a minute is asked about at the minute.

    /// The two waits: ten seconds for a stop, a minute for a finish. A push the
    /// agent sent on purpose does not wait.
    #[test]
    fn the_waits_are_ten_seconds_for_a_stop_and_a_minute_for_a_finish() {
        assert_eq!(quiet_ms(Ring::Attention), 10_000);
        assert_eq!(quiet_ms(Ring::Completion), 60_000);
        assert_eq!(quiet_ms(Ring::Push), 0);
        assert_eq!(LONG_TURN_MS, 60_000);
    }

    /// Acceptance 1a: a stop that clears inside ten seconds rings nothing.
    #[test]
    fn a_stop_that_clears_inside_ten_seconds_rings_nothing() {
        let mut quiet = Quiet::default();
        quiet.arm("pane-1", Ring::Attention, 1_000, 1_000);
        // The stop is gone at 6 s: the pane is working, so no stop stands.
        assert_eq!(quiet.settle(&"pane-1", 11_000, None), None);
    }

    /// Acceptance 1a, the other side: a stop that stands for ten seconds rings
    /// once, and the same stop never rings twice.
    #[test]
    fn a_stop_that_stands_for_ten_seconds_rings_once() {
        let mut quiet = Quiet::default();
        quiet.arm("pane-1", Ring::Attention, 1_000, 1_000);
        assert_eq!(quiet.settle(&"pane-1", 10_999, Some(1_000)), None, "not due yet");
        assert_eq!(
            quiet.settle(&"pane-1", 11_000, Some(1_000)),
            Some(Ring::Attention)
        );
        assert_eq!(quiet.settle(&"pane-1", 60_000, Some(1_000)), None);
    }

    /// Acceptance 1b: a turn of a minute or more that finishes rings exactly once.
    #[test]
    fn a_long_turn_that_finishes_rings_exactly_once() {
        // The turn began at 0 s and the pane said Done at 65 s.
        let turn = turn_ms(Some(0), 65_000);
        assert_eq!(turn, Some(65_000));
        assert!(FinishRing::Long.earns(turn));
        let mut quiet = Quiet::default();
        quiet.arm("pane-1", Ring::Completion, 65_000, 65_000);
        assert_eq!(quiet.settle(&"pane-1", 124_999, Some(65_000)), None);
        assert_eq!(
            quiet.settle(&"pane-1", 125_000, Some(65_000)),
            Some(Ring::Completion)
        );
        assert_eq!(quiet.settle(&"pane-1", 200_000, Some(65_000)), None);
    }

    /// Acceptance 1c: under the long-only mode a short turn's finish rings
    /// nothing. Off rings no finish at all, and always rings every one.
    #[test]
    fn a_short_turn_that_finishes_rings_nothing_under_long_only() {
        let short = turn_ms(Some(0), 30_000);
        assert!(!FinishRing::Long.earns(short));
        assert!(!FinishRing::Off.earns(Some(900_000)));
        assert!(FinishRing::Always.earns(short));
        assert!(
            !FinishRing::Long.earns(None),
            "a turn of unknown length is not proven long"
        );
    }

    /// A finish the agent works past inside the minute is cancelled.
    #[test]
    fn a_finish_the_agent_works_past_inside_a_minute_is_cancelled() {
        let mut quiet = Quiet::default();
        quiet.arm("pane-1", Ring::Completion, 65_000, 65_000);
        // Working again at 70 s: the Done stamp no longer stands at 125 s.
        assert_eq!(quiet.settle(&"pane-1", 125_000, None), None);
    }

    /// Acceptance 1d: a Stop hook that never comes still leaves one finished
    /// mark when the pane goes to rest, and the mark lists one waiting pane.
    #[test]
    fn a_missed_stop_hook_leaves_one_finished_mark() {
        // Working from 0 s; no Stop; the pane is at rest at 90 s.
        let mark = finish_mark_after(
            false,
            HookState::Idle,
            Some(HookState::Working),
            false,
            false,
        );
        assert!(mark, "the rest after work is a finish");
        let look = Look {
            state: HookState::Idle,
            asking: false,
            mark,
            turn_ms: turn_ms(Some(0), 90_000),
        };
        assert_eq!(waiting_for(look, FinishRing::Long), Some(Waiting::Finished));
        // A missed finish of a short turn is still marked, but the long-only
        // mode lists no finish for it.
        let short = Look {
            turn_ms: turn_ms(Some(0), 20_000),
            ..look
        };
        assert_eq!(waiting_for(short, FinishRing::Long), None);
    }

    /// The mark is released by new work, and a turn a person stopped or a
    /// resumed session never sets it.
    #[test]
    fn the_finished_mark_clears_on_new_work_and_only_a_real_finish_sets_it() {
        assert!(!finish_mark_after(
            true,
            HookState::Working,
            Some(HookState::Done),
            false,
            false,
        ));
        assert!(
            !finish_mark_after(false, HookState::Done, Some(HookState::Working), true, false),
            "a turn a person stopped is not a finish"
        );
        assert!(
            !finish_mark_after(false, HookState::Done, Some(HookState::Working), false, true),
            "a resumed session is nobody's finish"
        );
        assert!(finish_mark_after(
            false,
            HookState::Done,
            Some(HookState::Working),
            false,
            false,
        ));
    }

    /// The waiting list names what each pane waits on, under each mode.
    #[test]
    fn the_waiting_list_names_what_each_pane_waits_for() {
        let finished = Look {
            state: HookState::Done,
            asking: false,
            mark: true,
            turn_ms: Some(90_000),
        };
        let asking = Look {
            state: HookState::NeedsAttention,
            asking: true,
            mark: false,
            turn_ms: None,
        };
        let blocked = Look {
            asking: false,
            ..asking
        };
        assert_eq!(waiting_for(asking, FinishRing::Long), Some(Waiting::Question));
        assert_eq!(waiting_for(blocked, FinishRing::Long), Some(Waiting::Blocked));
        assert_eq!(waiting_for(finished, FinishRing::Long), Some(Waiting::Finished));
        assert_eq!(waiting_for(finished, FinishRing::Off), None, "off lists no finish");
        let working = Look {
            state: HookState::Working,
            ..finished
        };
        assert_eq!(waiting_for(working, FinishRing::Always), None);
        let short = Look {
            turn_ms: Some(20_000),
            ..finished
        };
        assert_eq!(waiting_for(short, FinishRing::Long), None);
        assert_eq!(waiting_for(short, FinishRing::Always), Some(Waiting::Finished));
    }

    /// A turn keeps its start through a question, a new prompt starts its own,
    /// and the first turn of a pane starts when it is first heard.
    #[test]
    fn a_turn_keeps_its_start_through_a_question_and_a_new_prompt_starts_one() {
        assert_eq!(
            turn_started_after(Some((HookState::Working, Some(0))), HookState::NeedsAttention, 20_000),
            Some(0)
        );
        assert_eq!(
            turn_started_after(Some((HookState::NeedsAttention, Some(0))), HookState::Working, 40_000),
            Some(0),
            "the answered question is the same turn"
        );
        assert_eq!(
            turn_started_after(Some((HookState::Working, Some(0))), HookState::Done, 65_000),
            Some(0)
        );
        assert_eq!(
            turn_started_after(Some((HookState::Done, Some(0))), HookState::Working, 100_000),
            Some(100_000),
            "a new prompt starts a new turn"
        );
        assert_eq!(
            turn_started_after(None, HookState::Working, 5_000),
            Some(5_000)
        );
    }

    /// The finish setting reads back from its word, and the default is the
    /// long-turn rule.
    #[test]
    fn the_finish_setting_reads_back_and_defaults_to_long_turns() {
        assert_eq!(FinishRing::default(), FinishRing::Long);
        for mode in FinishRing::ALL {
            assert_eq!(FinishRing::from_word(mode.word()), Some(mode));
        }
        assert_eq!(FinishRing::from_word("bell"), None);
    }
}
