use super::*;

#[test]
fn equal_task_names_in_different_runs_do_not_share_jev_usage() {
    let mut book = JevBook::default();
    for (run, tokens) in [("first", 100), ("second", 200)] {
        book.read(
            &json!({"at": 1, "run": run, "task": "task", "outcome": "answered",
            "requests": 1, "inputTokens": tokens}),
        );
    }
    assert_eq!(book.tally_in("first", "task").input_tokens, 100);
    assert_eq!(book.tally_in("second", "task").input_tokens, 200);
    book.read(
        &json!({"at": 1, "task": "task", "outcome": "answered", "requests": 1, "inputTokens": 300}),
    );
    let unscoped = book.tally_in("first", "task");
    assert!(unscoped.rows_with_tokens < unscoped.rows);
    assert_eq!(unscoped.input_tokens, 100);
}

#[test]
fn session_lifetime_before_after_or_between_attempts_is_not_a_task_bill() {
    let held = run(
        json!([attempt("dp-1", "t-1", "w-1", 10_000, Some(20_000))]),
        json!([worker("w-1", "claude", Some("conversation"))]),
        json!([]),
    );
    for (first, last, expected) in [
        (10_000, 20_000, true),
        (9_999, 15_000, false),
        (15_000, 20_001, false),
    ] {
        let mut session = claude_session("conversation", OPUS, [100, 10, 0, 0]);
        session.first_timestamp = crate::civil::iso_utc_of(first);
        session.last_timestamp = crate::civil::iso_utc_of(last);
        let sessions = book_of(vec![session]);
        let cost = super::super::task_cost(
            &held,
            "t-1",
            &sessions,
            JevTally::default(),
            &SessionAttribution::new([&held]),
        );
        assert_eq!(cost.generation.measured_tokens().is_some(), expected);
        if !expected {
            assert_eq!(cost.generation.usd_reason, Some(UsdReason::OutsideAttempt));
        }
    }
    let mut split = held.clone();
    split.dispatches[0].ended_ms = Some(12_000);
    let mut second = split.dispatches[0].clone();
    second.id = "dp-2".into();
    second.started_ms = 18_000;
    second.ended_ms = Some(20_000);
    split.dispatches.push(second);
    let mut session = claude_session("conversation", OPUS, [100, 10, 0, 0]);
    session.first_timestamp = crate::civil::iso_utc_of(11_000);
    session.last_timestamp = crate::civil::iso_utc_of(19_000);
    let cost = super::super::task_cost(
        &split,
        "t-1",
        &book_of(vec![session]),
        JevTally::default(),
        &SessionAttribution::new([&split]),
    );
    assert_eq!(cost.generation.usd_reason, Some(UsdReason::OutsideAttempt));
}

#[test]
fn a_clock_reversal_in_any_attempt_is_unknown_not_a_cheap_elapsed_time() {
    for dispatches in [
        json!([attempt("dp-1", "t-1", "w-1", 20, Some(10))]),
        json!([
            attempt("dp-1", "t-1", "w-1", 20, Some(10)),
            attempt("dp-2", "t-1", "w-2", 5, Some(30))
        ]),
        json!([attempt("dp-1", "t-1", "w-1", i64::MIN, Some(i64::MAX))]),
    ] {
        let held = run(dispatches, json!([]), json!([]));
        assert_eq!(
            task_cost(&held, "t-1", &SessionBook::default(), JevTally::default()).wall_ms,
            None
        );
    }
}

#[test]
fn indexed_fingerprints_change_only_when_a_runs_usage_attribution_changes() {
    let held = run(
        json!([attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE))]),
        json!([worker("w-1", "claude", Some("first"))]),
        json!([]),
    );
    let mut other = run(
        json!([attempt("dp-2", "t-2", "w-2", 0, Some(MINUTE))]),
        json!([worker("w-2", "claude", Some("second"))]),
        json!([]),
    );
    other.id = "another-run".into();
    let original = SessionAttribution::new([&held, &other]);
    let expected = original.generation_fingerprint(&held.id);
    assert!(expected.is_some());
    assert_eq!(original.generation_fingerprint("absent-run"), None);
    other.dispatches[0].ended_ms = Some(2 * MINUTE);
    let unrelated = SessionAttribution::new([&held, &other]);
    assert_eq!(unrelated.generation_fingerprint(&held.id), expected);
    other.dispatches[0]
        .session_history
        .as_mut()
        .unwrap()
        .sessions[0]
        .id = "first".into();
    let shared = SessionAttribution::new([&held, &other]);
    assert_ne!(shared.generation_fingerprint(&held.id), expected);
}

#[test]
fn session_key_aliases_share_one_vendor_bill_and_one_ownership_claim() {
    let mut dispatch = with_history(
        attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE)),
        &["conv-alias", "conv-alias"],
    );
    dispatch["session_history"]["sessions"][1]["key"] = json!("conversation_id");
    let held = run(
        json!([dispatch]),
        json!([worker("w-1", "codex", Some("conv-alias"))]),
        json!([]),
    );
    let mut book = SessionBook::default();
    book.read_codex(
        &vendor(vec![codex_session("conv-alias", &[SOL], 100, 20, 40)]),
        READ_AT,
    );
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    assert_eq!(cost.generation.sessions_known, 1);
    assert_eq!(cost.generation.measured_tokens(), Some(140));

    let mut other = held.clone();
    other.id = "another-run".into();
    other.dispatches[0]
        .session_history
        .as_mut()
        .unwrap()
        .sessions
        .remove(0);
    let attribution = SessionAttribution::new([&held, &other]);
    let shared = super::super::task_cost(&held, "t-1", &book, JevTally::default(), &attribution);
    assert_eq!(shared.generation.usd_reason, Some(UsdReason::Unlinked));
    assert_eq!(shared.generation.measured_tokens(), None);
}

#[test]
fn invalid_vendor_counters_cannot_wrap_into_a_bill() {
    let held = run(
        json!([attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE))]),
        json!([worker("w-1", "codex", Some("conv-invalid"))]),
        json!([]),
    );
    for (input, cached, output, reasoning) in [
        (i64::MAX, i64::MIN, 0, 0),
        (100, 101, 40, 0),
        (100, 0, 40, 41),
        (100, 0, -1, 0),
    ] {
        let mut session = codex_session("conv-invalid", &[SOL], 0, 0, 0);
        session.input_tokens = input;
        session.cached_input_tokens = cached;
        session.output_tokens = output;
        session.reasoning_output_tokens = reasoning;
        let mut book = SessionBook::default();
        book.read_codex(&vendor(vec![session]), READ_AT);
        let cost = task_cost(&held, "t-1", &book, JevTally::default());
        assert_eq!(cost.generation.usd_reason, Some(UsdReason::InvalidUsage));
        assert_eq!(cost.generation.measured_tokens(), None);
    }
}

#[test]
fn aggregate_overflow_keeps_the_total_unknown() {
    let held = run(
        json!([with_history(
            attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE)),
            &["first", "second"]
        )]),
        json!([worker("w-1", "claude", Some("second"))]),
        json!([]),
    );
    let book = book_of(vec![
        claude_session("first", OPUS, [i64::MAX, 0, 0, 0]),
        claude_session("second", OPUS, [1, 0, 0, 0]),
    ]);
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    assert_eq!(cost.generation.usd_reason, Some(UsdReason::InvalidUsage));
    assert_eq!(cost.generation.usd, None);
    assert_eq!(cost.generation.measured_tokens(), None);
}

#[test]
fn invalid_reported_currency_and_reasoning_overflow_stay_unknown() {
    let held = run(
        json!([attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE))]),
        json!([worker("w-1", "opencode", Some("conv-invalid"))]),
        json!([]),
    );
    for usd in [f64::INFINITY, f64::NAN, -1.0] {
        let mut reported = codex_session("conv-invalid", &[SOL], 100, 0, 20);
        reported.estimated_cost_usd = Some(usd);
        let mut book = SessionBook::default();
        book.read_opencode(&vendor(vec![reported]), READ_AT);
        let cost = task_cost(&held, "t-1", &book, JevTally::default());
        assert_eq!(cost.generation.usd_reason, Some(UsdReason::InvalidUsage));
        assert_eq!(cost.generation.measured_tokens(), None);
    }
    let mut reported = codex_session("conv-invalid", &[SOL], 0, 0, 0);
    reported.output_tokens = i64::MAX;
    reported.reasoning_output_tokens = 1;
    reported.estimated_cost_usd = Some(0.25);
    let mut book = SessionBook::default();
    book.read_opencode(&vendor(vec![reported]), READ_AT);
    assert_eq!(
        task_cost(&held, "t-1", &book, JevTally::default())
            .generation
            .usd_reason,
        Some(UsdReason::InvalidUsage)
    );
}
