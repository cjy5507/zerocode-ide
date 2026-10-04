//! What a refused request is answered with, and in which order.
//!
//! Compaction answers a prompt over the context window: it summarizes text. A
//! body over the provider's BYTE ceiling is a different wall — a screenshot is
//! ~1 MB for ~1,600 tokens, a Computer Use turn re-sends all of them every
//! step, and the tail compaction keeps verbatim can hold 25 of them on its own
//! (a 40k-token tail at 1,600 tokens a picture). A summary cannot shrink that,
//! so the turn used to end on the 413 with 80% of its window free.
//!
//! A refusal for bytes is therefore answered, cheapest first:
//!
//! 1. leave all but the newest [`KEEP_NEWEST_PICTURES`] pictures out of the
//!    request and send it again — instant, free, nothing stored is rewritten;
//! 2. leave all but the newest one out;
//! 3. compact (the text may be the wall after all);
//! 4. end the turn, saying which picture is the newest and how big the rest of
//!    the request is.
//!
//! A refusal for tokens skips the first two rungs and keeps its one compaction,
//! exactly as before. A rung that would change nothing — the refused request
//! already carried no more pictures than it keeps — is skipped, so a
//! picture-free session recovers as it always did and no rung re-sends a body
//! the provider has just refused.

use api::ProviderErrorClass;

use super::{ApiClient, ConversationRuntime, RuntimeError, ToolExecutor};
use crate::picture_budget::{count_pictures, newest_picture_note, KEEP_NEWEST_PICTURES};
use crate::{plan_picture_shed, WireTarget};

/// How many of the newest pictures each rung of the answer keeps, in the order
/// the rungs are tried. The first is the budget's own floor; the last is what
/// is left when only the picture the model is acting on remains.
const PICTURE_CAP_RUNGS: [usize; 2] = [KEEP_NEWEST_PICTURES, 1];

/// Which ceiling the provider refused the request for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The prompt is over the model's context window: only compaction helps.
    Tokens,
    /// The body is over the provider's byte ceiling (a 413, `request_too_large`,
    /// a gateway's buffer limit): pictures are the first thing to cut.
    Bytes,
}

impl Refusal {
    pub(super) fn of(error: &RuntimeError) -> Self {
        let lower = error.to_string().to_ascii_lowercase();
        if core_types::retry_signal::is_request_too_large_text(&lower) {
            Self::Bytes
        } else {
            Self::Tokens
        }
    }
}

/// The step a refusal is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverflowStep {
    /// Send the request again carrying only the newest `keep` pictures.
    LeaveOutPictures { keep: usize },
    /// Compact the session, then send again.
    Compact,
    /// Nothing left to try: the turn ends on the refusal.
    GiveUp,
}

/// Where one turn stands in the ladder above. Per turn: a new turn starts on
/// the byte budget again, so the pictures a refusal left out come back as soon
/// as there is room.
#[derive(Debug, Default)]
pub(super) struct OverflowRecovery {
    picture_cap: Option<usize>,
    compacted: bool,
}

impl OverflowRecovery {
    /// The cap the next request carries, once a refusal asked for one.
    pub(super) const fn picture_cap(&self) -> Option<usize> {
        self.picture_cap
    }

    /// The next step after a refusal, given how many pictures the refused request
    /// carried. Each call spends a rung: the same refusal never gets the same
    /// answer twice, so the ladder ends.
    pub(super) fn next(&mut self, refusal: Refusal, carried: usize) -> OverflowStep {
        if refusal == Refusal::Bytes {
            if let Some(keep) = Self::next_picture_cap(carried) {
                self.picture_cap = Some(keep);
                return OverflowStep::LeaveOutPictures { keep };
            }
        }
        if self.compacted {
            return OverflowStep::GiveUp;
        }
        self.compacted = true;
        OverflowStep::Compact
    }

    /// The first rung that keeps fewer pictures than the refused request carried.
    fn next_picture_cap(carried: usize) -> Option<usize> {
        PICTURE_CAP_RUNGS.into_iter().find(|&keep| keep < carried)
    }
}

/// The row a rung puts in front of the person: the request was refused and what
/// is being done about it. English, like the other turn notices.
pub(super) fn picture_overflow_notice(keep: usize) -> String {
    let pictures = if keep == 1 { "picture" } else { "pictures" };
    format!(
        "The provider refused the request as too large; leaving all but the newest {keep} \
         {pictures} out of it and trying again."
    )
}

impl<C: ApiClient, T: ToolExecutor> ConversationRuntime<C, T> {
    /// The answer to a request the provider refused as over its context
    /// ceiling: spends the next rung of this turn's ladder.
    pub(super) fn next_overflow_step(&mut self, error: &RuntimeError) -> OverflowStep {
        let carried = self.pictures_carried();
        self.overflow_recovery.next(Refusal::of(error), carried)
    }

    /// How many pictures the request that was just refused carried: the stored
    /// pictures less the ones its wire form left out for this model and this
    /// turn's cap — the same decision the lowering made, asked of the same
    /// function. A rung is only worth a round trip if it changes that number.
    fn pictures_carried(&self) -> usize {
        let messages = &self.session.messages;
        let target = WireTarget::for_model(self.effective_request_model().unwrap_or_default())
            .with_picture_cap(self.overflow_recovery.picture_cap());
        count_pictures(messages).saturating_sub(plan_picture_shed(messages, target).left_out)
    }

    /// A refusal that ends the turn, told with the facts of the pictures when
    /// the body was the wall and the history holds one: which is the newest,
    /// how big it is, and what the rest of the request weighs. Any other error
    /// passes through unchanged.
    pub(super) fn explain_overflow(&self, error: RuntimeError) -> RuntimeError {
        if error.provider_error_class() != Some(ProviderErrorClass::ContextOverflow)
            || Refusal::of(&error) != Refusal::Bytes
        {
            return error;
        }
        match newest_picture_note(&self.session.messages) {
            Some(note) => RuntimeError::with_provider_error_class(
                format!("{error}\n\n  {note}"),
                ProviderErrorClass::ContextOverflow,
            ),
            None => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_byte_refusal_cuts_pictures_before_it_compacts() {
        let mut recovery = OverflowRecovery::default();
        assert_eq!(
            recovery.next(Refusal::Bytes, 30),
            OverflowStep::LeaveOutPictures {
                keep: KEEP_NEWEST_PICTURES
            }
        );
        assert_eq!(recovery.picture_cap(), Some(KEEP_NEWEST_PICTURES));
        // The refused request now carries two: keep the newest one.
        assert_eq!(
            recovery.next(Refusal::Bytes, KEEP_NEWEST_PICTURES),
            OverflowStep::LeaveOutPictures { keep: 1 }
        );
        assert_eq!(recovery.picture_cap(), Some(1));
        // One picture: nothing is left to cut, so compaction — once.
        assert_eq!(recovery.next(Refusal::Bytes, 1), OverflowStep::Compact);
        assert_eq!(recovery.next(Refusal::Bytes, 1), OverflowStep::GiveUp);
    }

    #[test]
    fn a_token_refusal_keeps_its_one_compaction() {
        let mut recovery = OverflowRecovery::default();
        assert_eq!(recovery.next(Refusal::Tokens, 30), OverflowStep::Compact);
        assert_eq!(recovery.next(Refusal::Tokens, 30), OverflowStep::GiveUp);
        assert_eq!(recovery.picture_cap(), None, "tokens never ask for a cap");
    }

    #[test]
    fn a_rung_that_would_change_nothing_is_skipped() {
        // No picture on the wire at all: straight to compaction, as before.
        let mut recovery = OverflowRecovery::default();
        assert_eq!(recovery.next(Refusal::Bytes, 0), OverflowStep::Compact);
        // Exactly two on the wire (the budget already left the rest out): keeping
        // two changes nothing, so the first rung is skipped and one is kept.
        let mut recovery = OverflowRecovery::default();
        assert_eq!(
            recovery.next(Refusal::Bytes, KEEP_NEWEST_PICTURES),
            OverflowStep::LeaveOutPictures { keep: 1 }
        );
        assert_eq!(recovery.next(Refusal::Bytes, 1), OverflowStep::Compact);
        // One picture: nothing to cut.
        let mut recovery = OverflowRecovery::default();
        assert_eq!(recovery.next(Refusal::Bytes, 1), OverflowStep::Compact);
    }

    #[test]
    fn the_refusal_kind_comes_from_the_words_the_provider_used() {
        let classed = |text: &str| {
            RuntimeError::with_provider_error_class(text, ProviderErrorClass::ContextOverflow)
        };
        for bytes in [
            "api returned 413 Payload Too Large (request_too_large): Request exceeds the maximum size",
            "api returned 507 Insufficient Storage: exceeded request buffer limit while retrying upstream",
            "Request Entity Too Large",
        ] {
            assert_eq!(Refusal::of(&classed(bytes)), Refusal::Bytes, "{bytes}");
        }
        for tokens in [
            "prompt is too long: 211352 tokens > 200000 maximum",
            "context length exceeded",
        ] {
            assert_eq!(Refusal::of(&classed(tokens)), Refusal::Tokens, "{tokens}");
        }
    }

    #[test]
    fn the_notice_counts_in_the_right_grammar() {
        assert!(picture_overflow_notice(2).contains("newest 2 pictures"));
        assert!(picture_overflow_notice(1).contains("newest 1 picture out"));
    }
}
