//! The tool guards' seam (t-6348): a shell command handed to the command guard
//! right before it runs, and each text a tool hands back handed to the text
//! guard before the model reads it.
//!
//! Both loops pass here — the streaming one from where a tool is dispatched
//! and from the one place every result is finalized, the synchronous one from
//! its own — and both hand in the tool's own output, taken before any hook
//! merged text into it. What comes back joins the model-facing copy only: a
//! guard that does not act leaves the result a runtime with no seat hands
//! back, to the byte.

use std::sync::Arc;

use super::ConversationRuntime;
use crate::tool_cancel::CANCELLED_TOOL_RESULT;
use crate::tool_guard::{command_with_cwd, guarded_output, task_line, text_ask, CommandAsk, CommandRan, TextAsk, SHELL_TOOL};

impl<C, T> ConversationRuntime<C, T> {
    /// Hand a shell command to the command guard, right before it runs. Never
    /// waits; a runtime with no seat, another tool, and a command today's rule
    /// proves read-only pay nothing here.
    pub(super) fn guard_command(&self, tool_use_id: &str, tool_name: &str, input: &str)
    where
        T: super::ToolExecutor,
    {
        let Some(seat) = self.tool_guard_seat.as_ref() else {
            return;
        };
        let Some((command, cwd)) = command_with_cwd(tool_name, input, self.tool_executor.execution_cwd()) else {
            return;
        };
        seat.command(CommandAsk {
            attempt: self.attempt.clone(),
            owner: self.session.session_id.clone(),
            tool_use_id: tool_use_id.to_string(),
            command,
            cwd,
            task: task_line(&self.session.messages),
        });
    }

    /// What the text guard is handed for one tool's own `output`, with the
    /// output kept aside for the fence — `None` with no seat, for an error,
    /// and for a tool it does not read. Read before any hook merges text in.
    pub(super) fn text_guard_ask(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        output: &str,
        is_error: bool,
    ) -> Option<(TextAsk, String)> {
        self.tool_guard_seat.as_ref()?;
        if is_error {
            return None;
        }
        text_ask(&self.attempt, tool_use_id, tool_name, output).map(|mut ask| {
            ask.owner.clone_from(&self.session.session_id);
            (ask, output.to_string())
        })
    }

    /// The model-facing `output` once both guards have had their say: the text
    /// guard's fence and line for a text it was handed, the command guard's
    /// facts and line for a shell call. A guard that does not act changes
    /// nothing.
    ///
    /// `&mut self` for the reason [`Self::reviewed_edit_note`] gives: the
    /// future holds the borrow across the seat's await.
    pub(super) async fn guarded_result(
        &mut self,
        tool_use_id: &str,
        tool_name: &str,
        text: Option<(TextAsk, String)>,
        mut output: String,
        is_error: bool,
    ) -> String {
        let Some(seat) = self.tool_guard_seat.as_ref().map(Arc::clone) else {
            return output;
        };
        if let Some((ask, pristine)) = text {
            let guard = seat.text(ask).await;
            output = guarded_output(output, &pristine, &guard);
        }
        if tool_name == SHELL_TOOL {
            let ran = CommandRan {
                owner: self.session.session_id.clone(),
                tool_use_id: tool_use_id.to_string(),
                failed: is_error,
                cancelled: output.starts_with(CANCELLED_TOOL_RESULT),
            };
            output = super::reviewed_edit::with_review_note(output, seat.command_ran(ran).await);
        }
        output
    }

    /// [`Self::guarded_result`] for the synchronous road; a runtime with no
    /// seat pays nothing here.
    pub(super) fn guarded_result_blocking(
        &mut self,
        tool_use_id: &str,
        tool_name: &str,
        text: Option<(TextAsk, String)>,
        output: String,
        is_error: bool,
    ) -> String {
        if self.tool_guard_seat.is_none() {
            return output;
        }
        ::api::sync_bridge::run_blocking(self.guarded_result(tool_use_id, tool_name, text, output, is_error))
    }
}
