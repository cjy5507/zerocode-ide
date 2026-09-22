//! The patch review seat's seam (t-6203): one edit's result, asked about
//! after the tool wrote the patch and before the model reads it.
//!
//! Both loops pass here — the streaming one from the one place every tool
//! result is finalized, the synchronous one from its own — and both hand in
//! the tool's own output, taken before any hook merged text into it, since
//! the seat reads the patch out of that envelope. What comes back is at most
//! one line, which joins the model-facing copy only: a result the seat does
//! not act on is the result a runtime with no seat hands back, to the byte.

use std::sync::Arc;

use super::ConversationRuntime;

impl<C, T> ConversationRuntime<C, T> {
    /// Whether a result of `tool_name` is one the seat is asked about — read
    /// before the tool's own output is kept aside for it, so a runtime with
    /// no seat keeps nothing.
    pub(super) fn reviews_edits_of(&self, tool_name: &str) -> bool {
        self.patch_review_seat.is_some() && crate::compact::is_edit_result_tool(tool_name)
    }

    /// The line the patch review seat adds to one edit's result, or `None`:
    /// no seat, a result that carries no patch, no words of the person's to
    /// judge the patch against, a seat that only records, a review that
    /// permits, or one that never answered.
    ///
    /// `output` is the tool's own output. The conversation it is read against
    /// is the session as it stands — the result being finalized is not in it
    /// yet, so the review sees the present the patch was written in and
    /// nothing after it.
    ///
    /// `&mut self` for the reason [`Self::judged_compaction_plan`] gives: the
    /// future holds the borrow across the seat's await.
    pub(super) async fn reviewed_edit_note(
        &mut self,
        tool_use_id: &str,
        tool_name: &str,
        output: &str,
    ) -> Option<String> {
        let seat = self.patch_review_seat.as_ref().map(Arc::clone)?;
        let ask = crate::patch_review::ask_for(
            &self.session.messages,
            &self.attempt,
            tool_use_id,
            tool_name,
            output,
        )?;
        seat.review(ask).await.note
    }

    /// [`Self::reviewed_edit_note`] for the synchronous road; a runtime with
    /// no seat pays nothing here.
    pub(super) fn reviewed_edit_note_blocking(
        &mut self,
        tool_use_id: &str,
        tool_name: &str,
        output: &str,
    ) -> Option<String> {
        self.patch_review_seat.as_ref()?;
        ::api::sync_bridge::run_blocking(self.reviewed_edit_note(tool_use_id, tool_name, output))
    }
}

/// `output` with the seat's line after it, as a notice joins a result: a
/// blank line, then the line.
pub(super) fn with_review_note(mut output: String, note: Option<String>) -> String {
    if let Some(note) = note {
        output.push_str("\n\n");
        output.push_str(&note);
    }
    output
}
