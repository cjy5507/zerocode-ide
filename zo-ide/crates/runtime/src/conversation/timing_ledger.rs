//! The streaming turn loop hands each request's stream stamps to the timing
//! ledger (`crate::request_timings`) with the turn's context, so "where did
//! the first token's wait go" can be answered from a file after the fact.

use super::{ApiClient, ConversationRuntime, ToolExecutor};
use crate::request_timings::StreamStamps;

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Persist one request's waits. Best effort: the ledger is evidence,
    /// never a reason to fail a turn, so an I/O error is dropped; a request
    /// that never left the runtime (no probe blocks) writes nothing.
    /// A runtime no host armed (`relocate_traces_out_of_tree`) writes nothing
    /// either: its rows would land in the person's real home.
    pub(super) fn record_request_timing(&self, iteration: usize, stamps: &StreamStamps, outcome: &str) {
        if !stamps.left_the_runtime() {
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
        let context = crate::request_timings::TimingContext {
            session_id: &self.session.session_id,
            model: self.wire_model().map(|(model, _)| model),
            iteration,
            request_messages: self.session.messages.len(),
            recorded_at,
            outcome,
            attempt: self.attempt(),
        };
        let record = crate::request_timings::timing_record(&context, stamps);
        let _ = crate::request_timings::record_request_timing(&cwd, &record);
    }
}
