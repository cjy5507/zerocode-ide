//! A pane child's road to its parent's MCP runtime (t-2513 §2.1).
//!
//! An inline child reaches the parent session's MCP tools through an
//! in-process passthrough: the schemas are advertised on its wire, and a call
//! is dispatched by the parent's runtime. A pane child is another process, so
//! the same road has a socket in the middle: the brief's harness carries the
//! schemas ([`McpRoute::tools`]) and the parent's channel file
//! ([`McpRoute::channel`]), the schemas are registered as this session's
//! runtime tools, and a call becomes one `mcp.call` on the parent's events
//! channel, answered by the very dispatch the inline passthrough uses.
//!
//! Without this the pane child would either not see the parent's MCP tools
//! (a different agent from the inline one — the drift contract 1 closes) or
//! see them and fail every call.

use std::path::PathBuf;
use std::time::Duration;

use runtime::subagent_panes::{ChannelCoordinates, Limits, McpRoute};

/// Where a pane child's MCP calls go.
#[derive(Debug, Clone)]
pub struct RemoteMcp {
    channel: PathBuf,
    method: String,
    timeout: Duration,
}

impl RemoteMcp {
    /// From the harness's route. `None` when the parent has no channel to
    /// answer on — then the schemas may still be advertised, and a call fails
    /// by name rather than hanging.
    #[must_use]
    pub fn from_route(route: &McpRoute, limits: &Limits) -> Option<Self> {
        let channel = route.channel.clone()?;
        Some(Self {
            channel,
            method: route.method.clone(),
            timeout: limits.channel_timeout,
        })
    }

    /// The route's tools as this session's runtime-tool registry takes them.
    #[must_use]
    pub fn definitions(route: &McpRoute) -> Vec<tools::RuntimeToolDefinition> {
        route
            .tools
            .iter()
            .map(|tool| tools::RuntimeToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
                required_permission: tool
                    .required_permission
                    .as_deref()
                    .and_then(runtime::PermissionMode::parse)
                    .unwrap_or(runtime::PermissionMode::Prompt),
            })
            .collect()
    }

    /// One tool call, answered by the parent. The parent's `is_error` answer
    /// is this call's error, the way the inline passthrough's `Err` is.
    ///
    /// # Errors
    ///
    /// The parent's channel did not answer, refused, or the tool itself failed.
    pub fn dispatch(
        &self,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<String, runtime::ToolError> {
        let coordinates = ChannelCoordinates::read(&self.channel).map_err(|why| {
            runtime::ToolError::new(format!(
                "the parent's channel file is gone, so MCP tool `{tool_name}` has nobody to answer it: {why}"
            ))
        })?;
        let mut params = serde_json::json!({ "name": tool_name });
        params["input"] = input;
        let answer = coordinates
            .call(&self.method, params, self.timeout)
            .map_err(|error| {
                runtime::ToolError::new(format!("MCP tool `{tool_name}` via the parent: {error}"))
            })?;
        let content = answer
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if answer
            .get("is_error")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return Err(runtime::ToolError::new(content));
        }
        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::subagent_panes::McpTool;

    fn a_route(channel: Option<PathBuf>) -> McpRoute {
        McpRoute {
            tools: vec![McpTool {
                name: "mcp__ctx7__query".to_string(),
                description: Some("docs".to_string()),
                input_schema: serde_json::json!({"type": "object"}),
                required_permission: Some("read-only".to_string()),
            }],
            channel,
            method: runtime::subagent_panes::channel_method::MCP_CALL.to_string(),
        }
    }

    #[test]
    fn a_route_without_a_parent_channel_has_no_remote_and_its_tools_still_read() {
        assert!(RemoteMcp::from_route(&a_route(None), &Limits::default()).is_none());
        let definitions = RemoteMcp::definitions(&a_route(None));
        assert_eq!(definitions[0].name, "mcp__ctx7__query");
        assert_eq!(definitions[0].required_permission, runtime::PermissionMode::ReadOnly);
    }

    /// The call is one `mcp.call` line on the parent's channel; the parent's
    /// `is_error` is this side's `Err`.
    #[test]
    fn a_call_rides_the_parents_channel_and_reads_its_answer() {
        use std::io::{BufRead as _, Write as _};
        let directory = tempfile::tempdir().expect("tempdir");
        let channel = directory.path().join("zo-events-parent.addr");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        ChannelCoordinates {
            addr: listener.local_addr().unwrap().to_string(),
            token: Some("parent-token".to_string()),
            session_id: "parent".to_string(),
        }
        .write(&channel)
        .expect("write channel file");
        let served = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..2 {
                let Ok((stream, _)) = listener.accept() else { break };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                let request: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                seen.push(request.clone());
                let is_error = request["params"]["name"] != "mcp__ctx7__query";
                let content = if is_error { "unknown MCP tool" } else { "docs for tokio" };
                let mut writer = stream;
                let _ = writeln!(
                    writer,
                    "{}",
                    serde_json::json!({"jsonrpc": "2.0", "id": request["id"],
                        "result": {"name": request["params"]["name"], "content": content, "is_error": is_error}})
                );
            }
            seen
        });
        let remote = RemoteMcp::from_route(&a_route(Some(channel.clone())), &Limits::default()).expect("remote");
        let answer = remote
            .dispatch("mcp__ctx7__query", serde_json::json!({"q": "tokio"}))
            .expect("the parent answered");
        assert_eq!(answer, "docs for tokio");
        let failed = remote
            .dispatch("mcp__nope", serde_json::json!({}))
            .expect_err("the parent's tool error is this side's error");
        assert!(failed.to_string().contains("unknown MCP tool"));
        let seen = served.join().unwrap();
        assert_eq!(seen[0]["method"], "mcp.call");
        assert_eq!(seen[0]["params"]["name"], "mcp__ctx7__query");
        assert_eq!(seen[0]["params"]["input"]["q"], "tokio");
        assert_eq!(seen[0]["token"], "parent-token");
        // Nobody home: an error that names the tool, not a hang.
        std::fs::remove_file(&channel).unwrap();
        let gone = remote.dispatch("mcp__ctx7__query", serde_json::json!({})).expect_err("gone");
        assert!(gone.to_string().contains("mcp__ctx7__query"));
    }
}
