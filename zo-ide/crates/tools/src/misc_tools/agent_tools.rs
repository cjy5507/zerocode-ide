#[path = "agent_tools/runtime.rs"]
mod agent_runtime;
mod auto_verify;
mod completion;
mod custom;
mod fork;
mod labels;
mod manifest;
mod panes;
mod provider_client;
mod rate_limit;
pub mod registry;
mod resume;
mod roster;
mod spawn;
mod steer;
mod subagent_profile;
mod topology;
mod usage;

pub(crate) use self::usage::{agent_usage_cost_for_attempt, AgentUsageCost};

pub(crate) use self::topology::{prompt_with_seen_context, resolve_seen_outputs};
pub(crate) use self::fork::is_fork_type;

use core_types::helper_run::HelperRun;
use self::agent_runtime::allowed_tools_for_resolved;
#[cfg(test)]
use self::agent_runtime::{inherited_lsp, subagent_hook_context};
use self::completion::{notify_agent_completion, reset_agent_completion};
use self::custom::{load_custom_agent, CustomAgent};
pub use self::custom::{loaded_custom_agents, LoadedCustomAgent};
pub use self::labels::agent_store_dir;
pub use self::steer::{live_pane_children, PaneChildChannel};
pub(crate) use self::steer::steer_agent_with_receipt;
pub use self::registry::{
    locator_for as registry_locator_for, mark_session_process, store_root_for, AgentRegistry,
    RegistryRecord,
};
pub(crate) use self::labels::display_agent_label;
pub(crate) use self::roster::{session_agent_rows, AgentRosterRow};
use self::labels::{make_agent_id, slugify_agent_name};
use self::manifest::{
    load_agent_manifest_from_scanned_path, persist_agent_stopped_state_with, read_agent_output,
    run_if_agent_manifest_running, write_agent_manifest,
};

/// The on-disk manifest for one agent id, or `None` when the store has no
/// such agent (or the manifest is unreadable). The kernel bridge's `zo.result`
/// read — callers must validate the id's alphabet before it becomes a path.
pub(crate) fn agent_manifest_by_id(registry: &AgentRegistry, agent_id: &str) -> Option<AgentOutput> {
    registry.manifest_by_id(agent_id)
}

/// What an agent's manifest says about its progress right now, for schedulers
/// deciding whether a deadline should cancel it.
pub(crate) struct AgentProgressSnapshot {
    /// Seconds since the manifest heartbeat (`lastActivityAt`), or `None` when
    /// it was never stamped. Streaming refreshes it continuously — reasoning
    /// deltas every few seconds, output-tail flushes sub-second — because
    /// every `record_agent_*` path goes through the manifest read-modify-write
    /// that stamps it.
    pub seconds_since_activity: Option<u64>,
    /// Whether the agent is currently blocked inside a tool call. Tool
    /// execution stamps on entry and exit but not while it runs, so a long
    /// build or test would otherwise look idle for its whole duration; the
    /// field is the manifest's own answer to "is it in one right now".
    pub inside_tool_call: bool,
    /// Whether a provider request is in flight right now. A turn that streams
    /// no frame — silent thinking, a non-streaming call — refreshes nothing,
    /// so the heartbeat goes stale while the agent is perfectly healthy; the
    /// manifest's request bracket is what says the call itself is still open.
    pub awaiting_provider: bool,
}

/// Read an agent's progress snapshot in a single manifest load, or `None` when
/// no manifest exists for the id.
pub(crate) fn agent_progress_snapshot(
    registry: &AgentRegistry,
    agent_id: &str,
) -> Option<AgentProgressSnapshot> {
    let manifest = agent_manifest_by_id(registry, agent_id)?;
    let now = manifest::epoch_seconds_now_u64();
    Some(AgentProgressSnapshot {
        seconds_since_activity: manifest
            .last_activity_at
            .map(|stamped_at| now.saturating_sub(stamped_at)),
        inside_tool_call: manifest.current_tool.is_some(),
        awaiting_provider: manifest.awaiting_provider_since.is_some(),
    })
}
#[cfg(test)]
use self::manifest::persist_agent_stopped_state;
#[cfg(test)]
use self::manifest::{persist_agent_terminal_state_with_history, record_current_tool};
#[cfg(test)]
use self::rate_limit::{parse_agent_api_concurrency_limit, MAX_AGENT_MAX_CONCURRENCY};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use api::resolve_model_alias;
use super::{epoch_seconds_now, mvp_tool_specs, ToolError};
use runtime::{
    lsp_client::LspRegistry, ContentBlock, LaneEvent, LaneEventBlocker, PermissionMode,
    PermissionPolicy, RuntimeHookConfig,
};

pub(crate) use self::agent_runtime::SubagentToolExecutor;
pub use self::completion::{
    agent_message_source_id, background_agent_ids_snapshot,
    background_completion_matches_session,
    clear_background_agent, is_background_agent, mark_background_agent,
    background_task_completion, notify_background_task_completion,
    register_agent_completion_channel, wait_for_agent_completions,
    wait_for_agent_completions_cancellable,
    wait_for_agent_completions_until_done, AgentCompletion, AGENT_MESSAGE_STATUS,
    AGENT_STARVED_STATUS,
    provider_error_class_from_completion, provider_error_class_metadata,
};
pub(crate) use self::completion::{BackgroundTaskSession, background_task_session_id, wait_for_next_agent_completions};
#[cfg(test)]
pub(crate) use self::completion::{
    clear_agent_completion_channel_for_tests, inject_completion_for_tests,
    publish_agent_completion_for_tests,
};
pub(crate) use self::manifest::{
    AgentActivitySnapshot, agent_activity_snapshot_by_id, classify_lane_failure,
    persist_agent_terminal_state,
};
pub(crate) use self::provider_client::{build_provider_client_for_agent, push_output_block};
pub(crate) use self::rate_limit::{
    rate_limit_headroom_low, shared_agent_runtime, workflow_concurrency_limit,
};
use self::spawn::spawn_agent_job;
pub(crate) use self::spawn::AgentJob;
pub(crate) use self::subagent_profile::{resolve_subagent_type, implementation_task, supports_file_edits};
use self::subagent_profile::{
    build_agent_system_prompt, smart_routed_model_selection, AgentModelSelection,
};

// --- Input/Output structs ---

#[derive(Debug, Deserialize)]
pub(crate) struct AgentInput {
    pub description: String,
    pub prompt: String,
    pub subagent_type: Option<String>,
    pub name: Option<String>,
    /// The model-supplied `name` AFTER it passed
    /// [`validate_addressable_agent_name`] — kept verbatim (not slugified) so
    /// `SendMessage(to: "<that exact name>")` resolves. Hidden from tool input:
    /// only the model-facing dispatch boundary sets it, because internal
    /// spawners (workflow phases, fan-out plumbing) legitimately use names the
    /// CC identifier grammar rejects (`plan:f3`, Korean labels) and must keep
    /// their slugified behavior. `None` ⇒ legacy slugify path.
    #[serde(skip)]
    pub addressable_name: Option<String>,
    pub model: Option<String>,
    /// Explicit escape hatch for a cross-provider `model`: the family clamp
    /// (`try_resolve_agent_model_selection` step 2) exists to stop SILENT
    /// provider jumps, but when the USER explicitly asked for a specific
    /// model ("opus 에이전트로 실행해") the harness needs a legitimate,
    /// auditable path to honor it — without one, the model gets cornered
    /// into Config-override flailing (live push-session incident: an
    /// "opus-agent" silently running terra). The flag is visible in the
    /// transcript and the agent card, so the jump is loud by construction.
    #[serde(default)]
    pub allow_cross_provider: bool,
    /// Working directory for this agent's tools. `None` (the default) runs in
    /// the process cwd; the workflow engine sets it to a per-agent worktree so
    /// `isolation:"worktree"` is real isolation, not a shared process cwd.
    pub cwd: Option<std::path::PathBuf>,
    /// JSON schema this agent must answer in. When set, `StructuredOutput` is
    /// enabled and the agent is asked to emit its result via that tool call;
    /// the call's input is captured as the structured result (workflow 8c),
    /// replacing the brittle parse-JSON-from-prose path. `None` = free text.
    pub schema: Option<serde_json::Value>,
    /// Run this agent in the **background**: the `Agent` tool returns at spawn
    /// time with a `status: "running"` result instead of blocking until the
    /// sub-agent finishes, so the main model keeps working (and the user keeps
    /// chatting) while the agent runs. The real result is pushed back into the
    /// conversation when the agent completes (the host marks the agent id via
    /// [`mark_background_agent`]): mid-turn at the next tool-result boundary
    /// when a turn is live, as a fresh follow-up turn when the host is idle.
    /// The model must NOT poll the output file.
    ///
    /// `None` (field omitted) defers to the HOST default
    /// ([`crate::ToolContext::background_agent_default`]): background in the
    /// interactive main session — whose REPL re-injects completions — and
    /// blocking everywhere a detached result would be lost (sub-agents,
    /// headless, workflow/fan-out internals, which always set it explicitly).
    #[serde(default)]
    pub background: Option<bool>,
    /// Set by the workflow engine so its sub-agents use the higher
    /// workflow-specific concurrency cap (`ZO_WORKFLOW_MAX_CONCURRENCY`,
    /// default `min(16, cores-2)`) instead of the shared `SpawnMultiAgent`
    /// semaphore (default 1, tuned for rate-limit safety). Never set from
    /// external tool input — `#[serde(default)]` keeps it false there.
    #[serde(default)]
    pub workflow_member: bool,
    /// A fan-out lane — one member of a `SpawnMultiAgent` batch. Its answer
    /// is consumed with its siblings' in one summary and nothing more is
    /// asked of it, so a pane cut for it is released the moment the parent
    /// reads the answer (2026-09-07). A lone `Agent` is a teammate and keeps
    /// its pane idle for `idle_budget`. Never set from tool input — the
    /// fan-out sets it for every member.
    #[serde(skip)]
    pub one_shot: bool,
    /// The plan shape this spawn is part of, when the HOST laid it out: the
    /// prelude's read-only lanes, or the width of a fan-out the model asked
    /// for. Never set from tool input — like [`Self::one_shot`], only a
    /// host/fan-out caller that knows the layout sets it.
    ///
    /// `None` is not "unknown": a spawn nobody laid out IS one delegation, and
    /// the outcome recorder reads it as `PlanShape::Delegate`. The width lives
    /// only where the caller counted it, so no reader has to re-count lanes
    /// off the agent store.
    #[serde(skip)]
    pub plan_shape: Option<runtime::PlanShape>,
    /// Set when this spawn IS the routing tax rather than work: the fan-out
    /// decomposition call. Its outcome row is then filed as the tax
    /// (`RouteOutcomeRecord::route_tax`) instead of as a subagent route, so
    /// the cost of deciding is counted once, in the one place that counts it,
    /// and never teaches the router that `classifier` is a good route.
    #[serde(skip)]
    pub route_tax: Option<runtime::RouteTaxCall>,
    /// Per-call provider-request concurrency ceiling from the flat
    /// `SpawnMultiAgent` `concurrency` argument. Threaded to the adaptive rate
    /// governor so a tighter value actually caps real API concurrency (it used
    /// to bind only OS-thread spawn windowing). Hidden from per-agent tool input
    /// — the fan-out sets it for every member. `None` = governor ceiling only.
    #[serde(skip)]
    pub api_concurrency: Option<usize>,
    /// Parent's active permission mode at spawn time. Hidden from tool input;
    /// dispatch fills it (sub-agent/headless enforcer mode, else the
    /// foreground session's recorded mode) and the job build clamps the
    /// child's effective mode to it, so a read-only session can delegate
    /// without the child escalating to the historical `DangerFullAccess`
    /// default. `None` (untracked host) keeps the legacy behavior.
    #[serde(skip)]
    pub parent_permission_mode: Option<PermissionMode>,
    /// Internal foreground session id that owns this agent manifest. Hidden
    /// from tool input; dispatch fills it from [`ToolContext`] so HUD/detail
    /// views can filter workspace-global manifests by session.
    #[serde(skip)]
    pub parent_session_id: Option<String>,
    /// The session-owned agent registry this spawn writes into (t-2511).
    /// Hidden from tool input; dispatch fills it from [`ToolContext`], the
    /// workflow backend from its own handle, a nested spawn from the parent
    /// job's. `None` — a bare test harness — falls back to the cwd store
    /// ([`AgentRegistry::unowned_from_cwd`]).
    #[serde(skip)]
    pub registry: Option<std::sync::Arc<AgentRegistry>>,
    /// Internal `tool_use` id of the delegation call that spawned this agent.
    /// Hidden from tool input; dispatch fills it from the
    /// `__zo_tool_call_id` the runtime smuggles into Spawn-family execution
    /// input, and the manifest stamps it so the TUI attributes each agent to
    /// the right transcript batch on concurrent multi-delegation turns.
    #[serde(skip)]
    pub tool_call_id: Option<String>,
    /// Internal parent-session MCP passthrough (see
    /// [`crate::registry::McpPassthrough`]). Hidden from tool input; dispatch
    /// fills it from [`ToolContext`] so the sub-agent advertises and dispatches
    /// the session's MCP tools. `None` on headless/test paths.
    #[serde(skip)]
    pub mcp_passthrough: Option<crate::registry::McpPassthrough>,
    /// Internal wall-clock budget for host-spawned agents. Hidden from tool
    /// input; callers that do not set it keep the default sub-agent budget.
    #[serde(skip)]
    pub time_budget: Option<Duration>,
    /// Quality failures from earlier implementation attempts for this same
    /// workflow item. Internal-only: provider/rate-limit retries do not touch
    /// it, and untrusted tool input cannot manufacture an escalation.
    #[serde(skip)]
    pub prior_failures: u32,
    /// Why the Smart router picked this agent's model (role, selector, and
    /// score adjustments), plus the dispatch-inferred agent type when one was
    /// auto-selected. Stamped onto the manifest so the TUI can show the
    /// decision instead of leaving auto-routing opaque. Hidden from tool input.
    #[serde(skip)]
    pub route_reason: Option<String>,
    /// Resolved Smart-route model for this agent — a TRUSTED, config-driven
    /// decision the host fills from the `/smart` router (already gated to the
    /// connected inventory by `route_model`). When set it is honored VERBATIM by
    /// the spawn path, including a deliberate cross-provider route for a
    /// diversity role; unlike the on-wire `model` field it is NOT re-gated by
    /// provider family. Hidden from tool input (`#[serde(skip)]`); `None` =
    /// inherit the parent/session model (CC default).
    #[serde(skip)]
    pub route_model: Option<String>,
    /// Host-computed ranked fallback models for quota/rate-limit escape. Hidden
    /// from tool input; filled by Smart routing or internal callers, then copied
    /// into [`AgentJob`] so the provider client can switch models instead of
    /// parking indefinitely on an exhausted quota window.
    #[serde(skip)]
    pub route_fallback_models: Vec<String>,
    /// Recommended reasoning-effort tier for this agent's Smart route — a
    /// TRUSTED, config-driven decision the host fills from the `/smart`
    /// router's `(model × effort)` co-routing (`RouteDecision::recommended_effort`),
    /// smuggled alongside `route_model` (`__zo_route_effort`, scrubbed from
    /// untrusted input the same way). Hidden from tool input; `None` = no
    /// recommendation (the byte-identical default — the agent's effort is
    /// whatever the task-difficulty budget would already produce).
    #[serde(skip)]
    pub route_effort: Option<api::EffortLevel>,
    /// Smart-router role label (`RouteRole`, lowercased — e.g. `"coding"`),
    /// carried into the manifest and stamped on this spawn's route-outcome
    /// record (P3 v2 schema) so learning phases can bucket by role without
    /// re-classifying the prompt/description. Hidden from tool input; `None`
    /// when the model was explicit or routing produced no decision.
    #[serde(skip)]
    pub route_role: Option<String>,
    /// Task complexity (`RouteTaskComplexity`, lowercased) at decision time —
    /// same provenance/visibility as [`Self::route_role`].
    #[serde(skip)]
    pub route_complexity: Option<String>,
    /// Task risk (`RouteTaskRisk`, lowercased) at decision time — same
    /// provenance/visibility as [`Self::route_role`].
    #[serde(skip)]
    pub route_risk: Option<String>,
    /// How this route was decided (`"auto"` | `"pin"` | `"explicit"` |
    /// `"fallback"`), projected from `RouteDecision::source` — same
    /// provenance/visibility as [`Self::route_role`].
    #[serde(skip)]
    pub route_source: Option<String>,
    /// What the routing probe said its OWN confidence was (`"low"` |
    /// `"medium"` | `"high"`), or `None` when no probe arrived — same
    /// provenance/visibility as [`Self::route_role`]. Kept for every ARRIVED
    /// probe, including the `low` ones fusion discards, because
    /// [`Self::route_source`] already says whether a probe informed the route
    /// and cannot distinguish a declined probe from no probe.
    #[serde(skip)]
    pub route_probe_confidence: Option<String>,
    /// The ATTEMPT of the worker member this agent's route need judges (Phase
    /// 4 verdict channel — planner-bound reviewer→worker pairing): that
    /// worker's manifest as it stood when this verifier was BOUND to it, so
    /// the verdict names that attempt (`agentId` + `runGeneration`) and the
    /// model and route it ran on, even if the worker is resumed — a new
    /// generation, possibly on another model — before the verdict lands
    /// (t-3959). A TRUSTED, host-resolved value: the Smart router smuggles
    /// the worker's `name` (`__zo_route_judged_agent`, computed by
    /// `smart_router::apply::planner_bound_judged_agent_names` before any
    /// agent id exists), and `run_spawn_multi_agent_with_timeout_and_hooks`
    /// resolves that name to the worker spawned earlier in the SAME batch
    /// before building this input; the verify-by-default loop binds its
    /// verifier to the implementer attempt that just finished. Hidden from
    /// tool input; `None` when this agent has no recognized judged worker
    /// (routing off, not a well-bound 2-member reviewer/worker pair, or the
    /// name could not be resolved — the doctrine is ambiguous binding records
    /// nothing, never a best-effort guess).
    #[serde(skip)]
    pub judged_agent: Option<AgentOutput>,
    /// Set only by the verify-by-default loop on the VERIFIER it spawns: the
    /// implementer's original task and the attempt round, so a failing verdict
    /// can re-spawn the implementer with the finding. Hidden from tool input.
    #[serde(skip)]
    pub verify_loop: Option<auto_verify::VerifyLoop>,
    /// Information-topology edge (the Fugu access-list gap): names/ids of
    /// FINISHED agents whose deliverable this agent reads. The harness
    /// resolves each reference and injects the bounded deliverable
    /// (structured-first, text fallback — the workflow `{item}` payload
    /// contract) as a labeled shared-context section ahead of `prompt`, so
    /// the orchestrator never spends output tokens re-narrating one agent's
    /// result into another's prompt. MODEL-FACING by design — unlike
    /// [`Self::judged_agent`] (a host-resolved credit-attribution label), a
    /// sees edge only shares material the orchestrator could already paste
    /// by hand; declaring it is strictly less error-prone than re-typing it.
    /// Resolution is fail-loud: an unknown or still-running reference errors
    /// the spawn instead of silently injecting nothing.
    #[serde(default)]
    pub sees: Vec<String>,
    /// The conversation a `fork` inherits (t-2875). Hidden from tool input;
    /// dispatch fills it from [`crate::ToolContext::fork_source`] when the
    /// call names `subagent_type: "fork"`, and the spawn refuses a fork
    /// without it. `None` for every other spawn.
    #[serde(skip)]
    pub fork_source: Option<crate::ForkSource>,
}

/// Structured activity signals used by workflow watchdogs and post-run
/// diagnosis.  Keep these separate from the presentation-oriented manifest
/// fields (`currentTool`, `outputTail`, ...): a transport heartbeat is proof of
/// life, not proof that the delegated task is progressing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AgentActivityTelemetry {
    pub(crate) stream_open_at: Option<u64>,
    pub(crate) first_provider_event_at: Option<u64>,
    pub(crate) last_transport_at: Option<u64>,
    pub(crate) quiet_stream_since_at: Option<u64>,
    pub(crate) last_reasoning_at: Option<u64>,
    pub(crate) first_task_action_at: Option<u64>,
    pub(crate) last_task_progress_at: Option<u64>,
    pub(crate) effective_effort: Option<String>,
    pub(crate) thinking_budget_tokens: Option<u32>,
    pub(crate) retry_cause: Option<String>,
    pub(crate) quiet_notice_count: u32,
    pub(crate) reconnect_count: u32,
    pub(crate) loaded_skills: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) usage_cost: Option<AgentUsageCost>,
    pub(crate) fallback_models: Vec<String>,
}

impl AgentActivityTelemetry {
    fn is_empty(&self) -> bool {
        self.stream_open_at.is_none()
            && self.first_provider_event_at.is_none()
            && self.last_transport_at.is_none()
            && self.quiet_stream_since_at.is_none()
            && self.last_reasoning_at.is_none()
            && self.first_task_action_at.is_none()
            && self.last_task_progress_at.is_none()
            && self.effective_effort.is_none()
            && self.thinking_budget_tokens.is_none()
            && self.retry_cause.is_none()
            && self.quiet_notice_count == 0
            && self.reconnect_count == 0
            && self.loaded_skills.is_empty()
            && self.fallback_models.is_empty()
            && self.usage_cost.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AgentOutput {
    #[serde(rename = "agentId")]
    pub(crate) agent_id: String,
    #[serde(
        rename = "parentSessionId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) parent_session_id: Option<String>,
    /// `tool_use` id of the delegation call that spawned this agent, so the
    /// TUI's manifest scan can attribute the agent to the right transcript
    /// batch on concurrent multi-delegation turns. Absent on legacy manifests
    /// and host-spawned agents (which fall back to the collecting batch).
    #[serde(
        rename = "toolCallId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) tool_call_id: Option<String>,
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    pub(crate) description: String,
    #[serde(rename = "subagentType")]
    pub(crate) subagent_type: Option<String>,
    #[serde(
        rename = "requestedModel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) requested_model: Option<String>,
    #[serde(
        rename = "resolvedModel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) resolved_model: Option<String>,
    /// Why the Smart router picked the initial `model` (role/selector/score
    /// summary), or which agent type dispatch auto-selected when model routing
    /// was skipped. Runtime fallback transitions append their provenance here.
    #[serde(
        rename = "routeReason",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) route_reason: Option<String>,
    /// P3 v2 route-decision metadata (see [`AgentInput::route_role`] and
    /// siblings) — same absence conditions as `routeReason`. Read back by the
    /// spawn-completion route-outcome recorder so the persisted
    /// `RouteOutcomeRecord` carries `role`/`complexity`/`risk`/`routeSource`.
    /// A runtime provider switch replaces the original source with `fallback`.
    #[serde(rename = "routeRole", default, skip_serializing_if = "Option::is_none")]
    pub(crate) route_role: Option<String>,
    #[serde(
        rename = "routeComplexity",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) route_complexity: Option<String>,
    #[serde(rename = "routeRisk", default, skip_serializing_if = "Option::is_none")]
    pub(crate) route_risk: Option<String>,
    #[serde(
        rename = "routeSource",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) route_source: Option<String>,
    #[serde(
        rename = "routeProbeConfidence",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) route_probe_confidence: Option<String>,
    /// Names of the prior agents whose deliverables were injected into this
    /// agent's prompt as shared context (information-topology `sees` edges),
    /// in declaration order after dedupe. Durable so the topology is
    /// reconstructable from manifests alone — which agent read whose output
    /// — mirroring how `routeSource` makes the routing decision durable.
    /// Empty (and absent from the JSON) for the overwhelmingly common
    /// edge-less spawn, so legacy manifests round-trip byte-identically.
    #[serde(rename = "seesAgents", default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) sees: Vec<String>,
    pub(crate) model: Option<String>,
    pub(crate) status: String,
    #[serde(rename = "outputFile")]
    pub(crate) output_file: String,
    #[serde(rename = "manifestFile")]
    pub(crate) manifest_file: String,
    #[serde(rename = "createdAt")]
    pub(crate) created_at: String,
    /// The pane this agent's own zo is running in, when it was given one
    /// (`runtime::subagent_panes`). Absent for every inline agent, which is
    /// most of them: an agent that is a thread of this process has no pane of
    /// its own, and claiming one would point a reader at the PARENT's screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pane: Option<String>,
    /// PID of the zo process whose thread runs this agent. Agents are
    /// threads, not processes: when the owning process dies (crash, kill,
    /// `/restart`) a still-`running` manifest is a phantom that every store
    /// reader (HUD live rows, stop paths) would show as running forever —
    /// the boot-time orphan reap uses this stamp to settle them. Absent on
    /// legacy manifests, which fall back to a last-write staleness bound.
    #[serde(rename = "ownerPid", default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner_pid: Option<u32>,
    /// Monotonically increasing durable execution identity. Legacy manifests
    /// deserialize as generation zero; every resume atomically advances it.
    #[serde(rename = "runGeneration", default, skip_serializing_if = "is_zero_u64")]
    pub(crate) run_generation: u64,
    #[serde(rename = "startedAt", skip_serializing_if = "Option::is_none")]
    pub(crate) started_at: Option<String>,
    #[serde(rename = "completedAt", skip_serializing_if = "Option::is_none")]
    pub(crate) completed_at: Option<String>,
    /// Epoch-seconds stamp of the most recent completion publication for the
    /// CURRENT `run_generation` (cleared on resume). A terminal manifest
    /// without this stamp is the "died without delivering a result"
    /// signature that made today's silent-death incident undiagnosable
    /// post-mortem. Best-effort: publication never fails on a stamp error.
    #[serde(
        rename = "completionPublishedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) completion_published_at: Option<String>,
    #[serde(rename = "laneEvents", default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) lane_events: Vec<LaneEvent>,
    #[serde(rename = "currentBlocker", skip_serializing_if = "Option::is_none")]
    pub(crate) current_blocker: Option<LaneEventBlocker>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// Per-turn output-token counts captured from `AssistantEvent::Usage`.
    /// Persisted so the sidebar can render a sparkline of recent activity.
    /// Empty until Phase 4-data wires up the actual collection (this field
    /// is the persistence skeleton — readers gracefully fall back to no
    /// sparkline when empty).
    #[serde(
        rename = "tokenHistory",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub(crate) token_history: Vec<u32>,
    /// Tool the sub-agent is currently running, stamped on each tool call so
    /// the parent sidebar shows what the agent is doing live. Cleared on
    /// terminal states.
    #[serde(
        rename = "currentTool",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) current_tool: Option<String>,
    /// Epoch seconds at which the worker issued the provider request it is
    /// still blocked on, absent whenever no request is in flight. Every other
    /// progress signal is a *frame*: a leg that streams nothing at all — a
    /// silent thinking turn, a non-streaming fallback — produces none, so this
    /// is the only harness-recorded evidence that the API call itself is
    /// running. Cleared when the request returns and on terminal states.
    #[serde(
        rename = "awaitingProviderSince",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) awaiting_provider_since: Option<u64>,
    /// Rolling feed of the agent's most recent tool calls (oldest → newest,
    /// capped at [`manifest::RECENT_TOOLS_CAP`]) with a one-line argument
    /// brief, e.g. `read_file · src/main.rs`. The parent's agent viewer
    /// renders this as the live activity transcript while the agent runs —
    /// `currentTool` alone only says *now*, not *how it got here*.
    #[serde(rename = "recentTools", default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) recent_tools: Vec<String>,
    /// Number of tools the sub-agent has started. Kept separate from
    /// `laneEvents`, which are lifecycle/progress notes rather than tool calls.
    #[serde(rename = "toolCalls", default, skip_serializing_if = "is_zero_usize")]
    pub(crate) tool_calls: usize,
    /// Transient wait/stream phase the agent is in right now (e.g. `waiting
    /// for api slot`, `rate-limited · resumes in ~90s`, `thinking`). This is
    /// what makes an agent parked in the rate governor / cool-down visibly
    /// alive instead of a frozen `[running]`. Cleared on tool start and on
    /// terminal states.
    #[serde(
        rename = "currentPhase",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) current_phase: Option<String>,
    /// Rolling tail of the agent's latest streamed assistant text (capped at
    /// [`manifest::OUTPUT_TAIL_CAP`] chars), so the parent's agent viewer can
    /// show *what the agent is saying* live — not just which tool it runs.
    #[serde(
        rename = "outputTail",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub(crate) output_tail: String,
    /// Epoch seconds of the last observed liveness signal (transport notice,
    /// stream delta flush, tool state, phase change). This is presentation
    /// heartbeat only: use `activity` to distinguish transport life, decoded
    /// reasoning, and real task progress.
    #[serde(
        rename = "lastActivityAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) last_activity_at: Option<u64>,
    /// Machine-readable activity classes. `#[serde(default)]` keeps every
    /// pre-telemetry manifest readable; the nested object also avoids adding a
    /// dozen top-level fields to all existing Rust fixtures.
    #[serde(default, skip_serializing_if = "AgentActivityTelemetry::is_empty")]
    pub(crate) activity: AgentActivityTelemetry,
    /// Where and how this agent runs, and what it was last told (t-2513
    /// contract 5). Flattened onto the manifest so a reader sees plain keys
    /// (`execution`, `channel`, `transcript`, `harnessDigest`, `lastReceipt`)
    /// and a legacy manifest without them round-trips byte-identically.
    #[serde(flatten, default)]
    pub(crate) lifecycle: AgentLifecycle,
}

/// The identity facts a manifest carries about how its agent runs
/// (`docs/design/zo-teammate-lifecycle-contract.md` §2.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AgentLifecycle {
    /// `inline` (a thread of the spawning process) or `pane` (a zo of its
    /// own beside it). Absent on a manifest written before the stamp existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) execution: Option<String>,
    /// A pane child's channel file (`<store>/<id>/channel.addr`): where its
    /// events channel answers, so a `SendMessage` reaches it and `auth.reload`
    /// fans out to it. Absent for an inline agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) channel: Option<std::path::PathBuf>,
    /// The transcript this agent's turns live in. For an inline agent it is
    /// the sibling `<id>.session.jsonl` and needs no stamp; a pane child has
    /// a session file of its own, copied here from its `result.json` so a
    /// resume continues the SAME conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transcript: Option<std::path::PathBuf>,
    /// [`runtime::subagent_panes::ResolvedHarness::digest`] of the harness
    /// this agent was spawned with — one number for inline and pane alike.
    #[serde(rename = "harnessDigest", default, skip_serializing_if = "Option::is_none")]
    pub(crate) harness_digest: Option<String>,
    /// What the most recent `SendMessage` to this agent came to.
    #[serde(rename = "lastReceipt", default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_receipt: Option<runtime::subagent_panes::SteerReceiptRecord>,
    /// Why the parent released this idle pane child (`StopAgent`'s reason),
    /// once it did — the pane was asked to leave over its channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) released: Option<String>,
}

/// The two executors, as the manifest spells them.
pub(crate) const EXECUTION_INLINE: &str = "inline";
pub(crate) const EXECUTION_PANE: &str = "pane";

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

/// The model a spawn runs on when the person named one in their turn.
///
/// The person's own words outrank the assistant's `model` argument: "fable
/// 검토 받아" means Fable whatever the call asked for — a Gemini parent read
/// Fable as a persona and asked for the default Opus (2026-09-09). `None`
/// when the turn named no single model, so the ordinary precedence runs.
/// `corrected_from` is the assistant's request when it named something else,
/// so the door can say what it overrode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersonNamedModel {
    pub(crate) model: &'static str,
    pub(crate) corrected_from: Option<String>,
}

pub(crate) fn person_named_model(
    turn_pin: Option<&'static str>,
    requested: Option<&str>,
) -> Option<PersonNamedModel> {
    let model = turn_pin?;
    let same = |left: &str, right: &str| {
        super::canonicalize_route_model_id(left) == super::canonicalize_route_model_id(right)
    };
    let corrected_from = requested
        .map(str::trim)
        .filter(|asked| !asked.is_empty() && !same(asked, model))
        .map(str::to_string);
    Some(PersonNamedModel { model, corrected_from })
}

fn requested_agent_model(
    input_model: Option<&str>,
    custom_model: Option<&str>,
    parent_model: Option<&str>,
) -> Option<String> {
    input_model
        .or(custom_model)
        .or(parent_model)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn rate_limit_fallback_models(
    selected_model: &str,
    parent_model: Option<&str>,
    routed_candidates: &[String],
    implementation_route: bool,
    route_complexity: Option<&str>,
    prior_failures: u32,
    policy: runtime::SmartPolicy,
) -> Vec<String> {
    // ZO_AGENT_MODEL is an explicit user-level override for every agent; do
    // not silently escape it to another model under provider pressure.
    if std::env::var(AGENT_MODEL_ENV)
        .ok()
        .as_deref()
        .map(str::trim)
        .is_some_and(|model| !model.is_empty())
    {
        return Vec::new();
    }

    let mut fallbacks = Vec::new();
    let implementation_model_allowed = |model: &str| {
        !implementation_route
            || (implementation_model_refusal(model, parent_model, false).is_none() && runtime::implementation_route_model_allowed(
                model,
                route_complexity_for_gate(route_complexity),
                prior_failures,
                policy,
            ))
    };
    let parent_model = parent_model.map(str::trim).filter(|model| !model.is_empty());
    for candidate in routed_candidates {
        let candidate = candidate.trim();
        // The router's ranked alternates are the intentional continuation of
        // this route. If it also returned the session parent, defer that one
        // to the final rung instead of letting its inventory position restore
        // the flagship before the route-ranked alternates get a chance.
        if parent_model == Some(candidate) {
            continue;
        }
        if implementation_model_allowed(candidate) {
            push_rate_limit_fallback(&mut fallbacks, selected_model, Some(candidate));
        }
    }
    if parent_model.is_some_and(implementation_model_allowed) {
        push_rate_limit_fallback(&mut fallbacks, selected_model, parent_model);
    }
    fallbacks
}

/// One gate for primary implementation picks and their quota fallbacks.
fn implementation_model_refusal(model: &str, parent: Option<&str>, person_pin: bool) -> Option<String> {
    if person_pin { return None; }
    let identity = normalized_agent_model(model);
    let outside_family = parent.filter(|parent| !parent.trim().is_empty()).is_some_and(|parent|
        normalized_agent_model(parent) != identity
            && !subagent_profile::same_provider_family(Some(parent), &identity));
    let problem = if outside_family {
        "outside the parent's provider family"
    } else if runtime::is_reserved_orchestrator_model(&identity) {
        "reserved for orchestration"
    } else { return None; };
    Some(format!("Implementation model `{model}` is {problem}. Do this work inline. Only the person's launch or settings selection authorizes this exception; do not retry this spawn with a model pin."))
}

fn route_complexity_for_gate(value: Option<&str>) -> runtime::RouteTaskComplexity {
    if value.is_some_and(|value| value.eq_ignore_ascii_case("large")) {
        runtime::RouteTaskComplexity::Large
    } else {
        runtime::RouteTaskComplexity::Unknown
    }
}

fn push_rate_limit_fallback(
    fallbacks: &mut Vec<String>,
    selected_model: &str,
    candidate: Option<&str>,
) {
    let Some(candidate) = candidate.map(str::trim).filter(|model| !model.is_empty()) else {
        return;
    };
    if candidate == selected_model
        || suppress_cross_family_premium_fast_fallback(selected_model, candidate)
        || fallbacks.iter().any(|existing| existing == candidate)
    {
        return;
    }
    fallbacks.push(candidate.to_string());
}

/// Suppress a fallback from a first-party OpenAI model onto an older
/// generation's priority tier: the selected release outranks the candidate,
/// and its own catalog ladder (`demotes_to`) is the verified way down.
fn suppress_cross_family_premium_fast_fallback(selected_model: &str, candidate: &str) -> bool {
    let selected = normalized_agent_model(selected_model);
    let candidate = normalized_agent_model(candidate);
    api::is_openai_model(&selected)
        && api::openai_fast_tier_enabled(&candidate)
        && runtime::model_inventory::release_rank_for_model(&candidate)
            < runtime::model_inventory::release_rank_for_model(&selected)
}

fn normalized_agent_model(model: &str) -> String {
    let trimmed = model.trim();
    let model = match trimmed.split_once('/') {
        Some((provider, model)) if provider.eq_ignore_ascii_case("openai") => model.trim(),
        _ => trimmed,
    };
    resolve_model_alias(model)
}

// --- Constants ---

/// `ColdStartPrior` (see `model_router::policy::TiersProvenance`): the last-
/// resort process fallback when a spawn has NEITHER an explicit/env model NOR
/// a parent/session model to inherit — a genuinely non-live harness path
/// (`resolve_agent_model`/`try_resolve_agent_model_selection`'s final arm).
/// Deliberately NOT inventory-derived: deriving "the connected/main
/// provider's best model" needs a seed model to determine which provider's
/// inventory to inspect. Live spawns always inherit a parent; non-live harnesses
/// use the catalog-owned semantic Anthropic default rather than embedding a
/// release id here. Updating the provider catalog therefore updates this final
/// fallback without touching agent code.
const DEFAULT_AGENT_MODEL: &str = api::ANTHROPIC_LATEST_MODEL_ALIAS;
/// Env override applied to *every* sub-agent model before any provider routing.
/// Exposed crate-wide so the fan-out decomposer can base its structured-output
/// decision on the same effective model the spawn will actually run (BUG-R15).
pub(crate) const AGENT_MODEL_ENV: &str = "ZO_AGENT_MODEL";

/// The `run_generation` stamped on a *freshly spawned* agent manifest. A resume
/// increments from here, so fan-out members' initial worktrees are bound to this
/// generation and a same-id resume is detectably a newer generation (see
/// `agent_worker_generation_is_live`).
pub(crate) const AGENT_INITIAL_RUN_GENERATION: u64 = 1;

/// Cancel signals keyed by the *exact* `(agent_id, run_generation)`, so a same-id
/// resume (a new generation) coexists with an old generation still winding down
/// rather than overwriting it. Overwriting by id alone is an ABA hazard: a
/// deferred worktree cleanup owner bound to the old generation could see the id
/// "live" (new gen) or, worse, "not live" while the old physical worker is still
/// editing, and tear a live worktree down. Exact keys let `agent_worker_is_live`
/// answer "any generation live?" (HUD rescue) while
/// `agent_worker_generation_is_live` answers "is *this* worktree's worker still
/// live?" (teardown gating).
static AGENT_CANCEL_SIGNALS: OnceLock<Mutex<HashMap<(String, u64), runtime::HookAbortSignal>>> =
    OnceLock::new();

// --- Execution ---

pub(crate) fn resolve_subagent_type_and_custom_agent(
    explicit: Option<&str>,
    description: &str,
    prompt: &str,
) -> (String, Option<CustomAgent>) {
    let explicit_subagent_type = explicit.map(str::trim).filter(|token| !token.is_empty());
    let explicit_custom_agent = explicit_subagent_type.and_then(load_custom_agent);
    let resolved_subagent_type = if explicit_custom_agent.is_some() {
        explicit_subagent_type
            .expect("explicit custom agent requires a token")
            .to_string()
    } else {
        resolve_subagent_type(explicit, description, prompt)
    };
    let custom_agent = explicit_custom_agent.or_else(|| load_custom_agent(&resolved_subagent_type));
    (resolved_subagent_type, custom_agent)
}

pub(crate) fn custom_agent_route_context(explicit: Option<&str>) -> Option<String> {
    explicit
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .and_then(load_custom_agent)
        .map(|agent| {
            format!(
                "{} {} {}",
                agent.name, agent.description, agent.system_prompt
            )
        })
}

/// The mode one spawn actually runs in: the launch's mode, except for the
/// `classifier` micro-prompt (fan-out decomposition and triage) — a one-shot
/// `StructuredOutput` answer with no workspace tools and nothing a person
/// would watch, so a pane for it is a pane for nothing. Inline always. Pure,
/// so the rule is a test rather than an environment.
#[must_use]
pub(crate) fn spawn_mode_for(
    mode: runtime::subagent_panes::SubagentMode,
    subagent_type: Option<&str>,
) -> runtime::subagent_panes::SubagentMode {
    if subagent_type == Some("classifier") {
        return runtime::subagent_panes::SubagentMode::Inline;
    }
    mode
}

/// Start one sub-agent, in whichever world this launch runs them.
///
/// THE one place the two executors are chosen between. Both build the same
/// manifest, publish the same completion and hand the model the same tool
/// result; the only difference is whether the helper is a thread of this
/// process or a pane beside it (`runtime::subagent_panes`). Asked at every
/// spawn rather than cached, so a person who changes the setting mid-session
/// gets the answer they just wrote.
pub(crate) fn execute_agent_with_parent_model_and_hooks(
    input: AgentInput,
    parent_model: Option<&str>,
    parent_lsp: Option<&LspRegistry>,
    hook_config: Option<&RuntimeHookConfig>,
) -> Result<AgentOutput, ToolError> {
    match spawn_mode_for(runtime::subagent_panes::mode(), input.subagent_type.as_deref()) {
        runtime::subagent_panes::SubagentMode::Panes => {
            execute_agent_with_spawn_and_parent_model_and_hooks(
                input,
                panes::spawn_pane_job,
                parent_model,
                parent_lsp,
                hook_config,
            )
        }
        runtime::subagent_panes::SubagentMode::Inline => {
            execute_agent_with_spawn_and_parent_model_and_hooks(
                input,
                spawn_agent_job,
                parent_model,
                parent_lsp,
                hook_config,
            )
        }
    }
}

/// Spawn a sub-agent and **block until it finishes** (or the wait window
/// elapses), returning the manifest plus its terminal completion. This makes a
/// single `Agent` tool call synchronous — like `SpawnMultiAgent` and Claude
/// Code's `Task` — so the model receives the result inline instead of polling
/// the output file with `sleep`+`cat`. That polling multiplied foreground
/// provider requests and, sharing the sub-agent's account quota, tripped the
/// rate limit even though Claude Code runs the same work fine.
pub(crate) fn execute_agent_blocking(
    input: AgentInput,
    parent_model: Option<&str>,
    parent_lsp: Option<&LspRegistry>,
    hook_config: Option<&RuntimeHookConfig>,
) -> Result<(AgentOutput, Option<AgentCompletion>), ToolError> {
    let manifest =
        execute_agent_with_parent_model_and_hooks(input, parent_model, parent_lsp, hook_config)?;
    let agent_id = manifest.agent_id.clone();
    let completion = wait_for_agent_completions(
        std::slice::from_ref(&agent_id),
        super::SPAWN_MULTI_AGENT_WAIT_TIMEOUT,
    )
    .into_iter()
    .find(|completion| completion.agent_id == agent_id);
    Ok((manifest, completion))
}

// Test seam: lets unit tests inject a fake `spawn_fn` instead of launching
// a real sub-agent process (production goes through the
// `..._with_parent_model_and_hooks` chain).
#[allow(dead_code)]
pub(crate) fn execute_agent_with_spawn<F>(
    input: AgentInput,
    spawn_fn: F,
) -> Result<AgentOutput, ToolError>
where
    F: FnOnce(AgentJob) -> Result<(), ToolError> + Send,
{
    execute_agent_with_spawn_and_parent_model(input, spawn_fn, None, None)
}

pub(crate) fn execute_agent_with_spawn_and_parent_model<F>(
    input: AgentInput,
    spawn_fn: F,
    parent_model: Option<&str>,
    parent_lsp: Option<&LspRegistry>,
) -> Result<AgentOutput, ToolError>
where
    F: FnOnce(AgentJob) -> Result<(), ToolError> + Send,
{
    execute_agent_with_spawn_and_parent_model_and_hooks(
        input,
        spawn_fn,
        parent_model,
        parent_lsp,
        None,
    )
}

/// Longest model-supplied addressable agent name (CC parity).
const MAX_ADDRESSABLE_AGENT_NAME_CHARS: usize = 64;

/// The one agent name `SendMessage` reserves: it addresses the main
/// conversation, so no spawned worker may claim it.
pub(crate) const RESERVED_AGENT_NAME: &str = "main";

/// CC's `Agent.name` regex message, verbatim in shape — the model reads this
/// back as the tool error, so it must state the whole grammar in one line.
const ADDRESSABLE_AGENT_NAME_RULE: &str =
    "name must start with a letter or digit and contain only letters, digits, underscores, or \
     hyphens (max 64 chars)";

/// Validate a model-supplied `name` for [`AgentInput::addressable_name`] with
/// CC's identifier semantics: `[A-Za-z0-9][A-Za-z0-9_-]{0,63}`, and `main`
/// reserved for the main conversation. Returns the trimmed name verbatim —
/// callers must NOT slugify it, or `SendMessage(to: "<name>")` stops resolving.
pub(crate) fn validate_addressable_agent_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ADDRESSABLE_AGENT_NAME_RULE.to_string());
    }
    if name.eq_ignore_ascii_case(RESERVED_AGENT_NAME) {
        return Err(format!(
            "`{RESERVED_AGENT_NAME}` is a reserved name — SendMessage routes it to the main \
             conversation; pick another name"
        ));
    }
    if name.chars().count() > MAX_ADDRESSABLE_AGENT_NAME_CHARS
        || !name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(ADDRESSABLE_AGENT_NAME_RULE.to_string());
    }
    Ok(name.to_string())
}

/// Ensure the agent-manifest store directory exists and is writable, turning a
/// bare OS `Permission denied (os error 13)` into an actionable error.
///
/// The store now defaults to the user-global zo home (see
/// [`agent_store_dir`]), which is normally always writable — so this almost
/// never fails. But if even that home is unwritable (a locked-down `$HOME`, a
/// `ZO_STATE_DIR`/`ZO_AGENT_STORE` pointed at a read-only path, or a
/// `root`-owned store from a prior `sudo zo`), `create_dir_all` returns
/// EACCES. Spawning needs the manifest on disk for the HUD/readers, so we cannot
/// degrade to in-memory like the todo store; instead we surface a clear,
/// non-bare message naming the override that fixes it, rather than the opaque
/// `io error: Permission denied (os error 13)` the user reported.
fn ensure_agent_store_writable(output_dir: &std::path::Path) -> Result<(), ToolError> {
    match std::fs::create_dir_all(output_dir) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            Err(ToolError::Execution(format!(
                "agent store `{}` is not writable ({error}); set ZO_AGENT_STORE \
                 (or ZO_STATE_DIR) to a writable directory",
                output_dir.display(),
            )))
        }
        Err(error) => Err(error.into()),
    }
}

#[allow(clippy::too_many_lines)] // single spawn→wait→report pipeline kept in execution order
pub(crate) fn execute_agent_with_spawn_and_parent_model_and_hooks<F>(
    mut input: AgentInput,
    spawn_fn: F,
    parent_model: Option<&str>,
    parent_lsp: Option<&LspRegistry>,
    hook_config: Option<&RuntimeHookConfig>,
) -> Result<AgentOutput, ToolError>
where
    F: FnOnce(AgentJob) -> Result<(), ToolError> + Send,
{
    if input.description.trim().is_empty() {
        return Err(ToolError::InvalidInput(
            "description must not be empty".into(),
        ));
    }
    if input.prompt.trim().is_empty() {
        return Err(ToolError::InvalidInput("prompt must not be empty".into()));
    }

    let agent_id = make_agent_id();
    // The store is the SESSION's, fixed when it opened — never re-derived from
    // the process cwd here, which is what let an `EnterWorktree` mid-turn
    // strand every child of the turn in a store nobody read (t-2511).
    let registry = input
        .registry
        .clone()
        .unwrap_or_else(AgentRegistry::unowned_from_cwd);
    if let Some(cwd) = input.cwd.as_deref() {
        registry.note_execution_cwd(cwd);
    }
    let output_dir = registry.root().to_path_buf();
    ensure_agent_store_writable(&output_dir)?;
    let output_file = output_dir.join(format!("{agent_id}.md"));
    let manifest_file = output_dir.join(format!("{agent_id}.json"));
    // Resolve the harness: an explicit type is normalized; an absent/empty
    // type is inferred from the task so the model gets the best-fit harness
    // automatically. A non-built-in type may resolve to a file-based custom
    // agent definition (`.zo/agents/<name>.md`).
    // A fork (t-2875) is not a harness of its own: it inherits the parent
    // conversation the session host published, and no `.zo/agents/fork.md`
    // can shadow the word. Where nobody published one — a fan-out member, a
    // workflow phase, a bare harness — the word is refused rather than run
    // fresh under a name that promises inheritance.
    let fork_source = match (is_fork_type(input.subagent_type.as_deref()), input.fork_source.take()) {
        (true, Some(source)) => Some(source),
        (true, None) => {
            return Err(ToolError::InvalidInput(fork::NO_PARENT_TO_FORK.to_string()));
        }
        (false, _) => None,
    };
    let (resolved_subagent_type, custom_agent) = if fork_source.is_some() {
        // CC's fork contract: the parent's model, whatever the call or the
        // router named. Cleared here so the one model resolution below
        // inherits the parent the way an unnamed spawn does.
        input.model = None;
        input.allow_cross_provider = false;
        input.route_model = None;
        (fork::FORK_SUBAGENT_TYPE.to_string(), None)
    } else {
        resolve_subagent_type_and_custom_agent(
            input.subagent_type.as_deref(),
            &input.description,
            &input.prompt,
        )
    };
    let (smart_policy, person_deep_models) = super::smart_router::live_agent_model_policy();
    let same_model = |left: &str, right: &str| super::canonicalize_route_model_id(left) == super::canonicalize_route_model_id(right);
    let person_selected = |model: &str| person_deep_models.iter().any(|pin| same_model(pin, model));
    let route_is_person_pin = input
        .route_source
        .as_deref()
        .is_some_and(super::smart_router::route_source_is_a_persons);
    // A trusted Smart-route model (host, config-driven, already gated to the
    // connected inventory) is honored verbatim unless ZO_AGENT_MODEL is set:
    // that env var is the user's explicit all-agents override and still wins.
    // Otherwise the route is NOT re-gated by provider family the way an
    // untrusted on-wire `model` field is.
    let selection = match input
        .route_model
        .as_deref()
        .or_else(|| input.model.as_deref().filter(|model| route_is_person_pin || person_selected(model)))
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        Some(routed) => smart_routed_model_selection(
            routed,
            &resolved_subagent_type,
            &input.description,
            &input.prompt,
        ),
        None => subagent_profile::try_resolve_agent_model_selection(
            input.model.as_deref(),
            input.allow_cross_provider,
            custom_agent
                .as_ref()
                .and_then(|agent| agent.model.as_deref()),
            &resolved_subagent_type,
            parent_model,
            &input.description,
            &input.prompt,
        )
        .map_err(|error| ToolError::InvalidInput(error.to_string()))?,
    };
    // Model-authored explicit ids are normalized by `try_resolve` above.
    // Trusted Smart routes and host-computed inventory fallbacks stay exact;
    // they have already crossed the connected-inventory policy boundary.
    let AgentModelSelection {
        model,
        thinking_budget_tokens,
    } = selection;
    let requested_model = requested_agent_model(
        input.model.as_deref(),
        custom_agent
            .as_ref()
            .and_then(|agent| agent.model.as_deref()),
        parent_model,
    );
    // `route_role` is absent when an Agent/Workflow phase supplied an explicit
    // model because Smart correctly preserves that choice. Recover the same
    // effective role from either the built-in profile or task text (needed for
    // custom agents), so a plain 429 cannot silently escalate implementation
    // from Sonnet/Terra to a parent Fable/Sol.
    // A real custom definition wins over a colliding built-in-looking name
    // (`analysis.md`, `reviewer.md`, ...), matching harness resolution above.
    // For custom/unknown writable agents, treat any concrete write intent as
    // implementation-shaped even when review wording wins the specialty label.
    let custom_route_context = custom_agent.as_ref().map(|agent| {
        format!(
            "{} {} {}",
            agent.name, agent.description, agent.system_prompt
        )
    });
    let implementation_description = custom_route_context
        .as_deref()
        .map_or_else(
            || input.description.clone(),
            |custom| format!("{} {custom}", input.description),
        );
    let implementation_kind = if custom_agent.is_none() { resolved_subagent_type.as_str() } else { "" };
    let implementation_route = supports_file_edits(implementation_kind)
        && (matches!(input.route_role.as_deref(), Some("coding" | "debugging"))
            || implementation_task(implementation_kind, &implementation_description, &input.prompt));
    let person_model_pin = route_is_person_pin || person_selected(&model)
        || custom_agent.as_ref().and_then(|agent| agent.model.as_deref())
            .is_some_and(|pin| same_model(pin, &model))
        || std::env::var(AGENT_MODEL_ENV).ok().is_some_and(|pin| !pin.trim().is_empty());
    if implementation_route {
        if let Some(reason) = implementation_model_refusal(&model, parent_model, person_model_pin) {
            return Err(ToolError::Declined(reason));
        }
    }
    let route_fallback_models = if person_model_pin { Vec::new() } else { rate_limit_fallback_models(
        &model,
        parent_model,
        &input.route_fallback_models,
        implementation_route,
        input.route_complexity.as_deref(),
        input.prior_failures,
        smart_policy,
    ) };
    // A validated model-supplied name is the agent's ADDRESS: keep it byte
    // exact so `SendMessage(to: "Web_Scout")` finds it. Everything else (the
    // legacy `name` field internal spawners set, and the description fallback)
    // still goes through the display slug.
    let agent_name = input
        .addressable_name
        .clone()
        .or_else(|| {
            input
                .name
                .as_deref()
                .map(slugify_agent_name)
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| slugify_agent_name(&input.description));
    let agent_label = display_agent_label(
        input.name.as_deref(),
        &input.description,
        &agent_name,
        &resolved_subagent_type,
    );
    let created_at = epoch_seconds_now();
    // A fork's prompt is the parent's, as the parent's runtime holds it — no
    // harness instruction of its own. One harness either way: `AgentJob::
    // harness` reads this value like any other.
    let system_prompt = match fork_source.as_ref() {
        Some(source) => source.system_prompt.clone(),
        None => build_agent_system_prompt(
            &resolved_subagent_type,
            custom_agent.as_ref(),
            Some(&model),
        )?,
    };
    let mut allowed_tools =
        allowed_tools_for_resolved(&resolved_subagent_type, custom_agent.as_ref());
    extend_allowed_tools_with_mcp(&mut allowed_tools, input.mcp_passthrough.as_ref(), custom_agent.as_ref());

    // Information-topology edges resolve BEFORE any durable state exists, so
    // a bad reference fails the spawn without leaving a running-status
    // manifest behind. The resolved deliverables join the prompt engine-side
    // — the parent model spends zero output tokens relaying them.
    let seen_outputs = if input.sees.is_empty() {
        Vec::new()
    } else if telemetry::attest_ablated(telemetry::HarnessFeature::InformationTopology) {
        // Ablation arm: the field goes inert — no injection, and no reference
        // validation either, because the control arm has to behave as if
        // `sees` did not exist rather than as if it existed and failed.
        Vec::new()
    } else {
        match topology::resolve_seen_outputs(&registry, &input.sees, input.parent_session_id.as_deref()) {
            Ok(seen) => {
                telemetry::attest_fired(telemetry::HarnessFeature::InformationTopology);
                seen
            }
            Err(error) => {
                telemetry::attest_failed(
                    telemetry::HarnessFeature::InformationTopology,
                    "sees_reference_unresolvable",
                );
                return Err(error);
            }
        }
    };
    let sees_names: Vec<String> = seen_outputs
        .iter()
        .map(|seen| seen.name.clone())
        .collect();
    let prompt = topology::prompt_with_seen_context(&input.prompt, &seen_outputs);

    let sees_line = if sees_names.is_empty() {
        String::new()
    } else {
        format!("- sees: {}\n", sees_names.join(", "))
    };
    let output_contents = format!(
        "# Agent Task

- id: {}
- name: {}
- description: {}
- subagent_type: {}
- created_at: {}
{}
## Prompt

{}
",
        agent_id, agent_name, input.description, resolved_subagent_type, created_at, sees_line, input.prompt
    );
    manifest::write_new_agent_output(&output_file, &agent_id, &output_contents)
        .map_err(ToolError::Execution)?;

    let mut manifest = AgentOutput {
        agent_id,
        parent_session_id: input.parent_session_id.clone(),
        tool_call_id: input.tool_call_id.clone(),
        name: agent_name,
        label: agent_label,
        description: input.description,
        subagent_type: Some(resolved_subagent_type),
        requested_model,
        resolved_model: Some(model.clone()),
        route_reason: input.route_reason.clone(),
        route_role: input.route_role.clone(),
        route_complexity: input.route_complexity.clone(),
        route_risk: input.route_risk.clone(),
        route_source: input.route_source.clone(),
        route_probe_confidence: input.route_probe_confidence.clone(),
        sees: sees_names,
        model: Some(model),
        status: String::from("running"),
        output_file: output_file.display().to_string(),
        manifest_file: manifest_file.display().to_string(),
        created_at: created_at.clone(),
        pane: None,
        owner_pid: Some(std::process::id()),
        run_generation: AGENT_INITIAL_RUN_GENERATION,
        started_at: Some(created_at),
        completed_at: None,
        completion_published_at: None,
        lane_events: vec![LaneEvent::started(epoch_seconds_now())],
        current_blocker: None,
        error: None,
        token_history: Vec::new(),
        current_tool: None,
        awaiting_provider_since: None,
        recent_tools: Vec::new(),
        tool_calls: 0,
        current_phase: None,
        output_tail: String::new(),
        last_activity_at: None,
        activity: AgentActivityTelemetry {
            thinking_budget_tokens,
            fallback_models: route_fallback_models.clone(),
            ..AgentActivityTelemetry::default()
        },
        lifecycle: AgentLifecycle::default(),
    };
    let cancel_signal = runtime::HookAbortSignal::new();
    register_agent_cancel_signal(
        manifest.agent_id.clone(),
        manifest.run_generation,
        cancel_signal.clone(),
    );
    // A stop must have a live signal before `running` becomes discoverable.
    let steering = runtime::SteeringQueue::default();
    register_agent_steering(
        manifest.agent_id.clone(),
        manifest.run_generation,
        steering.clone(),
    );
    // SendMessage delivery + resume: the queue is registered before the
    // detached thread exists, and the session gets a live transcript next to
    // the manifest. The path is derived, not read — the manifest is not on
    // disk yet.
    let transcript_path =
        resume::transcript_path_for(&manifest).map_err(ToolError::Execution)?;
    // A fork's transcript is the parent's, copied now — before the manifest
    // and the stored harness exist — so a parent transcript that cannot be
    // read leaves no running-status child behind (the output file already
    // written is taken back). The job below is a resume of that copy: both
    // executors rehydrate it by the road a resumed child takes (t-2513 §2.4).
    let forked = match fork_source.as_ref() {
        Some(source) => match fork::write_fork_transcript(
            &source.transcript,
            &transcript_path,
            &manifest.agent_id,
            manifest.tool_call_id.as_deref(),
        ) {
            Ok(forked) => Some(forked),
            Err(error) => {
                unregister_agent_cancel_signal(&manifest.agent_id, manifest.run_generation);
                unregister_agent_steering(&manifest.agent_id, manifest.run_generation);
                let _ = std::fs::remove_file(&output_file);
                return Err(ToolError::Execution(error));
            }
        },
        None => None,
    };
    let prompt = match forked {
        Some(forked) => fork::fork_prompt(&prompt, forked),
        None => prompt,
    };

    // The job is built BEFORE the manifest is written so the harness it
    // resolves can be digested onto the manifest (t-2513 §2.1/§2.4): one
    // `harness.json` per child, read back by a resume instead of re-derived,
    // and one digest every reader can compare. The harness FILE itself and
    // the resume snapshot are stored only after the executor holds the job
    // (`materialise_agent_resume_files`, t-2902).
    let mut job = AgentJob {
        manifest: manifest.clone(),
        registry: Some(registry),
        prompt,
        system_prompt,
        allowed_tools,
        // Built-in types resolve `custom_agent` to `None`, so both stay `None`
        // and the spawned enforcer is byte-identical to today.
        permission_rules: custom_agent
            .as_ref()
            .and_then(|agent| agent.permission.clone()),
        permission_mode: clamped_spawn_mode(
            input.parent_permission_mode,
            custom_agent.as_ref().and_then(|agent| agent.permission_mode),
        ),
        cwd: input.cwd,
        // Shared into the sub-agent's context only when it runs in the parent
        // cwd (see [`build_agent_runtime`]); cloned here so it crosses into the
        // spawned job. An empty registry is harmless downstream.
        lsp: parent_lsp.cloned(),
        schema: input.schema,
        workflow_member: input.workflow_member,
        one_shot: input.one_shot,
        plan_shape: input.plan_shape,
        route_tax: input.route_tax,
        // Read HERE, while the launching turn is still the session's live
        // attempt: a background agent can outlive its turn, and reading at
        // record time would credit whichever turn happened to be running when
        // it ended.
        parent_attempt: served_turn_attempt(input.parent_session_id.as_deref()),
        time_budget: input.time_budget,
        thinking_budget_tokens,
        route_effort: input.route_effort,
        api_concurrency: input.api_concurrency,
        route_fallback_models,
        mcp_passthrough: input.mcp_passthrough,
        // Sub-agents run the parent's hooks through the sub-agent view:
        // main-agent-only events (Stop/TurnEnd, UserPromptSubmit, Session*)
        // stripped, tool/subagent/compaction hooks kept (CC contract).
        hook_config: hook_config
            .map(runtime::RuntimeHookConfig::for_subagent)
            .unwrap_or_default(),
        cancel_signal,
        judged_agent: input.judged_agent,
        verify_loop: input.verify_loop,
        parent_model: parent_model.map(str::to_string),
        steering,
        transcript_path: Some(transcript_path),
        resume: forked.is_some(),
    };
    let harness = job.harness();
    let digest = harness.digest();
    manifest.lifecycle.harness_digest = Some(digest.clone());
    job.manifest.lifecycle.harness_digest = Some(digest);
    if let Err(error) = write_agent_manifest(&manifest) {
        unregister_agent_cancel_signal(&manifest.agent_id, manifest.run_generation);
        unregister_agent_steering(&manifest.agent_id, manifest.run_generation);
        return Err(error.into());
    }
    // Read off the job now — it moves into the executor next — and written
    // after, with the harness file (below).
    let resume_snapshot = resume::AgentResumeSnapshot::from_job(&job);
    match run_if_agent_manifest_running(&manifest, || spawn_fn(job)) {
        Ok(Some(())) => {}
        Ok(None) => {
            unregister_agent_cancel_signal(&manifest.agent_id, manifest.run_generation);
            unregister_agent_steering(&manifest.agent_id, manifest.run_generation);
            return Err(ToolError::Execution(
                "sub-agent stopped before its worker could start".to_string(),
            ));
        }
        Err(error) => {
            let error_msg = format!("failed to spawn sub-agent: {error}");
            unregister_agent_cancel_signal(&manifest.agent_id, manifest.run_generation);
            unregister_agent_steering(&manifest.agent_id, manifest.run_generation);
            persist_agent_terminal_state(&manifest, "failed", None, Some(error_msg.clone()))?;
            return Err(ToolError::Execution(error_msg));
        }
    }
    materialise_agent_resume_files(&manifest, &harness, &resume_snapshot);

    Ok(manifest)
}

/// Store what a LATER resume or audit reads — the harness file beside the
/// child's brief (`<id>/harness.json`) and the resume snapshot
/// (`<id>.resume.json`) — once the executor already holds the job (t-2902).
///
/// Neither is a condition of the child's liveness: the inline executor reads
/// the harness off the job value it was handed, the pane executor writes the
/// same value into the brief, and the manifest — written BEFORE the executor
/// ran, so the registry reports the child from its first instant — already
/// names the harness by digest. Writing these two first put their cost
/// (serialising 15 KB and two file publishes) between the parent's `Agent`
/// call and the child's first provider request, where Claude Code writes a
/// 102-byte meta file. Now the child's own thread is already building its
/// runtime while these land.
///
/// The harness value is the one `AgentJob::harness` resolved for the digest —
/// nothing is re-resolved or re-serialised here beyond the file itself.
/// Best-effort, both: a lost harness file only costs a later resume its stored
/// copy (it re-resolves and says `stale`); a lost snapshot only costs future
/// resumability.
fn materialise_agent_resume_files(
    manifest: &AgentOutput,
    harness: &runtime::subagent_panes::ResolvedHarness,
    resume_snapshot: &resume::AgentResumeSnapshot,
) {
    if let Ok(directory) = panes::child_directory(manifest) {
        let _ = harness.write(&directory);
    }
    let _ = resume_snapshot.write_beside(manifest);
}

/// Resume a TERMINAL agent with a follow-up message, keeping its full prior
/// context: the persisted transcript is rehydrated into the new runtime's
/// session and `message` becomes its next user turn (the `SendMessage` resume
/// path — Claude Code's "continue a previously spawned agent" contract).
///
/// The resumed run is always detached and marked as a background agent, so
/// its reply rides the SAME completion channel a background spawn uses — the
/// interactive host re-injects it into the parent conversation automatically.
/// A running agent must be steered instead ([`steer_agent`]); callers gate on
/// the manifest status.
/// Child agents never exceed the spawning session's privilege: the requested
/// mode (custom-agent frontmatter, else the historical `DangerFullAccess`
/// default) is clamped to the parent's active mode on the `ReadOnly` <
/// `WorkspaceWrite` < `DangerFullAccess` ladder. An unranked or unknown
/// parent (`Prompt`/`Allow`/`None`) keeps the requested mode — those hosts
/// gate interactively instead of by static privilege.
fn clamped_spawn_mode(
    parent: Option<PermissionMode>,
    requested: Option<PermissionMode>,
) -> Option<PermissionMode> {
    match parent {
        Some(parent) => Some(
            requested
                .unwrap_or(PermissionMode::DangerFullAccess)
                .clamp_to(parent),
        ),
        None => requested,
    }
}

/// What a resume did about the harness (t-2513 §2.4): the stored copy was
/// re-loaded as written, or re-resolved because the caller asked, or the
/// stored copy is kept although its source definition has moved since.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumedHarness {
    /// `harness.json` reloaded, and it still matches a fresh resolution.
    Stored,
    /// `refresh_harness` was asked: re-resolved from today's definition.
    Refreshed,
    /// `harness.json` reloaded, but a fresh resolution would differ — the
    /// custom definition changed underneath. Said, not hidden.
    Stale,
    /// No stored harness (an agent spawned before t-2513): resolved fresh,
    /// as every resume used to.
    Derived,
}

impl ResumedHarness {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Stored => "carried",
            Self::Refreshed => "refreshed",
            Self::Stale => "stale",
            Self::Derived => "derived",
        }
    }
}

/// A resumed agent's manifest and what its harness came to.
#[derive(Debug, Clone)]
pub(crate) struct Resumed {
    pub(crate) manifest: AgentOutput,
    pub(crate) harness: ResumedHarness,
}

#[allow(clippy::too_many_arguments)] // the resuming host's handles ride beside the registry, as in `resume_agent_with_spawn`
pub(crate) fn resume_agent_with_message(
    registry: Option<std::sync::Arc<AgentRegistry>>,
    manifest: &AgentOutput,
    message: &str,
    parent_lsp: Option<&LspRegistry>,
    hook_config: Option<&RuntimeHookConfig>,
    mcp_passthrough: Option<crate::registry::McpPassthrough>,
    parent_permission_mode: Option<PermissionMode>,
    refresh_harness: bool,
) -> Result<Resumed, ToolError> {
    // The mode is the MANIFEST's (t-2513 contract 4): a pane child continues
    // in a pane — through its live channel when it is still idle there, in
    // a re-cut one on the same transcript when it is not — and a thread
    // continues as a thread. Nothing here turns one into the other.
    match manifest.lifecycle.execution.as_deref() {
        Some(EXECUTION_PANE) => resume_agent_with_spawn(
            registry,
            manifest,
            message,
            parent_lsp,
            hook_config,
            mcp_passthrough,
            parent_permission_mode,
            refresh_harness,
            panes::resume_pane_job,
        ),
        _ => resume_agent_with_spawn(
            registry,
            manifest,
            message,
            parent_lsp,
            hook_config,
            mcp_passthrough,
            parent_permission_mode,
            refresh_harness,
            spawn_agent_job,
        ),
    }
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // cohesive resume→claim→spawn transaction kept in order; the registry rides beside the parent's other handles
fn resume_agent_with_spawn<F>(
    registry: Option<std::sync::Arc<AgentRegistry>>,
    manifest: &AgentOutput,
    message: &str,
    parent_lsp: Option<&LspRegistry>,
    hook_config: Option<&RuntimeHookConfig>,
    mcp_passthrough: Option<crate::registry::McpPassthrough>,
    parent_permission_mode: Option<PermissionMode>,
    refresh_harness: bool,
    spawn_fn: F,
) -> Result<Resumed, ToolError>
where
    F: FnOnce(AgentJob) -> Result<(), ToolError> + Send,
{
    if message.trim().is_empty() {
        return Err(ToolError::InvalidInput("message must not be empty".into()));
    }
    let transcript_path =
        resume::transcript_path_for(manifest).map_err(ToolError::Execution)?;
    if !transcript_path.is_file() {
        return Err(ToolError::Execution(format!(
            "agent '{}' has no persisted transcript to resume from (it predates \
             transcript persistence); spawn a new Agent instead",
            manifest.name
        )));
    }
    let snapshot = resume::load_agent_resume_snapshot(manifest);
    // Re-resolve the harness exactly like a fresh spawn — either to USE it
    // (`refresh_harness`, or no stored copy) or to COMPARE it against the
    // stored copy and say `stale` when the definition moved underneath.
    let (resolved_subagent_type, custom_agent) = resolve_subagent_type_and_custom_agent(
        manifest.subagent_type.as_deref(),
        &manifest.description,
        message,
    );
    // Resume rebuilds the harness on the model the agent was spawned onto, so
    // the resumed prompt names the same model the original run did.
    let fresh_system_prompt = build_agent_system_prompt(
        &resolved_subagent_type,
        custom_agent.as_ref(),
        manifest.resolved_model.as_deref(),
    )?;
    let mut fresh_allowed_tools =
        allowed_tools_for_resolved(&resolved_subagent_type, custom_agent.as_ref());
    extend_allowed_tools_with_mcp(&mut fresh_allowed_tools, mcp_passthrough.as_ref(), custom_agent.as_ref());
    let fresh_permission_rules = custom_agent
        .as_ref()
        .and_then(|agent| agent.permission.clone());
    // A resumed agent re-clamps against the resuming host's mode — the
    // fresh-spawn clamp must not be escapable by a later SendMessage.
    let fresh_permission_mode = clamped_spawn_mode(
        parent_permission_mode,
        custom_agent.as_ref().and_then(|agent| agent.permission_mode),
    );
    let stored = if refresh_harness {
        None
    } else {
        panes::child_directory(manifest)
            .ok()
            .and_then(|directory| runtime::subagent_panes::ResolvedHarness::read(&directory))
    };
    let (system_prompt, allowed_tools, permission_rules, permission_mode, harness_state) = match stored {
        Some(stored) => {
            let matches_fresh = stored.system_prompt == fresh_system_prompt
                && stored.allowed_tools == fresh_allowed_tools
                && stored.permission_rules
                    == fresh_permission_rules
                        .as_ref()
                        .map(runtime::subagent_panes::PermissionRules::from);
            let state = if matches_fresh {
                ResumedHarness::Stored
            } else {
                ResumedHarness::Stale
            };
            (
                stored.system_prompt,
                stored.allowed_tools,
                stored
                    .permission_rules
                    .as_ref()
                    .map(runtime::subagent_panes::PermissionRules::to_config),
                // The mode is re-clamped to the RESUMING host even from a stored
                // harness: the parent's ceiling is a fact about now, not then.
                clamped_spawn_mode(
                    parent_permission_mode,
                    stored.permission_mode.as_deref().and_then(PermissionMode::parse),
                ),
                state,
            )
        }
        None => (
            fresh_system_prompt,
            fresh_allowed_tools,
            fresh_permission_rules,
            fresh_permission_mode,
            if refresh_harness {
                ResumedHarness::Refreshed
            } else {
                ResumedHarness::Derived
            },
        ),
    };
    let cancel_signal = runtime::HookAbortSignal::new();
    let steering = runtime::SteeringQueue::default();
    let next_manifest = manifest::persist_agent_resumed_state_with(
        manifest,
        message,
        |next_manifest| {
            register_agent_cancel_signal(
                next_manifest.agent_id.clone(),
                next_manifest.run_generation,
                cancel_signal.clone(),
            );
            register_agent_steering(
                next_manifest.agent_id.clone(),
                next_manifest.run_generation,
                steering.clone(),
            );
        },
        |next_manifest| {
            unregister_agent_cancel_signal(
                &next_manifest.agent_id,
                next_manifest.run_generation,
            );
            unregister_agent_steering(
                &next_manifest.agent_id,
                next_manifest.run_generation,
            );
        },
    )
    .map_err(ToolError::Execution)?;
    // A terminal completion belongs to the prior generation. Do not clear it
    // until the new running generation is durable.
    reset_agent_completion(&manifest.agent_id);
    // The resumed reply must ride the same push-back channel as a background
    // spawn so the interactive host re-invokes the parent model with it.
    mark_background_agent(next_manifest.agent_id.clone());
    let mut job = AgentJob {
        manifest: next_manifest.clone(),
        registry,
        prompt: message.to_string(),
        system_prompt,
        allowed_tools,
        permission_rules,
        permission_mode,
        cwd: snapshot.cwd,
        lsp: parent_lsp.cloned(),
        schema: snapshot.schema,
        workflow_member: false,
        one_shot: false,
        plan_shape: None,
        route_tax: None,
        // A resume is a new attempt at the same agent, served by whichever
        // turn asked for it.
        parent_attempt: served_turn_attempt(next_manifest.parent_session_id.as_deref()),
        // An interactive follow-up inherits no wall-clock budget — the
        // original one may already be spent.
        time_budget: None,
        thinking_budget_tokens: snapshot.thinking_budget_tokens,
        route_effort: snapshot.route_effort,
        api_concurrency: snapshot.api_concurrency,
        route_fallback_models: snapshot.route_fallback_models,
        mcp_passthrough,
        hook_config: hook_config
            .map(runtime::RuntimeHookConfig::for_subagent)
            .unwrap_or_default(),
        cancel_signal,
        // A follow-up turn must never re-credit a review verdict.
        judged_agent: None,
        verify_loop: None,
        parent_model: snapshot.parent_model,
        steering,
        transcript_path: Some(transcript_path),
        resume: true,
    };
    // The harness this resume runs is digested onto the manifest like a
    // fresh spawn's; a refreshed or re-derived one is stored so the NEXT
    // resume reloads it. A stale one is kept as stored — the digest already
    // names it.
    let harness = job.harness();
    let digest = harness.digest();
    job.manifest.lifecycle.harness_digest = Some(digest.clone());
    manifest::stamp_agent_lifecycle(&next_manifest, |lifecycle| {
        lifecycle.harness_digest = Some(digest);
    });
    if matches!(harness_state, ResumedHarness::Refreshed | ResumedHarness::Derived) {
        if let Ok(directory) = panes::child_directory(&next_manifest) {
            let _ = harness.write(&directory);
        }
    }
    match run_if_agent_manifest_running(&next_manifest, || spawn_fn(job)) {
        Ok(Some(())) => {}
        Ok(None) => {
            clear_background_agent(&next_manifest.agent_id);
            unregister_agent_cancel_signal(&next_manifest.agent_id, next_manifest.run_generation);
            unregister_agent_steering(&next_manifest.agent_id, next_manifest.run_generation);
            return Err(ToolError::Execution(
                "resumed sub-agent stopped before its worker could start".to_string(),
            ));
        }
        Err(error) => {
            let error_msg = format!("failed to resume sub-agent: {error}");
            clear_background_agent(&next_manifest.agent_id);
            unregister_agent_cancel_signal(&next_manifest.agent_id, next_manifest.run_generation);
            unregister_agent_steering(&next_manifest.agent_id, next_manifest.run_generation);
            persist_agent_terminal_state(&next_manifest, "failed", None, Some(error_msg.clone()))?;
            return Err(ToolError::Execution(error_msg));
        }
    }
    Ok(Resumed {
        manifest: next_manifest,
        harness: harness_state,
    })
}

/// Sub-agents inherit the parent session's MCP tools (Claude Code parity):
/// extend the type's allow-set with the advertised names so the executor
/// accepts them and the provider client advertises their schemas. A custom
/// agent with an explicit `tools:` list keeps exactly that list — it may
/// name MCP tools itself, but nothing is added behind its back.
fn extend_allowed_tools_with_mcp(
    allowed_tools: &mut BTreeSet<String>,
    mcp_passthrough: Option<&crate::registry::McpPassthrough>,
    custom_agent: Option<&CustomAgent>,
) {
    let Some(mcp) = mcp_passthrough else {
        return;
    };
    let custom_explicit = custom_agent
        .is_some_and(|agent| agent.tools.as_ref().is_some_and(|tools| !tools.is_empty()));
    if custom_explicit {
        return;
    }
    allowed_tools.extend(
        mcp.definitions_snapshot()
            .into_iter()
            .map(|definition| definition.name),
    );
}

/// Mark every live agent manifest created in this visible session as stopped.
///
/// Sub-agents run on detached OS threads, so cancelling the foreground turn may
/// drop the caller that was waiting for them before their own runtime gets a
/// chance to persist a terminal state. This best-effort close keeps the TUI HUD,
/// workflow viewer, and completion waiters from showing stale `running` rows for
/// the full abandoned-agent window. If a worker later reaches a real terminal
/// state, its normal completion write wins.
#[allow(clippy::must_use_candidate)] // the reap side effect is the point; the count is auxiliary
pub fn stop_running_agents_since(registry: &AgentRegistry, started_after: u64, reason: &str) -> usize {
    stop_running_agents_since_for_session(registry, started_after, None, reason)
}

#[allow(clippy::must_use_candidate)]
/// How old a LEGACY manifest (no `ownerPid` stamp) must be — by last write,
/// not creation — before the orphan reap settles it. Generous on purpose: a
/// live agent keeps writing its manifest (phase stamps, output tail, tool
/// liveness), so a day of total silence on a `running` manifest means the
/// owning process is long gone.
const LEGACY_ORPHAN_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Settle orphaned `running` manifests in this project's agent store: an
/// agent runs as a THREAD inside a zo process, so a manifest still marked
/// running whose owning process is dead is a phantom — the HUD shows it
/// running forever, and "the agent stopped but zo can't tell" is exactly
/// what a user sees. Stamped manifests are checked against live PIDs (one
/// batched `ps` probe); legacy unstamped ones fall back to
/// `LEGACY_ORPHAN_MAX_AGE` since their last write. The reap persists a
/// terminal `stopped` state (no completion notification — the waiter, if
/// any, died with the owner) and returns how many were settled.
#[must_use]
pub fn reap_orphaned_agents(registry: &AgentRegistry) -> usize {
    registry
        .stores()
        .iter()
        .map(|store| reap_orphaned_agents_in_store(store))
        .sum()
}

fn reap_orphaned_agents_in_store(store: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(store) else {
        return 0;
    };
    let mut candidates: Vec<AgentOutput> = Vec::new();
    let mut legacy: Vec<AgentOutput> = Vec::new();
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json")
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".resume.json"))
        {
            continue;
        }
        let Ok(manifest) = load_agent_manifest_from_scanned_path(&path) else {
            continue;
        };
        if agent_output_status_is_terminal(&manifest.status) {
            continue;
        }
        match manifest.owner_pid {
            // Our own live agents are never orphans.
            Some(pid) if pid == std::process::id() => {}
            Some(_) => candidates.push(manifest),
            None => {
                let stale = entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|modified| now.duration_since(modified).ok())
                    .is_some_and(|age| age >= LEGACY_ORPHAN_MAX_AGE);
                if stale {
                    legacy.push(manifest);
                }
            }
        }
    }
    if !candidates.is_empty() {
        let alive = live_zo_pids(candidates.iter().filter_map(|manifest| manifest.owner_pid));
        candidates.retain(|manifest| {
            manifest
                .owner_pid
                .is_some_and(|pid| !alive.contains(&pid))
        });
    }
    let mut reaped = 0usize;
    for manifest in candidates.into_iter().chain(legacy) {
        if persist_orphaned_agent_stopped(&manifest) {
            reaped = reaped.saturating_add(1);
        }
    }
    reaped
}

fn persist_orphaned_agent_stopped(manifest: &AgentOutput) -> bool {
    persist_agent_stopped_state_with(manifest, "orphaned: owning process exited", || {
        abort_and_unregister_agent_cancel_signal(&manifest.agent_id, manifest.run_generation);
        unregister_agent_steering(&manifest.agent_id, manifest.run_generation);
    })
    .is_ok_and(|transitioned| transitioned)
}

fn settle_dead_owner_agent_with_live(manifest: &AgentOutput, live: &HashSet<u32>) -> bool {
    manifest.status == "running"
        && manifest.owner_pid.is_some_and(|pid| !live.contains(&pid))
        && persist_orphaned_agent_stopped(manifest)
}

pub(crate) fn settle_dead_owner_agent(manifest: &AgentOutput) -> bool {
    let Some(owner_pid) = manifest.owner_pid else {
        return false;
    };
    let live = live_zo_pids(std::iter::once(owner_pid));
    settle_dead_owner_agent_with_live(manifest, &live)
}

/// The subset of `pids` that are live zo processes, probed with ONE
/// batched `ps` call. A PID whose executable basename is not exactly `zo`
/// counts as dead, so a recycled PID cannot hide behind names such as `zoom`
/// or `zoxide`. On any probe failure every pid is presumed alive — the reap
/// must fail closed (keep manifests) rather than kill live agents.
fn is_zo_process_comm(comm: &str) -> bool {
    Path::new(comm).file_name() == Some(std::ffi::OsStr::new("zo"))
}

fn live_zo_pids(pids: impl Iterator<Item = u32>) -> std::collections::HashSet<u32> {
    let unique: std::collections::HashSet<u32> = pids.collect();
    if unique.is_empty() {
        return unique;
    }
    let list = unique
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(output) = std::process::Command::new("ps")
        .args(["-o", "pid=,comm=", "-p", &list])
        .output()
    else {
        return unique; // probe failed — presume alive, reap nothing
    };
    let mut alive = std::collections::HashSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(comm)) = (parts.next(), parts.next()) else {
            continue;
        };
        if let Ok(pid) = pid.parse::<u32>() {
            if is_zo_process_comm(comm) {
                alive.insert(pid);
            }
        }
    }
    alive
}

#[must_use]
pub fn stop_running_agents_since_for_session(
    registry: &AgentRegistry,
    started_after: u64,
    session_id: Option<&str>,
    reason: &str,
) -> usize {
    registry
        .stores()
        .iter()
        .map(|store| stop_running_agents_in_store_since(store, started_after, session_id, reason))
        .sum()
}

// NOTE: there is deliberately no "steer every running agent" broadcast here.
// A mid-turn user message is addressed to the MAIN conversation (CC parity);
// reaching a specific agent is an explicit act — the agents viewer's message
// box (Alt+A → m → `send_agent_message`) or the model relaying via
// `SendMessage`. The old broadcast made workers act on asides meant for the
// main model (live report: a model-choice remark steered a running agent).

#[allow(clippy::must_use_candidate)]
pub fn stop_running_agents_since_for_strict_session(
    registry: &AgentRegistry,
    started_after: u64,
    session_id: &str,
    reason: &str,
) -> usize {
    registry
        .stores()
        .iter()
        .map(|store| {
            stop_running_agents_in_store_since_inner(
                store,
                started_after,
                Some(session_id),
                reason,
                true,
                false,
            )
        })
        .sum()
}

/// Result of stopping one agent from the host's session-scoped agents viewer.
///
/// The viewer is a destructive surface, so "same session" alone is not enough:
/// a second zo process can reopen the same session id. A stop is allowed only
/// when the manifest is stamped with both the exact session id and this process
/// id, and this process still owns the matching live run generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStopOutcome {
    Stopped { name: String },
    /// An idle pane child took the door: asked over its channel to leave,
    /// and it wrote its final result on the way out.
    Closed { name: String },
    AlreadyFinished { name: String, status: String },
    NotFound,
    NotOwned { name: String },
    Unreachable { name: String },
    Failed { name: String, error: String },
}

/// Stop exactly one live in-process agent owned by `session_id`.
///
/// This deliberately fails closed for legacy manifests without ownership
/// stamps. It never broadens to a name/prefix lookup and never stops sibling
/// agents: the UI passes the exact id from its session-filtered picker.
#[must_use]
pub fn stop_agent_for_session(
    registry: &AgentRegistry,
    agent_id: &str,
    session_id: &str,
    reason: &str,
) -> AgentStopOutcome {
    let Some(store) = registry.store_of(agent_id) else {
        return AgentStopOutcome::NotFound;
    };
    stop_agent_for_session_in(&store, agent_id, session_id, reason, true)
}

fn stop_agent_for_session_in(
    store: &Path,
    agent_id: &str,
    session_id: &str,
    reason: &str,
    emit_completion: bool,
) -> AgentStopOutcome {
    let Ok(filename) = manifest::expected_manifest_filename(agent_id) else {
        return AgentStopOutcome::NotFound;
    };
    let Ok(manifest) = load_agent_manifest_from_scanned_path(&store.join(filename)) else {
        return AgentStopOutcome::NotFound;
    };
    let name = manifest
        .label
        .clone()
        .unwrap_or_else(|| manifest.name.clone());
    if !agent_output_belongs_to_session(&manifest, Some(session_id), false)
        || manifest.owner_pid != Some(std::process::id())
    {
        return AgentStopOutcome::NotOwned { name };
    }
    // A pane child that finished its turn is IDLE, not gone: it waits in its
    // pane for the parent's next word until released (design §2.2). Its
    // status reads `completed` — terminal for an inline child — so this stop
    // used to answer "already finished" and the pane stood until the idle
    // budget ran out. The release is the parent's to give: over the child's
    // channel ("다시 필요 없음 닫는 기능까지", 2026-09-06).
    match panes::release_idle_pane_child(&manifest) {
        Some(true) => {
            let released = reason.trim().to_string();
            manifest::stamp_agent_lifecycle(&manifest, |lifecycle| {
                lifecycle.released = Some(released);
            });
            return AgentStopOutcome::Closed { name };
        }
        Some(false) => return AgentStopOutcome::Unreachable { name },
        None => {}
    }
    if agent_output_status_is_terminal(&manifest.status) {
        return AgentStopOutcome::AlreadyFinished {
            name,
            status: manifest.status,
        };
    }
    if !agent_worker_generation_is_live(&manifest.agent_id, manifest.run_generation) {
        return AgentStopOutcome::Unreachable { name };
    }

    let reason = reason.trim();
    let reason = if reason.is_empty() {
        "stopped by user from agents overview"
    } else {
        reason
    };
    let mut worker_will_notify = false;
    let transitioned = match persist_agent_stopped_state_with(&manifest, reason, || {
        worker_will_notify = abort_and_unregister_agent_cancel_signal(
            &manifest.agent_id,
            manifest.run_generation,
        );
        unregister_agent_steering(&manifest.agent_id, manifest.run_generation);
    }) {
        Ok(transitioned) => transitioned,
        Err(error) => return AgentStopOutcome::Failed { name, error },
    };
    if !transitioned {
        return match load_agent_manifest_from_scanned_path(Path::new(&manifest.manifest_file)) {
            Ok(latest) if agent_output_status_is_terminal(&latest.status) => {
                AgentStopOutcome::AlreadyFinished {
                    name,
                    status: latest.status,
                }
            }
            Ok(_) | Err(_) => AgentStopOutcome::Unreachable { name },
        };
    }
    if emit_completion {
        if worker_will_notify {
            schedule_external_stop_fallback(manifest, reason.to_string());
        } else {
            let generation = manifest.run_generation;
            let manifest_file = std::path::PathBuf::from(&manifest.manifest_file);
            notify_agent_completion(
                stopped_completion(manifest, reason.to_string()),
                Some(generation),
                Some(manifest_file),
            );
        }
    }
    AgentStopOutcome::Stopped { name }
}

fn stop_running_agents_in_store_since(
    store: &Path,
    started_after: u64,
    session_id: Option<&str>,
    reason: &str,
) -> usize {
    stop_running_agents_in_store_since_inner(store, started_after, session_id, reason, true, true)
}

#[cfg(test)]
fn stop_running_agents_in_store_since_without_notify(
    store: &Path,
    started_after: u64,
    reason: &str,
) -> usize {
    stop_running_agents_in_store_since_inner(store, started_after, None, reason, false, true)
}

#[cfg(test)]
fn stop_running_agents_in_store_since_for_session_without_notify(
    store: &Path,
    started_after: u64,
    session_id: Option<&str>,
    reason: &str,
) -> usize {
    stop_running_agents_in_store_since_inner(store, started_after, session_id, reason, false, true)
}

#[cfg(test)]
fn stop_running_agents_in_store_since_for_strict_session_without_notify(
    store: &Path,
    started_after: u64,
    session_id: &str,
    reason: &str,
) -> usize {
    stop_running_agents_in_store_since_inner(
        store,
        started_after,
        Some(session_id),
        reason,
        false,
        false,
    )
}

const EXTERNAL_STOP_FALLBACK_DELAY: Duration = Duration::from_secs(1);

fn external_stop_owns_completion(emit_completion: bool, transitioned: bool) -> bool {
    emit_completion && transitioned
}

fn stopped_completion(manifest: AgentOutput, reason: String) -> AgentCompletion {
    AgentCompletion {
        agent_id: manifest.agent_id,
        name: manifest.label.unwrap_or(manifest.name),
        status: String::from("stopped"),
        result: None,
        structured: None,
        error: Some(reason),
        run: HelperRun::default(),
    }
}

fn publish_external_stop_fallback(manifest: AgentOutput, reason: String) {
    // A resumed run reuses the same agent id and completion slot. Never let an
    // old stop watchdog publish into the newer durable generation.
    if manifest::manifest_generation_is_current(&manifest) {
        let generation = manifest.run_generation;
        let manifest_file = std::path::PathBuf::from(&manifest.manifest_file);
        notify_agent_completion(
            stopped_completion(manifest, reason),
            Some(generation),
            Some(manifest_file),
        );
    }
}

fn schedule_external_stop_fallback(manifest: AgentOutput, reason: String) {
    let fallback_manifest = manifest.clone();
    let fallback_reason = reason.clone();
    let fallback = move || {
        std::thread::sleep(EXTERNAL_STOP_FALLBACK_DELAY);
        publish_external_stop_fallback(fallback_manifest, fallback_reason);
    };
    if std::thread::Builder::new()
        .name("zo-agent-stop-watchdog".to_string())
        .spawn(fallback)
        .is_err()
    {
        // A thread creation failure must not make a cooperative stop wait forever.
        publish_external_stop_fallback(manifest, reason);
    }
}

fn stop_running_agents_in_store_since_inner(
    store: &Path,
    started_after: u64,
    session_id: Option<&str>,
    reason: &str,
    emit_completion: bool,
    allow_unstamped: bool,
) -> usize {
    let Ok(entries) = std::fs::read_dir(store) else {
        return 0;
    };
    let mut stopped = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(manifest) = load_agent_manifest_from_scanned_path(&path) else {
            continue;
        };
        if !agent_output_created_after(&manifest, started_after)
            || !agent_output_belongs_to_session(&manifest, session_id, allow_unstamped)
            || agent_output_status_is_terminal(&manifest.status)
        {
            continue;
        }
        let mut worker_will_notify = false;
        let transitioned = persist_agent_stopped_state_with(&manifest, reason, || {
            // This callback executes only for the caller that atomically claimed
            // the manifest transition, so signal removal cannot race a different
            // stop caller's terminal write.
            worker_will_notify = abort_and_unregister_agent_cancel_signal(
                &manifest.agent_id,
                manifest.run_generation,
            );
            unregister_agent_steering(&manifest.agent_id, manifest.run_generation);
        })
        .unwrap_or(false);
        if transitioned {
            stopped = stopped.saturating_add(1);
        }
        if external_stop_owns_completion(emit_completion, transitioned) {
            if worker_will_notify {
                // Provider streaming can remain blocked after cooperative abort.
                // The first of worker and watchdog publishes; the late one only
                // enriches the completion store without another channel notice.
                schedule_external_stop_fallback(manifest, reason.trim().to_string());
            } else {
                let generation = manifest.run_generation;
                let manifest_file = std::path::PathBuf::from(&manifest.manifest_file);
                notify_agent_completion(
                    stopped_completion(manifest, reason.trim().to_string()),
                    Some(generation),
                    Some(manifest_file),
                );
            }
        }
    }
    stopped
}

fn agent_output_created_after(manifest: &AgentOutput, started_after: u64) -> bool {
    if started_after == 0 {
        return true;
    }
    manifest
        .created_at
        .trim()
        .parse::<u64>()
        .is_ok_and(|created| created >= started_after)
}

/// Single source of truth for "does an agent manifest stamped with
/// The attempt a session is spending RIGHT NOW, as the conversation runtime
/// declared it at turn start — the attempt a spawn made here is serving.
///
/// Read through the same process-wide slot the prompt-cache recorder reads,
/// keyed by the session id the manifest is already stamped with, so nothing
/// new has to be threaded from the host down to a spawn. An unstamped
/// manifest (a bare harness) or a session that declared no attempt yields
/// `None`, and the spawn record simply names no parent — never a guess.
pub(crate) fn served_turn_attempt(parent_session_id: Option<&str>) -> Option<String> {
    let session_id = parent_session_id.map(str::trim).filter(|id| !id.is_empty())?;
    let attempt = api::attempt_for_session(session_id);
    (!attempt.is_empty()).then_some(attempt)
}

/// `parent_session_id` belong to the session identified by `session_id`?".
///
/// Both display surfaces filter on this one rule so it can never drift:
/// - the `tools` stop paths via `agent_output_belongs_to_session` (typed
///   `AgentOutput`);
/// - the CLI HUD/detail/workflow surfaces via
///   `zo_cli::tui::agent_session_filter::manifest_belongs_to_session`
///   (untyped `serde_json::Value`), which extracts `parentSessionId` and
///   delegates here.
///
/// Rule (allocation-free; ids are trimmed and empty is treated as absent):
/// - `session_id` absent/blank → `true` (the caller opted out of scoping);
/// - manifest stamped → `true` only on an exact id match (a foreign session is
///   hidden);
/// - manifest unstamped (no/blank `parent_session_id`) → `allow_unstamped`, so
///   a strict caller (`false`) hides legacy/benchmark agents that never carried
///   a session id, while a back-compat caller (`true`) keeps them.
#[must_use]
pub fn parent_session_belongs(
    parent_session_id: Option<&str>,
    session_id: Option<&str>,
    allow_unstamped: bool,
) -> bool {
    let Some(expected) = session_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return true;
    };
    match parent_session_id.map(str::trim).filter(|id| !id.is_empty()) {
        Some(actual) => actual == expected,
        None => allow_unstamped,
    }
}

fn agent_output_belongs_to_session(
    manifest: &AgentOutput,
    session_id: Option<&str>,
    allow_unstamped: bool,
) -> bool {
    // Thin typed adapter over the shared core: extract this manifest's stamp and
    // apply the one rule. Session-close cleanup passes `allow_unstamped = false`
    // so closing one chat cannot kill another session's legacy/unstamped agent.
    parent_session_belongs(
        manifest.parent_session_id.as_deref(),
        session_id,
        allow_unstamped,
    )
}

/// Best-effort live brief for a non-terminal agent, read from its manifest:
/// wait/stream phase, current tool, last few tool calls, and seconds since the
/// last liveness signal. Attached to `still_running` tool results so the model
/// (and the transcript) can tell a quota-parked agent from a wedged one.
pub(crate) fn agent_live_brief(manifest_file: &str) -> Option<serde_json::Value> {
    let manifest = load_agent_manifest_from_scanned_path(Path::new(manifest_file)).ok()?;
    let mut brief = serde_json::Map::new();
    if let Some(phase) = manifest
        .current_phase
        .as_deref()
        .map(str::trim)
        .filter(|phase| !phase.is_empty())
    {
        brief.insert("phase".to_string(), serde_json::Value::from(phase));
    }
    if let Some(tool) = manifest.current_tool.as_deref() {
        brief.insert("currentTool".to_string(), serde_json::Value::from(tool));
    }
    if !manifest.recent_tools.is_empty() {
        let recent: Vec<&str> = manifest
            .recent_tools
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(String::as_str)
            .collect();
        brief.insert("recentTools".to_string(), serde_json::json!(recent));
    }
    if let Some(at) = manifest.last_activity_at {
        brief.insert(
            "secondsSinceLastActivity".to_string(),
            serde_json::Value::from(manifest::epoch_seconds_now_u64().saturating_sub(at)),
        );
    }
    if !manifest.activity.is_empty() {
        if let Ok(activity) = serde_json::to_value(&manifest.activity) {
            brief.insert("activity".to_string(), activity);
        }
    }
    (!brief.is_empty()).then_some(serde_json::Value::Object(brief))
}

/// [`agent_live_brief`] keyed by agent id (resolves the manifest path under
/// the store dir) — for `SpawnMultiAgent` aggregation, which only has ids.
pub(crate) fn agent_live_brief_by_id(
    registry: &AgentRegistry,
    agent_id: &str,
) -> Option<serde_json::Value> {
    let path = registry.manifest_path(agent_id)?;
    agent_live_brief(path.to_str()?)
}

/// Read an agent's salvageable partial output (`outputTail`) from its manifest
/// file. Used by timeout cleanup to return partial work without keeping stale
/// cancel handles alive.
fn agent_partial_result(manifest_file: &str) -> Option<String> {
    let manifest = load_agent_manifest_from_scanned_path(Path::new(manifest_file)).ok()?;
    let tail = manifest.output_tail.trim();
    (!tail.is_empty()).then(|| tail.to_string())
}

/// Cancel a spawned agent that overran the collection window, persist a best-effort
/// terminal `stopped` manifest, and salvage its streamed `outputTail` as a
/// partial result. The cancel is cooperative — the worker may also observe the
/// abort later and try to unregister/persist, so every operation here is
/// idempotent and best-effort.
pub(crate) fn cancel_and_salvage_agent(
    registry: &AgentRegistry,
    agent_id: &str,
    reason: &str,
) -> Option<String> {
    let store = registry.store_of(agent_id)?;
    cancel_and_salvage_agent_in_with_completion(&store, agent_id, reason, true, false, None)
}

/// Cancel + salvage like [`cancel_and_salvage_agent`], but **leave the cancel
/// signal registered** so [`agent_worker_is_live`] keeps reporting the worker
/// live until the worker thread itself observes the abort and unregisters at its
/// physical exit.
///
/// The fan-out rolling scheduler needs this: cooperative cancel writes a terminal
/// `stopped` manifest immediately, but the worker may only observe the abort
/// later (mid tool call). Unregistering the signal here — as the default salvage
/// does — would make that synthetic terminal masquerade as a *joined* worker, so
/// the scheduler would reuse the slot (breaking `live workers <= window`) and
/// collect/drop the worktree while the physical worker is still writing to it.
/// Keeping the signal registered lets the scheduler gate slot reuse and worktree
/// teardown on the real physical-exit ack.
pub(crate) fn cancel_and_salvage_agent_keep_worker_registered(
    registry: &AgentRegistry,
    agent_id: &str,
    reason: &str,
) -> Option<String> {
    let store = registry.store_of(agent_id)?;
    cancel_and_salvage_agent_in_with_completion(&store, agent_id, reason, true, true, None)
}

pub(crate) fn cancel_and_salvage_agent_attempt(
    registry: &AgentRegistry,
    agent_id: &str,
    generation: u64,
    reason: &str,
) -> Option<String> {
    let store = registry.store_of(agent_id)?;
    cancel_and_salvage_agent_in_with_completion(
        &store, agent_id, reason, true, true, Some(generation),
    )
}

#[cfg(test)]
fn cancel_and_salvage_agent_in(store: &Path, agent_id: &str, reason: &str) -> Option<String> {
    cancel_and_salvage_agent_in_with_completion(store, agent_id, reason, false, false, None)
}

fn cancel_and_salvage_agent_in_with_completion(
    store: &Path,
    agent_id: &str,
    reason: &str,
    emit_completion: bool,
    keep_worker_registered: bool,
    expected_generation: Option<u64>,
) -> Option<String> {
    let path = store.join(format!("{agent_id}.json"));
    let manifest = load_agent_manifest_from_scanned_path(&path).ok()?;
    if expected_generation.is_some_and(|generation| generation != manifest.run_generation) {
        return None;
    }
    // The agent already finished — it won the race against this salvage. Leave its
    // manifest untouched and return its REAL output, so a mundane near-deadline
    // completion is not silently downgraded to a truncated `timed_out` result.
    if agent_output_status_is_terminal(&manifest.status) {
        let file_output = read_agent_output(&manifest)
            .ok()
            .map(|body| body.trim().to_string())
            .filter(|body| !body.is_empty());
        let salvaged_tail = || agent_partial_result(&manifest.manifest_file);
        // A COMPLETED agent's full final response is written to `output_file`
        // (the `### Final response` section), so return that — not the
        // length-capped rolling tail. A STOPPED/FAILED agent's `output_file` holds
        // only status boilerplate (`- status: stopped`); its real partial work
        // lives only in the streamed `output_tail`, so salvage the tail there and
        // fall back to the file only when the tail is empty.
        return if manifest.status == "completed" {
            file_output.or_else(salvaged_tail)
        } else {
            salvaged_tail().or(file_output)
        };
    }
    let mut worker_will_notify = false;
    let transitioned = persist_agent_stopped_state_with(&manifest, reason, || {
        // Both paths abort the cooperative-cancel signal so the worker unwinds.
        // The difference is registry lifetime: the fan-out scheduler keeps the
        // entry so `agent_worker_is_live` stays true until the worker physically
        // exits; every other caller unregisters here because it has no live
        // worker to wait on. `worker_will_notify` (true when a matching-generation
        // signal was found and aborted) is unchanged by that choice.
        worker_will_notify = if keep_worker_registered {
            abort_agent_cancel_signal(&manifest.agent_id, manifest.run_generation)
        } else {
            abort_and_unregister_agent_cancel_signal(
                &manifest.agent_id,
                manifest.run_generation,
            )
        };
        unregister_agent_steering(&manifest.agent_id, manifest.run_generation);
    })
    .unwrap_or(false);
    if external_stop_owns_completion(emit_completion, transitioned) {
        if worker_will_notify {
            schedule_external_stop_fallback(manifest.clone(), reason.trim().to_string());
        } else {
            notify_agent_completion(
                stopped_completion(manifest.clone(), reason.trim().to_string()),
                Some(manifest.run_generation),
                Some(std::path::PathBuf::from(&manifest.manifest_file)),
            );
        }
    }
    agent_partial_result(&manifest.manifest_file)
}

pub(super) fn agent_output_status_is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "stopped")
}

fn agent_cancel_signals() -> &'static Mutex<HashMap<(String, u64), runtime::HookAbortSignal>> {
    AGENT_CANCEL_SIGNALS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_agent_cancel_signal(
    agent_id: String,
    generation: u64,
    signal: runtime::HookAbortSignal,
) {
    if let Ok(mut signals) = agent_cancel_signals().lock() {
        signals.insert((agent_id, generation), signal);
    }
}

/// Register a fresh cancel signal for `agent_id`, marking its worker live for
/// [`agent_worker_is_live`]. Test-only seam so sibling modules (the fan-out
/// scheduler tests) can simulate a physically-live worker without spawning a
/// real runner.
#[cfg(test)]
pub(crate) fn register_agent_cancel_signal_for_tests(agent_id: &str, generation: u64) {
    register_agent_cancel_signal(
        agent_id.to_string(),
        generation,
        runtime::HookAbortSignal::new(),
    );
}

/// Unregister `agent_id`'s cancel signal, simulating the physical worker exit ack
/// (`agent_worker_is_live` flips to false). Test-only seam paired with
/// [`register_agent_cancel_signal_for_tests`].
#[cfg(test)]
pub(crate) fn unregister_agent_cancel_signal_for_tests(agent_id: &str, generation: u64) {
    unregister_agent_cancel_signal(agent_id, generation);
}

/// Whether `agent_id` has a live in-process worker: its cancel signal is
/// registered at spawn/resume and unregistered when the worker ends.
/// Sub-agents run in-process, so this is the authoritative liveness signal —
/// the HUD uses it to rescue a *running* agent whose manifest has gone quiet
/// through one long tool call (a cold `cargo build` writes nothing for longer
/// than the manifest stale-drop window).
#[must_use]
pub fn agent_worker_is_live(agent_id: &str) -> bool {
    agent_cancel_signals()
        .lock()
        .is_ok_and(|signals| signals.keys().any(|(id, _)| id == agent_id))
}

/// Reconcile a manifest whose current in-process worker disappeared before it
/// could publish a terminal completion.
#[must_use]
pub fn reconcile_dead_agent_worker(registry: &AgentRegistry, agent_id: &str) -> bool {
    const DEAD_WORKER_REASON: &str = "worker died without delivering a result";

    let Some(manifest) = registry.manifest_by_id(agent_id) else {
        return false;
    };
    if manifest.owner_pid != Some(std::process::id())
        || agent_worker_generation_is_live(&manifest.agent_id, manifest.run_generation)
    {
        return false;
    }

    if manifest.status == "running" {
        let Ok(true) = manifest::persist_agent_failed_state_if_running(
            &manifest,
            DEAD_WORKER_REASON.to_string(),
        ) else {
            return false;
        };
        let generation = manifest.run_generation;
        let manifest_file = std::path::PathBuf::from(&manifest.manifest_file);
        completion::notify_agent_completion(
            AgentCompletion {
                agent_id: manifest.agent_id,
                name: manifest.name,
                status: "failed".to_string(),
                result: None,
                structured: None,
                error: Some(DEAD_WORKER_REASON.to_string()),
                run: HelperRun::default(),
            },
            Some(generation),
            Some(manifest_file),
        );
        return true;
    }

    if !matches!(manifest.status.as_str(), "completed" | "failed")
        || completion::agent_completion_is_published(&manifest.agent_id)
    {
        return false;
    }

    let result = (manifest.status == "completed")
        .then(|| manifest::read_agent_output(&manifest).ok())
        .flatten()
        .and_then(|output| manifest::final_response_section(&output));
    let generation = manifest.run_generation;
    let manifest_file = std::path::PathBuf::from(&manifest.manifest_file);
    completion::notify_agent_completion(
        AgentCompletion {
            agent_id: manifest.agent_id,
            name: manifest.name,
            status: manifest.status,
            result,
            structured: None,
            error: manifest.error,
            run: HelperRun::default(),
        },
        Some(generation),
        Some(manifest_file),
    )
}

/// Whether the worker for *this exact* `(agent_id, generation)` is still live.
///
/// The registry keys each generation separately, so a same-id resume coexists
/// with an old generation still winding down. The any-generation
/// [`agent_worker_is_live`] is right for the HUD (rescue *whatever* run is
/// current), but the fan-out worktree cleanup owner must bind teardown to the
/// precise generation whose worktree it holds: this reports true only while that
/// exact generation's signal is registered, so an old worker still editing its
/// worktree keeps its guard even after a same-id resume, and the guard is
/// released only when *that* generation itself exits (unregisters).
#[must_use]
pub(crate) fn agent_worker_generation_is_live(agent_id: &str, generation: u64) -> bool {
    agent_cancel_signals()
        .lock()
        .is_ok_and(|signals| signals.contains_key(&(agent_id.to_string(), generation)))
}

/// Abort an agent's cooperative-cancel signal **without** removing its registry
/// entry, so [`agent_worker_generation_is_live`] keeps reporting that exact
/// generation live until the worker thread observes the abort and unregisters
/// itself at physical exit. Returns true iff that exact key was found and aborted.
fn abort_agent_cancel_signal(agent_id: &str, generation: u64) -> bool {
    let Ok(signals) = agent_cancel_signals().lock() else {
        return false;
    };
    let Some(signal) = signals.get(&(agent_id.to_string(), generation)) else {
        return false;
    };
    signal.abort();
    true
}

fn abort_and_unregister_agent_cancel_signal(agent_id: &str, generation: u64) -> bool {
    let Ok(mut signals) = agent_cancel_signals().lock() else {
        return false;
    };
    let key = (agent_id.to_string(), generation);
    let Some(signal) = signals.get(&key) else {
        return false;
    };
    signal.abort();
    signals.remove(&key);
    true
}

pub(super) fn unregister_agent_cancel_signal(agent_id: &str, generation: u64) {
    if let Ok(mut signals) = agent_cancel_signals().lock() {
        signals.remove(&(agent_id.to_string(), generation));
    }
}

/// Process-global registry of each live agent's mid-turn steering queue. The
/// generation is part of the entry so an old worker cannot tear down a queue
/// installed by a same-id resume.
#[derive(Clone)]
struct RegisteredSteeringQueue {
    generation: u64,
    queue: runtime::SteeringQueue,
}

static AGENT_STEERING_QUEUES: OnceLock<Mutex<HashMap<String, RegisteredSteeringQueue>>> =
    OnceLock::new();

fn agent_steering_queues() -> &'static Mutex<HashMap<String, RegisteredSteeringQueue>> {
    AGENT_STEERING_QUEUES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_agent_steering(agent_id: String, generation: u64, queue: runtime::SteeringQueue) {
    if let Ok(mut queues) = agent_steering_queues().lock() {
        queues.insert(agent_id, RegisteredSteeringQueue { generation, queue });
    }
}

pub(super) fn unregister_agent_steering(agent_id: &str, generation: u64) {
    if let Ok(mut queues) = agent_steering_queues().lock() {
        if queues
            .get(agent_id)
            .is_some_and(|entry| entry.generation == generation)
        {
            queues.remove(agent_id);
        }
    }
}

/// Push `message` onto the newest in-process run for this agent id.
pub(crate) fn steer_agent(agent_id: &str, message: String) -> bool {
    let Ok(queues) = agent_steering_queues().lock() else {
        return false;
    };
    let Some(entry) = queues.get(agent_id) else {
        return false;
    };
    if let Ok(mut pending) = entry.queue.lock() {
        pending.push(message);
        return true;
    }
    false
}

/// Push a mid-run message from a running sub-agent onto the parent host's
/// completion channel (the AGENT→MAIN edge). Parent-side sibling of
/// [`steer_agent`], which carries the MAIN→AGENT direction: both live here
/// because this module owns the process-global agent plumbing.
///
/// Returns `false` when no interactive host is consuming the channel in this
/// process, so the caller can tell the sub-agent its message went nowhere
/// instead of silently dropping it.
pub(crate) fn message_main_from_agent(agent_id: &str, name: &str, text: String) -> bool {
    completion::notify_agent_message(completion::agent_message_notice(agent_id, name, text))
}

/// Tools every non-trivial sub-agent gets on top of its role set.
///
/// `SendMessage` is here for the AGENT→MAIN edge only: a sub-agent's copy is
/// hard-restricted to `to: "main"` in `run_send_message`, so it can push a
/// finding or a blocking question to the parent conversation mid-run but can
/// neither steer a sibling nor resume a terminal agent (the full tool stays
/// custom-list-only). `classifier` is excluded: it answers in one reply with no
/// tool loop, so it has no mid-run to report from.
/// The role policy is shared by tool advertisement and the carried harness.
pub(crate) fn inspection_shell_for_subagent(kind: &str) -> bool {
    ["Explore", "Plan", "deep-research"].contains(&kind)
}

const SUBAGENT_UPSTREAM_TOOLS: &[&str] = &["SendMessage"];

#[allow(clippy::too_many_lines)] // a flat per-role tool table, clearer unsplit
pub(crate) fn allowed_tools_for_subagent(subagent_type: &str) -> BTreeSet<String> {
    let tools = match subagent_type {
        // One-shot classification (fan-out triage / decomposition): answers in
        // a single reply, so it needs no tools beyond the forced
        // StructuredOutput on the Anthropic path.
        "classifier" => vec!["StructuredOutput"],
        "Explore" => vec![
            "read_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "ToolSearch",
            "Skill",
            "StructuredOutput",
        ],
        // Plan and deep-research share a read-only code+web toolset (they differ
        // in harness prompt and model, not tools).
        "Plan" | "deep-research" => vec![
            "read_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "ToolSearch",
            "Skill",
            "TodoWrite",
            "StructuredOutput",
            "SendUserMessage",
        ],
        "Verification" => vec![
            "bash",
            "read_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "ToolSearch",
            "TodoWrite",
            "StructuredOutput",
            "SendUserMessage",
            "PowerShell",
        ],
        // Read-only critique: reads code and runs git/inspection via bash, but
        // does not edit — the review itself is the output.
        "code-reviewer" => vec![
            "bash",
            "read_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "ToolSearch",
            "TodoWrite",
            "StructuredOutput",
            "SendUserMessage",
        ],
        // Reproduce → root-cause → fix: needs the full edit + run toolset, plus
        // `InstrumentLog` for throwaway tracing that auto-reverts at run end and
        // `DebugHypothesis` to track root-cause guesses across iterations.
        "debugger" => vec![
            "bash",
            "read_file",
            "write_file",
            "edit_file",
            "InstrumentLog",
            "DebugHypothesis",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "ToolSearch",
            "TodoWrite",
            "StructuredOutput",
            "SendUserMessage",
            "REPL",
            "PowerShell",
        ],
        // Inspect data/logs and compute: read + bash/REPL, no source edits.
        "data-analyst" => vec![
            "bash",
            "read_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "ToolSearch",
            "TodoWrite",
            "StructuredOutput",
            "SendUserMessage",
            "REPL",
            "NotebookEdit",
        ],
        // Behavior-preserving structural edits: full edit set + run to keep green.
        "refactor" => vec![
            "bash",
            "read_file",
            "write_file",
            "edit_file",
            "glob_search",
            "grep_search",
            "ToolSearch",
            "TodoWrite",
            "StructuredOutput",
            "SendUserMessage",
            "PowerShell",
        ],
        "zo-guide" => vec![
            "read_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "ToolSearch",
            "Skill",
            "StructuredOutput",
            "SendUserMessage",
        ],
        "statusline-setup" => vec![
            "bash",
            "read_file",
            "write_file",
            "edit_file",
            "glob_search",
            "grep_search",
            "ToolSearch",
        ],
        _ => vec![
            "bash",
            "read_file",
            "write_file",
            "edit_file",
            "glob_search",
            "grep_search",
            "WebFetch",
            "WebSearch",
            "TodoWrite",
            "Skill",
            "ToolSearch",
            "NotebookEdit",
            "Sleep",
            "SendUserMessage",
            "Config",
            "StructuredOutput",
            "REPL",
            "PowerShell",
        ],
    };
    let classifier = subagent_type == "classifier";
    tools
        .into_iter()
        .chain(inspection_shell_for_subagent(subagent_type).then_some("bash"))
        .chain(
            SUBAGENT_UPSTREAM_TOOLS
                .iter()
                .copied()
                .filter(|_| !classifier),
        )
        .map(str::to_string)
        .collect()
}

/// Build the permission policy for a spawned sub-agent.
///
/// `mode` is the active permission mode (built-ins pass `DangerFullAccess`, the
/// historical default); `rules` are optional per-agent allow/deny/ask rules
/// from a custom-agent definition. With `mode == DangerFullAccess` and
/// `rules == None` this is byte-identical to the original hard-coded policy, so
/// built-in sub-agents are unaffected.
pub(crate) fn agent_permission_policy(
    mode: PermissionMode,
    rules: Option<&runtime::RuntimePermissionRuleConfig>,
) -> PermissionPolicy {
    let policy = mvp_tool_specs()
        .iter()
        .fold(PermissionPolicy::new(mode), |policy, spec| {
            policy.with_tool_requirement(spec.name, spec.required_permission)
        });
    match rules {
        Some(rules) => policy.with_permission_rules(rules),
        None => policy,
    }
}

pub(crate) fn normalize_subagent_type(subagent_type: Option<&str>) -> String {
    let trimmed = subagent_type.map(str::trim).unwrap_or_default();
    if trimmed.is_empty() {
        return String::from("general-purpose");
    }

    match canonical_tool_token(trimmed).as_str() {
        "general" | "generalpurpose" | "generalpurposeagent" => String::from("general-purpose"),
        "explore" | "explorer" | "exploreagent" => String::from("Explore"),
        "plan" | "planagent" => String::from("Plan"),
        "verification" | "verificationagent" | "verify" | "verifier" => {
            String::from("Verification")
        }
        "zoguide" | "zoguideagent" | "guide" => String::from("zo-guide"),
        "statusline" | "statuslinesetup" => String::from("statusline-setup"),
        "deepresearch" | "research" | "researcher" => String::from("deep-research"),
        "codereviewer" | "codereview" | "reviewer" | "review" => String::from("code-reviewer"),
        "debugger" | "debug" => String::from("debugger"),
        "dataanalyst" | "dataanalysis" | "analyst" => String::from("data-analyst"),
        "refactor" | "refactorer" | "refactoring" => String::from("refactor"),
        _ => trimmed.to_string(),
    }
}

use super::canonical_tool_token;

#[cfg(test)]
mod subagent_image_tests {
    use super::SubagentToolExecutor;
    use runtime::ToolExecutor;
    use std::collections::BTreeSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    // Smallest valid 1x1 PNG (signature + IHDR + IDAT + IEND).
    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn subagent_read_image_stages_and_drains_its_image() {
        // A sub-agent granted read_image must SEE the image: the executor stages
        // it into its own sink and `take_pending_images` (the override) drains it
        // — otherwise the agent_tools image-attach converter is dead code.
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zo-subagent-img-{unique}.png"));
        std::fs::write(&path, PNG_1X1).expect("write fixture");

        let mut allowed = BTreeSet::new();
        allowed.insert("read_image".to_string());
        let mut exec = SubagentToolExecutor::new(allowed);

        let input = serde_json::json!({ "path": path.to_string_lossy() }).to_string();
        let summary = exec.execute("read_image", &input).expect("read_image runs");
        assert!(summary.contains("image/png"), "summary: {summary}");

        let images = exec.take_pending_images();
        assert_eq!(
            images.len(),
            1,
            "sub-agent read_image must surface its image"
        );
        assert_eq!(images[0].0, "image/png");
        // Idempotent drain: a second call is empty (no stale leak).
        assert!(exec.take_pending_images().is_empty());

        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod subagent_lsp_tests {
    use super::{inherited_lsp, LspRegistry, SubagentToolExecutor};
    use runtime::lsp_client::LspServerStatus;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn with_lsp_shares_the_live_parent_registry() {
        // The wiring is only valuable if the sub-agent sees the SAME live servers
        // as the parent, so the edit/write enrich path can surface their
        // diagnostics. Prove it shares the Arc, not a snapshot: a server
        // registered AFTER the share is visible through the executor's context.
        let registry = LspRegistry::new();
        let exec = SubagentToolExecutor::new(BTreeSet::new()).with_lsp(registry.clone());
        assert!(exec.context.lsp.is_empty(), "starts empty");

        registry.register("rust", LspServerStatus::Connected, None, vec![]);
        assert!(
            !exec.context.lsp.is_empty(),
            "sub-agent observes a server registered on the shared parent registry"
        );
    }

    #[test]
    fn a_fresh_subagent_has_no_lsp() {
        // Without `with_lsp` (the isolated / no-parent path) the context LSP is
        // empty, so the enrich gate (`!ctx.lsp.is_empty()`) is correctly skipped.
        let exec = SubagentToolExecutor::new(BTreeSet::new());
        assert!(exec.context.lsp.is_empty());
    }

    #[test]
    fn inherited_lsp_is_shared_only_for_a_non_isolated_agent() {
        let registry = LspRegistry::new();
        // Non-isolated (cwd None): inherits the parent registry.
        assert!(inherited_lsp(None, Some(registry.clone())).is_some());
        // Isolated (cwd Some): must NOT inherit — the parent's servers are rooted
        // in the parent tree and LSP URIs resolve against the process cwd, so an
        // isolated worktree agent would sync/read the wrong tree.
        assert!(inherited_lsp(Some(Path::new("/wt")), Some(registry)).is_none());
        // No parent registry: nothing to share, isolated or not.
        assert!(inherited_lsp(None, None).is_none());
    }
}

#[cfg(test)]
mod subagent_instrument_tests {
    use super::SubagentToolExecutor;
    use runtime::ToolExecutor;
    use std::collections::BTreeSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn subagent_instrument_log_probe_is_reverted_at_run_end() {
        // The debugger sub-agent inserts a probe through the executor, and
        // `revert_probes` (called at run completion in `run_agent_job`) strips it
        // byte-for-byte — proving the executor's context is the very one
        // `InstrumentLog` wrote to, so the auto-revert wiring is not dead code.
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zo-g23-sub-{unique}.rs"));
        let original = "fn f() {\n    work();\n}\n";
        std::fs::write(&path, original).expect("write fixture");

        let mut allowed = BTreeSet::new();
        allowed.insert("InstrumentLog".to_string());
        let mut exec = SubagentToolExecutor::new(allowed);

        let input = serde_json::json!({
            "path": path.to_string_lossy(),
            "anchor": "work();",
            "statement": "eprintln!(\"reached f\");",
        })
        .to_string();
        let out = exec
            .execute("InstrumentLog", &input)
            .expect("InstrumentLog runs for an allowed sub-agent");
        assert!(out.contains("ZO_PROBE"), "marker reported: {out}");
        assert!(
            std::fs::read_to_string(&path)
                .expect("read")
                .contains("ZO_PROBE"),
            "probe inserted into the file",
        );

        assert_eq!(exec.revert_probes(), 1, "one probe reverted at run end");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            original,
            "file restored byte-for-byte",
        );
        // Idempotent: a second revert finds an empty ledger.
        assert_eq!(exec.revert_probes(), 0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn instrument_log_is_denied_for_a_subagent_without_it() {
        // A sub-agent whose allow-list omits InstrumentLog cannot call it, so the
        // probe path is unreachable outside the debugger.
        let mut exec = SubagentToolExecutor::new(BTreeSet::new());
        let err = exec
            .execute("InstrumentLog", "{}")
            .expect_err("InstrumentLog must be gated by the allow-list");
        assert!(err.to_string().contains("not enabled"), "reason: {err}");
    }
}

#[cfg(test)]
mod agent_manifest_tests {
    use super::manifest::{
        append_agent_output_tail, record_agent_phase, trim_agent_output_tail_suffix,
        OUTPUT_TAIL_CAP,
    };
    use super::{
        abort_agent_cancel_signal, abort_and_unregister_agent_cancel_signal, agent_live_brief,
        cancel_and_salvage_agent_in, clear_background_agent, display_agent_label,
        ensure_agent_store_writable, is_background_agent,
        persist_agent_terminal_state_with_history, record_current_tool,
        register_agent_cancel_signal, register_agent_steering, resume_agent_with_spawn,
        steer_agent, stop_agent_for_session_in,
        stop_running_agents_in_store_since_for_strict_session_without_notify,
        unregister_agent_cancel_signal, unregister_agent_steering, AgentJob, AgentOutput,
        AgentStopOutcome,
    };
    use crate::ToolError;
    use serde_json::json;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zo-agent-manifest-{}-{tag}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_agent_for_stop_test(
        dir: &std::path::Path,
        id: &str,
        session_id: Option<&str>,
        status: &str,
        created_at: &str,
    ) -> std::path::PathBuf {
        let manifest_path = dir.join(format!("{id}.json"));
        let output_path = dir.join(format!("{id}.md"));
        let mut manifest = json!({
            "agentId": id,
            "name": id,
            "description": "test agent",
            "subagentType": null,
            "model": null,
            "status": status,
            "outputFile": output_path,
            "manifestFile": manifest_path,
            "createdAt": created_at,
            "startedAt": created_at
        });
        if let Some(session_id) = session_id {
            manifest["parentSessionId"] = json!(session_id);
        }
        std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap())
            .expect("write manifest");
        std::fs::write(&output_path, "").expect("write output file");
        manifest_path
    }

    fn manifest_status(path: &std::path::Path) -> String {
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(path).unwrap())
            .unwrap()
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap()
            .to_string()
    }

    #[test]
    fn strict_session_stop_only_stops_current_stamped_running_agents() {
        let dir = temp_dir("strict-session-stop");
        let current =
            write_agent_for_stop_test(&dir, "current", Some("session-a"), "running", "200");
        let foreign =
            write_agent_for_stop_test(&dir, "foreign", Some("session-b"), "running", "210");
        let legacy = write_agent_for_stop_test(&dir, "legacy", None, "running", "220");
        let old = write_agent_for_stop_test(&dir, "old", Some("session-a"), "running", "100");
        let done = write_agent_for_stop_test(&dir, "done", Some("session-a"), "completed", "230");

        let stopped = stop_running_agents_in_store_since_for_strict_session_without_notify(
            &dir,
            150,
            "session-a",
            "parent session closed",
        );

        assert_eq!(stopped, 1);
        assert_eq!(manifest_status(&current), "stopped");
        assert_eq!(manifest_status(&foreign), "running");
        assert_eq!(manifest_status(&legacy), "running");
        assert_eq!(manifest_status(&old), "running");
        assert_eq!(manifest_status(&done), "completed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn single_agent_stop_requires_session_process_and_live_generation_ownership() {
        let dir = temp_dir("single-owned-stop");
        let owned_id = format!("owned-{}", std::process::id());
        let foreign_id = format!("foreign-{}", std::process::id());
        let unreachable_id = format!("unreachable-{}", std::process::id());
        for (id, session, owner) in [
            (owned_id.as_str(), "session-a", std::process::id()),
            (foreign_id.as_str(), "session-b", std::process::id()),
            (unreachable_id.as_str(), "session-a", std::process::id()),
        ] {
            let path = write_agent_for_stop_test(&dir, id, Some(session), "running", "200");
            let mut manifest: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&path).expect("read manifest"),
            )
            .expect("parse manifest");
            manifest["ownerPid"] = json!(owner);
            manifest["runGeneration"] = json!(3);
            std::fs::write(path, serde_json::to_vec(&manifest).expect("serialize manifest"))
                .expect("stamp ownership");
        }
        register_agent_cancel_signal(
            owned_id.clone(),
            3,
            runtime::HookAbortSignal::new(),
        );

        assert!(matches!(
            stop_agent_for_session_in(
                &dir,
                &foreign_id,
                "session-a",
                "stopped by user",
                false,
            ),
            AgentStopOutcome::NotOwned { .. }
        ));
        assert!(matches!(
            stop_agent_for_session_in(
                &dir,
                &unreachable_id,
                "session-a",
                "stopped by user",
                false,
            ),
            AgentStopOutcome::Unreachable { .. }
        ));
        assert_eq!(
            stop_agent_for_session_in(
                &dir,
                &owned_id,
                "session-a",
                "stopped by user",
                false,
            ),
            AgentStopOutcome::Stopped {
                name: owned_id.clone(),
            }
        );
        assert_eq!(
            manifest_status(&dir.join(format!("{owned_id}.json"))),
            "stopped"
        );
        assert!(!super::agent_worker_generation_is_live(&owned_id, 3));
        assert_eq!(manifest_status(&dir.join(format!("{foreign_id}.json"))), "running");
        assert_eq!(
            manifest_status(&dir.join(format!("{unreachable_id}.json"))),
            "running"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn abort_and_unregister_removes_stale_cancel_signal() {
        let agent_id = format!(
            "agent-cleanup-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        );
        register_agent_cancel_signal(agent_id.clone(), 0, runtime::HookAbortSignal::new());

        assert!(abort_and_unregister_agent_cancel_signal(&agent_id, 0));
        assert!(!abort_agent_cancel_signal(&agent_id, 0));
        assert!(!abort_and_unregister_agent_cancel_signal(&agent_id, 0));
    }

    #[test]
    fn steer_agent_delivers_only_while_registered() {
        let agent_id = format!(
            "agent-steer-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        );
        // Nothing registered yet — a send must report non-delivery.
        assert!(!steer_agent(&agent_id, "early".to_string()));

        let queue = runtime::SteeringQueue::default();
        register_agent_steering(agent_id.clone(), 0, queue.clone());
        assert!(steer_agent(&agent_id, "focus on auth".to_string()));
        assert_eq!(
            queue.lock().expect("queue").clone(),
            vec!["focus on auth".to_string()],
            "the message lands in the exact queue the runtime drains"
        );

        unregister_agent_steering(&agent_id, 0);
        assert!(!steer_agent(&agent_id, "too late".to_string()));
        assert_eq!(
            queue.lock().expect("queue").len(),
            1,
            "no delivery after the agent released its queue"
        );
    }

    fn seed_terminal_agent(dir: &Path, agent_id: &str, with_transcript: bool) -> AgentOutput {
        let manifest_path = dir.join(format!("{agent_id}.json"));
        let output_path = dir.join(format!("{agent_id}.md"));
        let manifest_json = json!({
            "agentId": agent_id,
            "name": agent_id,
            "description": "explore the auth module",
            "subagentType": "Explore",
            "model": null,
            "status": "completed",
            "outputFile": output_path,
            "manifestFile": manifest_path,
            "createdAt": "100",
            "startedAt": "100",
            "completedAt": "200",
        });
        std::fs::write(&manifest_path, serde_json::to_string(&manifest_json).unwrap()).unwrap();
        std::fs::write(&output_path, "# Agent Task\n").unwrap();
        if with_transcript {
            let mut session = runtime::Session::new();
            session
                .push_user_text("original task: map the auth module")
                .expect("seed transcript message");
            session
                .save_to_path(dir.join(format!("{agent_id}.session.jsonl")))
                .expect("persist transcript");
        }
        serde_json::from_str::<AgentOutput>(
            &std::fs::read_to_string(&manifest_path).expect("read seeded manifest"),
        )
        .expect("parse seeded manifest")
    }

    #[test]
    fn resume_requires_a_persisted_transcript() {
        let dir = temp_dir("resume-no-transcript");
        let manifest = seed_terminal_agent(&dir, "agent-resume-bare", false);

        let error = resume_agent_with_spawn(
            None,
            &manifest,
            "dig deeper",
            None,
            None,
            None,
            None,
            false,
            |_job: AgentJob| -> Result<(), ToolError> { unreachable!("must not spawn") },
        )
        .expect_err("a transcript-less agent cannot be resumed");
        assert!(
            error.to_string().contains("no persisted transcript"),
            "unexpected error: {error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_rehydrates_and_respawns_the_same_agent_in_background() {
        let agent_id = format!(
            "agent-resume-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        );
        let dir = temp_dir("resume-happy");
        let manifest = seed_terminal_agent(&dir, &agent_id, true);

        let captured: std::sync::Arc<Mutex<Option<AgentJob>>> = std::sync::Arc::default();
        let capture = std::sync::Arc::clone(&captured);
        let resumed = resume_agent_with_spawn(
            None,
            &manifest,
            "now check the token refresh path too",
            None,
            None,
            None,
            None,
            false,
            move |job: AgentJob| {
                *capture.lock().expect("capture slot") = Some(job);
                Ok(())
            },
        )
        .expect("resume spawns a continuation job")
        .manifest;

        let job = captured
            .lock()
            .expect("capture slot")
            .take()
            .expect("spawn_fn received the job");
        assert!(job.resume, "the job must rehydrate, not start fresh");
        assert_eq!(job.prompt, "now check the token refresh path too");
        let expected_transcript = std::fs::canonicalize(
            dir.join(format!("{agent_id}.session.jsonl")),
        )
        .expect("canonical transcript path");
        assert_eq!(
            job.transcript_path.as_deref(),
            Some(expected_transcript.as_path())
        );
        assert!(job.time_budget.is_none(), "no inherited wall-clock budget");
        assert!(job.judged_agent.is_none(), "follow-ups never re-credit verdicts");
        assert!(
            !job.system_prompt.is_empty(),
            "the harness system prompt is re-derived from the subagent type"
        );

        assert_eq!(resumed.status, "running");
        assert!(
            is_background_agent(&resumed.agent_id),
            "the reply must ride the background re-injection channel"
        );
        let on_disk: AgentOutput = serde_json::from_str(
            &std::fs::read_to_string(&manifest.manifest_file).expect("reread manifest"),
        )
        .expect("parse manifest");
        assert_eq!(on_disk.status, "running");
        assert!(on_disk.completed_at.is_none());
        let output = std::fs::read_to_string(&manifest.output_file).expect("read output");
        assert!(
            output.contains("## Follow-up (resumed)")
                && output.contains("now check the token refresh path too"),
            "the output file records the follow-up: {output}"
        );

        clear_background_agent(&resumed.agent_id);
        unregister_agent_cancel_signal(&resumed.agent_id, resumed.run_generation);
        unregister_agent_steering(&resumed.agent_id, resumed.run_generation);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workflow_attempt_cancel_preserves_registration_and_rejects_new_generation() {
        use super::{
            agent_worker_generation_is_live, cancel_and_salvage_agent_in_with_completion,
            register_agent_cancel_signal_for_tests, unregister_agent_cancel_signal_for_tests,
        };
        use serde_json::Value;

        let store = tempfile::tempdir().unwrap();
        let id = "workflow-pinned-cancel";
        let path = store.path().join(format!("{id}.json"));
        let output = store.path().join(format!("{id}.md"));
        std::fs::write(&output, "").unwrap();
        std::fs::write(&path, serde_json::to_vec(&json!({
            "agentId": id, "name": id, "description": "fixture", "status": "running",
            "subagentType": "general-purpose", "model": "fixture",
            "runGeneration": 2, "outputFile": output, "manifestFile": path,
            "createdAt": "1", "startedAt": "1", "outputTail": "partial"
        })).unwrap()).unwrap();
        register_agent_cancel_signal_for_tests(id, 2);
        let stale = cancel_and_salvage_agent_in_with_completion(
            store.path(), id, "old stop", false, true, Some(1),
        );
        assert!(stale.is_none());
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["status"], "running");
        let current = cancel_and_salvage_agent_in_with_completion(
            store.path(), id, "owned stop", false, true, Some(2),
        );
        assert_eq!(current.as_deref(), Some("partial"));
        assert!(agent_worker_generation_is_live(id, 2));
        unregister_agent_cancel_signal_for_tests(id, 2);
        assert!(!agent_worker_generation_is_live(id, 2));
    }

    #[test]
    fn cancel_and_salvage_persists_stopped_manifest() {
        let dir = temp_dir("cancel-salvage");
        let manifest_path = dir.join("agent-timeout.json");
        let output_path = dir.join("agent-timeout.md");
        let manifest = json!({
            "agentId": "agent-timeout",
            "name": "agent-timeout",
            "description": "test agent",
            "subagentType": null,
            "model": null,
            "status": "running",
            "outputFile": output_path,
            "manifestFile": manifest_path,
            "createdAt": "2026-01-01T00:00:00Z",
            "startedAt": "2026-01-01T00:00:00Z",
            "outputTail": "partial answer"
        });
        std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap()).unwrap();
        std::fs::write(&output_path, "").unwrap();

        let partial = cancel_and_salvage_agent_in(&dir, "agent-timeout", "test timeout");

        assert_eq!(partial.as_deref(), Some("partial answer"));
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(saved["status"], "stopped");
        assert_eq!(saved["error"], serde_json::Value::Null);
        assert!(
            std::fs::read_to_string(&output_path)
                .unwrap()
                .contains("test timeout")
        );
    }

    #[test]
    fn display_agent_label_preserves_readable_name_or_description() {
        assert_eq!(
            display_agent_label(
                Some("Find architecture SRP"),
                "ignored description",
                "find-architecture-srp",
                "general-purpose",
            ),
            Some("Find architecture SRP".to_string())
        );
        assert_eq!(
            display_agent_label(
                None,
                "review auth boundary",
                "review-auth-boundary",
                "general-purpose",
            ),
            Some("review auth boundary".to_string())
        );
        assert_eq!(
            display_agent_label(
                Some("find-architecture-srp"),
                "ignored",
                "find-architecture-srp",
                "general-purpose",
            ),
            None,
            "a label identical to the slug is redundant"
        );
    }

    #[test]
    fn display_agent_label_prefixes_specialized_type() {
        assert_eq!(
            display_agent_label(
                Some("Fix parser"),
                "ignored",
                "fix-parser",
                "debugger",
            ),
            Some("debugger·Fix parser".to_string())
        );
    }

    #[test]
    fn current_tool_stamp_increments_tool_calls() {
        let dir = temp_dir("tool-calls");
        let manifest_path = dir.join("agent-1.json");
        let output_path = dir.join("agent-1.md");
        let manifest = json!({
            "agentId": "agent-1",
            "name": "agent-1",
            "description": "test agent",
            "subagentType": null,
            "model": "claude-opus-4-8",
            "status": "running",
            "outputFile": output_path,
            "manifestFile": manifest_path,
            "createdAt": "100"
        });
        std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap())
            .expect("write manifest");

        record_current_tool(&manifest_path, "Read", r#"{"file_path":"src/main.rs"}"#);
        record_current_tool(&manifest_path, "Grep", r#"{"pattern":"fn main"}"#);

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(value["currentTool"], "Grep");
        assert_eq!(value["toolCalls"], 2);
        // The rolling activity feed carries tool + argument brief in order.
        assert_eq!(value["recentTools"][0], "Read \u{00b7} src/main.rs");
        assert_eq!(value["recentTools"][1], "Grep \u{00b7} fn main");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_tools_feed_caps_its_length() {
        let dir = temp_dir("recent-cap");
        let manifest_path = dir.join("agent-1.json");
        let output_path = dir.join("agent-1.md");
        let manifest = json!({
            "agentId": "agent-1",
            "name": "agent-1",
            "description": "test agent",
            "subagentType": null,
            "model": "claude-opus-4-8",
            "status": "running",
            "outputFile": output_path,
            "manifestFile": manifest_path,
            "createdAt": "100"
        });
        std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap())
            .expect("write manifest");

        for i in 0..20 {
            record_current_tool(
                &manifest_path,
                "Bash",
                &format!(r#"{{"command":"step {i}"}}"#),
            );
        }
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        let feed = value["recentTools"].as_array().expect("feed array");
        assert_eq!(feed.len(), super::manifest::RECENT_TOOLS_CAP);
        assert_eq!(
            feed.last().and_then(serde_json::Value::as_str),
            Some("Bash \u{00b7} step 19"),
            "feed keeps the newest entries"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn phase_stamp_round_trips_and_tool_start_clears_it() {
        let dir = temp_dir("phase");
        let manifest_path = dir.join("agent-1.json");
        let manifest = json!({
            "agentId": "agent-1",
            "name": "agent-1",
            "description": "test agent",
            "subagentType": null,
            "model": "claude-opus-4-8",
            "status": "running",
            "outputFile": dir.join("agent-1.md"),
            "manifestFile": manifest_path,
            "createdAt": "100"
        });
        std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap())
            .expect("write manifest");

        record_agent_phase(
            &manifest_path,
            Some("rate-limited \u{00b7} resumes in ~90s"),
        );
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(
            value["currentPhase"],
            "rate-limited \u{00b7} resumes in ~90s"
        );
        assert!(
            value["lastActivityAt"].as_u64().is_some_and(|at| at > 0),
            "phase stamp must bump the heartbeat: {value:?}"
        );

        // A starting tool proves the agent is past waiting — phase clears.
        record_current_tool(&manifest_path, "Read", r#"{"file_path":"src/main.rs"}"#);
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert!(
            value.get("currentPhase").is_none(),
            "tool start must clear the wait phase: {value:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_tail_appends_and_stays_capped() {
        let dir = temp_dir("tail");
        let manifest_path = dir.join("agent-1.json");
        let manifest = json!({
            "agentId": "agent-1",
            "name": "agent-1",
            "description": "test agent",
            "subagentType": null,
            "model": "claude-opus-4-8",
            "status": "running",
            "outputFile": dir.join("agent-1.md"),
            "manifestFile": manifest_path,
            "createdAt": "100"
        });
        std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap())
            .expect("write manifest");

        append_agent_output_tail(&manifest_path, "분석 시작 — ");
        append_agent_output_tail(&manifest_path, "core-types 의존 그래프 확인");
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(
            value["outputTail"],
            "분석 시작 — core-types 의존 그래프 확인"
        );

        // Overflow keeps only the newest OUTPUT_TAIL_CAP chars (char-boundary
        // safe for multibyte text).
        let flood = "가".repeat(OUTPUT_TAIL_CAP + 137);
        append_agent_output_tail(&manifest_path, &flood);
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        let tail = value["outputTail"].as_str().expect("tail string");
        assert_eq!(tail.chars().count(), OUTPUT_TAIL_CAP);
        assert!(tail.chars().all(|c| c == '가'), "oldest text evicted");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_tail_suffix_trim_is_conservative() {
        let dir = temp_dir("tail-trim");
        let manifest_path = dir.join("agent-1.json");
        let manifest = json!({
            "agentId": "agent-1",
            "name": "agent-1",
            "description": "test agent",
            "subagentType": null,
            "model": "claude-opus-4-8",
            "status": "running",
            "outputFile": dir.join("agent-1.md"),
            "manifestFile": manifest_path,
            "createdAt": "100"
        });
        std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap())
            .expect("write manifest");

        append_agent_output_tail(&manifest_path, "stable prefix");
        append_agent_output_tail(&manifest_path, " retry attempt");
        trim_agent_output_tail_suffix(&manifest_path, "not the suffix");
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(value["outputTail"], "stable prefix retry attempt");

        trim_agent_output_tail_suffix(&manifest_path, " retry attempt");
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(value["outputTail"], "stable prefix");

        trim_agent_output_tail_suffix(&manifest_path, "stable prefix");
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert!(
            value.get("outputTail").is_none(),
            "empty output tail should serialize as absent: {value:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_brief_reports_phase_tool_and_heartbeat() {
        let dir = temp_dir("brief");
        let manifest_path = dir.join("agent-1.json");
        let manifest = json!({
            "agentId": "agent-1",
            "name": "agent-1",
            "description": "test agent",
            "subagentType": null,
            "model": "claude-opus-4-8",
            "status": "running",
            "outputFile": dir.join("agent-1.md"),
            "manifestFile": manifest_path,
            "createdAt": "100"
        });
        std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap())
            .expect("write manifest");
        record_current_tool(&manifest_path, "Bash", r#"{"command":"cargo test"}"#);
        record_agent_phase(&manifest_path, Some("waiting for api slot"));

        let brief = agent_live_brief(manifest_path.to_str().unwrap())
            .expect("running agent yields a live brief");
        assert_eq!(brief["phase"], "waiting for api slot");
        assert_eq!(brief["currentTool"], "Bash");
        assert_eq!(brief["recentTools"][0], "Bash \u{00b7} cargo test");
        assert!(
            brief["secondsSinceLastActivity"].as_u64().is_some(),
            "heartbeat age must be present: {brief:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn terminal_state_preserves_tool_calls_from_live_manifest() {
        let dir = temp_dir("terminal");
        let manifest_path = dir.join("agent-1.json");
        let output_path = dir.join("agent-1.md");
        std::fs::write(&output_path, "").expect("write output");
        let manifest = json!({
            "agentId": "agent-1",
            "name": "agent-1",
            "description": "test agent",
            "subagentType": null,
            "model": "claude-opus-4-8",
            "status": "running",
            "outputFile": output_path,
            "manifestFile": manifest_path,
            "createdAt": "100"
        });
        let initial: AgentOutput = serde_json::from_value(manifest.clone()).unwrap();
        std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap())
            .expect("write manifest");

        record_current_tool(&manifest_path, "Read", "{}");
        persist_agent_terminal_state_with_history(
            &initial,
            "completed",
            Some("done"),
            None,
            Vec::new(),
        )
        .expect("persist terminal state");

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(value["status"], "completed");
        assert_eq!(value["toolCalls"], 1);
        assert!(
            value.get("currentTool").is_none(),
            "terminal state must clear live currentTool"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_agent_store_writable_creates_a_fresh_dir() {
        // The happy path: a not-yet-existing store dir under a writable parent
        // is created (recursively) without error — the normal spawn precondition.
        let dir = temp_dir("agent-store-mk").join("nested").join("agents");
        assert!(!dir.exists());
        ensure_agent_store_writable(&dir).expect("fresh store dir must be created");
        assert!(dir.is_dir(), "store dir should exist after ensure");
        let _ = std::fs::remove_dir_all(temp_dir("agent-store-mk"));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_agent_store_writable_maps_eacces_to_actionable_error() {
        use std::os::unix::fs::PermissionsExt;
        let base = temp_dir("agent-store-ro");
        let ro_parent = base.join("ro");
        std::fs::create_dir_all(&ro_parent).expect("mk ro parent");
        std::fs::set_permissions(&ro_parent, std::fs::Permissions::from_mode(0o555))
            .expect("chmod ro");

        // Skip when this uid can write to a 0555 dir anyway (e.g. running as
        // root): the EACCES we are reproducing cannot occur.
        if std::fs::create_dir(ro_parent.join(".probe")).is_ok() {
            let _ = std::fs::set_permissions(&ro_parent, std::fs::Permissions::from_mode(0o755));
            let _ = std::fs::remove_dir_all(&base);
            return;
        }

        // A store dir *under* the read-only parent cannot be created → EACCES.
        let store = ro_parent.join("agents");
        let error = ensure_agent_store_writable(&store).expect_err("read-only parent must error");

        // The fix: the user no longer sees a bare `io error: Permission denied
        // (os error 13)`. It is a clean Execution error that names the override
        // knobs that fix it — not a `ToolError::Io`.
        assert!(
            matches!(error, ToolError::Execution(_)),
            "EACCES must map to an actionable Execution error, got {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("ZO_AGENT_STORE") && message.contains("not writable"),
            "message must name the fix: {message}"
        );

        let _ = std::fs::set_permissions(&ro_parent, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&base);
    }
}

pub(crate) use runtime::final_assistant_text;

/// Capture a successful, unambiguous `StructuredOutput` call whose JSON input
/// satisfies the exact schema attached to the agent job when that schema uses
/// the locally supported subset. Unsupported schema vocabularies preserve the
/// captured value without local validation. Model-controlled tool ids are
/// untrusted: duplicate ids, duplicate/mismatched results, errors, and bad JSON
/// all fail closed.
pub(crate) fn final_structured_output(
    summary: &runtime::TurnSummary,
    schema: &serde_json::Value,
) -> Option<serde_json::Value> {
    let uses: Vec<(&str, &str)> = summary
        .assistant_messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } if name == "StructuredOutput" => {
                Some((id.as_str(), input.as_str()))
            }
            _ => None,
        })
        .collect();
    let [(id, input)] = uses.as_slice() else {
        return None;
    };
    let structured_results: Vec<&ContentBlock> = summary
        .tool_results
        .iter()
        .flat_map(|message| &message.blocks)
        .filter(|block| matches!(block, ContentBlock::ToolResult { tool_name, .. } if tool_name == "StructuredOutput"))
        .collect();
    if !matches!(
        structured_results.as_slice(),
        [ContentBlock::ToolResult { tool_use_id, tool_name, is_error: false, .. }]
            if tool_use_id == id && tool_name == "StructuredOutput"
    ) {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(input).ok()?;
    if structured_schema_is_supported(schema) {
        structured_schema_valid(schema, &value).then_some(value)
    } else {
        Some(value)
    }
}

const STRUCTURED_SCHEMA_KEYWORDS: &[&str] = &[
    "$schema",
    "title",
    "description",
    "default",
    "examples",
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "const",
    "minItems",
    "maxItems",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minProperties",
    "maxProperties",
];

/// Deliberately small JSON Schema validator for the subset this feature has
/// historically emitted: object/array/scalar types, object properties and
/// required/additional-properties rules, array items, constants/enums, and
/// basic size/range constraints. Unknown keywords mark the schema unsupported;
/// the capture boundary then returns the value without claiming validation.
#[allow(clippy::too_many_lines)]
fn structured_schema_is_supported(schema: &serde_json::Value) -> bool {
    let serde_json::Value::Object(schema) = schema else {
        return schema.is_boolean();
    };
    if schema
        .keys()
        .any(|key| !STRUCTURED_SCHEMA_KEYWORDS.contains(&key.as_str())) {
        return false;
    }
    if schema
        .get("$schema")
        .is_some_and(|value| !value.is_string())
        || schema
            .get("title")
            .is_some_and(|value| !value.is_string())
        || schema
            .get("description")
            .is_some_and(|value| !value.is_string())
        || schema
            .get("examples")
            .is_some_and(|value| !value.is_array())
    {
        return false;
    }
    if let Some(expected) = schema.get("type") {
        if !matches!(
            expected.as_str(),
            Some("object" | "array" | "string" | "number" | "integer" | "boolean" | "null")
        ) {
            return false;
        }
    }
    if let Some(properties) = schema.get("properties") {
        let Some(properties) = properties.as_object() else {
            return false;
        };
        if properties
            .values()
            .any(|child| !structured_schema_is_supported(child))
        {
            return false;
        }
    }
    if let Some(required) = schema.get("required") {
        let Some(required) = required.as_array() else {
            return false;
        };
        let mut names = std::collections::HashSet::new();
        if required
            .iter()
            .any(|key| !key.as_str().is_some_and(|key| names.insert(key)))
        {
            return false;
        }
    }
    for key in ["additionalProperties", "items"] {
        if let Some(child) = schema.get(key) {
            if !child.is_boolean() && !structured_schema_is_supported(child) {
                return false;
            }
        }
    }
    if schema
        .get("enum")
        .is_some_and(|values| values.as_array().is_none_or(std::vec::Vec::is_empty))
    {
        return false;
    }
    for (minimum, maximum) in [
        ("minItems", "maxItems"),
        ("minLength", "maxLength"),
        ("minProperties", "maxProperties"),
    ] {
        let Some(minimum_value) = schema.get(minimum) else {
            continue;
        };
        let Some(minimum_value) = minimum_value.as_u64() else {
            return false;
        };
        if schema.get(maximum).is_some_and(|maximum_value| {
            maximum_value
                .as_u64()
                .is_none_or(|maximum_value| maximum_value < minimum_value)
        }) {
            return false;
        }
    }
    for key in [
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
    ] {
        if schema.get(key).is_some_and(|value| exact_json_number(value).is_none()) {
            return false;
        }
    }
    if schema
        .get("multipleOf")
        .and_then(exact_json_number)
        .is_some_and(|value| value <= 0)
    {
        return false;
    }
    if schema.contains_key("minimum") && schema.contains_key("exclusiveMinimum")
        || schema.contains_key("maximum") && schema.contains_key("exclusiveMaximum")
    {
        // Supporting both forms would require preserving which bound wins at
        // equal values. Reject the ambiguous combination rather than silently
        // validating against only one side.
        return false;
    }
    let exclusive_lower = schema.contains_key("exclusiveMinimum");
    let exclusive_upper = schema.contains_key("exclusiveMaximum");
    let lower = schema
        .get(if exclusive_lower {
            "exclusiveMinimum"
        } else {
            "minimum"
        })
        .and_then(exact_json_number);
    let upper = schema
        .get(if exclusive_upper {
            "exclusiveMaximum"
        } else {
            "maximum"
        })
        .and_then(exact_json_number);
    match (lower, upper) {
        (Some(lower), Some(upper)) => match exact_decimal_cmp(&lower, &upper) {
            std::cmp::Ordering::Less => true,
            std::cmp::Ordering::Equal => !exclusive_lower && !exclusive_upper,
            std::cmp::Ordering::Greater => false,
        },
        _ => true,
    }
}

fn usize_as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn exact_json_number(value: &serde_json::Value) -> Option<i128> {
    let number = value.as_number()?;
    number
        .as_i64()
        .map(i128::from)
        .or_else(|| number.as_u64().map(i128::from))
}

fn exact_decimal_cmp(left: &i128, right: &i128) -> std::cmp::Ordering {
    left.cmp(right)
}

fn exact_multiple_of(value: &i128, divisor: &i128) -> bool {
    *divisor > 0 && value % divisor == 0
}

#[allow(clippy::too_many_lines)]
fn structured_schema_valid(schema: &serde_json::Value, value: &serde_json::Value) -> bool {
    if !structured_schema_is_supported(schema) {
        return false;
    }
    let serde_json::Value::Object(schema) = schema else {
        return matches!(schema, serde_json::Value::Bool(true));
    };
    if schema
        .keys()
        .any(|key| !STRUCTURED_SCHEMA_KEYWORDS.contains(&key.as_str())) {
        return false;
    }
    if let Some(expected) = schema.get("type") {
        let Some(expected) = expected.as_str() else {
            return false;
        };
        let valid_type = match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => return false,
        };
        if !valid_type {
            return false;
        }
    }
    if let Some(constant) = schema.get("const") {
        if value != constant {
            return false;
        }
    }
    if let Some(values) = schema.get("enum").and_then(serde_json::Value::as_array) {
        if values.is_empty() || !values.iter().any(|candidate| candidate == value) {
            return false;
        }
    } else if schema.contains_key("enum") {
        return false;
    }
    let valid_bound = |key: &str, actual: u64, inclusive: bool| match schema.get(key) {
        None => true,
        Some(bound) => bound
            .as_u64()
            .is_some_and(|bound| if inclusive { actual <= bound } else { actual >= bound }),
    };
    match value {
        serde_json::Value::Object(object) => {
            if !valid_bound("minProperties", usize_as_u64(object.len()), false)
                || !valid_bound("maxProperties", usize_as_u64(object.len()), true)
            {
                return false;
            }
            let properties = match schema.get("properties") {
                None => None,
                Some(serde_json::Value::Object(properties)) => Some(properties),
                Some(_) => return false,
            };
            if let Some(required) = schema.get("required") {
                let Some(required) = required.as_array() else {
                    return false;
                };
                if required.iter().any(|key| {
                    !key
                        .as_str()
                        .is_some_and(|key| object.contains_key(key))
                }) {
                    return false;
                }
            }
            if let Some(properties) = properties {
                if properties.iter().any(|(key, child)| {
                    object
                        .get(key)
                        .is_some_and(|value| !structured_schema_valid(child, value))
                }) {
                    return false;
                }
            }
            if let Some(additional) = schema.get("additionalProperties") {
                for (key, child) in object {
                    if properties.is_some_and(|properties| properties.contains_key(key)) {
                        continue;
                    }
                    match additional {
                        serde_json::Value::Bool(true) => {}
                        serde_json::Value::Bool(false) => return false,
                        child_schema if !structured_schema_valid(child_schema, child) => return false,
                        _ => {}
                    }
                }
            }
        }
        serde_json::Value::Array(items) => {
            if !valid_bound("minItems", usize_as_u64(items.len()), false)
                || !valid_bound("maxItems", usize_as_u64(items.len()), true)
            {
                return false;
            }
            if let Some(item_schema) = schema.get("items") {
                if items
                    .iter()
                    .any(|item| !structured_schema_valid(item_schema, item))
                {
                    return false;
                }
            }
        }
        serde_json::Value::String(text) => {
            let length = usize_as_u64(text.chars().count());
            if !valid_bound("minLength", length, false)
                || !valid_bound("maxLength", length, true)
            {
                return false;
            }
        }
        serde_json::Value::Number(_) => {
            let Some(number) = exact_json_number(value) else {
                // serde_json has already reduced non-integer JSON numbers to
                // f64, so their original decimal/exponent precision cannot be
                // recovered. Numeric constraints therefore fail closed.
                return false;
            };
            if schema.get("minimum").is_some_and(|bound| {
                exact_json_number(bound).is_none_or(|bound| number < bound)
            }) || schema.get("exclusiveMinimum").is_some_and(|bound| {
                exact_json_number(bound).is_none_or(|bound| number <= bound)
            }) || schema.get("maximum").is_some_and(|bound| {
                exact_json_number(bound).is_none_or(|bound| number > bound)
            }) || schema.get("exclusiveMaximum").is_some_and(|bound| {
                exact_json_number(bound).is_none_or(|bound| number >= bound)
            }) {
                return false;
            }
            if schema.get("multipleOf").is_some_and(|bound| {
                exact_json_number(bound).is_none_or(|bound| !exact_multiple_of(&number, &bound))
            }) {
                return false;
            }
        }
        _ => {}
    }
    true
}

#[cfg(test)]
mod addressable_agent_name_tests {
    use super::validate_addressable_agent_name;

    /// CC's identifier grammar, accepted verbatim: a validated name is the
    /// agent's ADDRESS, so it must survive byte-for-byte (no slugging, no
    /// lowercasing) or `SendMessage(to: "Web_Scout")` stops resolving.
    #[test]
    fn valid_names_survive_verbatim() {
        for name in [
            "researcher",
            "Web_Scout",
            "agent-2",
            "9lives",
            "a",
            &"x".repeat(64),
        ] {
            assert_eq!(
                validate_addressable_agent_name(name).as_deref(),
                Ok(name),
                "{name} must be accepted unchanged"
            );
        }
        // Surrounding whitespace is trimmed, not rejected.
        assert_eq!(
            validate_addressable_agent_name("  scout \n").as_deref(),
            Ok("scout")
        );
    }

    #[test]
    fn invalid_names_are_rejected_with_the_full_grammar() {
        for name in [
            "",
            "   ",
            "-leading-hyphen",
            "_leading-underscore",
            "has space",
            "has.dot",
            "한글",
            "plan:f3",
            &"x".repeat(65),
        ] {
            let error = validate_addressable_agent_name(name)
                .expect_err("{name} must be rejected");
            assert!(
                error.contains("must start with a letter or digit")
                    && error.contains("max 64 chars"),
                "rejection for {name:?} must state the whole grammar: {error}"
            );
        }
    }

    /// `main` addresses the parent conversation in `SendMessage`; a worker that
    /// claimed it would silently hijack every upstream send.
    #[test]
    fn main_is_reserved_in_any_case() {
        for name in ["main", "Main", "MAIN"] {
            let error =
                validate_addressable_agent_name(name).expect_err("main must be reserved");
            assert!(
                error.contains("reserved") && error.contains("main conversation"),
                "reserved rejection must explain why: {error}"
            );
        }
        // Only the exact word is reserved — a prefix is a normal name.
        assert!(validate_addressable_agent_name("main-thread").is_ok());
    }
}

#[cfg(test)]
mod tests;
