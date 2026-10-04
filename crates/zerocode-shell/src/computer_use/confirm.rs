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
use zerocode_core::computer_use::{COMPUTER_CONFIRM_TIMEOUT_MS, ConfirmKind, handoff_ms};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::handoff_code::{CodeLimits, OneTimeCode};

use super::ComputerUseError;

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

/// What may be sent into an open question: a yes or a no — or, for a card
/// that offers its line (`handoff --ask-code`), the code the person typed.
#[derive(Debug)]
pub enum Reply {
    Said(bool),
    Code(OneTimeCode),
}

/// A question waiting for its answer: where the answer goes, and whether the
/// card offers a line for a code. A card with a line is answered by a code or
/// a cancel, never by a bare "done" — nothing but a code can stand for one.
struct Waiting {
    reply: mpsc::Sender<Reply>,
    takes_code: bool,
}

/// Questions waiting for an answer, by id.
static PENDING: Mutex<Option<HashMap<String, Waiting>>> = Mutex::new(None);
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
    /// The page's words for the reason, when the window wrote it rather than
    /// an agent: an i18n key and what fills it — `reason` stays the words for
    /// a page that does not know the key.
    #[serde(rename = "reasonKey", skip_serializing_if = "Option::is_none")]
    pub reason_key: Option<&'static str>,
    #[serde(rename = "reasonArgs", skip_serializing_if = "Value::is_null")]
    pub reason_args: Value,
    #[serde(rename = "timeoutMs")]
    pub timeout_ms: u64,
    /// The line the card offers for a one-time code, when it offers one: what
    /// the page may type in and how much. Absent on the plain card — the
    /// page never decides whether a card takes a value, the window does.
    #[serde(rename = "codeAsk", skip_serializing_if = "Option::is_none")]
    pub code_ask: Option<CodeLimits>,
}

/// What a person's turn came back with: what they said, and the code they
/// typed when the card offered a line for one. The code is in no other
/// place — not the page's closing notice, not a record, not a log line.
#[derive(Debug)]
pub struct Handed {
    pub decision: Decision,
    pub code: Option<OneTimeCode>,
}

impl Handed {
    #[must_use]
    pub const fn said(decision: Decision) -> Self {
        Self {
            decision,
            code: None,
        }
    }
}

type HandoffAsker = dyn Fn(&Handoff) -> Handed + Send + Sync;
static HANDOFF_ASKER: OnceLock<Box<HandoffAsker>> = OnceLock::new();

pub fn install_handoff_asker(asker: Box<HandoffAsker>) {
    let _ = HANDOFF_ASKER.set(asker);
}

/// [`handoff`](hand_the_desk), with the reason in the page's own words: `key`
/// and `args` for the page, `reason` for a reader that has only the words.
/// The window's own hand-overs (a covered press) never ask for a code.
#[must_use]
pub fn handoff_said(reason: &str, key: &'static str, args: Value, timeout_ms: u64) -> Decision {
    hand_over(&Handoff {
        id: next_id(),
        reason: reason.to_string(),
        reason_key: Some(key),
        reason_args: args,
        timeout_ms,
        code_ask: None,
    })
    .decision
}

/// Give the desk to the person and wait for their word. With nobody to
/// hand to, the answer is a refusal — the work does not pretend it went on.
fn hand_over(ask: &Handoff) -> Handed {
    HANDOFF_ASKER
        .get()
        .map_or(Handed::said(Decision::Refused), |asker| asker(ask))
}

/// What a `handoff` command is answered with: the card, the wait, and what
/// comes back — one function for the plain turn and the card with a line for
/// a code, so a cancel and a silence read the same either way.
pub fn hand_the_desk(params: &Value) -> Result<Value, ComputerUseError> {
    hand_the_desk_through(params, &hand_over)
}

/// [`hand_the_desk`] with whoever asks the person handed in — the window's
/// asker in the app, a script in a test.
fn hand_the_desk_through(
    params: &Value,
    over: &dyn Fn(&Handoff) -> Handed,
) -> Result<Value, ComputerUseError> {
    let reason = params
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let timeout = handoff_ms(params.get("timeoutMs").and_then(Value::as_u64));
    let handed = over(&Handoff {
        id: next_id(),
        reason: reason.to_string(),
        reason_key: None,
        reason_args: Value::Null,
        timeout_ms: timeout,
        code_ask: None,
    });
    match handed.decision {
        Decision::Allowed => Ok(serde_json::json!({ "resumed": true, "reason": reason })),
        Decision::Refused => Err(ComputerUseError::new(
            error_code::CONFIRMATION_REFUSED,
            format!("the person cancelled the handoff ({reason})"),
        )),
        Decision::TimedOut => Err(ComputerUseError::new(
            error_code::CONFIRMATION_TIMEOUT,
            format!("nobody took over within {timeout} ms ({reason})"),
        )),
    }
}

/// What the page is told when a person's turn is over — who, and how it
/// ended. Never what was typed.
#[must_use]
pub fn closed_said(id: &str, decision: Decision) -> Value {
    serde_json::json!({ "id": id, "decision": decision })
}

#[must_use]
pub fn next_id() -> String {
    let mut held = NEXT_ID
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *held += 1;
    format!("confirm-{}-{}", std::process::id(), *held)
}

fn open_with(id: &str, takes_code: bool) -> mpsc::Receiver<Reply> {
    let (reply, receiver) = mpsc::channel();
    PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(id.to_string(), Waiting { reply, takes_code });
    receiver
}

/// Open a question: the receiver the asker waits on.
#[must_use]
pub fn open(id: &str) -> mpsc::Receiver<Reply> {
    open_with(id, false)
}

/// Open a person's turn: with a line for a code when the card offers one.
#[must_use]
pub fn open_handoff(card: &Handoff) -> mpsc::Receiver<Reply> {
    open_with(&card.id, false)
}

/// The person's answer to a question — false when no such question waits.
pub fn answer(id: &str, allow: bool) -> bool {
    let waiting = PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .and_then(|pending| pending.remove(id));
    waiting.is_some_and(|waiting| waiting.reply.send(Reply::Said(allow)).is_ok())
}

/// What became of a code the person typed on a card's line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeVerdict {
    /// The code is on its way to the agent that asked, once.
    Delivered,
    /// What was typed is not a code: the card stays, the person types again.
    Invalid,
    /// No card with a line waits under that id — answered, timed out, or a
    /// card that asks yes or no.
    Gone,
}

/// The person's code for a card that offers a line for one.
pub fn answer_code(id: &str, typed: &str) -> CodeVerdict {
    let _ = (id, typed);
    CodeVerdict::Gone
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
pub fn wait(receiver: &mpsc::Receiver<Reply>, timeout: Duration) -> Decision {
    wait_handed(receiver, timeout).decision
}

/// [`wait`] for a person's turn: what they said, and the code they typed
/// when the card had a line for one.
#[must_use]
pub fn wait_handed(receiver: &mpsc::Receiver<Reply>, timeout: Duration) -> Handed {
    match receiver.recv_timeout(timeout) {
        Ok(Reply::Said(true)) => Handed::said(Decision::Allowed),
        Ok(Reply::Said(false)) => Handed::said(Decision::Refused),
        Ok(Reply::Code(code)) => Handed {
            decision: Decision::Allowed,
            code: Some(code),
        },
        Err(_) => Handed::said(Decision::TimedOut),
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

    /// A question that closes itself when its test ends — passed or failed —
    /// so one that fails never leaves the person "being asked" for the tests
    /// that run after it.
    struct Open(String);

    impl Drop for Open {
        fn drop(&mut self) {
            close(&self.0);
        }
    }

    const PERSONS_TURN_FOR_A_CODE: &str = "카카오톡 인증번호";
    const A_CODE: &str = "493021";
    const SOON: Duration = Duration::from_millis(50);

    fn one_hand() -> std::sync::MutexGuard<'static, ()> {
        crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn card(id: &str, line: bool) -> Handoff {
        Handoff {
            id: id.to_string(),
            reason: PERSONS_TURN_FOR_A_CODE.to_string(),
            reason_key: None,
            reason_args: Value::Null,
            timeout_ms: 1_000,
            code_ask: line.then_some(CodeLimits::TABLE),
        }
    }

    /// A card with a line takes the code the person types, once, and the
    /// agent that asked gets it as the answer. A code is no yes either: it
    /// never answers a card that asks yes or no.
    #[test]
    fn a_code_card_takes_one_code_and_then_it_is_gone() {
        let _hand = one_hand();
        let id = next_id();
        let _open = Open(id.clone());
        let receiver = open_handoff(&card(&id, true));
        assert!(
            asking(),
            "a card with a line is the person's attention like any other"
        );
        assert_eq!(answer_code(&id, "493 021"), CodeVerdict::Delivered);
        assert!(!asking(), "a delivered code closes its card");
        let handed = wait_handed(&receiver, SOON);
        assert_eq!(handed.decision, Decision::Allowed);
        assert_eq!(
            handed.code.as_ref().map(OneTimeCode::reveal),
            Some(A_CODE),
            "the code arrives as the person wrote it, groups dropped"
        );
        assert_eq!(
            answer_code(&id, A_CODE),
            CodeVerdict::Gone,
            "a code goes once"
        );

        let asked_yes_or_no = next_id();
        let _yes_or_no = Open(asked_yes_or_no.clone());
        let _waiting = open(&asked_yes_or_no);
        assert_eq!(
            answer_code(&asked_yes_or_no, A_CODE),
            CodeVerdict::Gone,
            "a code is not a yes"
        );
        assert!(
            asking(),
            "the card that asks yes or no is still waiting for its person"
        );
    }

    /// What is typed that is not a code leaves the card standing, so the
    /// person types again; nothing is refused for good.
    #[test]
    fn a_wrong_code_leaves_the_card_open_and_the_next_one_is_taken() {
        let _hand = one_hand();
        let id = next_id();
        let _open = Open(id.clone());
        let receiver = open_handoff(&card(&id, true));
        for typed in ["", "12", "12!456", "12345678901"] {
            assert_eq!(
                answer_code(&id, typed),
                CodeVerdict::Invalid,
                "{typed:?} is not a code"
            );
            assert!(asking(), "a wrong code does not close the card: {typed:?}");
        }
        assert_eq!(answer_code(&id, A_CODE), CodeVerdict::Delivered);
        assert_eq!(
            wait_handed(&receiver, SOON).code.map(|code| code.chars()),
            Some(6)
        );
    }

    /// A bare "done" is the person saying they did a thing on their own
    /// device; on a card that asked for a code it says nothing the agent
    /// can use, so only a code or a cancel closes it.
    #[test]
    fn a_bare_done_cannot_stand_in_for_a_code_but_a_cancel_can() {
        let _hand = one_hand();
        let id = next_id();
        let _open = Open(id.clone());
        let receiver = open_handoff(&card(&id, true));
        assert!(!answer(&id, true), "'done' is not a code");
        assert!(asking(), "a refused 'done' leaves the card standing");
        assert!(answer(&id, false), "a cancel closes a card with a line");
        assert_eq!(wait_handed(&receiver, SOON).decision, Decision::Refused);
        assert!(!asking());
    }

    /// Silence is a timeout with nothing in its hand; the card is forgotten
    /// and takes no late code.
    #[test]
    fn silence_on_a_code_card_is_a_timeout_with_no_code() {
        let _hand = one_hand();
        let id = next_id();
        let _open = Open(id.clone());
        let receiver = open_handoff(&card(&id, true));
        let handed = wait_handed(&receiver, Duration::from_millis(10));
        assert_eq!(handed.decision, Decision::TimedOut);
        assert!(handed.code.is_none(), "silence carries no code");
        close(&id);
        assert_eq!(
            answer_code(&id, A_CODE),
            CodeVerdict::Gone,
            "a card that timed out takes no late code"
        );
    }

    /// What the page is told: the limits it may show, from the window's one
    /// table, on the card that has a line and on no other.
    #[test]
    fn what_the_page_is_told_carries_the_limits_and_never_a_value() {
        let with_line = serde_json::to_value(card("confirm-1-1", true)).expect("a card is data");
        assert_eq!(
            with_line["codeAsk"],
            serde_json::json!({ "min": 4, "max": 10, "typedMax": 20 })
        );
        let plain = serde_json::to_value(card("confirm-1-2", false)).expect("a card is data");
        assert!(
            plain.get("codeAsk").is_none(),
            "the plain card has no line: {plain}"
        );
        assert!(
            !with_line.to_string().contains(A_CODE),
            "no value rides on a card"
        );
    }

    /// The page hears who and how it ended, and nothing a person typed.
    #[test]
    fn the_closing_notice_names_the_card_and_how_it_ended_and_nothing_else() {
        assert_eq!(
            closed_said("confirm-1-1", Decision::Allowed),
            serde_json::json!({ "id": "confirm-1-1", "decision": "allowed" })
        );
    }

    fn asked_with(
        params: &Value,
        answer: impl Fn(&Handoff) -> Handed,
    ) -> (Result<Value, ComputerUseError>, Vec<Handoff>) {
        let seen = Mutex::new(Vec::new());
        let result = hand_the_desk_through(params, &|card| {
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(card.clone());
            answer(card)
        });
        let seen = seen
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (result, seen)
    }

    /// The line is offered when an agent asks for it and the reason does not
    /// name a secret; every other turn is the plain card it always was.
    #[test]
    fn a_reason_naming_a_secret_gets_the_card_without_the_line() {
        let mut shown = Vec::new();
        for (reason, asks) in [
            ("카카오톡 인증번호", true),
            ("일회용 비밀번호(OTP)", true),
            ("비밀번호", true),
            ("the card number", true),
            ("CVC", true),
            ("카카오톡 인증번호", false),
        ] {
            let params = if asks {
                serde_json::json!({ "reason": reason, "askCode": true })
            } else {
                serde_json::json!({ "reason": reason })
            };
            let (answered, cards) = asked_with(&params, |_| Handed::said(Decision::Allowed));
            assert!(answered.is_ok(), "{reason}: {answered:?}");
            shown.push((
                reason,
                asks,
                cards.iter().all(|card| card.code_ask.is_some()),
            ));
        }
        assert_eq!(
            shown,
            [
                ("카카오톡 인증번호", true, true),
                ("일회용 비밀번호(OTP)", true, true),
                ("비밀번호", true, false),
                ("the card number", true, false),
                ("CVC", true, false),
                ("카카오톡 인증번호", false, false),
            ],
            "(reason, asked for a code, the card had a line)"
        );
    }

    fn handed_a_code(_card: &Handoff) -> Handed {
        Handed {
            decision: Decision::Allowed,
            code: OneTimeCode::parse("493 021").ok(),
        }
    }

    /// The code is the answer of the handoff that asked for it — once, with
    /// its length — and an agent that did not ask is never given one.
    #[test]
    fn a_code_comes_back_in_the_answer_it_was_asked_for_and_in_no_other() {
        let asked = serde_json::json!({ "reason": PERSONS_TURN_FOR_A_CODE, "askCode": true });
        let (answered, _) = asked_with(&asked, handed_a_code);
        assert_eq!(
            answered.ok(),
            Some(serde_json::json!({
                "resumed": true,
                "reason": PERSONS_TURN_FOR_A_CODE,
                "code": A_CODE,
                "codeLength": 6,
            }))
        );
        let plain = serde_json::json!({ "reason": PERSONS_TURN_FOR_A_CODE });
        let (answered, _) = asked_with(&plain, handed_a_code);
        assert_eq!(
            answered.ok(),
            Some(serde_json::json!({ "resumed": true, "reason": PERSONS_TURN_FOR_A_CODE })),
            "a turn that asked for no code hands none over"
        );
    }

    /// A reason that names a secret got the plain card; the agent is told no
    /// code was taken and why, in the shape of the plain answer.
    #[test]
    fn an_agent_that_asked_for_a_secret_is_told_no_code_was_taken() {
        let asked = serde_json::json!({ "reason": "비밀번호를 입력", "askCode": true });
        let (answered, _) = asked_with(&asked, |_| Handed::said(Decision::Allowed));
        assert_eq!(
            answered.ok(),
            Some(serde_json::json!({
                "resumed": true,
                "reason": "비밀번호를 입력",
                "code": null,
                "codeRefused": "secret_reason",
            }))
        );
    }

    /// A cancel and a silence read the same whether or not the card had a
    /// line: the codes and the messages of the plain turn.
    #[test]
    fn a_card_with_a_line_answers_a_cancel_and_a_silence_as_the_plain_card_does() {
        for decision in [Decision::Refused, Decision::TimedOut] {
            let said = |params: Value| {
                let (answered, _) = asked_with(&params, |_| Handed::said(decision));
                answered.err().map(|error| (error.code, error.message))
            };
            let plain = said(serde_json::json!({ "reason": "2FA", "timeoutMs": 180_000 }));
            let coded =
                said(serde_json::json!({ "reason": "2FA", "timeoutMs": 180_000, "askCode": true }));
            assert!(
                plain.is_some(),
                "{decision:?} is an error for the plain card"
            );
            assert_eq!(coded, plain, "{decision:?}");
        }
    }
}
