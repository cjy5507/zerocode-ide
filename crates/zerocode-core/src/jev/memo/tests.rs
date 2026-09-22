//! What the memo remembers, under what key, and how much of it.

use super::*;
use crate::jev::{BROWSER, DESKTOP};

fn memo_in(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("jev").join(MEMO_FILE)
}

#[test]
fn the_key_is_the_seat_and_the_cleared_bytes_and_nothing_else() {
    let bytes = br#"{"state":{"goal":"open"},"questions":{}}"#;
    assert_eq!(key_of(&BROWSER, bytes), key_of(&BROWSER, bytes));
    assert_ne!(
        key_of(&BROWSER, bytes),
        key_of(&DESKTOP, bytes),
        "the same bytes under another seat are another question"
    );
    assert_ne!(
        key_of(&BROWSER, bytes),
        key_of(&BROWSER, br#"{"state":{"goal":"close"},"questions":{}}"#),
        "one word moved is another key"
    );
    // The fingerprint's own shape: sixteen hex digits, as every rubric
    // version is pinned with.
    let key = key_of(&BROWSER, bytes);
    assert_eq!(key.len(), 16);
    assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn a_memo_recalls_the_newest_answer_under_a_key_and_nothing_for_a_stranger() {
    let dir = tempfile::tempdir().expect("a home");
    let memo = memo_in(&dir);
    assert_eq!(recall(&memo, "abc"), None, "no file recalls nothing");

    remember(&memo, &BROWSER, "abc", r#"{"answers":1}"#, 10).expect("written");
    remember(&memo, &BROWSER, "def", r#"{"answers":2}"#, 11).expect("written");
    remember(&memo, &BROWSER, "abc", r#"{"answers":3}"#, 12).expect("written");

    assert_eq!(
        recall(&memo, "abc"),
        Some(Recalled {
            answer: r#"{"answers":3}"#.to_string(),
            at: 12
        }),
        "the newest row for the key answers"
    );
    assert_eq!(
        recall(&memo, "def").map(|r| r.answer),
        Some(r#"{"answers":2}"#.to_string())
    );
    assert_eq!(recall(&memo, "ghi"), None);
    assert_eq!(rows(&memo), 3);

    // A row that carries the key inside its answer text is not a row for it.
    remember(&memo, &BROWSER, "zzz", r#"{"key":"abc"}"#, 13).expect("written");
    assert_eq!(recall(&memo, "abc").map(|r| r.at), Some(12));
}

#[test]
fn a_memo_past_its_cap_keeps_its_newest_half() {
    let dir = tempfile::tempdir().expect("a home");
    let memo = memo_in(&dir);
    for n in 0..MEMO_ROWS_CAP {
        remember(&memo, &BROWSER, &format!("k{n}"), "{}", n as i64).expect("written");
    }
    assert_eq!(rows(&memo), MEMO_ROWS_CAP);
    remember(&memo, &BROWSER, "last", "{}", 9_999).expect("written");
    assert_eq!(rows(&memo), MEMO_ROWS_CAP / 2 + 1);
    assert_eq!(recall(&memo, "k0"), None, "the oldest half is gone");
    assert!(recall(&memo, &format!("k{}", MEMO_ROWS_CAP - 1)).is_some());
    assert_eq!(recall(&memo, "last").map(|r| r.at), Some(9_999));
}

#[test]
fn a_torn_row_is_stepped_over() {
    let dir = tempfile::tempdir().expect("a home");
    let memo = memo_in(&dir);
    remember(&memo, &BROWSER, "abc", r#"{"answers":1}"#, 10).expect("written");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&memo)
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"{\"key\":\"abc\",\"ans"))
        .expect("torn");
    assert_eq!(recall(&memo, "abc").map(|r| r.at), Some(10));
}
