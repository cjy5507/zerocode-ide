//! The harness wire contract is mirrored by hand from a server whose types are
//! crate-private, so these tests are the only thing standing between us and a
//! silent protocol drift.

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use zerocode_harness::{
    Client, HarnessError, Incoming, ServeErrorKind, classify, first_question_in, method,
};

#[test]
fn the_jsonrpc_key_alone_decides_response_versus_render_frame() {
    let response = classify(r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#).expect("classify");
    assert!(matches!(response, Incoming::Response { id: 7, .. }));

    let frame = classify(r#"{"type":"text_delta","text":"hello"}"#).expect("classify");
    assert!(matches!(frame, Incoming::Frame(_)));

    // A frame that happens to carry an `id` is still a frame.
    let frame = classify(r#"{"type":"tool_use","id":"toolu_1"}"#).expect("classify");
    assert!(matches!(frame, Incoming::Frame(_)));
}

/// The session name comes out of the history, and the history's schema belongs
/// to a server whose types we cannot import. So the extraction is tolerant by
/// construction, and these pin what that tolerance covers — including the shapes
/// that must yield nothing rather than a wrong name.
#[test]
fn the_first_question_is_read_from_whatever_shape_the_history_uses() {
    // Plain strings: the first entry is what was asked.
    assert_eq!(
        first_question_in(&json!(["드레인 게이트 봉인 리팩터링", "on it"])).as_deref(),
        Some("드레인 게이트 봉인 리팩터링")
    );

    // The shape the server really sends — `{role, text}`, roles drawn from
    // `system | user | assistant | tool`. A transcript opens with the system
    // prompt, so entry zero is exactly the wrong answer.
    assert_eq!(
        first_question_in(&json!([
            { "role": "system", "text": "you are zo" },
            { "role": "assistant", "text": "hello" },
            { "role": "user", "text": "fix the flaky drain test" },
            { "role": "user", "text": "and then run it" },
        ]))
        .as_deref(),
        Some("fix the flaky drain test")
    );
    assert_eq!(
        first_question_in(&json!([{ "role": "tool", "text": "bash: ok" }])),
        None
    );

    // An untagged object is still usable — refusing it would leave every
    // session showing its id.
    assert_eq!(
        first_question_in(&json!([{ "content": "  add lane rails  " }])).as_deref(),
        Some("add lane rails")
    );
}

#[test]
fn a_history_that_names_nothing_yields_no_name_rather_than_a_wrong_one() {
    assert_eq!(first_question_in(&json!([])), None);
    assert_eq!(first_question_in(&Value::Null), None);
    assert_eq!(first_question_in(&json!({ "not": "an array" })), None);
    assert_eq!(first_question_in(&json!(["   ", "\t\n"])), None);
    assert_eq!(
        first_question_in(&json!([{ "role": "assistant", "text": "hello" }])),
        None
    );
    // A shape we have never seen must degrade quietly, not guess.
    assert_eq!(
        first_question_in(&json!([{ "unknown_future_key": "fix the tests" }])),
        None
    );
}

#[test]
fn an_unknown_frame_type_passes_through_untouched() {
    let raw = r#"{"type":"some_future_block","payload":{"nested":[1,2,3]}}"#;
    let Incoming::Frame(frame) = classify(raw).expect("classify") else {
        panic!("expected a frame");
    };
    assert_eq!(frame, serde_json::from_str::<Value>(raw).expect("parse"));
}

#[test]
fn a_line_that_is_not_json_is_an_error_rather_than_a_silent_drop() {
    assert!(matches!(
        classify("not json at all"),
        Err(HarnessError::Malformed(_))
    ));
}

#[test]
fn every_documented_error_code_maps_to_its_product_meaning() {
    assert_eq!(
        ServeErrorKind::from_code(-32600),
        ServeErrorKind::InvalidRequest
    );
    assert_eq!(
        ServeErrorKind::from_code(-32601),
        ServeErrorKind::MethodNotFound
    );
    assert_eq!(
        ServeErrorKind::from_code(-32602),
        ServeErrorKind::InvalidParams
    );
    assert_eq!(ServeErrorKind::from_code(-32603), ServeErrorKind::Internal);
    assert_eq!(
        ServeErrorKind::from_code(-32000),
        ServeErrorKind::NoSuchSession
    );
    assert_eq!(ServeErrorKind::from_code(-32001), ServeErrorKind::Cancelled);
    assert_eq!(
        ServeErrorKind::from_code(-32002),
        ServeErrorKind::Unauthorized
    );
    assert_eq!(
        ServeErrorKind::from_code(-32003),
        ServeErrorKind::SteerDenied
    );
    assert_eq!(ServeErrorKind::from_code(-32004), ServeErrorKind::HelmHeld);
    assert_eq!(ServeErrorKind::from_code(-1), ServeErrorKind::Unknown(-1));
}

#[test]
fn helm_and_steer_refusals_are_contention_not_failure() {
    assert!(ServeErrorKind::HelmHeld.is_contention());
    assert!(ServeErrorKind::SteerDenied.is_contention());
    assert!(!ServeErrorKind::Internal.is_contention());
    assert!(!ServeErrorKind::NoSuchSession.is_contention());
}

/// Accept one connection, hand the first request line to `respond`, and write
/// back whatever lines it returns.
async fn mock_server(
    respond: impl FnOnce(Value) -> Vec<String> + Send + 'static,
) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let (read, mut write) = stream.into_split();
        let mut reader = BufReader::new(read);
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read request");
        let request: Value = serde_json::from_str(line.trim()).expect("request json");
        for mut out in respond(request) {
            out.push('\n');
            write.write_all(out.as_bytes()).await.expect("write");
        }
        write.flush().await.expect("flush");
    });
    addr
}

#[tokio::test]
async fn a_turn_streams_its_frames_and_then_returns_the_result() {
    let addr = mock_server(|request| {
        assert_eq!(request["method"], method::RUN_TURN);
        assert_eq!(request["params"]["id"], "session-1");
        assert_eq!(request["params"]["input"], "hello");
        let id = request["id"].as_u64().expect("id");
        vec![
            json!({"type":"text_delta","text":"thinking"}).to_string(),
            json!({"type":"brand_new_block","whatever":true}).to_string(),
            json!({"jsonrpc":"2.0","id":id,"result":{"stop_reason":"end_turn"}}).to_string(),
        ]
    })
    .await;

    let mut client = Client::connect(&addr.to_string(), Some("t".into()))
        .await
        .expect("connect");
    let mut frames = Vec::new();
    let result = client
        .run_turn("session-1", "hello", None, &mut |frame| frames.push(frame))
        .await
        .expect("turn");

    assert_eq!(result["stop_reason"], "end_turn");
    assert_eq!(frames.len(), 2, "both frames, including the unknown one");
    assert_eq!(frames[1]["type"], "brand_new_block");
}

/// The structured channel: after subscribing, frames arrive with no request
/// outstanding — that is how the IDE learns what the terminal lane's own
/// `zo attach` is doing without reading the screen.
#[tokio::test]
async fn subscribing_streams_fan_out_frames_with_no_request_outstanding() {
    let addr = mock_server(|request| {
        assert_eq!(request["method"], method::SUBSCRIBE);
        assert_eq!(request["params"]["id"], "session-1");
        assert_eq!(request["params"]["boundary"], true);
        let id = request["id"].as_u64().expect("id");
        vec![
            json!({"jsonrpc":"2.0","id":id,"result":{"next_seq":7}}).to_string(),
            json!({"type":"text_delta","text":"from another client"}).to_string(),
            json!({"type":"permission_prompt","prompt_id":3}).to_string(),
        ]
    })
    .await;

    let mut client = Client::connect(&addr.to_string(), None)
        .await
        .expect("connect");
    let ack = client
        .subscribe("session-1", true)
        .await
        .expect("subscribe");
    assert_eq!(ack["next_seq"], 7);

    let Some(Incoming::Frame(first)) = client.next_incoming().await.expect("frame") else {
        panic!("expected a fan-out frame");
    };
    assert_eq!(first["text"], "from another client");

    let Some(Incoming::Frame(second)) = client.next_incoming().await.expect("frame") else {
        panic!("expected a fan-out frame");
    };
    assert_eq!(second["type"], "permission_prompt");
    assert_eq!(second["prompt_id"], 3);

    // A closed server is a reconnect signal, not an error.
    assert!(client.next_incoming().await.expect("eof").is_none());
}

#[tokio::test]
async fn the_token_is_stamped_on_every_request_when_the_server_is_guarded() {
    let addr = mock_server(|request| {
        assert_eq!(request["token"], "shared-secret");
        let id = request["id"].as_u64().expect("id");
        vec![json!({"jsonrpc":"2.0","id":id,"result":{}}).to_string()]
    })
    .await;

    let mut client = Client::connect(&addr.to_string(), Some("shared-secret".into()))
        .await
        .expect("connect");
    client
        .call(method::LIST, json!({}))
        .await
        .expect("list should succeed");
}

#[tokio::test]
async fn a_held_helm_comes_back_as_contention_with_its_code_intact() {
    let addr = mock_server(|request| {
        let id = request["id"].as_u64().expect("id");
        vec![
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32004, "message": "another client holds the helm" }
            })
            .to_string(),
        ]
    })
    .await;

    let mut client = Client::connect(&addr.to_string(), None)
        .await
        .expect("connect");
    let error = client
        .run_turn("session-1", "hello", Some(9), &mut |_| {})
        .await
        .expect_err("must fail");

    let HarnessError::Rpc {
        code, kind, method, ..
    } = error
    else {
        panic!("expected an rpc error, got {error:?}");
    };
    assert_eq!(code, -32004);
    assert_eq!(kind, ServeErrorKind::HelmHeld);
    assert!(kind.is_contention());
    assert_eq!(method, zerocode_harness::method::RUN_TURN);
}

#[tokio::test]
async fn a_server_that_hangs_up_mid_request_is_reported_as_closed() {
    let addr = mock_server(|_| Vec::new()).await;
    let mut client = Client::connect(&addr.to_string(), None)
        .await
        .expect("connect");
    assert!(matches!(
        client.call(method::INFO, json!({"id":"s"})).await,
        Err(HarnessError::Closed)
    ));
}

#[tokio::test]
async fn capabilities_are_an_additive_rpc_and_typed_history_does_not_rename_the_session() {
    let receipt = json!({"protocol":{"name":"zo-events","major":1,"minor":0},"revision":7,
        "process":{"instance_id":"fixture-process","pid":42,"build":{"git_sha":null}},
        "identity":{"channel_session_id":"endpoint","session_id":"active"}});
    let expected = receipt.clone();
    let addr = mock_server(move |request| {
        assert_eq!(request["method"], "session.capabilities");
        vec![json!({"jsonrpc":"2.0","id":request["id"],"result":receipt}).to_string()]
    })
    .await;
    let mut client = Client::connect(&addr.to_string(), None)
        .await
        .expect("connect");
    assert_eq!(
        client
            .call("session.capabilities", json!({}))
            .await
            .expect("receipt"),
        expected
    );
    let mut frame = expected;
    frame["type"] = json!("session_capabilities");
    frame["seq"] = json!(9);
    assert!(matches!(
        classify(&frame.to_string()).expect("typed frame"),
        Incoming::Frame(_)
    ));
    assert_eq!(
        first_question_in(&json!([{"role":"user","text":"hello"}, frame])).as_deref(),
        Some("hello")
    );
}

#[tokio::test]
async fn an_old_server_refusing_capabilities_still_accepts_baseline_cancel() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listen");
    let addr = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        let (read, mut write) = socket.into_split();
        let mut read = BufReader::new(read);
        for expected in ["session.capabilities", method::CANCEL_TURN] {
            let mut line = String::new();
            read.read_line(&mut line).await.expect("request");
            let request: Value = serde_json::from_str(&line).expect("json");
            assert_eq!(request["method"], expected);
            let response = if expected == "session.capabilities" {
                json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32601,"message":"unknown method"}})
            } else {
                json!({"jsonrpc":"2.0","id":request["id"],"result":{"ok":true}})
            };
            write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("reply");
        }
    });
    let mut client = Client::connect(&addr.to_string(), None)
        .await
        .expect("connect");
    assert!(matches!(
        client.call("session.capabilities", json!({})).await,
        Err(HarnessError::Rpc {
            kind: ServeErrorKind::MethodNotFound,
            ..
        })
    ));
    assert_eq!(
        client
            .call(method::CANCEL_TURN, json!({}))
            .await
            .expect("baseline preserved")["ok"],
        true
    );
    server.await.expect("server");
}
