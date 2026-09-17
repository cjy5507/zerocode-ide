use runtime::{
    connected_model_inventory, exploration_slot_for_route, read_route_outcomes, route_model,
    route_model_fallback_candidates, summarize_route_outcomes_with_canonicalizer, EffortCeiling,
    LaneRouteMetadata, LearnedSpecialtyHint, ModelInventory, RouteAutoClassifierMode,
    RouteDecision, RouteDecisionSource, RouteFeedbackHint, RouteOutcomeSummary,
    RoutePolicyContext, RouteRequest, RouteRole, RouteTaskComplexity, RouteTaskRisk,
    RoutingTarget, SubagentProfileId,
};
use serde_json::Value;

use crate::misc_tools::{AgentInput, SpawnMultiAgentInput};

use super::canonical::canonicalize_route_model_id;
use super::evidence::{
    infer_route_shape_evidence, shape_input_with_evidence, RouteEvidenceInput,
};
use super::infer::{infer_route_role, role_key};
use super::metadata::{
    apply_calibration_to_metadata, apply_probe_to_metadata, classify_task_metadata,
    TaskMetadataInput, TaskRouteMetadata,
};
use super::planner::plan_agent_needs;
use super::settings::{read_smart_runtime_settings, LearnedSpecialtyMode, SmartRuntimeSettings};
use super::shape::{select_route_shape, RouteShape};

/// Everything the routing decision reads from disk, loaded ONCE per spawn
/// call: settings.json, the connected-model inventory (credential probes),
/// and the outcome-feedback summary (a JSONL parse). Routing a fan-out
/// previously re-read all three per agent — a 6-agent batch cost 6× the
/// probe/parse I/O before the first agent even launched.
/// The fleet agent a model belongs to, as the provider its id resolves to — the
/// same classification the client uses to pick an adapter, so outcome records
/// and live candidates are labelled identically.
pub(super) fn agent_of_model(model: &str) -> String {
    api::detect_provider_kind(model).to_string()
}

/// P3 agent-axis nudge: for `route_key`, the preferred agent's bounded
/// adjustment (`runtime::agent_preference_adjustment`) applied to each
/// `candidate` model of that agent, and to no other. Empty when no agent is
/// preferred (thin evidence, or a tie). Pure — the seam the tests pin.
pub(super) fn agent_bonus_adjustments<'a>(
    records: &[runtime::RouteOutcomeRecord],
    route_key: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Vec<(String, i16)> {
    let Some((agent, adjustment)) = runtime::agent_preference_adjustment(
        records,
        route_key,
        runtime::ACCURACY_MIN_DECISIVE,
        agent_of_model,
    ) else {
        return Vec::new();
    };
    candidates
        .filter(|candidate| agent_of_model(candidate) == agent)
        .map(|candidate| (candidate.to_string(), adjustment))
        .collect()
}

struct SmartRouteContext {
    settings: SmartRuntimeSettings,
    inventory: ModelInventory,
    outcomes: Option<RouteOutcomeSummary>,
    /// The raw records behind `outcomes`, kept so the agent-axis preference
    /// (P3) can be computed for the route being decided.
    records: Option<Vec<runtime::RouteOutcomeRecord>>,
    /// Providers currently headroom-low (active cool-down / recent 429 /
    /// OAuth window pressure), read ONCE per spawn batch — mirrors the
    /// settings/inventory/outcome batching above so a fan-out does not
    /// re-probe the rate-limit governor per agent. `RoutePolicyContext` only
    /// ever sees these plain strings (engine purity: no IO in the router core).
    cooldown_providers: Vec<String>,
    /// Per-provider *remaining* quota headroom (`rate_limit_key`, percent),
    /// read ONCE per spawn batch alongside `cooldown_providers` from
    /// `api::quota::provider_quota_views`. Feeds the router's GRADED headroom
    /// penalty (the leading signal) where `cooldown_providers` feeds the binary
    /// one — `RoutePolicyContext` only ever sees these plain integers (engine
    /// purity: no IO in the router core).
    provider_headroom: Vec<(String, u8)>,
    /// Phase 6 learned-specialty hint, computed ONCE per batch from the SAME
    /// raw records `outcomes` is built from (no extra JSONL read) — empty
    /// (`LearnedSpecialtyHint::disabled()`) whenever `smart.learnedSpecialty
    /// = off`, or the store is unreadable/empty. This is the "what learned
    /// specialty WOULD say" hint; whether/how it is actually injected into a
    /// route's `RoutePolicyContext` depends on `settings.learned_specialty`
    /// (off/shadow/on) — see `smart_model_for_fields`.
    learned_specialty: LearnedSpecialtyHint,
    /// Learned complexity calibration, computed ONCE per batch from the SAME
    /// raw records as the two fields above (no extra IO): a (role,
    /// complexity) class the outcome log shows keeps failing at its assigned
    /// band gets floored one band up at classification time. Active exactly
    /// when outcome learning already is (feedback or learned specialty on);
    /// `ZO_ROUTE_CALIBRATION=0` is the operator kill switch.
    calibration: runtime::ComplexityCalibration,
}

impl SmartRouteContext {
    /// `None` when Smart routing is off (or settings are unreadable) — the
    /// caller leaves the spawn on its default model.
    fn load(parent_model: &str) -> Option<Self> {
        let settings = read_smart_runtime_settings()?;
        if !settings.enabled {
            return None;
        }
        // Read the raw outcome records ONCE and reuse them for the feedback
        // summary, Phase 6's learned-specialty hint, AND the complexity
        // calibration — whichever of the three actually need it. A fan-out
        // must not pay for independent JSONL reads/parses of the same file;
        // calibration is in this gate so it works (and matches what `/smart
        // doctor` displays) even with feedback and learned specialty off.
        let need_raw_records = settings.feedback_informed_auto
            || settings.learned_specialty != LearnedSpecialtyMode::Off
            || route_calibration_enabled();
        let raw_records = need_raw_records
            .then(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|cwd| read_route_outcomes(&cwd).ok())
            })
            .flatten();
        // P3 canonicalization-at-read: summarize through the injected
        // tools-layer canonicalizer so historical id fragments (`claude-
        // opus-4-8` vs `claude-opus-4.8`, `fable`/`fable5` vs
        // `claude-fable-5`, `gpt-5.5` vs its dated canonical id) merge into
        // one bucket before feedback is computed. Byte-identical to the old
        // `summarize_route_outcomes` (identity canonicalizer) whenever the
        // history has no such fragments — the common case for a small/clean
        // fixture — so this changes real routing ONLY when the outcome store
        // actually holds merge-worthy fragments (the bug this fixes).
        let outcomes = settings.feedback_informed_auto.then_some(raw_records.as_ref()).flatten().map(
            |records| summarize_route_outcomes_with_canonicalizer(records, canonicalize_route_model_id),
        );
        let learned_specialty = if settings.learned_specialty == LearnedSpecialtyMode::Off {
            LearnedSpecialtyHint::disabled()
        } else {
            raw_records
                .as_ref()
                .map(|records| {
                    LearnedSpecialtyHint::compute(records, epoch_seconds_now(), canonicalize_route_model_id)
                })
                .unwrap_or_default()
        };
        let calibration = if route_calibration_enabled() {
            raw_records
                .as_ref()
                .map(|records| runtime::ComplexityCalibration::compute(records))
                .unwrap_or_default()
        } else {
            runtime::ComplexityCalibration::disabled()
        };
        Some(Self {
            inventory: connected_model_inventory(parent_model),
            settings,
            outcomes,
            records: raw_records,
            cooldown_providers: cooldown_provider_names(),
            provider_headroom: provider_headroom_percents(),
            learned_specialty,
            calibration,
        })
    }

    /// Outcome feedback for a route, keyed by the **resolved subagent type**
    /// the spawn will run as. Route outcomes are recorded under
    /// `subagent:{type}` (the manifest's resolved type,
    /// `agent_tools::resolve_subagent_type`), so keying the lookup the same
    /// way means BOTH explicit-subagent routes AND role-fallback routes learn
    /// from history. Served from the batch-loaded summary instead of
    /// re-parsing the outcome log per agent.
    fn feedback_for(&self, subagent_type: &str) -> RouteFeedbackHint {
        if !self.settings.feedback_informed_auto || subagent_type.trim().is_empty() {
            return RouteFeedbackHint::disabled();
        }
        let route_key = format!("subagent:{subagent_type}");
        let mut hint = self
            .outcomes
            .as_ref()
            .map_or_else(RouteFeedbackHint::disabled, |summary| summary.feedback_hint_for_route_key(&route_key));
        // P3: the agent the evidence prefers for this route earns a bounded
        // nudge on every candidate model of that agent — composed with (never
        // above) the per-model feedback, under the same feedback-informed
        // setting. A user-named model never reaches this path (it is pinned).
        if let Some(records) = self.records.as_deref() {
            for (model, adjustment) in agent_bonus_adjustments(
                records,
                &route_key,
                self.inventory.models().iter().map(runtime::ModelDescriptor::id),
            ) {
                hint = hint.with_model_adjustment(model, adjustment);
            }
        }
        hint
    }

    /// Phase 5 exploration slot for this route, computed from the
    /// ALREADY-LOADED outcome summary (no extra JSONL read) — the `apply.rs`
    /// half of `runtime::exploration_slot_for_route`'s contract: this method
    /// gathers the aggregated inputs (cadence, incumbent confidence,
    /// under-sampled-rival eligibility, decisive counts) and applies the
    /// `smart.exploration` master switch, then defers the actual cadence
    /// math and the risk/role hard gates to the pure engine function so they
    /// stay unit-testable independent of settings parsing.
    ///
    /// `has_undersampled_rival` is a COARSE, cheap proxy — not the precise
    /// tier/capability-qualified check the routing plan describes — because
    /// apply.rs has no business re-deriving the selector/rung membership
    /// logic that already lives in the engine's `ranked_auto_candidates`.
    /// Instead: true when EITHER some model already recorded under this
    /// `route_key` has fewer than 2 decisive samples, OR the connected
    /// inventory holds more models than this `route_key` has ever recorded at
    /// all (the common brand-new-model case — a model with ZERO history
    /// never appears in the outcome summary). This is a NECESSARY, not
    /// sufficient, precondition: the engine's `select_exploration_candidate`
    /// does the precise same-rung/window/decisive-count filtering and simply
    /// falls through (wasted slot, no error) when nothing actually qualifies,
    /// so an imprecise `true` here costs at most one wasted rotation.
    fn exploration_slot_for(
        &self,
        route_key: &str,
        role: RouteRole,
        risk: RouteTaskRisk,
    ) -> (Option<u32>, Vec<(String, u32)>) {
        let Some(summary) = self.outcomes.as_ref() else {
            return (None, Vec::new());
        };
        let decisive_counts = summary.decisive_counts_for_route_key(route_key);
        let exploration_decisive_counts: Vec<(String, u32)> = decisive_counts
            .iter()
            .map(|(model, count)| (model.clone(), u32::try_from(*count).unwrap_or(u32::MAX)))
            .collect();
        let top_incumbent_decisive = decisive_counts.iter().map(|(_, count)| *count).max().unwrap_or(0);
        let has_undersampled_rival = decisive_counts.iter().any(|(_, count)| *count < 2)
            || decisive_counts.len() < self.inventory.models().len();
        let total_records = summary.total_records_for_route_key(route_key);
        let slot = exploration_slot_for_route(
            total_records,
            self.settings.exploration_cadence,
            top_incumbent_decisive,
            has_undersampled_rival,
            risk,
            role,
            self.settings.exploration,
        );
        (slot, exploration_decisive_counts)
    }
}

/// Providers currently headroom-low, expressed as the same lowercase
/// provider-name strings `ModelDescriptor::provider()` uses
/// (`ProviderKind::rate_limit_key`), so `RoutePolicyContext::provider_in_cooldown`
/// can match them directly against a candidate's provider with no further
/// translation. Reads the SAME per-provider governor state
/// (`agent_tools::rate_limit::rate_limit_headroom_low`) the sub-agent spawn
/// path already gates admission on — an operational quota fact injected at
/// this application layer, never IO inside the pure router engine.
/// Current epoch seconds, read ONCE per spawn batch and injected into
/// [`LearnedSpecialtyHint::compute`] — that fn stays a pure function of its
/// inputs (no hidden clock read), mirroring
/// `runtime::weighted_feedback_hint_for_route_key`'s injected-`now` seam.
/// Plain `SystemTime`/`UNIX_EPOCH` inline read (this crate's established
/// convention — see e.g. `dispatch::epoch_seconds_now`, `team_tools.rs` —
/// rather than exporting a clock helper from the `runtime` crate).
fn epoch_seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Operator kill switch for learned complexity calibration
/// (`ZO_ROUTE_CALIBRATION=0` disables; the grind/cascade env idiom). Read
/// per batch so a retune needs no rebuild.
fn route_calibration_enabled() -> bool {
    std::env::var("ZO_ROUTE_CALIBRATION")
        .ok()
        .is_none_or(|raw| raw.trim() != "0")
}

fn cooldown_provider_names() -> Vec<String> {
    [
        api::ProviderKind::Anthropic,
        api::ProviderKind::OpenAi,
        api::ProviderKind::Google,
        api::ProviderKind::Xai,
        api::ProviderKind::Ollama,
    ]
    .into_iter()
    .filter(|kind| crate::misc_tools::agent_tools::rate_limit_headroom_low(*kind))
    .map(|kind| kind.rate_limit_key().to_string())
    .collect()
}

/// Per-provider *remaining* quota headroom for the router's GRADED penalty,
/// expressed as the same lowercase `ProviderKind::rate_limit_key` strings
/// `RoutePolicyContext::headroom_penalty_for` matches. Reads the unified quota
/// views (`api::quota::provider_quota_views`) — Anthropic's measured 5h/7d
/// windows plus each non-Anthropic provider's 429-estimate — and keeps, per
/// provider, the MINIMUM remaining across its ACCOUNT-WIDE windows (the
/// *binding* one, so a hot 5h window isn't masked by a cool 7d). Rows without a
/// known `remaining_percent` are skipped (unknown ≠ 0). Read ONCE per spawn
/// batch and injected as plain integers — the router engine never does IO of
/// its own.
fn provider_headroom_percents() -> Vec<(String, u8)> {
    binding_headroom_from_views(api::quota::provider_quota_views())
}

/// Pure core of [`provider_headroom_percents`]: fold the quota views into one
/// `(rate_limit_key, remaining)` per provider, keeping the MINIMUM remaining
/// across a provider's account-wide windows (the binding one), and skipping
/// rows whose `remaining_percent` is unknown. Split out so the min-per-provider
/// fold is unit-testable without touching the process-global quota state.
///
/// Per-model rows (`ProviderQuotaView::model`) are dropped: this figure grades a
/// PROVIDER, and one exhausted model is not the provider running dry. Keeping
/// them would let a spent side-model penalize every route to that provider,
/// including the routes that would still be served.
pub(super) fn binding_headroom_from_views(
    views: Vec<api::quota::ProviderQuotaView>,
) -> Vec<(String, u8)> {
    let mut binding: Vec<(String, u8)> = Vec::new();
    for view in views {
        if view.model.is_some() {
            continue;
        }
        let Some(remaining) = view.remaining_percent else {
            continue;
        };
        let key = view.provider.rate_limit_key().to_string();
        if let Some(existing) = binding.iter_mut().find(|(provider, _)| *provider == key) {
            existing.1 = existing.1.min(remaining);
        } else {
            binding.push((key, remaining));
        }
    }
    binding
}

/// A smart route decision for a spawned sub-agent: the resolved `model` to run
/// on plus the human-readable WHY (role, selector outcome, score adjustments)
/// that travels to the manifest (`routeReason`) so the TUI can show it. The
/// router returns `None` when the optimal pick IS the parent/session model, so
/// `model` here is always a genuinely different, deliberately-chosen model.
///
/// `model` is a TRUSTED, config-driven decision and is honored verbatim by the
/// spawn path: [`route_model`] already constrained it to the connected
/// inventory (so it is always launchable), the user's `/smart` config
/// explicitly opted into dynamic per-role routing, and — unlike an untrusted
/// on-wire `model` field — it is NOT re-gated by provider family, so a
/// deliberate cross-provider route for a diversity role (Verifier/Reviewer/
/// Judge) actually takes effect. For a coding/worker role the router's own
/// policy anchors to the parent model (which returns `None` here), so those keep
/// inheriting it — no silent cross-provider downgrade of Claude/GPT coding work.
#[derive(Debug, Clone)]
pub(crate) struct SmartRouteChoice {
    /// Host-computed model override. `None` means the selected route is the
    /// parent/session model, but `fallback_models` may still carry the router's
    /// second-best choices for quota fallback.
    pub model: Option<String>,
    /// Human-readable route reason, present only when `model` is a genuine
    /// override. Fallback-only choices should not make the manifest look like the
    /// agent was routed away from the parent before any quota pressure occurred.
    pub reason: Option<String>,
    /// Ranked alternate models to try when the selected provider is rate-limited
    /// or already parked in a long cool-down. These are host-only and already
    /// gated by the Smart router's connected inventory/policy.
    pub fallback_models: Vec<String>,
    /// Recommended reasoning-effort tier for this route
    /// (`RouteDecision::recommended_effort`), converted to the provider-neutral
    /// `api::EffortLevel` scale. Present independently of `model` — a route
    /// that keeps the parent model can still carry an effort recommendation
    /// (e.g. the parent itself is the Ultra-capable model the rule matched).
    pub effort: Option<api::EffortLevel>,
    /// P3 v2 route-decision metadata (role/complexity/risk/routeSource),
    /// present whenever this choice itself is (the same gate as `reason`'s
    /// "genuine decision" condition) — reused verbatim from the classifier
    /// output and `RouteDecision::source` that ALREADY computed this route,
    /// never re-derived from text in a recorder. Threaded to the manifest so
    /// the spawn-completion outcome record can stamp the P3 schema fields.
    pub decision_meta: RouteDecisionMeta,
}

/// Plain-string projection of the route decision's role/complexity/risk/
/// source, carried alongside [`SmartRouteChoice`] to the manifest and from
/// there into the persisted `RouteOutcomeRecord` (P3 v2 schema). Kept as
/// simple owned strings (not the `runtime` enums) so this crosses the
/// smuggle-JSON wire (`ROUTE_DECISION_META_SMUGGLE_KEY`) the same way the
/// other route metadata does.
#[derive(Debug, Clone, Default)]
pub(crate) struct RouteDecisionMeta {
    pub role: String,
    pub complexity: String,
    pub risk: String,
    pub route_source: String,
    /// The probe's own stated confidence (`"low"`/`"medium"`/`"high"`), or
    /// `None` when no probe arrived. Provenance only — nothing routes on it.
    pub probe_confidence: Option<String>,
}

/// Projects `RouteRole` to the SAME lowercase label `settings.roles`/
/// `settings.subagents` keys already use (`role_key`) — no new vocabulary.
fn route_role_label(role: RouteRole) -> String {
    role_key(role).to_string()
}

/// Projects `RouteDecisionSource` (already computed by `route_model`) onto
/// the v2 schema's `routeSource` vocabulary (`"auto"|"pin"|"explicit"|
/// "fallback"|"exploration"`). `Pinned` and `ManualSelector` both project to
/// `"pin"`: both are a `smart.roles`/`smart.subagents` CONFIG override (an
/// availability signal), not the unconstrained AUTO ranking — the same
/// distinction Phase 6's pin-zero-weight learning filter relies on.
/// `Exploration` (Phase 5's deterministic under-sampled rotation) gets its
/// own label, distinct from `"auto"`, so a later learning phase can tell a
/// deliberate cold-start sampling pick apart from an unforced best-of-breed
/// pick.
pub(crate) fn route_source_label(source: RouteDecisionSource) -> &'static str {
    match source {
        RouteDecisionSource::Explicit => "explicit",
        RouteDecisionSource::Pinned | RouteDecisionSource::ManualSelector => "pin",
        RouteDecisionSource::AutoSelector => "auto",
        RouteDecisionSource::Exploration => "exploration",
        RouteDecisionSource::LearnedAdmission => "learned-admission",
        RouteDecisionSource::MainOnly
        | RouteDecisionSource::MainModelFallback
        | RouteDecisionSource::FallbackDisabled => "fallback",
    }
}

/// Smuggle key carrying the route reason inside a fan-out member's JSON object;
/// the spawn loop strips it into `AgentInput::route_reason`. SECURITY: the
/// `__zo_` prefix does NOT make it uncraftable — untrusted agent JSON can
/// carry this exact key — so [`apply_smart_models_to_spawn_input`] scrubs any
/// caller-supplied copy up front; dispatch is the other trusted populator when
/// it appends an auto-inferred agent type (a crafted reason can never be stamped
/// onto the manifest).
pub(crate) const ROUTE_REASON_SMUGGLE_KEY: &str = "__zo_route_reason";

/// Smuggle key carrying the resolved smart-route model inside a fan-out member's
/// JSON object; the spawn loop strips it into `AgentInput::route_model` (a
/// trusted host field, never on-wire tool input). SECURITY: the `__zo_`
/// prefix does NOT make it uncraftable — untrusted agent JSON can carry this
/// exact key — so [`apply_smart_models_to_spawn_input`] scrubs any caller-supplied
/// copy up front and is the sole populator, keeping a crafted value from
/// bypassing the on-wire `model` provider-family fence.
pub(crate) const ROUTE_MODEL_SMUGGLE_KEY: &str = "__zo_route_model";

/// Host-only smuggle key carrying ranked rate-limit fallback models for a
/// fan-out member. Scrubbed with the other route keys before routing so untrusted
/// tool JSON cannot inject arbitrary fallback models.
pub(crate) const ROUTE_FALLBACK_MODELS_SMUGGLE_KEY: &str = "__zo_route_fallback_models";

/// Smuggle key carrying the recommended effort tier inside a fan-out member's
/// JSON object; the spawn loop strips it into `AgentInput::route_effort` (a
/// trusted host field). SECURITY: same trust boundary as the other route
/// keys — the `__zo_` prefix does NOT make it uncraftable, so
/// [`apply_smart_models_to_spawn_input`] scrubs any caller-supplied copy up
/// front. Without this an untrusted spawn JSON could force a Deep/Ultra
/// effort (and its paired thinking budget) onto an arbitrary agent.
pub(crate) const ROUTE_EFFORT_SMUGGLE_KEY: &str = "__zo_route_effort";

/// Smuggle key carrying [`RouteDecisionMeta`] (role/complexity/risk/
/// routeSource) as one JSON object, alongside the other route keys; the
/// spawn loop strips it into `AgentInput::route_role`/`route_complexity`/
/// `route_risk`/`route_source`. SECURITY: same trust boundary as the other
/// route keys — the `__zo_` prefix does NOT make it uncraftable, so
/// [`apply_smart_models_to_spawn_input`] scrubs any caller-supplied copy up
/// front. A single combined key (rather than 4 separate ones) keeps the
/// scrub/parse surface from growing 4× for what is purely descriptive
/// metadata (unlike `route_model`/`route_effort`, nothing here re-gates a
/// trust boundary on its own).
pub(crate) const ROUTE_DECISION_META_SMUGGLE_KEY: &str = "__zo_route_decision_meta";

/// Smuggle key carrying the `name` of the fan-out member a verifier/reviewer
/// member's need plan judges (Phase 4 verdict channel — source #2:
/// planner-bound reviewer→worker pairing, see
/// [`planner_bound_judged_agent_names`]); the spawn loop
/// (`misc_tools::run_spawn_multi_agent_with_timeout_and_hooks`) resolves that
/// name to the worker's real agent id (once one exists) into
/// `AgentInput::judged_agent`. SECURITY: same trust boundary as the other
/// route keys — the `__zo_` prefix does NOT make it uncraftable, so
/// [`apply_smart_models_to_spawn_input`] scrubs any caller-supplied copy up
/// front. Without this an untrusted spawn JSON could point verdict credit (a
/// completed/failed route-outcome record) at an arbitrary route.
pub(crate) const ROUTE_JUDGED_AGENT_SMUGGLE_KEY: &str = "__zo_route_judged_agent";

/// Conservative planner-bound reviewer→worker pairing (Phase 4 verdict
/// channel — source #2). A verdict is only ever attributed to a worker's
/// route when the binding is UNAMBIGUOUS by construction — mirroring the
/// existing verdict-attribution doctrine (`workflow_tools::engine::
/// attribution`: "only WELL-BOUND (verdict, worker) pairs are recorded").
///
/// The one shape this fn recognizes as unambiguous: the fan-out batch has
/// EXACTLY two members, and exactly one of them classifies as a
/// Reviewer/Verifier/Judge role (`infer_route_role`) — the other member is
/// then, by elimination, the work it judges. Any less certain shape (a batch
/// of any other size, both/neither member reviewer-shaped, or the worker
/// member has no `name` to reference later) returns `None` for every member —
/// no guessing which of 3+ siblings a reviewer targets.
///
/// Returns one `Option<String>` per member of `agents`, in order: `Some(worker
/// name)` for the reviewer-shaped member of a recognized pair, `None`
/// otherwise (including for the worker member itself — a worker is never its
/// own judge).
fn planner_bound_judged_agent_names(agents: &[Value]) -> Vec<Option<String>> {
    let none_for_all = || vec![None; agents.len()];
    if agents.len() != 2 {
        return none_for_all();
    }
    let roles: Vec<RouteRole> = agents.iter().map(member_route_role).collect();
    let judge_positions: Vec<usize> = roles
        .iter()
        .enumerate()
        .filter(|(_, role)| is_judging_role(**role))
        .map(|(index, _)| index)
        .collect();
    let [judge_index] = judge_positions.as_slice() else {
        // Zero or two reviewer-shaped members — no unambiguous worker to bind.
        return none_for_all();
    };
    let worker_index = 1 - judge_index;
    let Some(worker_name) = agents[worker_index]
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        // No `name` to reference — the spawn loop has no key to resolve the
        // worker's agent id by later, so the binding cannot be carried.
        return none_for_all();
    };
    let mut names = none_for_all();
    names[*judge_index] = Some(worker_name.to_string());
    names
}

const fn is_judging_role(role: RouteRole) -> bool {
    matches!(role, RouteRole::Reviewer | RouteRole::Verifier | RouteRole::Judge)
}

/// The same role classification `smart_model_for_fields` uses
/// (`infer_route_role`), read directly off a member's still-raw JSON —
/// pairing is computed before that per-member classification runs (it must
/// see the WHOLE batch at once), so it re-derives the same inputs
/// (`subagent_type`/`description`/`prompt`) rather than threading the
/// per-member `TaskRouteMetadata` out of the existing loop.
fn member_route_role(agent: &Value) -> RouteRole {
    let subagent_type = agent
        .get("subagent_type")
        .or_else(|| agent.get("subagentType"))
        .and_then(Value::as_str);
    let description = agent.get("description").and_then(Value::as_str).unwrap_or_default();
    let prompt = agent.get("prompt").and_then(Value::as_str).unwrap_or_default();
    infer_route_role(subagent_type, description, prompt)
}

pub(crate) fn smart_parent_model_for_agent(
    parent_model: Option<&str>,
    input: &AgentInput,
) -> Option<SmartRouteChoice> {
    smart_parent_model_for_agent_with_auto_type(parent_model, input, None)
}

pub(crate) fn smart_parent_model_for_agent_with_auto_type(
    parent_model: Option<&str>,
    input: &AgentInput,
    auto_type: Option<&str>,
) -> Option<SmartRouteChoice> {
    if input.model.as_deref().is_some_and(|model| !model.trim().is_empty()) {
        return None;
    }
    let parent_model = parent_model.map(str::trim).filter(|model| !model.is_empty())?;
    let context = SmartRouteContext::load(parent_model)?;
    let probe = (context.settings.auto_classifier == RouteAutoClassifierMode::Probed)
        .then(|| {
            super::probe_exec::route_probe_assessment(
                &context.inventory,
                parent_model,
                &input.description,
                &input.prompt,
                // The probe is spent deciding THIS spawn's model, so its tax
                // is billed to the turn that asked for the spawn.
                &crate::misc_tools::agent_tools::served_turn_attempt(
                    input.parent_session_id.as_deref(),
                )
                .unwrap_or_default(),
            )
        })
        .flatten();
    smart_model_for_fields(
        &context,
        parent_model,
        routing_subagent_type(input.subagent_type.as_deref(), auto_type),
        input.name.as_deref(),
        &input.description,
        &input.prompt,
        input.schema.is_some(),
        input.workflow_member,
        input.prior_failures,
        None,
        probe.as_ref(),
    )
}

#[cfg(test)]
pub(crate) fn apply_smart_models_to_spawn_input(
    parent_model: Option<&str>,
    input: &mut SpawnMultiAgentInput,
) {
    apply_smart_models_to_spawn_input_with_auto_types(parent_model, input, &[]);
}

#[allow(clippy::too_many_lines)]
pub(crate) fn apply_smart_models_to_spawn_input_with_auto_types(
    parent_model: Option<&str>,
    input: &mut SpawnMultiAgentInput,
    auto_types: &[Option<String>],
) {
    // SECURITY (trust boundary): the route smuggle keys are a HOST-ONLY channel
    // that `run_spawn_multi_agent` reads verbatim — and a routed `model` bypasses
    // the on-wire `model` provider-family fence. `input.agents` is untrusted,
    // model/user-authored JSON, so scrub any pre-existing smuggle keys from every
    // member UNCONDITIONALLY, BEFORE the early returns below, so ONLY this
    // function (the trusted host) can ever populate them. Without this a crafted
    // `__zo_route_model` would survive whenever the member carries an explicit
    // `model`, whenever routing returns no override, or whenever `/smart` is
    // disabled entirely — forcing an arbitrary cross-provider model past the
    // fence. The `__zo_` prefix does NOT make the key uncraftable.
    for agent in &mut input.agents {
        if let Some(object) = agent.as_object_mut() {
            object.remove(ROUTE_MODEL_SMUGGLE_KEY);
            object.remove(ROUTE_REASON_SMUGGLE_KEY);
            object.remove(ROUTE_FALLBACK_MODELS_SMUGGLE_KEY);
            object.remove(ROUTE_EFFORT_SMUGGLE_KEY);
            object.remove(ROUTE_DECISION_META_SMUGGLE_KEY);
            object.remove(ROUTE_JUDGED_AGENT_SMUGGLE_KEY);
        }
    }
    let Some(parent_model) = parent_model.map(str::trim).filter(|model| !model.is_empty()) else {
        return;
    };
    let Some(context) = SmartRouteContext::load(parent_model) else {
        return;
    };
    let total_agents = input.agents.len();
    // Phase 4 verdict channel — source #2: a conservative, well-bounded
    // planner-bound reviewer→worker pairing (see `planner_bound_judged_agent_names`).
    // Computed once over the whole (read-only at this point) batch, before the
    // mutable per-member loop below inserts the OTHER route smuggle keys.
    let judged_agent_names = planner_bound_judged_agent_names(&input.agents);
    // Probed mode: fire the WHOLE batch's probes concurrently up front (one
    // probe of wall-clock, memoized per task fingerprint), instead of one
    // blocking probe per member inside the routing loop below. Members that
    // carry an explicit model never probe — they never route.
    let member_probes: Vec<Option<runtime::ProbeAssessment>> =
        if context.settings.auto_classifier == RouteAutoClassifierMode::Probed {
            // Single borrowed pass: members carrying an explicit model never
            // route, so they contribute an empty task (skipped by the probe
            // executor) instead of paying a probe.
            let tasks: Vec<(&str, &str)> = input
                .agents
                .iter()
                .map(|agent| {
                    let Some(object) = agent.as_object() else {
                        return ("", "");
                    };
                    let explicit_model = object
                        .get("model")
                        .and_then(Value::as_str)
                        .is_some_and(|model| !model.trim().is_empty());
                    if explicit_model {
                        return ("", "");
                    }
                    (
                        object.get("description").and_then(Value::as_str).unwrap_or_default(),
                        object.get("prompt").and_then(Value::as_str).unwrap_or_default(),
                    )
                })
                .collect();
            super::probe_exec::route_probe_assessments(
                &context.inventory,
                parent_model,
                &tasks,
                &crate::misc_tools::agent_tools::served_turn_attempt(
                    input.parent_session_id.as_deref(),
                )
                .unwrap_or_default(),
            )
        } else {
            vec![None; input.agents.len()]
        };
    for (index, agent) in input.agents.iter_mut().enumerate() {
        let Some(object) = agent.as_object_mut() else {
            continue;
        };
        // Judged-agent binding is orthogonal to THIS member's own model
        // routing (an explicit `model` on the reviewer still lets it judge a
        // sibling worker), so it is smuggled before the explicit-model
        // early-continue below.
        if let Some(worker_name) = judged_agent_names.get(index).and_then(Option::as_ref) {
            object.insert(
                ROUTE_JUDGED_AGENT_SMUGGLE_KEY.to_string(),
                Value::String(worker_name.clone()),
            );
        }
        if object
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| !model.trim().is_empty())
        {
            continue;
        }
        let description = object
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let prompt = object
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let subagent_type = object
            .get("subagent_type")
            .or_else(|| object.get("subagentType"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let route_subagent_type = routing_subagent_type(
            subagent_type.as_deref(),
            auto_types.get(index).and_then(Option::as_deref),
        );
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(choice) = smart_model_for_fields(
            &context,
            parent_model,
            route_subagent_type,
            name.as_deref(),
            &description,
            &prompt,
            object.contains_key("schema"),
            true,
            0,
            Some((index, total_agents)),
            member_probes.get(index).and_then(Option::as_ref),
        ) {
            // Carry the resolved model only when it is a genuine override; carry
            // fallbacks separately so a parent-selected route can still escape a
            // quota stall without pretending it was pre-routed away from parent.
            if let Some(model) = choice.model {
                object.insert(ROUTE_MODEL_SMUGGLE_KEY.to_string(), Value::String(model));
            }
            if let Some(reason) = choice.reason {
                object.insert(ROUTE_REASON_SMUGGLE_KEY.to_string(), Value::String(reason));
            }
            if !choice.fallback_models.is_empty() {
                object.insert(
                    ROUTE_FALLBACK_MODELS_SMUGGLE_KEY.to_string(),
                    Value::Array(choice.fallback_models.into_iter().map(Value::String).collect()),
                );
            }
            if let Some(effort) = choice.effort {
                // `EffortLevel` serializes as its lowercase wire token
                // (`#[serde(rename_all = "lowercase")]`) — reused verbatim as
                // the smuggled value instead of hand-rolling a parallel token
                // table, and parsed back the same way in `misc_tools::mod`.
                if let Ok(value) = serde_json::to_value(effort) {
                    object.insert(ROUTE_EFFORT_SMUGGLE_KEY.to_string(), value);
                }
            }
            object.insert(
                ROUTE_DECISION_META_SMUGGLE_KEY.to_string(),
                serde_json::json!({
                    "role": choice.decision_meta.role,
                    "complexity": choice.decision_meta.complexity,
                    "risk": choice.decision_meta.risk,
                    "routeSource": choice.decision_meta.route_source,
                }),
            );
        }
    }
}

pub(super) fn routing_subagent_type<'a>(
    subagent_type: Option<&'a str>,
    auto_type: Option<&str>,
) -> Option<&'a str> {
    if auto_type.is_some_and(|agent_type| agent_type.eq_ignore_ascii_case("general-purpose")) {
        None
    } else {
        subagent_type
    }
}

struct SmartRoutingIdentity {
    target: RoutingTarget,
    fallback_role: RouteRole,
    effective_role: RouteRole,
    resolved_subagent_type: String,
    custom_target: bool,
}

fn smart_routing_identity(
    subagent_type: Option<&str>,
    description: &str,
    prompt: &str,
) -> SmartRoutingIdentity {
    // Agent execution resolves an explicit custom definition before built-in
    // aliases. Mirror that precedence so `analysis.md`/`reviewer.md` does not
    // acquire a read-only specialty role while asking for implementation.
    let custom_route_context =
        crate::misc_tools::agent_tools::custom_agent_route_context(subagent_type);
    let has_custom_definition = custom_route_context.is_some();
    let resolved_subagent_type = crate::misc_tools::agent_tools::resolve_subagent_type(
        subagent_type,
        description,
        prompt,
    );
    let target_profile = subagent_type
        .map(str::trim)
        .filter(|subagent| !subagent.is_empty())
        .and_then(|subagent| {
            if has_custom_definition {
                SubagentProfileId::custom(subagent)
            } else {
                SubagentProfileId::parse(&resolved_subagent_type)
            }
        });
    let custom_target = target_profile
        .as_ref()
        .is_some_and(|profile| profile.kind() == runtime::SubagentProfileKind::Custom);
    let role_subagent_type = if custom_target { None } else { subagent_type };
    let inferred_role = infer_route_role(role_subagent_type, description, prompt);
    let implementation_description = custom_route_context
        .as_deref()
        .map_or_else(|| description.to_string(), |custom| format!("{description} {custom}"));
    let fallback_role = if custom_target
        && super::agent_task_has_write_intent(&implementation_description, prompt)
    {
        RouteRole::Coding
    } else {
        inferred_role
    };
    let target = target_profile
        .map_or(RoutingTarget::RoleFallback(fallback_role), RoutingTarget::Subagent);
    let effective_role = target.route_role_hint().unwrap_or(fallback_role);
    SmartRoutingIdentity {
        target,
        fallback_role,
        effective_role,
        resolved_subagent_type,
        custom_target,
    }
}

fn apply_configured_route_override(
    request: &mut RouteRequest,
    settings: &SmartRuntimeSettings,
) {
    if let RoutingTarget::Subagent(profile) = &request.target {
        if let Some(override_rule) = settings.subagents.get(profile.key()) {
            request.override_rule = Some(override_rule.clone());
        }
    }
    if request.override_rule.is_none() {
        if let Some(override_rule) = settings.roles.get(role_key(request.effective_role())) {
            request.override_rule = Some(override_rule.clone());
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn smart_model_for_fields(
    context: &SmartRouteContext,
    parent_model: &str,
    subagent_type: Option<&str>,
    name: Option<&str>,
    description: &str,
    prompt: &str,
    has_schema: bool,
    workflow_member: bool,
    prior_failures: u32,
    fanout_position: Option<(usize, usize)>,
    probe: Option<&runtime::ProbeAssessment>,
) -> Option<SmartRouteChoice> {
    let settings = &context.settings;
    let SmartRoutingIdentity {
        target,
        fallback_role,
        effective_role,
        resolved_subagent_type,
        custom_target,
    } = smart_routing_identity(
        subagent_type,
        description,
        prompt,
    );
    let role_subagent_type = if custom_target { None } else { subagent_type };
    let metadata_input = TaskMetadataInput::new(role_subagent_type, description, prompt)
        .with_schema(has_schema)
        .with_workflow_member(workflow_member);
    let mut metadata = classify_task_metadata(&metadata_input, effective_role);
    // Probed mode: one bounded Fast-tier self-assessment call, fused BEFORE
    // anything downstream reads the metadata — need plans, shape evidence,
    // the Default-role difficulty gate, and the policy context all see the
    // fused complexity/risk, so a probe-raised verdict routes exactly like a
    // keyword-raised one. Any probe failure leaves the deterministic verdict
    // untouched (`route_probe_assessment` fails open to `None`).
    let mut probe_informed = false;
    // What the probe said about ITSELF, kept for every probe that ARRIVED —
    // including the `low` verdicts `apply_probe_to_metadata` discards on its
    // first line. `routeSource` already records whether a probe informed the
    // route; without this, a declined `low` probe looks exactly like no probe
    // at all, and `low` is the bucket a calibration read needs most: the whole
    // question is whether a stated confidence predicts being right.
    let mut probe_confidence = None;
    if settings.auto_classifier == RouteAutoClassifierMode::Probed {
        if let Some(probe) = probe {
            probe_confidence = Some(probe.confidence.as_label().to_string());
            probe_informed = apply_probe_to_metadata(&mut metadata, *probe);
        }
    }
    // Learned complexity calibration: a (role, complexity) class the outcome
    // log shows keeps failing at its band is floored one band up — after the
    // probe (so the promotion applies to the fused verdict) and before
    // anything downstream reads the metadata. Confidence is recomputed over
    // the promoted band inside the helper, mirroring probe fusion.
    let calibration_promoted = apply_calibration_to_metadata(
        &mut metadata,
        &context.calibration,
        role_key(effective_role),
    );
    let need_plans = plan_agent_needs(&metadata);
    let mut evidence = infer_route_shape_evidence(&RouteEvidenceInput {
        subagent_type,
        name,
        description,
        prompt,
        workflow_member,
        fanout_position,
        auto_classifier: settings.auto_classifier,
    });
    if calibration_promoted {
        evidence.audit_notes.push(format!(
            "smart-calibration:promoted-to-{}",
            metadata.complexity.as_label()
        ));
    }
    let shape_input = shape_input_with_evidence(&metadata, &need_plans, &evidence);
    let shape_decision = select_route_shape(&shape_input);
    // Generic work (Default role — no specialty inferred) can still produce an
    // audit route based on task difficulty, but that route is not applied as a
    // sub-agent model override. Only a truly uninferable task (Unknown
    // complexity — an empty description/prompt) has no useful route evidence, so
    // it stays quiet rather than recording a misleading route reason.
    if fallback_role == RouteRole::Default && metadata.complexity == RouteTaskComplexity::Unknown {
        return None;
    }

    // Outcome feedback is keyed by the resolved subagent type the spawn records its
    // outcome under (so role-fallback routes learn too, not just explicit subagents
    // — they record + look up under the same inferred type).
    let feedback = context.feedback_for(&resolved_subagent_type);
    // Phase 5 exploration gates on the EFFECTIVE role (the target's route
    // role hint when one exists, e.g. a `code-reviewer` subagent type is
    // Reviewer regardless of what free-text inference alone would guess) —
    // the same role `RouteRequest::effective_role()` resolves to once the
    // request below exists, computed a step early here so the hard
    // risk/role gates see exactly what the engine will treat as this
    // route's role.
    let route_key = format!("subagent:{resolved_subagent_type}");
    let (exploration_slot, exploration_decisive_counts) =
        context.exploration_slot_for(&route_key, effective_role, metadata.risk);
    let injected_learned_specialty =
        learned_specialty_for_real_request(settings.learned_specialty, &context.learned_specialty);
    let mut request = RouteRequest::for_target(target, fallback_role, parent_model)
        .with_context(route_policy_context(
            &metadata,
            prior_failures,
            shape_decision.shape,
            evidence.lane,
            settings.allow_cross_provider_diversity,
            settings.provider_allowlist.clone(),
            feedback,
            settings.auto_classifier,
            route_audit_notes(&shape_decision, evidence.audit_notes),
            context.cooldown_providers.clone(),
            context.provider_headroom.clone(),
            settings.headroom_penalty_threshold,
            exploration_slot,
            exploration_decisive_counts,
            injected_learned_specialty,
            settings.policy,
        ));
    // Actual write intent outranks a shell/network co-word for the execution
    // policy, including writing/design specialties that still edit files.
    let implementation_kind = if custom_target { "" } else { resolved_subagent_type.as_str() };
    if !crate::misc_tools::agent_tools::supports_file_edits(implementation_kind) {
        request.context.tool_need = runtime::RouteToolNeed::ReadOnly;
    } else if crate::misc_tools::agent_tools::implementation_task(implementation_kind, description, prompt) {
        request.context.tool_need = runtime::RouteToolNeed::Write;
    }
    apply_configured_route_override(&mut request, settings);
    let decision = route_model(&request, &context.inventory);
    let decision = annotate_learned_shadow_delta(
        decision,
        &request,
        &context.inventory,
        settings.learned_specialty,
        &context.learned_specialty,
    );
    let fallback_models = route_model_fallback_candidates(
        &request,
        &context.inventory,
        &decision.resolved_model,
        settings.fallback_candidate_limit,
    );
    let model_override = (decision.resolved_model != parent_model)
        .then(|| decision.resolved_model.clone());
    let effort = decision.recommended_effort.map(effort_ceiling_to_level);
    if model_override.is_none()
        && fallback_models.is_empty()
        && effort.is_none()
        && !matches!(effective_role, RouteRole::Coding | RouteRole::Debugging)
    {
        return None;
    }
    let reason = route_reason_is_worth_reporting(model_override.as_ref(), &decision)
        .then(|| compose_route_reason(&decision, effective_role, &metadata, prior_failures));
    let decision_meta = RouteDecisionMeta {
        role: route_role_label(effective_role),
        complexity: metadata.complexity.as_label().to_string(),
        risk: metadata.risk.as_label().to_string(),
        probe_confidence,
        // "+probe" marks routes whose classification a live self-assessment
        // informed, so the outcome store can compare probed vs deterministic
        // route quality later. Appended to the two AUTO-family picks a probe
        // actually shapes — "pin" is exact-matched by the Phase 6
        // pin-suppression filter (`learned.rs`), and the remaining labels are
        // availability signals a probe has no say in. `LearnedAdmission` is
        // included because leaving it out would bias that comparison: an
        // admitted pick would never be counted as probed, however the route was
        // classified.
        route_source: if probe_informed
            && matches!(
                decision.source,
                RouteDecisionSource::AutoSelector | RouteDecisionSource::LearnedAdmission
            ) {
            format!("{}+probe", route_source_label(decision.source))
        } else {
            route_source_label(decision.source).to_string()
        },
    };
    Some(SmartRouteChoice {
        model: model_override,
        reason,
        fallback_models,
        effort,
        decision_meta,
    })
}

/// Project the router's local, api-independent `EffortCeiling` axis onto the
/// provider-neutral `api::EffortLevel` scale — the (model × effort) analogue
/// of `model_inventory::effort_ceiling_for_model`'s reverse direction. A
/// simple widening map: `EffortCeiling` has no `Low`/`Medium` variants (only
/// the ceiling's top matters for routing), so those `EffortLevel` variants
/// are never produced here.
fn effort_ceiling_to_level(ceiling: EffortCeiling) -> api::EffortLevel {
    match ceiling {
        EffortCeiling::Ultra => api::EffortLevel::Ultra,
        EffortCeiling::Max => api::EffortLevel::Max,
        EffortCeiling::Xhigh => api::EffortLevel::Xhigh,
        EffortCeiling::High => api::EffortLevel::High,
    }
}

/// One human-readable line saying WHY the router would pick a route: role +
/// difficulty, selector outcome, and score adjustments that moved the pick
/// (feedback, cross-provider diversity, allowlist escapes). The reason is
/// stamped on the manifest for the TUI, while the resolved model is smuggled
/// alongside it (`ROUTE_MODEL_SMUGGLE_KEY`) and applied VERBATIM by the spawn
/// path — a deliberate cross-provider diversity route included — so a routed
/// agent actually runs on the picked model, not the parent. The router leaves a
/// user's explicit per-agent `model` untouched (it never routes those).
fn compose_route_reason(
    decision: &runtime::RouteDecision,
    role: RouteRole,
    metadata: &TaskRouteMetadata,
    prior_failures: u32,
) -> String {
    let mut reason = format!(
        "{role:?}·{complexity:?} — {}",
        decision.reason,
        complexity = metadata.complexity,
    );
    if decision.audit.cross_provider == Some(true) {
        reason.push_str(" · cross-provider");
    }
    let feedback = decision.audit.feedback_adjustment;
    if feedback != 0 {
        use std::fmt::Write as _;
        let _ = write!(reason, " · feedback {feedback:+}");
    }
    if prior_failures >= 2 {
        use std::fmt::Write as _;
        let _ = write!(reason, " · escalated after {prior_failures} failures");
    }
    if decision
        .audit
        .guardrails
        .iter()
        .any(|line| line.starts_with("provider-allowlist-escape:"))
    {
        reason.push_str(" · outside allowlist");
    }
    if decision.audit.guardrails.iter().any(|line| line == "quota-degraded") {
        reason.push_str(" · quota-degraded");
    }
    if decision.audit.guardrails.iter().any(|line| line == "exploration") {
        reason.push_str(" · exploration");
    }
    if let Some(model) = decision
        .audit
        .guardrails
        .iter()
        .find_map(|line| line.strip_prefix("learned-shadow-differs:"))
    {
        use std::fmt::Write as _;
        let _ = write!(reason, " · learned-shadow-differs:{model}");
    }
    reason
}

/// Phase 6 shadow-mode wiring: `request` already carries whatever
/// `injected_learned_specialty` (`smart_model_for_fields`) put into its
/// context for the REAL decision — an empty hint in `off`/`shadow` mode, the
/// live hint in `on` mode. In `Shadow` mode ONLY, additionally probe "what
/// would routing have picked with the learned hint applied for real": a
/// SECOND `route_model` call over the same in-memory inventory/request (no
/// IO — cheap), run ONLY when there is a non-empty computed hint to inject
/// at all (an empty hint can never change any pick, so the probe would be
/// wasted work). When the two disagree, stamp a
/// `learned-shadow-differs:<model>` note onto the REAL decision's audit
/// guardrails — the same extension point `"exploration"`/`"quota-degraded"`
/// already use — which `compose_route_reason` projects into the
/// human-readable `routeReason` (persisted on the manifest) as the cheap
/// read-back path for Phase 7's doctor delta table. `off`/`on` are no-ops:
/// `off` never computes a hint (`computed_hint` is already empty, so this
/// still no-ops even if called), and `on` already routed with the live hint
/// for real, so there is nothing left to compare against.
pub(super) fn annotate_learned_shadow_delta(
    mut decision: RouteDecision,
    request: &RouteRequest,
    inventory: &ModelInventory,
    mode: LearnedSpecialtyMode,
    computed_hint: &LearnedSpecialtyHint,
) -> RouteDecision {
    if mode != LearnedSpecialtyMode::Shadow || computed_hint.is_empty() {
        return decision;
    }
    let mut alt_request = request.clone();
    alt_request.context.learned_specialty = computed_hint.clone();
    let alt_decision = route_model(&alt_request, inventory);
    if alt_decision.resolved_model != decision.resolved_model {
        decision
            .audit
            .guardrails
            .push(format!("learned-shadow-differs:{}", alt_decision.resolved_model));
    }
    decision
}

/// `on` injects the real computed hint into the REAL request so the live
/// route actually blends it; `off`/`shadow` inject an EMPTY hint so the REAL
/// route stays seed-only (byte-identical to pre-Phase-6) — shadow's "what
/// would learned have picked" probe runs separately, AFTER the real
/// decision, via [`annotate_learned_shadow_delta`].
fn learned_specialty_for_real_request(
    mode: LearnedSpecialtyMode,
    computed_hint: &LearnedSpecialtyHint,
) -> LearnedSpecialtyHint {
    if mode == LearnedSpecialtyMode::On {
        computed_hint.clone()
    } else {
        LearnedSpecialtyHint::disabled()
    }
}

/// Whether `smart_model_for_fields` should surface the human-readable
/// `routeReason` at all: a genuine model override, OR a Phase 6 shadow-mode
/// delta to report. The latter can exist even when the REAL route stayed on
/// the parent model (shadow mode never changes the real pick; it only ever
/// reports what learned specialty WOULD have picked instead).
fn route_reason_is_worth_reporting(model_override: Option<&String>, decision: &RouteDecision) -> bool {
    model_override.is_some()
        || decision
            .audit
            .guardrails
            .iter()
            .any(|line| line.starts_with("learned-shadow-differs:"))
}

fn route_audit_notes(
    shape_decision: &super::shape::RouteShapeDecision,
    mut evidence_notes: Vec<String>,
) -> Vec<String> {
    evidence_notes.push(format!("smart-route-shape:{}", shape_decision.shape.label()));
    evidence_notes.push(format!("smart-route-shape-confidence:{:?}", shape_decision.confidence));
    evidence_notes.push(format!("smart-route-shape-outcome:{:?}", shape_decision.outcome));
    evidence_notes.push(format!("smart-route-shape-reason:{}", shape_decision.reason));
    evidence_notes
}

#[allow(clippy::too_many_arguments)]
fn route_policy_context(
    metadata: &TaskRouteMetadata,
    prior_failures: u32,
    shape: RouteShape,
    lane: Option<LaneRouteMetadata>,
    allow_cross_provider_diversity: bool,
    provider_allowlist: Vec<String>,
    feedback: RouteFeedbackHint,
    auto_classifier: RouteAutoClassifierMode,
    mut audit_notes: Vec<String>,
    cooldown_providers: Vec<String>,
    provider_headroom: Vec<(String, u8)>,
    headroom_penalty_threshold: u8,
    exploration_slot: Option<u32>,
    exploration_decisive_counts: Vec<(String, u32)>,
    learned_specialty: LearnedSpecialtyHint,
    policy: runtime::SmartPolicy,
) -> RoutePolicyContext {
    audit_notes.push(format!("smart-route-confidence:{:?}", metadata.confidence));
    audit_notes.push(auto_classifier.audit_note().to_string());
    RoutePolicyContext {
        risk: metadata.risk,
        complexity: metadata.complexity,
        prior_failures,
        context_need: metadata.context_need,
        tool_need: metadata.tool_need,
        verification_need: metadata.verification_need,
        route_shape: Some(shape),
        lane,
        allow_cross_provider_diversity,
        provider_allowlist,
        feedback,
        audit_notes,
        cooldown_providers,
        provider_headroom,
        headroom_penalty_threshold,
        exploration_slot,
        exploration_decisive_counts,
        learned_specialty,
        policy,
    }
}
