//! Preview, fork and recover the conversation owned by this process.
//! The frontend calls this only between turns; previews are revision-bound.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use core_types::{ContentBlock, MessageRole, PermissionMode, Session};
use serde_json::{Value, json};
use zerocode_core::conversation_control::{Action, RestoreScope};

use super::plain_session::PlainSession;

pub(crate) struct Outcome {
    pub(crate) response: Value,
    pub(crate) replay: bool,
    pub(crate) fork: Option<PathBuf>,
}

struct Target {
    local: usize,
    end: usize,
    steps: usize,
    prompt: String,
    revision: String,
}

fn revision(session: &Session) -> String {
    let mut hash = DefaultHasher::new();
    session.session_id.hash(&mut hash);
    session.first_message_index().hash(&mut hash);
    session.updated_at_ms.hash(&mut hash);
    // Include the content, not just a length/timestamp which can coincide
    // across a rewind and another message in the same clock tick.
    format!("{:?}", session.messages).hash(&mut hash);
    format!("{:016x}", hash.finish())
}

fn target(session: &Session, absolute: u32) -> Result<Target, String> {
    let local = absolute.checked_sub(session.first_message_index())
        .and_then(|index| usize::try_from(index).ok())
        .ok_or("conversation-message-not-retained")?;
    let message = session.messages.get(local).filter(|message| message.is_user_prompt())
        .ok_or("conversation-message-not-retained")?;
    let prompt = message.blocks.iter().filter_map(|block| match block {
        ContentBlock::Text { text } => Some(text.as_str()),
        _ => None,
    }).collect::<Vec<_>>().join("\n");
    let end = session.messages.iter().enumerate().skip(local + 1)
        .find_map(|(index, message)| message.is_user_prompt().then_some(index))
        .unwrap_or(session.messages.len());
    let steps = session.messages[local..].iter().filter(|message| message.is_user_prompt()).count();
    Ok(Target { local, end, steps, prompt, revision: revision(session) })
}

/// A checkpoint's monotonic index is not a message index. Only an explicit
/// persisted anchor may connect them; old unanchored files are never guessed.
fn checkpoint_for(checkpoints: &[tools::WorkspaceCheckpoint], message: u32) -> Result<Option<usize>, String> {
    // One checkpoint spans a model turn. A steer inside that turn cannot
    // restore only the writes after its message without guessing a snapshot.
    if checkpoints.iter().any(|checkpoint| checkpoint.message_index.is_some_and(|start| start < message)
        && checkpoint.message_end_index.is_some_and(|end| message < end)) {
        return Err("conversation-checkpoint-unmapped".into());
    }
    let unmapped = checkpoints.iter().any(|checkpoint| checkpoint.message_index.is_none()
        || (checkpoint.message_index.is_some_and(|start| start < message) && checkpoint.message_end_index.is_none()));
    if let Some(checkpoint) = checkpoints.iter().find(|checkpoint| checkpoint.message_index.is_some_and(|index| index >= message)) {
        if checkpoint.message_index != Some(message)
            && (unmapped
                || !checkpoints.iter().any(|item| item.message_index.is_some_and(|index| index < message))) {
            return Err("conversation-checkpoint-unmapped".into());
        }
        return Ok(Some(checkpoint.turn_index));
    }
    if unmapped {
        return Err("conversation-checkpoint-unmapped".into());
    }
    Ok(None)
}

fn can_rewind(session: &Session, target: &Target) -> bool {
    let expected = session.messages[..target.local].iter().filter(|message| message.is_user_prompt()).count();
    // A fork has no persistence binding, so this preflight cannot touch the
    // real transcript or its writer lease.
    let mut draft = session.fork(None);
    draft.rewind_turns(target.steps) > 0
        && draft.messages.iter().filter(|message| message.is_user_prompt()).count() == expected
}

impl PlainSession {
    pub(crate) fn conversation_action(&mut self, expected_session: &str, action: &Action) -> Result<Outcome, String> {
        if self.handle.id != expected_session { return Err("conversation-session-changed".into()); }
        let mut selected = target(self.runtime.session(), action.message_index())?;
        let snapshot = self.runtime.session();
        let runtime = self.runtime.try_runtime().ok_or("runtime is not available")?;
        let context = runtime.tool_executor().tool_registry().context();
        let checkpoints = context.workspace_checkpoints();
        selected.revision = format!("{}:{}", selected.revision, checkpoints.last().map_or(0, |checkpoint| checkpoint.turn_index));
        if let Action::Fork { revision, .. } | Action::Restore { revision, .. } = action {
            if *revision != selected.revision { return Err("conversation-preview-stale".into()); }
        }
        let checkpoint = checkpoint_for(&checkpoints, action.message_index());
        let preview = match &checkpoint {
            Ok(Some(index)) => Some(context.preview_workspace_restore(*index).map_err(|error| error.to_string())?),
            _ => None,
        };
        let conversation_available = can_rewind(snapshot, &selected);
        let file_problem = if self.permission_mode == PermissionMode::ReadOnly {
            Some("conversation-files-read-only")
        } else if checkpoint.is_err() {
            Some("conversation-checkpoint-unmapped")
        } else if preview.as_ref().is_some_and(|preview| !preview.conflicted.is_empty() || !preview.skipped.is_empty()) {
            Some("conversation-files-conflicted")
        } else { None };

        match action {
            Action::Preview { message_index } => Ok(Outcome {
                response: json!({
                    "session":self.handle.id, "message_index":message_index,
                    "revision":selected.revision, "prompt":selected.prompt,
                    "conversation_available":conversation_available,
                    "files_available":file_problem.is_none(), "file_problem":file_problem,
                    "files":preview,
                }), replay: false, fork: None,
            }),
            Action::Fork { .. } => {
                let mut source = snapshot.clone();
                Arc::make_mut(&mut source.messages).truncate(selected.end);
                let path = crate::session_cli::fork_session(&source, &self.handle.path, None, &self.cwd)?;
                Ok(Outcome {
                    response: json!({"kind":"forked","path":path}),
                    replay: false, fork: Some(path),
                })
            }
            Action::Restore { scope, .. } => {
                let chat = *scope != RestoreScope::Files;
                let files = *scope != RestoreScope::Conversation;
                if chat && !conversation_available { return Err("conversation-recovery-unavailable".into()); }
                if files {
                    if let Some(problem) = file_problem { return Err(problem.into()); }
                }
                let mut restored = None;
                if files {
                    if let Some(index) = checkpoint? {
                        let summary = context.restore_workspace_strict(index).map_err(|error| error.to_string())?;
                        if !summary.conflicted.is_empty() || !summary.skipped.is_empty() {
                            // A failed preflight writes nothing. An I/O error after
                            // a successful preflight is reported with the actual
                            // affected paths, and never drops the conversation.
                            return Ok(Outcome {
                                response: json!({"kind":"restore_incomplete","files":summary}),
                                replay: false, fork: None,
                            });
                        }
                        restored = Some(summary);
                    }
                }
                if chat {
                    if let Err(error) = context.retire_workspace_checkpoint_messages(action.message_index()) {
                        return Ok(Outcome {
                            response: json!({"kind":"conversation_save_failed","files":restored,"error":error.to_string()}),
                            replay: false, fork: None,
                        });
                    }
                }
                let runtime = self.runtime.try_runtime_mut().ok_or("runtime is not available")?;
                if chat && runtime.rewind_turns(selected.steps) == 0 {
                    return Ok(Outcome {
                        response: json!({"kind":"conversation_save_failed","files":restored}),
                        replay: false, fork: None,
                    });
                }
                if let Some(summary) = &restored {
                    let count = summary.restored.len() + summary.deleted.len();
                    if count > 0 {
                        runtime.reset_surface_repetition_state();
                        let mut note = core_types::session::ConversationMessage::user_text(
                            crate::tui::strings::workspace_restored_reminder(summary.target_turn_index, count),
                        );
                        note.role = MessageRole::System;
                        if let Err(error) = runtime.session_mut().push_message(note) {
                            return Ok(Outcome {
                                response: json!({"kind":"restored_note_failed","scope":scope,"files":restored,"error":error.to_string()}),
                                replay: chat, fork: None,
                            });
                        }
                    }
                }
                Ok(Outcome {
                    response: json!({"kind":"restored","scope":scope,"prompt":selected.prompt,"files":restored}),
                    replay: chat, fork: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::session::ConversationMessage;

    #[test]
    fn message_selection_is_revision_bound_and_never_a_display_row() {
        let mut session = Session::new();
        session.push_message(ConversationMessage::user_text("first")).unwrap();
        session.push_message(ConversationMessage::assistant(vec![ContentBlock::Text { text: "answer".into() }])).unwrap();
        session.push_message(ConversationMessage::user_text("second")).unwrap();
        let first = target(&session, 0).unwrap();
        assert_eq!((first.end, first.steps), (2, 2));
        assert!(can_rewind(&session, &first));
        assert!(target(&session, 1).is_err(), "an assistant message cannot be a recovery boundary");
        assert!(target(&session, 99).is_err());
        let old = first.revision;
        session.push_message(ConversationMessage::assistant(vec![ContentBlock::Text { text: "new".into() }])).unwrap();
        assert_ne!(revision(&session), old);
    }

    #[test]
    fn checkpoint_lookup_uses_the_explicit_message_anchor() {
        let checkpoints = vec![tools::WorkspaceCheckpoint { turn_index: 38, message_index: Some(20), message_end_index: Some(24), ..Default::default() }];
        assert_eq!(checkpoint_for(&checkpoints, 20).unwrap(), Some(38));
        assert_eq!(checkpoint_for(&checkpoints, 24).unwrap(), None);
        assert!(checkpoint_for(&checkpoints, 19).is_err(), "a turn before the retained file history is not a complete restore");
        assert!(checkpoint_for(&[tools::WorkspaceCheckpoint { turn_index: 20, ..Default::default() }], 20).is_err());
        let steered = vec![tools::WorkspaceCheckpoint { turn_index: 38, message_index: Some(20), message_end_index: Some(26), ..Default::default() }];
        assert_eq!(checkpoint_for(&steered, 20).unwrap(), Some(38));
        assert!(checkpoint_for(&steered, 23).is_err(), "a steer cannot restore half a file checkpoint");
        assert_eq!(checkpoint_for(&steered, 26).unwrap(), None);
    }
}
