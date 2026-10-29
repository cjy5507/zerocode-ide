//! One pump round of one shell's prompt delivery, and what settles it.
//!
//! # Why a seam
//!
//! Every programmatic producer of a prompt — a person's send from the window,
//! a worker's launch briefing, an automation, `zerocode-ssh send`, the
//! orchestration mail pointer — registers a [`PromptDelivery`] and the pump
//! turns it: waits for the composer, writes the paste, waits the gap, writes
//! the Enter. The decisions about whether each write may happen used to be
//! made by the producers, before registration, and acted on by the pump a
//! readiness wait later — about a line that had since moved. The integration
//! review of 2026-09-05 named the three doors that opened: the pointer's raw
//! `Host::send` consulted nobody at all; the ssh door checked a hand before
//! registering its Enter as a SECOND job, so a hand between registration and
//! the write went unseen and another producer's job could land between the
//! two; and a person's own `send_prompt` was never serialised against either.
//!
//! So the decision moved to the write. The delivery carries a
//! [`zerocode_pty::ready::Guard`] chosen by the door that made it, the pump
//! feeds it the [`Line`] facts every round beside the grid's, and the guard
//! answers at the moment the paste — and again the Enter — is due. One
//! delivery is the whole transaction, so the pane's prompt queue orders every
//! producer against every other. These two functions are exactly what the
//! pump calls, and they take their maps and their writer as arguments so the
//! same seam can be driven here without a pty behind it.

use super::*;

use zerocode_pty::ready::{Line, Refusal, Step};

/// The maps a prompt's way onto a pane's line is decided in, borrowed from
/// the shell runtime so a between-turns move and a test can hold the same
/// locks the registration door and the pump hold. `deliveries` is taken
/// first by every one of them.
pub(super) struct PromptLines<'a> {
    pub(super) deliveries: &'a Mutex<HashMap<TermId, PromptDelivery>>,
    pub(super) waiters: &'a Mutex<HashMap<TermId, std::sync::mpsc::SyncSender<DeliveryOutcome>>>,
    pub(super) queue: &'a Mutex<HashMap<TermId, VecDeque<QueuedPrompt>>>,
    pub(super) moving: &'a Mutex<std::collections::HashSet<TermId>>,
}

/// Observe an empty prompt line before committing a between-turns move, and
/// hold the pane's later prompts in its queue until the move is written
/// (t-74974).
///
/// The look and the hold are one step under deliveries, so a prompt either
/// registered before it and defers the move, or meets the hold and queues.
/// The commit itself — a ledger write that waits on the disk — runs with
/// deliveries let go: the pump turns every pane under that lock at the
/// display rate, and one pane's move must not stop the others' output. The
/// hold ends under deliveries again, where a prompt queued during it takes
/// the line on a fresh clock, as one queued behind a settled delivery does.
///
/// Answers whether a queued prompt took the line, for the caller to wake
/// the pump.
pub(super) fn with_no_prompt_in_flight(
    lines: &PromptLines<'_>,
    term: TermId,
    commit: &mut dyn FnMut(),
) -> bool {
    {
        let held = lines
            .deliveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let busy = held.contains_key(&term)
            || lines
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&term)
                .is_some_and(|queued| !queued.is_empty());
        if busy {
            return false;
        }
        lines
            .moving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(term);
    }
    let mut hold = MoveHold {
        lines,
        term,
        ended: false,
    };
    commit();
    hold.end()
}

/// The hold a between-turns move puts on its pane's line, ended when the
/// commit returns — or unwinds, so a panic cannot leave the pane's prompts
/// queued for good.
struct MoveHold<'l, 'a> {
    lines: &'l PromptLines<'a>,
    term: TermId,
    ended: bool,
}

impl MoveHold<'_, '_> {
    fn end(&mut self) -> bool {
        self.ended = true;
        let mut deliveries = self
            .lines
            .deliveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.lines
            .moving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.term);
        !deliveries.contains_key(&self.term)
            && take_next(
                self.term,
                &mut deliveries,
                &mut self
                    .lines
                    .waiters
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                &mut self
                    .lines
                    .queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                Instant::now(),
            )
    }
}

impl Drop for MoveHold<'_, '_> {
    fn drop(&mut self) {
        if !self.ended {
            self.end();
        }
    }
}

/// Whether a prompt for `term` waits in its queue: a delivery holds the
/// line, or the pane's account move is being written (t-74974). Asked under
/// `deliveries`, the lock both are set under.
pub(super) fn line_is_taken(
    deliveries: &HashMap<TermId, PromptDelivery>,
    moving: &Mutex<std::collections::HashSet<TermId>>,
    term: TermId,
) -> bool {
    deliveries.contains_key(&term)
        || moving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&term)
}

/// What the window knows about a pane's line, gathered for one round.
///
/// Permission and launch maps are read before the terminal lock. The hand
/// fields are refreshed under that lock by `refresh_hand` before a write.
pub(super) fn line_facts(state: &AppState, term: TermId) -> Line {
    Line {
        parked: pane_is_parked(state, term),
        launch: crate::cmd::terminal::launch_of(state, term),
        taken: reports_what_it_takes(state, term).then(|| crate::human_input::taken(term)),
        busy: pane_is_busy(state, term),
        ..Line::default()
    }
}

/// Whether the pane's program says which prompts it takes (t-14037) — its
/// agent row's `submit_ack`, the one table every delivery reads — so that a
/// delivery's Enter there waits for that word before it is called a send.
fn reports_what_it_takes(state: &AppState, term: TermId) -> bool {
    state
        .agent_terms()
        .get(&term)
        .and_then(|agent| zerocode_core::AgentKind::from_slug(agent))
        .is_some_and(zerocode_core::AgentKind::reports_prompt_submit)
}

/// Whether the pane's program says it is mid-turn — the hook road's word,
/// the same one the board's 「작업 중」 reads.
fn pane_is_busy(state: &AppState, term: TermId) -> bool {
    state
        .pane_states()
        .get(&term)
        .is_some_and(|held| held.state == zerocode_core::hook::HookState::Working)
}

/// Read the hand while holding the same terminal lock as `human_write`.
/// Sampling before that lock could miss a key written while we waited for it.
pub(super) fn refresh_hand(term: TermId, line: Line) -> Line {
    let (draft, hand) = crate::human_input::line_of(term);
    Line {
        draft,
        hand,
        ..line
    }
}

/// Record a successful human write before the caller releases the terminal
/// lock. Keys, paste and committed IME text all use this door.
///
/// Answers whether the hand put the PERSON's own words on the line, or sent
/// them — the question a takeover asks (t-17644). A key that adds no words —
/// an arrow, Escape, Tab, a control key — is a hand and takes nothing
/// (t-57712). A lone Enter on a line holding no words of theirs only
/// submitted what the window placed there (a resume nudge, a mail pointer, a
/// preamble) or nothing at all: the window's own record of the line says so,
/// read before this write moves it.
pub(super) fn human_write<E>(
    term: TermId,
    bytes: &[u8],
    write: impl FnOnce() -> Result<(), E>,
) -> Result<bool, E> {
    let theirs_before = crate::human_input::holds_own_words(term);
    write()?;
    let words = puts_words(bytes);
    if words {
        crate::human_input::typed(term);
    } else {
        crate::human_input::keyed(term);
    }
    let text = std::str::from_utf8(bytes).ok();
    if text.is_some_and(zerocode_core::ask::is_potential_submit_input) {
        crate::human_input::entered(term);
    }
    Ok(words || (theirs_before && text.is_some_and(zerocode_core::ask::is_submit_enter)))
}

/// Whether these bytes put words on the line (t-57712): printable text with
/// no escape in it — a printable key, committed IME text, a paste inside or
/// outside its brackets (a paste's own escapes are already inert,
/// `zerocode_pty::sanitize_paste`). An arrow, Escape, Tab, Shift-Tab, a
/// control key and an Enter carry none.
fn puts_words(bytes: &[u8]) -> bool {
    let bytes = bytes
        .strip_prefix(b"\x1b[200~")
        .and_then(|inside| inside.strip_suffix(b"\x1b[201~"))
        .unwrap_or(bytes);
    std::str::from_utf8(bytes)
        .is_ok_and(|text| !text.contains('\x1b') && text.chars().any(|ch| !ch.is_control()))
}

/// Record a composer chip's write at the person's click — the model chip's
/// command, the permission chip's key (t-57712). A hand, so a delivery yields
/// to the line it reached; never their words, so its answer to
/// [`human_write`]'s question is always no.
pub(super) fn chip_write<E>(
    term: TermId,
    write: impl FnOnce() -> Result<(), E>,
) -> Result<bool, E> {
    write()?;
    crate::human_input::keyed(term);
    Ok(false)
}

/// Whether a question or an approval is on the pane's screen, waiting for a
/// person — the hook road's word, which is the only reading of a TUI's
/// dialog this window trusts.
fn pane_is_parked(state: &AppState, term: TermId) -> bool {
    state
        .pane_states()
        .get(&term)
        .is_some_and(|held| held.state == zerocode_core::hook::HookState::NeedsAttention)
}

/// What one round did to one delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Turned {
    /// Nothing yet; keep pumping.
    Waiting,
    /// The pane's program is mid-turn, and the delivery waits the turn out
    /// (t-47952) — answered the round it first does, so the window can say
    /// the words are on their way; every round after is [`Self::Waiting`].
    Parked,
    /// The paste (and any clear keys ahead of it) went to the child.
    Pasted,
    /// The Enter went to the child — and was written down as the gesture
    /// that takes the line, for the provider's prompt event to answer.
    Entered,
    /// The delivery is over, one way or another. The caller settles it.
    Settled(DeliveryOutcome),
}

/// Turn one delivery one round.
///
/// `write` is the pane: the pump hands it `PtyLane::write_input`, a test hands
/// it a vector. `PtyLane::write_input` rejects a whole message on queue
/// pressure or a closed writer. Rejection does not mean the child has exited.
pub(super) fn turn(
    term: TermId,
    delivery: &mut PromptDelivery,
    seen: Observed,
    line: Line,
    now: Instant,
    write: impl FnOnce(&[u8]) -> bool,
) -> Turned {
    let parked_before = delivery.parked_at().is_some();
    match delivery.poll_line(seen, line, now) {
        Step::Waiting if !parked_before && delivery.parked_at().is_some() => Turned::Parked,
        Step::Waiting => Turned::Waiting,
        Step::Write(bytes) => {
            if write(&bytes) {
                Turned::Pasted
            } else {
                Turned::Settled(delivery.reject_write(false))
            }
        }
        Step::Submit(bytes) => {
            if !write(&bytes) {
                return Turned::Settled(delivery.reject_write(true));
            }
            crate::human_input::entered(term);
            Turned::Entered
        }
        Step::Done(outcome) => Turned::Settled(outcome),
    }
}

/// The words a settled delivery hands back to a person, if any.
///
/// A delivery that wrote nothing — timed out, or refused at the paste —
/// leaves the person holding the words, so they travel with the outcome. One
/// that pasted and withheld the Enter has left them in the composer, where
/// the person can already read them; carrying them again would be a second
/// copy of a line that is on screen. So has an Enter alone (t-17037): it
/// carries no words, and the ones it was for are on the line already.
///
/// And only the person's own words go back to them (t-52566,
/// [`PromptDelivery::hands_back`]). The window's own — the mail pointer's
/// advice, a restart's continuation, a briefing — were never theirs to
/// paste: the pointer's line that ran out of time inside a restored zo
/// worker's turn reached the person's clipboard as words they had lost. The
/// road that sent them answers for them, and the black box keeps the line.
pub(super) fn words_to_hand_back(
    delivery: &PromptDelivery,
    outcome: DeliveryOutcome,
) -> Option<String> {
    if !delivery.hands_back() || delivery.enter_again().is_some() {
        return None;
    }
    match outcome {
        DeliveryOutcome::Delivered | DeliveryOutcome::Unsubmitted(_) => None,
        DeliveryOutcome::TimedOut | DeliveryOutcome::Refused(_) => {
            Some(delivery.text().to_string())
        }
    }
}

/// Why a write was withheld, when one was — for the black box and the
/// `term:prompt` event.
pub(super) fn withheld(outcome: DeliveryOutcome) -> Option<Refusal> {
    match outcome {
        DeliveryOutcome::Refused(why) | DeliveryOutcome::Unsubmitted(why) => Some(why),
        DeliveryOutcome::Delivered | DeliveryOutcome::TimedOut => None,
    }
}

/// The word `term:prompt` carries for words parked behind a working agent
/// (t-47952): they go in when its turn ends, and the window says so.
pub(super) const PARKED_WHY: &str = "busy";

/// The word `term:prompt` carries for words whose program never said it was
/// ready (t-47952): the person's own come back for them to paste (t-52566).
pub(super) const NOT_READY_WHY: &str = "not_ready";

/// What the window hears about a settled delivery (`term:prompt`) — the
/// pump's settle and a refused zo launch both say it here, so the event has
/// one shape whoever settled it. A withheld write is named by the guard's
/// token: the window words it in the person's language from its own table
/// (t-10159), and the English sentence stays the receipt's and the black
/// box's. A timeout is named too, by a word of its own ([`NOT_READY_WHY`]).
/// `theirs` says whether the words were the person's own (t-52566): the
/// window says the timeout of its own words to nobody.
pub(super) fn settled_event(
    term: TermId,
    outcome: DeliveryOutcome,
    text: Option<String>,
    theirs: bool,
) -> PromptSettled {
    let why = match outcome {
        DeliveryOutcome::TimedOut => Some(NOT_READY_WHY),
        _ => withheld(outcome).map(Refusal::token),
    };
    PromptSettled {
        term,
        delivered: outcome == DeliveryOutcome::Delivered,
        pasted: outcome.pasted(),
        why,
        text,
        theirs,
    }
}

/// The black-box line for a withheld write.
pub(super) fn withheld_line(term: TermId, outcome: DeliveryOutcome) -> Option<String> {
    match outcome {
        DeliveryOutcome::Refused(why) => Some(format!(
            "term {term}: a queued line was not pasted — {}",
            why.says()
        )),
        DeliveryOutcome::Unsubmitted(why) => Some(format!(
            "term {term}: a pasted line was left unsubmitted — {}",
            why.says()
        )),
        DeliveryOutcome::Delivered | DeliveryOutcome::TimedOut => None,
    }
}

/// What the window hears about words parked behind a working agent
/// (t-47952) — the same event as a settled delivery's, so the window has
/// one listener, with nothing delivered, nothing pasted and nothing handed
/// back: the words are still on their way. Whose words they are rides with
/// it (t-52566): the person hears of their own send's park, and of nothing
/// the window parked for itself.
pub(super) fn parked_event(term: TermId, theirs: bool) -> PromptSettled {
    PromptSettled {
        term,
        delivered: false,
        pasted: false,
        why: Some(PARKED_WHY),
        text: None,
        theirs,
    }
}

/// What the window knew a pane to be doing when one of its prompts parked
/// or settled — the `state=` of the black box's line (t-47952).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaneDoing {
    /// Its program says it is mid-turn ([`pane_is_busy`]).
    Working,
    /// A restart's wake is still waiting on it.
    Resuming,
    /// Neither.
    Idle,
}

impl PaneDoing {
    /// The word the black box's line spells it with.
    pub(super) const fn word(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Resuming => "resuming",
            Self::Idle => "idle",
        }
    }
}

/// What `term` is doing as the window knows it, read once for a line: its
/// program's word that it is mid-turn first, then a restart's wake still
/// waiting on it. Each map is asked under its own lock and let go.
pub(super) fn pane_doing(state: &AppState, term: TermId) -> PaneDoing {
    if pane_is_busy(state, term) {
        PaneDoing::Working
    } else if state.pending_nudges().holds(term) {
        PaneDoing::Resuming
    } else {
        PaneDoing::Idle
    }
}

/// The black-box line one prompt's park, landing or timeout writes
/// (t-47952): `term N prompt <parked|delivered|timed out> after <ms>
/// state=<working|idle|resuming>`, the age counted from when the delivery
/// was made. `None` for a round that did neither, and for a withheld write,
/// whose line [`withheld_line`] already writes.
///
/// A timeout used to leave no line at all: the 08:5x report had the words on
/// the clipboard and nothing to say how long they had waited or what the
/// pane was doing.
pub(super) fn told_line(
    term: TermId,
    turned: Turned,
    after: Duration,
    doing: PaneDoing,
) -> Option<String> {
    let said = match turned {
        Turned::Parked => "parked",
        Turned::Settled(DeliveryOutcome::Delivered) => "delivered",
        Turned::Settled(DeliveryOutcome::TimedOut) => "timed out",
        _ => return None,
    };
    Some(format!(
        "term {term} prompt {said} after {} ms state={}",
        after.as_millis(),
        doing.word()
    ))
}

/// The line's `busy`, unless the pane has written nothing for as long as the
/// pointer takes to read a turn as over without hearing its end (t-47952,
/// [`crate::orchestration::PROVIDER_SILENCE_ENDS_TURN_MS`]).
///
/// A parked send waits on the hook's word that a turn is running, and the
/// word that it ended can be lost on its way to a loaded window. A working
/// program redraws its spinner; ten minutes of nothing from the pane is a
/// turn that ended without telling the window, and the send goes at its
/// rest instead of waiting for good. A pane that never wrote is not judged
/// by a silence nobody measured.
pub(super) fn still_mid_turn(line: Line, last_output: Option<Instant>, now: Instant) -> Line {
    let floor =
        Duration::from_millis(crate::orchestration::PROVIDER_SILENCE_ENDS_TURN_MS.unsigned_abs());
    let silent_past_floor =
        last_output.is_some_and(|wrote| now.saturating_duration_since(wrote) >= floor);
    Line {
        busy: line.busy && !silent_past_floor,
        ..line
    }
}

pub(super) fn departed(
    term: TermId,
    deliveries: &mut HashMap<TermId, PromptDelivery>,
    waiters: &mut HashMap<TermId, std::sync::mpsc::SyncSender<DeliveryOutcome>>,
    queue: &mut HashMap<TermId, VecDeque<QueuedPrompt>>,
) {
    let delivery = deliveries.remove(&term);
    if let Some(waiting) = waiters.remove(&term)
        && let Some(delivery) = delivery
    {
        let outcome = if delivery.pasted() {
            DeliveryOutcome::Unsubmitted(Refusal::LaunchChanged)
        } else {
            DeliveryOutcome::Refused(Refusal::LaunchChanged)
        };
        let _ = waiting.send(outcome);
    }
    queue.remove(&term);
}

/// A delivery has settled: drop it, answer whoever was waiting, and put the
/// next parked prompt on the line — under the same lock, on a fresh clock.
///
/// The swap is under one guard so nothing can start a second delivery for a
/// shell whose first is still being retired; the fresh clock is because a
/// queued prompt's wait begins at activation, not at enqueue, or it would
/// spend its whole readiness budget standing in line (P0-10). The queued
/// copy keeps the clear policy and the guard of the door that made it.
pub(super) fn settle(
    term: TermId,
    outcome: DeliveryOutcome,
    deliveries: &mut HashMap<TermId, PromptDelivery>,
    waiters: &mut HashMap<TermId, std::sync::mpsc::SyncSender<DeliveryOutcome>>,
    queue: &mut HashMap<TermId, VecDeque<QueuedPrompt>>,
    now: Instant,
) {
    deliveries.remove(&term);
    if let Some(waiting) = waiters.remove(&term) {
        let _ = waiting.send(outcome);
    }
    take_next(term, deliveries, waiters, queue, now);
}

/// Put the next parked prompt for `term` on its free line, on a fresh
/// clock, keeping the clear policy and the guard of the door that made it.
/// Answers whether one took the line.
fn take_next(
    term: TermId,
    deliveries: &mut HashMap<TermId, PromptDelivery>,
    waiters: &mut HashMap<TermId, std::sync::mpsc::SyncSender<DeliveryOutcome>>,
    queue: &mut HashMap<TermId, VecDeque<QueuedPrompt>>,
    now: Instant,
) -> bool {
    let Some(row) = queue.get_mut(&term) else {
        return false;
    };
    let next = row.pop_front();
    if row.is_empty() {
        queue.remove(&term);
    }
    let Some(next) = next else {
        return false;
    };
    waiters.insert(term, next.completion);
    deliveries.insert(
        term,
        PromptDelivery::new(next.text, next.submit, next.signal, now)
            .clearing(next.clearing)
            .guarded(next.guard)
            .pressing(next.enter_again)
            .words(next.words)
            .parking(next.parks)
            .handing_back(next.hands_back),
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The maps a pane's prompt line is decided in, owned by a test.
    #[derive(Default)]
    struct Lines {
        deliveries: Mutex<HashMap<TermId, PromptDelivery>>,
        waiters: Mutex<HashMap<TermId, std::sync::mpsc::SyncSender<DeliveryOutcome>>>,
        queue: Mutex<HashMap<TermId, VecDeque<QueuedPrompt>>>,
        moving: Mutex<std::collections::HashSet<TermId>>,
    }

    impl Lines {
        fn borrowed(&self) -> PromptLines<'_> {
            PromptLines {
                deliveries: &self.deliveries,
                waiters: &self.waiters,
                queue: &self.queue,
                moving: &self.moving,
            }
        }

        /// The registration door's decision (`type_prompt_at_term`), made
        /// under deliveries: queue behind a taken line, or take it.
        fn register(&self, term: TermId, text: &str) {
            let mut deliveries = self.deliveries.lock().unwrap();
            let (prompt, _receipt) = parked(text, Guard::for_its_own_line(None));
            if line_is_taken(&deliveries, &self.moving, term) {
                self.queue
                    .lock()
                    .unwrap()
                    .entry(term)
                    .or_default()
                    .push_back(prompt);
            } else {
                self.waiters.lock().unwrap().insert(term, prompt.completion);
                deliveries.insert(
                    term,
                    PromptDelivery::new(prompt.text, prompt.submit, prompt.signal, Instant::now()),
                );
            }
        }

        fn line(&self, term: TermId) -> Option<String> {
            self.deliveries
                .lock()
                .unwrap()
                .get(&term)
                .map(|delivery| delivery.text().to_string())
        }

        fn queued(&self, term: TermId) -> usize {
            self.queue
                .lock()
                .unwrap()
                .get(&term)
                .map_or(0, VecDeque::len)
        }
    }

    /// A pointer that registers between the switch's empty look and its
    /// commit queues behind the move instead of taking the line, and takes it
    /// once the move is written; another pane's prompt is not held at all.
    #[test]
    fn followup_a_switch_holds_its_panes_prompts_through_commit_and_then_hands_them_the_line() {
        const OTHER: TermId = 74975;
        let lines = Lines::default();
        let mut during = None;
        let took = with_no_prompt_in_flight(&lines.borrowed(), 47223, &mut || {
            lines.register(47223, "mail");
            lines.register(OTHER, "other");
            during = Some((lines.line(47223), lines.queued(47223), lines.line(OTHER)));
        });
        assert_eq!(
            during,
            Some((None, 1, Some("other".to_string()))),
            "(the moving pane's line, its queue, another pane's line) while the move commits"
        );
        assert!(took, "the held prompt was not handed the line");
        assert_eq!(lines.line(47223).as_deref(), Some("mail"));
        assert!(lines.waiters.lock().unwrap().contains_key(&47223));
        assert_eq!(lines.queued(47223), 0);
        assert!(
            lines.moving.lock().unwrap().is_empty(),
            "the move left its pane marked"
        );
    }

    #[test]
    fn followup_an_in_flight_prompt_defers_the_switch_until_it_settles() {
        for queued in [false, true] {
            let lines = Lines::default();
            if queued {
                let (prompt, _receipt) =
                    parked("mail", zerocode_pty::ready::Guard::for_its_own_line(None));
                lines
                    .queue
                    .lock()
                    .unwrap()
                    .entry(47223)
                    .or_default()
                    .push_back(prompt);
            } else {
                lines.register(47223, "mail");
            }
            let mut commits = 0;
            let took = with_no_prompt_in_flight(&lines.borrowed(), 47223, &mut || commits += 1);
            assert_eq!((commits, took), (0, false), "queued={queued}");
            assert!(lines.moving.lock().unwrap().is_empty(), "queued={queued}");
            lines.deliveries.lock().unwrap().clear();
            lines.queue.lock().unwrap().clear();
            let took = with_no_prompt_in_flight(&lines.borrowed(), 47223, &mut || commits += 1);
            assert_eq!(
                (commits, took),
                (1, false),
                "the move was lost instead of deferred; queued={queued}"
            );
        }
    }

    /// A switch's commit — the ledger's write, which waits on the disk —
    /// leaves the pump's lock free (t-74974). The pump turns every pane under
    /// deliveries at the display rate, so one pane's move must not stop every
    /// other pane's output for as long as the disk takes.
    #[test]
    fn followup_a_switch_commit_leaves_every_other_pane_pumping() {
        let lines = Lines::default();
        let mut pump_free = None;
        with_no_prompt_in_flight(&lines.borrowed(), 74974, &mut || {
            pump_free = Some(std::thread::scope(|scope| {
                scope
                    .spawn(|| lines.deliveries.try_lock().is_ok())
                    .join()
                    .unwrap()
            }));
        });
        assert_eq!(
            pump_free,
            Some(true),
            "the pump waits out the switch's ledger write"
        );
    }

    /// A commit that unwinds still ends the hold: the pane's prompts are not
    /// queued for good behind a move that never finished.
    #[test]
    fn followup_a_switch_commit_that_unwinds_still_frees_its_pane() {
        let lines = Lines::default();
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_no_prompt_in_flight(&lines.borrowed(), 74974, &mut || {
                lines.register(74974, "mail");
                panic!("the ledger's write failed");
            })
        }));
        assert!(unwound.is_err());
        assert!(lines.moving.lock().unwrap().is_empty());
        assert_eq!(lines.line(74974).as_deref(), Some("mail"));
    }

    /// How long the pump waits for its lock while a switch commits
    /// (t-74974): each switch's pump asks for deliveries the moment the
    /// commit begins, and the wait runs until it gets it. The commit stands
    /// in for the ledger's: one row written to a SQLite file in WAL with
    /// `synchronous = FULL`, on this machine's disk. A measurement, run by
    /// hand with `--ignored --nocapture`.
    #[test]
    #[ignore = "a measurement: prints the pump's wait beside the commit's time"]
    fn measure_the_pump_wait_during_switch_commits() {
        const SWITCHES: usize = 40;
        let root = tempfile::tempdir().expect("a scratch folder");
        let ledger = rusqlite::Connection::open(root.path().join("ledger.sqlite")).unwrap();
        ledger
            .execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; \
                 CREATE TABLE rests (worker INTEGER NOT NULL);",
            )
            .unwrap();
        let lines = Lines::default();
        let mut commits = Vec::with_capacity(SWITCHES);
        let mut waits = Vec::with_capacity(SWITCHES);
        let held_by_the_pump = &lines.deliveries;
        for switch in 0..SWITCHES {
            std::thread::scope(|scope| {
                let mut pump = None;
                with_no_prompt_in_flight(&lines.borrowed(), 74974, &mut || {
                    let asked = Instant::now();
                    pump = Some(scope.spawn(move || {
                        drop(held_by_the_pump.lock().unwrap());
                        asked.elapsed()
                    }));
                    ledger
                        .execute("INSERT INTO rests (worker) VALUES (?1)", [switch])
                        .unwrap();
                    commits.push(asked.elapsed());
                });
                waits.push(pump.expect("the commit ran").join().unwrap());
            });
        }
        commits.sort();
        waits.sort();
        eprintln!(
            "switches={SWITCHES} commit: median {:?}, longest {:?}; \
             pump's wait for its lock: median {:?}, longest {:?}",
            commits[SWITCHES / 2],
            commits[SWITCHES - 1],
            waits[SWITCHES / 2],
            waits[SWITCHES - 1],
        );
    }

    use zerocode_pty::ready::{Guard, PATIENCE, QUIET, Refusal, SUBMIT_GAP, TIMEOUT};

    #[test]
    fn committed_ime_text_is_a_draft_and_failed_human_writes_leave_no_marker() {
        const TERM: TermId = 8_313;
        crate::human_input::forget_term(TERM);
        let old = Line::default();
        assert!(human_write(TERM, "한글".as_bytes(), || Err::<(), _>(())).is_err());
        assert!(!refresh_hand(TERM, old).draft);
        human_write(TERM, "한글".as_bytes(), || Ok::<(), ()>(())).unwrap();
        let current = refresh_hand(TERM, old);
        assert!(current.draft && current.hand.is_some());
        let start = Instant::now();
        let mut delivery = guarded("notice", true, start);
        delivery.poll_line(composer(0), old, start);
        let result = turn(
            TERM,
            &mut delivery,
            composer(1),
            current,
            start + SUBMIT_GAP,
            |_| panic!("a pointer wrote over a committed IME draft"),
        );
        assert!(matches!(
            result,
            Turned::Settled(DeliveryOutcome::Refused(Refusal::HoldsADraft))
        ));
        human_write(TERM, b"\r", || Ok::<(), ()>(())).unwrap();
        crate::human_input::submitted(TERM);
        assert!(!refresh_hand(TERM, old).draft);
        crate::human_input::forget_term(TERM);
    }

    /// The same keystroke, two lines (t-17644): an Enter on words the WINDOW
    /// placed — a restart nudge, a pointer — puts nothing of the person's on
    /// the line; an Enter on words they typed closes over their own.
    #[test]
    fn an_enter_on_the_windows_line_is_not_the_persons_words_but_on_theirs_it_is() {
        const TERM: TermId = 17_644;
        crate::human_input::forget_term(TERM);
        // The window's nudge sat unsent; nobody's hand reached the line.
        assert!(!human_write(TERM, b"\r", || Ok::<(), ()>(())).unwrap());
        // Every Enter spelling says the same, and a second one before the
        // provider's word still adds nothing of theirs.
        for enter in ["\r", "\n", "\r\n", "\x1b[13u", "\x1b[13;1u"] {
            assert!(
                !human_write(TERM, enter.as_bytes(), || Ok::<(), ()>(())).unwrap(),
                "{enter:?} read as the person's words"
            );
        }
        crate::human_input::submitted(TERM);
        // Their own words, then the very same keystroke.
        assert!(human_write(TERM, b"fix it", || Ok::<(), ()>(())).unwrap());
        assert!(human_write(TERM, b"\r", || Ok::<(), ()>(())).unwrap());
        crate::human_input::submitted(TERM);
        assert!(!human_write(TERM, b"\r", || Ok::<(), ()>(())).unwrap());
        // Any other key — an arrow, an interrupt — is still a hand, and adds
        // none of their words (t-57712).
        assert!(!human_write(TERM, b"\x03", || Ok::<(), ()>(())).unwrap());
        crate::human_input::forget_term(TERM);
    }

    fn press(key: &str, ctrl: bool, shift: bool) -> zerocode_pty::KeyPress {
        zerocode_pty::KeyPress {
            key: key.to_string(),
            ctrl,
            alt: false,
            shift,
        }
    }

    fn keyed(key: &str) -> Vec<u8> {
        zerocode_pty::encode_key(&press(key, false, false)).expect("a key the window sends")
    }

    /// The ledger marked two workers' panes as the person's, and the person
    /// typed nothing in them (t-57712). A key that puts no words on the line
    /// takes no pane over, and it is still a hand: the delivery guard sees
    /// the line was reached. The bytes are `term_key`'s own.
    #[test]
    fn a_key_that_puts_no_words_on_the_line_takes_no_pane_over_but_is_still_a_hand() {
        const TERM: TermId = 57_712;
        let old = Line::default();
        for key in [
            press("ArrowUp", false, false),
            press("ArrowDown", false, false),
            press("Escape", false, false),
            press("Tab", false, false),
            press("Tab", false, true),
            press("c", true, false),
        ] {
            crate::human_input::forget_term(TERM);
            let bytes = zerocode_pty::encode_key(&key).expect("a key the window sends");
            assert!(
                !human_write(TERM, &bytes, || Ok::<(), ()>(())).unwrap(),
                "{key:?} took the pane over with no word of the person's"
            );
            let line = refresh_hand(TERM, old);
            assert!(
                line.draft && line.hand.is_some(),
                "{key:?} left the delivery guard blind to a hand"
            );
        }
        crate::human_input::forget_term(TERM);
    }

    /// A pick in a menu — an arrow, then Enter — on a line holding none of
    /// the person's words sends nothing of theirs (t-57712).
    #[test]
    fn an_arrow_then_enter_on_a_line_without_their_words_takes_no_pane_over() {
        const TERM: TermId = 57_713;
        crate::human_input::forget_term(TERM);
        human_write(TERM, &keyed("ArrowDown"), || Ok::<(), ()>(())).unwrap();
        assert!(
            !human_write(TERM, &keyed("Enter"), || Ok::<(), ()>(())).unwrap(),
            "the Enter after an arrow sent words the person never typed"
        );
        crate::human_input::forget_term(TERM);
    }

    /// The person's own words still take a pane over (t-17644, kept by
    /// t-57712): a printable key, committed IME text, a paste — the road the
    /// composer's own send takes — and the Enter that sends them, after any
    /// key that adds none.
    #[test]
    fn the_persons_own_words_still_take_the_pane_over_and_so_does_their_send() {
        const TERM: TermId = 57_714;
        for words in [
            keyed("a"),
            "한글".as_bytes().to_vec(),
            zerocode_pty::encode_paste("fix the build", true),
            zerocode_pty::encode_paste("fix the build", false),
        ] {
            crate::human_input::forget_term(TERM);
            assert!(
                human_write(TERM, &words, || Ok::<(), ()>(())).unwrap(),
                "{words:?} is the person's words"
            );
            human_write(TERM, &keyed("ArrowLeft"), || Ok::<(), ()>(())).unwrap();
            assert!(
                human_write(TERM, &keyed("Enter"), || Ok::<(), ()>(())).unwrap(),
                "the Enter sending {words:?} is the person's send"
            );
        }
        crate::human_input::forget_term(TERM);
    }

    /// A pane parked on its own question or approval reads its keys like any
    /// other line (t-57712). A digit is a printable key and counts as words,
    /// as it did before (the evidence did not settle whether answering the
    /// worker's own dialog should be exempt); an arrow and Enter add none.
    #[test]
    fn a_parked_panes_dialog_keys_follow_the_same_words_rule() {
        const TERM: TermId = 57_715;
        crate::human_input::forget_term(TERM);
        assert!(
            human_write(TERM, &keyed("1"), || Ok::<(), ()>(())).unwrap(),
            "a digit stopped counting as the person's words"
        );
        crate::human_input::forget_term(TERM);
        human_write(TERM, &keyed("ArrowDown"), || Ok::<(), ()>(())).unwrap();
        assert!(
            !human_write(TERM, &keyed("Enter"), || Ok::<(), ()>(())).unwrap(),
            "an arrow and Enter answering a dialog took the pane over"
        );
        crate::human_input::forget_term(TERM);
    }

    #[test]
    fn rejected_paste_never_reports_delivery_or_tries_enter() {
        let start = Instant::now();
        let mut delivery = guarded("mail is waiting", true, start);
        delivery.poll_line(composer(0), Line::default(), start);
        let refused = turn(
            8_311,
            &mut delivery,
            composer(1),
            Line::default(),
            start + SUBMIT_GAP,
            |_| false,
        );
        assert!(
            matches!(refused, Turned::Settled(outcome) if !outcome.pasted()),
            "rejected input was reported as pasted: {refused:?}"
        );
        assert!(
            matches!(delivery.poll_line(composer(1), Line::default(), start + SUBMIT_GAP * 2), Step::Done(outcome) if !outcome.pasted()),
            "a rejected paste was retried or submitted"
        );
    }

    #[test]
    fn rejected_enter_leaves_a_draft_and_never_reports_delivery() {
        let start = Instant::now();
        let mut delivery = guarded("mail is waiting", true, start);
        delivery.poll_line(composer(0), Line::default(), start);
        assert_eq!(
            turn(
                8_312,
                &mut delivery,
                composer(1),
                Line::default(),
                start + SUBMIT_GAP,
                |_| true
            ),
            Turned::Pasted
        );
        let refused = turn(
            8_312,
            &mut delivery,
            composer(1),
            Line::default(),
            start + SUBMIT_GAP * 2,
            |_| false,
        );
        assert!(
            matches!(refused, Turned::Settled(DeliveryOutcome::Unsubmitted(_))),
            "rejected Enter was reported as sent: {refused:?}"
        );
        assert!(
            matches!(
                delivery.poll_line(composer(1), Line::default(), start + SUBMIT_GAP * 3),
                Step::Done(DeliveryOutcome::Unsubmitted(_))
            ),
            "a rejected Enter later became delivered"
        );
    }

    /// t-17037: the Enter alone the restart's watch presses, for words a
    /// resumed pane left on its line with their Enter untaken, is the one
    /// typed-prompt door's own delivery turned by the pump's seam: one
    /// carriage return and nothing else — no clear keys, no paste — and a
    /// person's hand on the line since the words were placed withholds it
    /// entirely. A queued copy stays the Enter alone.
    #[test]
    fn the_enter_alone_writes_one_return_and_yields_to_a_hand_since_the_words() {
        use crate::cmd::terminal::{PromptReadiness, prompt_delivery_for};
        const TERM: TermId = 8_317;
        crate::human_input::forget_term(TERM);
        let placed = crate::human_input::line_of(TERM).1;
        let enter_alone = |now: Instant| {
            prompt_delivery_for(
                String::new(),
                true,
                Some("claude"),
                PromptReadiness::EnterAgain(placed),
                None,
                now,
            )
        };
        // Turn one delivery until it settles; answers every write and how it
        // settled.
        let drive = |delivery: &mut PromptDelivery, start: Instant| {
            let mut wrote: Vec<Vec<u8>> = Vec::new();
            let mut round = 0;
            loop {
                let line = refresh_hand(TERM, Line::default());
                let turned = turn(
                    TERM,
                    delivery,
                    composer(round),
                    line,
                    start + SUBMIT_GAP * u32::try_from(round).unwrap_or(u32::MAX),
                    |bytes| {
                        wrote.push(bytes.to_vec());
                        true
                    },
                );
                if let Turned::Settled(outcome) = turned {
                    return (wrote, outcome);
                }
                round += 1;
                assert!(round < 16, "the Enter alone never settled");
            }
        };
        let start = Instant::now();
        let (wrote, outcome) = drive(&mut enter_alone(start), start);
        assert_eq!(
            wrote,
            vec![b"\r".to_vec()],
            "not one Enter and nothing else"
        );
        assert_eq!(outcome, DeliveryOutcome::Delivered);

        // A person types after the words were placed: nothing is written.
        human_write(TERM, b"x", || Ok::<(), ()>(())).unwrap();
        let (wrote, outcome) = drive(&mut enter_alone(start), start);
        assert!(
            wrote.is_empty(),
            "an Enter went over a person's hand: {wrote:?}"
        );
        assert_eq!(outcome, DeliveryOutcome::Refused(Refusal::HandReached));

        // Parked behind another delivery, it comes back the Enter alone.
        let mut deliveries = HashMap::new();
        let mut waiters = HashMap::new();
        let mut queue: HashMap<TermId, VecDeque<QueuedPrompt>> = HashMap::new();
        let (completion, _receipt) = std::sync::mpsc::sync_channel(1);
        queue.entry(TERM).or_default().push_back(QueuedPrompt {
            text: String::new(),
            submit: true,
            signal: ReadySignal::Rest(None),
            clearing: false,
            guard: Guard::for_somebody_elses_line(None),
            enter_again: Some(zerocode_pty::EnterAgain { hand: placed }),
            words: zerocode_pty::Words::Pasted,
            parks: false,
            hands_back: false,
            completion,
        });
        settle(
            TERM,
            DeliveryOutcome::Delivered,
            &mut deliveries,
            &mut waiters,
            &mut queue,
            start,
        );
        assert_eq!(
            deliveries.get(&TERM).and_then(PromptDelivery::enter_again),
            Some(zerocode_pty::EnterAgain { hand: placed })
        );
        crate::human_input::forget_term(TERM);
    }

    /// The grid's facts for a composer that shook hands and showed its
    /// cursor: the shape every round below feeds.
    fn composer(shows: u64) -> Observed {
        Observed {
            wrote: true,
            bracketed_paste: true,
            cursor_shows: shows,
            ..Observed::default()
        }
    }

    /// A delivery from the door that types on somebody else's behalf: the
    /// ssh send, the mail pointer.
    fn guarded(text: &str, submit: bool, now: Instant) -> PromptDelivery {
        PromptDelivery::new(text.to_string(), submit, ReadySignal::CursorShown, now)
            .clearing(false)
            .guarded(Guard::for_somebody_elses_line(None))
    }

    /// A prompt parked behind another, as `type_prompt_at_term` parks it.
    fn parked(
        text: &str,
        guard: Guard,
    ) -> (QueuedPrompt, std::sync::mpsc::Receiver<DeliveryOutcome>) {
        let (completion, receipt) = std::sync::mpsc::sync_channel(1);
        (
            QueuedPrompt {
                text: text.to_string(),
                submit: true,
                signal: ReadySignal::CursorShown,
                clearing: false,
                guard,
                enter_again: None,
                words: zerocode_pty::Words::Pasted,
                parks: false,
                hands_back: false,
                completion,
            },
            receipt,
        )
    }

    /// One shell's share of the pump's three maps and the bytes its pane
    /// received — what a round below turns — with the black box's lines and
    /// the words handed back to the person, as the pump keeps them.
    #[derive(Default)]
    struct Bench {
        deliveries: HashMap<TermId, PromptDelivery>,
        waiters: HashMap<TermId, std::sync::mpsc::SyncSender<DeliveryOutcome>>,
        queue: HashMap<TermId, VecDeque<QueuedPrompt>>,
        wrote: Vec<u8>,
        lines: Vec<String>,
        handed_back: Vec<String>,
    }

    impl Bench {
        /// One pump round for one shell, exactly as the pump makes it: the
        /// delivery turned against the line facts, its park or its ending
        /// written down with what the line said the pane was doing, and a
        /// settled one retired with its queue swapped in.
        fn round(&mut self, term: TermId, seen: Observed, line: Line, now: Instant) -> Turned {
            let Some(delivery) = self.deliveries.get_mut(&term) else {
                return Turned::Waiting;
            };
            let wrote = &mut self.wrote;
            let turned = turn(term, delivery, seen, line, now, |bytes| {
                wrote.extend_from_slice(bytes);
                true
            });
            let doing = if line.busy {
                PaneDoing::Working
            } else {
                PaneDoing::Idle
            };
            let age = now.saturating_duration_since(delivery.since());
            self.lines.extend(told_line(term, turned, age, doing));
            if let Turned::Settled(outcome) = turned {
                self.handed_back
                    .extend(words_to_hand_back(delivery, outcome));
                settle(
                    term,
                    outcome,
                    &mut self.deliveries,
                    &mut self.waiters,
                    &mut self.queue,
                    now,
                );
            }
            turned
        }
    }

    #[test]
    fn a_departed_prompt_reports_its_paste_before_the_terminal_disappeared() {
        const TERM: TermId = 8_390;
        let start = Instant::now();
        let mut bench = Bench::default();
        let (waiting, receipt) = std::sync::mpsc::sync_channel(1);
        bench
            .deliveries
            .insert(TERM, guarded("continue once", true, start));
        bench.waiters.insert(TERM, waiting);
        bench.round(TERM, composer(0), Line::default(), start);
        assert_eq!(
            bench.round(TERM, composer(1), Line::default(), start + SUBMIT_GAP),
            Turned::Pasted
        );
        departed(
            TERM,
            &mut bench.deliveries,
            &mut bench.waiters,
            &mut bench.queue,
        );
        assert_eq!(
            receipt.try_recv(),
            Ok(DeliveryOutcome::Unsubmitted(Refusal::LaunchChanged)),
            "a pasted prompt was reported as safe to type again"
        );
        assert!(bench.deliveries.is_empty() && bench.waiters.is_empty() && bench.queue.is_empty());
    }

    #[test]
    fn a_departed_prompt_refuses_its_unstarted_input_and_discards_its_queue() {
        const TERM: TermId = 8_391;
        let start = Instant::now();
        let mut bench = Bench::default();
        let (waiting, receipt) = std::sync::mpsc::sync_channel(1);
        bench
            .deliveries
            .insert(TERM, guarded("continue once", true, start));
        bench.waiters.insert(TERM, waiting);
        let (queued, queued_receipt) = parked("later", Guard::for_somebody_elses_line(None));
        bench.queue.entry(TERM).or_default().push_back(queued);
        departed(
            TERM,
            &mut bench.deliveries,
            &mut bench.waiters,
            &mut bench.queue,
        );
        assert_eq!(
            receipt.try_recv(),
            Ok(DeliveryOutcome::Refused(Refusal::LaunchChanged))
        );
        assert!(bench.wrote.is_empty());
        assert_eq!(
            queued_receipt.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        );
        assert!(bench.deliveries.is_empty() && bench.waiters.is_empty() && bench.queue.is_empty());
    }

    /// The line facts as the pump gathers them, minus the two that need a
    /// window (`pane_states`, `launch_tokens`): the tracker's own.
    fn line_facts_without_a_window(term: TermId) -> Line {
        let (draft, hand) = crate::human_input::line_of(term);
        Line {
            draft,
            hand,
            ..Line::default()
        }
    }

    /// Review item 1, at the seam the production notification now uses: a
    /// draft on the line when the paste comes due refuses the whole
    /// delivery, nothing is written, the words come back, and the line is
    /// freed for the next prompt in the queue.
    #[test]
    fn a_draft_at_the_write_refuses_the_delivery_and_hands_the_line_to_the_next() {
        const TERM: TermId = 8_301;
        crate::human_input::forget_term(TERM);
        let start = Instant::now();
        let mut bench = Bench::default();

        let (settled, pointer) = std::sync::mpsc::sync_channel(1);
        bench
            .deliveries
            .insert(TERM, guarded("mail is waiting", true, start));
        bench.waiters.insert(TERM, settled);
        let (next, next_receipt) = parked("ls", Guard::for_its_own_line(None));
        bench.queue.entry(TERM).or_default().push_back(next);

        // Registered against an untouched line; the person types before
        // the composer settles.
        assert_eq!(
            bench.round(TERM, composer(0), Line::default(), start),
            Turned::Waiting
        );
        let drafting = Line {
            draft: true,
            hand: Some(1),
            ..Line::default()
        };
        assert_eq!(
            bench.round(TERM, composer(1), drafting, start + SUBMIT_GAP),
            Turned::Settled(DeliveryOutcome::Refused(Refusal::HoldsADraft))
        );
        assert!(
            bench.wrote.is_empty(),
            "something was written onto a draft: {:?}",
            bench.wrote
        );
        assert_eq!(
            pointer.try_recv(),
            Ok(DeliveryOutcome::Refused(Refusal::HoldsADraft))
        );
        assert!(
            !bench.queue.contains_key(&TERM),
            "the queue kept the prompt it activated"
        );
        assert!(
            bench.deliveries.contains_key(&TERM),
            "the next prompt did not take the line"
        );
        assert!(
            next_receipt.try_recv().is_err(),
            "the queued prompt was settled unstarted"
        );

        // The next prompt is a person's own: their draft does not refuse it,
        // and it goes through as it always did.
        let mut later = start + SUBMIT_GAP;
        bench.round(TERM, composer(1), drafting, later);
        later += SUBMIT_GAP;
        assert_eq!(
            bench.round(TERM, composer(2), drafting, later),
            Turned::Pasted
        );
        assert!(bench.wrote.starts_with(b"\x1b[200~ls"), "{:?}", bench.wrote);
    }

    /// Review item 2, at the seam: a hand AFTER the paste — the interval the
    /// old two-job ssh door could not see — withholds the Enter. No carriage
    /// return is written; the receipt says the words were left on the line;
    /// and the ordinary prompt queued during the gap lands only after, never
    /// between the paste and where the Enter would have gone.
    #[test]
    fn a_hand_after_the_paste_withholds_the_enter_and_the_next_prompt_waits_its_turn() {
        const TERM: TermId = 8_302;
        crate::human_input::forget_term(TERM);
        let start = Instant::now();
        let mut bench = Bench::default();

        let (settled, ssh) = std::sync::mpsc::sync_channel(1);
        bench
            .deliveries
            .insert(TERM, guarded("check --ack", true, start));
        bench.waiters.insert(TERM, settled);

        let before = Line {
            hand: Some(7),
            ..Line::default()
        };
        bench.round(TERM, composer(0), before, start);
        assert_eq!(
            bench.round(TERM, composer(1), before, start + SUBMIT_GAP),
            Turned::Pasted
        );
        let pasted = bench.wrote.len();
        assert!(bench.wrote.ends_with(b"\x1b[201~"), "{:?}", bench.wrote);

        // An ordinary prompt arrives during the gap — the case the review
        // named — and parks behind the transaction.
        let (ordinary, ordinary_receipt) = parked("and then this", Guard::for_its_own_line(None));
        bench.queue.entry(TERM).or_default().push_back(ordinary);
        // And the person reaches the line.
        let reached = Line {
            draft: true,
            hand: Some(8),
            ..Line::default()
        };
        assert_eq!(
            bench.round(TERM, composer(1), reached, start + SUBMIT_GAP * 2),
            Turned::Settled(DeliveryOutcome::Unsubmitted(Refusal::HandReached))
        );
        assert_eq!(
            bench.wrote.len(),
            pasted,
            "bytes followed the withheld Enter: {:?}",
            &bench.wrote[pasted..]
        );
        assert!(
            !bench.wrote.contains(&b'\r'),
            "a carriage return was written: {:?}",
            bench.wrote
        );
        assert_eq!(
            ssh.try_recv(),
            Ok(DeliveryOutcome::Unsubmitted(Refusal::HandReached))
        );
        assert!(ordinary_receipt.try_recv().is_err());

        // The ordinary prompt has the line now, on its own clock, and its
        // paste comes AFTER everything the transaction wrote.
        let activated = start + SUBMIT_GAP * 2;
        bench.round(TERM, composer(1), reached, activated);
        assert_eq!(
            bench.round(TERM, composer(2), reached, activated + SUBMIT_GAP),
            Turned::Pasted
        );
        assert!(
            bench.wrote[pasted..].starts_with(b"\x1b[200~and then this"),
            "{:?}",
            &bench.wrote[pasted..]
        );
    }

    /// An approval parked between the writes withholds the Enter: the words
    /// stay a draft rather than answering a question nobody asked this door.
    #[test]
    fn an_approval_parked_between_the_writes_withholds_the_enter() {
        const TERM: TermId = 8_303;
        crate::human_input::forget_term(TERM);
        let start = Instant::now();
        let mut bench = Bench::default();
        bench.deliveries.insert(TERM, guarded("go on", true, start));

        bench.round(TERM, composer(0), Line::default(), start);
        assert_eq!(
            bench.round(TERM, composer(1), Line::default(), start + SUBMIT_GAP),
            Turned::Pasted
        );
        let parked_now = Line {
            parked: true,
            ..Line::default()
        };
        assert_eq!(
            bench.round(TERM, composer(1), parked_now, start + SUBMIT_GAP * 2),
            Turned::Settled(DeliveryOutcome::Unsubmitted(Refusal::Parked))
        );
        assert!(!bench.wrote.contains(&b'\r'));
        assert!(bench.deliveries.is_empty());
        // And a question parked BEFORE the paste refuses it outright.
        let mut refused = Bench::default();
        refused
            .deliveries
            .insert(TERM, guarded("go on", true, start));
        refused.round(TERM, composer(0), parked_now, start);
        assert_eq!(
            refused.round(TERM, composer(1), parked_now, start + SUBMIT_GAP),
            Turned::Settled(DeliveryOutcome::Refused(Refusal::Parked))
        );
        assert!(refused.wrote.is_empty());
    }

    /// Review item 3, the other half: the pump's own Enter is a gesture the
    /// provider's prompt event answers — so a person's draft that a launch
    /// briefing or their own send legitimately submitted is cleared by the
    /// report of it, while a withheld Enter leaves the draft standing for
    /// the same report.
    #[test]
    fn the_pumps_enter_is_the_gesture_the_providers_word_answers() {
        const TERM: TermId = 8_304;
        const OTHER: TermId = 8_305;
        crate::human_input::forget_term(TERM);
        crate::human_input::forget_term(OTHER);
        let start = Instant::now();
        let mut wrote = Vec::new();

        // A person typed here; their own send from the window submits it.
        crate::human_input::typed(TERM);
        let mut own =
            PromptDelivery::new("their words".into(), true, ReadySignal::CursorShown, start)
                .clearing(false)
                .guarded(Guard::for_its_own_line(None));
        let facts = || line_facts_without_a_window(TERM);
        turn(TERM, &mut own, composer(0), facts(), start, |_| true);
        assert_eq!(
            turn(
                TERM,
                &mut own,
                composer(1),
                facts(),
                start + SUBMIT_GAP,
                |bytes| {
                    wrote.extend_from_slice(bytes);
                    true
                }
            ),
            Turned::Pasted
        );
        assert_eq!(
            turn(
                TERM,
                &mut own,
                composer(1),
                facts(),
                start + SUBMIT_GAP * 2,
                |bytes| {
                    wrote.extend_from_slice(bytes);
                    true
                }
            ),
            Turned::Entered
        );
        assert!(wrote.ends_with(b"\r"));
        assert!(
            crate::human_input::holds_a_draft(TERM),
            "the Enter alone was taken as the provider's word"
        );
        crate::human_input::submitted(TERM);
        assert!(
            !crate::human_input::holds_a_draft(TERM),
            "the provider's word did not answer the pump's Enter"
        );

        // The same person, the same draft, and a guarded delivery whose
        // Enter was withheld because they kept typing: the provider's next
        // report (of something else entirely) does not clear their draft.
        crate::human_input::typed(OTHER);
        let mut guarded_one = guarded("pointer", true, start);
        let hand = crate::human_input::generation(OTHER);
        let quiet = Line {
            draft: false,
            hand,
            ..Line::default()
        };
        turn(OTHER, &mut guarded_one, composer(0), quiet, start, |_| true);
        assert_eq!(
            turn(
                OTHER,
                &mut guarded_one,
                composer(1),
                quiet,
                start + SUBMIT_GAP,
                |_| true
            ),
            Turned::Pasted
        );
        crate::human_input::typed(OTHER);
        assert_eq!(
            turn(
                OTHER,
                &mut guarded_one,
                composer(1),
                line_facts_without_a_window(OTHER),
                start + SUBMIT_GAP * 2,
                |_| panic!("an Enter was written over a hand")
            ),
            Turned::Settled(DeliveryOutcome::Unsubmitted(Refusal::HandReached))
        );
        crate::human_input::submitted(OTHER);
        assert!(
            crate::human_input::holds_a_draft(OTHER),
            "a report nothing entered for cleared a standing draft"
        );
        crate::human_input::forget_term(TERM);
        crate::human_input::forget_term(OTHER);
    }

    /// A pane relaunched between registration and the write is a different
    /// program: the person's words come back rather than being typed at it.
    #[test]
    fn a_relaunch_between_registration_and_the_write_hands_the_words_back() {
        const TERM: TermId = 8_306;
        let start = Instant::now();
        let mut delivery = PromptDelivery::new(
            "resume the task".into(),
            true,
            ReadySignal::CursorShown,
            start,
        )
        .guarded(Guard::for_its_own_line(Some(1)))
        .handing_back(true);
        let relaunched = Line {
            launch: Some(2),
            ..Line::default()
        };
        turn(TERM, &mut delivery, composer(0), relaunched, start, |_| {
            true
        });
        let outcome = match turn(
            TERM,
            &mut delivery,
            composer(1),
            relaunched,
            start + SUBMIT_GAP,
            |_| panic!("typed at the wrong program"),
        ) {
            Turned::Settled(outcome) => outcome,
            other => panic!("{other:?}"),
        };
        assert_eq!(outcome, DeliveryOutcome::Refused(Refusal::LaunchChanged));
        assert_eq!(
            words_to_hand_back(&delivery, outcome).as_deref(),
            Some("resume the task")
        );
        assert_eq!(withheld(outcome), Some(Refusal::LaunchChanged));
        assert!(
            withheld_line(TERM, outcome)
                .expect("a line")
                .contains("not pasted")
        );
        // Words that reached the composer are not handed back a second time.
        assert_eq!(
            words_to_hand_back(
                &delivery,
                DeliveryOutcome::Unsubmitted(Refusal::HandReached)
            ),
            None
        );
        assert_eq!(withheld(DeliveryOutcome::Delivered), None);
        assert_eq!(withheld_line(TERM, DeliveryOutcome::TimedOut), None);
    }

    /// The window hears a withheld write by the guard's token, never by its
    /// English sentence: the sentence is the receipt's and the black box's,
    /// and the window words the token in the person's language from a table
    /// of its own (t-10159). A delivery that landed names nothing; one whose
    /// program never said it was ready names that, and words parked behind
    /// a working agent name the turn they wait out (t-47952) — neither of
    /// them a refusal, so neither one of the guard's tokens.
    #[test]
    fn the_window_hears_a_withheld_write_by_its_token() {
        const TERM: TermId = 8_314;
        let pointer = "\nYou have 2 orchestration messages. Run `zerocode-orc check`.\n";
        let refused = serde_json::to_value(settled_event(
            TERM,
            DeliveryOutcome::Refused(Refusal::HoldsADraft),
            None,
            false,
        ))
        .expect("the event serializes");
        assert_eq!(
            refused,
            serde_json::json!({
                "term": TERM,
                "delivered": false,
                "pasted": false,
                "why": "holds_a_draft",
                "text": null,
                "theirs": false,
            })
        );
        let held = serde_json::to_value(settled_event(
            TERM,
            DeliveryOutcome::Refused(Refusal::HoldsADraft),
            Some(pointer.to_string()),
            true,
        ))
        .expect("the event serializes");
        assert_eq!(held["text"], pointer);
        assert_eq!(held["theirs"], true);
        let left = serde_json::to_value(settled_event(
            TERM,
            DeliveryOutcome::Unsubmitted(Refusal::HandReached),
            None,
            true,
        ))
        .expect("the event serializes");
        assert_eq!(left["pasted"], true);
        assert_eq!(left["why"], "hand_reached");
        for refusal in Refusal::ALL {
            let said = serde_json::to_value(settled_event(
                TERM,
                DeliveryOutcome::Refused(refusal),
                None,
                false,
            ))
            .expect("the event serializes");
            assert_eq!(said["why"], refusal.token(), "{refusal:?}");
            assert_ne!(said["why"], refusal.says(), "{refusal:?}");
        }
        let landed =
            serde_json::to_value(settled_event(TERM, DeliveryOutcome::Delivered, None, true))
                .expect("serializes");
        assert_eq!(landed["why"], serde_json::Value::Null);
        let lost = serde_json::to_value(settled_event(
            TERM,
            DeliveryOutcome::TimedOut,
            Some("look at this page".to_string()),
            true,
        ))
        .expect("serializes");
        assert_eq!(
            lost,
            serde_json::json!({
                "term": TERM,
                "delivered": false,
                "pasted": false,
                "why": "not_ready",
                "text": "look at this page",
                "theirs": true,
            })
        );
        let parked = serde_json::to_value(parked_event(TERM, true)).expect("serializes");
        assert_eq!(
            parked,
            serde_json::json!({
                "term": TERM,
                "delivered": false,
                "pasted": false,
                "why": "busy",
                "text": null,
                "theirs": true,
            })
        );
        for refusal in Refusal::ALL {
            assert!(
                refusal.token() != PARKED_WHY && refusal.token() != NOT_READY_WHY,
                "{refusal:?} shares a word with a prompt that is no refusal"
            );
        }
    }

    /// A dead shell's receipt reaches its waiter, and a queue behind a
    /// settled delivery keeps its order and its guards.
    #[test]
    fn the_queue_keeps_its_order_and_each_prompts_guard() {
        const TERM: TermId = 8_307;
        let start = Instant::now();
        let mut bench = Bench::default();
        bench.deliveries.insert(TERM, guarded("first", true, start));
        let (guarded_next, _a) = parked("second", Guard::for_somebody_elses_line(Some(5)));
        let (own_next, _b) = parked("third", Guard::for_its_own_line(None));
        bench.queue.entry(TERM).or_default().push_back(guarded_next);
        bench.queue.entry(TERM).or_default().push_back(own_next);

        settle(
            TERM,
            DeliveryOutcome::TimedOut,
            &mut bench.deliveries,
            &mut bench.waiters,
            &mut bench.queue,
            start,
        );
        assert_eq!(bench.deliveries[&TERM].text(), "second");
        assert_eq!(
            bench.deliveries[&TERM].guard(),
            Guard::for_somebody_elses_line(Some(5))
        );
        assert_eq!(bench.queue[&TERM].len(), 1);
        settle(
            TERM,
            DeliveryOutcome::Delivered,
            &mut bench.deliveries,
            &mut bench.waiters,
            &mut bench.queue,
            start,
        );
        assert_eq!(bench.deliveries[&TERM].text(), "third");
        assert_eq!(
            bench.deliveries[&TERM].guard(),
            Guard::for_its_own_line(None)
        );
        assert!(!bench.queue.contains_key(&TERM));
        settle(
            TERM,
            DeliveryOutcome::Delivered,
            &mut bench.deliveries,
            &mut bench.waiters,
            &mut bench.queue,
            start,
        );
        assert!(bench.deliveries.is_empty());
    }

    /* ---- a send to a working agent waits its turn out (t-47952) -------- */

    /// The person's send, as the window's door makes it.
    fn sent(text: &str, agent: &str, now: Instant) -> PromptDelivery {
        crate::cmd::terminal::prompt_delivery_for(
            text.to_string(),
            true,
            Some(agent),
            crate::cmd::terminal::PromptReadiness::Resting,
            None,
            now,
        )
    }

    /// A program mid-turn as the grid sees it: a spinner redrawing, its
    /// bracketed paste long on, no cursor shown again.
    fn spinning() -> Observed {
        Observed {
            wrote: true,
            bracketed_paste: true,
            ..Observed::default()
        }
    }

    /// The line while the hook says the turn is running.
    fn working() -> Line {
        Line {
            busy: true,
            ..Line::default()
        }
    }

    /// The person's report, 10-12 08:5x: a prompt sent from the window to a
    /// zo resumed after the 1.1.58 restart, still in its long turn, ran out
    /// the send's eight seconds against the spinner and landed on the
    /// clipboard — and nothing in the black box said so. The send waits the
    /// turn out now: the round it parks is said, nothing is written and
    /// nothing settles past `PATIENCE`; the round the composer rests after
    /// the turn, the paste goes in, its Enter follows and the waiter hears
    /// it delivered — and the black box has one line for the park and one
    /// for the landing, each with its age and what the pane was doing.
    #[test]
    fn a_prompt_to_a_working_pane_is_parked_and_goes_in_at_the_rest_after_its_turn() {
        const TERM: TermId = 8_501;
        crate::human_input::forget_term(TERM);
        let start = Instant::now();
        let mut bench = Bench::default();
        let (settled, receipt) = std::sync::mpsc::sync_channel(1);
        bench
            .deliveries
            .insert(TERM, sent("look at this page", "zo", start));
        bench.waiters.insert(TERM, settled);

        assert_eq!(
            bench.round(TERM, spinning(), working(), start),
            Turned::Parked
        );
        let mut now = start;
        while now <= start + PATIENCE + TIMEOUT {
            now += QUIET / 2;
            assert_eq!(
                bench.round(TERM, spinning(), working(), now),
                Turned::Waiting,
                "a send to a working pane was turned {:?} in",
                now - start
            );
        }
        assert!(bench.wrote.is_empty(), "{:?}", bench.wrote);
        assert!(
            receipt.try_recv().is_err(),
            "a parked send was settled while its pane worked"
        );
        assert!(bench.handed_back.is_empty());

        // The turn is over: the wait's new handshake round, then the
        // composer's cursor shown again — the rest, and the paste with it.
        now += SUBMIT_GAP;
        assert_eq!(
            bench.round(TERM, spinning(), Line::default(), now),
            Turned::Waiting
        );
        now += SUBMIT_GAP;
        assert_eq!(
            bench.round(TERM, composer(1), Line::default(), now),
            Turned::Pasted
        );
        now += SUBMIT_GAP;
        assert_eq!(
            bench.round(TERM, composer(1), Line::default(), now),
            Turned::Entered
        );
        now += SUBMIT_GAP;
        assert_eq!(
            bench.round(TERM, composer(1), Line::default(), now),
            Turned::Settled(DeliveryOutcome::Delivered)
        );
        assert_eq!(receipt.try_recv(), Ok(DeliveryOutcome::Delivered));
        assert!(
            bench.wrote.starts_with(b"\x1b[200~look at this page") && bench.wrote.ends_with(b"\r"),
            "{:?}",
            bench.wrote
        );
        assert!(bench.handed_back.is_empty());
        assert_eq!(
            bench.lines,
            vec![
                format!("term {TERM} prompt parked after 0 ms state=working"),
                format!(
                    "term {TERM} prompt delivered after {} ms state=idle",
                    (now - start).as_millis()
                ),
            ]
        );
        crate::human_input::forget_term(TERM);
    }

    /// t-47952, the road that stays: a pane that is NOT working and never
    /// rests — its stream never quiet, no cursor shown, no glyph — still
    /// runs out the send's deadline, and its words still go back for the
    /// clipboard; the black box now has the one line that says so.
    #[test]
    fn an_idle_pane_that_never_rests_still_times_out_and_says_so_in_one_line() {
        const TERM: TermId = 8_502;
        let start = Instant::now();
        let mut bench = Bench::default();
        bench
            .deliveries
            .insert(TERM, sent("look at this page", "zo", start));
        let mut now = start;
        let outcome = loop {
            match bench.round(TERM, spinning(), Line::default(), now) {
                Turned::Waiting => {}
                Turned::Settled(outcome) => break outcome,
                other => panic!("a send to an idle pane was turned {other:?}"),
            }
            now += QUIET / 2;
            assert!(
                now <= start + TIMEOUT + QUIET,
                "the send outlived its deadline"
            );
        };
        assert_eq!(outcome, DeliveryOutcome::TimedOut);
        assert!(bench.wrote.is_empty(), "{:?}", bench.wrote);
        assert_eq!(bench.handed_back, ["look at this page"]);
        assert_eq!(
            bench.lines,
            [format!(
                "term {TERM} prompt timed out after {} ms state=idle",
                (now - start).as_millis()
            )]
        );
    }

    /// t-47952: a send queued behind another delivery parks too, once it
    /// has the line — the queue carries the door's choice like its guard;
    /// and a send that parks again, a new turn starting before its rest,
    /// is said once.
    #[test]
    fn a_queued_send_parks_when_it_takes_the_line_and_is_said_once() {
        const TERM: TermId = 8_503;
        let start = Instant::now();
        let mut bench = Bench::default();
        bench
            .deliveries
            .insert(TERM, guarded("mail is waiting", true, start));
        let resting = crate::cmd::terminal::PromptReadiness::Resting;
        let (completion, _receipt) = std::sync::mpsc::sync_channel(1);
        bench
            .queue
            .entry(TERM)
            .or_default()
            .push_back(QueuedPrompt {
                text: "look at this page".to_string(),
                submit: true,
                signal: ReadySignal::Rest(None),
                clearing: false,
                guard: Guard::for_its_own_line(None),
                enter_again: None,
                words: zerocode_pty::Words::Pasted,
                parks: resting.parks(),
                hands_back: resting.hands_back(),
                completion,
            });
        settle(
            TERM,
            DeliveryOutcome::Delivered,
            &mut bench.deliveries,
            &mut bench.waiters,
            &mut bench.queue,
            start,
        );
        assert!(
            bench.deliveries[&TERM].parks(),
            "the queued send lost the door's choice to wait out a turn"
        );
        assert!(
            bench.deliveries[&TERM].hands_back(),
            "the queued send stopped being the person's words (t-52566)"
        );
        assert_eq!(
            bench.round(TERM, spinning(), working(), start),
            Turned::Parked
        );
        // The turn ends, and another begins before the composer rests.
        assert_eq!(
            bench.round(TERM, spinning(), Line::default(), start + QUIET),
            Turned::Waiting
        );
        assert_eq!(
            bench.round(TERM, spinning(), working(), start + QUIET * 2),
            Turned::Waiting
        );
        assert_eq!(
            bench.round(TERM, spinning(), working(), start + TIMEOUT * 2),
            Turned::Waiting
        );
        assert_eq!(bench.lines.len(), 1, "{:?}", bench.lines);
    }

    /// t-47952, the roads that stay as they were. Only the person's own
    /// send parks: the mail pointer's door types once its pass has read the
    /// turn as over and reads the receipt on its next beat; a launch
    /// briefing answers its worker start, whose refusal names the door that
    /// stayed shut; a restart's continuation and its Enter alone have their
    /// own watch. Beside a working pane the pointer's line still runs out
    /// the deadline it always had.
    #[test]
    fn the_pointer_and_a_briefing_never_park() {
        use crate::cmd::terminal::PromptReadiness;
        let start = Instant::now();
        for readiness in [
            PromptReadiness::RestingBesideADraft,
            PromptReadiness::Mounting,
            PromptReadiness::MountingBesideADraft,
            PromptReadiness::EnterAgain(None),
        ] {
            assert!(!readiness.parks(), "{readiness:?}");
            let delivery = crate::cmd::terminal::prompt_delivery_for(
                "words".to_string(),
                true,
                Some("claude"),
                readiness,
                None,
                start,
            );
            assert!(!delivery.parks(), "{readiness:?}");
        }
        const TERM: TermId = 8_504;
        let mut bench = Bench::default();
        bench.deliveries.insert(
            TERM,
            crate::cmd::terminal::prompt_delivery_for(
                "You have 1 orchestration message.".to_string(),
                true,
                Some("claude"),
                PromptReadiness::RestingBesideADraft,
                None,
                start,
            ),
        );
        assert_eq!(
            bench.round(TERM, spinning(), working(), start),
            Turned::Waiting
        );
        assert_eq!(
            bench.round(TERM, spinning(), working(), start + TIMEOUT),
            Turned::Settled(DeliveryOutcome::TimedOut)
        );
    }

    /// t-47952: parking waits on the hook's word that a turn is running, and
    /// a turn's end can be lost on its way to the window. A pane that has
    /// written nothing for as long as the pointer takes to read a turn as
    /// over without hearing its end is not mid-turn — a working program
    /// redraws its spinner — so a parked send is not held there for good. A
    /// pane still writing stays mid-turn, and one that never wrote is not
    /// judged by a silence nobody measured.
    #[test]
    fn a_working_word_on_a_pane_silent_past_the_pointers_floor_is_not_a_turn() {
        let floor = Duration::from_millis(
            crate::orchestration::PROVIDER_SILENCE_ENDS_TURN_MS.unsigned_abs(),
        );
        let wrote = Instant::now();
        assert!(still_mid_turn(working(), Some(wrote), wrote + Duration::from_secs(1)).busy);
        assert!(still_mid_turn(working(), None, wrote + floor).busy);
        assert!(!still_mid_turn(working(), Some(wrote), wrote + floor).busy);
        assert!(!still_mid_turn(Line::default(), Some(wrote), wrote).busy);
    }

    /* ---- the window's own words are not the person's to paste (t-52566) - */

    /// Turn the delivery at `term` until it settles — no longer than its
    /// deadline and a beat — feeding the same grid and line every round.
    fn until_settled(
        bench: &mut Bench,
        term: TermId,
        seen: Observed,
        line: Line,
        from: Instant,
    ) -> (DeliveryOutcome, Instant) {
        let mut now = from;
        loop {
            if let Turned::Settled(outcome) = bench.round(term, seen, line, now) {
                return (outcome, now);
            }
            now += QUIET / 2;
            assert!(
                now <= from + TIMEOUT + QUIET,
                "the delivery outlived its deadline"
            );
        }
    }

    /// 2026-10-14 10:46, the person's 「또 이버그」: three deliveries at a zo
    /// worker the ledger reseated after a restart (term 11). The restore's
    /// wait for the resumed composer carried no words and landed at rest;
    /// the continuation landed, and the worker's turn began; the mail
    /// pointer, queued behind the continuation, took the line inside that
    /// turn and ran out its eight seconds against the spinner — and the pump
    /// handed the pointer's line back as words the person had lost: an
    /// error, and the window's own advice over whatever was on their
    /// clipboard. A worker's launch briefing that ran out of time went the
    /// same way. The window's own words are never handed back, whatever
    /// became of them, and the black box keeps the line that says what did;
    /// the person's own send still is (`an_idle_pane_that_never_rests_…`).
    #[test]
    fn the_windows_own_words_are_never_handed_back_to_the_person() {
        const TERM: TermId = 8_566;
        crate::human_input::forget_term(TERM);
        let start = Instant::now();
        let mut bench = Bench::default();
        // The restore's wait for the resumed composer: no words, no Enter.
        bench.deliveries.insert(
            TERM,
            PromptDelivery::with_deadlines(
                String::new(),
                false,
                ReadySignal::CursorShown,
                start,
                QUIET,
                TIMEOUT,
            )
            .clearing(false)
            .guarded(Guard::for_its_own_line(None)),
        );
        assert_eq!(
            bench.round(TERM, composer(0), Line::default(), start),
            Turned::Waiting
        );
        let mut now = start + SUBMIT_GAP;
        assert_eq!(
            bench.round(TERM, composer(1), Line::default(), now),
            Turned::Settled(DeliveryOutcome::Delivered)
        );
        // The continuation lands, and the worker's turn begins.
        bench
            .deliveries
            .insert(TERM, guarded(crate::RESTART_NUDGE, true, now));
        assert_eq!(
            bench.round(TERM, composer(1), Line::default(), now),
            Turned::Waiting
        );
        now += SUBMIT_GAP;
        assert_eq!(
            bench.round(TERM, composer(2), Line::default(), now),
            Turned::Pasted
        );
        now += SUBMIT_GAP;
        assert_eq!(
            bench.round(TERM, composer(2), Line::default(), now),
            Turned::Entered
        );
        now += SUBMIT_GAP;
        assert_eq!(
            bench.round(TERM, composer(2), Line::default(), now),
            Turned::Settled(DeliveryOutcome::Delivered)
        );
        let typed = bench.wrote.len();
        // The pointer's line takes the line as the continuation lands, inside
        // the turn the continuation began.
        bench.deliveries.insert(
            TERM,
            crate::cmd::terminal::prompt_delivery_for(
                zerocode_core::orchestration::pointer_text(1),
                true,
                Some("zo"),
                crate::cmd::terminal::PromptReadiness::RestingBesideADraft,
                None,
                now,
            ),
        );
        let (pointed, ended) = until_settled(&mut bench, TERM, spinning(), working(), now);
        assert_eq!(bench.wrote.len(), typed, "the pointer typed into the turn");
        // A worker's launch briefing at a composer that shook hands and never
        // stood.
        now = ended;
        bench.deliveries.insert(
            TERM,
            PromptDelivery::with_deadlines(
                "carry the task".to_string(),
                true,
                ReadySignal::CursorShown,
                now,
                QUIET,
                TIMEOUT,
            )
            .clearing(false)
            .guarded(Guard::for_its_own_line(None)),
        );
        let silent = Observed {
            bracketed_paste: true,
            ..Observed::default()
        };
        let (briefed, _) = until_settled(&mut bench, TERM, silent, Line::default(), now);
        assert_eq!(
            bench.handed_back,
            Vec::<String>::new(),
            "the window's own words were handed to the person as words they had lost \
             (the pointer {pointed:?}, the briefing {briefed:?})"
        );
        assert_eq!(
            bench
                .lines
                .iter()
                .filter(|line| line.contains("prompt timed out after")
                    && line.ends_with("state=working"))
                .count(),
            usize::from(pointed == DeliveryOutcome::TimedOut),
            "the black box lost the line that says what became of the pointer: {:?}",
            bench.lines
        );
        crate::human_input::forget_term(TERM);
    }

    /// t-52566: whose words a delivery carries is its door's answer. Only
    /// the person's own send hands its words back; every other readiness
    /// types the window's own, and nothing of them goes back whatever became
    /// of them — and a delivery a door builds for itself, a worker's launch
    /// briefing, is the window's unless the door says otherwise.
    #[test]
    fn only_the_persons_own_send_hands_its_words_back() {
        use crate::cmd::terminal::PromptReadiness;
        let start = Instant::now();
        for (readiness, theirs) in [
            (PromptReadiness::Resting, true),
            (PromptReadiness::RestingBesideADraft, false),
            (PromptReadiness::Mounting, false),
            (PromptReadiness::MountingBesideADraft, false),
            (PromptReadiness::EnterAgain(None), false),
        ] {
            assert_eq!(readiness.hands_back(), theirs, "{readiness:?}");
            let delivery = crate::cmd::terminal::prompt_delivery_for(
                "words".to_string(),
                true,
                Some("zo"),
                readiness,
                None,
                start,
            );
            assert_eq!(delivery.hands_back(), theirs, "{readiness:?}");
            for outcome in [
                DeliveryOutcome::TimedOut,
                DeliveryOutcome::Refused(Refusal::HoldsADraft),
            ] {
                assert_eq!(
                    words_to_hand_back(&delivery, outcome).is_some(),
                    theirs,
                    "{readiness:?} {outcome:?}"
                );
            }
        }
        let briefing = PromptDelivery::new(
            "carry the task".to_string(),
            true,
            ReadySignal::CursorShown,
            start,
        );
        assert!(!briefing.hands_back());
        assert!(briefing.handing_back(true).hands_back());
    }

    /* ---- a restart's words reach a real program once (t-14037) --------- */

    /// How a fake agent CLI treats what it is given — the shapes a restart
    /// meets at the panes it brings back, measured on Claude Code 2.1.284.
    #[cfg(unix)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Takes {
        /// At rest: the Enter is a send.
        AtRest,
        /// Still booting: the first Enter is swallowed and the words stay in
        /// the composer — 07:09 on 2026-09-29, two restored workers.
        Booting,
        /// Booting, and it takes the Enter late: the receipt comes after the
        /// window, when the Enter pressed again finds an empty composer — the
        /// fake reports the words at that Enter.
        Late,
        /// Mid-turn: the words wait in its own queue and are taken, and
        /// reported, when the turn ends — the fake's next line, which the
        /// restart sends once the window has called the delivery delivered.
        Busy,
    }

    /// A pane's program as a shell script on a cooked line: `read` returns a
    /// line at each Enter. It records each send in `$FAKE_SENT`, one line a
    /// send, and an empty Enter sends nothing — as Claude Code, Codex and zo
    /// all do. A late or busy send is reported at its next line, not on a
    /// clock: the `sleep` it once started took longer than the receipt's
    /// patience to start on the efficiency cores (t-82959).
    #[cfg(unix)]
    const FAKE_AGENT: &str = r#"#!/bin/sh
printf '\033[?2004h'
printf '\342\235\257 \033[?25h'
held=""
late=""
enters=0
while IFS= read -r line; do
  enters=$((enters + 1))
  if [ -n "$late" ]; then
    printf '%s\n' "$late" >> "$FAKE_SENT"
    late=""
  fi
  words="$held$line"
  held=""
  case "$FAKE_TAKES" in
    booting) if [ "$enters" -eq 1 ]; then held="$words"; continue; fi ;;
  esac
  [ -z "$words" ] && continue
  case "$FAKE_TAKES" in
    late|busy) late="$words" ;;
    *) printf '%s\n' "$words" >> "$FAKE_SENT" ;;
  esac
  printf '\342\235\257 \033[?25h'
done
"#;

    /// One restart's words through the pump's own seam at a fake program:
    /// every delivery in `words` registered as the window registers a
    /// continuation (at rest, beside a draft), the receipt fed back the way
    /// the hook road feeds it ([`crate::human_input::submitted`]), and what
    /// the program sent, in order.
    ///
    /// The deliveries are registered once the program has said its first
    /// word, and every clock here starts then: the window types a restart's
    /// words at a program long up (its fallback goes a receipt watch after
    /// the wake), while a loaded machine took up to 4.6 s to start the fake
    /// beside the suite, and past 6.5 s at load 28 — spending the readiness
    /// deadline ([`zerocode_pty::ready::TIMEOUT`]) before the program spoke
    /// (t-82959). And they run on the time the test could look ([`Looked`]),
    /// not on the wall's.
    #[cfg(unix)]
    fn restart_at(term: TermId, takes: Takes, words: &[&str]) -> Restart {
        use std::os::unix::fs::PermissionsExt;
        const WINDOW: Duration = Duration::from_millis(300);
        const PATIENCE: Duration = Duration::from_millis(1_500);
        const DEADLINE: Duration = Duration::from_secs(10);
        /// How long the fake may take to say its first word: spent in full
        /// only by a failure.
        const START_PATIENCE: Duration = Duration::from_secs(60);

        crate::human_input::forget_term(term);
        let root = tempfile::tempdir().expect("a fake agent's home");
        let program = root.path().join("claude");
        std::fs::write(&program, FAKE_AGENT).expect("the fake agent");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("an executable fake agent");
        let sent = root.path().join("sent");
        let mode = match takes {
            Takes::AtRest => "rest",
            Takes::Booting => "booting",
            Takes::Late => "late",
            Takes::Busy => "busy",
        };
        let later = WINDOW + WINDOW / 2;
        let env = vec![
            ("FAKE_TAKES".to_string(), mode.to_string()),
            ("FAKE_SENT".to_string(), sent.to_string_lossy().into_owned()),
        ];
        let mut pty = PtyLane::spawn(
            &program.to_string_lossy(),
            &[],
            Some(root.path()),
            &env,
            24,
            80,
        )
        .expect("spawn the fake agent");
        let spawned = Instant::now();
        while pty.pump().bytes == 0 {
            assert!(
                spawned.elapsed() < START_PATIENCE,
                "{takes:?}: the fake agent never spoke"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut looked = Looked::new();
        let start = looked.now;
        let mut bench = Bench::default();
        let mut first = words.iter();
        let delivery = |text: &str, now: Instant| {
            crate::cmd::terminal::prompt_delivery_for(
                text.to_string(),
                true,
                Some("claude"),
                crate::cmd::terminal::PromptReadiness::RestingBesideADraft,
                None,
                now,
            )
            .receipt_clock(WINDOW, PATIENCE)
        };
        let (settled, _outcome) = std::sync::mpsc::sync_channel(1);
        bench.deliveries.insert(
            term,
            delivery(first.next().expect("one word at least"), start),
        );
        bench.waiters.insert(term, settled);
        // Parked behind the first the way the window parks a second door's
        // words (`type_prompt_at_term`): the same resting signal and guard.
        for rest in first {
            let (completion, _receipt) = std::sync::mpsc::sync_channel(1);
            bench
                .queue
                .entry(term)
                .or_default()
                .push_back(QueuedPrompt {
                    text: (*rest).to_string(),
                    submit: true,
                    signal: ReadySignal::Rest(ready_signal_for(Some("claude")).marker()),
                    clearing: false,
                    guard: Guard::for_somebody_elses_line(None),
                    enter_again: None,
                    words: composer_words_for(Some("claude")),
                    parks: false,
                    hands_back: false,
                    completion,
                });
        }
        let mut heard = 0;
        let mut enters = 0;
        let mut settled_as = Vec::new();
        let mut turn_over = false;
        loop {
            let now = looked.tick();
            let pumped = pty.pump();
            let marker = bench.deliveries.get(&term).and_then(PromptDelivery::marker);
            let seen = {
                let grid = pty.terminal_mut().grid_mut();
                let drawn = grid.take_glyph_drawn(marker);
                Observed {
                    wrote: pumped.bytes > 0,
                    bracketed_paste: grid.bracketed_paste(),
                    cursor_shows: grid.cursor_shows(),
                    marker_written: drawn.anywhere,
                    marker_in_alt: drawn.in_alt_screen,
                    alt_screen: grid.alt_screen(),
                }
            };
            // The program's word about what it took — the hook road's.
            let count = std::fs::read_to_string(&sent).map_or(0, |text| text.lines().count());
            while heard < count {
                crate::human_input::submitted(term);
                heard += 1;
            }
            let line = Line {
                taken: Some(crate::human_input::taken(term)),
                busy: takes == Takes::Busy && heard < words.len(),
                ..line_facts_without_a_window(term)
            };
            if let Some(delivery) = bench.deliveries.get_mut(&term) {
                let wrote = turn(term, delivery, seen, line, now, |bytes| {
                    pty.write_input(bytes).is_ok()
                });
                enters += usize::from(wrote == Turned::Entered);
                if let Turned::Settled(outcome) = wrote {
                    settled_as.push(outcome);
                    settle(
                        term,
                        outcome,
                        &mut bench.deliveries,
                        &mut bench.waiters,
                        &mut bench.queue,
                        now,
                    );
                }
            } else if takes == Takes::Busy && !turn_over {
                // The turn ends: one empty line, the fake's next.
                turn_over = pty.write_input(b"\r").is_ok();
            } else if heard >= words.len() || now - start > PATIENCE * 2 + later {
                break;
            }
            assert!(
                now - start < DEADLINE,
                "{takes:?}: the restart never settled"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // Long enough for a second copy to show if one went.
        std::thread::sleep(later * 2);
        pty.kill().expect("stop the fake agent");
        crate::human_input::forget_term(term);
        let sent = std::fs::read_to_string(&sent)
            .unwrap_or_default()
            .lines()
            .map(|line| line.replace("\u{1b}[200~", "").replace("\u{1b}[201~", ""))
            .collect();
        Restart {
            sent,
            enters,
            settled_as,
        }
    }

    /// A restart's clock: the time its test could look (t-82959). A round
    /// counts as long as it took, up to [`Self::ROUND`]; one a loaded machine
    /// held the test back for longer — 11 s on the efficiency cores at load
    /// 23 — counts as that, so no deadline of the restart is spent while
    /// nobody could look, and each still runs out on a program that does not
    /// answer.
    #[cfg(unix)]
    struct Looked {
        /// When the last round began, on the wall.
        at: Instant,
        /// The restart's time at that round.
        now: Instant,
    }

    #[cfg(unix)]
    impl Looked {
        /// The longest round that counts in full: above the 200 to 490 ms
        /// a round takes on the efficiency cores beside the suite.
        const ROUND: Duration = Duration::from_millis(500);

        fn new() -> Self {
            let now = Instant::now();
            Self { at: now, now }
        }

        /// The restart's time at the round beginning now.
        fn tick(&mut self) -> Instant {
            let at = Instant::now();
            self.now += at.saturating_duration_since(self.at).min(Self::ROUND);
            self.at = at;
            self.now
        }
    }

    /// What one restart at a fake program came to: what it sent, how many
    /// Enters the pump wrote, and how each delivery settled.
    #[cfg(unix)]
    #[derive(Debug)]
    struct Restart {
        sent: Vec<String>,
        enters: usize,
        settled_as: Vec<DeliveryOutcome>,
    }

    #[cfg(unix)]
    impl Restart {
        /// Whether each of `deliveries` was called delivered, and none took
        /// more than its Enter and the one pressed again.
        fn kept_its_word(&self, deliveries: usize) -> bool {
            self.settled_as == vec![DeliveryOutcome::Delivered; deliveries]
                && (deliveries..=deliveries * 2).contains(&self.enters)
        }
    }

    /// t-14037, the acceptance count: each shape a restart meets, brought
    /// back three times — the words are sent, by the program itself, exactly
    /// once, and two deliveries queued on one pane each go once, in order.
    /// Before the receipt was the witness, a program still booting kept the
    /// words in its composer every time (0/3): the Enter was written and the
    /// delivery called it delivered.
    ///
    /// The five shapes run side by side, each on its own terminals, and each
    /// shape's three restarts one after another: fifteen restarts in a row
    /// were 62 s of a 413 s suite run, the last test standing (t-80605).
    #[cfg(unix)]
    #[test]
    fn a_restarts_words_are_sent_once_whatever_the_program_is_doing() {
        const RUNS: usize = 3;
        let words = "The window restarted and cut your last turn short.";
        let queued = ["the continuation", "You have 1 orchestration message."];
        let shapes: [(TermId, Takes, &[&str]); 5] = [
            (8_400, Takes::AtRest, &[words]),
            (8_410, Takes::Booting, &[words]),
            (8_420, Takes::Late, &[words]),
            (8_430, Takes::Busy, &[words]),
            (8_440, Takes::Booting, &queued),
        ];
        let [single @ .., two_queued] = std::thread::scope(|scope| {
            let shapes = shapes.map(|(base, takes, sent)| {
                scope.spawn(move || {
                    (0..RUNS)
                        .map(|run| restart_at(base + run as TermId, takes, sent))
                        .collect::<Vec<Restart>>()
                })
            });
            shapes.map(|shape| {
                shape
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
        });
        for ((_, takes, _), runs) in shapes.iter().zip(single) {
            let once = runs.iter().filter(|run| run.sent == [words]).count();
            assert_eq!(
                once, RUNS,
                "{takes:?}: the words were sent exactly once in {once}/{RUNS} restarts: {runs:?}"
            );
            assert!(
                runs.iter().all(|run| run.kept_its_word(1)),
                "{takes:?}: a delivery was pressed more than once again, or not called delivered: \
                 {runs:?}"
            );
        }
        let both = two_queued.iter().filter(|run| run.sent == queued).count();
        assert_eq!(
            both, RUNS,
            "two deliveries on one booting pane went each once, in order, in {both}/{RUNS}: \
             {two_queued:?}"
        );
        assert!(
            two_queued.iter().all(|run| run.kept_its_word(queued.len())),
            "a queued delivery was pressed more than once again, or not called delivered: \
             {two_queued:?}"
        );
    }
}

#[cfg(test)]
mod measured_composer;
