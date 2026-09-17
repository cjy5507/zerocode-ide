//! 이벤트 채널 ↔ IDE 하네스 왕복.
//!
//! 손으로 흉내 낸 프레임이 아니라 IDE 가 **실제로 쓰는 클라이언트**
//! (부모 저장소 `crates/zerocode-harness`)를 dev-dep 으로 붙여 친다. 이 파일이
//! 초록이면 "하네스 무변경으로 붙는다"는 계약이 주장이 아니라 측정이다.

use std::sync::Arc;
use std::time::Duration;

use runtime::message_stream::{
    BlockId, PermissionChoice, PermissionDecision, PermissionPrompt, RenderBlock, ToolCallId,
};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use zerocode_harness::{method, Client, HarnessError, Incoming, ServeErrorKind};
use zo_ide::ide::events::{
    self, Answer, Command, EventsChannel, EventsConfig, ResolvedBy, TurnOutcome,
};
use zo_ide::ide::render::PendingPrompt;

/// 한 프레임/응답을 기다리는 상한. 넘기면 테스트가 매달리는 대신 실패한다.
const PATIENCE: Duration = Duration::from_secs(5);

const SESSION: &str = "session-events-test";

async fn open(token: Option<&str>) -> EventsChannel {
    EventsChannel::open(&EventsConfig {
        bind: "127.0.0.1:0".to_string(),
        token: token.map(str::to_string),
        session_id: SESSION.to_string(),
        addr_file: None,
        discovery_file: None,
    })
    .await
    .expect("loopback bind")
}

/// 창이 요청마다 새 연결을 여는 것과 같은 모양(`zerocode-shell::with_client`).
async fn connect(events: &EventsChannel, token: Option<&str>) -> Client {
    Client::connect(&events.local_addr().to_string(), token.map(str::to_string))
        .await
        .expect("connect")
}

/// 패인이 파킹한 프롬프트와 그 답을 받는 쪽 — 런타임이 서 있는 자리다.
fn parked_permission(block_id: u64) -> (RenderBlock, PendingPrompt, oneshot::Receiver<PermissionDecision>) {
    let (responder, runtime_side) = oneshot::channel();
    let prompt = PermissionPrompt {
        id: BlockId(block_id),
        tool_call_id: ToolCallId(String::new()),
        tool_name: "Bash".to_string(),
        reasoning: "run cargo test".to_string(),
        audit_hint: Some("[y] Allow once  [n] Deny".to_string()),
        choices: vec![
            PermissionChoice {
                key: 'y',
                label: "Allow once".to_string(),
                decision: PermissionDecision::AllowOnce,
            },
            PermissionChoice {
                key: 'n',
                label: "Deny".to_string(),
                decision: PermissionDecision::Deny,
            },
        ],
        responder,
    };
    // 채널에 실을 블록과 패인이 파킹하는 프롬프트는 **같은 프롬프트**여야
    // 하는데 `PermissionPrompt` 는 responder 를 쥐고 있어 Clone 이 없다.
    // 그래서 프레임을 만드는 쪽에는 responder 만 다른 쌍둥이를 준다 —
    // 프레임에 실리는 것은 responder 가 아니라 이름·사유·선택지뿐이다.
    let (twin_responder, _unused) = oneshot::channel();
    let twin = RenderBlock::PermissionPrompt(PermissionPrompt {
        id: BlockId(block_id),
        tool_call_id: ToolCallId(String::new()),
        tool_name: prompt.tool_name.clone(),
        reasoning: prompt.reasoning.clone(),
        audit_hint: prompt.audit_hint.clone(),
        choices: prompt.choices.clone(),
        responder: twin_responder,
    });
    (twin, PendingPrompt::Permission(prompt), runtime_side)
}

async fn next_frame_of(client: &mut Client, wanted: &str) -> Value {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let incoming = tokio::time::timeout_at(deadline, client.next_incoming())
            .await
            .unwrap_or_else(|_| panic!("no {wanted} frame within {PATIENCE:?}"))
            .expect("read")
            .expect("stream stayed open");
        if let Incoming::Frame(frame) = incoming {
            if frame.get("type").and_then(Value::as_str) == Some(wanted) {
                return frame;
            }
        }
    }
}

/// 창이 붙어서 실제로 하는 일 한 바퀴: 정체 확인 → 구독 → 권한 프레임 →
/// 두 번째 연결로 `permission.respond` → 패인의 파킹이 그 결정으로 풀린다 →
/// Stop 이 취소 명령으로 도착한다.
#[tokio::test]
async fn the_harness_client_drives_a_pane_permission_and_a_stop() {
    let channel = Arc::new(open(None).await);
    channel.set_history(&[zo_ide::session::plain_session::ReplayItem::User(
        "첫 물음".to_string(),
    )]);

    // 1. `zerocode-lane` 의 정체 확인이 쓰는 그 메서드.
    let mut client = connect(&channel, None).await;
    let listed = client.call(method::LIST, json!({})).await.expect("list");
    assert_eq!(
        listed[0]["id"].as_str(),
        Some(SESSION),
        "정체 확인이 세션을 찾지 못하면 창은 이 패인을 세션 서버로 인정하지 않는다"
    );

    // 2. 구독 — 하네스는 하이드레이션에서 세션 이름을 읽는다.
    let name = client.session_name(SESSION).await.expect("subscribe");
    assert_eq!(name.as_deref(), Some("첫 물음"));

    // 3. 턴이 시작되고 권한 프롬프트가 뜬다.
    let turn = channel.begin_turn();
    let started = next_frame_of(&mut client, "turn").await;
    assert_eq!(started["phase"], json!("start"));
    assert_eq!(started["turn_id"], json!(turn));

    let (block, parked, runtime_side) = parked_permission(0);
    let prompt_id = channel.publish(&block).expect("a prompt frame carries an id");
    let frame = next_frame_of(&mut client, "permission_prompt").await;
    assert_eq!(frame["prompt_id"], json!(prompt_id));
    assert_eq!(frame["tool_name"], json!("Bash"));
    assert_eq!(frame["choices"][0]["decision"], json!("allow_once"));

    // 4. 창은 두 번째 연결로 답한다(첫 연결은 스트림 중이므로) — IDE 의
    //    `respond_permission` 이 하는 그대로.
    let waiting = tokio::spawn({
        let channel = Arc::clone(&channel);
        async move { channel.answer(prompt_id).await }
    });
    connect(&channel, None)
        .await
        .respond_to_permission(prompt_id, "allow_once")
        .await
        .expect("permission.respond");

    let answer = tokio::time::timeout(PATIENCE, waiting)
        .await
        .expect("the front-end sees the answer")
        .expect("join")
        .expect("an answer, not a retirement");
    assert_eq!(answer, Answer::Permission(PermissionDecision::AllowOnce));

    // 5. 패인에 파킹된 프롬프트가 그 결정으로 풀린다 — 런타임이 답을 받는다.
    assert!(events::resolve_pending(parked, &answer));
    assert_eq!(
        runtime_side.await.expect("runtime side"),
        PermissionDecision::AllowOnce
    );
    channel.retire_prompt(prompt_id, ResolvedBy::Ide);
    let resolved = next_frame_of(&mut client, "prompt_resolved").await;
    assert_eq!(resolved["prompt_id"], json!(prompt_id));

    // 6. Stop 버튼.
    let stopped = connect(&channel, None)
        .await
        .call(method::CANCEL_TURN, json!({ "turn_id": turn }))
        .await
        .expect("cancel_turn");
    assert_eq!(stopped["cancelled"], json!(true));
    assert_eq!(
        channel.take_incoming(),
        vec![Command::CancelTurn {
            turn_id: Some(turn)
        }]
    );

    channel.end_turn(turn, TurnOutcome::Cancelled, None);
    let ended = next_frame_of(&mut client, "turn").await;
    assert_eq!(ended["phase"], json!("end"));
    assert_eq!(ended["outcome"], json!("cancelled"));
}

/// 토큰이 걸린 채널은 틀린 비밀을 `-32002` 로 돌려보낸다. 하네스는 그 코드를
/// `Unauthorized` 로 읽고, `zerocode-lane` 의 프로브는 그것만을 "세션 서버이되
/// 우리 토큰을 거부함"으로 읽는다 — 다른 코드로 답하면 창은 이 패인을 남의
/// 서비스로 오인한다.
#[tokio::test]
async fn a_wrong_token_is_refused_with_the_code_the_window_reads() {
    let channel = open(Some("s3cret")).await;

    let mut wrong = connect(&channel, Some("not-the-secret")).await;
    let refused = wrong
        .call(method::LIST, json!({}))
        .await
        .expect_err("a wrong token must not pass");
    match refused {
        HarnessError::Rpc { code, kind, .. } => {
            assert_eq!(code, -32002);
            assert_eq!(kind, ServeErrorKind::Unauthorized);
        }
        other => panic!("unexpected error: {other}"),
    }

    let mut right = connect(&channel, Some("s3cret")).await;
    assert!(right.call(method::LIST, json!({})).await.is_ok());
}

/// 패인에서 사람이 먼저 y 를 눌렀다면 창의 늦은 답은 거절돼야 한다 — 은퇴한
/// `prompt_id` 를 조용히 받아 주면 답이 두 번 세어진다.
#[tokio::test]
async fn the_pane_answering_first_retires_the_prompt() {
    let channel = open(None).await;
    let (block, parked, runtime_side) = parked_permission(0);
    let prompt_id = channel.publish(&block).expect("prompt id");

    // 패인 쪽 승리: 파킹된 프롬프트가 제 responder 로 답하고, 프런트엔드가
    // 채널에서 그 프롬프트를 내린다.
    assert!(events::resolve_pending(
        parked,
        &Answer::Permission(PermissionDecision::Deny)
    ));
    channel.retire_prompt(prompt_id, ResolvedBy::Pane);
    assert_eq!(runtime_side.await.expect("runtime side"), PermissionDecision::Deny);

    let late = connect(&channel, None)
        .await
        .respond_to_permission(prompt_id, "allow_always")
        .await
        .expect_err("a retired prompt takes no answer");
    match late {
        HarnessError::Rpc { kind, .. } => assert_eq!(kind, ServeErrorKind::NoSuchSession),
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(channel.live_prompts(), 0);
}

/// 도는 턴이 없으면 스티어는 거절된다(`-32003`), 있으면 명령으로 도착한다.
#[tokio::test]
async fn steering_only_lands_while_a_turn_is_running() {
    let channel = open(None).await;

    let denied = connect(&channel, None)
        .await
        .steer(SESSION, "잠깐", None)
        .await
        .expect_err("nothing to steer");
    match denied {
        HarnessError::Rpc { kind, .. } => assert_eq!(kind, ServeErrorKind::SteerDenied),
        other => panic!("unexpected error: {other}"),
    }

    let turn = channel.begin_turn();
    connect(&channel, None)
        .await
        .steer(SESSION, "대신 이걸 봐", Some(turn))
        .await
        .expect("steer");
    assert_eq!(
        channel.take_incoming(),
        vec![Command::Steer {
            text: "대신 이걸 봐".to_string()
        }]
    );
}

#[tokio::test]
async fn capabilities_cross_the_real_harness_and_reconnect_without_changing_info() {
    use zo_ide::ide::channel::{auth::TokenPolicy, server, state::ChannelState};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let state = Arc::new(ChannelState::new(SESSION.into()));
    state.update_capabilities(|s| s.identity["session_id"] = json!("active-session"));
    let server = tokio::spawn(server::accept_loop(listener, Arc::clone(&state), TokenPolicy::new(Some("wire-secret".into()))));
    let mut client = Client::connect(&address,Some("wire-secret".into())).await.unwrap();
    let before = client.call(method::INFO,json!({})).await.unwrap();
    let snapshot = client.call("session.capabilities",json!({})).await.unwrap();
    assert_eq!(snapshot["protocol"]["major"],1);
    assert_eq!(snapshot["revision"],1);
    assert!(!snapshot.to_string().contains("wire-secret"));
    let hydration = client.subscribe(SESSION,true).await.unwrap();
    assert_eq!(hydration["history"].as_array().unwrap().last().unwrap()["revision"],1);
    state.update_capabilities(|s| s.identity["session_id"] = json!("resumed-session"));
    let live = next_frame_of(&mut client,"session_capabilities").await;
    assert_eq!(live["revision"],2);
    assert!(live.get("jsonrpc").is_none());
    let mut reconnected = Client::connect(&address,Some("wire-secret".into())).await.unwrap();
    let history = reconnected.subscribe(SESSION,true).await.unwrap();
    assert_eq!(history["history"].as_array().unwrap().last(),Some(&live));
    assert_eq!(reconnected.call(method::INFO,json!({})).await.unwrap(),before);
    let mut denied = Client::connect(&address,None).await.unwrap();
    assert!(denied.call("session.capabilities",json!({})).await.is_err());
    server.abort();
}

/// `PushNotification` on the window road (t-2943): the block the surface
/// sends becomes a `notify` frame the subscribed window reads verbatim —
/// title, body, level and the road word — and `has_subscribers` is the fact
/// the surface judges that road by: false on a bare channel, true once the
/// window's client has subscribed.
#[tokio::test]
async fn a_push_notification_reaches_the_subscribed_window_as_a_notify_frame() {
    use runtime::message_stream::{NotificationRoad, SystemLevel};

    let channel = open(None).await;
    assert!(
        !channel.has_subscribers(),
        "a channel nobody subscribed to must not count as a window"
    );

    let mut client = connect(&channel, None).await;
    let _ = client.session_name(SESSION).await.expect("subscribe");
    assert!(channel.has_subscribers(), "the subscribed window is a reader");

    let block = RenderBlock::Notification {
        id: BlockId(3),
        title: "zo · api".to_string(),
        body: "Build is green; merge when you are back".to_string(),
        level: SystemLevel::Info,
        road: NotificationRoad::Window,
    };
    assert_eq!(channel.publish(&block), None, "a notification is not a prompt");

    let frame = next_frame_of(&mut client, "notify").await;
    assert_eq!(frame["id"], json!(3));
    assert_eq!(frame["title"], json!("zo · api"));
    assert_eq!(frame["body"], json!("Build is green; merge when you are back"));
    assert_eq!(frame["level"], json!("info"));
    assert_eq!(frame["road"], json!("window"));
    assert!(
        frame.get("jsonrpc").is_none(),
        "a render frame never wears the JSON-RPC envelope"
    );
}
