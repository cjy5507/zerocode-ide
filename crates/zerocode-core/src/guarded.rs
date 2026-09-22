//! The controls a press cannot take back, known by the words their labels
//! carry — one table, and every reader of it here (t-6187).
//!
//! Two readers ask it:
//!
//! - The Computer Use confirmation gate (§1.5): before a payment, a transfer
//!   or a delete is pressed, a person is asked. It reads the first three rows
//!   ([`ConfirmKind`]), and the helper matches a control's label against the
//!   words the window hands it.
//! - A walk by judgment (`computer_use::errand`): a destructive control is
//!   held to a higher confidence floor than a plain one
//!   ([`crate::jev::SCREEN_DESTRUCTIVE_PRESS_FLOOR_PERMILLE`]). It reads every
//!   row ([`kind_of`]), the commit row included — a send, a submit or a
//!   confirmation pressed on a guess is as final as a payment, and the gate,
//!   which asks a person, does not ask about those today.
//!
//! The rows were the confirmation gate's own lists until this module; they
//! moved here whole so a walk and the gate read one table, and a word added
//! for one is a word the other sees.

use serde::{Deserialize, Serialize};

/// Words a control carries when pressing it pays, in the five languages the
/// window speaks.
pub const PAYMENT_WORDS: &[&str] = &[
    "결제",
    "구매",
    "주문",
    "pay",
    "buy",
    "purchase",
    "place order",
    "checkout",
    "支払",
    "購入",
    "注文",
    "支付",
    "购买",
    "下单",
    "pagar",
    "comprar",
];

/// Words a control carries when pressing it moves money to someone.
pub const TRANSFER_WORDS: &[&str] = &[
    "이체",
    "송금",
    "transfer",
    "send money",
    "wire",
    "振込",
    "送金",
    "转账",
    "汇款",
    "transferir",
    "enviar dinero",
];

/// Words a control carries when pressing it makes something vanish.
pub const DELETE_WORDS: &[&str] = &[
    "삭제", "제거", "delete", "remove", "erase", "削除", "删除", "eliminar", "borrar",
];

/// Words a control carries when pressing it sends, submits or settles
/// something that does not come back — a message sent, a form submitted, an
/// order confirmed. A walk holds these to the destructive floor; the
/// confirmation gate does not ask a person about them.
pub const COMMIT_WORDS: &[&str] = &[
    "전송",
    "보내기",
    "제출",
    "확정",
    "send",
    "submit",
    "confirm",
    "送信",
    "提出",
    "確定",
    "发送",
    "提交",
    "确认",
    "enviar",
    "confirmar",
];

/// The last step where money moves or things vanish (§1.5): the kinds a
/// person confirms by default, each with the words a control carries in the
/// five languages the window speaks. The helper matches labels against
/// these; the window asks; the person presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfirmKind {
    Payment,
    Transfer,
    Delete,
}

impl ConfirmKind {
    pub const ALL: [Self; 3] = [Self::Payment, Self::Transfer, Self::Delete];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Payment => "payment",
            Self::Transfer => "transfer",
            Self::Delete => "delete",
        }
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == word.trim().to_lowercase())
    }

    #[must_use]
    pub const fn words(self) -> &'static [&'static str] {
        match self {
            Self::Payment => PAYMENT_WORDS,
            Self::Transfer => TRANSFER_WORDS,
            Self::Delete => DELETE_WORDS,
        }
    }
}

/// Which kind a control's label answers to, if any — the table's order is
/// the tie-break, the match is a case-insensitive fragment.
#[must_use]
pub fn confirm_kind_of(label: &str) -> Option<ConfirmKind> {
    let shown = label.to_lowercase();
    ConfirmKind::ALL
        .into_iter()
        .find(|kind| kind.words().iter().any(|word| shown.contains(word)))
}

/// Whether pressing a control can be taken back, as a walk reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    /// It pays, moves money, deletes, sends, submits or settles something.
    Destructive,
    /// Anything else: the next press can undo it, or it undoes nothing.
    Plain,
}

impl ControlKind {
    /// The word a row names this kind by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Destructive => "destructive",
            Self::Plain => "plain",
        }
    }
}

/// The kind of the control `legend` describes — a label, or the legend line
/// a screen question offers it under — by the same rule the confirmation
/// gate matches with: any row's word as a case-insensitive fragment. Every
/// row counts here, the commit row included.
#[must_use]
pub fn kind_of(legend: &str) -> ControlKind {
    let shown = legend.to_lowercase();
    let destructive = [PAYMENT_WORDS, TRANSFER_WORDS, DELETE_WORDS, COMMIT_WORDS]
        .into_iter()
        .flatten()
        .any(|word| shown.contains(word));
    if destructive {
        ControlKind::Destructive
    } else {
        ControlKind::Plain
    }
}

#[cfg(test)]
mod tests;
