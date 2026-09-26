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

/// What the window knows about a pane's line, gathered for one round.
///
/// Permission and launch maps are read before the terminal lock. The hand
/// fields are refreshed under that lock by `refresh_hand` before a write.
pub(super) fn line_facts(state: &AppState, term: TermId) -> Line {
    Line {
        parked: pane_is_parked(state, term),
        launch: crate::cmd::terminal::launch_of(state, term),
        ..Line::default()
    }
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
pub(super) fn human_write<E>(
    term: TermId,
    bytes: &[u8],
    write: impl FnOnce() -> Result<(), E>,
) -> Result<(), E> {
    write()?;
    crate::human_input::typed(term);
    if let Ok(text) = std::str::from_utf8(bytes)
        && zerocode_core::ask::is_potential_submit_input(text)
    {
        crate::human_input::entered(term);
    }
    Ok(())
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
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Turned {
    /// Nothing yet; keep pumping.
    Waiting,
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
    match delivery.poll_line(seen, line, now) {
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
/// copy of a line that is on screen.
pub(super) fn words_to_hand_back(
    delivery: &PromptDelivery,
    outcome: DeliveryOutcome,
) -> Option<String> {
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

/// What the window hears about a settled delivery (`term:prompt`) — the
/// pump's settle and a refused zo launch both say it here, so the event has
/// one shape whoever settled it. A withheld write is named by the guard's
/// token: the window words it in the person's language from its own table
/// (t-10159), and the English sentence stays the receipt's and the black
/// box's.
pub(super) fn settled_event(
    term: TermId,
    outcome: DeliveryOutcome,
    text: Option<String>,
) -> PromptSettled {
    PromptSettled {
        term,
        delivered: outcome == DeliveryOutcome::Delivered,
        pasted: outcome.pasted(),
        why: withheld(outcome).map(Refusal::token),
        text,
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
    if let Some(row) = queue.get_mut(&term) {
        if let Some(next) = row.pop_front() {
            waiters.insert(term, next.completion);
            deliveries.insert(
                term,
                PromptDelivery::new(next.text, next.submit, next.signal, now)
                    .clearing(next.clearing)
                    .guarded(next.guard),
            );
        }
        if row.is_empty() {
            queue.remove(&term);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_pty::ready::{Guard, Refusal, SUBMIT_GAP};

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
                completion,
            },
            receipt,
        )
    }

    /// One shell's share of the pump's three maps and the bytes its pane
    /// received — what a round below turns.
    #[derive(Default)]
    struct Bench {
        deliveries: HashMap<TermId, PromptDelivery>,
        waiters: HashMap<TermId, std::sync::mpsc::SyncSender<DeliveryOutcome>>,
        queue: HashMap<TermId, VecDeque<QueuedPrompt>>,
        wrote: Vec<u8>,
    }

    impl Bench {
        /// One pump round for one shell, exactly as the pump makes it: the
        /// delivery turned against the line facts, and a settled one retired
        /// with its queue swapped in.
        fn round(&mut self, term: TermId, seen: Observed, line: Line, now: Instant) -> Turned {
            let Some(delivery) = self.deliveries.get_mut(&term) else {
                return Turned::Waiting;
            };
            let wrote = &mut self.wrote;
            let turned = turn(term, delivery, seen, line, now, |bytes| {
                wrote.extend_from_slice(bytes);
                true
            });
            if let Turned::Settled(outcome) = turned {
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
    /// program: the words come back rather than being typed at it.
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
        .guarded(Guard::for_its_own_line(Some(1)));
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
    /// of its own (t-10159). A delivery that landed or ran out of time names
    /// no refusal at all.
    #[test]
    fn the_window_hears_a_withheld_write_by_its_token() {
        const TERM: TermId = 8_314;
        let pointer = "\nYou have 2 orchestration messages. Run `zerocode-orc check`.\n";
        let refused = serde_json::to_value(settled_event(
            TERM,
            DeliveryOutcome::Refused(Refusal::HoldsADraft),
            Some(pointer.to_string()),
        ))
        .expect("the event serializes");
        assert_eq!(
            refused,
            serde_json::json!({
                "term": TERM,
                "delivered": false,
                "pasted": false,
                "why": "holds_a_draft",
                "text": pointer,
            })
        );
        let left = serde_json::to_value(settled_event(
            TERM,
            DeliveryOutcome::Unsubmitted(Refusal::HandReached),
            None,
        ))
        .expect("the event serializes");
        assert_eq!(left["pasted"], true);
        assert_eq!(left["why"], "hand_reached");
        for refusal in Refusal::ALL {
            let said =
                serde_json::to_value(settled_event(TERM, DeliveryOutcome::Refused(refusal), None))
                    .expect("the event serializes");
            assert_eq!(said["why"], refusal.token(), "{refusal:?}");
            assert_ne!(said["why"], refusal.says(), "{refusal:?}");
        }
        for outcome in [DeliveryOutcome::Delivered, DeliveryOutcome::TimedOut] {
            let quiet =
                serde_json::to_value(settled_event(TERM, outcome, None)).expect("serializes");
            assert_eq!(quiet["why"], serde_json::Value::Null, "{outcome:?}");
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
}
