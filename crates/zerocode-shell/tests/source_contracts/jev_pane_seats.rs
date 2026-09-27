//! The completion claim and file pick seats asked of a window pane and asked
//! by zo are one seat each (t-11349): both programs find a turn's claims,
//! build the one request, read its reply, come to a verdict and grade it by
//! the same functions of the core (`zerocode_core::jev::claim`,
//! `zerocode_core::jev::file_pick`), and neither keeps a rule of its own — a
//! function spelled a second time in one of them is a red here.

use crate::support::strip_rust_comments;

const ZO_CLAIM: &str =
    include_str!("../../../../zo-ide/crates/tools/src/misc_tools/smart_router/claim_check.rs");
const ZO_CLAIM_TURN: &str =
    include_str!("../../../../zo-ide/crates/runtime/src/conversation/claim_check.rs");
const ZO_FILE_PICK: &str =
    include_str!("../../../../zo-ide/crates/tools/src/misc_tools/smart_router/file_pick.rs");
const ZO_FILE_PICK_TURN: &str = include_str!("../../../../zo-ide/crates/runtime/src/file_pick.rs");
const ZO_TURN_START: &str =
    include_str!("../../../../zo-ide/crates/runtime/src/conversation/mod.rs");
const WINDOW: &str = include_str!("../../src/pane_guard.rs");

/// A source's shipped part: what comes before its test module.
fn shipped(text: &'static str) -> String {
    let code = text
        .split("\n#[cfg(test)]\nmod tests")
        .next()
        .unwrap_or(text);
    strip_rust_comments(code)
}

/// Whether `code` calls `function` of the core module the seat lives in,
/// by whichever name the file gave that module.
fn calls(code: &str, function: &str) -> bool {
    [
        "shared::",
        "claim::",
        "file_pick::",
        "zerocode_core::jev::claim::",
        "zerocode_core::jev::file_pick::",
    ]
    .iter()
    .any(|module| code.contains(&format!("{module}{function}(")))
}

#[test]
fn zo_and_a_window_pane_ask_and_grade_the_claim_seat_by_one_set_of_functions() {
    let zo = format!("{}\n{}", shipped(ZO_CLAIM), shipped(ZO_CLAIM_TURN));
    let window = shipped(WINDOW);
    for function in [
        "scan",
        "questions",
        "read_choices",
        "code_verdicts",
        "answered_verdict",
        "turn_verdict",
        "compared",
        "next_person_failed",
    ] {
        assert!(
            calls(&zo, function),
            "zo does not call the core's {function}"
        );
        assert!(
            calls(&window, function),
            "a window pane does not call the core's {function}"
        );
    }
    // Both grade a turn by the one label the core writes.
    assert!(zo.contains("done.label(") && window.contains("self.waiting.label(at)"));
    // Neither keeps the rule itself: the words a failed turn opens with, the
    // patterns a claim is read by and the check commands live in the core.
    for code in [&zo, &window] {
        for own in [
            "didn't work",
            "안 됐다",
            "Regex::new",
            "EXEC_CHECK_COMMAND_MARKERS",
        ] {
            assert!(!code.contains(own), "{own} spelled outside the core");
        }
    }
}

#[test]
fn zo_and_a_window_pane_ask_and_grade_the_file_pick_seat_by_one_set_of_functions() {
    let zo = format!(
        "{}\n{}\n{}",
        shipped(ZO_FILE_PICK),
        shipped(ZO_FILE_PICK_TURN),
        shipped(ZO_TURN_START)
    );
    let window = shipped(WINDOW);
    for function in [
        "questions",
        "search_terms",
        "interleave",
        "read_answers_naming",
        "probabilities",
        "edited_fingerprints",
        "label_row",
        "is_code_edit_intent",
    ] {
        let in_zo = calls(&zo, function)
            || zo.contains(&format!(" {function}("))
                && zo.contains("zerocode_core::jev::file_pick::{");
        assert!(in_zo, "zo does not call the core's {function}");
        assert!(
            calls(&window, function),
            "a window pane does not call the core's {function}"
        );
    }
    // One row, begun by one constructor.
    assert!(zo.contains("Self::of_batch(") && window.contains("FilePickRow::of_batch("));
    for code in [&zo, &window] {
        for own in [
            "FILE_PICK_SEARCH_STOP_WORDS",
            "fn description_line",
            "fn workspace_relative_path",
            "\"rankedPaths\"",
        ] {
            assert!(!code.contains(own), "{own} spelled outside the core");
        }
    }
}
