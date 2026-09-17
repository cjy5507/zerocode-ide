//! 소켓 — 받아들이기, 한 연결의 읽기/쓰기, 메서드 배차.
//!
//! 연결 하나는 **하나의 루프**가 읽고 쓴다: 요청 줄과 팬아웃 프레임을 같은
//! `select!` 에서 받아 같은 쓰기 반쪽으로 내보낸다. 쓰기를 따로 태스크로
//! 떼면 `session.subscribe` 의 응답과 그 직후 프레임의 순서를 다시 보장해야
//! 하는데, 한 루프면 그 문제가 아예 없다.
//!
//! 구현하지 않는 메서드가 있다는 것이 이 채널의 성격이다.
//! `session.create`/`load`/`close`/`run_turn`/`run_turn_detached`/`roster` 는
//! **일부러** 없다 — 패인의 세션은 패인의 stdin 이 몬다. IDE 가 턴을 시작할 문은
//! `session.steer`(도는 턴에 끼워 넣기) 하나이고, 그 밖은 `-32601` 이다.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

use super::auth::TokenPolicy;
use super::state::{Answer, AnswerRefusal, ChannelState, Command};
use super::wire::{
    method, RpcRequest, RpcResponse, CODE_INVALID_PARAMS, CODE_INVALID_REQUEST,
    CODE_METHOD_NOT_FOUND, CODE_NO_SUCH_SESSION, CODE_STEER_DENIED, CODE_UNAUTHORIZED,
};

/// 한 요청 줄의 최대 길이. 스티어 텍스트가 실려도 넉넉하고, 소켓 하나가
/// 메모리를 끌어올릴 수는 없는 크기. 넘기면 연결을 접는다 — 루프백 상대는
/// IDE 하나뿐이라 이만한 줄을 보내는 것은 이미 우리가 아는 상대가 아니다.
const MAX_LINE: usize = 1024 * 1024;

/// accept 가 계속 실패할 때의 물러섬 — 첫 실패는 거의 즉시 다시 받고, 지속
/// 실패에서는 초 단위까지 늘려 바쁜 루프가 되지 않게 한다.
const ACCEPT_BACKOFF_MIN: std::time::Duration = std::time::Duration::from_millis(5);
const ACCEPT_BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(1);

/// 받아들이기 루프. 리스너가 죽을 때까지 돈다.
pub async fn accept_loop(listener: TcpListener, state: Arc<ChannelState>, auth: TokenPolicy) {
    let mut backoff = ACCEPT_BACKOFF_MIN;
    loop {
        match listener.accept().await {
            Ok((stream, _peer)) => {
                let state = Arc::clone(&state);
                let auth = auth.clone();
                tokio::spawn(async move {
                    // 연결 하나가 끝나는 것은 사고가 아니다 — IDE 는 요청마다
                    // 새로 붙었다 끊는다(`with_client`). 조용히 접는다.
                    let _ = connection(stream, state, auth).await;
                });
                backoff = ACCEPT_BACKOFF_MIN;
            }
            // 파일 서술자 고갈 같은 지속 실패로 루프를 끝내면 패인은 채널을
            // 잃고도 멀쩡해 보인다. 그렇다고 즉시 재시도만 하면 그 상태에서
            // CPU 를 바쁘게 태우며 아무도 원인을 못 본다 — 사유를 남기고
            // 물러섰다가 다시 받는다(한 번 성공하면 물러섬은 초기화된다).
            Err(error) => {
                eprintln!("zo events channel: accept failed: {error}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(ACCEPT_BACKOFF_MAX);
            }
        }
    }
}

async fn connection(
    stream: TcpStream,
    state: Arc<ChannelState>,
    auth: TokenPolicy,
) -> std::io::Result<()> {
    // 요청/응답의 지연은 사람이 버튼을 누른 뒤의 지연이다.
    let _ = stream.set_nodelay(true);
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut line: Vec<u8> = Vec::new();
    let mut subscribed: Option<broadcast::Receiver<Arc<String>>> = None;

    loop {
        tokio::select! {
            // 요청이 프레임보다 먼저다: 답을 기다리는 사람이 있다.
            biased;
            read = read_request_line(&mut reader, &mut line) => match read {
                Ok(true) => {
                    let request = String::from_utf8_lossy(&line).trim().to_string();
                    line.clear();
                    if request.is_empty() {
                        continue;
                    }
                    let response = handle_line(&state, &auth, &request, &mut subscribed);
                    write_line(&mut write, &serde_json::to_string(&response)?).await?;
                }
                Ok(false) | Err(_) => return Ok(()),
            },
            frame = next_frame(&mut subscribed) => match frame {
                Some(frame) => write_line(&mut write, &frame).await?,
                None => subscribed = None,
            },
        }
    }
}

/// 한 줄 읽기. `Ok(false)` 는 상대가 정상적으로 닫았다는 뜻이다.
///
/// `read_line` 대신 손으로 채우는 이유가 둘 있다. 상한이 **자라기 전에**
/// 걸려야 하고(개행 없는 스트림에 `read_line` 을 걸면 버퍼가 먼저 자란다),
/// 이 future 는 `select!` 안에서 **취소될 수 있어야** 한다: 프레임 갈래가
/// 이기면 여기서 읽던 것이 버려지므로, 이미 소켓에서 꺼낸 바이트는 호출자가
/// 붙들고 있는 `line` 에 남아야 한다. 그래서 비우는 것은 호출자의 몫이다 —
/// 한 줄을 다 처리한 **뒤에** 비운다.
async fn read_request_line(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    line: &mut Vec<u8>,
) -> std::io::Result<bool> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(!line.is_empty());
        }
        if let Some(at) = available.iter().position(|byte| *byte == b'\n') {
            line.extend_from_slice(&available[..at]);
            reader.consume(at + 1);
            return Ok(true);
        }
        // 개행이 아직 안 왔다 — 있는 만큼 옮겨 담고 상한만 확인한다.
        let taken = available.len();
        line.extend_from_slice(available);
        reader.consume(taken);
        if line.len() > MAX_LINE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request line is too long",
            ));
        }
    }
}

/// 구독 중이면 다음 프레임을, 아니면 영원히 기다린다(다른 갈래가 이긴다).
async fn next_frame(subscribed: &mut Option<broadcast::Receiver<Arc<String>>>) -> Option<Arc<String>> {
    let Some(receiver) = subscribed.as_mut() else {
        return std::future::pending().await;
    };
    loop {
        match receiver.recv().await {
            Ok(frame) => return Some(frame),
            // 뒤처진 구독자에게 중요한 것은 놓친 델타가 아니라 지금 상태다.
            // 끊는 대신 건너뛴다 — 이 채널은 권한 모달이 지나는 유일한 길이라
            // 한 번 밀렸다고 닫아 버릴 수 없다.
            Err(broadcast::error::RecvError::Lagged(_)) => (),
            Err(broadcast::error::RecvError::Closed) => return None,
        }
    }
}

async fn write_line(
    write: &mut tokio::net::tcp::OwnedWriteHalf,
    line: &str,
) -> std::io::Result<()> {
    write.write_all(line.as_bytes()).await?;
    write.write_all(b"\n").await?;
    write.flush().await
}

fn handle_line(
    state: &ChannelState,
    auth: &TokenPolicy,
    line: &str,
    subscribed: &mut Option<broadcast::Receiver<Arc<String>>>,
) -> RpcResponse {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return RpcResponse::err(0, CODE_INVALID_REQUEST, "line is not JSON");
    };
    // id 는 봉투가 깨졌을 때에도 돌려줘야 클라이언트가 자기 요청과 맞춘다.
    let id = value.get("id").and_then(Value::as_u64).unwrap_or_default();
    let Ok(request) = serde_json::from_value::<RpcRequest>(value) else {
        return RpcResponse::err(id, CODE_INVALID_REQUEST, "not a JSON-RPC 2.0 request");
    };
    if request.jsonrpc != super::wire::JSONRPC_VERSION {
        return RpcResponse::err(id, CODE_INVALID_REQUEST, "jsonrpc must be \"2.0\"");
    }
    if !auth.authorize(request.token.as_deref()) {
        return RpcResponse::err(
            id,
            CODE_UNAUTHORIZED,
            "missing or wrong token for this events channel",
        );
    }
    dispatch(state, &request, subscribed)
}

fn dispatch(
    state: &ChannelState,
    request: &RpcRequest,
    subscribed: &mut Option<broadcast::Receiver<Arc<String>>>,
) -> RpcResponse {
    let id = request.id;
    let params = &request.params;
    match request.method.as_str() {
        method::LIST => RpcResponse::ok(
            id,
            json!([{ "id": state.session_id(), "messages": state.history().len() }]),
        ),
        method::CAPABILITIES => {
            if !params.is_null() && !params.is_object() {
                return RpcResponse::err(id, CODE_INVALID_PARAMS, "params must be an object");
            }
            state.note_capabilities_read();
            RpcResponse::ok(id, serde_json::to_value(state.capabilities()).unwrap_or(Value::Null))
        }
        method::INFO => {
            let status = state.status();
            RpcResponse::ok(
                id,
                json!({
                    "id": state.session_id(),
                    "model": status.model,
                    "permission_mode": status.permission_mode,
                    "cwd": status.cwd,
                    "git_branch": status.git_branch,
                    "effort": status.effort,
                    "ctx_tokens": status.ctx_tokens,
                    "context_window": status.context_window,
                }),
            )
        }
        method::SUBSCRIBE => {
            if let Err(response) = session_matches(state, params, id) {
                return response;
            }
            // 응답을 쓰기 **전에** 잡는다 — 그 사이에 나간 프레임이 새지 않게.
            // 스냅샷은 그 **뒤에** 읽는다: 그 사이의 변화는 스냅샷에 있거나
            // 스트림으로 오거나 둘 다이지, 어디에도 없을 수는 없다.
            *subscribed = Some(state.subscribe());
            RpcResponse::ok(
                id,
                json!({
                    "id": state.session_id(),
                    "history": state.hydration_history(),
                    "next_seq": state.next_seq(),
                    "helm": Value::Null,
                }),
            )
        }
        method::UNSUBSCRIBE => {
            *subscribed = None;
            RpcResponse::ok(id, json!({ "id": state.session_id() }))
        }
        method::PERMISSION_RESPOND => permission_respond(state, params, id),
        method::QUESTION_RESPOND => question_respond(state, params, id),
        method::CANCEL_TURN => cancel_turn(state, params, id),
        method::STEER => steer(state, params, id),
        // 세션과 무관한 하나 — 자격은 프로세스의 것이지 이 세션의 것이 아니라
        // `session_matches` 를 거치지 않는다.
        method::AUTH_RELOAD => auth_reload(state, params, id),
        method::TEAMMATE_CLOSE => teammate_close(state, params, id),
        method::MCP_CALL => mcp_call(state, params, id),
        other => RpcResponse::err(
            id,
            CODE_METHOD_NOT_FOUND,
            format!(
                "{other} is not on the pane's events channel — the pane's own stdin drives its \
                 turns; use session.steer to put words into the one that is running"
            ),
        ),
    }
}

fn auth_reload(state: &ChannelState, params: &Value, id: u64) -> RpcResponse {
    let response = super::auth_reload::handle(params, id);
    let label = response
        .result
        .as_ref()
        .and_then(|result| result.get("account"))
        .and_then(|account| account.get("label"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty());
    if let Some(label) = label {
        state.push_command(Command::AccountSwitch {
            label: label.to_string(),
        });
    }
    // The children hear the same words (t-2513 §2.6): a pane child that kept
    // running on the old account would be the one helper the switch missed.
    // Off this loop — a child that does not answer must not hold the window's
    // reply — and only after this process reloaded, so a child's own turn
    // never runs ahead of its parent's account.
    if response.error.is_none() {
        if let Some(fanout) = state.child_fanout() {
            let params = params.clone();
            std::thread::Builder::new()
                .name("zo-auth-reload-fanout".to_string())
                .spawn(move || fanout(&params))
                .ok();
        }
    }
    response
}

/// `teammate.close` — the parent asks this pane to leave (t-2513 §2.2).
///
/// Only a pane that declared itself a teammate answers: a root session's
/// composer belongs to the person at it, and no parent may close it.
fn teammate_close(state: &ChannelState, params: &Value, id: u64) -> RpcResponse {
    if !state.accepts_idle_steer() {
        return RpcResponse::err(
            id,
            CODE_METHOD_NOT_FOUND,
            "this pane is not a teammate; only a parent-opened teammate pane answers teammate.close",
        );
    }
    let reason = params
        .get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .unwrap_or(runtime::subagent_panes::CloseReason::ClosedByParent.as_str())
        .to_string();
    state.push_command(Command::Close { reason });
    RpcResponse::ok(id, json!({ "closing": true, "turn_id": state.turn() }))
}

/// `mcp.call` — a pane child's MCP tool call, answered by this session's MCP
/// runtime (t-2513 §2.1). `params.name` is the tool, `params.input` its JSON
/// input; the result is the tool's rendered text, exactly what the inline
/// passthrough hands a thread child.
fn mcp_call(state: &ChannelState, params: &Value, id: u64) -> RpcResponse {
    let Some(name) = params.get("name").and_then(Value::as_str).filter(|name| !name.trim().is_empty()) else {
        return RpcResponse::err(id, CODE_INVALID_PARAMS, "mcp.call needs name");
    };
    let input = params.get("input").cloned().unwrap_or_else(|| json!({}));
    let Some(bridge) = state.mcp_bridge() else {
        return RpcResponse::err(
            id,
            CODE_METHOD_NOT_FOUND,
            "this session has no MCP runtime to route mcp.call through",
        );
    };
    match bridge(name, &input) {
        Ok(text) => RpcResponse::ok(id, json!({ "name": name, "content": text, "is_error": false })),
        Err(error) => RpcResponse::ok(id, json!({ "name": name, "content": error, "is_error": true })),
    }
}

/// `params.id` 가 이 패인의 세션인지. 없거나 빈 문자열이면 "이 패인의 것"으로
/// 읽는다 — 채널에는 세션이 하나뿐이라 그 물음에 다른 답이 없다.
fn session_matches(state: &ChannelState, params: &Value, id: u64) -> Result<(), RpcResponse> {
    match params.get("id").and_then(Value::as_str) {
        None | Some("") => Ok(()),
        Some(named) if named == state.session_id() => Ok(()),
        Some(named) => Err(RpcResponse::err(
            id,
            CODE_NO_SUCH_SESSION,
            format!(
                "this channel carries session {}, not {named}",
                state.session_id()
            ),
        )),
    }
}

fn permission_respond(state: &ChannelState, params: &Value, id: u64) -> RpcResponse {
    let Some(prompt_id) = params.get("prompt_id").and_then(Value::as_u64) else {
        return RpcResponse::err(id, CODE_INVALID_PARAMS, "permission.respond needs prompt_id");
    };
    let Some(tag) = params.get("decision").and_then(Value::as_str) else {
        return RpcResponse::err(id, CODE_INVALID_PARAMS, "permission.respond needs decision");
    };
    // 모르는 태그는 Deny 로 접힌다 — `crate::sinks` 의 그 표 하나가 프레임을
    // 만들 때와 답을 읽을 때 모두 쓰인다. 우연한 허용은 만들지 않는다.
    let decision = crate::sinks::permission_decision_from_tag(tag);
    answered(state, id, prompt_id, Answer::Permission(decision))
}

fn question_respond(state: &ChannelState, params: &Value, id: u64) -> RpcResponse {
    let Some(prompt_id) = params.get("prompt_id").and_then(Value::as_u64) else {
        return RpcResponse::err(id, CODE_INVALID_PARAMS, "question.respond needs prompt_id");
    };
    let Some(answers) = params.get("answers").and_then(Value::as_array) else {
        return RpcResponse::err(
            id,
            CODE_INVALID_PARAMS,
            "question.respond needs answers: an array of strings",
        );
    };
    let answers: Vec<String> = answers
        .iter()
        .filter_map(|answer| answer.as_str().map(str::to_string))
        .collect();
    if answers.is_empty() {
        return RpcResponse::err(
            id,
            CODE_INVALID_PARAMS,
            "question.respond needs at least one answer",
        );
    }
    answered(state, id, prompt_id, Answer::Question(answers))
}

/// 두 `*.respond` 가 공유하는 마지막 걸음: 등록부에 넣고, 거절 사정을
/// JSON-RPC 코드로 낮춘다. 코드로 낮추는 일은 이 한 곳에서만 일어난다.
fn answered(state: &ChannelState, id: u64, prompt_id: u64, answer: Answer) -> RpcResponse {
    match state.answer_prompt(prompt_id, answer) {
        Ok(()) => RpcResponse::ok(id, json!({ "prompt_id": prompt_id })),
        Err(AnswerRefusal::WrongKind) => RpcResponse::err(
            id,
            CODE_INVALID_PARAMS,
            format!("prompt {prompt_id} is not that kind of prompt"),
        ),
        Err(AnswerRefusal::NotWaiting) => RpcResponse::err(
            id,
            CODE_NO_SUCH_SESSION,
            format!("prompt {prompt_id} is not waiting for an answer — it was already answered"),
        ),
    }
}

fn cancel_turn(state: &ChannelState, params: &Value, id: u64) -> RpcResponse {
    let asked = params.get("turn_id").and_then(Value::as_u64);
    let running = state.turn();
    // 도는 턴이 없는데 누른 Stop 은 실패가 아니다 — 사람이 늦게 누른 것이고,
    // 오류로 답하면 IDE 가 실패 배너를 띄운다. 무엇이 일어났는지만 말한다.
    let cancelled = match (asked, running) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(asked), Some(running)) => asked == running,
    };
    if cancelled {
        state.push_command(Command::CancelTurn { turn_id: asked });
    }
    RpcResponse::ok(id, json!({ "cancelled": cancelled, "turn_id": running }))
}

fn steer(state: &ChannelState, params: &Value, id: u64) -> RpcResponse {
    if let Err(response) = session_matches(state, params, id) {
        return response;
    }
    let Some(text) = params.get("text").and_then(Value::as_str) else {
        return RpcResponse::err(id, CODE_INVALID_PARAMS, "session.steer needs text");
    };
    if text.trim().is_empty() {
        return RpcResponse::err(id, CODE_INVALID_PARAMS, "session.steer needs non-empty text");
    }
    let Some(running) = state.turn() else {
        // A teammate between turns opens its NEXT turn with the words
        // (`steer.scope: idle-next-turn`, t-2513 §2.2). The answer carries no
        // `turn_id` — none is running — and says `queued`, which is what the
        // parent's receipt reads. A root session keeps refusing: its idle
        // composer is the person's, not a parent's.
        if state.accepts_idle_steer() {
            if params.get("turn_id").and_then(Value::as_u64).is_some() {
                return RpcResponse::err(
                    id,
                    CODE_STEER_DENIED,
                    "no turn is running; a steer for the next turn names none",
                );
            }
            state.push_command(Command::Steer {
                text: text.to_string(),
            });
            return RpcResponse::ok(
                id,
                json!({ "turn_id": Value::Null, "receipt": runtime::subagent_panes::SteerReceipt::Queued.as_str() }),
            );
        }
        return RpcResponse::err(
            id,
            CODE_STEER_DENIED,
            "no turn is running in this pane to steer",
        );
    };
    // 끝난 턴을 겨눈 스티어는 다음 턴에 접어 넣지 않는다 — `zo serve` 의
    // `SteerParams` 가 turn_id 를 검증하는 것과 같은 이유다.
    if let Some(asked) = params.get("turn_id").and_then(Value::as_u64) {
        if asked != running {
            return RpcResponse::err(
                id,
                CODE_STEER_DENIED,
                format!("turn {asked} is not the one running ({running})"),
            );
        }
    }
    state.push_command(Command::Steer {
        text: text.to_string(),
    });
    // `receipt` is appended for the parent's reader (t-2513 §2.3); the
    // `turn_id` older callers read is unchanged.
    RpcResponse::ok(
        id,
        json!({ "turn_id": running, "receipt": runtime::subagent_panes::SteerReceipt::Consumed.as_str() }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ide::channel::state::PromptKind;
    use runtime::message_stream::PermissionDecision;

    fn request(method: &str, params: Value) -> RpcRequest {
        RpcRequest {
            jsonrpc: super::super::wire::JSONRPC_VERSION.to_string(),
            id: 1,
            method: method.to_string(),
            params,
            token: None,
        }
    }

    fn code_of(response: &RpcResponse) -> Option<i64> {
        response.error.as_ref().map(|error| error.code)
    }

    #[test]
    fn capabilities_is_read_only_versioned_and_keeps_info_unchanged() {
        let state = ChannelState::new("endpoint".to_string());
        let before = dispatch(&state, &request(method::INFO, json!({})), &mut None);
        let response = dispatch(&state, &request("session.capabilities", json!({})), &mut None);
        assert!(response.error.is_none(), "{response:?}");
        let value = response.result.expect("capabilities");
        assert_eq!(value["protocol"], json!({"name":"zo-events","major":1,"minor":0}));
        assert_eq!(value["identity"]["channel_session_id"], "endpoint");
        assert!(value["identity"]["parent_session_id"].is_null());
        assert_eq!(value["support"]["roster"]["transport"], "subagents-snapshot");
        assert_eq!(value["support"]["exact_launch"], json!({"supported": true, "version": 1}));
        assert_eq!(value["launch"]["policy"], "legacy");
        assert!(value["launch"].get("contract").is_none_or(Value::is_null));
        assert_eq!(state.capabilities_reads(), 1);
        assert_eq!(before, dispatch(&state, &request(method::INFO, json!({})), &mut None));
        assert!(state.take_commands().is_empty());
        assert!(serde_json::to_vec(&value).unwrap().len() < 32 * 1024);
    }

    #[test]
    fn capabilities_rejects_bad_params_and_uses_the_existing_auth_boundary() {
        let state = ChannelState::new("endpoint".to_string());
        let invalid = dispatch(&state, &request("session.capabilities", json!([])), &mut None);
        assert_eq!(code_of(&invalid), Some(CODE_INVALID_PARAMS));
        let response = handle_line(&state, &TokenPolicy::new(Some("canary-secret".into())),
            r#"{"jsonrpc":"2.0","id":1,"method":"session.capabilities","params":{}}"#, &mut None);
        assert_eq!(code_of(&response), Some(CODE_UNAUTHORIZED));
    }

    #[test]
    fn turn_driving_methods_are_deliberately_absent() {
        let state = ChannelState::new("s".to_string());
        let mut subscribed = None;
        for absent in [
            "session.create",
            "session.load",
            "session.close",
            "session.run_turn",
            "session.run_turn_detached",
            "session.roster",
        ] {
            let response = dispatch(&state, &request(absent, json!({})), &mut subscribed);
            assert_eq!(code_of(&response), Some(CODE_METHOD_NOT_FOUND), "{absent}");
        }
    }

    #[test]
    fn steering_needs_a_turn_and_the_right_one() {
        let state = ChannelState::new("s".to_string());
        let mut subscribed = None;
        let idle = dispatch(
            &state,
            &request(method::STEER, json!({"id": "s", "text": "wait"})),
            &mut subscribed,
        );
        assert_eq!(code_of(&idle), Some(CODE_STEER_DENIED));

        let turn = state.begin_turn();
        let wrong_turn = dispatch(
            &state,
            &request(
                method::STEER,
                json!({"id": "s", "text": "wait", "turn_id": turn + 1}),
            ),
            &mut subscribed,
        );
        assert_eq!(code_of(&wrong_turn), Some(CODE_STEER_DENIED));
        assert!(state.take_commands().is_empty());

        let live = dispatch(
            &state,
            &request(method::STEER, json!({"id": "s", "text": "wait", "turn_id": turn})),
            &mut subscribed,
        );
        assert!(live.error.is_none(), "{live:?}");
        assert_eq!(
            state.take_commands(),
            vec![Command::Steer {
                text: "wait".to_string()
            }]
        );
    }

    /// A teammate between turns takes a steer as its next turn and says
    /// `queued`; a running one says `consumed` with the turn; a root session
    /// keeps refusing an idle steer (t-2513 §2.2/§2.3).
    #[test]
    fn a_teammate_takes_an_idle_steer_as_its_next_turn_and_reports_the_receipt() {
        let state = ChannelState::new("child".to_string());
        state.set_idle_steer(true);
        let mut subscribed = None;
        let idle = dispatch(
            &state,
            &request(method::STEER, json!({"id": "child", "text": "one more"})),
            &mut subscribed,
        );
        assert!(idle.error.is_none(), "{idle:?}");
        let result = idle.result.expect("result");
        assert!(result["turn_id"].is_null());
        assert_eq!(result["receipt"], "queued");
        assert_eq!(
            state.take_commands(),
            vec![Command::Steer {
                text: "one more".to_string()
            }]
        );
        // Naming a turn while none runs is still refused: the parent asked
        // for a turn that is over.
        let named = dispatch(
            &state,
            &request(method::STEER, json!({"id": "child", "text": "late", "turn_id": 3})),
            &mut subscribed,
        );
        assert_eq!(code_of(&named), Some(CODE_STEER_DENIED));

        let turn = state.begin_turn();
        let live = dispatch(
            &state,
            &request(method::STEER, json!({"id": "child", "text": "now"})),
            &mut subscribed,
        );
        let result = live.result.expect("result");
        assert_eq!(result["turn_id"], json!(turn));
        assert_eq!(result["receipt"], "consumed");
    }

    /// `teammate.close` is a teammate's door only, and it carries the
    /// parent's reason to the front-end.
    #[test]
    fn teammate_close_is_answered_by_a_teammate_and_refused_by_a_root_session() {
        let root = ChannelState::new("root".to_string());
        let mut subscribed = None;
        let refused = dispatch(
            &root,
            &request(method::TEAMMATE_CLOSE, json!({"reason": "closed_by_parent"})),
            &mut subscribed,
        );
        assert_eq!(code_of(&refused), Some(CODE_METHOD_NOT_FOUND));
        assert!(root.take_commands().is_empty());

        let child = ChannelState::new("child".to_string());
        child.set_idle_steer(true);
        let closed = dispatch(
            &child,
            &request(method::TEAMMATE_CLOSE, json!({})),
            &mut subscribed,
        );
        assert!(closed.error.is_none(), "{closed:?}");
        assert_eq!(closed.result.as_ref().and_then(|r| r.get("closing")), Some(&json!(true)));
        assert_eq!(
            child.take_commands(),
            vec![Command::Close {
                reason: "closed_by_parent".to_string()
            }]
        );
    }

    /// `auth.reload` on a parent fans the same params out to its children,
    /// off the socket loop (t-2513 §2.6).
    #[test]
    fn auth_reload_fans_its_params_out_to_the_installed_children() {
        let state = ChannelState::new("parent".to_string());
        let seen: Arc<std::sync::Mutex<Vec<Value>>> = Arc::default();
        let (told, heard) = std::sync::mpsc::channel::<Value>();
        let sink = Arc::clone(&seen);
        state.set_child_fanout(Some(Arc::new(move |params: &Value| {
            sink.lock().unwrap().push(params.clone());
            let _ = told.send(params.clone());
        })));
        let mut subscribed = None;
        let response = dispatch(
            &state,
            &request(method::AUTH_RELOAD, json!({"provider": "anthropic", "label": "work"})),
            &mut subscribed,
        );
        assert!(response.error.is_none(), "{response:?}");
        let params = heard
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the fan-out ran");
        assert_eq!(params["provider"], "anthropic");
        assert_eq!(params["label"], "work");
        // A refused reload fans out to nobody.
        let refused = dispatch(
            &state,
            &request(method::AUTH_RELOAD, json!({"provider": "nobody"})),
            &mut subscribed,
        );
        assert_eq!(code_of(&refused), Some(CODE_INVALID_PARAMS));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    /// `mcp.call` routes to the installed bridge and renders a tool error as
    /// an `is_error` answer, not a JSON-RPC failure; without a bridge the
    /// method fails by name.
    #[test]
    fn mcp_call_routes_through_the_installed_bridge_or_fails_by_name() {
        let state = ChannelState::new("parent".to_string());
        let mut subscribed = None;
        let none = dispatch(
            &state,
            &request(method::MCP_CALL, json!({"name": "mcp__ctx7__query", "input": {"q": "x"}})),
            &mut subscribed,
        );
        assert_eq!(code_of(&none), Some(CODE_METHOD_NOT_FOUND));
        state.set_mcp_bridge(Some(Arc::new(|name: &str, input: &Value| {
            if name == "mcp__ctx7__query" {
                Ok(format!("docs for {}", input["q"].as_str().unwrap_or("")))
            } else {
                Err(format!("unknown MCP tool `{name}`"))
            }
        })));
        let answered = dispatch(
            &state,
            &request(method::MCP_CALL, json!({"name": "mcp__ctx7__query", "input": {"q": "tokio"}})),
            &mut subscribed,
        );
        let result = answered.result.expect("result");
        assert_eq!(result["content"], "docs for tokio");
        assert_eq!(result["is_error"], false);
        let failed = dispatch(
            &state,
            &request(method::MCP_CALL, json!({"name": "mcp__nope", "input": {}})),
            &mut subscribed,
        );
        let result = failed.result.expect("a tool error is an answer");
        assert_eq!(result["is_error"], true);
        let bad = dispatch(&state, &request(method::MCP_CALL, json!({})), &mut subscribed);
        assert_eq!(code_of(&bad), Some(CODE_INVALID_PARAMS));
    }

    #[test]
    fn stop_on_an_idle_pane_says_so_instead_of_failing() {
        let state = ChannelState::new("s".to_string());
        let mut subscribed = None;
        let response = dispatch(&state, &request(method::CANCEL_TURN, json!({})), &mut subscribed);
        assert!(response.error.is_none());
        assert_eq!(
            response.result.as_ref().and_then(|r| r.get("cancelled")),
            Some(&json!(false))
        );
        assert!(state.take_commands().is_empty());

        let turn = state.begin_turn();
        let pressed = dispatch(
            &state,
            &request(method::CANCEL_TURN, json!({"turn_id": turn})),
            &mut subscribed,
        );
        assert_eq!(
            pressed.result.as_ref().and_then(|r| r.get("cancelled")),
            Some(&json!(true))
        );
        assert_eq!(
            state.take_commands(),
            vec![Command::CancelTurn {
                turn_id: Some(turn)
            }]
        );
    }

    #[test]
    fn an_unknown_decision_tag_denies_rather_than_allows() {
        let state = ChannelState::new("s".to_string());
        let mut subscribed = None;
        let prompt = state.register_prompt(PromptKind::Permission);
        let response = dispatch(
            &state,
            &request(
                method::PERMISSION_RESPOND,
                json!({"prompt_id": prompt, "decision": "allow_everything_forever"}),
            ),
            &mut subscribed,
        );
        assert!(response.error.is_none(), "{response:?}");
        // 답은 들어갔고, 그 답은 Deny 다.
        let landed = futures_util::future::FutureExt::now_or_never(state.wait_answer(prompt))
            .expect("answer is already there");
        assert_eq!(landed, Some(Answer::Permission(PermissionDecision::Deny)));
    }

    #[test]
    fn question_respond_accepts_a_freeform_answer() {
        let state = ChannelState::new("s".to_string());
        let mut subscribed = None;
        let prompt = state.register_prompt(PromptKind::Question);

        let response = dispatch(
            &state,
            &request(
                method::QUESTION_RESPOND,
                json!({"prompt_id": prompt, "answers": ["something else"]}),
            ),
            &mut subscribed,
        );

        assert!(response.error.is_none(), "{response:?}");
        let landed = futures_util::future::FutureExt::now_or_never(state.wait_answer(prompt))
            .expect("answer is already there");
        assert_eq!(
            landed,
            Some(Answer::Question(vec!["something else".to_string()]))
        );
    }

    /// A subscriber that attaches late gets the latest snapshot of each kind
    /// appended to the replay, and a frame published after the subscription
    /// arrives on the stream — the same fact twice at most, never lost.
    #[test]
    fn subscribe_hands_over_the_snapshot_set_and_keeps_the_stream_after_it() {
        let state = ChannelState::new("s".to_string());
        state.publish_subagents_snapshot(&json!({
            "type": "subagents", "registry": "s", "generation": 1, "running": [{"id": "a1"}]
        }));
        let mut subscribed = None;
        let response = dispatch(
            &state,
            &request(method::SUBSCRIBE, json!({"id": "s"})),
            &mut subscribed,
        );
        let history = response.result.as_ref().and_then(|r| r.get("history")).expect("history");
        let roster = history
            .as_array()
            .expect("array")
            .iter()
            .find(|frame| frame["type"] == "subagents")
            .expect("the roster snapshot rides the history");
        assert_eq!(roster["generation"], 1);
        assert_eq!(roster["running"][0]["id"], "a1");

        // Published after the subscription was taken: on the stream, and the
        // snapshot now says the same.
        state.publish_subagents_snapshot(&json!({
            "type": "subagents", "registry": "s", "generation": 2, "running": []
        }));
        let mut receiver = subscribed.expect("subscribed");
        let streamed = receiver.try_recv().expect("the later frame streams");
        let streamed: Value = serde_json::from_str(&streamed).expect("json");
        assert_eq!(streamed["generation"], 2);
        assert_eq!(
            state.hydration_history().last().map(|frame| frame["generation"].clone()),
            Some(json!(2))
        );
    }

    #[test]
    fn subscribing_to_another_session_is_refused() {
        let state = ChannelState::new("mine".to_string());
        let mut subscribed = None;
        let response = dispatch(
            &state,
            &request(method::SUBSCRIBE, json!({"id": "someone-elses", "boundary": true})),
            &mut subscribed,
        );
        assert_eq!(code_of(&response), Some(CODE_NO_SUCH_SESSION));
        assert!(subscribed.is_none());
    }

    #[test]
    fn a_guarded_channel_refuses_a_tokenless_line() {
        let state = ChannelState::new("s".to_string());
        let auth = TokenPolicy::new(Some("s3cret".to_string()));
        let mut subscribed = None;
        let refused = handle_line(
            &state,
            &auth,
            r#"{"jsonrpc":"2.0","id":7,"method":"session.list","params":{}}"#,
            &mut subscribed,
        );
        assert_eq!(code_of(&refused), Some(CODE_UNAUTHORIZED));
        assert_eq!(refused.id, 7, "클라이언트가 자기 요청과 맞출 수 있어야 한다");

        let allowed = handle_line(
            &state,
            &auth,
            r#"{"jsonrpc":"2.0","id":8,"method":"session.list","params":{},"token":"s3cret"}"#,
            &mut subscribed,
        );
        assert!(allowed.error.is_none(), "{allowed:?}");
    }

    #[test]
    fn a_line_that_is_not_a_request_is_an_invalid_request() {
        let state = ChannelState::new("s".to_string());
        let auth = TokenPolicy::default();
        let mut subscribed = None;
        for bad in [
            "not json at all",
            r#"{"id":3,"method":"session.list"}"#,
            r#"{"jsonrpc":"1.0","id":3,"method":"session.list"}"#,
        ] {
            let response = handle_line(&state, &auth, bad, &mut subscribed);
            assert_eq!(code_of(&response), Some(CODE_INVALID_REQUEST), "{bad}");
        }
    }
}
