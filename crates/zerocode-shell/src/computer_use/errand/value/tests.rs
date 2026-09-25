//! The value seat's writer over a real socket: what it sends, with which key,
//! and what it refuses. No keychain is asked — every key store here is the
//! test's own map — and every case has an endpoint on a port of its own.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::type_value::{ANTHROPIC_WIRE, FieldLook, ValueRefusal, asked, chosen, render};

use super::*;
use crate::api_routers::{HeldKeys, RouterKeys};
use crate::systemone::tests::Endpoint;
use crate::systemone::{SCHEMA, TIMEOUT, UNAUTHORIZED};

/// A key a person put in the window's key store for the chosen row.
pub(in crate::computer_use::errand) const KEY: &str = "a-key-a-person-set";

/// What a machine with a subscription login keeps, where the vendor's own
/// client keeps it — the login this seat never reads, never sends.
pub(in crate::computer_use::errand) const SUBSCRIPTION_ITEM: &str = "Claude Code-credentials";
pub(in crate::computer_use::errand) const SUBSCRIPTION: &str =
    r#"{"claudeAiOauth":{"accessToken":"a-subscription-login","refreshToken":"r"}}"#;

/// A key store holding a subscription login, and — `with_key` — the key a
/// person set for the chosen row where the row says it lives.
pub(in crate::computer_use::errand) fn store(with_key: bool) -> Box<dyn RouterKeys> {
    let keys = HeldKeys::default();
    keys.write(SUBSCRIPTION_ITEM, SUBSCRIPTION)
        .expect("a subscription login held");
    if with_key {
        let service = key_service(chosen().expect("a chosen row")).expect("a row names its key");
        keys.write(&service, KEY).expect("the person's key held");
    }
    Box::new(keys)
}

/// The words around one box, as a walk's look reads them.
fn look() -> FieldLook<'static> {
    FieldLook {
        goal: "Find a flight from Zurich to London",
        label: "To",
        placeholder: "City or airport",
        near: "Arrival airport",
    }
}

/// What the Messages endpoint answers when it wrote `text`.
pub(in crate::computer_use::errand) fn wrote(text: &str) -> String {
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

fn writer_at(endpoint: &Endpoint, with_key: bool) -> LiveWriter {
    LiveWriter::at(&format!("{}/v1/messages", endpoint.base()), store(with_key))
}

/// A person's key is the only key the seat asks with: it rides the row's own
/// key header and nothing else does — no `Authorization`, no subscription
/// beta, no line speaking for another client — and the body is the table's
/// own question: its instructions as the system, the rendered field as the
/// one user line.
#[test]
fn the_value_seat_asks_with_the_key_a_person_set_and_speaks_for_no_client() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 0);
    let mut writer = writer_at(&endpoint, true);
    assert!(writer.ready(), "a person set a key");
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
    assert!(
        head.contains(&format!("{}: {KEY}", ANTHROPIC_WIRE.key_header)),
        "the person's key rides its header:\n{head}"
    );
    assert!(head.contains(&format!("anthropic-version: {}", ANTHROPIC_WIRE.version)));
    for absent in ["authorization:", "anthropic-beta:"] {
        assert!(!head.contains(absent), "`{absent}` was sent:\n{head}");
    }
    assert!(!body.contains(KEY), "the key rode the body");
    let sent: Value = serde_json::from_str(body).expect("a json body");
    assert_eq!(sent["model"], json!(row.model));
    assert_eq!(
        sent["system"],
        json!(asked().instructions),
        "the table's words, nothing else"
    );
    assert_eq!(sent["messages"][0]["role"], json!("user"));
    assert_eq!(sent["messages"][0]["content"], json!(render(&look())));
    assert!(
        sent["max_tokens"].as_u64().is_some_and(|most| most > 0),
        "a bound on the value's length"
    );
    let printed = format!("{written:?}");
    assert!(
        !printed.contains(KEY) && !printed.contains("London"),
        "{printed}"
    );
}

/// A machine that holds only a subscription login has no value seat: the
/// writer is not ready, it asks nothing, and the login is on no request.
/// (The walk's side of it — no entry offered — is the goal world's test.)
#[test]
fn a_subscription_login_alone_sets_up_no_writer() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 0);
    let mut writer = writer_at(&endpoint, false);
    assert!(
        !writer.ready(),
        "a subscription login is not a key a person set"
    );
    assert_eq!(
        writer
            .write(&look(), Duration::from_secs(5))
            .map(|written| written.ms)
            .expect_err("no key, no value"),
        NO_KEY
    );
    assert!(endpoint.asked().is_empty(), "a request left without a key");
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
        let refused = writer_at(&endpoint, true)
            .write(&look(), Duration::from_secs(5))
            .map(|written| written.ms)
            .expect_err("no value");
        assert_eq!(refused, token, "{status} {body}");
    }

    let slow = Endpoint::serving("HTTP/1.1 200 OK", wrote("London"), 800);
    let began = Instant::now();
    let refused = writer_at(&slow, true)
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
    let refused = writer_at(&idle, true)
        .write(&look(), Duration::ZERO)
        .map(|written| written.ms)
        .expect_err("no time left");
    assert_eq!(refused, TIMEOUT);
    assert!(
        idle.asked().is_empty(),
        "a walk with no time left asked anyway"
    );
}

/// A row this product does not take is no writer however its key is set: a
/// road that would have the request speak as another client, and one not
/// built here.
#[test]
fn a_road_that_speaks_for_another_client_is_never_taken() {
    for row in zerocode_core::type_value::rows() {
        let taken = endpoint_of(row).is_some();
        if row.client_fingerprint.is_some() {
            assert!(!taken, "{} speaks as another client", row.id);
        }
        if row.road == Road::CodeAssist {
            assert!(!taken, "{} is not built here", row.id);
        }
    }
    let chosen = chosen().expect("a chosen row");
    assert!(
        endpoint_of(chosen).is_some(),
        "the chosen row's road is taken"
    );
    assert!(
        key_service(chosen).is_some(),
        "the chosen row names its key"
    );
}

/// The value seat's own sources never speak for a client they are not and
/// never read a subscription login: none of the words such a request is made
/// of is in them.
#[test]
fn the_value_seats_sources_speak_for_no_other_client() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for file in [
        "crates/zerocode-core/src/type_value.rs",
        "crates/zerocode-shell/src/computer_use/errand.rs",
        "crates/zerocode-shell/src/computer_use/errand/desk.rs",
        "crates/zerocode-shell/src/computer_use/errand/value.rs",
    ] {
        let source = std::fs::read_to_string(root.join(file)).expect("the seat's source");
        for word in [
            "You are Claude Code",
            "claudeAiOauth",
            "oauth-2025",
            "anthropic-beta",
            "usage_login",
            "claude_access_token",
        ] {
            assert!(!source.contains(word), "{file} spells `{word}`");
        }
    }
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

/// The value seat timed on its real road, one process, three writes: the
/// first pays for the client and the connection, the rest ride its socket.
/// A measurement, printed; the key is the one a person hands this command's
/// environment, never read from a keychain and never printed.
#[test]
#[ignore = "spends a person's own API key on the value seat's real road; a measurement"]
fn the_value_seat_timed_on_its_real_road() {
    let key = std::env::var("ZEROCODE_VALUE_PROBE_KEY").expect("a key a person set");
    let keys = HeldKeys::default();
    let service = key_service(chosen().expect("a chosen row")).expect("its key's name");
    keys.write(&service, &key).expect("held");
    let mut writer = LiveWriter::at(ANTHROPIC_WIRE.url, Box::new(keys));
    for pass in 1..=3 {
        let began = Instant::now();
        let written = writer.write(&look(), Duration::from_secs(10));
        println!(
            "pass {pass}: {:?} in {} ms",
            written.as_ref().map(|written| written.value.as_str()),
            began.elapsed().as_millis()
        );
    }
}
