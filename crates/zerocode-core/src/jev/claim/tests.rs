//! The claim seat's shared half on its own: the words its version is pinned
//! to, what code settles before any question, and the state a request
//! carries (t-11349 moved it here; zo's own cases still run on it through
//! zo's reader of its turns).

use super::*;

/// A shell result of the turn, as a host reads one into evidence.
fn shell<'a>(command: &str, output: &'a str, nonzero: bool) -> Evidence<&'a str> {
    Evidence {
        command: Some(command.to_string()),
        output,
        is_error: false,
        nonzero,
    }
}

/// The version is pinned to the words zo pinned it to (t-9469): moving the
/// words moved no word.
#[test]
fn the_version_is_pinned_to_the_same_words() {
    assert_eq!(CLAIM_RUBRIC_VERSION, 1);
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        "864bb8ebe6e00fe8"
    );
}

/// Code settles what it can before a question: a named command that never
/// ran is unsupported, a pass over a non-zero exit is contradicted, and lines
/// that match reach the one request as its state.
#[test]
fn code_settles_first_and_the_rest_reach_the_state() {
    let ran = [shell("cargo check", r#"{"stdout":"ok"}"#, false)];
    assert_eq!(
        scan(&ran, "`cargo test` passed.")[0].code,
        CodeVerdict::Unsupported
    );
    let failed = [shell("cargo test", r#"{"stdout":"failed"}"#, true)];
    assert_eq!(
        scan(&failed, "`cargo test` passed.")[0].code,
        CodeVerdict::Contradicted
    );
    let green = [shell(
        "cargo test",
        r#"{"stdout":"test result: ok"}"#,
        false,
    )];
    let claims = scan(&green, "`cargo test` passed.");
    assert_eq!(claims[0].code, CodeVerdict::NeedsReading);
    assert_eq!(claims[0].evidence, "test result: ok");
    let sent = state(&claims);
    assert_eq!(sent["claims"][0]["id"], "C1");
    assert_eq!(sent["evidence"]["C1"], "test result: ok");
    let asked = questions(&claims, |instructions, options| {
        (instructions.to_string(), options.len())
    });
    assert_eq!(
        asked.get("C1").map(|(_, options)| *options),
        Some(CLAIM_CRITERIA.len())
    );
}

