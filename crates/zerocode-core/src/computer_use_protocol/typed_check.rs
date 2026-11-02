//! Whether what keys typed shows on the screen (t-89036).
//!
//! A window with no tree to read a field from — a phone mirrored on the Mac, a
//! remote desktop — takes keys and answers `synthetic_input`: posted, and read
//! back by nobody. Its pixels still show what landed, so the window reads them
//! (`readText --ocr`) and answers one of three words:
//!
//! - `match`: the screen shows the value somewhere. It may have shown it
//!   before the keys, so this is no proof they landed.
//! - `mismatch`: the screen reads and the value is not on it. A walk stops at
//!   that step, and the answer names where the two part, never either's text.
//!   Nothing is typed again: the keys did go somewhere.
//! - `unread`: nobody can say — the reading failed, the value is of a kind OCR
//!   does not read back faithfully, or it is a secret, which is not compared.
//!   A walk goes on.
//!
//! OCR's own confusions are not the keys' mistakes, so the comparison folds
//! what reads alike ([`READS_ALIKE`]) before it says the two differ.

use serde_json::{Value, json};

use super::render::looks_like_secure_text;
use super::unverified_reason;
use super::validate::FOCUS_MOVING_CHARS;
use super::words::{self, ReadLine};
use crate::credential::{looks_like_a_credential, looks_like_a_payment_card};

/// The key an action's answer carries the check under.
pub const INPUT_CHECK_KEY: &str = "inputCheck";
/// How long the screen gets to show what was typed: a mirrored phone's keys
/// cross a network before its picture changes back.
pub const READ_BACK_BUDGET_MS: u64 = 1_500;
/// How long between two readings of the screen.
pub const READ_BACK_POLL_MS: u64 = 300;

const VERDICT: &str = "verdict";
const MISMATCH: &str = "mismatch";

/// Letters and digits fewer than this match by chance anywhere on a screen,
/// and OCR drops a lone digit in a cell of its own.
const SHORTEST_READABLE: usize = 3;
/// A value longer than this may not fit its field: the field scrolls to the
/// caret and shows the value's tail.
const LONGEST_SHOWN_WHOLE: usize = 24;
/// How much of a long value's tail says the field is showing it.
const SHOWN_TAIL: usize = 6;

/// What OCR and a phone's own keyboard write for one another, each folded to
/// one character. Case is folded first. `1`·`l`·`I` and `O`·`0` are OCR's;
/// the quotes and dashes are the phone's smart punctuation.
const READS_ALIKE: &[(&str, char)] = &[
    ("1li|", 'l'),
    ("0o", 'o'),
    ("'‘’`", '\''),
    ("\"“”", '"'),
    ("-–—", '-'),
];

/// Words on a screen that say its field holds a secret, besides the ones a
/// tree's field is judged by ([`looks_like_secure_text`]): a mirrored phone
/// shows them as pixels, in the phone's language.
const SECRET_SCREEN_WORDS: &[&str] = &[
    "card number",
    "cvc",
    "cvv",
    "비밀번호",
    "암호",
    "인증번호",
    "인증 번호",
    "보안 코드",
    "보안코드",
    "카드 번호",
    "카드번호",
];

/// Why a check could not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreadWhy {
    /// The window could not be read.
    ReadFailed,
    /// The window read, and held no text.
    NoText,
    /// Too few letters and digits to tell from chance.
    TooShort,
    /// It holds what OCR does not read back as written: an emoji, a combining
    /// mark, loose jamo.
    NotPlainText,
    /// A Tab or a line break carried the rest of it to another field.
    MovesFocus,
    /// Only the tail of a long value is on the screen.
    RunsOutOfView,
    /// A secret is not compared.
    Secret,
}

impl UnreadWhy {
    /// The word the answer says it by.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadFailed => "read_failed",
            Self::NoText => "no_text",
            Self::TooShort => "too_short",
            Self::NotPlainText => "not_plain_text",
            Self::MovesFocus => "moves_focus",
            Self::RunsOutOfView => "runs_out_of_view",
            Self::Secret => "secret",
        }
    }
}

/// What a check says. `at` and `of` count characters of the value as it was
/// given, from one: never the value or what the screen read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedCheck {
    Match,
    Mismatch { at: usize, of: usize },
    Unread(UnreadWhy),
}

impl TypedCheck {
    /// The check as an answer carries it.
    #[must_use]
    pub fn answer(self) -> Value {
        match self {
            Self::Match => json!({ VERDICT: "match" }),
            Self::Mismatch { at, of } => json!({ VERDICT: MISMATCH, "at": at, "of": of }),
            Self::Unread(why) => json!({ VERDICT: "unread", "why": why.as_str() }),
        }
    }
}

fn folded(c: char) -> char {
    let lower = c.to_lowercase().next().unwrap_or(c);
    READS_ALIKE
        .iter()
        .find(|(alike, _)| alike.contains(lower))
        .map_or(lower, |(_, one)| *one)
}

/// A text's characters as the comparison reads them, each with its place in
/// the text: OCR neither keeps the spaces nor the lines a value wraps over.
fn comparable(text: &str) -> Vec<(usize, char)> {
    text.chars()
        .enumerate()
        .filter(|(_, c)| !c.is_whitespace())
        .map(|(at, c)| (at, folded(c)))
        .collect()
}

/// Whether a helper's answer to typed text leaves the check to the screen: it
/// posted the keys, and nothing read them back. A field it could read has its
/// own verification, which needs no picture.
#[must_use]
pub fn asks_a_look(answer: &Value) -> bool {
    answer
        .pointer("/action/verification/reason")
        .and_then(Value::as_str)
        == Some(unverified_reason::SYNTHETIC_INPUT)
}

/// What is known before any reading: `None` when the screen may be read for
/// `text`. The helper's word that the keyboard's secure mode is on, and a
/// value that is itself a secret by its kind, are never compared.
#[must_use]
pub fn before_reading(text: &str, answer: &Value) -> Option<UnreadWhy> {
    let secure_mode = answer.pointer("/action/secureInput").is_some();
    if secure_mode
        || looks_like_a_payment_card(text)
        || text.split_whitespace().any(looks_like_a_credential)
    {
        return Some(UnreadWhy::Secret);
    }
    if text.contains(FOCUS_MOVING_CHARS) {
        return Some(UnreadWhy::MovesFocus);
    }
    let plain = |c: char| {
        c.is_whitespace()
            || c.is_ascii_punctuation()
            || (c.is_alphanumeric() && !('\u{1100}'..='\u{11ff}').contains(&c))
            || READS_ALIKE.iter().any(|(alike, _)| alike.contains(c))
    };
    if !text.chars().all(plain) {
        return Some(UnreadWhy::NotPlainText);
    }
    if text.chars().filter(|c| c.is_alphanumeric()).count() < SHORTEST_READABLE {
        return Some(UnreadWhy::TooShort);
    }
    None
}

fn shows_a_secret_field(read: &[ReadLine]) -> bool {
    read.iter().any(|line| {
        let lower = line.text.to_lowercase();
        looks_like_secure_text(&lower)
            || SECRET_SCREEN_WORDS.iter().any(|word| lower.contains(word))
    })
}

/// How many of `wanted`'s characters `seen` holds in a row from some start.
fn longest_shown<'a>(
    wanted: impl Iterator<Item = &'a char> + Clone,
    seen: impl Iterator<Item = &'a char> + Clone,
) -> usize {
    let mut seen = seen;
    let mut best = 0;
    loop {
        let run = wanted
            .clone()
            .zip(seen.clone())
            .take_while(|(a, b)| a == b)
            .count();
        best = best.max(run);
        if seen.next().is_none() {
            return best;
        }
    }
}

/// `text`, typed, against the lines a window reads, in reading order. A value
/// wrapped over lines reads whole, and a field's suggestion after it does not
/// count against it.
#[must_use]
pub fn judge(text: &str, read: &[ReadLine]) -> TypedCheck {
    if shows_a_secret_field(read) {
        return TypedCheck::Unread(UnreadWhy::Secret);
    }
    let seen: Vec<char> = read
        .iter()
        .flat_map(|line| comparable(&line.text))
        .map(|(_, c)| c)
        .collect();
    if seen.is_empty() {
        return TypedCheck::Unread(UnreadWhy::NoText);
    }
    let given = comparable(text);
    let wanted: Vec<char> = given.iter().map(|(_, c)| *c).collect();
    let from_the_start = longest_shown(wanted.iter(), seen.iter());
    if from_the_start == wanted.len() {
        return TypedCheck::Match;
    }
    if wanted.len() > LONGEST_SHOWN_WHOLE
        && from_the_start < SHOWN_TAIL
        && longest_shown(wanted.iter().rev(), seen.iter().rev()) >= SHOWN_TAIL
    {
        return TypedCheck::Unread(UnreadWhy::RunsOutOfView);
    }
    TypedCheck::Mismatch {
        at: given.get(from_the_start).map_or(0, |(at, _)| *at) + 1,
        of: text.chars().count(),
    }
}

/// The check over a screen that takes its time: reads through `read` until the
/// screen shows the value or the budget passes, and answers the last reading.
/// `read` answers `None` when the window could not be read; `pause` waits.
pub fn read_back(
    text: &str,
    read: &mut dyn FnMut() -> Option<Value>,
    pause: &mut dyn FnMut(u64),
) -> TypedCheck {
    let mut waited = 0;
    loop {
        let Some(answer) = read() else {
            return TypedCheck::Unread(UnreadWhy::ReadFailed);
        };
        let check = judge(text, &words::lines(&answer));
        let settled = matches!(
            check,
            TypedCheck::Match | TypedCheck::Unread(UnreadWhy::Secret)
        );
        if settled || waited >= READ_BACK_BUDGET_MS {
            return check;
        }
        pause(READ_BACK_POLL_MS);
        waited += READ_BACK_POLL_MS;
    }
}

/// What a walk says when a step's check found the screen reading otherwise:
/// where, and that nothing was typed again. `None` for any other answer.
#[must_use]
pub fn stop_sentence(result: &Value) -> Option<String> {
    let check = result.get(INPUT_CHECK_KEY)?;
    if check.get(VERDICT).and_then(Value::as_str) != Some(MISMATCH) {
        return None;
    }
    Some(format!(
        "the screen does not read what was typed: they part at character {} of {}. The keys went in and nothing was typed again — look at the field before going on",
        check["at"], check["of"]
    ))
}

#[cfg(test)]
mod tests;
