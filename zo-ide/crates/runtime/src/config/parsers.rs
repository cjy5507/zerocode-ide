//! Parsing + merge helpers for `RuntimeConfig` JSON I/O.
//!
//! All `parse_*` and `optional_*` helpers consume a [`JsonValue`] node
//! and return either the typed sub-config or a [`ConfigError`] that
//! pinpoints the offending file + field.  `merge_*` helpers compose
//! the per-scope `ConfigEntry` chain (default → user → project →
//! environment).  Together they form the I/O boundary that
//! [`super::ConfigLoader::load`] composes.
//!
//! Kept `pub(super)` so the outer module can call them without
//! re-exposing them to other crates.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::json::JsonValue;
use crate::sandbox::{FilesystemIsolationMode, SandboxConfig};

use super::{
    ConfigError, ConfigSource, HookMatcher, HookRule, LspServerConfig, McpManagedProxyServerConfig,
    McpOAuthConfig, McpRemoteServerConfig, McpSdkServerConfig, McpServerConfig,
    McpStdioServerConfig, McpTransport, McpWebSocketServerConfig, OAuthConfig,
    ResolvedPermissionMode, RuntimeHookConfig, RuntimePermissionRuleConfig, RuntimePluginConfig,
    RuntimeReviewConfig, RuntimeShipConfig, ScopedLspServerConfig, ScopedMcpServerConfig,
};

/// Every key the `mcpServers` section is spelled with, named once.
///
/// The writer (`super::mcp_edit`) is the inverse of the parsers below, and a
/// second copy of these spellings would be a file `zo mcp add` writes and
/// `ConfigLoader` cannot read back. Both sides read this module instead.
pub(super) mod mcp_keys {
    /// The settings section itself.
    pub const SERVERS: &str = "mcpServers";
    /// The transport tag, and the tags it may carry.
    pub const TYPE: &str = "type";
    pub const STDIO: &str = "stdio";
    pub const SSE: &str = "sse";
    pub const HTTP: &str = "http";
    pub const WS: &str = "ws";
    pub const SDK: &str = "sdk";
    pub const MANAGED_PROXY: &str = "claudeai-proxy";
    /// Stdio.
    pub const COMMAND: &str = "command";
    pub const ARGS: &str = "args";
    pub const ENV: &str = "env";
    pub const TOOL_CALL_TIMEOUT_MS: &str = "toolCallTimeoutMs";
    /// Remote (http/sse/ws) and the managed proxy.
    pub const URL: &str = "url";
    pub const HEADERS: &str = "headers";
    pub const HEADERS_HELPER: &str = "headersHelper";
    pub const ID: &str = "id";
    /// The SDK server's own name.
    pub const NAME: &str = "name";
    /// The per-server OAuth block.
    pub const OAUTH: &str = "oauth";
    pub const CLIENT_ID: &str = "clientId";
    pub const CALLBACK_PORT: &str = "callbackPort";
    pub const AUTH_SERVER_METADATA_URL: &str = "authServerMetadataUrl";
    pub const XAA: &str = "xaa";
    /// The two switches that take a declared server back out.
    pub const DISABLED: &str = "disabled";
    pub const ENABLED: &str = "enabled";
    /// The record, beside the project document it gates, of the project-scoped
    /// servers an operator has consented to run.
    pub const TRUSTED_SERVERS_FILE: &str = "trusted-mcp-servers.json";
}

/// The server names one trusted-MCP-servers record holds, in the order the
/// document lists them.
///
/// Whether a record may be BELIEVED at all is
/// [`ConfigLoader`](super::ConfigLoader)'s gate — a tracked, symlinked or
/// nested-`.zo` file fails closed there. This is only the record's grammar: a
/// JSON array of names. `super::mcp_edit` records a name through this same
/// function, so a name `zo mcp add --project --trust` writes is a name the
/// next session reads back.
pub(super) fn parse_trusted_mcp_server_names(
    path: &Path,
    contents: &str,
) -> Result<Vec<String>, ConfigError> {
    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }
    let parsed = JsonValue::parse(contents)
        .map_err(|error| ConfigError::Parse(format!("{}: {error}", path.display())))?;
    let Some(entries) = parsed.as_array() else {
        return Err(ConfigError::Parse(format!(
            "{}: trusted MCP servers must be a JSON array of server names",
            path.display()
        )));
    };
    entries
        .iter()
        .map(|entry| {
            entry.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                ConfigError::Parse(format!(
                    "{}: trusted MCP server names must be strings",
                    path.display()
                ))
            })
        })
        .collect()
}

pub(super) fn merge_lsp_servers(
    target: &mut BTreeMap<String, ScopedLspServerConfig>,
    source: ConfigSource,
    root: &BTreeMap<String, JsonValue>,
    path: &Path,
) -> Result<(), ConfigError> {
    let Some(lsp_servers) = root.get("lspServers") else {
        return Ok(());
    };
    let servers = expect_object(lsp_servers, &format!("{}: lspServers", path.display()))?;
    for (name, value) in servers {
        let parsed =
            parse_lsp_server_config(value, &format!("{}: lspServers.{name}", path.display()))?;
        target.insert(
            name.clone(),
            ScopedLspServerConfig {
                scope: source,
                config: parsed,
            },
        );
    }
    Ok(())
}

impl McpServerConfig {
    #[must_use]
    pub fn transport(&self) -> McpTransport {
        match self {
            Self::Stdio(_) => McpTransport::Stdio,
            Self::Sse(_) => McpTransport::Sse,
            Self::Http(_) => McpTransport::Http,
            Self::Ws(_) => McpTransport::Ws,
            Self::Sdk(_) => McpTransport::Sdk,
            Self::ManagedProxy(_) => McpTransport::ManagedProxy,
        }
    }
}

pub(super) fn read_optional_json_object(
    path: &Path,
) -> Result<Option<BTreeMap<String, JsonValue>>, ConfigError> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ConfigError::Io(error)),
    };
    parse_json_object_contents(path, &contents).map(Some)
}

pub(super) fn parse_json_object_contents(
    path: &Path,
    contents: &str,
) -> Result<BTreeMap<String, JsonValue>, ConfigError> {
    if contents.trim().is_empty() {
        return Ok(BTreeMap::new());
    }

    let parsed = JsonValue::parse(contents)
        .map_err(|error| ConfigError::Parse(format!("{}: {error}", path.display())))?;
    let Some(object) = parsed.as_object() else {
        return Err(ConfigError::Parse(format!(
            "{}: top-level settings value must be a JSON object",
            path.display()
        )));
    };
    Ok(object.clone())
}

pub(super) fn merge_mcp_servers(
    target: &mut BTreeMap<String, ScopedMcpServerConfig>,
    source: ConfigSource,
    root: &BTreeMap<String, JsonValue>,
    path: &Path,
) -> Result<(), ConfigError> {
    let Some(mcp_servers) = root.get(mcp_keys::SERVERS) else {
        return Ok(());
    };
    let servers = expect_object(
        mcp_servers,
        &format!("{}: {}", path.display(), mcp_keys::SERVERS),
    )?;
    for (name, value) in servers {
        let context = mcp_server_context(path, name);
        if mcp_server_disabled(value, &context)? {
            target.remove(name);
            continue;
        }
        let parsed = parse_mcp_server_config(name, value, &context)?;
        target.insert(
            name.clone(),
            ScopedMcpServerConfig {
                scope: source,
                config: parsed,
            },
        );
    }
    Ok(())
}

fn mcp_server_disabled(value: &JsonValue, context: &str) -> Result<bool, ConfigError> {
    let object = expect_object(value, context)?;
    let disabled = optional_bool(object, mcp_keys::DISABLED, context)?.unwrap_or(false);
    let enabled = optional_bool(object, mcp_keys::ENABLED, context)?.unwrap_or(true);
    Ok(disabled || !enabled)
}

pub(super) fn parse_optional_model(root: &JsonValue) -> Option<String> {
    root.as_object()
        .and_then(|object| object.get("model"))
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
}

pub(super) fn parse_optional_hooks_config(
    root: &JsonValue,
) -> Result<RuntimeHookConfig, ConfigError> {
    let Some(object) = root.as_object() else {
        return Ok(RuntimeHookConfig::default());
    };
    parse_optional_hooks_config_object(object, "merged settings.hooks")
}

pub(super) fn parse_optional_hooks_config_object(
    object: &BTreeMap<String, JsonValue>,
    context: &str,
) -> Result<RuntimeHookConfig, ConfigError> {
    let Some(hooks_value) = object.get("hooks") else {
        return Ok(RuntimeHookConfig::default());
    };
    let hooks = expect_object(hooks_value, context)?;
    let f =
        |key: &str| -> Result<Vec<HookRule>, ConfigError> { parse_hook_rules(hooks, key, context) };
    // Claude Code's `Stop` event fires at the end of a turn — the same moment
    // Zo calls `TurnEnd`. A copied CC `settings.json` only ever names `Stop`,
    // so fold its rules into the turn-end bucket (after any native `TurnEnd`
    // rules) instead of silently dropping them.
    let mut turn_end = f("TurnEnd")?;
    turn_end.append(&mut f("Stop")?);
    Ok(RuntimeHookConfig {
        pre_tool_use: f("PreToolUse")?,
        post_tool_use: f("PostToolUse")?,
        post_tool_use_failure: f("PostToolUseFailure")?,
        session_start: f("SessionStart")?,
        session_end: f("SessionEnd")?,
        user_prompt_submit: f("UserPromptSubmit")?,
        pre_compact: f("PreCompact")?,
        post_compact: f("PostCompact")?,
        subagent_start: f("SubagentStart")?,
        subagent_stop: f("SubagentStop")?,
        turn_start: f("TurnStart")?,
        turn_end,
        permission_request: f("PermissionRequest")?,
        permission_denied: f("PermissionDenied")?,
        cwd_changed: f("CwdChanged")?,
        task_created: f("TaskCreated")?,
        task_completed: f("TaskCompleted")?,
        file_changed: f("FileChanged")?,
        notification: f("Notification")?,
        timeout_seconds: parse_hook_timeout_seconds(hooks, context)?,
    })
}

fn parse_hook_timeout_seconds(
    hooks: &BTreeMap<String, JsonValue>,
    context: &str,
) -> Result<Option<u64>, ConfigError> {
    let Some(seconds) = optional_u64(hooks, "timeoutSeconds", context)? else {
        return Ok(None);
    };
    if seconds == 0 {
        return Err(ConfigError::Parse(format!(
            "{context}: field timeoutSeconds must be greater than zero"
        )));
    }
    Ok(Some(seconds))
}

pub(super) fn validate_optional_hooks_config(
    root: &BTreeMap<String, JsonValue>,
    path: &Path,
) -> Result<(), ConfigError> {
    parse_optional_hooks_config_object(root, &format!("{}: hooks", path.display())).map(|_| ())
}

/// Parse one hook event's array into rules. Accepts both the flat Zo form
/// (`["cmd", …]` — each command runs for every tool) and the Claude Code nested
/// form (`[{ "matcher": "Bash|Edit", "hooks": [{ "type": "command", "command":
/// "…" }] }]`). A bare string is a no-matcher command; an object is a matcher
/// group whose `matcher` gates every command under its `hooks`.
fn parse_hook_rules(
    hooks: &BTreeMap<String, JsonValue>,
    key: &str,
    context: &str,
) -> Result<Vec<HookRule>, ConfigError> {
    let Some(value) = hooks.get(key) else {
        return Ok(Vec::new());
    };
    let Some(array) = value.as_array() else {
        return Err(ConfigError::Parse(format!(
            "{context}: field {key} must be an array"
        )));
    };
    let mut rules = Vec::new();
    for entry in array {
        if let Some(command) = entry.as_str() {
            rules.push(HookRule::any(command));
        } else if let Some(group) = entry.as_object() {
            parse_hook_matcher_group(group, key, context, &mut rules)?;
        } else {
            return Err(ConfigError::Parse(format!(
                "{context}: field {key} entries must be command strings or matcher objects"
            )));
        }
    }
    Ok(rules)
}

/// Expand one Claude-Code-style `{ matcher, hooks: [...] }` group into rules.
/// Each `hooks` entry is either a bare command string or `{ "command": "…" }`
/// (the `type` field, if present, is ignored — only `command` is run).
fn parse_hook_matcher_group(
    group: &BTreeMap<String, JsonValue>,
    key: &str,
    context: &str,
    rules: &mut Vec<HookRule>,
) -> Result<(), ConfigError> {
    let matcher = group
        .get("matcher")
        .and_then(JsonValue::as_str)
        .map_or(HookMatcher::Any, HookMatcher::parse);
    let Some(hook_entries) = group.get("hooks").and_then(JsonValue::as_array) else {
        return Err(ConfigError::Parse(format!(
            "{context}: field {key} matcher group must have a \"hooks\" array"
        )));
    };
    for hook in hook_entries {
        let command = if let Some(command) = hook.as_str() {
            command.to_owned()
        } else if let Some(object) = hook.as_object() {
            object
                .get("command")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| {
                    ConfigError::Parse(format!(
                        "{context}: field {key} hook entry must have a string \"command\""
                    ))
                })?
                .to_owned()
        } else {
            return Err(ConfigError::Parse(format!(
                "{context}: field {key} hook entry must be a string or object"
            )));
        };
        rules.push(HookRule::new(matcher.clone(), command));
    }
    Ok(())
}

pub(super) fn parse_optional_permission_rules(
    root: &JsonValue,
) -> Result<RuntimePermissionRuleConfig, ConfigError> {
    let Some(object) = root.as_object() else {
        return Ok(RuntimePermissionRuleConfig::default());
    };
    let Some(permissions) = object.get("permissions").and_then(JsonValue::as_object) else {
        return Ok(RuntimePermissionRuleConfig::default());
    };

    let allow = optional_string_array(permissions, "allow", "merged settings.permissions")?
        .unwrap_or_default();
    let deny = optional_string_array(permissions, "deny", "merged settings.permissions")?
        .unwrap_or_default();
    let ask = optional_string_array(permissions, "ask", "merged settings.permissions")?
        .unwrap_or_default();
    let rules = optional_string_array(permissions, "rules", "merged settings.permissions")?
        .unwrap_or_default();

    // The ordered `rules` form supersedes the category vectors entirely (last
    // match wins), so mixing the two in one config would silently disable the
    // allow/deny/ask lists — a security footgun. Reject it: callers use one form
    // or the other.
    let has_ordered = !rules.is_empty();
    let has_category = !allow.is_empty() || !deny.is_empty() || !ask.is_empty();
    if has_ordered && has_category {
        return Err(ConfigError::Parse(
            "merged settings.permissions: `rules` (ordered, last-match-wins) cannot be combined \
             with `allow`/`deny`/`ask` (category) — the ordered form supersedes them; use one form \
             or the other"
                .to_string(),
        ));
    }

    // Validate the OpenCode-compatible ordered form at load time so malformed
    // entries surface an actionable error instead of being silently ignored.
    for spec in &rules {
        crate::permissions::validate_decision_rule_spec(spec).map_err(|error| {
            ConfigError::Parse(format!("merged settings.permissions.rules: {error}"))
        })?;
    }

    Ok(RuntimePermissionRuleConfig::new(allow, deny, ask).with_rules(rules))
}

pub(super) fn parse_optional_plugin_config(
    root: &JsonValue,
) -> Result<RuntimePluginConfig, ConfigError> {
    let Some(object) = root.as_object() else {
        return Ok(RuntimePluginConfig::default());
    };

    let mut config = RuntimePluginConfig::default();
    if let Some(enabled_plugins) = object.get("enabledPlugins") {
        config.enabled_plugins = parse_bool_map(enabled_plugins, "merged settings.enabledPlugins")?;
    }

    let Some(plugins_value) = object.get("plugins") else {
        return Ok(config);
    };
    let plugins = expect_object(plugins_value, "merged settings.plugins")?;

    if let Some(enabled_value) = plugins.get("enabled") {
        config.enabled_plugins = parse_bool_map(enabled_value, "merged settings.plugins.enabled")?;
    }
    config.external_directories =
        optional_string_array(plugins, "externalDirectories", "merged settings.plugins")?
            .unwrap_or_default();
    config.install_root =
        optional_string(plugins, "installRoot", "merged settings.plugins")?.map(str::to_string);
    config.registry_path =
        optional_string(plugins, "registryPath", "merged settings.plugins")?.map(str::to_string);
    config.bundled_root =
        optional_string(plugins, "bundledRoot", "merged settings.plugins")?.map(str::to_string);
    Ok(config)
}

pub(super) fn parse_optional_review_config(
    root: &JsonValue,
) -> Result<RuntimeReviewConfig, ConfigError> {
    let Some(object) = root.as_object() else {
        return Ok(RuntimeReviewConfig::default());
    };
    let Some(review_value) = object.get("review") else {
        return Ok(RuntimeReviewConfig::default());
    };
    let review = expect_object(review_value, "merged settings.review")?;
    let auto_after_edits = optional_u64(review, "auto_after_edits", "merged settings.review")?;
    let auto_after_edits = match auto_after_edits {
        None => return Ok(RuntimeReviewConfig::default()),
        Some(0) => None,
        Some(value) => Some(u32::try_from(value).map_err(|_| {
            ConfigError::Parse(
                "merged settings.review: field auto_after_edits is out of range".to_string(),
            )
        })?),
    };
    Ok(RuntimeReviewConfig::new(auto_after_edits))
}

pub(super) fn parse_optional_ship_config(
    root: &JsonValue,
) -> Result<RuntimeShipConfig, ConfigError> {
    let Some(object) = root.as_object() else {
        return Ok(RuntimeShipConfig::default());
    };
    let Some(ship_value) = object.get("ship") else {
        return Ok(RuntimeShipConfig::default());
    };
    let ship = expect_object(ship_value, "merged settings.ship")?;
    let gates = optional_string_array(ship, "gates", "merged settings.ship")?
        .unwrap_or_else(|| RuntimeShipConfig::default().gates);
    if gates.iter().any(|gate| gate.trim().is_empty()) {
        return Err(ConfigError::Parse(
            "merged settings.ship: field gates must not contain empty commands".to_string(),
        ));
    }
    let deploy = optional_string(ship, "deploy", "merged settings.ship")?.map(str::to_string);
    if deploy.as_ref().is_some_and(|command| command.trim().is_empty()) {
        return Err(ConfigError::Parse(
            "merged settings.ship: field deploy must not be empty".to_string(),
        ));
    }
    Ok(RuntimeShipConfig { gates, deploy })
}

pub(super) fn parse_optional_permission_mode(
    root: &JsonValue,
) -> Result<Option<ResolvedPermissionMode>, ConfigError> {
    let Some(object) = root.as_object() else {
        return Ok(None);
    };
    if let Some(mode) = object.get("permissionMode").and_then(JsonValue::as_str) {
        return parse_permission_mode_label(mode, "merged settings.permissionMode").map(Some);
    }
    let Some(mode) = object
        .get("permissions")
        .and_then(JsonValue::as_object)
        .and_then(|permissions| permissions.get("defaultMode"))
        .and_then(JsonValue::as_str)
    else {
        return Ok(None);
    };
    parse_permission_mode_label(mode, "merged settings.permissions.defaultMode").map(Some)
}

pub(super) fn parse_permission_mode_label(
    mode: &str,
    context: &str,
) -> Result<ResolvedPermissionMode, ConfigError> {
    match mode {
        "default" | "plan" | "read-only" => Ok(ResolvedPermissionMode::ReadOnly),
        "acceptEdits" | "auto" | "workspace-write" => Ok(ResolvedPermissionMode::WorkspaceWrite),
        "dontAsk" | "danger-full-access" => Ok(ResolvedPermissionMode::DangerFullAccess),
        other => Err(ConfigError::Parse(format!(
            "{context}: unsupported permission mode {other}"
        ))),
    }
}

pub(super) fn parse_optional_sandbox_config(
    root: &JsonValue,
) -> Result<SandboxConfig, ConfigError> {
    let Some(object) = root.as_object() else {
        return Ok(SandboxConfig::default());
    };
    let Some(sandbox_value) = object.get("sandbox") else {
        return Ok(SandboxConfig::default());
    };
    let sandbox = expect_object(sandbox_value, "merged settings.sandbox")?;
    let filesystem_mode = optional_string(sandbox, "filesystemMode", "merged settings.sandbox")?
        .map(parse_filesystem_mode_label)
        .transpose()?;
    Ok(SandboxConfig {
        enabled: optional_bool(sandbox, "enabled", "merged settings.sandbox")?,
        namespace_restrictions: optional_bool(
            sandbox,
            "namespaceRestrictions",
            "merged settings.sandbox",
        )?,
        network_isolation: optional_bool(sandbox, "networkIsolation", "merged settings.sandbox")?,
        filesystem_mode,
        allowed_mounts: optional_string_array(sandbox, "allowedMounts", "merged settings.sandbox")?
            .unwrap_or_default(),
        macos_seatbelt: optional_bool(sandbox, "macosSeatbelt", "merged settings.sandbox")?,
    })
}

pub(super) fn parse_filesystem_mode_label(
    value: &str,
) -> Result<FilesystemIsolationMode, ConfigError> {
    match value {
        "off" => Ok(FilesystemIsolationMode::Off),
        "workspace-only" => Ok(FilesystemIsolationMode::WorkspaceOnly),
        "allow-list" => Ok(FilesystemIsolationMode::AllowList),
        other => Err(ConfigError::Parse(format!(
            "merged settings.sandbox.filesystemMode: unsupported filesystem mode {other}"
        ))),
    }
}

pub(super) fn parse_optional_oauth_config(
    root: &JsonValue,
    context: &str,
) -> Result<Option<OAuthConfig>, ConfigError> {
    let Some(oauth_value) = root.as_object().and_then(|object| object.get("oauth")) else {
        return Ok(None);
    };
    let object = expect_object(oauth_value, context)?;
    let client_id = expect_string(object, "clientId", context)?.to_string();
    let authorize_url = expect_string(object, "authorizeUrl", context)?.to_string();
    let token_url = expect_string(object, "tokenUrl", context)?.to_string();
    let callback_port = optional_u16(object, "callbackPort", context)?;
    let manual_redirect_url =
        optional_string(object, "manualRedirectUrl", context)?.map(str::to_string);
    let scopes = optional_string_array(object, "scopes", context)?.unwrap_or_default();
    Ok(Some(OAuthConfig {
        client_id,
        authorize_url,
        token_url,
        callback_port,
        manual_redirect_url,
        scopes,
        client_secret: None,
    }))
}

pub(super) fn parse_mcp_server_config(
    server_name: &str,
    value: &JsonValue,
    context: &str,
) -> Result<McpServerConfig, ConfigError> {
    let object = expect_object(value, context)?;
    let server_type = optional_string(object, mcp_keys::TYPE, context)?
        .unwrap_or_else(|| infer_mcp_server_type(object));
    match server_type {
        mcp_keys::STDIO => Ok(McpServerConfig::Stdio(McpStdioServerConfig {
            command: expect_string(object, mcp_keys::COMMAND, context)?.to_string(),
            args: optional_string_array(object, mcp_keys::ARGS, context)?.unwrap_or_default(),
            env: optional_string_map(object, mcp_keys::ENV, context)?.unwrap_or_default(),
            tool_call_timeout_ms: optional_u64(object, mcp_keys::TOOL_CALL_TIMEOUT_MS, context)?,
        })),
        mcp_keys::SSE => Ok(McpServerConfig::Sse(parse_mcp_remote_server_config(
            object, context,
        )?)),
        mcp_keys::HTTP => Ok(McpServerConfig::Http(parse_mcp_remote_server_config(
            object, context,
        )?)),
        mcp_keys::WS => Ok(McpServerConfig::Ws(McpWebSocketServerConfig {
            url: expect_string(object, mcp_keys::URL, context)?.to_string(),
            headers: optional_string_map(object, mcp_keys::HEADERS, context)?.unwrap_or_default(),
            headers_helper: optional_string(object, mcp_keys::HEADERS_HELPER, context)?
                .map(str::to_string),
        })),
        mcp_keys::SDK => Ok(McpServerConfig::Sdk(McpSdkServerConfig {
            name: expect_string(object, mcp_keys::NAME, context)?.to_string(),
        })),
        mcp_keys::MANAGED_PROXY => {
            Ok(McpServerConfig::ManagedProxy(McpManagedProxyServerConfig {
                url: expect_string(object, mcp_keys::URL, context)?.to_string(),
                id: expect_string(object, mcp_keys::ID, context)?.to_string(),
            }))
        }
        other => Err(ConfigError::Parse(format!(
            "{context}: unsupported MCP server type for {server_name}: {other}"
        ))),
    }
}

/// Where a complaint about one server points: the document and the exact key
/// path inside it. Shared so the parser and the writer name the same place.
pub(super) fn mcp_server_context(path: &Path, name: &str) -> String {
    format!("{}: {}.{name}", path.display(), mcp_keys::SERVERS)
}

pub(super) fn parse_lsp_server_config(
    value: &JsonValue,
    context: &str,
) -> Result<LspServerConfig, ConfigError> {
    let object = expect_object(value, context)?;
    Ok(LspServerConfig {
        language: expect_string(object, "language", context)?.to_string(),
        command: expect_string(object, "command", context)?.to_string(),
        args: optional_string_array(object, "args", context)?.unwrap_or_default(),
        env: optional_string_map(object, "env", context)?.unwrap_or_default(),
        root_path: optional_string(object, "rootPath", context)?.map(str::to_string),
        capabilities: optional_string_array(object, "capabilities", context)?.unwrap_or_else(
            || {
                vec![
                    "diagnostics".to_string(),
                    "hover".to_string(),
                    "definition".to_string(),
                    "references".to_string(),
                    "symbols".to_string(),
                ]
            },
        ),
    })
}

pub(super) fn infer_mcp_server_type(object: &BTreeMap<String, JsonValue>) -> &'static str {
    if object.contains_key(mcp_keys::URL) {
        mcp_keys::HTTP
    } else {
        mcp_keys::STDIO
    }
}

pub(super) fn parse_mcp_remote_server_config(
    object: &BTreeMap<String, JsonValue>,
    context: &str,
) -> Result<McpRemoteServerConfig, ConfigError> {
    Ok(McpRemoteServerConfig {
        url: expect_string(object, mcp_keys::URL, context)?.to_string(),
        headers: optional_string_map(object, mcp_keys::HEADERS, context)?.unwrap_or_default(),
        headers_helper: optional_string(object, mcp_keys::HEADERS_HELPER, context)?
            .map(str::to_string),
        oauth: parse_optional_mcp_oauth_config(object, context)?,
    })
}

pub(super) fn parse_optional_mcp_oauth_config(
    object: &BTreeMap<String, JsonValue>,
    context: &str,
) -> Result<Option<McpOAuthConfig>, ConfigError> {
    let Some(value) = object.get(mcp_keys::OAUTH) else {
        return Ok(None);
    };
    let oauth = expect_object(value, &format!("{context}.{}", mcp_keys::OAUTH))?;
    Ok(Some(McpOAuthConfig {
        client_id: optional_string(oauth, mcp_keys::CLIENT_ID, context)?.map(str::to_string),
        callback_port: optional_u16(oauth, mcp_keys::CALLBACK_PORT, context)?,
        auth_server_metadata_url: optional_string(
            oauth,
            mcp_keys::AUTH_SERVER_METADATA_URL,
            context,
        )?
        .map(str::to_string),
        xaa: optional_bool(oauth, mcp_keys::XAA, context)?,
    }))
}

pub(super) fn expect_object<'a>(
    value: &'a JsonValue,
    context: &str,
) -> Result<&'a BTreeMap<String, JsonValue>, ConfigError> {
    value
        .as_object()
        .ok_or_else(|| ConfigError::Parse(format!("{context}: expected JSON object")))
}

pub(super) fn expect_string<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    key: &str,
    context: &str,
) -> Result<&'a str, ConfigError> {
    object
        .get(key)
        .and_then(JsonValue::as_str)
        .ok_or_else(|| ConfigError::Parse(format!("{context}: missing string field {key}")))
}

pub(super) fn optional_string<'a>(
    object: &'a BTreeMap<String, JsonValue>,
    key: &str,
    context: &str,
) -> Result<Option<&'a str>, ConfigError> {
    match object.get(key) {
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| ConfigError::Parse(format!("{context}: field {key} must be a string"))),
        None => Ok(None),
    }
}

pub(super) fn optional_bool(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
    context: &str,
) -> Result<Option<bool>, ConfigError> {
    match object.get(key) {
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| ConfigError::Parse(format!("{context}: field {key} must be a boolean"))),
        None => Ok(None),
    }
}

pub(super) fn optional_u16(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
    context: &str,
) -> Result<Option<u16>, ConfigError> {
    match object.get(key) {
        Some(value) => {
            let Some(number) = value.as_i64() else {
                return Err(ConfigError::Parse(format!(
                    "{context}: field {key} must be an integer"
                )));
            };
            let number = u16::try_from(number).map_err(|_| {
                ConfigError::Parse(format!("{context}: field {key} is out of range"))
            })?;
            Ok(Some(number))
        }
        None => Ok(None),
    }
}

pub(super) fn optional_u64(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
    context: &str,
) -> Result<Option<u64>, ConfigError> {
    match object.get(key) {
        Some(value) => {
            let Some(number) = value.as_i64() else {
                return Err(ConfigError::Parse(format!(
                    "{context}: field {key} must be a non-negative integer"
                )));
            };
            let number = u64::try_from(number).map_err(|_| {
                ConfigError::Parse(format!("{context}: field {key} is out of range"))
            })?;
            Ok(Some(number))
        }
        None => Ok(None),
    }
}

pub(super) fn parse_bool_map(
    value: &JsonValue,
    context: &str,
) -> Result<BTreeMap<String, bool>, ConfigError> {
    let Some(map) = value.as_object() else {
        return Err(ConfigError::Parse(format!(
            "{context}: expected JSON object"
        )));
    };
    map.iter()
        .map(|(key, value)| {
            value
                .as_bool()
                .map(|enabled| (key.clone(), enabled))
                .ok_or_else(|| {
                    ConfigError::Parse(format!("{context}: field {key} must be a boolean"))
                })
        })
        .collect()
}

pub(super) fn optional_string_array(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
    context: &str,
) -> Result<Option<Vec<String>>, ConfigError> {
    match object.get(key) {
        Some(value) => {
            let Some(array) = value.as_array() else {
                return Err(ConfigError::Parse(format!(
                    "{context}: field {key} must be an array"
                )));
            };
            array
                .iter()
                .map(|item| {
                    item.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                        ConfigError::Parse(format!(
                            "{context}: field {key} must contain only strings"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Some)
        }
        None => Ok(None),
    }
}

pub(super) fn optional_string_map(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
    context: &str,
) -> Result<Option<BTreeMap<String, String>>, ConfigError> {
    match object.get(key) {
        Some(value) => {
            let Some(map) = value.as_object() else {
                return Err(ConfigError::Parse(format!(
                    "{context}: field {key} must be an object"
                )));
            };
            map.iter()
                .map(|(entry_key, entry_value)| {
                    entry_value
                        .as_str()
                        .map(|text| (entry_key.clone(), text.to_string()))
                        .ok_or_else(|| {
                            ConfigError::Parse(format!(
                                "{context}: field {key} must contain only string values"
                            ))
                        })
                })
                .collect::<Result<BTreeMap<_, _>, _>>()
                .map(Some)
        }
        None => Ok(None),
    }
}

pub(super) fn deep_merge_objects(
    target: &mut BTreeMap<String, JsonValue>,
    source: &BTreeMap<String, JsonValue>,
) {
    for (key, value) in source {
        match (target.get_mut(key), value) {
            (Some(JsonValue::Object(existing)), JsonValue::Object(incoming)) => {
                deep_merge_objects(existing, incoming);
            }
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

pub(super) fn extend_unique<T: Clone + PartialEq>(target: &mut Vec<T>, values: &[T]) {
    for value in values {
        push_unique(target, value.clone());
    }
}

pub(super) fn push_unique<T: PartialEq>(target: &mut Vec<T>, value: T) {
    if !target.iter().any(|existing| existing == &value) {
        target.push(value);
    }
}

#[cfg(test)]
mod hook_alias_tests {
    use super::parse_optional_hooks_config;
    use crate::json::JsonValue;

    fn commands(rules: &[super::HookRule]) -> Vec<&str> {
        rules.iter().map(super::HookRule::command).collect()
    }

    /// A copied Claude Code `settings.json` only names `Stop`; it must land in
    /// the Zo `TurnEnd` bucket instead of being silently dropped.
    #[test]
    fn claude_code_stop_hook_folds_into_turn_end() {
        let root = JsonValue::parse(r#"{"hooks":{"Stop":["notify-done"]}}"#).expect("valid json");
        let config = parse_optional_hooks_config(&root).expect("hooks parse");
        assert_eq!(commands(config.turn_end()), vec!["notify-done"]);
    }

    /// Native `TurnEnd` rules keep precedence; folded `Stop` rules append after.
    #[test]
    fn turn_end_precedes_folded_stop_rules() {
        let root = JsonValue::parse(r#"{"hooks":{"TurnEnd":["native"],"Stop":["legacy"]}}"#)
            .expect("valid json");
        let config = parse_optional_hooks_config(&root).expect("hooks parse");
        assert_eq!(commands(config.turn_end()), vec!["native", "legacy"]);
    }

    #[test]
    fn sandbox_macos_seatbelt_setting_is_parsed() {
        let root = JsonValue::parse(r#"{"sandbox":{"macosSeatbelt":true}}"#)
            .expect("valid json");
        let config = super::parse_optional_sandbox_config(&root).expect("sandbox parse");
        assert_eq!(config.macos_seatbelt, Some(true));
    }
}
