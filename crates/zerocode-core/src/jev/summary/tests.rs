use serde_json::json;

use super::*;

#[test]
fn a_full_window_is_bounded_below_one_because_twenty_answers_are_not_a_promise() {
    // The whole reason the rise line reads a bound and not the share: twenty
    // of twenty is a share of 1.0 and evidence of rather less.
    let bound = wilson_lower(20, 20, WILSON_Z_95);
    assert!((0.83..0.84).contains(&bound), "20/20 bounded at {bound}");
    assert!(wilson_lower(19, 20, WILSON_Z_95) < bound);
    assert_eq!(
        wilson_lower(0, 0, WILSON_Z_95),
        0.0,
        "an empty window promises nothing"
    );
    assert_eq!(
        wilson_lower(0, 8, WILSON_Z_95),
        0.0,
        "no answer cannot bound below zero"
    );
}

#[test]
fn a_wider_window_of_the_same_share_bounds_higher() {
    let thin = wilson_lower(19, 20, WILSON_Z_95);
    let wide = wilson_lower(190, 200, WILSON_Z_95);
    assert!(wide > thin, "same share, more rows: {wide} !> {thin}");
}

#[test]
fn the_percentile_is_nearest_rank_and_an_empty_population_has_none() {
    let sorted = [10_u64, 20, 30, 40];
    assert_eq!(percentile(&sorted, 0.50), Some(20));
    assert_eq!(percentile(&sorted, 0.95), Some(40));
    assert_eq!(
        percentile(&sorted, 0.0),
        Some(10),
        "a rank below one still names the first"
    );
    assert_eq!(percentile(&[], 0.5), None);
}

#[test]
fn a_rerank_rows_own_spelling_is_read_like_every_other_ledgers() {
    // rerank-shadow writes `elapsed_ms` where every other ledger writes
    // `elapsedMs`. A reader that spelled its own key would have read this
    // ledger as a seat that never called anything.
    let rows = [
        json!({"at": 10, "outcome": "answered", "elapsed_ms": 300, "input_tokens": 40}),
        json!({"at": 20, "outcome": "answered", "elapsedMs": 100, "inputTokens": 60}),
    ];
    let tally = summarize(&rows, 0);
    assert_eq!(
        (tally.called, tally.p50_ms, tally.p95_ms),
        (2, Some(100), Some(300))
    );
    assert_eq!(tally.input_tokens, 100);
}

#[test]
fn a_memo_hit_answered_without_asking_so_it_leaves_the_latency_alone() {
    let rows = [
        json!({"at": 1, "outcome": "answered", "elapsedMs": 0, "cached": true}),
        json!({"at": 2, "outcome": "answered", "elapsedMs": 800}),
    ];
    let tally = summarize(&rows, 0);
    assert_eq!(tally.answered, 2, "a memo hit is an answer");
    assert_eq!(tally.called, 1, "and not a call");
    assert_eq!(tally.p95_ms, Some(800));
}

#[test]
fn a_door_refusal_is_counted_by_its_own_token_and_never_as_an_answer() {
    let rows = [
        json!({"at": 1, "outcome": "not_consented"}),
        json!({"at": 2, "outcome": "not_consented"}),
        json!({"at": 3, "outcome": "schema_not_one"}),
        json!({"at": 4, "outcome": "answered", "elapsedMs": 5}),
    ];
    let tally = summarize(&rows, 0);
    assert_eq!(tally.rows, 4);
    assert_eq!(tally.answered, 1);
    assert_eq!(
        tally.failures,
        vec![
            ("not_consented".to_string(), 2),
            ("schema_not_one".to_string(), 1)
        ],
        "most frequent first, then by token"
    );
    // The two refusals sent nothing, so the seat was asked twice and answered
    // once: a share of a half, not a quarter.
    assert_eq!(tally.refused, 2);
    assert_eq!(tally.asked(), 2);
    assert!((tally.answered_share().expect("rows") - 0.5).abs() < f64::EPSILON);
    let all_refused = summarize(&rows[..2], 0);
    assert_eq!(all_refused.rows, 2);
    assert_eq!(
        all_refused.answered_share(),
        None,
        "a seat the door refused every time is not a seat that answered nothing"
    );
    assert_eq!(all_refused.answered_lower_bound(), None);
}

#[test]
fn the_window_a_floor_needs_is_the_smallest_a_perfect_one_clears_it_on() {
    // 1 / (1 + z²/n) ≥ 0.95 ⇔ n ≥ 19 z² = 72.99.
    assert_eq!(rows_that_can_clear(950), 73);
    for floor in [500, 800, 950, 990] {
        let rows = rows_that_can_clear(floor);
        assert!(rows >= JUDGED_EVERY_ROWS, "{floor}: never under a cadence");
        assert!(
            crate::jev::promote::permille(wilson_lower(rows, rows, WILSON_Z_95)) >= floor,
            "{floor}: {rows} perfect rows do not clear it"
        );
        assert!(
            rows == JUDGED_EVERY_ROWS
                || crate::jev::promote::permille(wilson_lower(rows - 1, rows - 1, WILSON_Z_95))
                    < floor,
            "{floor}: {rows} is not the smallest"
        );
    }
    assert_eq!(
        rows_that_can_clear(800),
        JUDGED_EVERY_ROWS,
        "twenty of twenty bound at 0.839"
    );
    assert_eq!(
        rows_that_can_clear(1000),
        usize::MAX,
        "no window clears a share of one"
    );
}

#[test]
fn the_window_starts_where_it_is_told_and_an_empty_one_shares_nothing() {
    let rows = [
        json!({"at": 100, "outcome": "answered", "elapsedMs": 1}),
        json!({"at": 300, "outcome": "answered", "elapsedMs": 2}),
    ];
    assert_eq!(summarize(&rows, 200).rows, 1);
    let empty = summarize(&rows, 400);
    assert_eq!(empty.rows, 0);
    assert_eq!(
        empty.answered_share(),
        None,
        "no rows is not a share of zero"
    );
    assert_eq!(empty.answered_lower_bound(), None);
}

/// The countdown belongs to the seat, whose window decides when its first
/// judgment can happen at all — a tally has no seat and so cannot say
/// ([`crate::jev::promote::rows_to_next_judgment`], pinned in the table's own
/// tests).
#[test]
fn the_rows_a_use_owes_before_its_next_judgment_count_down_and_wrap() {
    let seat = &crate::jev::PLACEMENT;
    let wanted = crate::jev::promote::window_wanted_for(seat).expect("placement rises");
    let owed = |asked: usize| crate::jev::promote::rows_to_next_judgment(seat, asked);
    assert_eq!(
        owed(0),
        Some(wanted),
        "the first judgment waits for a full window"
    );
    assert_eq!(owed(wanted - 1), Some(1));
    assert_eq!(
        owed(wanted),
        Some(JUDGED_EVERY_ROWS),
        "a judgment just made owes a full cadence"
    );
    assert_eq!(owed(wanted + JUDGED_EVERY_ROWS - 1), Some(1));
}

#[test]
fn every_key_this_module_reads_names_its_canonical_spelling_first() {
    for key in LEDGER_KEYS {
        let spellings: Vec<&str> = key.spellings().collect();
        assert_eq!(
            spellings.first(),
            Some(&key.canonical),
            "{} leads with itself",
            key.canonical
        );
        assert!(
            !key.also.contains(&key.canonical),
            "{} names its own spelling twice",
            key.canonical
        );
    }
}

#[test]
fn the_judges_own_note_is_not_a_request_it_made() {
    // A transition row lives in the same file as the rows it was decided on.
    // Counted as a request it would have no outcome, so every judgment a seat
    // earned would lower the share it was judged on.
    let rows = [
        json!({"at": 1, "outcome": "answered", "elapsedMs": 10}),
        json!({"at": 2, (TRANSITION.canonical): "rise", "rows": 20}),
        json!({"at": 3, "outcome": "answered", "elapsedMs": 20}),
    ];
    let tally = summarize(&rows, 0);
    assert_eq!(
        (tally.rows, tally.answered),
        (2, 2),
        "the note was counted as a request"
    );
    assert_eq!(tally.answered_share(), Some(1.0));
    assert_eq!(asked_something(&rows[1]), None);
    assert_eq!(asked_something(&rows[0]), Some("answered"));
}

#[test]
fn the_judged_window_is_the_last_n_requests_and_not_the_last_n_lines() {
    let mut rows: Vec<Value> = (0..30)
        .map(|at| json!({"at": at, "outcome": if at < 10 { "timeout" } else { "answered" }, "elapsedMs": at}))
        .collect();
    rows.insert(15, json!({"at": 99, (TRANSITION.canonical): "rise"}));
    let window = summarize_last(&rows, 20);
    assert_eq!(
        window.rows, 20,
        "a note took a request's place in the window"
    );
    assert_eq!(
        window.answered, 20,
        "the window reached back past the answers"
    );
    assert_eq!(
        summarize_last(&rows, 100).rows,
        30,
        "more asked for than exist"
    );
    assert_eq!(summarize_last(&[], 20).rows, 0);
}

#[test]
fn failures_in_a_row_stop_at_the_first_answer_and_step_over_the_notes() {
    let rows = [
        json!({"at": 1, "outcome": "timeout"}),
        json!({"at": 2, "outcome": "answered"}),
        json!({"at": 3, "outcome": "timeout"}),
        json!({"at": 4, (TRANSITION.canonical): "fall", "line": "latency"}),
        json!({"at": 5, "outcome": "no_key"}),
    ];
    // The `no_key` at the end was never sent: the door's refusal is stepped
    // over like the note, and the run is the one timeout before the answer.
    assert_eq!(
        failures_in_a_row(&rows),
        1,
        "a refusal or the note broke the run"
    );
    assert_eq!(failures_in_a_row(&rows[..2]), 0, "it ends on an answer");
    assert_eq!(failures_in_a_row(&rows[..1]), 1);
    assert_eq!(failures_in_a_row(&[]), 0);
    assert!(is_refusal("no_key") && is_refusal("budget") && !is_refusal("timeout"));
}

#[test]
fn the_last_rows_are_handed_out_as_they_are_and_counted_the_same() {
    let rows: Vec<Value> = (0..5)
        .map(|at| json!({"at": at, "outcome": "answered", "elapsedMs": at}))
        .collect();
    let last = last_asked(&rows, 2);
    assert_eq!(last.len(), 2);
    assert_eq!(last[0]["at"], 3, "oldest first");
    assert_eq!(summarize_rows(last, i64::MIN), summarize_last(&rows, 2));
}

#[test]
fn a_control_row_is_not_a_request_and_is_counted_nowhere_but_by_name() {
    // The routing seat runs the probe it skipped once more for a sampled
    // active turn and writes the result as a control row. It answered
    // nothing the seat was asked — the judgment on it was answered on the
    // row before — so it is out of the window, the share, the latency and
    // the run of failures, and a reader that wants it asks for it by name.
    let rows = [
        json!({"at": 1, "outcome": "answered", "elapsedMs": 10, "requests": 1}),
        json!({"at": 2, "outcome": CONTROL, "elapsedMs": 0, "requests": 0, "cached": false}),
        json!({"at": 3, "outcome": "timeout", "elapsedMs": 1_500, "requests": 1}),
        json!({"at": 4, "outcome": CONTROL, "elapsedMs": 0, "requests": 0}),
    ];
    assert_eq!(
        asked_something(&rows[1]),
        None,
        "a control row was counted as a request"
    );
    assert!(is_control_row(&rows[1]) && !is_control_row(&rows[0]) && !is_control_row(&rows[2]));
    let tally = summarize(&rows, 0);
    assert_eq!((tally.rows, tally.answered, tally.called), (2, 1, 2));
    assert_eq!(
        tally.p95_ms,
        Some(1_500),
        "a control row's zero joined the latency"
    );
    let window = last_asked(&rows, 3);
    assert_eq!(
        window.len(),
        2,
        "a control row took a request's place in the window"
    );
    assert!(window.iter().all(|row| !is_control_row(row)));
    assert_eq!(summarize_last(&rows, 1).rows, 1);
    assert_eq!(
        failures_in_a_row(&rows),
        1,
        "the control row at the end broke the run, or was counted in it"
    );
}

/// The rows the judge weighs are the requests, the marks and the control
/// rows; the judge's own note and a seat's bookkeeping between them are not
/// (t-6284) — zo's step governor files a `step` row, with no outcome and no
/// mark, for every request of a turn.
#[test]
fn a_request_a_mark_and_a_control_row_are_weighed_and_a_note_or_a_step_is_not() {
    let weighed = [
        json!({"at": 1, "outcome": "answered", "model": "jev-1.13.0"}),
        json!({"at": 2, "outcome": "no_key"}),
        json!({"at": 3, "label": "s@1", "agreed": false}),
        json!({"at": 4, "outcome": CONTROL, "task": 5}),
    ];
    let not_weighed = [
        json!({"at": 5, (TRANSITION.canonical): "rise", "rows": 20}),
        json!({"at": 6, "kind": "step", "model": "claude-opus-5", "delta": 1}),
    ];
    for row in &weighed {
        assert!(is_request_or_mark(row), "{row}");
    }
    for row in &not_weighed {
        assert!(!is_request_or_mark(row), "{row}");
    }
}

#[test]
fn the_rows_whose_answer_was_acted_on_are_counted_off_whichever_word_the_seat_spelled() {
    let rows = [
        json!({"at": 1, "outcome": "answered", "routeUse": "applied"}),
        json!({"at": 2, "outcome": "answered", "routeUse": "fallback"}),
        json!({"at": 3, "outcome": "answered", "applied": true}),
        json!({"at": 4, "outcome": "answered", "pressed": true}),
        json!({"at": 5, "outcome": "answered", "chosen": "claude"}),
        json!({"at": 6, "outcome": "no_key"}),
    ];
    let tally = summarize(&rows, 0);
    assert_eq!(
        tally.applied, 3,
        "one per spelling, none for a seat with no apply stage"
    );
    assert_eq!(applied_of(&rows[1]), Some(false));
    assert_eq!(applied_of(&rows[4]), None);
    assert_eq!(Tally::default().applied, 0);
}

#[test]
fn the_doors_refusals_are_named_among_the_failures_one_token_at_a_time() {
    let rows = [
        json!({"at": 1, "outcome": "not_consented"}),
        json!({"at": 2, "outcome": "not_consented"}),
        json!({"at": 3, "outcome": "no_key"}),
        json!({"at": 4, "outcome": "timeout"}),
        json!({"at": 5, "outcome": "answered", "elapsedMs": 5}),
    ];
    let tally = summarize(&rows, 0);
    let refusals: Vec<(String, usize)> = tally.refusals().cloned().collect();
    assert_eq!(
        refusals,
        vec![("not_consented".to_string(), 2), ("no_key".to_string(), 1)],
        "most frequent first, and the wire's timeout is not the door's"
    );
    assert_eq!(tally.refused, 3);
    assert_eq!(tally.failures.len(), 3);
}

#[test]
fn an_agreement_over_rows_already_picked_out_reads_the_same_marks() {
    let rows = [
        json!({"at": 1, "agreed": true}),
        json!({"at": 2, "agreed": false}),
        json!({"at": 3, "label": "x", "agreed": true}),
    ];
    let held: Vec<&Value> = rows
        .iter()
        .filter(|row| row["at"].as_i64() != Some(2))
        .collect();
    let agreement = agreement_rows(held.iter().copied(), i64::MIN);
    assert_eq!((agreement.compared, agreement.agreed), (2, 2));
    assert_eq!(agreement_since(&rows, 0), agreement_rows(rows.iter(), 0));
}

/// What a screen seat's guards stopped and what it handed to the person are
/// counted once, here, off the rows' own words (t-6187's `barred` and
/// `controlKind`, the dashboard's drawer, t-6277 D6): a press the screen's own
/// text ordered is `instructed`, one a wall stood in front of is `walled`, and
/// a control a press cannot take back that an acting seat did not press is
/// `destructive_held` — a recording seat's row pressed nothing because it
/// only records, which hands nothing to anybody. `named` is every row that
/// named a control at all, so a reader can tell a seat that presses from one
/// that never does.
#[test]
fn a_screen_seats_guards_and_the_controls_it_handed_over_are_counted_once_per_row() {
    use crate::guarded::ControlKind;
    use crate::jev::{ROUTE_USE_APPLIED, ROUTE_USE_FALLBACK};
    use crate::screen_action::Stopped;
    let row = |extra: Value| {
        let mut row = json!({ "at": 5, "outcome": ANSWERED, "elapsedMs": 200, "requests": 1 });
        for (key, value) in extra.as_object().expect("an object") {
            row[key] = value.clone();
        }
        row
    };
    let destructive = ControlKind::Destructive.word();
    let plain = ControlKind::Plain.word();
    let rows = [
        row(
            json!({ "barred": Stopped::Injected.word(), "controlKind": plain, "routeUse": ROUTE_USE_FALLBACK }),
        ),
        row(
            json!({ "barred": Stopped::Injected.word(), "controlKind": destructive, "routeUse": ROUTE_USE_FALLBACK }),
        ),
        row(
            json!({ "barred": Stopped::Walled.word(), "controlKind": plain, "routeUse": ROUTE_USE_FALLBACK }),
        ),
        row(
            json!({ "barred": "low_confidence", "controlKind": destructive, "routeUse": ROUTE_USE_FALLBACK }),
        ),
        row(json!({ "controlKind": destructive, "routeUse": ROUTE_USE_APPLIED, "pressed": true })),
        row(json!({ "controlKind": destructive, "routeUse": crate::jev::JevMode::Shadow.key() })),
        row(json!({ "controlKind": plain, "routeUse": ROUTE_USE_APPLIED })),
        // A walk barred before it asked names no control and no guard.
        json!({ "at": 5, "outcome": "barred", "barred": "no_budget", "requests": 0 }),
        // A row from before the window is not the window's.
        row(
            json!({ "at": 1, "barred": Stopped::Walled.word(), "controlKind": destructive, "routeUse": ROUTE_USE_FALLBACK }),
        ),
    ];
    let tally = summarize(&rows, 2);
    assert_eq!(
        tally.guards,
        Guards {
            instructed: 2,
            walled: 1
        }
    );
    assert_eq!(
        tally.controls,
        Controls {
            named: 7,
            destructive_held: 2
        }
    );
    let quiet = summarize(&[json!({ "at": 5, "outcome": ANSWERED })], 0);
    assert_eq!(quiet.guards, Guards::default());
    assert_eq!(quiet.controls, Controls::default());
}

/// A seat's graded answers counted per fifth of confidence — the curve a
/// band's lines are read off — and per band of the seat's own lines
/// (t-6342). A reading outside `0..=1` is counted nowhere; `1.0` is the top
/// fifth's.
#[test]
fn graded_answers_are_counted_per_fifth_of_confidence_and_per_band() {
    let graded = [
        (0.05, false),
        (0.39, true),
        (0.41, false),
        (0.59, true),
        (0.6, true),
        (0.84, false),
        (0.85, true),
        (1.0, true),
        (1.2, true),
        (f64::NAN, true),
    ];
    let curve = confidence_curve(graded);
    let tally = |marks, agreed| ConfidenceTally { marks, agreed };
    assert_eq!(
        curve,
        [
            tally(1, 0),
            tally(1, 1),
            tally(2, 1),
            tally(1, 1),
            tally(3, 2)
        ]
    );
    // The confidence-routing pattern's lines: under 0.6, 0.6 to 0.85, from
    // 0.85.
    assert_eq!(
        band_tally(&crate::jev::STALL, graded),
        Some([tally(4, 2), tally(2, 1), tally(2, 2)])
    );
    // A seat whose lines meet has nothing between.
    assert_eq!(
        band_tally(&crate::jev::PLACEMENT, graded),
        Some([tally(4, 2), tally(0, 0), tally(4, 3)])
    );
    assert_eq!(band_tally(&crate::jev::AGENT_TOOL, graded), None);
}
