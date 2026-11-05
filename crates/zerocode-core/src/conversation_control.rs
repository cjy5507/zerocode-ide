//! Human conversation controls shared by the window and the owning zo process.

use serde::{Deserialize, Serialize};

/// Queue a control on the session owner's idle loop.
pub const REQUEST_METHOD: &str = "session.conversation_action";
/// Read the result of that control without submitting it again.
pub const RESULT_METHOD: &str = "session.conversation_action_result";

/// Which state a person chose to restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreScope {
    /// Restore the conversation, preserving current files.
    Conversation,
    /// Restore guarded file edits, preserving the conversation.
    Files,
    /// Restore both only after their preconditions have passed.
    Both,
}

/// A control names a persisted message, never a renderer's temporary row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    /// Read what this prompt would rewind, including current file conflicts.
    Preview {
        /// Absolute message index of the chosen user prompt.
        message_index: u32,
    },
    /// Fork the saved conversation through the chosen user turn.
    Fork {
        /// Absolute message index of the chosen user prompt.
        message_index: u32,
        /// Revision of the preview the person accepted.
        revision: String,
    },
    /// Restore to immediately before the chosen user prompt.
    Restore {
        /// Absolute message index of the chosen user prompt.
        message_index: u32,
        /// Revision of the preview the person accepted.
        revision: String,
        /// The state selected in the recovery dialog.
        scope: RestoreScope,
    },
}

impl Action {
    /// The chosen user prompt in the session's absolute message domain.
    #[must_use]
    pub const fn message_index(&self) -> u32 {
        match self {
            Self::Preview { message_index }
            | Self::Fork { message_index, .. }
            | Self::Restore { message_index, .. } => *message_index,
        }
    }
}
