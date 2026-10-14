//! What the hook bridge holds in memory (t-54317) — an inventory the census job
//! prints, not a contract.
//!
//! The bridge runs inside the window's process: it parses each hook into a
//! [`HookEnvelope`] and hands it to the window over a channel, and keeps
//! nothing once the window has taken it. The one place it holds memory is that
//! channel while the window is not taking — a main thread held up. This posts
//! hooks shaped like a tool's result with nobody taking, then counts what the
//! queue holds, envelope by envelope.

use axum::body::Body;
use axum::http::{Request, StatusCode, header::CONTENT_TYPE};
use tower::ServiceExt;
use zerocode_core::HookEnvelope;
use zerocode_hookd::{BridgeState, HOOK_TOKEN_HEADER, router};

const TOKEN: &str = "census-token";
/// Hooks posted while the window does not take them.
const QUEUED: usize = 500;
/// One tool result as a hook carries it — a test run's tail or a file read, as
/// `PostToolUse` posts it.
const RESULT_BYTES: usize = 16 * 1024;

/// The bytes one queued envelope holds: the struct and every string it owns.
fn held(envelope: &HookEnvelope) -> usize {
    size_of::<HookEnvelope>()
        + [
            &envelope.pane_key,
            &envelope.tab_id,
            &envelope.launch_token,
            &envelope.worktree_id,
            &envelope.env,
            &envelope.version,
            &envelope.hook_event_name,
            &envelope.payload,
        ]
        .iter()
        .map(|text| text.capacity())
        .sum::<usize>()
}

#[tokio::test]
#[ignore = "an inventory, printed by the t-54317 census job"]
async fn census_what_a_window_that_is_not_taking_leaves_queued() {
    let (state, mut events, _teams, _browser) = BridgeState::new(TOKEN, "census-browser");
    let app = router(state);
    let body = serde_json::json!({
        "paneKey": "term-7", "tabId": "t2", "launchToken": "lt-9", "worktreeId": "/w",
        "env": "production", "version": "1", "hook_event_name": "PostToolUse",
        "payload": {
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_response": { "stdout": "x".repeat(RESULT_BYTES) },
        },
    })
    .to_string();
    for _ in 0..QUEUED {
        let request = Request::post("/hook/claude")
            .header(CONTENT_TYPE, "application/json")
            .header(HOOK_TOKEN_HEADER, TOKEN)
            .body(Body::from(body.clone()))
            .expect("request");
        let response = app.clone().oneshot(request).await.expect("service");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }
    let mut bytes = 0;
    let mut count = 0;
    while let Ok(envelope) = events.try_recv() {
        bytes += held(&envelope);
        count += 1;
    }
    assert_eq!(count, QUEUED);
    println!(
        "hookd census: {count} hooks of a {RESULT_BYTES}-byte tool result (request body {} bytes) \
         queued while nobody took them hold {bytes} bytes, {} per hook; taken, they hold nothing",
        body.len(),
        bytes / count
    );
}
