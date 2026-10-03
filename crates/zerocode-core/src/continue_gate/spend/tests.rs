use super::*;

/// A model this table prices flat, so a call's dollars are arithmetic: a dollar
/// per million input tokens, five per million output, ten cents per million
/// read from the cache and a dollar and a quarter per million written to it.
const MODEL: &str = "claude-haiku-4-5";

fn record(message: &str, model: Option<&str>, tokens: [i64; 4]) -> String {
    let [input, output, cache_read, cache_write] = tokens;
    let mut message = serde_json::json!({
        "id": message,
        "usage": {
            "input_tokens": input,
            "output_tokens": output,
            "cache_read_input_tokens": cache_read,
            "cache_creation_input_tokens": cache_write,
        },
    });
    if let Some(model) = model {
        message["model"] = serde_json::json!(model);
    }
    serde_json::json!({
        "type": "assistant",
        "sessionId": "session-1",
        "timestamp": "2026-10-03T00:00:00.000Z",
        "uuid": format!("uuid-{message}"),
        "requestId": "request-1",
        "message": message,
    })
    .to_string()
}

fn priced(message: &str, tokens: [i64; 4]) -> String {
    record(message, Some(MODEL), tokens)
}

fn close(left: CallCost, dollars: f64) {
    let CallCost::Usd(usd) = left else {
        panic!("expected a price, got {left:?}");
    };
    assert!(
        (usd - dollars).abs() < 1e-9,
        "expected ${dollars}, got ${usd}"
    );
}

/// 1M in ($1.00) + 200k out ($1.00) + 5M cache read ($0.50).
const TWO_FIFTY: [i64; 4] = [1_000_000, 200_000, 5_000_000, 0];

#[test]
fn one_call_written_as_several_records_is_one_call_at_its_fullest_usage() {
    let mut reader = ClaudeFormat::default();
    let written = [
        priced("m1", [1_000_000, 10, 5_000_000, 0]),
        priced("m1", [1_000_000, 90_000, 5_000_000, 0]),
        priced("m1", TWO_FIFTY),
    ]
    .join("\n");
    assert!(
        reader.feed(&written).is_empty(),
        "the call is not finished while its records may still come"
    );
    let finished = reader.feed(&priced("m2", [1, 1, 1, 0]));
    assert_eq!(finished.len(), 1, "the next call finishes the one before");
    close(finished[0], 2.5);
}

#[test]
fn the_open_call_is_counted_once_a_quiet_stretch_flushes_it() {
    let mut reader = ClaudeFormat::default();
    assert!(reader.feed(&priced("m1", TWO_FIFTY)).is_empty());
    close(reader.flush().expect("the open call"), 2.5);
    assert_eq!(reader.flush(), None, "and not twice");
}

#[test]
fn a_late_record_of_a_finished_call_is_not_a_second_call() {
    let mut reader = ClaudeFormat::default();
    reader.feed(&priced("m1", TWO_FIFTY));
    close(reader.flush().expect("the open call"), 2.5);
    assert!(reader.feed(&priced("m1", TWO_FIFTY)).is_empty());
    assert_eq!(reader.flush(), None);
}

#[test]
fn what_is_not_a_model_call_costs_nothing() {
    let mut reader = ClaudeFormat::default();
    let user =
        r#"{"type":"user","sessionId":"session-1","message":{"role":"user","content":"hi"}}"#;
    let silent = priced("m0", [0, 0, 0, 0]);
    let written = [user, "not json at all", "", silent.as_str()].join("\n");
    assert!(reader.feed(&written).is_empty());
    assert_eq!(reader.flush(), None);
}

#[test]
fn a_model_with_no_price_is_counted_and_not_priced() {
    let mut reader = ClaudeFormat::default();
    reader.feed(&record("m1", Some("model-nobody-priced"), TWO_FIFTY));
    assert_eq!(reader.flush(), Some(CallCost::Unpriced));
    reader.feed(&record("m2", None, TWO_FIFTY));
    assert_eq!(reader.flush(), Some(CallCost::Unpriced));
    assert_eq!(CallCost::Unpriced.usd(), None);
    assert_eq!(CallCost::Usd(1.5).usd(), Some(1.5));
}

#[test]
fn a_call_that_names_its_model_late_is_priced_at_that_model() {
    let mut reader = ClaudeFormat::default();
    let written = [
        record("m1", None, [1_000_000, 10, 0, 0]),
        priced("m1", [1_000_000, 200_000, 5_000_000, 0]),
    ]
    .join("\n");
    reader.feed(&written);
    close(reader.flush().expect("the call"), 2.5);
}

/// Which CLIs a cost can be read for is a fact the board states, so it is
/// pinned: Claude Code's and Codex's formats are read, and every other CLI is
/// not yet.
#[test]
fn only_claude_codes_and_codexs_formats_have_a_reader_so_far() {
    assert!(reader_for("claude").is_some());
    assert!(reader_for("codex").is_some());
    for agent in [
        "zo",
        "antigravity",
        "kimi",
        "grok",
        "opencode",
        "cursor",
        "not-an-agent",
    ] {
        assert!(reader_for(agent).is_none(), "{agent} has no reader yet");
    }
}

/// One line of a Codex rollout: a record of `kind` carrying `payload`.
fn rollout(kind: &str, stamp: u32, payload: serde_json::Value) -> String {
    serde_json::json!({
        "timestamp": format!("2026-10-03T00:00:{stamp:02}.000Z"),
        "type": kind,
        "payload": payload,
    })
    .to_string()
}

fn session_meta() -> String {
    rollout(
        "session_meta",
        0,
        serde_json::json!({"id": "session-1", "cwd": "/Users/dev/repo"}),
    )
}

fn turn_context(model: &str) -> String {
    rollout(
        "turn_context",
        1,
        serde_json::json!({"cwd": "/Users/dev/repo", "model": model}),
    )
}

/// What a `token_count` event carries: the session's running total and the last
/// turn's own usage, each as `[input, cached input, output]`.
fn token_count(stamp: u32, total: [i64; 3], last: [i64; 3]) -> String {
    let usage = |[input, cached, output]: [i64; 3]| {
        serde_json::json!({
            "input_tokens": input,
            "cached_input_tokens": cached,
            "output_tokens": output,
            "reasoning_output_tokens": 0,
            "total_tokens": input + output,
        })
    };
    rollout(
        "event_msg",
        stamp,
        serde_json::json!({
            "type": "token_count",
            "info": {"total_token_usage": usage(total), "last_token_usage": usage(last)},
        }),
    )
}

/// A model this table prices flat: $1.25 per million input tokens, $0.125 per
/// million of them read from the cache, $10 per million output.
const CODEX_MODEL: &str = "gpt-5.1";

#[test]
fn a_codex_turn_is_priced_from_its_own_usage_and_not_from_the_running_total() {
    let mut reader = CodexFormat::default();
    let written = [
        session_meta(),
        turn_context(CODEX_MODEL),
        token_count(2, [1_000_000, 0, 0], [1_000_000, 0, 0]),
        token_count(3, [2_000_000, 0, 100_000], [1_000_000, 0, 100_000]),
    ]
    .join("\n");
    let finished = reader.feed(&written);
    assert_eq!(finished.len(), 2, "one call to a turn");
    close(finished[0], 1.25);
    close(finished[1], 2.25);
    assert_eq!(reader.flush(), None, "a turn is finished in its one line");
}

#[test]
fn cached_input_is_charged_at_the_cache_rate_once() {
    let mut reader = CodexFormat::default();
    let written = [
        turn_context(CODEX_MODEL),
        token_count(2, [1_000_000, 800_000, 0], [1_000_000, 800_000, 0]),
    ]
    .join("\n");
    let finished = reader.feed(&written);
    assert_eq!(finished.len(), 1);
    close(finished[0], 0.35);
}

#[test]
fn a_look_that_starts_mid_rollout_prices_the_turn_it_sees_and_not_what_came_before() {
    let mut reader = CodexFormat::default();
    let written = [
        turn_context(CODEX_MODEL),
        token_count(5, [50_000_000, 0, 0], [1_000_000, 0, 0]),
    ]
    .join("\n");
    let finished = reader.feed(&written);
    assert_eq!(finished.len(), 1);
    close(finished[0], 1.25);
}

#[test]
fn the_model_a_context_line_names_prices_the_turns_after_it_across_looks() {
    let mut reader = CodexFormat::default();
    assert!(
        reader
            .feed(&[session_meta(), turn_context(CODEX_MODEL)].join("\n"))
            .is_empty(),
        "context is not spend"
    );
    let finished = reader.feed(&token_count(2, [1_000_000, 0, 0], [1_000_000, 0, 0]));
    assert_eq!(finished.len(), 1);
    close(finished[0], 1.25);
}

#[test]
fn a_codex_model_with_no_price_is_counted_and_not_priced() {
    let mut reader = CodexFormat::default();
    let written = [
        turn_context("model-nobody-priced"),
        token_count(2, [1_000_000, 0, 0], [1_000_000, 0, 0]),
    ]
    .join("\n");
    assert_eq!(reader.feed(&written), vec![CallCost::Unpriced]);
    let nameless =
        CodexFormat::default().feed(&token_count(2, [1_000_000, 0, 0], [1_000_000, 0, 0]));
    assert_eq!(
        nameless,
        vec![CallCost::Unpriced],
        "no model seen is no price"
    );
}

#[test]
fn the_same_running_total_twice_is_one_turn_and_what_is_not_usage_costs_nothing() {
    let mut reader = CodexFormat::default();
    let ping = rollout(
        "event_msg",
        4,
        serde_json::json!({"type": "token_count", "info": null}),
    );
    let tool = rollout(
        "response_item",
        5,
        serde_json::json!({"type": "function_call_output", "output": "token_count in a result"}),
    );
    let same = token_count(2, [1_000_000, 0, 0], [1_000_000, 0, 0]);
    let written = [
        turn_context(CODEX_MODEL),
        same.clone(),
        same,
        ping,
        tool,
        "not json at all".to_string(),
        String::new(),
    ]
    .join("\n");
    let finished = reader.feed(&written);
    assert_eq!(finished.len(), 1, "a repeated reading is not a second turn");
    close(finished[0], 1.25);
}
