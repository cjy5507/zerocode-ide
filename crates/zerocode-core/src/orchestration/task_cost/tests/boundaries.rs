use super::*;

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
