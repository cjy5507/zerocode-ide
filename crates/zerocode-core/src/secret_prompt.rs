//! The secret a pane is waiting for, read off the line its cursor stands on
//! (herdr 3, t-26596).
//!
//! A program that asks for a password, a passphrase or a PIN — `sudo`, `ssh`,
//! `gpg`, `passwd`, a PIN prompt, or the same question in another language —
//! draws its question on the line the cursor stands on and then waits. This
//! module reads that one line and says which secret it asks for. It never sees
//! what the person types, and nothing it returns carries a value.
//!
//! The read is narrow on purpose, because a false answer puts a secret box in
//! front of a person who is reading a log:
//!
//! - the cursor must stand on the pane's last non-blank row, at the end of the
//!   question. A question followed by more output is not waiting, and neither
//!   is a file whose last line ends in `password:` with the cursor below it;
//! - the line must end with a colon, and what comes before the colon must be a
//!   form the tables below name: a bare secret word (after at most one
//!   qualifier such as `Enter` or `New`), `[sudo] password for <user>`,
//!   `Password for '<target>'`, `<user>@<host>'s password`, or
//!   `Enter passphrase for <key>`.
//!
//! The window's sentence for each kind is its own translation table; this file
//! only says which kind a question asks for.

use serde::{Deserialize, Serialize};

/// The most characters a line may have and still be read as a question. A
/// longer line is someone's text that happens to end in a colon.
pub const PROMPT_MAX_CHARS: usize = 160;

/// The most bytes a typed secret may have: the field's room, so a value the
/// field could not hold is refused before it reaches a pane.
pub const SECRET_MAX_BYTES: usize = 1024;

/// The most characters a `sudo` user name may have.
const USER_NAME_MAX_CHARS: usize = 32;

/// The kind of secret a question asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretKind {
    Password,
    Passphrase,
    Pin,
}

impl SecretKind {
    /// The word the window keys its sentence by (`secret.title.<word>`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Passphrase => "passphrase",
            Self::Pin => "pin",
        }
    }
}

/// A question a pane waits on for a secret: the kind, and the question's own
/// line as the pane shows it. Nothing the person types is in here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretPrompt {
    pub kind: SecretKind,
    pub line: String,
}

/// The words a secret is asked for by, each with the kind it asks for. Latin
/// words are matched in lower case; the rest as written. A phrase such as
/// "비밀번호를 입력하세요" is a whole question, so it is listed whole.
const SECRET_WORDS: &[(&str, SecretKind)] = &[
    ("password", SecretKind::Password),
    ("passphrase", SecretKind::Passphrase),
    ("pin", SecretKind::Pin),
    ("암호", SecretKind::Password),
    ("비밀번호", SecretKind::Password),
    ("패스워드", SecretKind::Password),
    ("비밀번호를 입력하세요", SecretKind::Password),
    ("암호를 입력하세요", SecretKind::Password),
    ("パスワード", SecretKind::Password),
    ("密码", SecretKind::Password),
    ("contraseña", SecretKind::Password),
    ("contrasena", SecretKind::Password),
];

/// The words that may stand before a secret word in a bare question: "Enter
/// passphrase", "New password", "새 비밀번호". Only these — any other word
/// before a secret word makes a sentence ("Reset your password:"), not a
/// question. Each ends in a space, so the secret word after it is whole. Longer
/// phrases come first, so "enter new " is taken before "enter ".
const QUALIFIERS: &[&str] = &[
    "enter your ",
    "enter new ",
    "enter current ",
    "enter old ",
    "enter ",
    "retype new ",
    "new ",
    "current ",
    "old ",
    "your ",
    "새 ",
    "현재 ",
];

/// Read the question on a pane's screen, if the pane waits for a secret.
///
/// `rows` are the visible rows from the top of the screen, and `cursor_row` is
/// the row the cursor stands on. The question is read only from the last
/// non-blank row, and only when the cursor stands on that row.
#[must_use]
pub fn read_secret_prompt(rows: &[String], cursor_row: usize) -> Option<SecretPrompt> {
    let last = rows.iter().rposition(|row| !row.trim().is_empty())?;
    if cursor_row != last {
        return None;
    }
    let line = rows[last].trim_end();
    if line.chars().count() > PROMPT_MAX_CHARS {
        return None;
    }
    let body = line.strip_suffix(':').or_else(|| line.strip_suffix('：'))?;
    let kind = kind_of_question(body)?;
    Some(SecretPrompt {
        kind,
        line: line.to_string(),
    })
}

/// The kind of secret a question asks for, from the question's text without its
/// colon — or `None` when the text is not one of the forms this module reads.
fn kind_of_question(body: &str) -> Option<SecretKind> {
    let question = body.trim().to_lowercase();
    let (sudo, rest) = match question.strip_prefix("[sudo] ") {
        Some(rest) => (true, rest),
        None => (false, question.as_str()),
    };
    if let Some(kind) = bare_kind(rest) {
        return Some(kind);
    }
    // `<user>@<host>'s password`, as ssh asks it: the user and the host are one
    // word, and the word has an `@`.
    if let Some(who) = rest.strip_suffix("'s password") {
        return (who.contains('@') && !who.contains(' ')).then_some(SecretKind::Password);
    }
    let (head, target) = rest.split_once(" for ")?;
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    match head {
        // `[sudo] password for <user>`: one user name and nothing after it.
        "password" if sudo => is_user_name(target).then_some(SecretKind::Password),
        // `Password for '<target>'`, which git and others quote, or a target
        // with an `@` in it.
        "password" => (quoted(target) || target.contains('@')).then_some(SecretKind::Password),
        // `Enter passphrase for <key>`: the key is a path or a name.
        "enter passphrase" | "passphrase" => Some(SecretKind::Passphrase),
        _ => None,
    }
}

/// The kind of a question that is only a secret word, with at most one
/// qualifier before it (`password`, `enter pin`, `new password`).
fn bare_kind(question: &str) -> Option<SecretKind> {
    let word = QUALIFIERS
        .iter()
        .find_map(|qualifier| question.strip_prefix(qualifier))
        .unwrap_or(question)
        .trim();
    SECRET_WORDS
        .iter()
        .find(|(known, _)| *known == word)
        .map(|(_, kind)| *kind)
}

/// A `sudo` user name: a short run of letters, digits, dots, dashes and
/// underscores, with no space in it.
fn is_user_name(target: &str) -> bool {
    !target.is_empty()
        && target.chars().count() <= USER_NAME_MAX_CHARS
        && target
            .chars()
            .all(|one| one.is_ascii_alphanumeric() || matches!(one, '.' | '_' | '-'))
}

/// Whether a target is wrapped in matching single or double quotes.
fn quoted(target: &str) -> bool {
    target.len() >= 2
        && ((target.starts_with('\'') && target.ends_with('\''))
            || (target.starts_with('"') && target.ends_with('"')))
}

/// Whether a typed value may be written into a pane: one non-empty line of
/// visible text, at most [`SECRET_MAX_BYTES`]. A control byte (a return, a tab,
/// an escape) would end the line early or be read as a key, so none is let
/// through. Non-ASCII bytes are visible text, so any language's letters pass.
#[must_use]
pub fn value_is_typable(value: &[u8]) -> bool {
    !value.is_empty()
        && value.len() <= SECRET_MAX_BYTES
        && value.iter().all(|byte| *byte >= 0x20 && *byte != 0x7f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_one_line_of_visible_text_of_bounded_size() {
        assert!(value_is_typable("hunter2-not-a-secret".as_bytes()));
        assert!(
            value_is_typable("비밀번호 123!".as_bytes()),
            "visible text in any language"
        );
        assert!(
            value_is_typable(&[b'a'; SECRET_MAX_BYTES]),
            "as long as the field's room"
        );
        assert!(!value_is_typable(b""), "nothing to type");
        assert!(
            !value_is_typable(&[b'a'; SECRET_MAX_BYTES + 1]),
            "longer than the field's room"
        );
        for refused in [
            &b"one\rtwo"[..],
            b"one\ntwo",
            b"one\ttwo",
            b"one\x1b[A",
            b"one\0two",
        ] {
            assert!(
                !value_is_typable(refused),
                "a control byte ends the line early: {refused:?}"
            );
        }
    }

    /// The bundle: one screen per line, written by hand from the prompts each
    /// tool prints and from the look-alikes ordinary output shows. Columns are
    /// separated by ` | `: the expected kind (`none` for no question), the
    /// cursor's row index, then the screen's rows from the top. `<blank>` is an
    /// empty row. A `#` line is a note.
    const CASES: &str = include_str!("secret_prompt/cases.tsv");

    /// The screen one case describes, and the verdict it is written with.
    fn case_of(line: &str) -> (&str, usize, Vec<String>) {
        let columns: Vec<&str> = line.split(" | ").collect();
        let cursor = columns[1]
            .parse()
            .expect("the cursor column is a row index");
        let rows = columns[2..]
            .iter()
            .map(|row| {
                if *row == "<blank>" {
                    String::new()
                } else {
                    (*row).to_string()
                }
            })
            .collect();
        (columns[0], cursor, rows)
    }

    /// The cases the bundle holds, without the notes and the blank lines.
    fn cases() -> impl Iterator<Item = &'static str> {
        CASES
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
    }

    #[test]
    fn the_bundle_of_questions_and_look_alikes_reads_as_written() {
        let (mut asked, mut quiet, mut missed, mut false_alarms) =
            (0_usize, 0_usize, 0_usize, 0_usize);
        for case in cases() {
            let (expect, cursor, rows) = case_of(case);
            let read = read_secret_prompt(&rows, cursor).map(|prompt| prompt.kind.as_str());
            if expect == "none" {
                quiet += 1;
                if read.is_some() {
                    false_alarms += 1;
                    eprintln!("FALSE ALARM: {case}");
                }
            } else {
                asked += 1;
                if read != Some(expect) {
                    missed += 1;
                    eprintln!("MISSED: {case} (read as {read:?})");
                }
            }
        }
        eprintln!(
            "BUNDLE secret_prompt: {asked} questions ({missed} missed), {quiet} look-alikes ({false_alarms} false alarms)"
        );
        assert!(
            asked >= 20 && quiet >= 20,
            "the bundle must hold both kinds of screen"
        );
        assert_eq!((missed, false_alarms), (0, 0));
    }

    #[test]
    #[ignore = "measurement: cargo test -p zerocode-core --lib secret_prompt::tests::measure -- --ignored --nocapture"]
    fn measure() {
        let screens: Vec<(&str, usize, Vec<String>)> = cases().map(case_of).collect();
        const ROUNDS: u32 = 2_000;
        let started = std::time::Instant::now();
        let mut found = 0_usize;
        for _ in 0..ROUNDS {
            for (_, cursor, rows) in &screens {
                found += usize::from(read_secret_prompt(rows, *cursor).is_some());
            }
        }
        let calls = f64::from(ROUNDS) * screens.len() as f64;
        let per_call_us = started.elapsed().as_secs_f64() * 1e6 / calls;
        eprintln!(
            "MEASURE secret_prompt: {per_call_us:.3} us per screen read ({} screens x {ROUNDS} rounds, {found} questions found)",
            screens.len()
        );
    }
}
