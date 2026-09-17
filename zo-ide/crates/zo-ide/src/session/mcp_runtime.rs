use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use api::sync_bridge::run_blocking;
use runtime::{ManagedMcpTool, McpDiscoveryClass, McpServerManager, McpTool, PermissionMode};
use serde_json::json;
use tools::{GlobalToolRegistry, RuntimeToolDefinition};

/// Out-of-band images returned by MCP tools, staged until the conversation loop
/// attaches them to that tool's `tool_result` message. This deliberately has a
/// separate tiny mutex instead of living behind [`RuntimeMcpState`]'s main lock:
/// startup MCP discovery may hold the main lock while doing slow `initialize` /
/// `tools/list` RPCs, and tool-result image draining runs on the TUI turn future
/// where blocking that lock freezes the render tick.
pub(crate) type PendingMcpImages = Arc<Mutex<Vec<(String, String)>>>;

/// Classified outcome of a failed per-server discovery: the human-readable
/// message plus whether the failure is a benign "waiting for interactive OAuth"
/// timeout (an interactive-OAuth bridge still waiting on the browser) rather
/// than a terminal failure. Carried from the off-lock `discover` through to the state
/// commit so the variant survives stringification.
#[derive(Debug, Clone)]
pub(crate) struct DiscoveryFailure {
    pub(crate) message: String,
    pub(crate) auth_pending: bool,
}

impl DiscoveryFailure {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            auth_pending: false,
        }
    }

    /// Classify a manager error against the server's transport: a `Timeout` on an
    /// interactive OAuth bridge (`mcp-remote`) is a recoverable "waiting for the
    /// browser auth callback", so it surfaces as auth-pending; every other error
    /// (transport, spawn, protocol — or a timeout on a non-OAuth server) is a
    /// terminal failure.
    fn classify(
        error: &runtime::McpServerManagerError,
        manager: &McpServerManager,
        server: &str,
    ) -> Self {
        let auth_pending = matches!(error, runtime::McpServerManagerError::Timeout { .. })
            && manager.is_interactive_oauth_bridge(server);
        Self {
            message: error.to_string(),
            auth_pending,
        }
    }
}

/// Wall-clock ceiling for ONE MCP `tools/call` dispatch, applied at the single
/// seam every MCP tool call crosses ([`RuntimeMcpState::call_tool`]).
///
/// The manager already bounds each individual RPC (`initialize` 15s/45s,
/// `tools/call` 60s — see `runtime::mcp_stdio`), but the *aggregate* call path
/// was unbounded: `call_tool_once` awaits `ensure_server_ready` → respawn /
/// transport connect, and the `with_reset_retry!` skeleton awaits a
/// kill-and-`wait` reset before retrying, none of which carry a deadline. A
/// server that wedges in one of those legs stalls the tool call forever — the
/// measured 22-minute `browser_resize` hang, where the `tool_use` stayed
/// orphaned until the user cancelled the turn.
///
/// 120s is deliberately generous: legitimately slow MCP tools (browser
/// navigation, large database queries) must still complete normally. It only
/// has to be *bounded*.
///
/// Override with `ZO_MCP_TOOL_TIMEOUT_SECS=<seconds>`; `0` disables the ceiling
/// entirely (restoring the old unbounded behavior). Anything unparseable falls
/// back to this default rather than failing the call. Env-only on purpose: a
/// `settings.json` key would need slash/modal writer wiring to actually take
/// effect, which this escape hatch does not warrant.
const MCP_TOOL_CALL_TIMEOUT_SECS: u64 = 120;

/// Env override for [`MCP_TOOL_CALL_TIMEOUT_SECS`].
const MCP_TOOL_CALL_TIMEOUT_ENV: &str = "ZO_MCP_TOOL_TIMEOUT_SECS";

/// Resolve the per-call ceiling; `None` = disabled (`…=0`).
fn mcp_tool_call_timeout() -> Option<std::time::Duration> {
    let seconds = std::env::var(MCP_TOOL_CALL_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(MCP_TOOL_CALL_TIMEOUT_SECS);
    (seconds > 0).then(|| std::time::Duration::from_secs(seconds))
}

/// The tool-result text a timed-out MCP call resolves to. Phrased for the model,
/// not the user: it is delivered as a normal `tool_result` (error flavor), so the
/// turn keeps running and the model can pick another route.
fn mcp_tool_call_timeout_message(
    qualified_tool_name: &str,
    budget: std::time::Duration,
) -> String {
    format!(
        "MCP tool `{qualified_tool_name}` timed out after {}s — the server may be hung. \
         The turn continues; consider an alternative approach.",
        budget.as_secs()
    )
}

/// Drive one MCP call to completion under [`MCP_TOOL_CALL_TIMEOUT_SECS`].
///
/// `Err` means the ceiling fired: the caller turns it into an ordinary tool
/// error result, never a turn abort. On expiry `tokio::time::timeout` drops
/// `call`, which cancels the in-flight request — nothing is spawned, so no task
/// or future leaks — and the stdio/ws readers discard the late reply because
/// they skip frames whose JSON-RPC id does not match the one they await, so the
/// next call cannot desync onto a stale response.
///
/// This stays async-shaped rather than parking a thread: MCP tools are flagged
/// long-running (see `runtime_support`), so this whole call already runs under
/// `spawn_blocking` and the TUI spinner/elapsed keeps ticking while it waits.
fn run_mcp_call_bounded<T>(
    qualified_tool_name: &str,
    call: impl Future<Output = T>,
) -> Result<T, runtime::ToolError> {
    let Some(budget) = mcp_tool_call_timeout() else {
        return Ok(run_blocking(call));
    };
    // The `Timeout` (and its `Sleep`) is constructed *inside* the async block so
    // it is only ever created under the runtime `run_blocking` enters, never on a
    // bare sync thread with no time driver registered.
    run_blocking(async move { tokio::time::timeout(budget, call).await }).map_err(|_elapsed| {
        bounded_tool_error(mcp_tool_call_timeout_message(qualified_tool_name, budget))
    })
}

pub(crate) struct RuntimeMcpState {
    pub(crate) manager: McpServerManager,
    pub(crate) pending_servers: Vec<String>,
    pub(crate) degraded_report: Option<runtime::McpDegradedReport>,
    discovery_errors: BTreeMap<String, DiscoveryFailure>,
    discovery_in_progress: bool,
    /// Image content (`media_type`, base64) extracted from MCP tool results,
    /// staged out-of-band so the conversation loop can attach it to the tool
    /// result as a multimodal block — the same channel `read_image` uses via the
    /// registry's `image_sink`. Drained by [`crate::CliToolExecutor::take_pending_images`].
    pending_images: PendingMcpImages,
}

impl RuntimeMcpState {
    /// 설정 서버가 0인 상태 — MCP 없이 부팅한 세션을 흉내 내는 테스트의 씨앗.
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            manager: McpServerManager::from_servers(&std::collections::BTreeMap::new()),
            pending_servers: Vec::new(),
            degraded_report: None,
            discovery_errors: BTreeMap::new(),
            discovery_in_progress: false,
            pending_images: Arc::new(Mutex::new(Vec::new())),
        }
    }


    /// Build a state directly from a manager with every server marked pending —
    /// the seam tests use to exercise scheduling/detach without a full
    /// `RuntimeConfig`. Mirrors [`Self::new`]'s pending seeding.
    #[cfg(test)]
    pub(crate) fn from_manager_for_test(manager: McpServerManager) -> Self {
        let pending_servers = manager.server_names();
        Self {
            manager,
            pending_servers,
            degraded_report: None,
            discovery_errors: BTreeMap::new(),
            discovery_in_progress: false,
            pending_images: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn new(runtime_config: &runtime::RuntimeConfig) -> Option<Self> {
        let manager = McpServerManager::from_runtime_config(runtime_config);
        if manager.server_names().is_empty() && manager.unsupported_servers().is_empty() {
            return None;
        }

        // Defer tool discovery OFF the startup path. Connecting + listing each
        // MCP server (especially an `npx`-launched stdio server doing a cold
        // start) can take many seconds. Instead every configured server starts
        // `pending`; callers choose the orchestration policy for their UX:
        // interactive surfaces start background discovery, while one-shot
        // headless turns run bounded discovery before assembling the request.
        let pending_servers = manager.server_names();
        Some(Self {
            manager,
            pending_servers,
            degraded_report: None,
            discovery_errors: BTreeMap::new(),
            discovery_in_progress: false,
            pending_images: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Clone the independent MCP image sink. Consumers that only need to drain
    /// staged images should use this sink directly, not the main
    /// `RuntimeMcpState` mutex, so slow background discovery cannot stall the TUI
    /// render loop at a tool-result boundary.
    pub(crate) fn pending_image_sink(&self) -> PendingMcpImages {
        Arc::clone(&self.pending_images)
    }

    pub(crate) fn shutdown(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        run_blocking(self.manager.shutdown())?;
        Ok(())
    }

    pub(crate) fn pending_servers(&self) -> Option<Vec<String>> {
        (!self.pending_servers.is_empty()).then(|| self.pending_servers.clone())
    }

    fn pending_servers_for_auto_discovery(&self) -> Option<Vec<String>> {
        let pending = self
            .pending_servers
            .iter()
            .filter(|server| !self.discovery_errors.get(*server).is_some_and(|failure| failure.auth_pending))
            .cloned()
            .collect::<Vec<_>>();
        (!pending.is_empty()).then_some(pending)
    }

    fn begin_discovery(&mut self) -> Option<Vec<String>> {
        if self.discovery_in_progress {
            return None;
        }
        let pending = self.pending_servers_for_auto_discovery()?;
        self.discovery_in_progress = true;
        Some(pending)
    }

    fn is_interactive_oauth_bridge(&self, server: &str) -> bool {
        self.manager.is_interactive_oauth_bridge(server)
    }

    fn finish_discovery(&mut self) {
        self.discovery_in_progress = false;
    }

    pub(crate) fn degraded_report(&self) -> Option<runtime::McpDegradedReport> {
        self.degraded_report.clone()
    }

    fn mark_server_ready(&mut self, server: &str) {
        self.pending_servers.retain(|name| name != server);
        self.discovery_errors.remove(server);
        self.refresh_degraded_report_from_status();
    }

    fn record_discovery_failure(&mut self, server: &str, failure: DiscoveryFailure) {
        self.discovery_errors.insert(server.to_string(), failure);
        self.refresh_degraded_report_from_status();
    }

    fn refresh_degraded_report_from_status(&mut self) {
        // Auth-pending servers are waiting on the user's browser OAuth, not
        // degraded — exclude them so they neither inflate the degraded count nor
        // surface as a runtime failure.
        let failed_servers = self
            .discovery_errors
            .iter()
            .filter(|(_, failure)| !failure.auth_pending)
            .map(|(server, failure)| runtime::McpFailedServer {
                server_name: server.clone(),
                phase: runtime::McpLifecyclePhase::ToolDiscovery,
                error: runtime::McpErrorSurface::new(
                    runtime::McpLifecyclePhase::ToolDiscovery,
                    Some(server.clone()),
                    failure.message.clone(),
                    BTreeMap::new(),
                    true,
                ),
            })
            .chain(
                self.manager
                    .unsupported_servers()
                    .iter()
                    .map(unsupported_server_to_failed_server),
            )
            .collect::<Vec<_>>();
        if failed_servers.is_empty() {
            self.degraded_report = None;
            return;
        }
        let failed_names = failed_servers
            .iter()
            .map(|server| server.server_name.as_str())
            .collect::<Vec<_>>();
        let working_servers = self
            .manager
            .server_names()
            .into_iter()
            .filter(|server| {
                !self.pending_servers.contains(server)
                    && !failed_names.iter().any(|failed| failed == server)
            })
            .collect();
        self.degraded_report = Some(runtime::McpDegradedReport::new(
            working_servers,
            failed_servers,
            self.routed_tool_names(),
            Vec::new(),
        ));
    }

    pub(crate) fn server_names(&self) -> Vec<String> {
        self.manager.server_names()
    }

    fn known_server_names(&self) -> Vec<String> {
        let mut names = BTreeSet::new();
        names.extend(self.manager.server_names());
        names.extend(self.pending_servers.iter().cloned());
        names.extend(self.discovery_errors.keys().cloned());
        names.extend(
            self.manager
                .unsupported_servers()
                .iter()
                .map(|server| server.server_name.clone()),
        );
        names.into_iter().collect()
    }

    fn server_unavailable_message(&self, server_name: &str) -> Option<String> {
        if let Some(failure) = self.discovery_errors.get(server_name) {
            return Some(format!(
                "MCP server `{server_name}` failed discovery: {}",
                failure.message
            ));
        }
        if let Some(unsupported) = self
            .manager
            .unsupported_servers()
            .iter()
            .find(|server| server.server_name == server_name)
        {
            return Some(format!(
                "MCP server `{server_name}` is unsupported: {}",
                unsupported.reason
            ));
        }
        if self.pending_servers.iter().any(|server| server == server_name) {
            return Some(format!(
                "MCP server `{server_name}` is still discovering; try again after discovery completes"
            ));
        }
        None
    }

    fn routed_tool_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for server in self.server_names() {
            names.extend(self.qualified_tool_names_for_server(&server));
        }
        names.sort();
        names.dedup();
        names
    }

    fn mcp_tool_alias_candidates(&self, requested: &str) -> Vec<String> {
        let requested = requested.trim();
        let mut candidates = vec![requested.to_string()];
        let mut servers = self.server_names();
        servers.extend(self.pending_servers.iter().cloned());
        servers.sort();
        servers.dedup();
        for server in servers {
            candidates.push(runtime::mcp_tool_name(&server, requested));
            let normalized_server = runtime::normalize_name_for_mcp(&server);
            for prefix in [
                format!("{server}."),
                format!("{server}:"),
                format!("{server}_"),
                format!("{server}__"),
                format!("{normalized_server}."),
                format!("{normalized_server}:"),
                format!("{normalized_server}_"),
                format!("{normalized_server}__"),
            ] {
                if let Some(raw_tool_name) = requested.strip_prefix(&prefix) {
                    if !raw_tool_name.is_empty() {
                        candidates.push(runtime::mcp_tool_name(&server, raw_tool_name));
                    }
                }
            }
        }
        candidates.sort();
        candidates.dedup();
        candidates
    }

    fn pending_server_for_requested_tool(&self, requested: &str) -> Option<String> {
        let requested = requested.trim();
        self.pending_servers.iter().find_map(|server| {
            let normalized_server = runtime::normalize_name_for_mcp(server);
            let prefixes = [
                runtime::mcp_tool_prefix(server),
                format!("{server}."),
                format!("{server}:"),
                format!("{server}_"),
                format!("{server}__"),
                format!("{normalized_server}."),
                format!("{normalized_server}:"),
                format!("{normalized_server}_"),
                format!("{normalized_server}__"),
            ];
            prefixes
                .iter()
                .any(|prefix| requested.starts_with(prefix))
                .then(|| server.clone())
        })
    }

    fn resolve_mcp_tool_request_name(&self, requested: &str) -> Result<String, runtime::ToolError> {
        let requested = requested.trim();
        if requested.is_empty() {
            return Err(bounded_tool_error(
                "missing required field `qualifiedName`".to_string(),
            ));
        }
        let routed = self.routed_tool_names();
        if routed.iter().any(|name| name == requested) {
            return Ok(requested.to_string());
        }

        let candidates = self.mcp_tool_alias_candidates(requested);
        let matches = candidates
            .iter()
            .filter(|candidate| routed.iter().any(|name| name == *candidate))
            .cloned()
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [single] => Ok(single.clone()),
            [] => {
                if let Some(server) = self.pending_server_for_requested_tool(requested) {
                    return Err(bounded_tool_error(format!(
                        "MCP server `{server}` is still discovering tools; `{requested}` is not available yet. Canonical MCP tool names use `mcp__{}__<tool>` after discovery completes.",
                        runtime::normalize_name_for_mcp(&server)
                    )));
                }
                Ok(requested.to_string())
            }
            many => Err(bounded_tool_error(format!(
                "MCP tool name `{requested}` is ambiguous; use one of: {}",
                many.join(", ")
            ))),
        }
    }

    fn resolve_mcp_tool_request_name_with_refresh(
        &mut self,
        requested: &str,
    ) -> Result<String, runtime::ToolError> {
        match self.resolve_mcp_tool_request_name(requested) {
            Ok(name) => Ok(name),
            Err(error) => {
                let Some(server) = self.pending_server_for_requested_tool(requested) else {
                    return Err(error);
                };
                if let Err(refresh_error) = self.refresh_server_tools(&server) {
                    return Err(bounded_tool_error(format!(
                        "MCP server `{server}` could not finish tool discovery for `{requested}`: {refresh_error}"
                    )));
                }
                self.resolve_mcp_tool_request_name(requested)
            }
        }
    }

    /// Dispatch a runtime (MCP-surface) tool by name. The meta-tools — the
    /// `MCPTool` wrapper plus `ListMcpResourcesTool` / `ReadMcpResourceTool` —
    /// are zo surfaces, NOT server-provided tools, so they get bespoke
    /// handling here; every other name is an actual server tool routed to
    /// [`Self::call_tool`]. Shared by the serial [`crate::CliToolExecutor`] and
    /// the concurrent-dispatch path so both agree — otherwise a meta-tool routed
    /// through the concurrent/long-running path reaches `call_tool` and fails as
    /// "unknown MCP tool".
    pub(crate) fn dispatch_runtime_tool(
        &mut self,
        tool_name: &str,
        value: serde_json::Value,
    ) -> Result<String, runtime::ToolError> {
        use crate::session::{ListMcpResourcesRequest, McpToolRequest, ReadMcpResourceRequest};
        match tool_name {
            "MCPTool" => {
                let input: McpToolRequest = serde_json::from_value(value).map_err(|error| {
                    bounded_tool_error(format!("invalid tool input JSON: {error}"))
                })?;
                let requested_name = input.qualified_name.or(input.tool).ok_or_else(|| {
                    bounded_tool_error("missing required field `qualifiedName`")
                })?;
                let qualified_name =
                    self.resolve_mcp_tool_request_name_with_refresh(&requested_name)?;
                self.call_tool(&qualified_name, input.arguments)
            }
            "ListMcpResourcesTool" => {
                let input: ListMcpResourcesRequest =
                    serde_json::from_value(value).map_err(|error| {
                        bounded_tool_error(format!("invalid tool input JSON: {error}"))
                    })?;
                match input.server {
                    Some(server_name) => self.list_resources_for_server(&server_name),
                    None => self.list_resources_for_all_servers(),
                }
            }
            "ReadMcpResourceTool" => {
                let input: ReadMcpResourceRequest =
                    serde_json::from_value(value).map_err(|error| {
                        bounded_tool_error(format!("invalid tool input JSON: {error}"))
                    })?;
                self.read_resource(&input.server, &input.uri)
            }
            _ => self.call_tool(tool_name, Some(value)),
        }
    }

    pub(crate) fn call_tool(
        &mut self,
        qualified_tool_name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<String, runtime::ToolError> {
        let response = run_mcp_call_bounded(
            qualified_tool_name,
            self.manager.call_tool(qualified_tool_name, arguments),
        )?
        .map_err(|error| bounded_tool_error(error.to_string()))?;
        if let Some(error) = response.error {
            return Err(bounded_tool_error(format!(
                "MCP tool `{qualified_tool_name}` returned JSON-RPC error: {} ({})",
                error.message, error.code
            )));
        }

        let mut result = response.result.ok_or_else(|| {
            bounded_tool_error(format!(
                "MCP tool `{qualified_tool_name}` returned no result payload"
            ))
        })?;
        // Surface image content (screenshots, generated images) as out-of-band
        // multimodal blocks so the model actually *sees* it, rather than the base64
        // buried in the text result where it is invisible. The base64 is stripped
        // from the text view (replaced with a marker) so the payload isn't sent
        // twice; CliToolExecutor drains these alongside read_image's staged images.
        let images = take_tool_call_images(&mut result);
        if !images.is_empty() {
            self.pending_images
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(images);
        }
        render_tool_call_result(qualified_tool_name, &result)
    }

    pub(crate) fn list_resources_for_server(
        &mut self,
        server_name: &str,
    ) -> Result<String, runtime::ToolError> {
        if !self.manager.server_names().iter().any(|name| name == server_name) {
            if let Some(message) = self.server_unavailable_message(server_name) {
                return Err(bounded_tool_error(message));
            }
        }

        let result = run_blocking(self.manager.list_resources(server_name))
            .map_err(|error| bounded_tool_error(error.to_string()))?;
        serde_json::to_string_pretty(&json!({
            "server": server_name,
            "resources": result.resources,
        }))
        .map_err(|error| bounded_tool_error(error.to_string()))
    }

    pub(crate) fn list_resources_for_all_servers(&mut self) -> Result<String, runtime::ToolError> {
        let mut resources = Vec::new();
        let mut failures = Vec::new();
        let live_servers = self.manager.server_names();

        for server_name in self.known_server_names() {
            if !live_servers.iter().any(|name| name == &server_name) {
                if let Some(message) = self.server_unavailable_message(&server_name) {
                    failures.push(json!({
                        "server": server_name,
                        "error": message,
                    }));
                    continue;
                }
            }

            match run_blocking(self.manager.list_resources(&server_name)) {
                Ok(result) => resources.push(json!({
                    "server": server_name,
                    "resources": result.resources,
                })),
                Err(error) => failures.push(json!({
                    "server": server_name,
                    "error": error.to_string(),
                })),
            }
        }

        if resources.is_empty() && !failures.is_empty() {
            let message = failures
                .iter()
                .filter_map(|failure| failure.get("error").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(bounded_tool_error(message));
        }

        serde_json::to_string_pretty(&json!({
            "resources": resources,
            "failures": failures,
        }))
        .map_err(|error| bounded_tool_error(error.to_string()))
    }

    pub(crate) fn read_resource(
        &mut self,
        server_name: &str,
        uri: &str,
    ) -> Result<String, runtime::ToolError> {
        if !self.manager.server_names().iter().any(|name| name == server_name) {
            if let Some(message) = self.server_unavailable_message(server_name) {
                return Err(bounded_tool_error(message));
            }
        }

        let result = run_blocking(self.manager.read_resource(server_name, uri))
            .map_err(|error| bounded_tool_error(error.to_string()))?;
        serde_json::to_string_pretty(&json!({
            "server": server_name,
            "contents": result.contents,
        }))
        .map_err(|error| bounded_tool_error(error.to_string()))
    }

    /// Re-discover one server's tools after a `tools/list_changed`, updating the
    /// manager's routing so subsequent calls reach the new set.
    pub(crate) fn refresh_server_tools(
        &mut self,
        server: &str,
    ) -> Result<Vec<ManagedMcpTool>, runtime::ToolError> {
        match run_blocking(self.manager.refresh_server_tools(server)) {
            Ok(tools) => {
                // A successful (re)discovery clears this server from the deferred
                // backlog so `pending_servers()` reflects what still needs probing.
                self.mark_server_ready(server);
                Ok(tools)
            }
            Err(error) => {
                let failure = DiscoveryFailure::classify(&error, &self.manager, server);
                let tool_error = bounded_tool_error(failure.message.clone());
                self.record_discovery_failure(server, failure);
                Err(tool_error)
            }
        }
    }

    /// Qualified names the manager currently routes to `server` (by raw config
    /// name). Snapshotted before a refresh so the splice removes exactly that
    /// server's advertised tools.
    pub(crate) fn qualified_tool_names_for_server(&self, server: &str) -> Vec<String> {
        self.manager.qualified_tool_names_for_server(server)
    }

    /// Order `pending` fastest-`initialize`-first within each concurrency class
    /// (stdio before remote ties broken by timeout), the schedule the concurrent
    /// background discovery follows so a quick local server's tools surface
    /// before a slow OAuth/remote bridge's. Pure read; mirrors the eager startup
    /// path's ordering.
    pub(crate) fn discovery_schedule(&self, pending: &[String]) -> Vec<(String, McpDiscoveryClass)> {
        let mut scheduled = pending
            .iter()
            .filter(|name| self.manager.server_names().iter().any(|known| known == *name))
            .map(|name| {
                (
                    name.clone(),
                    self.manager.discovery_class_for(name),
                    self.manager.initialize_timeout_ms_for(name),
                )
            })
            .collect::<Vec<_>>();
        // Stdio (cheaper, tighter cap) first, then remote; within each, fast
        // initialize first. A stable sort keeps config order for exact ties.
        scheduled.sort_by_key(|(_, class, timeout_ms)| {
            (matches!(class, McpDiscoveryClass::Remote), *timeout_ms)
        });
        scheduled
            .into_iter()
            .map(|(name, class, _)| (name, class))
            .collect()
    }

    /// Detach one pending server into a standalone single-server manager for
    /// off-lock concurrent discovery, snapshotting the routes it currently
    /// advertises (for the registry splice on completion). Returns `None` if the
    /// server is unknown or already detached. Brief-lock half of the concurrent
    /// path: the slow handshake then runs with **no** `RuntimeMcpState` lock
    /// held, so on-demand tool dispatch is never blocked behind a cold start.
    pub(crate) fn detach_pending_for_discovery(
        &mut self,
        server: &str,
    ) -> Option<DetachedDiscoveryUnit> {
        let old_names = self.manager.qualified_tool_names_for_server(server);
        let manager = self.manager.detach_for_discovery(server)?;
        Some(DetachedDiscoveryUnit {
            server: server.to_string(),
            manager,
            old_names,
        })
    }

    /// Commit a server whose detached discovery succeeded: re-absorb its live
    /// connection + fresh tools into the routing index, and clear it from the
    /// pending backlog. The
    /// caller splices the registry with `old_names`/`fresh` separately. Brief
    /// lock — the network round-trip already happened off-lock.
    pub(crate) fn absorb_discovered(
        &mut self,
        unit: DetachedDiscoveryUnit,
        fresh: &[ManagedMcpTool],
    ) {
        let DetachedDiscoveryUnit { server, manager, .. } = unit;
        let _ = self.manager.absorb_discovered(&server, manager, fresh);
        self.mark_server_ready(&server);
    }

    /// Commit a server whose detached discovery failed: re-attach the live entry
    /// (so a later turn can retry it) without touching routes, and record the
    /// classified failure so it surfaces in `/mcp` and the HUD — `failed` for a
    /// terminal failure, or `auth pending` for an interactive-OAuth bridge still
    /// waiting on the browser callback. Brief lock.
    pub(crate) fn reattach_failed_discovery(
        &mut self,
        unit: DetachedDiscoveryUnit,
        failure: DiscoveryFailure,
    ) {
        let DetachedDiscoveryUnit { server, manager, .. } = unit;
        self.manager.reattach_detached(&server, manager);
        self.record_discovery_failure(&server, failure);
    }
}

/// One pending MCP server detached from [`RuntimeMcpState`] for off-lock
/// concurrent discovery: its own single-server [`McpServerManager`] plus the
/// qualified tool names it advertised before the refresh (for the registry
/// splice). Re-absorbed via [`RuntimeMcpState::absorb_discovered`] on success or
/// [`RuntimeMcpState::reattach_failed_discovery`] on failure.
pub(crate) struct DetachedDiscoveryUnit {
    pub(crate) server: String,
    manager: McpServerManager,
    pub(crate) old_names: Vec<String>,
}

impl DetachedDiscoveryUnit {
    /// Drive this server's full discovery (initialize → tools/list, with the
    /// existing reset/OAuth/timeout/pagination logic) on the detached manager.
    /// Async and **lock-free** — run concurrently with other units under one
    /// `buffer_unordered`.
    pub(crate) async fn discover(
        &mut self,
    ) -> Result<Vec<ManagedMcpTool>, DiscoveryFailure> {
        let fresh = match self.manager.refresh_server_tools(&self.server).await {
            Ok(fresh) => fresh,
            Err(error) => {
                return Err(DiscoveryFailure::classify(&error, &self.manager, &self.server));
            }
        };
        Ok(fresh)
    }
}

/// Render a protocol-successful MCP `tools/call` result into the executor's
/// `Result<String, ToolError>`.
///
/// A JSON-RPC success can still carry a *tool-level* failure: per the MCP spec a
/// server signals an execution error with `isError: true` *inside* the result
/// (e.g. "file not found", "command exited non-zero"), not as a JSON-RPC error.
/// Returning that as `Ok` would mark the call successful — the model's tool
/// result would lose its `is_error` flag, `PostToolUse` hooks would fire as
/// success, and the TUI would render it green. So an `isError: true` result is
/// mapped to a [`runtime::ToolError`] carrying the result's text content (or the
/// full payload when it has none), making an MCP tool failure propagate exactly
/// like any builtin tool's. Pure, so the mapping is unit-testable without a live
/// MCP process.
fn render_tool_call_result(
    qualified_tool_name: &str,
    result: &runtime::McpToolCallResult,
) -> Result<String, runtime::ToolError> {
    let rendered = serde_json::to_string_pretty(result)
        .map_err(|error| bounded_tool_error(error.to_string()))?;
    if result.is_error == Some(true) {
        // The common MCP failure shape: a perfectly valid JSON-RPC response
        // whose *result* says `isError`. `detail` is whatever the server wrote
        // there, so it needs the same cap as the JSON-RPC error branch.
        let detail = mcp_tool_result_text(result).unwrap_or(rendered);
        return Err(bounded_tool_error(format!(
            "MCP tool `{qualified_tool_name}` reported an error: {detail}"
        )));
    }
    Ok(bounded_tool_output(rendered, qualified_tool_name))
}

/// The module's only SUCCESS-result constructor, mirroring
/// [`bounded_tool_error`]: caps the text at the global tool-output ceiling and
/// leaves the full result recoverable from the artifact store.
///
/// The error path has capped since it was written; the success path never did,
/// and the same sentence explains why it had to: the main session reaches MCP
/// through this module, not through the dispatch seam that caps builtin tools.
/// A server's success payload is as unbounded as its error message — r24a
/// measured 5,049,795 chars from one `evaluate_script` call — and unlike a
/// `read_file` envelope it gets no wire compression on the way out, so it is
/// carried by every later request of the session at full size.
///
/// Head+tail, the same shape as an over-cap bash result, because an MCP result's
/// tail is where a search's last rows and a script's final value live. Results
/// under the cap are returned unchanged, byte for byte.
fn bounded_tool_output(rendered: String, qualified_tool_name: &str) -> String {
    tools::bound_tool_output_text(rendered, qualified_tool_name, None)
}

/// The module's only tool-error constructor: caps the message and leaves the
/// full text recoverable from the artifact store.
///
/// The main session reaches MCP through this module, not through the tool
/// dispatch seam that caps builtin failures, so every error leaving here has to
/// apply the cap itself. Several shapes are server-controlled and bounded only
/// by the 32 MiB transport limit — a JSON-RPC `error.message`, the more common
/// case of a valid response whose result carries `isError`, a stored discovery
/// failure, and the joined per-server messages of a resource listing — and an
/// error tool result is exempt from historical wire compression, so uncapped
/// any of them would ride every later request at full size.
///
/// Every error return in this module goes through here rather than a selected
/// few, because "which of these strings can a server grow?" is exactly the
/// question that goes stale. Text already under the cap passes through
/// unchanged, so routing the fixed-message cases through it costs nothing.
fn bounded_tool_error(message: impl Into<String>) -> runtime::ToolError {
    runtime::ToolError::new(tools::bound_tool_error_text(message.into(), None))
}

/// Placeholder left in the text view of a tool result where an image's base64
/// once was, once the image has been staged as an out-of-band attachment. The
/// model receives the image as a real multimodal block, so duplicating the
/// (often large) base64 in text would only waste tokens.
const STAGED_IMAGE_MARKER: &str = "<image staged as an attached image>";

/// Remove image content from an MCP tool result for out-of-band multimodal
/// staging, returning each image as a `(media_type, base64)` pair (the tuple
/// shape the conversation loop and `read_image` already use). Each image block
/// is kept in the result with its base64 replaced by [`STAGED_IMAGE_MARKER`], so
/// the text view notes that an image was returned without re-sending the payload
/// the model already gets as an attachment. Non-image blocks are untouched.
fn take_tool_call_images(result: &mut runtime::McpToolCallResult) -> Vec<(String, String)> {
    let mut images = Vec::new();
    for block in &mut result.content {
        if block.kind != "image" {
            continue;
        }
        // `data` (base64) is required for a usable image; skip malformed blocks.
        let Some(data) = block
            .data
            .get("data")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let media_type = block
            .data
            .get("mimeType")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("image/png")
            .to_string();
        images.push((media_type, data));
        block.data.insert(
            "data".to_string(),
            serde_json::Value::String(STAGED_IMAGE_MARKER.to_string()),
        );
    }
    images
}

/// Concatenate the `text` content blocks of an MCP tool result, so a tool-level
/// error can be surfaced as a plain message rather than the serialized payload.
/// `None` when the result carries no text content (e.g. an image-only or
/// structured-only error), letting the caller fall back to the full rendering.
fn mcp_tool_result_text(result: &runtime::McpToolCallResult) -> Option<String> {
    let parts = result
        .content
        .iter()
        .filter(|block| block.kind == "text")
        .filter_map(|block| block.data.get("text").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// Replace a single server's tools in a runtime tool set.
///
/// `old_names` are the qualified names the manager routed to that server before
/// the refresh (sourced from the routing index, keyed by the *raw* server name).
/// Removing by this exact set — rather than by a normalized `mcp__<server>__`
/// prefix — keeps a co-located server whose config name normalizes to the same
/// prefix from being collaterally stripped.
///
/// Fresh tools are appended deduped by qualified name, so the advertised set
/// mirrors the manager's `tool_index` (a `BTreeMap` that collapses two tool
/// names which normalize to the same qualified name) and
/// [`GlobalToolRegistry::set_runtime_tools`] is never handed a server-driven
/// duplicate. Pure, so it can be tested without a live MCP process.
fn splice_server_tools(
    mut current: Vec<RuntimeToolDefinition>,
    old_names: &[String],
    fresh: &[ManagedMcpTool],
) -> Vec<RuntimeToolDefinition> {
    current.retain(|def| !old_names.contains(&def.name));
    for tool in fresh {
        if !current.iter().any(|def| def.name == tool.qualified_name) {
            current.push(mcp_runtime_tool_definition(tool));
        }
    }
    current
}

/// Turn-boundary consumer for inbound `tools/list_changed`: poll the long-lived
/// manager, re-discover each changed server, and splice the result into the
/// shared registry via [`GlobalToolRegistry::set_runtime_tools`]. Because the
/// registry's runtime tools are an `Arc<Mutex<…>>`, the new set propagates to
/// every clone — including the request builder that assembles the next turn's
/// tool definitions (the G20 propagation path).
///
/// Best-effort: a per-server refresh failure leaves that server's previously
/// advertised tools in place (a safe fallback) rather than aborting the turn.
/// MCP 를 부팅 때 미리 세우라는 환경 변수. 이름과 "존재하면 참" 규칙이
/// `runtime_builder` 에도 복제돼 있었다 — 한쪽만 고치면 초기 MCP 상태와 실제
/// 빌드 경로가 어긋나 도구가 광고만 되고 초기화되지 않는다.
pub(crate) const EAGER_MCP_ENV: &str = "ZO_EAGER_MCP";

/// 위 변수가 서 있는가.
pub(crate) fn eager_mcp_enabled() -> bool {
    std::env::var_os(EAGER_MCP_ENV).is_some()
}

pub(crate) type McpStateResult = (
    Option<Arc<Mutex<RuntimeMcpState>>>,
    Vec<RuntimeToolDefinition>,
);

pub(crate) fn build_runtime_mcp_state(runtime_config: &runtime::RuntimeConfig) -> McpStateResult {
    let Some(mcp_state) = RuntimeMcpState::new(runtime_config) else {
        return (None, Vec::new());
    };

    if eager_mcp_enabled() {
        return build_eager_runtime_mcp_state(mcp_state);
    }

    // Discovery is deferred to a background thread (see
    // `discover_pending_mcp_tools_in_background`), so at startup we advertise
    // only the always-available MCP meta-tools (ListMcpResources/ReadMcpResource).
    // Each server's own tools are spliced in as background discovery completes.
    let runtime_tools = if mcp_state.server_names().is_empty() {
        Vec::new()
    } else {
        mcp_wrapper_tool_definitions()
    };

    (Some(Arc::new(Mutex::new(mcp_state))), runtime_tools)
}

fn build_eager_runtime_mcp_state(mut mcp_state: RuntimeMcpState) -> McpStateResult {
    let report = run_blocking(mcp_state.manager.discover_tools_best_effort());
    mcp_state.pending_servers = report
        .failed_servers
        .iter()
        .map(|failure| failure.server_name.clone())
        .chain(
            report
                .unsupported_servers
                .iter()
                .map(|server| server.server_name.clone()),
        )
        .collect();
    mcp_state.degraded_report = degraded_report_from_discovery(&report);
    // The eager (headless / ZO_EAGER_MCP) path has no interactive browser, so
    // every failure here is terminal — no auth-pending classification.
    mcp_state.discovery_errors = report
        .failed_servers
        .iter()
        .map(|failure| {
            (
                failure.server_name.clone(),
                DiscoveryFailure::failed(failure.error.clone()),
            )
        })
        .chain(report.unsupported_servers.iter().map(|server| {
            (
                server.server_name.clone(),
                DiscoveryFailure::failed(server.reason.clone()),
            )
        }))
        .collect();

    let mut runtime_tools = mcp_wrapper_tool_definitions();
    runtime_tools.extend(report.tools.iter().map(mcp_runtime_tool_definition));
    (Some(Arc::new(Mutex::new(mcp_state))), runtime_tools)
}

fn degraded_report_from_discovery(
    report: &runtime::McpToolDiscoveryReport,
) -> Option<runtime::McpDegradedReport> {
    if report.degraded_startup.is_some() {
        return report.degraded_startup.clone();
    }

    let failed_servers = report
        .failed_servers
        .iter()
        .map(discovery_failure_to_failed_server)
        .chain(
            report
                .unsupported_servers
                .iter()
                .map(unsupported_server_to_failed_server),
        )
        .collect::<Vec<_>>();
    (!failed_servers.is_empty()).then(|| {
        runtime::McpDegradedReport::new(
            Vec::new(),
            failed_servers,
            report
                .tools
                .iter()
                .map(|tool| tool.qualified_name.clone())
                .collect(),
            Vec::new(),
        )
    })
}

fn discovery_failure_to_failed_server(
    failure: &runtime::McpDiscoveryFailure,
) -> runtime::McpFailedServer {
    runtime::McpFailedServer {
        server_name: failure.server_name.clone(),
        phase: failure.phase,
        error: runtime::McpErrorSurface::new(
            failure.phase,
            Some(failure.server_name.clone()),
            failure.error.clone(),
            failure.context.clone(),
            failure.recoverable,
        ),
    }
}

fn unsupported_server_to_failed_server(
    server: &runtime::UnsupportedMcpServer,
) -> runtime::McpFailedServer {
    runtime::McpFailedServer {
        server_name: server.server_name.clone(),
        phase: runtime::McpLifecyclePhase::ServerRegistration,
        error: runtime::McpErrorSurface::new(
            runtime::McpLifecyclePhase::ServerRegistration,
            Some(server.server_name.clone()),
            server.reason.clone(),
            std::collections::BTreeMap::from([(
                "transport".to_string(),
                format!("{:?}", server.transport),
            )]),
            false,
        ),
    }
}

/// Clears a discovery pass's in-progress flag even when the pass panics.
///
/// The per-unit handshakes are individually `catch_unwind`-guarded, but a
/// panic anywhere OUTSIDE them (the registry splice, the commit lock path,
/// the `run_blocking` machinery) unwinds past the plain
/// `finish_pending_discovery` call, leaving `discovery_in_progress` stuck
/// `true` — after which every `begin_discovery` returns `None` and MCP
/// discovery is silently wedged for the rest of the session, with every
/// still-pending server shown as "Discovering" forever. Tying the reset to
/// `Drop` makes the flag unwind-safe on both the background thread and the
/// synchronous on-demand path.
struct DiscoveryPassGuard(Arc<Mutex<RuntimeMcpState>>);

impl Drop for DiscoveryPassGuard {
    fn drop(&mut self) {
        finish_pending_discovery(&self.0);
    }
}

pub(crate) fn discover_pending_mcp_tools_in_background(
    mcp_state: &Arc<Mutex<RuntimeMcpState>>,
    registry: GlobalToolRegistry,
) {
    let Some(pending) = begin_pending_discovery(mcp_state) else {
        return;
    };
    let thread_mcp_state = Arc::clone(mcp_state);
    let spawned = std::thread::Builder::new()
        .name("mcp-discovery".to_string())
        .spawn(move || {
            let _finish = DiscoveryPassGuard(Arc::clone(&thread_mcp_state));
            discover_pending_mcp_tools(&thread_mcp_state, &registry, &pending);
        });
    // If the thread cannot spawn, clear the in-progress guard so a later turn or
    // command can retry discovery. Startup remains unaffected either way.
    if spawned.is_err() {
        finish_pending_discovery(mcp_state);
    }
}

fn begin_pending_discovery(mcp_state: &Arc<Mutex<RuntimeMcpState>>) -> Option<Vec<String>> {
    mcp_state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .begin_discovery()
}

fn finish_pending_discovery(mcp_state: &Arc<Mutex<RuntimeMcpState>>) {
    mcp_state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .finish_discovery();
}

/// Discover every `pending` MCP server **concurrently**, bounded per class
/// (stdio 3 / remote 20), splicing each server's tools into the shared registry
/// the moment it finishes. zo's analogue of Codex's `JoinSet`-per-server
/// startup and Claude Code's batched parallel connect: a slow OAuth/remote
/// bridge (`npx mcp-remote` for Atlassian, a cold `npx` start) no longer blocks
/// fast local stdio servers queued behind it — the single fix for "every MCP
/// server stuck discovering".
///
/// Lock discipline (the load-bearing invariant): the `RuntimeMcpState` lock is
/// held only briefly to detach each unit up front and to absorb each result as
/// it lands; the slow `initialize`/`tools/list` round-trips run entirely
/// off-lock on detached single-server managers. So on-demand MCP tool dispatch
/// (which takes the same blocking lock) is never stalled behind a multi-server
/// discovery budget — strictly better than the old serial loop, which still
/// held the lock for each server's full handshake.
fn detach_scheduled_discovery_units(
    mcp_state: &Arc<Mutex<RuntimeMcpState>>,
    pending: &[String],
) -> (Vec<DetachedDiscoveryUnit>, Vec<DetachedDiscoveryUnit>) {
    // Detach phase (brief lock): take each pending server fast-first by class so
    // a quick stdio server's tools surface before a slow remote bridge's.
    let schedule = {
        let state = mcp_state.lock().unwrap_or_else(PoisonError::into_inner);
        state.discovery_schedule(pending)
    };
    let mut stdio_units = Vec::new();
    let mut remote_units = Vec::new();
    let mut oauth_bridge_scheduled = false;
    for (server, class) in schedule {
        let unit = {
            let mut state = mcp_state.lock().unwrap_or_else(PoisonError::into_inner);
            if state
                .discovery_errors
                .get(&server)
                .is_some_and(|failure| failure.auth_pending)
            {
                // Do not auto-retry an interactive OAuth bridge that is already
                // waiting on browser auth; retrying respawns `mcp-remote` and
                // opens duplicate auth windows. A direct MCP tool/resource call
                // can still refresh on demand.
                None
            } else if class == McpDiscoveryClass::Stdio
                && state.is_interactive_oauth_bridge(&server)
            {
                if oauth_bridge_scheduled {
                    // Only one browser-opening bridge per automatic pass. Leave
                    // the rest pending for a later explicit/on-demand retry.
                    None
                } else {
                    oauth_bridge_scheduled = true;
                    state.detach_pending_for_discovery(&server)
                }
            } else {
                state.detach_pending_for_discovery(&server)
            }
        };
        let Some(unit) = unit else { continue };
        match class {
            McpDiscoveryClass::Stdio => stdio_units.push(unit),
            McpDiscoveryClass::Remote => remote_units.push(unit),
        }
    }
    (stdio_units, remote_units)
}

fn discover_pending_mcp_tools(
    mcp_state: &Arc<Mutex<RuntimeMcpState>>,
    registry: &GlobalToolRegistry,
    pending: &[String],
) {
    let (stdio_units, remote_units) = detach_scheduled_discovery_units(mcp_state, pending);

    // Concurrent phase (no lock): drive each class's handshakes under one
    // `buffer_unordered`, committing each result as it completes. One
    // `run_blocking` owns the whole batch, so the per-server futures interleave
    // cooperatively (IO-bound) without nesting runtimes.
    run_blocking(async {
        let stdio = run_discovery_class(
            mcp_state,
            registry,
            stdio_units,
            STDIO_DISCOVERY_CONCURRENCY,
        );
        let remote = run_discovery_class(
            mcp_state,
            registry,
            remote_units,
            REMOTE_DISCOVERY_CONCURRENCY,
        );
        join_discovery_class_futures(stdio, remote).await;
    });
}

async fn join_discovery_class_futures<S, R>(stdio: S, remote: R)
where
    S: Future<Output = ()>,
    R: Future<Output = ()>,
{
    futures_util::future::join(stdio, remote).await;
}

/// Per-class concurrency caps, mirroring Claude Code's
/// `MCP_SERVER_CONNECTION_BATCH_SIZE` (stdio, default 3) and
/// `MCP_REMOTE_SERVER_CONNECTION_BATCH_SIZE` (remote, default 20).
const STDIO_DISCOVERY_CONCURRENCY: usize = 3;
const REMOTE_DISCOVERY_CONCURRENCY: usize = 20;

/// Run one concurrency class's detached units with at most `cap` in flight,
/// committing each into shared state + registry as it finishes (so fast servers
/// land first regardless of completion order within the batch).
async fn run_discovery_class(
    mcp_state: &Arc<Mutex<RuntimeMcpState>>,
    registry: &GlobalToolRegistry,
    units: Vec<DetachedDiscoveryUnit>,
    cap: usize,
) {
    use futures_util::stream::{self, StreamExt};

    if units.is_empty() {
        return;
    }
    let mut in_flight = stream::iter(units.into_iter().map(discover_one_unit)).buffer_unordered(cap);
    while let Some(completed) = in_flight.next().await {
        commit_discovered_unit(mcp_state, registry, completed);
    }
}

/// The off-lock result of one server's discovery: the detached unit plus its
/// outcome (fresh tools, or a terminal error message). A panic inside
/// the handshake is caught and mapped to an `Err` so one malformed server can
/// never poison the whole batch — the same isolation the old serial
/// `catch_unwind` provided, now per concurrent task.
type DiscoveredUnit = (
    DetachedDiscoveryUnit,
    Result<Vec<ManagedMcpTool>, DiscoveryFailure>,
);

async fn discover_one_unit(mut unit: DetachedDiscoveryUnit) -> DiscoveredUnit {
    let future = std::panic::AssertUnwindSafe(unit.discover());
    let outcome = match futures_util::FutureExt::catch_unwind(future).await {
        Ok(Ok(found)) => Ok(found),
        Ok(Err(failure)) => Err(failure),
        Err(_panic) => Err(DiscoveryFailure::failed("discovery panicked")),
    };
    (unit, outcome)
}

/// Commit one completed unit under a brief lock: on success, absorb its
/// connection + tools, mark it ready, and splice the registry
/// so the model can call the new tools immediately; on failure, re-attach the
/// live entry and record the terminal `failed` status.
fn commit_discovered_unit(
    mcp_state: &Arc<Mutex<RuntimeMcpState>>,
    registry: &GlobalToolRegistry,
    (unit, outcome): DiscoveredUnit,
) {
    match outcome {
        Ok(fresh) => {
            let defs =
                splice_server_tools(registry.runtime_tool_definitions(), &unit.old_names, &fresh);
            let _ = registry.set_runtime_tools(defs);
            let mut state = mcp_state.lock().unwrap_or_else(PoisonError::into_inner);
            state.absorb_discovered(unit, &fresh);
        }
        Err(error) => {
            let mut state = mcp_state.lock().unwrap_or_else(PoisonError::into_inner);
            state.reattach_failed_discovery(unit, error);
        }
    }
}

fn mcp_runtime_tool_definition(tool: &runtime::ManagedMcpTool) -> RuntimeToolDefinition {
    RuntimeToolDefinition {
        name: tool.qualified_name.clone(),
        description: Some(
            tool.tool
                .description
                .clone()
                .unwrap_or_else(|| format!("Invoke MCP tool `{}`.", tool.qualified_name)),
        ),
        input_schema: tool
            .tool
            .input_schema
            .clone()
            .unwrap_or_else(|| json!({ "type": "object", "additionalProperties": true })),
        required_permission: permission_mode_for_mcp_tool(&tool.tool),
    }
}

fn mcp_wrapper_tool_definitions() -> Vec<RuntimeToolDefinition> {
    vec![
        RuntimeToolDefinition {
            name: "MCPTool".to_string(),
            description: Some(
                "Call a configured MCP tool by its qualified name and JSON arguments.".to_string(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "qualifiedName": { "type": "string" },
                    "arguments": {}
                },
                "required": ["qualifiedName"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::DangerFullAccess,
        },
        RuntimeToolDefinition {
            name: "ListMcpResourcesTool".to_string(),
            description: Some(
                "List MCP resources from one configured server or from every connected server."
                    .to_string(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": { "type": "string" }
                },
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        RuntimeToolDefinition {
            name: "ReadMcpResourceTool".to_string(),
            description: Some("Read a specific MCP resource from a configured server.".to_string()),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "server": { "type": "string" },
                    "uri": { "type": "string" }
                },
                "required": ["server", "uri"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
    ]
}

fn permission_mode_for_mcp_tool(tool: &McpTool) -> PermissionMode {
    let read_only = mcp_annotation_flag(tool, "readOnlyHint");
    let destructive = mcp_annotation_flag(tool, "destructiveHint");
    let open_world = mcp_annotation_flag(tool, "openWorldHint");

    if read_only && !destructive && !open_world {
        PermissionMode::ReadOnly
    } else if destructive || open_world {
        PermissionMode::DangerFullAccess
    } else {
        PermissionMode::WorkspaceWrite
    }
}

fn mcp_annotation_flag(tool: &McpTool, key: &str) -> bool {
    tool.annotations
        .as_ref()
        .and_then(|annotations| annotations.get(key))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{
        DiscoveryPassGuard, MCP_TOOL_CALL_TIMEOUT_ENV,
        MCP_TOOL_CALL_TIMEOUT_SECS, RuntimeMcpState,
        RuntimeToolDefinition, begin_pending_discovery, detach_scheduled_discovery_units,
        finish_pending_discovery, join_discovery_class_futures,
        mcp_tool_call_timeout, mcp_tool_result_text, mcp_wrapper_tool_definitions, run_blocking,
        permission_mode_for_mcp_tool,
        render_tool_call_result, run_mcp_call_bounded,
        splice_server_tools,
        take_tool_call_images,
    };
    use runtime::{
        ManagedMcpTool, McpServerManager, McpTool, McpToolCallContent, McpToolCallResult,
        PermissionMode, mcp_tool_name,
    };
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    fn managed_tool(server: &str, raw: &str) -> ManagedMcpTool {
        ManagedMcpTool {
            server_name: server.to_string(),
            qualified_name: mcp_tool_name(server, raw),
            raw_name: raw.to_string(),
            tool: McpTool {
                name: raw.to_string(),
                description: Some("tool".to_string()),
                input_schema: Some(json!({ "type": "object" })),
                annotations: None,
                meta: None,
            },
        }
    }

    fn runtime_def(name: &str) -> RuntimeToolDefinition {
        RuntimeToolDefinition {
            name: name.to_string(),
            description: None,
            input_schema: json!({ "type": "object" }),
            required_permission: PermissionMode::ReadOnly,
        }
    }

    fn stdio_server(command: &str, args: &[&str]) -> runtime::ScopedMcpServerConfig {
        runtime::ScopedMcpServerConfig {
            scope: runtime::ConfigSource::User,
            config: runtime::McpServerConfig::Stdio(runtime::McpStdioServerConfig {
                command: command.to_string(),
                args: args.iter().map(ToString::to_string).collect(),
                env: std::collections::BTreeMap::new(),
                tool_call_timeout_ms: None,
            }),
        }
    }

    fn http_server(url: &str) -> runtime::ScopedMcpServerConfig {
        runtime::ScopedMcpServerConfig {
            scope: runtime::ConfigSource::User,
            config: runtime::McpServerConfig::Http(runtime::McpRemoteServerConfig {
                url: url.to_string(),
                headers: std::collections::BTreeMap::new(),
                headers_helper: None,
                oauth: None,
            }),
        }
    }

    #[test]
    fn discovery_schedule_orders_fast_stdio_before_slow_bridge_and_remote_last() {
        // Background discovery must follow the eager path's fast-first ordering
        // AND keep tighter-capped stdio ahead of remote, so a quick local server
        // surfaces before a slow OAuth bridge or a network server.
        let servers = std::collections::BTreeMap::from([
            ("zfast".to_string(), stdio_server("uvx", &["local-mcp"])),
            (
                "atlassian".to_string(),
                stdio_server("npx", &["-y", "mcp-remote", "https://mcp.atlassian.com/v1/sse"]),
            ),
            ("context7".to_string(), http_server("https://mcp.context7.com/mcp")),
        ]);
        let state = RuntimeMcpState::from_manager_for_test(McpServerManager::from_servers(&servers));

        let order = state
            .discovery_schedule(&state.pending_servers)
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();

        // `zfast` (plain stdio, short initialize) first despite sorting last
        // alphabetically; the `mcp-remote` bridge is still stdio but slower;
        // `context7` (remote) is last regardless of its timeout.
        assert_eq!(
            order,
            vec![
                "zfast".to_string(),
                "atlassian".to_string(),
                "context7".to_string()
            ]
        );
    }

    #[test]
    fn discovery_classes_run_concurrently_not_stdio_then_remote() {
        let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let stdio_events = Arc::clone(&events);
        let remote_events = Arc::clone(&events);

        run_blocking(async move {
            let slow_stdio = async move {
                stdio_events.lock().expect("events lock").push("stdio-start");
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                stdio_events.lock().expect("events lock").push("stdio-end");
            };
            let fast_remote = async move {
                remote_events.lock().expect("events lock").push("remote-start");
            };
            join_discovery_class_futures(slow_stdio, fast_remote).await;
        });

        let events = events.lock().expect("events lock");
        let remote_start = events
            .iter()
            .position(|event| *event == "remote-start")
            .expect("remote should start");
        let stdio_end = events
            .iter()
            .position(|event| *event == "stdio-end")
            .expect("stdio should finish");
        assert!(
            remote_start < stdio_end,
            "remote discovery must not wait for the stdio class to finish: {events:?}"
        );
    }


    #[test]
    fn automatic_discovery_detaches_at_most_one_oauth_bridge_per_pass() {
        let servers = std::collections::BTreeMap::from([
            (
                "atlassian".to_string(),
                stdio_server("npx", &["-y", "mcp-remote", "https://mcp.atlassian.com/v1/mcp"]),
            ),
            (
                "vercel".to_string(),
                stdio_server("npx", &["-y", "mcp-remote", "https://mcp.vercel.com/mcp"]),
            ),
            ("context7".to_string(), http_server("https://mcp.context7.com/mcp")),
        ]);
        let state = Arc::new(Mutex::new(RuntimeMcpState::from_manager_for_test(
            McpServerManager::from_servers(&servers),
        )));
        let pending = begin_pending_discovery(&state).expect("pending servers");

        let (stdio_units, remote_units) = detach_scheduled_discovery_units(&state, &pending);
        finish_pending_discovery(&state);

        assert_eq!(
            stdio_units
                .iter()
                .filter(|unit| matches!(unit.server.as_str(), "atlassian" | "vercel"))
                .count(),
            1,
            "automatic pass must not spawn multiple browser-auth mcp-remote bridges"
        );
        assert!(
            remote_units.iter().any(|unit| unit.server == "context7"),
            "true HTTP remote discovery should still start without waiting for stdio auth bridges"
        );
    }

    #[test]
    fn discovery_pass_guard_clears_in_progress_on_panic() {
        let servers = std::collections::BTreeMap::from([(
            "context7".to_string(),
            http_server("https://mcp.context7.com/mcp"),
        )]);
        let state = Arc::new(Mutex::new(RuntimeMcpState::from_manager_for_test(
            McpServerManager::from_servers(&servers),
        )));
        begin_pending_discovery(&state).expect("pending servers");

        // A panic OUTSIDE the per-unit catch_unwind (registry splice, commit
        // path, run_blocking machinery) unwinds the pass. The guard must still
        // clear the in-progress flag, or begin_discovery returns None for the
        // rest of the session and MCP discovery is silently wedged.
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _finish = DiscoveryPassGuard(Arc::clone(&state));
            panic!("discovery pass blew up outside the per-unit guard");
        }));
        assert!(unwound.is_err(), "the pass must actually have panicked");

        let retried = begin_pending_discovery(&state);
        assert!(
            retried.is_some(),
            "a later pass must be able to start after a panicked one"
        );
        finish_pending_discovery(&state);
    }


    #[test]
    fn splice_replaces_only_target_server_and_keeps_the_rest() {
        let current = vec![
            runtime_def(&mcp_tool_name("srv", "old")),
            runtime_def(&mcp_tool_name("other", "keep")),
            runtime_def("MCPTool"),
        ];
        let old_names = vec![mcp_tool_name("srv", "old")];
        let fresh = vec![managed_tool("srv", "new1"), managed_tool("srv", "new2")];

        let names = splice_server_tools(current, &old_names, &fresh)
            .into_iter()
            .map(|def| def.name)
            .collect::<Vec<_>>();

        assert!(
            !names.contains(&mcp_tool_name("srv", "old")),
            "the changed server's stale tool is dropped"
        );
        assert!(names.contains(&mcp_tool_name("srv", "new1")));
        assert!(names.contains(&mcp_tool_name("srv", "new2")));
        assert!(
            names.contains(&mcp_tool_name("other", "keep")),
            "another server's tools are preserved"
        );
        assert!(
            names.contains(&"MCPTool".to_string()),
            "non-MCP wrapper tools are preserved"
        );
    }

    #[test]
    fn splice_dedups_fresh_tools_that_normalize_to_the_same_name() {
        // A server advertising `a.b` and `a_b` collapses both to `mcp__srv__a_b`,
        // exactly as the manager's tool_index BTreeMap does. The advertised set
        // must carry ONE entry so set_runtime_tools is never handed a duplicate
        // (which would atomically reject the whole refresh, suppressing even
        // valid new tools).
        let colliding = mcp_tool_name("srv", "a.b");
        assert_eq!(
            colliding,
            mcp_tool_name("srv", "a_b"),
            "precondition: names collide"
        );
        let fresh = vec![managed_tool("srv", "a.b"), managed_tool("srv", "a_b")];

        let count = splice_server_tools(Vec::new(), &[], &fresh)
            .into_iter()
            .filter(|def| def.name == colliding)
            .count();

        assert_eq!(
            count, 1,
            "normalization-colliding fresh tools collapse to one def"
        );
    }

    #[test]
    fn splice_uses_exact_old_names_so_a_co_located_server_survives() {
        // Two server configs whose names normalize to the same prefix
        // (`foo.bar` and `foo_bar` -> `mcp__foo_bar__`) coexist with distinct
        // tools. Refreshing one must NOT strip the other's advertised tool —
        // removal is by the exact routed names of the changed server only.
        let changed_tool = mcp_tool_name("foo.bar", "x"); // mcp__foo_bar__x
        let neighbor_tool = mcp_tool_name("foo_bar", "y"); // mcp__foo_bar__y
        assert_ne!(changed_tool, neighbor_tool, "precondition: distinct tools");
        let current = vec![runtime_def(&changed_tool), runtime_def(&neighbor_tool)];
        let old_names = vec![changed_tool.clone()]; // only the changed server's route
        let fresh = vec![managed_tool("foo.bar", "x2")];

        let names = splice_server_tools(current, &old_names, &fresh)
            .into_iter()
            .map(|def| def.name)
            .collect::<Vec<_>>();

        assert!(
            !names.contains(&changed_tool),
            "changed server's old tool replaced"
        );
        assert!(
            names.contains(&mcp_tool_name("foo.bar", "x2")),
            "changed server's new tool added"
        );
        assert!(
            names.contains(&neighbor_tool),
            "the prefix-colliding co-located server is not collaterally stripped"
        );
    }

    fn tool_with_annotations(annotations: serde_json::Value) -> McpTool {
        McpTool {
            name: "example".to_string(),
            description: None,
            input_schema: None,
            annotations: Some(annotations),
            meta: None,
        }
    }

    #[test]
    fn permission_mode_prefers_read_only_hint_when_safe() {
        let tool = tool_with_annotations(json!({ "readOnlyHint": true }));
        assert_eq!(
            permission_mode_for_mcp_tool(&tool),
            PermissionMode::ReadOnly
        );
    }

    #[test]
    fn permission_mode_escalates_for_destructive_or_open_world_tools() {
        let destructive = tool_with_annotations(json!({ "destructiveHint": true }));
        let open_world = tool_with_annotations(json!({ "openWorldHint": true }));
        assert_eq!(
            permission_mode_for_mcp_tool(&destructive),
            PermissionMode::DangerFullAccess
        );
        assert_eq!(
            permission_mode_for_mcp_tool(&open_world),
            PermissionMode::DangerFullAccess
        );
    }

    #[test]
    fn permission_mode_defaults_to_workspace_write_without_hints() {
        let tool = tool_with_annotations(json!({}));
        assert_eq!(
            permission_mode_for_mcp_tool(&tool),
            PermissionMode::WorkspaceWrite
        );
    }


    #[test]
    fn wrapper_tool_definitions_preserve_expected_surface() {
        let defs = mcp_wrapper_tool_definitions();
        let names = defs.iter().map(|def| def.name.as_str()).collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["MCPTool", "ListMcpResourcesTool", "ReadMcpResourceTool"]
        );
    }


    fn text_content(text: &str) -> McpToolCallContent {
        McpToolCallContent {
            kind: "text".to_string(),
            data: std::collections::BTreeMap::from([("text".to_string(), json!(text))]),
        }
    }

    fn tool_result(content: Vec<McpToolCallContent>, is_error: Option<bool>) -> McpToolCallResult {
        McpToolCallResult {
            content,
            structured_content: None,
            is_error,
            meta: None,
        }
    }

    fn image_content(data: Option<&str>, mime: Option<&str>) -> McpToolCallContent {
        let mut map = std::collections::BTreeMap::new();
        if let Some(data) = data {
            map.insert("data".to_string(), json!(data));
        }
        if let Some(mime) = mime {
            map.insert("mimeType".to_string(), json!(mime));
        }
        McpToolCallContent {
            kind: "image".to_string(),
            data: map,
        }
    }

    #[test]
    fn render_tool_call_result_passes_success_through() {
        let result = tool_result(vec![text_content("hello")], Some(false));
        let rendered =
            render_tool_call_result("mcp__srv__do", &result).expect("successful result is Ok");
        assert!(
            rendered.contains("hello"),
            "a non-error result returns its serialized payload"
        );
    }

    #[test]
    fn render_tool_call_result_maps_tool_level_error_to_err() {
        // Regression: a JSON-RPC *success* carrying `isError: true` is a tool-level
        // failure. It must surface as a ToolError (model `is_error`, hook failure,
        // red TUI), not as a successful call whose JSON merely mentions an error.
        let result = tool_result(vec![text_content("disk is full")], Some(true));
        let error = render_tool_call_result("mcp__srv__write", &result)
            .expect_err("isError: true maps to Err");
        let message = error.to_string();
        assert!(message.contains("reported an error"));
        assert!(
            message.contains("disk is full"),
            "the tool's own text is surfaced as the error detail, got: {message}"
        );
    }

    /// The main session reaches MCP here rather than through the tool dispatch
    /// seam that caps builtin failures, and an error tool result is exempt from
    /// historical wire compression — so an uncapped server error would sit at
    /// full size in every later request until a full compaction.
    #[test]
    fn render_tool_call_result_caps_a_huge_tool_level_error() {
        let head = "SERVER-ERROR-HEAD";
        let tail = "SERVER-ERROR-TAIL";
        let detail = format!("{head}{}{tail}", "x".repeat(200_000));
        let result = tool_result(vec![text_content(&detail)], Some(true));

        let error = render_tool_call_result("mcp__srv__write", &result)
            .expect_err("isError: true maps to Err");
        let message = error.to_string();
        let length = message.chars().count();

        assert!(
            length < detail.chars().count() / 10,
            "a 200k-char server error must not ride the transcript whole, got {length} chars"
        );
        assert!(
            message.contains(head),
            "the head must survive so the failure stays legible: {message}"
        );
        assert!(
            message.contains(tail),
            "the tail must survive so the cause stays readable"
        );
        assert!(
            message.contains("retrieve_tool_output"),
            "the full server error must stay recoverable"
        );
    }

    /// The r27 pin. A SUCCESS result had no ceiling at all — r24a measured one
    /// at 5,049,795 chars — and unlike a `read_file` envelope it gets no wire
    /// compression, so it rides every later request whole. Head+tail, the same
    /// shape bash gets.
    #[test]
    fn render_tool_call_result_caps_a_huge_success() {
        let head = "JIRA-ROW-HEAD";
        let tail = "JIRA-ROW-TAIL";
        let payload = format!("{head}{}{tail}", "x".repeat(200_000));
        let result = tool_result(vec![text_content(&payload)], Some(false));

        let rendered = render_tool_call_result("mcp__atlassian__searchJiraIssuesUsingJql", &result)
            .expect("a non-error result is still Ok");
        let length = rendered.chars().count();

        assert!(
            length <= 30_000 + 256,
            "an over-cap MCP success must be bounded, got {length} chars"
        );
        assert!(rendered.contains(head), "the head survives");
        assert!(
            rendered.contains(tail),
            "the tail survives — a search's last rows are not noise"
        );
        assert!(
            rendered.contains("middle elided"),
            "the model must be able to tell it was cut: {}",
            rendered.chars().take(200).collect::<String>()
        );
    }

    /// The other half: 96.6% of this corpus's MCP results are under the cap and
    /// must come back byte-identical — the cap is a ceiling, not a rewrite.
    #[test]
    fn render_tool_call_result_leaves_a_small_success_byte_identical() {
        let result = tool_result(vec![text_content("two rows returned")], Some(false));
        let expected = serde_json::to_string_pretty(&result).expect("serialize");

        let rendered =
            render_tool_call_result("mcp__srv__do", &result).expect("successful result is Ok");

        assert_eq!(rendered, expected);
    }

    #[test]
    fn render_tool_call_result_error_without_text_falls_back_to_payload() {
        // An error result with no text block (structured-only) still propagates as
        // an error, carrying the full serialized payload so nothing is lost.
        let mut result = tool_result(Vec::new(), Some(true));
        result.structured_content = Some(json!({ "code": 42 }));
        let error =
            render_tool_call_result("mcp__srv__op", &result).expect_err("isError maps to Err");
        let message = error.to_string();
        assert!(
            message.contains("42"),
            "with no text content the full payload is surfaced, got: {message}"
        );
    }

    #[test]
    fn mcp_tool_result_text_joins_text_blocks_and_skips_non_text() {
        let image = McpToolCallContent {
            kind: "image".to_string(),
            data: std::collections::BTreeMap::from([("data".to_string(), json!("base64..."))]),
        };
        let result = tool_result(
            vec![text_content("line one"), image, text_content("line two")],
            Some(true),
        );
        assert_eq!(
            mcp_tool_result_text(&result).as_deref(),
            Some("line one\nline two"),
            "text blocks join with newlines; non-text blocks are skipped"
        );
    }

    #[test]
    fn mcp_tool_result_text_is_none_without_text_blocks() {
        let result = tool_result(Vec::new(), Some(true));
        assert_eq!(
            mcp_tool_result_text(&result),
            None,
            "no text content → None so the caller falls back to the full payload"
        );
    }


    #[test]
    fn take_tool_call_images_extracts_and_strips_image_blocks() {
        // Regression: an MCP tool's image content (a screenshot, a generated
        // image) must be surfaced as a real multimodal block, not left as base64
        // buried in the text result where the model cannot see it.
        let mut result = tool_result(
            vec![
                text_content("see screenshot"),
                image_content(Some("BASE64DATA"), Some("image/jpeg")),
            ],
            Some(false),
        );
        let images = take_tool_call_images(&mut result);
        assert_eq!(
            images,
            vec![("image/jpeg".to_string(), "BASE64DATA".to_string())],
            "image content is returned as (media_type, base64) for staging"
        );
        assert_eq!(
            result.content[0].data.get("text").and_then(Value::as_str),
            Some("see screenshot"),
            "text blocks are untouched"
        );
        let staged = result.content[1]
            .data
            .get("data")
            .and_then(Value::as_str)
            .expect("image data field present");
        assert_eq!(
            staged,
            super::STAGED_IMAGE_MARKER,
            "the base64 is replaced with a marker so it is not sent twice"
        );
        assert!(!staged.contains("BASE64DATA"));
    }

    #[test]
    fn take_tool_call_images_defaults_media_type_and_skips_dataless() {
        let mut result = tool_result(
            vec![
                image_content(Some("X"), None),         // no mimeType → default png
                image_content(None, Some("image/png")), // no data → unusable, skipped
            ],
            Some(false),
        );
        assert_eq!(
            take_tool_call_images(&mut result),
            vec![("image/png".to_string(), "X".to_string())],
            "missing mimeType defaults to image/png; a block without base64 is skipped"
        );
    }

    #[test]
    fn take_tool_call_images_ignores_results_without_images() {
        let mut result = tool_result(vec![text_content("plain")], Some(false));
        assert!(
            take_tool_call_images(&mut result).is_empty(),
            "a text-only result stages no images"
        );
        assert_eq!(
            result.content[0].data.get("text").and_then(Value::as_str),
            Some("plain"),
            "non-image content is left unchanged"
        );
    }

    /// Set `ZO_MCP_TOOL_TIMEOUT_SECS` for the test's lifetime, restoring the
    /// previous value on drop. Callers must hold [`crate::test_env_lock`] first
    /// so the mutation cannot race sibling tests that read the same var.
    struct TimeoutEnvGuard {
        previous: Option<std::ffi::OsString>,
    }

    impl TimeoutEnvGuard {
        fn capture() -> Self {
            Self {
                previous: std::env::var_os(MCP_TOOL_CALL_TIMEOUT_ENV),
            }
        }

        fn set(value: Option<&str>) {
            match value {
                Some(value) => std::env::set_var(MCP_TOOL_CALL_TIMEOUT_ENV, value),
                None => std::env::remove_var(MCP_TOOL_CALL_TIMEOUT_ENV),
            }
        }
    }

    impl Drop for TimeoutEnvGuard {
        fn drop(&mut self) {
            Self::set(self.previous.take().as_deref().and_then(|v| v.to_str()));
        }
    }

    #[test]
    fn mcp_tool_call_timeout_parses_the_env_override_and_falls_back_on_garbage() {
        let _env_lock = crate::test_env_lock();
        let _guard = TimeoutEnvGuard::capture();

        let default = std::time::Duration::from_secs(MCP_TOOL_CALL_TIMEOUT_SECS);

        TimeoutEnvGuard::set(None);
        assert_eq!(
            mcp_tool_call_timeout(),
            Some(default),
            "unset falls back to the {MCP_TOOL_CALL_TIMEOUT_SECS}s default"
        );

        TimeoutEnvGuard::set(Some("45"));
        assert_eq!(
            mcp_tool_call_timeout(),
            Some(std::time::Duration::from_secs(45)),
            "a parseable value wins over the default"
        );

        TimeoutEnvGuard::set(Some(" 30 "));
        assert_eq!(
            mcp_tool_call_timeout(),
            Some(std::time::Duration::from_secs(30)),
            "surrounding whitespace is trimmed before parsing"
        );

        TimeoutEnvGuard::set(Some("0"));
        assert_eq!(
            mcp_tool_call_timeout(),
            None,
            "0 disables the ceiling entirely"
        );

        for garbage in ["banana", "", "-5", "12.5"] {
            TimeoutEnvGuard::set(Some(garbage));
            assert_eq!(
                mcp_tool_call_timeout(),
                Some(default),
                "unparseable `{garbage}` falls back to the default instead of failing the call"
            );
        }
    }

    #[test]
    fn run_mcp_call_bounded_resolves_a_never_finishing_call_as_a_tool_error() {
        let _env_lock = crate::test_env_lock();
        let _guard = TimeoutEnvGuard::capture();
        TimeoutEnvGuard::set(Some("1"));

        let started = std::time::Instant::now();
        let error = run_mcp_call_bounded(
            "mcp__playwright__browser_resize",
            std::future::pending::<()>(),
        )
        .expect_err("a call that never resolves must hit the ceiling");
        let elapsed = started.elapsed();

        assert_eq!(
            error.to_string(),
            "MCP tool `mcp__playwright__browser_resize` timed out after 1s — the server may be \
             hung. The turn continues; consider an alternative approach.",
            "the timeout surfaces as a tool error result the model can act on"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "the ceiling must fire near its budget, not hang: {elapsed:?}"
        );
    }

    #[test]
    fn run_mcp_call_bounded_passes_completed_calls_through_unchanged() {
        let _env_lock = crate::test_env_lock();
        let _guard = TimeoutEnvGuard::capture();

        // Default ceiling, an explicit large ceiling, and the disabled ceiling
        // must all be pure pass-throughs for a call that actually completes.
        for setting in [None, Some("600"), Some("0")] {
            TimeoutEnvGuard::set(setting);
            assert_eq!(
                run_mcp_call_bounded("mcp__alpha__echo", async { Ok::<_, ()>("payload") })
                    .expect("a completed call never trips the ceiling"),
                Ok("payload"),
                "the wrapper returns the call's own value verbatim (ZO_MCP_TOOL_TIMEOUT_SECS={setting:?})"
            );
        }
    }
}
