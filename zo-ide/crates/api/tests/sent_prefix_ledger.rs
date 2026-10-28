//! Z10 (adversarial review A, 2026-10-28): the request ledger records what the
//! server saw — a hash and length of the prefix each `cache_control` marker
//! closed, read off the bytes actually sent, and the `input_transformations`
//! the response named — and a read drop the request's own fingerprints call a
//! pure append is not filed as "prefix unchanged" when either says otherwise.
//!
//! Driven through the real client against a local server that answers with
//! canned SSE, the way `client_integration.rs` does; nothing leaves the
//! machine.

use std::sync::{Mutex, OnceLock};

use api::{
    AnthropicClient, InputContentBlock, InputMessage, MessageRequest, PromptCache, PromptCachePaths,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Serve `responses` in order, one connection each, reading every request
/// whole before answering it. `None` where binding a socket is not allowed.
async fn serve(responses: Vec<String>) -> Option<(String, tokio::task::JoinHandle<()>)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return None,
        Err(error) => panic!("listener should bind: {error}"),
    };
    let address = listener.local_addr().expect("local address");
    let handle = tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 8192];
            let body_start = loop {
                let read = socket.read(&mut chunk).await.expect("read");
                assert!(read > 0, "request ended inside its headers");
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&buffer[..body_start]).to_ascii_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |value| value.trim().parse().expect("content length"));
            while buffer.len() < body_start + length {
                let read = socket.read(&mut chunk).await.expect("read body");
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
            socket.write_all(response.as_bytes()).await.expect("write");
        }
    });
    Some((format!("http://{address}"), handle))
}

fn sse_reply(cache_read: u32, transformations: &str) -> String {
    let body = format!(
        concat!(
            "event: message_start\n",
            "data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-3-7-sonnet-latest\",\"stop_reason\":null,\"stop_sequence\":null{transformations},\"usage\":{{\"input_tokens\":3,\"cache_creation_input_tokens\":40,\"cache_read_input_tokens\":{read},\"output_tokens\":0}}}}}}\n\n",
            "event: message_delta\n",
            "data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\",\"stop_sequence\":null}},\"usage\":{{\"input_tokens\":3,\"cache_creation_input_tokens\":40,\"cache_read_input_tokens\":{read},\"output_tokens\":5}}}}\n\n",
            "event: message_stop\n",
            "data: {{\"type\":\"message_stop\"}}\n\n",
            "data: [DONE]\n\n"
        ),
        read = cache_read,
        transformations = transformations,
    );
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

const DROPPED_THINKING: &str = ",\"input_transformations\":[{\"type\":\"thinking_dropped\",\"path\":\"messages.1.content.0\",\"reason\":\"prefix_binding_mismatch\"}]";

fn text(role: &str, text: &str, marked: bool) -> InputMessage {
    InputMessage {
        role: role.to_string(),
        content: vec![InputContentBlock::Text {
            text: text.to_string(),
            cache_control: marked.then(api::CacheControl::ephemeral),
        }],
        ..InputMessage::user_text(String::new())
    }
}

fn request(messages: Vec<InputMessage>, thinking: Option<api::ThinkingConfig>) -> MessageRequest {
    MessageRequest {
        model: "claude-3-7-sonnet-latest".to_string(),
        max_tokens: 64,
        messages,
        system: None,
        tools: None,
        tool_choice: None,
        stream: false,
        thinking,
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    }
}

struct Ledger {
    reason: String,
    rows: Vec<serde_json::Value>,
}

/// Send `first` then `second` through one client and read what the ledger
/// wrote for them.
async fn record(session: &str, replies: Vec<String>, first: MessageRequest, second: MessageRequest) -> Option<Ledger> {
    let home = std::env::temp_dir().join(format!(
        "api-sent-prefix-{session}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::env::set_var("ZO_CONFIG_HOME", &home);
    let (base_url, server) = serve(replies).await?;
    let client = AnthropicClient::new("test-key")
        .with_base_url(base_url)
        .with_prompt_cache(PromptCache::new(session));
    for request in [first, second] {
        let mut stream = client.stream_message(&request).await.expect("stream starts");
        while stream.next_event().await.expect("event parses").is_some() {}
    }
    server.abort();
    let reason = client
        .prompt_cache_stats()
        .and_then(|stats| stats.last_break_reason)
        .unwrap_or_default();
    let requests = std::fs::read_to_string(PromptCachePaths::for_session(session).requests_path)
        .expect("request ledger");
    let rows = requests
        .lines()
        .map(|line| serde_json::from_str(line).expect("ledger row"))
        .collect();
    std::fs::remove_dir_all(&home).ok();
    std::env::remove_var("ZO_CONFIG_HOME");
    Some(Ledger { reason, rows })
}

/// The server dropped a replayed thinking block before the model saw the
/// prompt, and reads fell on what the fingerprints call a pure append. The
/// ledger must name the rewrite, not "prefix unchanged", and every row must
/// carry the sent prefix hashes.
#[tokio::test]
// Single-threaded test; the env lock across `.await` only serialises env access.
#[allow(clippy::await_holding_lock)]
async fn a_drop_the_server_rewrote_is_not_filed_as_an_unchanged_prefix() {
    let _guard = env_lock();
    let first = request(vec![text("user", "first prompt", true)], None);
    let second = request(
        vec![
            text("user", "first prompt", true),
            text("assistant", "first answer", false),
            text("user", "second prompt", true),
        ],
        None,
    );
    let Some(ledger) = record(
        "sent-prefix-transformed",
        vec![sse_reply(9_000, ""), sse_reply(1_000, DROPPED_THINKING)],
        first,
        second,
    )
    .await
    else {
        return;
    };

    assert!(!ledger.reason.is_empty(), "the read drop is a break");
    assert!(
        !ledger.reason.contains("prefix unchanged"),
        "a rewritten prompt was filed as an unchanged prefix: {}",
        ledger.reason
    );
    assert!(ledger.reason.contains("input_transformations"), "{}", ledger.reason);
    assert_eq!(ledger.rows.len(), 2, "{:?}", ledger.rows);
    for row in &ledger.rows {
        assert!(row.get("prefix_hashes").is_some_and(|hashes| hashes.as_array().is_some_and(|all| !all.is_empty())), "row without prefix hashes: {row}");
    }
    assert_eq!(ledger.rows[1]["input_transformations"], 1, "{}", ledger.rows[1]);
    assert_eq!(
        ledger.rows[1]["input_transformation_kinds"],
        serde_json::json!(["thinking_dropped:prefix_binding_mismatch"]),
        "{}",
        ledger.rows[1]
    );
}

/// The second request turns thinking off, so the client strips the replayed
/// thinking block from the history it sends (`strip_thinking_blocks_when_disabled`)
/// — after the fingerprints were taken, which therefore still see a pure
/// append. The prefix hash at the marker both requests carry tells the truth,
/// and the request-parameter line in front of it (thinking itself changed)
/// is the first change named (adversarial review W3, P3-3).
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_drop_after_the_client_rewrote_the_sent_bytes_names_the_first_changed_marker() {
    let _guard = env_lock();
    let thought = || InputMessage {
        role: "assistant".to_string(),
        content: vec![
            InputContentBlock::Thinking {
                thinking: "plan".to_string(),
                signature: "sig".to_string(),
            },
            InputContentBlock::Text {
                text: "first answer".to_string(),
                cache_control: None,
            },
        ],
        ..InputMessage::user_text(String::new())
    };
    let first = request(
        vec![text("user", "first prompt", false), thought(), text("user", "second prompt", true)],
        Some(api::ThinkingConfig::enabled(1_024)),
    );
    let second = request(
        vec![
            text("user", "first prompt", false),
            thought(),
            text("user", "second prompt", true),
            text("assistant", "second answer", false),
            text("user", "third prompt", true),
        ],
        None,
    );
    let Some(ledger) = record(
        "sent-prefix-normalized",
        vec![sse_reply(9_000, ""), sse_reply(1_000, "")],
        first,
        second,
    )
    .await
    else {
        return;
    };

    assert!(!ledger.reason.is_empty(), "the read drop is a break");
    assert!(
        !ledger.reason.contains("prefix unchanged"),
        "bytes the client changed were filed as an unchanged prefix: {}",
        ledger.reason
    );
    assert!(ledger.reason.contains("parameters"), "thinking changed first: {}", ledger.reason);
    assert_eq!(ledger.rows[1]["prefix_changed_at"], "p", "{}", ledger.rows[1]);
    assert_ne!(
        prefix_at(&ledger.rows[0], "m2"),
        prefix_at(&ledger.rows[1], "m2"),
        "the stripped thinking block changes the prefix at the shared marker too"
    );
}

/// The `at:bytes:hash` entry a ledger row carries for marker `at`.
fn prefix_at(row: &serde_json::Value, at: &str) -> String {
    row["prefix_hashes"]
        .as_array()
        .and_then(|all| all.iter().filter_map(|entry| entry.as_str()).find(|entry| entry.starts_with(&format!("{at}:"))))
        .unwrap_or_else(|| panic!("no {at} in {row}"))
        .split_once(':')
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_default()
}

/// Only the thinking budget changes between two requests whose history is a
/// pure append: the provider keys the messages' cache entry on it, so the
/// read drop is the parameters', not an unchanged prefix (adversarial review
/// W3, P3-3; the shape effort A/B runs, Z11).
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_drop_after_only_the_thinking_budget_changed_names_the_request_parameters() {
    let _guard = env_lock();
    let first = request(vec![text("user", "first prompt", true)], Some(api::ThinkingConfig::enabled(1_024)));
    let second = request(
        vec![
            text("user", "first prompt", true),
            text("assistant", "first answer", false),
            text("user", "second prompt", true),
        ],
        Some(api::ThinkingConfig::enabled(2_048)),
    );
    let Some(ledger) = record(
        "sent-prefix-parameters",
        vec![sse_reply(9_000, ""), sse_reply(1_000, "")],
        first,
        second,
    )
    .await
    else {
        return;
    };

    assert!(!ledger.reason.is_empty(), "the read drop is a break");
    assert!(
        !ledger.reason.contains("prefix unchanged"),
        "a changed thinking budget was filed as an unchanged prefix: {}",
        ledger.reason
    );
    assert_eq!(ledger.rows[1]["prefix_changed_at"], "p", "{}", ledger.rows[1]);
    assert_eq!(
        prefix_at(&ledger.rows[0], "m0"),
        prefix_at(&ledger.rows[1], "m0"),
        "the message bytes in front of the shared marker did not change"
    );
}
