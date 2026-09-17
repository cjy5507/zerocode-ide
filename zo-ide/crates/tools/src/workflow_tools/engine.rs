//! Phase-mode workflow execution engine (roadmap MVP).
//!
//! The engine wraps zo's existing multi-agent primitives in a deterministic
//! phase loop: resolve a phase's items, fan them out in parallel, wait on a
//! barrier, collect, then feed the results into the next phase. It is the
//! "control layer" the design calls for — `SpawnMultiAgent` generalized from a
//! single 1-phase parallel into multi-phase + control flow.
//!
//! ## Why a backend trait
//!
//! The engine never spawns threads or touches the network directly. It talks to
//! an [`AgentBackend`] — `spawn` (launch one agent, return its id) and `wait`
//! (barrier over agent ids). Production binds these to `execute_agent` +
//! `wait_for_agent_completions` (see `mod.rs`'s `LiveBackend`); tests bind an
//! in-memory mock. This is the `execute_agent_with_spawn` injection precedent,
//! widened to also cover the barrier so the whole orchestration is testable
//! with zero global state and zero network.
//!
//! ## Honest limits (see the design's §6/§13)
//!
//! * **Cancellation** is checked at each phase boundary via [`RunOptions::cancel`].
//!   The engine stops spawning further phases when the flag trips and asks the
//!   live backend to cooperatively stop any phase agents that miss the barrier,
//!   so timeout stragglers do not keep stale cancel handles forever.
//! * **`repeat`** runs a phase over multiple rounds: `until: fixed` always runs
//!   `max_rounds`, `until: no_new` stops early once a round adds no new
//!   `dedup_by` result. Prior-round results accumulate into `{seen}` and the
//!   final phase items are the deduped union across rounds (loop-until-dry).
//! * **`pipeline` mode** streams each first-phase item through every later
//!   phase as an independent chain (no cross-item barrier); see [`run_pipeline`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::misc_tools::{AgentActivitySnapshot, AgentCompletion, AgentInput};
use crate::ToolError;

use super::progress::{PhaseSkeleton, ProgressEvent, ProgressSink};
use super::spec::{
    NormalizedPhase, NormalizedWorkflow, PhaseSource, RepeatPolicy, Until, WorkflowMode,
};
use super::worktree::WorktreeProvider;

// `pub(crate)`: reused by the general single-agent spawn path
// (`misc_tools::agent_tools::spawn`, Phase 4 verdict widening) for the SAME
// verdict recorder and structured-verdict classifier the workflow engine's
// repair loop already uses — one parser/recorder pair, two callers.
pub(crate) mod attribution;
pub(crate) mod items;
mod prompts;
mod rolling;
#[cfg(test)]
mod rolling_tests;
mod spawn;
mod synthesis;

use items::{item_text_for_mapping, resolve_items};
use prompts::{render_prompt, value_to_prompt_string};
use spawn::{resolve_isolation, run_phase, spawn_and_collect, SpawnUnit};
#[cfg(test)]
use spawn::alternate_provider_route_in_inventory;
use synthesis::{append_outcome_notes, run_judge, run_synthesize};

// ---------------------------------------------------------------------------
// Backend seam
// ---------------------------------------------------------------------------

/// The two operations the engine needs from the agent infrastructure. Injected
/// so the orchestration logic is testable without spawning real threads.
pub(crate) trait AgentBackend {
    /// Launch one agent and return its id, or fail to launch.
    fn spawn(&mut self, input: AgentInput) -> Result<String, ToolError>;
    /// Block until every id has a completion or `timeout` elapses. Stragglers
    /// are surfaced as `still_running` by simple/test backends; live backends
    /// should implement [`Self::cancel`] so the engine can stop them after the
    /// barrier.
    fn wait(&self, ids: &[String], timeout: Duration) -> Vec<AgentCompletion>;
    /// Opt in only when every spawned agent is forcibly read-only.
    fn allows_readonly_refill(&self) -> bool {
        false
    }
    /// Observe without cancelling survivors or resetting the phase's watchdog.
    fn wait_next(
        &self,
        ids: &[String],
        timeout: Duration,
        _watch: &mut super::wait::WaitState,
    ) -> Vec<AgentCompletion> {
        self.wait(ids, timeout)
    }
    /// Liveness of the exact attempt owned by this backend, not its logical status.
    fn worker_is_live(&self, _id: &str) -> bool {
        false
    }
    /// [`Self::wait`] plus a completion-order observer: `on_done` fires once
    /// per agent the moment it completes, *during* the barrier, so per-agent
    /// progress events move while the phase is still collecting. The default
    /// degrades gracefully for simple backends: barrier first, then replay
    /// every completion through the observer.
    fn wait_observed(
        &self,
        ids: &[String],
        timeout: Duration,
        on_done: &mut dyn FnMut(&AgentCompletion),
    ) -> Vec<AgentCompletion> {
        let completions = self.wait(ids, timeout);
        for completion in &completions {
            on_done(completion);
        }
        completions
    }
    /// Cooperatively stop an agent that did not produce a terminal completion
    /// before the barrier elapsed. Test/simple backends may return `None` to
    /// keep the legacy `still_running` item; the live backend aborts the
    /// sub-agent's cancel signal and returns a terminal `stopped` completion.
    fn cancel(&self, _id: &str) -> Option<AgentCompletion> {
        None
    }
    /// Best-effort manifest activity for startup diagnosis, bounded recovery,
    /// and actual tool-observed skill receipts. Test/simple backends can omit
    /// it without changing their wait semantics.
    fn activity(&self, _id: &str) -> Option<AgentActivitySnapshot> {
        None
    }
    fn usage_cost(&self, _id: &str) -> Option<crate::misc_tools::AgentUsageCost> {
        None
    }

    /// Output-only estimate for backends without request usage receipts.
    /// The live backend uses per-request receipts instead.
    fn output_price_per_token(&self) -> f64 {
        0.0
    }
    /// The spawning session's agent registry (t-2511), for the engine's own
    /// id-keyed reads — verdict attribution looks a manifest up by id. Simple
    /// and test backends have none; the live backend hands over its own.
    fn agent_registry(&self) -> Option<std::sync::Arc<crate::misc_tools::AgentRegistry>> {
        None
    }
}

/// Resume cache seam (roadmap step 7). The engine consults it at each phase
/// boundary: a hit replays the cached [`PhaseReport`] without spawning, so a
/// re-run of the same spec+input resumes instead of repeating completed work.
/// Injected so file I/O is behind a trait and the resume logic is testable with
/// an in-memory mock. The cache itself owns the `run_id` scoping (see
/// `cache::FileCache`); the engine only ever sees the completed-phase prefix.
pub(crate) trait WorkflowCache {
    /// The completed-phase prefix cached for this run, or `None` when there is
    /// no cache, it is unreadable, or it belongs to a different spec+input.
    fn load(&self) -> Option<Vec<PhaseReport>>;
    /// Persist the completed-phase prefix so far (called after each phase).
    fn store(&self, phases: &[PhaseReport]);
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct SemanticCacheKey {
    pub phase_id: String,
    pub item: String,
    pub schema_fingerprint: String,
    pub verifier_fingerprint: String,
}

impl SemanticCacheKey {
    /// Stable file-cache key. JSON object keys must be strings, so the
    /// production cache never serializes this struct directly as a map key.
    pub(crate) fn stable_key(&self) -> String {
        format!(
            "v1:{}",
            serde_json::to_string(self).unwrap_or_else(|_| "null".to_string())
        )
    }
}

/// Cross-run semantic verifier cache. Unlike [`WorkflowCache`], this stores only
/// validated pass receipts for selective schema verifiers, so an edited workflow
/// can reuse still-valid item-level proofs without replaying whole phases.
pub(crate) trait SemanticCache {
    fn load_pass(&self, key: &SemanticCacheKey) -> Option<PassReceipt>;
    fn store_pass(&self, key: &SemanticCacheKey, receipt: &PassReceipt);
}

/// Default phase inactivity window. Live workflow agents reset this window with
/// task progress; simple/test backends retain the bounded barrier semantics.
const DEFAULT_PHASE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Absolute safety cap for a live phase even when task progress keeps arriving.
const DEFAULT_PHASE_HARD_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
const DEFAULT_FIRST_ACTION_TIMEOUT: Duration = Duration::from_secs(4 * 60);
const DEFAULT_REASONING_EXTENSION: Duration = Duration::from_secs(4 * 60);

/// Verification did not produce a process exit code (spawn/timeout/runner
/// failure). It keeps `command_green` red without pretending the implementation
/// itself failed quality verification.
pub(super) const CHECK_INFRA_ERROR: i32 = -1;

/// Run-time knobs threaded into [`run`].
pub(crate) struct RunOptions<'a> {
    /// How long each phase barrier waits before marking stragglers
    /// `still_running`.
    pub phase_timeout: Duration,
    /// Cooperative cancellation, checked at every phase boundary.
    pub cancel: Option<&'a AtomicBool>,
    /// Optional resume cache. When set, completed phases are replayed from it
    /// instead of re-spawned (roadmap step 7). `None` disables caching.
    pub cache: Option<&'a dyn WorkflowCache>,
    /// Optional item-level pass cache for selective schema verifier phases.
    /// This is narrower than full phase replay: only semantic `pass` receipts
    /// are reused, and every reuse is still gated by `command_green`.
    pub semantic_cache: Option<&'a dyn SemanticCache>,
    /// Per-agent worktree isolation provider (roadmap step 8a). Consulted only
    /// when the spec sets `isolation:"worktree"`; `None` (or an absent provider
    /// under that isolation) runs without isolation and the engine notes it.
    pub worktree: Option<&'a dyn WorktreeProvider>,
    /// Live-progress sink. When set, the engine emits a topology event at every
    /// phase / round / spawn boundary so the TUI can draw a live workflow tree.
    /// `None` (the default) makes every emit a no-op — offline tests and the
    /// resume path pay nothing.
    pub progress: Option<&'a dyn ProgressSink>,
    /// Verification command runner for `repeat.until = "command_green"`: maps a
    /// shell command to its exit code (0 = green, positive = quality red,
    /// negative = verification infrastructure failure). Production wires it to
    /// `runtime::execute_bash` (run in the main working tree); tests inject a
    /// scripted closure. `None` means a `command_green` loop never greens early
    /// (runs to `max_rounds`) — the engine never spawns a shell on its own.
    pub check: Option<&'a dyn Fn(&str) -> i32>,
}

impl<'a> RunOptions<'a> {
    /// Production defaults: a bounded phase barrier, with no cancel flag, no
    /// cache, and no worktree isolation.
    pub(crate) fn production() -> Self {
        Self {
            phase_timeout: phase_timeout_from_env(),
            cancel: None,
            cache: None,
            semantic_cache: None,
            worktree: None,
            progress: None,
            check: None,
        }
    }

    /// Attach a resume cache (the production path wires a `cache::FileCache`).
    pub(crate) fn with_cache(mut self, cache: &'a dyn WorkflowCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Attach the item-level semantic verifier cache.
    pub(crate) fn with_semantic_cache(mut self, cache: &'a dyn SemanticCache) -> Self {
        self.semantic_cache = Some(cache);
        self
    }

    /// Attach a worktree isolation provider (the production path wires a
    /// `worktree::GitWorktreeProvider`).
    pub(crate) fn with_worktree(mut self, worktree: &'a dyn WorktreeProvider) -> Self {
        self.worktree = Some(worktree);
        self
    }

    /// Attach a live-progress sink (the production path wires a
    /// `progress::LiveProgressSink`).
    pub(crate) fn with_progress(mut self, progress: &'a dyn ProgressSink) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Attach the verification-command runner for `command_green` repeat loops
    /// (the production path wires a `runtime::execute_bash` closure).
    pub(crate) fn with_check(mut self, check: &'a dyn Fn(&str) -> i32) -> Self {
        self.check = Some(check);
        self
    }

    /// Attach a cooperative cancel flag (the production path wires the
    /// process-global foreground signal, [`foreground_workflow_cancel_flag`]).
    /// Checked at every phase / stage / repeat boundary by [`is_cancelled`].
    pub(crate) fn with_cancel(mut self, cancel: &'a AtomicBool) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

/// Process-global cooperative cancel for the foreground workflow run.
///
/// The `Workflow` tool runs on a `spawn_blocking` worker, which cannot be
/// aborted by dropping the turn future on Ctrl+C — so the phase loop would keep
/// spawning new phases after the user interrupts. The TUI sets this flag on
/// cancel ([`request_foreground_workflow_cancel`]); the engine polls it through
/// the `RunOptions::cancel` seam and stops at the next phase boundary. One
/// global suffices because at most one foreground workflow runs at a time.
static FOREGROUND_WORKFLOW_CANCEL: AtomicBool = AtomicBool::new(false);

/// Signal the in-flight foreground workflow (if any) to stop at its next phase
/// boundary. Idempotent and safe to call when no workflow is running — the flag
/// is cleared when the next run starts (`clear_foreground_workflow_cancel`).
pub fn request_foreground_workflow_cancel() {
    FOREGROUND_WORKFLOW_CANCEL.store(true, Ordering::SeqCst);
}

/// Clear the foreground cancel flag. Called at the start of every workflow run
/// so a cancel from a previous run never aborts the next one.
pub(crate) fn clear_foreground_workflow_cancel() {
    FOREGROUND_WORKFLOW_CANCEL.store(false, Ordering::SeqCst);
}

/// The foreground cancel flag, for wiring into [`RunOptions::with_cancel`].
pub(crate) fn foreground_workflow_cancel_flag() -> &'static AtomicBool {
    &FOREGROUND_WORKFLOW_CANCEL
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        Self::production()
    }
}

// ---------------------------------------------------------------------------
// Result types (serialized into the tool result JSON)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct WorkflowReport {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// `completed` | `failed` | `budget_exhausted` | `cancelled`.
    pub status: String,
    pub phases: Vec<PhaseReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthesis: Option<String>,
    /// Verdict of the optional `judge` step: the selected winner plus rationale
    /// and ranking. `None` when no judge was declared, the run was cancelled,
    /// budget was exhausted, or the judge produced no parseable verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judgement: Option<Judgement>,
    pub agents_spawned: usize,
    pub budget_exhausted: bool,
    /// Total output tokens every sub-agent reported spending this run (the sum of
    /// each completion's per-turn `token_history`). Surfaces what the optional
    /// `max_output_tokens` budget measured against. `0` when no usage flowed (a
    /// mock backend, or a run that spawned nothing), so it is omitted then.
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub output_tokens: u64,
    /// Estimated cumulative cost (USD); missing usage and prices are reported separately.
    #[serde(skip_serializing_if = "is_zero_f64")]
    pub cost_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_cost: Option<crate::misc_tools::AgentUsageCost>,
    /// Output tokens left under `max_output_tokens`, when that budget was set.
    /// Surfaces the headroom alongside `output_tokens` so a budgeted run reads
    /// honestly even when it stopped early.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_output_tokens: Option<u64>,
    /// USD left under `max_cost_usd`, when that budget was set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_cost_usd: Option<f64>,
    /// Honest caveats surfaced to the model: deferred `repeat`, budget cut-offs,
    /// cancellation, still-running stragglers, failures. Never hidden.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// `skip_serializing_if` predicate for the `output_tokens` field — omit it from
/// the report JSON when no usage was recorded. Serde's contract hands the
/// predicate a `&u64`, so the by-ref signature is required (mirrors
/// `is_zero_usize` in `misc_tools::agent_tools`).
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

/// `skip_serializing_if` predicate for the estimated `cost_usd` field — omit it
/// when no model price was known (so cost stayed exactly `0.0`).
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

/// A judge's verdict over the completed candidates. `winner_index` is the
/// 0-based ordinal of the chosen candidate in [`collect_candidates`] order
/// (validated in range, so a hallucinated index yields no verdict).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Judgement {
    pub winner_index: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub rationale: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ranking: Vec<usize>,
}

// `PhaseReport`/`ItemResult` are both the tool-output shape (Serialize) and the
// resume-cache shape (Deserialize). Per the context7 cache convention they are
// *not* `deny_unknown_fields` — a newer zo writing an extra field must not
// break an older zo reading the cache — and every optional field is
// `#[serde(default)]` so an older cache missing a field still loads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PhaseReport {
    pub id: String,
    pub rounds: u32,
    #[serde(default)]
    pub items: Vec<ItemResult>,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub carried_pass_count: usize,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub retried_finding_count: usize,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub skipped_count: usize,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub blocked_finding_count: usize,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub escalated_finding_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pass_receipts: Vec<PassReceipt>,
    /// Output tokens this phase's agents reported (the run-cumulative delta across
    /// the phase). Omitted when zero (a mock backend or a no-spawn phase).
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub output_tokens: u64,
    /// Bytes of prior-round results relayed into later rounds of this phase,
    /// summed over rounds. Zero for a single-round phase — there is nothing
    /// earlier to carry. Persisted because the run record outlives the session
    /// transcript, and reconstructing relay presence from spec text is exactly
    /// what produced two disagreeing answers before this field existed (a
    /// preset expands its relay engine-side, so the authored spec does not
    /// show it).
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub relay_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ItemResult {
    pub index: usize,
    /// The rendered `{item}` text this agent worked on.
    pub input: String,
    pub agent_id: String,
    /// `completed` | `failed` | `still_running` | `stopped`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Extracted JSON when the phase declared a `schema` and extraction
    /// succeeded; `None` (with `result` preserved) when it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<Value>,
    /// Output tokens this agent reported. Omitted when zero (a mock backend, a
    /// `still_running`/`stopped` straggler, or a spawn that never streamed).
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub output_tokens: u64,
    /// Skills this agent actually loaded through the Skill tool. Never
    /// inferred from plan prose or files it happened to read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loaded_skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_verdict: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry_reason: Option<String>,
    #[serde(default)]
    pub carried: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FindingState {
    Queued,
    Fixing,
    Verifying,
    Fixed,
    Blocked,
    Escalated,
}

/// How broadly a finding/pass touches the tree, taken from the validator's
/// structured `risk` output — evidence, not a hardcoded "always critical" path
/// list (safety rule #6 / invalidation policy §9). Drives how broadly a fix
/// invalidates carried pass receipts: a `Global` fix invalidates everything, a
/// `Shared` fix invalidates other shared/global passes, a `Local` fix only
/// invalidates passes whose coverage the change directly overlaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Risk {
    #[default]
    Local,
    Shared,
    Global,
}

impl Risk {
    // `&self` is required by serde's `skip_serializing_if`, which only accepts
    // `fn(&T) -> bool`; the by-ref signature is intentional despite `Risk: Copy`.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn is_local(&self) -> bool {
        matches!(self, Risk::Local)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Finding {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affected_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
    #[serde(default, skip_serializing_if = "Risk::is_local")]
    pub risk: Risk,
    pub state: FindingState,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub attempts: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PassReceipt {
    pub item_index: usize,
    pub receipt_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub coverage: String,
    #[serde(default, skip_serializing_if = "Risk::is_local")]
    pub risk: Risk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SemanticVerdict {
    Pass(PassReceipt),
    Finding(Finding),
    Unknown,
    Invalid(String),
}

pub(super) const STATUS_COMPLETED: &str = "completed";
pub(super) const STATUS_FAILED: &str = "failed";
pub(super) const STATUS_STILL_RUNNING: &str = "still_running";
pub(super) const STATUS_STOPPED: &str = "stopped";

/// Terminal error stamped when a live agent makes no task progress for the
/// configured phase inactivity window. The timeout retry pass keys on this
/// exact marker; active work resets the window instead of being cancelled.
pub(crate) const PHASE_TIMEOUT_STOP_ERROR: &str =
    "agent exceeded workflow phase timeout due to inactivity and was stopped";
/// Absolute live-phase safety cap. Unlike inactivity, this does not earn an
/// automatic retry because starting the same long phase over would repeat the
/// failure and discard more work.
pub(crate) const PHASE_HARD_TIMEOUT_STOP_ERROR: &str =
    "agent exceeded workflow phase hard timeout and was stopped";
/// A separate startup failure class: the provider/agent remained alive but no
/// task action appeared before the effort-aware first-action deadline. Keeping
/// this distinct from the 20-minute phase cap lets recovery exclude a stalled
/// provider without globally truncating legitimate long-running work.
pub(crate) const STARTUP_NO_PROGRESS_STOP_ERROR: &str =
    "agent made no task progress before the workflow startup watchdog deadline";

#[derive(Debug, Clone, Copy)]
pub(crate) struct StartupWatchdogPolicy {
    pub(crate) first_action_timeout: Duration,
    pub(crate) reasoning_extension: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupWatchdogDecision {
    Continue,
    ExtendOnce,
    Stop,
}

/// Pure startup decision so the 4m + one 4m extension contract can be tested
/// without sleeping. Transport/quiet fields intentionally do not participate:
/// only a decoded reasoning delta can earn the one extension, and any actual
/// task action disables the startup watchdog for the rest of the phase.
pub(crate) fn startup_watchdog_decision(
    snapshot: &AgentActivitySnapshot,
    now_epoch_secs: u64,
    extension_used: bool,
    policy: StartupWatchdogPolicy,
) -> StartupWatchdogDecision {
    if snapshot.first_task_action_at.is_some() {
        return StartupWatchdogDecision::Continue;
    }
    let Some(anchor) = snapshot.stream_open_at.or(snapshot.started_at) else {
        return StartupWatchdogDecision::Continue;
    };
    let extension = if extension_used {
        policy.reasoning_extension.as_secs()
    } else {
        0
    };
    let deadline = anchor
        .saturating_add(policy.first_action_timeout.as_secs())
        .saturating_add(extension);
    if now_epoch_secs < deadline {
        return StartupWatchdogDecision::Continue;
    }
    if !extension_used {
        let deep_effort = matches!(
            snapshot.effective_effort.as_deref(),
            Some("xhigh" | "max" | "ultra")
        );
        let reasoning_is_eligible = snapshot.last_reasoning_at.is_some_and(|at| {
            at >= anchor
                && at <= now_epoch_secs
                && (deep_effort
                    || now_epoch_secs.saturating_sub(at)
                        <= policy.first_action_timeout.as_secs())
        });
        if reasoning_is_eligible {
            return StartupWatchdogDecision::ExtendOnce;
        }
    }
    StartupWatchdogDecision::Stop
}

/// Whether a live agent has made no task progress for the configured phase
/// inactivity window. Transport keep-alives and reasoning deltas deliberately
/// do not reset this clock; only tool/output progress recorded in the manifest
/// does. The phase start is a floor so stale manifest timestamps cannot cause an
/// immediate stop when a barrier begins.
pub(crate) fn phase_inactivity_exceeded(
    snapshot: &AgentActivitySnapshot,
    phase_started_at: u64,
    now_epoch_secs: u64,
    inactivity_timeout: Duration,
) -> bool {
    if snapshot.current_tool.is_some() {
        return false;
    }
    let last_progress = snapshot
        .last_task_progress_at
        .or(snapshot.first_task_action_at)
        .unwrap_or(phase_started_at)
        .max(phase_started_at)
        .max(snapshot.started_at.unwrap_or(phase_started_at));
    now_epoch_secs.saturating_sub(last_progress) >= inactivity_timeout.as_secs().max(1)
}

/// Production startup watchdog, with narrow operational overrides for shadow
/// benchmarks. `ZO_WORKFLOW_STARTUP_WATCHDOG=off` is the rollback switch;
/// positive per-window values override the default 4m + 4m contract.
pub(crate) fn startup_watchdog_policy_from_env() -> Option<StartupWatchdogPolicy> {
    if std::env::var("ZO_WORKFLOW_STARTUP_WATCHDOG").is_ok_and(|value| {
        let value = value.trim();
        value.eq_ignore_ascii_case("off") || value == "0"
    }) {
        return None;
    }
    Some(StartupWatchdogPolicy {
        first_action_timeout: positive_duration_env(
            "ZO_WORKFLOW_FIRST_ACTION_TIMEOUT_SECS",
            DEFAULT_FIRST_ACTION_TIMEOUT,
        ),
        reasoning_extension: positive_duration_env(
            "ZO_WORKFLOW_REASONING_EXTENSION_SECS",
            DEFAULT_REASONING_EXTENSION,
        ),
    })
}

fn positive_duration_env(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map_or(default, Duration::from_secs)
}

/// Production phase-inactivity override: `ZO_WORKFLOW_PHASE_TIMEOUT_SECS`
/// replaces the 20-minute [`DEFAULT_PHASE_TIMEOUT`]. Live agents reset this
/// window with task progress; simple/test backends retain their bounded barrier
/// behavior.
fn phase_timeout_from_env() -> Duration {
    positive_duration_env("ZO_WORKFLOW_PHASE_TIMEOUT_SECS", DEFAULT_PHASE_TIMEOUT)
}

/// Absolute live-phase safety cap. It is intentionally separate from the
/// progress-resetting inactivity window, and never shorter than that window.
pub(crate) fn phase_hard_timeout_from_env(inactivity_timeout: Duration) -> Duration {
    positive_duration_env(
        "ZO_WORKFLOW_PHASE_HARD_TIMEOUT_SECS",
        DEFAULT_PHASE_HARD_TIMEOUT,
    )
    .max(inactivity_timeout)
}

/// `"$input"` fan-out sentinel: expand the workflow input (when an array) into
/// one item per element.
const INPUT_SENTINEL: &str = "$input";

/// Per-item cap (bytes) applied to each result fed into `synthesize`'s `{all}`,
/// so a large fan-out cannot blow up the synthesis prompt.
const SYNTH_ITEM_CAP_BYTES: usize = 2048;

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Run a validated workflow, dispatching on its execution mode. Infallible: a
/// spec is validated before it reaches here, and every per-agent failure is
/// captured in the report (never propagated), so there is nothing left to error.
pub(crate) fn run(
    workflow: &NormalizedWorkflow,
    input: &Value,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
) -> WorkflowReport {
    // Emit the full phase skeleton up-front so a viewer can draw the tree before
    // any agent spawns; the matching `Finished` follows once the report is
    // assembled. Both are no-ops when no sink is attached.
    if let Some(sink) = opts.progress {
        let skeleton: Vec<PhaseSkeleton> = workflow
            .phases
            .iter()
            .map(|phase| PhaseSkeleton {
                id: phase.id.clone(),
                kind: phase_kind(&phase.source),
            })
            .collect();
        sink.emit(ProgressEvent::Started {
            name: &workflow.name,
            description: &workflow.description,
            mode: workflow_mode_str(workflow.mode),
            phases: &skeleton,
        });
    }
    let report = match workflow.mode {
        WorkflowMode::Phases => run_phases(workflow, input, backend, opts),
        WorkflowMode::Pipeline => run_pipeline(workflow, input, backend, opts),
    };
    if let Some(sink) = opts.progress {
        sink.emit(ProgressEvent::Finished {
            status: &report.status,
        });
    }
    report
}

/// Workflow-tree label for a phase source.
fn phase_kind(source: &PhaseSource) -> &'static str {
    match source {
        PhaseSource::Fanout(_) => "fanout",
        PhaseSource::Over(_) => "over",
        PhaseSource::Single => "single",
    }
}

fn workflow_mode_str(mode: WorkflowMode) -> &'static str {
    match mode {
        WorkflowMode::Phases => "phases",
        WorkflowMode::Pipeline => "pipeline",
    }
}

/// Tally terminal item statuses for a `PhaseDone` progress event.
fn count_statuses(items: &[ItemResult]) -> (usize, usize, usize) {
    let mut completed = 0;
    let mut failed = 0;
    let mut still_running = 0;
    for item in items {
        match item.status.as_str() {
            STATUS_COMPLETED => completed += 1,
            STATUS_FAILED | STATUS_STOPPED => failed += 1,
            STATUS_STILL_RUNNING => still_running += 1,
            _ => {}
        }
    }
    (completed, failed, still_running)
}

fn emit_phase_done(sink: &dyn ProgressSink, phase_id: &str, report: &PhaseReport) {
    let (completed, failed, still_running) = count_statuses(&report.items);
    sink.emit(ProgressEvent::PhaseDone {
        id: phase_id,
        completed,
        failed,
        still_running,
        carried: report.carried_pass_count,
        retried: report.retried_finding_count,
        skipped: report.skipped_count,
    });
}

/// BUG-R6: a `$input` fan-out over a null/empty input expands to zero agents.
/// Without a note the run reports "completed" with no work done, which reads as
/// success — surface it. `None` when work ran or the run was a pure cache replay.
fn zero_work_note(resumed: usize, reports: &[PhaseReport]) -> Option<String> {
    (resumed == 0 && !reports.is_empty() && reports.iter().all(|report| report.items.is_empty()))
        .then(|| {
            "workflow ran no agents — the input expanded to zero work (empty or null `$input`?)"
                .to_string()
        })
}

/// Whether a finished phase still has unfinished (`still_running`) stragglers,
/// in which case it must not be cached as complete (BUG-R4).
fn phase_has_stragglers(report: &PhaseReport) -> bool {
    report
        .items
        .iter()
        .any(|item| item.status == STATUS_STILL_RUNNING || item.status == STATUS_STOPPED)
}

/// What to do with a phase that has a cache entry on resume.
enum ReplayDecision<'a> {
    /// Replay the cached report — already complete, so no spawn.
    Replay(&'a PhaseReport),
    /// Re-run the phase. `stale_green` distinguishes a `command_green` cache that
    /// failed revalidation from a plain cache miss / id mismatch.
    Rerun { stale_green: bool },
}

/// Decide whether a phase can be replayed from the resume cache. A
/// `command_green` phase is revalidated against the *current* tree first, because
/// the run id keys only (spec, input) — a cached green can be stale (BUG-R5).
fn replay_decision<'a>(
    cached_prefix: &'a [PhaseReport],
    idx: usize,
    phase: &NormalizedPhase,
    opts: &RunOptions,
) -> ReplayDecision<'a> {
    let Some(cached) = cached_prefix
        .get(idx)
        .filter(|report| report.id == phase.id)
    else {
        return ReplayDecision::Rerun { stale_green: false };
    };
    let stale_green = matches!(
        phase.repeat.as_ref(),
        Some(RepeatPolicy { until: Until::CommandGreen { command }, .. })
            if opts.check.is_none_or(|check| check(command) != 0)
    );
    if stale_green {
        ReplayDecision::Rerun { stale_green: true }
    } else {
        ReplayDecision::Replay(cached)
    }
}

/// `phases` mode: run each phase behind a barrier, feeding every completed
/// phase's items into the `prior` map so a later phase can map `over` them.
#[allow(clippy::too_many_lines)]
fn run_phases(
    workflow: &NormalizedWorkflow,
    input: &Value,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
) -> WorkflowReport {
    let input_text = value_to_prompt_string(input);
    let mut state = EngineState::new(
        workflow.max_agents,
        workflow.max_output_tokens,
        workflow.max_cost_usd,
        backend.output_price_per_token(),
    );
    let mut prior: HashMap<String, Vec<ItemResult>> = HashMap::new();
    let mut reports: Vec<PhaseReport> = Vec::with_capacity(workflow.phases.len());
    let mut notes: Vec<String> = Vec::new();
    let mut cancelled = false;
    let iso = resolve_isolation(
        workflow.isolation,
        workflow.apply,
        opts.worktree,
        &mut notes,
    );

    // Resume cache: replay any completed-phase prefix for this exact spec+input
    // instead of re-spawning it. An absent / unreadable / mismatched cache
    // yields no prefix, so this is a no-op when caching is disabled.
    let cached_prefix = opts.cache.and_then(WorkflowCache::load).unwrap_or_default();
    let mut resumed = 0usize;
    // Once a cached phase is missing or fails revalidation, stop replaying: the
    // resumable prefix is contiguous from the start, and a re-run invalidates any
    // later cached phase that might depend on it.
    let mut replay_ok = true;
    // Only the contiguous fully-complete prefix is cacheable. A phase left with
    // `still_running` stragglers must not be stored as if finished, or a resume
    // would replay it complete and skip the unfinished work (BUG-R4).
    let mut cacheable = true;

    for (idx, phase) in workflow.phases.iter().enumerate() {
        // Cache hit: replay the cached phase — no spawn, no budget, and no
        // cancellation effect, since it is already complete. Position *and* id
        // must match (belt-and-suspenders against a run_id collision).
        if replay_ok {
            match replay_decision(&cached_prefix, idx, phase, opts) {
                ReplayDecision::Replay(cached) => {
                    prior.insert(phase.id.clone(), cached.items.clone());
                    reports.push(cached.clone());
                    resumed += 1;
                    if let Some(sink) = opts.progress {
                        sink.emit(ProgressEvent::PhaseResumed { id: &phase.id });
                    }
                    continue;
                }
                ReplayDecision::Rerun { stale_green } => {
                    replay_ok = false;
                    if stale_green {
                        notes.push(format!(
                            "phase `{}` was cached green but its command_green check no longer \
                             passes; re-running it",
                            phase.id
                        ));
                    }
                }
            }
        }
        if is_cancelled(opts.cancel) {
            cancelled = true;
            notes.push(format!(
                "cancelled before phase `{}` — completed phases kept, no further agents spawned",
                phase.id
            ));
            break;
        }
        // Stop before a phase we cannot afford even one agent for (or whose prior
        // phases already blew the output-token budget), so a cut-off skips the
        // phase entirely instead of recording an empty one.
        if let Some(reason) = state.exhaustion_reason() {
            state.mark_exhausted();
            notes.push(format!(
                "{reason}; phase `{}` and any later phases skipped",
                phase.id
            ));
            break;
        }
        let report = run_phase(
            phase,
            input,
            &input_text,
            &prior,
            backend,
            opts,
            &mut state,
            iso,
        );
        if let Some(sink) = opts.progress {
            emit_phase_done(sink, &phase.id, &report);
        }
        // BUG-R4: once a phase carries unfinished stragglers, stop extending the
        // cache so the stored prefix is only ever fully-complete phases.
        cacheable &= !phase_has_stragglers(&report);
        // A phase whose every agent failed cannot feed the phases after it —
        // running them anyway verifies/synthesizes against work that never
        // happened (the live bug: `implement` failed on a provider error and
        // `verify` still spawned, reading green against nothing). Mirror the
        // budget gate: record the phase, skip the rest, and let `finalize`
        // report the run as `failed`.
        let phase_produced_nothing = !report.items.is_empty()
            && report
                .items
                .iter()
                .all(|item| item.status != STATUS_COMPLETED);
        prior.insert(phase.id.clone(), report.items.clone());
        let failed_phase_id = phase_produced_nothing.then(|| phase.id.clone());
        reports.push(report);
        if let Some(id) = failed_phase_id {
            notes.push(format!(
                "phase `{id}` produced no completed item; later phases skipped"
            ));
            break;
        }
        // Persist the completed-phase prefix so a later run resumes from here.
        if cacheable {
            if let Some(cache) = opts.cache {
                cache.store(&reports);
            }
        }
    }

    if resumed > 0 {
        notes.push(format!(
            "resumed {resumed} phase(s) from cache (no agents spawned for them)"
        ));
    }
    if let Some(note) = zero_work_note(resumed, &reports) {
        notes.push(note);
    }

    finalize(
        workflow, reports, notes, cancelled, backend, opts, &mut state,
    )
}

/// `pipeline` mode: the first phase's fan-out items each become an independent
/// chain threaded through every later phase as a stage. Stage k of a chain
/// receives that chain's stage (k-1) result as `{item}` — 1:1 identity
/// threading (the chain's original index is preserved), not a re-fan-out. A
/// chain whose stage fails or times out retires; its siblings keep flowing.
///
/// Scheduling is a single-threaded round-robin interleave: each stage advances
/// every live chain one step behind a single batch wait. Genuine wall-clock
/// stage skew (chain A at stage 3 while B is at stage 1) would need per-chain
/// threads, but `AgentBackend::spawn` is `&mut` (so it would need
/// synchronization) and the skew would not survive `wait_for_agent_completions`'s
/// batch barrier anyway. What `pipeline` adds over `phases` + `over` is the
/// per-item chain model — a preserved `{index}` and independent per-chain
/// failure; real concurrency stays bounded by the shared `agent_api_semaphore`,
/// exactly as `phases`. A later stage's declared `fanout`/`over` is ignored:
/// the chain supplies its `{item}`.
#[allow(clippy::too_many_lines)]
fn run_pipeline(
    workflow: &NormalizedWorkflow,
    input: &Value,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
) -> WorkflowReport {
    let input_text = value_to_prompt_string(input);
    let mut state = EngineState::new(
        workflow.max_agents,
        workflow.max_output_tokens,
        workflow.max_cost_usd,
        backend.output_price_per_token(),
    );
    let mut notes: Vec<String> = Vec::new();
    let mut cancelled = false;
    let iso = resolve_isolation(
        workflow.isolation,
        workflow.apply,
        opts.worktree,
        &mut notes,
    );

    // The first phase resolves the items to stream; later phases are stages.
    // (validate guarantees ≥1 phase, and that phase 0 is `fanout`/`single` —
    // `over` on a first phase is impossible, there is no earlier phase.)
    let stages = &workflow.phases;
    let base_items = resolve_items(&stages[0].source, input, &HashMap::new());

    // `carried[i]` holds chain i's input for its next stage, or `None` once the
    // chain retires (failed, timed out, or dropped by a budget clamp). The
    // chain index is the original item index, kept stable across every stage.
    let mut carried: Vec<Option<String>> = base_items.into_iter().map(Some).collect();
    let mut reports: Vec<PhaseReport> = Vec::with_capacity(stages.len());

    for stage in stages {
        if is_cancelled(opts.cancel) {
            cancelled = true;
            notes.push(format!(
                "cancelled before stage `{}` — completed stages kept, no further agents spawned",
                stage.id
            ));
            break;
        }

        // Chains live at this stage's entry (the full set, before any clamp).
        let entry: Vec<(usize, String)> = carried
            .iter()
            .enumerate()
            .filter_map(|(idx, slot)| slot.clone().map(|text| (idx, text)))
            .collect();
        if entry.is_empty() {
            break;
        }
        if let Some(reason) = state.exhaustion_reason() {
            state.mark_exhausted();
            notes.push(format!(
                "{reason}; stage `{}` and any later stages skipped",
                stage.id
            ));
            break;
        }

        // Clamp the live chains to the remaining agent budget; dropped chains
        // retire. (The token cap is post-hoc, so it gates stage entry above
        // rather than clamping a stage's chain count here.)
        let mut live = entry.clone();
        if let Some(remaining) = state.remaining() {
            if live.len() > remaining {
                let dropped = live.len() - remaining;
                live.truncate(remaining);
                state.mark_exhausted();
                notes.push(format!(
                    "budget exhausted (max_agents={}); {dropped} chain(s) dropped at stage `{}`",
                    workflow.max_agents.unwrap_or_default(),
                    stage.id
                ));
            }
        }

        if let Some(sink) = opts.progress {
            sink.emit(ProgressEvent::PhaseEnter {
                id: &stage.id,
                round: 1,
            });
        }

        // One agent per live chain, the chain's index carried as the item index.
        let units: Vec<SpawnUnit> = live
            .iter()
            .map(|(idx, text)| SpawnUnit {
                index: *idx,
                item: text.clone(),
                prior_failures: 0,
                prompt: render_prompt(
                    &stage.prompt,
                    text,
                    *idx,
                    &input_text,
                    "",
                    stage.schema.as_ref(),
                ),
            })
            .collect();
        let tokens_before = state.output_tokens_spent;
        let items = spawn_and_collect(stage, &input_text, units, backend, opts, &mut state, iso);

        let report = PhaseReport {
            id: stage.id.clone(),
            rounds: 1,
            output_tokens: state.output_tokens_spent.saturating_sub(tokens_before),
            // A pipeline stage runs once; `{seen}` is a repeat-phase relay.
            relay_bytes: 0,
            items,
            carried_pass_count: 0,
            retried_finding_count: 0,
            skipped_count: 0,
            blocked_finding_count: 0,
            escalated_finding_count: 0,
            findings: Vec::new(),
            pass_receipts: Vec::new(),
        };

        if let Some(sink) = opts.progress {
            emit_phase_done(sink, &stage.id, &report);
        }

        // Route: every chain that entered this stage retires unless it completed
        // and so can feed the next stage its result as `{item}`.
        for (idx, _) in &entry {
            carried[*idx] = None;
        }
        for item in &report.items {
            if item.status == STATUS_COMPLETED {
                carried[item.index] = Some(item_text_for_mapping(item));
            }
        }
        reports.push(report);
    }

    finalize(
        workflow, reports, notes, cancelled, backend, opts, &mut state,
    )
}

/// Shared tail for both modes: run the optional synthesis agent, append honest
/// outcome notes, decide the run status, and assemble the report.
fn finalize(
    workflow: &NormalizedWorkflow,
    reports: Vec<PhaseReport>,
    mut notes: Vec<String>,
    cancelled: bool,
    backend: &mut dyn AgentBackend,
    opts: &RunOptions,
    state: &mut EngineState,
) -> WorkflowReport {
    let synthesis = match workflow.synthesize.as_ref().filter(|_| !cancelled) {
        None => None,
        Some(synth) => {
            if let Some(reason) = state.exhaustion_reason() {
                // Skipping a declared deliverable is a budget cut-off, not a
                // clean finish — mark exhausted so `status` reads
                // `budget_exhausted` (matching the phase/stage gates), never a
                // misleading `completed` with the synthesis silently missing.
                state.mark_exhausted();
                notes.push(format!("synthesize skipped — {reason}"));
                None
            } else {
                if let Some(sink) = opts.progress {
                    sink.emit(ProgressEvent::SynthesizeEnter);
                }
                run_synthesize(synth, &reports, backend, opts, state)
            }
        }
    };

    let judgement = match workflow.judge.as_ref().filter(|_| !cancelled) {
        None => None,
        Some(judge) => {
            if let Some(reason) = state.exhaustion_reason() {
                // Same as synthesize: a budget-skipped judge means the run did
                // not finish all declared work, so mark it exhausted.
                state.mark_exhausted();
                notes.push(format!("judge skipped — {reason}"));
                None
            } else {
                run_judge(judge, &reports, backend, opts, state)
            }
        }
    };

    if let Some(reason) = state.spawn_block {
        notes.push(reason.to_string());
    }
    append_outcome_notes(&reports, &mut notes);

    // Honest budget caveat: a still_running straggler reports 0 output tokens
    // (its real spend lands after the phase barrier and is never re-collected),
    // so under a token budget the tally under-counts when agents overran the
    // phase timeout. Surface the gap rather than letting it read as exact.
    if state.max_output_tokens.is_some()
        && reports
            .iter()
            .flat_map(|phase| &phase.items)
            .any(|item| item.status == STATUS_STILL_RUNNING || item.status == STATUS_STOPPED)
    {
        notes.push(
            "output-token budget under-counts: timed-out agents' tokens are not charged against max_output_tokens (their spend is not captured after the phase timeout)".to_string(),
        );
    }

    if state.timeout_retries > 0 {
        notes.push(format!(
            "{} agent(s) hit the phase timeout or startup watchdog and were automatically retried once; {} recovered. After two startup-no-progress stops, an unpinned route may use one bounded alternate-provider fallback. If recovery keeps failing, split the phase into narrower steps or inspect the recorded provider activity.",
            state.timeout_retries, state.timeout_retry_recoveries
        ));
    }
    if state.worktree_fallbacks > 0 {
        notes.push(format!(
            "{} agent(s) ran without worktree isolation (worktree creation failed); their file changes were not isolated",
            state.worktree_fallbacks
        ));
    }
    notes.append(&mut state.worktree_warnings);
    if state.patches_applied > 0 {
        notes.push(format!(
            "merged {} agent change-set(s) back into the working tree (git apply --3way)",
            state.patches_applied
        ));
    }
    if !state.apply_failures.is_empty() {
        notes.push(format!(
            "{} agent change-set(s) did not merge cleanly (left for manual resolution): {}",
            state.apply_failures.len(),
            state.apply_failures.join("; ")
        ));
    }
    notes.append(&mut state.command_green_notes);

    let gate_failed = verification_gate_failed(&reports, &mut notes);

    let status = final_status(
        &reports, cancelled || is_cancelled(opts.cancel), gate_failed, state,
    );

    if state.unmetered_agents > 0 {
        notes.push(format!("request-level usage unavailable for {} agent(s); their cost is unverified", state.unmetered_agents));
    }
    if let Some(cost) = &state.usage_cost {
        if reports.iter().flat_map(|phase| &phase.items).any(|item| item.status == STATUS_STOPPED) {
            notes.push("cost is a snapshot at logical stop; usage arriving before physical worker exit may not be included".to_string());
        }
        notes.push(format!(
            "cost is an observed-usage estimate, not a billing receipt or hard spending cap; {} request(s) have unreported usage and {} have unknown pricing",
            cost.unreported_requests, cost.unpriced_requests
        ));
    }
    WorkflowReport {
        name: workflow.name.clone(),
        description: workflow.description.clone(),
        status: status.to_string(),
        phases: reports,
        synthesis,
        judgement,
        agents_spawned: state.agents_spawned,
        budget_exhausted: state.budget_exhausted,
        output_tokens: state.output_tokens_spent,
        cost_usd: state.cost_usd_spent,
        usage_cost: state.usage_cost.clone(),
        remaining_output_tokens: state.remaining_output_tokens(),
        remaining_cost_usd: state.remaining_cost_usd(),
        notes,
    }
}

fn final_status(
    reports: &[PhaseReport],
    cancelled: bool,
    gate_failed: bool,
    state: &EngineState,
) -> &'static str {
    // Both modes halt when their last phase has no completed item.
    let halted_on_failure = reports.last().is_some_and(|last| {
        !last.items.is_empty()
            && last.items.iter().all(|item| item.status != STATUS_COMPLETED)
    });
    if cancelled {
        "cancelled"
    } else if state.budget_exhausted {
        "budget_exhausted"
    } else if halted_on_failure || gate_failed || state.spawn_block.is_some() {
        "failed"
    } else {
        "completed"
    }
}

/// Record the hard verification gate's outcome and report whether it failed. A
/// `fix_until_verified` phase ending with a blocked finding did not deliver
/// verified work — either a repair the loop could not verify, or a red
/// `final_check` — so that gate decides the RUN status too: otherwise every
/// agent merely completing reads as success while the command is still failing.
fn verification_gate_failed(reports: &[PhaseReport], notes: &mut Vec<String>) -> bool {
    let blocked: Vec<&str> = reports
        .iter()
        .filter(|phase| phase.blocked_finding_count > 0)
        .map(|phase| phase.id.as_str())
        .collect();
    if blocked.is_empty() {
        return false;
    }
    notes.push(format!(
        "verification gate failed: phase(s) {} ended with blocked finding(s), so the work is not verified",
        blocked.join(", ")
    ));
    true
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

struct EngineState {
    max_agents: Option<usize>,
    agents_spawned: usize,
    /// Cumulative output-token cap (`budget.max_output_tokens`); `None` is
    /// unbounded. Checked post-hoc at work boundaries, never as a pre-spawn clamp.
    max_output_tokens: Option<u64>,
    /// Output tokens every collected agent has reported so far this run.
    output_tokens_spent: u64,
    /// Estimated USD output-cost cap (`budget.max_cost_usd`); `None` is unbounded.
    max_cost_usd: Option<f64>,
    /// Per-output-token USD price of the active model, used to derive cost from
    /// the token tally. `0.0` when unknown — the cost budget then never trips.
    output_price_per_token: f64,
    /// Estimated cumulative output cost (USD) this run.
    cost_usd_spent: f64,
    usage_cost: Option<crate::misc_tools::AgentUsageCost>,
    unmetered_agents: usize,
    budget_exhausted: bool,
    spawn_block: Option<&'static str>,
    /// Agents that asked for worktree isolation but whose worktree could not be
    /// created (degraded to no isolation). Surfaced as an honest note.
    worktree_fallbacks: usize,
    /// Distinct low-disk warnings captured at worktree creation.
    worktree_warnings: Vec<String>,
    /// Isolated change-sets merged back into the main tree (`apply:"sequential"`).
    patches_applied: usize,
    /// Change-sets that could not be merged cleanly, with a per-agent reason —
    /// surfaced verbatim so the user knows exactly what to resolve by hand.
    apply_failures: Vec<String>,
    /// One note per `command_green` repeat phase: whether the verification
    /// command passed and at which round — so the report is honest about a
    /// green stop vs an exhausted loop.
    command_green_notes: Vec<String>,
    /// Agents force-stopped at the phase timeout that were automatically
    /// retried once (the "plan agent timed out → whole workflow cancelled"
    /// recovery). Surfaced as an honest note in `finalize`.
    timeout_retries: usize,
    /// How many of those retries recovered to `completed`.
    timeout_retry_recoveries: usize,
}

impl EngineState {
    fn new(
        max_agents: Option<usize>,
        max_output_tokens: Option<u64>,
        max_cost_usd: Option<f64>,
        output_price_per_token: f64,
    ) -> Self {
        Self {
            max_agents,
            agents_spawned: 0,
            max_output_tokens,
            output_tokens_spent: 0,
            max_cost_usd,
            output_price_per_token,
            cost_usd_spent: 0.0,
            usage_cost: None,
            unmetered_agents: 0,
            budget_exhausted: false,
            spawn_block: None,
            worktree_fallbacks: 0,
            worktree_warnings: Vec::new(),
            patches_applied: 0,
            apply_failures: Vec::new(),
            command_green_notes: Vec::new(),
            timeout_retries: 0,
            timeout_retry_recoveries: 0,
        }
    }

    /// Spawns still permitted (`None` = unlimited).
    fn remaining(&self) -> Option<usize> {
        self.max_agents
            .map(|max| max.saturating_sub(self.agents_spawned))
    }

    fn budget_would_exceed(&self, additional: usize) -> bool {
        self.remaining()
            .is_some_and(|remaining| remaining < additional)
    }

    /// Fold one collected agent's reported output tokens into the running tally.
    /// Saturating so a pathological sum can never wrap and silently reopen a
    /// budget that has already been blown.
    fn record_usage(
        &mut self,
        tokens: u64,
        usage_cost: Option<crate::misc_tools::AgentUsageCost>,
    ) {
        self.output_tokens_spent = self.output_tokens_spent.saturating_add(tokens);
        if let Some(cost) = usage_cost {
            self.cost_usd_spent += cost.estimated_cost_usd;
            self.usage_cost.get_or_insert_with(Default::default).accumulate(&cost);
        } else {
            self.unmetered_agents = self.unmetered_agents.saturating_add(1);
            #[allow(clippy::cast_precision_loss)]
            let token_cost = tokens as f64 * self.output_price_per_token;
            self.cost_usd_spent += token_cost;
        }
    }

    /// Whether the cumulative output-token spend has reached the cap. Post-hoc:
    /// a spawn's token cost is only known once it streams, so this gates the
    /// *next* unit of work, not the spawn that pushed it over.
    fn output_tokens_exhausted(&self) -> bool {
        self.max_output_tokens
            .is_some_and(|max| self.output_tokens_spent >= max)
    }

    /// Whether the estimated cumulative cost has reached the `max_cost_usd` cap.
    /// Post-hoc, mirroring [`EngineState::output_tokens_exhausted`].
    fn cost_is_incomplete(&self) -> bool {
        (self.unmetered_agents > 0 && self.output_price_per_token == 0.0)
            || self.usage_cost.as_ref().is_some_and(|cost| {
                cost.unreported_requests > 0 || cost.unpriced_requests > 0
            })
    }

    fn cost_exhausted(&self) -> bool {
        self.max_cost_usd
            .is_some_and(|max| self.cost_usd_spent >= max || self.cost_is_incomplete())
    }

    /// Output tokens still available before the `max_output_tokens` cap, if any.
    fn remaining_output_tokens(&self) -> Option<u64> {
        self.max_output_tokens
            .map(|max| max.saturating_sub(self.output_tokens_spent))
    }

    /// Estimated USD still available before the `max_cost_usd` cap, if any.
    fn remaining_cost_usd(&self) -> Option<f64> {
        if self.cost_is_incomplete() {
            return None;
        }
        self.max_cost_usd
            .map(|max| (max - self.cost_usd_spent).max(0.0))
    }

    /// The cheap "may we start another agent right now?" predicate — either cap
    /// can forbid it. Pair with [`EngineState::exhaustion_reason`] when the caller
    /// needs an honest note explaining which limit tripped.
    fn budget_blocks_spawn(&self) -> bool {
        self.spawn_block.is_some()
            || self.output_tokens_exhausted() || self.cost_exhausted() || self.budget_would_exceed(1)
    }

    /// The honest reason the budget forbids more work, or `None` when work may
    /// proceed. Reports the post-hoc token cap before the agent-count cap; the
    /// two are mutually exhaustive with [`EngineState::budget_blocks_spawn`].
    fn exhaustion_reason(&self) -> Option<String> {
        if let Some(reason) = self.spawn_block {
            Some(reason.to_string())
        } else if self.output_tokens_exhausted() {
            Some(format!(
                "output-token budget exhausted (max_output_tokens={}, spent={})",
                self.max_output_tokens.unwrap_or_default(),
                self.output_tokens_spent
            ))
        } else if self.max_cost_usd.is_some_and(|max| self.cost_usd_spent < max)
            && self.cost_is_incomplete()
        {
            Some("cost budget blocked: request usage or model pricing is unverified".to_string())
        } else if self.cost_exhausted() {
            Some(format!(
                "cost budget exhausted (max_cost_usd={:.4}, spent={:.4})",
                self.max_cost_usd.unwrap_or_default(),
                self.cost_usd_spent
            ))
        } else if self.budget_would_exceed(1) {
            Some(format!(
                "agent budget exhausted (max_agents={})",
                self.max_agents.unwrap_or_default()
            ))
        } else {
            None
        }
    }

    fn record_spawn(&mut self) {
        self.agents_spawned += 1;
    }

    fn record_worktree_fallback(&mut self) {
        self.worktree_fallbacks += 1;
    }

    fn record_worktree_warning(&mut self, warning: &str) {
        if !self.worktree_warnings.iter().any(|item| item == warning) {
            self.worktree_warnings.push(warning.to_string());
        }
    }

    fn record_timeout_retry(&mut self, recovered: bool) {
        self.timeout_retries += 1;
        if recovered {
            self.timeout_retry_recoveries += 1;
        }
    }

    fn record_patch_applied(&mut self) {
        self.patches_applied += 1;
    }

    fn record_apply_failure(&mut self, reason: String) {
        self.apply_failures.push(reason);
    }

    fn record_command_green(&mut self, phase_id: &str, command: &str, rounds: u32, passed: bool) {
        self.command_green_notes.push(if passed {
            format!(
                "phase `{phase_id}`: verification command `{command}` passed at round {rounds}"
            )
        } else {
            format!(
                "phase `{phase_id}`: verification command `{command}` did not pass within {rounds} round(s)"
            )
        });
    }

    fn mark_exhausted(&mut self) {
        if self.spawn_block.is_none() {
            self.budget_exhausted = true;
        }
    }
}

#[cfg(test)]
mod tests;
