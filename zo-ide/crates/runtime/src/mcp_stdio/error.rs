//! Error surface for the MCP stdio server manager.
//!
//! [`McpServerManagerError`] is the single thiserror-free enum returned
//! by every fallible operation in this module. It carries enough context
//! (server name, RPC method, optional source) to produce both a
//! human-readable [`Display`] message and a structured
//! [`McpDiscoveryFailure`] that the lifecycle reporter can surface in
//! the TUI.

use std::collections::BTreeMap;
use std::io;

use crate::mcp_lifecycle_hardened::McpLifecyclePhase;

use super::McpDiscoveryFailure;
use super::types::JsonRpcError;

#[derive(Debug)]
pub enum McpServerManagerError {
    Io(io::Error),
    Transport {
        server_name: String,
        method: &'static str,
        source: io::Error,
    },
    JsonRpc {
        server_name: String,
        method: &'static str,
        error: Box<JsonRpcError>,
    },
    InvalidResponse {
        server_name: String,
        method: &'static str,
        details: String,
    },
    Timeout {
        server_name: String,
        method: &'static str,
        timeout_ms: u64,
    },
    DuplicateToolRoute {
        qualified_name: String,
        existing_server: String,
        existing_raw_name: String,
        new_server: String,
        new_raw_name: String,
    },
    UnknownTool {
        qualified_name: String,
    },
    UnknownServer {
        server_name: String,
    },
}

impl std::fmt::Display for McpServerManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Transport {
                server_name,
                method,
                source,
            } => write!(
                f,
                "MCP server `{server_name}` transport failed during {method}: {source}"
            ),
            Self::JsonRpc {
                server_name,
                method,
                error,
            } => write!(
                f,
                "MCP server `{server_name}` returned JSON-RPC error for {method}: {} ({})",
                error.message, error.code
            ),
            Self::InvalidResponse {
                server_name,
                method,
                details,
            } => write!(
                f,
                "MCP server `{server_name}` returned invalid response for {method}: {details}"
            ),
            Self::Timeout {
                server_name,
                method,
                timeout_ms,
            } => write!(
                f,
                "MCP server `{server_name}` timed out after {timeout_ms} ms while handling {method}"
            ),
            Self::DuplicateToolRoute {
                qualified_name,
                existing_server,
                existing_raw_name,
                new_server,
                new_raw_name,
            } => write!(
                f,
                "duplicate MCP tool route `{qualified_name}` advertised by `{existing_server}`/`{existing_raw_name}` and `{new_server}`/`{new_raw_name}`"
            ),
            Self::UnknownTool { qualified_name } => {
                write!(f, "unknown MCP tool `{qualified_name}`")
            }
            Self::UnknownServer { server_name } => write!(f, "unknown MCP server `{server_name}`"),
        }
    }
}

impl std::error::Error for McpServerManagerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Transport { source, .. } => Some(source),
            Self::JsonRpc { .. }
            | Self::InvalidResponse { .. }
            | Self::Timeout { .. }
            | Self::DuplicateToolRoute { .. }
            | Self::UnknownTool { .. }
            | Self::UnknownServer { .. } => None,
        }
    }
}

impl From<io::Error> for McpServerManagerError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl McpServerManagerError {
    pub(super) fn lifecycle_phase(&self) -> McpLifecyclePhase {
        match self {
            Self::Io(_) => McpLifecyclePhase::SpawnConnect,
            Self::Transport { method, .. }
            | Self::JsonRpc { method, .. }
            | Self::InvalidResponse { method, .. }
            | Self::Timeout { method, .. } => lifecycle_phase_for_method(method),
            Self::DuplicateToolRoute { .. } | Self::UnknownTool { .. } => {
                McpLifecyclePhase::ToolDiscovery
            }
            Self::UnknownServer { .. } => McpLifecyclePhase::ServerRegistration,
        }
    }

    pub(super) fn recoverable(&self) -> bool {
        !matches!(
            self.lifecycle_phase(),
            McpLifecyclePhase::InitializeHandshake
        ) && matches!(self, Self::Transport { .. } | Self::Timeout { .. })
    }

    pub(super) fn discovery_failure(&self, server_name: &str) -> McpDiscoveryFailure {
        let phase = self.lifecycle_phase();
        let recoverable = self.recoverable();
        let context = self.error_context();

        McpDiscoveryFailure {
            server_name: server_name.to_string(),
            phase,
            error: self.to_string(),
            recoverable,
            context,
        }
    }

    fn error_context(&self) -> BTreeMap<String, String> {
        match self {
            Self::Io(error) => BTreeMap::from([("kind".to_string(), error.kind().to_string())]),
            Self::Transport {
                server_name,
                method,
                source,
            } => BTreeMap::from([
                ("server".to_string(), server_name.clone()),
                ("method".to_string(), (*method).to_string()),
                ("io_kind".to_string(), source.kind().to_string()),
            ]),
            Self::JsonRpc {
                server_name,
                method,
                error,
            } => BTreeMap::from([
                ("server".to_string(), server_name.clone()),
                ("method".to_string(), (*method).to_string()),
                ("jsonrpc_code".to_string(), error.code.to_string()),
            ]),
            Self::InvalidResponse {
                server_name,
                method,
                details,
            } => BTreeMap::from([
                ("server".to_string(), server_name.clone()),
                ("method".to_string(), (*method).to_string()),
                ("details".to_string(), details.clone()),
            ]),
            Self::Timeout {
                server_name,
                method,
                timeout_ms,
            } => BTreeMap::from([
                ("server".to_string(), server_name.clone()),
                ("method".to_string(), (*method).to_string()),
                ("timeout_ms".to_string(), timeout_ms.to_string()),
            ]),
            Self::DuplicateToolRoute {
                qualified_name,
                existing_server,
                existing_raw_name,
                new_server,
                new_raw_name,
            } => BTreeMap::from([
                ("qualified_tool".to_string(), qualified_name.clone()),
                ("existing_server".to_string(), existing_server.clone()),
                ("existing_raw_name".to_string(), existing_raw_name.clone()),
                ("new_server".to_string(), new_server.clone()),
                ("new_raw_name".to_string(), new_raw_name.clone()),
            ]),
            Self::UnknownTool { qualified_name } => {
                BTreeMap::from([("qualified_tool".to_string(), qualified_name.clone())])
            }
            Self::UnknownServer { server_name } => {
                BTreeMap::from([("server".to_string(), server_name.clone())])
            }
        }
    }
}

/// Map a JSON-RPC method name back to the lifecycle phase it belongs
/// to. Used by [`McpServerManagerError::lifecycle_phase`] so a
/// transport / timeout / invalid-response failure carries the right
/// stage label when surfaced.
pub(super) fn lifecycle_phase_for_method(method: &str) -> McpLifecyclePhase {
    match method {
        "initialize" => McpLifecyclePhase::InitializeHandshake,
        "tools/list" | "discover" => McpLifecyclePhase::ToolDiscovery,
        "resources/list" => McpLifecyclePhase::ResourceDiscovery,
        "resources/read" | "tools/call" => McpLifecyclePhase::Invocation,
        _ => McpLifecyclePhase::ErrorSurfacing,
    }
}
