//! What a refused request is answered with, and in which order.
//!
//! Compaction answers a prompt over the context window: it summarizes text. A
//! body over the provider's BYTE ceiling is a different wall — a screenshot is
//! ~1 MB for ~1,600 tokens, a Computer Use turn re-sends all of them every
//! step, and the tail compaction keeps verbatim can hold 25 of them on its own
//! (a 40k-token tail at 1,600 tokens a picture). A summary cannot shrink that,
//! so the turn used to end on the 413 with 80% of its window free.
//!
//! A refusal for bytes is therefore answered, cheapest first — instant, free,
//! and nothing stored is rewritten until the third rung:
//!
//! 1. leave the oldest pictures out by the picture budget's own rule
//!    (`picture_budget`), on the ceiling this refusal taught: the one the
//!    client recorded for the endpoint (`api::max_request_bytes_for_model`
//!    reads it back), else three quarters of the refused body;
//! 2. the same again while more than the newest two ride, then keep one;
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
use crate::picture_budget::{count_pictures, newest_picture_note, pictures_after_refusal};
use crate::{plan_picture_shed, WireTarget};

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

/// How the next send of a request refused for its size carries fewer pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PictureRetry {
    /// The client recorded a ceiling from this refusal, and the budget it sets
    /// already leaves all but `keep` out. Every later request plans with that
    /// budget too, so nothing is capped: the rest of the turn moves in the
    /// budget's whole steps, and the prompt cache with it, instead of sliding
    /// a window over the newest pictures.
    Budget { keep: usize },
    /// Nothing was recorded: carry at most the newest `keep` for the rest of
    /// the turn.
    Cap { keep: usize },
}

impl PictureRetry {
    const fn keep(self) -> usize {
        match self {
            Self::Budget { keep } | Self::Cap { keep } => keep,
        }
    }
}

/// Where one turn stands in the ladder above. Per turn: a new turn starts on
/// the byte budget again, so the pictures a refusal left out come back as soon
/// as there is room.
#[derive(Debug, Default)]
pub(super) struct OverflowRecovery {
    picture_cap: Option<usize>,
    compacted: bool,
    /// How many pictures the request in flight carries, counted before it was
    /// sent: a 413 can teach the budget a smaller ceiling while the request is
    /// out, and a count taken afterwards would no longer be that request's.
    in_flight: Option<usize>,
}

impl OverflowRecovery {
    /// The cap the next request carries, once a refusal asked for one.
    pub(super) const fn picture_cap(&self) -> Option<usize> {
        self.picture_cap
    }

    /// Spend the next rung. `retry` is the picture answer the caller worked out
    /// for a refusal for bytes; it always carries fewer pictures than the
    /// refused request did, so the picture rungs end, then one compaction.
    pub(super) fn next(&mut self, refusal: Refusal, retry: Option<PictureRetry>) -> OverflowStep {
        if refusal == Refusal::Bytes {
            if let Some(retry) = retry {
                if let PictureRetry::Cap { keep } = retry {
                    self.picture_cap = Some(keep);
                }
                return OverflowStep::LeaveOutPictures { keep: retry.keep() };
            }
        }
        if self.compacted {
            return OverflowStep::GiveUp;
        }
        self.compacted = true;
        OverflowStep::Compact
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
        let refusal = Refusal::of(error);
        let carried = self
            .overflow_recovery
            .in_flight
            .take()
            .unwrap_or_else(|| self.pictures_carried());
        let retry = if refusal == Refusal::Bytes {
            self.picture_retry(carried)
        } else {
            None
        };
        self.overflow_recovery
            .next(refusal, retry.filter(|retry| retry.keep() < carried))
    }

    /// The pictures the next send of a request refused for its size carries.
    /// A client that keeps a record of the endpoint wrote the refused size
    /// before the error reached this loop, so the budget already plans with the
    /// ceiling it teaches — the same one the next session will start on; the
    /// count it leaves is the answer, and nothing is capped. Otherwise the same
    /// rule runs on three quarters of the refused body, held as a cap for the
    /// rest of the turn.
    fn picture_retry(&self, carried: usize) -> Option<PictureRetry> {
        let planned = self.pictures_carried();
        if planned < carried {
            return Some(PictureRetry::Budget { keep: planned });
        }
        pictures_after_refusal(&self.session.messages, carried)
            .map(|keep| PictureRetry::Cap { keep })
    }

    /// Count the pictures of the request about to be sent, before it goes:
    /// [`OverflowRecovery::in_flight`].
    pub(super) fn note_request_pictures(&mut self) {
        self.overflow_recovery.in_flight = Some(self.pictures_carried());
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
    fn a_byte_refusal_spends_the_picture_rungs_before_it_compacts() {
        let mut recovery = OverflowRecovery::default();
        for keep in [3, 2, 1] {
            assert_eq!(
                recovery.next(Refusal::Bytes, Some(PictureRetry::Cap { keep })),
                OverflowStep::LeaveOutPictures { keep }
            );
            assert_eq!(recovery.picture_cap(), Some(keep));
        }
        assert_eq!(recovery.next(Refusal::Bytes, None), OverflowStep::Compact);
        assert_eq!(recovery.next(Refusal::Bytes, None), OverflowStep::GiveUp);
    }

    #[test]
    fn a_ceiling_the_client_recorded_leaves_the_rest_of_the_turn_on_the_budget() {
        // The recorded ceiling's budget already carries five: no cap holds the
        // rest of the turn to the newest five, so its later requests move in
        // the budget's whole steps.
        let mut recovery = OverflowRecovery::default();
        assert_eq!(
            recovery.next(Refusal::Bytes, Some(PictureRetry::Budget { keep: 5 })),
            OverflowStep::LeaveOutPictures { keep: 5 }
        );
        assert_eq!(recovery.picture_cap(), None);
        // A later refusal nothing was recorded for caps from there on.
        assert_eq!(
            recovery.next(Refusal::Bytes, Some(PictureRetry::Cap { keep: 2 })),
            OverflowStep::LeaveOutPictures { keep: 2 }
        );
        assert_eq!(recovery.picture_cap(), Some(2));
    }

    #[test]
    fn a_token_refusal_keeps_its_one_compaction() {
        let mut recovery = OverflowRecovery::default();
        let retry = Some(PictureRetry::Cap { keep: 15 });
        assert_eq!(recovery.next(Refusal::Tokens, retry), OverflowStep::Compact);
        assert_eq!(recovery.next(Refusal::Tokens, retry), OverflowStep::GiveUp);
        assert_eq!(recovery.picture_cap(), None, "tokens never ask for a cap");
    }

    #[test]
    fn a_rung_that_would_change_nothing_is_skipped() {
        let mut recovery = OverflowRecovery::default();
        assert_eq!(recovery.next(Refusal::Bytes, None), OverflowStep::Compact);
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
