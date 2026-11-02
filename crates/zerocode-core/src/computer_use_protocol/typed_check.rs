//! Whether what keys typed shows on the screen (t-89036).
//!
//! A window with no tree to read a field from — a phone mirrored on the Mac, a
//! remote desktop — takes keys and answers `synthetic_input`: posted, and read
//! back by nobody. Its pixels still show what landed, so the window reads them
//! (`readText --ocr`) and answers one of three words:
//!
//! - `match`: the screen shows the value somewhere. It may have shown it
//!   before the keys, when no press read the screen first, so this is no proof
//!   they landed.
//! - `mismatch`: the screen reads, and nothing on it comes near the value: keys
//!   in the wrong alphabet, or keys that went elsewhere. A walk stops at that
//!   step, and the answer names where the two part, never either's text.
//!   Nothing is typed again: the keys did go somewhere.
//! - `unread`: nobody can say. A walk goes on.
//!
//! How often OCR misreads a real phone's screen has not been measured, and
//! stopping a good walk costs more than missing a bad one, so what is only
//! close to the value says `unread`, not `mismatch`. The numbers below are this
//! side of that measurement.

use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

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
/// The same for a value of digits alone: a phone's keyboard shows a `123` key.
const SHORTEST_DIGITS: usize = 4;
/// A value shorter than this is not pieced together from two lines: two lines
/// of a screen spell a few letters by chance.
const SHORTEST_WRAPPED: usize = 8;
/// A value longer than this may not fit its field: the field scrolls to the
/// caret and shows the value's tail, or has not scrolled and shows its head.
const LONGEST_SHOWN_WHOLE: usize = 12;
/// How much of a long value's head or tail says the field is showing it.
const SHOWN_END: usize = 6;
/// A value longer than this fits no screen, and is not measured against one.
const LONGEST_MEASURED: usize = 512;
/// How much of a value, in percent, may differ and still be only close: an OCR
/// slip, or a screen that has not caught up.
const CLOSE_PERCENT: usize = 50;
/// This many of these in a row are a field masking what it is given.
const MASK_RUN: usize = 3;
const MASK_CHARS: &str = "•●∙·*";

/// What a phone's own keyboard writes for what it was given, each folded to one
/// character: the quotes and dashes of its smart punctuation.
const WRITES_ALIKE: &[(&str, char)] = &[("'‘’`", '\''), ("\"“”", '"'), ("-–—", '-')];
/// What OCR reads for one another, each folded to one character, after case.
const READS_ALIKE: &[(&str, char)] = &[("1li|", 'l'), ("0o", 'o')];
/// One character a phone's keyboard writes for three.
const ELLIPSIS: char = '…';

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
    /// The Mac's screen-recording permission keeps its pixels from the window.
    NoPermission,
    /// The window read, and held no text.
    NoText,
    /// Too few letters and digits to tell from chance.
    TooShort,
    /// It holds what OCR does not read back as written: an emoji, a combining
    /// mark, loose jamo.
    NotPlainText,
    /// A Tab or a line break carried the rest of it to another field.
    MovesFocus,
    /// Only one end of a long value is on the screen.
    RunsOutOfView,
    /// A secret is not compared.
    Secret,
    /// What the screen reads is near the value: OCR's slip or a few keys'.
    Close,
    /// The value stands twice in a row on one line.
    ShownTwice,
    /// The screen showed the value before the keys went.
    AlreadyShown,
    /// The window has a tree of its own: its pixels are not read for keys.
    HasTree,
}

impl UnreadWhy {
    /// The word the answer says it by.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadFailed => "read_failed",
            Self::NoPermission => "no_permission",
            Self::NoText => "no_text",
            Self::TooShort => "too_short",
            Self::NotPlainText => "not_plain_text",
            Self::MovesFocus => "moves_focus",
            Self::RunsOutOfView => "runs_out_of_view",
            Self::Secret => "secret",
            Self::Close => "close",
            Self::ShownTwice => "shown_twice",
            Self::AlreadyShown => "already_shown",
            Self::HasTree => "has_tree",
        }
    }
}

/// What a check says. `at` and `of` count characters of the value in its
/// composed form, from one: never the value or what the screen read.
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

fn one_of(table: &[(&str, char)], c: char) -> Option<char> {
    table
        .iter()
        .find(|(alike, _)| alike.contains(c))
        .map(|(_, one)| *one)
}

/// A text's characters as the comparison reads them, each with its place in
/// the composed text: one normal form, no case, none of the spaces OCR drops or
/// the lines a value wraps over, the keyboard's punctuation as what it was
/// given, and with `alike` what OCR reads for one another as one.
fn comparable(text: &str, alike: bool) -> Vec<(usize, char)> {
    let mut read = Vec::new();
    for (at, c) in text.nfc().enumerate() {
        if c.is_whitespace() {
            continue;
        }
        if c == ELLIPSIS {
            read.extend([(at, '.'); 3]);
            continue;
        }
        for lower in c.to_lowercase() {
            let written = one_of(WRITES_ALIKE, lower).unwrap_or(lower);
            let folded = if alike {
                one_of(READS_ALIKE, written).unwrap_or(written)
            } else {
                written
            };
            read.push((at, folded));
        }
    }
    read
}

fn chars_of(text: &str, alike: bool) -> Vec<char> {
    comparable(text, alike)
        .into_iter()
        .map(|(_, c)| c)
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
            || c == ELLIPSIS
            || one_of(WRITES_ALIKE, c).is_some()
    };
    if !text.nfc().all(plain) {
        return Some(UnreadWhy::NotPlainText);
    }
    let shortest = if text.chars().any(char::is_alphabetic) {
        SHORTEST_READABLE
    } else {
        SHORTEST_DIGITS
    };
    if text.nfc().filter(|c| c.is_alphanumeric()).count() < shortest {
        return Some(UnreadWhy::TooShort);
    }
    None
}

/// Whether a screen's words or its dots say a field there masks what it is given.
#[must_use]
pub fn shows_a_secret_field(read: &[ReadLine]) -> bool {
    read.iter().any(|line| {
        let lower = line.text.to_lowercase();
        let masked = lower
            .chars()
            .scan(0, |run, c| {
                *run = if MASK_CHARS.contains(c) { *run + 1 } else { 0 };
                Some(*run)
            })
            .any(|run| run >= MASK_RUN);
        masked
            || looks_like_secure_text(&lower)
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

/// The fewest characters to change for `wanted` to stand anywhere in `seen`
/// (Sellers' edit distance: a start anywhere costs nothing).
fn fewest_changes(wanted: &[char], seen: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=wanted.len()).collect();
    let mut best = wanted.len();
    for &c in seen {
        let mut diagonal = 0;
        for (at, &w) in wanted.iter().enumerate() {
            let above = row[at + 1];
            row[at + 1] = (diagonal + usize::from(w != c))
                .min(above + 1)
                .min(row[at] + 1);
            diagonal = above;
        }
        best = best.min(row[wanted.len()]);
    }
    best
}

/// Where a value stands among a screen's lines.
enum Shown {
    TwiceInARow,
    OnOneLine,
    OverLines,
    Nowhere,
}

fn holds(line: &[char], wanted: &[char]) -> bool {
    line.windows(wanted.len()).any(|at| at == wanted)
}

fn shown_among(wanted: &[char], lines: &[Vec<char>]) -> Shown {
    let twice = [wanted, wanted].concat();
    if lines.iter().any(|line| holds(line, &twice)) {
        Shown::TwiceInARow
    } else if lines.iter().any(|line| holds(line, wanted)) {
        Shown::OnOneLine
    } else if holds(&lines.concat(), wanted) {
        Shown::OverLines
    } else {
        Shown::Nowhere
    }
}

/// `text`, typed, against the lines a window reads, in reading order. A long
/// value wrapped over lines reads whole, and a field's suggestion after it does
/// not count against it.
#[must_use]
pub fn judge(text: &str, read: &[ReadLine]) -> TypedCheck {
    if shows_a_secret_field(read) {
        return TypedCheck::Unread(UnreadWhy::Secret);
    }
    let lines_of = |alike: bool| -> Vec<Vec<char>> {
        read.iter()
            .map(|line| chars_of(&line.text, alike))
            .collect()
    };
    let lines = lines_of(true);
    let seen = lines.concat();
    if seen.is_empty() {
        return TypedCheck::Unread(UnreadWhy::NoText);
    }
    let given = comparable(text, true);
    let wanted: Vec<char> = given.iter().map(|(_, c)| *c).collect();
    if wanted.is_empty() {
        return TypedCheck::Unread(UnreadWhy::TooShort);
    }
    // A value of digits is on the screen only as digits: `100` is not `Look`.
    let shown = if text.chars().any(char::is_alphabetic) {
        shown_among(&wanted, &lines)
    } else {
        shown_among(&chars_of(text, false), &lines_of(false))
    };
    match shown {
        Shown::OnOneLine => return TypedCheck::Match,
        Shown::OverLines if wanted.len() >= SHORTEST_WRAPPED => return TypedCheck::Match,
        Shown::TwiceInARow => return TypedCheck::Unread(UnreadWhy::ShownTwice),
        Shown::OverLines => return TypedCheck::Unread(UnreadWhy::Close),
        Shown::Nowhere => {}
    }
    let from_the_start = longest_shown(wanted.iter(), seen.iter());
    let one_end_shown = from_the_start >= SHOWN_END
        || longest_shown(wanted.iter().rev(), seen.iter().rev()) >= SHOWN_END;
    if wanted.len() > LONGEST_MEASURED || (wanted.len() > LONGEST_SHOWN_WHOLE && one_end_shown) {
        return TypedCheck::Unread(UnreadWhy::RunsOutOfView);
    }
    if fewest_changes(&wanted, &seen) * 100 <= wanted.len() * CLOSE_PERCENT {
        return TypedCheck::Unread(UnreadWhy::Close);
    }
    TypedCheck::Mismatch {
        at: given.get(from_the_start).map_or(0, |(at, _)| *at) + 1,
        of: text.nfc().count(),
    }
}

/// The check over a screen that takes its time: reads through `read` until the
/// screen shows the value or the budget passes. `read` answers why not when the
/// window could not be read; `pause` waits. `before` is what the screen read
/// before the keys, when something read it: a value it held is no proof. A
/// `mismatch` is said only when no reading in the budget could not tell.
pub fn read_back(
    text: &str,
    before: &[ReadLine],
    read: &mut dyn FnMut() -> Result<Value, UnreadWhy>,
    pause: &mut dyn FnMut(u64),
) -> TypedCheck {
    let was_shown = judge(text, before) == TypedCheck::Match;
    let mut could_not_tell = None;
    let mut waited = 0;
    loop {
        let answer = match read() {
            Ok(answer) => answer,
            Err(why) => return TypedCheck::Unread(why),
        };
        let check = judge(text, &words::lines(&answer));
        match check {
            TypedCheck::Match if was_shown => {
                return TypedCheck::Unread(UnreadWhy::AlreadyShown);
            }
            TypedCheck::Match | TypedCheck::Unread(UnreadWhy::Secret | UnreadWhy::ShownTwice) => {
                return check;
            }
            TypedCheck::Unread(_) => could_not_tell = Some(check),
            TypedCheck::Mismatch { .. } => {}
        }
        if waited >= READ_BACK_BUDGET_MS {
            return could_not_tell.unwrap_or(check);
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
