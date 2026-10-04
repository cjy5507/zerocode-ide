//! The door's own small decisions (t-32787): which word a refusal becomes,
//! what a request's name makes of a file name, how much of a report is read,
//! and that the two words the desk refuses by are the guard's own.

use super::*;
use zerocode_pty::ready::Refusal;

#[test]
fn the_desks_two_guard_words_are_spelled_as_the_guard_spells_them() {
    assert_eq!(why::PARKED, Refusal::Parked.token());
    assert_eq!(why::HOLDS_A_DRAFT, Refusal::HoldsADraft.token());
}

#[test]
fn a_resting_provider_is_a_quota_wall_a_running_job_is_in_flight_and_any_other_ceiling_is_the_budget()
 {
    assert_eq!(
        budget_token(&BudgetRefusal::Resting { until_ms: 1 }),
        why::QUOTA_WALL
    );
    assert_eq!(budget_token(&BudgetRefusal::Running), why::IN_FLIGHT);
    assert_eq!(
        budget_token(&BudgetRefusal::Concurrent {
            active: 3,
            limit: 2
        }),
        why::BUDGET
    );
}

#[test]
fn a_request_name_makes_a_file_stem_of_nothing_but_safe_characters() {
    assert_eq!(file_stem("explain-0f3a-9b"), "explain-0f3a-9b");
    assert_eq!(file_stem("../../etc/pass wd?"), "etcpasswd");
    assert_eq!(file_stem(""), "");
}

#[test]
fn a_refusal_that_is_not_a_wall_is_the_clis_own() {
    let once = Once {
        success: false,
        stdout: String::new(),
        stderr_tail: "boom".to_string(),
    };
    assert_eq!(
        refused_token("claude", "I cannot do that", &once),
        why::CLI_REFUSED
    );
    assert_eq!(refused_token("codex", "", &once), why::CLI_REFUSED);
}

#[test]
fn a_report_is_read_to_the_bytes_a_request_may_receive_and_no_further() {
    let path = std::env::temp_dir().join(format!(
        "zerocode-explain-door-{}-cap.txt",
        std::process::id()
    ));
    std::fs::write(&path, "한국어 abc def").expect("a temp file");
    let read = read_bounded_text(&path, 10);
    let _ = std::fs::remove_file(&path);
    let text = read.expect("a bounded read");
    assert!(text.starts_with("한국어"), "{text:?}");
    assert!(
        text.len() <= 10 + '\u{fffd}'.len_utf8(),
        "{} bytes: {text:?}",
        text.len()
    );
}

#[test]
fn a_request_with_neither_text_nor_report_is_not_readable() {
    assert!(material_of(None, None).is_err());
    assert_eq!(
        material_of(Some("the chosen text"), None).as_deref(),
        Ok("the chosen text")
    );
}
