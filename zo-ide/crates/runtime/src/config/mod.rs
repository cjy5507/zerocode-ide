mod mcp_edit;
mod parsers;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Duration;

use core_types::paths::{write_private_file, ParentDirPolicy};

use crate::json::JsonValue;
use crate::sandbox::SandboxConfig;

pub use self::mcp_edit::{remove_mcp_server, trust_mcp_server, write_mcp_server, McpEdit};

use self::parsers::mcp_keys;
use self::parsers::{
    deep_merge_objects, extend_unique, merge_lsp_servers, merge_mcp_servers, optional_string_map,
    parse_optional_hooks_config, parse_optional_model, parse_optional_oauth_config,
    parse_json_object_contents, parse_optional_permission_mode, parse_optional_permission_rules,
    parse_optional_plugin_config, parse_optional_review_config, parse_optional_sandbox_config,
    parse_optional_ship_config, parse_trusted_mcp_server_names, read_optional_json_object,
    validate_optional_hooks_config,
};

/// Schema name advertised by generated settings files.
pub const ZO_SETTINGS_SCHEMA_NAME: &str = "SettingsSchema";

/// The project's record of the MCP servers an operator has consented to run,
/// as a person types it. The one spelling: the parser that reads the record,
/// the writer that records a name in it, and the message that explains the
/// gate all name this constant.
pub const TRUSTED_MCP_SERVERS_FILE: &str = mcp_keys::TRUSTED_SERVERS_FILE;

/// Origin of a loaded settings file in the configuration precedence chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfigSource {
    User,
    Project,
    Local,
}

/// Effective permission mode after decoding config values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedPermissionMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

/// A discovered config file and the scope it contributes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEntry {
    pub source: ConfigSource,
    pub path: PathBuf,
}

/// Fully merged runtime configuration plus parsed feature-specific views.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    merged: BTreeMap<String, JsonValue>,
    loaded_entries: Vec<ConfigEntry>,
    feature_config: RuntimeFeatureConfig,
}

/// Parsed plugin-related settings extracted from runtime config.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimePluginConfig {
    enabled_plugins: BTreeMap<String, bool>,
    external_directories: Vec<String>,
    install_root: Option<String>,
    registry_path: Option<String>,
    bundled_root: Option<String>,
}

/// Parsed review-related settings extracted from runtime config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReviewConfig {
    auto_after_edits: Option<NonZeroU32>,
}

impl Default for RuntimeReviewConfig {
    fn default() -> Self {
        Self::new(Some(1))
    }
}

/// Trusted user-configured commands used by `/ship`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeShipConfig {
    gates: Vec<String>,
    deploy: Option<String>,
}

impl Default for RuntimeShipConfig {
    fn default() -> Self {
        Self {
            gates: vec![
                "cargo test --workspace --locked".to_string(),
                "cargo clippy --workspace --all-targets --locked -- -D warnings".to_string(),
                "git diff --check".to_string(),
            ],
            deploy: None,
        }
    }
}

/// Parsed `secondBrain` settings: where the vault is and whether a session may
/// write what it learned back into it.
///
/// Absent settings mean "no vault, write-back irrelevant" — the whole feature
/// is then off, and its cost is one absent map key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSecondBrainConfig {
    vault: Option<String>,
    write_back: bool,
}

impl Default for RuntimeSecondBrainConfig {
    fn default() -> Self {
        Self {
            vault: None,
            // Reading a vault and never adding to it makes the second brain a
            // library instead of a brain, so the default is on. The switch
            // exists for a vault somebody wants zo to consult read-only.
            write_back: true,
        }
    }
}

impl RuntimeSecondBrainConfig {
    /// `secondBrain.vault`, trimmed, when it is a non-empty string.
    #[must_use]
    pub fn vault(&self) -> Option<&str> {
        self.vault.as_deref()
    }

    /// `secondBrain.writeBack` — whether session-end promotion may append to
    /// the vault. Defaults to `true`.
    #[must_use]
    pub const fn write_back(&self) -> bool {
        self.write_back
    }
}

/// Structured feature configuration consumed by runtime subsystems.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // each bool is an independent user-facing feature gate, not a state machine
pub struct RuntimeFeatureConfig {
    hooks: RuntimeHookConfig,
    plugins: RuntimePluginConfig,
    review: RuntimeReviewConfig,
    ship: RuntimeShipConfig,
    mcp: McpConfigCollection,
    lsp: LspConfigCollection,
    oauth: Option<OAuthConfig>,
    model: Option<String>,
    permission_mode: Option<ResolvedPermissionMode>,
    permission_rules: RuntimePermissionRuleConfig,
    sandbox: SandboxConfig,
    /// Extra environment variables declared under settings.json `env`, injected
    /// into every zo-spawned subprocess (bash tool, hooks, powershell). CC
    /// parity: `settings.env` is a session-wide environment overlay. Previously
    /// parsed for display (`/config env`) yet never reaching any child — a
    /// silent footgun where a declared `env` block did nothing.
    env: BTreeMap<String, String>,
    auto_compact_enabled: bool,
    /// Settings `autoCompactThresholdPercent`: user override for the full
    /// auto-compaction ceiling as a percent of the model context window,
    /// clamped to 20–95 at parse. `None` keeps the model-family policy
    /// defaults (80% Claude, 85% otherwise).
    auto_compact_threshold_percent: Option<u8>,
    auto_memory_enabled: bool,
    auto_dream_enabled: bool,
    /// Settings `autoImproveProposalsEnabled`: opt-in. When on (and dream
    /// automation is on), the startup self-improve preflight does not stop at
    /// the read-only fusion report — it runs the headless generator and parks a
    /// gated patch **proposal** (`state: proposed`, quarantined) for review.
    /// Applying still requires an explicit `/improve apply`; this only automates
    /// the propose half a human used to trigger with `/improve`. Defaults off
    /// because it spends a minutes-long `zo -p` turn in the background.
    auto_improve_proposals_enabled: bool,
    team_inbox_digest_enabled: bool,
    team_inbox_digest_max_updates: usize,
    /// Settings `recallHintEnabled`: whether a turn that references a past
    /// conversation ("earlier", "그때", …) gets a one-line reminder pointing at
    /// `session_recall`. Defaults on; the hint never recalls anything itself, so
    /// disabling it only silences the affordance.
    recall_hint_enabled: bool,
    checkpoint_durable: bool,
    /// Settings `tui.inlineMode`: render the interactive TUI in the primary
    /// screen and emit settled transcript content into native scrollback.
    ///
    /// Three-state on purpose. Inline is now the default presentation, so an
    /// absent key (`None`) must mean "take the default" rather than "the user
    /// asked for fullscreen" — while an explicit `false` from someone who set
    /// it back when fullscreen was the default still pins fullscreen.
    tui_inline_mode: Option<bool>,
    /// Settings `secondBrain`: the knowledge vault zo consults and adds to.
    second_brain: RuntimeSecondBrainConfig,
}

impl Default for RuntimeFeatureConfig {
    fn default() -> Self {
        Self {
            hooks: RuntimeHookConfig::default(),
            plugins: RuntimePluginConfig::default(),
            review: RuntimeReviewConfig::default(),
            ship: RuntimeShipConfig::default(),
            mcp: McpConfigCollection::default(),
            lsp: LspConfigCollection::default(),
            oauth: None,
            model: None,
            permission_mode: None,
            permission_rules: RuntimePermissionRuleConfig::default(),
            sandbox: SandboxConfig::default(),
            env: BTreeMap::new(),
            auto_compact_enabled: true,
            auto_compact_threshold_percent: None,
            auto_memory_enabled: true,
            auto_dream_enabled: true,
            auto_improve_proposals_enabled: false,
            team_inbox_digest_enabled: true,
            team_inbox_digest_max_updates: 8,
            recall_hint_enabled: true,
            checkpoint_durable: false,
            tui_inline_mode: None,
            second_brain: RuntimeSecondBrainConfig::default(),
        }
    }
}

/// Tool-name matcher gating a hook command, mirroring Claude Code's matcher
/// semantics so an existing CC hook config routes identically here:
///
/// * `Any` — `"*"`, `""`, or omitted: runs for every tool.
/// * `Exact` — matcher contains only `[A-Za-z0-9_|]`, read as a `|`-separated
///   list of literal tool names (`"Bash"`, `"Edit|Write"`).
/// * `Regex` — anything else is a regular expression over the tool name
///   (`"mcp__.*"`, `"^Notebook"`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HookMatcher {
    #[default]
    Any,
    Exact(Vec<String>),
    Regex(String),
}

impl HookMatcher {
    /// Classify a raw matcher string from settings.json.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        if raw.is_empty() || raw == "*" {
            return Self::Any;
        }
        if raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '|')
        {
            return Self::Exact(
                raw.split('|')
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned)
                    .collect(),
            );
        }
        Self::Regex(raw.to_owned())
    }

    /// Whether this matcher accepts `tool_name`. An invalid regex matches
    /// nothing — the pattern is user-supplied, so we fail closed rather than
    /// panic. Compilation is per call, which is fine: hook command lists are
    /// tiny and fire rarely, and caching would force a non-`Eq` field onto
    /// every config clone.
    #[must_use]
    pub fn matches(&self, tool_name: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(names) => names.iter().any(|name| name == tool_name),
            Self::Regex(pattern) => {
                regex::Regex::new(pattern).is_ok_and(|re| re.is_match(tool_name))
            }
        }
    }
}

/// A hook command paired with the tool-name matcher that gates it. A flat
/// command string in settings.json parses to [`HookRule::any`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRule {
    matcher: HookMatcher,
    command: String,
}

impl HookRule {
    /// A rule that runs for every tool (the flat-list / no-matcher form).
    #[must_use]
    pub fn any(command: impl Into<String>) -> Self {
        Self {
            matcher: HookMatcher::Any,
            command: command.into(),
        }
    }

    /// A rule gated by `matcher`.
    #[must_use]
    pub fn new(matcher: HookMatcher, command: impl Into<String>) -> Self {
        Self {
            matcher,
            command: command.into(),
        }
    }

    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    #[must_use]
    pub fn matcher(&self) -> &HookMatcher {
        &self.matcher
    }
}

impl std::fmt::Display for HookRule {
    /// `command` for an unmatched rule, `[Bash|Edit] command` for an exact
    /// matcher, `[/mcp__.*/ ] command` for a regex — so `zo hooks` shows
    /// which tools each command is gated to.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.matcher {
            HookMatcher::Any => write!(f, "{}", self.command),
            HookMatcher::Exact(names) => write!(f, "[{}] {}", names.join("|"), self.command),
            HookMatcher::Regex(pattern) => write!(f, "[/{pattern}/] {}", self.command),
        }
    }
}

/// Hook rules grouped by lifecycle stage.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeHookConfig {
    pre_tool_use: Vec<HookRule>,
    post_tool_use: Vec<HookRule>,
    post_tool_use_failure: Vec<HookRule>,
    session_start: Vec<HookRule>,
    session_end: Vec<HookRule>,
    user_prompt_submit: Vec<HookRule>,
    pre_compact: Vec<HookRule>,
    post_compact: Vec<HookRule>,
    subagent_start: Vec<HookRule>,
    subagent_stop: Vec<HookRule>,
    turn_start: Vec<HookRule>,
    turn_end: Vec<HookRule>,
    permission_request: Vec<HookRule>,
    permission_denied: Vec<HookRule>,
    cwd_changed: Vec<HookRule>,
    task_created: Vec<HookRule>,
    task_completed: Vec<HookRule>,
    file_changed: Vec<HookRule>,
    notification: Vec<HookRule>,
    timeout_seconds: Option<u64>,
}

/// Raw permission rule lists grouped by allow, deny, and ask behavior.
///
/// `allow`/`deny`/`ask` are the legacy category vectors (first match within a
/// vector, with category precedence deny > ask > allow). `rules` is the optional
/// OpenCode-compatible ordered form (`"bash(git *)=allow"`); when non-empty the
/// last matching ordered rule wins and supersedes the category vectors.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimePermissionRuleConfig {
    allow: Vec<String>,
    deny: Vec<String>,
    ask: Vec<String>,
    rules: Vec<String>,
}

/// Collection of configured MCP servers after scope-aware merging.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpConfigCollection {
    servers: BTreeMap<String, ScopedMcpServerConfig>,
    /// Project-scoped servers the supply-chain trust gate skipped (see
    /// [`UntrustedMcpServer`]). Kept separate from `servers` so they never load,
    /// but surfaced (e.g. by `/mcp`) so a deliberately-gated server is an
    /// actionable hint instead of a silently-missing one.
    untrusted: Vec<UntrustedMcpServer>,
}

/// A project-scoped MCP server declared in a repo-committed settings document
/// (`.zo/mcp.json` or `.zo/settings.json`) but skipped by the
/// supply-chain trust gate: it is neither listed in
/// `.zo/trusted-mcp-servers.json` nor covered by `enableAllProjectMcpServers`.
/// These never connect (by design — a hostile clone must not auto-spawn an MCP
/// command), but are surfaced so the user knows *why* the server is missing and
/// how to enable it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedMcpServer {
    /// Configured server name (the `mcpServers` key).
    pub name: String,
    /// The settings document that declared it.
    pub path: PathBuf,
}

/// Collection of configured LSP servers after scope-aware merging.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LspConfigCollection {
    servers: BTreeMap<String, ScopedLspServerConfig>,
}

/// MCP server config paired with the scope that defined it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedMcpServerConfig {
    pub scope: ConfigSource,
    pub config: McpServerConfig,
}

/// LSP server config paired with the scope that defined it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedLspServerConfig {
    pub scope: ConfigSource,
    pub config: LspServerConfig,
}

/// Transport families supported by configured MCP servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    Stdio,
    Sse,
    Http,
    Ws,
    Sdk,
    ManagedProxy,
}

/// Scope-normalized MCP server configuration variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerConfig {
    Stdio(McpStdioServerConfig),
    Sse(McpRemoteServerConfig),
    Http(McpRemoteServerConfig),
    Ws(McpWebSocketServerConfig),
    Sdk(McpSdkServerConfig),
    ManagedProxy(McpManagedProxyServerConfig),
}

/// Configuration for an MCP server launched as a local stdio process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStdioServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub tool_call_timeout_ms: Option<u64>,
}

/// Configuration for an MCP server reached over HTTP or SSE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRemoteServerConfig {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub headers_helper: Option<String>,
    pub oauth: Option<McpOAuthConfig>,
}

/// Configuration for an MCP server reached over WebSocket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpWebSocketServerConfig {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub headers_helper: Option<String>,
}

/// Configuration for an MCP server addressed through an SDK name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSdkServerConfig {
    pub name: String,
}

/// Configuration for an MCP managed-proxy endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpManagedProxyServerConfig {
    pub url: String,
    pub id: String,
}

/// Configuration for an LSP server launched as a local stdio process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspServerConfig {
    pub language: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub root_path: Option<String>,
    pub capabilities: Vec<String>,
}

/// OAuth overrides associated with a remote MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthConfig {
    pub client_id: Option<String>,
    pub callback_port: Option<u16>,
    pub auth_server_metadata_url: Option<String>,
    pub xaa: Option<bool>,
}

// Re-export `OAuthConfig` from core-types so that existing consumers of
// `runtime::OAuthConfig` (via config) continue to compile unchanged.
pub use core_types::OAuthConfig;

/// Errors raised while reading or parsing runtime configuration files.
#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(String),
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Parse(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Discovers config files and merges them into a [`RuntimeConfig`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLoader {
    cwd: PathBuf,
    config_home: PathBuf,
    config_roots: Vec<PathBuf>,
    mcp_config: Option<PathBuf>,
}

impl ConfigLoader {
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>, config_home: impl Into<PathBuf>) -> Self {
        let config_home = config_home.into();
        Self {
            cwd: cwd.into(),
            config_roots: vec![config_home.clone()],
            config_home,
            mcp_config: None,
        }
    }

    #[must_use]
    pub fn default_for(cwd: impl Into<PathBuf>) -> Self {
        let cwd = cwd.into();
        let mut config_roots = zo_global_config_roots();
        if config_roots.is_empty() {
            config_roots.push(default_config_home());
        }
        let config_home = config_roots[0].clone();
        Self {
            cwd,
            config_home,
            config_roots,
            mcp_config: None,
        }
    }

    /// Add one explicit MCP config file from `--mcp-config`.
    ///
    /// The file is parsed with the normal settings schema, but only its
    /// top-level `mcpServers` section is merged. That keeps the flag honest to
    /// its name and prevents an MCP-only command-line flag from silently
    /// overriding unrelated settings such as `model`, `hooks`, or `permissions`.
    #[must_use]
    pub fn with_mcp_config(mut self, path: impl Into<PathBuf>) -> Self {
        self.mcp_config = Some(path.into());
        self
    }

    #[must_use]
    pub fn config_home(&self) -> &Path {
        &self.config_home
    }

    /// The workspace this loader discovers project documents under.
    ///
    /// A writer that edits `<cwd>/.zo/settings.json` asks the loader where
    /// that is rather than joining its own `.zo`, so the file it writes is the
    /// file [`discover`](Self::discover) reads back.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Canonical global config roots in priority order (highest first); the
    /// primary [`config_home`](Self::config_home) is the first entry.
    #[must_use]
    pub fn config_roots(&self) -> &[PathBuf] {
        &self.config_roots
    }

    #[must_use]
    pub fn discover(&self) -> Vec<ConfigEntry> {
        let path = self.cwd.join(".zo").join("settings.local.json");
        self.discover_with_local_scope(local_settings_scope(&self.cwd, &path))
    }

    fn discover_with_local_scope(&self, local_scope: ConfigSource) -> Vec<ConfigEntry> {
        // The canonical roots are stored highest-priority first. Read them in
        // reverse so lower-priority `$HOME/.zo` defaults merge first and
        // `ZO_CONFIG_HOME` wins, while `config_home` remains the primary write
        // location.
        let mut candidates = self
            .config_roots
            .iter()
            .rev()
            .map(|root| ConfigEntry {
                source: ConfigSource::User,
                path: root.join("settings.json"),
            })
            .collect::<Vec<_>>();
        candidates.extend([
            ConfigEntry {
                source: ConfigSource::Project,
                path: self.cwd.join(".zo").join("settings.json"),
            },
            // `settings.local.json` is Local only when one securely retained
            // snapshot proves it is the operator's uncommitted file. `load`
            // supplies that same scope while reusing the snapshot contents.
            ConfigEntry {
                source: local_scope,
                path: self.cwd.join(".zo").join("settings.local.json"),
            },
        ]);
        let mut entries = dedupe_discovered_config_entries(candidates);
        // `--settings <file>` (CC parity): an extra settings document merged
        // last, so its keys take the highest precedence for this process.
        // Keep this explicit override out of the discovery dedupe so passing the
        // same file deliberately still gives it CLI precedence.
        if let Some(path) = cli_overrides().settings_file.clone() {
            entries.push(ConfigEntry {
                source: ConfigSource::Local,
                path,
            });
        }
        entries
    }

    fn project_mcp_entry(&self) -> ConfigEntry {
        ConfigEntry {
            source: ConfigSource::Project,
            path: self.cwd.join(".zo").join("mcp.json"),
        }
    }

    /// Path of the per-project record of MCP servers the user has trusted.
    ///
    /// Public for the same reason [`cwd`](Self::cwd) is: `zo mcp add
    /// --project --trust` records a name in the very file this loader reads
    /// back, rather than joining a `.zo` of its own.
    #[must_use]
    pub fn trusted_mcp_servers_path(&self) -> PathBuf {
        self.cwd.join(".zo").join(mcp_keys::TRUSTED_SERVERS_FILE)
    }

    /// Names of project `.zo/mcp.json` servers the user has explicitly trusted.
    ///
    /// The record is a JSON array under `.zo/trusted-mcp-servers.json`, but it
    /// is honored only when it is the operator's real, uncommitted local file.
    /// A tracked file, symlink, nested `.zo` repository, or canonical-path
    /// mismatch fails closed to an empty allowlist so a repository cannot
    /// self-authorize its own MCP command.
    fn trusted_project_mcp_servers(&self) -> Result<BTreeSet<String>, ConfigError> {
        let path = self.trusted_mcp_servers_path();
        let Some(contents) = trusted_uncommitted_zo_file_snapshot(
            &self.cwd,
            &path,
            mcp_keys::TRUSTED_SERVERS_FILE,
        ) else {
            return Ok(BTreeSet::new());
        };
        parse_trusted_mcp_server_names(&path, &contents)
            .map(|names| names.into_iter().collect())
    }

    /// Replace the process-wide CLI config overrides. Called once at
    /// argument-parse time, before any loader runs.
    pub fn set_cli_overrides(overrides: CliConfigOverrides) {
        *cli_overrides_cell()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = overrides;
    }

    /// Return the explicit `--settings` document, if one was supplied.
    ///
    /// Startup preferences use this narrow accessor so model/effort selection
    /// observes the same highest-precedence document as the runtime config
    /// loader without accidentally broadening project preferences to unrelated
    /// user-global settings.
    #[must_use]
    pub fn cli_settings_file() -> Option<PathBuf> {
        cli_overrides().settings_file
    }

    // One cohesive merge pass over the discovered settings documents (scope
    // precedence, MCP/LSP/plugin/hook gating, feature parsing); splitting a stage
    // out would scatter the shared `merged`/`mcp_servers`/trust-snapshot state.
    #[allow(clippy::too_many_lines)]
    pub fn load(&self) -> Result<RuntimeConfig, ConfigError> {
        let mut merged = BTreeMap::new();
        let mut loaded_entries = Vec::new();
        let mut mcp_servers = BTreeMap::new();
        let mut untrusted_project_mcp: Vec<UntrustedMcpServer> = Vec::new();
        let mut lsp_servers = BTreeMap::new();

        let local_settings_path = self.cwd.join(".zo").join("settings.local.json");
        let local_settings_contents = trusted_uncommitted_zo_file_snapshot(
            &self.cwd,
            &local_settings_path,
            "settings.local.json",
        );
        let local_settings_source = if local_settings_contents.is_some() {
            ConfigSource::Local
        } else {
            ConfigSource::Project
        };
        let trusted_local_settings_snapshot = local_settings_contents
            .as_deref()
            .map(|contents| parse_json_object_contents(&local_settings_path, contents))
            .transpose()?;
        let discovered_entries = self.discover_with_local_scope(local_settings_source);
        let read_discovered_entry = |entry: &ConfigEntry| {
            if entry.source == ConfigSource::Local && entry.path == local_settings_path {
                if let Some(snapshot) = &trusted_local_settings_snapshot {
                    return Ok(Some(snapshot.clone()));
                }
            }
            read_optional_json_object(&entry.path)
        };

        let project_mcp_entry = self.project_mcp_entry();
        let mut loaded_project_mcp = false;

        // `--strict-mcp-config` (CC parity): MCP servers come *only* from the
        // explicit `--mcp-config` file — every discovered settings document
        // still merges normally, but its `mcpServers` section is ignored.
        let strict_mcp = cli_overrides().strict_mcp_config;

        // Per-server allowlist for project-scoped (repo-committed,
        // attacker-controllable) MCP servers. Read once before the loop:
        // `.zo/mcp.json` and `.zo/settings.json` both gate against the same
        // record rather than re-reading it per document.
        let trusted_project_servers = self.trusted_project_mcp_servers()?;

        // Opt-in to bypass the project supply-chain gates (auto-discovered MCP
        // servers and repo-committed plugin directories) is honored ONLY from
        // trusted, operator-authored scopes — never from the repo-committed
        // Project documents the gates exist to contain. Reading the flag from
        // the running `merged` map let one project document opt the next one
        // (and `.zo/mcp.json`) in — a cross-document self-authorization bypass that
        // defeated the gate. Snapshot the opt-in once, up front, from the
        // non-Project documents (User config, the gitignored
        // `.zo/settings.local.json`, and any `--settings` override).
        let mut trusted_settings = BTreeMap::new();
        for entry in &discovered_entries {
            if entry.source != ConfigSource::Project {
                if let Some(value) = read_discovered_entry(entry)? {
                    deep_merge_objects(&mut trusted_settings, &value);
                }
            }
        }
        let project_mcp_opt_in = enable_all_project_mcp_servers(&trusted_settings);
        let project_plugins_opt_in = enable_all_project_plugins(&trusted_settings);
        let project_permissions_opt_in = enable_all_project_permissions(&trusted_settings);
        let project_env_opt_in = enable_all_project_env(&trusted_settings);
        let project_hooks_opt_in = enable_all_project_flag(&trusted_settings, "enableAllProjectHooks");
        let project_ship_opt_in = enable_all_project_flag(&trusted_settings, "enableAllProjectShip");
        let project_sandbox_opt_in =
            enable_all_project_flag(&trusted_settings, "enableAllProjectSandbox");
        let project_providers_opt_in =
            enable_all_project_flag(&trusted_settings, "enableAllProjectProviders");
        let project_oauth_opt_in =
            enable_all_project_flag(&trusted_settings, "enableAllProjectOauth");
        let project_statusline_opt_in =
            enable_all_project_flag(&trusted_settings, "enableAllProjectStatusLine");
        let project_lsp_opt_in = enable_all_project_flag(&trusted_settings, "enableAllProjectLsp");
        // Whether the operator's trusted config is in ordered `rules` mode. If so,
        // an untrusted project's category deny/ask cannot coexist with it (the
        // strict parse rejects the combination and bricks the whole load), so it is
        // stripped below — a cloned repo must not be able to brick the operator.
        let trusted_uses_ordered_rules = trusted_config_uses_ordered_rules(&trusted_settings);

        // Restriction lists (`permissions.deny` / `ask`) accumulate across every
        // scope instead of last-writer replacement, so a later document can never
        // ERASE an earlier one's entries by supplying a shorter (or empty) array.
        // This is the core of the fix for the "hostile repo sets `deny: []` to
        // unlock what the user globally forbade" hole; unioning restrictions is
        // always safe. Written back over the merged arrays after the loop.
        let mut cumulative_deny: Vec<JsonValue> = Vec::new();
        let mut cumulative_ask: Vec<JsonValue> = Vec::new();
        // Provider registrations are global; accumulate them by name so a
        // project-scoped `providers` array adds to (and overrides by name) the
        // global registry instead of replacing it wholesale.
        let mut cumulative_providers: Vec<JsonValue> = Vec::new();

        for entry in discovered_entries {
            if !loaded_project_mcp && entry.path == local_settings_path {
                if !strict_mcp {
                    if let Some(value) = read_optional_json_object(&project_mcp_entry.path)? {
                        // Project-scoped `.zo/mcp.json` is auto-discovered, so its
                        // servers are gated behind a per-server trust check
                        // (supply-chain protection). `enableAllProjectMcpServers`
                        // opts the whole project in; otherwise only servers
                        // recorded in `.zo/trusted-mcp-servers.json` merge.
                        let enable_all = project_mcp_opt_in;
                        merge_trusted_project_mcp_servers(
                            &mut mcp_servers,
                            &mut untrusted_project_mcp,
                            &value,
                            &project_mcp_entry.path,
                            enable_all,
                            &trusted_project_servers,
                        )?;
                        loaded_entries.push(project_mcp_entry.clone());
                    }
                }
                loaded_project_mcp = true;
            }

            let Some(mut value) = read_discovered_entry(&entry)? else {
                continue;
            };
            // Repo-committed Project documents are attacker-controllable on clone,
            // so their code-execution / security-downgrade surfaces are stripped
            // before validation or merge unless an operator opts each in (honored
            // only from the trusted non-Project snapshot). `hooks` runs
            // `sh -lc <cmd>` at session start = zero-click RCE; `env` (handled
            // below) is injected into every subprocess; `sandbox` can disable
            // isolation; `providers` can point completions at an attacker
            // endpoint; `oauth` can redirect the auth flow. Mirrors the existing
            // mcpServers/plugins/permissions gates. `hooks` is stripped BEFORE
            // `validate_optional_hooks_config` so a malformed project hook cannot
            // even DoS the load.
            if entry.source == ConfigSource::Project {
                if !project_hooks_opt_in {
                    value.remove("hooks");
                }
                if !project_ship_opt_in {
                    value.remove("ship");
                }
                if !project_sandbox_opt_in {
                    value.remove("sandbox");
                }
                if !project_providers_opt_in {
                    value.remove("providers");
                }
                if !project_oauth_opt_in {
                    value.remove("oauth");
                }
                // `statusLine` runs `sh -c <command>` on a ~1s TUI poll, and each
                // `lspServers` entry spawns `command`+`args`+`env` at session boot
                // (its `env` also bypasses the top-level `env` strip). Both are
                // zero-click RCE from a cloned repo — strip before `deep_merge`
                // (statusLine) and before `merge_lsp_servers` below (lspServers).
                if !project_statusline_opt_in {
                    value.remove("statusLine");
                }
                if !project_lsp_opt_in {
                    value.remove("lspServers");
                }
            }
            validate_optional_hooks_config(&value, &entry.path)?;
            if !strict_mcp {
                // Project-scoped `.zo/settings.json` lives in the repository and is
                // attacker-controllable on clone, so their `mcpServers` are
                // gated by the same per-server trust check as `.zo/mcp.json`
                // instead of merging silently. User/Local documents are
                // operator-authored and merge directly. This closes the
                // clone-and-run supply-chain hole where a hostile repo could
                // auto-spawn an MCP server command with no confirmation.
                if entry.source == ConfigSource::Project {
                    let enable_all = project_mcp_opt_in;
                    merge_trusted_project_mcp_servers(
                        &mut mcp_servers,
                        &mut untrusted_project_mcp,
                        &value,
                        &entry.path,
                        enable_all,
                        &trusted_project_servers,
                    )?;
                } else {
                    merge_mcp_servers(&mut mcp_servers, entry.source, &value, &entry.path)?;
                }
            }
            merge_lsp_servers(&mut lsp_servers, entry.source, &value, &entry.path)?;
            // Project-scoped `.zo/settings.json` lives in the repository, so its
            // plugin path keys
            // (`externalDirectories` / `installRoot` / `registryPath` /
            // `bundledRoot`) point at repo-committed directories whose manifests
            // run `Command::new` on load. Gate them like project MCP servers:
            // strip them before they merge unless `enableAllProjectPlugins`
            // (operator-authored) opts the project in. This closes the
            // clone-and-run hole where a hostile repo auto-executes a plugin
            // command. The `enabled` / `enabledPlugins` toggles are plugin-id
            // references, not executable paths, so they merge unchanged.
            if entry.source == ConfigSource::Project && !project_plugins_opt_in {
                strip_untrusted_project_plugin_paths(&mut value);
            }
            // A repo-committed Project document is attacker-controllable, so it
            // must not ESCALATE permissions — only add restrictions or pin a safer
            // posture. Strip its capability grants (`allow`/`rules`) and any
            // escalation to danger-full-access, keeping `deny`/`ask` and the safer
            // modes, unless an operator opts the project in. Mirrors the
            // mcpServers/plugins supply-chain gates.
            if entry.source == ConfigSource::Project && !project_permissions_opt_in {
                strip_untrusted_project_permission_grants(&mut value, trusted_uses_ordered_rules);
            }
            // A repo-committed `env` is injected into every zo-spawned
            // subprocess, so it is a code-execution vector (`LD_PRELOAD`,
            // `BASH_ENV`, …). Strip the whole Project `env` block unless an
            // operator opts in, mirroring the mcpServers/plugins/permissions gates.
            if entry.source == ConfigSource::Project && !project_env_opt_in {
                value.remove("env");
            }
            // Fold this scope's (post-strip) deny/ask into the cumulative unions
            // so the final policy is a superset of every scope's restrictions,
            // regardless of merge order.
            collect_permission_list(&value, "deny", &mut cumulative_deny);
            collect_permission_list(&value, "ask", &mut cumulative_ask);
            collect_provider_entries(&value, &mut cumulative_providers);
            deep_merge_objects(&mut merged, &value);
            loaded_entries.push(entry);
        }

        // Replace the last-writer deny/ask arrays `deep_merge_objects` produced
        // with the cumulative unions built above. `allow`/`rules` stay
        // last-writer among the trusted User/Local scopes (Project can no longer
        // contribute either after the strip).
        apply_cumulative_permission_lists(&mut merged, cumulative_deny, cumulative_ask);
        apply_cumulative_providers(&mut merged, cumulative_providers);

        if let Some(path) = &self.mcp_config {
            let entry = ConfigEntry {
                source: ConfigSource::Local,
                path: path.clone(),
            };
            let Some(value) = read_optional_json_object(path)? else {
                return Err(ConfigError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("--mcp-config file not found: {}", path.display()),
                )));
            };
            merge_mcp_servers(&mut mcp_servers, entry.source, &value, &entry.path)?;
            loaded_entries.push(entry);
        }

        let merged_value = JsonValue::Object(merged.clone());

        let feature_config = RuntimeFeatureConfig {
            hooks: parse_optional_hooks_config(&merged_value)?,
            plugins: parse_optional_plugin_config(&merged_value)?,
            review: parse_optional_review_config(&merged_value)?,
            ship: parse_optional_ship_config(&merged_value)?,
            mcp: McpConfigCollection {
                servers: mcp_servers,
                untrusted: untrusted_project_mcp,
            },
            lsp: LspConfigCollection {
                servers: lsp_servers,
            },
            oauth: parse_optional_oauth_config(&merged_value, "merged settings.oauth")?,
            model: parse_optional_model(&merged_value),
            permission_mode: parse_optional_permission_mode(&merged_value)?,
            permission_rules: parse_optional_permission_rules(&merged_value)?,
            sandbox: parse_optional_sandbox_config(&merged_value)?,
            env: optional_string_map(&merged, "env", "merged settings.env")?.unwrap_or_default(),
            auto_compact_enabled: parse_enabled_flag(&merged_value, "autoCompactEnabled")?,
            auto_compact_threshold_percent: parse_auto_compact_threshold_percent(&merged_value)?,
            auto_memory_enabled: parse_enabled_flag(&merged_value, "autoMemoryEnabled")?,
            auto_dream_enabled: parse_enabled_flag(&merged_value, "autoDreamEnabled")?,
            auto_improve_proposals_enabled: parse_opt_in_flag(
                &merged_value,
                "autoImproveProposalsEnabled",
            )?,
            team_inbox_digest_enabled: parse_enabled_flag(
                &merged_value,
                "teamInboxDigestEnabled",
            )?,
            team_inbox_digest_max_updates: parse_team_inbox_digest_max_updates(&merged_value)?,
            recall_hint_enabled: parse_enabled_flag(&merged_value, "recallHintEnabled")?,
            checkpoint_durable: parse_checkpoint_durable(&merged_value)?,
            tui_inline_mode: parse_tui_inline_mode(&merged_value)?,
            second_brain: parse_second_brain_config(&merged_value)?,
        };

        Ok(RuntimeConfig {
            merged,
            loaded_entries,
            feature_config,
        })
    }
}

/// Whether trusted settings opt every auto-discovered project MCP server in,
/// bypassing the per-server trust gate (CC parity: `enableAllProjectMcpServers`).
///
/// The flag is honored only from the merged non-Project (operator-authored)
/// settings — never from a repo-committed Project document, which could
/// otherwise self-authorize. When false the `.zo/trusted-mcp-servers.json`
/// allowlist still gates individual servers, so an untrusted server is skipped
/// rather than spawned.
fn enable_all_project_mcp_servers(settings: &BTreeMap<String, JsonValue>) -> bool {
    settings
        .get("enableAllProjectMcpServers")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

/// Whether trusted settings opt every repo-committed plugin directory in,
/// bypassing the supply-chain gate (the plugin analogue of
/// `enableAllProjectMcpServers`).
///
/// Like its MCP sibling, the flag is honored only from the merged non-Project
/// (operator-authored) settings, so a hostile repo cannot opt its own plugin
/// directories in. When false, project-declared plugin paths are stripped
/// before they merge.
fn enable_all_project_plugins(settings: &BTreeMap<String, JsonValue>) -> bool {
    settings
        .get("enableAllProjectPlugins")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

/// Whether trusted settings opt a repo-committed Project `env` block in. Project
/// `env` is injected into every zo-spawned subprocess (bash/hooks/powershell),
/// so a hostile repo could ship `LD_PRELOAD` / `BASH_ENV` / `DYLD_INSERT_LIBRARIES`
/// and gain code execution the moment a shell runs. Like the other gates, honored
/// only from operator-authored non-Project settings. When false the whole Project
/// `env` object is stripped before it can reach a child.
fn enable_all_project_env(settings: &BTreeMap<String, JsonValue>) -> bool {
    settings
        .get("enableAllProjectEnv")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

/// Read a boolean `enableAllProject*` opt-in from the trusted (non-Project)
/// settings snapshot. Shared by the `hooks` / `sandbox` / `providers` / `oauth`
/// supply-chain gates; each is a code-execution or security-downgrade surface a
/// repo-committed document must not control unless an operator explicitly opts in
/// from a scope the repo cannot write. Absent/non-bool → not opted in.
fn enable_all_project_flag(settings: &BTreeMap<String, JsonValue>, flag: &str) -> bool {
    settings
        .get(flag)
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

/// Scope for `cwd/.zo/settings.local.json`. It is trusted (`Local`) only when
/// it is genuinely the operator's own real, uncommitted file; otherwise it is
/// reclassified `Project` so every supply-chain strip applies. Fails CLOSED, so
/// any indirection a repo could commit to smuggle a payload onto disk without the
/// literal path being tracked is untrusted.
fn local_settings_scope(cwd: &Path, path: &Path) -> ConfigSource {
    if is_trusted_local_settings(cwd, path) {
        ConfigSource::Local
    } else {
        ConfigSource::Project
    }
}

/// Whether `settings.local.json` may be trusted as `Local`. True only for the
/// operator's own real, uncommitted file. It fails closed to `Project` on every
/// indirection a repo can commit to smuggle a payload onto disk at (or aliasing)
/// this path without the literal path being tracked:
/// * the leaf being a symlink;
/// * the real on-disk path not being EXACTLY `<canonical cwd>/.zo/settings.local.json`
///   — this rejects a symlinked `.zo` / intermediate symlink AND a
///   case-/Unicode-folded name (`.zo/Settings.local.json`) that a
///   case-insensitive FS (macOS, Windows — the default target) aliases to the
///   literal path, which `git ls-files` (case-sensitive index lookup) misses;
/// * a `.zo` submodule / nested repo (`.zo/.git`) whose file lands on a
///   recursive clone yet is a gitlink (untracked) in the outer repo;
/// * a git-tracked (committed) path;
/// * an absent file (moot — nothing loads).
fn is_trusted_local_settings(cwd: &Path, path: &Path) -> bool {
    is_trusted_uncommitted_zo_file(cwd, path, "settings.local.json")
}

/// Persist live `AllowAlways` grants into the trusted project-local settings.
///
/// The file must already be the operator's trusted, uncommitted
/// `.zo/settings.local.json`; this function never creates or authorizes a
/// project settings file. Existing settings are preserved, and the candidate
/// document is parsed again after rendering so a malformed rule or a lossy JSON
/// round-trip fails closed before any write.
///
/// A grant that cannot be persisted is deliberately silent: the session already
/// honored it, and refusing to write is always the narrow outcome. Only a
/// malformed document or rule is reported, and even then nothing is written.
pub fn persist_allow_always_rules(cwd: &Path, rules: &[String]) -> Result<(), ConfigError> {
    const FILE_NAME: &str = "settings.local.json";

    if rules.is_empty() {
        return Ok(());
    }

    let path = cwd.join(".zo").join(FILE_NAME);
    if !is_trusted_uncommitted_zo_file(cwd, &path, FILE_NAME) {
        return Ok(());
    }
    let Some(original_contents) = trusted_uncommitted_zo_file_snapshot(cwd, &path, FILE_NAME)
    else {
        return Ok(());
    };

    let Some(rendered) = merge_allow_always_rules(&path, &original_contents, rules)? else {
        return Ok(());
    };

    // Abort if the trusted file changed while the candidate was being built;
    // never merge over a newer operator edit or a changed trust decision.
    let Some(current_contents) = trusted_uncommitted_zo_file_snapshot(cwd, &path, FILE_NAME)
    else {
        return Ok(());
    };
    if current_contents != original_contents {
        return Ok(());
    }

    write_private_file(&path, rendered.as_bytes(), &ParentDirPolicy::LeaveParent)?;
    Ok(())
}

/// Merge `rules` into `contents`, returning the document to write, or `None`
/// when every rule is already present.
///
/// Pure on purpose: the trust gate and the write live in
/// [`persist_allow_always_rules`], so the part that decides **what bytes a
/// future session will read back** can be tested without a git worktree.
///
/// The form follows the file. `parse_optional_permission_rules` rejects a
/// document that mixes the ordered `rules` list with the `allow`/`deny`/`ask`
/// categories, so a file already using the ordered form gets `"<rule>=allow"`
/// appended to `rules`, and every other file gets the bare rule appended to
/// `allow`. Writing into the wrong one would make the whole document
/// unreadable on the next load — which fails open into "ask again", not into
/// "allow more", but still loses the operator's own settings.
fn merge_allow_always_rules(
    path: &Path,
    contents: &str,
    rules: &[String],
) -> Result<Option<String>, ConfigError> {
    let mut root = parse_json_object_contents(path, contents)?;
    let existing = parse_optional_permission_rules(&JsonValue::Object(root.clone()))?;

    if let Some(permissions) = root.get("permissions") {
        if permissions.as_object().is_none() {
            return Err(ConfigError::Parse(format!(
                "{}: permissions must be an object",
                path.display()
            )));
        }
    }

    let ordered = !existing.rules().is_empty();
    let persisted = rules
        .iter()
        .map(|rule| {
            if rule.trim().is_empty() {
                Err(ConfigError::Parse(format!(
                    "{}: AllowAlways rule must not be empty",
                    path.display()
                )))
            } else if ordered {
                // A grant arrives as `tool(subject)` — the drain never appends an
                // action. If one ever did, suffixing again would write
                // `bash(rm *)=deny=allow`: `rsplit_once('=')` reads that as the
                // subject `bash(rm *)=deny`, a rule that can never match. That
                // fails narrow, but it is still a grant the operator can see on
                // disk and can never rely on. Refuse instead of writing a lie.
                if crate::permissions::validate_decision_rule_spec(rule).is_ok() {
                    return Err(ConfigError::Parse(format!(
                        "{}: AllowAlways rule `{rule}` already carries an action",
                        path.display()
                    )));
                }
                Ok(format!("{rule}=allow"))
            } else {
                Ok(rule.clone())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    let list_key = if ordered { "rules" } else { "allow" };
    let mut expected = if ordered {
        existing.rules().to_vec()
    } else {
        existing.allow().to_vec()
    };
    for rule in &persisted {
        if !expected.contains(rule) {
            expected.push(rule.clone());
        }
    }

    let permissions = root
        .entry("permissions".to_string())
        .or_insert_with(|| JsonValue::Object(BTreeMap::new()));
    let JsonValue::Object(permissions) = permissions else {
        unreachable!("permissions was checked to be an object above");
    };
    let list = permissions
        .entry(list_key.to_string())
        .or_insert_with(|| JsonValue::Array(Vec::new()));
    let JsonValue::Array(list) = list else {
        return Err(ConfigError::Parse(format!(
            "{}: permissions.{list_key} must be an array",
            path.display()
        )));
    };

    let mut changed = false;
    for rule in &persisted {
        if !list.iter().any(|value| value.as_str() == Some(rule.as_str())) {
            list.push(JsonValue::String(rule.clone()));
            changed = true;
        }
    }
    if !changed {
        return Ok(None);
    }

    // Read the candidate back through the very parser a future session uses.
    // A rule the ordered form cannot express, or a document our renderer
    // mangles, is caught here — before it becomes the file on disk.
    let rendered = JsonValue::Object(root).render();
    let roundtrip = parse_optional_permission_rules(&JsonValue::Object(
        parse_json_object_contents(path, &rendered)?,
    ))?;
    let actual = if ordered {
        roundtrip.rules()
    } else {
        roundtrip.allow()
    };
    if actual != expected.as_slice() {
        return Err(ConfigError::Parse(format!(
            "{}: permission rules did not survive JSON round-trip",
            path.display()
        )));
    }

    Ok(Some(rendered))
}

/// Trust a project-local Zo control file only when it is the operator's real,
/// uncommitted file at the exact expected path. This shared gate prevents a
/// repository from self-authorizing through tracked files, symlinks, aliases,
/// or a nested `.zo` repository.
fn is_trusted_uncommitted_zo_file(cwd: &Path, path: &Path, file_name: &str) -> bool {
    trusted_uncommitted_zo_file_snapshot(cwd, path, file_name).is_some()
}

fn trusted_uncommitted_zo_file_snapshot(
    cwd: &Path,
    path: &Path,
    file_name: &str,
) -> Option<String> {
    let git_program = resolve_trusted_git_program(cwd)?;
    trusted_uncommitted_zo_file_snapshot_with_git(
        cwd,
        path,
        file_name,
        git_program.as_os_str(),
    )
}

fn resolve_trusted_git_program(cwd: &Path) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    resolve_trusted_git_program_from(cwd, &path)
}

fn resolve_trusted_git_program_from(cwd: &Path, path: &std::ffi::OsStr) -> Option<PathBuf> {
    let canonical_cwd = cwd.canonicalize().ok()?;
    let untrusted_roots = git_control_roots(&canonical_cwd).ok()?;
    for directory in std::env::split_paths(path) {
        if !directory.is_absolute() {
            continue;
        }
        let executable = if cfg!(windows) { "git.exe" } else { "git" };
        let Ok(candidate) = directory.join(executable).canonicalize() else {
            continue;
        };
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file()
            && git_candidate_is_executable(&metadata)
            && !untrusted_roots.iter().any(|root| candidate.starts_with(root))
        {
            return Some(candidate);
        }
    }
    None
}

fn git_control_roots(cwd: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    for ancestor in cwd.ancestors() {
        match std::fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => roots.push(ancestor.to_path_buf()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if roots.is_empty() {
        roots.push(cwd.to_path_buf());
    }
    Ok(roots)
}

#[cfg(unix)]
fn git_candidate_is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn git_candidate_is_executable(_metadata: &std::fs::Metadata) -> bool {
    true
}

#[cfg(test)]
fn is_trusted_uncommitted_zo_file_with_git(
    cwd: &Path,
    path: &Path,
    file_name: &str,
    git_program: &std::ffi::OsStr,
) -> bool {
    trusted_uncommitted_zo_file_snapshot_with_git(cwd, path, file_name, git_program).is_some()
}

fn trusted_uncommitted_zo_file_snapshot_with_git(
    cwd: &Path,
    path: &Path,
    file_name: &str,
    git_program: &std::ffi::OsStr,
) -> Option<String> {
    let relative = PathBuf::from(".zo").join(file_name);
    if path != cwd.join(&relative) {
        return None;
    }

    crate::secure_fs::read_to_string_no_symlink_with_validation(cwd, &relative, || {
        let (Ok(real), Ok(base)) = (path.canonicalize(), cwd.canonicalize()) else {
            return Err(untrusted_control_file());
        };
        if real != base.join(&relative) {
            return Err(untrusted_control_file());
        }
        ensure_path_absent(&base.join(".zo").join(".git"))?;
        let repository_before = git_repository_fingerprint(&base)?;
        let git_path = PathBuf::from(git_program);
        if !git_path.is_absolute() {
            return Err(untrusted_control_file());
        }
        let Ok(git_before) = git_path.canonicalize() else {
            return Err(untrusted_control_file());
        };
        let git_metadata = std::fs::metadata(&git_before)?;
        if !git_metadata.is_file() || !git_candidate_is_executable(&git_metadata) {
            return Err(untrusted_control_file());
        }
        let git_identity = filesystem_identity(&git_metadata)?;
        if repository_before.markers.iter().any(|marker| {
            marker
                .path
                .parent()
                .is_some_and(|root| git_before.starts_with(root))
        }) || (repository_before.markers.is_empty() && git_before.starts_with(&base))
        {
            return Err(untrusted_control_file());
        }
        let tracking = git_tracking_state_with_program(
            &base,
            &real,
            git_before.as_os_str(),
            repository_before.markers.is_empty(),
        );

        let (Ok(real_after), Ok(base_after)) = (path.canonicalize(), cwd.canonicalize()) else {
            return Err(untrusted_control_file());
        };
        if real_after != real || base_after != base {
            return Err(untrusted_control_file());
        }
        let Ok(git_after) = git_path.canonicalize() else {
            return Err(untrusted_control_file());
        };
        if git_after != git_before
            || filesystem_identity(&std::fs::metadata(&git_after)?)? != git_identity
        {
            return Err(untrusted_control_file());
        }
        ensure_path_absent(&base_after.join(".zo").join(".git"))?;
        if git_repository_fingerprint(&base_after)? != repository_before {
            return Err(untrusted_control_file());
        }
        if !matches!(
            tracking,
            GitTrackingState::Untracked | GitTrackingState::NotRepository
        ) {
            return Err(untrusted_control_file());
        }
        Ok(())
    })
    .ok()
}

fn untrusted_control_file() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "project-local Zo control file provenance could not be trusted",
    )
}

fn ensure_path_absent(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(untrusted_control_file()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitRepositoryFingerprint {
    cwd: FilesystemIdentity,
    markers: Vec<GitAdminMarkerIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitAdminMarkerIdentity {
    path: PathBuf,
    identity: FilesystemIdentity,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FilesystemIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(not(unix))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FilesystemIdentity;

#[cfg(unix)]
fn filesystem_identity(metadata: &std::fs::Metadata) -> std::io::Result<FilesystemIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    // Missing timestamp support makes provenance indeterminate, so fail closed.
    let _ = metadata.modified()?;
    Ok(FilesystemIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        links: metadata.nlink(),
        length: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    })
}

#[cfg(not(unix))]
fn filesystem_identity(_metadata: &std::fs::Metadata) -> std::io::Result<FilesystemIdentity> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "trusted project control files require Unix filesystem identities",
    ))
}

fn git_repository_fingerprint(cwd: &Path) -> std::io::Result<GitRepositoryFingerprint> {
    let cwd_identity = filesystem_identity(&std::fs::metadata(cwd)?)?;
    let mut markers = Vec::new();
    for ancestor in cwd.ancestors() {
        let path = ancestor.join(".git");
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => markers.push(GitAdminMarkerIdentity {
                path,
                identity: filesystem_identity(&metadata)?,
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(GitRepositoryFingerprint {
        cwd: cwd_identity,
        markers,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitTrackingState {
    Tracked,
    Untracked,
    NotRepository,
    Indeterminate,
}

/// Determine Git provenance without conflating an untracked file with a Git
/// failure. Only a successful repository probe followed by a successful index
/// query can yield `Tracked`/`Untracked`; launch failures, dubious ownership,
/// repository corruption, and unexpected output fail closed as `Indeterminate`.
fn git_tracking_state_with_program(
    cwd: &Path,
    path: &Path,
    git_program: &std::ffi::OsStr,
    allow_not_repository: bool,
) -> GitTrackingState {
    let configure = |command: &mut std::process::Command| {
        command
            .current_dir(cwd)
            .env("LC_ALL", "C")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_CEILING_DIRECTORIES");
    };

    let mut probe = std::process::Command::new(git_program);
    configure(&mut probe);
    let Ok(probe) = probe.args(["rev-parse", "--is-inside-work-tree"]).output() else {
        return GitTrackingState::Indeterminate;
    };
    if !probe.status.success() {
        let stderr = probe.stderr.as_slice().trim_ascii();
        return if probe.stdout.is_empty()
            && allow_not_repository
            && stderr.starts_with(b"fatal: not a git repository")
        {
            GitTrackingState::NotRepository
        } else {
            GitTrackingState::Indeterminate
        };
    }
    if !probe.stderr.as_slice().trim_ascii().is_empty()
        || probe.stdout.as_slice().trim_ascii() != b"true"
    {
        return GitTrackingState::Indeterminate;
    }

    let Ok(relative) = path.strip_prefix(cwd) else {
        return GitTrackingState::Indeterminate;
    };
    let mut query = std::process::Command::new(git_program);
    configure(&mut query);
    let Ok(query) = query
        .args(["ls-files", "--stage", "-z", "--"])
        .arg(relative)
        .output()
    else {
        return GitTrackingState::Indeterminate;
    };
    if !query.status.success() || !query.stderr.as_slice().trim_ascii().is_empty() {
        return GitTrackingState::Indeterminate;
    }
    if query.stdout.is_empty() {
        GitTrackingState::Untracked
    } else if valid_git_stage_output(&query.stdout) {
        GitTrackingState::Tracked
    } else {
        GitTrackingState::Indeterminate
    }
}

fn valid_git_stage_output(output: &[u8]) -> bool {
    let Some(records) = output.strip_suffix(b"\0") else {
        return false;
    };
    !records.is_empty()
        && records.split(|byte| *byte == 0).all(|record| {
            let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
                return false;
            };
            let header = &record[..tab];
            let path = &record[tab + 1..];
            let mut fields = header.split(|byte| *byte == b' ');
            let (Some(mode), Some(object_id), Some(stage)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return false;
            };
            fields.next().is_none()
                && mode.len() == 6
                && mode.iter().all(|byte| matches!(byte, b'0'..=b'7'))
                && object_id.len() >= 40
                && object_id.iter().all(u8::is_ascii_hexdigit)
                && matches!(stage, b"0" | b"1" | b"2" | b"3")
                && !path.is_empty()
        })
}

/// Remove the executable-path plugin keys from an untrusted project settings
/// document so they never reach the merged config. The `plugins.enabled` /
/// top-level `enabledPlugins` toggles are left intact: they reference plugin ids
/// (no path, no command), so they cannot introduce code on their own.
fn strip_untrusted_project_plugin_paths(value: &mut BTreeMap<String, JsonValue>) {
    if let Some(JsonValue::Object(plugins)) = value.get_mut("plugins") {
        for key in [
            "externalDirectories",
            "installRoot",
            "registryPath",
            "bundledRoot",
        ] {
            plugins.remove(key);
        }
    }
}

/// Whether trusted settings opt the repo-committed Project `permissions` block
/// in, bypassing the escalation strip below. The permissions analogue of
/// `enableAllProjectMcpServers` / `enableAllProjectPlugins`: honored only from
/// operator-authored non-Project settings so a hostile repo cannot self-authorize
/// by setting the flag in its own committed document.
fn enable_all_project_permissions(settings: &BTreeMap<String, JsonValue>) -> bool {
    settings
        .get("enableAllProjectPermissions")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

/// Remove the permission-ESCALATION keys from an untrusted Project settings
/// document. A repo-committed document may only ever ADD restrictions or pin a
/// *bounded* posture — never grant itself capability, escalate to prompt-free
/// access, or erase what the operator set (trust boundary = scope provenance):
///
/// * category `allow` and ordered `rules` (which can grant `allow`) are stripped
///   outright — a repo does not decide what the user auto-approves.
/// * the permission **mode** (`permissions.defaultMode` / top-level
///   `permissionMode`) is stripped only when it escalates to danger-full-access
///   (`dontAsk` / `danger-full-access`); a cloned repo pinning that would
///   auto-approve arbitrary bash on the first agent action. A project may still
///   pin the workspace-bounded `acceptEdits` or `read-only` — a deliberate,
///   dedicated CC-parity feature (see `default_permission_mode` tests).
/// * the restriction keys `deny` and `ask` are always kept (a repo may add
///   restrictions).
/// * a NON-object `permissions` value (e.g. `[]`, `"x"`) is dropped: from a repo
///   it can only be an erase/DoS vector — it would clobber the merged
///   permissions object via `deep_merge_objects` and slip past the object-guarded
///   accumulation, silently deleting the user's deny/ask.
fn strip_untrusted_project_permission_grants(
    value: &mut BTreeMap<String, JsonValue>,
    trusted_uses_ordered_rules: bool,
) {
    if permission_value_is_danger_full_access(value.get("permissionMode")) {
        value.remove("permissionMode");
    }
    match value.get_mut("permissions") {
        Some(JsonValue::Object(permissions)) => {
            permissions.remove("allow");
            permissions.remove("rules");
            if permission_value_is_danger_full_access(permissions.get("defaultMode")) {
                permissions.remove("defaultMode");
            }
            // When the operator's TRUSTED config is in ordered `rules` mode, this
            // project's category `deny`/`ask` cannot coexist with it — the strict
            // parse rejects mixing the two forms and bricks the ENTIRE config load
            // (a trivially-triggered supply-chain DoS: commit any well-formed
            // `deny` and a rules-mode operator can't start). `rules` supersedes the
            // category form and is authoritative, so drop the untrusted project's
            // restrictions rather than let them brick the operator. A TRUSTED
            // scope's `deny`/`ask` beside `rules` is left intact so the
            // mutual-exclusion footgun still fails loud for the operator's own mix.
            if trusted_uses_ordered_rules {
                permissions.remove("deny");
                permissions.remove("ask");
            }
        }
        Some(_) => {
            value.remove("permissions");
        }
        None => {}
    }
}

/// Whether the merged TRUSTED (non-Project) settings put the operator in the
/// ordered `rules` permission form. Used to decide whether an untrusted project's
/// mutually-exclusive category `deny`/`ask` must be dropped to avoid bricking the
/// load; the check is provenance-gated so a trusted scope's own mix still errors.
fn trusted_config_uses_ordered_rules(trusted_settings: &BTreeMap<String, JsonValue>) -> bool {
    trusted_settings
        .get("permissions")
        .and_then(JsonValue::as_object)
        .and_then(|permissions| permissions.get("rules"))
        .and_then(JsonValue::as_array)
        .is_some_and(|rules| !rules.is_empty())
}

/// Whether a `permissionMode` / `defaultMode` JSON value names the
/// danger-full-access (prompt-free) tier — the one mode escalation a
/// repo-committed document must never be able to grant itself.
fn permission_value_is_danger_full_access(value: Option<&JsonValue>) -> bool {
    value
        .and_then(JsonValue::as_str)
        .is_some_and(|mode| matches!(mode, "dontAsk" | "danger-full-access"))
}

/// Union the entries of `permissions.<key>` from one settings document into
/// `acc`, preserving first-seen order and skipping duplicates. Non-string entries
/// are carried through verbatim; the strict parse (`optional_string_array`) still
/// validates the final merged array and surfaces any type error.
fn collect_permission_list(
    value: &BTreeMap<String, JsonValue>,
    key: &str,
    acc: &mut Vec<JsonValue>,
) {
    let Some(list) = value
        .get("permissions")
        .and_then(JsonValue::as_object)
        .and_then(|permissions| permissions.get(key))
        .and_then(JsonValue::as_array)
    else {
        return;
    };
    for entry in list {
        // Only string entries are valid permission specs. Dropping a non-string
        // (a typo, or a hostile repo's `[123]`) keeps a malformed entry out of the
        // union so it cannot fail the strict parse and brick the ENTIRE config
        // load — mirroring the hooks strip-before-validate protection. The raw
        // array `deep_merge_objects` left in `merged` is replaced by this clean
        // union in `apply_cumulative_permission_lists`.
        if entry.as_str().is_some() && !acc.contains(entry) {
            acc.push(entry.clone());
        }
    }
}

/// Write the cumulative deny/ask unions back into the merged `permissions`
/// object, replacing whatever last-writer array `deep_merge_objects` produced, so
/// the effective restrictions are a superset of every scope's regardless of merge
/// order. Empty unions write nothing.
/// Fold one scope's `providers` array into the cross-scope accumulator, keyed by
/// provider name (case-insensitively, matching how model routing resolves it).
///
/// A provider registration is user-global by design — `/connect` and
/// `/providers` only ever write the config home — but `deep_merge_objects`
/// replaces arrays wholesale, so a `providers` array in a *trusted* project
/// `settings.local.json` would hide every globally registered provider rather
/// than adding to them. Accumulating by name keeps the local file authoritative
/// for the entries it actually names and leaves the rest visible, the same
/// reasoning that makes `permissions.deny` cumulative.
///
/// Untrusted `ConfigSource::Project` documents never reach here: their
/// `providers` key is stripped earlier in the load, and that gate is unchanged.
fn collect_provider_entries(value: &BTreeMap<String, JsonValue>, acc: &mut Vec<JsonValue>) {
    let Some(list) = value.get("providers").and_then(JsonValue::as_array) else {
        return;
    };
    for entry in list {
        let name = entry
            .as_object()
            .and_then(|entry| entry.get("name"))
            .and_then(JsonValue::as_str);
        let existing = name.and_then(|name| {
            acc.iter().position(|candidate| {
                candidate
                    .as_object()
                    .and_then(|candidate| candidate.get("name"))
                    .and_then(JsonValue::as_str)
                    .is_some_and(|existing| existing.eq_ignore_ascii_case(name))
            })
        });
        match existing {
            Some(index) => acc[index] = entry.clone(),
            None => acc.push(entry.clone()),
        }
    }
}

/// Write the cross-scope provider union back over whatever last-writer array
/// `deep_merge_objects` produced. A no-op when no scope declared any.
fn apply_cumulative_providers(merged: &mut BTreeMap<String, JsonValue>, providers: Vec<JsonValue>) {
    if providers.is_empty() {
        return;
    }
    merged.insert("providers".to_string(), JsonValue::Array(providers));
}

fn apply_cumulative_permission_lists(
    merged: &mut BTreeMap<String, JsonValue>,
    deny: Vec<JsonValue>,
    ask: Vec<JsonValue>,
) {
    // Proceed when there is a union to write OR a permissions object that may hold
    // a raw (possibly malformed) `deny`/`ask` array left by `deep_merge_objects`
    // that must be replaced with the clean, string-only union — otherwise a
    // non-string entry survives to the strict parse and fails the whole load.
    let has_permissions_object = matches!(merged.get("permissions"), Some(JsonValue::Object(_)));
    if deny.is_empty() && ask.is_empty() && !has_permissions_object {
        return;
    }
    // Force a well-formed permissions object even if some document left a
    // non-object there, so the accumulated restrictions always land (a repo's
    // non-object `permissions` is already stripped upstream; this also repairs an
    // operator's own malformed value instead of silently dropping the union).
    if !has_permissions_object {
        merged.insert(
            "permissions".to_string(),
            JsonValue::Object(BTreeMap::new()),
        );
    }
    let Some(JsonValue::Object(permissions)) = merged.get_mut("permissions") else {
        return;
    };
    // Never inject category arrays on top of the ordered `rules` form: the two
    // are mutually exclusive and mixing them is a hard parse error. `rules` only
    // ever comes from a trusted scope (Project `rules` is stripped), so the
    // operator is intentionally in ordered mode.
    if permissions
        .get("rules")
        .and_then(JsonValue::as_array)
        .is_some_and(|rules| !rules.is_empty())
    {
        // We don't inject the union here, but a raw deny/ask array that
        // `deep_merge_objects` left (a hostile cloned repo's `deny:[123]` lands
        // beside a trusted `rules`) must STILL be sanitized — otherwise the
        // non-string entry survives to the strict parse and bricks the whole
        // config load in this branch. Drop non-strings; remove an array that
        // becomes empty so a purely-malformed list can't read as the category
        // form coexisting with `rules`.
        for key in ["deny", "ask"] {
            let now_empty = match permissions.get_mut(key) {
                Some(JsonValue::Array(entries)) => {
                    entries.retain(|entry| entry.as_str().is_some());
                    entries.is_empty()
                }
                _ => false,
            };
            if now_empty {
                permissions.remove(key);
            }
        }
        return;
    }
    for (key, list) in [("deny", deny), ("ask", ask)] {
        // Write the string-only union whenever there are entries OR a raw array is
        // already present — so a non-string entry `deep_merge_objects` left behind
        // is replaced by the clean union (empty if the scope's entries were all
        // malformed) rather than reaching the strict parse and DoS-ing the load.
        if !list.is_empty() || permissions.contains_key(key) {
            permissions.insert(key.to_string(), JsonValue::Array(list));
        }
    }
}

/// Merge project `.zo/mcp.json` servers, but only the ones the user has trusted.
///
/// Auto-discovered `.zo/mcp.json` lives in the repository and is therefore
/// attacker-controllable on `git pull`, so each server stays gated until either
/// `enableAllProjectMcpServers` opts the whole project in or its name is listed
/// in `.zo/trusted-mcp-servers.json`. Untrusted servers are skipped (not
/// merged), closing the silent auto-merge supply-chain hole.
fn merge_trusted_project_mcp_servers(
    target: &mut BTreeMap<String, ScopedMcpServerConfig>,
    untrusted: &mut Vec<UntrustedMcpServer>,
    root: &BTreeMap<String, JsonValue>,
    path: &Path,
    enable_all: bool,
    trusted: &BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(servers) = root.get(mcp_keys::SERVERS).and_then(JsonValue::as_object) else {
        return Ok(());
    };
    let mut allowed = BTreeMap::new();
    for (name, value) in servers {
        // A server the operator already defined in a TRUSTED scope (User/Local —
        // e.g. the global `~/.zo/settings.json`) is authoritative: a project
        // config redefining the same NAME must not shadow it. Ignore the project's
        // redefinition entirely, because otherwise it would (a) downgrade the
        // trusted server to Project scope — spamming the "project-scoped config"
        // warning and, without an opt-in, GATING the operator's own global server
        // (the regression this fixes), and (b) let a hostile repo shadow a trusted
        // server's command with the same name. The trusted definition wins.
        if target
            .get(name)
            .is_some_and(|existing| existing.scope != ConfigSource::Project)
        {
            continue;
        }
        if enable_all || trusted.contains(name) {
            allowed.insert(name.clone(), value.clone());
        } else {
            // Skipped by the trust gate. Record it (not merged) so `/mcp` can tell
            // the user the server is gated and how to enable it, instead of the
            // server silently vanishing.
            untrusted.push(UntrustedMcpServer {
                name: name.clone(),
                path: path.to_path_buf(),
            });
        }
    }
    if allowed.is_empty() {
        return Ok(());
    }
    let mut gated_root = BTreeMap::new();
    gated_root.insert(mcp_keys::SERVERS.to_string(), JsonValue::Object(allowed));
    merge_mcp_servers(target, ConfigSource::Project, &gated_root, path)
}

fn parse_enabled_flag(root: &JsonValue, key: &str) -> Result<bool, ConfigError> {
    let Some(object) = root.as_object() else {
        return Ok(true);
    };
    let Some(value) = object.get(key) else {
        return Ok(true);
    };
    value.as_bool().ok_or_else(|| {
        ConfigError::Parse(format!(
            "merged settings.{key}: field {key} must be a boolean"
        ))
    })
}

/// Like [`parse_enabled_flag`] but defaults **off** — for opt-in features that
/// must stay dormant until a user explicitly sets the key to `true`.
fn parse_opt_in_flag(root: &JsonValue, key: &str) -> Result<bool, ConfigError> {
    let Some(value) = root.as_object().and_then(|object| object.get(key)) else {
        return Ok(false);
    };
    value.as_bool().ok_or_else(|| {
        ConfigError::Parse(format!(
            "merged settings.{key}: field {key} must be a boolean"
        ))
    })
}

fn parse_checkpoint_durable(root: &JsonValue) -> Result<bool, ConfigError> {
    let Some(root) = root.as_object() else {
        return Ok(false);
    };
    let Some(checkpoint) = root.get("checkpoint") else {
        return Ok(false);
    };
    let Some(checkpoint) = checkpoint.as_object() else {
        return Err(ConfigError::Parse(
            "merged settings.checkpoint: field checkpoint must be an object".to_string(),
        ));
    };
    let Some(durable) = checkpoint.get("durable") else {
        return Ok(false);
    };
    durable.as_bool().ok_or_else(|| {
        ConfigError::Parse(
            "merged settings.checkpoint.durable: field durable must be a boolean".to_string(),
        )
    })
}

/// Absent at any level (no `tui` object, no `inlineMode` key) yields `None` —
/// "unset", which the presentation layer resolves to its own default. Only an
/// explicit boolean pins the mode.
fn parse_tui_inline_mode(root: &JsonValue) -> Result<Option<bool>, ConfigError> {
    let Some(root) = root.as_object() else {
        return Ok(None);
    };
    let Some(tui) = root.get("tui") else {
        return Ok(None);
    };
    let Some(tui) = tui.as_object() else {
        return Err(ConfigError::Parse(
            "merged settings.tui: field tui must be an object".to_string(),
        ));
    };
    let Some(inline_mode) = tui.get("inlineMode") else {
        return Ok(None);
    };
    inline_mode.as_bool().map(Some).ok_or_else(|| {
        ConfigError::Parse(
            "merged settings.tui.inlineMode: field inlineMode must be a boolean".to_string(),
        )
    })
}

/// Parses the `secondBrain` object: `vault` (an absolute path string) and
/// `writeBack` (a boolean, default `true`).
///
/// A missing object, or either field missing from it, keeps the default — the
/// feature is off without a vault and writes back with one. A field of the
/// wrong type is a parse error like every other typed key, so a hand-edited
/// `"writeBack": "no"` is reported rather than silently read as `false`.
fn parse_second_brain_config(root: &JsonValue) -> Result<RuntimeSecondBrainConfig, ConfigError> {
    use crate::second_brain::{SETTINGS_KEY, VAULT_SETTINGS_FIELD, WRITE_BACK_SETTINGS_FIELD};

    let mut config = RuntimeSecondBrainConfig::default();
    let Some(section) = root.as_object().and_then(|root| root.get(SETTINGS_KEY)) else {
        return Ok(config);
    };
    let Some(section) = section.as_object() else {
        return Err(ConfigError::Parse(format!(
            "merged settings.{SETTINGS_KEY}: field {SETTINGS_KEY} must be an object"
        )));
    };
    if let Some(vault) = section.get(VAULT_SETTINGS_FIELD) {
        let vault = vault.as_str().ok_or_else(|| {
            ConfigError::Parse(format!(
                "merged settings.{SETTINGS_KEY}.{VAULT_SETTINGS_FIELD}: field {VAULT_SETTINGS_FIELD} must be a string"
            ))
        })?;
        config.vault = Some(vault.trim().to_string()).filter(|vault| !vault.is_empty());
    }
    if let Some(write_back) = section.get(WRITE_BACK_SETTINGS_FIELD) {
        config.write_back = write_back.as_bool().ok_or_else(|| {
            ConfigError::Parse(format!(
                "merged settings.{SETTINGS_KEY}.{WRITE_BACK_SETTINGS_FIELD}: field {WRITE_BACK_SETTINGS_FIELD} must be a boolean"
            ))
        })?;
    }
    Ok(config)
}

/// Parses the settings `autoCompactThresholdPercent` override: the full
/// auto-compaction ceiling as a percent of the model context window. Values
/// outside 20–95 are clamped to the nearest bound (not rejected) so a
/// hand-edited `100` degrades to the safe 95 instead of failing the whole
/// config load; non-integer values are a parse error like every other typed
/// key. Absent → `None` (model-family policy defaults apply).
fn parse_auto_compact_threshold_percent(root: &JsonValue) -> Result<Option<u8>, ConfigError> {
    const MIN: i64 = 20;
    const MAX: i64 = 95;
    let Some(object) = root.as_object() else {
        return Ok(None);
    };
    let Some(value) = object.get("autoCompactThresholdPercent") else {
        return Ok(None);
    };
    let raw = value.as_i64().filter(|raw| *raw > 0).ok_or_else(|| {
        ConfigError::Parse(
            "merged settings.autoCompactThresholdPercent: field autoCompactThresholdPercent must be a positive integer percent".to_string(),
        )
    })?;
    Ok(Some(u8::try_from(raw.clamp(MIN, MAX)).unwrap_or(95)))
}

fn parse_team_inbox_digest_max_updates(root: &JsonValue) -> Result<usize, ConfigError> {
    const DEFAULT: usize = 8;
    const MAX: usize = 32;
    let Some(object) = root.as_object() else {
        return Ok(DEFAULT);
    };
    let Some(value) = object.get("teamInboxDigestMaxUpdates") else {
        return Ok(DEFAULT);
    };
    let raw = value.as_i64().ok_or_else(|| {
        ConfigError::Parse(
            "merged settings.teamInboxDigestMaxUpdates: field teamInboxDigestMaxUpdates must be a non-negative integer".to_string(),
        )
    })?;
    if raw < 0 {
        return Err(ConfigError::Parse(
            "merged settings.teamInboxDigestMaxUpdates: field teamInboxDigestMaxUpdates must be a non-negative integer".to_string(),
        ));
    }
    Ok(usize::try_from(raw).unwrap_or(MAX).min(MAX))
}

fn dedupe_discovered_config_entries(entries: Vec<ConfigEntry>) -> Vec<ConfigEntry> {
    let mut seen = BTreeSet::new();
    entries
        .into_iter()
        .filter(|entry| seen.insert(config_entry_identity(&entry.path)))
        .collect()
}

fn config_entry_identity(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize_config_entry_path(path))
}

fn normalize_config_entry_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    core_types::paths::normalize_path_components(&absolute)
}

/// Process-wide config overrides carried in from CLI flags (CC parity):
/// `--settings <file>` adds a highest-precedence settings document and
/// `--strict-mcp-config` restricts MCP servers to the explicit `--mcp-config`.
/// Held in a mutex (not `OnceLock`) so tests can install and clear them.
#[derive(Debug, Clone, Default)]
pub struct CliConfigOverrides {
    pub settings_file: Option<PathBuf>,
    pub strict_mcp_config: bool,
}

fn cli_overrides_cell() -> &'static std::sync::Mutex<CliConfigOverrides> {
    static CELL: std::sync::OnceLock<std::sync::Mutex<CliConfigOverrides>> =
        std::sync::OnceLock::new();
    CELL.get_or_init(|| std::sync::Mutex::new(CliConfigOverrides::default()))
}

fn cli_overrides() -> CliConfigOverrides {
    cli_overrides_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

impl RuntimeConfig {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            merged: BTreeMap::new(),
            loaded_entries: Vec::new(),
            feature_config: RuntimeFeatureConfig::default(),
        }
    }

    #[must_use]
    pub fn merged(&self) -> &BTreeMap<String, JsonValue> {
        &self.merged
    }

    #[must_use]
    pub fn loaded_entries(&self) -> &[ConfigEntry] {
        &self.loaded_entries
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.merged.get(key)
    }

    /// Session-transcript retention in days (CC-parity `cleanupPeriodDays`):
    /// how long to keep transcripts by last-activity date. Missing or
    /// non-numeric → the 30-day default; `0` → `None`, meaning cleanup is
    /// DISABLED (deliberately safer than a literal "retain zero days", which
    /// would silently delete every past session).
    #[must_use]
    pub fn session_retention_days(&self) -> Option<u32> {
        let days = self
            .merged
            .get("cleanupPeriodDays")
            .and_then(JsonValue::as_i64)
            .map_or(
                crate::session_control::DEFAULT_SESSION_RETENTION_DAYS,
                |days| u32::try_from(days).unwrap_or(u32::MAX),
            );
        (days > 0).then_some(days)
    }

    #[must_use]
    pub fn as_json(&self) -> JsonValue {
        JsonValue::Object(self.merged.clone())
    }

    /// The merged `providers` array serialized as a JSON string, ready to inject
    /// into `api`'s `CUSTOM_PROVIDERS_ENV` for the OpenAI-compatible custom
    /// provider path (Ollama / LM Studio / `DeepSeek` / …). Returns `None` unless
    /// a non-empty `providers` array is configured.
    ///
    /// The `api` crate reads custom providers only from that env var — it cannot
    /// depend on runtime config — so the bootstrap mirrors settings into the env
    /// before the first provider client is built (an explicit env var still
    /// wins; see the caller).
    #[must_use]
    pub fn custom_providers_json(&self) -> Option<String> {
        let value = self.merged.get("providers")?;
        match value.as_array() {
            Some(entries) if !entries.is_empty() => Some(value.render()),
            _ => None,
        }
    }

    #[must_use]
    pub fn feature_config(&self) -> &RuntimeFeatureConfig {
        &self.feature_config
    }

    #[must_use]
    pub fn mcp(&self) -> &McpConfigCollection {
        &self.feature_config.mcp
    }

    #[must_use]
    pub fn lsp(&self) -> &LspConfigCollection {
        &self.feature_config.lsp
    }

    #[must_use]
    pub fn hooks(&self) -> &RuntimeHookConfig {
        &self.feature_config.hooks
    }

    #[must_use]
    pub fn plugins(&self) -> &RuntimePluginConfig {
        &self.feature_config.plugins
    }

    #[must_use]
    pub fn review(&self) -> &RuntimeReviewConfig {
        &self.feature_config.review
    }

    #[must_use]
    pub fn ship(&self) -> &RuntimeShipConfig {
        &self.feature_config.ship
    }

    #[must_use]
    pub fn oauth(&self) -> Option<&OAuthConfig> {
        self.feature_config.oauth.as_ref()
    }

    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.feature_config.model.as_deref()
    }

    #[must_use]
    pub fn permission_mode(&self) -> Option<ResolvedPermissionMode> {
        self.feature_config.permission_mode
    }

    #[must_use]
    pub fn permission_rules(&self) -> &RuntimePermissionRuleConfig {
        &self.feature_config.permission_rules
    }

    #[must_use]
    pub fn sandbox(&self) -> &SandboxConfig {
        &self.feature_config.sandbox
    }

    /// Extra environment variables declared under settings.json `env`, injected
    /// into zo-spawned subprocesses. See [`RuntimeFeatureConfig::env`].
    #[must_use]
    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.feature_config.env
    }

    #[must_use]
    pub fn auto_compact_enabled(&self) -> bool {
        self.feature_config.auto_compact_enabled()
    }

    #[must_use]
    pub fn auto_compact_threshold_percent(&self) -> Option<u8> {
        self.feature_config.auto_compact_threshold_percent()
    }

    #[must_use]
    pub fn auto_memory_enabled(&self) -> bool {
        self.feature_config.auto_memory_enabled()
    }

    #[must_use]
    pub fn auto_dream_enabled(&self) -> bool {
        self.feature_config.auto_dream_enabled()
    }

    /// Opt-in: the startup preflight auto-generates a gated `/improve` proposal
    /// (never applies). Only meaningful while `dream_automation_enabled` is
    /// also on — that master switch still gates whether the preflight runs.
    #[must_use]
    pub fn auto_improve_proposals_enabled(&self) -> bool {
        self.feature_config.auto_improve_proposals_enabled()
    }

    #[must_use]
    pub fn checkpoint_durable(&self) -> bool {
        self.feature_config.checkpoint_durable()
    }

    /// `None` when the user never pinned a presentation mode — see
    /// [`RuntimeFeatureConfig::tui_inline_mode`].
    #[must_use]
    pub fn tui_inline_mode(&self) -> Option<bool> {
        self.feature_config.tui_inline_mode()
    }

    /// The merged `secondBrain` settings. See
    /// [`RuntimeFeatureConfig::second_brain`].
    #[must_use]
    pub const fn second_brain(&self) -> &RuntimeSecondBrainConfig {
        self.feature_config.second_brain()
    }

    /// Central kill switch for all Dreamer automation, including natural
    /// self-improve pulses. Kept as an alias so callers express intent without
    /// introducing another user-facing setting.
    #[must_use]
    pub fn dream_automation_enabled(&self) -> bool {
        self.auto_dream_enabled()
    }
}

impl RuntimeFeatureConfig {
    #[must_use]
    pub fn with_hooks(mut self, hooks: RuntimeHookConfig) -> Self {
        self.hooks = hooks;
        self
    }

    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    #[must_use]
    pub fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// Programmatic sibling of the settings `autoCompactThresholdPercent` key.
    /// Stored as given; the runtime's context policy clamps to 20–95 when it
    /// folds the override in, so out-of-range values are safe here too.
    #[must_use]
    pub fn with_auto_compact_threshold_percent(mut self, percent: u8) -> Self {
        self.auto_compact_threshold_percent = Some(percent);
        self
    }

    #[must_use]
    pub fn with_auto_dream_enabled(mut self, enabled: bool) -> Self {
        self.auto_dream_enabled = enabled;
        self
    }

    #[must_use]
    pub fn with_auto_improve_proposals_enabled(mut self, enabled: bool) -> Self {
        self.auto_improve_proposals_enabled = enabled;
        self
    }

    #[must_use]
    pub fn with_recall_hint_enabled(mut self, enabled: bool) -> Self {
        self.recall_hint_enabled = enabled;
        self
    }

    #[must_use]
    pub fn hooks(&self) -> &RuntimeHookConfig {
        &self.hooks
    }

    #[must_use]
    pub fn plugins(&self) -> &RuntimePluginConfig {
        &self.plugins
    }

    #[must_use]
    pub fn review(&self) -> &RuntimeReviewConfig {
        &self.review
    }

    #[must_use]
    pub fn ship(&self) -> &RuntimeShipConfig {
        &self.ship
    }

    #[must_use]
    pub fn mcp(&self) -> &McpConfigCollection {
        &self.mcp
    }

    #[must_use]
    pub fn lsp(&self) -> &LspConfigCollection {
        &self.lsp
    }

    #[must_use]
    pub fn oauth(&self) -> Option<&OAuthConfig> {
        self.oauth.as_ref()
    }

    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    #[must_use]
    pub fn permission_mode(&self) -> Option<ResolvedPermissionMode> {
        self.permission_mode
    }

    #[must_use]
    pub fn permission_rules(&self) -> &RuntimePermissionRuleConfig {
        &self.permission_rules
    }

    #[must_use]
    pub fn sandbox(&self) -> &SandboxConfig {
        &self.sandbox
    }

    /// Extra environment variables from settings.json `env`, injected into every
    /// zo-spawned subprocess (bash, hooks, powershell).
    #[must_use]
    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    #[must_use]
    pub fn auto_compact_enabled(&self) -> bool {
        self.auto_compact_enabled
    }

    #[must_use]
    pub fn auto_compact_threshold_percent(&self) -> Option<u8> {
        self.auto_compact_threshold_percent
    }

    #[must_use]
    pub fn auto_memory_enabled(&self) -> bool {
        self.auto_memory_enabled
    }

    #[must_use]
    pub fn auto_dream_enabled(&self) -> bool {
        self.auto_dream_enabled
    }

    #[must_use]
    pub fn auto_improve_proposals_enabled(&self) -> bool {
        self.auto_improve_proposals_enabled
    }

    #[must_use]
    pub fn team_inbox_digest_enabled(&self) -> bool {
        self.team_inbox_digest_enabled
    }

    #[must_use]
    pub fn team_inbox_digest_max_updates(&self) -> usize {
        self.team_inbox_digest_max_updates
    }

    #[must_use]
    pub fn recall_hint_enabled(&self) -> bool {
        self.recall_hint_enabled
    }

    #[must_use]
    pub fn checkpoint_durable(&self) -> bool {
        self.checkpoint_durable
    }

    /// `Some(true)`/`Some(false)` only when `tui.inlineMode` is set; `None`
    /// leaves the choice to the presentation default (inline).
    #[must_use]
    pub fn tui_inline_mode(&self) -> Option<bool> {
        self.tui_inline_mode
    }

    /// The merged `secondBrain` settings — the vault path and the write-back
    /// switch.
    #[must_use]
    pub const fn second_brain(&self) -> &RuntimeSecondBrainConfig {
        &self.second_brain
    }

    /// Central kill switch for startup Dreamer, natural candidate pulses, and
    /// future self-improve scheduler/runner work. This intentionally aliases the
    /// existing `autoDreamEnabled` setting instead of adding another knob.
    #[must_use]
    pub fn dream_automation_enabled(&self) -> bool {
        self.auto_dream_enabled()
    }
}

impl RuntimePluginConfig {
    #[must_use]
    pub fn enabled_plugins(&self) -> &BTreeMap<String, bool> {
        &self.enabled_plugins
    }

    #[must_use]
    pub fn external_directories(&self) -> &[String] {
        &self.external_directories
    }

    #[must_use]
    pub fn install_root(&self) -> Option<&str> {
        self.install_root.as_deref()
    }

    #[must_use]
    pub fn registry_path(&self) -> Option<&str> {
        self.registry_path.as_deref()
    }

    #[must_use]
    pub fn bundled_root(&self) -> Option<&str> {
        self.bundled_root.as_deref()
    }

    pub fn set_plugin_state(&mut self, plugin_id: String, enabled: bool) {
        self.enabled_plugins.insert(plugin_id, enabled);
    }

    #[must_use]
    pub fn state_for(&self, plugin_id: &str, default_enabled: bool) -> bool {
        self.enabled_plugins
            .get(plugin_id)
            .copied()
            .unwrap_or(default_enabled)
    }
}

impl RuntimeReviewConfig {
    #[must_use]
    pub fn new(auto_after_edits: Option<u32>) -> Self {
        Self {
            auto_after_edits: auto_after_edits.and_then(NonZeroU32::new),
        }
    }

    #[must_use]
    pub fn auto_after_edits(&self) -> Option<u32> {
        self.auto_after_edits.map(NonZeroU32::get)
    }
}

impl RuntimeShipConfig {
    #[must_use]
    pub fn new(gates: Vec<String>, deploy: Option<String>) -> Self {
        Self { gates, deploy }
    }

    #[must_use]
    pub fn gates(&self) -> &[String] {
        &self.gates
    }

    #[must_use]
    pub fn deploy(&self) -> Option<&str> {
        self.deploy.as_deref()
    }
}

#[must_use]
/// Returns the default per-user config directory used by the runtime.
///
/// The first entry of [`zo_global_config_roots`] — i.e. the highest-priority
/// global home (`ZO_CONFIG_HOME`, else `ZO_HOME`, else `~/.zo`). Use
/// [`zo_global_config_roots`] when you need to *search* every configured
/// global home (skills/agents/mcp discovery); use this when you need the single
/// canonical write location (sessions, generated settings).
pub fn default_config_home() -> PathBuf {
    core_types::paths::default_config_home()
}

/// The home this PERSON's sessions use (`~/.zo`), whatever this process was
/// told — the one question [`default_config_home`] cannot answer, asked by
/// the guard that keeps an unarmed process out of it
/// (`tools::smart_router::shadow_ledger`).
#[must_use]
pub fn conventional_config_home() -> Option<PathBuf> {
    core_types::paths::conventional_config_home()
}

/// All per-user global config homes, highest priority first:
/// `ZO_CONFIG_HOME`, then `ZO_HOME`, then `~/.zo`.
///
/// This is the single source of truth for where Zo looks for user-global
/// state — sessions, skills, agents, and MCP config all resolve their global
/// roots through here so the lookup order is identical everywhere (mirroring
/// Claude Code's single global home model, with env overrides on top). Returned
/// paths are de-duplicated; an unset `HOME` simply contributes nothing.
#[must_use]
pub fn zo_global_config_roots() -> Vec<PathBuf> {
    core_types::paths::zo_global_config_roots()
}

/// Base directory for a workspace's per-project `.zo/*` state: the
/// `ZO_STATE_DIR` override if set, else `cwd`. Re-export of
/// [`core_types::paths::zo_state_base`] so crates that depend on `runtime`
/// (the CLI, tools) relocate per-project state through a single override.
#[must_use]
pub fn zo_state_base(cwd: &std::path::Path) -> PathBuf {
    core_types::paths::zo_state_base(cwd)
}

/// Global per-project state directory for workspace-scoped operational state.
///
/// `ZO_STATE_DIR` keeps its legacy precedence/meaning as an explicit base;
/// otherwise state moves out of the worktree into the user-global Zo home,
/// partitioned by a stable workspace slug.
#[must_use]
pub fn zo_project_state_dir(cwd: &std::path::Path) -> PathBuf {
    let slug = project_slug(cwd);
    if let Some(dir) = std::env::var_os(core_types::paths::ZO_STATE_DIR_ENV) {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("projects").join(slug).join("state");
        }
    }
    default_config_home()
        .join("projects")
        .join(slug)
        .join("state")
}

/// The folder under a project's state where every Jev seat zo owns appends
/// its ledger, one file per seat named by the use table's row.
///
/// It lives here rather than in the crate that writes those ledgers because
/// two crates now need to find them: the tools crate, which appends, and this
/// one, which reads a seat's standing before deciding what the system prompt
/// says about skills. A folder name spelled twice is a reader looking in a
/// place nothing writes to.
pub const JEV_LEDGER_DIR: &str = "smart-router";

/// Where a project's Jev ledgers live.
#[must_use]
pub fn jev_ledger_dir(cwd: &std::path::Path) -> PathBuf {
    zo_project_state_dir(cwd).join(JEV_LEDGER_DIR)
}

/// Whether `seat`'s own evidence has raised its `auto` to acting, read back
/// from the transitions its ledger recorded
/// (`zerocode_core::jev::promote::stand_from`).
///
/// One reader, because two programs ask it of the same seat: the system
/// prompt, deciding whether to render the skill index at all, and the seat's
/// own executor, deciding whether a tool result is the judgment's. Two
/// readings of one ledger are two answers to one question, and the one that
/// drifts is the one nobody is looking at.
///
/// A ledger that cannot be read is a seat that has never risen, which is
/// where every seat starts.
#[must_use]
pub fn jev_seat_applies(cwd: &std::path::Path, seat: &zerocode_core::jev::JevUse) -> bool {
    let Ok(text) = std::fs::read_to_string(jev_ledger_dir(cwd).join(seat.ledger)) else {
        return false;
    };
    let rows: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    zerocode_core::jev::promote::stand_from(&rows) == zerocode_core::jev::promote::Stand::Applying
}

/// Whether durable traces (`.zo/turns`, `.zo/dream`) live under the global
/// per-project state directory instead of the working tree. Off by default —
/// the crates' own tests read and write traces beside a scratch cwd — and
/// turned on once by the host that owns the process: a session that leaves
/// `.zo/` in the person's checkout puts zo's own bookkeeping into their
/// `git status` (6/6 runs in the 2026-09-02 benchmark, and once a rename
/// looked unfinished because the old name survived in zo's own trace).
static RELOCATE_TRACES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Root every workspace's traces at its global state directory from now on.
pub fn relocate_traces_out_of_tree() {
    RELOCATE_TRACES.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether a host armed durable traces for this process. The evidence
/// ledgers under the global per-project state (`prompt-cache/breaks.jsonl`,
/// `request-timings/timings.jsonl`) consult this before writing: a runtime
/// nobody armed is a crate test or an embedded host that never asked, and
/// its rows would land in the person's real home as if a session produced
/// them (ten fixture rows did, 2026-09-10).
#[must_use]
pub fn durable_traces_armed() -> bool {
    RELOCATE_TRACES.load(std::sync::atomic::Ordering::Relaxed)
}

/// The directory a workspace's `.zo/` traces are rooted at: the working tree
/// itself, or — once the host asked — that workspace's global state
/// directory. The workspace stays the identity everywhere else (memory,
/// todos, slugs); only the trace files move, so producer and consumer keep
/// agreeing on one `.zo/`.
#[must_use]
pub fn traces_base(cwd: &std::path::Path) -> PathBuf {
    traces_base_from(RELOCATE_TRACES.load(std::sync::atomic::Ordering::Relaxed), cwd)
}

fn traces_base_from(relocated: bool, cwd: &std::path::Path) -> PathBuf {
    if relocated {
        zo_project_state_dir(cwd)
    } else {
        cwd.to_path_buf()
    }
}

#[must_use]
pub fn project_slug(cwd: &std::path::Path) -> String {
    let sanitized: String = cwd
        .to_string_lossy()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let tail_start = sanitized.len().saturating_sub(80);
    let stem = sanitized[tail_start..].trim_matches('-');
    let hash = crate::sandbox::workspace_scratch_key(cwd);
    if stem.is_empty() {
        hash
    } else {
        format!("{stem}-{hash}")
    }
}

impl RuntimeHookConfig {
    #[must_use]
    pub fn new(
        pre_tool_use: Vec<String>,
        post_tool_use: Vec<String>,
        post_tool_use_failure: Vec<String>,
    ) -> Self {
        Self {
            pre_tool_use: pre_tool_use.into_iter().map(HookRule::any).collect(),
            post_tool_use: post_tool_use.into_iter().map(HookRule::any).collect(),
            post_tool_use_failure: post_tool_use_failure
                .into_iter()
                .map(HookRule::any)
                .collect(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_timeout_seconds(mut self, timeout_seconds: u64) -> Self {
        self.timeout_seconds = Some(timeout_seconds);
        self
    }

    /// Append `TurnEnd` (Stop) hook commands. A `TurnEnd` hook that returns a
    /// `followupMessage` drives the [`ConversationRuntime`](crate::ConversationRuntime)
    /// stop-loop (continue until done).
    #[must_use]
    pub fn with_turn_end(mut self, commands: Vec<String>) -> Self {
        self.turn_end
            .extend(commands.into_iter().map(HookRule::any));
        self
    }

    /// Append `CwdChanged` hook commands — fired when a tool batch left the
    /// process in a different working directory than it started in.
    #[must_use]
    pub fn with_cwd_changed(mut self, commands: Vec<String>) -> Self {
        self.cwd_changed
            .extend(commands.into_iter().map(HookRule::any));
        self
    }

    /// Append `TaskCreated` hook commands — a todo item appeared that was not
    /// in the plan at turn entry.
    #[must_use]
    pub fn with_task_created(mut self, commands: Vec<String>) -> Self {
        self.task_created
            .extend(commands.into_iter().map(HookRule::any));
        self
    }

    /// Append `TaskCompleted` hook commands — a todo item reached `completed`.
    #[must_use]
    pub fn with_task_completed(mut self, commands: Vec<String>) -> Self {
        self.task_completed
            .extend(commands.into_iter().map(HookRule::any));
        self
    }

    /// Append `FileChanged` hook commands — a tool created, modified, or
    /// deleted a file.
    #[must_use]
    pub fn with_file_changed(mut self, commands: Vec<String>) -> Self {
        self.file_changed
            .extend(commands.into_iter().map(HookRule::any));
        self
    }

    /// The hook view a spawned sub-agent runs with (Claude Code parity).
    /// Tool, sub-agent lifecycle, compaction, and permission hooks all apply
    /// inside sub-agents; the main-agent-only events are stripped:
    /// `TurnEnd`/`TurnStart` (CC `Stop` fires "when the *main* agent finishes
    /// responding" — a sub-agent's terminal hook is `SubagentStop`),
    /// `UserPromptSubmit` (a *user* prompt, not a programmatic sub-agent
    /// prompt), and `SessionStart`/`SessionEnd` (session-level). Without this
    /// view a user Stop-gate returning a `followupMessage` would re-loop every
    /// sub-agent's narrow task.
    ///
    /// `TaskCreated`/`TaskCompleted` are stripped for the same reason and one
    /// more: the todo plan is a single SESSION-level file that every sub-agent
    /// sharing the workspace reads. Left in, one completion would be announced
    /// once by the parent and once again by each of N concurrent sub-agents
    /// that happened to look afterwards — turning a count an outside watcher
    /// relies on into noise. `CwdChanged` and `FileChanged` do stay: those read
    /// the sub-agent's OWN process state and OWN transcript, so each reports
    /// only what it actually did.
    #[must_use]
    pub fn for_subagent(&self) -> Self {
        let mut view = self.clone();
        view.turn_start = Vec::new();
        view.turn_end = Vec::new();
        view.user_prompt_submit = Vec::new();
        view.session_start = Vec::new();
        view.session_end = Vec::new();
        view.task_created = Vec::new();
        view.task_completed = Vec::new();
        view
    }

    #[must_use]
    pub fn with_subagent_lifecycle(
        mut self,
        start_commands: Vec<String>,
        stop_commands: Vec<String>,
    ) -> Self {
        self.subagent_start
            .extend(start_commands.into_iter().map(HookRule::any));
        self.subagent_stop
            .extend(stop_commands.into_iter().map(HookRule::any));
        self
    }

    #[must_use]
    pub fn pre_tool_use(&self) -> &[HookRule] {
        &self.pre_tool_use
    }
    #[must_use]
    pub fn post_tool_use(&self) -> &[HookRule] {
        &self.post_tool_use
    }
    #[must_use]
    pub fn post_tool_use_failure(&self) -> &[HookRule] {
        &self.post_tool_use_failure
    }
    #[must_use]
    pub fn session_start(&self) -> &[HookRule] {
        &self.session_start
    }
    #[must_use]
    pub fn session_end(&self) -> &[HookRule] {
        &self.session_end
    }
    #[must_use]
    pub fn user_prompt_submit(&self) -> &[HookRule] {
        &self.user_prompt_submit
    }
    #[must_use]
    pub fn pre_compact(&self) -> &[HookRule] {
        &self.pre_compact
    }
    #[must_use]
    pub fn post_compact(&self) -> &[HookRule] {
        &self.post_compact
    }
    #[must_use]
    pub fn subagent_start(&self) -> &[HookRule] {
        &self.subagent_start
    }
    #[must_use]
    pub fn subagent_stop(&self) -> &[HookRule] {
        &self.subagent_stop
    }
    #[must_use]
    pub fn turn_start(&self) -> &[HookRule] {
        &self.turn_start
    }
    #[must_use]
    pub fn turn_end(&self) -> &[HookRule] {
        &self.turn_end
    }
    #[must_use]
    pub fn permission_request(&self) -> &[HookRule] {
        &self.permission_request
    }
    #[must_use]
    pub fn permission_denied(&self) -> &[HookRule] {
        &self.permission_denied
    }
    #[must_use]
    pub fn cwd_changed(&self) -> &[HookRule] {
        &self.cwd_changed
    }
    #[must_use]
    pub fn task_created(&self) -> &[HookRule] {
        &self.task_created
    }
    #[must_use]
    pub fn task_completed(&self) -> &[HookRule] {
        &self.task_completed
    }
    #[must_use]
    pub fn file_changed(&self) -> &[HookRule] {
        &self.file_changed
    }
    #[must_use]
    pub fn notification(&self) -> &[HookRule] {
        &self.notification
    }

    #[must_use]
    pub fn hook_timeout(&self) -> Option<Duration> {
        self.timeout_seconds.map(Duration::from_secs)
    }

    #[must_use]
    pub fn timeout_seconds(&self) -> Option<u64> {
        self.timeout_seconds
    }

    #[must_use]
    pub fn rules_for_event(&self, event: crate::hooks::HookEvent) -> &[HookRule] {
        use crate::hooks::HookEvent;
        match event {
            HookEvent::PreToolUse => &self.pre_tool_use,
            HookEvent::PostToolUse => &self.post_tool_use,
            HookEvent::PostToolUseFailure => &self.post_tool_use_failure,
            HookEvent::SessionStart => &self.session_start,
            HookEvent::SessionEnd => &self.session_end,
            HookEvent::UserPromptSubmit => &self.user_prompt_submit,
            HookEvent::PreCompact => &self.pre_compact,
            HookEvent::PostCompact => &self.post_compact,
            HookEvent::SubagentStart => &self.subagent_start,
            HookEvent::SubagentStop => &self.subagent_stop,
            HookEvent::TurnStart => &self.turn_start,
            HookEvent::TurnEnd => &self.turn_end,
            HookEvent::PermissionRequest => &self.permission_request,
            HookEvent::PermissionDenied => &self.permission_denied,
            HookEvent::CwdChanged => &self.cwd_changed,
            HookEvent::TaskCreated => &self.task_created,
            HookEvent::TaskCompleted => &self.task_completed,
            HookEvent::FileChanged => &self.file_changed,
            HookEvent::Notification => &self.notification,
        }
    }

    /// Commands for `event` whose matcher accepts `tool_name`. Pass `None` for
    /// tool-agnostic lifecycle events (`SessionStart`, `UserPromptSubmit`, …)
    /// where the matcher is meaningless and every command runs — this mirrors
    /// Claude Code's per-tool matcher routing for `PreToolUse`/`PostToolUse`.
    #[must_use]
    pub fn matching_commands(
        &self,
        event: crate::hooks::HookEvent,
        tool_name: Option<&str>,
    ) -> Vec<String> {
        self.rules_for_event(event)
            .iter()
            .filter(|rule| match tool_name {
                Some(name) => rule.matcher.matches(name),
                None => true,
            })
            .map(|rule| rule.command.clone())
            .collect()
    }

    #[must_use]
    pub fn merged(&self, other: &Self) -> Self {
        let mut merged = self.clone();
        merged.extend(other);
        merged
    }

    pub fn extend(&mut self, other: &Self) {
        extend_unique(&mut self.pre_tool_use, other.pre_tool_use());
        extend_unique(&mut self.post_tool_use, other.post_tool_use());
        extend_unique(
            &mut self.post_tool_use_failure,
            other.post_tool_use_failure(),
        );
        extend_unique(&mut self.session_start, other.session_start());
        extend_unique(&mut self.session_end, other.session_end());
        extend_unique(&mut self.user_prompt_submit, other.user_prompt_submit());
        extend_unique(&mut self.pre_compact, other.pre_compact());
        extend_unique(&mut self.post_compact, other.post_compact());
        extend_unique(&mut self.subagent_start, other.subagent_start());
        extend_unique(&mut self.subagent_stop, other.subagent_stop());
        extend_unique(&mut self.turn_start, other.turn_start());
        extend_unique(&mut self.turn_end, other.turn_end());
        extend_unique(&mut self.permission_request, other.permission_request());
        extend_unique(&mut self.permission_denied, other.permission_denied());
        extend_unique(&mut self.cwd_changed, other.cwd_changed());
        extend_unique(&mut self.task_created, other.task_created());
        extend_unique(&mut self.task_completed, other.task_completed());
        extend_unique(&mut self.file_changed, other.file_changed());
        extend_unique(&mut self.notification, other.notification());
        if other.timeout_seconds.is_some() {
            self.timeout_seconds = other.timeout_seconds;
        }
    }
}

impl RuntimePermissionRuleConfig {
    #[must_use]
    pub fn new(allow: Vec<String>, deny: Vec<String>, ask: Vec<String>) -> Self {
        Self {
            allow,
            deny,
            ask,
            rules: Vec::new(),
        }
    }

    /// Attach OpenCode-compatible ordered rules (`"bash(git *)=allow"`), which
    /// supersede the category vectors when present.
    ///
    /// Callers are responsible for pre-validating each spec with
    /// `crate::permissions::validate_decision_rule_spec`; the runtime compiler
    /// silently skips entries that fail to parse (see `PermissionPolicy::with_permission_rules`).
    #[must_use]
    pub fn with_rules(mut self, rules: Vec<String>) -> Self {
        self.rules = rules;
        self
    }

    #[must_use]
    pub fn allow(&self) -> &[String] {
        &self.allow
    }

    #[must_use]
    pub fn deny(&self) -> &[String] {
        &self.deny
    }

    #[must_use]
    pub fn ask(&self) -> &[String] {
        &self.ask
    }

    /// Ordered OpenCode-compatible rules; empty unless configured.
    #[must_use]
    pub fn rules(&self) -> &[String] {
        &self.rules
    }
}

impl McpConfigCollection {
    #[must_use]
    pub fn servers(&self) -> &BTreeMap<String, ScopedMcpServerConfig> {
        &self.servers
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ScopedMcpServerConfig> {
        self.servers.get(name)
    }

    /// Project-scoped servers the trust gate skipped, excluding any name that is
    /// also loaded from a trusted scope (so a server present globally is not
    /// reported as "blocked" just because a project document also declared it).
    /// De-duplicated by name, keeping the first declaring document.
    #[must_use]
    pub fn untrusted_project_servers(&self) -> Vec<&UntrustedMcpServer> {
        let mut seen = BTreeSet::new();
        self.untrusted
            .iter()
            .filter(|entry| !self.servers.contains_key(&entry.name))
            .filter(|entry| seen.insert(entry.name.clone()))
            .collect()
    }
}

impl LspConfigCollection {
    #[must_use]
    pub fn servers(&self) -> &BTreeMap<String, ScopedLspServerConfig> {
        &self.servers
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ScopedLspServerConfig> {
        self.servers.get(name)
    }
}

impl ScopedMcpServerConfig {
    #[must_use]
    pub fn transport(&self) -> McpTransport {
        self.config.transport()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod feature_toggle_tests {
    use super::{parse_enabled_flag, JsonValue, RuntimeFeatureConfig};
    use std::collections::BTreeMap;

    #[test]
    fn auto_feature_flags_default_to_enabled() {
        let config = RuntimeFeatureConfig::default();
        assert!(config.auto_compact_enabled());
        assert!(config.auto_memory_enabled());
        assert!(config.auto_dream_enabled());
        assert!(config.dream_automation_enabled());
        assert!(config.team_inbox_digest_enabled());
        assert_eq!(config.team_inbox_digest_max_updates(), 8);
        assert!(config.recall_hint_enabled());

        let empty = JsonValue::Object(BTreeMap::new());
        assert!(parse_enabled_flag(&empty, "autoCompactEnabled").unwrap());
        assert!(parse_enabled_flag(&empty, "recallHintEnabled").unwrap());
    }

    #[test]
    fn auto_feature_flags_parse_explicit_booleans() {
        let mut object = BTreeMap::new();
        object.insert("autoCompactEnabled".to_string(), JsonValue::Bool(false));
        object.insert("autoMemoryEnabled".to_string(), JsonValue::Bool(false));
        object.insert("autoDreamEnabled".to_string(), JsonValue::Bool(false));
        let value = JsonValue::Object(object);

        assert!(!parse_enabled_flag(&value, "autoCompactEnabled").unwrap());
        assert!(!parse_enabled_flag(&value, "autoMemoryEnabled").unwrap());
        assert!(!parse_enabled_flag(&value, "autoDreamEnabled").unwrap());
        assert!(!RuntimeFeatureConfig::default()
            .with_auto_dream_enabled(false)
            .dream_automation_enabled());
    }

    #[test]
    fn auto_improve_proposals_is_opt_in_off_by_default() {
        // Opt-in: absent key and empty settings both stay off, unlike the
        // default-on flags above.
        assert!(!RuntimeFeatureConfig::default().auto_improve_proposals_enabled());
        let empty = JsonValue::Object(BTreeMap::new());
        assert!(!super::parse_opt_in_flag(&empty, "autoImproveProposalsEnabled").unwrap());

        let mut object = BTreeMap::new();
        object.insert(
            "autoImproveProposalsEnabled".to_string(),
            JsonValue::Bool(true),
        );
        let value = JsonValue::Object(object);
        assert!(super::parse_opt_in_flag(&value, "autoImproveProposalsEnabled").unwrap());
        assert!(RuntimeFeatureConfig::default()
            .with_auto_improve_proposals_enabled(true)
            .auto_improve_proposals_enabled());
    }

    #[test]
    fn auto_feature_flags_reject_non_booleans() {
        let mut object = BTreeMap::new();
        object.insert(
            "autoDreamEnabled".to_string(),
            JsonValue::String("false".to_string()),
        );
        let value = JsonValue::Object(object);

        let error = parse_enabled_flag(&value, "autoDreamEnabled")
            .expect_err("non-bool flag should fail config parsing");
        assert!(error.to_string().contains("autoDreamEnabled"));
    }

    #[test]
    fn auto_compact_threshold_percent_defaults_parses_and_clamps() {
        let empty = JsonValue::Object(BTreeMap::new());
        assert_eq!(
            super::parse_auto_compact_threshold_percent(&empty).unwrap(),
            None,
            "absent key keeps the model-family policy defaults"
        );
        assert_eq!(
            RuntimeFeatureConfig::default().auto_compact_threshold_percent(),
            None
        );

        let mut object = BTreeMap::new();
        object.insert(
            "autoCompactThresholdPercent".to_string(),
            JsonValue::Number(60),
        );
        let value = JsonValue::Object(object.clone());
        assert_eq!(
            super::parse_auto_compact_threshold_percent(&value).unwrap(),
            Some(60)
        );

        // Out-of-range values clamp to the nearest bound instead of failing
        // the whole config load.
        object.insert(
            "autoCompactThresholdPercent".to_string(),
            JsonValue::Number(5),
        );
        let value = JsonValue::Object(object.clone());
        assert_eq!(
            super::parse_auto_compact_threshold_percent(&value).unwrap(),
            Some(20)
        );

        object.insert(
            "autoCompactThresholdPercent".to_string(),
            JsonValue::Number(100),
        );
        let value = JsonValue::Object(object);
        assert_eq!(
            super::parse_auto_compact_threshold_percent(&value).unwrap(),
            Some(95)
        );
    }

    #[test]
    fn auto_compact_threshold_percent_rejects_non_positive_and_non_numbers() {
        let mut object = BTreeMap::new();
        object.insert(
            "autoCompactThresholdPercent".to_string(),
            JsonValue::String("80".to_string()),
        );
        let value = JsonValue::Object(object.clone());
        let error = super::parse_auto_compact_threshold_percent(&value)
            .expect_err("non-numeric percent should fail config parsing");
        assert!(error.to_string().contains("autoCompactThresholdPercent"));

        object.insert(
            "autoCompactThresholdPercent".to_string(),
            JsonValue::Number(0),
        );
        let value = JsonValue::Object(object);
        super::parse_auto_compact_threshold_percent(&value)
            .expect_err("zero/negative percent should fail config parsing");
    }

    #[test]
    fn team_inbox_digest_max_updates_defaults_and_clamps() {
        let empty = JsonValue::Object(BTreeMap::new());
        assert_eq!(super::parse_team_inbox_digest_max_updates(&empty).unwrap(), 8);

        let mut object = BTreeMap::new();
        object.insert("teamInboxDigestMaxUpdates".to_string(), JsonValue::Number(0));
        let value = JsonValue::Object(object.clone());
        assert_eq!(super::parse_team_inbox_digest_max_updates(&value).unwrap(), 0);

        object.insert("teamInboxDigestMaxUpdates".to_string(), JsonValue::Number(99));
        let value = JsonValue::Object(object);
        assert_eq!(super::parse_team_inbox_digest_max_updates(&value).unwrap(), 32);
    }
}

#[cfg(test)]
mod traces_base_tests {
    #[test]
    fn traces_stay_in_the_tree_until_the_host_relocates_them() {
        let cwd = std::path::Path::new("/tmp/some-repo");
        assert_eq!(super::traces_base_from(false, cwd), cwd);
        let moved = super::traces_base_from(true, cwd);
        assert!(moved.ends_with("state"), "{moved:?}");
        assert!(!moved.starts_with(cwd), "{moved:?}");
        assert_eq!(moved, super::zo_project_state_dir(cwd));
    }
}

