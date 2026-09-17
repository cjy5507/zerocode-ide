//! 이벤트 채널의 봉투 — 줄 단위 JSON-RPC 와 렌더 프레임.
//!
//! 정본은 두 곳이다. 클라이언트 쪽은 이 저장소가 아닌 부모 저장소의
//! `crates/zerocode-harness/src/lib.rs`("Client for the `zo serve` session
//! server"), 서버 쪽 원문은 `port-staging/zo-cli/src/serve_protocol.rs` 다.
//! 여기 있는 것은 그 둘의 **교집합**을 zo 패인 크기로 줄인 것 — 하네스가
//! 기대하는 봉투·오류코드·구분 규칙만 담고, 하네스에 없는 것은 없다.
//!
//! 세 종류가 한 소켓을 쓰고 **구조로** 구분된다(하네스 `classify` 와 같은 규칙):
//!
//! - 요청(클라이언트 → zo): 언제나 `jsonrpc` · `id` · `method`.
//! - 응답(zo → 클라이언트): 언제나 `jsonrpc` · `id`, 그리고 `result`/`error` 중 하나.
//! - 렌더 프레임(zo → 클라이언트): `type` 을 달고 `jsonrpc` 는 **절대** 달지 않는다.
//!
//! 그래서 키 하나가 판정한다 — 하네스는 `value.get("jsonrpc").is_none()` 이면
//! 프레임으로 읽는다. 프레임 스키마는 [`crate::sinks::SerializableRenderBlock`]
//! 이 그대로 소유하므로 여기서 다시 정의하지 않는다.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 봉투에 찍히는 JSON-RPC 버전.
pub const JSONRPC_VERSION: &str = "2.0";

/// zo 패인 채널이 받는 메서드 이름. 철자는 하네스 `method` 모듈과 같다 —
/// 하네스가 부르지 않는 이름은 여기 없다(`session.create`/`run_turn` 계열은
/// 패인이 제 stdin 으로 몰기 때문에 일부러 뺐다. [`super::server`] 참조).
pub mod method {
    /// 하네스 `method::LIST`. `zerocode-lane` 의 정체 확인 핸드셰이크가
    /// 이 메서드를 쓴다(`probe_session_server` → `handshake`).
    pub const LIST: &str = "session.list";
    /// 하네스 `method::INFO`.
    pub const INFO: &str = "session.info";
    /// Read-only, versioned public process and launch facts.
    pub const CAPABILITIES: &str = "session.capabilities";
    /// 하네스 `method::SUBSCRIBE` — 이 연결을 프레임 팬아웃에 넣는다.
    pub const SUBSCRIBE: &str = "session.subscribe";
    /// 하네스 `method::UNSUBSCRIBE`.
    pub const UNSUBSCRIBE: &str = "session.unsubscribe";
    /// 하네스 `method::CANCEL_TURN` — IDE 의 Stop 버튼.
    pub const CANCEL_TURN: &str = "session.cancel_turn";
    /// 하네스 `method::STEER`.
    pub const STEER: &str = "session.steer";
    /// 하네스 `method::PERMISSION_RESPOND` — IDE 모달의 답.
    pub const PERMISSION_RESPOND: &str = "permission.respond";
    /// 하네스에는 없다. `AskUserQuestion` 프롬프트의 답이 돌아올 문으로,
    /// `ide/events.rs` 머리말이 적어 둔 in 목록에 들어 있다.
    pub const QUESTION_RESPOND: &str = "question.respond";
    /// 하네스 `method::AUTH_RELOAD` — IDE 에서 계정이 바뀌었다.
    ///
    /// params 는 **값을 싣는다**: 판이 태어날 때 받은 `CLAUDE_CONFIG_DIR` ·
    /// `CODEX_HOME` 은 이미 굳어 있어서 밖에서 고칠 수 없기 때문이다
    /// (`docs/design/zo-ide-account-oauth.md` §2.3 addendum).
    pub const AUTH_RELOAD: &str = "auth.reload";
    /// A parent asks its idle teammate to leave (t-2513 §2.2). The child
    /// writes `result-final.json{exit: closed, reason}` and exits. Spelled by
    /// the runtime, which both sides of a pane depend on.
    pub const TEAMMATE_CLOSE: &str = runtime::subagent_panes::channel_method::TEAMMATE_CLOSE;
    /// One MCP tool call from a pane child, answered by this session's MCP
    /// runtime (t-2513 §2.1) — the same road an inline child's passthrough
    /// takes, with a socket in the middle.
    pub const MCP_CALL: &str = runtime::subagent_panes::channel_method::MCP_CALL;
}

#[cfg(test)]
mod method_tests {
    /// The runtime spells the methods a parent calls on its child; this
    /// server must answer the same spellings, so the shared ones are pinned.
    #[test]
    fn the_shared_methods_are_spelled_as_the_runtime_spells_them() {
        use runtime::subagent_panes::channel_method;
        assert_eq!(super::method::LIST, channel_method::LIST);
        assert_eq!(super::method::STEER, channel_method::STEER);
        assert_eq!(super::method::CANCEL_TURN, channel_method::CANCEL_TURN);
        assert_eq!(super::method::AUTH_RELOAD, channel_method::AUTH_RELOAD);
        assert_eq!(super::method::TEAMMATE_CLOSE, "teammate.close");
        assert_eq!(super::method::MCP_CALL, "mcp.call");
    }
}

/// 봉투가 JSON-RPC 2.0 요청이 아니다.
pub const CODE_INVALID_REQUEST: i64 = -32600;
/// 이 채널이 구현하지 않는 메서드.
pub const CODE_METHOD_NOT_FOUND: i64 = -32601;
/// `params` 가 없거나 모양이 다르다.
pub const CODE_INVALID_PARAMS: i64 = -32602;
/// 서버 쪽 실패.
pub const CODE_INTERNAL: i64 = -32603;
/// 이 채널이 모르는 세션(또는 은퇴한 `prompt_id`).
pub const CODE_NO_SUCH_SESSION: i64 = -32000;
/// 토큰이 걸린 채널에 토큰이 없거나 틀렸다. 하네스는 이 코드를
/// `ServeErrorKind::Unauthorized` 로 읽고, `zerocode-lane` 의 프로브는
/// 이것만을 "세션 서버이되 우리 토큰을 거부함"으로 읽는다.
pub const CODE_UNAUTHORIZED: i64 = -32002;
/// 스티어할 턴이 없다. 하네스 `ServeErrorKind::SteerDenied`.
pub const CODE_STEER_DENIED: i64 = -32003;

/// 턴 상태 프레임의 `type`.
pub const FRAME_TURN: &str = "turn";
/// 세션 상태(모델·권한·effort·ctx) 프레임의 `type`.
pub const FRAME_SESSION_STATUS: &str = "session_status";
/// This session's complete set of currently running sub-agents.
pub const FRAME_SUBAGENTS: &str = "subagents";
/// Latest public capability snapshot.
pub const FRAME_SESSION_CAPABILITIES: &str = "session_capabilities";
/// 프롬프트가 은퇴했다는 프레임의 `type` — 패인이 먼저 답했을 때 IDE 모달을
/// 내리라는 신호다. 하네스는 모르는 `type` 을 그대로 통과시키므로
/// (`lib.rs` "Forward compatibility") 하네스 변경 없이 붙는다.
pub const FRAME_PROMPT_RESOLVED: &str = "prompt_resolved";

/// 턴 프레임이 말하는 순간.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    /// 턴이 시작했다.
    Start,
    /// 턴이 끝났다.
    End,
}

/// 턴이 끝난 사정.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    /// 모델이 턴을 마쳤다.
    Completed,
    /// 사람이(패인 Ctrl-C 든 IDE Stop 이든) 끊었다.
    Cancelled,
    /// 턴이 오류로 끝났다.
    Failed,
}

/// 프롬프트를 닫은 손.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedBy {
    /// 패인 안의 다이얼로그가 먼저 답했다.
    Pane,
    /// IDE 모달이 먼저 답했다.
    Ide,
    /// 아무도 답하지 않은 채 턴이 끝났다(런타임은 hard deny 로 읽는다).
    Dismissed,
}

/// 턴 프레임.
#[derive(Debug, Clone, Serialize)]
pub struct TurnFrame<'a> {
    /// 언제나 [`FRAME_TURN`].
    #[serde(rename = "type")]
    pub kind: &'a str,
    /// 이 프레임이 말하는 턴.
    pub turn_id: u64,
    /// 시작인가 끝인가.
    pub phase: TurnPhase,
    /// 끝일 때의 사정.
    pub outcome: Option<TurnOutcome>,
    /// 끝이 실패였을 때의 사유.
    pub error: Option<&'a str>,
}

impl<'a> TurnFrame<'a> {
    /// 턴이 시작했다.
    #[must_use]
    pub fn started(turn_id: u64) -> Self {
        Self {
            kind: FRAME_TURN,
            turn_id,
            phase: TurnPhase::Start,
            outcome: None,
            error: None,
        }
    }

    /// 턴이 끝났다.
    #[must_use]
    pub fn ended(turn_id: u64, outcome: TurnOutcome, error: Option<&'a str>) -> Self {
        Self {
            kind: FRAME_TURN,
            turn_id,
            phase: TurnPhase::End,
            outcome: Some(outcome),
            error,
        }
    }
}

/// 프롬프트 은퇴 프레임 — `prompt_id` 는 더 이상 답을 받지 않는다.
#[derive(Debug, Clone, Serialize)]
pub struct PromptResolvedFrame<'a> {
    /// 언제나 [`FRAME_PROMPT_RESOLVED`].
    #[serde(rename = "type")]
    pub kind: &'a str,
    /// 은퇴한 프롬프트.
    pub prompt_id: u64,
    /// 누가 닫았는가.
    pub by: ResolvedBy,
}

impl PromptResolvedFrame<'_> {
    /// 은퇴 프레임 하나.
    #[must_use]
    pub fn new(prompt_id: u64, by: ResolvedBy) -> Self {
        Self {
            kind: FRAME_PROMPT_RESOLVED,
            prompt_id,
            by,
        }
    }
}

/// 클라이언트 → zo 요청 한 줄.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcRequest {
    /// 언제나 `"2.0"`.
    pub jsonrpc: String,
    /// 응답이 되돌려 줄 요청 id.
    pub id: u64,
    /// 메서드 이름.
    pub method: String,
    /// 메서드 파라미터. 빠지면 `null`.
    #[serde(default)]
    pub params: Value,
    /// 공유 비밀. 토큰이 걸린 채널에서만 실린다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// zo → 클라이언트 응답 한 줄.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcResponse {
    /// 언제나 `"2.0"`.
    pub jsonrpc: String,
    /// 대응하는 요청 id.
    pub id: u64,
    /// 성공 결과.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// 실패 사유.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    /// `result` 를 실은 성공 응답.
    #[must_use]
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// 코드와 사유를 실은 실패 응답.
    #[must_use]
    pub fn err(id: u64, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

/// JSON-RPC 오류 몸통.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcError {
    /// 위의 `CODE_*` 중 하나.
    pub code: i64,
    /// 사람이 읽는 사유.
    pub message: String,
}

/// 한 줄이 응답인가(프레임이 아니라). 하네스 `classify` 와 같은 한 키 규칙.
#[must_use]
pub fn is_response_line(line: &Value) -> bool {
    line.get("jsonrpc").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_told_from_a_response_by_one_key() {
        let response = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {}});
        let frame = serde_json::json!({"type": "text_delta", "id": 1, "text": "hi", "done": false});
        assert!(is_response_line(&response));
        assert!(!is_response_line(&frame));
        // 우리가 더한 상태 프레임도 같은 규칙 아래 있어야 한다: `type` 은 있고
        // `jsonrpc` 는 없다.
        for added in [
            serde_json::to_value(TurnFrame::started(1)).expect("serialize"),
            serde_json::to_value(TurnFrame::ended(1, TurnOutcome::Cancelled, None))
                .expect("serialize"),
            serde_json::to_value(PromptResolvedFrame::new(3, ResolvedBy::Pane)).expect("serialize"),
        ] {
            assert!(!is_response_line(&added), "{added}");
            assert!(added.get("type").is_some(), "{added}");
        }
    }

    #[test]
    fn the_wire_spells_phases_and_outcomes_in_snake_case() {
        let ended = serde_json::to_value(TurnFrame::ended(4, TurnOutcome::Failed, Some("boom")))
            .expect("serialize");
        assert_eq!(ended["phase"], serde_json::json!("end"));
        assert_eq!(ended["outcome"], serde_json::json!("failed"));
        assert_eq!(ended["error"], serde_json::json!("boom"));
        let resolved =
            serde_json::to_value(PromptResolvedFrame::new(9, ResolvedBy::Dismissed)).expect("ok");
        assert_eq!(resolved["by"], serde_json::json!("dismissed"));
    }

    #[test]
    fn a_tokenless_request_puts_no_token_on_the_wire() {
        let request = RpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: 1,
            method: method::LIST.to_string(),
            params: Value::Null,
            token: None,
        };
        let line = serde_json::to_string(&request).expect("serialize");
        assert!(!line.contains("token"), "{line}");
    }

    #[test]
    fn a_request_without_params_defaults_to_null() {
        let request: RpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"session.list"}"#)
                .expect("deserialize");
        assert_eq!(request.params, Value::Null);
    }

    #[test]
    fn responses_carry_exactly_one_of_result_and_error() {
        let ok = serde_json::to_string(&RpcResponse::ok(1, Value::Object(serde_json::Map::new()))).expect("serialize");
        assert!(ok.contains("result") && !ok.contains("error"), "{ok}");
        let err =
            serde_json::to_string(&RpcResponse::err(1, CODE_UNAUTHORIZED, "no")).expect("serialize");
        assert!(err.contains("error") && !err.contains("result"), "{err}");
    }
}
