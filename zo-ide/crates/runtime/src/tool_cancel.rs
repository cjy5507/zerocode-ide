//! Per-tool cancellation: "Esc once kills the running tool, the turn lives on".
//!
//! This is deliberately *not* [`crate::HookAbortSignal`]. That signal means
//! "this turn is over"; this one means "whatever tool is running right now is
//! over, keep the turn". The distinction is the whole point of the feature: the
//! most common mid-turn interrupt in practice is a wedged MCP/bash call, and the
//! user's actual intent there is "drop that call and try something else", not
//! "throw away the turn and re-type the request".
//!
//! ## Why an epoch and not a bool
//!
//! A boolean flag would have to be reset after the cancel is consumed, which
//! races: the next tool of the *same* turn can start before the reset lands and
//! die instantly for a cancel it was never the target of. An epoch has no reset.
//! Every dispatch snapshots the epoch **before** it starts; a cancel bumps it.
//! A dispatch is cancelled if and only if the epoch moved while it was in
//! flight, so tools started after the keypress are untouched by construction and
//! every tool of a parallel batch that *was* in flight is cancelled together.

use std::sync::Arc;
use crate::conversation::ToolTextKind;

use tokio::sync::watch;

/// Body of the synthetic `tool_result` that settles a user-cancelled tool.
///
/// This is written **to the model**, not to the user: it has to say both what
/// happened and what is expected next, or the model reads a bare error and
/// retries the identical wedged call.
pub const CANCELLED_TOOL_RESULT: &str =
    "cancelled by user (Esc) — the turn continues; adapt your approach";

/// Handle used by the host (TUI key handler → turn controller) to cancel the
/// tools that are executing right now, without touching the turn.
///
/// Cloneable and cheap; all clones share one epoch.
#[derive(Clone, Debug)]
pub struct ToolCancelSignal {
    epoch: Arc<watch::Sender<u64>>,
}

impl Default for ToolCancelSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolCancelSignal {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(0);
        Self {
            epoch: Arc::new(tx),
        }
    }

    /// Cancel every tool dispatch currently in flight.
    ///
    /// Idempotence is not wanted here: two keypresses are two distinct cancels,
    /// and the second must also fire for tools that started between them.
    pub fn cancel_running_tools(&self) {
        self.epoch.send_modify(|epoch| *epoch += 1);
    }

    /// Current epoch. Only meaningful relative to a [`ToolCancelWatch`].
    #[must_use]
    pub fn epoch(&self) -> u64 {
        *self.epoch.borrow()
    }

    /// Open a dispatch-scoped view. The snapshot is taken **now**, so this must
    /// be created before the tool starts running.
    #[must_use]
    pub fn watch(&self) -> ToolCancelWatch {
        let mut rx = self.epoch.subscribe();
        // Mark the current value seen: `changed()` then resolves only on a
        // cancel issued strictly after this point.
        rx.borrow_and_update();
        ToolCancelWatch { rx }
    }
}

/// A single tool dispatch's view of the cancel signal.
#[derive(Debug)]
pub struct ToolCancelWatch {
    rx: watch::Receiver<u64>,
}

impl ToolCancelWatch {
    /// Non-blocking probe: has a cancel been requested since this watch opened?
    pub fn is_cancelled(&mut self) -> bool {
        self.rx.has_changed().unwrap_or(false)
    }

    /// Resolves once a cancel is requested after this watch opened.
    ///
    /// If the signal has been dropped (host gone) this never resolves, so a
    /// teardown can never be mistaken for a user cancel and silently poison an
    /// otherwise healthy dispatch.
    pub async fn cancelled(&mut self) {
        if self.rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// How a raced tool dispatch ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolDispatchOutcome {
    /// The tool produced its text, and how that text is to be read.
    Completed(String, ToolTextKind),
    /// The user cancelled it mid-flight; the caller settles a synthetic result.
    Cancelled,
}

impl ToolDispatchOutcome {
    /// Collapse into the `(output, kind, cancelled)` triple the streaming
    /// tool loop finalizes with. A cancel is a failure on purpose: the
    /// Anthropic tool-result contract has no third state, and a non-error
    /// cancellation reads to the model as "this succeeded and returned prose".
    #[must_use]
    pub fn into_tool_result(self) -> (String, ToolTextKind, bool) {
        match self {
            Self::Completed(output, kind) => (output, kind, false),
            Self::Cancelled => (CANCELLED_TOOL_RESULT.to_string(), ToolTextKind::Failure, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_opened_before_cancel_sees_it() {
        let signal = ToolCancelSignal::new();
        let mut watch = signal.watch();
        assert!(!watch.is_cancelled());
        signal.cancel_running_tools();
        assert!(watch.is_cancelled());
    }

    #[test]
    fn watch_opened_after_cancel_is_clean() {
        let signal = ToolCancelSignal::new();
        signal.cancel_running_tools();
        // The next tool of the same turn must not inherit the previous cancel.
        let mut watch = signal.watch();
        assert!(!watch.is_cancelled());
    }

    #[test]
    fn every_watch_of_a_parallel_batch_is_cancelled_together() {
        let signal = ToolCancelSignal::new();
        let mut batch: Vec<_> = (0..8).map(|_| signal.watch()).collect();
        signal.cancel_running_tools();
        assert!(batch.iter_mut().all(ToolCancelWatch::is_cancelled));
    }

    #[test]
    fn a_second_cancel_fires_for_a_tool_started_after_the_first() {
        let signal = ToolCancelSignal::new();
        signal.cancel_running_tools();
        let mut watch = signal.watch();
        assert!(!watch.is_cancelled());
        signal.cancel_running_tools();
        assert!(watch.is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_future_resolves_on_request() {
        let signal = ToolCancelSignal::new();
        let mut watch = signal.watch();
        let cancel = signal.clone();
        tokio::spawn(async move {
            cancel.cancel_running_tools();
        });
        watch.cancelled().await;
    }

    #[tokio::test]
    async fn dropped_signal_never_reads_as_a_cancel() {
        let signal = ToolCancelSignal::new();
        let mut watch = signal.watch();
        drop(signal);
        // A teardown must not settle a synthetic cancellation on a live tool.
        let raced = tokio::time::timeout(std::time::Duration::from_millis(50), watch.cancelled());
        assert!(raced.await.is_err());
    }

    #[test]
    fn cancelled_outcome_settles_as_an_error_result() {
        let (body, kind, cancelled) = ToolDispatchOutcome::Cancelled.into_tool_result();
        assert_eq!(body, CANCELLED_TOOL_RESULT);
        assert!(kind.is_error());
        assert!(cancelled);
    }

    #[test]
    fn completed_outcome_passes_through() {
        let (body, kind, cancelled) =
            ToolDispatchOutcome::Completed("ok".into(), ToolTextKind::Answer).into_tool_result();
        assert_eq!(body, "ok");
        assert!(!kind.is_error());
        assert!(!cancelled);
        let (_, kind, _) = ToolDispatchOutcome::Completed("declined: inline".into(), ToolTextKind::Declined)
            .into_tool_result();
        assert!(kind.is_error() && kind == ToolTextKind::Declined);
    }
}
