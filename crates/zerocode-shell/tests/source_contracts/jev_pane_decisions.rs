//! A person's own decisions asked by zo and asked for a window pane's agent
//! are one decision each (t-106308): both programs build what a point lends,
//! make the one request, read its JSON answer, judge whether it earns its
//! line, say that line and write the one row by the same functions of the
//! core (`zerocode_core::jev::custom`), and neither keeps a reader or a rule
//! of its own — a function spelled a second time in one of them is a red
//! here.

use crate::support::strip_rust_comments;

const ZO: &str =
    include_str!("../../../../zo-ide/crates/tools/src/misc_tools/smart_router/custom_decisions.rs");
const ZO_ADVICE: &str =
    include_str!("../../../../zo-ide/crates/tools/src/misc_tools/smart_router/project_rules.rs");
const WINDOW: &str = include_str!("../../src/pane_guard/decisions.rs");

/// A source's shipped part: what comes before its test module.
fn shipped(text: &'static str) -> String {
    let code = text
        .split("\n#[cfg(test)]\nmod tests")
        .next()
        .unwrap_or(text);
    strip_rust_comments(code)
}

#[test]
fn zo_and_a_window_pane_ask_a_persons_decision_by_one_set_of_functions() {
    let (zo, window) = (shipped(ZO), shipped(WINDOW));
    for (name, code) in [("zo", &zo), ("a window pane", &window)] {
        for function in [
            "custom::lent_before_command(",
            "custom::lent_at_turn_end(",
            "custom::body_of(",
            "custom::read(",
            "custom::earns_its_line(",
            "custom::line(",
            "custom::said(",
            "custom::introduced",
            "custom::check(",
            "custom::asks_in(",
            "DecisionRow::of(",
            "deadline_at(",
            "standing(",
        ] {
            assert!(
                code.contains(function),
                "{name} does not call the core's {function}"
            );
        }
        // Neither keeps the rules itself: the readers of an answer, the line's
        // bar, the request's shape and the row live in the core.
        for own in [
            "fn earns_its_line",
            "fn nearest_level",
            "fn body_of",
            "fn lent_before_command",
            "fn lent_at_turn_end",
            "fn deadline_at",
            "fn standing",
            "struct Reading",
            "YES_FROM",
            "HALF_A_LEVEL",
            "noul::read_value",
            "choice::read_value",
            "score::read_value",
            "\"rubricVersion\"",
            "LINES_INTRO.chars()",
        ] {
            assert!(
                !code.contains(own),
                "{own} spelled in {name}, outside the core"
            );
        }
    }
    // zo's turn's-end advice bounds its lines by the core's one count.
    let advice = shipped(ZO_ADVICE);
    assert!(advice.contains("custom::introduced_within("));
    assert!(!advice.contains("LINES_INTRO.chars()"));
}

/// A decision whose refusal repeats is quiet in one book (t-107587): zo and a
/// window pane both keep the core's (`zerocode_core::jev::custom::quiet`),
/// ask it before a decision and tell it each refusal, and neither keeps
/// marks, spans or the rule of which refusals repeat of its own.
#[test]
fn zo_and_a_window_pane_keep_one_book_of_quiet_decisions() {
    let (zo, window) = (shipped(ZO), shipped(WINDOW));
    for (name, code) in [("zo", &zo), ("a window pane", &window)] {
        for call in ["quiet::Now", "quiet.is(", "quiet.refused(", "quiet.broke("] {
            assert!(
                code.contains(call),
                "{name} does not call the core's book: {call}"
            );
        }
        for own in [
            "struct Marks",
            "enum Until",
            "Until::",
            "struct Quiet",
            "fn mark(",
            ".repeats()",
            "Refused::repeats",
            "of_the_folder",
            "is_full()",
        ] {
            assert!(
                !code.contains(own),
                "{own} spelled in {name}, outside the core's book"
            );
        }
    }
}
