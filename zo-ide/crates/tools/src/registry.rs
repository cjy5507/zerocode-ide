//! Tool registry surfaces — `ToolSpec`, `ToolRegistry`, `GlobalToolRegistry`.
//!
//! Three layers stack here:
//!
//! 1. [`ToolSpec`] — a single builtin tool's name + JSON schema + permission.
//!    [`mvp_tool_specs`] returns the cached list assembled from each
//!    `*_tools.rs` submodule.
//! 2. [`ToolRegistry`] — lightweight manifest (name + base/conditional source)
//!    used by the runtime when deciding which tools to expose.
//! 3. [`GlobalToolRegistry`] — the live registry the dispatcher reads at
//!    request time. Holds builtin specs, plugin tools, runtime-injected
//!    tools, and the permission enforcer.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use plugins::PluginTool;
use runtime::{permission_enforcer::PermissionEnforcer, McpDegradedReport, PermissionMode};
use serde_json::Value;

use crate::aliases::{normalize_tool_name, permission_mode_from_plugin, TOOL_NAME_ALIASES};
use crate::context::{disabled_tool_error, ToolContext, TOOL_TOGGLE_DENIAL_REASON};
use crate::dispatch::{
    enforce_permission_check, execute_tool_with_context, finish_successful_tool_output, from_value,
    to_pretty_json,
};
use crate::error::ToolError;
use crate::gateway::{
    self, failed_result, successful_result, toggle_denied_decision, ToolFamily, ToolResultMetadata,
};
use crate::{
    bash_tools, codegraph_tools, file_tools, mcp_tools, misc_tools, plan_mode_v2, task_tools,
    team_tools, web_tools, worker_tools, workflow_tools, worktree_tools,
};

pub use misc_tools::ToolSearchOutput;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolManifestEntry {
    pub name: String,
    pub source: ToolSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    Base,
    Conditional,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolRegistry {
    entries: Vec<ToolManifestEntry>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new(entries: Vec<ToolManifestEntry>) -> Self {
        Self { entries }
    }

    #[must_use]
    pub fn entries(&self) -> &[ToolManifestEntry] {
        &self.entries
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub required_permission: PermissionMode,
}

#[derive(Debug, Clone)]
pub struct GlobalToolRegistry {
    plugin_tools: Vec<PluginTool>,
    /// MCP / dynamically-discovered tools. Wrapped in `Arc<Mutex<>>` so a
    /// mid-session refresh ([`Self::set_runtime_tools`]) propagates to **every**
    /// clone of the registry — the registry is cloned into the API client, the
    /// tool executor, the dispatch closure, and the request builder, and a plain
    /// `Vec` clone would leave those copies stale (the G20 inbound-MCP gap).
    /// `Clone` stays derived: cloning an `Arc` shares the same `Mutex`.
    runtime_tools: Arc<Mutex<Vec<RuntimeToolDefinition>>>,
    enforcer: Option<PermissionEnforcer>,
    context: ToolContext,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub required_permission: PermissionMode,
}

/// Dispatch closure routing one named MCP tool call through the parent
/// session's MCP runtime (error rendered as a display string).
pub type McpPassthroughDispatchFn =
    Arc<dyn Fn(&str, &Value) -> Result<String, String> + Send + Sync>;

/// Parent-session MCP access shared with spawned sub-agents.
///
/// Installed once by the session host via
/// [`GlobalToolRegistry::install_subagent_mcp_passthrough`], carried on
/// [`ToolContext`], and copied onto each spawn's `AgentJob`: the definitions
/// share the registry's live `runtime_tools` Arc — a mid-session
/// `tools/list_changed` refresh reaches later spawns automatically — and
/// `dispatch` routes one named MCP tool call through the parent session's MCP
/// runtime. Without this seam sub-agents had no MCP route at all (the
/// tools-crate dispatcher only knows builtin families).
#[derive(Clone)]
pub struct McpPassthrough {
    definitions: Arc<Mutex<Vec<RuntimeToolDefinition>>>,
    dispatch: McpPassthroughDispatchFn,
    allowed_tools: Option<Arc<BTreeSet<String>>>,
}

impl std::fmt::Debug for McpPassthrough {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpPassthrough").finish_non_exhaustive()
    }
}

impl McpPassthrough {
    /// Snapshot of the currently advertised MCP tool definitions.
    #[must_use]
    pub(crate) fn definitions_snapshot(&self) -> Vec<RuntimeToolDefinition> {
        let definitions = self
            .definitions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match self.allowed_tools.as_ref() {
            Some(allowed_tools) => definitions
                .into_iter()
                .filter(|definition| allowed_tools.contains(&definition.name))
                .collect(),
            None => definitions,
        }
    }

    /// Return a view that exposes only MCP tools allowed for one sub-agent while
    /// still sharing the parent session's live definition list and dispatch fn.
    #[must_use]
    pub(crate) fn filtered_to_allowed(&self, allowed_tools: &BTreeSet<String>) -> Self {
        Self {
            definitions: Arc::clone(&self.definitions),
            dispatch: Arc::clone(&self.dispatch),
            allowed_tools: Some(Arc::new(allowed_tools.clone())),
        }
    }

    /// Whether `name` is a currently advertised MCP tool.
    #[must_use]
    pub(crate) fn covers(&self, name: &str) -> bool {
        self.definitions_snapshot()
            .iter()
            .any(|definition| definition.name == name)
    }

    /// Dispatch one MCP tool call through the parent session's runtime.
    pub(crate) fn dispatch(&self, tool_name: &str, input: &Value) -> Result<String, String> {
        (self.dispatch)(tool_name, input)
    }

    /// Test-only constructor with an in-memory dispatch fn.
    #[cfg(test)]
    pub(crate) fn for_tests(
        definitions: Vec<RuntimeToolDefinition>,
        dispatch: McpPassthroughDispatchFn,
    ) -> Self {
        Self {
            definitions: Arc::new(Mutex::new(definitions)),
            dispatch,
            allowed_tools: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SearchableToolSpec {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleableToolSource {
    Builtin,
    Runtime,
    Plugin,
}

impl ToggleableToolSource {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Runtime => "mcp",
            Self::Plugin => "plugin",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToggleableTool {
    pub name: String,
    pub description: Option<String>,
    pub source: ToggleableToolSource,
    pub enabled: bool,
}

type ToolSpecFactory = fn() -> Vec<ToolSpec>;

/// Builtin tool families advertised on the wire for every request — the core
/// set that covers ordinary coding work (read/edit/run/search, delegation,
/// planning, task tracking, web, MCP).
const CORE_TOOL_SPEC_FACTORIES: &[ToolSpecFactory] = &[
    bash_tools::tool_specs,
    file_tools::tool_specs,
    task_tools::tool_specs,
    mcp_tools::tool_specs,
    misc_tools::tool_specs,
    plan_mode_v2::tool_specs,
];

/// Builtin tool families kept out of the per-request wire advertisement and
/// surfaced on demand through `ToolSearch` instead — capabilities a typical
/// turn does not need (codegraph, git worktrees, workflow runs, team ledger,
/// cron, background workers, web, image generation). They stay registered and
/// fully executable (execution resolves by name, not by advertisement);
/// deferral only drops their schemas from the cached prefix so a simple task
/// does not pay for tools it will not use.
const DEFERRED_TOOL_SPEC_FACTORIES: &[ToolSpecFactory] = &[
    crate::image_tools::tool_specs,
    crate::computer_tools::tool_specs,
    crate::artifact_tools::tool_specs,
    codegraph_tools::tool_specs,
    worktree_tools::tool_specs,
    workflow_tools::tool_specs,
    team_tools::tool_specs,
    worker_tools::tool_specs,
    web_tools::tool_specs,
];

/// Individually deferred builtins that live in families whose other members
/// stay on the wire (`TodoWrite` keeps `task_tools` core, `bash` keeps
/// `bash_tools` core, and `misc_tools` mixes both kinds). What is left
/// advertised is the twelve-schema coding surface: `bash`, the five file tools
/// (`read_file`, `edit_file`, `write_file`, `MultiEdit`, `grep_search`,
/// `glob_search`), `TodoWrite`, `Skill`, `Agent` (back since t-2903), and the
/// two doors deferral itself depends on (`ToolSearch`, `CapabilityInvoke`). A test
/// asserts every name here resolves to a registered spec, so a rename cannot
/// silently re-advertise a tool.
const DEFERRED_TOOL_NAMES_EXTRA: &[&str] = &[
    // bash_tools: niche shells — bash stays.
    "PowerShell",
    "REPL",
    // One delegation door on the wire, not two. `Agent` covers the common
    // case and its own description points here for real fan-out; the
    // delegation prompt section names this tool too. Measured at 723 tokens
    // on every request against an event the prompt itself calls rare
    // ("usually just one [agent]") — one lookup before a fan-out is the
    // cheaper trade, and §2.2's target surface is a single `delegate`.
    "SpawnMultiAgent",
    // `AskUserQuestion` — 597 tokens on every request against 210 calls in 31%
    // of sessions (279 local sessions / 88,676 tool calls when r49 measured
    // it) — is deferred on the same trade as `SpawnMultiAgent` above: the
    // prompt already teaches when to reach for it (`# Clarifying questions`),
    // the manifest below names it, and the lookup it costs runs 729 times
    // across 57% of sessions anyway. Deferral is this harness's ordinary way
    // of holding a schema. `Agent` sat here beside it from r49 until t-2903;
    // the arithmetic that put it back is at `ExitPlanModeV2` below.
    //
    // Reversible in one word: `ZO_WIRE_TOOLS=AskUserQuestion` puts it back,
    // for a session that would rather pay the tokens than the lookup.
    "AskUserQuestion",
    // A worktree-local SETTINGS pair, not the in-turn plan flow: the user
    // enters plan mode with Shift+Tab or `/plan`, and the model submits a plan
    // with `ExitPlanModeV2` (deferred below since t-2903, still reachable by
    // name and through `CapabilityInvoke`). Deferring them also removed the
    // last thing that moved the wire mid-session — both were dropped from the
    // advertisement whenever plan mode was selected.
    "EnterPlanMode",
    "ExitPlanMode",
    // ── t-2903: `Agent` back on the wire, `ExitPlanModeV2` off it ──────────
    //
    // Axis K of `docs/analysis/zo-agent-parity-20260906.md`: with every
    // delegation door deferred, a session's first `Agent` cost TWO parent
    // turns (ToolSearch → CapabilityInvoke) where Claude Code spends one, and
    // the runtime was never the reason — `CapabilityInvoke(Agent)` with no
    // ToolSearch first spawns in one turn (harness probe, 63.5 ms). The
    // second turn is the model not knowing a schema it was never shown, so
    // the fix is the advertisement. The wire was full (twelve schemas, 1,808
    // of the 2,000-token bucket) and the r49 `Agent` text measured 1,200
    // tokens (`ZO_WIRE_TOOLS=Agent`: 3,008 / 2,000 and 13 / 12), so two
    // things had to be true, and both were measured on this machine's
    // corpus (917 session files · 347 sessions with a turn · 26,729
    // assistant messages, 2026-09-06):
    //
    //   * one wire schema earns less per session than `Agent`:
    //       Agent            47 sessions · 13.5% · 225 calls, plus 66 through
    //                        CapabilityInvoke and 18 `select:Agent` lookups
    //                        in the one day since r49 deferred it
    //       ExitPlanModeV2    7 sessions ·  2.0% ·   7 calls
    //     every other wire schema is used in 16–71% of sessions;
    //   * `Agent`'s schema fits the seat: the r49 description was routing
    //     policy that `# Delegation and workflow routing` already teaches and
    //     delivery rules the detached receipt (`BACKGROUND_AGENT_NOTE`) already
    //     states per call, so the schema now carries only what a CALL needs
    //     (see `specs.rs`): 1,200 → 248 tokens. Bucket after: 1,992 / 2,000
    //     (Agent 248 + manifest 441, which grew 7 for this entry's hook, − the
    //     71 `ExitPlanModeV2` gave back); fixed harness 6,420 → 6,604.
    //
    // Axis K after: parent turns to the child's first request 2 → 1 with
    // twelve schemas on the wire (harness runs K-zo-before-t2903 /
    // K-zo-after-t2903 under /tmp/zo-agent-parity-20260906/runs/).
    //
    // Plan submission loses nothing: `execute` resolves the name whether or
    // not it is advertised, `CapabilityInvoke` carries it for strict
    // providers, the manifest hook names it, the `ExitPlanMode` denial points
    // at it, and the TUI's plan screen reads the effective name. Reversible in
    // one word: `ZO_WIRE_TOOLS=ExitPlanModeV2`.
    "ExitPlanModeV2",
    // task_tools: cross-session task ledger — TodoWrite stays. (TaskCreate /
    // RunTaskPacket are not in the registered specs at all; see task_tools.)
    "TaskGet",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "TaskUpdate",
    // misc_tools: orchestration / scheduling / diagnostics a plain coding
    // turn does not need.
    "Audit",
    "Brief",
    "Config",
    "Council",
    // The roster answer, deferred on the same trade as the two tools it sits
    // beside: it is only reached once delegation is already in play, and the
    // wire is FULL — twelve schemas is the r49 ceiling and all twelve are
    // pinned coding affordances. Its name says what it does, so the manifest
    // line below is the whole promise it needs; `ZO_WIRE_TOOLS=ListAgents`
    // advertises it for a session that would rather pay the tokens.
    "ListAgents",
    "Monitor",
    "NotebookEdit",
    // Claude Code carries this one on its wire; here it sits behind the door
    // on the same trade as `ListAgents`: the twelve seats are full (Tools
    // 1,992/2,000 after t-2903), the honest schema measures far more than a
    // 60-token seat (`push_notification_schema_is_measured_and_deferred`
    // pins the number), and the name alone says what it does — so the
    // manifest line (18 chars) is the whole promise it needs. Reversible in
    // one word: `ZO_WIRE_TOOLS=PushNotification`.
    "PushNotification",
    // The agent tool seat (t-6040), deferred on the same trade as
    // `PushNotification`: the wire is full, a typed judge is reached for on
    // a minority of turns, and the hook below is the whole promise it needs.
    // Reversible in one word: `ZO_WIRE_TOOLS=Jev`.
    "Jev",
    "RemoteTrigger",
    "ScheduleWakeup",
    "SendMessage",
    "SendUserMessage",
    "SkillDistill",
    "SkillReview",
    // The skill index's replacement (t-5629), deferred for the reason every
    // ranked list is: a turn that needs no skill never looks, and a turn that
    // does pays one lookup against the 900 tokens the index spent on every
    // request.
    //
    // They carry no `DEFERRED_TOOL_HOOKS` line — the prompt section that
    // replaces the index already says when to reach for them, on the one road
    // where they matter — but they ARE named in the manifest, because a
    // deferred tool the fixed harness never promises is one the model has no
    // reason to open on the other road (`harness_budget`'s
    // `every_fixed_harness_tool_name_is_registered_or_deferred_reachable`).
    "skill_search",
    "skill_load",
    "Sleep",
    // Only reachable once you hold an agent id, and the result that hands you
    // one names this tool — so the name in the deferred list is enough.
    "StopAgent",
    "StructuredOutput",
    "SyntheticOutput",
    // ── r49: the last five, to bring the wire to twelve ──────────────────
    //
    // The tool bucket's budget is 2,000 tokens and it stood at 3,935. There
    // was no head left to aim at (`the_wire_budget_has_no_head_left_to_aim_at`
    // measured that after `Agent` went), so the remaining saving had to come
    // from several schemas at once. These five are what a plain coding turn
    // does not touch — 1,227 tokens of schema on every request:
    //
    //   session_recall        623   reads a PAST conversation
    //   MemoryWrite           192   writes a memory file
    //   retrieve_tool_output  177   recovers a truncated output by id
    //   send_to_user          165   pushes a file/message to the person
    //   read_image             70   reads an image file
    //
    // None of them is pinned to the wire by a contract (checked against
    // `selecting_plan_does_not_move_the_wire`, which is why `ExitPlanModeV2`
    // is NOT here, and against the wire assertions in this file). Every place
    // that names them names them in TEXT the model is already reading — a
    // truncation notice, a compaction continuation, a prompt section — and
    // `execute` resolves any registered spec by name whether or not it was
    // advertised, so a blind call still runs; a name the model cannot place
    // gets `unknown_tool_error`'s "load it with ToolSearch first".
    //
    // `retrieve_tool_output` and `session_recall` carry manifest hooks below,
    // because they are the two whose name alone does not say what they undo.
    "session_recall",
    "MemoryWrite",
    "retrieve_tool_output",
    "send_to_user",
    // file_tools: the text six stay; images are not a coding turn.
    "read_image",
];

/// Cached builtin tool specifications. Built once on first access.
static MVP_TOOL_SPECS: OnceLock<Vec<ToolSpec>> = OnceLock::new();

#[must_use]
pub fn mvp_tool_specs() -> &'static [ToolSpec] {
    MVP_TOOL_SPECS.get_or_init(|| {
        let mut specs = Vec::new();
        for factory in CORE_TOOL_SPEC_FACTORIES
            .iter()
            .chain(DEFERRED_TOOL_SPEC_FACTORIES)
        {
            specs.extend(factory());
        }
        specs
    })
}

/// Names of builtin tools that are registered and searchable but not advertised
/// on the wire (see [`DEFERRED_TOOL_SPEC_FACTORIES`]). Derived from the deferred
/// factories themselves so the set can never drift from what they produce.
static DEFERRED_TOOL_NAMES: OnceLock<BTreeSet<&'static str>> = OnceLock::new();

fn deferred_tool_names() -> &'static BTreeSet<&'static str> {
    DEFERRED_TOOL_NAMES.get_or_init(|| {
        DEFERRED_TOOL_SPEC_FACTORIES
            .iter()
            .flat_map(|factory| factory())
            .map(|spec| spec.name)
            .chain(DEFERRED_TOOL_NAMES_EXTRA.iter().copied())
            .collect()
    })
}

/// Whether a builtin tool is deferred from the wire advertisement (surfaced via
/// `ToolSearch` on demand). Deferral hides the schema from the model; it never
/// blocks execution, which resolves by name.
fn is_deferred_tool(name: &str) -> bool {
    deferred_tool_names().contains(name) && !forced_onto_wire(name)
}

/// Names an operator wants advertised despite being deferred
/// (`ZO_WIRE_TOOLS=Agent,AskUserQuestion`).
///
/// The escape hatch for the trade the deferred list makes: a session that would
/// rather pay a schema on every request than a `ToolSearch` before first use can
/// say so. Read once — the advertised set must not change between two requests
/// of one conversation, or the cached prefix behind it is re-billed.
pub const WIRE_TOOLS_ENV: &str = "ZO_WIRE_TOOLS";

fn forced_onto_wire(name: &str) -> bool {
    static FORCED: OnceLock<BTreeSet<String>> = OnceLock::new();
    FORCED
        .get_or_init(|| {
            std::env::var(WIRE_TOOLS_ENV)
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(str::to_string)
                .collect()
        })
        .contains(name)
}

/// System-prompt section naming every deferred builtin tool. Deferral only
/// works if the model knows the hidden tools exist — it cannot `ToolSearch`
/// for a name it has never seen. Derived from the deferred factories (same
/// source as [`is_deferred_tool`]) so the manifest can never drift from what
/// is actually off the wire. ~1 line per family; the whole point of deferral
/// is that this costs a fraction of the schemas it replaces.
/// One-line capability hooks for deferred tools whose NAME alone under-sells
/// a distinctive capability — without the hook the model has no reason to
/// ever load them, and the feature ships as a gate with no door. Kept tiny
/// deliberately: every entry costs prompt tokens on every turn.
const DEFERRED_TOOL_HOOKS: &[(&str, &str)] = &[
    ("Artifact", "Publish/export HTML."),
    (
        "Computer",
        "Desktop UI, apps, OCR.",
    ),
    (
        "REPL",
        "Persist Python across calls/compaction; zo.spawn/result handles agents, zo.skill assets, \
         and zo.history transcript search.",
    ),
    (
        "Council",
        "Compare anonymized answers for agreement or honest ties.",
    ),
    // Twelve characters, because the tool bucket had five tokens of head
    // left under its r49 line when this seat landed (2,005 of 2,010) and the
    // manifest is billed there; the schema behind the door says the rest.
    ("Jev", "typed judge"),
    (
        "find_symbol",
        "find_symbol/find_references/file_outline: structure grep lacks.",
    ),
    (
        "WebSearch",
        "WebSearch/WebFetch: stale or external facts.",
    ),
    (
        "WorkerCreate",
        "Create/observe/prompt/restart/terminate guarded workers.",
    ),
    (
        "TeamCreate",
        "Track team tasks/inbox without launching agents.",
    ),
    (
        "SkillDistill",
        "Save repeatable procedures as a proposed skill.",
    ),
    (
        "SkillReview",
        "Approve/discard proposed skills before active use.",
    ),
    (
        "retrieve_tool_output",
        "Recover truncated output by the notice's sha256 id.",
    ),
    (
        "ExitPlanModeV2",
        "Plan submission.",
    ),
    (
        "session_recall",
        "Read/search past sessions and pre-compaction originals.",
    ),
];

#[must_use]
pub fn deferred_tool_manifest_section() -> String {
    let names = deferred_tool_names()
        .iter()
        .map(|name| {
            DEFERRED_TOOL_HOOKS
                .iter()
                .find(|(hooked, _)| hooked == name)
                .map_or_else(
                    || (*name).to_string(),
                    |(_, hook)| format!("{name} ({hook})"),
                )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "# Deferred tools\nHidden but callable: {names}. Get builtin/MCP/plugin schemas \
         in one ToolSearch (`select:Name,...` or keywords); use \
         CapabilityInvoke({{name,input}})."
    )
}

// ── Why nothing preloads any more ────────────────────────────────────────────
//
// A usage-profile hybrid used to sit here: record every mid-session
// `ToolSearch` load per project and preload the tools that project keeps
// reaching for, because a load ADDED a schema to the tool block at the top of
// the prompt prefix and re-billed the whole conversation behind it (live
// ledger: 191 breaks, 67.5M tokens, 18.6% of every lost cache read).
//
// It was a mitigation, and `CapabilityInvoke` removes what it was mitigating.
// A found tool now runs through a schema that is always on the wire, so a
// search never changes the advertisement and there is no break to pay a
// preload against — only the preloaded schemas' own tokens, on every request,
// forever. So the profile, its ledger and its slot budget are gone rather than
// left running with nothing to buy.

impl GlobalToolRegistry {
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            plugin_tools: Vec::new(),
            runtime_tools: Arc::new(Mutex::new(Vec::new())),
            enforcer: None,
            context: ToolContext::new(),
        }
    }

    pub fn with_plugin_tools(plugin_tools: Vec<PluginTool>) -> Result<Self, ToolError> {
        let builtin_names = mvp_tool_specs()
            .iter()
            .map(|spec| spec.name.to_string())
            .collect::<BTreeSet<_>>();
        let mut seen_plugin_names = BTreeSet::new();

        for tool in &plugin_tools {
            let name = tool.definition().name.clone();
            if builtin_names.contains(&name) {
                return Err(ToolError::PluginConflict(name));
            }
            if !seen_plugin_names.insert(name.clone()) {
                return Err(ToolError::DuplicateName(name));
            }
        }

        Ok(Self {
            plugin_tools,
            runtime_tools: Arc::new(Mutex::new(Vec::new())),
            enforcer: None,
            context: ToolContext::new(),
        })
    }

    pub fn with_runtime_tools(
        self,
        runtime_tools: Vec<RuntimeToolDefinition>,
    ) -> Result<Self, ToolError> {
        self.set_runtime_tools(runtime_tools)?;
        Ok(self)
    }

    /// Install the sub-agent MCP passthrough: spawned agents advertise and
    /// dispatch the parent session's MCP tools through this seam (see
    /// `McpPassthrough`). The definitions share this registry's live
    /// `runtime_tools` Arc, so a later `tools/list_changed` refresh reaches
    /// new spawns without re-installation; the shared context cell makes the
    /// install visible to every registry/context clone.
    pub fn install_subagent_mcp_passthrough(&self, dispatch: McpPassthroughDispatchFn) {
        self.context.install_mcp_passthrough(McpPassthrough {
            definitions: Arc::clone(&self.runtime_tools),
            dispatch,
            allowed_tools: None,
        });
    }

    /// The dispatch the sub-agent passthrough routes through, for a host that
    /// answers a PANE child's `mcp.call` on its events channel (t-2513 §2.1):
    /// the same runtime an inline child reaches in-process. `None` when no
    /// passthrough was installed — a session without MCP servers.
    #[must_use]
    pub fn subagent_mcp_dispatch(&self) -> Option<McpPassthroughDispatchFn> {
        self.context
            .mcp_passthrough()
            .map(|passthrough| Arc::clone(&passthrough.dispatch))
    }

    /// Replace the runtime (MCP) tool set in place. Takes `&self` (not `&mut`)
    /// and mutates the shared `Arc<Mutex<>>`, so the new set is visible to every
    /// clone of this registry — this is what lets a mid-session
    /// `tools/list_changed` refresh reach the request builder (G20). Names are
    /// validated against builtin + plugin tools just like [`Self::with_runtime_tools`].
    pub fn set_runtime_tools(
        &self,
        runtime_tools: Vec<RuntimeToolDefinition>,
    ) -> Result<(), ToolError> {
        let mut seen_names = mvp_tool_specs()
            .iter()
            .map(|spec| spec.name.to_string())
            .chain(
                self.plugin_tools
                    .iter()
                    .map(|tool| tool.definition().name.clone()),
            )
            .collect::<BTreeSet<_>>();

        for tool in &runtime_tools {
            if !seen_names.insert(tool.name.clone()) {
                return Err(ToolError::DuplicateName(tool.name.clone()));
            }
        }

        *self
            .runtime_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = runtime_tools;
        Ok(())
    }

    #[must_use]
    pub fn with_enforcer(mut self, enforcer: PermissionEnforcer) -> Self {
        self.set_enforcer(enforcer);
        self
    }

    #[must_use]
    pub fn with_disabled_tools(self, disabled_tools: BTreeSet<String>) -> Self {
        self.context.set_disabled_tools(disabled_tools);
        self
    }

    pub fn set_disabled_tools(&self, disabled_tools: BTreeSet<String>) {
        self.context.set_disabled_tools(disabled_tools);
    }

    #[must_use]
    pub fn disabled_tool_names(&self) -> BTreeSet<String> {
        self.context.disabled_tools()
    }

    #[must_use]
    pub fn is_tool_disabled(&self, name: &str) -> bool {
        self.context.is_tool_disabled(name)
    }

    /// Provide a pre-built context (for testing or custom setups).
    #[must_use]
    pub fn with_context(mut self, context: ToolContext) -> Self {
        self.context = context;
        self
    }

    /// Access the underlying tool context.
    #[must_use]
    pub fn context(&self) -> &ToolContext {
        &self.context
    }

    /// Mutable access to the underlying tool context for in-process registry operations.
    #[must_use]
    pub fn context_mut(&mut self) -> &mut ToolContext {
        &mut self.context
    }

    pub fn normalize_allowed_tools(
        &self,
        values: &[String],
    ) -> Result<Option<BTreeSet<String>>, ToolError> {
        if values.is_empty() {
            return Ok(None);
        }

        let builtin_specs = mvp_tool_specs();
        let mut name_map = BTreeMap::new();
        let mut expected_names = String::new();
        let mut register_name = |name: &str| {
            if self.is_tool_disabled(name) {
                return;
            }
            if !expected_names.is_empty() {
                expected_names.push_str(", ");
            }
            expected_names.push_str(name);
            name_map.insert(normalize_tool_name(name), name.to_owned());
        };

        for spec in builtin_specs {
            register_name(spec.name);
        }
        for tool in &self.plugin_tools {
            register_name(&tool.definition().name);
        }
        for tool in self
            .runtime_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            register_name(&tool.name);
        }

        // Canonical alias table shared with runtime dispatch; see
        // [`TOOL_NAME_ALIASES`] for the full list and rationale.
        for (alias, canonical) in TOOL_NAME_ALIASES {
            if !self.is_tool_disabled(canonical) {
                name_map.insert((*alias).to_string(), (*canonical).to_string());
            }
        }

        let mut allowed = BTreeSet::new();
        for value in values {
            // A Claude-Code permission spec is `Tool(scope…)` (e.g.
            // `Bash(git status:*)`). The parenthesized scope is a
            // permission-policy concern, not a tool-offer concern, so reduce
            // each spec to its bare tool name before lookup; a plain value may
            // still be a comma/whitespace-separated list of bare tool names.
            let names = value.split('(').next().unwrap_or(value.as_str());
            for token in names
                .split(|ch: char| ch == ',' || ch.is_whitespace())
                .filter(|token| !token.is_empty())
            {
                let normalized = normalize_tool_name(token);
                let canonical = name_map.get(&normalized).ok_or_else(|| {
                    ToolError::InvalidInput(format!(
                        "unsupported tool in --allowedTools: {token} (expected one of: {expected_names})"
                    ))
                })?;
                allowed.insert(canonical.clone());
            }
        }

        Ok(Some(allowed))
    }

    #[must_use]
    pub fn definitions(
        &self,
        allowed_tools: Option<&BTreeSet<String>>,
    ) -> Vec<api::ToolDefinition> {
        let builtin = self
            .builtin_tool_specs()
            .into_iter()
            .filter(|spec| allowed_tools.is_none_or(|allowed| allowed.contains(spec.name)))
            // Deferred families are reached through `CapabilityInvoke`, not
            // advertised — except when an explicit `--allowedTools` list names
            // them (honored verbatim; deferring there would advertise nothing).
            .filter(|spec| allowed_tools.is_some() || !is_deferred_tool(spec.name))
            .map(|spec| api::ToolDefinition {
                name: spec.name.to_string(),
                description: Some(spec.description.to_string()),
                input_schema: spec.input_schema,
            });
        // MCP and plugin schemas are never advertised unless `--allowedTools`
        // names them: they are found with `ToolSearch` and run with
        // `CapabilityInvoke`.
        //
        // Two earlier shapes are worth remembering, because this one is the
        // answer to both. OpenAI targets front-loaded every discovered schema
        // so a search could not mutate the cached prefix — ~77 tools and
        // ~55 KB on every request, including turns that call no tool at all,
        // and the loudest schemas in that surface are the orchestration
        // families, which biases plain requests toward over-orchestration.
        // Then activation replaced it: cheap at rest, but each search that
        // matched something new rewrote the tool block and re-billed the whole
        // conversation (191 breaks, 67.5M tokens live). Activation existed for
        // one reason — a strict function-calling provider can only emit a call
        // for a tool that is advertised on that very request — and
        // `CapabilityInvoke` satisfies that reason with a schema that is
        // always there. So the wire list is now a function of configuration
        // alone, identical on every request of a session.
        let runtime: Vec<api::ToolDefinition> = self
            .runtime_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|tool| !self.is_tool_disabled(&tool.name))
            .filter(|tool| allowed_tools.is_some_and(|allowed| allowed.contains(tool.name.as_str())))
            .map(|tool| api::ToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
            .collect();
        let plugin = self
            .plugin_tools
            .iter()
            .filter(|tool| !self.is_tool_disabled(&tool.definition().name))
            .filter(|tool| {
                allowed_tools.is_some_and(|allowed| allowed.contains(tool.definition().name.as_str()))
            })
            .map(|tool| api::ToolDefinition {
                name: tool.definition().name.clone(),
                description: tool.definition().description.clone(),
                input_schema: tool.definition().input_schema.clone(),
            });
        builtin.chain(runtime).chain(plugin).collect()
    }

    pub fn permission_specs(
        &self,
        allowed_tools: Option<&BTreeSet<String>>,
    ) -> Result<Vec<(String, PermissionMode)>, ToolError> {
        let mut builtin_specs = self.builtin_tool_specs();
        if !builtin_specs.iter().any(|spec| spec.name == "imagegen")
            && !self.is_tool_disabled("imagegen")
        {
            builtin_specs.extend(crate::image_tools::tool_specs());
        }
        let builtin = builtin_specs
            .into_iter()
            .filter(|spec| allowed_tools.is_none_or(|allowed| allowed.contains(spec.name)))
            .map(|spec| (spec.name.to_string(), spec.required_permission));
        let runtime: Vec<(String, PermissionMode)> = self
            .runtime_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|tool| !self.is_tool_disabled(&tool.name))
            .filter(|tool| allowed_tools.is_none_or(|allowed| allowed.contains(tool.name.as_str())))
            .map(|tool| (tool.name.clone(), tool.required_permission))
            .collect();
        let plugin = self
            .plugin_tools
            .iter()
            .filter(|tool| !self.is_tool_disabled(&tool.definition().name))
            .filter(|tool| {
                allowed_tools
                    .is_none_or(|allowed| allowed.contains(tool.definition().name.as_str()))
            })
            .map(|tool| {
                permission_mode_from_plugin(tool.required_permission())
                    .map(|permission| (tool.definition().name.clone(), permission))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(builtin.chain(runtime).chain(plugin).collect())
    }

    #[must_use]
    pub fn has_runtime_tool(&self, name: &str) -> bool {
        if self.is_tool_disabled(name) {
            return false;
        }
        self.runtime_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|tool| tool.name == name)
    }

    /// Snapshot of the currently advertised runtime (MCP) tool definitions.
    /// A mid-session `tools/list_changed` refresh reads this, splices the
    /// changed server's tools, and writes the result back via
    /// [`Self::set_runtime_tools`] — the registry is the source of truth for
    /// "what the model currently sees".
    #[must_use]
    pub fn runtime_tool_definitions(&self) -> Vec<RuntimeToolDefinition> {
        self.runtime_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Names of all registered plugin-backed tools. The runtime treats these
    /// as long-running (executed via `spawn_blocking`) because each plugin
    /// tool spawns a blocking subprocess like `Bash` — running it inline would
    /// freeze the TUI render loop.
    #[must_use]
    pub fn plugin_tool_names(&self) -> Vec<String> {
        self.plugin_tools
            .iter()
            .filter(|tool| !self.is_tool_disabled(&tool.definition().name))
            .map(|tool| tool.definition().name.clone())
            .collect()
    }

    /// Snapshot every currently resolvable tool name, including builtins kept
    /// off the wire, runtime/MCP tools, and plugin tools. This is deliberately
    /// broader than [`Self::definitions`]: Anthropic accepts historical tool
    /// names without requiring their schemas in the current request, while
    /// strict providers still reconcile against the advertised subset.
    #[must_use]
    pub fn resolvable_tool_names(&self) -> BTreeSet<String> {
        self.searchable_tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect()
    }

    #[must_use]
    pub fn toggleable_tools(&self) -> Vec<ToggleableTool> {
        let builtin = mvp_tool_specs()
            .iter()
            .filter(|spec| spec.name != "LSP" || !self.context.lsp.is_empty())
            .filter(|spec| !is_agent_only_tool(spec.name))
            .map(|spec| ToggleableTool {
                name: spec.name.to_string(),
                description: Some(spec.description.to_string()),
                source: ToggleableToolSource::Builtin,
                enabled: !self.is_tool_disabled(spec.name),
            });
        let runtime = self
            .runtime_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|tool| ToggleableTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                source: ToggleableToolSource::Runtime,
                enabled: !self.is_tool_disabled(&tool.name),
            })
            .collect::<Vec<_>>();
        let plugin = self.plugin_tools.iter().map(|tool| {
            let definition = tool.definition();
            ToggleableTool {
                name: definition.name.clone(),
                description: definition.description.clone(),
                source: ToggleableToolSource::Plugin,
                enabled: !self.is_tool_disabled(&definition.name),
            }
        });
        builtin.chain(runtime).chain(plugin).collect()
    }

    #[must_use]
    pub fn search(
        &self,
        query: &str,
        max_results: usize,
        pending_mcp_servers: Option<Vec<String>>,
        mcp_degraded: Option<McpDegradedReport>,
    ) -> ToolSearchOutput {
        let query = query.trim().to_string();
        let normalized_query = misc_tools::normalize_tool_search_query(&query);
        let specs = self.searchable_tool_specs();
        let matches = misc_tools::search_tool_specs(&query, max_results.max(1), &specs);
        // Return the matched tools' full definitions, not just names: a
        // deferred tool's schema is exactly what the caller is here for —
        // a bare name list would leave the model guessing the input shape.
        let schemas: Vec<api::ToolDefinition> = matches
            .iter()
            .filter_map(|name| self.definition_for_name(name))
            .collect();
        // Looking up is NOT activation. A search used to insert its matches
        // into the wire advertisement so a strict function-calling provider
        // could emit the call the model had just found — which meant every
        // search rewrote the tool block at the very front of the cached
        // prefix and re-billed the whole conversation behind it: 191 breaks
        // and 67.5M tokens in the live ledger, 18.6% of every cache read this
        // machine has lost. `CapabilityInvoke` removes the reason to activate:
        // the model runs a found tool through a schema that is ALWAYS on the
        // wire, so the wire never has to change and the prefix stays
        // byte-identical for the whole session.
        ToolSearchOutput {
            matches,
            schemas,
            query,
            normalized_query,
            total_deferred_tools: specs.len(),
            pending_mcp_servers,
            mcp_degraded,
        }
    }

    /// Full definition (description + input schema) for one registered tool,
    /// searched across builtin, runtime (MCP), and plugin tools. `None` for
    /// unknown or agent-only names.
    fn definition_for_name(&self, name: &str) -> Option<api::ToolDefinition> {
        if let Some(spec) = self
            .builtin_tool_specs()
            .into_iter()
            .find(|spec| spec.name == name)
        {
            return Some(api::ToolDefinition {
                name: spec.name.to_string(),
                description: Some(spec.description.to_string()),
                input_schema: spec.input_schema,
            });
        }
        if let Some(tool) = self
            .runtime_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|tool| tool.name == name)
        {
            return Some(api::ToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            });
        }
        self.plugin_tools
            .iter()
            .map(plugins::PluginTool::definition)
            .find(|definition| definition.name == name)
            .map(|definition| api::ToolDefinition {
                name: definition.name.clone(),
                description: definition.description.clone(),
                input_schema: definition.input_schema.clone(),
            })
    }

    pub fn set_enforcer(&mut self, enforcer: PermissionEnforcer) {
        self.enforcer = Some(enforcer);
    }

    // The linear order of toggle, agent-only, plan-mode, builtin, and plugin
    // gates is security-significant; splitting it would hide early-return
    // precedence merely to meet a line-count heuristic.
    #[allow(clippy::too_many_lines)]
    pub fn execute(&self, name: &str, input: &Value) -> Result<String, ToolError> {
        // Accept PascalCase (`Read`, `Write`, ...) and short forms (`read`)
        // by resolving to the canonical handler name before lookup/dispatch.
        let canonical = crate::aliases::canonical_tool_name(name);
        let canonical_ref = canonical.as_str();
        // Keep the registry honest about the always-advertised address too.
        // CLI executors unwrap before their own allow/deny gates; this final
        // route makes direct registry users and future executor seams obey the
        // same advertised contract instead of falling through to NotFound.
        if canonical_ref == crate::capability::CAPABILITY_INVOKE {
            let call = crate::capability::unwrap_capability_invoke(input)
                .map_err(ToolError::InvalidInput)?;
            let inner_input: Value = serde_json::from_str(&call.input)?;
            return self.execute(&call.name, &inner_input);
        }
        if self.is_tool_disabled(canonical_ref) {
            let invocation =
                gateway::begin_tool_invocation(name, canonical_ref, input, self.enforcer.as_ref());
            let error = disabled_tool_error(canonical_ref);
            self.context.record_tool_invocation(
                invocation
                    .with_policy_decision(toggle_denied_decision(TOOL_TOGGLE_DENIAL_REASON))
                    .finish(failed_result(&error), gateway::epoch_millis_now()),
            );
            return Err(error);
        }
        // Agent-only tools are never runnable through this registry (the main /
        // headless / serve session). `InstrumentLog` is hidden from advertisement
        // and search, but a model could still emit the name out-of-band; running
        // it here would stage a probe in a context that has NO revert lifecycle —
        // leaking the marker AND (via the auto-format guard) suppressing
        // formatting session-wide. The debugger sub-agent reaches it through
        // `execute_tool_with_context` directly, not this path, so it is unaffected.
        if is_agent_only_tool(canonical_ref) {
            return Err(ToolError::NotFound(name.to_owned()));
        }
        // Authoritative plan gate. When the user has explicitly selected Plan the
        // toggle tools are dropped from advertisement (`builtin_tool_specs`), but a
        // stale or in-flight tool call the model already emitted can still arrive.
        // Intercept it here — ahead of the generic `WorkspaceWrite` dispatch — so
        // the deterministic write denial cannot recur turn after turn:
        //   * `EnterPlanMode`: already in Plan, so treat as an idempotent no-op
        //     success. No file is written and no permission mode is touched.
        //   * legacy `ExitPlanMode`: the model must not be able to restore write
        //     access. Fail with a clear user-controlled-mode message; the user
        //     leaves Plan only via Shift+Tab or `/plan off`. Settings are untouched.
        // `ExitPlanModeV2` is intentionally excluded — it stays runnable (by
        // name or through `CapabilityInvoke`; it left the wire in t-2903) so
        // the model can submit a plan for approval.
        if self.context.plan_selected() && is_plan_reentry_tool(canonical_ref) {
            let invocation =
                gateway::begin_tool_invocation(name, canonical_ref, input, self.enforcer.as_ref())
                    .with_family(ToolFamily::PlanMode);
            if canonical_ref == "EnterPlanMode" {
                let metadata = ToolResultMetadata {
                    output_chars: PLAN_ALREADY_ACTIVE_MESSAGE.chars().count(),
                    returned_chars: PLAN_ALREADY_ACTIVE_MESSAGE.chars().count(),
                    truncated: false,
                    artifact: None,
                };
                self.context.record_tool_invocation(
                    invocation.finish(successful_result(metadata), gateway::epoch_millis_now()),
                );
                return Ok(PLAN_ALREADY_ACTIVE_MESSAGE.to_owned());
            }
            let error = ToolError::PermissionDenied {
                tool: canonical_ref.to_owned(),
                reason: PLAN_EXIT_USER_CONTROLLED_MESSAGE.to_owned(),
            };
            self.context
                .record_tool_invocation(invocation.finish(
                    failed_result(&error),
                    gateway::epoch_millis_now(),
                ));
            return Err(error);
        }
        if canonical_ref == "ToolSearch" {
            return self.execute_tool_search(name, input);
        }
        if mvp_tool_specs()
            .iter()
            .any(|spec| spec.name == canonical_ref)
        {
            return execute_tool_with_context(
                &self.context,
                self.enforcer.as_ref(),
                canonical_ref,
                input,
            );
        }
        // Genuine unknown tool name: neither a builtin, ToolSearch, nor a plugin
        // tool. Surface an actionable error (nearest registered names + the
        // deferred-tool ToolSearch hint) instead of a dead string.
        let Some(plugin_tool) = self
            .plugin_tools
            .iter()
            .find(|tool| tool.definition().name == canonical_ref)
        else {
            // The name is whatever the model emitted, so cap it here too — this
            // early return is outside the dispatch seam.
            return Err(crate::dispatch::bound_tool_error(
                self.unknown_tool_error(name),
                None,
            ));
        };
        let invocation =
            gateway::begin_tool_invocation(name, canonical_ref, input, self.enforcer.as_ref())
                .with_family(ToolFamily::Plugin);
        if let Some(enforcer) = self.enforcer.as_ref() {
            if let Err(error) = enforce_permission_check(enforcer, canonical_ref, input) {
                self.context.record_tool_invocation(
                    invocation.finish(failed_result(&error), gateway::epoch_millis_now()),
                );
                return Err(error);
            }
        }
        match plugin_tool.execute(input) {
            Ok(output) => {
                let metadata = ToolResultMetadata {
                    output_chars: output.chars().count(),
                    returned_chars: output.chars().count(),
                    truncated: false,
                    artifact: None,
                };
                self.context.record_tool_invocation(
                    invocation.finish(successful_result(metadata), gateway::epoch_millis_now()),
                );
                Ok(output)
            }
            Err(error) => {
                // This branch never enters `dispatch_tool_inner`, so it applies
                // the error cap itself. A plugin's stderr rides this string up
                // to `MAX_PLUGIN_OUTPUT_BYTES` (1 MiB), and an error result is
                // exempt from historical wire compression, so uncapped it would
                // sit at full size in every later request.
                let error =
                    crate::dispatch::bound_tool_error(ToolError::Execution(error.to_string()), None);
                self.context.record_tool_invocation(
                    invocation.finish(failed_result(&error), gateway::epoch_millis_now()),
                );
                Err(error)
            }
        }
    }

    fn execute_tool_search(&self, name: &str, input: &Value) -> Result<String, ToolError> {
        let invocation =
            gateway::begin_tool_invocation(name, "ToolSearch", input, self.enforcer.as_ref());
        if let Some(enforcer) = self.enforcer.as_ref() {
            if let Err(error) = enforce_permission_check(enforcer, "ToolSearch", input) {
                self.context.record_tool_invocation(
                    invocation.finish(failed_result(&error), gateway::epoch_millis_now()),
                );
                return Err(error);
            }
        }
        let raw = match from_value::<misc_tools::ToolSearchInput>(input).and_then(|input| {
            to_pretty_json(self.search(&input.query, input.max_results.unwrap_or(5), None, None))
        }) {
            Ok(raw) => raw,
            Err(error) => {
                self.context.record_tool_invocation(
                    invocation.finish(failed_result(&error), gateway::epoch_millis_now()),
                );
                return Err(error);
            }
        };

        Ok(finish_successful_tool_output(
            &self.context,
            invocation,
            "ToolSearch",
            input,
            raw,
            None,
        ))
    }

    pub(crate) fn searchable_tool_specs(&self) -> Vec<SearchableToolSpec> {
        let builtin = self
            .builtin_tool_specs()
            .into_iter()
            .map(|spec| SearchableToolSpec {
                name: spec.name.to_string(),
                description: spec.description.to_string(),
            });
        let runtime: Vec<SearchableToolSpec> = self
            .runtime_tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|tool| !self.is_tool_disabled(&tool.name))
            .map(|tool| SearchableToolSpec {
                name: tool.name.clone(),
                description: tool.description.clone().unwrap_or_default(),
            })
            .collect();
        let plugin = self
            .plugin_tools
            .iter()
            .filter(|tool| !self.is_tool_disabled(&tool.definition().name))
            .map(|tool| SearchableToolSpec {
                name: tool.definition().name.clone(),
                description: tool.definition().description.clone().unwrap_or_default(),
            });
        builtin.chain(runtime).chain(plugin).collect()
    }

    fn builtin_tool_specs(&self) -> Vec<ToolSpec> {
        // While the user has explicitly selected Plan the session is read-only,
        // so advertising the write-gated plan-toggle tools (`EnterPlanMode` /
        // legacy `ExitPlanMode`) only lets the model re-request a mode it is
        // already in and hit the deterministic WorkspaceWrite denial every turn.
        // Drive their visibility from the authoritative plan flag, not a prompt
        // hint. `ExitPlanModeV2` (ReadOnly plan submission) is deferred like
        // everything else that is not the coding core, and stays reachable.
        let plan_selected = self.context.plan_selected();
        let imagegen_available = self
            .context
            .active_model()
            .is_some_and(|model| crate::image_tools::supports_model(&model));
        mvp_tool_specs()
            .iter()
            .filter(|spec| spec.name != "LSP" || !self.context.lsp.is_empty())
            .filter(|spec| spec.name != "imagegen" || imagegen_available)
            .filter(|spec| !is_agent_only_tool(spec.name))
            .filter(|spec| !self.is_tool_disabled(spec.name))
            .filter(|spec| !plan_selected || !is_plan_reentry_tool(spec.name))
            .cloned()
            .collect()
    }

    /// Build the actionable [`ToolError::NotFound`] for a tool name that resolved
    /// to nothing. Candidates are every currently-known executable tool name
    /// (builtin — including deferred ones — plus runtime/MCP and plugin tools),
    /// the same universe `ToolSearch` searches, so a near-miss typo gets a
    /// concrete "did you mean" and the model is reminded that deferred tools must
    /// be loaded via `ToolSearch` before they can be called.
    fn unknown_tool_error(&self, name: &str) -> ToolError {
        let candidates: Vec<String> = self
            .searchable_tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        ToolError::NotFound(unknown_tool_message(name, &candidates))
    }
}

/// Compose the human/model-readable body of an "unsupported tool" error:
/// the offending name, up to two nearest registered names (only when close
/// enough to be a real typo — never a noise guess), and the always-present
/// deferred-tool hint. Rendered through [`ToolError::NotFound`]'s
/// `"unsupported tool: {0}"` shell, so the full line reads e.g.
/// `unsupported tool: reed — did you mean `Read`? If this is a deferred tool,
/// load it with ToolSearch first (query "select:<Name>").`
fn unknown_tool_message(name: &str, candidates: &[String]) -> String {
    use std::fmt::Write as _;

    let mut message = name.to_string();
    let suggestions = closest_tool_names(name, candidates);
    match suggestions.as_slice() {
        [] => {}
        // `write!` to a String is infallible, so the result is intentionally
        // discarded.
        [only] => {
            let _ = write!(message, " — did you mean `{only}`?");
        }
        [first, second, ..] => {
            let _ = write!(message, " — did you mean `{first}` or `{second}`?");
        }
    }
    message.push_str(
        " If this is a deferred tool, load it with ToolSearch first (query \"select:<Name>\").",
    );
    message
}

/// Up to two registered tool names within a short edit distance of `name`,
/// nearest first. Case-insensitive Levenshtein; a candidate qualifies only when
/// the edit distance is at most a third of the longer name (clamped to 2..=4),
/// so a genuine typo (`reed`→`Read`, `tool_serch`→`ToolSearch`) is offered while
/// an unrelated name (`frobnicate`) yields nothing — the "no wild guesses" bar.
fn closest_tool_names(name: &str, candidates: &[String]) -> Vec<String> {
    let query = name.to_ascii_lowercase();
    let mut ranked: Vec<(usize, &str)> = candidates
        .iter()
        .filter_map(|candidate| {
            let lowered = candidate.to_ascii_lowercase();
            if lowered == query {
                // Identical modulo case — the caller already failed to resolve
                // it, so echoing it back as a suggestion would be noise.
                return None;
            }
            let distance = core_types::text::levenshtein_distance(&query, &lowered);
            let longer = query.chars().count().max(lowered.chars().count());
            let threshold = (longer / 3).clamp(2, 4);
            (distance <= threshold).then_some((distance, candidate.as_str()))
        })
        .collect();
    // Nearest first; break ties by name for deterministic output.
    ranked.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));
    ranked
        .into_iter()
        .map(|(_, candidate)| candidate.to_string())
        .take(2)
        .collect()
}

/// Tools that exist for sub-agents only and must never be surfaced to the
/// main (interactive / headless / serve) session through this registry —
/// neither advertised ([`GlobalToolRegistry::definitions`]) nor discoverable
/// ([`GlobalToolRegistry::search`]).
///
/// `InstrumentLog` inserts auto-reverted debug probes whose cleanup is bound to
/// a sub-agent's run lifecycle (`SubagentToolExecutor::revert_probes`, drained
/// at completion). The main session has no such boundary, so offering it there
/// would leak `/*ZO_PROBE*/` markers into the user's files. Sub-agents reach
/// it via their own explicit allow-list path (`tool_specs_for_allowed_tools` +
/// `execute_tool_with_context`), which does not go through `builtin_tool_specs`
/// or this registry's [`GlobalToolRegistry::execute`], so the debugger is
/// unaffected. [`GlobalToolRegistry::execute`] additionally REJECTS these names
/// so an out-of-band call from the main session cannot run them.
///
/// `DebugHypothesis` is hidden for the same reason of scope, not safety: it is a
/// debugger sub-agent's per-run hypothesis ledger (paired with `InstrumentLog`),
/// meaningless to the main interactive session, which has no debugging run to
/// track. Same policy as `InstrumentLog` keeps debug-mode tooling sub-agent-only.
fn is_agent_only_tool(name: &str) -> bool {
    matches!(name, "InstrumentLog" | "DebugHypothesis")
}

/// The write-gated plan-*toggle* tools. While the user has explicitly selected
/// Plan the session is already read-only Plan, so these only let the model
/// re-request the mode it is in — `EnterPlanMode` re-enters, legacy
/// `ExitPlanMode` would try to leave. Both are dropped from the wire and
/// handled specially in [`GlobalToolRegistry::execute`] under an active plan.
/// `ExitPlanModeV2` (the read-only plan-submission tool) is deliberately not
/// listed: it stays reachable so the model can present a plan for approval.
fn is_plan_reentry_tool(name: &str) -> bool {
    matches!(name, "EnterPlanMode" | "ExitPlanMode")
}

/// Returned for a stale `EnterPlanMode` call while the user has already selected
/// Plan: an idempotent no-op success so the model observes the mode it asked for
/// without triggering the write path.
const PLAN_ALREADY_ACTIVE_MESSAGE: &str =
    "Plan mode is already active (selected by the user). No change was made.";

/// Denial reason for a stale legacy `ExitPlanMode` call under user-selected
/// Plan: the model cannot restore write access; only the user leaves Plan.
const PLAN_EXIT_USER_CONTROLLED_MESSAGE: &str =
    "Plan mode is controlled by the user and cannot be exited by a tool call. \
The user leaves plan mode with Shift+Tab or `/plan off`.";

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;

    use super::{
        deferred_tool_manifest_section, deferred_tool_names, GlobalToolRegistry,
        DEFERRED_TOOL_HOOKS,
    };
    use crate::ToolPolicyDecision;
    use plugins::{PluginTool, PluginToolDefinition, PluginToolPermission};
    use runtime::{permission_enforcer::PermissionEnforcer, PermissionMode, PermissionPolicy};
    use serde_json::json;


    fn temp_path(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("zo-registry-{unique}-{name}"))
    }

    fn plugin_tool(name: &str) -> PluginTool {
        PluginTool::new(
            "demo@external",
            "demo",
            PluginToolDefinition {
                name: name.to_string(),
                description: Some("demo tool".to_string()),
                input_schema: json!({ "type": "object" }),
            },
            "./tool.sh",
            Vec::new(),
            PluginToolPermission::ReadOnly,
            None,
        )
    }

    #[test]
    fn imagegen_is_deferred_and_reachable_only_for_current_gpt_models() {
        let registry = GlobalToolRegistry::builtin();
        let advertised = |registry: &GlobalToolRegistry| {
            registry
                .definitions(None)
                .iter()
                .any(|definition| definition.name == "imagegen")
        };

        let imagegen_permission = |registry: &GlobalToolRegistry| {
            registry
                .permission_specs(None)
                .expect("permission specs")
                .into_iter()
                .find_map(|(name, mode)| (name == "imagegen").then_some(mode))
        };
        let reachable = |registry: &GlobalToolRegistry| {
            registry
                .search("select:imagegen", 1, None, None)
                .schemas
                .iter()
                .any(|definition| definition.name == "imagegen")
        };

        assert!(!advertised(&registry));
        assert!(!reachable(&registry));
        assert_eq!(imagegen_permission(&registry), Some(PermissionMode::WorkspaceWrite));
        registry.context().set_active_model("gpt-5.6-sol");
        assert!(!advertised(&registry));
        assert!(reachable(&registry));
        assert_eq!(imagegen_permission(&registry), Some(PermissionMode::WorkspaceWrite));
        registry.context().set_active_model("claude-opus-5");
        assert!(!advertised(&registry));
        assert!(!reachable(&registry));
        assert_eq!(imagegen_permission(&registry), Some(PermissionMode::WorkspaceWrite));
    }

    #[test]
    fn deferred_families_are_off_the_wire_but_searchable() {
        let registry = GlobalToolRegistry::builtin();

        let wire: BTreeSet<String> = registry
            .definitions(None)
            .into_iter()
            .map(|def| def.name)
            .collect();
        let searchable: BTreeSet<String> = registry
            .searchable_tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect();

        // Deferred orchestration / scheduling tools are surfaced only through
        // ToolSearch, never advertised on the per-request wire.
        for deferred in [
            "EnterWorktree",
            "Workflow",
            "WorkflowValidate",
            "WorkflowLibrary",
            "WorkflowRuns",
            "WorkflowSkillProject",
            "TeamInboxPost",
            "CronCreate",
            "WorkerCreate",
            "WebSearch",
            "TaskList",
            "Council",
            "NotebookEdit",
        ] {
            assert!(
                !wire.contains(deferred),
                "{deferred} must not be advertised on the wire"
            );
            assert!(
                searchable.contains(deferred),
                "{deferred} must remain reachable via ToolSearch"
            );
        }

        // The lean coding core stays advertised regardless. This is the full
        // intended wire set (plus LSP when a server is registered) — if it
        // grows, that is a deliberate prefix-cost decision, not drift.
        for core in [
            "bash",
            "read_file",
            "write_file",
            "edit_file",
            "MultiEdit",
            "glob_search",
            "grep_search",
            "TodoWrite",
            // `Agent` and `AskUserQuestion` used to sit here. They are deferred
            // now — the two biggest schemas on the wire against 0.8% of tool
            // calls — and the reasoning with its measurements is at their entry
            // in `DEFERRED_TOOL_NAMES_EXTRA`. `deferred_families_are_off_the_wire_but_searchable`
            // is the test that would catch them creeping back.
            //
            // `read_image`, `MemoryWrite`, `retrieve_tool_output`,
            // `session_recall` and `send_to_user` left in r49, for the tool
            // bucket's 2,000-token budget. `send_to_user` is the one that had
            // an argument here — "a mid-run push tool; deferring it adds a
            // round-trip of friction to a long autonomous run". The friction is
            // real and it is bounded: `searching_never_changes_the_wire_advertisement`
            // means a searched schema arrives in the RESULT, not in the prefix,
            // so the cost is one lookup on first use in a session, and only in
            // the sessions that push anything at all. Its 165 tokens were paid
            // by every request of every session instead.
            "Skill",
            "ToolSearch",
            // Back since t-2903 in the seat `ExitPlanModeV2` held (7 calls in
            // 7 of 347 sessions against `Agent`'s 47): the first delegation of
            // a session is one parent turn again, as it is in Claude Code. The
            // schema that came back is the slim one — the numbers are at the
            // `ExitPlanModeV2` entry of `DEFERRED_TOOL_NAMES_EXTRA`.
            "Agent",
            // The door to everything else. Without it on the wire, a deferred
            // tool found by `ToolSearch` would have no way to be called at
            // all — and the wire would have to grow to make one.
            "CapabilityInvoke",
        ] {
            assert!(wire.contains(core), "{core} must stay on the wire");
        }
        // r49 closes the default wire at twelve schemas. Dynamic LSP schemas
        // are registered separately; this builtin registry has none.
        assert!(
            wire.len() <= 12,
            "wire advertisement crept past the lean core ({} tools): {wire:?}",
            wire.len()
        );
    }

    /// The wire list must be a function of configuration alone.
    ///
    /// It used to grow: a `ToolSearch` that matched something new ADDED the
    /// schema, which rewrote the tool block at the head of the cached prefix
    /// and re-billed the conversation behind it — 191 breaks and 67.5M tokens
    /// in the live ledger. `CapabilityInvoke` is what made growth unnecessary,
    /// so this test is the property that replaced it: search all you like, the
    /// advertisement does not move.
    #[test]
    fn searching_never_changes_the_wire_advertisement() {
        let registry = GlobalToolRegistry::builtin();
        let wire = |registry: &GlobalToolRegistry| {
            registry
                .definitions(None)
                .into_iter()
                .map(|definition| definition.name)
                .collect::<Vec<_>>()
        };
        let before = wire(&registry);
        for query in ["select:Workflow", "select:REPL,LSP", "worktree", "cron schedule"] {
            let found = registry.search(query, 10, None, None);
            assert!(
                !found.schemas.is_empty(),
                "{query} should still FIND tools — deferral is about advertising, not hiding"
            );
        }
        assert_eq!(
            before,
            wire(&registry),
            "a search moved the wire advertisement"
        );
    }

    #[test]
    fn extra_deferred_names_resolve_to_registered_specs() {
        // Drift guard for DEFERRED_TOOL_NAMES_EXTRA: a renamed or removed tool
        // must fail here instead of silently rejoining the wire advertisement.
        let registered: BTreeSet<&str> = super::mvp_tool_specs()
            .iter()
            .map(|spec| spec.name)
            .collect();
        for name in super::DEFERRED_TOOL_NAMES_EXTRA {
            assert!(
                registered.contains(name),
                "deferred name {name} does not match any registered tool spec"
            );
        }
    }

    #[test]
    fn deferred_manifest_carries_capability_hooks() {
        let section = deferred_tool_manifest_section();
        for (name, hook) in DEFERRED_TOOL_HOOKS {
            assert!(
                deferred_tool_names().contains(name),
                "hooked tool {name} must actually be deferred"
            );
            assert!(
                section.contains(&format!("{name} ({hook})")),
                "manifest must sell {name}'s capability: {section}"
            );
        }
    }

    /// A hidden schema needs more than its name: each of these families has a
    /// distinctive reason to pay the `ToolSearch` round-trip. Keep this list
    /// explicit so a refactor cannot leave an armed feature behind a door the
    /// model has no reason to open.
    #[test]
    fn deferred_arsenal_families_have_reason_to_load_hooks() {
        let hooks: std::collections::BTreeMap<&str, &str> =
            DEFERRED_TOOL_HOOKS.iter().copied().collect();
        for (anchor, clues) in [
            ("Council", &["anonymized", "tie"][..]),
            (
                "find_symbol",
                &["find_references", "file_outline", "grep"][..],
            ),
            ("WebSearch", &["WebFetch", "stale"][..]),
            ("WorkerCreate", &["observe", "restart", "terminate"][..]),
            ("TeamCreate", &["team tasks", "inbox", "without launching"][..]),
            ("SkillDistill", &["repeatable", "proposed", "skill"][..]),
            ("SkillReview", &["approve", "discard", "active"][..]),
        ] {
            let hook = hooks
                .get(anchor)
                .unwrap_or_else(|| panic!("{anchor} lost its deferred capability hook"));
            for clue in clues {
                assert!(
                    hook.to_ascii_lowercase()
                        .contains(&clue.to_ascii_lowercase()),
                    "{anchor} hook must explain {clue:?}, got {hook:?}"
                );
            }
        }
    }

    #[test]
    fn deferred_manifest_names_every_deferred_tool_and_nothing_else() {
        let section = deferred_tool_manifest_section();
        assert!(section.starts_with("# Deferred tools"));
        // Factory-derived: every deferred name appears, and no advertised core
        // tool leaks in.
        for name in deferred_tool_names() {
            assert!(section.contains(name), "manifest must name {name}");
        }
        let listed: std::collections::BTreeSet<&str> = section
            .split(": ")
            .nth(1)
            .expect("manifest carries a name list")
            .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
            .split(". Before calling")
            .next()
            .expect("name list precedes the usage hint")
            .split(", ")
            .collect();
        // Tools that stay ON the wire must never appear in the manifest.
        // `Agent` was here until it was deferred — see its entry in
        // `DEFERRED_TOOL_NAMES_EXTRA` for the measurement that moved it.
        for core in ["bash", "read_file", "edit_file", "TodoWrite"] {
            assert!(
                !listed.contains(core),
                "advertised tool {core} must not be listed as deferred"
            );
        }
        assert!(
            section.contains("ToolSearch"),
            "manifest must tell the model how to load a deferred schema"
        );
    }

    #[test]
    fn tool_search_returns_full_schemas_for_selected_deferred_tools() {
        let registry = GlobalToolRegistry::builtin();
        let output = registry.search("select:Workflow", 3, None, None);

        assert_eq!(output.matches, vec!["Workflow".to_string()]);
        let schema = output
            .schemas
            .iter()
            .find(|definition| definition.name == "Workflow")
            .expect("selected deferred tool must come back with its definition");
        assert!(
            schema.description.as_deref().unwrap_or_default().len() > 40,
            "definition carries the real description"
        );
        assert!(
            schema.input_schema.get("properties").is_some(),
            "definition carries the input schema, not just a name"
        );
    }

    #[test]
    fn explicit_allowlist_overrides_deferral() {
        let registry = GlobalToolRegistry::builtin();
        let allowed: BTreeSet<String> = ["EnterWorktree".to_string()].into_iter().collect();

        let wire: Vec<String> = registry
            .definitions(Some(&allowed))
            .into_iter()
            .map(|def| def.name)
            .collect();

        // An explicit --allowedTools list is honored verbatim; deferral must not
        // strip a tool the caller asked for by name (that would advertise none).
        assert_eq!(wire, vec!["EnterWorktree".to_string()]);
    }

    #[test]
    fn plugin_tool_names_lists_registered_plugin_tools() {
        let registry = GlobalToolRegistry::with_plugin_tools(vec![
            plugin_tool("alpha_tool"),
            plugin_tool("beta_tool"),
        ])
        .expect("registry builds");

        let mut names = registry.plugin_tool_names();
        names.sort();
        assert_eq!(names, ["alpha_tool", "beta_tool"]);
    }

    #[test]
    fn plugin_tool_names_is_empty_without_plugins() {
        let registry = GlobalToolRegistry::with_plugin_tools(Vec::new()).expect("empty registry");
        assert!(registry.plugin_tool_names().is_empty());
    }

    #[test]
    fn resolvable_tool_names_include_deferred_runtime_and_plugin_tools() {
        let registry = GlobalToolRegistry::with_plugin_tools(vec![plugin_tool("plugin_demo")])
            .expect("plugin registry")
            .with_runtime_tools(vec![runtime_tool("mcp__demo__echo")])
            .expect("runtime tool");

        let names = registry.resolvable_tool_names();
        assert!(names.contains("TaskList"), "deferred builtin");
        assert!(names.contains("mcp__demo__echo"), "runtime/MCP tool");
        assert!(names.contains("plugin_demo"), "plugin tool");
        assert!(
            registry
                .definitions(None)
                .iter()
                .all(|definition| !matches!(
                    definition.name.as_str(),
                    "TaskList" | "mcp__demo__echo" | "plugin_demo"
                )),
            "the resolvable getter must not alter wire advertisement"
        );
    }

    #[test]
    fn plugin_tool_execution_respects_permission_enforcer_before_spawn() {
        let root = temp_path("plugin-permission");
        fs::create_dir_all(&root).expect("create root");
        let script = root.join("tool.sh");
        let marker = root.join("marker");
        fs::write(
            &script,
            "#!/bin/sh\nprintf ran > marker\nprintf plugin-output\n",
        )
        .expect("write script");

        let tool = PluginTool::new(
            "demo@external",
            "demo",
            PluginToolDefinition {
                name: "plugin_write".to_string(),
                description: Some("write-capable plugin".to_string()),
                input_schema: json!({ "type": "object" }),
            },
            "sh",
            vec!["tool.sh".to_string()],
            PluginToolPermission::WorkspaceWrite,
            Some(root.clone()),
        );
        let mut registry = GlobalToolRegistry::with_plugin_tools(vec![tool]).expect("registry");
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("plugin_write", PermissionMode::WorkspaceWrite);
        registry.set_enforcer(PermissionEnforcer::new(policy));

        let err = registry
            .execute("plugin_write", &json!({ "message": "hello" }))
            .expect_err("plugin tool should be denied before spawn");

        let error = err.to_string();
        assert!(error.contains("permission denied for `plugin_write`"));
        assert!(error.contains("current mode is read-only"));
        assert!(error.contains("Permission audit:"));
        assert!(error.contains("/permissions workspace-write"));
        assert!(!marker.exists(), "denied plugin tool must not spawn");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn instrument_log_is_hidden_from_the_main_session() {
        // Agent-only: it exists as a builtin spec (so dispatch and sub-agents
        // can resolve it) but must never be advertised to or discoverable by the
        // main session — there is no run boundary to auto-revert its probes.
        assert!(
            super::mvp_tool_specs()
                .iter()
                .any(|s| s.name == "InstrumentLog"),
            "InstrumentLog must exist as a builtin spec",
        );
        let registry = GlobalToolRegistry::builtin();
        assert!(
            !registry
                .definitions(None)
                .iter()
                .any(|t| t.name == "InstrumentLog"),
            "InstrumentLog must not be advertised to the main session",
        );
        assert!(
            !registry
                .searchable_tool_specs()
                .iter()
                .any(|t| t.name == "InstrumentLog"),
            "InstrumentLog must not be discoverable via search",
        );
        // And it is not RUNNABLE through the registry: an out-of-band call (a
        // hallucinated name) is rejected before dispatch, so the main session can
        // never stage a probe it has no lifecycle to revert.
        let result = registry.execute(
            "InstrumentLog",
            &json!({ "path": "x.rs", "anchor": "a", "statement": "b" }),
        );
        assert!(
            matches!(result, Err(super::ToolError::NotFound(_))),
            "main-session InstrumentLog must be rejected, got: {result:?}",
        );
    }

    #[test]
    fn normalize_allowed_tools_accepts_claude_code_permission_specs() {
        // A custom command's `allowed-tools: Bash(git status:*), Read` frontmatter
        // reaches the parser as the spec `Bash(git status:*)` plus `Read`. The
        // parenthesized scope is a permission-policy concern, not a tool-offer
        // concern, so the normalizer must reduce `Bash(git status:*)` to the bare
        // `bash` tool rather than tokenizing on the inner space and rejecting it.
        let registry = GlobalToolRegistry::builtin();
        let allowed = registry
            .normalize_allowed_tools(&["Bash(git status:*)".to_string(), "Read".to_string()])
            .expect("CC permission-spec form must normalize, not error")
            .expect("allow list");
        assert_eq!(
            allowed,
            BTreeSet::from(["bash".to_string(), "read_file".to_string()]),
            "Bash(git status:*) must reduce to the bare bash tool offer",
        );
    }

    #[test]
    fn debug_hypothesis_is_hidden_from_the_main_session() {
        // Like InstrumentLog, the debugger's hypothesis ledger is a sub-agent-only
        // tool: it exists as a builtin spec (so the debugger can resolve it) but is
        // never advertised, discoverable, or runnable through the main session.
        assert!(
            super::mvp_tool_specs()
                .iter()
                .any(|s| s.name == "DebugHypothesis"),
            "DebugHypothesis must exist as a builtin spec",
        );
        let registry = GlobalToolRegistry::builtin();
        assert!(
            !registry
                .definitions(None)
                .iter()
                .any(|t| t.name == "DebugHypothesis"),
            "DebugHypothesis must not be advertised to the main session",
        );
        assert!(
            !registry
                .searchable_tool_specs()
                .iter()
                .any(|t| t.name == "DebugHypothesis"),
            "DebugHypothesis must not be discoverable via search",
        );
        let result = registry.execute(
            "DebugHypothesis",
            &json!({ "hypothesis": "x", "status": "open" }),
        );
        assert!(
            matches!(result, Err(super::ToolError::NotFound(_))),
            "main-session DebugHypothesis must be rejected, got: {result:?}",
        );
    }

    #[test]
    fn unknown_tool_message_suggests_near_names_and_always_hints_toolsearch() {
        let candidates = vec![
            "read_file".to_string(),
            "write_file".to_string(),
            "edit_file".to_string(),
            "grep_search".to_string(),
            "glob_search".to_string(),
            "ToolSearch".to_string(),
        ];

        // A near-miss typo yields a concrete "did you mean" plus the hint.
        let close = super::unknown_tool_message("read_fil", &candidates);
        assert!(
            close.contains("did you mean `read_file`"),
            "expected a suggestion, got: {close}"
        );
        assert!(
            close.contains("If this is a deferred tool, load it with ToolSearch first"),
            "hint must always be present, got: {close}"
        );

        // A name far from everything gets NO suggestion (no wild guesses) but
        // still the deferred-tool ToolSearch hint.
        let far = super::unknown_tool_message("frobnicate", &candidates);
        assert!(far.starts_with("frobnicate"), "got: {far}");
        assert!(!far.contains("did you mean"), "no wild guess, got: {far}");
        assert!(
            far.contains("If this is a deferred tool, load it with ToolSearch first"),
            "hint must still be present, got: {far}"
        );

        // Two close names render with an `or` and are capped at two.
        let two = super::unknown_tool_message("grop_search", &candidates);
        assert!(
            two.contains("did you mean `grep_search` or `glob_search`?"),
            "expected two ranked suggestions, got: {two}"
        );
    }

    #[test]
    fn closest_tool_names_caps_at_two_and_skips_identical_casing() {
        let candidates = vec!["ToolSearch".to_string(), "TodoWrite".to_string()];
        // Identical-modulo-case is not echoed back as a suggestion.
        assert!(super::closest_tool_names("toolsearch", &candidates).is_empty());
        // Never more than two are returned.
        let many = vec![
            "read_file".to_string(),
            "read_image".to_string(),
            "reap_files".to_string(),
        ];
        assert!(super::closest_tool_names("read_fime", &many).len() <= 2);
    }

    #[test]
    fn execute_unknown_tool_name_returns_actionable_notfound() {
        let registry = GlobalToolRegistry::builtin();
        let result = registry.execute("read_fil", &json!({}));
        let error = result.expect_err("unknown tool must error");
        assert!(
            matches!(error, super::ToolError::NotFound(_)),
            "got: {error:?}"
        );
        let rendered = error.to_string();
        assert!(
            rendered.starts_with("unsupported tool: read_fil"),
            "rendered: {rendered}"
        );
        assert!(
            rendered.contains("did you mean `read_file`"),
            "rendered: {rendered}"
        );
        assert!(
            rendered.contains("load it with ToolSearch first"),
            "rendered: {rendered}"
        );
    }

    fn runtime_tool(name: &str) -> super::RuntimeToolDefinition {
        super::RuntimeToolDefinition {
            name: name.to_string(),
            description: Some("mcp tool".to_string()),
            input_schema: json!({ "type": "object" }),
            required_permission: runtime::PermissionMode::ReadOnly,
        }
    }

    #[test]
    fn disabled_tools_are_hidden_from_registry_surfaces_and_rejected() {
        let disabled = BTreeSet::from([
            "WebSearch".to_string(),
            "mcp__demo__echo".to_string(),
            "plugin_demo".to_string(),
        ]);
        let registry = GlobalToolRegistry::with_plugin_tools(vec![plugin_tool("plugin_demo")])
            .expect("plugin registry")
            .with_runtime_tools(vec![runtime_tool("mcp__demo__echo")])
            .expect("runtime tool")
            .with_disabled_tools(disabled);

        assert!(registry.is_tool_disabled("web_search"));
        assert!(registry.is_tool_disabled("mcp__demo__echo"));
        assert!(registry.is_tool_disabled("plugin_demo"));
        assert!(!registry.has_runtime_tool("mcp__demo__echo"));
        assert!(registry.plugin_tool_names().is_empty());

        let advertised = registry
            .definitions(None)
            .into_iter()
            .map(|definition| definition.name)
            .collect::<BTreeSet<_>>();
        assert!(!advertised.contains("WebSearch"));
        assert!(!advertised.contains("mcp__demo__echo"));
        assert!(!advertised.contains("plugin_demo"));

        let permissions = registry
            .permission_specs(None)
            .expect("permission specs")
            .into_iter()
            .map(|(name, _)| name)
            .collect::<BTreeSet<_>>();
        assert!(!permissions.contains("WebSearch"));
        assert!(!permissions.contains("mcp__demo__echo"));
        assert!(!permissions.contains("plugin_demo"));

        let search = registry.search(
            "select:WebSearch,mcp__demo__echo,plugin_demo",
            5,
            None,
            None,
        );
        assert!(search.matches.is_empty());

        let allowed = registry
            .normalize_allowed_tools(&["read_file".to_string()])
            .expect("enabled tool remains allow-listable")
            .expect("allow list");
        assert_eq!(allowed, BTreeSet::from(["read_file".to_string()]));
        let disabled_allowed = registry.normalize_allowed_tools(&["WebSearch".to_string()]);
        assert!(
            disabled_allowed.is_err(),
            "disabled names are not allow-listable"
        );

        for name in ["WebSearch", "mcp__demo__echo", "plugin_demo"] {
            let error = registry
                .execute(name, &json!({}))
                .expect_err("disabled tool should be rejected");
            assert!(
                matches!(error, super::ToolError::PermissionDenied { .. }),
                "{name} should return PermissionDenied, got {error:?}"
            );
        }
    }

    #[test]
    fn toggleable_tools_keep_disabled_tools_visible_with_state() {
        let registry = GlobalToolRegistry::with_plugin_tools(vec![plugin_tool("plugin_demo")])
            .expect("plugin registry")
            .with_runtime_tools(vec![runtime_tool("mcp__demo__echo")])
            .expect("runtime tool")
            .with_disabled_tools(BTreeSet::from([
                "WebSearch".to_string(),
                "mcp__demo__echo".to_string(),
                "plugin_demo".to_string(),
            ]));

        let tools = registry.toggleable_tools();
        for name in ["WebSearch", "mcp__demo__echo", "plugin_demo"] {
            let tool = tools
                .iter()
                .find(|tool| tool.name == name)
                .unwrap_or_else(|| panic!("{name} should stay visible in /tools"));
            assert!(!tool.enabled, "{name} should be marked disabled");
        }
        assert!(
            tools
                .iter()
                .any(|tool| tool.name == "read_file" && tool.enabled),
            "enabled builtin tools remain visible and enabled"
        );
    }

    #[test]
    fn set_runtime_tools_propagates_across_clones() {
        let registry = GlobalToolRegistry::builtin();
        let clone = registry.clone();
        assert!(!clone.has_runtime_tool("mcp_demo"));
        // Refreshing the ORIGINAL must be visible on a CLONE — they share the
        // same Arc<Mutex>, which is exactly what the request builder relies on
        // for a mid-session tools/list_changed refresh (G20).
        registry
            .set_runtime_tools(vec![runtime_tool("mcp_demo")])
            .expect("valid runtime tool");
        assert!(
            clone.has_runtime_tool("mcp_demo"),
            "a mid-session refresh propagates to every clone"
        );
        // MCP schemas are deferred: the refresh must NOT splice the schema
        // into the wire advertisement (that splice used to invalidate the
        // whole prefix cache mid-session).
        assert!(
            !clone
                .definitions(None)
                .iter()
                .any(|d| d.name == "mcp_demo"),
            "a refreshed MCP tool is reachable, not advertised"
        );
        // Searching on the CLONE finds it for the ORIGINAL too — the tool list
        // is one shared Arc — and neither wire moves.
        let output = clone.search("select:mcp_demo", 3, None, None);
        assert_eq!(output.matches, vec!["mcp_demo".to_string()]);
        assert!(
            !registry
                .definitions(None)
                .iter()
                .any(|d| d.name == "mcp_demo"),
            "a server connecting mid-session must not rewrite the cached tool prefix"
        );
    }

    #[test]
    fn execute_tool_search_respects_permission_enforcer() {
        let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
            .with_tool_requirement("ToolSearch", PermissionMode::WorkspaceWrite);
        let registry = GlobalToolRegistry::builtin().with_enforcer(PermissionEnforcer::new(policy));

        let error = registry
            .execute("ToolSearch", &json!({ "query": "read", "max_results": 5 }))
            .expect_err("ToolSearch should be denied by the active permission policy");
        assert!(error.to_string().contains("permission denied"));
        let invocations = registry.context().tool_invocations();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].request.tool_name, "ToolSearch");
        assert!(matches!(
            invocations[0].policy_decision,
            ToolPolicyDecision::Denied { .. }
        ));
    }

    #[test]
    fn execute_tool_search_uses_live_registry_for_runtime_and_plugin_tools() {
        let registry = GlobalToolRegistry::with_plugin_tools(vec![plugin_tool("plugin_demo")])
            .expect("plugin registry");
        registry
            .set_runtime_tools(vec![runtime_tool("mcp__demo__echo")])
            .expect("valid runtime tool");

        let output = registry
            .execute(
                "ToolSearch",
                &json!({ "query": "select:mcp__demo__echo,plugin_demo", "max_results": 5 }),
            )
            .expect("ToolSearch should execute against live registry");
        let value: serde_json::Value = serde_json::from_str(&output).expect("valid json");
        assert_eq!(value["matches"][0], "mcp__demo__echo");
        assert_eq!(value["matches"][1], "plugin_demo");
        assert!(
            value["schemas"]
                .as_array()
                .expect("schemas")
                .iter()
                .any(|schema| schema["name"] == "mcp__demo__echo"),
            "runtime MCP schema should be returned"
        );
        assert!(
            value["schemas"]
                .as_array()
                .expect("schemas")
                .iter()
                .any(|schema| schema["name"] == "plugin_demo"),
            "plugin schema should be returned"
        );
        let wire: Vec<String> = registry
            .definitions(None)
            .into_iter()
            .map(|definition| definition.name)
            .collect();
        assert!(
            !wire.contains(&"mcp__demo__echo".to_string())
                && !wire.contains(&"plugin_demo".to_string()),
            "the search returned both schemas as data and left the wire alone: {wire:?}"
        );
    }

    /// Deferral applies to every provider, not just the ones that defer
    /// builtins — the wire list is one shape now, so an OpenAI target and an
    /// Anthropic target advertise the same set. (OpenAI once front-loaded
    /// every discovered MCP and plugin schema to keep the prefix stable
    /// against activation: ~55 KB on every request, including turns that call
    /// no tool. `CapabilityInvoke` buys that same stability for the cost of
    /// one schema.)
    #[test]
    fn mcp_and_plugin_schemas_stay_off_the_wire_unless_explicitly_allowed() {
        let registry = GlobalToolRegistry::with_plugin_tools(vec![plugin_tool("plugin_demo")])
            .expect("plugin registry");
        registry
            .set_runtime_tools(vec![runtime_tool("mcp__demo__echo")])
            .expect("valid runtime tool");

        // Neither schema rides the default wire advertisement…
        let wire: Vec<String> = registry
            .definitions(None)
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(!wire.contains(&"mcp__demo__echo".to_string()));
        assert!(!wire.contains(&"plugin_demo".to_string()));

        // …but an explicit allow-list is honored verbatim, no activation needed
        // (the sub-agent MCP passthrough depends on this path).
        let allowed: BTreeSet<String> = ["mcp__demo__echo".to_string(), "plugin_demo".to_string()]
            .into_iter()
            .collect();
        let allowed_wire: BTreeSet<String> = registry
            .definitions(Some(&allowed))
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(allowed_wire.contains("mcp__demo__echo"));
        assert!(allowed_wire.contains("plugin_demo"));

        // A search finds both and returns both schemas — and neither joins the
        // advertisement. A machine with several MCP servers connected is where
        // this matters most: activation would have put dozens of schemas at
        // the head of the cached prefix, one re-billing at a time.
        let output = registry.search("select:mcp__demo__echo,plugin_demo", 5, None, None);
        assert_eq!(
            output.matches,
            vec!["mcp__demo__echo".to_string(), "plugin_demo".to_string()]
        );
        assert_eq!(output.schemas.len(), 2, "both schemas come back as data");
        let after_search: BTreeSet<String> = registry
            .definitions(None)
            .into_iter()
            .map(|definition| definition.name)
            .collect();
        assert!(!after_search.contains("mcp__demo__echo"));
        assert!(!after_search.contains("plugin_demo"));
    }

    /// A search hands back the schema and leaves the wire alone.
    ///
    /// The schema is the whole point of searching — a bare name would leave
    /// the model guessing the input shape — and it now travels as tool-result
    /// DATA rather than as a new entry in the advertisement.
    #[test]
    fn a_found_deferred_builtin_returns_its_schema_without_joining_the_wire() {
        let registry = GlobalToolRegistry::builtin();
        let advertised = |registry: &GlobalToolRegistry| {
            registry
                .definitions(None)
                .iter()
                .any(|definition| definition.name == "Workflow")
        };
        assert!(!advertised(&registry), "precondition: Workflow is deferred");
        let output = registry.search("select:Workflow", 3, None, None);
        assert_eq!(output.matches, vec!["Workflow".to_string()]);
        let schema = output
            .schemas
            .iter()
            .find(|definition| definition.name == "Workflow")
            .expect("the schema is what the model came for");
        assert!(schema.input_schema.get("properties").is_some());
        assert!(
            !advertised(&registry),
            "finding a tool must not advertise it — that is the re-billing this design removed"
        );
    }

    #[test]
    fn set_runtime_tools_preserves_disabled_runtime_filter_across_refreshes() {
        let disabled_name = "mcp__demo__fresh";
        let registry = GlobalToolRegistry::builtin()
            .with_disabled_tools(BTreeSet::from([disabled_name.to_string()]));
        let clone = registry.clone();

        registry
            .set_runtime_tools(vec![
                runtime_tool(disabled_name),
                runtime_tool("mcp__demo__visible"),
            ])
            .expect("valid refreshed runtime tools");

        assert!(
            registry
                .runtime_tool_definitions()
                .iter()
                .any(|tool| tool.name == disabled_name),
            "the raw refreshed tool remains registered so /tools can show it"
        );
        assert!(
            !clone.has_runtime_tool(disabled_name),
            "disabled refreshed tools stay hidden on registry clones"
        );
        assert!(
            clone
                .definitions(None)
                .iter()
                .all(|tool| tool.name != disabled_name),
            "disabled refreshed tools are not advertised to the model"
        );
        assert!(
            clone
                .permission_specs(None)
                .expect("permission specs")
                .iter()
                .all(|(name, _)| name != disabled_name),
            "disabled refreshed tools do not receive permission specs"
        );
        assert!(
            clone
                .normalize_allowed_tools(&[disabled_name.to_string()])
                .is_err(),
            "disabled refreshed tools are not allow-listable"
        );
        assert!(
            clone
                .search(disabled_name, 5, None, None)
                .matches
                .is_empty(),
            "disabled refreshed tools are not discoverable through search"
        );

        let toggleable = clone
            .toggleable_tools()
            .into_iter()
            .find(|tool| tool.name == disabled_name)
            .expect("disabled refreshed tool remains visible in /tools");
        assert!(!toggleable.enabled);
    }

    #[test]
    fn set_runtime_tools_rejects_duplicate_names() {
        let registry = GlobalToolRegistry::builtin();
        let err = registry.set_runtime_tools(vec![runtime_tool("dup"), runtime_tool("dup")]);
        assert!(err.is_err(), "duplicate runtime tool names are rejected");
    }

    #[test]
    fn runtime_tool_definitions_round_trips_the_current_set() {
        let registry = GlobalToolRegistry::builtin();
        assert!(registry.runtime_tool_definitions().is_empty());
        registry
            .set_runtime_tools(vec![runtime_tool("mcp_a"), runtime_tool("mcp_b")])
            .expect("valid runtime tools");
        let names = registry
            .runtime_tool_definitions()
            .into_iter()
            .map(|def| def.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["mcp_a".to_string(), "mcp_b".to_string()]);
    }

    fn wire_names(registry: &GlobalToolRegistry) -> BTreeSet<String> {
        registry
            .definitions(None)
            .into_iter()
            .map(|def| def.name)
            .collect()
    }

    /// Selecting Plan no longer moves the wire at all.
    ///
    /// The write-gated toggle pair used to be advertised and then dropped when
    /// the user selected Plan — a mid-session change to the tool block at the
    /// head of the cached prefix, i.e. a re-billing of the whole conversation
    /// for a keystroke. Both are deferred now (they are worktree SETTINGS, and
    /// the user enters Plan with Shift+Tab). Plan submission, `ExitPlanModeV2`,
    /// is deferred too since t-2903 (its seat went to `Agent`; the arithmetic
    /// is at its entry in `DEFERRED_TOOL_NAMES_EXTRA`) — what a turn in Plan
    /// needs is that it stays REACHABLE, by name and through
    /// `CapabilityInvoke`, in both states, and that the wire is the same
    /// bytes in both.
    #[test]
    fn selecting_plan_does_not_move_the_wire() {
        let registry = GlobalToolRegistry::builtin();
        let reachable = |registry: &GlobalToolRegistry| {
            let found = registry.search("select:ExitPlanModeV2", 1, None, None);
            found
                .schemas
                .iter()
                .any(|schema| schema.name == "ExitPlanModeV2")
                && !matches!(
                    registry.execute("ExitPlanModeV2", &json!({})),
                    Err(crate::ToolError::NotFound(_))
                )
        };

        let normal = wire_names(&registry);
        assert!(!normal.contains("EnterPlanMode"));
        assert!(!normal.contains("ExitPlanMode"));
        assert!(
            !normal.contains("ExitPlanModeV2"),
            "plan submission is deferred; its wire seat belongs to `Agent`"
        );
        assert!(reachable(&registry), "plan submission reachable normally");

        registry.context().set_plan_selected(true);
        let planned = wire_names(&registry);
        assert_eq!(
            planned, normal,
            "entering Plan must not rewrite the advertised tool block"
        );
        assert!(
            reachable(&registry),
            "ExitPlanModeV2 must remain reachable under Plan"
        );

        registry.context().set_plan_selected(false);
        assert_eq!(
            wire_names(&registry),
            normal,
            "exiting Plan restores the normal wire surface"
        );
    }

    #[test]
    fn stale_enter_plan_mode_is_idempotent_success_under_selected_plan() {
        let registry = GlobalToolRegistry::builtin();
        registry.context().set_plan_selected(true);

        // A stale/in-flight EnterPlanMode call still arrives even though it is
        // off the wire. It must resolve as an idempotent already-active success,
        // never the deterministic WorkspaceWrite denial, and must not write.
        let output = registry
            .execute("EnterPlanMode", &json!({}))
            .expect("stale EnterPlanMode is a no-op success under selected Plan");
        assert_eq!(output, super::PLAN_ALREADY_ACTIVE_MESSAGE);
    }

    #[test]
    fn stale_legacy_exit_plan_mode_is_denied_as_user_controlled_under_plan() {
        let registry = GlobalToolRegistry::builtin();
        registry.context().set_plan_selected(true);

        // The model must not be able to restore write access by calling the
        // legacy exit tool; it fails with a clear user-controlled-mode message.
        let error = registry
            .execute("ExitPlanMode", &json!({}))
            .expect_err("legacy ExitPlanMode is denied under user-selected Plan");
        match error {
            crate::ToolError::PermissionDenied { tool, reason } => {
                assert_eq!(tool, "ExitPlanMode");
                assert_eq!(reason, super::PLAN_EXIT_USER_CONTROLLED_MESSAGE);
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[test]
    fn tool_search_cannot_re_advertise_plan_reentry_tools_under_selected_plan() {
        let registry = GlobalToolRegistry::builtin();

        // ToolSearch surfaces builtins via `searchable_tool_specs`, which is
        // built from `builtin_tool_specs`. Confirm the plan filter carries
        // through so an exact-name search cannot re-advertise the write-gated
        // toggle tools under Plan and reopen the WorkspaceWrite path. This is a
        // pure spec inspection — it never executes a tool against the repo.
        let searchable_names = |reg: &GlobalToolRegistry| -> BTreeSet<String> {
            reg.searchable_tool_specs()
                .into_iter()
                .map(|spec| spec.name)
                .collect()
        };

        let normal = searchable_names(&registry);
        assert!(normal.contains("EnterPlanMode"));
        assert!(normal.contains("ExitPlanMode"));

        registry.context().set_plan_selected(true);
        let planned = searchable_names(&registry);
        assert!(
            !planned.contains("EnterPlanMode"),
            "ToolSearch must not surface EnterPlanMode under user-selected Plan"
        );
        assert!(
            !planned.contains("ExitPlanMode"),
            "ToolSearch must not surface legacy ExitPlanMode under user-selected Plan"
        );
        assert!(
            planned.contains("ExitPlanModeV2"),
            "ExitPlanModeV2 stays searchable under Plan"
        );

        registry.context().set_plan_selected(false);
        assert_eq!(
            searchable_names(&registry),
            normal,
            "exiting Plan restores the searchable surface exactly"
        );
    }
}
