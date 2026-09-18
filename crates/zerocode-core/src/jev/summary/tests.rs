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
    assert!((tally.answered_share().expect("rows") - 0.25).abs() < f64::EPSILON);
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

#[test]
fn the_rows_a_use_owes_before_its_next_judgment_count_down_and_wrap() {
    let owed = |rows: usize| {
        Tally {
            rows,
            ..Tally::default()
        }
        .rows_to_next_judgment()
    };
    assert_eq!(owed(0), JUDGED_EVERY_ROWS);
    assert_eq!(owed(1), JUDGED_EVERY_ROWS - 1);
    assert_eq!(owed(JUDGED_EVERY_ROWS - 1), 1);
    assert_eq!(
        owed(JUDGED_EVERY_ROWS),
        JUDGED_EVERY_ROWS,
        "a judgment just made owes a full window"
    );
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
