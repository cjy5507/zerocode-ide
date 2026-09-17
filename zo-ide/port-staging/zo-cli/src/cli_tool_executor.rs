//! [`CliToolExecutor`] — the CLI's [`ToolExecutor`] implementation.
//!
//! A behaviour-preserving SRP split lifting the tool-execution concern out of
//! `main.rs`: it routes a tool call to the search tool, an MCP runtime tool, or
//! the built-in registry, and (outside the TUI) streams the formatted result to
//! stdout. The crate root re-exports it so existing `crate::CliToolExecutor`
//! references are unchanged.

use std::io;
use std::sync::{Arc, Mutex};

use runtime::{ToolError, ToolExecutor};
use tools::GlobalToolRegistry;

use crate::cli_args::{AllowedToolSet, DisallowedToolSet};
use crate::render::TerminalRenderer;
use crate::session::{PendingMcpImages, RuntimeMcpState, ToolSearchRequest};
use crate::{format_tool_result, tui_active};

const TOOL_INPUT_PREVIEW_CHARS: usize = 240;

/// Per-turn allowed-tools set shared between the serial executor and the
/// concurrent dispatch closure, so both execution paths enforce the same
/// one-turn restriction. A shared slot (not a rebuilt executor) because the
/// restriction must NOT ride the wire advertisement: shrinking the `tools`
/// array for one turn and restoring it the next changes the first prompt-cache
/// block twice per scoped command, and a change there cold-rewrites the whole
/// cached prefix (~the full context, "tool definitions changed" in the cache
/// diagnostics). Execution-time gating keeps the advertisement byte-stable by
/// construction.
pub(crate) type SharedTurnAllowedTools = Arc<Mutex<Option<AllowedToolSet>>>;

pub(crate) struct CliToolExecutor {
    renderer: TerminalRenderer,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    disallowed_tools: Option<DisallowedToolSet>,
    turn_allowed_tools: SharedTurnAllowedTools,
    tool_registry: GlobalToolRegistry,
    mcp_state: Option<Arc<Mutex<RuntimeMcpState>>>,
    mcp_pending_images: Option<PendingMcpImages>,
}

impl CliToolExecutor {
    pub(crate) fn new(
        allowed_tools: Option<AllowedToolSet>,
        disallowed_tools: Option<DisallowedToolSet>,
        emit_output: bool,
        tool_registry: GlobalToolRegistry,
        mcp_state: Option<Arc<Mutex<RuntimeMcpState>>>,
    ) -> Self {
        let mcp_pending_images = mcp_state
            .as_ref()
            .and_then(Self::pending_image_sink_from_state);
        Self {
            renderer: TerminalRenderer::new(),
            emit_output,
            allowed_tools,
            disallowed_tools,
            turn_allowed_tools: Arc::new(Mutex::new(None)),
            tool_registry,
            mcp_state,
            mcp_pending_images,
        }
    }

    /// Handle to the per-turn allowed-tools slot, for the concurrent dispatch
    /// closure (same gate on both execution paths) and the turn build site.
    pub(crate) fn shared_turn_allowed_tools(&self) -> SharedTurnAllowedTools {
        Arc::clone(&self.turn_allowed_tools)
    }

    /// Install (or clear, with `None`) this turn's allowed-tools restriction.
    /// Interior mutability so the turn build site can set it through the
    /// runtime's `&self` executor accessor.
    pub(crate) fn set_turn_allowed_tools(&self, set: Option<AllowedToolSet>) {
        *self
            .turn_allowed_tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = set;
    }

    fn pending_image_sink_from_state(
        state: &Arc<Mutex<RuntimeMcpState>>,
    ) -> Option<PendingMcpImages> {
        match state.try_lock() {
            Ok(guard) => Some(guard.pending_image_sink()),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                Some(poisoned.into_inner().pending_image_sink())
            }
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }

    fn mcp_pending_image_sink(&mut self) -> Option<PendingMcpImages> {
        if self.mcp_pending_images.is_none() {
            if let Some(mcp_state) = &self.mcp_state {
                self.mcp_pending_images = Self::pending_image_sink_from_state(mcp_state);
            }
        }
        self.mcp_pending_images.clone()
    }

    fn mcp_search_metadata(&self) -> (Option<Vec<String>>, Option<runtime::McpDegradedReport>) {
        let Some(state) = &self.mcp_state else {
            return (None, None);
        };
        match state.try_lock() {
            Ok(state) => (state.pending_servers(), state.degraded_report()),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                let state = poisoned.into_inner();
                (state.pending_servers(), state.degraded_report())
            }
            Err(std::sync::TryLockError::WouldBlock) => (None, None),
        }
    }

    fn execute_search_tool(&self, value: serde_json::Value) -> Result<String, ToolError> {
        let input: ToolSearchRequest = serde_json::from_value(value)
            .map_err(|error| ToolError::new(format!("invalid tool input JSON: {error}")))?;
        let (pending_mcp_servers, mcp_degraded) = self.mcp_search_metadata();
        serde_json::to_string_pretty(&self.tool_registry.search(
            &input.query,
            input.max_results.unwrap_or(5),
            pending_mcp_servers,
            mcp_degraded,
        ))
        .map_err(|error| ToolError::new(error.to_string()))
    }

    fn execute_runtime_tool(
        &self,
        tool_name: &str,
        value: serde_json::Value,
    ) -> Result<String, ToolError> {
        let Some(mcp_state) = &self.mcp_state else {
            return Err(ToolError::new(format!(
                "runtime tool `{tool_name}` is unavailable without configured MCP servers"
            )));
        };
        let mut mcp_state = mcp_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Shared with the concurrent-dispatch path (see `runtime_support`) so
        // meta-tools route identically regardless of serial vs spawn_blocking.
        mcp_state.dispatch_runtime_tool(tool_name, value)
    }

    #[must_use]
    pub(crate) fn tool_registry_mut(&mut self) -> &mut GlobalToolRegistry {
        &mut self.tool_registry
    }
}

pub(crate) fn parse_tool_input_json(
    tool_name: &str,
    input: &str,
) -> Result<serde_json::Value, ToolError> {
    // A no-argument tool (e.g. `Audit`, which takes none) legitimately arrives
    // with an empty or whitespace-only argument payload — the model emits no JSON
    // body. `serde_json::from_str("")` then fails with "EOF while parsing a value
    // at line 1 column 0", rejecting a perfectly valid call (the repeated
    // `Audit` failures the user hit). Coerce an empty payload to an empty object
    // so no-arg tools dispatch cleanly, matching CC's tolerance.
    if input.trim().is_empty() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str(input).map_err(|error| {
        ToolError::new(format!(
            "tool input for `{tool_name}` was not valid JSON, so the tool was not executed. \
             Reissue the tool call with complete valid JSON arguments. Parser error: {error}. \
             Input preview: {}",
            preview_tool_input(input)
        ))
    })
}

fn preview_tool_input(input: &str) -> String {
    let preview: String = input.chars().take(TOOL_INPUT_PREVIEW_CHARS).collect();
    if input.chars().count() > TOOL_INPUT_PREVIEW_CHARS {
        format!("{preview}...[truncated]")
    } else {
        preview
    }
}

/// One-turn allowed-tools gate shared by the serial executor and the
/// concurrent dispatch closure. `Err` names the permitted set so the model
/// self-corrects instead of retrying blind. The restriction is enforced here —
/// at execution time — precisely so it never touches the wire `tools`
/// advertisement (see [`SharedTurnAllowedTools`]).
///
/// Always exempt, regardless of the scoped set:
/// - `ToolSearch`: with the advertisement no longer shrunk to the scoped set,
///   a scoped deferred tool's schema only reaches the model through
///   `ToolSearch` — gating it would deadlock the very tools the command
///   scoped itself to.
/// - `AskUserQuestion`: user interaction is not a capability the scope
///   restricts (the streaming loop answers it inline without ever reaching
///   this gate; exempting it keeps the sync path consistent instead of
///   asymmetric).
pub(crate) fn check_turn_allowed_tools(
    slot: &SharedTurnAllowedTools,
    tool_name: &str,
) -> Result<(), ToolError> {
    if matches!(tool_name, "ToolSearch" | "AskUserQuestion") {
        return Ok(());
    }
    let guard = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(allowed) = guard.as_ref() {
        if !allowed.contains(tool_name) {
            let permitted = allowed.iter().cloned().collect::<Vec<_>>().join(", ");
            return Err(ToolError::new(format!(
                "tool `{tool_name}` is outside this turn's allowed-tools set; \
                 this turn permits only: {permitted}"
            )));
        }
    }
    Ok(())
}

/// Session-wide operator deny gate. Call this before every allow-list gate so
/// a tool present in both sets is denied deterministically.
pub(crate) fn check_disallowed_tools(
    disallowed: Option<&DisallowedToolSet>,
    tool_name: &str,
) -> Result<(), ToolError> {
    if disallowed.is_some_and(|tools| tools.contains(tool_name)) {
        return Err(ToolError::new(format!(
            "tool `{tool_name}` was disallowed by the operator via --disallowedTools; it cannot be executed in this session"
        )));
    }
    Ok(())
}

impl ToolExecutor for CliToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        check_disallowed_tools(self.disallowed_tools.as_ref(), tool_name)?;
        if self
            .allowed_tools
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(tool_name))
        {
            return Err(ToolError::new(format!(
                "tool `{tool_name}` is not enabled by the current --allowedTools setting"
            )));
        }
        check_turn_allowed_tools(&self.turn_allowed_tools, tool_name)?;
        let input = if tool_name == "TaskList" && input.trim().is_empty() {
            "{}"
        } else {
            input
        };
        let value = parse_tool_input_json(tool_name, input)?;
        let result = if tool_name == "ToolSearch" {
            self.execute_search_tool(value)
        } else if self.tool_registry.has_runtime_tool(tool_name) {
            self.execute_runtime_tool(tool_name, value)
        } else {
            self.tool_registry
                .execute(tool_name, &value)
                .map_err(|e| ToolError::new(e.to_string()))
        };
        // While the TUI owns the terminal, suppress stdout markdown
        // streaming — the TUI renders tool results through RenderBlock
        // events, and any direct stdout write would corrupt the
        // alt-screen frame (the "staircase" bug).
        let should_emit = self.emit_output && !tui_active();
        match result {
            Ok(output) => {
                if should_emit {
                    let markdown = format_tool_result(tool_name, &output, false);
                    self.renderer
                        .stream_markdown(&markdown, &mut io::stdout())
                        .map_err(|error| ToolError::new(error.to_string()))?;
                }
                Ok(output)
            }
            Err(error) => {
                if should_emit {
                    let markdown = format_tool_result(tool_name, &error.to_string(), true);
                    self.renderer
                        .stream_markdown(&markdown, &mut io::stdout())
                        .map_err(|stream_error| ToolError::new(stream_error.to_string()))?;
                }
                Err(error)
            }
        }
    }

    fn take_pending_images(&mut self) -> Vec<(String, String)> {
        // Drain whatever the just-run tool (e.g. read_image) staged into the
        // registry's shared ToolContext sink.
        let mut images = {
            let mut guard = self
                .tool_registry
                .context()
                .image_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *guard)
        };
        // MCP tool results stage their image content on a small sink mutex that
        // is deliberately separate from the main RuntimeMcpState lock. Startup
        // discovery can hold that main lock for slow MCP handshakes; draining
        // images happens on the TUI turn future, so it must never wait for the
        // discovery/manager mutex and freeze the render tick.
        if let Some(mcp_pending_images) = self.mcp_pending_image_sink() {
            let mut staged = mcp_pending_images
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            images.extend(std::mem::take(&mut *staged));
        }
        images
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use runtime::ToolExecutor;
    use serde_json::Value;
    use tools::GlobalToolRegistry;

    use super::{check_disallowed_tools, parse_tool_input_json, CliToolExecutor};
    use crate::cli_args::AllowedToolSet;
    use crate::session::RuntimeMcpState;

    #[test]
    fn disallowed_tools_win_over_allowed_tools_before_execution() {
        let tools: AllowedToolSet = ["read_file".to_string()].into_iter().collect();
        let mut executor = CliToolExecutor::new(
            Some(tools.clone()),
            Some(tools),
            false,
            GlobalToolRegistry::builtin(),
            None,
        );

        let error = executor
            .execute("read_file", r#"{"path":"Cargo.toml"}"#)
            .expect_err("operator deny must override the allow list");
        let message = error.to_string();
        assert!(message.contains("disallowed by the operator"));
        assert!(message.contains("cannot be executed in this session"));
    }

    #[test]
    fn concurrent_disallowed_gate_returns_actionable_operator_denial() {
        let denied = ["grep_search".to_string()].into_iter().collect();
        let error = check_disallowed_tools(Some(&denied), "grep_search")
            .expect_err("concurrent dispatch must enforce the same deny set");
        assert!(error.to_string().contains("disallowed by the operator"));
    }

    #[test]
    fn parse_tool_input_json_reports_retriable_truncated_arguments() {
        let error = parse_tool_input_json(
            "bash",
            r#"{"command":"rg -n \"CargoAction|GitAction|ProcessSpec"#,
        )
        .expect_err("truncated JSON must not parse");
        let message = error.to_string();

        assert!(message.contains("tool input for `bash` was not valid JSON"));
        assert!(message.contains("tool was not executed"));
        assert!(message.contains("Reissue the tool call with complete valid JSON arguments"));
        assert!(message.contains("EOF while parsing a string"));
    }

    #[test]
    fn empty_arguments_coerce_to_empty_object_for_no_arg_tools() {
        // A no-arg tool like `Audit` arrives with an empty payload; that must
        // dispatch as `{}`, not fail with "EOF while parsing a value".
        for payload in ["", "   ", "\n", "\t "] {
            let value = parse_tool_input_json("Audit", payload).unwrap_or_else(|error| {
                panic!("empty args must coerce, got {error} for {payload:?}")
            });
            assert_eq!(value, serde_json::json!({}));
        }
        // A non-empty but malformed payload still reports the error.
        assert!(parse_tool_input_json("bash", "{not json").is_err());
    }
    #[test]
    fn take_pending_images_does_not_wait_for_main_mcp_state_lock() {
        let registry = GlobalToolRegistry::builtin();
        {
            let mut sink = registry
                .context()
                .image_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sink.push(("image/png".to_string(), "REGISTRY_IMAGE".to_string()));
        }

        let mcp_state = Arc::new(Mutex::new(RuntimeMcpState::empty()));
        let mcp_sink = {
            let state = mcp_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.pending_image_sink()
        };
        {
            let mut sink = mcp_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sink.push(("image/jpeg".to_string(), "MCP_IMAGE".to_string()));
        }

        let mut executor =
            CliToolExecutor::new(None, None, false, registry, Some(Arc::clone(&mcp_state)));
        assert!(
            executor.mcp_pending_images.is_some(),
            "executor should cache the MCP image sink before background discovery can contend"
        );

        let main_lock = mcp_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let first = ToolExecutor::take_pending_images(&mut executor);
            let second = ToolExecutor::take_pending_images(&mut executor);
            tx.send((first, second)).expect("send image drain result");
        });

        let outcome = rx.recv_timeout(Duration::from_secs(2));
        drop(main_lock);
        handle.join().expect("image drain thread should finish");
        let (first, second) = outcome.expect(
            "take_pending_images must not wait for the main RuntimeMcpState lock held by MCP discovery",
        );

        assert_eq!(
            first,
            vec![
                ("image/png".to_string(), "REGISTRY_IMAGE".to_string()),
                ("image/jpeg".to_string(), "MCP_IMAGE".to_string()),
            ],
            "registry and MCP images drain together without touching the main MCP lock"
        );
        assert!(second.is_empty(), "staged images drain exactly once");
    }

    #[test]
    fn tool_search_does_not_wait_for_main_mcp_state_lock() {
        let mcp_state = Arc::new(Mutex::new(RuntimeMcpState::empty()));
        {
            let mut state = mcp_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.pending_servers = vec!["slow-server".to_string()];
        }
        let mut executor = CliToolExecutor::new(
            None,
            None,
            false,
            GlobalToolRegistry::builtin(),
            Some(Arc::clone(&mcp_state)),
        );

        let main_lock = mcp_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result = executor
                .execute("ToolSearch", r#"{"query":"read","max_results":1}"#)
                .map_err(|error| error.to_string());
            tx.send(result).expect("send ToolSearch result");
        });

        let outcome = rx.recv_timeout(Duration::from_secs(2));
        drop(main_lock);
        handle.join().expect("ToolSearch thread should finish");
        let output = outcome
            .expect("ToolSearch metadata lookup must not wait for active MCP discovery")
            .expect("ToolSearch should succeed while MCP metadata is temporarily unavailable");
        let value: Value = serde_json::from_str(&output).expect("ToolSearch output is JSON");
        assert!(
            value.get("matches").is_some(),
            "normal search results remain present"
        );
        assert_eq!(
            value.get("pending_mcp_servers"),
            Some(&Value::Null),
            "pending MCP metadata is best-effort and omitted while the main lock is busy"
        );
        assert!(
            value.get("mcp_degraded").is_none(),
            "degraded MCP metadata is also skipped while the main lock is busy"
        );
    }
}
