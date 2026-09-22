//! What the seats' `#[ignore]` replays share when they read this machine's
//! own transcripts: one reading of a transcript's whole history, so two
//! replays of the same file see the same messages at the same indices.

use runtime::{ConversationMessage, Session};

/// [`whole_history`] with every tool result microcompact later blanked put
/// back where the session still holds its body — the history as each moment
/// of it stood, which is what a replay asking "what did a seat see then"
/// reads (`runtime::heal_cleared_tool_results`).
pub(super) fn history_as_it_stood(session: &Session) -> Option<Vec<ConversationMessage>> {
    let mut history = whole_history(session)?;
    runtime::heal_cleared_tool_results(&mut history, session);
    Some(history)
}

/// Every message a transcript ever held, in order: the vault's evicted
/// records below `first_message_index`, then what is still live. `None`
/// when the vault has a hole, since an index would then not be a seq.
pub(super) fn whole_history(session: &Session) -> Option<Vec<ConversationMessage>> {
    let first_live = session.first_message_index();
    let mut evicted: Vec<ConversationMessage> = Vec::new();
    for record in session.read_vault() {
        if record.vault_seq >= first_live {
            break;
        }
        if record.vault_seq != u32::try_from(evicted.len()).ok()? {
            return None;
        }
        evicted.push(record.message);
    }
    if u32::try_from(evicted.len()).ok()? != first_live {
        return None;
    }
    evicted.extend(session.messages.iter().cloned());
    Some(evicted)
}
