//! The value seat's writer over a real socket: what it sends, where its
//! login goes, and what it refuses. No keychain is asked — every login here
//! is handed in — and every case has an endpoint on a port of its own.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::type_value::{ANTHROPIC_WIRE, FieldLook, ValueRefusal, asked, chosen, render};

use super::*;
use crate::systemone::tests::Endpoint;
use crate::systemone::{SCHEMA, TIMEOUT, UNAUTHORIZED};

/// A login handed in, shaped like the subscription's.
const LOGIN: &str = "a-login-handed-in-by-this-test";

/// The words around one box, as a walk's look reads them.
fn look() -> FieldLook<'static> {
    FieldLook {
        goal: "Find a flight from Zurich to London",
        label: "To",
        placeholder: "City or airport",
        near: "Arrival airport",
    }
}

/// What the endpoint answers when it wrote `text`.
fn wrote(text: &str) -> String {
    json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "model": chosen().expect("a chosen row").model,
        "content": [{ "type": "text", "text": text }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 40, "output_tokens": 3 },
    })
    .to_string()
}

fn writer_at(endpoint: &Endpoint) -> LiveWriter {
    LiveWriter::at(&format!("{}/v1/messages", endpoint.base()), LOGIN)
}

/// The window's writer asks the seat's chosen row the table's own question,
/// with the login in the `Authorization` header and nowhere else: not in the
/// body, not in what the writer's value prints. The system lines are the
/// subscription road's identity, then the question's instructions; the one
/// user line is the rendered field.
#[test]
fn the_windows_writer_asks_with_its_login_in_the_header_alone() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 0);
    let mut writer = writer_at(&endpoint);
    let written = writer
        .write(&look(), Duration::from_secs(5))
        .expect("a value");
    assert_eq!(written.value, "London");
    let row = chosen().expect("a chosen row");
    assert_eq!(written.model, row.model);

    let heard = endpoint.asked();
    assert_eq!(heard.len(), 1, "one request for one value");
    let (head, body) = heard[0].split_once("\r\n\r\n").expect("a head and a body");
    let head = head.to_ascii_lowercase();
    for line in [
        format!("authorization: bearer {}", LOGIN.to_ascii_lowercase()),
        format!("anthropic-beta: {}", ANTHROPIC_WIRE.beta),
        format!("anthropic-version: {}", ANTHROPIC_WIRE.version),
    ] {
        assert!(head.contains(&line), "the request lost `{line}`:\n{head}");
    }
    assert!(!body.contains(LOGIN), "the login rode the body");
    let sent: Value = serde_json::from_str(body).expect("a json body");
    assert_eq!(sent["model"], json!(row.model));
    assert_eq!(sent["system"][0]["text"], json!(ANTHROPIC_WIRE.identity));
    assert_eq!(sent["system"][1]["text"], json!(asked().instructions));
    assert_eq!(sent["messages"][0]["role"], json!("user"));
    assert_eq!(sent["messages"][0]["content"], json!(render(&look())));
    assert!(
        sent["max_tokens"].as_u64().is_some_and(|most| most > 0),
        "a bound on the value's length"
    );
    let printed = format!("{written:?}");
    assert!(
        !printed.contains(LOGIN) && !printed.contains("London"),
        "{printed}"
    );
}

/// An answer that is no value says why in a word of its own — the wire's
/// for a refusal or a body nothing reads, the seat's rule for a value it
/// would not type — and a writer that has not answered by the wall the walk
/// gave it is no value either; a walk with no time left asks nothing.
#[test]
fn a_refused_or_misshapen_answer_is_no_value_and_says_why() {
    for (status, body, token) in [
        (
            "HTTP/1.1 401 Unauthorized",
            "{}".to_string(),
            UNAUTHORIZED.to_string(),
        ),
        (
            "HTTP/1.1 200 OK",
            "not json".to_string(),
            SCHEMA.to_string(),
        ),
        (
            "HTTP/1.1 200 OK",
            json!({ "content": [] }).to_string(),
            SCHEMA.to_string(),
        ),
        (
            "HTTP/1.1 200 OK",
            wrote("London\nThat is the arrival city."),
            ValueRefusal::NotOneLine.token().to_string(),
        ),
        (
            "HTTP/1.1 200 OK",
            wrote("   "),
            ValueRefusal::Empty.token().to_string(),
        ),
    ] {
        let endpoint = Endpoint::serving(status, body.clone(), 0);
        let refused = writer_at(&endpoint)
            .write(&look(), Duration::from_secs(5))
            .map(|written| written.ms)
            .expect_err("no value");
        assert_eq!(refused, token, "{status} {body}");
    }

    let slow = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 800);
    let began = Instant::now();
    let refused = writer_at(&slow)
        .write(&look(), Duration::from_millis(200))
        .map(|written| written.ms)
        .expect_err("past its wall");
    assert_eq!(refused, TIMEOUT);
    assert!(
        began.elapsed() < Duration::from_millis(700),
        "{:?}",
        began.elapsed()
    );

    let idle = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 0);
    let refused = writer_at(&idle)
        .write(&look(), Duration::ZERO)
        .map(|written| written.ms)
        .expect_err("no time left");
    assert_eq!(refused, TIMEOUT);
    assert!(
        idle.asked().is_empty(),
        "a walk with no time left asked anyway"
    );
}

/// The window's memory keeps a value per identity, as many as the longest
/// walk the verb allows can write, the oldest out first; keeping an identity
/// again replaces its value without growing.
#[test]
fn the_memory_keeps_the_longest_walks_values_oldest_out_first() {
    let mut values = Values::new(2);
    values.keep("a".to_string(), "1".to_string());
    values.keep("b".to_string(), "2".to_string());
    values.keep("c".to_string(), "3".to_string());
    assert_eq!(values.recall("a"), None, "the oldest went first");
    assert_eq!(values.recall("b").as_deref(), Some("2"));
    assert_eq!(values.recall("c").as_deref(), Some("3"));
    values.keep("c".to_string(), "4".to_string());
    assert_eq!(values.recall("c").as_deref(), Some("4"));
    assert_eq!(
        values.recall("b").as_deref(),
        Some("2"),
        "a replaced value grows nothing"
    );
    assert_eq!(
        held(&window_values()).cap(),
        zerocode_core::computer_use::WALK_STEPS_MAX
    );
}
