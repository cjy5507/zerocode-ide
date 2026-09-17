//! Contract tests for the hook endpoint.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode};
use tokio::time::timeout;
use tokio_stream::wrappers::ReceiverStream;
use tower::ServiceExt;
use zerocode_core::AgentKind;
use zerocode_hookd::{BridgeState, HOOK_TOKEN_HEADER, MAX_HOOK_BODY_BYTES, router};

const TOKEN: &str = "test-token";
const BROWSER: &str = "browser-secret";
const COMPUTER: &str = "computer-secret";
const FORM: &str = "pane_key=tab%2Fleaf&tab_id=t1&payload=%7B%22event%22%3A%22stop%22%7D";

fn post(uri: &str, token: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::post(uri).header(
        axum::http::header::CONTENT_TYPE,
        "application/x-www-form-urlencoded",
    );
    if let Some(token) = token {
        builder = builder.header(HOOK_TOKEN_HEADER, token);
    }
    builder.body(Body::from(body.to_owned())).expect("request")
}

fn post_json(uri: &str, token: Option<&str>, body: &str) -> Request<Body> {
    let mut builder =
        Request::post(uri).header(axum::http::header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(HOOK_TOKEN_HEADER, token);
    }
    builder.body(Body::from(body.to_owned())).expect("request")
}

fn team_post(body: impl Into<String>) -> Request<Body> {
    Request::post("/agent-teams")
        .header(HOOK_TOKEN_HEADER, TOKEN)
        .header(zerocode_hookd::TEAM_ID_HEADER, "team-test")
        .header(zerocode_hookd::TEAM_PANE_HEADER, "%1")
        .header(zerocode_hookd::TEAM_TOKEN_HEADER, "pane-token")
        .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(body.into()))
        .expect("team request")
}

struct NeverTeamBody {
    polled: Arc<AtomicUsize>,
    seen: bool,
}

impl tokio_stream::Stream for NeverTeamBody {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if !self.seen {
            self.seen = true;
            self.polled.fetch_add(1, Ordering::SeqCst);
        }
        Poll::Pending
    }
}

fn slow_team_post(polled: Arc<AtomicUsize>) -> Request<Body> {
    Request::post("/agent-teams")
        .header(HOOK_TOKEN_HEADER, TOKEN)
        .header(zerocode_hookd::TEAM_ID_HEADER, "team-test")
        .header(zerocode_hookd::TEAM_PANE_HEADER, "%1")
        .header(zerocode_hookd::TEAM_TOKEN_HEADER, "pane-token")
        .header(axum::http::header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from_stream(NeverTeamBody {
            polled,
            seen: false,
        }))
        .expect("slow team request")
}

#[test]
fn the_parsed_team_body_is_released_before_queue_or_answer_wait() {
    let source = include_str!("../src/lib.rs");
    let handler = source
        .split_once("async fn receive_team_command(")
        .expect("team handler")
        .1;
    let parsed = handler.find("let argv = ").expect("argv parse");
    let dropped = handler.find("drop(body);").expect("body drop");
    let queued = handler
        .find("state.teams.try_send(team_request)")
        .expect("bounded queue");
    let waiting = handler
        .find("tokio::time::timeout(TEAM_DEADLINE, wait)")
        .expect("answer wait");
    assert!(
        parsed < dropped && dropped < queued && queued < waiting,
        "the original team body outlives parsing or queue admission"
    );
}

/// The plugin road. Agents that load code rather than run a script post JSON
/// from inside their host's Node process, with `payload` as an OBJECT and the
/// metadata in camelCase — because those plugins are JavaScript, and asking a JS
/// author for snake_case is how a field silently arrives empty.
#[tokio::test]
async fn a_plugin_may_post_json_with_camel_case_metadata_and_a_structured_payload() {
    let (state, mut events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let body = r#"{"paneKey":"term-7","tabId":"t2","launchToken":"lt-9",
        "worktreeId":"/w","env":"production","version":"1",
        "hook_event_name":"agent.end",
        "payload":{"hook_event_name":"agent.end","threadId":"th-1"}}"#;
    let response = router(state)
        .oneshot(post_json("/hook/amp", Some(TOKEN), body))
        .await
        .expect("service");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let envelope = events.recv().await.expect("envelope");
    assert_eq!(envelope.agent, AgentKind::Amp);
    assert_eq!(envelope.pane_key, "term-7");
    assert_eq!(envelope.tab_id, "t2");
    assert_eq!(envelope.launch_token, "lt-9");
    assert_eq!(envelope.hook_event_name, "agent.end");
    // The payload arrives as text, which is what a payload IS to this bridge:
    // the agent's own schema, not our business to parse. And the reader that
    // goes looking for an event name inside it must find one.
    assert!(envelope.payload.contains(r#""threadId":"th-1""#));
    assert_eq!(
        zerocode_core::hook::envelope_event_name(&envelope).as_deref(),
        Some("agent.end")
    );
    assert_eq!(
        zerocode_core::hook::hook_state("agent.end", "{}"),
        Some(zerocode_core::hook::HookState::Done)
    );
}

/// The federation road: the transport token opens the wire, the home
/// fingerprint header names the caller, and the call rides the channel
/// whole — with the window's answer coming back verbatim. No fingerprint
/// is a 400 before any channel is touched.
#[tokio::test]
async fn a_federation_call_needs_its_home_and_rides_the_channel_whole() {
    let (state, _events, _teams, _browser, _computer, mut federation) =
        BridgeState::new_with_computer(TOKEN, BROWSER, "computer-token");
    let federation_token = zerocode_hookd::federation_token(TOKEN);

    let anonymous = post_json(
        zerocode_hookd::FEDERATION_PATH,
        Some(federation_token.as_str()),
        r#"{"verb":"pull"}"#,
    );
    let refused = router(state.clone())
        .oneshot(anonymous)
        .await
        .expect("service");
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);

    // An address book populated by an older invite still carries the bridge
    // token. Keep that road alive while federation-servers tells the user how
    // to replace the wider grant.
    let legacy = post_json(
        zerocode_hookd::FEDERATION_PATH,
        Some(TOKEN),
        r#"{"verb":"pull"}"#,
    );
    let legacy = router(state.clone())
        .oneshot(legacy)
        .await
        .expect("legacy service");
    assert_eq!(legacy.status(), StatusCode::BAD_REQUEST);

    let mut named = post_json(
        zerocode_hookd::FEDERATION_PATH,
        Some(federation_token.as_str()),
        r#"{"verb":"pull","afterSeq":0}"#,
    );
    named.headers_mut().insert(
        zerocode_hookd::FEDERATION_HOME_HEADER,
        "home-fp-77".parse().expect("a header"),
    );
    let answering = tokio::spawn(async move {
        let call = federation
            .recv()
            .await
            .expect("the call reaches the window");
        assert_eq!(call.home, "home-fp-77");
        assert_eq!(call.body["verb"], "pull");
        call.answer
            .send(r#"{"items":[]}"#.to_string())
            .expect("the answer road");
    });
    let response = router(state).oneshot(named).await.expect("service");
    assert_eq!(response.status(), StatusCode::OK);
    let said = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("a body");
    assert_eq!(&said[..], br#"{"items":[]}"#);
    answering.await.expect("the window half");
}

/// The credential handed to a remote federation home must stop at the
/// federation route. These requests are deliberately valid enough to get a
/// route-specific answer if the outer transport gate admits them, which makes
/// an accidental shared-token grant visible as something other than 401.
#[tokio::test]
async fn the_federation_token_cannot_open_local_bridge_routes() {
    let (state, _events, teams, browser) = BridgeState::new(TOKEN, BROWSER);
    drop(teams);
    drop(browser);
    let token = zerocode_hookd::federation_token(TOKEN);

    let hook = router(state.clone())
        .oneshot(post("/hook/claude", Some(token.as_str()), FORM))
        .await
        .expect("hook response")
        .status();
    let teams = router(state.clone())
        .oneshot(post("/agent-teams", Some(token.as_str()), ""))
        .await
        .expect("team response")
        .status();
    let browser = router(state)
        .oneshot(
            Request::post("/browser")
                .header(HOOK_TOKEN_HEADER, token.as_str())
                .header(zerocode_hookd::BROWSER_TOKEN_HEADER, BROWSER)
                .body(Body::from("list\u{1f}"))
                .expect("browser request"),
        )
        .await
        .expect("browser response")
        .status();

    assert_eq!(
        [hook, teams, browser],
        [StatusCode::UNAUTHORIZED; 3],
        "the federation bearer escaped its route"
    );
}

#[test]
fn the_federation_token_is_distinct_well_formed_and_debug_redacted() {
    let token = zerocode_hookd::federation_token(TOKEN);
    assert_ne!(token.as_str(), TOKEN);
    assert!(zerocode_hookd::is_scoped_federation_token(token.as_str()));
    let debug = format!("{token:?}");
    assert!(!debug.contains(token.as_str()), "token leaked: {debug}");
    assert!(debug.contains("token_bytes"), "length was omitted: {debug}");
}

/// A JSON body still needs the token, and the refusal still happens before the
/// body is read — the content type does not open a second door.
#[tokio::test]
async fn a_json_post_without_the_token_is_refused_like_any_other() {
    let (state, mut events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state)
        .oneshot(post_json("/hook/amp", Some("wrong"), r#"{"paneKey":"a"}"#))
        .await
        .expect("service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        events.try_recv().is_err(),
        "an unauthenticated post arrived"
    );
}

/// A payload that is already a string stays the string it was. Quoted, the
/// readers looking for an event name inside it would find nothing.
#[tokio::test]
async fn a_string_payload_is_not_re_quoted_on_its_way_through() {
    let (state, mut events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let body = r#"{"paneKey":"term-1","payload":"{\"hook_event_name\":\"Stop\"}"}"#;
    router(state)
        .oneshot(post_json("/hook/amp", Some(TOKEN), body))
        .await
        .expect("service");
    let envelope = events.recv().await.expect("envelope");
    assert_eq!(envelope.payload, r#"{"hook_event_name":"Stop"}"#);
    assert_eq!(
        zerocode_core::hook::envelope_event_name(&envelope).as_deref(),
        Some("Stop")
    );
}

#[tokio::test]
async fn a_valid_hook_is_accepted_and_delivered_with_the_agent_from_the_url() {
    let (state, mut events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state)
        .oneshot(post("/hook/claude", Some(TOKEN), FORM))
        .await
        .expect("service");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let envelope = events.recv().await.expect("envelope");
    assert_eq!(envelope.agent, AgentKind::Claude);
    assert_eq!(envelope.pane_key, "tab/leaf");
    assert_eq!(envelope.tab_id, "t1");
    assert_eq!(envelope.payload, r#"{"event":"stop"}"#);
    // Absent optional fields default rather than rejecting the request.
    assert_eq!(envelope.worktree_id, "");
}

#[tokio::test]
async fn a_hook_is_refused_when_the_server_can_no_longer_reach_the_window() {
    let (state, events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    drop(events);
    let response = router(state)
        .oneshot(post("/hook/claude", Some(TOKEN), FORM))
        .await
        .expect("service");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn every_measured_coordinator_gets_the_shared_contract_in_its_own_event_shape() {
    for (agent, event_name) in [
        (AgentKind::Claude, "SessionStart"),
        (AgentKind::Codex, "UserPromptSubmit"),
    ] {
        let (state, mut events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
        let body = serde_json::json!({
            "paneKey": format!("pane-{}", agent.slug()),
            "payload": {
                "hook_event_name": event_name,
                "session_id": format!("session-{}", agent.slug()),
                "prompt": "사용자가 선택한 에이전트로 병렬 실행",
            }
        })
        .to_string();
        let response = router(state)
            .oneshot(post_json(
                &format!("/hook/{}", agent.slug()),
                Some(TOKEN),
                &body,
            ))
            .await
            .expect("service");

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("context body");
        let reply: serde_json::Value = serde_json::from_slice(&bytes).expect("valid hook JSON");
        assert_eq!(
            reply["hookSpecificOutput"]["hookEventName"],
            event_name,
            "{} received another provider's event shape",
            agent.slug()
        );
        assert_eq!(
            reply["hookSpecificOutput"]["additionalContext"],
            zerocode_core::delegation::AGENT_SELECTION_CONTEXT
        );
        assert_eq!(events.recv().await.expect("envelope").agent, agent);
    }
}

#[tokio::test]
async fn the_contract_is_not_repeated_on_every_prompt() {
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let service = router(state);
    let body = r#"{"paneKey":"p","payload":{"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"one"}}"#;
    let first = service
        .clone()
        .oneshot(post_json("/hook/claude", Some(TOKEN), body))
        .await
        .expect("first");
    let second = service
        .oneshot(post_json("/hook/claude", Some(TOKEN), body))
        .await
        .expect("second");
    assert!(
        !axum::body::to_bytes(first.into_body(), 4096)
            .await
            .expect("first body")
            .is_empty()
    );
    assert!(
        axum::body::to_bytes(second.into_body(), 4096)
            .await
            .expect("second body")
            .is_empty()
    );
}

#[tokio::test]
async fn a_prompt_seeded_launch_skips_only_its_initial_session_context() {
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let service = router(state);
    let post_start = |source: &str| {
        let body = serde_json::json!({
            "paneKey": "seeded/pane",
            "selectionSeeded": true,
            "payload": {
                "hook_event_name": "SessionStart",
                "session_id": "seeded-session",
                "source": source,
            }
        })
        .to_string();
        post_json("/hook/claude", Some(TOKEN), &body)
    };

    let startup = service
        .clone()
        .oneshot(post_start("startup"))
        .await
        .expect("startup");
    let compact = service
        .oneshot(post_start("compact"))
        .await
        .expect("compact");
    assert!(
        axum::body::to_bytes(startup.into_body(), 4096)
            .await
            .expect("startup body")
            .is_empty()
    );
    let body = axum::body::to_bytes(compact.into_body(), 4096)
        .await
        .expect("compact body");
    let reply: serde_json::Value = serde_json::from_slice(&body).expect("compact JSON");
    assert_eq!(
        reply["hookSpecificOutput"]["additionalContext"],
        zerocode_core::delegation::AGENT_SELECTION_CONTEXT
    );
}

#[tokio::test]
async fn a_request_without_a_token_is_rejected() {
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state)
        .oneshot(post("/hook/claude", None, FORM))
        .await
        .expect("service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// The load-bearing half of "auth before parsing": an unauthenticated request
/// must be answered **without its body ever being read**. Status codes alone
/// cannot show this — a token check inside the handler also answers 401, after
/// the body extractor has already consumed the stream. So this sends a body
/// that never produces a byte and never ends: anything that polls it waits
/// forever, and only a gate that runs before the extractors can answer.
#[tokio::test]
async fn an_unauthenticated_request_is_answered_without_reading_its_body() {
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);

    // Sender stays alive and sends nothing, so the stream is pending forever.
    let (_never_sends, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    let request = Request::post("/hook/claude")
        .header(
            axum::http::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .header(HOOK_TOKEN_HEADER, "wrong")
        .body(Body::from_stream(ReceiverStream::new(rx)))
        .expect("request");

    let response = timeout(Duration::from_secs(2), router(state).oneshot(request))
        .await
        .expect("the bridge must answer without waiting on the body")
        .expect("service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_wrong_token_cannot_be_distinguished_from_a_malformed_body() {
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let bad_token = router(state.clone())
        .oneshot(post("/hook/claude", Some("wrong"), FORM))
        .await
        .expect("service");
    let bad_token_and_body = router(state)
        .oneshot(post("/hook/claude", Some("wrong"), "not=a&valid=form"))
        .await
        .expect("service");

    assert_eq!(bad_token.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(bad_token_and_body.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_unknown_agent_slug_is_not_found() {
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state)
        .oneshot(post("/hook/not-an-agent", Some(TOKEN), FORM))
        .await
        .expect("service");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_body_missing_required_fields_is_a_client_error_not_a_panic() {
    let (state, mut events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state)
        .oneshot(post("/hook/codex", Some(TOKEN), "tab_id=t1"))
        .await
        .expect("service");

    assert!(
        response.status().is_client_error(),
        "expected a 4xx, got {}",
        response.status()
    );
    assert!(events.try_recv().is_err(), "nothing should be delivered");
}

#[tokio::test]
async fn an_oversized_payload_is_refused_rather_than_buffered() {
    let (state, mut events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let huge = format!(
        "pane_key=tab%2Fleaf&payload={}",
        "x".repeat(MAX_HOOK_BODY_BYTES + 1)
    );
    let response = router(state)
        .oneshot(post("/hook/claude", Some(TOKEN), &huge))
        .await
        .expect("service");

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(events.try_recv().is_err(), "nothing should be delivered");
}

#[tokio::test]
async fn fields_the_bridge_does_not_know_are_ignored() {
    let (state, mut events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let body = format!("{FORM}&future_field=1&another=two");
    let response = router(state)
        .oneshot(post("/hook/antigravity", Some(TOKEN), &body))
        .await
        .expect("service");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        events.recv().await.expect("envelope").agent,
        AgentKind::Antigravity
    );
}

/// The bridge never waits for queue space. Exactly N requests can wait for the
/// window; N+1 receives a distinct busy response and can never appear later as
/// a surprise effect.
#[tokio::test]
async fn the_team_queue_is_bounded_and_a_rejected_request_never_arrives_late() {
    let (state, _events, mut teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let service = router(state);
    let mut pending = Vec::new();
    for index in 0..zerocode_hookd::TEAM_QUEUE_CAPACITY {
        let service = service.clone();
        pending.push(tokio::spawn(async move {
            service
                .oneshot(team_post(format!("list-panes\u{1f}{index}\u{1f}")))
                .await
                .expect("queued team response")
        }));
    }
    timeout(Duration::from_secs(2), async {
        while teams.len() != zerocode_hookd::TEAM_QUEUE_CAPACITY {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the requests fill the queue");

    let rejected = timeout(
        Duration::from_secs(2),
        service
            .clone()
            .oneshot(team_post("split-window\u{1f}rejected\u{1f}")),
    )
    .await
    .expect("busy is immediate")
    .expect("busy response");
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
    let rejected_body = axum::body::to_bytes(rejected.into_body(), 4096)
        .await
        .expect("busy body");
    assert_eq!(
        &rejected_body[..],
        zerocode_hookd::TEAM_BUSY_MESSAGE.as_bytes()
    );

    let mut seen = Vec::new();
    for _ in 0..zerocode_hookd::TEAM_QUEUE_CAPACITY {
        let request = teams.recv().await.expect("queued request");
        seen.push(request.argv.clone());
        let _ = request.answer.send(zerocode_hookd::TeamAnswer {
            stdout: "ok\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        });
    }
    assert!(
        seen.iter()
            .all(|argv| !argv.iter().any(|word| word == "rejected")),
        "the busy request entered the queue: {seen:?}"
    );
    assert!(
        teams.try_recv().is_err(),
        "the rejected request arrived later"
    );
    for task in pending {
        assert_eq!(task.await.expect("queued handler").status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn slow_team_bodies_are_bounded_before_any_request_reaches_the_queue() {
    let (state, _events, mut teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let service = router(state);
    let polled = Arc::new(AtomicUsize::new(0));
    let mut slow = Vec::new();
    for _ in 0..zerocode_hookd::TEAM_BODY_READ_LIMIT {
        let service = service.clone();
        let polled = Arc::clone(&polled);
        slow.push(tokio::spawn(async move {
            service
                .oneshot(slow_team_post(polled))
                .await
                .expect("slow response")
        }));
    }
    timeout(Duration::from_secs(2), async {
        while polled.load(Ordering::SeqCst) != zerocode_hookd::TEAM_BODY_READ_LIMIT {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("every body-read credit is held");
    assert!(
        teams.try_recv().is_err(),
        "a never-ending body was enqueued"
    );

    let busy = timeout(
        Duration::from_secs(2),
        service.clone().oneshot(team_post("list-panes\u{1f}")),
    )
    .await
    .expect("body admission is immediate")
    .expect("busy response");
    assert_eq!(busy.status(), StatusCode::TOO_MANY_REQUESTS);

    for task in slow {
        task.abort();
        let _ = task.await;
    }
    let service_after = service.clone();
    let after = tokio::spawn(async move {
        service_after
            .oneshot(team_post("list-panes\u{1f}"))
            .await
            .expect("response after cancelled bodies")
    });
    let request = timeout(Duration::from_secs(2), teams.recv())
        .await
        .expect("body credits return after cancellation")
        .expect("request after cancellation");
    let _ = request.answer.send(zerocode_hookd::TeamAnswer {
        stdout: "ok\n".to_string(),
        stderr: String::new(),
        exit_code: 0,
    });
    assert_eq!(after.await.expect("after handler").status(), StatusCode::OK);
}

#[tokio::test]
async fn eight_waits_leave_ingress_and_completion_progress_for_control() {
    let (state, _events, mut teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let service = router(state);
    let mut waiting_handlers = Vec::new();
    for index in 0..zerocode_hookd::TEAM_WAIT_LIMIT {
        let service = service.clone();
        waiting_handlers.push(tokio::spawn(async move {
            service
                .oneshot(team_post(format!("check\u{1f}--wait\u{1f}{index}\u{1f}")))
                .await
                .expect("waiting response")
        }));
    }
    timeout(Duration::from_secs(2), async {
        while teams.len() != zerocode_hookd::TEAM_WAIT_LIMIT {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the wait quota reaches the queue");

    let ninth = timeout(
        Duration::from_secs(2),
        service
            .clone()
            .oneshot(team_post("check\u{1f}--wait\u{1f}ninth\u{1f}")),
    )
    .await
    .expect("ninth wait is immediate")
    .expect("ninth wait response");
    assert_eq!(ninth.status(), StatusCode::TOO_MANY_REQUESTS);

    let control_service = service.clone();
    let control_handler = tokio::spawn(async move {
        control_service
            .oneshot(team_post("send\u{1f}--type\u{1f}status\u{1f}"))
            .await
            .expect("control response")
    });
    timeout(Duration::from_secs(2), async {
        while teams.len() != zerocode_hookd::TEAM_WAIT_LIMIT + 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("control enters beside saturated waits");

    let mut held_waits = Vec::new();
    for _ in 0..zerocode_hookd::TEAM_WAIT_LIMIT {
        let request = teams.recv().await.expect("waiting request");
        assert!(request.reserves_wait_slot());
        held_waits.push(request);
    }
    let control = teams.recv().await.expect("control request");
    assert!(!control.reserves_wait_slot());
    let _ = control.answer.send(zerocode_hookd::TeamAnswer {
        stdout: "sent\n".to_string(),
        stderr: String::new(),
        exit_code: 0,
    });
    assert_eq!(
        control_handler.await.expect("control handler").status(),
        StatusCode::OK
    );

    for request in held_waits {
        let _ = request.answer.send(zerocode_hookd::TeamAnswer {
            stdout: "{\"count\":0}\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        });
    }
    for handler in waiting_handlers {
        assert_eq!(
            handler.await.expect("wait handler").status(),
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn a_closed_team_queue_is_distinct_from_a_busy_one() {
    let (state, _events, teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    drop(teams);
    let response = timeout(
        Duration::from_secs(2),
        router(state).oneshot(team_post("list-panes\u{1f}")),
    )
    .await
    .expect("closed is immediate")
    .expect("closed response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .expect("closed body");
    assert_eq!(&body[..], zerocode_hookd::TEAM_CLOSED_MESSAGE.as_bytes());
}

#[tokio::test]
async fn an_oversized_team_command_is_refused_before_it_reaches_the_queue() {
    let (state, _events, mut teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state)
        .oneshot(team_post("x".repeat(MAX_HOOK_BODY_BYTES + 1)))
        .await
        .expect("oversized response");
    // This road reads the body explicitly rather than through an extractor, so
    // its long-standing bounded-read answer is the shim's conflict/refusal.
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(teams.try_recv().is_err());
}

/// The browser road: the argv crosses whole, the answer's exit code decides
/// the status, and a missing token never reaches the channel.
#[tokio::test]
async fn a_browser_command_rides_the_same_bridge_with_the_same_token() {
    let (state, _events, _teams, mut browser) = BridgeState::new(TOKEN, BROWSER);
    let body = "list\u{1f}";
    let answering = tokio::spawn(async move {
        let request = browser.recv().await.expect("request");
        assert_eq!(request.argv, vec!["list".to_string()]);
        let _ = request.answer.send(zerocode_hookd::TeamAnswer {
            stdout: "browser-1\thttps://example.com/\n".into(),
            stderr: String::new(),
            exit_code: 0,
        });
    });
    let response = router(state)
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/browser")
                .header("x-zerocode-hook-token", TOKEN)
                .header("x-zerocode-browser-token", BROWSER)
                .header("content-type", "application/octet-stream")
                .body(axum::body::Body::from(body))
                .expect("request"),
        )
        .await
        .expect("service");
    assert_eq!(response.status(), StatusCode::OK);
    answering.await.expect("answered");
}

#[tokio::test]
async fn computer_use_has_a_separate_capability_and_carries_argv_and_answers() {
    let (state, _events, _teams, _browser, mut computer, _federation) =
        BridgeState::new_with_computer(TOKEN, BROWSER, COMPUTER);
    let answering = tokio::spawn(async move {
        let request = computer.recv().await.expect("computer request");
        assert_eq!(
            request.argv,
            vec!["get-app-state", "--app", "Finder", "--json"]
        );
        let _ = request.answer.send(zerocode_hookd::TeamAnswer {
            stdout: "{\"ok\":true}\n".into(),
            stderr: String::new(),
            exit_code: 0,
        });
    });
    let request = |computer_token: Option<&str>| {
        let mut builder = Request::post("/computer")
            .header(HOOK_TOKEN_HEADER, TOKEN)
            .header("content-type", "application/octet-stream");
        if let Some(token) = computer_token {
            builder = builder.header(zerocode_hookd::COMPUTER_TOKEN_HEADER, token);
        }
        builder
            .body(Body::from(
                "get-app-state\u{1f}--app\u{1f}Finder\u{1f}--json\u{1f}",
            ))
            .expect("request")
    };
    let rejected = router(state.clone())
        .oneshot(request(Some(BROWSER)))
        .await
        .expect("rejected response");
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    let accepted = router(state)
        .oneshot(request(Some(COMPUTER)))
        .await
        .expect("accepted response");
    assert_eq!(accepted.status(), StatusCode::OK);
    answering.await.expect("answered");
}

/// A Computer Use command carries the directory its caller stands in when the
/// door said one: the header's spelling read back, and nothing when the
/// header is missing or does not read — the Jev door refuses a walk whose
/// workspace is unknown, so nothing may be guessed for it here.
#[tokio::test]
async fn a_computer_command_carries_the_directory_its_caller_stands_in() {
    let (state, _events, _teams, _browser, mut computer, _federation) =
        BridgeState::new_with_computer(TOKEN, BROWSER, COMPUTER);
    let folder = std::env::temp_dir().join("작업 \"폴더\"");
    let folder = folder.to_str().expect("a UTF-8 temp folder").to_string();
    let presented = [
        Some(zerocode_core::computer_use::cwd_header_value(&folder)),
        None,
        Some("not a spelling".to_string()),
    ];
    let asked = presented.len();
    let answering = tokio::spawn(async move {
        let mut carried = Vec::new();
        for _ in 0..asked {
            let request = computer.recv().await.expect("computer request");
            carried.push(request.cwd.clone());
            let _ = request.answer.send(zerocode_hookd::TeamAnswer {
                stdout: "{\"ok\":true}\n".into(),
                stderr: String::new(),
                exit_code: 0,
            });
        }
        carried
    });
    for header in presented {
        let mut builder = Request::post("/computer")
            .header(HOOK_TOKEN_HEADER, TOKEN)
            .header(zerocode_hookd::COMPUTER_TOKEN_HEADER, COMPUTER)
            .header("content-type", "application/octet-stream");
        if let Some(value) = header {
            builder = builder.header(zerocode_core::computer_use::CWD_HEADER, value);
        }
        let response = router(state.clone())
            .oneshot(
                builder
                    .body(Body::from("list-apps\u{1f}--json\u{1f}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_eq!(
        answering.await.expect("answered"),
        [Some(folder), None, None]
    );
}

#[tokio::test]
async fn a_browser_command_without_the_token_is_turned_away() {
    let (state, _events, _teams, mut browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state)
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/browser")
                .header("content-type", "application/octet-stream")
                .body(axum::body::Body::from("list\u{1f}"))
                .expect("request"),
        )
        .await
        .expect("service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        browser.try_recv().is_err(),
        "an unauthenticated knock reached the window"
    );
}

/// A refused answer travels as the BODY with a conflict status — the shim
/// prints it on stderr and exits 1, so the agent reads the reason.
#[tokio::test]
async fn a_refused_browser_answer_reaches_the_shim_as_stderr() {
    let (state, _events, _teams, mut browser) = BridgeState::new(TOKEN, BROWSER);
    let answering = tokio::spawn(async move {
        let request = browser.recv().await.expect("request");
        let _ = request.answer.send(zerocode_hookd::TeamAnswer {
            stdout: String::new(),
            stderr: "zerocode-browser: 그런 라벨이 없습니다\n".into(),
            exit_code: 1,
        });
    });
    let response = router(state)
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/browser")
                .header("x-zerocode-hook-token", TOKEN)
                .header("x-zerocode-browser-token", BROWSER)
                .header("content-type", "application/octet-stream")
                .body(axum::body::Body::from("read\u{1f}nope\u{1f}"))
                .expect("request"),
        )
        .await
        .expect("service");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    answering.await.expect("answered");
}

/// The hook token alone is NOT the browser capability — every PTY child has
/// the hook token, and "reports lifecycle events" must not imply "reads
/// pages" (1-g4 리뷰 발견 1).
#[tokio::test]
async fn the_hook_token_alone_does_not_open_the_browser_door() {
    let (state, _events, _teams, mut browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state)
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/browser")
                .header("x-zerocode-hook-token", TOKEN)
                .header("content-type", "application/octet-stream")
                .body(axum::body::Body::from("list\u{1f}"))
                .expect("request"),
        )
        .await
        .expect("service");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        browser.try_recv().is_err(),
        "the hook token opened the browser door"
    );
}

/// And the answers CARRY their words — a status alone proves routing, not
/// the contract the shim prints (1-g4 리뷰 발견: 문자열 존재 검사에 머묾).
#[tokio::test]
async fn a_browser_answer_carries_its_words_both_ways() {
    let (state, _events, _teams, mut browser) = BridgeState::new(TOKEN, BROWSER);
    let answering = tokio::spawn(async move {
        let first = browser.recv().await.expect("request");
        let _ = first.answer.send(zerocode_hookd::TeamAnswer {
            stdout: "browser-1\thttps://example.com/\n".into(),
            stderr: String::new(),
            exit_code: 0,
        });
        let second = browser.recv().await.expect("request");
        let _ = second.answer.send(zerocode_hookd::TeamAnswer {
            stdout: String::new(),
            stderr: "zerocode-browser: 그런 라벨이 없습니다\n".into(),
            exit_code: 1,
        });
    });
    let ask = |state: BridgeState, body: &'static str| async move {
        router(state)
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/browser")
                    .header("x-zerocode-hook-token", TOKEN)
                    .header("x-zerocode-browser-token", BROWSER)
                    .header("content-type", "application/octet-stream")
                    .body(axum::body::Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("service")
    };
    let ok = ask(state.clone(), "list\u{1f}").await;
    assert_eq!(ok.status(), StatusCode::OK);
    let ok_body = axum::body::to_bytes(ok.into_body(), 4096)
        .await
        .expect("body");
    assert_eq!(&ok_body[..], "browser-1\thttps://example.com/\n".as_bytes());

    let refused = ask(state, "read\u{1f}nope\u{1f}").await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let refused_body = axum::body::to_bytes(refused.into_body(), 4096)
        .await
        .expect("body");
    assert_eq!(
        &refused_body[..],
        "zerocode-browser: 그런 라벨이 없습니다\n".as_bytes()
    );
    answering.await.expect("answered");
}

/// One mailbox that hands its pointer over once, and records every knock.
struct StandingPointer {
    notice: std::sync::Mutex<Option<zerocode_hookd::session_notify::PointerNotice>>,
    knocks: std::sync::Mutex<
        Vec<(
            String,
            String,
            zerocode_hookd::pointer_mailbox::PointerMoment,
        )>,
    >,
}

impl StandingPointer {
    fn holding(pending: usize) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            notice: std::sync::Mutex::new(zerocode_hookd::session_notify::PointerNotice::new(
                "run-1",
                "run:run-1",
                "m-9",
                pending,
            )),
            knocks: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn knocks(
        &self,
    ) -> Vec<(
        String,
        String,
        zerocode_hookd::pointer_mailbox::PointerMoment,
    )> {
        self.knocks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl StandingPointer {
    fn still_holding(&self) -> bool {
        self.notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }
}

impl zerocode_hookd::pointer_mailbox::PointerMailbox for StandingPointer {
    fn take(
        &self,
        pane_key: &str,
        launch_token: &str,
        moment: zerocode_hookd::pointer_mailbox::PointerMoment,
    ) -> Option<zerocode_hookd::session_notify::PointerNotice> {
        self.knocks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((pane_key.to_string(), launch_token.to_string(), moment));
        self.notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn restore(&self, _pane_key: &str) {
        *self
            .notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            zerocode_hookd::session_notify::PointerNotice::new("run-1", "run:run-1", "m-9", 1);
    }
}

fn stop_body(pane: &str, already_continuing: bool) -> String {
    serde_json::json!({
        "paneKey": pane,
        "launchToken": "launch-7",
        "payload": {
            "hook_event_name": "Stop",
            "session_id": "s-1",
            "stop_hook_active": already_continuing,
        }
    })
    .to_string()
}

async fn reply_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 8192)
        .await
        .expect("a hook body");
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).expect("valid hook JSON")
}

/// A turn ending is where a working agent can be reached without a keystroke.
///
/// The measured provider is handed its waiting ledger pointer as a
/// continuation decision — no composer, no draft, and nobody pressing Enter —
/// and the pointer is the FIXED advice, never a message body.
#[tokio::test]
async fn a_waiting_pointer_reaches_a_measured_provider_as_its_turn_ends() {
    let mailbox = StandingPointer::holding(2);
    let (state, mut events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let service = router(state.with_pointer_mailbox(mailbox.clone()));

    let response = service
        .clone()
        .oneshot(post_json(
            "/hook/claude",
            Some(TOKEN),
            &stop_body("term-4", false),
        ))
        .await
        .expect("the stop hook");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let reply = reply_of(response).await;
    assert_eq!(reply["decision"], "block");
    assert_eq!(
        reply["reason"],
        zerocode_core::orchestration::pointer_text(2)
    );
    assert_eq!(
        mailbox.knocks(),
        vec![(
            "term-4".to_string(),
            "launch-7".to_string(),
            zerocode_hookd::pointer_mailbox::PointerMoment::TurnEnding
        )],
        "the mailbox was not told which launch was knocking"
    );
    // The payload still reaches the window: a pointer is an extra answer, not
    // a swallowed event.
    assert_eq!(events.recv().await.expect("envelope").pane_key, "term-4");

    // Collected once. The next turn end finds the mailbox empty and the hook
    // answers nothing, which is what keeps a continuation from continuing
    // itself forever.
    let again = service
        .oneshot(post_json(
            "/hook/claude",
            Some(TOKEN),
            &stop_body("term-4", false),
        ))
        .await
        .expect("the second stop hook");
    assert_eq!(reply_of(again).await, serde_json::Value::Null);
}

/// A turn that is itself a hook's continuation is never blocked again, and the
/// mailbox is not even asked — so the pointer stays for a turn that can carry
/// it.
#[tokio::test]
async fn a_turn_a_hook_already_continued_is_never_blocked_again() {
    let mailbox = StandingPointer::holding(1);
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state.with_pointer_mailbox(mailbox.clone()))
        .oneshot(post_json(
            "/hook/claude",
            Some(TOKEN),
            &stop_body("term-5", true),
        ))
        .await
        .expect("the stop hook");

    assert_eq!(reply_of(response).await, serde_json::Value::Null);
    assert!(
        mailbox.knocks().is_empty(),
        "the pointer was spent on a turn that could not carry it"
    );
}

/// A provider nobody measured is never handed a decision it may not
/// understand. Codex has a native queue route of its own; what it must not get
/// is Claude's continuation shape guessed at on its behalf.
#[tokio::test]
async fn an_unmeasured_provider_is_never_offered_a_continuation() {
    let mailbox = StandingPointer::holding(1);
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state.with_pointer_mailbox(mailbox.clone()))
        .oneshot(post_json(
            "/hook/codex",
            Some(TOKEN),
            &stop_body("term-6", false),
        ))
        .await
        .expect("the stop hook");

    assert_eq!(reply_of(response).await, serde_json::Value::Null);
    assert!(mailbox.knocks().is_empty());
}

/// A fresh, resumed or adopted pane learns about mail that arrived while it
/// was not there — as context on its own SessionStart, again with no
/// keystroke.
#[tokio::test]
async fn a_session_starting_collects_the_pointer_as_context() {
    let mailbox = StandingPointer::holding(3);
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let body = serde_json::json!({
        "paneKey": "term-7",
        "payload": {
            "hook_event_name": "SessionStart",
            "session_id": "s-2",
            "source": "resume",
        }
    })
    .to_string();
    let response = router(state.with_pointer_mailbox(mailbox.clone()))
        .oneshot(post_json("/hook/claude", Some(TOKEN), &body))
        .await
        .expect("the session start hook");

    let reply = reply_of(response).await;
    assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "SessionStart");
    assert!(
        reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("a composed context")
            .ends_with(zerocode_core::orchestration::pointer_text(3).as_str()),
        "the resumed pane was not told about the mail waiting for it"
    );
    assert_eq!(
        mailbox.knocks(),
        vec![(
            "term-7".to_string(),
            String::new(),
            zerocode_hookd::pointer_mailbox::PointerMoment::SessionStarting
        )]
    );
}

/// A session start that is owed BOTH gets both.
///
/// The pointer stands with the orchestration contract, never instead of it —
/// and the once-per-context tracker has spent that contract by the time this
/// reply is composed, so a pointer that replaced it would take it away from
/// the agent for the rest of the session.
#[tokio::test]
async fn a_session_start_owed_context_and_a_pointer_is_given_both() {
    let mailbox = StandingPointer::holding(1);
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let body = serde_json::json!({
        "paneKey": "term-9",
        "payload": {
            "hook_event_name": "SessionStart",
            "session_id": "s-3",
            "source": "startup",
        }
    })
    .to_string();
    let response = router(state.with_pointer_mailbox(mailbox))
        .oneshot(post_json("/hook/claude", Some(TOKEN), &body))
        .await
        .expect("the session start hook");

    let reply = reply_of(response).await;
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("a composed context");
    assert!(
        context.contains(zerocode_core::delegation::AGENT_SELECTION_CONTEXT),
        "the pointer replaced the orchestration contract instead of joining it"
    );
    assert!(
        context.ends_with(zerocode_core::orchestration::pointer_text(1).as_str()),
        "the pointer was dropped from a session start that was owed one: {context}"
    );
    assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "SessionStart");
}

/// A bridge with no mailbox installed answers hooks exactly as it always did.
#[tokio::test]
async fn a_bridge_without_a_mailbox_answers_a_stop_hook_with_nothing() {
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state)
        .oneshot(post_json(
            "/hook/claude",
            Some(TOKEN),
            &stop_body("term-8", false),
        ))
        .await
        .expect("the stop hook");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(reply_of(response).await, serde_json::Value::Null);
}

fn tool_body(pane: &str) -> String {
    serde_json::json!({
        "paneKey": pane,
        "launchToken": "launch-7",
        "payload": {
            "hook_event_name": "PostToolUse",
            "session_id": "s-4",
            "tool_name": "Bash",
            "tool_response": {"stdout": "ok"},
        }
    })
    .to_string()
}

/// The moment that reaches an agent still WORKING.
///
/// A turn end alone leaves a long implementation deaf until every last thing
/// it is doing is finished — measured on this very task, where a
/// coordinator's mail waited out a worker's turn. A tool boundary is a
/// documented place to hand a provider non-error context, and the reply
/// carries context and nothing else: no decision, no permission, no tool
/// output.
#[tokio::test]
async fn a_working_agent_is_told_at_its_next_tool_boundary() {
    let mailbox = StandingPointer::holding(2);
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state.with_pointer_mailbox(mailbox.clone()))
        .oneshot(post_json(
            "/hook/claude",
            Some(TOKEN),
            &tool_body("term-11"),
        ))
        .await
        .expect("the tool hook");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let reply = reply_of(response).await;
    assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    assert!(
        reply["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("a context")
            .ends_with(zerocode_core::orchestration::pointer_text(2).as_str())
    );
    for forbidden in [
        "decision",
        "permissionDecision",
        "updatedToolOutput",
        "continue",
        "stopReason",
    ] {
        assert!(
            reply.get(forbidden).is_none() && reply["hookSpecificOutput"].get(forbidden).is_none(),
            "a pointer reached for {forbidden}, which is not its to touch: {reply}"
        );
    }
    assert_eq!(
        mailbox.knocks(),
        vec![(
            "term-11".to_string(),
            "launch-7".to_string(),
            zerocode_hookd::pointer_mailbox::PointerMoment::ToolBoundary
        )]
    );
}

/// A provider nobody measured gets no context at a tool boundary either.
#[tokio::test]
async fn an_unmeasured_provider_is_never_given_a_tool_boundary_pointer() {
    let mailbox = StandingPointer::holding(1);
    let (state, _events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    let response = router(state.with_pointer_mailbox(mailbox.clone()))
        .oneshot(post_json("/hook/codex", Some(TOKEN), &tool_body("term-12")))
        .await
        .expect("the tool hook");

    assert_eq!(reply_of(response).await, serde_json::Value::Null);
    assert!(mailbox.knocks().is_empty());
}

/// A taken pointer whose reply cannot be delivered is HANDED BACK.
///
/// The window half of the bridge being gone means nothing will consume the
/// payload and nothing will read this answer. A pointer left taken there is
/// mail that goes quiet — and if the take had also written a durable
/// watermark, no restart would repair it.
#[tokio::test]
async fn a_pointer_is_returned_when_the_window_cannot_be_reached() {
    let mailbox = StandingPointer::holding(1);
    let (state, events, _teams, _browser) = BridgeState::new(TOKEN, BROWSER);
    // The window half goes away: exactly the 503 road.
    drop(events);
    let response = router(state.with_pointer_mailbox(mailbox.clone()))
        .oneshot(post_json(
            "/hook/claude",
            Some(TOKEN),
            &stop_body("term-13", false),
        ))
        .await
        .expect("the stop hook");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        mailbox.still_holding(),
        "a pointer nobody was ever offered stayed taken"
    );
}

#[tokio::test]
async fn artifact_route_authenticates_before_forwarding_and_returns_the_publication_receipt() {
    struct Publisher(Arc<AtomicUsize>);
    impl zerocode_hookd::ArtifactCommands for Publisher {
        fn execute(&self, request: serde_json::Value) -> Result<serde_json::Value, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request["action"], "publish");
            assert_eq!(request["file_path"], "/tmp/a # 한.html");
            Ok(
                serde_json::json!({"id":"p-test", "version":2, "url":"file:///pages/p-test/index.html"}),
            )
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let (state, ..) = BridgeState::new(TOKEN, BROWSER);
    let app = router(state.with_artifacts(Arc::new(Publisher(calls.clone()))));
    let body = zerocode_core::computer_use::pack_argv(&[
        "publish".into(),
        "--file-path".into(),
        "/tmp/a # 한.html".into(),
    ]);
    let denied = app
        .clone()
        .oneshot(post("/artifact", None, &body))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let allowed = app
        .oneshot(post("/artifact", Some(TOKEN), &body))
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(allowed.into_body(), MAX_HOOK_BODY_BYTES)
        .await
        .unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(answer["version"], 2);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
