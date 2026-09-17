//! zo's sub-agents as PANES of the pane that started them.
//!
//! ## What the person asked for
//!
//! "zo도 에이전트가 소환되면 \[Claude처럼\] 쪼개지거나 하면 좋을 것 같아."
//! In the `ZeroCode` window a Claude leader that starts three helpers ends up
//! beside three real panes, each running a visible CLI, with the lineage drawn
//! in the sidebar. zo, asked for the same three helpers, ran them inside its
//! own process: the only trace was a status line, and nobody could open a
//! child to see what it was reading.
//!
//! ## The road, and why it is a fake tmux
//!
//! The window already knows how to cut a pane for an agent, and the way it is
//! asked is **tmux**: a leader launched in panes mode finds a fake `tmux` at
//! the front of its `PATH`, and every `split-window` it runs becomes a real
//! leaf beside it (`zerocode_core::agent_teams`, `crates/zerocode-shell/src/
//! agent_teams.rs`). That road was built for Claude's Agent Teams, and it is
//! not Claude-shaped: it answers argv, not a vendor. So zo reaches its panes
//! by speaking the same three verbs — `split-window`, `kill-pane`,
//! `list-panes` — instead of inventing a protocol the window would also have
//! to learn.
//!
//! ## The two files, and why they are files
//!
//! A pane is a process, so parent and child cannot share a channel the way two
//! threads do. They share a DIRECTORY: the parent writes `brief.json` (what to
//! do), the child writes `result.json` (what happened), and both are written
//! to a temporary name and renamed, so a reader never sees half a file. The
//! parent waits by stat'ing for the result — 250ms, no watcher crate — because
//! the thing being waited for is a file appearing, and a `kqueue` dependency
//! to learn that sooner would buy nothing a person could see.
//!
//! ## What this module refuses
//!
//! `ZEROCODE_PANE_KEY` is NOT evidence of a team. The window puts it in every
//! shell it opens, agent or not (`ide::reporter`'s own note says so), so a
//! bare `zsh` in which somebody typed `zo` carries one — and a mode decided
//! off it would try to split a pane in a window that never opened a team,
//! against a shim that is not on `PATH`. The evidence is the team's own
//! namespaced variables plus `TMUX_PANE`, which only the window's launch road
//! writes together.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Settings key holding the sub-agent mode (`auto` | `panes` | `inline`).
pub const SETTINGS_MODE_KEY: &str = "subagentMode";
/// Settings object holding the lifecycle numbers ([`Limits`]): every key is a
/// millisecond count under `"subagents"`.
pub const SETTINGS_LIMITS_KEY: &str = "subagents";
/// Environment override for one run, which beats the settings file.
pub const MODE_ENV: &str = "ZO_SUBAGENT_MODE";

/* The window's own launch variables, spelled here because this crate cannot
 * see `zerocode_core` — zo is its own cargo workspace, deliberately. They are
 * a wire contract with the window, so a rename on either side has to be a
 * rename on both; the mode test below is where that would be caught. */
const TEAM_ID_VAR: &str = "ZEROCODE_AGENT_TEAM_ID";
const TEAM_TOKEN_VAR: &str = "ZEROCODE_AGENT_TEAM_TOKEN";
const TEAM_TOKEN_FILE_VAR: &str = "ZEROCODE_AGENT_TEAM_TOKEN_FILE";
/// Which pane this process is, in the window's own name for it. Read as a
/// fallback for `TMUX_PANE`, exactly as the shim reads them.
const TEAM_PANE_VAR: &str = "ZEROCODE_AGENT_TEAM_PANE";

/// How a sub-agent actually runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentMode {
    /// One pane per child, beside the leader's.
    Panes,
    /// Inside the leader's own process — everything zo did before this module.
    Inline,
}

/// What a person may ask for, before the launch is looked at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModeChoice {
    /// Panes where the window opened a team, inline everywhere else.
    #[default]
    Auto,
    /// Panes, and an honest failure where they cannot be cut.
    Panes,
    /// Never panes, even inside a window that offered a team.
    Inline,
}

impl ModeChoice {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "panes" | "pane" | "split" => Some(Self::Panes),
            "inline" | "in-process" | "off" => Some(Self::Inline),
            _ => None,
        }
    }

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Panes => "panes",
            Self::Inline => "inline",
        }
    }
}

/// The team coordinates one launch carries, as evidence rather than as
/// configuration: every field is something the window wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeamEvidence {
    /// `$TMUX` — a multiplexer this process believes it is inside.
    pub tmux: Option<String>,
    /// This process's own pane address (`TMUX_PANE`, or the window's own name
    /// for it).
    pub pane: Option<String>,
    /// The team the window opened for this launch.
    pub team_id: Option<String>,
    /// Whether a capability to speak to that team came with it.
    pub token: bool,
}

impl TeamEvidence {
    /// Read the evidence out of one environment.
    ///
    /// A closure rather than `std::env` so a test can state a launch without
    /// touching the process every other test shares.
    pub fn read(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let held = |name: &str| {
            lookup(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        Self {
            tmux: held("TMUX"),
            pane: held("TMUX_PANE").or_else(|| held(TEAM_PANE_VAR)),
            team_id: held(TEAM_ID_VAR),
            token: held(TEAM_TOKEN_VAR).is_some() || held(TEAM_TOKEN_FILE_VAR).is_some(),
        }
    }

    /// From the real environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self::read(|name| std::env::var(name).ok())
    }

    /// Can this launch cut a pane at all?
    ///
    /// All four together, because each one alone is a different situation: a
    /// `TMUX` with no team is a REAL multiplexer somebody started zo inside,
    /// and a team id with no pane is a ledger team, which reaches its workers
    /// by another road entirely.
    #[must_use]
    pub fn can_split(&self) -> bool {
        self.tmux.is_some() && self.pane.is_some() && self.team_id.is_some() && self.token
    }
}

/// Decide the mode for one launch.
///
/// `on_screen` is whether the front-end is the interactive TUI. `--plain` and
/// `--json` are contracts with a program: their bytes are the answer, and a
/// child whose transcript lands in another pane would be output that caller
/// never receives. `nested` is whether this process is itself a teammate: a
/// child's helpers run inline whatever was asked, so the tree of panes is one
/// level deep — the parent's — and can never feed on itself.
#[must_use]
pub fn decide(choice: ModeChoice, team: &TeamEvidence, on_screen: bool, nested: bool) -> SubagentMode {
    if nested {
        return SubagentMode::Inline;
    }
    match choice {
        ModeChoice::Inline => SubagentMode::Inline,
        ModeChoice::Panes => SubagentMode::Panes,
        ModeChoice::Auto => {
            if team.can_split() && on_screen {
                SubagentMode::Panes
            } else {
                SubagentMode::Inline
            }
        }
    }
}

/// What the person asked for: the environment first, then the settings file.
#[must_use]
pub fn choice_from(env: Option<&str>, settings: &Path) -> ModeChoice {
    env
        .and_then(ModeChoice::parse)
        .or_else(|| {
            std::fs::read_to_string(settings)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|value| {
                    value
                        .get(SETTINGS_MODE_KEY)
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .as_deref()
                .and_then(ModeChoice::parse)
        })
        .unwrap_or_default()
}

/// Whether this process's front-end is a screen a person is looking at.
///
/// Declared by the host at launch and read from worker threads that have no
/// front-end to ask — the same shape, and for the same reason, as
/// [`crate::declared_attendance`]. Never declared (tests, `--plain`, a pipe)
/// reads as false, which keeps the answer inline.
static ON_SCREEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Say whether this process draws an interactive front-end.
pub fn declare_on_screen(on_screen: bool) {
    ON_SCREEN.store(on_screen, std::sync::atomic::Ordering::Relaxed);
}

#[must_use]
pub fn on_screen() -> bool {
    ON_SCREEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether this process IS a child — a teammate running its parent's brief.
///
/// Declared once by the teammate launch and read wherever a decision would
/// otherwise treat the child as a root: a child never cuts panes for its own
/// helpers ([`decide`]) and never runs the host's pre-analysis on the brief
/// it was handed (`decide_host_prelude`). Without this, a teammate whose
/// brief read as a broad task pre-analysed it, spawned a decomposition helper
/// as a pane, whose brief read as broad … — thirty-five panes in ninety
/// seconds before somebody killed the tree ("무한 스폰", 2026-09-07).
static NESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Say that this process runs as a child of another session.
pub fn declare_nested(nested: bool) {
    NESTED.store(nested, std::sync::atomic::Ordering::Relaxed);
}

#[must_use]
pub fn nested() -> bool {
    NESTED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The mode this process runs sub-agents in.
///
/// One place, read at each spawn rather than cached: a person who changes the
/// setting mid-session gets the answer they just wrote, and nothing has to
/// invalidate anything.
#[must_use]
pub fn mode() -> SubagentMode {
    let asked = std::env::var(MODE_ENV).ok();
    let choice = choice_from(
        asked.as_deref(),
        &crate::default_config_home().join("settings.json"),
    );
    decide(choice, &TeamEvidence::from_env(), on_screen(), nested())
}

/* ---- the two files ---- */

/// Schema generation of [`Brief`] and [`TeammateResult`].
///
/// A child is a SEPARATE BINARY, and after an upgrade the one on `PATH` may
/// not be the one that wrote the brief. A version it does not know is refused
/// rather than half-read.
///
/// Generation 2 (t-2513) added the carried harness and the lifecycle fields
/// to the brief, all with defaults: a v1 brief still reads, and the child
/// says `harness: derived` about it rather than pretending the parent chose.
pub const PROTOCOL_VERSION: u32 = 2;

/// The generations this binary reads. A brief from an OLDER parent is
/// honoured with the old behavior; one from a newer parent is refused by name.
pub const READABLE_VERSIONS: [u32; 2] = [1, PROTOCOL_VERSION];

/// What the parent hands one child.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brief {
    pub version: u32,
    /// The agent id the parent's manifest already carries, so a child's pane
    /// and its manifest are the same helper.
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub prompt: String,
    /// The one-line task the spawning tool call described.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "subagentType", default, skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(
        rename = "permissionMode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub permission_mode: Option<String>,
    /// Where the child works. Absent means the parent's own directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(
        rename = "parentSession",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_session: Option<String>,
    #[serde(rename = "toolCallId", default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Where in one message's wave this child is. The first cuts the leader
    /// side by side; the rest stack under it.
    #[serde(rename = "waveIndex", default)]
    pub wave_index: usize,
    /* ---- generation 2 (t-2513): what makes a pane child the SAME agent as
     * an inline one. Every field defaults, so a v1 brief still reads. ---- */
    /// The harness the parent resolved ONCE — the child runs it as written and
    /// never re-derives it (re-derivation is drift). Absent on a v1 brief, in
    /// which case the child falls back to `subagent_type` and says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<ResolvedHarness>,
    /// The parent session's agent registry record, so the child opens the
    /// SAME registry (stamps, reaping and lookups land in the parent's root).
    #[serde(rename = "registryLocator", default, skip_serializing_if = "Option::is_none")]
    pub registry_locator: Option<PathBuf>,
    /// A transcript to continue instead of starting fresh — the resume of a
    /// pane child whose pane had closed.
    #[serde(rename = "resumeTranscript", default, skip_serializing_if = "Option::is_none")]
    pub resume_transcript: Option<PathBuf>,
    /// The parent's own events-channel discovery file: the evidence the child
    /// watches to know its parent is still there.
    #[serde(rename = "parentChannel", default, skip_serializing_if = "Option::is_none")]
    pub parent_channel: Option<PathBuf>,
    /// How long the child may sit idle between turns before it closes itself.
    /// Absent means the child's own [`Limits`].
    #[serde(rename = "idleBudgetMs", default, skip_serializing_if = "Option::is_none")]
    pub idle_budget_ms: Option<u64>,
    /// The number of the first turn this child writes. A fresh child starts
    /// at 1 (`result.json`); a child re-cut to continue a transcript starts
    /// where the parent's count left off, so `result-<n>.json` stays one axis.
    #[serde(rename = "firstTurn", default, skip_serializing_if = "Option::is_none")]
    pub first_turn: Option<u32>,
    /// The carried harness was stored before its source definition changed
    /// and the resume did not ask to refresh it — said out loud rather than
    /// hidden (`launch.harness: "stale"`).
    #[serde(rename = "harnessStale", default, skip_serializing_if = "std::ops::Not::not")]
    pub harness_stale: bool,
}

/// Where a child's harness came from, as the capability snapshot reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessOrigin {
    /// The parent resolved it and the brief carried it.
    Carried,
    /// A v1 brief: the child re-derived the harness from `subagent_type`.
    Derived,
    /// Carried, but its source definition changed since it was stored.
    Stale,
}

impl HarnessOrigin {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Carried => "carried",
            Self::Derived => "derived",
            Self::Stale => "stale",
        }
    }
}

impl Brief {
    /// Where this brief's harness came from.
    #[must_use]
    pub const fn harness_origin(&self) -> HarnessOrigin {
        match (&self.harness, self.harness_stale) {
            (None, _) => HarnessOrigin::Derived,
            (Some(_), true) => HarnessOrigin::Stale,
            (Some(_), false) => HarnessOrigin::Carried,
        }
    }

    /// The turn number the child's first `result` file carries.
    #[must_use]
    pub fn first_turn(&self) -> u32 {
        self.first_turn.unwrap_or(1).max(1)
    }
}

/* ---- the harness ---- */

/// Everything that makes a sub-agent the agent its parent asked for, resolved
/// ONCE by the parent and carried as written (contract 1 of t-2513).
///
/// The inline executor and the pane brief are both fed from the same value
/// — `AgentJob::harness()` in the tools crate — so a child that runs beside
/// its parent is provably the same helper as one that runs inside it:
/// [`Self::digest`] of the two is one number.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedHarness {
    /// The role's system prompt, section by section.
    #[serde(rename = "systemPrompt", default)]
    pub system_prompt: Vec<String>,
    /// The tools the role may call — canonical names.
    #[serde(rename = "allowedTools", default)]
    pub allowed_tools: BTreeSet<String>,
    /// Local inspection Bash policy, carried unchanged into a pane child.
    #[serde(default)]
    pub inspection_shell: bool,
    /// The coarse permission mode, as `PermissionMode::as_str` spells it.
    #[serde(rename = "permissionMode", default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// A custom agent's allow/deny/ask rules. `None` for built-in roles.
    #[serde(rename = "permissionRules", default, skip_serializing_if = "Option::is_none")]
    pub permission_rules: Option<PermissionRules>,
    /// The parent session's MCP tools and the road back to its runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpRoute>,
    /// What was asked for and what the catalog resolved it to.
    #[serde(default)]
    pub model: ModelSelection,
    /// The reasoning tier, in zo's own spelling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// A hard wall-clock budget, when the spawn set one.
    #[serde(rename = "timeBudgetMs", default, skip_serializing_if = "Option::is_none")]
    pub time_budget_ms: Option<u64>,
    /// The structured-output schema the parent expects the answer in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    /// Where the child works.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// The parent session's registry record.
    #[serde(rename = "registryLocator", default, skip_serializing_if = "Option::is_none")]
    pub registry_locator: Option<PathBuf>,
}

/// The model half of a harness: the requested spelling and the resolved id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<String>,
}

/// A custom agent's permission rules, in the settings grammar — the wire
/// shape of [`crate::RuntimePermissionRuleConfig`], whose fields are private.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRules {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ask: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
}

impl From<&crate::RuntimePermissionRuleConfig> for PermissionRules {
    fn from(config: &crate::RuntimePermissionRuleConfig) -> Self {
        Self {
            allow: config.allow().to_vec(),
            deny: config.deny().to_vec(),
            ask: config.ask().to_vec(),
            rules: config.rules().to_vec(),
        }
    }
}

impl PermissionRules {
    /// Back into the runtime's own type, for the enforcer.
    #[must_use]
    pub fn to_config(&self) -> crate::RuntimePermissionRuleConfig {
        crate::RuntimePermissionRuleConfig::new(
            self.allow.clone(),
            self.deny.clone(),
            self.ask.clone(),
        )
        .with_rules(self.rules.clone())
    }
}

/// The parent session's MCP tools as a child sees them: the schemas it
/// advertises, and the coordinates of the parent whose runtime actually
/// answers a call (`mcp.call` on the parent's channel). An inline child
/// reaches the same runtime through the in-process passthrough; this is the
/// same road with a socket in the middle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpRoute {
    #[serde(default)]
    pub tools: Vec<McpTool>,
    /// The parent's events-channel discovery file. Absent when the parent has
    /// no channel — then the schemas are advertised and a call fails by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<PathBuf>,
    /// The channel method that carries a call.
    #[serde(default = "default_mcp_method")]
    pub method: String,
}

fn default_mcp_method() -> String {
    channel_method::MCP_CALL.to_string()
}

/// One MCP tool's schema, as the provider client advertises it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: serde_json::Value,
    /// `PermissionMode::as_str` of the permission the tool requires.
    #[serde(rename = "requiredPermission", default, skip_serializing_if = "Option::is_none")]
    pub required_permission: Option<String>,
}

impl ResolvedHarness {
    /// One number for one harness: the SHA-256 of its canonical JSON, keys
    /// sorted at every level. Two executors fed the same harness print the
    /// same digest, whatever order their serializers walked the maps in.
    #[must_use]
    pub fn digest(&self) -> String {
        use sha2::Digest as _;
        use std::fmt::Write as _;
        let canonical = canonical_json(&serde_json::to_value(self).unwrap_or_default());
        let hash = sha2::Sha256::digest(canonical.as_bytes());
        hash.iter().fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
    }

    /// Where a stored harness lives, beside the child's brief and results.
    #[must_use]
    pub fn path_in(directory: &Path) -> PathBuf {
        directory.join(HARNESS_FILE)
    }

    /// Store this harness atomically in `directory`.
    pub fn write(&self, directory: &Path) -> std::io::Result<PathBuf> {
        let path = Self::path_in(directory);
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        write_atomic(&path, &bytes)?;
        Ok(path)
    }

    /// The harness stored in `directory`, if one is there and reads.
    #[must_use]
    pub fn read(directory: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(Self::path_in(directory)).ok()?;
        serde_json::from_str(&text).ok()
    }
}

/// JSON with every object's keys in sorted order, recursively — the input a
/// digest can be taken over.
fn canonical_json(value: &serde_json::Value) -> String {
    fn write(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push('{');
                for (index, key) in keys.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::String((*key).clone()).to_string());
                    out.push(':');
                    write(&map[*key], out);
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

/// The stored harness's filename inside a child's directory.
pub const HARNESS_FILE: &str = "harness.json";

/* ---- the numbers ---- */

/// Every duration the child lifecycle uses, in one table (design §5).
///
/// Overridden from `settings.json` under [`SETTINGS_LIMITS_KEY`], each key a
/// millisecond count: `idleBudgetMs`, `parentLivenessGraceMs`,
/// `parentLivenessPollMs`, `resultPollMs`, `channelTimeoutMs`,
/// `paneBudgetMs`, `closeGraceMs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// How long a child waits for its parent's next word before it closes.
    pub idle_budget: Duration,
    /// How long the parent's channel may be gone before the child treats the
    /// parent as dead.
    pub parent_liveness_grace: Duration,
    /// How often an idle child looks at its parent's channel.
    pub parent_liveness_poll: Duration,
    /// How often the parent looks for a child's result file.
    pub result_poll: Duration,
    /// Connect-and-answer budget for one call on a child's or parent's channel.
    pub channel_timeout: Duration,
    /// How long a parent waits for one pane turn before ending the pane.
    pub pane_budget: Duration,
    /// After `teammate.close` is accepted, how long the parent gives the child
    /// to write `result-final.json` before it reaches for `kill-pane`.
    pub close_grace: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            idle_budget: Duration::from_secs(30 * 60),
            parent_liveness_grace: Duration::from_secs(10),
            parent_liveness_poll: Duration::from_secs(2),
            result_poll: Duration::from_millis(250),
            channel_timeout: Duration::from_secs(3),
            pane_budget: Duration::from_secs(60 * 60),
            close_grace: Duration::from_secs(2),
        }
    }
}

impl Limits {
    /// The table with the settings overlay applied. A key that is missing or
    /// not a positive integer leaves the default standing.
    #[must_use]
    pub fn from_settings(settings: Option<&serde_json::Value>) -> Self {
        let mut limits = Self::default();
        let Some(table) = settings.and_then(|value| value.get(SETTINGS_LIMITS_KEY)) else {
            return limits;
        };
        let millis = |key: &str| {
            table
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .filter(|value| *value > 0)
                .map(Duration::from_millis)
        };
        for (key, slot) in [
            ("idleBudgetMs", &mut limits.idle_budget),
            ("parentLivenessGraceMs", &mut limits.parent_liveness_grace),
            ("parentLivenessPollMs", &mut limits.parent_liveness_poll),
            ("resultPollMs", &mut limits.result_poll),
            ("channelTimeoutMs", &mut limits.channel_timeout),
            ("paneBudgetMs", &mut limits.pane_budget),
            ("closeGraceMs", &mut limits.close_grace),
        ] {
            if let Some(value) = millis(key) {
                *slot = value;
            }
        }
        limits
    }

    /// The table for this process: `settings.json` in the config home.
    #[must_use]
    pub fn load() -> Self {
        let settings = std::fs::read_to_string(crate::default_config_home().join("settings.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        Self::from_settings(settings.as_ref())
    }
}

/* ---- the channel between them ---- */

/// The channel methods the two sides of a pane speak. The child's server
/// (`ide::channel::wire`) spells the shared ones identically; a test there
/// pins it.
pub mod channel_method {
    /// The identity handshake — also the liveness probe.
    pub const LIST: &str = "session.list";
    /// Put words into the child's turn (or, when it is idle, open its next).
    pub const STEER: &str = "session.steer";
    /// Stop the child's running turn.
    pub const CANCEL_TURN: &str = "session.cancel_turn";
    /// Ask an idle teammate to leave. The child writes `result-final.json`.
    pub const TEAMMATE_CLOSE: &str = "teammate.close";
    /// The parent switched accounts; the child re-reads its credentials.
    pub const AUTH_RELOAD: &str = "auth.reload";
    /// One MCP tool call, routed to the parent session's MCP runtime.
    pub const MCP_CALL: &str = "mcp.call";
}

/// The child's channel file inside its directory — the discovery record
/// (address, token, session id) copied where the parent can find it without
/// knowing the child's pid.
pub const CHANNEL_FILE: &str = "channel.addr";

/// The id every one-shot call on a fresh connection uses: one request per
/// connection, so there is nothing to tell apart.
const REQUEST_ID: u64 = 1;

/// Where one events channel answers: the three lines of a discovery file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelCoordinates {
    pub addr: String,
    pub token: Option<String>,
    pub session_id: String,
}

/// Why a channel call did not get an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelCallError {
    /// No process answered at the address (or the file is gone).
    Unreachable(String),
    /// The other side answered with a JSON-RPC error.
    Rpc { code: i64, message: String },
    /// The other side answered something that is not a response.
    Protocol(String),
}

impl std::fmt::Display for ChannelCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(why) => write!(f, "channel unreachable: {why}"),
            Self::Rpc { code, message } => write!(f, "channel refused ({code}): {message}"),
            Self::Protocol(why) => write!(f, "channel answered nonsense: {why}"),
        }
    }
}

impl ChannelCoordinates {
    /// Read a discovery file: address, token (may be blank), session id.
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let mut lines = text.lines().map(str::trim);
        let addr = lines
            .next()
            .filter(|line| !line.is_empty())
            .ok_or_else(|| format!("{}: no address line", path.display()))?
            .to_string();
        let token = lines
            .next()
            .filter(|line| !line.is_empty())
            .map(str::to_string);
        let session_id = lines.next().unwrap_or_default().to_string();
        Ok(Self {
            addr,
            token,
            session_id,
        })
    }

    /// Write the three lines atomically, readable by this user only.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let mut text = format!("{}\n", self.addr);
        text.push_str(self.token.as_deref().unwrap_or_default());
        text.push('\n');
        text.push_str(&self.session_id);
        text.push('\n');
        write_atomic(path, text.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// One JSON-RPC call over a fresh TCP connection, blocking, bounded by
    /// `timeout` for the connect and for each read. Frames the other side
    /// streams (lines without `jsonrpc`) are skipped until the response with
    /// our id arrives.
    pub fn call(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, ChannelCallError> {
        use std::io::{BufRead as _, Write as _};

        let address: std::net::SocketAddr = self
            .addr
            .parse()
            .map_err(|error| ChannelCallError::Unreachable(format!("{}: {error}", self.addr)))?;
        let mut stream = std::net::TcpStream::connect_timeout(&address, timeout)
            .map_err(|error| ChannelCallError::Unreachable(error.to_string()))?;
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));
        let _ = stream.set_nodelay(true);
        let mut request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": REQUEST_ID,
            "method": method,
        });
        request["params"] = params;
        if let Some(token) = self.token.as_deref() {
            request["token"] = serde_json::Value::String(token.to_string());
        }
        let mut line = request.to_string();
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .map_err(|error| ChannelCallError::Unreachable(error.to_string()))?;
        let mut reader = std::io::BufReader::new(stream);
        let mut answer = String::new();
        loop {
            answer.clear();
            let read = reader
                .read_line(&mut answer)
                .map_err(|error| ChannelCallError::Unreachable(error.to_string()))?;
            if read == 0 {
                return Err(ChannelCallError::Protocol(
                    "the connection closed before a response".to_string(),
                ));
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(answer.trim()) else {
                return Err(ChannelCallError::Protocol(answer.trim().to_string()));
            };
            if value.get("jsonrpc").is_none() || value.get("id").and_then(serde_json::Value::as_u64) != Some(REQUEST_ID) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(ChannelCallError::Rpc {
                    code: error.get("code").and_then(serde_json::Value::as_i64).unwrap_or_default(),
                    message: error
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            return Ok(value.get("result").cloned().unwrap_or(serde_json::Value::Null));
        }
    }

    /// Does a process still answer here? Any answer — even a refusal — is a
    /// live process; only "nobody there" is death.
    #[must_use]
    pub fn alive(&self, timeout: Duration) -> bool {
        !matches!(
            self.call(channel_method::LIST, serde_json::json!({}), timeout),
            Err(ChannelCallError::Unreachable(_))
        )
    }
}

/// Is the parent still there, judged by its discovery file?
///
/// Gone file = the parent left cleanly (its exit guard removes it). File
/// present but nobody answering = the parent was killed. Both are "lost".
#[must_use]
pub fn parent_channel_alive(discovery: &Path, timeout: Duration) -> bool {
    ChannelCoordinates::read(discovery).is_ok_and(|coordinates| coordinates.alive(timeout))
}

/// This process's own discovery file, for the briefs it writes.
///
/// Declared by the host once its channel is open — the same shape as
/// [`declare_on_screen`], and for the same reason: the spawn that writes a
/// brief runs on a worker thread with no front-end to ask.
static PARENT_CHANNEL: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

/// Say where this process's events channel can be found.
pub fn declare_parent_channel(discovery: Option<PathBuf>) {
    if let Ok(mut held) = PARENT_CHANNEL.write() {
        *held = discovery;
    }
}

/// The discovery file this process declared, if any.
#[must_use]
pub fn parent_channel() -> Option<PathBuf> {
    PARENT_CHANNEL.read().ok().and_then(|held| held.clone())
}

/* ---- receipts ---- */

/// What a `SendMessage` to a child actually achieved (contract 2 of t-2513).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SteerReceipt {
    /// The child READ it in the turn that is running.
    Consumed,
    /// The child is alive and the message sits in front of its next turn.
    Queued,
    /// There is no child to read it — gone, or its door is shut.
    Rejected,
}

impl SteerReceipt {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Consumed => "consumed",
            Self::Queued => "queued",
            Self::Rejected => "rejected",
        }
    }

    /// The old boolean, kept for readers that only knew it.
    #[must_use]
    pub const fn delivered(self) -> bool {
        !matches!(self, Self::Rejected)
    }
}

/// One steer's receipt with its reason and, when it landed in a running turn,
/// which turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerOutcome {
    pub receipt: SteerReceipt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<u64>,
}

impl SteerOutcome {
    #[must_use]
    pub fn consumed(turn_id: Option<u64>) -> Self {
        Self {
            receipt: SteerReceipt::Consumed,
            reason: None,
            turn_id,
        }
    }

    #[must_use]
    pub fn queued() -> Self {
        Self {
            receipt: SteerReceipt::Queued,
            reason: None,
            turn_id: None,
        }
    }

    #[must_use]
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self {
            receipt: SteerReceipt::Rejected,
            reason: Some(reason.into()),
            turn_id: None,
        }
    }

    #[must_use]
    pub const fn delivered(&self) -> bool {
        self.receipt.delivered()
    }
}

/// The last receipt an agent's manifest remembers (`lastReceipt`): what the
/// most recent `SendMessage` to it came to, and when. The `subagents` frame
/// and the hook reporter carry it so a window can say "queued since …" beside
/// a helper instead of only "running".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerReceiptRecord {
    pub receipt: SteerReceipt,
    /// Epoch seconds when the receipt was written.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<u64>,
}

impl SteerReceiptRecord {
    /// Stamp an outcome with the clock.
    #[must_use]
    pub fn now(outcome: &SteerOutcome) -> Self {
        Self {
            receipt: outcome.receipt,
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
            reason: outcome.reason.clone(),
            turn_id: outcome.turn_id,
        }
    }
}

/// Send one steer to a child over its channel and read the receipt off the
/// answer: a `turn_id` means it landed in a running turn (`consumed`), none
/// means the child was idle and opened its next turn with it (`queued`).
/// A connection nobody answers, or a door the child keeps shut, is
/// `rejected` with the reason.
#[must_use]
pub fn steer_over_channel(
    channel_file: &Path,
    text: &str,
    timeout: Duration,
) -> SteerOutcome {
    let coordinates = match ChannelCoordinates::read(channel_file) {
        Ok(coordinates) => coordinates,
        Err(why) => return SteerOutcome::rejected(format!("the child's channel file is gone: {why}")),
    };
    match coordinates.call(
        channel_method::STEER,
        serde_json::json!({ "id": coordinates.session_id, "text": text }),
        timeout,
    ) {
        Ok(answer) => match answer.get("turn_id").and_then(serde_json::Value::as_u64) {
            Some(turn_id) => SteerOutcome::consumed(Some(turn_id)),
            None => SteerOutcome::queued(),
        },
        Err(error) => SteerOutcome::rejected(error.to_string()),
    }
}

/// How a child's one turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Exit {
    Ok,
    Error,
    Cancelled,
    /// The child left for good (`result-final.json`); `reason` says why.
    Closed,
}

/// Why a child closed — the `reason` of its `result-final.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    /// The parent's channel stayed gone past `parent_liveness_grace`.
    ParentLost,
    /// Nobody spoke to the child for `idle_budget`.
    IdleBudget,
    /// The parent asked (`teammate.close`).
    ClosedByParent,
    /// A person at the child's keyboard left it (Esc twice, Ctrl-D).
    UserExit,
    /// A fan-out lane whose answer the parent read. The lanes of one
    /// `SpawnMultiAgent` are consumed together in one summary and nothing
    /// more is asked of any of them, so the parent releases the pane the
    /// moment the answer is in its hands (2026-09-07) — a lone `Agent` is a
    /// teammate and keeps its pane idle for `idle_budget` instead.
    LaneDone,
}

impl CloseReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParentLost => "parent_lost",
            Self::IdleBudget => "idle_budget",
            Self::ClosedByParent => "closed_by_parent",
            Self::UserExit => "user_exit",
            Self::LaneDone => "lane_done",
        }
    }
}

/// What the child hands back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(rename = "outputTokens", default)]
    pub output_tokens: u64,
    #[serde(rename = "toolCalls", default)]
    pub tool_calls: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeammateResult {
    pub version: u32,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    pub exit: Exit,
    #[serde(rename = "finalMessage", default)]
    pub final_message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub usage: Usage,
    /// The child's own transcript, so a parent that got nothing can still say
    /// where to look.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<PathBuf>,
    /// Why a `Closed` child left. Absent on every per-turn result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<CloseReason>,
    /// Which turn this result answers, counted from the brief's `first_turn`.
    /// Absent on a v1 child's result, which only ever had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
}

/// The brief's filename inside a child's directory.
pub const BRIEF_FILE: &str = "brief.json";
/// The first turn's result. Later turns are `result-<n>.json`
/// ([`result_file_for_turn`]).
pub const RESULT_FILE: &str = "result.json";
/// The last file a child writes: `exit: closed` and why.
pub const RESULT_FINAL_FILE: &str = "result-final.json";

/// The result file of turn `turn`. Turn 1 keeps the v1 name so an old parent
/// waiting on `result.json` still hears its child's first answer.
#[must_use]
pub fn result_file_for_turn(turn: u32) -> String {
    if turn <= 1 {
        RESULT_FILE.to_string()
    } else {
        format!("result-{turn}.json")
    }
}

/// Write one JSON document so no reader ever sees half of it.
///
/// Temporary file in the SAME directory, then rename: a rename within one
/// filesystem is atomic, and a temporary elsewhere would degrade to a copy
/// across a device boundary — which is exactly the partial file this exists to
/// prevent.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(directory)?;
    let temporary = directory.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map_or_else(|| "document".to_string(), |name| name
                .to_string_lossy()
                .into_owned()),
        std::process::id()
    ));
    std::fs::write(&temporary, bytes)?;
    match std::fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

impl Brief {
    /// Put this brief in `directory`, atomically.
    pub fn write(&self, directory: &Path) -> std::io::Result<PathBuf> {
        let path = directory.join(BRIEF_FILE);
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        write_atomic(&path, &bytes)?;
        Ok(path)
    }

    /// Read the brief a parent left in `directory`.
    ///
    /// A document from a generation this binary does not know is refused by
    /// name, because "the zo on PATH is older than the one that spawned it" is
    /// a sentence somebody can act on and "missing field" is not.
    pub fn read(directory: &Path) -> Result<Self, String> {
        let path = directory.join(BRIEF_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let brief: Self = serde_json::from_str(&text)
            .map_err(|error| format!("{} is not a brief: {error}", path.display()))?;
        if !READABLE_VERSIONS.contains(&brief.version) {
            return Err(format!(
                "{} was written by a different zo (brief version {}, this one speaks {PROTOCOL_VERSION})",
                path.display(),
                brief.version
            ));
        }
        if brief.prompt.trim().is_empty() {
            return Err(format!("{} carries no prompt", path.display()));
        }
        Ok(brief)
    }
}

impl TeammateResult {
    #[must_use]
    pub fn new(agent_id: &str, exit: Exit) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            agent_id: agent_id.to_string(),
            exit,
            final_message: String::new(),
            error: None,
            usage: Usage::default(),
            transcript: None,
            reason: None,
            turn: None,
        }
    }

    /// The closing document: `exit: closed` and why.
    #[must_use]
    pub fn closed(agent_id: &str, reason: CloseReason) -> Self {
        let mut result = Self::new(agent_id, Exit::Closed);
        result.reason = Some(reason);
        result
    }

    /// Write this result under the file its own `turn` names — `result.json`
    /// for turn 1 and for a result that names no turn (a v1 child's).
    pub fn write(&self, directory: &Path) -> std::io::Result<PathBuf> {
        self.write_named(&directory.join(result_file_for_turn(self.turn.unwrap_or(1))))
    }

    /// Write the result of turn `turn` (`result.json`, then `result-<n>.json`).
    pub fn write_turn(&self, directory: &Path, turn: u32) -> std::io::Result<PathBuf> {
        let mut stamped = self.clone();
        stamped.turn = Some(turn.max(1));
        stamped.write_named(&directory.join(result_file_for_turn(turn)))
    }

    /// Write the closing document (`result-final.json`).
    pub fn write_final(&self, directory: &Path) -> std::io::Result<PathBuf> {
        self.write_named(&directory.join(RESULT_FINAL_FILE))
    }

    fn write_named(&self, path: &Path) -> std::io::Result<PathBuf> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        write_atomic(path, &bytes)?;
        Ok(path.to_path_buf())
    }

    /// The first turn's result a child left, or nothing at all.
    ///
    /// `None` for both "not there yet" and "there but unreadable": the parent
    /// keeps waiting either way, and a child that wrote nonsense is settled by
    /// the same deadline as a child that wrote nothing. The rename above is
    /// what makes a torn read impossible in the first place.
    #[must_use]
    pub fn read(directory: &Path) -> Option<Self> {
        Self::read_turn(directory, 1)
    }

    /// The result of turn `turn`, if the child has written it.
    #[must_use]
    pub fn read_turn(directory: &Path, turn: u32) -> Option<Self> {
        Self::read_named(&directory.join(result_file_for_turn(turn)))
    }

    /// The closing document, if the child has left.
    #[must_use]
    pub fn read_final(directory: &Path) -> Option<Self> {
        Self::read_named(&directory.join(RESULT_FINAL_FILE))
    }

    fn read_named(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let result: Self = serde_json::from_str(&text).ok()?;
        READABLE_VERSIONS.contains(&result.version).then_some(result)
    }
}

/* ---- the multiplexer ---- */

/// The `tmux` this process reaches its panes through.
///
/// In a `ZeroCode` pane it is the window's fake one, found at the front of
/// `PATH` (`crates/zerocode-shell/src/agent_teams.rs`). Nothing here knows
/// that, and nothing should: the contract is tmux's own argv, which is what
/// makes a fake and a real one interchangeable — and what lets a test put a
/// three-line shell script in front of both.
#[derive(Debug, Clone)]
pub struct Tmux {
    program: std::ffi::OsString,
}

impl Default for Tmux {
    fn default() -> Self {
        Self::on_path()
    }
}

impl Tmux {
    /// Whatever `PATH` answers for `tmux`, resolved at spawn time.
    #[must_use]
    pub fn on_path() -> Self {
        Self {
            program: std::ffi::OsString::from("tmux"),
        }
    }

    /// One named binary — a test's recorder, or a person's own tmux.
    #[must_use]
    pub fn at(program: impl Into<std::ffi::OsString>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// What will actually be run. A bare `tmux` is resolved by `PATH` at spawn
    /// time, which is the whole point: the window's shim goes in front.
    #[must_use]
    pub fn program(&self) -> &std::ffi::OsStr {
        &self.program
    }

    fn run(&self, args: &[String]) -> Result<String, String> {
        let output = std::process::Command::new(&self.program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| format!("could not run tmux: {error}"))?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said.trim();
        Err(if said.is_empty() {
            format!("tmux {} failed", args.first().map_or("", String::as_str))
        } else {
            said.to_string()
        })
    }

    /// Cut a pane and start a teammate in it. Answers the new pane's id.
    pub fn split(&self, spec: &SplitSpec<'_>) -> Result<String, String> {
        let answer = self.run(&split_argv(spec))?;
        let pane = answer.trim();
        if pane.is_empty() {
            return Err("tmux cut a pane and did not say which".to_string());
        }
        Ok(pane.to_string())
    }

    /// End one pane. `false` when tmux refused — the pane may already be gone,
    /// which is the same outcome.
    #[must_use]
    pub fn kill_pane(&self, pane: &str) -> bool {
        self.run(&[
            "kill-pane".to_string(),
            "-t".to_string(),
            pane.to_string(),
        ])
        .is_ok()
    }

    /// Is this pane still one of the team's?
    ///
    /// The question a parent asks about a child that has written nothing: a
    /// pane that is gone with no `result.json` is a child that died, and
    /// waiting out the whole budget for it would hide that for an hour.
    #[must_use]
    pub fn pane_exists(&self, pane: &str) -> bool {
        let Ok(listed) = self.run(&[
            "list-panes".to_string(),
            "-F".to_string(),
            "#{pane_id}".to_string(),
        ]) else {
            // tmux could not be asked. That is not evidence the pane died, and
            // treating it as such would end a child that is still working.
            return true;
        };
        listed.lines().any(|line| line.trim() == pane)
    }
}

/// The environment pair a split hands its pane: which helper the pane is.
///
/// The value is the manifest's `agent_id`. The window reads the pair off the
/// `split-window` argv (`zerocode_core::agent_teams::plan`) and never from
/// the pane's environment — a child pane does not inherit the parent's
/// environment, and the fold has to be known the moment the pane is cut.
pub const ZO_AGENT_ID_VAR: &str = "ZO_AGENT_ID";

/// One split, before it is words.
#[derive(Debug, Clone)]
pub struct SplitSpec<'a> {
    /// The child's directory — where its brief already is, and where its
    /// result will be.
    pub directory: &'a Path,
    /// The child's identity — the manifest's `agent_id`, the same word the
    /// `SubagentStart` hook already carries. Ridden on the split as
    /// `-e ZO_AGENT_ID=<id>` so the window can fold the helper row the hook
    /// stood and the pane row the split stands into ONE row (t-3024).
    pub agent_id: &'a str,
    /// The zo to run. [`teammate_program`] is what production passes.
    pub program: &'a str,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    /// Which child of one message's wave this is.
    pub wave_index: usize,
    /// A transcript the child continues instead of starting fresh — the
    /// re-cut pane of a resumed child. Also in the brief; on the command line
    /// so a person reading the pane's argv can see it is a continuation.
    pub resume_transcript: Option<&'a Path>,
}

/// The words one split is.
///
/// `-h` for the first child and `-v` for the rest is Claude's measured shape,
/// and the window redirects it into the column beside the leader
/// (`resolve_split_target`): the leader is divided ONCE and the helpers stack.
/// `-d` keeps the keyboard where it is — a pane that steals focus mid-sentence
/// is the one thing an orchestration must not do. `-P -F #{pane_id}` is how
/// the id comes back, and the id is the only handle a cancelled turn has.
/// `-e ZO_AGENT_ID=<id>` is tmux's own way of handing a new pane an
/// environment pair, and it is how the window learns WHICH helper this pane
/// is: the same id the `SubagentStart` hook named, so the sidebar draws the
/// helper once instead of a hook row beside a pane row (t-3024).
/// `--` because everything after it is zo's command line, not tmux's.
#[must_use]
pub fn split_argv(spec: &SplitSpec<'_>) -> Vec<String> {
    let mut argv = vec![
        "split-window".to_string(),
        if spec.wave_index == 0 { "-h" } else { "-v" }.to_string(),
        "-d".to_string(),
        "-P".to_string(),
        "-F".to_string(),
        "#{pane_id}".to_string(),
        "-e".to_string(),
        format!("{ZO_AGENT_ID_VAR}={}", spec.agent_id),
        "--".to_string(),
        spec.program.to_string(),
        "--teammate".to_string(),
        spec.directory.to_string_lossy().into_owned(),
    ];
    if let Some(model) = spec.model.map(str::trim).filter(|one| !one.is_empty()) {
        argv.push("--model".to_string());
        argv.push(model.to_string());
    }
    if let Some(effort) = spec.effort.map(str::trim).filter(|one| !one.is_empty()) {
        argv.push("--effort".to_string());
        argv.push(effort.to_string());
    }
    if let Some(transcript) = spec.resume_transcript {
        argv.push("--resume-transcript".to_string());
        argv.push(transcript.to_string_lossy().into_owned());
    }
    argv
}

/// Which zo a teammate pane runs.
///
/// This process's own binary, because a child that is a DIFFERENT zo than its
/// parent would read a brief in a protocol it may not speak — an upgrade that
/// replaced `zo` on `PATH` mid-session is exactly when that happens. A path
/// this window would have to re-split into words is refused back to the bare
/// name: the command crosses to the window as ONE STRING, and a program with
/// a space in it would arrive as two.
#[must_use]
pub fn teammate_program() -> String {
    std::env::current_exe()
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
        .filter(|path| {
            !path.is_empty()
                && !path.chars().any(char::is_whitespace)
                && !path.contains([';', '&', '|', '<', '>', '`', '"', '\''])
        })
        .unwrap_or_else(|| "zo".to_string())
}

/// How a child's pane ended, from the parent's side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneOutcome {
    /// The child wrote a result.
    Finished(Box<TeammateResult>),
    /// The pane is gone and nothing was written — the child died.
    Vanished,
    /// The parent's turn was cancelled and the pane was ended.
    Cancelled,
    /// The budget ran out with the pane still standing.
    TimedOut,
    /// The child wrote `result-final.json` instead of this turn's result: it
    /// left (parent lost, idle budget, closed) before answering.
    Closed(Box<TeammateResult>),
}

/// How often the parent looks for `result.json`.
///
/// A `stat` every quarter second for a job measured in minutes: the cost is
/// invisible, and the alternative is a filesystem-watcher dependency to learn
/// the same fact a few milliseconds sooner.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// Wait for one child, ending its pane if the parent's turn is cancelled.
///
/// Every exit checks for a result FIRST. A child that wrote its answer in the
/// same instant its pane closed — which is exactly what a well-behaved child
/// does, since it writes and then exits — must be read as finished, not as a
/// death or a cancellation.
pub fn wait_for_result(
    tmux: &Tmux,
    directory: &Path,
    pane: &str,
    budget: std::time::Duration,
    cancelled: &dyn Fn() -> bool,
) -> PaneOutcome {
    wait_for_turn_result(tmux, directory, pane, 1, budget, cancelled, &|| {})
}

/// [`wait_for_result`] for turn `turn` of a child that stays alive between
/// turns.
///
/// `close` is what the parent does INSTEAD of `kill-pane` first when its turn
/// is cancelled or the budget runs out: the polite road, `session.cancel_turn`
/// and then `teammate.close` over the child's channel. The pane is still
/// killed after it, so a child that ignored the door is not left standing.
#[allow(clippy::too_many_arguments)] // one wait, one table of ways it ends
pub fn wait_for_turn_result(
    tmux: &Tmux,
    directory: &Path,
    pane: &str,
    turn: u32,
    budget: std::time::Duration,
    cancelled: &dyn Fn() -> bool,
    close: &dyn Fn(),
) -> PaneOutcome {
    let started = std::time::Instant::now();
    let answered = || {
        TeammateResult::read_turn(directory, turn)
            .map(|result| PaneOutcome::Finished(Box::new(result)))
            .or_else(|| TeammateResult::read_final(directory).map(|result| PaneOutcome::Closed(Box::new(result))))
    };
    loop {
        if let Some(outcome) = answered() {
            return outcome;
        }
        if cancelled() {
            // The door first, then the pane. A pane that is already gone
            // answers `false`, which is the same outcome as one this ended.
            close();
            let _ = tmux.kill_pane(pane);
            return answered().unwrap_or(PaneOutcome::Cancelled);
        }
        if started.elapsed() >= budget {
            close();
            let _ = tmux.kill_pane(pane);
            return answered().unwrap_or(PaneOutcome::TimedOut);
        }
        if !tmux.pane_exists(pane) {
            // The child may have written between the read above and this ask.
            return answered().unwrap_or(PaneOutcome::Vanished);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |name: &str| {
            owned
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    /// The window's launch, as it really arrives.
    fn a_team() -> Vec<(&'static str, &'static str)> {
        vec![
            ("TMUX", "/tmp/zerocode-agent-teams/team-7,0,1"),
            ("TMUX_PANE", "%1"),
            ("ZEROCODE_AGENT_TEAM_ID", "team-7"),
            ("ZEROCODE_AGENT_TEAM_TOKEN", "s3cret"),
        ]
    }

    #[test]
    fn auto_splits_only_where_the_window_opened_a_team_and_a_person_can_watch() {
        let inside = TeamEvidence::read(env(&a_team()));
        assert_eq!(
            decide(ModeChoice::Auto, &inside, true, false),
            SubagentMode::Panes
        );
        // The same window, but the front-end is a contract with a program.
        assert_eq!(
            decide(ModeChoice::Auto, &inside, false, false),
            SubagentMode::Inline,
            "`--plain`/`--json` bytes are the answer; a child's pane is not"
        );
        // A bare terminal: no team, nothing to split.
        assert_eq!(
            decide(ModeChoice::Auto, &TeamEvidence::default(), true, false),
            SubagentMode::Inline
        );
    }

    /// A teammate is a child: its helpers run inline whatever the team
    /// evidence, the screen, or even an explicit `panes` say — the pane tree is
    /// one level deep. Before this a teammate whose brief read as broad
    /// pre-analysed it into another pane teammate, which did the same
    /// (thirty-five panes in ninety seconds, 2026-09-07).
    #[test]
    fn a_teammate_process_never_cuts_panes() {
        let inside = TeamEvidence::read(env(&a_team()));
        assert_eq!(decide(ModeChoice::Auto, &inside, true, true), SubagentMode::Inline);
        assert_eq!(decide(ModeChoice::Panes, &inside, true, true), SubagentMode::Inline);
        // The root beside it keeps its answer.
        assert_eq!(decide(ModeChoice::Auto, &inside, true, false), SubagentMode::Panes);
    }

    #[test]
    fn every_coordinate_of_the_team_has_to_be_there() {
        // Each of the four alone is a DIFFERENT launch, and none of them can
        // cut a pane.
        for missing in [
            "TMUX",
            "TMUX_PANE",
            "ZEROCODE_AGENT_TEAM_ID",
            "ZEROCODE_AGENT_TEAM_TOKEN",
        ] {
            let partial: Vec<(&str, &str)> = a_team()
                .into_iter()
                .filter(|(key, _)| *key != missing)
                .collect();
            let evidence = TeamEvidence::read(env(&partial));
            assert_eq!(
                decide(ModeChoice::Auto, &evidence, true, false),
                SubagentMode::Inline,
                "a launch without {missing} still tried to split"
            );
        }
        // A real tmux somebody started zo inside is not our team.
        let real = TeamEvidence::read(env(&[("TMUX", "/tmp/tmux-501/default,90,0"), ("TMUX_PANE", "%3")]));
        assert_eq!(decide(ModeChoice::Auto, &real, true, false), SubagentMode::Inline);
        // The token may arrive as a path instead of a value.
        let by_file: Vec<(&str, &str)> = a_team()
            .into_iter()
            .filter(|(key, _)| *key != "ZEROCODE_AGENT_TEAM_TOKEN")
            .chain([("ZEROCODE_AGENT_TEAM_TOKEN_FILE", "/tmp/tok")])
            .collect();
        assert!(TeamEvidence::read(env(&by_file)).can_split());
        // And the window's own name for a pane answers when tmux's is absent.
        let ours: Vec<(&str, &str)> = a_team()
            .into_iter()
            .filter(|(key, _)| *key != "TMUX_PANE")
            .chain([("ZEROCODE_AGENT_TEAM_PANE", "%1")])
            .collect();
        assert!(TeamEvidence::read(env(&ours)).can_split());
    }

    /// The refusal this module exists to keep.
    #[test]
    fn a_pane_key_alone_is_never_evidence_of_a_team() {
        // Every shell this window opens carries one, agent or not. A mode
        // decided off it would try to split from a pane in a window that
        // opened no team, against a shim that is not on `PATH`.
        let bare = TeamEvidence::read(env(&[
            ("ZEROCODE_PANE_KEY", "pane-3"),
            ("ZEROCODE_HOOK_PORT", "51234"),
        ]));
        assert!(!bare.can_split());
        assert_eq!(decide(ModeChoice::Auto, &bare, true, false), SubagentMode::Inline);
    }

    #[test]
    fn what_a_person_said_beats_what_the_launch_looks_like() {
        let inside = TeamEvidence::read(env(&a_team()));
        assert_eq!(
            decide(ModeChoice::Inline, &inside, true, false),
            SubagentMode::Inline
        );
        // And the other way: asked for panes, they are what is attempted —
        // the failure to cut one is reported, not quietly turned inline.
        assert_eq!(
            decide(ModeChoice::Panes, &TeamEvidence::default(), false, false),
            SubagentMode::Panes
        );
    }

    #[test]
    fn the_environment_beats_the_settings_file_and_nonsense_beats_nothing() {
        let home = tempfile::tempdir().expect("tempdir");
        let settings = home.path().join("settings.json");
        std::fs::write(&settings, br#"{"subagentMode": "inline"}"#).expect("write");
        assert_eq!(choice_from(None, &settings), ModeChoice::Inline);
        assert_eq!(
            choice_from(Some("panes"), &settings),
            ModeChoice::Panes
        );
        // An unreadable answer is no answer — the default stands rather than
        // a typo silently turning the feature off.
        assert_eq!(
            choice_from(Some("sideways"), &home.path().join("absent.json")),
            ModeChoice::Auto
        );
        std::fs::write(&settings, br#"{"subagentMode": 7}"#).expect("write");
        assert_eq!(choice_from(None, &settings), ModeChoice::Auto);
    }

    #[test]
    fn a_brief_and_a_result_survive_the_round_trip_and_a_stranger_does_not() {
        let directory = tempfile::tempdir().expect("tempdir");
        let brief = Brief {
            version: PROTOCOL_VERSION,
            agent_id: "agent-1".to_string(),
            prompt: "read the map and report".to_string(),
            description: "recon".to_string(),
            subagent_type: Some("Explore".to_string()),
            name: Some("recon-graph".to_string()),
            model: Some("claude-fable-5-1".to_string()),
            effort: Some("high".to_string()),
            permission_mode: Some("workspace-write".to_string()),
            cwd: Some(PathBuf::from("/tmp/tree")),
            parent_session: Some("session-9".to_string()),
            tool_call_id: Some("toolu_1".to_string()),
            wave_index: 1,
            ..Brief::default()
        };
        brief.write(directory.path()).expect("write brief");
        assert_eq!(Brief::read(directory.path()).expect("read brief"), brief);

        let mut result = TeammateResult::new("agent-1", Exit::Ok);
        result.final_message = "the map has three edges".to_string();
        result.usage = Usage {
            output_tokens: 812,
            tool_calls: 4,
        };
        result.transcript = Some(PathBuf::from("/tmp/agent-1.session.jsonl"));
        result.write(directory.path()).expect("write result");
        assert_eq!(
            TeammateResult::read(directory.path()).expect("read result"),
            result
        );

        // A document from another generation is refused by name, not
        // half-read: the zo that reads it may not be the one that wrote it.
        let stranger = directory.path().join(BRIEF_FILE);
        std::fs::write(&stranger, br#"{"version": 99, "agentId": "a", "prompt": "p"}"#)
            .expect("write");
        let refused = Brief::read(directory.path()).expect_err("a stranger was accepted");
        assert!(refused.contains("different zo"), "{refused}");
        std::fs::write(directory.path().join(RESULT_FILE), br#"{"version": 99}"#).expect("write");
        assert!(TeammateResult::read(directory.path()).is_none());
    }

    /// A harness with every field a custom role can carry.
    fn a_harness() -> ResolvedHarness {
        ResolvedHarness {
            inspection_shell: false,
            system_prompt: vec!["You are the reviewer.".to_string(), "Stop when done.".to_string()],
            allowed_tools: ["read_file", "grep_search", "mcp__ctx7__query"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            permission_mode: Some("read-only".to_string()),
            permission_rules: Some(PermissionRules {
                allow: vec!["read_file".to_string()],
                deny: vec!["bash(rm *)".to_string()],
                ask: Vec::new(),
                rules: vec!["bash(git *)=allow".to_string()],
            }),
            mcp: Some(McpRoute {
                tools: vec![McpTool {
                    name: "mcp__ctx7__query".to_string(),
                    description: Some("docs lookup".to_string()),
                    input_schema: serde_json::json!({"type": "object", "properties": {"q": {"type": "string"}}}),
                    required_permission: Some("prompt".to_string()),
                }],
                channel: Some(PathBuf::from("/tmp/zo-events-1.addr")),
                method: channel_method::MCP_CALL.to_string(),
            }),
            model: ModelSelection {
                requested: Some("fable".to_string()),
                effective: Some("claude-fable-5-1".to_string()),
            },
            effort: Some("high".to_string()),
            time_budget_ms: Some(60_000),
            schema: Some(serde_json::json!({"type": "object", "properties": {"b": {}, "a": {}}})),
            cwd: Some(PathBuf::from("/tmp/tree")),
            registry_locator: Some(PathBuf::from("/tmp/agents/registries/session-9.json")),
        }
    }

    /// Generation 2: the harness and the lifecycle fields ride the brief and
    /// come back as written.
    #[test]
    fn a_v2_brief_carries_the_harness_and_the_lifecycle_fields_round_trip() {
        let directory = tempfile::tempdir().expect("tempdir");
        let brief = Brief {
            version: PROTOCOL_VERSION,
            agent_id: "agent-2".to_string(),
            prompt: "review the patch".to_string(),
            harness: Some(a_harness()),
            registry_locator: Some(PathBuf::from("/tmp/agents/registries/session-9.json")),
            resume_transcript: Some(PathBuf::from("/tmp/sessions/agent-2.jsonl")),
            parent_channel: Some(PathBuf::from("/tmp/zo-events-1.addr")),
            idle_budget_ms: Some(120_000),
            first_turn: Some(3),
            ..Brief::default()
        };
        brief.write(directory.path()).expect("write brief");
        let read = Brief::read(directory.path()).expect("read brief");
        assert_eq!(read, brief);
        assert_eq!(read.harness_origin(), HarnessOrigin::Carried);
        assert_eq!(read.first_turn(), 3);
        // The document says its generation and spells the new keys as the
        // child reads them.
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(directory.path().join(BRIEF_FILE)).unwrap()).unwrap();
        assert_eq!(raw["version"], 2);
        assert_eq!(raw["harness"]["systemPrompt"][0], "You are the reviewer.");
        assert_eq!(raw["harness"]["permissionRules"]["deny"][0], "bash(rm *)");
        assert_eq!(raw["harness"]["mcp"]["tools"][0]["name"], "mcp__ctx7__query");
        assert_eq!(raw["registryLocator"], "/tmp/agents/registries/session-9.json");
        assert_eq!(raw["parentChannel"], "/tmp/zo-events-1.addr");
        assert_eq!(raw["idleBudgetMs"], 120_000);
    }

    /// A brief from an OLD parent still reads, with the old meaning: the
    /// child derives its harness and says so.
    #[test]
    fn a_v1_brief_still_reads_and_reports_a_derived_harness() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            directory.path().join(BRIEF_FILE),
            br#"{"version": 1, "agentId": "agent-old", "prompt": "map the edges", "subagentType": "Explore"}"#,
        )
        .expect("write v1 brief");
        let brief = Brief::read(directory.path()).expect("a v1 brief reads");
        assert_eq!(brief.version, 1);
        assert!(brief.harness.is_none());
        assert_eq!(brief.harness_origin(), HarnessOrigin::Derived);
        assert_eq!(brief.first_turn(), 1);
        assert!(brief.parent_channel.is_none());
        // A carried harness whose source moved is said out loud.
        let stale = Brief {
            harness: Some(a_harness()),
            harness_stale: true,
            ..Brief::default()
        };
        assert_eq!(stale.harness_origin(), HarnessOrigin::Stale);
    }

    /// One harness, one number — whatever order the maps were built in.
    #[test]
    fn the_harness_digest_is_one_number_for_one_harness() {
        let harness = a_harness();
        let same = a_harness();
        assert_eq!(harness.digest(), same.digest());
        assert_eq!(harness.digest().len(), 64, "sha-256 hex");
        // Key order inside the schema does not move the digest …
        let mut reordered = a_harness();
        reordered.schema = Some(serde_json::json!({"properties": {"a": {}, "b": {}}, "type": "object"}));
        assert_eq!(reordered.digest(), harness.digest());
        // … but a different tool set, prompt or rule does.
        let mut other_tools = a_harness();
        other_tools.allowed_tools.insert("bash".to_string());
        assert_ne!(other_tools.digest(), harness.digest());
        let mut other_prompt = a_harness();
        other_prompt.system_prompt.push("Also write tests.".to_string());
        assert_ne!(other_prompt.digest(), harness.digest());
        let mut other_rules = a_harness();
        other_rules.permission_rules = None;
        assert_ne!(other_rules.digest(), harness.digest());
        // And the stored copy digests the same as the one in memory.
        let directory = tempfile::tempdir().expect("tempdir");
        harness.write(directory.path()).expect("write harness");
        assert_eq!(ResolvedHarness::read(directory.path()).expect("read").digest(), harness.digest());
        assert!(ResolvedHarness::read(&directory.path().join("absent")).is_none());
    }

    /// The rule config round-trips through its wire shape.
    #[test]
    fn permission_rules_round_trip_through_the_runtime_type() {
        let rules = PermissionRules {
            allow: vec!["read_file".to_string()],
            deny: vec!["bash(rm *)".to_string()],
            ask: vec!["write_file".to_string()],
            rules: vec!["bash(git *)=allow".to_string()],
        };
        let config = rules.to_config();
        assert_eq!(config.allow(), ["read_file".to_string()]);
        assert_eq!(config.deny(), ["bash(rm *)".to_string()]);
        assert_eq!(config.ask(), ["write_file".to_string()]);
        assert_eq!(config.rules(), ["bash(git *)=allow".to_string()]);
        assert_eq!(PermissionRules::from(&config), rules);
    }

    /// One table of numbers, one settings object over it.
    #[test]
    fn the_limits_table_reads_its_settings_overlay_and_ignores_nonsense() {
        let defaults = Limits::default();
        assert_eq!(Limits::from_settings(None), defaults);
        let settings = serde_json::json!({
            "subagents": {
                "idleBudgetMs": 5000,
                "parentLivenessGraceMs": 1500,
                "parentLivenessPollMs": 0,
                "channelTimeoutMs": "fast",
                "closeGraceMs": 700
            }
        });
        let limits = Limits::from_settings(Some(&settings));
        assert_eq!(limits.idle_budget, Duration::from_millis(5000));
        assert_eq!(limits.parent_liveness_grace, Duration::from_millis(1500));
        assert_eq!(limits.close_grace, Duration::from_millis(700));
        // Zero and a word are not durations — the defaults stand.
        assert_eq!(limits.parent_liveness_poll, defaults.parent_liveness_poll);
        assert_eq!(limits.channel_timeout, defaults.channel_timeout);
        assert_eq!(limits.result_poll, defaults.result_poll);
        assert_eq!(limits.pane_budget, defaults.pane_budget);
    }

    /// Turn results are one axis of files, and the closing document is
    /// separate from all of them.
    #[test]
    fn each_turn_has_its_own_result_file_and_the_closing_document_its_own() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert_eq!(result_file_for_turn(0), "result.json");
        assert_eq!(result_file_for_turn(1), "result.json");
        assert_eq!(result_file_for_turn(2), "result-2.json");
        assert_eq!(result_file_for_turn(7), "result-7.json");

        let mut first = TeammateResult::new("agent-3", Exit::Ok);
        first.final_message = "first".to_string();
        first.write_turn(directory.path(), 1).expect("write turn 1");
        let mut second = TeammateResult::new("agent-3", Exit::Ok);
        second.final_message = "second".to_string();
        second.write_turn(directory.path(), 2).expect("write turn 2");
        assert_eq!(TeammateResult::read(directory.path()).unwrap().final_message, "first");
        assert_eq!(TeammateResult::read_turn(directory.path(), 1).unwrap().turn, Some(1));
        let read_second = TeammateResult::read_turn(directory.path(), 2).expect("turn 2");
        assert_eq!(read_second.final_message, "second");
        assert_eq!(read_second.turn, Some(2));
        assert!(TeammateResult::read_turn(directory.path(), 3).is_none());
        assert!(TeammateResult::read_final(directory.path()).is_none());

        let closed = TeammateResult::closed("agent-3", CloseReason::ParentLost);
        closed.write_final(directory.path()).expect("write final");
        let read_closed = TeammateResult::read_final(directory.path()).expect("final");
        assert_eq!(read_closed.exit, Exit::Closed);
        assert_eq!(read_closed.reason, Some(CloseReason::ParentLost));
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(directory.path().join(RESULT_FINAL_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!(raw["exit"], "closed");
        assert_eq!(raw["reason"], "parent_lost");
        // A v1 child's result (no `turn`) still reads as turn 1.
        let v1 = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            v1.path().join(RESULT_FILE),
            br#"{"version": 1, "agentId": "agent-old", "exit": "ok", "finalMessage": "done"}"#,
        )
        .unwrap();
        let old = TeammateResult::read(v1.path()).expect("v1 result reads");
        assert_eq!(old.final_message, "done");
        assert!(old.turn.is_none());
    }

    /// A child that left instead of answering is heard as `Closed`, and a
    /// cancelled wait knocks on the door before it kills the pane.
    #[test]
    fn a_wait_for_a_later_turn_hears_a_closing_document_and_closes_politely_first() {
        let directory = tempfile::tempdir().expect("tempdir");
        let tmux = Tmux::at(fake_tmux(directory.path(), "'%9'"));
        let child = directory.path().join("agent-6");
        std::fs::create_dir_all(&child).expect("mkdir");
        TeammateResult::closed("agent-6", CloseReason::IdleBudget)
            .write_final(&child)
            .expect("write final");
        let outcome = wait_for_turn_result(&tmux, &child, "%9", 2, Duration::from_secs(5), &|| false, &|| {});
        match outcome {
            PaneOutcome::Closed(result) => assert_eq!(result.reason, Some(CloseReason::IdleBudget)),
            other => panic!("a closing document read as {other:?}"),
        }

        let knocked = std::cell::Cell::new(false);
        let cancelled = wait_for_turn_result(
            &tmux,
            directory.path(),
            "%9",
            2,
            Duration::from_secs(30),
            &|| true,
            &|| knocked.set(true),
        );
        assert_eq!(cancelled, PaneOutcome::Cancelled);
        assert!(knocked.get(), "the door was not tried before the pane was killed");
        assert!(tmux_log(directory.path()).iter().any(|line| line == "kill-pane -t %9"));
    }

    /// A one-line JSON-RPC server that answers the way a child's channel does.
    ///
    /// `answers` is what it says to `session.steer`; everything else gets an
    /// `-32601`. Frames without `jsonrpc` are streamed first so the client
    /// proves it skips them.
    fn fake_channel(answers: serde_json::Value) -> (ChannelCoordinates, std::thread::JoinHandle<Vec<serde_json::Value>>) {
        use std::io::{BufRead as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let Ok((stream, _)) = listener.accept() else {
                return seen;
            };
            let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let request: serde_json::Value = serde_json::from_str(line.trim()).expect("json");
            seen.push(request.clone());
            let mut writer = stream;
            let _ = writeln!(writer, r#"{{"type":"turn","turn_id":7,"phase":"start"}}"#);
            let id = request["id"].as_u64().unwrap_or_default();
            let response = if request["method"] == channel_method::STEER {
                serde_json::json!({"jsonrpc": "2.0", "id": id, "result": answers})
            } else {
                serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "no such method"}})
            };
            let _ = writeln!(writer, "{response}");
            seen
        });
        (
            ChannelCoordinates {
                addr,
                token: Some("s3cret".to_string()),
                session_id: "child-session".to_string(),
            },
            handle,
        )
    }

    /// The receipt is read off the child's own answer: a turn id means the
    /// words landed in a running turn, none means an idle child opened its
    /// next turn with them, and nobody home is a rejection with the reason.
    #[test]
    fn a_steer_over_the_channel_reads_its_receipt_off_the_answer() {
        let directory = tempfile::tempdir().expect("tempdir");
        let channel_file = directory.path().join(CHANNEL_FILE);

        let (running, seen) = fake_channel(serde_json::json!({"turn_id": 7}));
        running.write(&channel_file).expect("write channel file");
        assert_eq!(ChannelCoordinates::read(&channel_file).expect("read"), running);
        let outcome = steer_over_channel(&channel_file, "look again", Duration::from_secs(5));
        assert_eq!(outcome.receipt, SteerReceipt::Consumed);
        assert_eq!(outcome.turn_id, Some(7));
        assert!(outcome.delivered());
        let requests = seen.join().expect("server");
        assert_eq!(requests[0]["method"], channel_method::STEER);
        assert_eq!(requests[0]["params"]["text"], "look again");
        assert_eq!(requests[0]["params"]["id"], "child-session");
        assert_eq!(requests[0]["token"], "s3cret", "the token rides every call");

        let (idle, seen) = fake_channel(serde_json::json!({"turn_id": null, "receipt": "queued"}));
        idle.write(&channel_file).expect("write channel file");
        let outcome = steer_over_channel(&channel_file, "one more", Duration::from_secs(5));
        assert_eq!(outcome.receipt, SteerReceipt::Queued);
        assert!(outcome.turn_id.is_none());
        let _ = seen.join();

        // Nobody home: the listener is gone.
        let (gone, seen) = fake_channel(serde_json::Value::Null);
        gone.write(&channel_file).expect("write channel file");
        let probe = gone.alive(Duration::from_secs(5));
        assert!(probe, "a live listener answers, even with a refusal");
        let _ = seen.join();
        // The thread accepted once and left; the port is closed now.
        let outcome = steer_over_channel(&channel_file, "anyone?", Duration::from_millis(500));
        assert_eq!(outcome.receipt, SteerReceipt::Rejected);
        assert!(!outcome.delivered());
        assert!(outcome.reason.as_deref().is_some_and(|why| why.contains("unreachable")), "{outcome:?}");
        assert!(!gone.alive(Duration::from_millis(500)));
        assert!(!parent_channel_alive(&channel_file, Duration::from_millis(500)));

        // And a missing file is a rejection that names the file, not a panic.
        std::fs::remove_file(&channel_file).unwrap();
        let outcome = steer_over_channel(&channel_file, "anyone?", Duration::from_millis(500));
        assert_eq!(outcome.receipt, SteerReceipt::Rejected);
        assert!(outcome.reason.as_deref().is_some_and(|why| why.contains("channel file")), "{outcome:?}");
        assert!(!parent_channel_alive(&channel_file, Duration::from_millis(500)));
    }

    /// The receipt vocabulary and its one old boolean.
    #[test]
    fn receipts_spell_themselves_and_only_a_rejection_is_undelivered() {
        assert_eq!(SteerReceipt::Consumed.as_str(), "consumed");
        assert_eq!(SteerReceipt::Queued.as_str(), "queued");
        assert_eq!(SteerReceipt::Rejected.as_str(), "rejected");
        assert!(SteerReceipt::Consumed.delivered());
        assert!(SteerReceipt::Queued.delivered());
        assert!(!SteerReceipt::Rejected.delivered());
        let json = serde_json::to_value(SteerOutcome::rejected("gone")).unwrap();
        assert_eq!(json, serde_json::json!({"receipt": "rejected", "reason": "gone"}));
        let json = serde_json::to_value(SteerOutcome::consumed(Some(3))).unwrap();
        assert_eq!(json, serde_json::json!({"receipt": "consumed", "turnId": 3}));
        for reason in [
            CloseReason::ParentLost,
            CloseReason::IdleBudget,
            CloseReason::ClosedByParent,
            CloseReason::UserExit,
            CloseReason::LaneDone,
        ] {
            assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
        }
    }

    /// The parent's own channel is a process fact, declared once.
    #[test]
    fn the_parent_channel_is_declared_and_read_back() {
        let _guard = crate::test_env_lock();
        assert!(parent_channel().is_none() || parent_channel().is_some());
        declare_parent_channel(Some(PathBuf::from("/tmp/zo-events-42.addr")));
        assert_eq!(parent_channel(), Some(PathBuf::from("/tmp/zo-events-42.addr")));
        declare_parent_channel(None);
        assert!(parent_channel().is_none());
    }

    /// A `tmux` that records what it was asked and answers like the window's.
    ///
    /// A shell script rather than a Rust double, because the thing under test
    /// is a PROCESS BOUNDARY: the argv this module hands `Command`, and the
    /// stdout it reads back. A trait object would test neither.
    fn fake_tmux(directory: &Path, panes: &str) -> PathBuf {
        let log = directory.join("tmux.log");
        let script = directory.join("tmux");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$*\" >> {log}\n\
                 case \"$1\" in\n\
                 split-window) echo '%9' ;;\n\
                 list-panes) printf '%s\\n' {panes} ;;\n\
                 kill-pane) : ;;\n\
                 esac\n",
                log = log.display(),
                panes = panes,
            ),
        )
        .expect("write fake tmux");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        script
    }

    fn tmux_log(directory: &Path) -> Vec<String> {
        std::fs::read_to_string(directory.join("tmux.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn the_first_child_of_a_wave_cuts_sideways_and_the_rest_stack_under_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        let child = directory.path().join("agent-3");
        let spec = SplitSpec {
            directory: &child,
            agent_id: "agent-3",
            program: "/opt/zo",
            model: Some("claude-fable-5-1"),
            effort: None,
            wave_index: 0,
            resume_transcript: None,
        };
        let first = split_argv(&spec);
        assert_eq!(
            first,
            [
                "split-window",
                "-h",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-e",
                "ZO_AGENT_ID=agent-3",
                "--",
                "/opt/zo",
                "--teammate",
                &child.to_string_lossy(),
                "--model",
                "claude-fable-5-1",
            ]
        );
        let stacked = split_argv(&SplitSpec {
            wave_index: 1,
            model: None,
            effort: Some("high"),
            ..spec
        });
        assert_eq!(stacked[1], "-v", "the second child halved the leader again");
        assert!(stacked.ends_with(&["--effort".to_string(), "high".to_string()]));
        // `--` is what keeps zo's own flags out of tmux's parse.
        let separator = stacked
            .iter()
            .position(|word| word == "--")
            .expect("no separator");
        assert!(stacked[separator + 1].ends_with("zo"));
    }

    /// One identity, one row (t-3024).
    ///
    /// The `SubagentStart` hook names the helper by its `agent_id`, and the
    /// window stands a row for it; the split stands a PANE row for the same
    /// helper. Unless the split carries that same id, nothing links the two
    /// and the person sees one helper drawn twice. The pair rides as tmux's
    /// own `-e KEY=VALUE`, BEFORE `--`, because after it the words are zo's.
    #[test]
    fn the_split_names_the_child_by_the_id_the_hook_already_said() {
        let directory = tempfile::tempdir().expect("tempdir");
        let child = directory.path().join("agent-1757000000000");
        let argv = split_argv(&SplitSpec {
            directory: &child,
            agent_id: "agent-1757000000000",
            program: "zo",
            model: None,
            effort: None,
            wave_index: 0,
            resume_transcript: None,
        });
        let flag = argv
            .iter()
            .position(|word| word == "-e")
            .expect("the split carries no environment pair");
        assert_eq!(argv[flag + 1], "ZO_AGENT_ID=agent-1757000000000");
        let separator = argv
            .iter()
            .position(|word| word == "--")
            .expect("no separator");
        assert!(flag < separator, "the pair is tmux's, not zo's: {argv:?}");
        // Exactly one pair: a second `-e` would be a second identity.
        assert_eq!(argv.iter().filter(|word| *word == "-e").count(), 1);
    }

    #[test]
    fn a_split_asks_the_multiplexer_and_reads_the_pane_id_back() {
        let directory = tempfile::tempdir().expect("tempdir");
        let tmux = Tmux::at(fake_tmux(directory.path(), "'%1'"));
        let child = directory.path().join("agent-4");
        let pane = tmux
            .split(&SplitSpec {
                directory: &child,
                agent_id: "agent-4",
                program: "zo",
                model: None,
                effort: None,
                wave_index: 0,
                resume_transcript: None,
            })
            .expect("split");
        assert_eq!(pane, "%9", "the pane id came from tmux's own answer");
        let asked = tmux_log(directory.path());
        assert_eq!(asked.len(), 1);
        assert!(asked[0].starts_with(
            "split-window -h -d -P -F #{pane_id} -e ZO_AGENT_ID=agent-4 -- zo --teammate"
        ));
        assert!(asked[0].contains("agent-4"));
    }

    #[test]
    fn a_result_that_appears_is_the_answer_and_a_pane_that_vanishes_is_not() {
        let directory = tempfile::tempdir().expect("tempdir");
        // `%9` is listed, so the pane is alive; the result is already there.
        let tmux = Tmux::at(fake_tmux(directory.path(), "'%9'"));
        let child = directory.path().join("agent-5");
        std::fs::create_dir_all(&child).expect("mkdir");
        let mut wrote = TeammateResult::new("agent-5", Exit::Ok);
        wrote.final_message = "three edges".to_string();
        wrote.write(&child).expect("write result");
        let outcome = wait_for_result(
            &tmux,
            &child,
            "%9",
            std::time::Duration::from_secs(5),
            &|| false,
        );
        assert_eq!(outcome, PaneOutcome::Finished(Box::new(wrote)));

        // The same wait, with the pane gone and nothing written: a child that
        // died is said so rather than waited out.
        let empty = tempfile::tempdir().expect("tempdir");
        let gone = Tmux::at(fake_tmux(empty.path(), "'%1'"));
        assert_eq!(
            wait_for_result(
                &gone,
                empty.path(),
                "%9",
                std::time::Duration::from_secs(5),
                &|| false
            ),
            PaneOutcome::Vanished
        );
    }

    #[test]
    fn a_cancelled_turn_ends_the_pane_it_opened() {
        let directory = tempfile::tempdir().expect("tempdir");
        let tmux = Tmux::at(fake_tmux(directory.path(), "'%9'"));
        let outcome = wait_for_result(
            &tmux,
            directory.path(),
            "%9",
            std::time::Duration::from_secs(30),
            &|| true,
        );
        assert_eq!(outcome, PaneOutcome::Cancelled);
        let asked = tmux_log(directory.path());
        assert!(
            asked.iter().any(|line| line == "kill-pane -t %9"),
            "the cancelled child's pane was left running: {asked:?}"
        );
    }

    #[test]
    fn a_budget_that_runs_out_ends_the_pane_and_says_so() {
        let directory = tempfile::tempdir().expect("tempdir");
        let tmux = Tmux::at(fake_tmux(directory.path(), "'%9'"));
        let outcome = wait_for_result(
            &tmux,
            directory.path(),
            "%9",
            std::time::Duration::ZERO,
            &|| false,
        );
        assert_eq!(outcome, PaneOutcome::TimedOut);
        assert!(tmux_log(directory.path())
            .iter()
            .any(|line| line == "kill-pane -t %9"));
    }

    /// A tmux that cannot be asked is not evidence a child died.
    #[test]
    fn an_unaskable_multiplexer_does_not_bury_a_working_child() {
        let tmux = Tmux::at("/nonexistent/tmux-that-is-not-there");
        assert!(tmux.pane_exists("%9"));
        assert!(!tmux.kill_pane("%9"));
    }

    /// Production reaches tmux through `PATH`, which is where the window's
    /// shim is put — so the bare name has to be what gets run.
    ///
    /// The name, not a resolved path: `Command` resolves it at spawn time,
    /// and resolving it here would pin whatever `PATH` said when this process
    /// started rather than what the window put in front of it.
    #[test]
    fn the_multiplexer_production_uses_is_the_one_on_the_path() {
        assert_eq!(Tmux::on_path().program(), std::ffi::OsStr::new("tmux"));
        assert_eq!(Tmux::default().program(), Tmux::on_path().program());
    }

    /// The child is THIS zo, not whatever `PATH` answers today.
    #[test]
    fn a_teammate_runs_the_binary_that_spawned_it_unless_its_path_needs_quoting() {
        let program = teammate_program();
        assert!(!program.is_empty());
        assert!(
            !program.chars().any(char::is_whitespace),
            "a program with a space arrives at the window as two words: {program}"
        );
    }

    #[test]
    fn a_half_written_document_is_never_the_one_a_reader_finds() {
        // The temporary is a sibling — a rename across a device boundary
        // degrades to a copy, which is the torn read this avoids — and it is
        // gone once the document is in place.
        let directory = tempfile::tempdir().expect("tempdir");
        let result = TeammateResult::new("agent-2", Exit::Cancelled);
        result.write(directory.path()).expect("write");
        let left: Vec<String> = std::fs::read_dir(directory.path())
            .expect("read dir")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec![RESULT_FILE.to_string()], "a temporary was left behind");
        // And a partial file under the real name reads as nothing rather than
        // as a finished run with an empty answer.
        std::fs::write(directory.path().join(RESULT_FILE), br#"{"version": 1, "agen"#)
            .expect("write");
        assert!(TeammateResult::read(directory.path()).is_none());
    }
}
