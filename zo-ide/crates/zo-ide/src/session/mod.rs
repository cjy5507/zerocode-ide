//! 세션 계층 — 엔진 빌드·턴 구동·권한/질문 브리지.
//!
//! zo-cli `session/` 에서 **터미널 비결합 파일만** 가져왔다. `LiveCli`(5.7k줄,
//! 164개 명령의 세션 객체)는 가져오지 않고, 그 자리를 얇은 [`plain_session`]
//! 이 대신한다. 모듈 이름은 zo-cli 와 같게 두어(`crate::session::*`) 포팅된
//! 파일의 경로가 그대로 컴파일된다.

mod built_runtime;
mod agent_completion_pump;
mod dreamer_hook;
mod lsp_runtime;
mod mcp_runtime;
mod orchestration;
#[doc(hidden)]
pub mod process_lifecycle;
pub mod permission_bridge;
pub mod plain_session;
mod request_types;
pub mod runtime_bridge;
mod runtime_builder;
mod smart_runtime;
pub mod stream;
pub(crate) mod subagent_progress;
mod tool_toggles;
mod turn_harness;
pub(crate) mod turn_scaffold;
pub mod user_question_bridge;

use std::path::PathBuf;

pub(crate) use built_runtime::{BuiltRuntime, RuntimePluginState};
pub(crate) use agent_completion_pump::{AgentCompletionPump, AgentFollowup};
pub(crate) use lsp_runtime::{build_runtime_lsp_state, RuntimeLspState};
pub(crate) use mcp_runtime::{build_runtime_mcp_state, PendingMcpImages, RuntimeMcpState};
pub(crate) use request_types::{
    ListMcpResourcesRequest, McpToolRequest, ReadMcpResourceRequest, ToolSearchRequest,
};
pub(crate) use runtime_builder::build_runtime_plugin_state_with_loader;
pub(crate) use turn_harness::TurnHarness;

/// 관리 세션 하나의 (id, 트랜스크립트 경로).
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub id: String,
    pub path: PathBuf,
}

/// 세션 목록 행 — `session_registry` 가 디스크에서 읽어 채운다.
#[derive(Debug, Clone)]
pub struct ManagedSessionSummary {
    pub id: String,
    pub name: Option<String>,
    pub path: PathBuf,
    pub modified_epoch_millis: u128,
    /// 트랜스크립트 머리글의 `created_at_ms`. codex `Row::created_at` 자리이고
    /// `/resume` 의 `Sort: [Created]` 가 읽는 축이다 — 파일이 그 열쇠를 들지
    /// 않으면 `Session::from_json` 이 지금 시각으로 메우므로 언제나 값이 있다.
    pub created_epoch_millis: u128,
    pub parent_session_id: Option<String>,
    pub branch_name: Option<String>,
    pub first_user_text: Option<String>,
}
