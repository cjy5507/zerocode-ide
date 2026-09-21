//! The pre-send door: a wall this process inherited is answered without a
//! request, and one it earned itself is left to its own retry ladder.
//!
//! Its own test binary on purpose. The door reads process-global cool-down
//! state (`api::quota`), so a test that parks Anthropic for 60 s would refuse
//! every sibling test that streams through an `AnthropicClient` in the same
//! binary — and both `api`'s lib tests and `client_integration` are full of
//! them. `rate_limit_test_guard` only serialises the tests that ask for it;
//! a separate process is what makes the isolation total. The inherited half
//! of the door needs the shared registry live, which a test binary disables
//! for the whole process, so it lives in `quota_door_shared.rs`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use api::{AnthropicClient, CapacityScope, InputMessage, MessageRequest, ProviderKind};
use core_types::retry_signal::reset_hint_in_text;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// How long the test asks the registry to park the Anthropic window for. The
/// provider-capacity ceiling clamps it, so the door's refusal names less than
/// this; `PARKED_FLOOR_MS` is what must survive the clamp.
const PARKED_MS: u64 = 60_000;
/// The least the inherited park may still have left when the door answers.
const PARKED_FLOOR_MS: u64 = 10_000;

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

/// A listener that counts every connection it accepts and answers each with an
/// empty 200 — the request going out is the only fact these tests read.
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
            let head = "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-type: text/event-stream\r\ncontent-length: 0\r\n\r\n";
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.flush().await;
            let _ = socket.shutdown().await;
        }
    });
    (format!("http://{addr}"), hits, server)
}

#[tokio::test]
// Single-threaded test; the quota guard is held only to serialise the
// process-global cool-down state these two tests share.
#[allow(clippy::await_holding_lock)]
async fn a_parked_window_is_refused_without_a_request() {
    let _quota_serial = api::quota::rate_limit_test_guard();
    api::quota::isolate_rate_limit_state_for_tests();
    let (base_url, hits, server) = counting_server().await;
    // A provider-capacity pause parks the window without stamping the
    // own-429 mark — the same shape, in one process, as a park inherited
    // from another `zo`.
    api::quota::mark_capacity_stall_from(
        ProviderKind::Anthropic,
        CapacityScope::Provider,
        Some(Duration::from_millis(PARKED_MS)),
        0,
    );

    let client = AnthropicClient::new("token").with_base_url(base_url);
    let error = client
        .stream_message(&request())
        .await
        .expect_err("a parked window is answered, not sent into");
    server.await.expect("server task");
    api::quota::isolate_rate_limit_state_for_tests();

    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the door let a request through a window it knows is parked"
    );
    let hint = error.retry_after().expect("the park rides up as the reset");
    assert!(
        hint <= Duration::from_millis(PARKED_MS) && hint > Duration::from_millis(PARKED_FLOOR_MS),
        "the refusal must name what is left of the park, got {hint:?}"
    );
    assert!(
        error.is_rate_limit(),
        "the escape reads this as capacity: {error}"
    );
    assert!(
        matches!(
            error.provider_error_class(),
            api::ProviderErrorClass::RateLimit {
                scope: CapacityScope::Account,
                ..
            }
        ),
        "an account window, not a provider overload: {error}"
    );
    // The text carries whole seconds and the runtime parses whole seconds
    // back; the park itself is read to the millisecond, so a 14.999 s park
    // is "14" on the wire (a load-flake on the merged tree, 2026-09-21).
    assert_eq!(
        reset_hint_in_text(&error.to_string()),
        Some(Duration::from_secs(hint.as_secs())),
        "the flattened text must carry the seconds the runtime parses back: {error}"
    );
}

#[tokio::test]
// Same guard, same reason as the test above.
#[allow(clippy::await_holding_lock)]
async fn an_unparked_window_still_sends() {
    let _quota_serial = api::quota::rate_limit_test_guard();
    api::quota::isolate_rate_limit_state_for_tests();
    let (base_url, hits, server) = counting_server().await;

    let client = AnthropicClient::new("token").with_base_url(base_url);
    let sent = client.stream_message(&request()).await;
    server.await.expect("server task");

    let refused = sent.as_ref().err().map(ToString::to_string);
    assert!(sent.is_ok(), "no cool-down is no refusal: {refused:?}");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the request must reach the server when nothing is parked"
    );
}

/// A park this process earned itself is the retry ladder's to time. Hard-
/// refusing on it overrides the layer that owns the wait: with a provider
/// answering `retry-after: 0`, the ladder's own 15 s floor became a wall the
/// turn could not cross, and a `/goal` that resumes on the next attempt
/// stalled instead (`e2e_goal_resumes_after_provider_retry_after_…`).
#[tokio::test]
// Same guard, same reason as the tests above.
#[allow(clippy::await_holding_lock)]
async fn a_park_this_process_earned_is_left_to_the_ladder() {
    let _quota_serial = api::quota::rate_limit_test_guard();
    api::quota::isolate_rate_limit_state_for_tests();
    let (base_url, hits, server) = counting_server().await;
    api::quota::mark_rate_limit_cooldown(ProviderKind::Anthropic, PARKED_MS);
    assert!(
        api::quota::took_own_rate_limit(ProviderKind::Anthropic),
        "the mark this process wrote is its own"
    );

    let client = AnthropicClient::new("token").with_base_url(base_url);
    let sent = client.stream_message(&request()).await;
    server.await.expect("server task");
    api::quota::isolate_rate_limit_state_for_tests();

    let refused = sent.as_ref().err().map(ToString::to_string);
    assert!(sent.is_ok(), "the door closed on this process's own park: {refused:?}");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "a park the ladder earned must not stop the ladder's own next attempt"
    );
}
