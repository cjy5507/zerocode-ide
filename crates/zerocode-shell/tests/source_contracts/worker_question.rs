//! A worker's own question goes to its coordinator (t-82296, t-84343,
//! t-84470). The behaviour is `worker_question`'s own unit tests; these
//! contracts hold what a test of one function cannot see: that every reason
//! the window gives the person a worker's question has its words on the card
//! in five languages, that the words a coordinator gives a question to the
//! person with are the guide's words, and that one beat reads the ledger once.

use std::path::Path;

use super::quiet_children::code_only;
use super::support::{ORCHESTRATION_GUIDE, block_after, window_source};

const QUESTIONS: &str = include_str!("../../src/worker_question.rs");

/// The module's shipped half, comments and strings off.
fn questions_code() -> String {
    code_only(
        QUESTIONS
            .split_once("#[cfg(test)]")
            .map_or(QUESTIONS, |(code, _)| code),
    )
}

fn ui_file(name: &str) -> String {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    std::fs::read_to_string(ui.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// `TO_PERSON`'s words, read off its declaration.
fn to_person_words() -> Vec<&'static str> {
    let opens = "const TO_PERSON: &[&str] = &[";
    let start = QUESTIONS
        .find(opens)
        .unwrap_or_else(|| panic!("`{opens}` is gone from worker_question.rs"))
        + opens.len();
    let list = &QUESTIONS[start..start + QUESTIONS[start..].find("];").expect("TO_PERSON closes")];
    let words: Vec<&str> = list.split('"').skip(1).step_by(2).collect();
    assert!(!words.is_empty(), "TO_PERSON holds no word: {list}");
    words
}

/// A variant's name as serde's `kebab-case` writes it on the wire.
fn kebab(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (at, glyph) in name.chars().enumerate() {
        if glyph.is_uppercase() && at > 0 {
            out.push('-');
        }
        out.extend(glyph.to_lowercase());
    }
    out
}

/// Every reason the window gives the person a worker's question after all
/// (`Raised`, sent as `ask:raised`) opens the card with its own sentence:
/// a case in `askRaisedWords` with its Korean default, and its key in the
/// four other catalogs — a reason with no words is a card that does not say
/// why it is the person's (t-84470, r1 P3-2).
#[test]
fn every_reason_a_worker_question_reaches_the_person_has_words_in_five_catalogs() {
    let code = questions_code();
    let reasons: Vec<String> = block_after(&code, "enum Raised {")
        .lines()
        .skip(1)
        .filter_map(|line| {
            let name = line.trim().strip_suffix(',')?;
            name.chars().all(char::is_alphanumeric).then(|| kebab(name))
        })
        .collect();
    assert!(
        reasons.len() >= 6,
        "the reader found only these reasons: {reasons:?}"
    );
    let words = block_after(window_source(), "function askRaisedWords(");
    let catalogs = ui_file("shell-i18n.js");
    for reason in &reasons {
        let case = words
            .split(&format!("case \"{reason}\":"))
            .nth(1)
            .unwrap_or_else(|| panic!("`{reason}` has no words on the card:\n{words}"));
        let key = case
            .split("t(\"")
            .nth(1)
            .and_then(|call| call.split_once("\", \""))
            .map(|(key, default)| {
                assert!(
                    !default.starts_with('"'),
                    "`{reason}` has no Korean default"
                );
                key
            })
            .unwrap_or_else(|| panic!("`{reason}` says nothing through t():\n{case}"));
        assert!(
            key.starts_with("board.ask.raised"),
            "`{reason}` is said by `{key}`"
        );
        assert_eq!(
            catalogs.matches(&format!("\"{key}\":")).count(),
            4,
            "`{key}` is not in the four catalogs"
        );
    }
}

/// The words a coordinator gives its worker's question to the person with
/// are the window's own list, word for word, in the guide a coordinator reads
/// — so the guide cannot teach a word the window does not take (t-84343).
#[test]
fn the_guide_gives_a_question_to_the_person_in_the_windows_own_words() {
    let line = ORCHESTRATION_GUIDE
        .lines()
        .find(|line| line.contains("worker's own CLI question"))
        .expect("the guide says how a worker's own CLI question is answered");
    for word in to_person_words() {
        assert!(
            line.contains(&format!("`{word}`")),
            "the guide does not give the person a question with `{word}`:\n{line}"
        );
    }
    assert!(
        line.contains("the question stays open until the person answers"),
        "{line}"
    );
}

/// One beat over every worker question reads the ledger once — the seat at
/// each pane, the term each written-down worker sits at, where every letter
/// stands — however many questions wait (t-84470, r1 P3-6: two reads a
/// question a second was twenty a second for ten workers). The one other
/// read is a pane's first asking.
#[test]
fn a_beat_of_worker_questions_reads_the_ledger_once() {
    let code = questions_code();
    let beat = block_after(&code, "fn sweep(");
    assert_eq!(
        (
            code.matches("read_questions(").count(),
            beat.matches("read_questions(").count()
        ),
        (1, 1),
        "the beat reads the ledger more than once, or outside `sweep`"
    );
    for one_by_one in ["question_standing(", "worker_term("] {
        assert!(
            !code.contains(one_by_one),
            "the beat reads the ledger again through `{one_by_one}`"
        );
    }
    assert_eq!(code.matches("worker_ask_seat(").count(), 1);
    assert!(block_after(&code, "fn heard(").contains("worker_ask_seat("));
}
