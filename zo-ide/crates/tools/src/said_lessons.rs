//! The lessons a session's own words teach, asked of a model: the host half of
//! `decision_core::said_lessons`.
//!
//! The core says what a model is shown of a session, what it is asked and how
//! its answer is read. This module reads the session, dials the fast model of
//! the provider the session ran on and hands the reply to that reading. It
//! writes nothing: what becomes of a lesson taken is its caller's.
//!
//! What leaves the machine is the digest and nothing else — the beginning of
//! each thing the person said and the end of what the assistant had said
//! before it, within `DigestLimits` — and it goes to the provider the session
//! already sent those words to.

use std::time::Duration;

use decision_core::said_lessons::{self, Digest, DigestLimits, Pending, ReadReply};
use runtime::ConversationMessage;

use crate::fast_model::FastModel;

/// The longest a question a person asked for may take on the wall clock
/// (`zo dream-lessons preview`): they are waiting on it, and a model that is
/// not answering by now will not.
pub const ASK_WITHIN: Duration = Duration::from_secs(45);

/// The longest a question an idle pass asks may take. Nobody waits on it and
/// the pass comes after it: a slow provider costs the lessons, not the pass.
pub const PASS_ASK_WITHIN: Duration = Duration::from_secs(20);

/// Room for the answer: three lessons of one line each, their quotes, and the
/// JSON around them.
pub const ANSWER_MAX_TOKENS: u32 = 1_200;

/// What a model is shown of the session `messages` are: what the person said,
/// each with the end of what the assistant had said before it.
#[must_use]
pub fn digest_of(messages: &[ConversationMessage]) -> Digest {
    Digest::of(&runtime::patch_review::said_with_what_came_before(messages), DigestLimits::default())
}

/// The model the session `messages` are ran on: its newest answer's. `None`
/// for a session no model answered in.
#[must_use]
pub fn model_of(messages: &[ConversationMessage]) -> Option<&str> {
    messages
        .iter()
        .rev()
        .find_map(|message| message.model.as_deref().map(str::trim).filter(|model| !model.is_empty()))
}

/// One question asked and its answer read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// The model that answered: the fast model of the session's provider.
    pub model: String,
    /// The lessons taken, and how many the reply offered that were not.
    pub read: ReadReply,
}

/// Ask the fast model of `session_model`'s provider what `digest` teaches,
/// beside the `pending` candidates earlier sessions left, and read the reply.
/// Only a fast model is asked: where the provider has none, nothing is sent —
/// the person agreed to their provider's fast model, not to the session's own.
///
/// # Errors
/// In words for a log: a provider with no fast model, no client for the
/// model, a request that failed or outlasted `within`, or a reply with no
/// answer of the asked shape.
pub fn ask(digest: &Digest, pending: &[Pending], session_model: &str, within: Duration) -> Result<Asked, String> {
    let fast = FastModel::for_fast_tier(Some(session_model))?;
    let reply = fast.reply(
        &[said_lessons::INSTRUCTIONS.to_string()],
        &said_lessons::request(pending, digest),
        ANSWER_MAX_TOKENS,
        within,
    )?;
    let read = said_lessons::read_reply(&reply, digest, pending)
        .map_err(|_| "the reply held no answer of the asked shape".to_string())?;
    Ok(Asked {
        model: fast.model().to_string(),
        read,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{ContentBlock, MessageRole};

    fn message(role: MessageRole, words: &str, model: Option<&str>) -> ConversationMessage {
        let mut message = match role {
            MessageRole::User => ConversationMessage::user_text(words),
            _ => ConversationMessage::assistant(vec![ContentBlock::Text { text: words.to_string() }]),
        };
        message.model = model.map(str::to_string);
        message
    }

    /// The digest shows each thing the person said with what the assistant
    /// had said just before it, oldest first, and the session's model is its
    /// newest answer's.
    #[test]
    fn a_digest_shows_what_the_person_said_beside_what_came_before() {
        let messages = [
            message(MessageRole::User, "Answer in Korean from now on", None),
            message(MessageRole::Assistant, "Understood, switching to Korean.", Some("model-a")),
            message(MessageRole::User, "Never push to main here", None),
            message(MessageRole::Assistant, "I will work on a branch.", Some("model-b")),
        ];
        let digest = digest_of(&messages);
        assert_eq!(digest.len(), 2);
        assert_eq!(
            digest.text(),
            "[1] the person: Answer in Korean from now on\n\
             [2] the assistant had said: Understood, switching to Korean.\n    \
             the person: Never push to main here\n"
        );
        assert_eq!(model_of(&messages), Some("model-b"));
        assert_eq!(model_of(&messages[..1]), None, "nothing answered yet");
        assert!(digest_of(&[]).is_empty());
    }
}
