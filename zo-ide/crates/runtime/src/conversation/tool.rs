//! Tool-execution seam.
//!
//! The conversation loop interacts with model-requested tools through a
//! tiny [`ToolExecutor`] trait that any dispatcher (the real
//! `GlobalToolRegistry`, a mock, or [`StaticToolExecutor`]) can satisfy.
//!
//! Three concerns live here:
//!
//! 1. [`ToolExecutor`] / [`StaticToolExecutor`] — the executor seam and a
//!    lightweight in-memory implementation used by tests.
//! 2. [`ConcurrentDispatchFn`] — the thread-safe alternative used by the
//!    live streaming path to run tool dispatch on blocking workers.
//! 3. Pure policy helpers — [`is_concurrency_safe`],
//!    [`sleep_tool_execution_input`], and [`tool_execution_input`] — plus the
//!    `unblock_tool_execute` fallback for hosts without a thread-safe dispatch
//!    seam.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::MAX_SLEEP_TOOL_DURATION_MS;
use super::error::ToolError;

/// Thread-safe function that can execute a tool by name and JSON input.
/// Captures shared state (e.g. `Arc<GlobalToolRegistry>`) so live streaming
/// can send ordinary tool dispatch to `spawn_blocking`. Read-only tools may
/// still fan out in parallel; mutating tools are awaited one-by-one by the
/// conversation loop.
use super::error::ToolTextKind;

pub type ConcurrentDispatchFn = Arc<dyn Fn(&str, &str) -> Result<String, ToolError> + Send + Sync>;

/// Predicate the host installs to mark tools whose execution may block. This is
/// retained as compatibility metadata for dynamic plugin/MCP registrations; the
/// live streaming path uses [`ConcurrentDispatchFn`] for every ordinary tool
/// when that seam is installed.
pub type LongRunningPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Trait implemented by tool dispatchers that execute model-requested tools.
pub trait ToolExecutor {
    /// Reset evidence scoped to one user turn; internal continuations keep it.
    fn begin_user_turn(&mut self) {}

    /// Server-pinned working directory used by Bash when input has no cwd.
    fn execution_cwd(&self) -> Option<&Path> { None }

    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError>;

    /// Out-of-band images (`media_type`, base64) staged by the most recent
    /// [`Self::execute`] (e.g. by a `read_image` tool). The conversation loop
    /// drains this right after a serial tool runs and attaches the images to
    /// that tool's result. The default returns none, so text-only executors
    /// (tests, `StaticToolExecutor`) need no change.
    fn take_pending_images(&mut self) -> Vec<(String, String)> {
        Vec::new()
    }
}

type ToolHandler = Box<dyn FnMut(&str) -> Result<String, ToolError>>;

/// Simple in-memory tool executor for tests and lightweight integrations.
#[derive(Default)]
pub struct StaticToolExecutor {
    handlers: BTreeMap<String, ToolHandler>,
}

impl StaticToolExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn register(
        mut self,
        tool_name: impl Into<String>,
        handler: impl FnMut(&str) -> Result<String, ToolError> + 'static,
    ) -> Self {
        self.handlers.insert(tool_name.into(), Box::new(handler));
        self
    }
}

impl ToolExecutor for StaticToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        self.handlers
            .get_mut(tool_name)
            .ok_or_else(|| ToolError::new(format!("unknown tool: {tool_name}")))?(input)
    }
}

/// Run a synchronous tool execution on the legacy executor path.
///
/// This is only a fallback for hosts that have not installed
/// [`ConcurrentDispatchFn`]. In live streaming, even a single blocking call can
/// pause the outer turn future, so the main loop prefers `spawn_blocking` via
/// the thread-safe dispatch seam whenever it is available.
pub(super) fn unblock_tool_execute<T: ToolExecutor>(
    executor: &mut T,
    tool_name: &str,
    input: &str,
) -> (String, ToolTextKind) {
    let mut run = || match executor.execute(tool_name, input) {
        Ok(output) => (output, ToolTextKind::Answer),
        Err(error) => (error.to_string(), error.kind()),
    };
    match tokio::runtime::Handle::try_current() {
        Ok(h)
            if matches!(
                h.runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            ) =>
        {
            tokio::task::block_in_place(&mut run)
        }
        _ => run(),
    }
}

/// Allow-list of tools whose side effects can safely run concurrently.
/// Only pure read-only / lookup operations belong here — anything that
/// mutates the workspace must execute sequentially so subsequent reads
/// observe the write.
pub(super) fn is_concurrency_safe(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Read"
            | "read_file"
            | "Glob"
            | "glob_search"
            | "Grep"
            | "grep_search"
            | "WebFetch"
            | "WebSearch"
            | "TaskGet"
            | "TaskList"
            | "TaskOutput"
            | "ToolSearch"
            | "ListMcpResourcesTool"
            // `ReadMcpResourceTool` can stage out-of-band images through the
            // shared pending-image sink, so it must stay on the ordered path
            // where image drains are attributed to the just-finished tool.
            | "LSP"
            // GetAgentCompletion 은 read-only polling — 같은 turn 에 여러 개
            // 호출되면 병렬 spawn_blocking 으로 worker 절약.
            | "GetAgentCompletion"
    )
}

/// 호출이 **수십 ms~수십 초** 차단될 수 있는 도구.
///
/// This list is retained for host predicates and tests, but live streaming no
/// longer relies on this allow-list as the only guard: when
/// [`ConcurrentDispatchFn`] is installed, every ordinary permitted tool runs
/// through `spawn_blocking`, while mutating order remains sequential.
///
/// **왜 `block_in_place` 로 부족한가:** live TUI 의 turn loop
/// (`turn_controller.rs`) 는 agent future 와 `render_tick` 을 **같은
/// `select!`** 에서 폴링한다. 도구가 `block_in_place` 로 동기 차단되면 그
/// `select!` future 전체가 현재 스레드에서 멈춰 `render_tick` 팔이 돌지
/// 못한다 → spinner/elapsed 가 얼고, 도구 완료 시 누적된 `RenderBlock` 이
/// 한꺼번에 쏟아진다 (freeze-then-burst). `spawn_blocking().await` 는 그
/// 지점에서 future 를 yield 해 `render_tick` 이 매 frame 정상 동작한다.
/// (`concurrent_dispatch` 가 설정된 live 경로에서만 `spawn_blocking` 분기를
/// 탄다 — [`crate::conversation`] 의 dispatch 참조.)
///
/// 등록 대상: 서브프로세스(`Bash`), 파일/절차 로드(`Skill`), 네트워크
/// I/O(`WebFetch`/`WebSearch`, `RemoteTrigger` 의 HTTP 왕복, `MCPTool` 의 MCP 서버 RPC),
/// Condvar/sleep 기반 폴링(`GetAgentCompletion`/
/// `SpawnMultiAgent`). 직렬 순서는 유지된다 — Pass 3 가 각 도구의
/// `spawn_blocking` 을 `.await` 로 기다린 뒤 다음 도구를 실행하므로
/// mutate 도구의 순서 보장은 깨지지 않는다.
///
/// `RemoteTrigger`/`MCPTool` 는 모두 동기 dispatch 래퍼가 내부에서
/// `block_on`(HTTP) 또는 MCP 서버 IO 로 차단될 수 있다. 등록하지 않으면
/// `block_in_place` 경로를 타 turn loop 의 `render_tick` 이 멈춰
/// spinner/elapsed 가 얼고 freeze-then-burst 가 발생한다 (`WebFetch` 와
/// 동일 패턴).
#[allow(dead_code)]
pub(super) fn is_long_running(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Bash"
            | "bash"
            | "WebFetch"
            | "WebSearch"
            | "Skill"
            | "RemoteTrigger"
            | "MCPTool"
            | "GetAgentCompletion"
            | "SpawnMultiAgent"
            // A single `Agent` call now blocks until the sub-agent finishes
            // (synchronous, like `SpawnMultiAgent`), so it must take the
            // `spawn_blocking` path too — otherwise the wait freezes the TUI
            // render loop instead of streaming the sub-agent's progress.
            | "Agent"
            | "Workflow"
            // Filesystem walks over a large worktree can block for seconds.
            | "Glob"
            | "glob_search"
            | "Grep"
            | "grep_search"
            // `session_recall` search mode loads and scans up to 100 prior
            // session transcripts off disk synchronously — seconds of blocking
            // I/O for a user with many sessions. Without this it freezes the TUI
            // render loop (frozen spinner/elapsed, then a burst) like Glob/Grep.
            | "session_recall"
            // Language-server / MCP-server RPC — same blocking network/IO as
            // `MCPTool`; without this they take the `block_in_place` path and
            // freeze the TUI render loop on a slow server (the same
            // freeze-then-burst as `WebFetch`).
            | "LSP"
            | "ListMcpResourcesTool"
            | "ReadMcpResourceTool"
    )
}

/// The spawn family: `Agent`, `Task`, `SpawnMultiAgent`, `Workflow`.
///
/// A spawn owns the slice it was given, and several issued in one message are
/// the model asking for them side by side — Claude Code runs its `Task` calls
/// that way. They change the workspace through sessions of their own, not
/// through this turn's tools, so they ride a wave with reads and with each
/// other; an ordered tool between them still ends the wave and runs alone in
/// model order. Four `Agent` calls in one message ran one after another
/// before, each waiting for the previous to finish ("병렬로 돌리지 않음",
/// 2026-09-03).
#[must_use]
pub fn is_fan_out_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Agent" | "Task" | "SpawnMultiAgent" | "Workflow")
}

/// Whether a tool may run in a wave with its neighbours: a read, or a spawn.
pub(super) fn rides_a_wave(tool_name: &str) -> bool {
    is_concurrency_safe(tool_name) || is_fan_out_tool(tool_name)
}

/// Tools whose execution input carries the owning `tool_use` id. Spawn-family
/// calls use it for manifest attribution; Bash uses it for keyed live output.
fn carries_tool_call_id(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Agent" | "Task" | "SpawnMultiAgent" | "Workflow" | "Bash" | "bash"
    )
}

/// Smuggle the owning `tool_use` id into execution-only JSON. Spawn-family
/// dispatch stamps manifests with it; Bash keys its live-output registration
/// with it. Returns `None` for other tools or non-object input.
pub(super) fn tool_execution_input(
    tool_name: &str,
    tool_use_id: &str,
    input: &str,
) -> Option<String> {
    if !carries_tool_call_id(tool_name) || tool_use_id.is_empty() {
        return None;
    }
    let mut value = serde_json::from_str::<Value>(input).ok()?;
    let Value::Object(map) = &mut value else {
        return None;
    };
    map.insert(
        "__zo_tool_call_id".to_string(),
        Value::String(tool_use_id.to_string()),
    );
    Some(value.to_string())
}

/// Decode a `Sleep` tool invocation into the actual wait duration plus
/// the rewritten input JSON the dispatcher will pass through to record
/// the side effect. Returns `None` for any other tool so the dispatcher
/// can keep the original input.
pub(super) fn sleep_tool_execution_input(
    tool_name: &str,
    input: &str,
) -> Option<(Duration, String)> {
    if tool_name != "Sleep" {
        return None;
    }

    let mut value = serde_json::from_str::<Value>(input).ok()?;
    let duration_ms = value
        .get("duration_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(MAX_SLEEP_TOOL_DURATION_MS);

    if let Value::Object(map) = &mut value {
        map.insert("__zo_already_slept".to_string(), Value::Bool(true));
    }

    Some((Duration::from_millis(duration_ms), value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_input_carries_the_owning_tool_use_id() {
        let rewritten =
            tool_execution_input("SpawnMultiAgent", "toolu_123", r#"{"agents": []}"#)
                .expect("spawn-family input is rewritten");
        let value: Value = serde_json::from_str(&rewritten).expect("valid JSON");
        assert_eq!(
            value.get("__zo_tool_call_id").and_then(Value::as_str),
            Some("toolu_123")
        );
        assert!(value.get("agents").is_some(), "original fields preserved");
    }

    #[test]
    fn non_spawn_tools_and_bad_input_keep_the_original() {
        assert_eq!(
            tool_execution_input("bash", "toolu_bash", r#"{"command":"pwd"}"#)
                .and_then(|json| serde_json::from_str::<Value>(&json).ok())
                .and_then(|value| value.get("__zo_tool_call_id").cloned()),
            Some(Value::String("toolu_bash".to_string()))
        );
        assert!(tool_execution_input("Read", "toolu_123", "{}").is_none());
        assert!(tool_execution_input("Agent", "", "{}").is_none());
        assert!(tool_execution_input("Agent", "toolu_123", "not json").is_none());
        assert!(tool_execution_input("Agent", "toolu_123", "[1]").is_none());
    }
}
