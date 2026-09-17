//! The last step is the person's (docs/design/computer-use-full-operator.md
//! §1.5): which kinds are guarded (the settings' policy), the words the
//! helper matches labels against, and the question the window puts to the
//! person — opened here, answered by a command from the page, waited on by
//! the CLI request that needs it, bounded by the table.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zerocode_core::computer_use::{COMPUTER_CONFIRM_TIMEOUT_MS, ConfirmKind};

/// Which kinds ask. On by default — a person who never chose is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub payment: bool,
    pub transfer: bool,
    pub delete: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            payment: true,
            transfer: true,
            delete: true,
        }
    }
}

impl Policy {
    #[must_use]
    pub const fn asks(self, kind: ConfirmKind) -> bool {
        match kind {
            ConfirmKind::Payment => self.payment,
            ConfirmKind::Transfer => self.transfer,
            ConfirmKind::Delete => self.delete,
        }
    }

    /// The words the helper matches, for the kinds that ask — nothing when
    /// none does, so an unguarded request carries nothing extra.
    #[must_use]
    pub fn guard_words(self) -> Option<Value> {
        let kinds: serde_json::Map<String, Value> = ConfirmKind::ALL
            .into_iter()
            .filter(|kind| self.asks(*kind))
            .map(|kind| (kind.as_str().to_string(), serde_json::json!(kind.words())))
            .collect();
        (!kinds.is_empty()).then_some(Value::Object(kinds))
    }
}

static POLICY: Mutex<Policy> = Mutex::new(Policy {
    payment: true,
    transfer: true,
    delete: true,
});

pub fn set_policy(policy: Policy) {
    *POLICY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = policy;
}

#[must_use]
pub fn policy() -> Policy {
    *POLICY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Who answers a guarded press: the person, asked now — or the caller the
/// press is handed back to unpressed (a recipe's walk has nobody to wait on
/// a question for; it stops there and says the step is the person's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asking {
    Person,
    HandBack,
}

/// What the person said, or did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allowed,
    Refused,
    TimedOut,
}

/// One question, as the page sees it.
#[derive(Debug, Clone, Serialize)]
pub struct Ask {
    pub id: String,
    pub kind: ConfirmKind,
    pub label: String,
    pub verb: String,
    #[serde(rename = "timeoutMs")]
    pub timeout_ms: u64,
}

/// Questions waiting for an answer, by id.
static PENDING: Mutex<Option<HashMap<String, mpsc::Sender<bool>>>> = Mutex::new(None);
static NEXT_ID: Mutex<u64> = Mutex::new(0);

/// The window's way of putting a question to the person — installed at boot
/// with the page in hand; a process without one (a test) has nobody to ask.
type Asker = dyn Fn(&Ask) -> Decision + Send + Sync;
static ASKER: OnceLock<Box<Asker>> = OnceLock::new();

pub fn install_asker(asker: Box<Asker>) {
    let _ = ASKER.set(asker);
}

/// The person's turn (§7.4): the desk is theirs until they say they are
/// done — a 2FA code, a CAPTCHA, the press the operator may not make.
#[derive(Debug, Clone, Serialize)]
pub struct Handoff {
    pub id: String,
    pub reason: String,
    #[serde(rename = "timeoutMs")]
    pub timeout_ms: u64,
}

type HandoffAsker = dyn Fn(&Handoff) -> Decision + Send + Sync;
static HANDOFF_ASKER: OnceLock<Box<HandoffAsker>> = OnceLock::new();

pub fn install_handoff_asker(asker: Box<HandoffAsker>) {
    let _ = HANDOFF_ASKER.set(asker);
}

/// Give the desk to the person and wait for their word. With nobody to
/// hand to, the answer is a refusal — the work does not pretend it went on.
#[must_use]
pub fn handoff(reason: &str, timeout_ms: u64) -> Decision {
    let ask = Handoff {
        id: next_id(),
        reason: reason.to_string(),
        timeout_ms,
    };
    HANDOFF_ASKER
        .get()
        .map_or(Decision::Refused, |asker| asker(&ask))
}

#[must_use]
pub fn next_id() -> String {
    let mut held = NEXT_ID
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *held += 1;
    format!("confirm-{}-{}", std::process::id(), *held)
}

/// Open a question: the receiver the asker waits on.
#[must_use]
pub fn open(id: &str) -> mpsc::Receiver<bool> {
    let (sender, receiver) = mpsc::channel();
    PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(id.to_string(), sender);
    receiver
}

/// The person's answer to a question — false when no such question waits.
pub fn answer(id: &str, allow: bool) -> bool {
    let sender = PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .and_then(|pending| pending.remove(id));
    sender.is_some_and(|sender| sender.send(allow).is_ok())
}

/// How many questions are open now — what `status` says, so a caller that
/// has gone (a bench run past its budget) waits for them before it looks.
#[must_use]
pub fn open_questions() -> usize {
    PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map_or(0, HashMap::len)
}

/// Whether the person is being asked something — a press's question or the
/// desk's handoff. While they are, no other action goes (§1.5): one sent
/// meanwhile could answer the question for them.
#[must_use]
pub fn asking() -> bool {
    PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|pending| !pending.is_empty())
}

/// Wait for the answer, for at most the table.
#[must_use]
pub fn wait(receiver: &mpsc::Receiver<bool>, timeout: Duration) -> Decision {
    match receiver.recv_timeout(timeout) {
        Ok(true) => Decision::Allowed,
        Ok(false) => Decision::Refused,
        Err(_) => Decision::TimedOut,
    }
}

/// Forget a question nobody will answer any more.
pub fn close(id: &str) {
    if let Some(pending) = PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
    {
        pending.remove(id);
    }
}

/// Put the question to the person and wait. With nobody to ask, the answer
/// is a refusal — a press is never let through on silence.
#[must_use]
pub fn ask(kind: ConfirmKind, label: &str, verb: &str) -> Decision {
    let ask = Ask {
        id: next_id(),
        kind,
        label: label.to_string(),
        verb: verb.to_string(),
        timeout_ms: COMPUTER_CONFIRM_TIMEOUT_MS,
    };
    ASKER.get().map_or(Decision::Refused, |asker| asker(&ask))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_names_the_words_for_the_kinds_that_ask() {
        let all = Policy::default();
        let words = all.guard_words().expect("three kinds ask");
        assert!(
            words["payment"]
                .as_array()
                .is_some_and(|list| list.iter().any(|w| w == "결제"))
        );
        assert!(
            words["delete"]
                .as_array()
                .is_some_and(|list| list.iter().any(|w| w == "delete"))
        );
        let none = Policy {
            payment: false,
            transfer: false,
            delete: false,
        };
        assert_eq!(none.guard_words(), None, "nothing guarded, nothing sent");
        let only_transfer = Policy {
            payment: false,
            transfer: true,
            delete: false,
        };
        let words = only_transfer.guard_words().expect("one kind");
        assert!(words.get("payment").is_none() && words.get("transfer").is_some());
        assert!(
            only_transfer.asks(ConfirmKind::Transfer) && !only_transfer.asks(ConfirmKind::Payment)
        );
    }

    #[test]
    fn a_question_is_answered_by_id_or_times_out_and_silence_refuses() {
        // An open question refuses every action in this binary.
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = next_id();
        let receiver = open(&id);
        assert!(asking(), "an open question is the person's attention");
        assert!(!answer("no-such-question", true));
        assert!(answer(&id, true));
        assert!(!asking(), "an answered question is closed");
        assert_eq!(
            wait(&receiver, Duration::from_millis(50)),
            Decision::Allowed
        );

        let id = next_id();
        let receiver = open(&id);
        assert!(answer(&id, false));
        assert_eq!(
            wait(&receiver, Duration::from_millis(50)),
            Decision::Refused
        );

        let id = next_id();
        let receiver = open(&id);
        assert_eq!(
            wait(&receiver, Duration::from_millis(10)),
            Decision::TimedOut
        );
        close(&id);
        assert!(!answer(&id, true), "a closed question takes no answer");
        assert_eq!(
            ask(ConfirmKind::Payment, "Pay", "mouse-click"),
            Decision::Refused,
            "nobody installed to ask"
        );
    }
}
