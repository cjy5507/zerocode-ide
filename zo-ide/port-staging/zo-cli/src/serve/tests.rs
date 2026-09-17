use super::*;
use runtime::session::{ContentBlock, ConversationMessage, MessageRole, Session};
use std::sync::Arc as StdArc;

fn message(role: MessageRole, blocks: Vec<ContentBlock>) -> ConversationMessage {
    ConversationMessage {
        role,
        blocks,
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
            model: None,
    }
}

#[test]
fn project_history_flattens_roles_and_blocks() {
    let mut session = Session::new();
    session.messages = StdArc::new(vec![
        message(
            MessageRole::User,
            vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        ),
        message(
            MessageRole::Assistant,
            vec![
                ContentBlock::Text {
                    text: "on it".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "Bash".to_string(),
                    input: "{}".to_string(),
                },
            ],
        ),
        message(
            MessageRole::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                tool_name: "Bash".to_string(),
                output: "ok".to_string(),
                is_error: false,
                images: Vec::new(),
            }],
        ),
    ]);

    let history = project_history(&session);
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].text, "hello");
    assert_eq!(history[1].role, "assistant");
    assert_eq!(history[1].text, "on it\n⚙ Bash");
    assert_eq!(history[2].role, "tool");
    assert_eq!(history[2].text, "✓ Bash");
}

#[test]
fn tool_result_error_is_marked() {
    let mut session = Session::new();
    session.messages = StdArc::new(vec![message(
        MessageRole::Tool,
        vec![ContentBlock::ToolResult {
            tool_use_id: "t1".to_string(),
            tool_name: "Edit".to_string(),
            output: "boom".to_string(),
            is_error: true,
            images: Vec::new(),
        }],
    )]);
    let history = project_history(&session);
    assert_eq!(history[0].text, "✗ Edit");
}

/// End-to-end transport + dispatch + framing over a real loopback socket,
/// exercising only the paths that don't build a full session (so the test
/// stays fast and network-free): empty `session.list`, unknown method,
/// `run_turn` against a missing id, and a malformed request line.
// A flat sequence of routed round-trips over one socket; splitting it would
// re-establish the server/connection per case for no readability gain.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn dispatches_protocol_errors_over_a_socket() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    async fn round_trip(
        write_half: &mut tokio::net::tcp::OwnedWriteHalf,
        lines: &mut tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
        request: &str,
    ) -> serde_json::Value {
        write_half
            .write_all(request.as_bytes())
            .await
            .expect("write");
        write_half.write_all(b"\n").await.expect("newline");
        write_half.flush().await.expect("flush");
        let line = lines.next_line().await.expect("read").expect("line");
        serde_json::from_str(&line).expect("json")
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let jobs: JobMap = Arc::new(Mutex::new(HashMap::new()));
    let permission = SocketPrompterConfig::new();
    // F2: park a responder so a later `permission.respond` resolves it. The
    // map is an `Arc`, so the spawned server shares this entry.
    let (decision_tx, decision_rx) = tokio::sync::oneshot::channel();
    permission
        .responders
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(7, decision_tx);
    let config = Arc::new(ServeConfig {
        model: "test-model".to_string(),
        allowed_tools: None,
        permission_mode: PermissionMode::ReadOnly,
        auth: ServeAuthPolicy::open(),
    });
    let server = tokio::spawn(async move {
        let (stream, _peer) = listener.accept().await.expect("accept");
        let _ = handle_conn(stream, sessions, cancels, jobs, permission, pair::PairHub::default(), config).await;
    });

    let stream = TcpStream::connect(addr).await.expect("connect");
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    // 1) session.list on an empty pool → empty array.
    let list = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":1,"method":"session.list"}"#,
    )
    .await;
    assert_eq!(list["id"], 1);
    assert_eq!(list["result"]["sessions"], serde_json::json!([]));

    // 2) unknown method → method-not-found.
    let unknown = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":2,"method":"session.bogus"}"#,
    )
    .await;
    assert_eq!(unknown["error"]["code"], CODE_METHOD_NOT_FOUND);

    // 3) run_turn against a missing id → no-such-session (never builds one).
    let missing = round_trip(
            &mut write_half,
            &mut lines,
            r#"{"jsonrpc":"2.0","id":3,"method":"session.run_turn","params":{"id":"nope","input":"hi"}}"#,
        )
        .await;
    assert_eq!(missing["error"]["code"], CODE_NO_SUCH_SESSION);

    let detached_missing = round_trip(
            &mut write_half,
            &mut lines,
            r#"{"jsonrpc":"2.0","id":12,"method":"session.run_turn_detached","params":{"id":"nope","input":"hi","notify_url":"http://127.0.0.1:9/hook"}}"#,
        )
        .await;
    assert_eq!(detached_missing["error"]["code"], CODE_NO_SUCH_SESSION);

    let missing_job_status = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":13,"method":"session.job_status","params":{"job_id":999}}"#,
    )
    .await;
    assert_eq!(missing_job_status["result"]["status"], "missing");

    let missing_job_result = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":14,"method":"session.job_result","params":{"job_id":999}}"#,
    )
    .await;
    assert_eq!(missing_job_result["result"]["status"], "missing");

    // 3b) F3 meta RPCs against a missing id → no-such-session: proves each
    // new method is routed and looks the session up (none build a runtime
    // here, so this needs no auth).
    for (n, method, params) in [
        (
            4u64,
            "session.set_model",
            r#"{"id":"nope","model":"claude-opus-4-8"}"#,
        ),
        (
            5,
            "session.set_permission",
            r#"{"id":"nope","mode":"read-only"}"#,
        ),
        (6, "session.select_session", r#"{"id":"nope"}"#),
        (7, "session.rewind_checkpoint", r#"{"id":"nope"}"#),
        (8, "session.commit_push_pr", r#"{"id":"nope"}"#),
        (
            16,
            "session.connect_custom_provider",
            r#"{"id":"nope","name":"custom","base_url":"http://127.0.0.1:1/v1","models":["m"]}"#,
        ),
        (15, "session.close", r#"{"id":"nope"}"#),
    ] {
        let req = format!(r#"{{"jsonrpc":"2.0","id":{n},"method":"{method}","params":{params}}}"#);
        let resp = round_trip(&mut write_half, &mut lines, &req).await;
        assert_eq!(
            resp["error"]["code"], CODE_NO_SUCH_SESSION,
            "{method} on a missing id must report no-such-session"
        );
    }

    // 3c) set_model with a missing `model` field → invalid-params (rejected
    // before the session lookup).
    let bad = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":8,"method":"session.set_model","params":{"id":"x"}}"#,
    )
    .await;
    assert_eq!(bad["error"]["code"], CODE_INVALID_PARAMS);

    let bad_custom = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":17,"method":"session.connect_custom_provider","params":{"id":"x","name":"custom"}}"#,
    )
    .await;
    assert_eq!(bad_custom["error"]["code"], CODE_INVALID_PARAMS);

    // 3d) F4 cancel_turn for an unknown turn → cancelled:false (idempotent,
    // never panics, and is routed).
    let no_turn = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":9,"method":"session.cancel_turn","params":{"turn_id":999}}"#,
    )
    .await;
    assert_eq!(no_turn["result"]["cancelled"], false);

    // 3e) F2 permission.respond for an unknown prompt → resolved:false
    // (idempotent, never panics, and is routed).
    let no_prompt = round_trip(
            &mut write_half,
            &mut lines,
            r#"{"jsonrpc":"2.0","id":10,"method":"permission.respond","params":{"prompt_id":42,"decision":"allow_once"}}"#,
        )
        .await;
    assert_eq!(no_prompt["result"]["resolved"], false);

    // 3f) F2 permission.respond for the parked prompt 7 → resolved:true,
    // and the parked responder receives the mapped decision.
    let answered = round_trip(
            &mut write_half,
            &mut lines,
            r#"{"jsonrpc":"2.0","id":11,"method":"permission.respond","params":{"prompt_id":7,"decision":"allow_always"}}"#,
        )
        .await;
    assert_eq!(answered["result"]["resolved"], true);
    assert_eq!(
        decision_rx.await.expect("parked responder fired"),
        runtime::message_stream::PermissionDecision::AllowAlways,
        "the wire tag maps to the render decision the turn awaits"
    );

    // 4) malformed line → invalid-params answered against id 0.
    let malformed = round_trip(&mut write_half, &mut lines, "{not json").await;
    assert_eq!(malformed["id"], 0);
    assert_eq!(malformed["error"]["code"], CODE_INVALID_PARAMS);

    drop(write_half); // EOF → handle_conn returns
    server.await.expect("server task");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_close_persists_and_removes_session_from_pool() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    async fn round_trip(
        write_half: &mut tokio::net::tcp::OwnedWriteHalf,
        lines: &mut tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
        request: &str,
    ) -> serde_json::Value {
        write_half
            .write_all(request.as_bytes())
            .await
            .expect("write");
        write_half.write_all(b"\n").await.expect("newline");
        write_half.flush().await.expect("flush");
        let line = lines.next_line().await.expect("read").expect("line");
        serde_json::from_str(&line).expect("json")
    }

    let root = std::env::temp_dir().join(format!(
        "zo-close-session-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let dir = root.join("sessions");
    std::fs::create_dir_all(&dir).expect("sessions dir");
    let session = Session::new();
    let id = session.session_id.clone();
    session
        .save_to_path(dir.join(format!("{id}.jsonl")))
        .expect("persist session");

    let restored = {
        let _guard = crate::test_cwd_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var("ZO_SESSION_ROOT", &root);
        std::env::set_var("ANTHROPIC_API_KEY", "test-dummy-key-for-close");
        let restored = rehydrate_persisted_sessions(&rehydrate_config());
        std::env::remove_var("ZO_SESSION_ROOT");
        std::env::remove_var("ANTHROPIC_API_KEY");
        restored
    };
    assert_eq!(restored.len(), 1, "session should rehydrate");

    let sessions: SessionMap = Arc::new(Mutex::new(restored.into_iter().collect()));
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let jobs: JobMap = Arc::new(Mutex::new(HashMap::new()));
    let permission = SocketPrompterConfig::new();
    let config = Arc::new(rehydrate_config());

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let server_sessions = sessions.clone();
    let server = tokio::spawn(async move {
        let (stream, _peer) = listener.accept().await.expect("accept");
        let _ = handle_conn(stream, server_sessions, cancels, jobs, permission, pair::PairHub::default(), config).await;
    });

    let stream = TcpStream::connect(addr).await.expect("connect");
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    let close_req =
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"session.close","params":{{"id":"{id}"}}}}"#);
    let closed = round_trip(&mut write_half, &mut lines, &close_req).await;
    assert_eq!(closed["result"]["closed"], true);
    assert!(
        sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "close should remove the live session handle"
    );

    let load_req =
        format!(r#"{{"jsonrpc":"2.0","id":2,"method":"session.load","params":{{"id":"{id}"}}}}"#);
    let missing = round_trip(&mut write_half, &mut lines, &load_req).await;
    assert_eq!(missing["error"]["code"], CODE_NO_SUCH_SESSION);

    drop(write_half);
    server.await.expect("server task");
    let _ = std::fs::remove_dir_all(&root);
}

/// G21 auth gate end-to-end: a server built with a shared secret rejects a
/// request that omits the token and one that carries a wrong token (both
/// `CODE_UNAUTHORIZED`, before any handler runs), and admits a request whose
/// `token` field matches. Builds the `ServeConfig` directly with a token —
/// no env mutation — so the test is deterministic under parallel runs.
#[tokio::test]
async fn auth_gate_guards_every_request() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    async fn round_trip(
        write_half: &mut tokio::net::tcp::OwnedWriteHalf,
        lines: &mut tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
        request: &str,
    ) -> serde_json::Value {
        write_half
            .write_all(request.as_bytes())
            .await
            .expect("write");
        write_half.write_all(b"\n").await.expect("newline");
        write_half.flush().await.expect("flush");
        let line = lines.next_line().await.expect("read").expect("line");
        serde_json::from_str(&line).expect("json")
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let jobs: JobMap = Arc::new(Mutex::new(HashMap::new()));
    let permission = SocketPrompterConfig::new();
    let config = Arc::new(ServeConfig {
        model: "test-model".to_string(),
        allowed_tools: None,
        permission_mode: PermissionMode::ReadOnly,
        auth: ServeAuthPolicy::new(Some("s3cret".to_string()), None),
    });
    let hub = PairHub::default();
    let server_hub = hub.clone();
    let server = tokio::spawn(async move {
        let (stream, _peer) = listener.accept().await.expect("accept");
        let _ = handle_conn(stream, sessions, cancels, jobs, permission, server_hub, config).await;
    });

    let stream = TcpStream::connect(addr).await.expect("connect");
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    // 1) No token → unauthorized, even for an otherwise-harmless method.
    let missing = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":1,"method":"session.list"}"#,
    )
    .await;
    assert_eq!(missing["error"]["code"], CODE_UNAUTHORIZED);
    assert!(
        hub.roster("any-session")["peers"].as_array().expect("peers").is_empty(),
        "an unauthenticated socket must not enter the roster"
    );

    // 2) Wrong token → unauthorized.
    let wrong = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":2,"method":"session.list","token":"nope"}"#,
    )
    .await;
    assert_eq!(wrong["error"]["code"], CODE_UNAUTHORIZED);

    // 3) Correct token → the request reaches the handler (empty pool → []).
    let ok = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":3,"method":"session.list","token":"s3cret"}"#,
    )
    .await;
    assert_eq!(ok["id"], 3);
    assert_eq!(ok["result"]["sessions"], serde_json::json!([]));

    drop(write_half); // EOF → handle_conn returns
    server.await.expect("server task");
}

#[tokio::test]
async fn read_only_token_can_read_but_cannot_control_sessions() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};

    async fn round_trip(
        write_half: &mut tokio::net::tcp::OwnedWriteHalf,
        lines: &mut tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
        request: &str,
    ) -> serde_json::Value {
        write_half
            .write_all(request.as_bytes())
            .await
            .expect("write");
        write_half.write_all(b"\n").await.expect("newline");
        write_half.flush().await.expect("flush");
        let line = lines.next_line().await.expect("read").expect("line");
        serde_json::from_str(&line).expect("json")
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let jobs: JobMap = Arc::new(Mutex::new(HashMap::new()));
    let permission = SocketPrompterConfig::new();
    let config = Arc::new(ServeConfig {
        model: "test-model".to_string(),
        allowed_tools: None,
        permission_mode: PermissionMode::ReadOnly,
        auth: ServeAuthPolicy::new(
            Some("full-token".to_string()),
            Some("read-token".to_string()),
        ),
    });
    let server = tokio::spawn(async move {
        let (stream, _peer) = listener.accept().await.expect("accept");
        let _ = handle_conn(stream, sessions, cancels, jobs, permission, pair::PairHub::default(), config).await;
    });

    let stream = TcpStream::connect(addr).await.expect("connect");
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    let read_ok = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":1,"method":"session.list","token":"read-token"}"#,
    )
    .await;
    assert_eq!(read_ok["result"]["sessions"], serde_json::json!([]));

    let read_denied = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":2,"method":"session.cancel_turn","params":{"turn_id":7},"token":"read-token"}"#,
    )
    .await;
    assert_eq!(read_denied["error"]["code"], CODE_UNAUTHORIZED);
    assert!(read_denied["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("lacks capability"));

    let full_ok = round_trip(
        &mut write_half,
        &mut lines,
        r#"{"jsonrpc":"2.0","id":3,"method":"session.cancel_turn","params":{"turn_id":7},"token":"full-token"}"#,
    )
    .await;
    assert_eq!(full_ok["result"]["cancelled"], false);

    drop(write_half);
    server.await.expect("server task");
}

#[tokio::test]
async fn cancel_registry_signals_an_in_flight_turn() {
    // F4 plumbing: `run_turn` registers a oneshot under `(session_id, turn_id)`;
    // a `cancel_turn` removes + fires it, and the turn side observes the
    // signal (which, in `dispatch_run_turn`, closes the render channel and
    // cancels the turn). A second cancel finds nothing.
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let key = ("session-a".to_string(), 7u64);
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    cancels
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key.clone(), tx);

    let sender = cancels
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&key)
        .expect("turn 7 was registered");
    assert!(
        sender.send(()).is_ok(),
        "the run_turn side is still listening"
    );
    assert!(
        rx.try_recv().is_ok(),
        "the in-flight turn received the cancel signal"
    );
    assert!(
        cancels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key)
            .is_none(),
        "a second cancel for the same turn is a no-op"
    );
}

/// Build a `ConnWriter` backed by an in-memory channel and return the receiver
/// so a test can read the single response line a dispatch handler writes.
fn test_conn_writer() -> (ConnWriter, mpsc::Receiver<Arc<str>>) {
    let (tx, rx) = mpsc::channel::<Arc<str>>(8);
    (ConnWriter::new(tx), rx)
}

async fn read_dispatch_response(rx: &mut mpsc::Receiver<Arc<str>>) -> serde_json::Value {
    let line = rx.recv().await.expect("a response line");
    serde_json::from_str(line.trim_end()).expect("json response")
}


#[tokio::test]
async fn conn_writer_times_out_when_outbound_funnel_stalls() {
    let (tx, _rx) = mpsc::channel::<Arc<str>>(1);
    tx.send(Arc::from("first\n")).await.expect("fill funnel");
    let writer = ConnWriter::new(tx);
    let error = writer
        .send_with_timeout(Arc::from("blocked\n"), std::time::Duration::from_millis(1))
        .await
        .expect_err("a full funnel must not wait forever");
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}


#[test]
fn subscribe_permit_timeout_propagates_to_connection_teardown() {
    // Runtime construction during rehydration uses a synchronous bridge. Build
    // this fixture before entering Tokio so this regression remains runnable as
    // an isolated test rather than panicking in the bridge.
    let (id, sessions, root) = seed_one_session_pool();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        let hub = PairHub::default();
        hub.ensure_peer(1, "subscriber".to_string(), ServeCapability::Read);
        let (tx, _rx) = mpsc::channel::<Arc<str>>(1);
        tx.send(Arc::from("occupied\n"))
            .await
            .expect("fill outbound funnel");
        let writer = ConnWriter::new(tx);
        let (_writer_done_tx, mut writer_done) = oneshot::channel();
        let request = RpcRequest::new(1, "session.subscribe", serde_json::json!({ "id": id }));

        let error = dispatch_subscribe_with_timeout(
            &request,
            &sessions,
            &hub,
            1,
            &writer,
            &mut writer_done,
            std::time::Duration::from_millis(1),
        )
        .await
        .expect_err("a saturated subscribe ACK must fail promptly");
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(hub.viewer_count(&id), 0, "timed-out ACK never activates a subscriber");
    });
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn socket_write_timeout_signals_conn_read_loop() {
    let (socket, _peer) = tokio::io::duplex(1);
    let (tx, rx) = mpsc::channel::<Arc<str>>(1);
    tx.send(Arc::from("blocked socket write"))
        .await
        .expect("queue line");
    drop(tx);
    let error = drain_conn_writer(socket, rx, std::time::Duration::from_millis(1))
        .await
        .expect_err("unread socket must time out");
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let client = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let (server, _) = listener.accept().await.expect("accept");
    let (read_half, _write_half) = server.into_split();
    let (out_tx, _out_rx) = mpsc::channel::<Arc<str>>(1);
    let writer = ConnWriter::new(out_tx);
    let (done_tx, done_rx) = oneshot::channel();
    done_tx.send(()).expect("writer completion signal");

    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let jobs: JobMap = Arc::new(Mutex::new(HashMap::new()));
    let permission = SocketPrompterConfig::new();
    let hub = PairHub::default();
    let config = rehydrate_config();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        Box::pin(conn_read_loop(
            read_half,
            &sessions,
            &cancels,
            &jobs,
            &permission,
            &hub,
            &config,
            1,
            &writer,
            done_rx,
        )),
    )
    .await
    .expect("writer completion wakes pending socket read")
    .expect_err("writer completion terminates the read loop");
    assert_eq!(result.kind(), std::io::ErrorKind::BrokenPipe);
    drop(client);
}

#[tokio::test]
async fn drain_frames_broadcast_but_skip_helm_after_first_write_failure() {
    let hub = PairHub::default();
    hub.ensure_peer(1, "spectator".to_string(), ServeCapability::Read);
    let (spectator_tx, mut spectator_rx) = mpsc::channel::<Arc<str>>(8);
    let permit_sender = spectator_tx.clone();
    let permit = permit_sender.reserve().await.expect("reserve spectator ACK");
    hub.subscribe_with_permit(spectator_tx, permit, "session-a", 1, Some(Vec::new()), 1, true)
        .expect("subscribe");
    while spectator_rx.try_recv().is_ok() {}

    let (helm_tx, mut helm_rx) = mpsc::channel::<Arc<str>>(1);
    helm_tx
        .send(Arc::from("already queued\n"))
        .await
        .expect("fill helm funnel");
    let writer = ConnWriter::with_timeout(helm_tx, std::time::Duration::from_millis(1));
    let ids = BlockIdGen::default();
    let (block_tx, mut block_rx) = mpsc::channel(4);
    block_tx
        .send(RenderBlock::UserMessage {
            id: ids.next(),
            text: "first".to_string(),
        })
        .await
        .expect("first frame");
    block_tx
        .send(RenderBlock::UserMessage {
            id: ids.next(),
            text: "second".to_string(),
        })
        .await
        .expect("second frame");
    drop(block_tx);

    let error = drain_completed_turn_frames(&mut block_rx, "session-a", &hub, &writer, true)
        .await
        .expect_err("first helm write fails");
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(helm_rx.try_recv().is_ok(), "only the pre-filled helm line exists");
    assert!(helm_rx.try_recv().is_err(), "no later frame was sent to the helm");

    let mut broadcast_frames = 0;
    while broadcast_frames < 2 {
        let line = tokio::time::timeout(std::time::Duration::from_millis(100), spectator_rx.recv())
            .await
            .expect("spectator gets every drained frame")
            .expect("spectator channel open");
        let value: serde_json::Value = serde_json::from_str(line.trim_end()).expect("JSON frame");
        broadcast_frames += usize::from(value.get("frame_seq").is_some());
    }
}

#[tokio::test]
async fn dispatch_rejects_non_2_0_jsonrpc_with_invalid_request() {
    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let jobs: JobMap = Arc::new(Mutex::new(HashMap::new()));
    let permission = SocketPrompterConfig::new();
    let hub = PairHub::default();
    let config = rehydrate_config();
    let (writer, mut rx) = test_conn_writer();
    let (_writer_done_tx, mut writer_done) = oneshot::channel();

    let invalid = RpcRequest {
        jsonrpc: "1.0".to_string(),
        id: 7,
        method: "session.list".to_string(),
        params: serde_json::Value::Null,
        token: None,
    };
    Box::pin(dispatch(
        &invalid,
        &sessions,
        &cancels,
        &jobs,
        &permission,
        &hub,
        &config,
        1,
        &writer,
        &mut writer_done,
    ))
    .await
    .expect("dispatch invalid request");
    let rejected = read_dispatch_response(&mut rx).await;
    assert_eq!(rejected["error"]["code"], CODE_INVALID_REQUEST);
    assert_eq!(rejected["error"]["message"], "jsonrpc must be \"2.0\"");

    let valid = RpcRequest::new(8, "session.list", serde_json::Value::Null);
    Box::pin(dispatch(
        &valid,
        &sessions,
        &cancels,
        &jobs,
        &permission,
        &hub,
        &config,
        1,
        &writer,
        &mut writer_done,
    ))
    .await
    .expect("dispatch 2.0 request");
    let accepted = read_dispatch_response(&mut rx).await;
    assert_eq!(accepted["result"]["sessions"], serde_json::json!([]));
}

/// A scoped `cancel_turn` (with `session_id`) cancels exactly the named
/// session's turn and leaves a same-`turn_id` turn in another session running.
#[tokio::test]
async fn scoped_cancel_only_cancels_intended_session() {
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let (tx_a, mut rx_a) = tokio::sync::oneshot::channel::<()>();
    let (tx_b, mut rx_b) = tokio::sync::oneshot::channel::<()>();
    {
        let mut map = cancels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.insert(("session-a".to_string(), 7), tx_a);
        map.insert(("session-b".to_string(), 7), tx_b);
    }

    let request = RpcRequest::new(
        1,
        "session.cancel_turn",
        serde_json::json!({ "turn_id": 7, "session_id": "session-a" }),
    );
    let (writer, mut rx) = test_conn_writer();
    dispatch_cancel_turn(&request, &cancels, &writer)
        .await
        .expect("dispatch");
    let response = read_dispatch_response(&mut rx).await;
    assert_eq!(response["result"]["cancelled"], true);

    assert!(rx_a.try_recv().is_ok(), "session-a's turn was cancelled");
    assert!(
        rx_b.try_recv().is_err(),
        "session-b's same-turn_id turn is untouched"
    );
}

/// A legacy `cancel_turn` (no `session_id`) refuses to act when two sessions
/// share the `turn_id`, rather than cancelling the wrong one.
#[tokio::test]
async fn legacy_cancel_without_session_id_rejects_ambiguous() {
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let (tx_a, mut rx_a) = tokio::sync::oneshot::channel::<()>();
    let (tx_b, mut rx_b) = tokio::sync::oneshot::channel::<()>();
    {
        let mut map = cancels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.insert(("session-a".to_string(), 7), tx_a);
        map.insert(("session-b".to_string(), 7), tx_b);
    }

    let request = RpcRequest::new(1, "session.cancel_turn", serde_json::json!({ "turn_id": 7 }));
    let (writer, mut rx) = test_conn_writer();
    dispatch_cancel_turn(&request, &cancels, &writer)
        .await
        .expect("dispatch");
    let response = read_dispatch_response(&mut rx).await;
    assert_eq!(response["result"]["cancelled"], false);
    assert!(
        response["result"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("ambiguous"),
        "the collision is reported as ambiguous"
    );
    assert!(rx_a.try_recv().is_err(), "neither turn was cancelled (a)");
    assert!(rx_b.try_recv().is_err(), "neither turn was cancelled (b)");
    assert_eq!(
        cancels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        2,
        "both registrations survive an ambiguous refusal"
    );
}

/// A legacy `cancel_turn` (no `session_id`) still works when the `turn_id` is
/// unique across sessions.
#[tokio::test]
async fn legacy_cancel_without_session_id_works_when_unique() {
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let (tx_a, mut rx_a) = tokio::sync::oneshot::channel::<()>();
    cancels
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(("session-a".to_string(), 7), tx_a);

    let request = RpcRequest::new(1, "session.cancel_turn", serde_json::json!({ "turn_id": 7 }));
    let (writer, mut rx) = test_conn_writer();
    dispatch_cancel_turn(&request, &cancels, &writer)
        .await
        .expect("dispatch");
    let response = read_dispatch_response(&mut rx).await;
    assert_eq!(response["result"]["cancelled"], true);
    assert!(rx_a.try_recv().is_ok(), "the unique turn was cancelled");
}

#[test]
fn job_result_removes_terminal_jobs_but_keeps_running_jobs() {
    let jobs: JobMap = Arc::new(Mutex::new(HashMap::new()));
    insert_job(
        &jobs,
        1,
        JobHandle::running("session-1".to_string(), Instant::now()),
    );
    record_job_frame(
        &jobs,
        1,
        serde_json::json!({"type": "text_delta", "text": "hi"}),
    );

    let running = job_result_json(&jobs, 1);
    assert_eq!(running["status"], "running");
    assert_eq!(running["frame_count"], 1);
    assert!(
        jobs.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&1),
        "running job must not be removed by job_result"
    );

    let payload = finish_job_success(&jobs, 1, serde_json::json!({"iterations": 2}));
    assert_eq!(payload["status"], "done");
    assert_eq!(payload["result"]["iterations"], 2);

    let result = job_result_json(&jobs, 1);
    assert_eq!(result["status"], "done");
    assert_eq!(result["frames"].as_array().expect("frames").len(), 1);
    assert_eq!(job_status_json(&jobs, 1)["status"], "missing");
}

#[test]
fn job_frame_buffer_is_bounded() {
    let mut job = JobHandle::running("session-1".to_string(), Instant::now());
    for index in 0..(MAX_JOB_FRAMES + 3) {
        job.push_frame(serde_json::json!({ "index": index }));
    }

    assert_eq!(job.frames.len(), MAX_JOB_FRAMES);
    assert_eq!(job.frames.front().expect("front")["index"], 3);
    assert_eq!(
        job.frames.back().expect("back")["index"],
        (MAX_JOB_FRAMES + 2) as u64
    );
}

#[test]
fn job_prune_removes_expired_completed_and_caps_oldest_terminal_jobs() {
    let now = Instant::now();
    let mut map = HashMap::new();
    let mut expired = JobHandle::running("old".to_string(), now);
    let expired_at = now
        .checked_sub(JOB_TTL + Duration::from_secs(1))
        .expect("test timestamp stays within Instant range");
    expired.finish_success(serde_json::json!({}), expired_at);
    map.insert(1, expired);

    let running_id = 2;
    map.insert(
        running_id,
        JobHandle::running(
            "running".to_string(),
            now.checked_sub(Duration::from_secs(10))
                .expect("test timestamp stays within Instant range"),
        ),
    );

    for index in 0..(MAX_JOBS + 5) {
        let id = 10_000 + index as u64;
        let completed_at = now
            .checked_sub(Duration::from_secs((MAX_JOBS + 5 - index) as u64))
            .expect("test timestamp stays within Instant range");
        let mut job = JobHandle::running(format!("done-{index}"), now);
        job.finish_success(serde_json::json!({ "index": index }), completed_at);
        map.insert(id, job);
    }

    prune_jobs_locked(&mut map, now);

    assert!(!map.contains_key(&1), "expired terminal job is swept");
    assert!(map.contains_key(&running_id), "running jobs are not swept");
    assert!(
        map.len() <= MAX_JOBS,
        "completed job cap is enforced without evicting running work"
    );
    assert!(
        map.contains_key(&(10_000 + (MAX_JOBS + 4) as u64)),
        "newest completed job survives cap pruning"
    );
}

fn rehydrate_config() -> ServeConfig {
    ServeConfig {
        model: "claude-opus-4-8".to_string(),
        allowed_tools: None,
        permission_mode: PermissionMode::DangerFullAccess,
        auth: ServeAuthPolicy::open(),
    }
}

/// A corrupt transcript is skipped (logged, not fatal) and the server still
/// boots — rehydration never panics on bad input. No live `LiveCli` is
/// built (the load fails first), so this needs no auth.
#[test]
fn rehydrate_skips_corrupt_session_files() {
    let _guard = crate::test_cwd_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = std::env::temp_dir().join(format!(
        "zo-f1-corrupt-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let dir = root.join("sessions");
    std::fs::create_dir_all(&dir).expect("sessions dir");
    std::fs::write(dir.join("broken.jsonl"), "{ this is not valid json").expect("corrupt file");

    std::env::set_var("ZO_SESSION_ROOT", &root);
    let restored = rehydrate_persisted_sessions(&rehydrate_config());
    std::env::remove_var("ZO_SESSION_ROOT");
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        restored.is_empty(),
        "corrupt transcript must be skipped, got {} session(s)",
        restored.len()
    );
}

/// End-to-end: a persisted Project transcript is rebuilt into a live pool
/// entry under its original id. Builds the runtime tower, so it needs an
/// auth source — a dummy `ANTHROPIC_API_KEY` satisfies construction (no
/// network call happens until a turn runs). Called from a plain `#[test]`
/// (no ambient tokio runtime), so `build_runtime` does not hit the
/// nested-runtime panic that forces `spawn_blocking` on the server path.
#[test]
fn rehydrate_restores_persisted_session_under_its_id() {
    let _guard = crate::test_cwd_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = std::env::temp_dir().join(format!(
        "zo-f1-restore-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let dir = root.join("sessions");
    std::fs::create_dir_all(&dir).expect("sessions dir");
    let session = Session::new();
    let id = session.session_id.clone();
    session
        .save_to_path(dir.join(format!("{id}.jsonl")))
        .expect("persist session");

    std::env::set_var("ZO_SESSION_ROOT", &root);
    std::env::set_var("ANTHROPIC_API_KEY", "test-dummy-key-for-construction");
    let restored = rehydrate_persisted_sessions(&rehydrate_config());
    std::env::remove_var("ZO_SESSION_ROOT");
    std::env::remove_var("ANTHROPIC_API_KEY");
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(restored.len(), 1, "exactly the one persisted session");
    assert_eq!(restored[0].0, id, "restored under its original id");
}

// --- Track 5: pair-session socket E2E ---------------------------------------
//
// `BufReader`, `OwnedWriteHalf`, `TcpListener`, `TcpStream`, the async IO
// traits, `PairHub`, and the `pair` module are all in scope via `use super::*`.

use tokio::net::tcp::OwnedReadHalf;

type SocketLines = tokio::io::Lines<BufReader<OwnedReadHalf>>;

/// Accept connections forever on `listener`, handling each with a **shared**
/// pair hub so fan-out crosses connections. Returns the hub and the accept task
/// (aborted by the test at teardown).
fn spawn_pair_server(
    listener: TcpListener,
    sessions: SessionMap,
    config: Arc<ServeConfig>,
) -> (PairHub, tokio::task::JoinHandle<()>) {
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let jobs: JobMap = Arc::new(Mutex::new(HashMap::new()));
    let permission = SocketPrompterConfig::new();
    let hub = PairHub::default();
    let hub_for_server = hub.clone();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((stream, _peer)) = listener.accept().await else {
                break;
            };
            let sessions = sessions.clone();
            let cancels = cancels.clone();
            let jobs = jobs.clone();
            let permission = permission.clone();
            let hub = hub_for_server.clone();
            let config = config.clone();
            tokio::spawn(async move {
                let _ = handle_conn(stream, sessions, cancels, jobs, permission, hub, config).await;
            });
        }
    });
    (hub, handle)
}

async fn connect(addr: std::net::SocketAddr) -> (OwnedWriteHalf, SocketLines) {
    let stream = TcpStream::connect(addr).await.expect("connect");
    let (read_half, write_half) = stream.into_split();
    (write_half, BufReader::new(read_half).lines())
}

async fn send(write_half: &mut OwnedWriteHalf, request: &str) {
    write_half
        .write_all(request.as_bytes())
        .await
        .expect("write");
    write_half.write_all(b"\n").await.expect("newline");
    write_half.flush().await.expect("flush");
}

/// Read one line as JSON.
async fn read_json(lines: &mut SocketLines) -> serde_json::Value {
    let line = lines.next_line().await.expect("read").expect("line");
    serde_json::from_str(&line).expect("json")
}

/// Read lines until a JSON-RPC response arrives, returning it plus any push
/// frames (roster/resync/render) seen before it.
async fn read_response(lines: &mut SocketLines) -> serde_json::Value {
    // Subscription roster controls may remain queued after their ACK. Consume
    // controls until the response for the subsequent request arrives.
    loop {
        let value = read_json(lines).await;
        if crate::serve_protocol::is_response_line(&value) {
            return value;
        }
    }
}

/// Build one real (turnless) session in a fresh pool, following the rehydration
/// pattern the close test uses. No turn runs, so no network call happens.
fn seed_one_session_pool() -> (String, SessionMap, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "zo-pair-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let dir = root.join("sessions");
    std::fs::create_dir_all(&dir).expect("sessions dir");
    let session = Session::new();
    let id = session.session_id.clone();
    session
        .save_to_path(dir.join(format!("{id}.jsonl")))
        .expect("persist session");

    let restored = {
        let _guard = crate::test_cwd_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var("ZO_SESSION_ROOT", &root);
        std::env::set_var("ANTHROPIC_API_KEY", "test-dummy-key-for-pair");
        let restored = rehydrate_persisted_sessions(&rehydrate_config());
        std::env::remove_var("ZO_SESSION_ROOT");
        std::env::remove_var("ANTHROPIC_API_KEY");
        restored
    };
    assert_eq!(restored.len(), 1, "session should rehydrate");
    let sessions: SessionMap = Arc::new(Mutex::new(restored.into_iter().collect()));
    (id, sessions, root)
}

#[test]
fn boundary_subscribe_aborts_when_writer_is_already_dead() {
    let (id, sessions, root) = seed_one_session_pool();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        let hub = PairHub::default();
        hub.ensure_peer(1, "subscriber".to_string(), ServeCapability::Read);
        let (tx, _rx) = mpsc::channel::<Arc<str>>(1);
        let writer = ConnWriter::new(tx);
        let (writer_done_tx, mut writer_done) = oneshot::channel();
        writer_done_tx.send(()).expect("writer exits");
        let request = RpcRequest::new(
            1,
            "session.subscribe",
            serde_json::json!({ "id": id, "boundary": true, "resync_v2": true }),
        );
        // Hold the session mutex so the boundary branch is genuinely waiting
        // in `handle.lock()` when the writer completion is observed.
        let handle = lookup(&sessions, &id).expect("seeded handle");
        let _held_guard = handle.lock().await;
        let error = dispatch_subscribe_with_timeout(
            &request,
            &sessions,
            &hub,
            1,
            &writer,
            &mut writer_done,
            std::time::Duration::from_secs(1),
        )
        .await
        .expect_err("writer death wins before the boundary lock");
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(hub.viewer_count(&id), 0, "dead writer cannot activate a subscriber");
    });
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn acquire_live_guard_rejects_removed_arc_before_meta_side_effects() {
    let (id, sessions, root) = seed_one_session_pool();
    let handle = lookup(&sessions, &id).expect("seeded handle");
    sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&id);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        assert!(acquire_live_guard(&sessions, &id, &handle).await.is_err());
    });
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn run_blocking_meta_rejects_close_raced_handle_before_side_effect() {
    let (id, sessions, root) = seed_one_session_pool();
    let handle = lookup(&sessions, &id).expect("seeded handle");
    let side_effect_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");

    runtime.block_on(async {
        // Keep the blocking worker behind the same session mutex while its
        // task is scheduled, then make close win the map identity race.
        let guard = handle.lock().await;
        let task = tokio::spawn({
            let sessions = Arc::clone(&sessions);
            let id = id.clone();
            let handle = Arc::clone(&handle);
            let side_effect_ran = Arc::clone(&side_effect_ran);
            async move {
                run_blocking_meta(&sessions, &id, handle, move |_| {
                    side_effect_ran.store(true, std::sync::atomic::Ordering::Release);
                })
                .await
            }
        });
        tokio::task::yield_now().await;
        sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id);
        drop(guard);

        assert_eq!(
            task.await.expect("meta task"),
            Err(format!("no such session: {id}")),
        );
    });
    assert!(
        !side_effect_ran.load(std::sync::atomic::Ordering::Acquire),
        "stale identity must be rejected before the blocking closure runs"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Run/subscribe paths share this identity predicate, so a stale handle cannot
/// register against a ghost `PairHub` channel after close.
#[test]
fn live_session_identity_rejects_removed_or_recreated_arc() {
    let old = Arc::new(());
    let replacement = Arc::new(());
    let mut map = HashMap::from([("session-a".to_string(), Arc::clone(&old))]);

    assert!(session_handle_is_current(&map, "session-a", &old));
    map.remove("session-a");
    assert!(
        !session_handle_is_current(&map, "session-a", &old),
        "removed session invalidates a stale run/subscribe Arc"
    );

    map.insert("session-a".to_string(), Arc::clone(&replacement));
    assert!(
        !session_handle_is_current(&map, "session-a", &old),
        "same id with a new Arc must not revive a close-raced operation"
    );
    assert!(session_handle_is_current(&map, "session-a", &replacement));
}

/// Two connections subscribe to the same session over real sockets. The second
/// subscribe fans a roster control frame to the first (proving fan-out crosses
/// connections onto a spectator's socket), `session.roster` lists both peers,
/// and a `session.steer` with no in-flight turn is refused with `STEER_DENIED`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pair_spectators_receive_fanned_roster_and_steer_needs_a_turn() {
    let (id, sessions, root) = seed_one_session_pool();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let config = Arc::new(rehydrate_config());
    let (_hub, server) = spawn_pair_server(listener, sessions, config);

    // Spectator A subscribes: response carries an empty history, seq 0, no helm.
    let (mut wa, mut la) = connect(addr).await;
    send(
        &mut wa,
        &format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"session.subscribe","params":{{"id":"{id}"}}}}"#
        ),
    )
    .await;
    let sub_a = read_response(&mut la).await;
    assert_eq!(sub_a["result"]["next_seq"], 0);
    assert_eq!(sub_a["result"]["helm"], serde_json::Value::Null);
    assert_eq!(sub_a["result"]["history"], serde_json::json!([]));

    // Spectator B subscribes.
    let (mut wb, mut lb) = connect(addr).await;
    send(
        &mut wb,
        &format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"session.subscribe","params":{{"id":"{id}"}}}}"#
        ),
    )
    .await;
    let sub_b = read_response(&mut lb).await;
    assert_eq!(sub_b["result"]["id"], id);

    // A receives a fanned roster frame reflecting the two peers — a real push
    // frame arriving on a spectator socket with no request outstanding.
    let mut roster_frame = read_json(&mut la).await;
    while roster_frame["type"] != "roster" || roster_frame["viewers"] != serde_json::json!(2) {
        roster_frame = read_json(&mut la).await;
    }

    // `session.roster` on a third connection lists both spectators.
    let (mut wc, mut lc) = connect(addr).await;
    send(
        &mut wc,
        &format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"session.roster","params":{{"id":"{id}"}}}}"#
        ),
    )
    .await;
    let roster = read_response(&mut lc).await;
    let peers = roster["result"]["peers"].as_array().expect("peers");
    assert!(peers.len() >= 2, "roster lists the connected peers: {peers:?}");

    // Steering with no in-flight turn is denied (there is nothing to steer).
    send(
        &mut wc,
        &format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"session.steer","params":{{"id":"{id}","text":"go left"}}}}"#
        ),
    )
    .await;
    let steer = read_response(&mut lc).await;
    assert_eq!(steer["error"]["code"], CODE_STEER_DENIED);

    // B unsubscribes; A sees the roster shrink to one viewer.
    send(
        &mut wb,
        &format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"session.unsubscribe","params":{{"id":"{id}"}}}}"#
        ),
    )
    .await;
    let unsub = read_response(&mut lb).await;
    assert_eq!(unsub["result"]["unsubscribed"], true);
    let mut saw_shrink = false;
    for _ in 0..8 {
        let frame = read_json(&mut la).await;
        if frame["type"] == "roster" && frame["viewers"] == serde_json::json!(1) {
            saw_shrink = true;
            break;
        }
    }
    assert!(saw_shrink, "A should observe B leaving via a roster frame");

    server.abort();
    let _ = std::fs::remove_dir_all(&root);
}

/// The read tier spectates but cannot steer or drive: `subscribe`/`roster` pass
/// the capability gate on a read-only token, while `steer`/`run_turn` are
/// refused UNAUTHORIZED before any handler runs (track 5 §2.7).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_token_spectates_but_cannot_steer() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));
    let config = Arc::new(ServeConfig {
        model: "test-model".to_string(),
        allowed_tools: None,
        permission_mode: PermissionMode::ReadOnly,
        auth: ServeAuthPolicy::new(Some("full-token".to_string()), Some("read-token".to_string())),
    });
    let (_hub, server) = spawn_pair_server(listener, sessions, config);

    let (mut w, mut l) = connect(addr).await;

    // Read token may reach `session.roster` (Read tier).
    send(
        &mut w,
        r#"{"jsonrpc":"2.0","id":1,"method":"session.roster","params":{"id":"none"},"token":"read-token"}"#,
    )
    .await;
    let roster = read_response(&mut l).await;
    assert_eq!(roster["result"]["type"], "roster");

    // Read token may reach `session.subscribe` (Read tier); missing id → NO_SUCH.
    send(
        &mut w,
        r#"{"jsonrpc":"2.0","id":2,"method":"session.subscribe","params":{"id":"none"},"token":"read-token"}"#,
    )
    .await;
    let sub = read_response(&mut l).await;
    assert_eq!(sub["error"]["code"], CODE_NO_SUCH_SESSION);

    // Read token is blocked from `session.steer` (Full tier) before any handler.
    send(
        &mut w,
        r#"{"jsonrpc":"2.0","id":3,"method":"session.steer","params":{"id":"none","text":"x"},"token":"read-token"}"#,
    )
    .await;
    let steer = read_response(&mut l).await;
    assert_eq!(steer["error"]["code"], CODE_UNAUTHORIZED);

    // Full token may steer routing-wise; no turn → STEER_DENIED (not UNAUTHORIZED).
    send(
        &mut w,
        r#"{"jsonrpc":"2.0","id":4,"method":"session.steer","params":{"id":"none","text":"x"},"token":"full-token"}"#,
    )
    .await;
    let steer_full = read_response(&mut l).await;
    assert_eq!(steer_full["error"]["code"], CODE_STEER_DENIED);

    server.abort();
}
