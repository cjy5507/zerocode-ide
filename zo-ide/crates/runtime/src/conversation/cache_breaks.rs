//! The turn loops hand every response's prompt-cache events to the break
//! ledger (`crate::prompt_cache_breaks`) with the turn's context, so the
//! "why did the prefix move" question can be answered from a file rather
//! than from a `[cache]` row that scrolled away.

use super::{ApiClient, ConversationRuntime, PromptCacheEvent, ToolExecutor};

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Persist the unexpected events of one response. Best effort: the ledger
    /// is evidence, never a reason to fail a turn, so an I/O error is dropped.
    /// A runtime no host armed (`relocate_traces_out_of_tree`) writes nothing
    /// either: its rows would land in the person's real home.
    pub(super) fn record_prompt_cache_breaks(&self, iteration: usize, events: &[PromptCacheEvent]) {
        if !events.iter().any(|event| event.unexpected || event.warning.is_some()) {
            return;
        }
        if !crate::durable_traces_armed() {
            return;
        }
        let Some(cwd) = self
            .workspace_cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
        else {
            return;
        };
        let recorded_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let context = crate::prompt_cache_breaks::BreakContext {
            session_id: &self.session.session_id,
            model: self.wire_model().map(|(model, _)| model),
            iteration,
            request_messages: self.session.messages.len(),
            recorded_at,
        };
        let records = crate::prompt_cache_breaks::break_records(&context, events);
        let _ = crate::prompt_cache_breaks::record_prompt_cache_breaks(&cwd, &records);
    }
}
