//! Where a fixed ledger pointer waits for a provider's own hook to collect it.
//!
//! [`crate::session_notify`] is the fast path for a provider that can be TOLD
//! something. This is the other shape, and it is the only one the measured
//! Claude CLI offers: nothing outside a session can push a message into it,
//! but the session itself calls out — on every prompt, on every turn end —
//! and the reply to that call is a documented place to answer.
//!
//! So the window leaves the pointer here, and the bridge hands it back when
//! the agent's own hook next knocks. No keystroke, no composer draft, and no
//! person pressing Enter.
//!
//! What may be left here is a [`PointerNotice`] and nothing else: the same
//! structural guard the notifier contract keeps. The ledger remains the only
//! authority that leases, acknowledges or completes mail, and collecting a
//! pointer is never an acknowledgement of anything.

use crate::session_notify::PointerNotice;

/// Which knock is asking.
///
/// The two moments differ in what the provider will accept back, so the
/// mailbox is told which one it is rather than guessing from the event name:
/// a turn ending takes a decision, a session starting takes context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerMoment {
    /// A tool has just finished and the provider takes non-error context
    /// there. The moment that reaches an agent in the MIDDLE of a long turn:
    /// a turn end alone leaves one deaf until all of its work is over.
    ToolBoundary,
    /// A turn is ending and the provider honours a continuation decision.
    /// Only offered when the provider was MEASURED to honour one, and never
    /// on a turn that is itself already a hook's continuation.
    TurnEnding,
    /// A session is starting — fresh, resumed, or adopted — and the provider
    /// accepts `hookSpecificOutput.additionalContext`.
    SessionStarting,
}

/// The window's side of the mailbox, as the bridge is allowed to see it.
///
/// One method, and it TAKES: a pointer handed to an agent must not be handed
/// to it again on the next knock, or a hook that continues a turn would keep
/// continuing it. What the window does with the emptied slot — whether the
/// PTY road may still speak later — is the window's decision and not this
/// trait's.
pub trait PointerMailbox: Send + Sync + 'static {
    /// The pointer waiting for this pane at this moment, if there is one.
    ///
    /// `pane_key` is the envelope's own identity for the terminal; resolving
    /// it to whatever the window calls a pane is the implementation's job, so
    /// this crate never learns the window's pane numbering.
    ///
    /// `launch_token` is the knock's own claim about WHICH launch it belongs
    /// to, and it is here because this road consumes. Every other hook road
    /// forwards the payload and lets the window check identity afterwards; a
    /// mailbox that took first would let an agent from a previous launch of a
    /// reused pane swallow the pointer meant for the one living there now,
    /// and the pointer would be gone before anybody noticed. An empty token
    /// is an older script that carries none — a fact, not a claim, and the
    /// implementation decides what to do with it.
    fn take(
        &self,
        pane_key: &str,
        launch_token: &str,
        moment: PointerMoment,
    ) -> Option<PointerNotice>;

    /// Put back a pointer this knock could not be answered with.
    ///
    /// A take is not a delivery. The reply still has to be composed, written
    /// and read, and the window still has to have been there to consume the
    /// event — and when any of that fails, a pointer that stayed taken is one
    /// the provider never saw and nothing will ever say again. So the caller
    /// hands it back at the first sign the answer did not go out.
    ///
    /// Defaulted to doing nothing so a mailbox with no such state stays
    /// source-compatible; an implementation that CONSUMES must implement it.
    fn restore(&self, _pane_key: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Once(Mutex<Option<PointerNotice>>);

    impl PointerMailbox for Once {
        fn take(
            &self,
            _pane_key: &str,
            _launch_token: &str,
            _moment: PointerMoment,
        ) -> Option<PointerNotice> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        }
    }

    #[test]
    fn a_collected_pointer_is_not_collected_twice() {
        let held = Once(Mutex::new(PointerNotice::new(
            "run-1",
            "run:run-1",
            "m-2",
            1,
        )));
        let first = held.take("term-3", "launch-1", PointerMoment::TurnEnding);
        assert_eq!(
            first.as_ref().map(PointerNotice::text),
            Some(zerocode_core::orchestration::pointer_text(1).as_str())
        );
        assert!(
            held.take("term-3", "launch-1", PointerMoment::TurnEnding)
                .is_none(),
            "a hook that collects the same pointer twice continues the same turn twice"
        );
    }
}
