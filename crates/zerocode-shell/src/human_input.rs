//! Whether a pane is holding words a PERSON typed and has not sent.
//!
//! # Why a door needs this
//!
//! A composer is one line, and more than one hand reaches it. When something
//! types at a pane on somebody else's behalf — an agent driving
//! `zerocode-ssh send`, the orchestration pointer — refusing to send clear
//! keys protects a draft from being ERASED and does nothing at all about the
//! other half: text pasted onto a line that already has words on it is
//! appended to them, and the Enter that follows submits somebody's
//! half-written thought along with it.
//!
//! Not clearing is not preservation. The only safe answer is evidence, and
//! this is the evidence the window actually holds.
//!
//! # What is observed
//!
//! Three facts, from roads that already exist:
//!
//! · A person's hand — `term_key` and `term_paste` are the only doors a
//!   keystroke of theirs comes through; every programmatic write takes
//!   another. Each one marks this pane as holding something unsent.
//!
//! · An Enter reaching the pane — the person's own, through `term_key` in
//!   any of its spellings, or one the pump wrote for a delivery. Either is a
//!   GESTURE: whatever was on the line at that moment went in, or is about
//!   to. The gesture is remembered with the hand count it closed over.
//!
//! · The provider's word that a prompt went in — its own prompt event
//!   (`PaneHookReport::prompt`), the same signal a launch waits on to know
//!   its briefing was consumed. That word answers the LAST GESTURE, not the
//!   line as it is now: a person who typed again between their Enter and the
//!   provider's report of it still holds a draft, and the report must not
//!   clear it. Correlated by the hand count, which is the one thing both
//!   moments share.
//!
//! Between a hand and the provider's word about the gesture that took it,
//! the pane is presumed to be holding a draft, and a door that cannot account
//! for what is on the line does not write on it.
//!
//! # What this deliberately is not
//!
//! It is not a reading of the composer. No window can parse every TUI's input
//! buffer, and one that tried would be wrong quietly. This says only "somebody
//! typed here and nothing has reported taking it", which is the smaller claim
//! and the one that can be made honestly.
//!
//! It fails CLOSED, three ways. A provider that reports no prompt event
//! leaves the mark standing, so a pane a person once typed into stays refused
//! rather than written into on a guess. A prompt event with no gesture behind
//! it — a submission through a door this tracker never saw — clears nothing,
//! because nothing says which words it took. And the tracker's ceiling never
//! evicts a pane that still holds a draft: forgetting one would answer "no
//! draft" about a line somebody is mid-sentence on, which is the one lie this
//! module exists to refuse. Only the pane's own end (`forget_term`) retires a
//! standing draft.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The most SETTLED panes this tracker remembers a hand in.
///
/// A window's terminals come and go, and `forget_term` is the ordinary road
/// out; this bound is for the one that never runs. Oldest settled hand first,
/// because an old settled mark is the one nothing will ever ask about again —
/// and never a pending one, whatever its age, for the reason the module
/// documentation gives.
const TRACKED_PANES: usize = 256;

/// One pane's unsent words.
#[derive(Debug, Clone, Copy)]
struct Typed {
    /// Bumped by every hand, and never rolled back. A door compares it across
    /// its own paste to see whether somebody reached the line in between —
    /// which has to stay true even when the line was then SENT, because a
    /// person's Enter landing on our half-pasted words is exactly the case
    /// where we must not send an Enter of our own.
    generation: u64,
    /// Whether the words that hand left are still sitting there. A provider's
    /// prompt event answering the last gesture clears this; the generation
    /// above stays.
    pending: bool,
    /// The hand count an Enter last closed over — a person's own key, or a
    /// delivery's — and `None` once a prompt event has answered it or nothing
    /// has been entered since the last hand.
    entered_at: Option<u64>,
    at: Instant,
}

#[derive(Default)]
struct Hands {
    typed: Mutex<HashMap<u32, Typed>>,
    next: Mutex<u64>,
}

fn hands() -> &'static Hands {
    static HANDS: OnceLock<Hands> = OnceLock::new();
    HANDS.get_or_init(Hands::default)
}

fn next_generation() -> u64 {
    let mut next = hands()
        .next
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *next = next.wrapping_add(1);
    *next
}

/// A person's hand reached this pane's line.
pub(crate) fn typed(term: u32) {
    let generation = next_generation();
    let mut typed = hands()
        .typed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if typed.len() >= TRACKED_PANES && !typed.contains_key(&term) {
        // Only a SETTLED mark may make room. A pending one is somebody's
        // unsent words, and forgetting it would answer "nothing here" about
        // a line they are still writing on.
        let oldest_settled = typed
            .iter()
            .filter(|(_, held)| !held.pending)
            .min_by_key(|(_, held)| held.at)
            .map(|(term, _)| *term);
        if let Some(oldest) = oldest_settled {
            typed.remove(&oldest);
        }
    }
    typed.insert(
        term,
        Typed {
            generation,
            pending: true,
            // A new hand after an Enter is a new draft; the Enter before it
            // is no longer what the next prompt event answers.
            entered_at: None,
            at: Instant::now(),
        },
    );
}

/// An Enter reached this pane's line — a person's own, or one the pump wrote
/// for a delivery.
///
/// The gesture that takes whatever is on the line. Remembered with the hand
/// count it closed over, so the provider's report of a prompt going in can be
/// matched to THIS Enter rather than to whatever the person has typed since.
/// A pane nobody ever typed into has nothing to remember: there is no draft
/// for the report to clear.
pub(crate) fn entered(term: u32) {
    if let Some(held) = hands()
        .typed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_mut(&term)
    {
        held.entered_at = Some(held.generation);
    }
}

/// The provider reports a prompt went in at this pane.
///
/// Answers the last Enter this tracker saw, and only when no hand has reached
/// the line since: then the words that Enter closed over are gone and the
/// mark clears. A hand that came after the Enter is a NEW draft the report
/// says nothing about, so the mark stands and only the Enter is retired. A
/// report with no Enter behind it clears nothing — see the module
/// documentation on failing closed.
///
/// The generation stays either way. "Nothing is on the line" and "nobody has
/// touched the line" are different facts, and a door mid-paste needs the
/// second one.
pub(crate) fn submitted(term: u32) {
    if let Some(held) = hands()
        .typed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_mut(&term)
    {
        match held.entered_at.take() {
            Some(closed_over) if closed_over == held.generation => held.pending = false,
            // A newer hand: the report answers an older Enter, and the words
            // typed since are still somebody's draft.
            Some(_) | None => {}
        }
    }
}

/// Every road out of a terminal reaches this. The pane is gone; so is its
/// whole history.
pub(crate) fn forget_term(term: u32) {
    hands()
        .typed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&term);
}

/// Whether this pane is presumed to be holding words nobody has sent.
///
/// The doors read [`line_of`], which answers this and the hand count in one
/// look; this is the tests' shorter question.
#[cfg(test)]
pub(crate) fn holds_a_draft(term: u32) -> bool {
    hands()
        .typed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&term)
        .is_some_and(|held| held.pending)
}

/// How many times a hand has reached this line, ever.
///
/// For a caller that has to notice a hand arriving BETWEEN two of its own
/// writes — including one whose line was then submitted, which
/// [`holds_a_draft`] can no longer see and which is the worst case of all: a
/// person's Enter landing on our half-pasted words. `None` is a line nobody
/// has touched.
#[cfg(test)]
pub(crate) fn generation(term: u32) -> Option<u64> {
    hands()
        .typed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&term)
        .map(|held| held.generation)
}

/// What the window knows about this pane's line, in the delivery's words —
/// the two facts a guarded write reads at the moment it is due.
pub(crate) fn line_of(term: u32) -> (bool, Option<u64>) {
    hands()
        .typed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&term)
        .map_or((false, None), |held| (held.pending, Some(held.generation)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hand_marks_the_line_and_the_providers_word_about_its_enter_clears_it() {
        const TERM: u32 = 7_701;
        forget_term(TERM);
        assert!(!holds_a_draft(TERM));
        assert_eq!(generation(TERM), None);
        assert_eq!(line_of(TERM), (false, None));

        typed(TERM);
        assert!(holds_a_draft(TERM));
        let first = generation(TERM).expect("a mark");

        // A second hand is a different generation: a door that pasted between
        // the two has to be able to see that somebody reached the line.
        typed(TERM);
        assert_ne!(generation(TERM), Some(first));

        // The person presses Enter and the provider reports the prompt going
        // in: the line is empty — but the hand that wrote it is still on the
        // record. A door that pasted before that Enter must not now send an
        // Enter of its own.
        let second = generation(TERM).expect("a mark");
        entered(TERM);
        assert!(
            holds_a_draft(TERM),
            "an Enter alone was taken as the provider's word"
        );
        submitted(TERM);
        assert!(!holds_a_draft(TERM));
        assert_eq!(
            generation(TERM),
            Some(second),
            "a submitted line erased the evidence that a hand had been there"
        );
        assert_eq!(line_of(TERM), (false, Some(second)));

        // The pane goes: so does its whole history.
        forget_term(TERM);
        assert_eq!(generation(TERM), None);
    }

    /// The coordinator's review, item 3: a delayed prompt event must not
    /// clear a draft typed AFTER the Enter it reports.
    ///
    /// The person sends a line, starts the next one, and only then does the
    /// provider's hook about the first arrive. The hook answers the Enter it
    /// belongs to; the new words stay a draft.
    #[test]
    fn a_delayed_prompt_event_does_not_clear_a_newer_draft() {
        const TERM: u32 = 7_704;
        forget_term(TERM);

        typed(TERM);
        entered(TERM);
        // The next line begins before the provider says anything.
        typed(TERM);
        let newer = generation(TERM).expect("the newer hand");

        submitted(TERM);
        assert!(
            holds_a_draft(TERM),
            "a delayed prompt event cleared words typed after the Enter it reported"
        );
        assert_eq!(generation(TERM), Some(newer));

        // The Enter that report answered is spent: a second report with no
        // new Enter behind it clears nothing either.
        submitted(TERM);
        assert!(holds_a_draft(TERM));

        // Their next Enter, reported, is what clears the newer draft.
        entered(TERM);
        submitted(TERM);
        assert!(!holds_a_draft(TERM));
        forget_term(TERM);
    }

    /// A prompt event with no Enter behind it clears nothing. The tracker
    /// does not know which words it took, so it says nothing was taken.
    #[test]
    fn a_prompt_event_with_no_gesture_behind_it_fails_closed() {
        const TERM: u32 = 7_705;
        forget_term(TERM);
        typed(TERM);
        submitted(TERM);
        assert!(
            holds_a_draft(TERM),
            "a report nothing entered for was taken as the draft going in"
        );
        // A delivery's own Enter is a gesture too: the pump reports it, and
        // the provider's word then answers it.
        entered(TERM);
        submitted(TERM);
        assert!(!holds_a_draft(TERM));
        // An Enter at a pane nobody typed into remembers nothing — there is
        // no draft for a report to clear, and none is invented.
        forget_term(TERM);
        entered(TERM);
        assert_eq!(line_of(TERM), (false, None));
        submitted(TERM);
        assert_eq!(line_of(TERM), (false, None));
    }

    /// One pane's hand is not another's.
    #[test]
    fn the_mark_belongs_to_one_pane() {
        const ONE: u32 = 7_702;
        const OTHER: u32 = 7_703;
        forget_term(ONE);
        forget_term(OTHER);

        typed(ONE);
        assert!(holds_a_draft(ONE));
        assert!(!holds_a_draft(OTHER));
        entered(ONE);
        submitted(ONE);
        assert!(!holds_a_draft(ONE));
    }

    /// A window that never forgets a terminal still does not grow forever —
    /// and what it drops to stay bounded is never a standing draft.
    #[test]
    fn the_tracker_is_bounded_and_never_evicts_a_standing_draft() {
        const BASE: u32 = 900_000;
        // Fill the tracker with SETTLED marks, past its bound.
        for term in 0..(TRACKED_PANES as u32 + 8) {
            typed(BASE + term);
            entered(BASE + term);
            submitted(BASE + term);
        }
        let counted = || {
            hands()
                .typed
                .lock()
                .expect("the tracker")
                .keys()
                .filter(|term| **term >= BASE)
                .count()
        };
        assert!(
            counted() <= TRACKED_PANES,
            "the tracker kept {} panes past its bound",
            counted()
        );

        // One pane with a draft standing, then a flood of new hands: every
        // eviction takes a settled mark and the draft is still there.
        const DRAFTING: u32 = BASE + 100_000;
        typed(DRAFTING);
        for term in 0..(TRACKED_PANES as u32 * 2) {
            typed(BASE + 200_000 + term);
            entered(BASE + 200_000 + term);
            submitted(BASE + 200_000 + term);
        }
        assert!(
            holds_a_draft(DRAFTING),
            "the ceiling forgot a pane somebody is still writing on"
        );

        for term in 0..(TRACKED_PANES as u32 + 8) {
            forget_term(BASE + term);
        }
        for term in 0..(TRACKED_PANES as u32 * 2) {
            forget_term(BASE + 200_000 + term);
        }
        forget_term(DRAFTING);
    }
}
