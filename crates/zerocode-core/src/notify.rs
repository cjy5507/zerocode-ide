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

use crate::hook::HookState;
use crate::lane::LaneState;

/// Per-worktree quiet period between rings.
pub const COOLDOWN_MS: i64 = 5_000;

/// How long a `Done` must stand before it counts as finished.
pub const DONE_QUIET_MS: i64 = 1_500;

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
}
