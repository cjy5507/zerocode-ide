//! Runtime SSH credential prompts — the connect asks the person mid-flight.
//!
//! Orca's contract, ported whole (`src/main/ipc/ssh-passphrase.ts`): a
//! connect that needs a secret sends `ssh:credential-request` `{requestId,
//! targetId, kind, detail}` to the window and waits; the window answers
//! through one command carrying the request id and the value — or `null`
//! for "the person declined". Whichever way a request ends, a
//! `ssh:credential-resolved` broadcast follows, so a modal left open in any
//! window closes itself instead of collecting stale questions. A request
//! nobody answers resolves to `None` on its own after
//! [`CREDENTIAL_TIMEOUT`], and the connect then fails with the error it
//! already had — the prompt buys a chance, never a hang.
//!
//! The secret itself only ever crosses as the command's argument and is
//! handed to [`zerocode_ssh::SshPassword`] at the connect; this module
//! stores nothing but the request's one-shot channel.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::oneshot;

/// How long an unanswered question stands before it answers itself with
/// nothing. Orca's own number: `CREDENTIAL_TIMEOUT_MS = 120_000`
/// (`ssh-passphrase.ts:5`) — long enough to walk to a password manager,
/// bounded so an abandoned connect cannot hold its caller forever.
pub const CREDENTIAL_TIMEOUT: Duration = Duration::from_millis(120_000);

/// Which secret the connect is missing. Orca's wire carries the same two
/// words (`ssh-api.ts:72`); this window's russh roads only ever ask for a
/// password today — keys live in the system agent, which does its own
/// asking — but the wire keeps both spellings so the modal is one contract,
/// not one per kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialKind {
    Password,
    #[expect(
        dead_code,
        reason = "the wire carries both of Orca's words so the modal is one \
                  contract; only the russh password road constructs a kind \
                  today, and the day a key road does, this expectation fails \
                  and says so"
    )]
    Passphrase,
}

/// What `ssh:credential-request` carries. Field names are Orca's own
/// (`ssh-passphrase.ts:42`), so the modal reads one shape wherever it came
/// from.
#[derive(Clone, Serialize)]
pub struct CredentialRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "targetId")]
    pub target_id: String,
    pub kind: CredentialKind,
    pub detail: String,
}

/// What `ssh:credential-resolved` carries — just the id, both on answer and
/// on timeout, exactly as Orca broadcasts it (`ssh-passphrase.ts:14`).
#[derive(Clone, Serialize)]
pub struct CredentialResolved {
    #[serde(rename = "requestId")]
    pub request_id: String,
}

/// The pending questions, keyed by request id. One map for the window, held
/// behind the app state; the ask road and the answer command meet here.
#[derive(Default)]
pub struct CredentialBroker {
    pending: Mutex<HashMap<String, oneshot::Sender<Option<String>>>>,
}

impl CredentialBroker {
    /// Opens a question and returns the channel its answer will arrive on.
    /// Dropping the receiver abandons the question harmlessly — a late
    /// submit finds nothing and reports so.
    pub fn begin(&self, request_id: &str) -> oneshot::Receiver<Option<String>> {
        let (sender, receiver) = oneshot::channel();
        self.lock().insert(request_id.to_string(), sender);
        receiver
    }

    /// Answers a question. `false` means the id was unknown — already
    /// answered, timed out, or invented — which Orca treats as a no-op for
    /// the same reason ("fire-once, so double invocation is safe",
    /// `pane`-side ack contract).
    pub fn submit(&self, request_id: &str, value: Option<String>) -> bool {
        match self.lock().remove(request_id) {
            // A receiver that hung up is the ask road timing out in the same
            // instant; the answer has nowhere to land and that is fine.
            Some(sender) => sender.send(value).is_ok(),
            None => false,
        }
    }

    /// Withdraws a question the ask road gave up on. `true` when it was
    /// still standing — the caller uses that to broadcast the resolution
    /// exactly once.
    pub fn abandon(&self, request_id: &str) -> bool {
        self.lock().remove(request_id).is_some()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Sender<Option<String>>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The happy road: the window answers, the ask road receives that very
    /// value — including an explicit `None`, which is the person saying no,
    /// not the absence of an answer.
    #[tokio::test]
    async fn an_answer_reaches_the_road_that_asked() {
        let broker = CredentialBroker::default();
        let receiver = broker.begin("one");
        assert!(broker.submit("one", Some("hunter2".into())));
        assert_eq!(receiver.await.unwrap(), Some("hunter2".into()));

        let declined = broker.begin("two");
        assert!(broker.submit("two", None));
        assert_eq!(declined.await.unwrap(), None);
    }

    /// Orca's fire-once contract: a second answer to the same question is a
    /// no-op, and an answer to a question nobody asked reports itself.
    #[tokio::test]
    async fn a_question_is_answered_at_most_once() {
        let broker = CredentialBroker::default();
        let receiver = broker.begin("one");
        assert!(broker.submit("one", Some("first".into())));
        assert!(!broker.submit("one", Some("second".into())));
        assert!(!broker.submit("never-asked", Some("noise".into())));
        assert_eq!(receiver.await.unwrap(), Some("first".into()));
    }

    /// The timeout road: the ask side withdraws, exactly once, and a submit
    /// arriving after that finds nothing — the modal was already told to
    /// close by the resolved broadcast the withdrawal earns.
    #[tokio::test]
    async fn an_abandoned_question_swallows_the_late_answer() {
        let broker = CredentialBroker::default();
        let receiver = broker.begin("one");
        assert!(broker.abandon("one"));
        assert!(!broker.abandon("one"));
        assert!(!broker.submit("one", Some("too-late".into())));
        assert!(receiver.await.is_err());
    }
}
