use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use api::{
    AnthropicClient, AuthRoute, ContentBlockDelta, MessageRequest, ProviderClient, ProviderKind,
    StreamEvent as ApiStreamEvent, ThinkingConfig, ToolChoice, ToolDefinition, detect_provider_kind,
    max_tokens_for_model, resolve_model_alias,
};
use runtime::{ApiClient, ApiRequest, AssistantEvent, ProviderStateBlob, RuntimeError};

use super::super::{ToolSpec, mvp_tool_specs};
use super::completion::{notify_agent_starvation, starvation_notice};
use super::manifest::epoch_seconds_now_u64;
use super::manifest::{
    append_agent_output_tail, record_agent_phase, record_agent_provider_event,
    record_agent_reasoning_activity, record_agent_retry_cause, record_agent_runtime_model,
    record_agent_stream_notice, record_agent_stream_open, record_agent_task_activity,
    record_provider_request_finished, retain_output_tail_window, trim_agent_output_tail_suffix,
};
use super::rate_limit::{
    QUOTA_SNAPSHOT_FRESH, RateGovernor, agent_rate_governor, binding_window,
    mark_capacity_stall_from, rate_limit_cooldown_remaining_ms, rate_limit_headroom_low,
    shared_agent_runtime, wait_for_rate_limit_cooldown_cancellable, workflow_rate_governor,
};
use super::subagent_profile::starvation_demotion;

pub(crate) struct ProviderRuntimeClient {
    run_generation: Option<u64>,
    handle: tokio::runtime::Handle,
    client: ProviderClient,
    model: String,
    allowed_tools: BTreeSet<String>,
    /// Inherited parent-session MCP tool schemas (already filtered by the
    /// allow-set), appended to the builtin definitions each turn so the model
    /// can call them; the executor's passthrough serves the calls.
    mcp_tools: Vec<ToolDefinition>,
    structured_schema: Option<serde_json::Value>,
    /// Per-turn `output_tokens` capture for the sidebar sparkline. Capped
    /// at [`TOKEN_HISTORY_CAP`] to avoid unbounded growth across long
    /// agent runs. Wrapped in `Arc<Mutex<…>>` so the spawning caller
    /// (`run_agent_job`) can read it back when the conversation ends
    /// without threading the value through `ConversationRuntime`.
    ///
    /// **Display only.** This buffer is lossy by design (oldest samples drop at
    /// the cap), so it must never be the budget source — use
    /// [`Self::output_tokens_total`] for that.
    token_history: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
    /// Never-capped running sum of every turn's `output_tokens` — the workflow
    /// `max_output_tokens` budget's source of truth, decoupled from the
    /// display-bounded `token_history` so a maxed-out (or retrying) agent's spend
    /// is counted in full. The spawning caller reads it back after the turn.
    output_tokens_total: std::sync::Arc<AtomicU64>,
    /// Whether this agent belongs to a workflow run. Selects the workflow rate
    /// governor (higher ceiling) over the shared `SpawnMultiAgent` one. Carried
    /// from [`AgentJob::workflow_member`].
    workflow_member: bool,
    /// Optional thinking budget picked by the smart sub-agent router.
    thinking_budget_tokens: Option<u32>,
    /// Named reasoning-effort tier the Smart router recommends for this
    /// agent's route (smuggled `__zo_route_effort` → `AgentInput.route_effort`
    /// → `AgentJob.route_effort`; today only ever `Ultra`, and only when the
    /// resolved model's declared ceiling actually reaches it — see
    /// `model_router::policy::recommended_effort_for`). Merged with the
    /// budget-derived effort by RANK in [`Self::stream`] (never by
    /// re-deriving from the merged budget number — `api::effort_level_for_budget`
    /// structurally cannot produce `Ultra`), mirroring
    /// `runtime_bridge::effort_with_budget_floor`'s semantics for the main
    /// session turn (duplicated rather than shared cross-crate — `tools`
    /// cannot depend on the CLI crate). `None` is the byte-identical default:
    /// every code path that predates this field leaves it `None`.
    route_effort: Option<api::EffortLevel>,
    /// Optional per-call ceiling on provider-request concurrency, carried from
    /// the flat `SpawnMultiAgent` `concurrency` argument. Caps the governor's
    /// admission to `min(live_limit, this)` so a smaller user value actually
    /// tightens the real API concurrency (it used to bind only OS-thread spawn
    /// windowing and never reach the semaphore). `None` = governor ceiling only.
    api_concurrency: Option<usize>,
    /// Ranked host-computed fallback models to try before parking on a
    /// rate-limited provider or after repeated transient stream faults. Drained
    /// as candidates are attempted. Explicit model pins arrive with this list
    /// empty, so recovery never overrides user authority.
    rate_limit_fallback_models: VecDeque<String>,
    /// Cooperative cancel flag shared with the agent registry / foreground
    /// Ctrl+C. Observed while parked in an open-ended rate-limit cool-down and
    /// again at provider-event boundaries, so a late event cannot revive a
    /// timed-out agent. `None` on paths that never register a cancel signal
    /// (tests).
    cancel_signal: Option<runtime::HookAbortSignal>,
    /// Manifest to stamp with live wait-phase (`currentPhase`) and streamed
    /// output tail (`outputTail`) so the parent HUD / agent viewer shows what
    /// this agent is actually doing — including the invisible waits (governor
    /// admission queue, rate-limit cool-down). `None` for test paths.
    manifest_path: Option<std::path::PathBuf>,
    /// `(agent_id, display name)` for the W9-3 starvation notice posted to the
    /// parent transcript. `None` (tests, workflow internals) silently skips
    /// the notice — starvation still shows on the manifest phase label.
    agent_identity: Option<(String, String)>,
}

/// W9-1: consecutive absorbed 429s on a single turn before starvation
/// fallback (for example opus → sonnet → haiku) kicks in. Five hits walk the
/// entire cool-down ladder (15+30+60+120+120 s ≈ 5.8 min parked) — long enough
/// to be a genuine starvation signal rather than a blip, short enough to rescue
/// the 2026-06-10 live incident (16 min of zero tool calls) well before half-way.
const STARVATION_DEMOTE_AFTER_429S: u32 = 5;
/// W9-3: cumulative rate-limit park time on one turn before the one-shot
/// parent-transcript warning. Mirrors the 5-minute spawn headroom window so
/// the two starvation surfaces agree on what "stuck for a while" means.
const STARVATION_WARN_AFTER_MS: u64 = 300_000;
/// Terminal give-up: once an agent has fallen all the way to the bottom tier
/// (no lower model to fall to) and is *still* hit by this many 429s on that
/// tier, the provider quota is genuinely exhausted. Keep absorbing the higher
/// tiers indefinitely (a 429 there is recoverable by lower-tier fallback), but
/// stop the bottom tier from retrying forever — otherwise one starved agent
/// hangs in `[running]` and the whole fan-out never completes (the user-reported
/// "에이전트가 멈춘듯해..완료알림을 못받거나"). Five matches the per-tier fallback
/// budget, so the bottom tier gets the same fair chance every tier above it got.
const STARVATION_GIVE_UP_BOTTOM_429S: u32 = 5;

/// A transient stream fault gets one decisive retry on the selected provider.
/// A second fault may consume one Smart-router candidate from a *different*
/// provider; the third fault ends the turn so a dead backend cannot rebuild the
/// old 8 × 90 s (~12 minute) tail. This mirrors the workflow startup recovery
/// contract: selected provider, one same-provider retry, one bounded alternate.
const TRANSIENT_PROVIDER_FALLBACK_AFTER_FAILURES: u32 = 2;
const MAX_TRANSIENT_FAILURES: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransientRecoveryStage {
    RetrySameProvider,
    TryFallbackProvider,
    Exhausted,
}

fn record_transient_failure(
    failures: &mut u32,
    fallback_attempted: bool,
) -> TransientRecoveryStage {
    *failures = (*failures).saturating_add(1);
    if *failures >= MAX_TRANSIENT_FAILURES {
        TransientRecoveryStage::Exhausted
    } else if *failures >= TRANSIENT_PROVIDER_FALLBACK_AFTER_FAILURES && !fallback_attempted {
        TransientRecoveryStage::TryFallbackProvider
    } else {
        TransientRecoveryStage::RetrySameProvider
    }
}

/// Maximum number of token samples kept per agent. 64 samples covers the
/// typical 20-30 turn ceiling with headroom while staying well under the
/// sidebar Sparkline's visual width (~16 cells with truncation).
const TOKEN_HISTORY_CAP: usize = 64;

/// Minimum interval between manifest `outputTail` flushes.
///
/// This is intentionally tighter than the parent TUI's live-manifest poll
/// cadence: sub-agent prose does not flow through the foreground
/// `RenderBlock::TextDelta` pacer, it reaches the inline/pinned agent tree via
/// the manifest's rolling `outputTail`. A long writer-side hold-back combines
/// with the reader poll into a visible "pause, then dump a chunk" burst. Keep
/// writes coalesced enough to avoid one manifest write per token, but short
/// enough that live agent output feels continuous.
const TAIL_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);
/// Pending-buffer size that forces a flush ahead of the interval.
const TAIL_FLUSH_PENDING_CHARS: usize = 120;
/// Reasoning streams can emit many tiny deltas. Persist the first immediately,
/// then coalesce heartbeat writes so observability does not become token-rate
/// disk I/O.
const REASONING_ACTIVITY_FLUSH_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(5);

/// Best-effort live-phase stamp on the agent manifest (no-op without a path).
fn stamp_phase(manifest_path: Option<&std::path::Path>, phase: Option<&str>) {
    if let Some(path) = manifest_path {
        record_agent_phase(path, phase);
    }
}

/// Holds the manifest's "a provider request is in flight" claim for the life of
/// one attempt.
///
/// The claim must be released on *every* way out of the attempt — clean stream,
/// mid-stream error, rate-limit retry, transient fallback, cancellation — and
/// the retry loop has a dozen of them, so `Drop` owns the release rather than
/// each exit remembering to. What it buys: a turn the model thinks about
/// silently streams no frame, refreshes no heartbeat, and would otherwise be
/// cancelled by the fan-out deadline as if it had hung.
struct ProviderRequestGuard {
    manifest_path: Option<std::path::PathBuf>,
}

impl ProviderRequestGuard {
    fn open(
        manifest_path: Option<std::path::PathBuf>,
        effective_effort: Option<&str>,
        thinking_budget_tokens: Option<u32>,
    ) -> Self {
        if let Some(path) = manifest_path.as_deref() {
            record_agent_stream_open(path, effective_effort, thinking_budget_tokens);
        }
        Self { manifest_path }
    }
}

impl Drop for ProviderRequestGuard {
    fn drop(&mut self) {
        if let Some(path) = self.manifest_path.as_deref() {
            record_provider_request_finished(path);
        }
    }
}

/// Throttled writer for the manifest's `outputTail`: buffers streamed text
/// deltas and flushes at most ~once per [`TAIL_FLUSH_INTERVAL`] (or earlier
/// when the buffer passes [`TAIL_FLUSH_PENDING_CHARS`]), so live output
/// reaches the agent viewer without one manifest write per delta.
struct TailFlusher {
    manifest_path: Option<std::path::PathBuf>,
    pending: String,
    flushed_attempt: String,
    last_flush: std::time::Instant,
    wrote_any: bool,
    segment_break: bool,
}

impl TailFlusher {
    fn new(manifest_path: Option<std::path::PathBuf>) -> Self {
        Self {
            manifest_path,
            pending: String::new(),
            flushed_attempt: String::new(),
            last_flush: std::time::Instant::now(),
            wrote_any: false,
            segment_break: false,
        }
    }

    /// Note that the next text belongs to a new turn.
    ///
    /// The separator cannot live in the flush path: flushes fire on a character
    /// count or an interval, so they land mid-word and would shred the prose.
    /// A turn boundary is the one seam where the model stopped, called a tool,
    /// and started speaking again — which is where the rolling tail used to read
    /// `…involved in both blockers.The nightly snapshot…`, two sentences fused
    /// with nothing between them.
    fn mark_segment_break(&mut self) {
        self.segment_break = true;
    }

    fn push(&mut self, text: &str) {
        if self.manifest_path.is_none() {
            return;
        }
        if std::mem::take(&mut self.segment_break)
            && self.wrote_any
            && !text.starts_with(['\n', '\r', ' ', '\t'])
        {
            self.pending.push('\n');
        }
        self.wrote_any = true;
        self.pending.push_str(text);
        if self.pending.chars().count() >= TAIL_FLUSH_PENDING_CHARS
            || self.last_flush.elapsed() >= TAIL_FLUSH_INTERVAL
        {
            self.flush();
        }
    }

    /// Drop the current streaming attempt's live tail (used before a clean turn
    /// restart, where the provider re-streams the same text and the viewer would
    /// otherwise show duplicate partial prose). Pending buffered text is local;
    /// already-flushed text is rolled back only when the manifest still ends in
    /// exactly the suffix this flusher wrote during the attempt.
    fn discard_attempt_tail(&mut self) {
        self.pending.clear();
        if let Some(path) = &self.manifest_path {
            trim_agent_output_tail_suffix(path, &self.flushed_attempt);
        }
        self.flushed_attempt.clear();
        self.last_flush = std::time::Instant::now();
    }

    fn flush(&mut self) {
        if self.pending.is_empty() {
            self.last_flush = std::time::Instant::now();
            return;
        }
        if let Some(path) = &self.manifest_path {
            append_agent_output_tail(path, &self.pending);
        }
        self.flushed_attempt.push_str(&self.pending);
        retain_output_tail_window(&mut self.flushed_attempt);
        self.pending.clear();
        self.last_flush = std::time::Instant::now();
    }
}

fn agent_model_route(model: &str) -> (String, String) {
    let selection = resolve_model_alias(model);
    let wire = api::wire_model_id(&selection);
    (selection, wire)
}

impl ProviderRuntimeClient {
    /// Variant accepting an externally-shared `token_history` handle so the
    /// spawner can keep its own reference and read the final series back
    /// for persistence into the agent manifest (Phase 4 sparkline data).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new_with_history(
        model: &str,
        allowed_tools: BTreeSet<String>,
        token_history: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
        output_tokens_total: std::sync::Arc<AtomicU64>,
        workflow_member: bool,
        thinking_budget_tokens: Option<u32>,
        route_effort: Option<api::EffortLevel>,
        api_concurrency: Option<usize>,
        rate_limit_fallback_models: Vec<String>,
        cancel_signal: Option<runtime::HookAbortSignal>,
    ) -> Result<Self, String> {
        let (selection, model) = agent_model_route(model);
        let client = build_provider_client_for_agent_with_rate_limit_failover(
            &selection,
            !rate_limit_fallback_models.is_empty(),
        )?;
        Ok(Self {
            handle: shared_agent_runtime().handle().clone(),
            client,
            model,
            allowed_tools,
            mcp_tools: Vec::new(),
            structured_schema: None,
            token_history,
            output_tokens_total,
            workflow_member,
            thinking_budget_tokens,
            route_effort,
            api_concurrency,
            rate_limit_fallback_models: rate_limit_fallback_models.into(),
            cancel_signal,
            manifest_path: None,
            run_generation: None,
            agent_identity: None,
        })
    }

    /// Attach inherited parent-session MCP tool schemas (see the field docs).
    /// Builder-style, mirroring [`Self::with_manifest_path`].
    pub(super) fn with_mcp_tools(mut self, mcp_tools: Vec<ToolDefinition>) -> Self {
        self.mcp_tools = mcp_tools;
        self
    }

    pub(super) fn with_structured_schema(mut self, schema: Option<serde_json::Value>) -> Self {
        self.structured_schema = schema;
        self
    }

    /// Attach the agent manifest this client stamps with live wait-phase and
    /// streamed output tail. Builder-style, mirroring the tool executor's
    /// `with_manifest_path`.
    pub(super) fn with_run_generation(mut self, generation: u64) -> Self {
        self.run_generation = Some(generation);
        self
    }

    pub(super) fn with_manifest_path(mut self, path: std::path::PathBuf) -> Self {
        self.manifest_path = Some(path);
        self
    }

    /// Attach the `(agent_id, display name)` used by the W9-3 starvation
    /// notice. Builder-style, mirroring [`Self::with_manifest_path`].
    ///
    /// Also pins the provider prompt-cache scope to the agent id: the spawn
    /// path builds its ChatGPT client outside `build_provider_client` (which
    /// pins the main session's scope), so without this hop the spawn's cache
    /// key rides the client's random per-instance id and rolls on every
    /// mid-run model swap ([`Self::switch_runtime_model`] rebuilds the
    /// client). The agent id is stable for the agent's whole lifetime, so the
    /// key survives those rebuilds.
    pub(super) fn with_agent_identity(mut self, agent_id: String, name: String) -> Self {
        self.client = pin_agent_cache(self.client, &agent_id);
        self.agent_identity = Some((agent_id, name));
        self
    }

    /// W9-3: post a one-shot starvation message to the parent transcript via
    /// the completion channel. No-op without an identity (tests, paths that
    /// never registered one) — the manifest phase label still tells the story.
    fn send_starvation_notice(&self, message: String) {
        if let Some((agent_id, name)) = &self.agent_identity {
            notify_agent_starvation(starvation_notice(agent_id, name, message));
        }
    }

    fn switch_runtime_model(
        &mut self,
        message_request: &mut MessageRequest,
        next_model: &str,
    ) -> Result<String, String> {
        let (selection, resolved) = agent_model_route(next_model);
        if selection == self.model {
            return Ok(self.model.clone());
        }
        let client = build_provider_client_for_agent_with_rate_limit_failover(
            &selection,
            !self.rate_limit_fallback_models.is_empty(),
        )?;
        // Re-pin the prompt-cache scope (see [`Self::with_agent_identity`]):
        // the rebuild replaces the whole client, and an unpinned one would
        // roll the provider cache key on this very swap.
        let client = match self.prompt_cache_session_id() {
            Some(scope) => pin_agent_cache(client, scope),
            None => client,
        };
        let from = std::mem::replace(&mut self.model, resolved.clone());
        self.client = client;
        message_request.model.clone_from(&resolved);
        message_request.max_tokens = max_tokens_for_model(&resolved);
        if let Some(path) = self.manifest_path.as_deref() {
            record_agent_runtime_model(path, &resolved);
        }
        Ok(from)
    }

    /// Try a host-computed parent/Smart-router alternate before parking on a
    /// rate-limited provider. This is separate from starvation fallback: the
    /// fallback list may cross provider families, whereas the starvation ladder
    /// only walks provider-local tiers after repeated 429s. Anthropic callers
    /// fall through without consuming a candidate while a fresh binding-window
    /// reading is below the 95% model-swap boundary.
    fn try_rate_limit_model_fallback(
        &mut self,
        message_request: &mut MessageRequest,
        rl_hits: &mut u32,
        rl_backoff_step: &mut u32,
    ) -> bool {
        if !api::quota::quota_fallback_permitted(self.provider_kind()) {
            return false;
        }
        while let Some(candidate) = next_rate_limit_fallback_candidate(
            &mut self.rate_limit_fallback_models,
            &self.model,
        ) {
            match self.switch_runtime_model(message_request, &candidate) {
                Ok(from) => {
                    stamp_phase(
                        self.manifest_path.as_deref(),
                        Some(&format!(
                            "rate-limited · fallback {from} → {}",
                            self.model
                        )),
                    );
                    self.send_starvation_notice(format!(
                        "rate-limited on {from}{suffix} — retrying on fallback {}",
                        self.model,
                        suffix = oauth_window_suffix(),
                    ));
                    *rl_hits = 0;
                    *rl_backoff_step = 0;
                    return true;
                }
                Err(error) => {
                    self.send_starvation_notice(format!(
                        "rate-limit fallback candidate {candidate} unavailable ({error}); trying next candidate"
                    ));
                }
            }
        }
        false
    }

    /// After one same-provider retry, escape a repeatedly stalled transport via
    /// the Smart router's first candidate on a different provider. Same-provider
    /// variants are discarded: changing Sol speed/tier still reaches the same
    /// unhealthy backend. The inventory lookup preserves distinct configured
    /// providers such as `OpenAI` and `DeepSeek` even when both use an
    /// OpenAI-compatible wire protocol.
    fn try_transient_model_fallback(
        &mut self,
        message_request: &mut MessageRequest,
    ) -> bool {
        let inventory = runtime::connected_model_inventory(&self.model);
        while let Some(candidate) = next_cross_provider_fallback_candidate(
            &mut self.rate_limit_fallback_models,
            &self.model,
            &inventory,
        ) {
            match self.switch_runtime_model(message_request, &candidate) {
                Ok(from) => {
                    stamp_phase(
                        self.manifest_path.as_deref(),
                        Some(&format!(
                            "transient fault · fallback {from} → {}",
                            self.model
                        )),
                    );
                    if let Some(path) = self.manifest_path.as_deref() {
                        record_agent_retry_cause(path, "provider_transient_fallback");
                    }
                    self.send_starvation_notice(format!(
                        "repeated transient stream faults on {from} — retrying once on fallback {}",
                        self.model
                    ));
                    return true;
                }
                Err(error) => {
                    self.send_starvation_notice(format!(
                        "transient fallback candidate {candidate} unavailable ({error}); trying next provider"
                    ));
                }
            }
        }
        false
    }

    /// Record one retryable non-rate-limit failure and choose the bounded next
    /// step. Returns whether the caller should restart the turn. A successful
    /// provider switch is observable by comparing `self.model` before/after.
    fn prepare_transient_retry(
        &mut self,
        message_request: &mut MessageRequest,
        transient_failures: &mut u32,
        transient_fallback_attempted: &mut bool,
        cause: &str,
    ) -> bool {
        if let Some(path) = self.manifest_path.as_deref() {
            record_agent_retry_cause(path, cause);
        }
        match record_transient_failure(transient_failures, *transient_fallback_attempted) {
            TransientRecoveryStage::RetrySameProvider => {
                stamp_phase(
                    self.manifest_path.as_deref(),
                    Some(&format!(
                        "retrying after transient fault ({transient_failures}/{MAX_TRANSIENT_FAILURES})"
                    )),
                );
                true
            }
            TransientRecoveryStage::TryFallbackProvider => {
                *transient_fallback_attempted = true;
                if self.try_transient_model_fallback(message_request) {
                    true
                } else {
                    stamp_phase(
                        self.manifest_path.as_deref(),
                        Some(&format!(
                            "no alternate provider · final transient retry ({transient_failures}/{MAX_TRANSIENT_FAILURES})"
                        )),
                    );
                    true
                }
            }
            TransientRecoveryStage::Exhausted => {
                stamp_phase(
                    self.manifest_path.as_deref(),
                    Some(&format!(
                        "transient retry budget exhausted ({transient_failures}/{MAX_TRANSIENT_FAILURES})"
                    )),
                );
                false
            }
        }
    }

    /// W9-1 starvation fallback: after [`STARVATION_DEMOTE_AFTER_429S`]
    /// consecutive absorbed 429s on this turn, switch to one provider-local
    /// fallback tier and retry there instead of cycling the throttled tier
    /// forever. This ladder deliberately bypasses the Anthropic 95% model-swap
    /// gate: it is the bounded escape from prolonged zero-progress starvation.
    /// Resets the 429 streak and backoff ladder for the new tier. No-op when no
    /// lower tier exists or the streak is still short.
    fn try_starvation_demotion(
        &mut self,
        message_request: &mut MessageRequest,
        rl_hits: &mut u32,
        rl_backoff_step: &mut u32,
    ) {
        if *rl_hits < STARVATION_DEMOTE_AFTER_429S {
            return;
        }
        let Some(alias) = starvation_demotion(&self.model) else {
            return;
        };
        let demoted = resolve_model_alias(&alias);
        let Ok(from) = self.switch_runtime_model(message_request, &demoted) else {
            return;
        };
        stamp_phase(
            self.manifest_path.as_deref(),
            Some(&format!(
                "rate-limit starvation: fallback {from} · retrying on {demoted}"
            )),
        );
        self.send_starvation_notice(format!(
            "starved by rate-limit (429 ×{hits}){suffix} — fallback {from} → {demoted} and retrying",
            hits = *rl_hits,
            suffix = oauth_window_suffix(),
        ));
        *rl_hits = 0;
        *rl_backoff_step = 0;
    }

    /// This agent's provider bucket, used to scope every rate-limit surface
    /// (governor + cool-down window) so a 429 on one provider never throttles a
    /// sibling agent on another. Derived from the live client, which already
    /// knows its provider.
    fn provider_kind(&self) -> ProviderKind {
        self.client.provider_kind()
    }

    fn prompt_cache_session_id(&self) -> Option<&str> {
        self.agent_identity.as_ref().map(|(agent_id, _)| agent_id.as_str())
    }

    /// The adaptive rate governor this agent admits through: the higher-ceiling
    /// workflow governor for workflow members, else the flat fan-out governor.
    /// Scoped to this agent's provider so each provider ramps independently.
    fn governor(&self) -> &'static RateGovernor {
        if self.workflow_member {
            workflow_rate_governor(self.provider_kind())
        } else {
            agent_rate_governor(self.provider_kind())
        }
    }

    /// Borrow the cancel flag for cooperative cool-down polling, if registered.
    fn cancel_flag(&self) -> Option<&std::sync::atomic::AtomicBool> {
        self.cancel_signal
            .as_ref()
            .map(runtime::HookAbortSignal::flag)
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_signal
            .as_ref()
            .is_some_and(runtime::HookAbortSignal::is_aborted)
    }

    /// Record one turn's output-token count on both surfaces it feeds: the
    /// never-lossy budget total and the capped sparkline history. Both
    /// observation points — the streamed `MessageDelta` and the non-stream
    /// fallback — funnel through here so the budget total and the display buffer
    /// cannot diverge, and so the fallback path is no longer a silent accounting
    /// hole. A zero sample is a no-op.
    fn record_output_sample(&self, output_tokens: u32) {
        record_output_sample_into(
            &self.output_tokens_total,
            &self.token_history,
            output_tokens,
        );
    }

    /// One-shot 401 recovery, mirroring the interactive client's
    /// `refresh_claude_oauth` hook: re-resolve the credential chain (keychain
    /// refresh included) under the api layer's single-flight lock and install
    /// the fresh bearer on the live client. Sub-agents previously died on the
    /// first 401 — fatal for a long-running agent whose inherited bearer
    /// lapsed mid-run, while the parent (blocked waiting on this very agent)
    /// could not refresh on its behalf. Returns whether a fresh credential was
    /// installed and the failed call should be retried.
    async fn try_recover_unauthorized(&mut self) -> bool {
        let ProviderClient::Anthropic(client) = &self.client else {
            return false;
        };
        // Only an OAuth bearer can be refreshed; an env-pinned API key that
        // 401s is simply a bad key — re-resolving would hand the same key back
        // and burn a doomed retry.
        let Some(stale) = client.auth().bearer_token().map(str::to_string) else {
            return false;
        };
        // The resolve chain does blocking keychain + token-endpoint round
        // trips; hop off the async task so they cannot stall the shared agent
        // runtime's cooperative scheduling.
        let fresh = tokio::task::spawn_blocking(move || {
            api::refresh_claude_auth_after_unauthorized(Some(&stale))
        })
        .await
        .ok()
        .flatten();
        match (fresh, &mut self.client) {
            (Some(fresh), ProviderClient::Anthropic(client)) => {
                client.set_auth(fresh);
                true
            }
            _ => false,
        }
    }
}

/// Accumulate one turn's `output_tokens` onto the never-lossy budget total and
/// the capped display sparkline. Free-standing (not a method) so it is testable
/// without the credentials-bound `ProviderRuntimeClient` constructor: the budget
/// total must equal the full sum even after the display buffer drops old samples.
fn record_output_sample_into(
    output_tokens_total: &AtomicU64,
    token_history: &std::sync::Mutex<Vec<u32>>,
    output_tokens: u32,
) {
    if output_tokens == 0 {
        return;
    }
    output_tokens_total.fetch_add(u64::from(output_tokens), Ordering::Relaxed);
    if let Ok(mut hist) = token_history.lock() {
        hist.push(output_tokens);
        if hist.len() > TOKEN_HISTORY_CAP {
            let excess = hist.len() - TOKEN_HISTORY_CAP;
            hist.drain(0..excess);
        }
    }
}

/// Reasoning budget paired with a route-recommended named effort so it never
/// ships with a tiny default thinking allowance. Above `Max`'s `24_000` preset
/// so a budget-derived comparison — which can never itself reach `Ultra`,
/// `api::effort_level_for_budget` has no such arm — still never outranks the
/// named tier in [`merge_route_effort`]. Mirrors the CLI's
/// `Effort::Smart::budget()` preset (`28_000`, unchanged by the P9
/// `Ultracode` → `Smart` rename), the codebase's existing convention for
/// this exact number.
const ROUTE_EFFORT_MIN_BUDGET_TOKENS: u32 = 28_000;

/// Combine the deep-gate's numeric escalation floor with the numeric floor a
/// route-recommended named effort implies ([`ROUTE_EFFORT_MIN_BUDGET_TOKENS`]),
/// taking the max of whichever are present. `(None, None)` ⇒ `None`, so a
/// `route_effort`-free agent (the byte-identical default) sees exactly the
/// deep-gate floor it always has.
fn combined_effort_floor(deep_gate_floor: Option<u32>, route_effort: Option<api::EffortLevel>) -> Option<u32> {
    let route_floor = route_effort.map(|_| ROUTE_EFFORT_MIN_BUDGET_TOKENS);
    match (deep_gate_floor.filter(|&floor| floor > 0), route_floor) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Merge a route-recommended named effort with a budget-derived level by
/// RANK — never by re-deriving the level from the merged budget number
/// (`api::effort_level_for_budget` structurally cannot produce `Ultra`, so
/// re-deriving would silently lose it). A named tier is only displaced by a
/// budget-derived level that ranks STRICTLY higher (mirrors
/// `runtime_bridge::effort_with_budget_floor`'s main-session-turn semantics:
/// a floor may only raise, never lower, an explicit tier). `route_effort =
/// None` ⇒ the budget-derived level unchanged — the byte-identical default.
fn merge_route_effort(
    route_effort: Option<api::EffortLevel>,
    budget_derived: Option<api::EffortLevel>,
) -> Option<api::EffortLevel> {
    match (route_effort, budget_derived) {
        (Some(named), Some(budget)) if api::effort_rank(budget) > api::effort_rank(named) => {
            Some(budget)
        }
        (Some(named), _) => Some(named),
        (None, budget) => budget,
    }
}

pub(crate) fn build_provider_client_for_agent(model: &str) -> Result<ProviderClient, String> {
    build_provider_client_for_agent_with_rate_limit_failover(model, false)
}

fn build_provider_client_for_agent_with_rate_limit_failover(
    model: &str,
    fail_fast_on_rate_limit: bool,
) -> Result<ProviderClient, String> {
    let catalog = runtime::model_catalog::ModelCatalog::load().ok();
    let catalog_provider = catalog
        .as_ref()
        .and_then(|catalog| catalog.provider_for_model(model));
    let auth_route = catalog
        .as_ref()
        .and_then(|catalog| catalog.auth_route_for_model(model))
        .unwrap_or(AuthRoute::Auto);
    let provider_kind = catalog_provider.unwrap_or_else(|| detect_provider_kind(model));
    if provider_kind != ProviderKind::Anthropic {
        let client = if let Some(provider_kind) = catalog_provider {
            ProviderClient::from_provider_kind_with_auth_route(provider_kind, auth_route)
        } else {
            ProviderClient::from_model_with_auth_route(model, auth_route)
        };
        return client.map_err(|error| error.to_string());
    }

    // Inherit the parent runtime's credential when one is cached; otherwise run
    // the *same* resolution chain the interactive client uses (env → Claude
    // Code keychain, refreshing an expired token in place → saved `zo login`
    // OAuth). Sub-agents previously skipped the keychain and could land on a
    // scope-less saved token (403 every call) while the parent ran fine.
    //
    // Under a cloud gateway (Bedrock/Vertex) the gateway credential replaces
    // the first-party chain at send time, so sub-agents must not fail here
    // just because no Anthropic credential exists on the machine.
    let auth = match auth_route {
        AuthRoute::Auto => {
            match api::AuthSource::cached().or_else(api::resolve_claude_auth_fresh) {
                Some(auth) => auth,
                None if api::cloud_gateway_active() => api::AuthSource::None,
                None => {
                    return Err(
                        "Anthropic credentials unavailable for background agent".to_string()
                    );
                }
            }
        }
        AuthRoute::OAuth => api::AuthSource::from_oauth_only().map_err(|error| error.to_string())?,
        AuthRoute::ApiKey => {
            api::AuthSource::from_api_key_only().map_err(|error| error.to_string())?
        }
    };
    let mut client = AnthropicClient::from_auth(auth.clone()).with_base_url(api::read_base_url());
    if auth.bearer_token().is_some() && !api::cloud_gateway_active() {
        client = client.with_beta("oauth-2025-04-20");
    }
    // Server-side clear_tool_uses defaults on; the environment flag is an opt-out.
    client = client.with_env_context_editing();
    if fail_fast_on_rate_limit {
        client = client.with_rate_limit_fail_fast();
    }
    Ok(ProviderClient::Anthropic(client))
}

impl ApiClient for ProviderRuntimeClient {
    // A cohesive async streaming + rate-limit-retry state machine: the
    // `'retry` loop shares `rl_attempts`/`permit` across three failure points
    // (stream open, mid-stream, non-stream fallback), so splitting it would
    // scatter the retry semantics across helpers without making them clearer.
    #[allow(clippy::too_many_lines)]
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let mut tools = agent_tool_definitions(&self.allowed_tools, self.structured_schema.as_ref());
        // Inherited parent-session MCP tools ride along with the builtins; the
        // history reconcile below then treats them as known, so a stored MCP
        // tool_use never 400s the OpenAI-compatible path.
        tools.extend(self.mcp_tools.iter().cloned());
        // The sub-agent executor can resolve every builtin name; its explicit
        // allow-set is an authorization boundary, not a name-resolution one.
        // Anthropic may therefore keep that full history linked, while strict
        // providers remain limited to the actual request definitions.
        let resolvable: BTreeSet<String> = mvp_tool_specs()
            .iter()
            .map(|spec| spec.name.to_string())
            .chain(self.mcp_tools.iter().map(|definition| definition.name.clone()))
            .collect();
        let advertised: BTreeSet<String> = tools.iter().map(|def| def.name.clone()).collect();
        let known = if self.provider_kind() == ProviderKind::Anthropic {
            &resolvable
        } else {
            &advertised
        };
        let reconciled = runtime::session::reconcile_tool_history(
            &request.messages,
            known,
            &resolvable,
        );
        // Clamp the route-recommended effort to the internal tier THIS model
        // exposes (defensive: a starvation demotion can swap `self.model`
        // after the route was computed) before it feeds the floor/merge below.
        let route_effort = self
            .route_effort
            .map(|level| api::effective_effort_for_model(level, &self.model));
        let effort_budget = api::effort_budget_with_floor(
            self.thinking_budget_tokens.filter(|&budget| budget > 0),
            combined_effort_floor(request.effort_override, route_effort),
        );
        // Reminders ride the newest user message (see runtime_bridge) so the
        // system blocks and cached history stay byte-identical across turns.
        let mut messages = runtime::convert_messages_for(
            &reconciled,
            runtime::ReasoningReplay::for_model(&self.model),
        );
        runtime::append_wire_reminders(&mut messages, &request.wire_reminders);
        // Anchor-plus-rolling conversation-prefix breakpoints. Without them
        // only the system blocks below ever cache, so every iteration re-bills
        // its full transcript as uncached input — the dominant cost of a
        // tool-heavy agent loop. The foreground turn's anchor follows the
        // host's declared policy (`cache.anchorTtl`: a 1h anchor survives a
        // person's break); a sub-agent's tail dies within minutes, so its
        // markers stay at five minutes under every policy.
        if self.agent_identity.is_some() {
            runtime::mark_conversation_cache_breakpoints_short_lived(&mut messages);
        } else {
            runtime::mark_conversation_cache_breakpoints(&mut messages);
        }
        // Per-turn wire-model override (refusal → Opus family head). Only the
        // wire model id and its `max_tokens` change; the bound Anthropic client
        // is unchanged — the fallback target is Anthropic, same as the refused
        // Fable, and a refusal never arises on this client's non-Anthropic path.
        let wire_model = api::wire_model_id(
            request
                .model_override
                .as_deref()
                .unwrap_or(self.model.as_str()),
        );
        let mut message_request = MessageRequest {
            model: wire_model.clone(),
            max_tokens: max_tokens_for_model(&wire_model),
            messages,
            // CC 429-parity: lower the system prompt through the same
            // identity-splitting path the foreground turn uses. The Claude Max
            // OAuth fingerprint requires the FIRST system block to be exactly
            // the Claude Code identity line; sending the whole prompt as one
            // plain block (the old `system_from_string`) was rejected as a
            // fingerprint mismatch — surfaced as zo-only agent 429s that
            // Claude Code itself never hits. The split also restores the
            // cache_control breakpoints, so sub-agent turns stop re-paying the
            // full prompt every iteration (less token pressure on the shared
            // rate-limit window).
            system: agent_system_blocks(&request.system_prompt),
            tools: (!tools.is_empty()).then_some(tools),
            // 8c: honor a forced tool choice from the request (e.g. the final
            // StructuredOutput turn); otherwise default to `auto` when tools
            // exist, as before.
            tool_choice: request
                .tool_choice
                .clone()
                .or_else(|| (!self.allowed_tools.is_empty()).then_some(ToolChoice::Auto)),
            stream: true,
            // `effort_override` is a floor (deep-gate escalation); inert for
            // sub-agents (the foreground deep-gate never sets it on their
            // requests), but applied uniformly so the budget→effort derivation
            // is identical across every client. Computed once and reused for
            // both `thinking` and `effort` (mirrors the other two client sites).
            thinking: effort_budget.map(ThinkingConfig::enabled),
            output_config: None,
            // Derive provider-neutral effort from the budget so adaptive
            // Anthropic models and GPT backends get the right wire shape; the
            // Anthropic wire seam translates it to `output_config.effort`.
            // A route-recommended named effort (`route_effort`) is merged in
            // by RANK, never displaced by a budget-derived level that ranks
            // lower — see `merge_route_effort`.
            effort: merge_route_effort(route_effort, effort_budget.map(api::effort_level_for_budget)),
            effort_band_ceiling: None,
        };

        let effective_effort_label = message_request
            .effort
            .and_then(|effort| serde_json::to_value(effort).ok())
            .and_then(|value| value.as_str().map(str::to_string));

        // Cloned out so the async block below can borrow `self` mutably (the
        // 401 recovery swaps the client's auth in place).
        let handle = self.handle.clone();
        // Owned clone so the live-activity stamps below never borrow `self`.
        let manifest_path = self.manifest_path.clone();
        let mut tail = TailFlusher::new(manifest_path.clone());
        handle.block_on(async {
            // Background agents share the foreground turn's provider quota. The
            // adaptive `RateGovernor` admits a *live* number of concurrent
            // requests that halves on a 429 and ramps back up on sustained
            // success (AIMD), while a process-wide cool-down parks every agent
            // for the back-off / `Retry-After` window. On top of that this loop
            // treats a rate-limit as *absorbable, not fatal*: a 429 never spends
            // the retry budget — it re-waits the cool-down (cancellably) and
            // retries indefinitely, so a `SpawnMultiAgent`/workflow burst only
            // ever ends when quota clears or the user cancels. A genuinely
            // non-rate-limit error (auth, 400, schema) fails fast instead of
            // burning a retry budget; `transient_failures` bounds only the
            // non-rate-limit-but-retryable hiccups (transient network faults).
            // Cool-down backoff escalation index for rate-limit waits. Grows on
            // each rate-limit hit (never resets down) so a persistent throttle
            // ratchets toward the 120s cap, but it does *not* cap retry count.
            let mut rl_backoff_step: u32 = 0;
            // W9 starvation bookkeeping for this turn: consecutive absorbed
            // 429s, cumulative parked wall-clock, and the one-shot parent
            // warning latch.
            let mut rl_hits: u32 = 0;
            let mut rl_waited_ms: u64 = 0;
            let mut starvation_warned = false;
            let mut transient_failures: u32 = 0;
            let mut transient_fallback_attempted = false;
            // One recovery attempt per turn for an expired/invalid bearer
            // (HTTP 401): refresh the credential chain and retry, like the
            // foreground client. One-shot so genuinely revoked credentials
            // still fail fast instead of looping.
            let mut auth_recovery_attempted = false;
            'retry: loop {
                if self.is_cancelled() {
                    return Err(RuntimeError::new("agent cancelled"));
                }
                // Terminal give-up: once the agent has fallen to the bottom tier
                // (no lower model to fall to) and *still* repeatedly 429'd, the
                // provider quota is genuinely exhausted. Stop absorbing forever
                // so the fan-out completes with a partial result instead of this
                // agent hanging in `[running]` indefinitely. Higher tiers still
                // get their full fallback-and-retry — only the bottom tier gives
                // up.
                if starved_past_bottom_tier(&self.model, rl_hits) {
                    let waited = format_wait_brief(rl_waited_ms);
                    self.send_starvation_notice(format!(
                        "gave up on {} after {rl_hits} rate-limit retries ({waited} waited){} — \
                         provider quota exhausted; run fewer parallel agents or wait for the window \
                         to reset",
                        self.model,
                        oauth_window_suffix(),
                    ));
                    return Err(RuntimeError::with_provider_error_class(
                        format!(
                            "rate-limit starvation: gave up on {} after {rl_hits} retries ({waited} \
                             waited); provider quota exhausted — reduce parallel agents or wait for the \
                             quota window to reset",
                            self.model,
                        ),
                        api::ProviderErrorClass::account_rate_limit(None),
                    ));
                }
                // Live visibility: a parked agent must read as alive. Stamp the
                // cool-down wait (with its real remaining window) before
                // blocking on it, so the HUD shows *why* nothing is streaming.
                let cooldown_ms = rate_limit_cooldown_remaining_ms(self.provider_kind());
                if cooldown_ms > 1_000
                    && self.try_rate_limit_model_fallback(
                        &mut message_request,
                        &mut rl_hits,
                        &mut rl_backoff_step,
                    )
                {
                    auth_recovery_attempted = false;
                    continue 'retry;
                }
                if cooldown_ms > 1_000 {
                    stamp_phase(
                        manifest_path.as_deref(),
                        Some(&format!(
                            "{}{}",
                            rate_limit_phase_label(cooldown_ms, rl_hits, rl_waited_ms),
                            oauth_window_suffix(),
                        )),
                    );
                    // Account the park we are about to take (projected window;
                    // a cancel aborts the turn anyway), and post the one-shot
                    // parent warning once the cumulative park crosses the
                    // starvation horizon.
                    rl_waited_ms = rl_waited_ms.saturating_add(cooldown_ms);
                    if !starvation_warned && rl_waited_ms >= STARVATION_WARN_AFTER_MS && rl_hits > 0
                    {
                        starvation_warned = true;
                        self.send_starvation_notice(format!(
                            "rate-limit starved for {} (retry {rl_hits} on {}){} — still \
                             waiting for quota; consider `/smart status` or fewer parallel agents",
                            format_wait_brief(rl_waited_ms),
                            self.model,
                            oauth_window_suffix(),
                        ));
                    }
                }
                // Cancellable cool-down: an agent absorbing a long throttle still
                // wakes on a foreground Ctrl+C instead of being un-interruptible.
                if !wait_for_rate_limit_cooldown_cancellable(
                    self.provider_kind(),
                    self.cancel_flag(),
                )
                .await
                {
                    return Err(RuntimeError::new("agent cancelled"));
                }
                // If provider headroom is already low (recent 429 / hot quota),
                // try an available parent/Smart-router alternate. Anthropic's
                // fallback helper still refuses the swap below 95%, falling
                // through to same-model admission; a plain user `concurrency`
                // cap with healthy headroom also waits normally.
                if should_try_pre_slot_fallback(rate_limit_headroom_low(self.provider_kind()))
                    && self.try_rate_limit_model_fallback(
                        &mut message_request,
                        &mut rl_hits,
                        &mut rl_backoff_step,
                    )
                {
                    auth_recovery_attempted = false;
                    continue 'retry;
                }
                // Flat fan-outs start conservatively and may ramp only with
                // quota headroom; stamp the admission wait or queued siblings
                // look frozen.
                stamp_phase(manifest_path.as_deref(), Some("waiting for api slot"));
                // Adaptive admission: the governor's live limit, optionally
                // tightened by the per-call `concurrency` ceiling. The permit is
                // released (Drop) before any cool-down re-wait so a parked agent
                // never holds a slot hostage.
                let permit = self.governor().acquire(self.api_concurrency);
                // Request is about to open: the model is now genuinely working.
                // The first streamed-text flush (or tool start) clears this.
                stamp_phase(manifest_path.as_deref(), Some("thinking"));

                // Spans the non-stream fallback below too: it is part of the
                // same attempt, and it is the leg that streams nothing at all.
                let _provider_request = ProviderRequestGuard::open(
                    manifest_path.clone(),
                    effective_effort_label.as_deref(),
                    self.thinking_budget_tokens,
                );
                let mut request_usage = super::usage::RequestUsageGuard::open(
                    manifest_path.as_deref(),
                    self.run_generation,
                    &message_request.model,
                );
                let stream = match self.client.stream_message(&message_request).await {
                    Ok(stream) => stream,
                    Err(error) => {
                        request_usage.reject(&error);
                        if let Some(scope) = error.provider_error_class().capacity_scope() {
                            // Absorb: tighten concurrency, engage the cool-down
                            // (honoring `Retry-After`, and scoped to whichever
                            // window actually refused), and retry without
                            // spending the transient budget.
                            self.governor().on_rate_limit();
                            mark_capacity_stall_from(
                                self.provider_kind(),
                                scope,
                                error.retry_after(),
                                rl_backoff_step,
                            );
                            rl_backoff_step = rl_backoff_step.saturating_add(1);
                            rl_hits = rl_hits.saturating_add(1);
                            drop(permit);
                            if self.try_rate_limit_model_fallback(
                                &mut message_request,
                                &mut rl_hits,
                                &mut rl_backoff_step,
                            ) {
                                auth_recovery_attempted = false;
                                continue 'retry;
                            }
                            self.try_starvation_demotion(
                                &mut message_request,
                                &mut rl_hits,
                                &mut rl_backoff_step,
                            );
                            continue 'retry;
                        }
                        if error.is_unauthorized() && !auth_recovery_attempted {
                            auth_recovery_attempted = true;
                            drop(permit);
                            if self.try_recover_unauthorized().await {
                                continue 'retry;
                            }
                            return Err(RuntimeError::from_api_error(&error));
                        }
                        if error.is_retryable() {
                            drop(permit);
                            let previous_model = self.model.clone();
                            if self.prepare_transient_retry(
                                &mut message_request,
                                &mut transient_failures,
                                &mut transient_fallback_attempted,
                                "provider_transient_open",
                            ) {
                                if self.model != previous_model {
                                    auth_recovery_attempted = false;
                                }
                                continue 'retry;
                            }
                        }
                        return Err(RuntimeError::from_api_error(&error));
                    }
                };
                let notice_manifest_path = manifest_path.clone();
                let notice_cancel_signal = self.cancel_signal.clone();
                let mut stream = stream.with_stream_retry_notice(move |notice| {
                    if !notice_cancel_signal
                        .as_ref()
                        .is_some_and(runtime::HookAbortSignal::is_aborted)
                    {
                        if let Some(path) = notice_manifest_path.as_deref() {
                            record_agent_stream_notice(path, &notice);
                        }
                    }
                });
                let mut events = Vec::new();
                let mut pending_tools: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
                // Thinking blocks accumulated across `ThinkingDelta`/`SignatureDelta`
                // (text, signature), keyed by content-block index and flushed on
                // the block stop so a sub-agent's reasoning is stored and replayed
                // verbatim on its next Anthropic request — parity with the main
                // conversation parser. (`redacted_thinking` needs no buffer: it
                // arrives complete on `content_block_start` via `push_output_block`.)
                let mut pending_thinking: BTreeMap<u32, (String, Option<String>)> = BTreeMap::new();
                let mut saw_stop = false;
                let mut saw_provider_event = false;
                let mut saw_task_action = false;
                let mut last_reasoning_flush: Option<std::time::Instant> = None;
                // (error, capacity_scope, retry_after, retryable). The scope is
                // carried instead of a bare `is_rate_limit` flag so the cool-down
                // this records lands on the right window: an account 429 climbs
                // the ladder, a provider overload takes a short flat pause and
                // leaves the account's throttle history untouched.
                let mut mid_stream_error: Option<(
                    RuntimeError,
                    Option<api::CapacityScope>,
                    Option<std::time::Duration>,
                    bool,
                )> = None;

                loop {
                    let event = match stream.next_event().await {
                        Ok(Some(event)) => event,
                        Ok(None) => break,
                        Err(error) => {
                            let capacity = error.provider_error_class().capacity_scope();
                            let retry_after = error.retry_after();
                            let retryable = error.is_retryable();
                            // 429 면 재시도한다 — 일부 출력이 스트리밍된 뒤 발생한
                            // mid-stream rate-limit 도 포함. `events` 는 매 `'retry`
                            // 반복에서 새로 선언되므로 재시도는 partial 출력을 버리고
                            // 깨끗이 재시작한다(중복 없음). rate-limit 은 무한히
                            // 흡수하고(예산 소모 없음), 그 외 retryable 만 transient
                            // 예산을 쓴다.
                            mid_stream_error = Some((
                                RuntimeError::from_api_error(&error),
                                capacity,
                                retry_after,
                                retryable,
                            ));
                            break;
                        }
                    };
                    // The workflow timeout may commit `stopped` while the
                    // provider stream is still blocked. Discard the first late
                    // frame after cancellation before it can become text or a
                    // tool call; the manifest layer independently rejects late
                    // live stamps as defense in depth.
                    if self.is_cancelled() {
                        return Err(RuntimeError::new("agent cancelled"));
                    }
                    if !saw_provider_event {
                        if let Some(path) = manifest_path.as_deref() {
                            record_agent_provider_event(path);
                        }
                        saw_provider_event = true;
                    }
                    match event {
                        ApiStreamEvent::MessageStart(start) => {
                            if start.message.stop_reason.is_some() {
                                request_usage.observe(&start.message.usage.token_usage());
                            } else {
                                request_usage.observe_start(&start.message.usage.token_usage());
                            }
                            // A new streamed message is a turn boundary: whatever
                            // text arrives next resumed after a tool call and is
                            // not a continuation of the last sentence.
                            tail.mark_segment_break();
                            // Capture the turn's reasoning signature (Gemini 3's
                            // `thoughtSignature`) just like the main conversation
                            // parser does. Without this, a sub-agent's parallel
                            // tool call echoes its functionCalls back unsigned and
                            // Gemini 400s ("missing a thought_signature ... position
                            // 2"). Opaque to every other provider.
                            if let Some(signature) = &start.message.thought_signature {
                                events.push(AssistantEvent::ProviderState(
                                    ProviderStateBlob::gemini_thought_signature(signature.clone()),
                                ));
                            }
                            // ChatGPT/Codex reasoning-replay payload, mirroring the
                            // main conversation parser — a sub-agent's own
                            // `function_call`s need their preceding reasoning
                            // items replayed just like the foreground turn does.
                            if let Some(replay) = &start.message.reasoning_replay {
                                events.push(AssistantEvent::ReasoningReplay(replay.clone()));
                            }
                            for block in start.message.content {
                                push_output_block(block, 0, &mut events, &mut pending_tools, true);
                            }
                        }
                        ApiStreamEvent::ContentBlockStart(start) => {
                            push_output_block(
                                start.content_block,
                                start.index,
                                &mut events,
                                &mut pending_tools,
                                true,
                            );
                        }
                        ApiStreamEvent::ContentBlockDelta(delta) => match delta.delta {
                            ContentBlockDelta::TextDelta { text } => {
                                if !text.is_empty() {
                                    if !saw_task_action {
                                        if let Some(path) = manifest_path.as_deref() {
                                            record_agent_task_activity(path);
                                        }
                                        saw_task_action = true;
                                    }
                                    tail.push(&text);
                                    events.push(AssistantEvent::TextDelta(text));
                                }
                            }
                            ContentBlockDelta::InputJsonDelta { partial_json } => {
                                if !partial_json.is_empty() && !saw_task_action {
                                    if let Some(path) = manifest_path.as_deref() {
                                        record_agent_task_activity(path);
                                    }
                                    saw_task_action = true;
                                }
                                if let Some((_, _, input)) = pending_tools.get_mut(&delta.index) {
                                    input.push_str(&partial_json);
                                }
                            }
                            ContentBlockDelta::ThinkingDelta { thinking } => {
                                if !thinking.is_empty()
                                    && last_reasoning_flush.is_none_or(|last| {
                                        last.elapsed() >= REASONING_ACTIVITY_FLUSH_INTERVAL
                                    })
                                {
                                    if let Some(path) = manifest_path.as_deref() {
                                        record_agent_reasoning_activity(path);
                                    }
                                    last_reasoning_flush = Some(std::time::Instant::now());
                                }
                                pending_thinking.entry(delta.index).or_default().0.push_str(&thinking);
                            }
                            ContentBlockDelta::SignatureDelta { signature } => {
                                pending_thinking.entry(delta.index).or_default().1 = Some(signature);
                            }
                        },
                        ApiStreamEvent::ContentBlockStop(stop) => {
                            // A block index is either a thinking block or a tool,
                            // never both.
                            if let Some((thinking, signature)) = pending_thinking.remove(&stop.index) {
                                events.push(AssistantEvent::Thinking { thinking, signature });
                            }
                            if let Some((id, name, input)) = pending_tools.remove(&stop.index) {
                                events.push(AssistantEvent::ToolUse { id, name, input });
                            }
                        }
                        ApiStreamEvent::MessageDelta(delta) => {
                            // A streaming Gemini turn carries its thoughtSignature
                            // on the closing delta (unknown at MessageStart). Mirror
                            // the main parser's capture so a sub-agent's tool calls
                            // echo back signed; opaque to every other provider.
                            if let Some(signature) = &delta.delta.thought_signature {
                                events.push(AssistantEvent::ProviderState(
                                    ProviderStateBlob::gemini_thought_signature(signature.clone()),
                                ));
                            }
                            if let Some(replay) = &delta.delta.reasoning_replay {
                                events.push(AssistantEvent::ReasoningReplay(replay.clone()));
                            }
                            let usage = delta.usage.token_usage();
                            request_usage.observe(&usage);
                            // Sample this turn's output tokens onto both the
                            // sparkline and the budget total. One `MessageDelta`
                            // per turn — fine-grained per-chunk capture would
                            // over-sample without improving either.
                            self.record_output_sample(usage.output_tokens);
                            events.push(AssistantEvent::Usage(usage));
                            // Surface the stop reason so a sub-agent turn cut off
                            // at the output-token limit is continued by the
                            // conversation loop rather than read as completion.
                            if let Some(reason) = delta
                                .delta
                                .stop_reason
                                .as_deref()
                                .filter(|reason| !reason.is_empty())
                            {
                                events.push(AssistantEvent::StopReason(reason.to_string()));
                            }
                            runtime::push_refusal_category(
                                &mut events,
                                delta
                                    .delta
                                    .stop_details
                                    .as_ref()
                                    .and_then(|details| details.category.as_deref()),
                            );
                        }
                        ApiStreamEvent::MessageStop(_) => {
                            saw_stop = true;
                            runtime::flush_pending_tool_events(&mut events, &mut pending_tools);
                            events.push(AssistantEvent::MessageStop);
                        }
                    }
                }

                if let Some((error, capacity, retry_after, retryable)) = mid_stream_error {
                    if let Some(scope) = capacity {
                        // A retry restarts the turn from scratch, so remove this
                        // attempt's live tail (buffered and already-flushed). The
                        // restart re-streams the same text and would otherwise
                        // duplicate partial prose in the viewer.
                        tail.discard_attempt_tail();
                        // Mid-stream capacity refusal: absorb indefinitely, exactly
                        // like the stream-open case.
                        self.governor().on_rate_limit();
                        mark_capacity_stall_from(
                            self.provider_kind(),
                            scope,
                            retry_after,
                            rl_backoff_step,
                        );
                        rl_backoff_step = rl_backoff_step.saturating_add(1);
                        rl_hits = rl_hits.saturating_add(1);
                        drop(permit);
                        drop(stream);
                        if self.try_rate_limit_model_fallback(
                            &mut message_request,
                            &mut rl_hits,
                            &mut rl_backoff_step,
                        ) {
                            auth_recovery_attempted = false;
                            continue 'retry;
                        }
                        self.try_starvation_demotion(
                            &mut message_request,
                            &mut rl_hits,
                            &mut rl_backoff_step,
                        );
                        continue 'retry;
                    }
                    if retryable {
                        // Same retry semantics as the rate-limit path: remove
                        // this failed attempt's live tail before the provider
                        // re-streams the turn.
                        tail.discard_attempt_tail();
                        drop(permit);
                        drop(stream);
                        let previous_model = self.model.clone();
                        if self.prepare_transient_retry(
                            &mut message_request,
                            &mut transient_failures,
                            &mut transient_fallback_attempted,
                            "provider_transient_stream",
                        ) {
                            if self.model != previous_model {
                                auth_recovery_attempted = false;
                            }
                            continue 'retry;
                        }
                    }
                    // No retry remains: preserve any partial streamed prose
                    // for the agent viewer / terminal-state salvage path.
                    tail.flush();
                    return Err(error);
                }

                // Clean end of stream: surface whatever text is still buffered.
                tail.flush();

                // A clean streamed turn: tell the governor it may grow the live
                // limit back toward the ceiling (AIMD additive increase).
                self.governor().on_success(!rate_limit_headroom_low(self.provider_kind()));

                runtime::flush_pending_tool_events(&mut events, &mut pending_tools);
                push_prompt_cache_record(&self.client, &mut events);

                if !saw_stop
                    && events.iter().any(|event| {
                        matches!(event, AssistantEvent::TextDelta(text) if !text.is_empty())
                            || matches!(event, AssistantEvent::ToolUse { .. })
                    })
                {
                    events.push(AssistantEvent::MessageStop);
                }

                if events
                    .iter()
                    .any(|event| matches!(event, AssistantEvent::MessageStop))
                {
                    if let Some(session_id) = self.prompt_cache_session_id() {
                        runtime::record_non_anthropic_prompt_cache_usage(
                            session_id,
                            self.provider_kind(),
                            &message_request,
                            &mut events,
                        );
                    }
                    return Ok(events);
                }

                if should_skip_non_stream_fallback(&self.model) {
                    if let Some(session_id) = self.prompt_cache_session_id() {
                        runtime::record_non_anthropic_prompt_cache_usage(
                            session_id,
                            self.provider_kind(),
                            &message_request,
                            &mut events,
                        );
                    }
                    return Ok(events);
                }

                drop(request_usage);
                let mut request_usage = super::usage::RequestUsageGuard::open(
                    manifest_path.as_deref(),
                    self.run_generation,
                    &message_request.model,
                );
                let response = match self
                    .client
                    .send_message(&MessageRequest {
                        stream: false,
                        ..message_request.clone()
                    })
                    .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        request_usage.reject(&error);
                        if let Some(scope) = error.provider_error_class().capacity_scope() {
                            self.governor().on_rate_limit();
                            mark_capacity_stall_from(
                                self.provider_kind(),
                                scope,
                                error.retry_after(),
                                rl_backoff_step,
                            );
                            rl_backoff_step = rl_backoff_step.saturating_add(1);
                            rl_hits = rl_hits.saturating_add(1);
                            drop(permit);
                            if self.try_rate_limit_model_fallback(
                                &mut message_request,
                                &mut rl_hits,
                                &mut rl_backoff_step,
                            ) {
                                auth_recovery_attempted = false;
                                continue 'retry;
                            }
                            self.try_starvation_demotion(
                                &mut message_request,
                                &mut rl_hits,
                                &mut rl_backoff_step,
                            );
                            continue 'retry;
                        }
                        if error.is_unauthorized() && !auth_recovery_attempted {
                            auth_recovery_attempted = true;
                            drop(permit);
                            if self.try_recover_unauthorized().await {
                                continue 'retry;
                            }
                            return Err(RuntimeError::from_api_error(&error));
                        }
                        if error.is_retryable() {
                            drop(permit);
                            let previous_model = self.model.clone();
                            if self.prepare_transient_retry(
                                &mut message_request,
                                &mut transient_failures,
                                &mut transient_fallback_attempted,
                                "provider_transient_non_stream",
                            ) {
                                if self.model != previous_model {
                                    auth_recovery_attempted = false;
                                }
                                continue 'retry;
                            }
                        }
                        return Err(RuntimeError::from_api_error(&error));
                    }
                };
                // The non-stream fallback succeeded too — let the governor grow.
                self.governor().on_success(!rate_limit_headroom_low(self.provider_kind()));
                // The non-stream fallback observes usage too; record it on the
                // same surfaces as the streamed path so the budget total does not
                // silently undercount turns that fell through to it.
                request_usage.observe(&response.usage.token_usage());
                let fallback_output_tokens = response.usage.token_usage().output_tokens;
                let mut events = response_to_events(response);
                self.record_output_sample(fallback_output_tokens);
                push_prompt_cache_record(&self.client, &mut events);
                if let Some(session_id) = self.prompt_cache_session_id() {
                    runtime::record_non_anthropic_prompt_cache_usage(
                        session_id,
                        self.provider_kind(),
                        &message_request,
                        &mut events,
                    );
                }
                return Ok(events);
            }
        })
    }
}

/// Terminal give-up predicate. `true` once the agent sits on a tier with no
/// lower fallback ([`starvation_demotion`] → `None`: the bottom Anthropic tier
/// `haiku`, or any non-ladder model) and has absorbed at least the bottom-tier
/// 429 budget there. Higher tiers never give up — a 429 there is recoverable by
/// lower-tier fallback — so the ladder is always walked first. Pure so the
/// policy is
/// unit-testable without driving the streaming retry loop.
fn next_rate_limit_fallback_candidate(
    candidates: &mut VecDeque<String>,
    current_model: &str,
) -> Option<String> {
    while let Some(candidate) = candidates.pop_front() {
        let candidate = candidate.trim();
        if candidate.is_empty() || resolve_model_alias(candidate) == current_model {
            continue;
        }
        return Some(candidate.to_string());
    }
    None
}

fn next_cross_provider_fallback_candidate(
    candidates: &mut VecDeque<String>,
    current_model: &str,
    inventory: &runtime::ModelInventory,
) -> Option<String> {
    let current_model = resolve_model_alias(current_model);
    while let Some(candidate) = candidates.pop_front() {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        let resolved = resolve_model_alias(candidate);
        if resolved == current_model
            || same_inventory_provider(inventory, &current_model, &resolved)
        {
            continue;
        }
        return Some(candidate.to_string());
    }
    None
}

fn same_inventory_provider(
    inventory: &runtime::ModelInventory,
    left: &str,
    right: &str,
) -> bool {
    match (inventory.find(left), inventory.find(right)) {
        (Some(left), Some(right)) => left.provider().eq_ignore_ascii_case(right.provider()),
        _ => detect_provider_kind(left) == detect_provider_kind(right),
    }
}

fn should_try_pre_slot_fallback(provider_headroom_low: bool) -> bool {
    provider_headroom_low
}

fn starved_past_bottom_tier(model: &str, rl_hits: u32) -> bool {
    rl_hits >= STARVATION_GIVE_UP_BOTTOM_429S && starvation_demotion(model).is_none()
}

/// OAuth quota context for parked-agent surfaces: `` · 5h 92% · resets 38m``
/// from the freshest unified snapshot. Empty for api-key sessions (no unified
/// headers) and for stale snapshots — better no figure than a reset one.
fn oauth_window_suffix() -> String {
    api::quota::latest_rate_limit_snapshot().map_or_else(String::new, |(snapshot, age)| {
        window_suffix_for(snapshot, age, epoch_seconds_now_u64())
    })
}

/// Pure core of [`oauth_window_suffix`].
fn window_suffix_for(
    snapshot: api::RateLimitSnapshot,
    age: std::time::Duration,
    now_unix: u64,
) -> String {
    use std::fmt::Write as _;
    if age > QUOTA_SNAPSHOT_FRESH {
        return String::new();
    }
    let Some((kind, window)) = binding_window(snapshot) else {
        return String::new();
    };
    let mut suffix = format!(" \u{00b7} {kind} {}%", window.used_percent());
    if let Some(resets_at) = window.resets_at_unix {
        let minutes = resets_at.saturating_sub(now_unix) / 60;
        if minutes > 0 {
            let _ = write!(suffix, " \u{00b7} resets {minutes}m");
        }
    }
    suffix
}

/// W9-2: phase label for a rate-limit-parked agent. The bare
/// `resumes in ~Ns` reads as frozen once the backoff caps (the string stops
/// changing at ~120s — the 2026-06-10 misdiagnosis), so after the first
/// absorbed 429 the label also carries the cumulative retry count and parked
/// wall-clock, which keep visibly growing while the agent is alive.
fn rate_limit_phase_label(cooldown_ms: u64, rl_hits: u32, rl_waited_ms: u64) -> String {
    use std::fmt::Write as _;
    let mut label = format!(
        "rate-limited \u{00b7} resumes in ~{}s",
        cooldown_ms.div_ceil(1_000)
    );
    if rl_hits > 0 {
        let _ = write!(
            label,
            " \u{00b7} retry {rl_hits} \u{00b7} waited {}",
            format_wait_brief(rl_waited_ms)
        );
    }
    label
}

/// Brief human duration for wait labels: whole seconds under a minute, whole
/// minutes above (`45s`, `14m`).
fn format_wait_brief(ms: u64) -> String {
    let secs = ms / 1_000;
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m", secs / 60)
    }
}

/// The catalog capability of a model that serves streaming requests only
/// (Codex Spark), so the non-stream fallback is never re-armed for it.
const STREAMING_ONLY_CAPABILITY: &str = "streaming_only";

fn should_skip_non_stream_fallback(model: &str) -> bool {
    api::model_has_capability(model, STREAMING_ONLY_CAPABILITY)
}

fn agent_tool_definitions(allowed: &BTreeSet<String>, schema: Option<&serde_json::Value>) -> Vec<ToolDefinition> {
    tool_specs_for_allowed_tools(Some(allowed)).into_iter().map(|spec| ToolDefinition {
        name: spec.name.to_string(),
        description: Some(spec.description.to_string()),
        input_schema: if spec.name == "StructuredOutput" { schema.cloned().unwrap_or(spec.input_schema) } else { spec.input_schema },
    }).collect()
}

fn tool_specs_for_allowed_tools(allowed_tools: Option<&BTreeSet<String>>) -> Vec<ToolSpec> {
    mvp_tool_specs()
        .iter()
        .filter(|spec| allowed_tools.is_none_or(|allowed| allowed.contains(spec.name)))
        .cloned()
        .collect()
}

/// Lower a sub-agent's system prompt sections into wire blocks via the shared
/// [`runtime::split_system_with_identity`] — the exact path the foreground
/// turn uses. Returns `None` for an empty prompt (no `system` field at all).
fn agent_system_blocks(system_prompt: &[String]) -> Option<Vec<api::SystemBlock>> {
    (!system_prompt.is_empty())
        .then(|| runtime::split_system_with_identity(&system_prompt.join("\n\n")))
}

// The headless `push_output_block` / `response_to_events` transforms live in
// `runtime` (`conversation::api`) as the single source of truth; the streaming
// loop above and the non-stream fallback below delegate to them. `push_output_block`
// stays `pub(crate)` re-exported because `agent_tools` re-exports it for the
// registry streaming-index regression test.
pub(crate) use runtime::push_output_block;
use runtime::response_to_events;

/// Pin one agent's prompt-cache identity onto a freshly built client: the
/// ChatGPT cache-key scope, and the Anthropic recorder.
///
/// The two are different mechanisms for the same fact. `with_cache_scope`
/// pins the OpenAI-compatible `prompt_cache_key`; Anthropic caching is
/// prefix-based and has no key, so that call is a no-op there — which is why
/// an Anthropic sub-agent used to write no `requests.jsonl` at all and its
/// attempt could not be priced from disk. The cache attached here RECORDS
/// only ([`api::PromptCacheConfig::recording_only`]): it never answers a
/// request from a stored completion, so the bytes on the wire are exactly
/// what they were.
///
/// Called from both places a client is bound to an agent — first binding and
/// the mid-run model swap — so a swap cannot silently drop the recorder.
fn pin_agent_cache(client: ProviderClient, agent_id: &str) -> ProviderClient {
    client
        .with_cache_scope(agent_id)
        .with_prompt_cache(api::PromptCache::with_config(
            api::PromptCacheConfig::recording_only(agent_id),
        ))
}

fn push_prompt_cache_record(client: &ProviderClient, events: &mut Vec<AssistantEvent>) {
    if let Some(record) = client.take_last_prompt_cache_record() {
        if let Some(event) = runtime::prompt_cache_record_to_event(record) {
            events.push(AssistantEvent::PromptCache(event));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_TRANSIENT_FAILURES, QUOTA_SNAPSHOT_FRESH, ROUTE_EFFORT_MIN_BUDGET_TOKENS,
        STARVATION_GIVE_UP_BOTTOM_429S, TAIL_FLUSH_INTERVAL, TAIL_FLUSH_PENDING_CHARS,
        TOKEN_HISTORY_CAP, TailFlusher, TransientRecoveryStage, agent_model_route,
        agent_system_blocks, combined_effort_floor, format_wait_brief, merge_route_effort,
        next_cross_provider_fallback_candidate, next_rate_limit_fallback_candidate,
        rate_limit_phase_label, record_output_sample_into, record_transient_failure,
        response_to_events, should_skip_non_stream_fallback, should_try_pre_slot_fallback,
        starved_past_bottom_tier, window_suffix_for,
    };
    use super::super::manifest::OUTPUT_TAIL_CAP;
    use runtime::RuntimeError;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn temp_tail_manifest(tag: &str) -> (PathBuf, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zo-agent-tail-flusher-{}-{tag}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let manifest_path = dir.join("agent-tail.json");
        let manifest = json!({
            "agentId": "agent-tail",
            "name": "agent-tail",
            "description": "tail flusher test agent",
            "subagentType": null,
            "model": "claude-opus-4-8",
            "status": "running",
            "outputFile": dir.join("agent-tail.md"),
            "manifestFile": manifest_path,
            "createdAt": "100"
        });
        std::fs::write(&manifest_path, serde_json::to_string(&manifest).unwrap())
            .expect("write manifest");
        (dir, manifest_path)
    }

    fn manifest_output_tail(path: &std::path::Path) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(path).unwrap())
            .unwrap()
            .get("outputTail")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }

    #[test]
    fn structured_output_advertises_the_delegated_schema() {
        let allowed = std::collections::BTreeSet::from(["StructuredOutput".to_string()]);
        let schema = serde_json::json!({"type":"object","required":["subtasks"],"properties":{"subtasks":{"type":"array"}}});
        let tools = super::agent_tool_definitions(&allowed, Some(&schema));
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].input_schema, schema);
    }

    #[test]
    fn tail_flusher_uses_low_latency_thresholds_for_live_agent_output() {
        let interval = std::hint::black_box(TAIL_FLUSH_INTERVAL);
        let pending_chars = std::hint::black_box(TAIL_FLUSH_PENDING_CHARS);
        assert!(
            (Duration::from_millis(50)..=Duration::from_millis(200)).contains(&interval),
            "sub-agent tail flush interval should stay low-latency without becoming per-token I/O"
        );
        assert!(
            (64..=160).contains(&pending_chars),
            "small prose bursts should reach the manifest before a large chunk accumulates"
        );

        let (dir, manifest_path) = temp_tail_manifest("threshold");
        let mut tail = TailFlusher::new(Some(manifest_path.clone()));
        let almost = "x".repeat(TAIL_FLUSH_PENDING_CHARS.saturating_sub(1));
        tail.push(&almost);
        assert_eq!(
            manifest_output_tail(&manifest_path),
            None,
            "sub-threshold text stays buffered until time or size makes it visible"
        );
        tail.push("y");
        assert_eq!(
            manifest_output_tail(&manifest_path).as_deref(),
            Some(format!("{almost}y").as_str()),
            "crossing the low pending-char threshold flushes immediately"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_flusher_breaks_turns_instead_of_fusing_sentences() {
        let (dir, manifest_path) = temp_tail_manifest("segment");
        let mut tail = TailFlusher::new(Some(manifest_path.clone()));

        // One turn arriving as two deltas: these are fragments of a single
        // sentence, so nothing may be inserted between them.
        tail.mark_segment_break();
        tail.push("...involved in both ");
        tail.push("blockers.");
        // The next turn resumed after a tool call.
        tail.mark_segment_break();
        tail.push("The nightly snapshot already has semicolons.");
        tail.flush();

        let text = manifest_output_tail(&manifest_path).expect("flushed tail");
        assert!(
            text.starts_with("...involved in both blockers."),
            "the opening turn takes no leading break and mid-turn deltas must \
             concatenate untouched: {text}"
        );
        assert!(
            !text.contains("blockers.The"),
            "a turn boundary must not fuse two sentences: {text}"
        );
        assert!(
            text.contains("blockers.\nThe nightly"),
            "a turn boundary must separate the two segments: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_flusher_discard_drops_retry_text_without_duplication() {
        let (dir, manifest_path) = temp_tail_manifest("discard");
        let mut tail = TailFlusher::new(Some(manifest_path.clone()));

        tail.push("partial retry text");
        tail.discard_attempt_tail();
        tail.flush();
        assert_eq!(
            manifest_output_tail(&manifest_path),
            None,
            "discarded unflushed retry text must not leak into outputTail on a later flush"
        );

        let visible = "v".repeat(TAIL_FLUSH_PENDING_CHARS);
        tail.push(&visible);
        assert_eq!(
            manifest_output_tail(&manifest_path).as_deref(),
            Some(visible.as_str()),
            "threshold-sized retry text should have been flushed to the viewer"
        );

        tail.discard_attempt_tail();
        assert_eq!(
            manifest_output_tail(&manifest_path),
            None,
            "already-flushed retry text must be rolled back before the turn restarts"
        );

        tail.push(&visible);
        assert_eq!(
            manifest_output_tail(&manifest_path).as_deref(),
            Some(visible.as_str()),
            "re-streamed retry text should appear once, not as duplicated flushed suffixes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_flusher_discard_rolls_back_visible_window_for_long_retry_attempts() {
        let (dir, manifest_path) = temp_tail_manifest("discard-long");
        let mut tail = TailFlusher::new(Some(manifest_path.clone()));
        let long_attempt = "가".repeat(OUTPUT_TAIL_CAP + TAIL_FLUSH_PENDING_CHARS);

        tail.push(&long_attempt);
        let flushed = manifest_output_tail(&manifest_path).expect("long attempt flushed");
        assert_eq!(flushed.chars().count(), OUTPUT_TAIL_CAP);
        assert!(flushed.chars().all(|ch| ch == '가'));

        tail.discard_attempt_tail();
        assert_eq!(
            manifest_output_tail(&manifest_path),
            None,
            "discard should roll back the same visible tail window that append keeps capped"
        );

        tail.push(&long_attempt);
        let retried = manifest_output_tail(&manifest_path).expect("retry flushed");
        assert_eq!(retried.chars().count(), OUTPUT_TAIL_CAP);
        assert!(retried.chars().all(|ch| ch == '가'));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CC 429-parity regression: the sub-agent request must lower its system
    /// prompt through the same identity-splitting path as the foreground turn.
    /// The Claude Max OAuth fingerprint requires the FIRST system block to be
    /// exactly the Claude Code identity line with no `cache_control`; the old
    /// `system_from_string` single-block shape was rejected as a fingerprint
    /// mismatch (surfaced as zo-only agent 429s).
    #[test]
    fn agent_system_prompt_isolates_identity_and_caches_the_rest() {
        let sections = vec![
            format!(
                "{}\nYou are an interactive agent that helps users.",
                runtime::CLAUDE_CODE_IDENTITY
            ),
            "# Doing tasks\nwork carefully".to_string(),
        ];
        let blocks = agent_system_blocks(&sections).expect("non-empty prompt yields blocks");
        assert!(blocks.len() >= 2, "identity must split from the body");
        let api::SystemBlock::Text {
            text,
            cache_control,
        } = &blocks[0];
        assert_eq!(
            text,
            runtime::CLAUDE_CODE_IDENTITY,
            "first block must be the verbatim identity line"
        );
        assert!(
            cache_control.is_none(),
            "identity block must carry no cache_control (OAuth fingerprint)"
        );
        // Every remaining block is cached so agent iterations reuse the prefix.
        for block in &blocks[1..] {
            let api::SystemBlock::Text { cache_control, .. } = block;
            assert!(
                cache_control.is_some(),
                "non-identity system blocks must carry a cache breakpoint"
            );
        }
        // Empty prompt sends no system field at all.
        assert!(agent_system_blocks(&[]).is_none());
    }

    #[test]
    fn agent_model_route_separates_catalog_selection_from_wire_id() {
        let (selection, wire) = agent_model_route("google/gemini-3.6-flash");
        assert_eq!(selection, "google/gemini-3.6-flash");
        assert_eq!(wire, "gemini-3.6-flash");

        let (selection, wire) = agent_model_route("sonnet");
        assert_eq!(selection, wire);
        assert!(!wire.contains('/'));
    }

    #[test]
    fn codex_spark_skips_non_stream_fallback() {
        assert!(should_skip_non_stream_fallback("gpt-5.3-codex-spark"));
        assert!(should_skip_non_stream_fallback(" gpt-5.3-CODEX-SPARK "));
        assert!(!should_skip_non_stream_fallback("gpt-5.6-luna"));
    }

    #[test]
    fn agent_provider_client_uses_rate_limit_class_without_changing_retry_budget() {
        let api_error = api::ApiError::Api {
            status: reqwest::StatusCode::TOO_MANY_REQUESTS,
            error_type: Some("rate_limit_error".to_string()),
            message: Some("slow down".to_string()),
            body: String::new(),
            retryable: true,
            retry_after: Some(std::time::Duration::from_secs(9)),
        };

        let runtime_error = RuntimeError::from_api_error(&api_error);
        assert_eq!(
            runtime_error.provider_error_class(),
            Some(api::ProviderErrorClass::account_rate_limit(Some(std::time::Duration::from_secs(9))))
        );
        assert_eq!(STARVATION_GIVE_UP_BOTTOM_429S, 5);
    }

    #[test]
    fn terminal_starvation_runtime_error_keeps_rate_limit_class() {
        let err = RuntimeError::with_provider_error_class(
            "rate-limit starvation: gave up after retries",
            api::ProviderErrorClass::account_rate_limit(None),
        );
        assert_eq!(
            err.provider_error_class(),
            Some(api::ProviderErrorClass::account_rate_limit(None))
        );
    }

    #[test]
    fn non_stream_fallback_emits_provider_state_for_thought_signature() {
        use api::{MessageResponse, OutputContentBlock, Usage};

        let events = response_to_events(MessageResponse {
            id: "msg-signed".to_string(),
            kind: "message".to_string(),
            model: "gemini-3.5-flash".to_string(),
            role: "assistant".to_string(),
            content: vec![OutputContentBlock::Text {
                text: "ok".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            stop_sequence: None,
            stop_details: None,
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens_details: None,
            },
            request_id: None,
            thought_signature: Some("SIG_AGENT_NON_STREAM".to_string()),
            reasoning_replay: None,
            context_management: None,
        });

        assert!(events.iter().any(|event| matches!(
            event,
            runtime::AssistantEvent::ProviderState(state)
                if state.as_gemini_thought_signature() == Some("SIG_AGENT_NON_STREAM")
        )));
    }

    /// Terminal give-up only on the bottom tier: opus/sonnet, a legacy non-fast
    /// gpt-5.5 model, or a gemini pro walk the fallback ladder no matter how
    /// many 429s (recoverable), but a true bottom tier — haiku, a `-fast`/`flash` variant,
    /// or a model with no catalog ladder — gives up once it has absorbed the
    /// bottom-tier 429 budget, so a starved fan-out always completes.
    #[test]
    fn rate_limit_fallback_candidate_skips_current_blank_and_aliases() {
        let mut candidates: std::collections::VecDeque<String> = [
            String::new(),
            "  claude-sonnet-4-6  ".to_string(),
            "gpt-5.5-fast".to_string(),
        ]
        .into();

        assert_eq!(
            next_rate_limit_fallback_candidate(&mut candidates, "claude-sonnet-4-6"),
            Some("gpt-5.5-fast".to_string())
        );
        assert!(next_rate_limit_fallback_candidate(&mut candidates, "gpt-5.5-fast").is_none());
    }

    #[test]
    fn transient_recovery_retries_once_then_requests_one_bounded_fallback() {
        let mut failures = 0;
        assert_eq!(
            record_transient_failure(&mut failures, false),
            TransientRecoveryStage::RetrySameProvider
        );
        assert_eq!(
            record_transient_failure(&mut failures, false),
            TransientRecoveryStage::TryFallbackProvider
        );
        assert_eq!(
            record_transient_failure(&mut failures, true),
            TransientRecoveryStage::Exhausted
        );
        assert_eq!(failures, MAX_TRANSIENT_FAILURES);
    }

    #[test]
    fn transient_fallback_discards_stalled_provider_but_keeps_compat_provider_identity() {
        let inventory = runtime::ModelInventory::new(
            "gpt-5.6-sol[fast]",
            vec![
                runtime::ModelDescriptor::new("gpt-5.6-sol[fast]", "openai", "gpt"),
                runtime::ModelDescriptor::new("gpt-5.6-sol", "openai", "gpt"),
                runtime::ModelDescriptor::new("gpt-5.6-terra", "openai", "gpt"),
                runtime::ModelDescriptor::new("deepseek-v4-pro", "deepseek", "deepseek"),
                runtime::ModelDescriptor::new("claude-fable-5", "anthropic", "claude"),
            ],
        );
        let mut candidates: std::collections::VecDeque<String> = [
            "gpt-5.6-sol".to_string(),
            "gpt-5.6-terra".to_string(),
            "deepseek-v4-pro".to_string(),
            "claude-fable-5".to_string(),
        ]
        .into();

        assert_eq!(
            next_cross_provider_fallback_candidate(
                &mut candidates,
                "gpt-5.6-sol[fast]",
                &inventory,
            ),
            Some("deepseek-v4-pro".to_string()),
            "same-provider GPT variants must be discarded while a distinct OpenAI-compatible provider remains eligible"
        );
        assert_eq!(candidates.front().map(String::as_str), Some("claude-fable-5"));
    }

    #[test]
    fn pre_slot_fallback_only_triggers_under_provider_pressure() {
        assert!(
            should_try_pre_slot_fallback(true),
            "recent 429/cooldown/hot quota should escape before waiting for an API slot"
        );
        assert!(
            !should_try_pre_slot_fallback(false),
            "healthy-headroom waits, including user concurrency caps, should not switch models"
        );
    }

    #[test]
    fn give_up_only_after_bottom_tier_exhausts_its_429_budget() {
        let n = STARVATION_GIVE_UP_BOTTOM_429S;
        // Higher tiers never give up — they take a lower-tier fallback instead.
        assert!(!starved_past_bottom_tier("claude-opus-4-8", n + 100));
        assert!(!starved_past_bottom_tier("claude-sonnet-4-6", n + 100));
        assert!(!starved_past_bottom_tier("gpt-5.5", n + 100));
        assert!(!starved_past_bottom_tier("gemini-3.1-pro-preview", n + 100));
        // Bottom tier (no fallback) gives up at/after the budget, not before.
        assert!(!starved_past_bottom_tier(
            "claude-haiku-4-5-20251001",
            n - 1
        ));
        assert!(starved_past_bottom_tier("claude-haiku-4-5-20251001", n));
        // Each family's bottom tier follows the same rule. gpt-5.5-fast는 더는
        // 바닥이 아니다(카탈로그 퇴역·terra[fast]로 이관) — GPT 바닥은 luna.
        assert!(!starved_past_bottom_tier("gpt-5.5-fast", n + 100));
        assert!(starved_past_bottom_tier("gpt-5.6-luna", n));
        assert!(starved_past_bottom_tier("gemini-3.5-flash", n));
        // A model with no catalog ladder at all (xAI) is its own bottom tier.
        assert!(starved_past_bottom_tier("grok-3", n));
        assert!(!starved_past_bottom_tier("grok-3", 0));
    }

    #[test]
    fn budget_total_is_lossless_while_history_buffer_stays_capped() {
        let total = AtomicU64::new(0);
        let history = Mutex::new(Vec::new());
        // Push more samples than the display cap can hold; each is 10 tokens.
        let samples = TOKEN_HISTORY_CAP + 25;
        for _ in 0..samples {
            record_output_sample_into(&total, &history, 10);
        }
        // The budget total counts every sample (never drops the oldest)…
        assert_eq!(total.load(Ordering::Relaxed), (samples as u64) * 10);
        // …while the display buffer is bounded to the cap.
        assert_eq!(history.lock().unwrap().len(), TOKEN_HISTORY_CAP);
    }

    /// OAuth 윈도우 suffix: 신선한 스냅샷이면 `· 5h 92% · resets Nm`,
    /// stale·빈 스냅샷이면 빈 문자열(리셋됐을 수 있는 수치 미표기).
    #[test]
    fn oauth_window_suffix_formats_binding_window_and_reset() {
        use api::{RateLimitSnapshot, RateLimitWindow, RateLimitWindowKind};
        use std::time::Duration;
        let snapshot = RateLimitSnapshot {
            five_hour: Some(RateLimitWindow {
                utilization: 0.92,
                resets_at_unix: Some(10_000 + 38 * 60),
            }),
            seven_day: None,
            representative: Some(RateLimitWindowKind::FiveHour),
        };
        assert_eq!(
            window_suffix_for(snapshot, Duration::from_secs(30), 10_000),
            " \u{00b7} 5h 92% \u{00b7} resets 38m"
        );
        // reset 시각이 없으면 퍼센트만.
        let bare = RateLimitSnapshot {
            five_hour: Some(RateLimitWindow {
                utilization: 0.92,
                resets_at_unix: None,
            }),
            ..snapshot
        };
        assert_eq!(
            window_suffix_for(bare, Duration::from_secs(30), 10_000),
            " \u{00b7} 5h 92%"
        );
        // stale 스냅샷·빈 스냅샷은 무표기.
        assert_eq!(
            window_suffix_for(
                snapshot,
                QUOTA_SNAPSHOT_FRESH + Duration::from_secs(1),
                10_000
            ),
            ""
        );
        assert_eq!(
            window_suffix_for(
                RateLimitSnapshot::default(),
                Duration::from_secs(30),
                10_000
            ),
            ""
        );
    }

    /// W9-2: 첫 429 전에는 종전 라벨 그대로, 이후엔 누적 retry/waited가 붙어
    /// 백오프 캡(120s)에서도 라벨이 계속 변한다(멈춤 오인 방지).
    #[test]
    fn rate_limit_label_grows_with_retries_and_wait() {
        assert_eq!(
            rate_limit_phase_label(120_000, 0, 0),
            "rate-limited \u{00b7} resumes in ~120s"
        );
        assert_eq!(
            rate_limit_phase_label(120_000, 8, 14 * 60_000),
            "rate-limited \u{00b7} resumes in ~120s \u{00b7} retry 8 \u{00b7} waited 14m"
        );
        assert_eq!(format_wait_brief(45_000), "45s");
        assert_eq!(format_wait_brief(60_000), "1m");
        assert_eq!(format_wait_brief(14 * 60_000 + 30_000), "14m");
    }

    #[test]
    fn zero_sample_is_a_no_op_on_both_surfaces() {
        let total = AtomicU64::new(0);
        let history = Mutex::new(Vec::new());
        record_output_sample_into(&total, &history, 0);
        assert_eq!(total.load(Ordering::Relaxed), 0);
        assert!(history.lock().unwrap().is_empty());
    }

    // Phase 2a: route_effort byte-identical-when-None + rank-based merge.

    #[test]
    fn combined_effort_floor_is_byte_identical_without_a_route_effort() {
        assert_eq!(combined_effort_floor(None, None), None);
        assert_eq!(combined_effort_floor(Some(12_000), None), Some(12_000));
        // A zero deep-gate floor is inert, exactly like `effort_budget_with_floor`.
        assert_eq!(combined_effort_floor(Some(0), None), None);
    }

    #[test]
    fn combined_effort_floor_raises_to_the_route_effort_preset_when_higher() {
        assert_eq!(
            combined_effort_floor(None, Some(api::EffortLevel::Ultra)),
            Some(ROUTE_EFFORT_MIN_BUDGET_TOKENS)
        );
        // The deep-gate floor still wins when it is already higher than the
        // route-effort preset.
        assert_eq!(
            combined_effort_floor(Some(ROUTE_EFFORT_MIN_BUDGET_TOKENS + 5_000), Some(api::EffortLevel::Ultra)),
            Some(ROUTE_EFFORT_MIN_BUDGET_TOKENS + 5_000)
        );
    }

    #[test]
    fn merge_route_effort_is_byte_identical_without_a_route_effort() {
        assert_eq!(merge_route_effort(None, None), None);
        assert_eq!(
            merge_route_effort(None, Some(api::EffortLevel::High)),
            Some(api::EffortLevel::High)
        );
    }

    #[test]
    fn merge_route_effort_never_lets_a_lower_ranked_budget_level_displace_the_named_tier() {
        // `api::effort_level_for_budget` can never itself return `Ultra`, so the
        // budget-derived side here is `Max` at most — still ranked below `Ultra`.
        assert_eq!(
            merge_route_effort(Some(api::EffortLevel::Ultra), Some(api::EffortLevel::Max)),
            Some(api::EffortLevel::Ultra),
            "a budget floor must never lower an explicit route-recommended Ultra"
        );
        assert_eq!(
            merge_route_effort(Some(api::EffortLevel::Ultra), None),
            Some(api::EffortLevel::Ultra)
        );
    }

    #[test]
    fn merge_route_effort_lets_a_higher_ranked_budget_escalate_a_lower_named_tier() {
        assert_eq!(
            merge_route_effort(Some(api::EffortLevel::Low), Some(api::EffortLevel::Xhigh)),
            Some(api::EffortLevel::Xhigh),
            "a deep-gate floor must still raise a lower named tier, mirroring runtime_bridge"
        );
    }
}
