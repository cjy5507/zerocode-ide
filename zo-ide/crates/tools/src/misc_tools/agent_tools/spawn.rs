use std::collections::BTreeSet;
use std::time::Duration;

use core_types::helper_run::HelperRun;
use runtime::subagent_panes::{McpRoute, McpTool, ModelSelection, PermissionRules, ResolvedHarness};
use runtime::{PermissionMode, RouteOutcomeRecord, RuntimeHookConfig, lsp_client::LspRegistry};

use super::agent_runtime::{build_agent_runtime, subagent_hook_context};
use super::completion::{AgentCompletion, notify_agent_completion, provider_error_class_metadata};
use super::manifest::{
    manifest_generation_is_current, persist_agent_stopped_state, persist_agent_terminal_state,
    persist_agent_terminal_state_with_history,
};
use super::unregister_agent_cancel_signal;
use super::{AgentOutput, final_assistant_text, final_structured_output};
use crate::ToolError;
use crate::context::ProbeRevertGuard;

#[derive(Debug, Clone)]
pub(crate) struct AgentJob {
    pub manifest: AgentOutput,
    /// The session-owned registry the manifest was written into (t-2511),
    /// carried across the spawn thread so the child's own tool context — and
    /// therefore its nested spawns, stamps and lookups — inherits the parent
    /// session's store instead of re-deriving one from the process cwd.
    /// `None` on bare test harnesses.
    pub registry: Option<std::sync::Arc<super::AgentRegistry>>,
    pub prompt: String,
    pub system_prompt: Vec<String>,
    pub allowed_tools: BTreeSet<String>,
    /// Per-agent permission rules from the custom-agent definition. `None` for
    /// built-in types, which keeps [`agent_permission_policy`] rule-free.
    /// Carried (like `allowed_tools`) across the spawn's OS-thread boundary.
    pub permission_rules: Option<runtime::RuntimePermissionRuleConfig>,
    /// Per-agent permission mode. `None` defaults to `DangerFullAccess` in
    /// [`build_agent_runtime`] - byte-identical to the pre-override behavior.
    pub permission_mode: Option<PermissionMode>,
    /// Working directory for this agent's tools (worktree isolation). `None`
    /// runs in the process cwd. Carried from [`AgentInput::cwd`] into
    /// [`build_agent_runtime`], which loads it onto the executor's context.
    pub cwd: Option<std::path::PathBuf>,
    /// Parent session's LSP registry (a cheap `Arc`-clone), shared into the
    /// sub-agent's context by [`build_agent_runtime`] only when `cwd` is `None`
    /// (the agent runs in the parent tree, so the parent's servers - rooted
    /// there - give correct diagnostics). A worktree-isolated agent
    /// (`cwd` `Some`) skips it, since its tree has no matching server. `None` on
    /// the workflow-engine and test paths. `LspRegistry` is all `Arc`-backed, so
    /// it crosses the spawn OS-thread boundary safely.
    pub lsp: Option<LspRegistry>,
    /// Structured-output schema (workflow 8c). When set, `build_agent_runtime`
    /// enables `StructuredOutput` and `run_agent_job` captures that tool call's
    /// input as the structured result. Carried from [`AgentInput::schema`].
    pub schema: Option<serde_json::Value>,
    /// Carried from [`AgentInput::workflow_member`]: selects the workflow API
    /// semaphore (higher cap) over the shared one in [`run_agent_job`].
    pub workflow_member: bool,
    /// A fan-out lane (`AgentInput::one_shot`): the pane watcher releases the
    /// pane once the lane's answer is read, instead of leaving it idle.
    pub one_shot: bool,
    /// The plan shape the HOST laid this spawn out in, carried from
    /// [`AgentInput::plan_shape`]. `None` means nobody laid it out, which the
    /// outcome recorder reads as one delegation.
    pub plan_shape: Option<runtime::PlanShape>,
    /// This spawn IS the routing tax, not work (`AgentInput::route_tax`).
    pub route_tax: Option<runtime::RouteTaxCall>,
    /// The attempt this spawn SERVES — the turn that launched it, read from
    /// the session's live attempt at spawn time (inside that turn), so a
    /// background agent that outlives the turn still names the turn that paid
    /// for it rather than whichever turn happened to be running when it
    /// ended. A verifier overrides it with the attempt it judges
    /// (`judged_agent`), which is the work this spawn actually serves.
    /// `None` on a bare harness and wherever no turn declared an attempt.
    pub parent_attempt: Option<String>,
    /// Optional per-agent wall-clock budget. `None` leaves the spawned turn
    /// unbounded; callers that need a hard kill must pass an explicit budget.
    pub time_budget: Option<Duration>,
    /// Optional thinking budget chosen by the sub-agent model router. `None`
    /// keeps the provider default; GPT hard/deep routes set this so gpt-5.5
    /// receives the effort tier that matches the delegated task.
    pub thinking_budget_tokens: Option<u32>,
    /// Named reasoning-effort tier the Smart router recommends for this
    /// agent's route (carried from [`AgentInput::route_effort`]). `None` (the
    /// default for every pre-existing spawn path) keeps
    /// [`ProviderRuntimeClient`](super::provider_client::ProviderRuntimeClient)'s
    /// behavior byte-identical.
    pub route_effort: Option<api::EffortLevel>,
    /// Per-call provider-request concurrency ceiling, carried from the flat
    /// `SpawnMultiAgent` `concurrency` argument. Caps the adaptive governor's
    /// admission to `min(live_limit, this)` so a tighter user value actually
    /// throttles real API concurrency. `None` = governor ceiling only.
    pub api_concurrency: Option<usize>,
    /// Ranked host-computed fallback models to try when the selected model's
    /// provider is rate-limited or parked in a long cool-down.
    pub route_fallback_models: Vec<String>,
    /// Parent-session MCP passthrough carried from [`AgentInput`]: the
    /// sub-agent's client advertises these tool schemas and its executor
    /// routes their calls back through the parent session's MCP runtime.
    pub mcp_passthrough: Option<crate::registry::McpPassthrough>,
    /// Hook config inherited from the parent runtime so spawned agents honor
    /// SubagentStart/SubagentStop and regular tool hooks across the thread boundary.
    pub hook_config: RuntimeHookConfig,
    /// Cooperative cancel signal shared with the parent-side agent registry.
    /// Foreground Ctrl+C aborts it, and the sync conversation loop exits at the
    /// next safe boundary instead of running another model/tool iteration.
    pub cancel_signal: runtime::HookAbortSignal,
    /// The worker attempt this agent's route need judges (Phase 4 verdict
    /// channel), frozen when this verifier was bound to it — carried verbatim
    /// from [`super::AgentInput::judged_agent`]. `None` = no recognized
    /// judged worker (see that field's doc for the exact absence conditions).
    pub judged_agent: Option<AgentOutput>,
    /// The verify-by-default loop state this job carries when it IS the
    /// verifier spawned for an implementer (see `auto_verify`).
    pub verify_loop: Option<super::auto_verify::VerifyLoop>,
    /// The PARENT/session model this agent was spawned FROM (Phase 4 verdict
    /// channel — ad-hoc standalone review, source #3): the model whose route
    /// an ad-hoc reviewer's verdict is credited to when it judges the current
    /// turn's work rather than a specific sibling worker. `None` when the
    /// caller passed no parent model (e.g. a bare test harness) — a verdict
    /// recorder must skip recording rather than guess.
    pub parent_model: Option<String>,
    /// Mid-turn steering queue shared with the parent-side registry
    /// (`SendMessage` delivery). Created and registered at spawn time — before
    /// the detached thread exists — and installed into the sub-agent's runtime
    /// via `with_steering_queue`, so a send can never race the runtime build.
    pub steering: runtime::SteeringQueue,
    /// On-disk JSONL transcript for this agent's conversation
    /// (`<store>/<id>.session.jsonl`). Written incrementally while the turn
    /// runs and snapshotted in full when it ends, so a terminal agent can be
    /// resumed by `SendMessage` with its context intact. `None` only on bare
    /// test harnesses.
    pub transcript_path: Option<std::path::PathBuf>,
    /// Rehydrate the session from `transcript_path` instead of starting fresh
    /// — the `SendMessage` resume path. A missing/unreadable transcript fails
    /// the job rather than silently continuing without the prior context.
    pub resume: bool,
}

impl AgentJob {
    /// THE harness this job runs, resolved once (t-2513 contract 1).
    ///
    /// Both executors are fed from here and nowhere else: the inline runtime
    /// ([`build_agent_runtime`]) reads its prompt, tools, mode, rules and
    /// schema off this value, and the pane executor writes this same value
    /// into the child's brief. So `harness().digest()` is one number for one
    /// agent, whichever way it runs — the equivalence the design's first test
    /// pins. Nothing here is re-derived from `subagent_type`; that is what
    /// the child used to do, and it is exactly the drift this closes.
    pub(crate) fn harness(&self) -> ResolvedHarness {
        let mut allowed_tools = self.allowed_tools.clone();
        // 8c: a schema phase forces a final `StructuredOutput` call, so the
        // tool is part of the harness rather than a runtime afterthought.
        if self.schema.is_some() {
            allowed_tools.insert("StructuredOutput".to_string());
        }
        let mcp = self.mcp_passthrough.as_ref().map(|passthrough| McpRoute {
            tools: passthrough
                .definitions_snapshot()
                .into_iter()
                .filter(|definition| allowed_tools.contains(&definition.name))
                .map(|definition| McpTool {
                    name: definition.name,
                    description: definition.description,
                    input_schema: definition.input_schema,
                    required_permission: Some(definition.required_permission.as_str().to_string()),
                })
                .collect(),
            channel: runtime::subagent_panes::parent_channel(),
            method: runtime::subagent_panes::channel_method::MCP_CALL.to_string(),
        });
        ResolvedHarness {
            system_prompt: self.system_prompt.clone(),
            allowed_tools,
            inspection_shell: super::inspection_shell_for_subagent(self.manifest.subagent_type.as_deref().unwrap_or_default()),
            // Always said, never left to the child's own default: the
            // in-process executor's absent mode means `DangerFullAccess`
            // (`build_agent_runtime`), and a pane child that quietly ran
            // read-only instead would be a different agent from the one its
            // parent asked for.
            permission_mode: Some(
                self.permission_mode
                    .unwrap_or(PermissionMode::DangerFullAccess)
                    .as_str()
                    .to_string(),
            ),
            permission_rules: self.permission_rules.as_ref().map(PermissionRules::from),
            mcp,
            model: ModelSelection {
                requested: self.manifest.requested_model.clone(),
                effective: self
                    .manifest
                    .model
                    .clone()
                    .or_else(|| self.manifest.resolved_model.clone()),
            },
            effort: self.route_effort.map(|effort| effort_word(effort).to_string()),
            time_budget_ms: self
                .time_budget
                .map(|budget| u64::try_from(budget.as_millis()).unwrap_or(u64::MAX)),
            schema: self.schema.clone(),
            // Where the child works. An `Agent` call that named no directory
            // means "the parent's own", which for a THREAD is simply the
            // process cwd it inherits — a pane child inherits the WINDOW's
            // idea of the leader's checkout instead, so the parent's answer
            // is written down rather than assumed.
            cwd: self.cwd.clone().or_else(|| std::env::current_dir().ok()),
            registry_locator: self
                .registry
                .as_ref()
                .and_then(|registry| registry.locator().map(std::path::Path::to_path_buf)),
        }
    }

    /// The permission mode the harness names, as the enforcer needs it.
    pub(crate) fn harness_permission_mode(harness: &ResolvedHarness) -> PermissionMode {
        harness
            .permission_mode
            .as_deref()
            .and_then(PermissionMode::parse)
            .unwrap_or(PermissionMode::DangerFullAccess)
    }
}

/// zo's own spelling of a reasoning tier, for a brief and a child's command
/// line.
pub(super) fn effort_word(effort: api::EffortLevel) -> &'static str {
    match effort {
        api::EffortLevel::Low => "low",
        api::EffortLevel::Medium => "medium",
        api::EffortLevel::High => "high",
        api::EffortLevel::Xhigh => "xhigh",
        api::EffortLevel::Max => "max",
        api::EffortLevel::Ultra => "ultra",
    }
}

/// Per-turn cap on model turns for a spawned sub-agent of an UNATTENDED
/// parent. Reaching it no longer fails a healthy long-running task
/// immediately: [`run_agent_job`] grants a small number of fresh continuation
/// turns when the exhausted turn produced a successful tool result. The
/// per-turn cap still catches a locally stuck loop.
///
/// A helper of an ATTENDED parent has no cap ([`SpawnedAgentCaps::for_attendance`]):
/// the person watching the parent sees the helper's live row — tool count,
/// elapsed, what it is doing — and Esc on the parent aborts it. Capped, a
/// helper implementing a real slice was cut at its third window with the
/// work half done (203 messages, "Now let me verify my diff… and re-run.",
/// 2026-09-03), which is not a thing Claude Code does to a Task.
const SPAWNED_AGENT_MAX_ITERATIONS: usize = 64;

/// The iteration cap for a read-only `Explore` helper — half the general cap.
///
/// Measured on this repository's agent records (2026-09-10): twelve Explore
/// runs on a well-behaved fast model took 4–27 iterations; eleven on another
/// fast model were bimodal, 1–14 or 44–64, six of them at the 64 cap with
/// 18–28k output tokens and 150–256 tool calls each, three ending with no
/// report. A reconnaissance report needs nothing past 32 rounds (tool waves
/// make each round several calls), so the cap costs no well-behaved run its
/// answer and halves a wanderer's spend; the runtime's budget wrap-up turns
/// the cap into a report instead of a `failed` closer.
const EXPLORE_MAX_ITERATIONS: usize = 32;
const _: () = assert!(EXPLORE_MAX_ITERATIONS < SPAWNED_AGENT_MAX_ITERATIONS);

/// Maximum fresh turns granted after a recoverable budget cutoff. Two windows
/// let a legitimate implementation finish while bounding total provider spend;
/// hard-stop budgets (deadline, tool calls, verification treadmill) never earn
/// a continuation.
const SPAWNED_AGENT_MAX_BUDGET_CONTINUATIONS: u8 = 2;

/// Read-only agents cannot produce edit/plan events, so a substantial partial
/// deliverable can also earn a continuation. A short status sentence does not.
const SPAWNED_AGENT_MIN_CONTINUATION_TEXT_CHARS: usize = 256;

/// Hard cap on total model-requested tool calls for a spawned sub-agent. One
/// iteration can issue many parallel `tool_use` blocks, so the iteration cap
/// above does **not** bound tool-call volume on its own — the runaway that
/// prompted this reached 174 tool calls (2026-06-07). This complements
/// [`SPAWNED_AGENT_MAX_ITERATIONS`]: `256` is generous for a focused delegated
/// task while still hard-stopping a model that bursts tool calls every turn.
/// Enforcement and the graceful `failed` completion live in the conversation
/// loop (`check_tool_call_budget` → `record_turn_failed`).
const SPAWNED_AGENT_MAX_TOOL_CALLS: usize = 256;

/// The caps a spawned helper runs under, decided by who can stop it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SpawnedAgentCaps {
    iterations: usize,
    tool_calls: usize,
}

impl SpawnedAgentCaps {
    /// Unattended: the documented caps, with their continuation windows.
    /// Attended: none — the person at the parent's keyboard is the breaker,
    /// as in Claude Code, and the repetition guard still catches a loop.
    fn for_attendance(attendance: runtime::Attendance) -> Self {
        match attendance {
            runtime::Attendance::Attended => Self {
                iterations: usize::MAX,
                tool_calls: usize::MAX,
            },
            runtime::Attendance::Unattended => Self {
                iterations: SPAWNED_AGENT_MAX_ITERATIONS,
                tool_calls: SPAWNED_AGENT_MAX_TOOL_CALLS,
            },
        }
    }

    /// The caps for one spawn: attendance first, then the helper's shape — a
    /// read-only `Explore` runs under [`EXPLORE_MAX_ITERATIONS`] when nobody
    /// can stop it. Every other profile keeps the documented caps.
    fn for_spawn(attendance: runtime::Attendance, subagent_type: &str) -> Self {
        let mut caps = Self::for_attendance(attendance);
        let explore = runtime::SubagentProfileId::parse(subagent_type)
            .and_then(|profile| profile.builtin_profile())
            == Some(runtime::BuiltinSubagentProfile::Explore);
        if attendance == runtime::Attendance::Unattended && explore {
            caps.iterations = EXPLORE_MAX_ITERATIONS;
        }
        caps
    }
}

/// Spawn one sub-agent on its own OS thread and return immediately (the thread is
/// detached; completion is reported via [`notify_agent_completion`]).
///
/// **OS-thread bounding lives at the call site, not here.** This spawns a real
/// thread eagerly per agent; the API semaphore acquired *inside* the turn loop
/// (`workflow_api_semaphore` / `agent_api_semaphore`) bounds concurrent provider
/// streams, not thread count. The workflow engine bounds live threads by spawning
/// in windows of `workflow_concurrency_limit()` and awaiting each batch before
/// the next (`engine::spawn_and_collect`), so a multi-phase workflow stays within
/// the cap *except* for stragglers that overran the phase timeout. The flat
/// `SpawnMultiAgent` fan-out has no such window - its cap is its own concurrency
/// setting (default 1). A true permit-before-spawn OS-thread gate would belong
/// here, but a blocking acquire risks wedging the engine behind a non-terminating
/// straggler, so it is deliberately not added; see `engine.rs` for the windowing
/// contract this relies on.
struct AgentWorkerRegistrationGuard {
    agent_id: String,
    run_generation: u64,
}

impl Drop for AgentWorkerRegistrationGuard {
    fn drop(&mut self) {
        unregister_agent_cancel_signal(&self.agent_id, self.run_generation);
        super::unregister_agent_steering(&self.agent_id, self.run_generation);
    }
}

/// The same worker registration a threaded agent's runner holds, for the pane
/// runner in [`super::panes`].
///
/// It is what `agent_worker_is_live` answers about — the HUD rescues a running
/// manifest whose worker has gone — and a pane child's watcher is that worker
/// as much as a thread is. Registered by the caller and dropped here, so a
/// pane that ends any way at all stops claiming to be live.
pub(super) fn pane_worker_registration(job: &AgentJob) -> impl Drop + use<> {
    AgentWorkerRegistrationGuard {
        agent_id: job.manifest.agent_id.clone(),
        run_generation: job.manifest.run_generation,
    }
}

/// Publish a pane child's completion through the one channel every spawn
/// publishes through — route outcome recorded, background pump woken, and the
/// blocking `Agent` call's wait satisfied.
pub(super) fn publish_pane_completion(job: &AgentJob, completion: AgentCompletion) {
    notify_agent_completion_with_route_outcome(job, completion);
}

pub(super) fn spawn_agent_job(job: AgentJob) -> Result<(), ToolError> {
    let thread_name = format!("zo-agent-{}", job.manifest.agent_id);
    // This executor's mark on the manifest: a thread of this process. The
    // pane executor writes `pane` at the same moment (`stamp_agent_pane`).
    super::manifest::stamp_agent_execution(&job.manifest, super::EXECUTION_INLINE);
    std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let agent_id = job.manifest.agent_id.clone();
            let agent_name = job.manifest.name.clone();
            let _worker_registration = AgentWorkerRegistrationGuard {
                agent_id: agent_id.clone(),
                run_generation: job.manifest.run_generation,
            };
            // Owned here (not inside `run_agent_job`) so the budget total is
            // readable on *every* terminal branch — including a returned error or
            // an unwinding panic, where the inner function never hands anything
            // back. The provider client accumulates into it (and into the display
            // sparkline) through the clones passed into the runtime.
            let token_history: std::sync::Arc<std::sync::Mutex<Vec<u32>>> =
                std::sync::Arc::default();
            // Never-lossy output-token total (the workflow budget source),
            // separate from the display-capped `token_history`.
            let output_tokens_total: std::sync::Arc<std::sync::atomic::AtomicU64> =
                std::sync::Arc::default();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                super::manifest::with_agent_run_generation(job.manifest.run_generation, || {
                    run_agent_job(&job, &token_history, &output_tokens_total)
                })
            }));
            // Read after the turn fully returns (or unwinds) — every `fetch_add`
            // happens-before this load, so the total is exact.
            let output_tokens = output_tokens_total.load(std::sync::atomic::Ordering::Relaxed);
            // Identity and measured cost are filled ONCE for all three
            // terminal arms below. They used to be three hand-written
            // literals, so every new field had to be remembered three times —
            // and a forgotten one reads as a truthful zero rather than as a
            // compile error.
            let terminal = |status: &str,
                            result: Option<String>,
                            structured: Option<serde_json::Value>,
                            error: Option<String>| AgentCompletion {
                agent_id: agent_id.clone(),
                name: agent_name.clone(),
                status: status.to_string(),
                result,
                structured,
                error,
                run: HelperRun {
                    output_tokens,
                    ..HelperRun::default()
                },
            };
            match result {
                Ok(Ok(outcome)) => {
                    let structured = completion_structured_with_provider_error_class(
                        outcome.structured,
                        outcome.provider_error_class,
                    );
                    // `run_agent_job` already persisted the terminal manifest
                    // (completed, or failed-with-partial-work on a budget cutoff)
                    // and fired `SubagentStop`; here we only surface the in-memory
                    // completion, carrying whichever terminal status it settled on.
                    notify_agent_completion_with_route_outcome(
                        &job,
                        terminal(
                            outcome.status,
                            Some(outcome.final_text),
                            structured,
                            outcome.error,
                        ),
                    );
                }
                Ok(Err(error)) => {
                    let provider_error_class = error.provider_error_class();
                    let error = error.to_string();
                    let status = if agent_error_is_cancelled(&error) {
                        let _ = persist_agent_stopped_state(&job.manifest, error.as_str());
                        "stopped"
                    } else {
                        let _ = persist_agent_terminal_state(
                            &job.manifest,
                            "failed",
                            None,
                            Some(error.clone()),
                        );
                        "failed"
                    };
                    notify_agent_completion_with_route_outcome(
                        &job,
                        terminal(
                            status,
                            None,
                            provider_error_class.map(provider_error_class_metadata),
                            Some(error),
                        ),
                    );
                }
                Err(_) => {
                    let panic_msg = String::from("sub-agent thread panicked");
                    let _ = persist_agent_terminal_state(
                        &job.manifest,
                        "failed",
                        None,
                        Some(panic_msg.clone()),
                    );
                    notify_agent_completion_with_route_outcome(
                        &job,
                        terminal("failed", None, None, Some(panic_msg)),
                    );
                }
            }
        })
        .map(|_| ())
        .map_err(|error| ToolError::Execution(error.to_string()))
}

fn reconcile_completion_terminal_status(
    completion: &mut AgentCompletion,
    status: &str,
    persisted_error: Option<&str>,
) {
    if !super::agent_output_status_is_terminal(status) || completion.status == status {
        return;
    }
    completion.status = status.to_string();
    if completion.error.is_none() {
        completion.error = persisted_error
            .map(str::to_string)
            .or_else(|| (status == "stopped").then(|| "agent stopped".to_string()));
    }
}

/// What the agent's own manifest counted for this run.
///
/// The manifest is the truth for both numbers, and the worker thread can
/// supply neither: its token counter knows nothing about tool calls, and it
/// keeps no record of when the run began. `toolCalls` is stamped as each tool
/// starts, and `startedAt`/`completedAt` bracket the run — a manifest that has
/// not been stamped with an end yet is measured against now, which is what a
/// still-arriving completion means.
fn run_from_manifest(stored: &AgentOutput, output_tokens: u64) -> HelperRun {
    let epoch = |value: Option<&str>| value.and_then(|value| value.parse::<u64>().ok());
    let started = epoch(stored.started_at.as_deref());
    let finished =
        epoch(stored.completed_at.as_deref()).unwrap_or_else(super::manifest::epoch_seconds_now_u64);
    HelperRun {
        tool_calls: u64::try_from(stored.tool_calls).unwrap_or(u64::MAX),
        output_tokens,
        elapsed: started.map(|started| Duration::from_secs(finished.saturating_sub(started))),
    }
}

fn reconcile_completion_with_manifest(manifest: &AgentOutput, completion: &mut AgentCompletion) {
    let Some(stored) = super::manifest::load_agent_manifest_from_scanned_path(
        std::path::Path::new(&manifest.manifest_file),
    )
    .ok()
    else {
        return;
    };
    reconcile_completion_terminal_status(completion, &stored.status, stored.error.as_deref());
    // The cost the worker measured in memory (output tokens) meets the cost
    // only the manifest kept (tools started, wall clock) — one place, so every
    // surface that spells `Done (…)` reads the same three numbers.
    completion.run = run_from_manifest(&stored, completion.run.output_tokens);
}

fn notify_agent_completion_with_route_outcome(job: &AgentJob, mut completion: AgentCompletion) {
    if !manifest_generation_is_current(&job.manifest) {
        return;
    }
    // An external stop can win the durable terminal transition while a provider
    // stream is already returning. Keep the richer worker payload, but the
    // user-facing status and route attribution must reflect the durable winner.
    reconcile_completion_with_manifest(&job.manifest, &mut completion);
    notify_agent_completion(
        completion.clone(),
        Some(job.manifest.run_generation),
        Some(std::path::PathBuf::from(&job.manifest.manifest_file)),
    );
    record_agent_route_outcome(job, &completion);
    record_agent_verdict_outcome(job, &completion);
    // P2 live: an unattended implementation spawn is verified by default, and
    // a verifier's failing verdict re-spawns the implementer with the finding
    // up to the difficulty ceiling, all under the implementer's own execution
    // contract; any ending without a pass sends the owning session an explicit
    // "not verified" receipt. Attended turns keep the ask-once rule.
    super::auto_verify::maybe_auto_verify(job, &completion, spawn_agent_job);
    super::auto_verify::maybe_continue_verify_loop(job, &completion, spawn_agent_job);
}

/// Pure pass/fail projection of `semantic_verdict`'s label. `None` for
/// anything ambiguous (the `"retry"` label — status completed but no usable
/// structured verdict recovered, e.g. missing/malformed `StructuredOutput` —
/// or any other unrecognized label) — never a guess, per the shared
/// verdict-attribution doctrine.
pub(super) fn verdict_passed(label: &str) -> Option<bool> {
    match label {
        "pass" => Some(true),
        "finding" => Some(false),
        _ => None,
    }
}

/// Phase 4 verdict channel — sources #2 and #3: a completed verifier/reviewer
/// agent's own structured verdict, folded back onto whatever it judged.
/// Reuses the workflow engine's EXISTING verdict recorder and
/// structured-verdict classifier verbatim (`workflow_tools::engine::
/// attribution`/`items::semantic_verdict`, already `pub(crate)` for this)
/// rather than a second parser — one classifier, multiple callers. Never
/// guesses a pass or a failure out of anything ambiguous: provenance over
/// volume. A bound verifier that settled nothing only marks the attempt it
/// judged as unverified (`record_bound_verdict`).
fn record_agent_verdict_outcome(job: &AgentJob, completion: &AgentCompletion) {
    // Only a finished verifier says anything — a live placeholder never does.
    if !runtime::is_terminal_outcome_status(&completion.status) {
        return;
    }
    let verdict = crate::workflow_tools::engine::items::semantic_verdict(
        &completion.status,
        completion.structured.as_ref(),
    );
    let passed = verdict.and_then(verdict_passed);

    // Source #2: planner-bound reviewer→worker — an exact, host-resolved
    // binding to the JUDGED worker's attempt, frozen when this verifier was
    // bound (see `AgentInput::judged_agent`'s doc for how the binding is
    // established and its exact absence conditions).
    if let Some(judged) = job.judged_agent.as_ref() {
        record_bound_verdict(job, judged, verdict, passed);
        return;
    }
    let Some(passed) = passed else {
        return;
    };

    // Source #3: an ad-hoc standalone review/verification agent judging the
    // PARENT turn's own work — conservative whitelist only (see
    // `is_ad_hoc_turn_review`), never a guess when the target is unclear.
    if !is_ad_hoc_turn_review(job) {
        return;
    }
    let Some(parent_model) = job.parent_model.as_deref() else {
        // No parent model available at this seam (e.g. a bare test harness,
        // or a caller that never threaded one through) — skip rather than
        // credit an unknown route.
        return;
    };
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let mut record = RouteOutcomeRecord::new(
        "main",
        "turn",
        crate::misc_tools::canonicalize_route_model_id(parent_model),
        if passed { "completed" } else { "failed" },
    )
    .with_signal("verdict")
    .with_decision(runtime::DecisionKind::Verify)
    // Strict pass/fail judgement — same convention/weight as
    // `workflow_tools::engine::attribution::VerdictKind::PassFail`.
    .with_signal_weight(Some(1.0));
    // The turn this verdict is ABOUT. A main-turn verdict used to name no
    // attempt at all, which is exactly why it met no request row and no
    // timing row: `<sessionId>@<turn>` is the key that joins it to what the
    // turn actually spent.
    //
    // Only `run_id`. The fact that the REVIEWER served this same turn is
    // already on the reviewer's own run row (`record_agent_route_outcome`
    // stamps its `parent_attempt`); writing it here too would put one fact in
    // two places, and on this row it would say nothing `run_id` does not.
    if let Some(judged_turn) = job.parent_attempt.clone() {
        // And the shape that turn ran under, as the host deposited it.
        record = record
            .with_shape_label(api::plan_shape_for_attempt(&judged_turn).unwrap_or_default())
            .with_attempt_key(judged_turn);
    }
    let _ = runtime::record_route_outcome(&cwd, &record);
}

/// A bound verifier's ending, folded onto the attempt it judged. A settled
/// verdict is that attempt's pass or failure. Anything else — the verifier
/// never finished, or finished without a usable verdict — leaves the worker's
/// attempt UNVERIFIED: its completion stops counting as a win, and nothing
/// counts against the work, since a verifier's crash or parse failure is not
/// evidence the work was wrong. When the verifier did finish but its output
/// was unusable (`retry`, the workflow's `Invalid`), that is the VERIFIER's
/// own failure, recorded on its own attempt exactly as the workflow repair
/// loop records it. Whatever is said about the worker is written from
/// `judged`, the attempt frozen at binding — never the store's copy, which a
/// resume may have moved to another generation and model by now.
fn record_bound_verdict(
    job: &AgentJob,
    judged: &AgentOutput,
    verdict: Option<&str>,
    passed: Option<bool>,
) {
    use crate::workflow_tools::engine::attribution;
    if let Some(passed) = passed {
        attribution::record_verdict_outcome_for_attempt(
            Some(judged),
            passed,
            attribution::VerdictKind::PassFail,
            runtime::VerdictBasis::Model,
        );
        return;
    }
    attribution::record_verification_unavailable_for_attempt(Some(judged));
    if verdict == Some("retry") {
        attribution::record_verdict_outcome_for_agent(
            job.registry.as_deref(),
            &job.manifest.agent_id,
            false,
            attribution::VerdictKind::ValidatorFault,
            runtime::VerdictBasis::Model,
        );
    }
}

/// Substring markers a review/verification prompt uses to name the CURRENT
/// turn's own diff/working tree (Phase 4 verdict channel source #3's
/// whitelist). Faithful to this repo's own review prompts — mirrors
/// `workflow_tools::presets`'s `cross_model_verified` preset ("review the
/// current working tree", "Inspect the current diff") — plus their Korean
/// equivalents (the classifier keyword tables' existing parity convention).
const CURRENT_TURN_REVIEW_MARKERS: &[&str] = &[
    "current diff",
    "this diff",
    "working tree",
    "uncommitted",
    "이 diff",
    "현재 diff",
    "워킹트리",
    "미커밋",
];

/// Conservative whitelist for Phase 4 verdict channel source #3 (ad-hoc
/// standalone review): a bare reviewer/verifier spawn — no judged-agent
/// binding — is credited to the PARENT turn ONLY when it is unmistakably
/// reviewing the turn's OWN current work, never a delegated/unrelated task.
/// Requires BOTH:
/// - the resolved built-in type is the reviewer/verification harness
///   (`"code-reviewer"` or `"Verification"` — the SAME canonical strings
///   `route_outcome_target` already keys run-level outcomes by), and
/// - the prompt explicitly names the current turn's diff/working tree
///   (`CURRENT_TURN_REVIEW_MARKERS`).
///
/// Any other subagent type, or a prompt that never names the CURRENT
/// diff/tree, returns `false` — this fn only recognizes what it's certain of;
/// a delegated review of some OTHER (unrelated) diff/file must never be
/// credited to the main turn.
fn is_ad_hoc_turn_review(job: &AgentJob) -> bool {
    let subagent_type = job.manifest.subagent_type.as_deref().unwrap_or_default();
    if !matches!(subagent_type, "code-reviewer" | "Verification") {
        return false;
    }
    let haystack = job.prompt.to_ascii_lowercase();
    CURRENT_TURN_REVIEW_MARKERS
        .iter()
        .any(|marker| haystack.contains(marker))
}

fn record_agent_route_outcome(job: &AgentJob, completion: &AgentCompletion) {
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let manifest = current_on_disk_manifest_or_spawn_time(&job.manifest);
    // A classification call the host paid before it decided is the routing
    // TAX, not a route worth learning from: one row, filed as the tax, in
    // place of the subagent row — never both, or the same call would be
    // counted twice and `classifier` would look like a route that keeps
    // succeeding.
    let record = if let Some(call) = job.route_tax {
        route_tax_record(call, &manifest, completion, job)
    } else {
        let record = route_outcome_record(&manifest, completion, job.route_effort, &job.manifest)
            .with_shape(spawn_plan_shape(job));
        match served_attempt(job) {
            Some(parent) => record.with_parent_attempt(parent),
            None => record,
        }
    };
    let _ = runtime::record_route_outcome(&cwd, &record);
}

/// The tax row for a classification spawn: what the call cost, and which
/// attempt paid it. Carries no role/complexity/route provenance — those
/// describe a WORK route, and this row is not one.
fn route_tax_record(
    call: runtime::RouteTaxCall,
    manifest: &AgentOutput,
    completion: &AgentCompletion,
    job: &AgentJob,
) -> RouteOutcomeRecord {
    let record = RouteOutcomeRecord::route_tax(
        call,
        crate::misc_tools::canonicalize_route_model_id(&selected_route_model(manifest)),
        completion_status(completion),
    )
    .with_output_tokens(completion.run.output_tokens)
    .with_duration_ms(run_duration_ms(manifest));
    match served_attempt(job) {
        Some(parent) => record.with_attempt_key(parent),
        None => record,
    }
}

/// The attempt this spawn SERVED — the one its cost rolls up into.
///
/// A verifier serves the attempt it JUDGES, not the turn that launched it:
/// the tokens it spends are the price of settling that attempt, and rolling
/// them into the turn instead would price the verification onto whatever else
/// that turn did. Everything else serves the turn that launched it, captured
/// at spawn time (`AgentJob::parent_attempt`).
fn served_attempt(job: &AgentJob) -> Option<String> {
    if let Some(judged) = job.judged_agent.as_ref() {
        return runtime::spawn_attempt_key(&judged.agent_id, judged.run_generation);
    }
    job.parent_attempt.clone()
}

/// The shape this spawn ran under: whatever the host laid out, and otherwise
/// one delegation — which is what a spawn nobody laid out IS.
fn spawn_plan_shape(job: &AgentJob) -> runtime::PlanShape {
    job.plan_shape.unwrap_or(runtime::PlanShape::Delegate)
}

/// Prefer the manifest's CURRENT on-disk state over the spawn-time in-memory
/// copy captured in `job.manifest` (**verified misattribution fix**): a
/// mid-run rate-limit/starvation swap (`record_agent_runtime_model`,
/// `manifest.rs:~301`, triggered by `provider_client.rs`'s
/// `switch_runtime_model`) updates the on-disk manifest's `resolvedModel` /
/// `model` and fallback provenance — the in-memory `job.manifest` this recorder
/// used to read verbatim never observes them, so a quota-pressure run credited
/// the model the agent had already swapped AWAY FROM. By the time this fires,
/// the terminal `persist_agent_*_state` call has already written the final
/// status to the SAME manifest file, so a fresh read here picks up both the
/// final model AND the terminal timestamps. Falls back to the in-memory
/// manifest verbatim on any read/parse failure (e.g. a test harness that
/// never wrote a real manifest file, or the file already having been cleaned
/// up) — never a hard failure.
///
/// The file speaks only while it is still THIS attempt's: once a resume has
/// advanced it to a newer generation (possibly on another model, route or
/// effort), nothing it says belongs to the run that ended, and the spawn-time
/// copy is this attempt's truth (t-3959). The same copy is what a
/// verify-by-default verifier is bound to.
pub(super) fn current_on_disk_manifest_or_spawn_time(spawn_time_manifest: &AgentOutput) -> AgentOutput {
    super::manifest::load_agent_manifest_from_scanned_path(std::path::Path::new(
        &spawn_time_manifest.manifest_file,
    ))
    .ok()
    .filter(|on_disk| {
        on_disk.agent_id == spawn_time_manifest.agent_id
            && on_disk.run_generation == spawn_time_manifest.run_generation
    })
    .unwrap_or_else(|| spawn_time_manifest.clone())
}

/// `attempt` is the JOB's own manifest — the run that just ended. Its id and
/// generation name the attempt, not the on-disk copy's: once the completion
/// is published a resume may already have advanced the file's generation, and
/// a verdict about THIS run must meet THIS run's outcome.
fn route_outcome_record(
    manifest: &AgentOutput,
    completion: &AgentCompletion,
    route_effort: Option<api::EffortLevel>,
    attempt: &AgentOutput,
) -> RouteOutcomeRecord {
    RouteOutcomeRecord::new(
        "subagent",
        route_outcome_target(manifest),
        // P3 canonicalization-at-write: new records are keyed by the
        // canonical model id from the start, so they never need a read-time
        // merge (`claude-opus-4-8` vs a hand-typed `claude-opus-4.8`, etc.).
        crate::misc_tools::canonicalize_route_model_id(&selected_route_model(manifest)),
        completion_status(completion),
    )
    .with_decision(runtime::DecisionKind::Model)
    .with_requested_model(requested_route_model(manifest))
    .with_provider_error_class(provider_error_class_label(completion))
    .with_output_tokens(completion.run.output_tokens)
    .with_role(manifest.route_role.clone())
    .with_complexity(manifest.route_complexity.clone())
    .with_risk(manifest.route_risk.clone())
    .with_route_source(manifest.route_source.clone())
    .with_probe_confidence(manifest.route_probe_confidence.clone())
    .with_effort_level(effort_level_label(route_effort))
    .with_duration_ms(run_duration_ms(manifest))
    .with_attempt(&attempt.agent_id, attempt.run_generation)
}

/// `EffortLevel`'s lowercase wire token (`#[serde(rename_all = "lowercase")]`)
/// — the same projection `apply.rs` uses to smuggle it, reused here instead
/// of a parallel token table. `route_effort` is the Smart router's
/// RECOMMENDATION for this route (`AgentJob::route_effort`), not necessarily
/// the effort the provider request ultimately used (a starvation/budget
/// clamp can still lower it) — the closest available signal without
/// threading the provider client's actual per-request effort back through
/// the job boundary.
fn effort_level_label(route_effort: Option<api::EffortLevel>) -> Option<String> {
    route_effort
        .and_then(|effort| serde_json::to_value(effort).ok())
        .and_then(|value| value.as_str().map(str::to_string))
}

/// Best-effort wall-clock run duration from the manifest's own
/// `startedAt`/`completedAt` (epoch-second strings) — `None` on any missing/
/// unparsable timestamp rather than a guess.
fn run_duration_ms(manifest: &AgentOutput) -> Option<u64> {
    let started: u64 = manifest.started_at.as_deref()?.parse().ok()?;
    let completed: u64 = manifest.completed_at.as_deref()?.parse().ok()?;
    Some(completed.saturating_sub(started).saturating_mul(1000))
}

fn route_outcome_target(manifest: &AgentOutput) -> String {
    non_empty_owned(manifest.subagent_type.as_deref())
        .unwrap_or_else(|| "general-purpose".to_string())
}

fn selected_route_model(manifest: &AgentOutput) -> String {
    non_empty_owned(manifest.resolved_model.as_deref())
        .or_else(|| non_empty_owned(manifest.model.as_deref()))
        .unwrap_or_else(|| "unknown".to_string())
}

fn requested_route_model(manifest: &AgentOutput) -> Option<String> {
    non_empty_owned(manifest.requested_model.as_deref())
}

fn completion_status(completion: &AgentCompletion) -> String {
    completion.status.clone()
}

fn provider_error_class_label(completion: &AgentCompletion) -> Option<String> {
    completion
        .structured
        .as_ref()?
        .get("providerErrorClass")?
        .as_str()
        .and_then(|value| non_empty_owned(Some(value)))
}

fn non_empty_owned(value: Option<&str>) -> Option<String> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

#[cfg(test)]
pub(super) fn route_outcome_record_for_tests(
    manifest: &AgentOutput,
    completion: &AgentCompletion,
    attempt: &AgentOutput,
) -> RouteOutcomeRecord {
    route_outcome_record(manifest, completion, None, attempt)
}

#[cfg(test)]
pub(super) fn current_on_disk_manifest_or_spawn_time_for_tests(
    spawn_time_manifest: &AgentOutput,
) -> AgentOutput {
    current_on_disk_manifest_or_spawn_time(spawn_time_manifest)
}

/// A sub-agent turn's terminal outcome, handed back to [`spawn_agent_job`] for
/// notification. `status` is `"completed"` for a natural end and `"failed"` when
/// the turn stopped on an exhausted budget — in which case `final_text` still
/// carries the preserved partial work (the runtime no longer discards it), so
/// the parent and the transcript viewer both see what got done.
struct AgentJobOutcome {
    final_text: String,
    structured: Option<serde_json::Value>,
    status: &'static str,
    /// Why a non-`completed` outcome failed (e.g. the budget-exhausted kind).
    /// Carried into the `AgentCompletion` so the parent's notice names the
    /// real cause instead of fabricating "unknown error" for an agent that
    /// visibly returned a (partial) result.
    error: Option<String>,
    /// Provider classification survives a failed continuation so route health
    /// does not misclassify quota/transport failures as model-quality failures.
    provider_error_class: Option<api::ProviderErrorClass>,
}

fn agent_budget_can_auto_continue(kind: runtime::BudgetExhausted) -> bool {
    // A sub-agent continuation is a fresh model-loop window, not a fresh cost
    // budget. Only the local iteration guard is recoverable; token, deadline,
    // tool-call, verification-treadmill, and tool-repetition cutoffs remain hard
    // stops. The last two are loop-detection stops in particular: continuing one
    // hands the model back the very context that produced the loop, so it would
    // buy another identical round rather than progress.
    kind == runtime::BudgetExhausted::Iterations
}

fn subagent_allows_text_progress(
    subagent_type: &str,
    permission_mode: Option<runtime::PermissionMode>,
) -> bool {
    permission_mode == Some(runtime::PermissionMode::ReadOnly)
        || matches!(
            subagent_type,
            "Explore"
                | "Plan"
                | "Verification"
                | "deep-research"
                | "code-reviewer"
                | "data-analyst"
        )
}

fn continuation_text_is_meaningful(text: &str) -> bool {
    text.chars().filter(|ch| !ch.is_whitespace()).count()
        >= SPAWNED_AGENT_MIN_CONTINUATION_TEXT_CHARS
}

fn agent_turn_has_successful_tool_result(summary: &runtime::TurnSummary) -> bool {
    summary
        .tool_results
        .iter()
        .flat_map(|message| &message.blocks)
        .any(|block| {
            matches!(
                block,
                runtime::session::ContentBlock::ToolResult {
                    is_error: false,
                    ..
                }
            )
        })
}

fn agent_turn_made_progress(
    summary: &runtime::TurnSummary,
    allow_text_progress: bool,
) -> bool {
    summary.progress_tool_results() > 0
        || (allow_text_progress
            && agent_turn_has_successful_tool_result(summary)
            && continuation_text_is_meaningful(&last_substantive_assistant_text(
                &summary.assistant_messages,
            )))
}

fn agent_turn_tool_calls(summary: &runtime::TurnSummary) -> usize {
    summary
        .assistant_messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter(|block| matches!(block, runtime::session::ContentBlock::ToolUse { .. }))
        .count()
}

struct AgentContinuationState {
    /// The tool-call cap this helper runs under ([`SpawnedAgentCaps`]) —
    /// what `remaining_tool_calls` counts down from, so an uncapped helper of
    /// an attended parent never reads as out of budget.
    tool_call_cap: usize,
    continuations_used: u8,
    /// The one tool-free synthesis turn granted after the tool-call budget
    /// ran out (see [`agent_wrap_up_eligible`]). Separate from
    /// `continuations_used`: a wrap-up spends no tool budget by construction,
    /// so it must not consume (or be blocked by) the auto-continue allowance.
    wrap_up_used: bool,
    /// Whether ANY turn so far produced a successful tool result — the
    /// wrap-up's cumulative "gathered something worth synthesizing" test. A
    /// single turn's `tool_results` is not enough: an auto-continued turn can
    /// re-exhaust with an empty result set of its own (a parallel batch
    /// larger than the re-armed remainder is dropped whole) after earlier
    /// turns gathered hundreds.
    gathered_tool_result: bool,
    cumulative_iterations: usize,
    cumulative_tool_calls: usize,
    cumulative_output_tokens: u32,
    cumulative_input_tokens: u32,
    latest_structured: Option<serde_json::Value>,
    latest_budget_text: String,
    latest_budget_result: String,
}

impl Default for AgentContinuationState {
    /// The unattended caps — the documented ones every spawn test runs under.
    fn default() -> Self {
        Self::for_caps(SpawnedAgentCaps::for_attendance(
            runtime::Attendance::Unattended,
        ))
    }
}

impl AgentContinuationState {
    fn for_caps(caps: SpawnedAgentCaps) -> Self {
        Self {
            tool_call_cap: caps.tool_calls,
            continuations_used: 0,
            wrap_up_used: false,
            gathered_tool_result: false,
            cumulative_iterations: 0,
            cumulative_tool_calls: 0,
            cumulative_output_tokens: 0,
            cumulative_input_tokens: 0,
            latest_structured: None,
            latest_budget_text: String::new(),
            latest_budget_result: String::new(),
        }
    }

    fn observe(
        &mut self,
        summary: &runtime::TurnSummary,
        schema: Option<&serde_json::Value>,
    ) {
        self.cumulative_iterations = self
            .cumulative_iterations
            .saturating_add(summary.iterations);
        self.cumulative_tool_calls = self
            .cumulative_tool_calls
            .saturating_add(agent_turn_tool_calls(summary));
        self.gathered_tool_result |= agent_turn_has_successful_tool_result(summary);
        self.cumulative_output_tokens = self
            .cumulative_output_tokens
            .saturating_add(summary.turn_output_tokens);
        // `usage.input_tokens` is cumulative for this runtime. Keep the high
        // water mark so compaction/accounting resets cannot create fake budget.
        self.cumulative_input_tokens = self
            .cumulative_input_tokens
            .max(summary.usage.input_tokens);
        if let Some(schema) = schema {
            if let Some(structured) = final_structured_output(summary, schema) {
                self.latest_structured = Some(structured);
            }
        }
        if let Some(kind) = summary.budget_exhausted {
            let text = last_substantive_assistant_text(&summary.assistant_messages);
            if !text.trim().is_empty() {
                // Preserve the latest checkpoint, even when it is concise. A
                // later short conclusion is more current than an older essay.
                self.latest_budget_text = text;
            }
            self.latest_budget_result = agent_budget_exhausted_result(
                &self.latest_budget_text,
                kind,
                self.cumulative_iterations,
                self.cumulative_tool_calls,
            );
        }
    }

    fn remaining_tool_calls(&self) -> usize {
        self.tool_call_cap.saturating_sub(self.cumulative_tool_calls)
    }

    fn remaining_output_tokens(&self, budget: Option<u32>) -> Option<u32> {
        budget.map(|budget| budget.saturating_sub(self.cumulative_output_tokens))
    }

    fn remaining_input_tokens(&self, budget: Option<u32>) -> Option<u32> {
        budget.map(|budget| budget.saturating_sub(self.cumulative_input_tokens))
    }

    fn token_budgets_have_headroom(
        &self,
        output_budget: Option<u32>,
        input_budget: Option<u32>,
    ) -> bool {
        output_budget.is_none_or(|budget| self.cumulative_output_tokens < budget)
            && input_budget.is_none_or(|budget| self.cumulative_input_tokens < budget)
    }
}

fn continuation_error_result(
    continuation: &AgentContinuationState,
    error: &str,
) -> Option<String> {
    let partial = if !continuation.latest_budget_text.trim().is_empty() {
        continuation.latest_budget_text.clone()
    } else if continuation.latest_structured.is_some() {
        "The agent returned a partial result via StructuredOutput.".to_string()
    } else if !continuation.latest_budget_result.trim().is_empty() {
        continuation.latest_budget_result.clone()
    } else {
        return None;
    };
    Some(format!(
        "{partial}\n\n[zo:auto-continue] The continuation failed: {error}"
    ))
}

fn completion_structured_with_provider_error_class(
    structured: Option<serde_json::Value>,
    provider_error_class: Option<api::ProviderErrorClass>,
) -> Option<serde_json::Value> {
    let Some(provider_error_class) = provider_error_class else {
        return structured;
    };
    let serde_json::Value::Object(mut metadata) =
        provider_error_class_metadata(provider_error_class)
    else {
        return structured;
    };
    match structured {
        None => Some(serde_json::Value::Object(metadata)),
        Some(serde_json::Value::Object(mut result)) => {
            result.extend(metadata);
            Some(serde_json::Value::Object(result))
        }
        Some(result) => {
            metadata.insert("structuredResult".to_string(), result);
            Some(serde_json::Value::Object(metadata))
        }
    }
}

fn should_auto_continue_budget(
    kind: Option<runtime::BudgetExhausted>,
    made_progress: bool,
    continuations_used: u8,
    remaining_tool_calls: usize,
    token_budgets_have_headroom: bool,
) -> bool {
    continuations_used < SPAWNED_AGENT_MAX_BUDGET_CONTINUATIONS
        && remaining_tool_calls > 0
        && token_budgets_have_headroom
        && kind.is_some_and(agent_budget_can_auto_continue)
        && made_progress
}

/// What the agent loop does with a turn that came back.
///
/// The three arms were only ever expressed as the order of two `if`s inside the
/// loop, which made the *sequence* — auto-continue, then at most one wrap-up,
/// then stop — untestable without a live provider. Naming it lets a test drive
/// the real decision over a scripted run instead of restating the loop's shape,
/// which would drift from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentTurnStep {
    /// Budget ran out with work still worth doing: re-arm and prompt again.
    Continue(runtime::BudgetExhausted),
    /// Tool budget is spent but results are gathered: one tool-free synthesis
    /// turn instead of failing with a mid-sentence partial.
    WrapUp,
    /// Nothing further to grant; the loop breaks on this summary.
    Stop,
}

/// Decide what follows `summary`. Pure: the loop applies the runtime effects.
fn agent_turn_step(
    summary: &runtime::TurnSummary,
    continuation: &AgentContinuationState,
    schema: Option<&serde_json::Value>,
    remaining_tool_calls: usize,
    token_budgets_have_headroom: bool,
    allow_text_progress: bool,
) -> AgentTurnStep {
    if should_auto_continue_agent(
        summary,
        schema,
        continuation.continuations_used,
        remaining_tool_calls,
        token_budgets_have_headroom,
        allow_text_progress,
    ) {
        // The predicate above only fires on an exhausted budget, so this is
        // the kind that fired rather than an assumption about it.
        if let Some(kind) = summary.budget_exhausted {
            return AgentTurnStep::Continue(kind);
        }
    }
    // A schema agent that ALREADY submitted its StructuredOutput needs no
    // synthesis turn — its deliverable is in hand, and the terminal disposition
    // accepts a tool-call cutoff with a submission as completed; prompting it
    // to "call StructuredOutput exactly once" again would only solicit a
    // duplicate overwrite. This guard is also what lets the deliverable gate
    // read `has_structured` as "the wrap-up itself delivered".
    let already_submitted = schema.is_some() && continuation.latest_structured.is_some();
    if token_budgets_have_headroom
        && !already_submitted
        && agent_wrap_up_eligible(
            summary,
            continuation.wrap_up_used,
            continuation.gathered_tool_result,
        )
    {
        return AgentTurnStep::WrapUp;
    }
    AgentTurnStep::Stop
}

fn should_auto_continue_agent(
    summary: &runtime::TurnSummary,
    schema: Option<&serde_json::Value>,
    continuations_used: u8,
    remaining_tool_calls: usize,
    token_budgets_have_headroom: bool,
    allow_text_progress: bool,
) -> bool {
    // A schema agent that already submitted its deliverable is done; a schema
    // agent cut off before `StructuredOutput` is eligible like any other agent.
    schema.is_none_or(|schema| final_structured_output(summary, schema).is_none())
        && should_auto_continue_budget(
            summary.budget_exhausted,
            agent_turn_made_progress(summary, allow_text_progress),
            continuations_used,
            remaining_tool_calls,
            token_budgets_have_headroom,
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentTerminalDisposition {
    Completed,
    BudgetFailed(runtime::BudgetExhausted),
}

fn agent_terminal_disposition(
    summary: &runtime::TurnSummary,
    schema_requested: bool,
    has_structured: bool,
    wrap_up_used: bool,
) -> AgentTerminalDisposition {
    match summary.budget_exhausted {
        // A schema agent whose deliverable is already in hand is not a
        // failure when the fence that cut it bounds WORK (iterations or tool
        // calls): the extra iteration/call it attempted past its
        // StructuredOutput submission changes nothing about the submission.
        // Hard resource fences (deadline, tokens) stay failures — a
        // deliverable must not bypass those.
        Some(runtime::BudgetExhausted::Iterations | runtime::BudgetExhausted::ToolCalls)
            if schema_requested && has_structured =>
        {
            AgentTerminalDisposition::Completed
        }
        Some(kind) => AgentTerminalDisposition::BudgetFailed(kind),
        // The wrap-up deliverable gate: a granted synthesis turn that ended
        // without producing anything NEW — no final text, and (for a schema
        // agent) no StructuredOutput submission — must fall back to the
        // honest budget failure it was granted to redeem. Without this, a
        // wrap-up that merely terminated turned `failed`+banner into a bare
        // `completed` whose text was the OLD mid-sentence partial: a fake
        // success that also fed the router's learned win rates. (The
        // structured check is sound because a schema agent that already
        // holds a submission never receives a wrap-up — see the eligibility
        // guard in `run_agent_job` — so `has_structured` here can only mean
        // the wrap-up turn itself delivered.)
        None if wrap_up_used
            && final_assistant_text(summary).trim().is_empty()
            && !(schema_requested && has_structured) =>
        {
            AgentTerminalDisposition::BudgetFailed(runtime::BudgetExhausted::ToolCalls)
        }
        None => AgentTerminalDisposition::Completed,
    }
}

fn agent_budget_continuation_prompt(
    kind: runtime::BudgetExhausted,
    continuation: u8,
) -> String {
    format!(
        "[zo:auto-continue] The previous sub-agent turn reached its {} after making progress. \
         Continue the same task from the preserved conversation and finish the requested work and \
         verification now. Do not restart or merely summarize. Continuation {continuation}/{}.",
        budget_exhausted_kind_label(kind),
        SPAWNED_AGENT_MAX_BUDGET_CONTINUATIONS
    )
}

/// Whether an exhausted turn has earned the one tool-free synthesis pass.
///
/// A tool-call budget bounds TOOL USE, not text: an agent cut at the fence
/// mid-sentence ("이제 정리하겠습니다" — observed in the field) had already
/// gathered everything it needed and only lacked the turn to write it up, yet
/// it surfaced as a `failed` lane with a partial-result banner. Granting one
/// wrap-up turn that spends no tool budget turns that into a completed
/// deliverable, honestly composed from what was actually gathered.
///
/// Narrow by design: only the `ToolCalls` kind (a deadline leaves no time to
/// write, an output-token cutoff leaves no tokens to write WITH, and an
/// iteration cutoff is the auto-continue path's job), only once, and only
/// when the JOB actually gathered something. The gathering test is
/// "any successful tool result, across every turn so far"
/// ([`AgentContinuationState::gathered_tool_result`]) — cumulative, because
/// an auto-continued turn can re-exhaust with zero tool results of its own
/// (a parallel batch larger than the re-armed remainder is dropped whole)
/// while the turns before it gathered plenty. And deliberately NOT
/// `agent_turn_made_progress`, whose edit-or-substantial-text bar measures
/// "worth continuing the work", the auto-continue question; a research agent
/// that spent its whole budget on reads has zero edit-progress and exactly
/// the gathered material a synthesis pass exists for.
fn agent_wrap_up_eligible(
    summary: &runtime::TurnSummary,
    wrap_up_used: bool,
    gathered_tool_result: bool,
) -> bool {
    !wrap_up_used
        && summary.budget_exhausted == Some(runtime::BudgetExhausted::ToolCalls)
        && gathered_tool_result
}

/// Tool budget for the wrap-up turn: a schema agent still owes its
/// `StructuredOutput` call (exactly one), a plain agent gets no tools at all
/// — any tool attempt re-exhausts the budget immediately and falls through
/// to the honest failed-with-partial path.
fn agent_wrap_up_tool_allowance(schema_requested: bool) -> usize {
    usize::from(schema_requested)
}

fn agent_budget_wrap_up_prompt(schema_requested: bool) -> String {
    let deliverable = if schema_requested {
        "call StructuredOutput exactly once with your best final result — no other tool calls"
    } else {
        "write the complete final deliverable as text — do NOT call any tools"
    };
    format!(
        "[zo:budget-wrap-up] The tool-call budget is exhausted; no further investigation is \
         possible. Using ONLY what you already gathered in this conversation, {deliverable}. \
         Include your findings, conclusions, and concrete specifics; mark anything you could \
         not verify as unverified instead of guessing."
    )
}

// Cohesive terminal-settling core: build runtime → run turn → settle one of
// three outcomes (error, budget-exhausted-with-work, completed). The budget arm
// mirrors the completed arm's persist + `SubagentStop`, so keeping them in one
// scope reads more clearly than a helper that would need the generic runtime
// type threaded through it.
#[allow(clippy::too_many_lines)]
fn run_agent_job(
    job: &AgentJob,
    token_history: &std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
    output_tokens_total: &std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> Result<AgentJobOutcome, runtime::RuntimeError> {
    let attendance = runtime::declared_attendance();
    let caps = SpawnedAgentCaps::for_spawn(
        attendance,
        job.manifest.subagent_type.as_deref().unwrap_or_default(),
    );
    let mut runtime = build_agent_runtime(job, token_history.clone(), output_tokens_total.clone())
        .map_err(runtime::RuntimeError::new)?
        .with_max_iterations(caps.iterations)
        .with_max_tool_calls(caps.tool_calls)
        .with_hook_abort_signal(job.cancel_signal.clone());
    // CC parity: hooks firing inside this sub-agent carry agent_id/agent_type.
    runtime.set_hook_agent_context(
        job.manifest.agent_id.clone(),
        job.manifest
            .subagent_type
            .clone()
            .unwrap_or_else(|| "general-purpose".to_string()),
    );
    // Stop(TurnEnd) is a main-agent contract; the `for_subagent()` hook view
    // already strips those rules — this is defense-in-depth so no future hook
    // source can re-loop a sub-agent's narrow task.
    runtime.set_max_stop_loops(0);
    runtime.fire_lifecycle_hook(
        runtime::HookEvent::SubagentStart,
        &subagent_hook_context(job, "running", None, None),
    );
    // Only explicit budgets are hard deadlines. Workflow phase agents pass
    // `None` because the user needs their real result instead of a synthetic
    // "agent exceeded its time budget" failure.
    if let Some(deadline) = agent_deadline(std::time::Instant::now(), job.time_budget) {
        runtime.set_deadline(deadline);
    }
    // Cost circuit breakers (output/input tokens), same env-driven defaults as
    // every other turn host. Sub-agents bill on their own runtimes, so without
    // these a parent could route unbounded generation — or a cache-dead
    // full-transcript re-bill loop — through its spawns and bypass its own
    // breakers entirely. The iteration/tool-call caps above bound *count*, not
    // token cost; a few iterations can each carry a six-figure token bill.
    // The net follows the parent's attendance like the caps above: nobody
    // types to a helper, but the person at the parent's keyboard watches it
    // and can stop it, and Claude Code puts no such net on a Task.
    let (_, output_budget, input_budget) = runtime::env_turn_budgets(attendance);
    runtime.set_turn_output_token_budget(output_budget);
    runtime.set_turn_input_token_budget(input_budget);
    // Debug mode: revert any `InstrumentLog` probes so a debugger sub-agent's
    // tracing never leaks into the working tree. The explicit call below covers
    // the normal success/returned-error paths; the guard - built from a clone of
    // the sink before the turn - covers the PANIC path, where an unwind inside
    // `run_turn` would otherwise skip the explicit call and drop the runtime
    // (and its sink) with markers still on disk. A no-op for agents that never
    // instrumented (the ledger is empty).
    let probe_guard = ProbeRevertGuard::new(runtime.tool_executor().probe_sink_handle());
    let mut next_prompt = job.prompt.clone();
    let schema_requested = job.schema.is_some();
    let allow_text_progress = subagent_allows_text_progress(
        job.manifest.subagent_type.as_deref().unwrap_or("general-purpose"),
        job.permission_mode,
    );
    let mut continuation = AgentContinuationState::for_caps(caps);
    let summary_result = loop {
        let result = runtime.run_turn(next_prompt, None);
        runtime.tool_executor().revert_probes();
        match result {
            Ok(summary) => {
                continuation.observe(&summary, job.schema.as_ref());
                let remaining_tool_calls = continuation.remaining_tool_calls();
                let token_budgets_have_headroom =
                    continuation.token_budgets_have_headroom(output_budget, input_budget);
                match agent_turn_step(
                    &summary,
                    &continuation,
                    job.schema.as_ref(),
                    remaining_tool_calls,
                    token_budgets_have_headroom,
                    allow_text_progress,
                ) {
                    AgentTurnStep::Continue(kind) => {
                        continuation.continuations_used += 1;
                        runtime.set_max_tool_calls(remaining_tool_calls);
                        runtime.set_turn_output_token_budget(
                            continuation.remaining_output_tokens(output_budget),
                        );
                        runtime.set_turn_input_token_budget(
                            continuation.remaining_input_tokens(input_budget),
                        );
                        next_prompt =
                            agent_budget_continuation_prompt(kind, continuation.continuations_used);
                    }
                    AgentTurnStep::WrapUp => {
                        continuation.wrap_up_used = true;
                        runtime.set_max_tool_calls(agent_wrap_up_tool_allowance(schema_requested));
                        runtime.set_turn_output_token_budget(
                            continuation.remaining_output_tokens(output_budget),
                        );
                        runtime.set_turn_input_token_budget(
                            continuation.remaining_input_tokens(input_budget),
                        );
                        next_prompt = agent_budget_wrap_up_prompt(schema_requested);
                    }
                    AgentTurnStep::Stop => break Ok(summary),
                }
            }
            Err(error) => break Err(error),
        }
    };
    // Free any write leases this agent acquired (track 4-2) so the paths it
    // edited are immediately available to the next sequential agent instead of
    // waiting out the lease TTL. No-op unless the guard was opt-in enabled.
    runtime.tool_executor().release_write_leases();
    drop(probe_guard);
    // Full-snapshot the conversation for `SendMessage` resume, on EVERY exit
    // (completed, budget-cut, returned error). `push_message`'s incremental
    // append misses direct message mutations (steer folds, compaction
    // rewrites), so the terminal snapshot is the ground truth a resume
    // rehydrates. Best-effort: a failed write only degrades resumability.
    if let Some(path) = job.transcript_path.as_ref() {
        let _ = runtime.session().save_to_path(path);
    }
    let summary = match summary_result {
        Ok(summary) => summary,
        Err(error) => {
            let provider_error_class = error.provider_error_class();
            let message = error.to_string();
            let status = if agent_error_is_cancelled(&message) {
                "stopped"
            } else {
                "failed"
            };
            if let Some(final_text) = continuation_error_result(&continuation, &message) {
                let structured = continuation.latest_structured.clone();
                let history_snapshot = token_history
                    .lock()
                    .map(|guard| guard.clone())
                    .unwrap_or_default();
                if let Err(persist_error) = persist_agent_terminal_state_with_history(
                    &job.manifest,
                    status,
                    Some(final_text.as_str()),
                    Some(message.clone()),
                    history_snapshot,
                ) {
                    runtime.fire_lifecycle_hook(
                        runtime::HookEvent::SubagentStop,
                        &subagent_hook_context(
                            job,
                            "failed",
                            None,
                            Some(persist_error.as_str()),
                        ),
                    );
                    return Err(runtime::RuntimeError::new(persist_error));
                }
                runtime.fire_lifecycle_hook(
                    runtime::HookEvent::SubagentStop,
                    &subagent_hook_context(
                        job,
                        status,
                        Some(final_text.as_str()),
                        Some(message.as_str()),
                    ),
                );
                return Ok(AgentJobOutcome {
                    final_text,
                    structured,
                    status,
                    error: Some(message),
                    provider_error_class,
                });
            }
            runtime.fire_lifecycle_hook(
                runtime::HookEvent::SubagentStop,
                &subagent_hook_context(job, status, None, Some(message.as_str())),
            );
            return Err(error);
        }
    };
    // Budget exhausted mid-task: the runtime now preserves the turn's work and
    // returns Ok with this marker instead of erroring, so a cut-off agent no
    // longer vaporizes everything it did. Surface the partial result and mark
    // the agent `failed` WITH that result (not a silent `None`) — the parent and
    // the transcript viewer both see what got done, and a follow-up can continue
    // or narrow the task. Persistence + `SubagentStop` mirror the completed path
    // below, differing only in the status and the budget-banner text.
    let structured = continuation.latest_structured.clone();
    if let AgentTerminalDisposition::BudgetFailed(kind) = agent_terminal_disposition(
        &summary,
        schema_requested,
        structured.is_some(),
        continuation.wrap_up_used,
    ) {
        let final_text = agent_budget_exhausted_result(
            &continuation.latest_budget_text,
            kind,
            continuation.cumulative_iterations,
            continuation.cumulative_tool_calls,
        );
        let budget_error = format!(
            "budget exhausted: {} — partial result preserved",
            budget_exhausted_kind_label(kind)
        );
        let history_snapshot = token_history
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        if let Err(error) = persist_agent_terminal_state_with_history(
            &job.manifest,
            "failed",
            Some(final_text.as_str()),
            Some(budget_error.clone()),
            history_snapshot,
        ) {
            runtime.fire_lifecycle_hook(
                runtime::HookEvent::SubagentStop,
                &subagent_hook_context(job, "failed", None, Some(error.as_str())),
            );
            return Err(runtime::RuntimeError::new(error));
        }
        runtime.fire_lifecycle_hook(
            runtime::HookEvent::SubagentStop,
            &subagent_hook_context(job, "failed", Some(final_text.as_str()), None),
        );
        return Ok(AgentJobOutcome {
            final_text,
            structured,
            status: "failed",
            error: Some(budget_error),
            provider_error_class: None,
        });
    }
    // Capture the `StructuredOutput` tool call's input only when a schema was
    // requested (8c); otherwise this is a plain free-text agent. Resolved BEFORE
    // the diagnostic fallback so an empty final text with a structured result is
    // reported as "returned via StructuredOutput" rather than a misleading
    // "no final response" warning.
    let structured = continuation.latest_structured;
    // A completed continuation can legitimately end through StructuredOutput
    // or with only a final tool result. Preserve the latest substantive text
    // from an earlier budget window instead of replacing it with a diagnostic.
    let current_final_text = final_assistant_text(&summary);
    let final_text_source = if current_final_text.trim().is_empty() {
        continuation.latest_budget_text
    } else {
        current_final_text
    };
    let final_text = agent_result_or_diagnostic(
        &final_text_source,
        structured.is_some(),
        continuation.cumulative_iterations,
        continuation.cumulative_tool_calls,
    );
    let history_snapshot = token_history
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    if let Err(error) = persist_agent_terminal_state_with_history(
        &job.manifest,
        "completed",
        Some(final_text.as_str()),
        None,
        history_snapshot,
    ) {
        runtime.fire_lifecycle_hook(
            runtime::HookEvent::SubagentStop,
            &subagent_hook_context(job, "failed", None, Some(error.as_str())),
        );
        return Err(runtime::RuntimeError::new(error));
    }
    runtime.fire_lifecycle_hook(
        runtime::HookEvent::SubagentStop,
        &subagent_hook_context(job, "completed", Some(final_text.as_str()), None),
    );
    Ok(AgentJobOutcome {
        final_text,
        structured,
        status: "completed",
        error: None,
        provider_error_class: None,
    })
}

/// The caller-facing result text for a sub-agent turn that ran to completion.
///
/// Normally this is the turn's final assistant text ([`final_assistant_text`]).
/// A turn can, however, complete with no final text at all — the model's last
/// message was tool calls only, or it exhausted its iteration / tool-call budget
/// mid-task. A raw extract is then an empty string, which both the in-memory
/// completion surfaced to the spawn caller and the persisted `## Result` block
/// render as a silent blank (no "Final response" section). To keep a completion
/// from ever being a silent empty, fall back to a short diagnostic derived from
/// the turn signals so the caller can always tell what happened and how to
/// recover.
///
/// When `has_structured` is true the agent returned its result through the
/// `StructuredOutput` tool rather than a final text message — that is a normal,
/// successful completion, so we surface a short pointer to the structured
/// payload instead of a "no final response" warning. Pure and independent of the
/// summary type, so it is exhaustively unit-tested.
fn agent_result_or_diagnostic(
    final_text: &str,
    has_structured: bool,
    iterations: usize,
    tool_calls: usize,
) -> String {
    if !final_text.trim().is_empty() {
        return final_text.to_string();
    }
    if has_structured {
        return "(results returned via StructuredOutput tool)".to_string();
    }
    // A successful completion with no final text was not stopped by a budget
    // error, so we do NOT claim a budget cutoff or tell the caller to "re-run
    // with a higher budget" — the turn simply ended on a tool call.
    format!(
        "[no final response] The sub-agent completed {iterations} iteration(s) and \
         {tool_calls} tool call(s) without producing a final text response — it likely \
         ended on a tool call. Re-run with a narrower task."
    )
}

/// The last non-empty assistant TEXT the agent produced before the budget cut
/// it off — the partial work worth surfacing to the parent.
///
/// On a budget-exhausted turn the runtime deterministically appends its
/// synthetic `[budget] …` closer as the LAST assistant message (every budget
/// arm pushes it, and a failed push errors the turn out of the budget path
/// entirely), so `final_assistant_text` would return that notice — duplicating
/// the banner [`agent_budget_exhausted_result`] already prepends and hiding the
/// agent's real narration. Skip the trailing closer structurally (drop the last
/// message, no string matching) and reverse-scan the rest for text; a turn that
/// only ever issued tool calls yields the empty string, which the banner
/// builder turns into the transcript-pointer note.
fn last_substantive_assistant_text(messages: &[runtime::session::ConversationMessage]) -> String {
    let before_closer = &messages[..messages.len().saturating_sub(1)];
    before_closer
        .iter()
        .rev()
        .map(|message| {
            message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    runtime::session::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .find(|text| !text.trim().is_empty())
        .unwrap_or_default()
}

/// Short, lowercase label for a budget-exhaustion kind, used in the caller-
/// facing banner.
fn budget_exhausted_kind_label(kind: runtime::BudgetExhausted) -> &'static str {
    match kind {
        runtime::BudgetExhausted::Iterations => "iteration budget",
        runtime::BudgetExhausted::Deadline => "time budget",
        runtime::BudgetExhausted::ToolCalls => "tool-call budget",
        runtime::BudgetExhausted::OutputTokens => "output-token budget",
        runtime::BudgetExhausted::InputTokens => "input-token budget",
        runtime::BudgetExhausted::VerificationTreadmill => "verification loop",
        runtime::BudgetExhausted::ToolRepetition => "tool-repetition guard",
    }
}

/// The caller-facing result for a sub-agent turn that stopped on an exhausted
/// budget (iteration cap, wall-clock deadline, or tool-call budget). Unlike
/// [`agent_result_or_diagnostic`], the turn's work IS preserved, so this leads
/// with a one-line budget banner and then the agent's final assistant text — or,
/// when the agent ended mid-task with no final prose, a note pointing at the
/// preserved transcript plus a narrow-retry hint. Pure and independent of the
/// summary type, so it is exhaustively unit-tested.
fn agent_budget_exhausted_result(
    final_text: &str,
    kind: runtime::BudgetExhausted,
    iterations: usize,
    tool_calls: usize,
) -> String {
    let banner = format!(
        "[budget exhausted: {} after {iterations} iteration(s), {tool_calls} tool call(s)]",
        budget_exhausted_kind_label(kind)
    );
    if final_text.trim().is_empty() {
        format!(
            "{banner}\nNo final text; partial work is recorded in the agent transcript. \
             Continue in a follow-up turn or re-run with a narrower task."
        )
    } else {
        format!("{banner}\n{final_text}")
    }
}

fn agent_error_is_cancelled(error: &str) -> bool {
    error.to_ascii_lowercase().contains("agent cancelled")
}

fn agent_deadline(
    now: std::time::Instant,
    time_budget: Option<Duration>,
) -> Option<std::time::Instant> {
    time_budget.map(|budget| now + budget)
}

#[cfg(test)]
mod caps_tests {
    use super::{
        SpawnedAgentCaps, EXPLORE_MAX_ITERATIONS, SPAWNED_AGENT_MAX_ITERATIONS,
        SPAWNED_AGENT_MAX_TOOL_CALLS,
    };

    /// Who can stop the helper decides its caps: none under a person, the
    /// documented ones under nobody.
    #[test]
    fn a_helper_is_capped_only_when_nobody_can_stop_it() {
        let attended = SpawnedAgentCaps::for_attendance(runtime::Attendance::Attended);
        assert_eq!(
            (attended.iterations, attended.tool_calls),
            (usize::MAX, usize::MAX)
        );
        let unattended = SpawnedAgentCaps::for_attendance(runtime::Attendance::Unattended);
        assert_eq!(
            (unattended.iterations, unattended.tool_calls),
            (SPAWNED_AGENT_MAX_ITERATIONS, SPAWNED_AGENT_MAX_TOOL_CALLS)
        );
        // Never declared, a process reads as unattended — the safe default
        // every existing spawn test runs under.
        assert_eq!(runtime::declared_attendance(), runtime::Attendance::Unattended);
    }

    /// A read-only Explore helper runs under half the iteration cap when
    /// nobody can stop it; every other profile, and every attended spawn,
    /// keeps the documented caps.
    #[test]
    fn an_unattended_explore_runs_under_half_the_iteration_cap() {
        let explore = SpawnedAgentCaps::for_spawn(runtime::Attendance::Unattended, "Explore");
        assert_eq!(explore.iterations, EXPLORE_MAX_ITERATIONS);
        assert_eq!(explore.tool_calls, SPAWNED_AGENT_MAX_TOOL_CALLS);
        let general =
            SpawnedAgentCaps::for_spawn(runtime::Attendance::Unattended, "general-purpose");
        assert_eq!(general.iterations, SPAWNED_AGENT_MAX_ITERATIONS);
        let attended = SpawnedAgentCaps::for_spawn(runtime::Attendance::Attended, "Explore");
        assert_eq!(attended.iterations, usize::MAX);
    }
}

#[cfg(test)]
mod tests {
    use core_types::helper_run::HelperRun;
    use super::{
        AgentContinuationState, AgentJob, AgentOutput, AgentTerminalDisposition, AgentTurnStep,
        SPAWNED_AGENT_MAX_BUDGET_CONTINUATIONS, SPAWNED_AGENT_MAX_ITERATIONS,
        SPAWNED_AGENT_MAX_TOOL_CALLS, agent_budget_continuation_prompt,
        agent_budget_exhausted_result, agent_budget_wrap_up_prompt, agent_deadline,
        agent_result_or_diagnostic, agent_terminal_disposition, agent_turn_made_progress,
        agent_turn_step,
        agent_wrap_up_eligible, agent_wrap_up_tool_allowance,
        completion_structured_with_provider_error_class, continuation_error_result,
        continuation_text_is_meaningful, is_ad_hoc_turn_review,
        persist_agent_terminal_state_with_history, reconcile_completion_terminal_status,
        record_agent_verdict_outcome, route_outcome_record, should_auto_continue_agent,
        should_auto_continue_budget, subagent_allows_text_progress, verdict_passed,
    };
    use super::super::{final_structured_output, completion::AgentCompletion};
    use super::super::auto_verify as av;
    use crate::misc_tools::agent_tools::AgentActivityTelemetry;
    use core_types::usage::TokenUsage;
    use runtime::session::{ContentBlock, ConversationMessage, MessageRole};
    use std::time::Duration;

    fn test_message(role: MessageRole, blocks: Vec<ContentBlock>) -> ConversationMessage {
        ConversationMessage {
            role,
            blocks,
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        }
    }

    fn continuation_summary(
        assistant_messages: Vec<ConversationMessage>,
        tool_results: Vec<ConversationMessage>,
        budget_exhausted: Option<runtime::BudgetExhausted>,
    ) -> runtime::TurnSummary {
        runtime::TurnSummary {
            assistant_messages,
            tool_results,
            prompt_cache_events: Vec::new(),
            iterations: SPAWNED_AGENT_MAX_ITERATIONS + 1,
            usage: TokenUsage::default(),
            turn_output_tokens: 0,
            auto_compaction: None,
            microcompact: None,
            deep_verification: None,
            verification_issues: Vec::new(),
            deep_verifier_parse: None,
            deep_verifier_model: None,
            budget_exhausted,
        }
    }

    fn successful_tool_result(tool_name: &str) -> ConversationMessage {
        test_message(
            MessageRole::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: format!("{tool_name}-id"),
                tool_name: tool_name.to_string(),
                output: "ok".to_string(),
                is_error: false,
                images: Vec::new(),
            }],
        )
    }

    /// A minimal `completed` manifest backed by real temp files, so a
    /// persistence path test can read back exactly what a spawn caller and the
    /// `## Result` block would see.
    /// The two numbers the worker thread cannot know come from the manifest,
    /// and a run still missing its end is measured against now rather than
    /// reported as instant.
    #[test]
    fn a_run_is_measured_from_the_manifest_the_worker_wrote() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut manifest = empty_result_manifest(temp.path(), "agent-1");
        manifest.tool_calls = 12;
        manifest.started_at = Some("100".to_string());
        manifest.completed_at = Some("180".to_string());

        let run = super::run_from_manifest(&manifest, 45_300);
        assert_eq!(run.tool_calls, 12);
        assert_eq!(run.output_tokens, 45_300);
        assert_eq!(run.elapsed, Some(Duration::from_secs(80)));
        assert_eq!(
            run.summary().as_deref(),
            Some("12 tool uses · 45.3k tokens · 1m 20s")
        );

        manifest.started_at = None;
        assert!(super::run_from_manifest(&manifest, 0).elapsed.is_none());
    }

    fn empty_result_manifest(dir: &std::path::Path, id: &str) -> AgentOutput {
        let manifest = AgentOutput {
            route_probe_confidence: None,
            agent_id: id.to_string(),
            parent_session_id: None,
            tool_call_id: None,
            name: id.to_string(),
            label: None,
            description: "agent".to_string(),
            subagent_type: Some("Explore".to_string()),
            requested_model: None,
            resolved_model: None,
            route_reason: None,
            route_role: None,
            route_complexity: None,
            route_risk: None,
            route_source: None,
        sees: Vec::new(),
            model: None,
            status: "running".to_string(),
            output_file: dir.join(format!("{id}.md")).display().to_string(),
            manifest_file: dir.join(format!("{id}.json")).display().to_string(),
            created_at: "100".to_string(),
            pane: None,
            owner_pid: None,
            run_generation: 0,
            started_at: Some("100".to_string()),
            completed_at: None,
            completion_published_at: None,
            lane_events: Vec::new(),
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
            activity: AgentActivityTelemetry::default(),
            lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
        };
        super::super::manifest::write_agent_manifest(&manifest).expect("write manifest");
        std::fs::write(&manifest.output_file, "# Agent\n").expect("seed output file");
        manifest
    }

    fn unique_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zo-{label}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// Path-level: an empty-final-text turn WITH a structured result persists the
    /// `StructuredOutput` pointer as the completion result (never a "no final
    /// response" warning), mirroring `run_agent_job`'s exact ordering — compute
    /// `structured`, then derive `final_text`, then persist.
    #[test]
    fn empty_result_with_structured_output_persists_structured_pointer() {
        let dir = unique_dir("spawn-empty-structured");
        let manifest = empty_result_manifest(&dir, "structured");

        // run_agent_job resolves `structured` before the diagnostic fallback.
        let has_structured = true;
        let final_text = agent_result_or_diagnostic("", has_structured, 3, 4);
        assert_eq!(final_text, "(results returned via StructuredOutput tool)");

        persist_agent_terminal_state_with_history(
            &manifest,
            "completed",
            Some(final_text.as_str()),
            None,
            Vec::new(),
        )
        .expect("persist completed state");

        let reread: AgentOutput = serde_json::from_str(
            &std::fs::read_to_string(&manifest.manifest_file).expect("reread manifest"),
        )
        .expect("parse manifest");
        assert_eq!(reread.status, "completed");
        let output = std::fs::read_to_string(&manifest.output_file).expect("read output");
        assert!(
            output.contains("(results returned via StructuredOutput tool)"),
            "the persisted Final response points at the structured payload: {output}",
        );
        assert!(
            !output.contains("[no final response]"),
            "a structured completion must not persist a warning: {output}",
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Path-level: an empty-final-text turn WITHOUT a structured result persists
    /// the simplified diagnostic — no misleading budget wording — as a real
    /// `## Result` block so the spawn caller never sees a silent blank.
    #[test]
    fn empty_result_without_structured_persists_simplified_diagnostic() {
        let dir = unique_dir("spawn-empty-blank");
        let manifest = empty_result_manifest(&dir, "blank");

        let has_structured = false;
        let final_text = agent_result_or_diagnostic("", has_structured, 5, 9);
        assert!(final_text.contains("[no final response]"), "{final_text}");
        assert!(!final_text.contains("budget"), "{final_text}");

        persist_agent_terminal_state_with_history(
            &manifest,
            "completed",
            Some(final_text.as_str()),
            None,
            Vec::new(),
        )
        .expect("persist completed state");

        let output = std::fs::read_to_string(&manifest.output_file).expect("read output");
        assert!(
            output.contains("### Final response") && output.contains("[no final response]"),
            "the empty turn still persists a non-blank Final response: {output}",
        );
        assert!(
            !output.contains("higher budget") && !output.contains("after reaching its"),
            "the persisted diagnostic must not claim a budget cutoff: {output}",
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn agent_result_passes_through_a_real_final_response() {
        assert_eq!(
            agent_result_or_diagnostic("the answer is 42", false, 3, 5),
            "the answer is 42",
        );
    }

    /// Pure: a budget-exhausted turn WITH a final assistant text leads with the
    /// budget banner (kind + counts) and then the preserved text.
    #[test]
    fn agent_budget_exhausted_result_leads_with_banner_then_final_text() {
        let out = agent_budget_exhausted_result(
            "partial progress: edited foo.rs",
            runtime::BudgetExhausted::Iterations,
            64,
            120,
        );
        assert!(
            out.starts_with(
                "[budget exhausted: iteration budget after 64 iteration(s), 120 tool call(s)]"
            ),
            "{out}"
        );
        assert!(out.contains("partial progress: edited foo.rs"), "{out}");
    }

    /// Pure: with no final text, the banner is followed by a transcript pointer
    /// and a narrow-retry hint — never a silent blank.
    #[test]
    fn agent_budget_exhausted_result_points_at_transcript_when_no_final_text() {
        let out =
            agent_budget_exhausted_result("   ", runtime::BudgetExhausted::ToolCalls, 10, 256);
        assert!(
            out.starts_with(
                "[budget exhausted: tool-call budget after 10 iteration(s), 256 tool call(s)]"
            ),
            "{out}"
        );
        assert!(
            out.contains("partial work is recorded in the agent transcript"),
            "{out}"
        );
        assert!(out.contains("narrower task"), "{out}");
    }

    /// Pure: the deadline kind is labelled "time budget".
    #[test]
    fn agent_budget_exhausted_result_labels_the_deadline_kind() {
        let out = agent_budget_exhausted_result("x", runtime::BudgetExhausted::Deadline, 1, 1);
        assert!(out.contains("time budget"), "{out}");
    }

    /// Pure: the partial-work extractor must skip the runtime's synthetic
    /// trailing `[budget] …` closer (always the LAST assistant message on a
    /// budget-exhausted turn) and return the last REAL narration — feeding
    /// `final_assistant_text` here would surface the closer itself, duplicating
    /// the banner and hiding the work. A tool-use-only remainder yields the
    /// empty string, which the banner builder turns into the transcript pointer.
    #[test]
    fn last_substantive_assistant_text_skips_the_synthetic_closer() {
        use runtime::session::{ContentBlock, ConversationMessage};
        let text = |s: &str| {
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: s.to_string(),
            }])
        };
        let closer = text("[budget] Iteration budget exhausted after 64 iteration(s); …");

        // Real narration before the closer is surfaced.
        let messages = vec![text("did A; next is B"), closer.clone()];
        assert_eq!(super::last_substantive_assistant_text(&messages), "did A; next is B");

        // Empty/whitespace narration is skipped in the reverse scan.
        let messages = vec![text("real work note"), text("   "), closer.clone()];
        assert_eq!(super::last_substantive_assistant_text(&messages), "real work note");

        // Only the closer (no prior text) → empty → transcript-pointer branch.
        assert_eq!(super::last_substantive_assistant_text(&[closer]), "");
        assert_eq!(super::last_substantive_assistant_text(&[]), "");
    }

    /// Path-level: a budget-exhausted turn persists status `failed` WITH the
    /// partial-work result (not a silent `None`), so the parent and the `##
    /// Result` block both see what got done. Mirrors `run_agent_job`'s budget
    /// branch: build the partial text, then persist it as a failed terminal.
    #[test]
    fn budget_exhausted_persists_failed_status_with_partial_result() {
        let dir = unique_dir("spawn-budget-exhausted");
        let manifest = empty_result_manifest(&dir, "budget");

        let final_text = agent_budget_exhausted_result(
            "did some work",
            runtime::BudgetExhausted::Iterations,
            SPAWNED_AGENT_MAX_ITERATIONS,
            3,
        );
        persist_agent_terminal_state_with_history(
            &manifest,
            "failed",
            Some(final_text.as_str()),
            None,
            Vec::new(),
        )
        .expect("persist failed-with-partial state");

        let reread: AgentOutput = serde_json::from_str(
            &std::fs::read_to_string(&manifest.manifest_file).expect("reread manifest"),
        )
        .expect("parse manifest");
        assert_eq!(reread.status, "failed");
        let output = std::fs::read_to_string(&manifest.output_file).expect("read output");
        assert!(
            output.contains("[budget exhausted:") && output.contains("did some work"),
            "a budget cutoff must persist the partial work, not a blank: {output}",
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn agent_result_passes_through_final_text_even_when_structured_present() {
        // A real final text always wins, structured or not.
        assert_eq!(
            agent_result_or_diagnostic("the answer is 42", true, 3, 5),
            "the answer is 42",
        );
    }

    #[test]
    fn agent_result_reports_structured_output_for_blank_text() {
        // Empty final text but a StructuredOutput result: a normal, successful
        // completion — surface a pointer to the structured payload, NOT a warning.
        let out = agent_result_or_diagnostic("   \n  ", true, 4, 7);
        assert_eq!(out, "(results returned via StructuredOutput tool)", "{out}");
        assert!(!out.contains("[no final response]"), "{out}");
    }

    #[test]
    fn agent_result_diagnoses_a_blank_response_without_structured() {
        let out = agent_result_or_diagnostic("   \n  ", false, 4, 7);
        assert!(out.contains("[no final response]"), "{out}");
        assert!(out.contains("4 iteration(s)"), "{out}");
        assert!(out.contains("7 tool call(s)"), "{out}");
    }

    #[test]
    fn agent_result_empty_diagnostic_omits_budget_wording() {
        // A successful completion was not stopped by a budget error, so the
        // diagnostic must not claim a budget cutoff or tell the caller to raise
        // the budget — even at the iteration / tool-call ceilings.
        for out in [
            agent_result_or_diagnostic("", false, SPAWNED_AGENT_MAX_ITERATIONS, 3),
            agent_result_or_diagnostic("", false, 2, SPAWNED_AGENT_MAX_TOOL_CALLS),
        ] {
            assert!(out.contains("[no final response]"), "{out}");
            assert!(!out.contains("budget"), "must not mention a budget: {out}");
            assert!(
                !out.contains("after reaching its"),
                "must not claim a budget cutoff: {out}",
            );
            assert!(
                !out.contains("higher budget"),
                "must not advise raising the budget: {out}",
            );
        }
    }

    #[test]
    fn spawned_agents_default_to_no_wall_clock_deadline() {
        assert!(
            agent_deadline(std::time::Instant::now(), None).is_none(),
            "omitted budgets should wait for the actual agent result"
        );
    }

    #[test]
    fn spawned_agents_honor_explicit_wall_clock_budget() {
        let now = std::time::Instant::now();
        let deadline = agent_deadline(now, Some(Duration::from_secs(20 * 60)))
            .expect("explicit budget should set a deadline");
        assert_eq!(deadline.duration_since(now), Duration::from_secs(20 * 60));
    }

    #[test]
    fn spawned_agents_are_bounded_to_stop_runaways() {
        // A finite, modest cap turns a runaway sub-agent (the 174-tool-call
        // fan-out of 2026-06-07) into a graceful `failed` completion instead of
        // unbounded thrash. The enforcement + failure path are proven in
        // `runtime::conversation::tests::run_turn_errors_when_max_iterations_is_exceeded`.
        assert_eq!(
            SPAWNED_AGENT_MAX_ITERATIONS, 64,
            "spawned sub-agents must keep a finite iteration cap; usize::MAX reintroduces the runaway"
        );
    }

    #[test]
    fn spawned_agent_progress_requires_mutation_or_read_only_deliverable() {
        let read_only = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("read_file")],
            Some(runtime::BudgetExhausted::Iterations),
        );
        assert!(!agent_turn_made_progress(&read_only, false));
        assert!(!agent_turn_made_progress(&read_only, true));

        let mutation = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("edit_file")],
            Some(runtime::BudgetExhausted::Iterations),
        );
        assert!(agent_turn_made_progress(&mutation, false));

        let report = "substantive finding ".repeat(20);
        assert!(continuation_text_is_meaningful(&report));
        let filler_without_evidence = continuation_summary(
            vec![
                test_message(
                    MessageRole::Assistant,
                    vec![ContentBlock::Text {
                        text: report.clone(),
                    }],
                ),
                test_message(
                    MessageRole::Assistant,
                    vec![ContentBlock::Text {
                        text: "[budget] continuation required".to_string(),
                    }],
                ),
            ],
            Vec::new(),
            Some(runtime::BudgetExhausted::Iterations),
        );
        assert!(!agent_turn_made_progress(&filler_without_evidence, true));
        let read_only_report = continuation_summary(
            vec![
                test_message(
                    MessageRole::Assistant,
                    vec![ContentBlock::Text { text: report }],
                ),
                test_message(
                    MessageRole::Assistant,
                    vec![ContentBlock::Text {
                        text: "[budget] continuation required".to_string(),
                    }],
                ),
            ],
            vec![successful_tool_result("read_file")],
            Some(runtime::BudgetExhausted::Iterations),
        );
        assert!(!agent_turn_made_progress(&read_only_report, false));
        assert!(agent_turn_made_progress(&read_only_report, true));
    }

    #[test]
    fn spawned_agent_schema_continues_until_structured_deliverable() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["verdict"],
            "properties": { "verdict": { "type": "string" } },
            "additionalProperties": false
        });
        let mutation = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("edit_file")],
            Some(runtime::BudgetExhausted::Iterations),
        );
        assert!(should_auto_continue_agent(
            &mutation,
            Some(&schema),
            0,
            SPAWNED_AGENT_MAX_TOOL_CALLS,
            true,
            false,
        ));

        let structured = continuation_summary(
            vec![test_message(
                MessageRole::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "structured".to_string(),
                    name: "StructuredOutput".to_string(),
                    input: serde_json::json!({"verdict": "pass"}).to_string(),
                }],
            )],
            vec![test_message(
                MessageRole::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "structured".to_string(),
                    tool_name: "StructuredOutput".to_string(),
                    output: "ok".to_string(),
                    is_error: false,
                    images: Vec::new(),
                }],
            )],
            Some(runtime::BudgetExhausted::Iterations),
        );
        assert!(!should_auto_continue_agent(
            &structured,
            Some(&schema),
            0,
            SPAWNED_AGENT_MAX_TOOL_CALLS,
            true,
            false,
        ));
        assert_eq!(
            agent_terminal_disposition(&structured, true, true, false),
            AgentTerminalDisposition::Completed,
            "a schema agent that submitted StructuredOutput at the iteration boundary is complete",
        );
        assert_eq!(
            agent_terminal_disposition(&structured, false, true, false),
            AgentTerminalDisposition::BudgetFailed(runtime::BudgetExhausted::Iterations),
            "non-schema agents must not turn an iteration cutoff into success",
        );

        let mut tool_cut = structured.clone();
        tool_cut.budget_exhausted = Some(runtime::BudgetExhausted::ToolCalls);
        assert_eq!(
            agent_terminal_disposition(&tool_cut, true, true, true),
            AgentTerminalDisposition::Completed,
            "a submission in hand survives a tool-call cutoff too — both fences bound WORK, \
             not the resources a deliverable must never bypass",
        );

        let mut hard_stop = structured;
        hard_stop.budget_exhausted = Some(runtime::BudgetExhausted::InputTokens);
        assert_eq!(
            agent_terminal_disposition(&hard_stop, true, true, false),
            AgentTerminalDisposition::BudgetFailed(runtime::BudgetExhausted::InputTokens),
            "StructuredOutput must not bypass a hard token budget",
        );
    }

    /// **wrap-up 산출물 게이트 회귀 핀** — 종합 턴이 아무것도 생산하지 않고
    /// 종료만 하면(최종 텍스트 없음·schema 제출 없음) `completed`로 확정되어
    /// 옛 mid-sentence partial이 배너 없이 성공 결과물로 둔갑하고 라우터
    /// 학습까지 오염시켰다. 무산출 wrap-up은 그것이 구제하려던 예산 실패로
    /// 정직하게 낙하해야 한다.
    #[test]
    fn a_wrap_up_that_produces_nothing_falls_back_to_the_honest_budget_failure() {
        // The wrap-up turn terminated without a budget marker but wrote no
        // final text (its last message ended on nothing substantive).
        let starved = continuation_summary(
            vec![test_message(
                MessageRole::Assistant,
                vec![ContentBlock::Text { text: "   ".to_string() }],
            )],
            Vec::new(),
            None,
        );
        assert_eq!(
            agent_terminal_disposition(&starved, false, false, true),
            AgentTerminalDisposition::BudgetFailed(runtime::BudgetExhausted::ToolCalls),
            "an empty wrap-up must not turn a budget failure into a bare completed",
        );
        // A schema agent whose wrap-up DID deliver its submission passes the
        // gate even with no final text (the eligibility guard means a
        // wrap-up only ever runs when no submission existed before it).
        assert_eq!(
            agent_terminal_disposition(&starved, true, true, true),
            AgentTerminalDisposition::Completed,
            "a wrap-up that delivered the StructuredOutput submission is a real completion",
        );
        // A wrap-up that wrote a real deliverable text completes.
        let delivered = continuation_summary(
            vec![test_message(
                MessageRole::Assistant,
                vec![ContentBlock::Text {
                    text: "종합: 수집한 근거 3건 기준 결론은 다음과 같다 …".to_string(),
                }],
            )],
            Vec::new(),
            None,
        );
        assert_eq!(
            agent_terminal_disposition(&delivered, false, false, true),
            AgentTerminalDisposition::Completed,
            "a wrap-up that actually wrote the deliverable completes",
        );
        // The gate is wrap-up-scoped: an ordinary (non-wrap-up) turn that
        // ends on a tool call with no final text keeps its long-standing
        // completed-with-diagnostic contract.
        assert_eq!(
            agent_terminal_disposition(&starved, false, false, false),
            AgentTerminalDisposition::Completed,
            "the deliverable gate must not reclassify ordinary tool-call-final turns",
        );
    }

    /// **툴콜 예산 하드컷 회귀 핀** — deep-research 에이전트가 255 툴콜을
    /// 쓰고 "이제 정리하겠습니다"에서 잘려 failed로 집계됐다(실측). 툴콜
    /// 예산은 툴만 제한하므로, 수집이 있었던 `ToolCalls` 소진은 도구 없는
    /// 종합 턴 1회를 받는다 — 단 한 번, `ToolCalls`에만. 자격 기준은 잡
    /// 누적의 "성공한 툴 결과 존재"다: 단일 턴 기준이면 auto-continue 직후
    /// 병렬 배치가 통째로 드롭된 재소진 턴(자기 툴 결과 0)이 앞 턴들의
    /// 수백 건 수집을 들고도 자격을 잃는다.
    #[test]
    fn wrap_up_fires_once_for_tool_call_exhaustion_with_progress() {
        let exhausted = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("read_file")],
            Some(runtime::BudgetExhausted::ToolCalls),
        );
        // The gathered flag is the CUMULATIVE one `observe` maintains.
        let mut gathered = AgentContinuationState::default();
        gathered.observe(&exhausted, None);
        assert!(gathered.gathered_tool_result, "observe must latch the successful result");
        assert!(
            agent_wrap_up_eligible(&exhausted, false, gathered.gathered_tool_result),
            "read-only gathering is exactly what the synthesis turn is for"
        );
        assert!(
            !agent_wrap_up_eligible(&exhausted, true, true),
            "the wrap-up is granted exactly once"
        );

        // Re-exhaustion with an empty result set of its own (parallel batch
        // dropped whole) stays eligible on the strength of EARLIER turns.
        let re_exhausted_empty = continuation_summary(
            Vec::new(),
            Vec::new(),
            Some(runtime::BudgetExhausted::ToolCalls),
        );
        gathered.observe(&re_exhausted_empty, None);
        assert!(
            agent_wrap_up_eligible(&re_exhausted_empty, false, gathered.gathered_tool_result),
            "a batch-dropped re-exhaustion must not forfeit hundreds of earlier gathers"
        );

        let iterations = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("read_file")],
            Some(runtime::BudgetExhausted::Iterations),
        );
        assert!(
            !agent_wrap_up_eligible(&iterations, false, true),
            "iteration cutoffs belong to the auto-continue path, not the wrap-up"
        );

        assert!(
            !agent_wrap_up_eligible(&re_exhausted_empty, false, false),
            "nothing gathered across the whole job means nothing to synthesize"
        );

        let natural_end = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("read_file")],
            None,
        );
        assert!(
            !agent_wrap_up_eligible(&natural_end, false, true),
            "a naturally completed turn needs no wrap-up"
        );
    }

    /// The wrap-up's tool allowance: zero for plain agents (any tool attempt
    /// re-exhausts immediately and falls through to the honest
    /// failed-with-partial path), exactly one for schema agents — they still
    /// owe their `StructuredOutput` submission.
    #[test]
    fn wrap_up_tool_allowance_reserves_structured_output_for_schema_agents() {
        assert_eq!(agent_wrap_up_tool_allowance(false), 0);
        assert_eq!(agent_wrap_up_tool_allowance(true), 1);
        assert!(
            agent_budget_wrap_up_prompt(false).contains("do NOT call any tools"),
            "plain wrap-up must forbid tools outright"
        );
        assert!(
            agent_budget_wrap_up_prompt(true).contains("StructuredOutput exactly once"),
            "schema wrap-up must reserve the deliverable call"
        );
    }

    #[test]
    fn spawned_agent_completion_respects_durable_stopped_status() {
        let rich_result = serde_json::json!({"verdict": "pass"});
        let mut completion = AgentCompletion {
            agent_id: "agent-status-race".to_string(),
            name: "status-race".to_string(),
            status: "completed".to_string(),
            result: Some("recovered rich result".to_string()),
            structured: Some(rich_result.clone()),
            error: None,
            run: HelperRun { output_tokens: 42, ..HelperRun::default() },
        };

        reconcile_completion_terminal_status(&mut completion, "stopped", None);

        assert_eq!(completion.status, "stopped");
        assert_eq!(completion.result.as_deref(), Some("recovered rich result"));
        assert_eq!(completion.structured, Some(rich_result));
        assert_eq!(completion.error.as_deref(), Some("agent stopped"));
        assert_eq!(completion.run.output_tokens, 42);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // cohesive acceptance/rejection contract matrix
    fn spawned_agent_schema_requires_unique_successful_valid_structured_output() {
        fn boundary(
            assistant: Vec<ContentBlock>,
            results: Vec<ContentBlock>,
        ) -> runtime::TurnSummary {
            continuation_summary(
                vec![test_message(MessageRole::Assistant, assistant)],
                vec![test_message(MessageRole::Tool, results)],
                Some(runtime::BudgetExhausted::Iterations),
            )
        }
        let schema = serde_json::json!({
            "type": "object",
            "required": ["verdict"],
            "properties": { "verdict": { "type": "string" } },
            "additionalProperties": false
        });
        let use_block = |id: &str, input: serde_json::Value| ContentBlock::ToolUse {
            id: id.to_string(),
            name: "StructuredOutput".to_string(),
            input: input.to_string(),
        };
        let result = |id: &str, name: &str, is_error| ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            tool_name: name.to_string(),
            output: "ok".to_string(),
            is_error,
            images: Vec::new(),
        };
        let progress = || result("progress", "edit_file", false);
        let continues = |summary: &runtime::TurnSummary, schema: &serde_json::Value| {
            should_auto_continue_agent(
                summary,
                Some(schema),
                0,
                SPAWNED_AGENT_MAX_TOOL_CALLS,
                true,
                false,
            )
        };
        let rejects = |summary: &runtime::TurnSummary, schema: &serde_json::Value| {
            assert!(final_structured_output(summary, schema).is_none());
            assert_eq!(
                agent_terminal_disposition(summary, true, false, false),
                AgentTerminalDisposition::BudgetFailed(runtime::BudgetExhausted::Iterations),
            );
        };

        let valid = boundary(
            vec![use_block("valid", serde_json::json!({"verdict": "pass"}))],
            vec![result("valid", "StructuredOutput", false)],
        );
        assert_eq!(
            final_structured_output(&valid, &schema),
            Some(serde_json::json!({"verdict": "pass"}))
        );
        assert!(!continues(&valid, &schema));
        assert_eq!(
            agent_terminal_disposition(&valid, true, true, false),
            AgentTerminalDisposition::Completed,
        );

        let duplicate_id = boundary(
            vec![
                use_block("duplicate", serde_json::json!({"verdict": "pass"})),
                use_block("duplicate", serde_json::json!({"verdict": 7})),
            ],
            vec![result("duplicate", "StructuredOutput", false), progress()],
        );
        rejects(&duplicate_id, &schema);
        assert!(continues(&duplicate_id, &schema));

        let different_ids = boundary(
            vec![
                use_block("first", serde_json::json!({"verdict": "pass"})),
                use_block("second", serde_json::json!({"verdict": "pass"})),
            ],
            vec![
                result("first", "StructuredOutput", false),
                result("second", "StructuredOutput", false),
                progress(),
            ],
        );
        rejects(&different_ids, &schema);
        assert!(continues(&different_ids, &schema));

        let error_result = boundary(
            vec![use_block("error", serde_json::json!({"verdict": "pass"}))],
            vec![result("error", "StructuredOutput", true), progress()],
        );
        rejects(&error_result, &schema);
        assert!(continues(&error_result, &schema));

        let mismatched_result = boundary(
            vec![use_block("mismatch", serde_json::json!({"verdict": "pass"}))],
            vec![result("mismatch", "read_file", false), progress()],
        );
        rejects(&mismatched_result, &schema);
        assert!(continues(&mismatched_result, &schema));

        let invalid_value = boundary(
            vec![use_block("invalid", serde_json::json!({"verdict": 7}))],
            vec![result("invalid", "StructuredOutput", false), progress()],
        );
        rejects(&invalid_value, &schema);
        assert!(continues(&invalid_value, &schema));

        let unsupported_schema = serde_json::json!({"type": "object", "oneOf": []});
        let unsupported_schema_value = boundary(
            vec![use_block("unsupported", serde_json::json!({"verdict": "pass"}))],
            vec![result("unsupported", "StructuredOutput", false), progress()],
        );
        assert_eq!(
            final_structured_output(&unsupported_schema_value, &unsupported_schema),
            Some(serde_json::json!({"verdict": "pass"})),
            "unsupported schema keywords must preserve the captured value verbatim"
        );
        assert!(!continues(&unsupported_schema_value, &unsupported_schema));
        assert_eq!(
            agent_terminal_disposition(&unsupported_schema_value, true, true, false),
            AgentTerminalDisposition::Completed,
        );
    }

    #[test]
    fn spawned_agent_text_progress_is_limited_to_read_only_agents() {
        assert!(subagent_allows_text_progress("Explore", None));
        assert!(subagent_allows_text_progress(
            "custom",
            Some(runtime::PermissionMode::ReadOnly),
        ));
        assert!(!subagent_allows_text_progress("general-purpose", None));
        assert!(!subagent_allows_text_progress("debugger", None));
    }

    #[test]
    fn spawned_agent_continuation_state_preserves_outputs_and_total_tool_calls() {
        let schema = serde_json::json!({"type": "object"});
        let partial = "verified partial result ".repeat(20);
        let first = continuation_summary(
            vec![
                test_message(
                    MessageRole::Assistant,
                    vec![
                        ContentBlock::Text {
                            text: partial.clone(),
                        },
                        ContentBlock::ToolUse {
                            id: "structured".to_string(),
                            name: "StructuredOutput".to_string(),
                            input: serde_json::json!({"verdict": "pass"}).to_string(),
                        },
                    ],
                ),
                test_message(
                    MessageRole::Assistant,
                    vec![ContentBlock::Text {
                        text: "[budget] continuation required".to_string(),
                    }],
                ),
            ],
            vec![test_message(
                MessageRole::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "structured".to_string(),
                    tool_name: "StructuredOutput".to_string(),
                    output: "ok".to_string(),
                    is_error: false,
                    images: Vec::new(),
                }],
            )],
            Some(runtime::BudgetExhausted::Iterations),
        );
        let shorter = "shorter partial result ".repeat(13);
        let second = continuation_summary(
            vec![
                test_message(
                    MessageRole::Assistant,
                    vec![ContentBlock::Text {
                        text: shorter.clone(),
                    }],
                ),
                test_message(
                    MessageRole::Assistant,
                    vec![ContentBlock::Text {
                        text: "[budget] continuation required".to_string(),
                    }],
                ),
            ],
            Vec::new(),
            Some(runtime::BudgetExhausted::Iterations),
        );
        let completed = continuation_summary(Vec::new(), Vec::new(), None);

        let mut state = AgentContinuationState::default();
        state.observe(&first, Some(&schema));
        state.observe(&second, Some(&schema));
        state.observe(&completed, Some(&schema));

        assert_eq!(state.cumulative_tool_calls, 1);
        assert_eq!(state.remaining_tool_calls(), SPAWNED_AGENT_MAX_TOOL_CALLS - 1);
        assert_eq!(state.latest_budget_text, shorter);
        assert_eq!(
            state.latest_structured,
            Some(serde_json::json!({"verdict": "pass"}))
        );
        let failed = continuation_error_result(&state, "provider disconnected")
            .expect("partial result should survive a continuation error");
        assert!(failed.contains("shorter partial result"), "{failed}");
        assert!(failed.contains("provider disconnected"), "{failed}");
    }

    #[test]
    fn spawned_agent_provider_error_metadata_preserves_partial_payload() {
        let structured = completion_structured_with_provider_error_class(
            Some(serde_json::json!({"partial": "preserved"})),
            Some(api::ProviderErrorClass::account_rate_limit(None)),
        )
        .expect("provider metadata should be attached");

        assert_eq!(structured["partial"], "preserved");
        assert_eq!(structured["providerErrorClass"], "rateLimit");
    }

    #[test]
    fn spawned_agent_mutation_only_window_preserves_fallback_on_continuation_error() {
        let mutation = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("edit_file")],
            Some(runtime::BudgetExhausted::Iterations),
        );
        let mut state = AgentContinuationState::default();
        state.observe(&mutation, None);

        let failed = continuation_error_result(&state, "provider disconnected")
            .expect("mutation progress must retain a parent-facing fallback");
        assert!(failed.contains("[budget exhausted:"), "{failed}");
        assert!(failed.contains("partial work is recorded in the agent transcript"), "{failed}");
        assert!(failed.contains("provider disconnected"), "{failed}");
    }

    #[test]
    fn spawned_agent_worker_enriches_an_externally_stopped_manifest() {
        let dir = unique_dir("spawn-external-stop-enrichment");
        let manifest = empty_result_manifest(&dir, "stopped");
        assert!(
            super::super::manifest::persist_agent_stopped_state(
                &manifest,
                "external stop requested",
            )
            .expect("persist external stop")
        );

        persist_agent_terminal_state_with_history(
            &manifest,
            "stopped",
            Some("recovered partial result"),
            Some("agent cancelled".to_string()),
            Vec::new(),
        )
        .expect("worker should enrich the stopped terminal state");

        let output = std::fs::read_to_string(&manifest.output_file).expect("read output");
        assert!(output.contains("recovered partial result"), "{output}");
        let reread: AgentOutput = serde_json::from_str(
            &std::fs::read_to_string(&manifest.manifest_file).expect("read manifest"),
        )
        .expect("parse manifest");
        assert_eq!(reread.status, "stopped");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn spawned_agent_tool_budget_counts_issued_tool_uses_not_results() {
        let summary = continuation_summary(
            vec![test_message(
                MessageRole::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "first".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "first.rs"}).to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "second".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path": "second.rs"}).to_string(),
                    },
                ],
            )],
            vec![successful_tool_result("read_file")],
            Some(runtime::BudgetExhausted::Iterations),
        );

        let mut state = AgentContinuationState::default();
        state.observe(&summary, None);

        assert_eq!(state.cumulative_tool_calls, 2);
        assert_eq!(state.remaining_tool_calls(), SPAWNED_AGENT_MAX_TOOL_CALLS - 2);
    }

    #[test]
    fn spawned_agent_continuation_keeps_token_budgets_cumulative() {
        let mut first = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("edit_file")],
            Some(runtime::BudgetExhausted::Iterations),
        );
        first.turn_output_tokens = 60;
        first.usage.input_tokens = 400;

        let mut state = AgentContinuationState::default();
        state.observe(&first, None);
        assert_eq!(state.remaining_output_tokens(Some(100)), Some(40));
        assert_eq!(state.remaining_input_tokens(Some(1_000)), Some(600));
        assert!(state.token_budgets_have_headroom(Some(100), Some(1_000)));

        let mut second = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("edit_file")],
            Some(runtime::BudgetExhausted::Iterations),
        );
        second.turn_output_tokens = 40;
        second.usage.input_tokens = 1_000;
        state.observe(&second, None);

        assert_eq!(state.remaining_output_tokens(Some(100)), Some(0));
        assert_eq!(state.remaining_input_tokens(Some(1_000)), Some(0));
        assert!(!state.token_budgets_have_headroom(Some(100), Some(1_000)));
        assert_eq!(
            state.cumulative_iterations,
            2 * (SPAWNED_AGENT_MAX_ITERATIONS + 1),
        );
    }

    #[test]
    fn spawned_agents_auto_continue_iteration_cutoffs_with_progress() {
        assert!(should_auto_continue_budget(
            Some(runtime::BudgetExhausted::Iterations),
            true,
            0,
            SPAWNED_AGENT_MAX_TOOL_CALLS,
            true,
        ));
    }

    #[test]
    fn spawned_agents_do_not_continue_hard_stop_or_stalled_cutoffs() {
        for kind in [
            runtime::BudgetExhausted::InputTokens,
            runtime::BudgetExhausted::OutputTokens,
            runtime::BudgetExhausted::Deadline,
            runtime::BudgetExhausted::ToolCalls,
            runtime::BudgetExhausted::VerificationTreadmill,
            runtime::BudgetExhausted::ToolRepetition,
        ] {
            assert!(!should_auto_continue_budget(
                Some(kind),
                true,
                0,
                SPAWNED_AGENT_MAX_TOOL_CALLS,
                true,
            ));
        }
        assert!(!should_auto_continue_budget(
            Some(runtime::BudgetExhausted::Iterations),
            false,
            0,
            SPAWNED_AGENT_MAX_TOOL_CALLS,
            true,
        ));
        assert!(!should_auto_continue_budget(
            Some(runtime::BudgetExhausted::InputTokens),
            true,
            SPAWNED_AGENT_MAX_BUDGET_CONTINUATIONS,
            SPAWNED_AGENT_MAX_TOOL_CALLS,
            true,
        ));
        assert!(!should_auto_continue_budget(
            Some(runtime::BudgetExhausted::Iterations),
            true,
            0,
            0,
            true,
        ));
        assert!(!should_auto_continue_budget(
            Some(runtime::BudgetExhausted::Iterations),
            true,
            0,
            SPAWNED_AGENT_MAX_TOOL_CALLS,
            false,
        ));
    }

    /// A sub-agent whose turn the repetition guard killed used to report
    /// `budget_exhausted: None` — indistinguishable from a clean end — so it fell
    /// through `agent_terminal_disposition`'s `None` arm and the parent received
    /// a loop the harness had to kill as a `Completed` result carrying whatever
    /// partial text was lying around. It is now an honest budget failure whose
    /// caller-facing banner names the guard.
    #[test]
    fn a_repetition_killed_subagent_is_a_budget_failure_not_a_silent_completion() {
        let killed = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("read_file")],
            Some(runtime::BudgetExhausted::ToolRepetition),
        );
        assert_eq!(
            agent_terminal_disposition(&killed, false, false, false),
            AgentTerminalDisposition::BudgetFailed(runtime::BudgetExhausted::ToolRepetition),
        );
        // A StructuredOutput deliverable in hand must NOT excuse this one: that
        // exemption is scoped to the fences that bound WORK (iterations, tool
        // calls), and a repetition loop is neither.
        assert_eq!(
            agent_terminal_disposition(&killed, true, true, false),
            AgentTerminalDisposition::BudgetFailed(runtime::BudgetExhausted::ToolRepetition),
            "a submission in hand must not turn a loop-detection stop into success",
        );
        let banner = agent_budget_exhausted_result(
            "partial work",
            runtime::BudgetExhausted::ToolRepetition,
            12,
            30,
        );
        assert!(
            banner.contains("tool-repetition guard"),
            "the banner must name the guard that stopped the agent: {banner}"
        );
        assert!(
            banner.contains("partial work"),
            "the partial result must survive the banner: {banner}"
        );
    }

    #[test]
    fn spawned_agent_continuation_prompt_requires_completion() {
        let prompt = agent_budget_continuation_prompt(runtime::BudgetExhausted::Iterations, 1);
        assert!(prompt.contains("Continue the same task"), "{prompt}");
        assert!(prompt.contains("finish the requested work"), "{prompt}");
        assert!(prompt.contains("Do not restart or merely summarize"), "{prompt}");
        assert!(prompt.contains("Continuation 1/2"), "{prompt}");
    }

    #[test]
    fn subagent_stops_at_max_tool_calls() {
        // The iteration cap alone does not bound tool-call volume: one model turn
        // can issue many parallel `tool_use` blocks. `run_agent_job` wires this
        // cap via `.with_max_tool_calls`, and the enforcement + graceful `failed`
        // completion are proven in
        // `runtime::conversation::tests::run_turn_errors_when_max_tool_calls_is_exceeded`.
        assert_eq!(
            SPAWNED_AGENT_MAX_TOOL_CALLS, 256,
            "spawned sub-agents must keep a finite tool-call cap; usize::MAX reintroduces the \
             unbounded tool-call burst (the 174-call runaway of 2026-06-07)"
        );
    }

    // ── Phase 4 verdict channel (sources #2 planner-bound, #3 ad-hoc) ──────

    #[test]
    fn verdict_passed_maps_pass_and_finding_only() {
        assert_eq!(verdict_passed("pass"), Some(true));
        assert_eq!(verdict_passed("finding"), Some(false));
        // "retry" (no usable structured verdict) and any unrecognized label
        // are ambiguous — never a signal.
        assert_eq!(verdict_passed("retry"), None);
        assert_eq!(verdict_passed("unknown-label"), None);
        assert_eq!(verdict_passed(""), None);
    }

    fn verdict_test_manifest(id: &str, subagent_type: &str) -> AgentOutput {
        AgentOutput {
            route_probe_confidence: None,
            agent_id: id.to_string(),
            parent_session_id: None,
            tool_call_id: None,
            name: id.to_string(),
            label: None,
            description: "agent".to_string(),
            subagent_type: Some(subagent_type.to_string()),
            requested_model: None,
            resolved_model: Some("worker-model".to_string()),
            route_reason: None,
            route_role: None,
            route_complexity: None,
            route_risk: None,
            route_source: None,
        sees: Vec::new(),
            model: Some("worker-model".to_string()),
            status: "running".to_string(),
            output_file: format!("/tmp/{id}.md"),
            manifest_file: format!("/tmp/{id}.json"),
            created_at: "100".to_string(),
            pane: None,
            owner_pid: None,
            run_generation: 0,
            started_at: Some("100".to_string()),
            completed_at: None,
            completion_published_at: None,
            lane_events: Vec::new(),
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
            activity: AgentActivityTelemetry::default(),
            lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
        }
    }

    #[test]
    fn ad_hoc_turn_review_requires_both_type_and_marker() {
        let reviewer = verdict_test_manifest("r1", "code-reviewer");
        let verifier = verdict_test_manifest("r2", "Verification");
        let explorer = verdict_test_manifest("r3", "Explore");

        let job_with = |manifest: AgentOutput, prompt: &str| AgentJob {
            manifest,
            registry: None,
            prompt: prompt.to_string(),
            system_prompt: Vec::new(),
            allowed_tools: std::collections::BTreeSet::new(),
            permission_rules: None,
            permission_mode: None,
            cwd: None,
            lsp: None,
            schema: None,
            workflow_member: false,
            one_shot: false,
            plan_shape: None,
            route_tax: None,
            parent_attempt: None,
            time_budget: None,
            thinking_budget_tokens: None,
            route_effort: None,
            api_concurrency: None,
            route_fallback_models: Vec::new(),
            mcp_passthrough: None,
            hook_config: runtime::RuntimeHookConfig::default(),
            cancel_signal: runtime::HookAbortSignal::new(),
            judged_agent: None,
            verify_loop: None,
            parent_model: None,
            steering: runtime::SteeringQueue::default(),
            transcript_path: None,
            resume: false,
        };

        // Right type + English marker.
        assert!(is_ad_hoc_turn_review(&job_with(
            reviewer.clone(),
            "Adversarially review the current working tree after the implementation."
        )));
        // Right type + Korean marker.
        assert!(is_ad_hoc_turn_review(&job_with(
            verifier.clone(),
            "이 diff를 검증해줘"
        )));
        // Right type, but no whitelist marker — some OTHER unrelated review.
        assert!(!is_ad_hoc_turn_review(&job_with(
            reviewer.clone(),
            "Review the design doc in docs/architecture.md"
        )));
        // Whitelist marker present, but not a reviewer/verifier type.
        assert!(!is_ad_hoc_turn_review(&job_with(
            explorer,
            "Inspect the current diff for stray debug prints"
        )));
        // Neither.
        assert!(!is_ad_hoc_turn_review(&job_with(
            reviewer,
            "Summarize the README"
        )));
    }

    /// Env isolation for the verdict-recording integration tests below:
    /// `ZO_AGENT_STORE` (the worker-manifest lookup `attribution`'s
    /// `record_verdict_outcome_for_agent` reads) and `ZO_STATE_DIR` (the
    /// route-outcome log's root, `runtime::zo_project_state_dir`) both
    /// redirect to fresh temp dirs — regardless of the real process cwd — so
    /// these tests never touch the developer's real `~/.zo`. Guarded by the
    /// crate-wide env-mutation lock (other modules assert on the same globals).
    struct VerdictTestEnv {
        _guard: std::sync::MutexGuard<'static, ()>,
        state_dir: std::path::PathBuf,
        agent_store: std::path::PathBuf,
        prior_state_dir: Option<std::ffi::OsString>,
        prior_agent_store: Option<std::ffi::OsString>,
    }

    impl VerdictTestEnv {
        fn setup(tag: &str) -> Self {
            let guard = crate::tests::env_lock()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let base = std::env::temp_dir().join(format!(
                "zo-verdict-spawn-{tag}-{}-{nanos}",
                std::process::id()
            ));
            let state_dir = base.join("state");
            let agent_store = base.join("agents");
            std::fs::create_dir_all(&state_dir).expect("state dir");
            std::fs::create_dir_all(&agent_store).expect("agent store dir");
            let prior_state_dir = std::env::var_os("ZO_STATE_DIR");
            let prior_agent_store = std::env::var_os(super::super::labels::AGENT_STORE_ENV);
            std::env::set_var("ZO_STATE_DIR", &state_dir);
            std::env::set_var(super::super::labels::AGENT_STORE_ENV, &agent_store);
            Self {
                _guard: guard,
                state_dir,
                agent_store,
                prior_state_dir,
                prior_agent_store,
            }
        }

        fn write_worker_manifest(&self, manifest: &AgentOutput) {
            std::fs::write(
                self.agent_store.join(format!("{}.json", manifest.agent_id)),
                serde_json::to_string(manifest).expect("manifest json"),
            )
            .expect("write worker manifest");
        }

        // Associated fn (not `&self`): the route-outcome log's root is fixed
        // by `ZO_STATE_DIR` (set in `setup`), and the cwd-derived slug
        // underneath it needs no other `self` state to resolve.
        fn read_outcomes() -> Vec<runtime::RouteOutcomeRecord> {
            let cwd = std::env::current_dir().expect("cwd");
            runtime::read_route_outcomes(&cwd).unwrap_or_default()
        }
    }

    impl Drop for VerdictTestEnv {
        fn drop(&mut self) {
            match self.prior_state_dir.take() {
                Some(value) => std::env::set_var("ZO_STATE_DIR", value),
                None => std::env::remove_var("ZO_STATE_DIR"),
            }
            match self.prior_agent_store.take() {
                Some(value) => std::env::set_var(super::super::labels::AGENT_STORE_ENV, value),
                None => std::env::remove_var(super::super::labels::AGENT_STORE_ENV),
            }
            let _ = std::fs::remove_dir_all(&self.state_dir);
            let _ = std::fs::remove_dir_all(&self.agent_store);
        }
    }

    fn passing_completion(structured: Option<serde_json::Value>) -> AgentCompletion {
        AgentCompletion {
            agent_id: "reviewer-agent".to_string(),
            name: "reviewer".to_string(),
            status: "completed".to_string(),
            result: Some("done".to_string()),
            structured,
            error: None,
            run: HelperRun::default(),
        }
    }

    /// A verifier serves the attempt it JUDGES, not the turn that launched it:
    /// its tokens are the price of settling that attempt, and rolling them
    /// into the turn would price the verification onto whatever else the turn
    /// did.
    #[test]
    fn a_verifier_serves_the_attempt_it_judges_over_the_turn_that_launched_it() {
        let mut job = reviewer_job(Some("worker-agent"), Some("parent-model"), "review the diff");
        job.parent_attempt = Some("session-1@5".to_string());
        assert_eq!(
            super::served_attempt(&job).as_deref(),
            Some("worker-agent#0"),
            "the judged attempt wins"
        );

        let mut plain = reviewer_job(None, Some("parent-model"), "do the work");
        plain.parent_attempt = Some("session-1@5".to_string());
        assert_eq!(
            super::served_attempt(&plain).as_deref(),
            Some("session-1@5"),
            "everything else serves the turn that launched it"
        );

        let orphan = reviewer_job(None, None, "do the work");
        assert_eq!(
            super::served_attempt(&orphan),
            None,
            "a spawn nobody launched from a turn names no parent — never a guess"
        );
    }

    /// A spawn nobody laid out IS one delegation; a lane keeps the width its
    /// batch counted, so no reader has to re-count lanes off the agent store.
    #[test]
    fn a_spawns_shape_is_the_hosts_layout_or_one_delegation() {
        let plain = reviewer_job(None, None, "work");
        assert_eq!(super::spawn_plan_shape(&plain), runtime::PlanShape::Delegate);

        let mut lane = reviewer_job(None, None, "work");
        lane.plan_shape = Some(runtime::PlanShape::Parallel { width: 3 });
        assert_eq!(
            super::spawn_plan_shape(&lane),
            runtime::PlanShape::Parallel { width: 3 }
        );

        let mut prelude = reviewer_job(None, None, "work");
        prelude.plan_shape = Some(runtime::PlanShape::HostPrelude { width: 2 });
        assert_eq!(
            super::spawn_plan_shape(&prelude).label(),
            "host-prelude:2",
            "the host's read-only pre-analysis is not a fan-out the model asked for"
        );
    }

    /// A reviewer job; a `judged_agent` id binds it to that worker's attempt
    /// as [`verdict_test_manifest`] spells it (generation 0, `worker-model`,
    /// `Refactor`) — the copy frozen at binding.
    fn reviewer_job(judged_agent: Option<&str>, parent_model: Option<&str>, prompt: &str) -> AgentJob {
        AgentJob {
            manifest: verdict_test_manifest("reviewer-agent", "code-reviewer"),
            registry: None,
            prompt: prompt.to_string(),
            system_prompt: Vec::new(),
            allowed_tools: std::collections::BTreeSet::new(),
            permission_rules: None,
            permission_mode: None,
            cwd: None,
            lsp: None,
            schema: None,
            workflow_member: false,
            one_shot: false,
            plan_shape: None,
            route_tax: None,
            parent_attempt: None,
            time_budget: None,
            thinking_budget_tokens: None,
            route_effort: None,
            api_concurrency: None,
            route_fallback_models: Vec::new(),
            mcp_passthrough: None,
            hook_config: runtime::RuntimeHookConfig::default(),
            cancel_signal: runtime::HookAbortSignal::new(),
            judged_agent: judged_agent.map(|id| verdict_test_manifest(id, "Refactor")),
            verify_loop: None,
            parent_model: parent_model.map(str::to_string),
            steering: runtime::SteeringQueue::default(),
            transcript_path: None,
            resume: false,
        }
    }

    fn epoch(secs: u64) -> std::time::SystemTime {
        std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn launched(plan: Option<av::LoopPlan>) -> super::super::AgentInput {
        match plan {
            Some(av::LoopPlan::Launch(input)) => *input,
            other => panic!("expected a launch, got {other:?}"),
        }
    }

    fn stopped(plan: Option<av::LoopPlan>) -> av::LoopStop {
        match plan {
            Some(av::LoopPlan::Stop(stop)) => stop,
            other => panic!("expected a stop, got {other:?}"),
        }
    }

    fn failing(title: &str, evidence: &str) -> AgentCompletion {
        passing_completion(Some(serde_json::json!({"verdict": "fail", "title": title, "evidence": evidence})))
    }

    /// A first implementation attempt as the loop meets it: rooted in its own
    /// checkout, under a restrictive mode, with a finite budget that began at
    /// epoch second 1000, on a person-pinned model, effort and `large` route.
    fn contracted_implementer(cwd: &std::path::Path) -> AgentJob {
        let mut job = reviewer_job(None, Some("parent-session-model"), "Refactor the parser and update its tests.");
        job.manifest = verdict_test_manifest("impl-1", "Refactor");
        job.manifest.started_at = Some("1000".to_string());
        job.manifest.model = Some("pinned-implementer-model".to_string());
        job.manifest.route_source = Some("pin".to_string());
        job.manifest.route_role = Some("coding".to_string());
        job.manifest.route_complexity = Some("large".to_string());
        job.cwd = Some(cwd.to_path_buf());
        job.permission_mode = Some(runtime::PermissionMode::WorkspaceWrite);
        job.time_budget = Some(Duration::from_secs(600));
        job.route_effort = Some(api::EffortLevel::High);
        job
    }

    /// The verifier job judging `judged` at `round`, carrying `state`.
    fn verifier_carrying(judged: &str, state: av::VerifyLoop) -> AgentJob {
        let mut job = reviewer_job(Some(judged), Some("parent-session-model"), "verify");
        job.verify_loop = Some(state);
        job
    }

    fn loop_at(round: u32, complexity: &str) -> av::VerifyLoop {
        let mut implementer = reviewer_job(None, Some("parent-session-model"), "Refactor the parser.");
        implementer.manifest = verdict_test_manifest("impl-1", "Refactor");
        implementer.manifest.route_complexity = Some(complexity.to_string());
        av::VerifyLoop {
            description: "refactor parser".to_string(),
            prompt: "Refactor the parser.".to_string(),
            subagent_type: Some("Refactor".to_string()),
            contract: av::ExecutionContract::of(&implementer, epoch(0)),
            round,
            repair_target: None,
        }
    }

    /// P2 live: an unattended, completed implementation spawn plans a verifier
    /// bound to it (`judged_agent`, verdict schema, round 1) that works in the
    /// implementer's checkout, under its permission ceiling and on what is left
    /// of its budget — never the process cwd or a fresh window; attended turns,
    /// a verifier itself, and workflow members plan nothing.
    #[test]
    fn a_completed_unattended_implementation_spawn_plans_its_verifier() {
        let _env = VerdictTestEnv::setup("auto-verify-plan");
        let checkout = std::path::PathBuf::from("/nonexistent/checkout-a");
        let job = contracted_implementer(&checkout);
        let completion = passing_completion(None);

        let plan = launched(av::plan_auto_verify(&job, &completion, runtime::Attendance::Unattended, None, epoch(1100)));
        // Bound to the attempt that just finished — identity and route frozen.
        let judged = plan.judged_agent.as_ref().expect("bound to the implementer's attempt");
        assert_eq!((judged.agent_id.as_str(), judged.run_generation), ("impl-1", 0));
        assert_eq!(judged.model.as_deref(), Some("pinned-implementer-model"));
        assert_eq!(judged.route_complexity.as_deref(), Some("large"));
        assert_eq!(plan.subagent_type.as_deref(), Some(av::VERIFIER_SUBAGENT_TYPE));
        assert_eq!(plan.schema, Some(crate::workflow_tools::verdict_schema()));
        assert!(plan.one_shot);
        assert_eq!(plan.cwd.as_deref(), Some(checkout.as_path()), "the verifier inspects the implementer's checkout");
        assert_eq!(plan.parent_permission_mode, Some(runtime::PermissionMode::WorkspaceWrite));
        assert_eq!(plan.time_budget, Some(Duration::from_secs(500)), "what is left of 1000+600 at 1100");
        assert!(plan.route_model.is_none() && plan.route_effort.is_none(), "the reviewer keeps its own route");
        let loop_state = plan.verify_loop.expect("loop state rides the verifier");
        assert_eq!(loop_state.round, 1);
        assert_eq!(loop_state.prompt, "Refactor the parser and update its tests.");
        assert_eq!(loop_state.contract.route_complexity.as_deref(), Some("large"));
        assert!(loop_state.repair_target.is_none());

        let attended = av::plan_auto_verify(&job, &completion, runtime::Attendance::Attended, None, epoch(1100));
        assert!(attended.is_none());
        assert!(av::plan_auto_verify(&job, &completion, runtime::Attendance::Unattended, Some("off"), epoch(1100)).is_none());
        let mut member = job.clone();
        member.workflow_member = true;
        assert!(av::plan_auto_verify(&member, &completion, runtime::Attendance::Unattended, None, epoch(1100)).is_none());
        let judge = reviewer_job(Some("impl-1"), Some("parent-session-model"), "judge impl-1");
        assert!(av::plan_auto_verify(&judge, &completion, runtime::Attendance::Unattended, None, epoch(1100)).is_none());
        let mut first_failed = job.clone();
        first_failed.time_budget = None;
        let failed = AgentCompletion { status: "failed".to_string(), ..passing_completion(None) };
        assert!(
            av::plan_auto_verify(&first_failed, &failed, runtime::Attendance::Unattended, None, epoch(1100)).is_none(),
            "a first attempt that failed is its own red; no loop exists yet"
        );
    }

    /// P2 live: the verifier's failing verdict plans an implementer retry that
    /// carries the finding, the round and the WHOLE saved contract — checkout,
    /// ceiling, remaining budget, the pinned model/effort and the routed
    /// complexity — until the difficulty ceiling, where the loop stops with
    /// an explicit receipt; a pass plans nothing, an unusable or unfinished
    /// verifier stops.
    #[test]
    fn a_failing_verdict_plans_a_retry_until_the_ceiling() {
        let _env = VerdictTestEnv::setup("auto-verify-retry");
        let checkout = std::path::PathBuf::from("/nonexistent/checkout-a");
        let mut implementer = contracted_implementer(&checkout);
        implementer.manifest.route_complexity = Some("medium".to_string());
        let first = launched(av::plan_auto_verify(&implementer, &passing_completion(None), runtime::Attendance::Unattended, None, epoch(1100)))
            .verify_loop
            .expect("loop state");
        let verifier = |round: u32| verifier_carrying("impl-1", av::VerifyLoop { round, ..first.clone() });
        let failing = failing("missing test", "empty input panics");

        let retry = launched(av::plan_retry(&verifier(1), &failing, runtime::Attendance::Unattended, None, epoch(1200)));
        assert_eq!(retry.subagent_type.as_deref(), Some("Refactor"));
        assert!(retry.prompt.starts_with("Refactor the parser and update its tests."));
        assert!(retry.prompt.contains(av::VERIFY_FINDING_MARKER));
        assert!(retry.prompt.contains("missing test: empty input panics"));
        assert_eq!(retry.prior_failures, 1, "one failed attempt so far");
        assert!(retry.judged_agent.is_none(), "the implementer judges nobody");
        assert_eq!(retry.cwd.as_deref(), Some(checkout.as_path()), "the repair works where the implementer did");
        assert_eq!(retry.parent_permission_mode, Some(runtime::PermissionMode::WorkspaceWrite), "never broader");
        assert_eq!(retry.time_budget, Some(Duration::from_secs(400)), "the budget does not restart");
        assert_eq!(retry.route_model.as_deref(), Some("pinned-implementer-model"), "the pin binds the repair");
        assert_eq!(retry.route_source.as_deref(), Some("pin"));
        assert_eq!(retry.route_effort, Some(api::EffortLevel::High));
        assert_eq!(retry.route_complexity.as_deref(), Some("medium"), "the repair's manifest keeps the label");
        let carried = retry.verify_loop.clone().expect("the retry carries the loop state forward");
        assert_eq!(carried.round, 2);
        assert!(carried.repair_target.is_some(), "the repair remembers what it was asked to fix");

        // …and when that retried implementer completes, it is verified again at
        // round 2 (only a verifier itself is exempt), with the saved label.
        let mut retried = reviewer_job(None, Some("parent-session-model"), &retry.prompt);
        retried.manifest = verdict_test_manifest("impl-2", "Refactor");
        retried.verify_loop = retry.verify_loop.clone();
        let again = launched(av::plan_auto_verify(&retried, &passing_completion(None), runtime::Attendance::Unattended, None, epoch(1300)));
        let again = again.verify_loop.expect("loop state");
        assert_eq!(again.round, 2);
        assert_eq!(again.contract.route_complexity.as_deref(), Some("medium"));

        let ceiling = u32::try_from(av::ceiling_for(Some("medium"))).unwrap();
        assert!(
            matches!(
                stopped(av::plan_retry(&verifier(ceiling), &failing, runtime::Attendance::Unattended, None, epoch(1200))),
                av::LoopStop::CeilingReached { .. }
            ),
            "at the ceiling the loop stops, explicitly"
        );
        let passing = passing_completion(Some(serde_json::json!({"verdict": "pass", "coverage": "tests"})));
        assert!(av::plan_retry(&verifier(1), &passing, runtime::Attendance::Unattended, None, epoch(1200)).is_none());
        assert_eq!(
            stopped(av::plan_retry(&verifier(1), &passing_completion(None), runtime::Attendance::Unattended, None, epoch(1200))),
            av::LoopStop::UnusableVerdict
        );
        let crashed = AgentCompletion { status: "failed".to_string(), ..passing_completion(None) };
        assert_eq!(
            stopped(av::plan_retry(&verifier(1), &crashed, runtime::Attendance::Unattended, None, epoch(1200))),
            av::LoopStop::VerifierDidNotFinish { status: "failed".to_string() }
        );
    }

    /// The bug the saved label closes: a `large` first failure used to buy a
    /// round-2 repair whose manifest had no label, so the round-2 verifier
    /// read `Unknown`'s ceiling (2) and escalated where `Large` allows 4.
    #[test]
    fn the_routed_complexity_survives_every_round_of_the_loop() {
        let state = loop_at(2, "large");
        let verifier = verifier_carrying("impl-2", state);
        let next = launched(av::plan_retry(&verifier, &failing("new", "another defect"), runtime::Attendance::Unattended, None, epoch(10)));
        assert_eq!(next.verify_loop.expect("loop state").round, 3, "Large still retries after round 2");
        assert!(av::ceiling_for(Some("large")) > av::ceiling_for(None), "premise: the label is what buys round 3");
    }

    /// Convergence by the workflow's own same-finding identity: the same
    /// finding standing after a repair stops the loop with no further
    /// implementation spawn; a changed finding keeps its retry within the
    /// ceiling. Severity or wording of the SAME fields is still the same.
    #[test]
    fn the_same_finding_after_a_repair_stops_the_loop_and_a_new_one_retries() {
        let first = loop_at(1, "large");
        let found = failing("missing test", "empty input panics");
        let repair = launched(av::plan_retry(&verifier_carrying("impl-1", first), &found, runtime::Attendance::Unattended, None, epoch(10)));
        let after_repair = verifier_carrying("impl-2", repair.verify_loop.expect("loop state"));

        assert_eq!(
            stopped(av::plan_retry(&after_repair, &found, runtime::Attendance::Unattended, None, epoch(20))),
            av::LoopStop::RepeatedFinding { finding: "missing test: empty input panics".to_string() },
            "the repair left the same finding standing"
        );
        let changed = failing("missing test", "the null case panics now");
        let next = launched(av::plan_retry(&after_repair, &changed, runtime::Attendance::Unattended, None, epoch(20)));
        assert_eq!(next.verify_loop.expect("loop state").round, 3);
    }

    /// One deadline for the whole loop: each spawn gets what is LEFT of the
    /// first implementer's budget, and a repaired implementer's own later
    /// start never re-opens the window.
    #[test]
    fn the_loop_deadline_never_restarts() {
        let checkout = std::path::PathBuf::from("/nonexistent/checkout-a");
        let implementer = contracted_implementer(&checkout);
        let state = launched(av::plan_auto_verify(&implementer, &passing_completion(None), runtime::Attendance::Unattended, None, epoch(1100)))
            .verify_loop
            .expect("loop state");
        assert_eq!(state.contract.deadline, Some(epoch(1600)));
        let verifier = verifier_carrying("impl-1", state);
        let found = failing("missing test", "empty input panics");
        assert_eq!(
            launched(av::plan_retry(&verifier, &found, runtime::Attendance::Unattended, None, epoch(1500))).time_budget,
            Some(Duration::from_secs(100))
        );
        assert_eq!(
            stopped(av::plan_retry(&verifier, &found, runtime::Attendance::Unattended, None, epoch(1600))),
            av::LoopStop::DeadlineSpent
        );

        let repair = launched(av::plan_retry(&verifier, &found, runtime::Attendance::Unattended, None, epoch(1200)));
        let mut repaired = reviewer_job(None, Some("parent-session-model"), &repair.prompt);
        repaired.manifest = verdict_test_manifest("impl-2", "Refactor");
        repaired.manifest.started_at = Some("1500".to_string());
        repaired.time_budget = repair.time_budget;
        repaired.verify_loop = repair.verify_loop;
        assert_eq!(
            stopped(av::plan_auto_verify(&repaired, &passing_completion(None), runtime::Attendance::Unattended, None, epoch(1650))),
            av::LoopStop::DeadlineSpent,
            "the repair's own start (1500) must not buy a new window"
        );
    }

    #[test]
    fn an_unrepresentable_finite_budget_stops_instead_of_becoming_unbounded() {
        let mut job = contracted_implementer(std::path::Path::new("/nonexistent/checkout-a"));
        job.time_budget = Some(Duration::MAX);
        let now = epoch(1100);
        assert_eq!(av::ExecutionContract::of(&job, now).budget_at(now), av::Budget::Spent);
    }

    #[test]
    fn an_unrepresentable_manifest_start_keeps_a_finite_budget_without_panicking() {
        let mut job = contracted_implementer(std::path::Path::new("/nonexistent/checkout-a"));
        job.manifest.started_at = Some(u64::MAX.to_string());
        assert_eq!(av::ExecutionContract::of(&job, epoch(1100)).deadline, Some(epoch(1700)));
    }

    /// A repair the loop spawned that ends without completing is a loop
    /// ending too, and says so.
    #[test]
    fn a_repair_that_does_not_finish_stops_the_loop() {
        let mut repaired = reviewer_job(None, Some("parent-session-model"), "Refactor the parser.");
        repaired.manifest = verdict_test_manifest("impl-2", "Refactor");
        repaired.verify_loop = Some(loop_at(2, "large"));
        let stopped_run = AgentCompletion { status: "stopped".to_string(), ..passing_completion(None) };
        assert_eq!(
            stopped(av::plan_auto_verify(&repaired, &stopped_run, runtime::Attendance::Unattended, None, epoch(10))),
            av::LoopStop::RepairDidNotFinish { status: "stopped".to_string() }
        );
        assert!(av::plan_auto_verify(&repaired, &stopped_run, runtime::Attendance::Attended, None, epoch(10)).is_none());
    }

    /// The receipt names the work the loop was verifying, is a failure, and
    /// can never be mistaken for that agent's own completion.
    #[test]
    fn a_stop_receipt_names_the_work_and_never_reads_as_done() {
        let verifier = verifier_carrying("impl-2", loop_at(2, "large"));
        let receipt = av::stop_receipt(&verifier, &av::LoopStop::RepeatedFinding { finding: "missing test".to_string() });
        assert_eq!(receipt.agent_id, "impl-2#verify");
        assert_eq!(receipt.status, "failed");
        let text = receipt.result.expect("receipt text");
        assert!(text.contains("round 2") && text.contains("`impl-2`"), "{text}");
        assert!(text.contains("NOT verified"), "{text}");
        assert!(receipt.error.expect("reason").contains("missing test"));
    }

    /// Restores one env var on drop, inside the env lock the test holds.
    struct EnvRestore(&'static str, Option<std::ffi::OsString>);

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match self.1.take() {
                Some(value) => std::env::set_var(self.0, value),
                None => std::env::remove_var(self.0),
            }
        }
    }

    /// The whole loop through the REAL input→job constructor, with a fake
    /// spawn capturing each job: implementer (checkout A, not the process
    /// cwd; workspace-write; 600 s from t=1000; pinned model/effort; `large`)
    /// → verifier → failing verdict → repair → verifier → changed finding →
    /// retry allowed at round 3; the same finding again → stop; and a
    /// verifier whose spawn cannot start → an explicit launch-failed stop.
    #[test]
    fn the_loop_runs_every_round_under_the_implementers_contract() {
        let env = VerdictTestEnv::setup("auto-verify-contract");
        let _config = EnvRestore("ZO_CONFIG_HOME", std::env::var_os("ZO_CONFIG_HOME"));
        std::env::set_var("ZO_CONFIG_HOME", env.state_dir.join("config"));
        let checkout = env.state_dir.join("checkout-a");
        std::fs::create_dir_all(&checkout).expect("checkout a");
        assert_ne!(std::env::current_dir().ok().as_deref(), Some(checkout.as_path()), "premise: A is not the process cwd");
        let run = |plan: Option<av::LoopPlan>, fired_by: &AgentJob| -> AgentJob {
            let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<AgentJob>));
            let slot = std::sync::Arc::clone(&captured);
            let plan = plan.expect("a plan");
            assert!(matches!(plan, av::LoopPlan::Launch(_)), "expected a launch, got {plan:?}");
            let stop = av::settle(plan, fired_by, move |job| {
                *slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(job);
                Ok(())
            });
            assert_eq!(stop, None, "the launch started");
            let job = captured.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            job.expect("the fake spawn captured the job")
        };
        let unattended = runtime::Attendance::Unattended;
        let implementer = contracted_implementer(&checkout);

        let verifier_1 = run(av::plan_auto_verify(&implementer, &passing_completion(None), unattended, None, epoch(1100)), &implementer);
        assert_eq!(verifier_1.judged_agent.as_ref().map(|judged| judged.agent_id.as_str()), Some("impl-1"));
        assert_eq!(verifier_1.cwd.as_deref(), Some(checkout.as_path()));
        assert_eq!(verifier_1.permission_mode, Some(runtime::PermissionMode::WorkspaceWrite), "no broader than the implementer");
        assert_eq!(verifier_1.time_budget, Some(Duration::from_secs(500)));

        let finding_1 = failing("missing test", "empty input panics");
        let repair = run(av::plan_retry(&verifier_1, &finding_1, unattended, None, epoch(1200)), &verifier_1);
        assert_eq!(repair.cwd.as_deref(), Some(checkout.as_path()), "the repair edits checkout A");
        assert_eq!(repair.permission_mode, Some(runtime::PermissionMode::WorkspaceWrite));
        assert_eq!(repair.time_budget, Some(Duration::from_secs(400)));
        assert_eq!(repair.manifest.model.as_deref(), Some("pinned-implementer-model"), "the person pin binds");
        assert_eq!(repair.manifest.route_source.as_deref(), Some("pin"));
        assert_eq!(repair.route_effort, Some(api::EffortLevel::High));
        assert_eq!(repair.manifest.route_complexity.as_deref(), Some("large"));
        assert!(repair.route_fallback_models.is_empty(), "a pin does not escape to fallbacks");

        let verifier_2 = run(av::plan_auto_verify(&repair, &passing_completion(None), unattended, None, epoch(1300)), &repair);
        let state_2 = verifier_2.verify_loop.clone().expect("loop state");
        assert_eq!(state_2.round, 2);
        assert_eq!(state_2.contract.route_complexity.as_deref(), Some("large"), "round 2 still reads Large");
        assert_eq!(verifier_2.cwd.as_deref(), Some(checkout.as_path()));
        assert_eq!(verifier_2.time_budget, Some(Duration::from_secs(300)));

        let changed = failing("missing test", "the null case panics now");
        let round_3 = launched(av::plan_retry(&verifier_2, &changed, unattended, None, epoch(1350)));
        assert_eq!(round_3.verify_loop.expect("loop state").round, 3, "Large's ceiling, not Unknown's");
        assert_eq!(round_3.time_budget, Some(Duration::from_secs(250)));
        assert!(matches!(
            stopped(av::plan_retry(&verifier_2, &finding_1, unattended, None, epoch(1350))),
            av::LoopStop::RepeatedFinding { .. }
        ));

        let cannot_start = av::settle(
            av::plan_auto_verify(&implementer, &passing_completion(None), unattended, None, epoch(1100)).expect("a plan"),
            &implementer,
            |_job| Err(crate::ToolError::Execution("no worker slot".to_string())),
        );
        assert!(
            matches!(&cannot_start, Some(av::LoopStop::LaunchFailed { error }) if error.contains("no worker slot")),
            "{cannot_start:?}"
        );
    }

    #[test]
    fn planner_bound_verdict_records_against_the_judged_worker() {
        let env = VerdictTestEnv::setup("planner-bound");
        env.write_worker_manifest(&verdict_test_manifest("worker-1", "Refactor"));

        let job = reviewer_job(Some("worker-1"), None, "judge sibling worker-1's change");
        let completion = passing_completion(Some(serde_json::json!({"verdict": "pass", "coverage": "tests"})));
        record_agent_verdict_outcome(&job, &completion);

        let outcomes = VerdictTestEnv::read_outcomes();
        assert_eq!(outcomes.len(), 1, "exactly one verdict for the judged worker");
        let record = &outcomes[0];
        assert_eq!(record.route_key, "subagent:Refactor");
        assert_eq!(record.selected_model, "worker-model");
        assert_eq!(record.signal.as_deref(), Some("verdict"));
        assert_eq!(record.status, "completed");
    }

    /// Whether there is an attempt to credit is settled at BINDING: a bound
    /// attempt that names no model has nothing to credit, and records nothing
    /// even though the store now holds a manifest with a model for that id.
    #[test]
    fn planner_bound_verdict_is_silent_when_the_bound_attempt_names_no_model() {
        let env = VerdictTestEnv::setup("planner-bound-modelless");
        env.write_worker_manifest(&verdict_test_manifest("worker-6", "Refactor"));

        let mut job = reviewer_job(Some("worker-6"), None, "judge a worker");
        if let Some(judged) = job.judged_agent.as_mut() {
            judged.resolved_model = None;
            judged.model = None;
        }
        let completion = passing_completion(Some(serde_json::json!({"verdict": "pass", "coverage": "tests"})));
        record_agent_verdict_outcome(&job, &completion);
        record_agent_verdict_outcome(&job, &passing_completion(None));

        assert!(
            VerdictTestEnv::read_outcomes().is_empty(),
            "a bound attempt with no model must record nothing"
        );
    }

    /// The worker attempt a verifier is bound to — generation 0 on
    /// `worker-model`, a `coding` route the router picked — is resumed on
    /// another model and route between binding and verdict (the store now
    /// holds generation 1). Neither the settled verdict nor an unavailable
    /// verification moves: both name `impl-9#0`, its model and its route,
    /// never generation 1's (t-3959).
    #[test]
    fn a_resume_between_binding_and_verdict_does_not_move_the_attribution() {
        let env = VerdictTestEnv::setup("bound-then-resumed");
        let mut attempt = verdict_test_manifest("impl-9", "Refactor");
        attempt.route_role = Some("coding".to_string());
        attempt.route_source = Some("auto".to_string());
        env.write_worker_manifest(&attempt);
        env.write_worker_manifest(&verdict_test_manifest("reviewer-agent", "code-reviewer"));
        let mut job = reviewer_job(None, None, "judge sibling impl-9's change");
        job.judged_agent = Some(attempt.clone());

        let mut resumed = attempt.clone();
        resumed.run_generation = 1;
        resumed.subagent_type = Some("Debug".to_string());
        resumed.resolved_model = Some("resumed-model".to_string());
        resumed.model = Some("resumed-model".to_string());
        resumed.route_role = Some("debugging".to_string());
        resumed.route_source = Some("pin".to_string());
        env.write_worker_manifest(&resumed);

        record_agent_verdict_outcome(&job, &failing("off by one", "parser.rs:12"));
        record_agent_verdict_outcome(&job, &passing_completion(None));
        let mut crashed = passing_completion(None);
        crashed.status = "failed".to_string();
        record_agent_verdict_outcome(&job, &crashed);

        let outcomes = VerdictTestEnv::read_outcomes();
        let about_worker: Vec<_> = outcomes
            .iter()
            .filter(|record| record.run_id.as_deref().is_some_and(|id| id.starts_with("impl-9#")))
            .collect();
        assert_eq!(
            about_worker.iter().map(|record| record.status.as_str()).collect::<Vec<_>>(),
            ["failed", "stopped", "stopped"],
            "the verdict, then two unavailable verifications: {outcomes:?}"
        );
        for record in about_worker {
            assert_eq!(record.run_id.as_deref(), Some("impl-9#0"), "{record:?}");
            assert_eq!(record.route_key, "subagent:Refactor");
            assert_eq!(record.selected_model, "worker-model");
            assert_eq!(record.role.as_deref(), Some("coding"));
            assert_eq!(record.route_source.as_deref(), Some("auto"));
        }
    }

    /// The verify-by-default loop binds its verifier when it PLANS it, from
    /// the implementer attempt that just finished: resume that implementer
    /// on another model before the verifier's verdict lands, and the verdict
    /// still names the finished attempt and the model it ran on (t-3959).
    #[test]
    fn an_auto_verifier_speaks_for_the_attempt_it_was_planned_for() {
        let env = VerdictTestEnv::setup("auto-verify-bound");
        let implementer = contracted_implementer(std::path::Path::new("/nonexistent/checkout-a"));
        let plan = launched(av::plan_auto_verify(
            &implementer,
            &passing_completion(None),
            runtime::Attendance::Unattended,
            None,
            epoch(1100),
        ));
        let mut resumed = implementer.manifest.clone();
        resumed.run_generation = 1;
        resumed.resolved_model = Some("resumed-model".to_string());
        resumed.route_complexity = Some("small".to_string());
        env.write_worker_manifest(&resumed);

        let mut verifier = reviewer_job(None, None, "verify");
        verifier.judged_agent = plan.judged_agent;
        record_agent_verdict_outcome(&verifier, &failing("missing test", "empty input panics"));

        let outcomes = VerdictTestEnv::read_outcomes();
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        assert_eq!(outcomes[0].run_id.as_deref(), Some("impl-1#0"));
        assert_eq!(outcomes[0].selected_model, "worker-model");
        assert_eq!(outcomes[0].complexity.as_deref(), Some("large"));
        assert_eq!(outcomes[0].status, "failed");
    }

    #[test]
    fn ad_hoc_review_records_against_the_main_turn_when_whitelisted() {
        let _env = VerdictTestEnv::setup("ad-hoc-whitelisted");

        let job = reviewer_job(
            None,
            Some("claude-opus-4-8"),
            "Inspect the current diff and relevant tests.",
        );
        let completion = passing_completion(Some(serde_json::json!({"verdict": "pass", "coverage": "tests"})));
        record_agent_verdict_outcome(&job, &completion);

        let outcomes = VerdictTestEnv::read_outcomes();
        assert_eq!(outcomes.len(), 1);
        let record = &outcomes[0];
        assert_eq!(record.route_key, "main:turn");
        assert_eq!(record.selected_model, "claude-opus-4-8");
        assert_eq!(record.signal.as_deref(), Some("verdict"));
        assert_eq!(record.decision.as_deref(), Some("verify"), "an ad-hoc review is a VERIFY decision");
        assert_eq!(record.status, "completed");
    }

    #[test]
    fn ad_hoc_review_is_silent_without_a_whitelist_marker() {
        let _env = VerdictTestEnv::setup("ad-hoc-no-marker");

        let job = reviewer_job(
            None,
            Some("claude-opus-4-8"),
            "Please look over the project and tell me what you think.",
        );
        let completion = passing_completion(Some(serde_json::json!({"verdict": "pass", "coverage": "tests"})));
        record_agent_verdict_outcome(&job, &completion);

        assert!(
            VerdictTestEnv::read_outcomes().is_empty(),
            "an ad-hoc review without a current-turn marker must never guess its target"
        );
    }

    #[test]
    fn ad_hoc_review_is_silent_without_a_parent_model() {
        let _env = VerdictTestEnv::setup("ad-hoc-no-parent-model");

        let job = reviewer_job(None, None, "Inspect the current diff and relevant tests.");
        let completion = passing_completion(Some(serde_json::json!({"verdict": "pass", "coverage": "tests"})));
        record_agent_verdict_outcome(&job, &completion);

        assert!(
            VerdictTestEnv::read_outcomes().is_empty(),
            "no parent model available at this seam must skip recording, never guess a route"
        );
    }

    /// No `structured` output at all — `semantic_verdict` recovers no usable
    /// label. That never becomes a pass or a failure of the WORK: the bound
    /// worker's attempt is only marked unverified (a `stopped` verify record,
    /// non-decisive), the verifier's own attempt takes the fault for its
    /// unusable output, and the ad-hoc path — no bound attempt — still records
    /// nothing (t-3920).
    #[test]
    fn an_unusable_bound_verdict_leaves_the_worker_unverified_and_faults_the_verifier() {
        let env = VerdictTestEnv::setup("unparseable");
        env.write_worker_manifest(&verdict_test_manifest("worker-2", "Refactor"));
        env.write_worker_manifest(&verdict_test_manifest("reviewer-agent", "code-reviewer"));

        let bound_job = reviewer_job(Some("worker-2"), None, "judge sibling worker-2's change");
        record_agent_verdict_outcome(&bound_job, &passing_completion(None));

        let ad_hoc_job = reviewer_job(
            None,
            Some("claude-opus-4-8"),
            "Inspect the current diff and relevant tests.",
        );
        record_agent_verdict_outcome(&ad_hoc_job, &passing_completion(None));

        let outcomes = VerdictTestEnv::read_outcomes();
        assert_eq!(outcomes.len(), 2, "{outcomes:?}");
        let worker = outcomes
            .iter()
            .find(|record| record.run_id.as_deref() == Some("worker-2#0"))
            .expect("the worker's attempt is marked");
        assert_eq!(worker.status, "stopped", "unverified is neither a pass nor a failure");
        assert_eq!(worker.decision_kind(), runtime::DecisionKind::Verify);
        let verifier = outcomes
            .iter()
            .find(|record| record.run_id.as_deref() == Some("reviewer-agent#0"))
            .expect("the verifier's own attempt takes the fault");
        assert_eq!(verifier.status, "failed");
        assert_eq!(verifier.verdict_subject_kind(), runtime::VerdictSubject::Validator);
        assert!(
            outcomes.iter().all(|record| record.route_key != "main:turn"),
            "the ad-hoc path never guesses a verdict out of unusable output"
        );
    }

    /// A bound verifier that never finished judged nothing: the worker's
    /// attempt is unverified, and the verifier's own crash is already its own
    /// run outcome — no fault recorded on top of it.
    #[test]
    fn a_bound_verifier_that_never_finished_leaves_the_worker_unverified_without_blame() {
        let env = VerdictTestEnv::setup("verifier-failed");
        env.write_worker_manifest(&verdict_test_manifest("worker-4", "Refactor"));
        env.write_worker_manifest(&verdict_test_manifest("reviewer-agent", "code-reviewer"));

        let job = reviewer_job(Some("worker-4"), None, "judge sibling worker-4's change");
        let mut crashed = passing_completion(None);
        crashed.status = "failed".to_string();
        record_agent_verdict_outcome(&job, &crashed);

        let outcomes = VerdictTestEnv::read_outcomes();
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        assert_eq!(outcomes[0].run_id.as_deref(), Some("worker-4#0"));
        assert_eq!(outcomes[0].status, "stopped");
    }

    /// The spawn recorder and the verdict recorder name the SAME implementer
    /// attempt, so the worker's finished-but-wrong run is one failure — not
    /// the 1-1 wash per-receipt counting made of it — and an unusable second
    /// verdict about that attempt does not clear the failure (t-3920).
    #[test]
    fn a_failing_bound_verdict_speaks_for_the_workers_own_run() {
        let env = VerdictTestEnv::setup("same-attempt");
        let worker = verdict_test_manifest("worker-5", "Refactor");
        env.write_worker_manifest(&worker);
        env.write_worker_manifest(&verdict_test_manifest("reviewer-agent", "code-reviewer"));
        let cwd = std::env::current_dir().expect("cwd");
        let finished = passing_completion(None);
        runtime::record_route_outcome(&cwd, &route_outcome_record(&worker, &finished, None, &worker))
            .expect("the worker's run outcome");

        let job = reviewer_job(Some("worker-5"), None, "judge sibling worker-5's change");
        record_agent_verdict_outcome(&job, &failing("off by one", "parser.rs:12"));
        record_agent_verdict_outcome(&job, &passing_completion(None));

        let outcomes = VerdictTestEnv::read_outcomes();
        let about_worker: Vec<_> = outcomes
            .iter()
            .filter(|record| record.run_id.as_deref() == Some("worker-5#0"))
            .cloned()
            .collect();
        assert_eq!(about_worker.len(), 3, "run outcome, failing verdict, unverified mark: {outcomes:?}");
        let summary = runtime::summarize_route_outcomes(&about_worker);
        let bucket = &summary.by_route[0];
        assert_eq!(bucket.route_key, "subagent:Refactor");
        assert_eq!((bucket.completed, bucket.failed, bucket.stopped), (0, 1, 0));
    }

    #[test]
    fn still_running_completion_is_never_recorded() {
        let env = VerdictTestEnv::setup("still-running");
        env.write_worker_manifest(&verdict_test_manifest("worker-3", "Refactor"));

        let job = reviewer_job(Some("worker-3"), None, "judge sibling worker-3's change");
        let mut completion = passing_completion(Some(serde_json::json!({"verdict": "pass", "coverage": "tests"})));
        completion.status = "still_running".to_string();
        record_agent_verdict_outcome(&job, &completion);

        assert!(
            VerdictTestEnv::read_outcomes().is_empty(),
            "a still-running completion must never be recorded — only a terminal status may reach the verdict recorder"
        );
    }

    /// Drives the loop's own decision over a scripted run.
    ///
    /// `agent_turn_step` is the function `run_agent_job` matches on, so this
    /// exercises the real sequencing rather than a restatement of it — the loop
    /// itself only applies the runtime effects each arm names.
    fn drive(
        turns: &[runtime::TurnSummary],
        schema: Option<&serde_json::Value>,
    ) -> (Vec<AgentTurnStep>, AgentContinuationState) {
        let mut state = AgentContinuationState::default();
        let mut steps = Vec::new();
        for summary in turns {
            state.observe(summary, schema);
            let step = agent_turn_step(
                summary,
                &state,
                schema,
                state.remaining_tool_calls(),
                true,
                false,
            );
            steps.push(step);
            match step {
                AgentTurnStep::Continue(_) => state.continuations_used += 1,
                AgentTurnStep::WrapUp => state.wrap_up_used = true,
                AgentTurnStep::Stop => break,
            }
        }
        (steps, state)
    }

    /// The whole point of the wrap-up: a run that gathered results and then hit
    /// the tool-call fence gets ONE tool-free synthesis turn, and a synthesis
    /// turn that delivers nothing still ends as the honest budget failure
    /// rather than a `completed` that never produced anything.
    #[test]
    fn a_gathered_run_gets_one_wrap_up_and_an_empty_one_still_fails_honestly() {
        let gathered = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("Read")],
            Some(runtime::BudgetExhausted::ToolCalls),
        );
        // The synthesis turn produced no text and submitted nothing.
        let empty_wrap_up = continuation_summary(Vec::new(), Vec::new(), None);

        let (steps, state) = drive(&[gathered, empty_wrap_up], None);

        assert_eq!(
            steps,
            vec![AgentTurnStep::WrapUp, AgentTurnStep::Stop],
            "tool fence with results gathered must grant exactly one synthesis turn"
        );
        assert!(state.wrap_up_used, "the allowance is spent, not re-grantable");
        assert_eq!(
            agent_terminal_disposition(
                &continuation_summary(Vec::new(), Vec::new(), None),
                false,
                false,
                state.wrap_up_used,
            ),
            AgentTerminalDisposition::BudgetFailed(runtime::BudgetExhausted::ToolCalls),
            "a wrap-up that delivered nothing must not be filed as completed"
        );
    }

    /// The allowance is once per run. A second tool fence after the synthesis
    /// turn has to fall through to the honest failure instead of looping.
    #[test]
    fn the_wrap_up_allowance_is_not_granted_twice() {
        let gathered = continuation_summary(
            Vec::new(),
            vec![successful_tool_result("Grep")],
            Some(runtime::BudgetExhausted::ToolCalls),
        );
        // The synthesis turn reached for a tool anyway and re-exhausted.
        let re_exhausted = continuation_summary(
            Vec::new(),
            Vec::new(),
            Some(runtime::BudgetExhausted::ToolCalls),
        );

        let (steps, _) = drive(&[gathered, re_exhausted], None);

        assert_eq!(
            steps,
            vec![AgentTurnStep::WrapUp, AgentTurnStep::Stop],
            "a re-exhausted synthesis turn falls through, it does not re-arm"
        );
    }

    /// A run that never gathered anything has nothing to synthesize, so the
    /// fence is a plain failure — the allowance keys on gathered work, not on
    /// "the turn made progress".
    #[test]
    fn a_run_with_nothing_gathered_gets_no_wrap_up() {
        let barren = continuation_summary(
            Vec::new(),
            Vec::new(),
            Some(runtime::BudgetExhausted::ToolCalls),
        );
        let (steps, state) = drive(&[barren], None);
        assert_eq!(steps, vec![AgentTurnStep::Stop]);
        assert!(!state.wrap_up_used);
    }

    /// A schema agent that already submitted its deliverable is done: prompting
    /// it to call `StructuredOutput` exactly once again would only solicit a
    /// duplicate overwrite.
    #[test]
    fn a_schema_agent_that_already_submitted_gets_no_wrap_up() {
        // `final_structured_output` pairs the use with its result by id, so the
        // two must match or the submission is invisible to the loop.
        const SUBMISSION_ID: &str = "structured-1";

        let schema = serde_json::json!({"type": "object"});
        let submitted = continuation_summary(
            vec![test_message(
                MessageRole::Assistant,
                vec![ContentBlock::ToolUse {
                    id: SUBMISSION_ID.to_string(),
                    name: "StructuredOutput".to_string(),
                    input: serde_json::json!({"ok": true}).to_string(),
                }],
            )],
            vec![test_message(
                MessageRole::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: SUBMISSION_ID.to_string(),
                    tool_name: "StructuredOutput".to_string(),
                    output: "ok".to_string(),
                    is_error: false,
                    images: Vec::new(),
                }],
            )],
            Some(runtime::BudgetExhausted::ToolCalls),
        );

        let (steps, state) = drive(&[submitted], Some(&schema));

        assert_eq!(steps, vec![AgentTurnStep::Stop]);
        assert!(
            state.latest_structured.is_some(),
            "precondition: the deliverable is in hand"
        );
        assert_eq!(
            agent_terminal_disposition(
                &continuation_summary(
                    Vec::new(),
                    Vec::new(),
                    Some(runtime::BudgetExhausted::ToolCalls)
                ),
                true,
                true,
                state.wrap_up_used,
            ),
            AgentTerminalDisposition::Completed,
            "a tool-call cutoff WITH a submission is a completion, not a failure"
        );
    }

}
