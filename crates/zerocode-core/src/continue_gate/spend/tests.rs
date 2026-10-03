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
/// pinned: Claude Code's format is read, and every other CLI is not yet.
#[test]
fn only_claude_codes_format_has_a_reader_so_far() {
    assert!(reader_for("claude").is_some());
    for agent in [
        "codex",
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
