//! What the seats' `#[ignore]` replays share when they read this machine's
//! own transcripts: one reading of a transcript's whole history, so two
//! replays of the same file see the same messages at the same indices.

use runtime::patch_review::{ask_reading, task_by, TaskReading};
use runtime::{ContentBlock, ConversationMessage, PatchAsk, Session};

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

/// One patch of a transcript, as the replay asks about it and grades it.
pub(super) struct ReplayPoint {
    /// Where in the history the edit's result sits — the replay's order
    /// inside one transcript.
    pub(super) at: usize,
    pub(super) ask: PatchAsk,
    pub(super) hindsight: Option<runtime::patch_review::Hindsight>,
    /// The task every reading reads at this patch, in `TaskReading::ALL`'s
    /// order — read here, never sent, so one replay shows what the others
    /// would have asked about the same sample.
    pub(super) tasks: Vec<String>,
}

/// What became of the patch `ask` names, read off the turns from the one it
/// was written in onward — `None` when the transcript ends before its window
/// does. Only the label reads past the patch; the ask was made from the
/// messages before it.
pub(super) fn hindsight_in(history: &[ConversationMessage], at: usize, ask: &PatchAsk) -> Option<runtime::patch_review::Hindsight> {
    let turns = runtime::patch_review::persons_turns(history);
    let mut start = 0;
    let mut watched = vec![runtime::patch_review::Watched::of(ask)];
    for turn in turns {
        let end = start + turn.len();
        if end > at {
            let decided = runtime::patch_review::hindsight_of_turn(&mut watched, turn);
            if let Some((_, hindsight)) = decided.into_iter().next() {
                return Some(hindsight);
            }
        }
        start = end;
    }
    None
}

/// Every patch in `history` a review would have been asked about: an edit's
/// result that wrote one, asked from the messages before it with the task
/// read as `reading` reads it.
pub(super) fn replay_points(history: &[ConversationMessage], reading: TaskReading) -> Vec<ReplayPoint> {
    let mut points = Vec::new();
    for (at, message) in history.iter().enumerate() {
        for block in &message.blocks {
            let ContentBlock::ToolResult { tool_use_id, tool_name, output, is_error, .. } = block else {
                continue;
            };
            if *is_error {
                continue;
            }
            let before = &history[..at];
            let Some(asked) = ask_reading(before, "replay", tool_use_id, tool_name, output, reading) else {
                continue;
            };
            let hindsight = hindsight_in(history, at, &asked);
            let tasks = TaskReading::ALL.iter().map(|other| task_by(before, *other)).collect();
            points.push(ReplayPoint { at, ask: asked, hindsight, tasks });
        }
    }
    points
}
