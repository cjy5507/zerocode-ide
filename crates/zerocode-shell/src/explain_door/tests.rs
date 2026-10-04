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

#[test]
fn a_request_names_where_it_came_from_only_when_the_window_held_it() {
    let request = |from: serde_json::Value| -> Start {
        let mut said = serde_json::json!({
            "id": "explain-1",
            "kind": "turn",
            "title": "t",
            "text": "x",
            "language": "ko",
            "headline": "h",
            "route": { "via": "one_shot", "agent": "claude" },
        });
        if !from.is_null() {
            said["from"] = from;
        }
        serde_json::from_value(said).expect("a request the window can send")
    };
    // A turn: the pane it was read in, and that pane's folder.
    let turn = request(serde_json::json!({ "term": 7, "cwd": "/work/one" }));
    let from = turn.from.expect("a source");
    assert_eq!(from.term, Some(7));
    assert_eq!(from.cwd.as_deref(), Some("/work/one"));
    // A diff: a folder and no pane — the null the window sends is no pane at all.
    let diff = request(serde_json::json!({ "term": null, "cwd": "/work/one" }));
    let from = diff.from.expect("a source");
    assert_eq!(from.term, None);
    assert_eq!(from.cwd.as_deref(), Some("/work/one"));
    // A report: its seat's pane, and no folder (the report's own row has it).
    let report = request(serde_json::json!({ "term": 3, "cwd": null }));
    let from = report.from.expect("a source");
    assert_eq!((from.term, from.cwd), (Some(3), None));
    // A window older than this field sends none, and nothing is invented.
    assert!(request(serde_json::Value::Null).from.is_none());
}

/// A measurement, not a check: what the door adds to every report a pane's state
/// makes. With no request standing the road is one atomic load; with the table full
/// and none of them this pane's, a lock and a scan of sixteen.
/// `cargo test -p zerocode-shell --bin zerocode-shell explain_door_cost -- --ignored --nocapture`
#[test]
#[ignore = "a measurement, run on purpose through the build line"]
fn explain_door_cost_probe() {
    use std::hint::black_box;
    use std::time::Instant;
    use zerocode_core::explain::ACTIVE_MAX;

    const CALLS: u32 = 5_000_000;

    let started = Instant::now();
    let mut stood = 0_u32;
    for _ in 0..CALLS {
        stood += u32::from(black_box(anything_stands()));
    }
    let idle_ns = started.elapsed().as_nanos() / u128::from(CALLS);
    assert_eq!(stood, 0, "no request stands in this test");

    for n in 0..ACTIVE_MAX {
        let held = Held {
            id: format!("probe-{n}"),
            term: Some(9_000 + u32::try_from(n).unwrap_or_default()),
            agent: "claude".to_string(),
            state: State::Asked,
            made_ms: 0,
            prompt: None,
        };
        assert_eq!(with_desk(|desk| desk.admit(held)), Ok(()));
    }
    assert!(anything_stands());

    let started = Instant::now();
    let mut found = 0_u32;
    for _ in 0..CALLS {
        found += u32::from(with_desk(|desk| desk.turn_ended(black_box(1))).is_some());
    }
    let standing_ns = started.elapsed().as_nanos() / u128::from(CALLS);
    assert_eq!(found, 0, "pane 1 has no request");

    for n in 0..ACTIVE_MAX {
        assert!(with_desk(|desk| desk.finish(&format!("probe-{n}"))).is_some());
    }
    assert!(!anything_stands());
    println!("explain_cost door calls={CALLS} idle_ns={idle_ns} standing_ns={standing_ns}");
}
