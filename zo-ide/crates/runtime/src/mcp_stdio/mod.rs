use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::time::Duration;

use serde_json::Value as JsonValue;
use tokio::time::timeout;

use crate::config::{McpTransport, RuntimeConfig, ScopedMcpServerConfig};
use crate::mcp::mcp_tool_name;
use crate::mcp_client::{
    DEFAULT_MCP_TOOL_CALL_TIMEOUT_MS, McpClientBootstrap, McpClientTransport,
};
use crate::mcp_http::McpHttpProcess;
use crate::mcp_lifecycle_hardened::{
    McpDegradedReport, McpErrorSurface, McpFailedServer, McpLifecyclePhase,
};
use crate::mcp_sse::McpSseProcess;
use crate::mcp_ws::McpWsProcess;

#[cfg(test)]
const MCP_INITIALIZE_TIMEOUT_MS: u64 = 200;
// Production: 15s is enough for large OAuth-backed servers (Atlassian,
// Vercel, Firebase) that need a token refresh during `initialize`.
// The previous 3s value was set for fast local stdio servers and broke
// network-backed ones — see the L8 issue thread on commit c9c1825.
#[cfg(not(test))]
const MCP_INITIALIZE_TIMEOUT_MS: u64 = 15_000;
#[cfg(test)]
const MCP_REMOTE_BRIDGE_INITIALIZE_TIMEOUT_MS: u64 = 1_000;
#[cfg(not(test))]
const MCP_REMOTE_BRIDGE_INITIALIZE_TIMEOUT_MS: u64 = 45_000;

#[cfg(test)]
const MCP_LIST_TOOLS_TIMEOUT_MS: u64 = 300;
// Production: 30s matches the upstream `claude-code` default. Servers
// with 100+ tools (Playwright, Atlassian, Chrome DevTools MCP) routinely
// take 8–15s to assemble their tool list, so the prior 5s value timed
// out before any tools could surface.
#[cfg(not(test))]
const MCP_LIST_TOOLS_TIMEOUT_MS: u64 = 30_000;

/// Hard ceiling on pages fetched for one paginated listing (`tools/list`,
/// `resources/list`, …). A broken or hostile server can hand back a non-null
/// `nextCursor` forever, which would loop unboundedly — growing memory and
/// firing requests without end (the per-request timeout bounds each call, not
/// the count). Far above any real server's page count, so it only ever trips on
/// a misbehaving one, which is then surfaced as a degraded response.
const MAX_PAGINATION_PAGES: usize = 1_000;

#[cfg(test)]
const MCP_DISCOVERY_MARGIN_MS: u64 = 100;
#[cfg(not(test))]
const MCP_DISCOVERY_MARGIN_MS: u64 = 10_000;

#[cfg(test)]
const MCP_DISCOVER_TOTAL_TIMEOUT_MS: u64 = 3_000;
// Production: overall fairness budget for eager multi-server discovery. The
// default covers one full remote/OAuth discovery window (initialize timeout,
// one timeout-induced reset retry, tools/list, and margin). Discovery probes
// shorter-initialize servers first, so local/global stdio tools are not starved
// by a slow remote bridge while a single remote server still gets a complete
// initialize/retry/list opportunity.
#[cfg(not(test))]
const MCP_DISCOVER_TOTAL_TIMEOUT_MS: u64 = MCP_REMOTE_BRIDGE_INITIALIZE_TIMEOUT_MS * 2
    + MCP_LIST_TOOLS_TIMEOUT_MS
    + MCP_DISCOVERY_MARGIN_MS;

/// Maximum consecutive *timeout-induced* process respawns for one server before
/// the manager stops killing a still-alive child and instead lets it keep
/// running. A genuinely hung server recovers in a single respawn (see the
/// `initialize_hang` reconnect test); an unbounded respawn loop only happens
/// when a slow first-time handshake never completes within the timeout. The
/// counter clears on the next successful `initialize`, so a later genuine
/// disconnect still gets a full respawn budget. Transport and invalid-response
/// failures are never bounded — those mean the connection is already dead or
/// desynced and must be reset.
///
/// A *known* interactive OAuth bridge (`npx mcp-remote` for Atlassian, Vercel,
/// …) never consumes this budget: its child opens a browser tab per spawn, so
/// even the first kill-and-respawn orphans the tab the user was authenticating
/// in. [`McpServerManager::reset_server_for_error`] keeps a live bridge child
/// from the very first timeout.
const MAX_CONSECUTIVE_TIMEOUT_RESETS: u32 = 2;

mod error;
mod process;
mod types;

pub use error::McpServerManagerError;
/// Shared with the network transports (SSE/HTTP/WS) so they surface the same
/// inbound notifications (`tools/list_changed`) the stdio read loop does.
pub(crate) use process::inbound_event_for_notification;
pub use process::{McpStdioProcess, spawn_mcp_stdio_process};
pub use types::{
    InboundEvent, JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse, McpGetPromptParams,
    McpGetPromptResult, McpInitializeClientInfo, McpInitializeParams, McpInitializeResult,
    McpInitializeServerInfo, McpListPromptsParams, McpListPromptsResult, McpListResourcesParams,
    McpListResourcesResult, McpListToolsParams, McpListToolsResult, McpPrompt, McpPromptArgument,
    McpPromptMessage, McpReadResourceParams, McpReadResourceResult, McpResource,
    McpResourceContents, McpTool, McpToolCallContent, McpToolCallParams, McpToolCallResult,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ManagedMcpTool {
    pub server_name: String,
    pub qualified_name: String,
    pub raw_name: String,
    pub tool: McpTool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedMcpServer {
    pub server_name: String,
    pub transport: McpTransport,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDiscoveryFailure {
    pub server_name: String,
    pub phase: McpLifecyclePhase,
    pub error: String,
    pub recoverable: bool,
    pub context: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpToolDiscoveryReport {
    pub tools: Vec<ManagedMcpTool>,
    pub failed_servers: Vec<McpDiscoveryFailure>,
    pub unsupported_servers: Vec<UnsupportedMcpServer>,
    pub degraded_startup: Option<McpDegradedReport>,
}

/// Scheduling class for background discovery concurrency, mirroring Claude
/// Code's split between local stdio servers (a spawned child process — heavier,
/// `MCP_SERVER_CONNECTION_BATCH_SIZE` default 3) and remote network servers
/// (HTTP/SSE/WS — lighter, `MCP_REMOTE_SERVER_CONNECTION_BATCH_SIZE` default
/// 20). The scheduler applies a per-class concurrency cap so one slow class
/// cannot starve the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpDiscoveryClass {
    /// A local stdio child process (`uvx`, `node`, or an `npx mcp-remote`
    /// bridge): launched per server, so it is capped tighter.
    Stdio,
    /// A remote network transport (SSE/HTTP/WS): no local child, so a wider
    /// concurrency cap is safe.
    Remote,
}

fn duration_millis_u64(duration: std::time::Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn unsupported_server_failed_server(server: &UnsupportedMcpServer) -> McpFailedServer {
    McpFailedServer {
        server_name: server.server_name.clone(),
        phase: McpLifecyclePhase::ServerRegistration,
        error: McpErrorSurface::new(
            McpLifecyclePhase::ServerRegistration,
            Some(server.server_name.clone()),
            server.reason.clone(),
            BTreeMap::from([("transport".to_string(), format!("{:?}", server.transport))]),
            false,
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolRoute {
    server_name: String,
    raw_name: String,
}

#[derive(Debug)]
struct ManagedMcpServer {
    bootstrap: McpClientBootstrap,
    process: Option<ManagedMcpProcess>,
    initialized: bool,
    /// Consecutive timeout-induced respawns since the last successful
    /// `initialize` (see [`MAX_CONSECUTIVE_TIMEOUT_RESETS`]). Bounds the OAuth
    /// respawn loop without affecting genuine dead-process reconnects.
    consecutive_timeout_resets: u32,
    /// On the first `initialize_server_once` that spawns an interactive OAuth
    /// bridge (`mcp-remote` for Atlassian, etc.), this is set to `true` and
    /// prevents any later re-spawn while the first browser tab is still
    /// waiting for user authentication. Once `initialize` succeeds the flag is
    /// cleared — the bridge is now authenticated and `mcp-remote` has cached
    /// tokens, so a later crash can recover with a normal re-spawn that does
    /// NOT open another browser tab.
    interactive_oauth_bridge_spawned: bool,
}

impl ManagedMcpServer {
    fn new(bootstrap: McpClientBootstrap) -> Self {
        Self {
            bootstrap,
            process: None,
            initialized: false,
            consecutive_timeout_resets: 0,
            interactive_oauth_bridge_spawned: false,
        }
    }
}

#[derive(Debug)]
enum ManagedMcpProcess {
    Stdio(McpStdioProcess),
    Sse(McpSseProcess),
    Http(McpHttpProcess),
    Ws(Box<McpWsProcess>),
}

/// Forward one `.await`ed method call to whichever transport variant is
/// live. A local macro instead of the `enum_dispatch` crate: identical
/// expansion, zero added dependencies (supply-chain pinning policy).
/// Methods whose arms are not uniform across transports (`shutdown`,
/// `has_exited`, `poll_inbound`) keep hand-written matches below.
macro_rules! delegate_to_transport {
    ($self:ident, $method:ident($($arg:expr),*)) => {
        match $self {
            Self::Stdio(process) => process.$method($($arg),*).await,
            Self::Sse(process) => process.$method($($arg),*).await,
            Self::Http(process) => process.$method($($arg),*).await,
            Self::Ws(process) => process.$method($($arg),*).await,
        }
    };
}

impl ManagedMcpProcess {
    async fn initialize(
        &mut self,
        id: JsonRpcId,
        params: McpInitializeParams,
    ) -> io::Result<JsonRpcResponse<McpInitializeResult>> {
        delegate_to_transport!(self, initialize(id, params))
    }

    async fn list_tools(
        &mut self,
        id: JsonRpcId,
        params: Option<McpListToolsParams>,
    ) -> io::Result<JsonRpcResponse<McpListToolsResult>> {
        delegate_to_transport!(self, list_tools(id, params))
    }

    async fn call_tool(
        &mut self,
        id: JsonRpcId,
        params: McpToolCallParams,
    ) -> io::Result<JsonRpcResponse<McpToolCallResult>> {
        delegate_to_transport!(self, call_tool(id, params))
    }

    async fn list_resources(
        &mut self,
        id: JsonRpcId,
        params: Option<McpListResourcesParams>,
    ) -> io::Result<JsonRpcResponse<McpListResourcesResult>> {
        delegate_to_transport!(self, list_resources(id, params))
    }

    async fn read_resource(
        &mut self,
        id: JsonRpcId,
        params: McpReadResourceParams,
    ) -> io::Result<JsonRpcResponse<McpReadResourceResult>> {
        delegate_to_transport!(self, read_resource(id, params))
    }

    /// `prompts/list` — routed through each transport's generic `request`
    /// (no per-transport typed wrapper needed for a two-method surface).
    async fn list_prompts(
        &mut self,
        id: JsonRpcId,
        params: Option<McpListPromptsParams>,
    ) -> io::Result<JsonRpcResponse<McpListPromptsResult>> {
        delegate_to_transport!(self, request(id, "prompts/list", params))
    }

    /// `prompts/get` — resolve one prompt (server substitutes arguments).
    async fn get_prompt(
        &mut self,
        id: JsonRpcId,
        params: McpGetPromptParams,
    ) -> io::Result<JsonRpcResponse<McpGetPromptResult>> {
        delegate_to_transport!(self, request(id, "prompts/get", Some(params)))
    }

    /// Send a JSON-RPC notification (no `id` field, no response expected).
    async fn send_notification(&mut self, method: &str) -> io::Result<()> {
        delegate_to_transport!(self, send_notification(method))
    }

    async fn shutdown(&mut self) -> io::Result<()> {
        // Not delegated: the SSE arm's shutdown is synchronous.
        match self {
            Self::Stdio(process) => process.shutdown().await,
            Self::Sse(process) => process.shutdown(),
            Self::Http(process) => process.shutdown().await,
            Self::Ws(process) => process.shutdown().await,
        }
    }

    fn has_exited(&mut self) -> io::Result<bool> {
        match self {
            Self::Stdio(process) => process.has_exited(),
            Self::Sse(_) | Self::Http(_) | Self::Ws(_) => Ok(false),
        }
    }

    /// Drain inbound (server→client) events captured while reading responses.
    /// Only the stdio transport buffers them today; the network transports have
    /// no inbound-notification reader, so they report nothing.
    fn poll_inbound(&mut self) -> Vec<InboundEvent> {
        match self {
            Self::Stdio(process) => process.poll_inbound(),
            Self::Sse(process) => process.poll_inbound(),
            Self::Http(process) => process.poll_inbound(),
            Self::Ws(process) => process.poll_inbound(),
        }
    }
}

/// Retry-once-with-reset skeleton shared by every manager request wrapper.
///
/// Evaluates to `Result<T, McpServerManagerError>`: a first retryable
/// failure (transport/timeout — see [`McpServerManager::is_retryable_error`])
/// resets the server process and retries the call exactly once; any other
/// failure resets the server when the error shape warrants it
/// ([`McpServerManager::should_reset_server`]) and is yielded as `Err`.
/// A macro rather than a helper fn because the retried expression must
/// reborrow `&mut self` on every attempt, which a closure argument cannot
/// express under the current borrow checker.
macro_rules! with_reset_retry {
    ($self:ident, $server:expr, $call:expr) => {{
        let mut attempts = 0;
        loop {
            match $call {
                Ok(value) => break Ok(value),
                Err(error) if attempts == 0 && Self::is_retryable_error(&error) => {
                    $self.reset_server_for_error($server, &error).await?;
                    attempts += 1;
                }
                Err(error) => {
                    if Self::should_reset_server(&error) {
                        $self.reset_server_for_error($server, &error).await?;
                    }
                    break Err(error);
                }
            }
        }
    }};
}

#[derive(Debug)]
pub struct McpServerManager {
    servers: BTreeMap<String, ManagedMcpServer>,
    unsupported_servers: Vec<UnsupportedMcpServer>,
    tool_index: BTreeMap<String, ToolRoute>,
    discovered_tools_cache: Option<Vec<ManagedMcpTool>>,
    next_request_id: u64,
    /// Optional per-server discovery time budget override for tests.
    discover_server_timeout_override_ms: Option<u64>,
    /// Soft overall discovery scheduling budget (see [`MCP_DISCOVER_TOTAL_TIMEOUT_MS`]).
    discover_total_timeout_ms: u64,
}

impl McpServerManager {
    #[must_use]
    pub fn from_runtime_config(config: &RuntimeConfig) -> Self {
        Self::from_servers(config.mcp().servers())
    }

    #[must_use]
    pub fn from_servers(servers: &BTreeMap<String, ScopedMcpServerConfig>) -> Self {
        let mut managed_servers = BTreeMap::new();
        let mut unsupported_servers = Vec::new();

        for (server_name, server_config) in servers {
            match server_config.transport() {
                McpTransport::Stdio | McpTransport::Sse | McpTransport::Http | McpTransport::Ws => {
                    let bootstrap =
                        McpClientBootstrap::from_scoped_config(server_name, server_config);
                    managed_servers.insert(server_name.clone(), ManagedMcpServer::new(bootstrap));
                }
                _ => {
                    unsupported_servers.push(UnsupportedMcpServer {
                        server_name: server_name.clone(),
                        transport: server_config.transport(),
                        reason: format!(
                            "transport {:?} is not supported by McpServerManager",
                            server_config.transport()
                        ),
                    });
                }
            }
        }

        Self {
            servers: managed_servers,
            unsupported_servers,
            tool_index: BTreeMap::new(),
            discovered_tools_cache: None,
            next_request_id: 1,
            discover_server_timeout_override_ms: None,
            discover_total_timeout_ms: MCP_DISCOVER_TOTAL_TIMEOUT_MS,
        }
    }

    /// Register one more server after construction (the `/ide` bridge connects
    /// a discovered IDE extension mid-session). Replaces any prior entry under
    /// the same name and invalidates the discovery cache so the next refresh
    /// re-lists. Returns `false` for transports the manager cannot drive.
    pub fn add_server(&mut self, server_name: &str, server_config: &ScopedMcpServerConfig) -> bool {
        match server_config.transport() {
            McpTransport::Stdio | McpTransport::Sse | McpTransport::Http | McpTransport::Ws => {
                let bootstrap = McpClientBootstrap::from_scoped_config(server_name, server_config);
                self.servers
                    .insert(server_name.to_string(), ManagedMcpServer::new(bootstrap));
                // Re-registering a name replaces its process; drop any stale
                // routes for it (not just the discovery cache) so `call_tool`
                // can't reach a tool the new config no longer advertises until
                // the next discovery rebuilds the index.
                self.clear_routes_for_server(server_name);
                true
            }
            _ => false,
        }
    }

    /// Override the per-server discovery timeout (tests inject a short budget).
    #[cfg(test)]
    fn set_discover_server_timeout_ms(&mut self, timeout_ms: u64) {
        self.discover_server_timeout_override_ms = Some(timeout_ms);
    }

    /// Override the overall discovery budget (tests inject a short budget).
    #[cfg(test)]
    fn set_discover_total_timeout_ms(&mut self, timeout_ms: u64) {
        self.discover_total_timeout_ms = timeout_ms;
    }

    #[must_use]
    pub fn unsupported_servers(&self) -> &[UnsupportedMcpServer] {
        &self.unsupported_servers
    }

    #[must_use]
    pub fn server_names(&self) -> Vec<String> {
        self.servers.keys().cloned().collect()
    }

    /// The per-server `initialize` timeout (ms) used as the fast-first ordering
    /// key for background discovery. Mirrors the eager startup path, which
    /// already probes shorter-initialize servers first (see
    /// [`Self::discover_tools_best_effort`]) so a slow OAuth/remote bridge does
    /// not starve fast local stdio tools. Unknown servers sort last (`u64::MAX`).
    #[must_use]
    pub fn initialize_timeout_ms_for(&self, server_name: &str) -> u64 {
        self.initialize_timeout_ms(server_name).unwrap_or(u64::MAX)
    }

    /// The concurrency [`McpDiscoveryClass`] for a server, used by the
    /// background scheduler to apply a per-class cap (stdio tighter than
    /// remote). An `npx mcp-remote` bridge is still a local stdio child, so it
    /// is classed [`McpDiscoveryClass::Stdio`]; only true network transports
    /// (SSE/HTTP/WS) are [`McpDiscoveryClass::Remote`]. Unknown servers default
    /// to the tighter `Stdio` class.
    #[must_use]
    pub fn discovery_class_for(&self, server_name: &str) -> McpDiscoveryClass {
        match self.servers.get(server_name).map(|server| &server.bootstrap.transport) {
            Some(
                McpClientTransport::Sse(_)
                | McpClientTransport::Http(_)
                | McpClientTransport::WebSocket(_),
            ) => McpDiscoveryClass::Remote,
            _ => McpDiscoveryClass::Stdio,
        }
    }

    /// Detach one server into a standalone single-server [`McpServerManager`] so
    /// its discovery can run on an independently-owned manager — the unit of the
    /// bounded-concurrent background path, which holds **no** `RuntimeMcpState`
    /// lock during the handshake. The detached manager inherits this manager's
    /// test timeout overrides so injected budgets still apply. Returns `None`
    /// for an unknown name. The server's routes are left in place until
    /// [`Self::absorb_discovered`] re-splices them, so a concurrent reader still
    /// sees the previously-advertised set during the handshake.
    ///
    /// Detaching transfers ownership of the live child process (if any), giving
    /// each concurrent discovery a disjoint `&mut` — the borrow-checker-clean
    /// alternative to sharing one `&mut self` across N in-flight handshakes.
    #[must_use]
    pub fn detach_for_discovery(&mut self, server_name: &str) -> Option<Self> {
        let server = self.servers.remove(server_name)?;
        let mut detached = BTreeMap::new();
        detached.insert(server_name.to_string(), server);
        Some(Self {
            servers: detached,
            unsupported_servers: Vec::new(),
            tool_index: BTreeMap::new(),
            discovered_tools_cache: None,
            next_request_id: 1,
            discover_server_timeout_override_ms: self.discover_server_timeout_override_ms,
            discover_total_timeout_ms: self.discover_total_timeout_ms,
        })
    }

    /// Re-absorb a server detached by [`Self::detach_for_discovery`] after its
    /// concurrent discovery finished, splicing its freshly-discovered tools into
    /// the routing index. Mirrors the commit half of
    /// [`Self::refresh_server_tools`] so a concurrently-discovered server lands
    /// in exactly the same routing state as the serial path: the server's stale
    /// routes are dropped first, then the fresh routes are inserted atomically
    /// (a duplicate-name clash surfaces as
    /// [`McpServerManagerError::DuplicateToolRoute`] and aborts the splice). The
    /// live connection is always re-attached, even on a clash, so it is never
    /// leaked.
    pub fn absorb_discovered(
        &mut self,
        server_name: &str,
        mut detached: Self,
        fresh: &[ManagedMcpTool],
    ) -> Result<(), McpServerManagerError> {
        if let Some(server) = detached.servers.remove(server_name) {
            self.servers.insert(server_name.to_string(), server);
        }

        let mut next_index = self.tool_index.clone();
        next_index.retain(|_, route| route.server_name != server_name);
        Self::insert_tool_routes(&mut next_index, fresh)?;
        self.tool_index = next_index;

        if let Some(cache) = self.discovered_tools_cache.as_mut() {
            cache.retain(|tool| tool.server_name != server_name);
            cache.extend(fresh.iter().cloned());
        }
        Ok(())
    }

    /// Re-attach a server detached by [`Self::detach_for_discovery`] *without*
    /// touching the routing index — the failure/panic-safe counterpart of
    /// [`Self::absorb_discovered`]. Used when a server's concurrent discovery
    /// errored: the live entry (and its OAuth respawn budget) returns to the
    /// manager so a later turn can retry it, while its previously-advertised
    /// routes (usually none at startup) are left exactly as they were — matching
    /// the serial path, where a `refresh_server_tools` failure leaves routes
    /// untouched.
    pub fn reattach_detached(&mut self, server_name: &str, mut detached: Self) {
        if let Some(server) = detached.servers.remove(server_name) {
            self.servers.insert(server_name.to_string(), server);
        }
    }

    pub async fn discover_tools(&mut self) -> Result<Vec<ManagedMcpTool>, McpServerManagerError> {
        if let Some(cached_tools) = self.discovered_tools_cache.clone() {
            self.rebuild_tool_index(&cached_tools)?;
            return Ok(cached_tools);
        }

        let server_names = self.server_names();
        let mut discovered_tools = Vec::new();
        let mut next_index = BTreeMap::new();

        for server_name in server_names {
            let server_tools = self.discover_tools_for_server(&server_name).await?;
            Self::insert_tool_routes(&mut next_index, &server_tools)?;
            discovered_tools.extend(server_tools);
        }

        self.tool_index = next_index;
        self.discovered_tools_cache = Some(discovered_tools.clone());
        Ok(discovered_tools)
    }

    pub async fn discover_tools_best_effort(&mut self) -> McpToolDiscoveryReport {
        if let Some(cached_tools) = self.discovered_tools_cache.clone() {
            if self.rebuild_tool_index(&cached_tools).is_err() {
                self.discovered_tools_cache = None;
            } else {
                return McpToolDiscoveryReport {
                    tools: cached_tools,
                    failed_servers: Vec::new(),
                    unsupported_servers: self.unsupported_servers.clone(),
                    degraded_startup: None,
                };
            }
        }

        let mut server_names = self.server_names();
        server_names.sort_by_key(|server_name| {
            self.initialize_timeout_ms(server_name).unwrap_or(u64::MAX)
        });
        let mut discovered_tools = Vec::new();
        let mut working_servers = Vec::new();
        let mut failed_servers = Vec::new();
        let mut next_index = BTreeMap::new();

        let total_budget = std::time::Duration::from_millis(self.discover_total_timeout_ms);
        let discovery_start = std::time::Instant::now();
        for server_name in server_names {
            // The total budget bounds eager startup fairness. If there is not
            // enough remaining time to let a server's initialize request reach
            // its own timeout, do not start that server: starting it would mask
            // the initialize failure as an outer discovery timeout and starve
            // later healthy servers. Otherwise cap this server by the remaining
            // total budget so one slow probe cannot monopolize discovery.
            if discovery_start.elapsed() >= total_budget {
                self.clear_routes_for_server(&server_name);
                failed_servers.push(
                    McpServerManagerError::Timeout {
                        server_name: server_name.clone(),
                        method: "discover",
                        timeout_ms: self.discover_total_timeout_ms,
                    }
                    .discovery_failure(&server_name),
                );
                continue;
            }
            let timeout_ms = match self.discovery_timeout_ms(&server_name) {
                Ok(timeout_ms) => timeout_ms,
                Err(error) => {
                    failed_servers.push(error.discovery_failure(&server_name));
                    continue;
                }
            };
            let initialize_timeout_ms = match self.initialize_timeout_ms(&server_name) {
                Ok(timeout_ms) => timeout_ms,
                Err(error) => {
                    failed_servers.push(error.discovery_failure(&server_name));
                    continue;
                }
            };
            let remaining_ms = duration_millis_u64(total_budget.saturating_sub(discovery_start.elapsed()));
            if self.requires_full_initialize_budget(&server_name, initialize_timeout_ms)
                && remaining_ms < initialize_timeout_ms
            {
                self.clear_routes_for_server(&server_name);
                failed_servers.push(
                    self.initialize_budget_error(&server_name, remaining_ms, initialize_timeout_ms)
                        .discovery_failure(&server_name),
                );
                continue;
            }
            let effective_timeout_ms = timeout_ms.min(remaining_ms);
            let outcome = tokio::time::timeout(
                std::time::Duration::from_millis(effective_timeout_ms),
                self.discover_tools_for_server(&server_name),
            )
            .await;
            match outcome {
                Ok(Ok(server_tools)) => {
                    let mut candidate_index = next_index.clone();
                    match Self::insert_tool_routes(&mut candidate_index, &server_tools) {
                        Ok(()) => {
                            next_index = candidate_index;
                            working_servers.push(server_name.clone());
                            discovered_tools.extend(server_tools);
                        }
                        Err(error) => {
                            failed_servers.push(error.discovery_failure(&server_name));
                        }
                    }
                }
                Ok(Err(error)) => {
                    self.clear_routes_for_server(&server_name);
                    failed_servers.push(error.discovery_failure(&server_name));
                }
                Err(_elapsed) => {
                    let method = self.discovery_timeout_method(&server_name);
                    self.clear_routes_for_server(&server_name);
                    let error = McpServerManagerError::Timeout {
                        server_name: server_name.clone(),
                        method,
                        timeout_ms: effective_timeout_ms,
                    };
                    failed_servers.push(error.discovery_failure(&server_name));
                }
            }
        }

        self.tool_index = next_index;
        self.assemble_discovery_report(discovered_tools, working_servers, failed_servers)
    }

    /// Assemble the final [`McpToolDiscoveryReport`] from a discovery pass and
    /// cache the tools when every server succeeded. Split out of
    /// [`Self::discover_tools_best_effort`] to keep that method within the
    /// clippy line budget.
    fn assemble_discovery_report(
        &mut self,
        discovered_tools: Vec<ManagedMcpTool>,
        working_servers: Vec<String>,
        failed_servers: Vec<McpDiscoveryFailure>,
    ) -> McpToolDiscoveryReport {
        let degraded_failed_servers = failed_servers
            .iter()
            .map(|failure| McpFailedServer {
                server_name: failure.server_name.clone(),
                phase: failure.phase,
                error: McpErrorSurface::new(
                    failure.phase,
                    Some(failure.server_name.clone()),
                    failure.error.clone(),
                    failure.context.clone(),
                    failure.recoverable,
                ),
            })
            .chain(
                self.unsupported_servers
                    .iter()
                    .map(unsupported_server_failed_server),
            )
            .collect::<Vec<_>>();
        let degraded_startup = (!working_servers.is_empty() && !degraded_failed_servers.is_empty())
            .then(|| {
                McpDegradedReport::new(
                    working_servers,
                    degraded_failed_servers,
                    discovered_tools
                        .iter()
                        .map(|tool| tool.qualified_name.clone())
                        .collect(),
                    Vec::new(),
                )
            });

        let report = McpToolDiscoveryReport {
            tools: discovered_tools,
            failed_servers,
            unsupported_servers: self.unsupported_servers.clone(),
            degraded_startup,
        };
        if report.failed_servers.is_empty() {
            self.discovered_tools_cache = Some(report.tools.clone());
        }
        report
    }

    pub async fn call_tool(
        &mut self,
        qualified_tool_name: &str,
        arguments: Option<JsonValue>,
    ) -> Result<JsonRpcResponse<McpToolCallResult>, McpServerManagerError> {
        let route = self
            .tool_index
            .get(qualified_tool_name)
            .cloned()
            .ok_or_else(|| McpServerManagerError::UnknownTool {
                qualified_name: qualified_tool_name.to_string(),
            })?;
        let server_name = route.server_name.clone();

        // Tool invocations can be non-idempotent, but transport/timeout errors
        // already mean we did not receive a usable JSON-RPC response. Match the
        // rest of the manager's bounded policy: reset and retry exactly once for
        // retryable failures, never loop indefinitely.
        with_reset_retry!(
            self,
            &server_name,
            self.call_tool_once(&route, arguments.clone()).await
        )
    }

    async fn call_tool_once(
        &mut self,
        route: &ToolRoute,
        arguments: Option<JsonValue>,
    ) -> Result<JsonRpcResponse<McpToolCallResult>, McpServerManagerError> {
        let timeout_ms = self.tool_call_timeout_ms(&route.server_name)?;

        self.ensure_server_ready(&route.server_name).await?;
        let request_id = self.take_request_id();
        let server = self.server_mut(&route.server_name)?;
        let process =
            server
                .process
                .as_mut()
                .ok_or_else(|| McpServerManagerError::InvalidResponse {
                    server_name: route.server_name.clone(),
                    method: "tools/call",
                    details: "server process missing after initialization".to_string(),
                })?;
        Self::run_process_request(
            &route.server_name,
            "tools/call",
            timeout_ms,
            process.call_tool(
                request_id,
                McpToolCallParams {
                    name: route.raw_name.clone(),
                    arguments,
                    meta: None,
                },
            ),
        )
        .await
    }

    pub async fn list_resources(
        &mut self,
        server_name: &str,
    ) -> Result<McpListResourcesResult, McpServerManagerError> {
        with_reset_retry!(
            self,
            server_name,
            self.list_resources_once(server_name).await
        )
    }

    pub async fn read_resource(
        &mut self,
        server_name: &str,
        uri: &str,
    ) -> Result<McpReadResourceResult, McpServerManagerError> {
        with_reset_retry!(
            self,
            server_name,
            self.read_resource_once(server_name, uri).await
        )
    }

    /// List a server's prompts (paginated). A server that does not implement
    /// the prompts capability answers `prompts/list` with JSON-RPC `-32601`
    /// (method not found); that is a *normal* shape, not a failure, so it maps
    /// to an empty list instead of an error. (`-32601` is a JSON-RPC error,
    /// which never triggers a reset, so mapping it after the retry skeleton
    /// is equivalent to the old inline check.)
    pub async fn list_prompts(
        &mut self,
        server_name: &str,
    ) -> Result<Vec<McpPrompt>, McpServerManagerError> {
        match with_reset_retry!(self, server_name, self.list_prompts_once(server_name).await) {
            Err(error) if Self::is_method_not_found(&error) => Ok(Vec::new()),
            outcome => outcome,
        }
    }

    /// Resolve one prompt by RAW name (server-side argument substitution).
    pub async fn get_prompt(
        &mut self,
        server_name: &str,
        prompt_name: &str,
        arguments: Option<JsonValue>,
    ) -> Result<McpGetPromptResult, McpServerManagerError> {
        with_reset_retry!(
            self,
            server_name,
            self.get_prompt_once(server_name, prompt_name, arguments.clone())
                .await
        )
    }

    pub async fn shutdown(&mut self) -> Result<(), McpServerManagerError> {
        let server_names = self.server_names();
        for server_name in server_names {
            let server = self.server_mut(&server_name)?;
            if let Some(process) = server.process.as_mut() {
                process.shutdown().await?;
            }
            server.process = None;
            server.initialized = false;
        }
        Ok(())
    }

    /// Drain buffered inbound events from every live server process, tagged
    /// with the server they came from. Sync: it only collects events the read
    /// loops already captured during prior requests — it does not poll stdout.
    pub fn poll_all_inbound(&mut self) -> Vec<(String, InboundEvent)> {
        let mut events = Vec::new();
        for (server_name, server) in &mut self.servers {
            if let Some(process) = server.process.as_mut() {
                for event in process.poll_inbound() {
                    events.push((server_name.clone(), event));
                }
            }
        }
        events
    }

    /// Qualified names currently routed to `server_name`, read from the routing
    /// index. The index is keyed by qualified name but valued by the server's
    /// RAW config name, so this is exact even when two distinct server names
    /// normalize to the same tool-name prefix — the consumer uses it to remove
    /// precisely one server's advertised tools on refresh.
    #[must_use]
    pub fn qualified_tool_names_for_server(&self, server_name: &str) -> Vec<String> {
        self.tool_index
            .iter()
            .filter(|(_, route)| route.server_name == server_name)
            .map(|(qualified, _)| qualified.clone())
            .collect()
    }

    /// Re-discover a single server's tools after a `tools/list_changed` and
    /// splice the result into the routing tables, leaving every *other* server
    /// untouched. Returns the server's fresh tool set.
    ///
    /// This deliberately avoids `Self::clear_routes_for_server`, which nukes
    /// the whole `discovered_tools_cache`: a blanket invalidation would force a
    /// full re-discovery of *all* servers on the next `discover_tools` call.
    /// The per-server sequence below keeps `tool_index` (the table `call_tool`
    /// routes through) and the cache consistent with only one server re-queried.
    pub async fn refresh_server_tools(
        &mut self,
        server_name: &str,
    ) -> Result<Vec<ManagedMcpTool>, McpServerManagerError> {
        // 1. Re-discover just this server (reuses the live process; a healthy
        //    long-lived server's `ensure_server_ready` is a no-op).
        //
        //    Bound the whole open -> initialize -> tools/list round-trip by the
        //    per-server budget. The LIVE discovery paths (background discovery
        //    thread, turn-boundary inbound refresh, on-demand resolve) all call
        //    this directly and would otherwise inherit only the inner per-RPC
        //    timeouts — so a process open that never yields a frame (npx
        //    cold-start, a stalled WS/TLS upgrade, an auth wait) could hang this
        //    server forever, pinning it to "discovering" with no terminal state.
        //    Forcing a Timeout here turns that into a terminal "failed". (The
        //    eager startup path bounds each server the same way in
        //    `discover_tools_best_effort`.)
        let timeout_ms = self.discovery_timeout_ms(server_name)?;
        let fresh = match tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            self.discover_tools_for_server(server_name),
        )
        .await
        {
            Ok(result) => result?,
            Err(_elapsed) => {
                return Err(McpServerManagerError::Timeout {
                    server_name: server_name.to_string(),
                    method: self.discovery_timeout_method(server_name),
                    timeout_ms,
                });
            }
        };

        // 2. Drop this server's stale routes in a candidate index, preserving
        //    every other server. Commit only after all fresh routes pass the
        //    duplicate-name check so a bad refresh cannot leave partial routes.
        let mut next_index = self.tool_index.clone();
        next_index.retain(|_, route| route.server_name != server_name);
        Self::insert_tool_routes(&mut next_index, &fresh)?;
        self.tool_index = next_index;

        // 3. Keep the discovered-tools cache in sync ONLY when it already
        //    exists. A `None` cache marks a degraded startup that must trigger a
        //    full re-discovery later; fabricating `Some` here would mask that.
        if let Some(cache) = self.discovered_tools_cache.as_mut() {
            cache.retain(|tool| tool.server_name != server_name);
            cache.extend(fresh.iter().cloned());
        }

        Ok(fresh)
    }

    fn clear_routes_for_server(&mut self, server_name: &str) {
        self.discovered_tools_cache = None;
        self.tool_index
            .retain(|_, route| route.server_name != server_name);
    }

    fn rebuild_tool_index(
        &mut self,
        tools: &[ManagedMcpTool],
    ) -> Result<(), McpServerManagerError> {
        let mut rebuilt = BTreeMap::new();
        Self::insert_tool_routes(&mut rebuilt, tools)?;
        self.tool_index = rebuilt;
        Ok(())
    }

    fn insert_tool_routes(
        index: &mut BTreeMap<String, ToolRoute>,
        tools: &[ManagedMcpTool],
    ) -> Result<(), McpServerManagerError> {
        for tool in tools {
            Self::insert_tool_route(index, tool)?;
        }
        Ok(())
    }

    fn insert_tool_route(
        index: &mut BTreeMap<String, ToolRoute>,
        tool: &ManagedMcpTool,
    ) -> Result<(), McpServerManagerError> {
        let next = ToolRoute {
            server_name: tool.server_name.clone(),
            raw_name: tool.raw_name.clone(),
        };
        if let Some(existing) = index.get(&tool.qualified_name) {
            if existing != &next {
                return Err(McpServerManagerError::DuplicateToolRoute {
                    qualified_name: tool.qualified_name.clone(),
                    existing_server: existing.server_name.clone(),
                    existing_raw_name: existing.raw_name.clone(),
                    new_server: next.server_name,
                    new_raw_name: next.raw_name,
                });
            }
        }
        index.insert(tool.qualified_name.clone(), next);
        Ok(())
    }

    fn server_mut(
        &mut self,
        server_name: &str,
    ) -> Result<&mut ManagedMcpServer, McpServerManagerError> {
        self.servers
            .get_mut(server_name)
            .ok_or_else(|| McpServerManagerError::UnknownServer {
                server_name: server_name.to_string(),
            })
    }

    fn take_request_id(&mut self) -> JsonRpcId {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        JsonRpcId::Number(id)
    }

    fn initialize_budget_error(
        &self,
        server_name: &str,
        remaining_ms: u64,
        initialize_timeout_ms: u64,
    ) -> McpServerManagerError {
        let target = self
            .server_transport_target(server_name)
            .unwrap_or_else(|| server_name.to_string());
        McpServerManagerError::Transport {
            server_name: server_name.to_string(),
            method: "initialize",
            source: io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "discovery budget has {remaining_ms} ms remaining, below the {initialize_timeout_ms} ms initialize timeout for {target}"
                ),
            ),
        }
    }

    fn server_transport_target(&self, server_name: &str) -> Option<String> {
        let server = self.servers.get(server_name)?;
        Some(match &server.bootstrap.transport {
            McpClientTransport::Stdio(transport) => std::iter::once(transport.command.as_str())
                .chain(transport.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
            McpClientTransport::Sse(transport)
            | McpClientTransport::Http(transport)
            | McpClientTransport::WebSocket(transport) => transport.url.clone(),
            McpClientTransport::Sdk(transport) => transport.name.clone(),
            McpClientTransport::ManagedProxy(transport) => transport.url.clone(),
        })
    }

    fn discovery_timeout_ms(&self, server_name: &str) -> Result<u64, McpServerManagerError> {
        if let Some(timeout_ms) = self.discover_server_timeout_override_ms {
            return Ok(timeout_ms);
        }
        // `ensure_server_ready` may run initialize twice (first failure + one
        // reset retry). The outer discovery guard must be wider than those
        // inner request timeouts; otherwise it masks the true initialize error
        // as a generic discovery/tools-list timeout and bypasses the reset
        // policy that keeps slow OAuth bridges recoverable.
        Ok(self
            .initialize_timeout_ms(server_name)?
            .saturating_mul(2)
            .saturating_add(MCP_LIST_TOOLS_TIMEOUT_MS)
            .saturating_add(MCP_DISCOVERY_MARGIN_MS))
    }

    fn initialize_timeout_ms(&self, server_name: &str) -> Result<u64, McpServerManagerError> {
        let server = self
            .servers
            .get(server_name)
            .ok_or_else(|| McpServerManagerError::UnknownServer {
                server_name: server_name.to_string(),
            })?;
        Ok(match &server.bootstrap.transport {
            McpClientTransport::Stdio(transport)
                if is_stdio_remote_bridge(transport)
                    || is_stdio_cold_start_launcher(transport) =>
            {
                MCP_REMOTE_BRIDGE_INITIALIZE_TIMEOUT_MS
            }
            McpClientTransport::Sse(_)
            | McpClientTransport::Http(_)
            | McpClientTransport::WebSocket(_) => MCP_REMOTE_BRIDGE_INITIALIZE_TIMEOUT_MS,
            _ => MCP_INITIALIZE_TIMEOUT_MS,
        })
    }

    /// True when this server is an interactive OAuth bridge — an `mcp-remote`
    /// stdio launch that opens a browser to authenticate. A discovery *timeout*
    /// on such a server usually means it is still waiting for the user to finish
    /// that browser auth (mcp-remote blocks `initialize` until the callback
    /// lands), not that the server is broken. Callers use this to surface those
    /// as "auth pending" instead of "failed".
    #[must_use]
    pub fn is_interactive_oauth_bridge(&self, server_name: &str) -> bool {
        matches!(
            self.servers
                .get(server_name)
                .map(|server| &server.bootstrap.transport),
            Some(McpClientTransport::Stdio(transport)) if is_stdio_remote_bridge(transport)
        )
    }

    fn requires_full_initialize_budget(&self, server_name: &str, initialize_timeout_ms: u64) -> bool {
        initialize_timeout_ms > MCP_INITIALIZE_TIMEOUT_MS
            || self
                .servers
                .get(server_name)
                .is_some_and(|server| match &server.bootstrap.transport {
                    McpClientTransport::Stdio(transport) => is_stdio_remote_bridge(transport),
                    McpClientTransport::Sse(_)
                    | McpClientTransport::Http(_)
                    | McpClientTransport::WebSocket(_) => true,
                    McpClientTransport::Sdk(_) | McpClientTransport::ManagedProxy(_) => false,
                })
    }

    fn tool_call_timeout_ms(&self, server_name: &str) -> Result<u64, McpServerManagerError> {
        let server =
            self.servers
                .get(server_name)
                .ok_or_else(|| McpServerManagerError::UnknownServer {
                    server_name: server_name.to_string(),
                })?;
        match &server.bootstrap.transport {
            McpClientTransport::Stdio(transport) => Ok(transport.resolved_tool_call_timeout_ms()),
            McpClientTransport::Sse(_)
            | McpClientTransport::Http(_)
            | McpClientTransport::WebSocket(_) => Ok(DEFAULT_MCP_TOOL_CALL_TIMEOUT_MS),
            other => Err(McpServerManagerError::InvalidResponse {
                server_name: server_name.to_string(),
                method: "tools/call",
                details: format!("unsupported MCP transport for MCP manager: {other:?}"),
            }),
        }
    }

    fn server_process_exited(&mut self, server_name: &str) -> Result<bool, McpServerManagerError> {
        let server = self.server_mut(server_name)?;
        match server.process.as_mut() {
            Some(process) => Ok(process.has_exited()?),
            None => Ok(false),
        }
    }

    fn server_initialized(&self, server_name: &str) -> bool {
        self.servers
            .get(server_name)
            .is_some_and(|server| server.initialized)
    }

    fn discovery_timeout_method(&self, server_name: &str) -> &'static str {
        if self.server_initialized(server_name) {
            "discover"
        } else {
            "initialize"
        }
    }

    async fn discover_tools_for_server(
        &mut self,
        server_name: &str,
    ) -> Result<Vec<ManagedMcpTool>, McpServerManagerError> {
        with_reset_retry!(
            self,
            server_name,
            self.discover_tools_for_server_once(server_name).await
        )
    }

    async fn discover_tools_for_server_once(
        &mut self,
        server_name: &str,
    ) -> Result<Vec<ManagedMcpTool>, McpServerManagerError> {
        self.paginated(
            server_name,
            "tools/list",
            |process, id, cursor| {
                Box::pin(process.list_tools(id, Some(McpListToolsParams { cursor })))
            },
            |discovered_tools, page| {
                for tool in page.tools {
                    let qualified_name = mcp_tool_name(server_name, &tool.name);
                    discovered_tools.push(ManagedMcpTool {
                        server_name: server_name.to_string(),
                        qualified_name,
                        raw_name: tool.name.clone(),
                        tool,
                    });
                }
                page.next_cursor
            },
        )
        .await
    }

    async fn list_resources_once(
        &mut self,
        server_name: &str,
    ) -> Result<McpListResourcesResult, McpServerManagerError> {
        let resources = self
            .paginated(
                server_name,
                "resources/list",
                |process, id, cursor| {
                    Box::pin(process.list_resources(id, Some(McpListResourcesParams { cursor })))
                },
                |resources, page| {
                    resources.extend(page.resources);
                    page.next_cursor
                },
            )
            .await?;

        Ok(McpListResourcesResult {
            resources,
            next_cursor: None,
        })
    }

    async fn read_resource_once(
        &mut self,
        server_name: &str,
        uri: &str,
    ) -> Result<McpReadResourceResult, McpServerManagerError> {
        self.single_request(server_name, "resources/read", |process, id| {
            Box::pin(process.read_resource(
                id,
                McpReadResourceParams {
                    uri: uri.to_string(),
                },
            ))
        })
        .await
    }

    async fn list_prompts_once(
        &mut self,
        server_name: &str,
    ) -> Result<Vec<McpPrompt>, McpServerManagerError> {
        self.paginated(
            server_name,
            "prompts/list",
            |process, id, cursor| {
                Box::pin(process.list_prompts(id, Some(McpListPromptsParams { cursor })))
            },
            |prompts, page| {
                prompts.extend(page.prompts);
                page.next_cursor
            },
        )
        .await
    }

    async fn get_prompt_once(
        &mut self,
        server_name: &str,
        prompt_name: &str,
        arguments: Option<JsonValue>,
    ) -> Result<McpGetPromptResult, McpServerManagerError> {
        self.single_request(server_name, "prompts/get", |process, id| {
            Box::pin(process.get_prompt(
                id,
                McpGetPromptParams {
                    name: prompt_name.to_string(),
                    arguments,
                },
            ))
        })
        .await
    }

    /// One non-paginated request: ready check, id allocation, transport
    /// dispatch with the standard list timeout, JSON-RPC error mapping, and
    /// result-payload extraction. The request future is boxed because it
    /// must reborrow the live transport (`&mut ManagedMcpProcess`) handed
    /// out per call — the HRTB + `Box::pin` form is how stable Rust spells
    /// that borrowing closure.
    async fn single_request<Payload>(
        &mut self,
        server_name: &str,
        method: &'static str,
        request: impl for<'p> FnOnce(
            &'p mut ManagedMcpProcess,
            JsonRpcId,
        ) -> Pin<
            Box<dyn Future<Output = io::Result<JsonRpcResponse<Payload>>> + 'p>,
        >,
    ) -> Result<Payload, McpServerManagerError> {
        self.ensure_server_ready(server_name).await?;

        let request_id = self.take_request_id();
        let response =
            {
                let server = self.server_mut(server_name)?;
                let process = server.process.as_mut().ok_or_else(|| {
                    McpServerManagerError::InvalidResponse {
                        server_name: server_name.to_string(),
                        method,
                        details: "server process missing after initialization".to_string(),
                    }
                })?;
                Self::run_process_request(
                    server_name,
                    method,
                    MCP_LIST_TOOLS_TIMEOUT_MS,
                    request(process, request_id),
                )
                .await?
            };

        if let Some(error) = response.error {
            return Err(McpServerManagerError::JsonRpc {
                server_name: server_name.to_string(),
                method,
                error: Box::new(error),
            });
        }

        response
            .result
            .ok_or_else(|| McpServerManagerError::InvalidResponse {
                server_name: server_name.to_string(),
                method,
                details: "missing result payload".to_string(),
            })
    }

    /// Drain one paginated MCP list endpoint to completion.
    ///
    /// `request_page` issues a single page request (`None` cursor = first
    /// page); `collect` appends that page's items to the accumulator and
    /// returns the next cursor (`None` ends the loop). The readiness check
    /// runs once up front — a server death mid-pagination surfaces as a
    /// transport error for the retry layer, exactly like the previous
    /// hand-rolled loops. See [`Self::single_request`] for why the page
    /// future is boxed.
    async fn paginated<Page, Item>(
        &mut self,
        server_name: &str,
        method: &'static str,
        mut request_page: impl for<'p> FnMut(
            &'p mut ManagedMcpProcess,
            JsonRpcId,
            Option<String>,
        ) -> Pin<
            Box<dyn Future<Output = io::Result<JsonRpcResponse<Page>>> + 'p>,
        >,
        mut collect: impl FnMut(&mut Vec<Item>, Page) -> Option<String>,
    ) -> Result<Vec<Item>, McpServerManagerError> {
        self.ensure_server_ready(server_name).await?;

        let mut items = Vec::new();
        let mut cursor: Option<String> = None;
        for page_number in 1..=MAX_PAGINATION_PAGES {
            let request_id = self.take_request_id();
            // Clone the current cursor for the request; `cursor` itself is left
            // intact (only the last match arm below reassigns it), so it still
            // holds the value sent this page for the no-progress check — a server
            // repeating its `nextCursor` must not be paged forever.
            let request_cursor = cursor.clone();
            let response = {
                let server = self.server_mut(server_name)?;
                let process = server.process.as_mut().ok_or_else(|| {
                    McpServerManagerError::InvalidResponse {
                        server_name: server_name.to_string(),
                        method,
                        details: "server process missing after initialization".to_string(),
                    }
                })?;
                Self::run_process_request(
                    server_name,
                    method,
                    MCP_LIST_TOOLS_TIMEOUT_MS,
                    request_page(process, request_id, request_cursor),
                )
                .await?
            };

            if let Some(error) = response.error {
                return Err(McpServerManagerError::JsonRpc {
                    server_name: server_name.to_string(),
                    method,
                    error: Box::new(error),
                });
            }

            let page = response
                .result
                .ok_or_else(|| McpServerManagerError::InvalidResponse {
                    server_name: server_name.to_string(),
                    method,
                    details: "missing result payload".to_string(),
                })?;

            match collect(&mut items, page) {
                None => return Ok(items),
                Some(next_cursor) if Some(&next_cursor) == cursor.as_ref() => {
                    return Err(McpServerManagerError::InvalidResponse {
                        server_name: server_name.to_string(),
                        method,
                        details: format!(
                            "pagination cursor did not advance after page {page_number} — \
                             the server is repeating the same cursor"
                        ),
                    });
                }
                Some(next_cursor) => cursor = Some(next_cursor),
            }
        }

        Err(McpServerManagerError::InvalidResponse {
            server_name: server_name.to_string(),
            method,
            details: format!(
                "pagination exceeded {MAX_PAGINATION_PAGES} pages — \
                 the server may be returning an unbounded cursor"
            ),
        })
    }

    /// JSON-RPC `-32601` (method not found): the server simply does not
    /// implement the capability. Consumers treat this as "no prompts", never
    /// as a degraded server.
    fn is_method_not_found(error: &McpServerManagerError) -> bool {
        matches!(
            error,
            McpServerManagerError::JsonRpc { error, .. } if error.code == -32601
        )
    }

    async fn reset_server(&mut self, server_name: &str) -> Result<(), McpServerManagerError> {
        let mut process = {
            let server = self.server_mut(server_name)?;
            server.initialized = false;
            server.process.take()
        };

        if let Some(process) = process.as_mut() {
            let _ = process.shutdown().await;
        }

        Ok(())
    }

    /// Reset a server in response to a specific error, bounding the OAuth respawn
    /// loop. A timeout against a child that is *still alive* is treated as a slow
    /// in-progress handshake (the interactive `mcp-remote` OAuth case): once a
    /// server has burned its [`MAX_CONSECUTIVE_TIMEOUT_RESETS`] budget, the live
    /// process is left running rather than killed-and-respawned, so its single
    /// browser tab can finish authenticating instead of being relaunched on every
    /// timeout. Every other reset-worthy error (transport failure, invalid
    /// response) — and a timeout against a child that has already exited — resets
    /// unconditionally, because those mean the connection is genuinely dead. The
    /// budget clears on the next successful `initialize` (see
    /// [`Self::initialize_server_once`]).
    async fn reset_server_for_error(
        &mut self,
        server_name: &str,
        error: &McpServerManagerError,
    ) -> Result<(), McpServerManagerError> {
        if matches!(error, McpServerManagerError::Timeout { .. })
            && self.server_process_alive(server_name)?
        {
            // A *known* interactive OAuth bridge gets no respawn budget at all:
            // its `initialize` blocks on the user finishing browser auth, which
            // routinely outlasts any timeout. Killing the live child orphans the
            // auth tab it already opened, and every respawn opens another —
            // discovery's timeout retry plus one on-demand refresh stacked three
            // browser windows, of which only the last could complete. Keep the
            // one live process (and its one tab) from the very first timeout.
            if self.is_interactive_oauth_bridge(server_name) {
                return Ok(());
            }
            let server = self.server_mut(server_name)?;
            server.consecutive_timeout_resets += 1;
            if server.consecutive_timeout_resets > MAX_CONSECUTIVE_TIMEOUT_RESETS {
                // Budget exhausted: keep the live process so a slow-but-alive
                // handshake can complete instead of being relaunched forever.
                return Ok(());
            }
        }
        self.reset_server(server_name).await
    }

    /// Whether the server currently owns a child process that has *not* exited.
    /// A missing process (never spawned, or already reset) counts as not alive,
    /// so a timeout in that state resets normally.
    fn server_process_alive(&mut self, server_name: &str) -> Result<bool, McpServerManagerError> {
        let server = self.server_mut(server_name)?;
        match server.process.as_mut() {
            Some(process) => Ok(!process.has_exited()?),
            None => Ok(false),
        }
    }

    fn is_retryable_error(error: &McpServerManagerError) -> bool {
        matches!(
            error,
            McpServerManagerError::Transport { .. } | McpServerManagerError::Timeout { .. }
        )
    }

    fn should_reset_server(error: &McpServerManagerError) -> bool {
        matches!(
            error,
            McpServerManagerError::Transport { .. }
                | McpServerManagerError::Timeout { .. }
                | McpServerManagerError::InvalidResponse { .. }
        )
    }

    async fn run_process_request<T, F>(
        server_name: &str,
        method: &'static str,
        timeout_ms: u64,
        future: F,
    ) -> Result<T, McpServerManagerError>
    where
        F: Future<Output = io::Result<T>>,
    {
        match timeout(Duration::from_millis(timeout_ms), future).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) if error.kind() == io::ErrorKind::InvalidData => {
                Err(McpServerManagerError::InvalidResponse {
                    server_name: server_name.to_string(),
                    method,
                    details: error.to_string(),
                })
            }
            Ok(Err(source)) => Err(McpServerManagerError::Transport {
                server_name: server_name.to_string(),
                method,
                source,
            }),
            Err(_) => Err(McpServerManagerError::Timeout {
                server_name: server_name.to_string(),
                method,
                timeout_ms,
            }),
        }
    }

    async fn ensure_server_ready(
        &mut self,
        server_name: &str,
    ) -> Result<(), McpServerManagerError> {
        if self.server_process_exited(server_name)? {
            self.reset_server(server_name).await?;
        }

        with_reset_retry!(
            self,
            server_name,
            self.initialize_server_once(server_name).await
        )
    }

    /// One spawn-if-needed + `initialize` handshake attempt. Pulled out of
    /// [`Self::ensure_server_ready`] so the retry skeleton is the shared
    /// `with_reset_retry!` instead of a hand-rolled inline variant. Spawn
    /// failures surface as [`McpServerManagerError::Io`], which is neither
    /// retryable nor reset-worthy — identical to the old inline `?`.
    async fn initialize_server_once(
        &mut self,
        server_name: &str,
    ) -> Result<(), McpServerManagerError> {
        let needs_spawn = self
            .servers
            .get(server_name)
            .map(|server| server.process.is_none())
            .ok_or_else(|| McpServerManagerError::UnknownServer {
                server_name: server_name.to_string(),
            })?;

        // Interactive OAuth bridges (`mcp-remote` for Atlassian, etc.) open a
        // browser tab on every spawn. The flag guards against re-spawn only
        // while browser auth is still pending (we spawned once but initialize
        // never succeeded — the user hasn't clicked "Allow" yet). Once
        // initialize completes, the flag is cleared: the bridge is now
        // authenticated (mcp-remote has cached tokens), so a later crash can
        // recover with a normal re-spawn that does NOT open another browser tab.
        //
        // The guard only fires when the process is gone (needs_spawn), so a
        // live, already-initialized OAuth bridge is never blocked: its
        // `needs_spawn` is false, so `ensure_server_ready` returns a no-op.
        let is_oauth_bridge = self.is_interactive_oauth_bridge(server_name);
        if needs_spawn {
            if is_oauth_bridge {
                let already_spawned = self
                    .servers
                    .get(server_name)
                    .is_some_and(|server| server.interactive_oauth_bridge_spawned);
                if already_spawned {
                    return Err(McpServerManagerError::InvalidResponse {
                        server_name: server_name.to_string(),
                        method: "initialize",
                        details: "interactive OAuth bridge is waiting for browser \
                                  authentication — finish the open browser tab first, \
                                  then re-open zo"
                            .to_string(),
                    });
                }
            }

            let server = self.server_mut(server_name)?;
            server.process = Some(open_mcp_process(&server.bootstrap).await?);
            server.initialized = false;
            // Set the "auth pending" gate: we've spawned this OAuth bridge but
            // it hasn't completed the handshake yet. Cleared when initialize
            // succeeds (see the `initialized = true` block below).
            if is_oauth_bridge {
                server.interactive_oauth_bridge_spawned = true;
            }
        }

        let needs_initialize = self
            .servers
            .get(server_name)
            .map(|server| !server.initialized)
            .ok_or_else(|| McpServerManagerError::UnknownServer {
                server_name: server_name.to_string(),
            })?;

        if !needs_initialize {
            return Ok(());
        }

        let request_id = self.take_request_id();
        let timeout_ms = self.initialize_timeout_ms(server_name)?;
        let response =
            {
                let server = self.server_mut(server_name)?;
                let process = server.process.as_mut().ok_or_else(|| {
                    McpServerManagerError::InvalidResponse {
                        server_name: server_name.to_string(),
                        method: "initialize",
                        details: "server process missing before initialize".to_string(),
                    }
                })?;
                Self::run_process_request(
                    server_name,
                    "initialize",
                    timeout_ms,
                    process.initialize(request_id, default_initialize_params()),
                )
                .await?
            };

        if let Some(error) = response.error {
            return Err(McpServerManagerError::JsonRpc {
                server_name: server_name.to_string(),
                method: "initialize",
                error: Box::new(error),
            });
        }

        if response.result.is_none() {
            // `should_reset_server` treats InvalidResponse as reset-worthy,
            // so the retry skeleton performs the reset this arm used to do
            // inline before returning the error.
            return Err(McpServerManagerError::InvalidResponse {
                server_name: server_name.to_string(),
                method: "initialize",
                details: "missing result payload".to_string(),
            });
        }

        // Send the required `notifications/initialized` notification per MCP spec.
        // This is a fire-and-forget JSON-RPC notification (no `id` field).
        // Errors are intentionally ignored: the notification is best-effort
        // and some servers may not handle it.
        let server = self.server_mut(server_name)?;
        if let Some(process) = server.process.as_mut() {
            let _ = process.send_notification("notifications/initialized").await;
        }

        server.initialized = true;
        // A clean handshake means any prior slow-OAuth respawns are behind us:
        // reset the timeout-respawn budget so a *later* genuine disconnect still
        // gets a full respawn allowance (see [`MAX_CONSECUTIVE_TIMEOUT_RESETS`]).
        // Also clear the OAuth "auth pending" gate so a post-auth crash can
        // recover with a normal re-spawn (mcp-remote has cached tokens and
        // won't open another browser tab).
        server.consecutive_timeout_resets = 0;
        server.interactive_oauth_bridge_spawned = false;
        Ok(())
    }
}


fn is_stdio_remote_bridge(transport: &crate::mcp_client::McpStdioTransport) -> bool {
    std::iter::once(transport.command.as_str())
        .chain(transport.args.iter().map(String::as_str))
        .any(|part| {
            let name = part.rsplit(['/', '\\']).next().unwrap_or(part);
            name == "mcp-remote" || name.starts_with("mcp-remote@")
        })
}

/// Stdio servers launched through a package runner that fetches-and-installs on
/// first use (`npx`, `bunx`, `pnpx`) pay a cold-start cost: the package is
/// downloaded before the child can answer `initialize`. Like the `mcp-remote`
/// bridge (itself an `npx` launch), they need the wider remote-bridge initialize
/// window — the 15s `MCP_INITIALIZE_TIMEOUT_MS` default is sized for
/// already-installed local stdio (`uvx`, `node`, a packaged binary) and times
/// out a first-boot install, surfacing an otherwise-healthy server (e.g. one
/// run via `npx chrome-devtools-mcp`) as failed. Matched on the launcher
/// command's basename, so it covers any npx-run server, not just known packages.
fn is_stdio_cold_start_launcher(transport: &crate::mcp_client::McpStdioTransport) -> bool {
    let command = transport
        .command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(transport.command.as_str());
    matches!(command, "npx" | "bunx" | "pnpx")
}

/// The storage key (the server name) under which this remote server's OAuth
/// bearer token is cached. Always the server name for remote transports, so each
/// request can inject `Authorization: Bearer` when a token exists.
///
/// A token may be cached even when the config declares no explicit `oauth` block
/// — `zo mcp auth` can obtain one through native discovery and store it under the
/// server name. Keying injection on token presence (a cheap credential lookup
/// that returns `None` when absent) rather than on config makes such a discovered
/// token take effect on subsequent requests, and lets an external refresh be
/// picked up without reconnecting.
fn oauth_server_name(bootstrap: &McpClientBootstrap) -> String {
    bootstrap.server_name.clone()
}

async fn open_mcp_process(bootstrap: &McpClientBootstrap) -> io::Result<ManagedMcpProcess> {
    if bootstrap.is_project_scoped {
        eprintln!(
            "Warning: MCP server '{}' is defined in project-scoped config and may execute project-defined behavior.",
            bootstrap.server_name
        );
    }

    match &bootstrap.transport {
        McpClientTransport::Stdio(_) => {
            spawn_mcp_stdio_process(bootstrap).map(ManagedMcpProcess::Stdio)
        }
        McpClientTransport::Sse(transport) => {
            crate::mcp_sse::connect_mcp_sse(transport, Some(oauth_server_name(bootstrap)))
                .await
                .map(ManagedMcpProcess::Sse)
        }
        McpClientTransport::Http(transport) => {
            crate::mcp_http::connect_mcp_http(transport, Some(oauth_server_name(bootstrap)))
                .map(ManagedMcpProcess::Http)
        }
        McpClientTransport::WebSocket(transport) => {
            crate::mcp_ws::connect_mcp_ws(transport, Some(oauth_server_name(bootstrap)))
                .await
                .map(Box::new)
                .map(ManagedMcpProcess::Ws)
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "MCP bootstrap transport for {} is unsupported by manager: {other:?}",
                bootstrap.server_name
            ),
        )),
    }
}

fn default_initialize_params() -> McpInitializeParams {
    McpInitializeParams {
        protocol_version: "2025-03-26".to_string(),
        capabilities: JsonValue::Object(serde_json::Map::new()),
        client_info: McpInitializeClientInfo {
            name: "runtime".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    }
}

#[cfg(test)]
mod tests;
