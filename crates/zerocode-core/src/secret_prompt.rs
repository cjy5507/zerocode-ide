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
//!   form the table below names: a bare secret word (after at most one
//!   qualifier such as `Enter` or `New`), `[sudo] password for <user>`,
//!   `Password for '<target>'`, `<user>@<host>'s password`, or
//!   `Enter passphrase for <key>`.
//!
//! The words and their kinds are the tables in this file. The window's
//! sentence for each kind is its own translation table.

use serde::Serialize;

/// The most characters a line may have and still be read as a question. A
/// longer line is someone's text that happens to end in a colon.
pub const PROMPT_MAX_CHARS: usize = 160;

/// The kind of secret a question asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretKind {
    Password,
    Passphrase,
    Pin,
}

impl SecretKind {
    /// The word the window keys its sentence by (`secret.kind.<word>`).
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

/// Read the question on a pane's screen, if the pane waits for a secret.
///
/// `rows` are the visible rows from the top of the screen, and `cursor_row` is
/// the row the cursor stands on. Not yet implemented: this red stage answers
/// nothing, so the bundle in `secret_prompt/cases.tsv` fails.
#[must_use]
pub fn read_secret_prompt(rows: &[String], cursor_row: usize) -> Option<SecretPrompt> {
    let _ = (rows, cursor_row);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
