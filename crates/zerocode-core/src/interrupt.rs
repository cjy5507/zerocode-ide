//! Was that Escape an interrupt? — the inference, without its timers.
//!
//! No agent reports "I was interrupted". A person presses Escape or Ctrl+C in
//! the terminal, the TUI stops its turn, and the only hook that may follow is
//! a plain `Stop` — or nothing at all. Orca watches the KEY instead: if an
//! agent was verifiably mid-turn when the interrupt gesture landed, and the
//! turn's identity is unchanged when the dust settles, the pane's state is
//! synthesized to an interrupted done
//! (`agent-interrupt-inference.ts:112-278`).
//!
//! This module is that watching, pure: the clock arrives as an argument and
//! the original's two timers leave as values — an armed inference returns the
//! deadline the caller should call [`InterruptInference::flush`] at, and the
//! double-escape window carries its own expiry and is judged against the
//! clock when the second escape arrives, which is the only moment it was
//! ever read. What the inference DOES — synthesizing the done, marking the
//! card — stays with the wiring; this answers only "was that an interrupt".
//!
//! The original also guards against out-of-order async observers
//! (`baselineSequence`, the captured-entry identity check, :215-224). Those
//! exist because its key handler and its status store race; a caller that
//! reads the entry and observes in one breath has nothing to reorder, and
//! the wiring order sheet owns saying so.

use crate::agent::AgentKind;
use crate::hook::HookState;

/// How long a gesture waits for the turn to still be the same turn
/// (`AGENT_INTERRUPT_SETTLE_MS`, agent-interrupt-intent.ts:5). Also the
/// double-escape window: a first escape older than this armed nothing.
pub const SETTLE_MS: i64 = 500;

/// How old an explicit status may be and still count as "mid-turn"
/// (`AGENT_STATUS_STALE_AFTER_MS`, agent-status-types.ts:281).
pub const STALE_AFTER_MS: i64 = 30 * 60 * 1000;

/// The two gestures that can mean "stop" (`AgentInterruptInputIntent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Intent {
    PlainEscape,
    CtrlC,
}

/// The pane's agent status, as much of it as the inference reads.
///
/// **In this window's own vocabulary, and that is the whole point of the
/// types.** This was worded in the ORIGINAL's words — `agent: &str` holding
/// `"claude"`, `state: &str` holding `"waiting"` — and the arm that reads them
/// could never have fired: our [`HookState`] serialises kebab-case and spells
/// `needs-attention`, so `state == "waiting"` is a comparison against a word
/// nothing in this application produces. The tests passed because they built
/// the string by hand. Nothing outside this module constructs an `AgentTurn`
/// yet, so no behaviour was lost — but the day it is wired, a silently dead
/// arm is what a caller would have got. Typed, the mismatch is a compile
/// error.
#[derive(Debug, Clone, Copy)]
pub struct AgentTurn<'a> {
    pub agent: AgentKind,
    pub state: HookState,
    /// The pane is stopped on a QUESTION rather than on a permission gate.
    ///
    /// Orca discriminates by tool name (`isAskUserQuestionTool`) because its
    /// `waiting` covers both. Ours cannot: [`HookState::NeedsAttention`] is the
    /// one word for both stops, and what actually keeps them apart on this
    /// side is whether the payload carried a questions SHAPE — the pane's
    /// `ask_prompt` rather than a vendor's spelling of a tool. That is also
    /// the original's real test one layer down (`parseToolInput ??
    /// parseQuestionsShape`, cited at `ask.rs`), so this is the same question
    /// asked where we can actually answer it.
    pub blocked_on_question: bool,
    pub prompt: &'a str,
    pub updated_at: i64,
    pub state_started_at: i64,
    /// A hydrated row no live receiver has confirmed — never fresh
    /// (`pane-agent-evidence.ts:22-23`).
    pub restored_unconfirmed: bool,
}

/// The interrupt the inference is confident of — what the wiring hands to
/// the synthesizer (`AgentInterruptInferenceRequest`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct InterruptCall {
    pub agent: AgentKind,
    pub prompt: String,
    pub intent: Intent,
    pub baseline_updated_at: i64,
    pub baseline_state_started_at: i64,
    /// `Some(2)` when the interrupt took two escapes — the vendors whose
    /// first escape is an editor cancel (`:252-259`).
    pub input_count: Option<u8>,
}

/// What one observed gesture amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observed {
    /// Not an interrupt: no turn to interrupt, a stale one, or a vendor that
    /// spends this gesture on something else.
    Nothing,
    /// First escape of a vendor that needs two; nothing is pending yet, and
    /// a second escape after `expires_at` starts over (`:243-249`).
    FirstEscape { expires_at: i64 },
    /// Armed: call [`InterruptInference::flush`] at `flush_at` with the
    /// entry as it stands then.
    Armed { flush_at: i64 },
    /// Confident now — the vendors whose own done hook can outrun the settle
    /// timer and erase the evidence (`:264-268`).
    Infer(InterruptCall),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Baseline {
    agent: AgentKind,
    prompt: String,
    intent: Intent,
    updated_at: i64,
    state_started_at: i64,
    input_count: Option<u8>,
}

impl Baseline {
    fn of(turn: &AgentTurn<'_>, intent: Intent) -> Self {
        Self {
            agent: turn.agent,
            prompt: turn.prompt.to_string(),
            intent,
            updated_at: turn.updated_at,
            state_started_at: turn.state_started_at,
            input_count: None,
        }
    }

    /// Same turn — vendor, prompt and the moment the state began
    /// (`isSameTurnBaseline`, `:76-85`).
    fn same_turn(&self, other: &Baseline) -> bool {
        same_agent(self.agent, other.agent)
            && self.prompt == other.prompt
            && self.state_started_at == other.state_started_at
    }

    fn call(self) -> InterruptCall {
        InterruptCall {
            agent: self.agent,
            prompt: self.prompt,
            intent: self.intent,
            baseline_updated_at: self.updated_at,
            baseline_state_started_at: self.state_started_at,
            input_count: self.input_count,
        }
    }
}

/// A status fresh enough to be evidence (`isExplicitAgentStatusFresh`).
fn fresh(turn: &AgentTurn<'_>, now_ms: i64) -> bool {
    !turn.restored_unconfirmed && now_ms - turn.updated_at <= STALE_AFTER_MS
}

/// A turn this gesture could be interrupting (`canInferInterrupt`, `:64-72`):
/// a working agent — or claude standing on its blocked question, which a
/// plain escape dismisses.
fn can_infer(turn: &AgentTurn<'_>, intent: Intent) -> bool {
    turn.state == HookState::Working
        || (intent == Intent::PlainEscape
            && turn.state == HookState::NeedsAttention
            && turn.agent == AgentKind::Claude
            && turn.blocked_on_question)
}

/// Whether two records name the same vendor (`equivalentInterruptAgentType`,
/// `main/agent-hooks/server.ts:238-245`).
///
/// The original normalises `'unknown'` and `undefined` to each other before
/// comparing, because its status entry can carry a vendor it did not recognise.
/// **Here that degenerates to equality**, and deliberately: [`AgentKind`] has no
/// `Unknown` variant (`agent.rs`), and a pane whose vendor we cannot name
/// produces no [`AgentTurn`] at all rather than one wearing a placeholder. The
/// function exists anyway so the comparison has ONE home — the day an `Unknown`
/// variant lands, this is the line that has to change, and a pin says so.
#[must_use]
pub fn same_agent(left: AgentKind, right: AgentKind) -> bool {
    left == right
}

/// Vendors whose FIRST escape is an editor/menu cancel (`:40-45`).
fn needs_two_escapes(agent: AgentKind, intent: Intent) -> bool {
    matches!(agent, AgentKind::Opencode | AgentKind::Copilot) && intent == Intent::PlainEscape
}

/// Gestures a vendor spends on something other than stopping (`:57-62`):
/// droid's Ctrl+C is its own clipboard/cancel chord.
fn spent_elsewhere(agent: AgentKind, intent: Intent) -> bool {
    agent == AgentKind::Droid && intent == Intent::CtrlC
}

/// Vendors whose interrupt must be believed immediately (`:47-55`): their
/// idle hook can land before the settle timer and overwrite the working
/// baseline, losing the interrupted outcome.
fn flushes_immediately(baseline: &Baseline) -> bool {
    needs_two_escapes(baseline.agent, baseline.intent)
        || (baseline.agent == AgentKind::Codex && baseline.intent == Intent::PlainEscape)
}

/// The per-pane inference. One per terminal, fed by its key handler.
#[derive(Debug, Default)]
pub struct InterruptInference {
    pending: Option<Baseline>,
    /// The double-escape first leg, and when it stops counting.
    first_escape: Option<(Baseline, i64)>,
}

impl InterruptInference {
    pub fn new() -> Self {
        Self::default()
    }

    /// A stop gesture landed. Decide what it was.
    pub fn observe(
        &mut self,
        intent: Intent,
        turn: Option<&AgentTurn<'_>>,
        now_ms: i64,
    ) -> Observed {
        let Some(turn) = turn else {
            self.clear();
            return Observed::Nothing;
        };
        if !can_infer(turn, intent) || !fresh(turn, now_ms) {
            self.clear();
            return Observed::Nothing;
        }
        if spent_elsewhere(turn.agent, intent) {
            self.clear();
            return Observed::Nothing;
        }
        let mut baseline = Baseline::of(turn, intent);
        if needs_two_escapes(turn.agent, intent) {
            let second_of_pair = self.first_escape.take().is_some_and(|(first, expires_at)| {
                // 배타적 경계다: 원본의 `setTimer(clearDoubleEscapeBaseline,
                // 500)`(`:252`)은 정확히 t+500에 **이미 발화했다** — 그
                // 순간의 두 번째 escape는 짝이 아니라 새 첫 번째다
                // (`agent-interrupt-inference.test.ts:285-288`이 500을
                // 흘려 보내고 추론 없음을 요구한다).
                now_ms < expires_at && first.same_turn(&baseline)
            });
            self.pending = None;
            if !second_of_pair {
                let expires_at = now_ms + SETTLE_MS;
                self.first_escape = Some((baseline, expires_at));
                return Observed::FirstEscape { expires_at };
            }
            baseline.input_count = Some(2);
        } else {
            self.first_escape = None;
        }
        if flushes_immediately(&baseline) {
            self.pending = None;
            return Observed::Infer(baseline.call());
        }
        let flush_at = now_ms + SETTLE_MS;
        self.pending = Some(baseline);
        Observed::Armed { flush_at }
    }

    /// The settle deadline arrived. Still the same turn?
    ///
    /// The gesture only meant "stop" if nothing about the turn moved while
    /// it settled (`flushPending`, `:168-205`): same vendor, same prompt,
    /// same timestamps, still inferable, still fresh. A pane whose entry is
    /// GONE may still infer — from the baseline's own freshness — because an
    /// interrupt can tear the entry down with it.
    pub fn flush(&mut self, turn: Option<&AgentTurn<'_>>, now_ms: i64) -> Option<InterruptCall> {
        let baseline = self.pending.take()?;
        match turn {
            Some(turn) => {
                let unchanged = can_infer(turn, baseline.intent)
                    && same_agent(turn.agent, baseline.agent)
                    && turn.prompt == baseline.prompt
                    && turn.updated_at == baseline.updated_at
                    && turn.state_started_at == baseline.state_started_at
                    && fresh(turn, now_ms);
                unchanged.then(|| baseline.call())
            }
            None => (now_ms - baseline.updated_at <= STALE_AFTER_MS).then(|| baseline.call()),
        }
    }

    /// Forget everything — the pane closed, or the wiring saw a newer turn.
    pub fn clear(&mut self) {
        self.pending = None;
        self.first_escape = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_000_000;

    fn working(agent: AgentKind, prompt: &str) -> AgentTurn<'_> {
        AgentTurn {
            agent,
            state: HookState::Working,
            blocked_on_question: false,
            prompt,
            updated_at: T0 - 100,
            state_started_at: T0 - 5_000,
            restored_unconfirmed: false,
        }
    }

    /// The common vendor: arm, wait the settle, same turn → interrupt.
    #[test]
    fn a_working_turn_unchanged_through_the_settle_is_an_interrupt() {
        let mut inference = InterruptInference::new();
        let turn = working(AgentKind::Claude, "fix the tests");
        let Observed::Armed { flush_at } = inference.observe(Intent::CtrlC, Some(&turn), T0) else {
            panic!("a working claude Ctrl+C did not arm");
        };
        assert_eq!(flush_at, T0 + SETTLE_MS);

        let call = inference
            .flush(Some(&turn), flush_at)
            .expect("the unchanged turn was not inferred");
        assert_eq!(call.agent, AgentKind::Claude);
        assert_eq!(call.intent, Intent::CtrlC);
        assert_eq!(call.input_count, None);
        // Flushed once, answered once.
        assert!(inference.flush(Some(&turn), flush_at).is_none());
    }

    /// A turn that moved while settling was not interrupted — its own hook
    /// spoke first.
    #[test]
    fn a_turn_that_moved_while_settling_infers_nothing() {
        let mut inference = InterruptInference::new();
        let turn = working(AgentKind::Claude, "fix the tests");
        inference.observe(Intent::CtrlC, Some(&turn), T0);

        let moved = AgentTurn {
            updated_at: T0 + 200,
            ..turn
        };
        assert!(inference.flush(Some(&moved), T0 + SETTLE_MS).is_none());
    }

    /// The entry torn down WITH the interrupt still infers, from the
    /// baseline's own freshness.
    #[test]
    fn a_pane_whose_entry_vanished_still_infers_from_the_baseline() {
        let mut inference = InterruptInference::new();
        let turn = working(AgentKind::Claude, "fix");
        inference.observe(Intent::CtrlC, Some(&turn), T0);
        assert!(inference.flush(None, T0 + SETTLE_MS).is_some());

        inference.observe(Intent::CtrlC, Some(&turn), T0);
        assert!(
            inference
                .flush(None, turn.updated_at + STALE_AFTER_MS + 1)
                .is_none(),
            "a baseline past the stale line answered"
        );
    }

    /// Codex escape cannot wait: its idle hook outruns the settle and erases
    /// the evidence.
    #[test]
    fn the_vendors_whose_done_outruns_the_settle_infer_immediately() {
        let mut inference = InterruptInference::new();
        let codex = working(AgentKind::Codex, "p");
        assert!(matches!(
            inference.observe(Intent::PlainEscape, Some(&codex), T0),
            Observed::Infer(_)
        ));
        // codex Ctrl+C settles like everyone else.
        assert!(matches!(
            inference.observe(Intent::CtrlC, Some(&codex), T0),
            Observed::Armed { .. }
        ));
    }

    /// opencode and copilot spend the first escape on their editor: only the
    /// second, on the SAME turn and inside the window, interrupts — and it
    /// carries the count.
    #[test]
    fn the_double_escape_vendors_interrupt_on_the_second_escape_of_one_turn() {
        for vendor in [AgentKind::Opencode, AgentKind::Copilot] {
            let mut inference = InterruptInference::new();
            let turn = working(vendor, "p");
            let Observed::FirstEscape { expires_at } =
                inference.observe(Intent::PlainEscape, Some(&turn), T0)
            else {
                panic!("{vendor:?}: first escape armed something");
            };
            // INSIDE the window — strictly before the boundary.
            let Observed::Infer(call) =
                inference.observe(Intent::PlainEscape, Some(&turn), expires_at - 1)
            else {
                panic!("{vendor:?}: second escape did not infer");
            };
            assert_eq!(call.input_count, Some(2));
        }
    }

    /// A first escape expires: a late second escape is a new first — and
    /// `expires_at` itself is already late.
    ///
    /// 경계는 배타적이다. 원본에서 창을 닫는 것은 비교가 아니라 **타이머**이고
    /// (`setTimer(clearDoubleEscapeBaseline, AGENT_INTERRUPT_SETTLE_MS)`,
    /// `agent-interrupt-inference.ts:252`), 그 타이머는 정확히 +500에 이미
    /// 발화한다. 원본 시험이 그것을 그대로 요구한다:
    /// `advanceTimersByTime(500)` 뒤의 두 번째 escape는 추론하지 않는다
    /// (`agent-interrupt-inference.test.ts:283-288`). 이 창을 값으로 실어
    /// 나르는 우리 철자에서 그 문장은 `now < expires_at`이다 — `<=`는 이미
    /// 지나간 타이머를 살아 있다고 읽는다.
    #[test]
    fn a_late_second_escape_starts_the_pair_over() {
        let mut inference = InterruptInference::new();
        let turn = working(AgentKind::Opencode, "p");
        let Observed::FirstEscape { expires_at } =
            inference.observe(Intent::PlainEscape, Some(&turn), T0)
        else {
            panic!("first escape armed something");
        };
        // 경계 그 순간부터 늦었다 — 짝이 아니라 새 첫 번째다.
        assert!(matches!(
            inference.observe(Intent::PlainEscape, Some(&turn), expires_at),
            Observed::FirstEscape { .. }
        ));

        // 그 뒤도 늦다. 새 인스턴스로 재는 것이 이 시험이 처음 스스로를
        // 속인 자리다: 위의 관찰이 **새 짝을 시작**하므로, 같은 인스턴스에서
        // 1ms 뒤의 escape는 늦은 것이 아니라 그 새 짝의 정당한 두 번째다.
        let mut later = InterruptInference::new();
        let Observed::FirstEscape { expires_at } =
            later.observe(Intent::PlainEscape, Some(&turn), T0)
        else {
            panic!("first escape armed something");
        };
        assert!(matches!(
            later.observe(Intent::PlainEscape, Some(&turn), expires_at + 1),
            Observed::FirstEscape { .. }
        ));
    }

    /// A second escape on a DIFFERENT turn is a first escape of that turn.
    #[test]
    fn a_second_escape_on_another_turn_does_not_borrow_the_first() {
        let mut inference = InterruptInference::new();
        let first = working(AgentKind::Opencode, "first prompt");
        inference.observe(Intent::PlainEscape, Some(&first), T0);
        let second = AgentTurn {
            prompt: "second prompt",
            ..first
        };
        assert!(matches!(
            inference.observe(Intent::PlainEscape, Some(&second), T0 + 100),
            Observed::FirstEscape { .. }
        ));
    }

    /// droid's Ctrl+C is its own chord — never an interrupt; its escape
    /// settles like anyone's.
    #[test]
    fn droids_ctrl_c_is_spent_elsewhere() {
        let mut inference = InterruptInference::new();
        let turn = working(AgentKind::Droid, "p");
        assert_eq!(
            inference.observe(Intent::CtrlC, Some(&turn), T0),
            Observed::Nothing
        );
        assert!(matches!(
            inference.observe(Intent::PlainEscape, Some(&turn), T0),
            Observed::Armed { .. }
        ));
    }

    /// claude parked on its blocked question: a plain escape dismisses it —
    /// that IS an interrupt — but Ctrl+C on waiting is not, and neither is
    /// another vendor's waiting.
    #[test]
    fn escaping_claudes_blocked_question_counts_as_an_interrupt() {
        let mut inference = InterruptInference::new();
        let asking = AgentTurn {
            state: HookState::NeedsAttention,
            blocked_on_question: true,
            ..working(AgentKind::Claude, "p")
        };
        assert!(matches!(
            inference.observe(Intent::PlainEscape, Some(&asking), T0),
            Observed::Armed { .. }
        ));
        assert_eq!(
            inference.observe(Intent::CtrlC, Some(&asking), T0),
            Observed::Nothing
        );
        let codex_waiting = AgentTurn {
            agent: AgentKind::Codex,
            ..asking
        };
        assert_eq!(
            inference.observe(Intent::PlainEscape, Some(&codex_waiting), T0),
            Observed::Nothing
        );
    }

    /// **A permission wait is not a question an Escape dismisses.**
    ///
    /// This is the arm that could not fire until the type spoke this window's
    /// words. Our [`HookState::NeedsAttention`] is one word for two stops —
    /// claude standing on its own question, and claude standing on a tool
    /// asking to be allowed — and Orca keeps them apart by the tool's NAME
    /// (`isAskUserQuestionTool`), which we have no reliable copy of. What we
    /// have is whether a questions shape arrived, and the two must not be
    /// treated alike: an Escape on a permission gate is the person declining
    /// to answer YET, not the person stopping the turn, and synthesizing an
    /// interrupted done there retires a pane that is still waiting for them.
    #[test]
    fn a_permission_wait_is_not_a_question_a_gesture_dismisses() {
        let stopped = working(AgentKind::Claude, "p");
        let gate = AgentTurn {
            state: HookState::NeedsAttention,
            blocked_on_question: false,
            ..stopped
        };
        let question = AgentTurn {
            blocked_on_question: true,
            ..gate
        };

        let mut on_the_gate = InterruptInference::new();
        assert_eq!(
            on_the_gate.observe(Intent::PlainEscape, Some(&gate), T0),
            Observed::Nothing,
            "an escape at a permission gate was read as stopping the turn"
        );

        let mut on_the_question = InterruptInference::new();
        assert!(
            matches!(
                on_the_question.observe(Intent::PlainEscape, Some(&question), T0),
                Observed::Armed { .. }
            ),
            "the one arm this type change exists for still cannot fire"
        );
    }

    /// The judgements name vendors and states by TYPE, never by hand.
    ///
    /// This module was written in the original's words and one arm of it could
    /// never fire, because `state == "waiting"` is a comparison against a word
    /// this application does not produce — [`HookState`] spells
    /// `needs-attention`. Nothing complained: it compiled, and the tests built
    /// the string themselves. A spelling that only the tests ever produce is a
    /// gate on nothing, so the spellings are gone and this keeps them gone.
    ///
    /// Scoped to the four predicate BODIES rather than the file, because the
    /// module's own prose has to be able to quote the words it is warning
    /// about.
    #[test]
    fn the_judgements_name_no_vendor_and_no_state_by_hand() {
        let module = include_str!("interrupt.rs");
        let body = |signature: &str| -> String {
            let at = module
                .find(signature)
                .unwrap_or_else(|| panic!("{signature} left this module"));
            let rest = &module[at..];
            let end = rest.find("\n}\n").unwrap_or(rest.len());
            rest[..end].to_string()
        };
        for signature in [
            "fn can_infer(",
            "fn needs_two_escapes(",
            "fn spent_elsewhere(",
            "fn flushes_immediately(",
        ] {
            let judging = body(signature);
            assert!(
                !judging.contains('"'),
                "{signature} spells a word by hand again; a vendor or a state \
                 written as text is a comparison nothing in this application \
                 has to match:\n{judging}"
            );
        }
    }

    /// The vendor comparison has one home, and the argument for its shape is
    /// checkable rather than asserted.
    ///
    /// Orca folds `'unknown'` into `undefined` before comparing
    /// (`equivalentInterruptAgentType`); here that is equality, and the reason
    /// is that no vendor we cannot name ever reaches this module. The day an
    /// `Unknown` variant lands in [`AgentKind`], that reason stops holding and
    /// this is the line that has to change — so it fails here first.
    #[test]
    fn the_vendor_comparison_degenerates_only_while_no_vendor_is_nameless() {
        assert!(same_agent(AgentKind::Claude, AgentKind::Claude));
        assert!(!same_agent(AgentKind::Claude, AgentKind::Codex));
        assert!(
            crate::agent::AgentKind::from_slug("unknown").is_none(),
            "a nameless vendor now exists, so `same_agent` owes it Orca's fold"
        );
    }

    /// Stale or unconfirmed evidence arms nothing.
    #[test]
    fn stale_or_unconfirmed_status_is_not_evidence() {
        let mut inference = InterruptInference::new();
        let old = AgentTurn {
            updated_at: T0 - STALE_AFTER_MS - 1,
            ..working(AgentKind::Claude, "p")
        };
        assert_eq!(
            inference.observe(Intent::CtrlC, Some(&old), T0),
            Observed::Nothing
        );
        let hydrated = AgentTurn {
            restored_unconfirmed: true,
            ..working(AgentKind::Claude, "p")
        };
        assert_eq!(
            inference.observe(Intent::CtrlC, Some(&hydrated), T0),
            Observed::Nothing
        );
        assert_eq!(
            inference.observe(Intent::CtrlC, None, T0),
            Observed::Nothing
        );
    }

    /// A no-turn observation clears whatever was pending — the pane is not
    /// what it was.
    #[test]
    fn observing_without_a_turn_clears_the_pending_inference() {
        let mut inference = InterruptInference::new();
        let turn = working(AgentKind::Claude, "p");
        inference.observe(Intent::CtrlC, Some(&turn), T0);
        inference.observe(Intent::CtrlC, None, T0 + 10);
        assert!(inference.flush(Some(&turn), T0 + SETTLE_MS).is_none());
    }
}
