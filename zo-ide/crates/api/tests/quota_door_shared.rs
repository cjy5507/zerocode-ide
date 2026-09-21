//! The inherited half of the pre-send door: a wall another `zo` already paid
//! for is answered without a request.
//!
//! Its own test binary because the shared registry is off in any test harness
//! (`quota_shared::running_under_test_harness`) and the isolation switch that
//! turns it off is one-way for the whole process. Here it is turned on
//! explicitly under a config home of this test's own, and the park is written
//! as bytes — which is exactly what "another process" is from this side of
//! the file.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::{AnthropicClient, InputMessage, MessageRequest, ProviderKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// How far ahead the inherited park reaches.
const PARKED_MS: u64 = 60_000;

fn request() -> MessageRequest {
    MessageRequest {
        model: "claude-opus-4-6".to_string(),
        max_tokens: 16,
        messages: vec![InputMessage::user_text("hi")],
        system: None,
        tools: None,
        tool_choice: None,
        stream: false,
        thinking: None,
        output_config: None,
        effort: None,
        effort_band_ceiling: None,
    }
}

/// A listener that counts every connection it accepts. Nothing must reach it.
async fn counting_server() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let server_hits = Arc::clone(&hits);
    let server = tokio::spawn(async move {
        loop {
            let Ok(Ok((mut socket, _))) =
                tokio::time::timeout(Duration::from_millis(300), listener.accept()).await
            else {
                break;
            };
            server_hits.fetch_add(1, Ordering::SeqCst);
            let mut scratch = [0u8; 2048];
            let _ = socket.read(&mut scratch).await;
            let head = "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 0\r\n\r\n";
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (format!("http://{addr}"), hits, server)
}

/// Write the shared record the way another `zo` on this account would have
/// left it: the single `v1 <cooldown_until> <last_429> <util> <util_at>` line
/// `api::quota_shared` reads. Spelling it out here is the point — a reader
/// that stops understanding these bytes stops inheriting the wall, and this
/// test says so.
fn park_written_by_another_process(home: &std::path::Path, parked_for: Duration) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_millis();
    let now = u64::try_from(now).expect("unix ms fits");
    let until = now + u64::try_from(parked_for.as_millis()).expect("park fits");
    let directory = home.join("rate");
    std::fs::create_dir_all(&directory).expect("rate directory");
    std::fs::write(
        directory.join("anthropic.v1"),
        format!("v1 {until} {now} 0 0\n"),
    )
    .expect("shared record");
}

#[tokio::test]
async fn a_wall_another_process_paid_for_is_answered_without_a_request() {
    let home = std::env::temp_dir().join(format!("api-quota-door-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("config home");
    // This binary holds exactly this one test, so nothing else reads or
    // writes the environment while it runs.
    std::env::set_var("ZO_CONFIG_HOME", &home);
    std::env::set_var("ZO_SHARED_RATE_COORD", "1");
    park_written_by_another_process(&home, Duration::from_millis(PARKED_MS));
    assert!(
        !api::quota::took_own_rate_limit(ProviderKind::Anthropic),
        "this process has taken no 429 of its own"
    );

    let (base_url, hits, server) = counting_server().await;
    let client = AnthropicClient::new("token").with_base_url(base_url);
    let error = client
        .stream_message(&request())
        .await
        .expect_err("an inherited wall is answered, not sent into");
    server.await.expect("server task");

    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the door let a request through a wall another process already paid for"
    );
    let hint = error.retry_after().expect("the park rides up as the reset");
    assert!(
        hint <= Duration::from_millis(PARKED_MS) && hint > Duration::from_millis(PARKED_MS - 5_000),
        "the refusal must name what is left of the inherited park, got {hint:?}"
    );
    assert!(error.is_rate_limit(), "the escape reads this as capacity: {error}");

    std::fs::remove_dir_all(&home).ok();
}
