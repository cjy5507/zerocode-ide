//! Runtime configuration, builders, and simple accessors for
//! [`ConversationRuntime`] — installation setters, permission/session accessors,
//! compaction thresholds, and the lifecycle/tool hook runners. Split out of
//! `mod.rs` so the turn loops there read as orchestration. Behaviour-preserving:
//! these were `ConversationRuntime` methods, now `pub(super)` where the loops in
//! `mod.rs` (and siblings/tests) still reach the private ones.

use std::sync::Arc;

use serde_json::{json, Value};
use telemetry::SessionTracer;

use crate::hooks::HookProgressEvent;

use super::{
    auto_compaction_threshold_from_env_or_policy, estimate_session_tokens,
    is_long_running, lifecycle_hook_outcome, trace_attrs, AgentNotification,
    AgentNotificationInbox, ApiClient, AsyncApiClient, CompactionConfig, CompactionResult, ConcurrentDispatchFn, ContextPolicy, ConversationRuntime,
    HookAbortSignal, HookEvent, HookProgressReporter, HookRunResult, LongRunningPredicate,
    MemoryRetriever, PermissionMode, PermissionPolicy, RuntimeError, Session, SteeringQueue,
    TemporaryAllowGrant, ToolExecutor, UsageTracker,
    FALLBACK_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD,
};
use crate::tool_cancel::ToolCancelSignal;

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Cloneable handle to the mid-turn steering queue. The TUI command pump
    /// pushes user-typed steering messages here while a streaming turn is in
    /// flight; the turn drains them at each tool-result boundary. Pushing when
    /// no turn is active is harmless — the next turn drains them on its first
    /// tool-result step.
    #[must_use]
    pub fn steering_handle(&self) -> SteeringQueue {
        Arc::clone(&self.steering)
    }

    /// Replace the runtime's steering queue with a caller-provided one, so a
    /// host can hold the delivery handle BEFORE the runtime exists (and
    /// therefore before [`Self::steering_handle`] could be called). The
    /// sub-agent spawn path registers the queue in a parent-side registry at
    /// spawn time and builds the runtime on a detached thread later; without
    /// this seam a `SendMessage` racing that thread start would have nowhere
    /// to deliver.
    #[must_use]
    pub fn with_steering_queue(mut self, queue: SteeringQueue) -> Self {
        self.steering = queue;
        self
    }

    /// Be told each time a boundary drains steering — the fact a `SendMessage`
    /// receipt turns on. The observer runs on the turn's thread with the
    /// drained texts; it must be cheap and must not touch the queue.
    #[must_use]
    pub fn with_steering_observer(mut self, observer: super::SteeringObserver) -> Self {
        self.steering_observer = Some(observer);
        self
    }

    /// Install (or clear with `None`) the observer told each model switch —
    /// the quota and refusal fallbacks, the overload demotion, a deep-gate
    /// leg's client swap (see [`super::ModelSwitch`]). The host sets-or-clears
    /// it on every turn entry, like the deep clients, so an observer built
    /// for one turn's shadow inputs cannot outlive them. The observer runs on
    /// the turn's thread at the moment of the switch; it must be cheap and
    /// must never fail the turn.
    pub fn set_switch_observer(&mut self, observer: Option<super::SwitchObserver>) {
        self.switch_observer = observer;
    }

    /// Take all pending steering messages, leaving the queue empty (FIFO). A
    /// poisoned lock degrades to "no steering this step" rather than panicking
    /// the turn.
    pub(super) fn drain_steering(&self) -> Vec<String> {
        let drained = self
            .steering
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default();
        if !drained.is_empty() {
            if let Some(observer) = self.steering_observer.as_ref() {
                observer(&drained);
            }
        }
        drained
    }

    /// Fold already-drained steering into the settled tail of the session,
    /// using the same wire shape both existing boundary folds use.
    ///
    /// Two shapes, picked by the role of the last *settled* message:
    ///
    /// * `User` / `Tool` — append the steering as extra `Text` blocks on that
    ///   message. Tool-result messages lower to wire role `user`, so a
    ///   separate user message would be two consecutive user turns; appending
    ///   keeps one valid turn (the judgement recorded at the sync tool-result
    ///   boundary in `mod.rs`, mirrored by the streaming boundary).
    /// * anything else — push a fresh user message. `Assistant` is the
    ///   text-only-boundary case. `System` is the persisted reminder message
    ///   [`Self::absorb_wire_reminders_into_session`] leaves as the tail of an
    ///   in-flight request: appending into it would both bury a user
    ///   instruction inside a host-authored reminder message and break that
    ///   function's dedupe (it matches on exact block equality), so the steer
    ///   gets its own message — the shape the reminder absorb itself already
    ///   puts on the wire after a tool-result message.
    ///
    /// Callers own the transcript echo; this only touches the session.
    pub(super) fn fold_steering_into_settled_tail(
        &mut self,
        steers: &[String],
    ) -> Result<(), RuntimeError> {
        if steers.is_empty() {
            return Ok(());
        }
        let append_in_place = matches!(
            self.session.messages.last().map(|message| message.role),
            Some(crate::session::MessageRole::User | crate::session::MessageRole::Tool)
        );
        if append_in_place {
            let messages = Arc::make_mut(&mut self.session.messages);
            if let Some(last) = messages.last_mut() {
                for steer in steers {
                    last.blocks.push(crate::ContentBlock::Text {
                        text: super::steering_message(steer),
                    });
                }
            }
            self.session.mark_transcript_dirty();
            return Ok(());
        }
        let mut text = String::new();
        for steer in steers {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&super::steering_message(steer));
        }
        self.session
            .push_user_text(text)
            .map_err(|error| RuntimeError::new(error.to_string()))
    }

    /// Cloneable handle to the mid-turn agent-notification inbox. The host's
    /// agent-completion consumer stages finished background agents here while a
    /// turn is in flight; the turn drains them at each tool-result boundary
    /// (same seam as steering) so the main model keeps working through agent
    /// completions instead of ending its turn to receive them. Whatever the
    /// turn never reached a boundary to fold stays in the inbox — the host
    /// drains the leftovers after the turn and re-queues them as follow-up
    /// turns, keeping delivery exactly-once.
    #[must_use]
    pub fn agent_notification_inbox(&self) -> AgentNotificationInbox {
        Arc::clone(&self.agent_notifications)
    }

    /// Take all pending mid-turn agent notifications, leaving the inbox empty
    /// (FIFO). A poisoned lock degrades to "no notifications this step" rather
    /// than panicking the turn.
    pub(super) fn drain_agent_notifications(&self) -> Vec<AgentNotification> {
        self.agent_notifications
            .lock()
            .map(|mut inbox| std::mem::take(&mut *inbox))
            .unwrap_or_default()
    }

    /// Borrow the underlying [`ApiClient`]. Used by the L7c TUI bridge
    /// to clone the live HTTP client when constructing an
    /// [`AsyncApiClient`] adapter for streaming turns.
    #[must_use]
    pub fn api_client(&self) -> &C {
        &self.api_client
    }

    pub fn api_client_mut(&mut self) -> &mut C {
        &mut self.api_client
    }

    /// Install an [`AsyncApiClient`] that drives `run_turn_streaming`'s
    /// upstream request instead of the synchronous
    /// [`ApiClient::stream`]. The synchronous `run_turn` path is
    /// unaffected.
    pub fn set_async_api_client(&mut self, client: Arc<dyn AsyncApiClient>) {
        self.async_api_client = Some(client);
    }

    /// Install (or clear with `None`) one cross-model verifier client used by
    /// the deep gate's VERIFY sub-turns. This compatibility setter installs a
    /// single-entry ranked list; new hosts should use
    /// [`Self::set_deep_verify_candidates`].
    pub fn set_deep_verify_client(&mut self, client: Option<(Arc<dyn AsyncApiClient>, String)>) {
        self.deep_verify_candidates = client.into_iter().collect();
    }

    /// The first cross-model VERIFY client model currently installed by the
    /// host, or `None` when VERIFY remains native.
    #[must_use]
    pub fn deep_verify_model(&self) -> Option<&str> {
        self.deep_verify_candidates
            .first()
            .map(|(_, model)| model.as_str())
    }

    /// Install (or clear with an empty list) the ordered cross-model VERIFY
    /// candidate clients used by the deep gate, top-ranked first. The deep
    /// gate walks this list so a hard-rate-limited candidate can fail over to
    /// the next different-provider candidate. The host rebuilds the list from
    /// Smart Router ranking on every turn entry, so no fallback model is
    /// hardcoded and stale rankings cannot survive a model switch. An empty
    /// list runs VERIFY on the native main client, the pre-feature behavior.
    pub fn set_deep_verify_candidates(
        &mut self,
        candidates: Vec<(Arc<dyn AsyncApiClient>, String)>,
    ) {
        self.deep_verify_candidates = candidates;
    }

    /// Install the Architect PLAN client for a session whose native model is
    /// not reserved deep-tier. Set-or-cleared on every turn entry.
    pub fn set_deep_plan_client(
        &mut self,
        client: Option<(Arc<dyn AsyncApiClient>, String)>,
    ) {
        self.deep_plan_client = client;
    }

    /// The Architect PLAN client model currently installed by the host.
    #[must_use]
    pub fn deep_plan_model(&self) -> Option<&str> {
        self.deep_plan_client
            .as_ref()
            .map(|(_, model)| model.as_str())
    }

    /// Enforce the Architect invariant that PLAN and VERIFY never fall back to
    /// a non-deep native session model. Set-or-cleared on every turn entry.
    pub fn set_deep_tier_only(&mut self, required: bool) {
        self.deep_tier_only = required;
    }

    /// Whether PLAN/VERIFY are currently restricted to the deep-tier pool.
    #[must_use]
    pub fn deep_tier_only(&self) -> bool {
        self.deep_tier_only
    }

    /// Install the ordered Architect PLAN/VERIFY membership pool resolved by
    /// the host from merged smart settings. Set on every turn entry alongside
    /// [`Self::set_deep_tier_only`].
    pub fn set_deep_tier_models(&mut self, models: Vec<String>) {
        self.deep_tier_models = models;
    }

    /// The ordered PLAN/VERIFY membership pool installed by the host.
    #[must_use]
    pub fn deep_tier_models(&self) -> &[String] {
        &self.deep_tier_models
    }

    /// Install (or clear with `None`) the per-turn Architect execution
    /// contract for a `smart.policy=architect` implementation-shaped turn
    /// whose session main model is reserved for plan/verify duty. Its optional
    /// implementer client records whether `smart.execSwap` armed for this
    /// turn; plan-first promotion and edit-gate metadata remain installed when
    /// EXEC stays native. The host
    /// sets-or-clears this on every turn entry (like
    /// [`Self::set_deep_verify_candidates`]) so a contract can never outlive
    /// its turn. `None` keeps every leg on the native client.
    pub fn set_exec_contract(&mut self, contract: Option<super::deep_gate::ExecContract>) {
        self.exec_contract = contract;
    }

    /// The installed per-turn Architect execution contract, if any.
    #[must_use]
    pub fn exec_contract(&self) -> Option<&super::deep_gate::ExecContract> {
        self.exec_contract.as_ref()
    }

    /// Arm (or clear) the per-turn Architect edit gate. The host arms it only
    /// when [`Self::set_exec_contract`] installs a live implementer swap; the
    /// deep gate clears it mid-turn when failure escalation hands
    /// implementation back to the native model.
    pub fn set_reserved_edit_gate(&mut self, armed: bool) {
        self.reserved_edit_gate = armed;
    }


    /// Set (or clear with `None`) the reasoning-effort floor applied to outgoing
    /// requests via `ApiRequest::effort_override`. A floor, not an override:
    /// the client uses `max(this, its configured budget)`, so it can only raise
    /// effort. The deep-gate uses this to power up a stalled hard task and
    /// clears it when the deep turn ends. See `Self::effort_override`.
    pub fn set_effort_override(&mut self, budget_tokens: Option<u32>) {
        self.effort_override = budget_tokens;
    }


    pub fn set_memory_retriever(
        &mut self,
        retriever: Option<Arc<dyn MemoryRetriever + Send + Sync>>,
    ) {
        self.memory_retriever = retriever;
    }

    /// Seat something beside recall that sees every recall's query and hits
    /// after recall settles and answers with the order the turn reads.
    pub fn set_recall_seat(&mut self, seat: Option<Arc<dyn crate::RecallSeat>>) {
        self.recall_seat = seat;
    }

    /// Seat something beside full compaction that is asked which tool
    /// results the summary still needs — and, in the modes that act,
    /// answered by taking the rest out of the summary's input.
    pub fn set_compaction_seat(&mut self, seat: Option<Arc<dyn crate::CompactionSeat>>) {
        self.compaction_seat = seat;
    }

    /// Seat something beside the edit tools that is asked about every patch
    /// they write, after the write and before the model reads the result —
    /// and, in the modes that act, answered by one line added to that result.
    pub fn set_patch_review_seat(&mut self, seat: Option<Arc<dyn crate::PatchReviewSeat>>) {
        self.patch_review_seat = seat;
    }

    pub fn set_auto_compaction_enabled(&mut self, enabled: bool) {
        self.auto_compaction_enabled = enabled;
        if !enabled {
            self.state_distill_deferred_precompaction = false;
        }
    }

    /// Builder-style variant of [`Self::set_async_api_client`].
    #[must_use]
    pub fn with_async_api_client(mut self, client: Arc<dyn AsyncApiClient>) -> Self {
        self.async_api_client = Some(client);
        self
    }

    #[must_use]
    pub fn with_max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations;
        self
    }

    /// Cap the agentic loop in place, for callers that hold an already-built
    /// runtime (e.g. the headless `-p` path wiring `--max-turns`) rather than
    /// the consuming builder. `usize::MAX` (the default) leaves it unbounded.
    pub fn set_max_iterations(&mut self, max_iterations: usize) {
        self.max_iterations = max_iterations;
    }

    /// Set a wall-clock deadline for the turn. Once passed, the agentic loop stops
    /// at the next iteration boundary with a "time budget" error instead of
    /// running on. Used to bound spawned sub-agents (the foreground turn leaves it
    /// unset, i.e. unbounded). Cooperative: a single in-flight provider stream
    /// still completes before the check, so the bound is to the nearest iteration.
    pub fn set_deadline(&mut self, deadline: std::time::Instant) {
        self.deadline = Some(deadline);
    }

    /// Clear any wall-clock deadline, restoring unbounded (foreground) behavior.
    /// The interactive host calls this when the turn wall-clock breaker is
    /// disabled (budget `0`) so a stale deadline from a prior turn can't fire.
    pub fn clear_deadline(&mut self) {
        self.deadline = None;
    }

    /// Set (or clear) the progress-gated deadline-extension policy
    /// `(max_extensions, step)`: when the wall-clock deadline passes but the
    /// turn shows fresh objective progress (new successful edit/write/plan
    /// tool results since the last window), the deadline is pushed out by
    /// `step`, at most `max_extensions` times, instead of stopping mid-work.
    /// Only the interactive host sets this — spawned sub-agents keep their
    /// deadline as a hard straggler bound. Re-set at every turn entry (like
    /// [`Self::set_deadline`]) so an env change takes effect next turn.
    pub fn set_deadline_extension(&mut self, policy: Option<(u8, std::time::Duration)>) {
        self.deadline_extension = policy;
    }

    /// Set the per-turn cumulative-output-token budget (the cost circuit
    /// breaker). `None` clears it (unbounded). See
    /// `ConversationRuntime::turn_output_token_budget`.
    pub fn set_turn_output_token_budget(&mut self, budget: Option<u32>) {
        self.turn_output_token_budget = budget;
    }

    /// Set the per-turn cumulative full-price-input-token budget (the
    /// cache-miss cost circuit breaker). `None` clears it (unbounded). See
    /// `ConversationRuntime::turn_input_token_budget`.
    pub fn set_turn_input_token_budget(&mut self, budget: Option<u32>) {
        self.turn_input_token_budget = budget;
    }

    pub(super) fn check_sync_turn_cancelled(&mut self, iteration: usize) -> Result<(), RuntimeError> {
        if self.hook_abort_signal.is_aborted() {
            self.record_turn_cancelled(iteration, "turn cancelled by abort signal");
            return Err(RuntimeError::new("agent cancelled"));
        }
        Ok(())
    }

    /// Tag this runtime's hooks as belonging to a spawned sub-agent: every
    /// hook payload gains `agent_id`/`agent_type` (CC parity), so user hooks
    /// can distinguish sub-agent tool calls from main-agent ones.
    pub fn set_hook_agent_context(
        &mut self,
        agent_id: impl Into<String>,
        agent_type: impl Into<String>,
    ) {
        self.hook_runner.set_agent_context(agent_id, agent_type);
    }

    /// Cap consecutive `TurnEnd`-hook continuations (the Stop-loop). `0`
    /// disables continuation entirely (the turn always returns after one run).
    pub fn set_max_stop_loops(&mut self, max_stop_loops: usize) {
        self.max_stop_loops = max_stop_loops;
    }

    /// Mark this runtime as an autonomous surface: nobody is present to answer
    /// a mid-run question, so the turn-end gate also lints question endings.
    /// Hosts set it for headless one-shots; interactive sessions leave it off.
    pub fn set_autonomous_surface(&mut self, autonomous: bool) {
        self.autonomous_surface = autonomous;
    }

    /// Whether this runtime is currently an autonomous surface — see
    /// [`Self::set_autonomous_surface`]. The host reads it on the way into a
    /// turn to decide who is there to stop the turn ([`Attendance`]).
    #[must_use]
    pub fn is_autonomous_surface(&self) -> bool {
        self.autonomous_surface
    }

    /// Who can stop the turns this runtime runs — see [`Attendance`]. The
    /// host sets it beside the turn budgets, every turn, for the same reason
    /// it re-arms those: a runtime rebuilt for a model or permission switch
    /// starts over as unattended, which is the safe default.
    pub fn set_attendance(&mut self, attendance: Attendance) {
        self.attendance = attendance;
    }

    /// Set or clear the session goal (`/goal`). Mirrored into the persisted
    /// session header immediately — goal flips are rare and a crash before the
    /// next message append would otherwise lose the goal a resume should
    /// restore. Also surfaces in the `TurnEnd` hook context (`sessionGoal`)
    /// so Stop-hook gates can judge completion against it.
    pub fn set_session_goal(&mut self, goal: Option<String>) {
        if self.session.session_goal == goal {
            return;
        }
        self.session.session_goal = goal;
        if let Some(path) = self
            .session
            .persistence_path()
            .map(std::path::Path::to_path_buf)
        {
            if let Err(err) = self.session.save_to_path(&path) {
                eprintln!(
                    "[zo] warning: failed to persist session goal to {}: {err}",
                    path.display()
                );
            }
        }
    }

    #[must_use]
    pub fn with_max_tool_calls(mut self, max_tool_calls: usize) -> Self {
        self.max_tool_calls = max_tool_calls;
        self
    }

    /// Cap model-requested tool calls in place. This complements
    /// [`Self::set_max_iterations`]: one iteration can contain many parallel
    /// `tool_use` blocks, so turn budgets need both guards.
    pub fn set_max_tool_calls(&mut self, max_tool_calls: usize) {
        self.max_tool_calls = max_tool_calls;
    }

    pub(super) fn check_tool_call_budget(
        &self,
        used_tool_calls: usize,
        pending_tool_use_count: usize,
    ) -> Result<(), RuntimeError> {
        if pending_tool_use_count > self.max_tool_calls.saturating_sub(used_tool_calls) {
            return Err(RuntimeError::new(format!(
                "conversation loop exceeded the maximum number of tool calls ({})",
                self.max_tool_calls
            )));
        }
        Ok(())
    }
    /// Force the agent to call `tool_name` before the turn ends (workflow 8c).
    /// A schema phase wires `StructuredOutput` here so its result is always a
    /// captured tool input rather than parsed-from-prose text. The forced turn
    /// only fires when the agent did not already call the tool, so a compliant
    /// agent pays no extra request.
    #[must_use]
    pub fn with_structured_output_tool(mut self, tool_name: impl Into<String>) -> Self {
        let tool_name = tool_name.into();
        self.structured_output_tool = (!tool_name.trim().is_empty()).then_some(tool_name);
        self
    }

    #[must_use]
    pub fn with_auto_compaction_input_tokens_threshold(mut self, threshold: u32) -> Self {
        self.auto_compaction_input_tokens_threshold = threshold;
        self
    }

    #[must_use]
    pub fn with_hook_abort_signal(mut self, hook_abort_signal: HookAbortSignal) -> Self {
        self.hook_abort_signal = hook_abort_signal;
        self
    }

    pub fn set_hook_abort_signal(&mut self, hook_abort_signal: HookAbortSignal) {
        self.hook_abort_signal = hook_abort_signal;
    }

    /// Clone this runtime's abort signal (an `Arc`-backed flag). The TUI moves
    /// the whole runtime into a spawned turn task so a heavy synchronous segment
    /// inside the turn cannot starve the render loop; it keeps a clone of this
    /// signal to wind the task down on Ctrl+C (the task polls the flag and drops
    /// the in-flight turn, then hands the runtime back).
    #[must_use]
    pub fn hook_abort_signal(&self) -> HookAbortSignal {
        self.hook_abort_signal.clone()
    }

    pub fn set_tool_cancel_signal(&mut self, tool_cancel_signal: ToolCancelSignal) {
        self.tool_cancel_signal = tool_cancel_signal;
    }

    /// Clone this runtime's per-tool cancel signal. The host keeps a clone so a
    /// single Esc can drop the tool that is executing right now **without**
    /// ending the turn — the turn resumes from the synthetic tool result and
    /// the model picks another approach. Ending the turn is
    /// [`Self::hook_abort_signal`]'s job and stays on Ctrl+C / Esc-Esc.
    #[must_use]
    pub fn tool_cancel_signal(&self) -> ToolCancelSignal {
        self.tool_cancel_signal.clone()
    }


    #[must_use]
    pub fn with_hook_progress_reporter(
        mut self,
        hook_progress_reporter: Box<dyn HookProgressReporter + Send>,
    ) -> Self {
        self.hook_progress_reporter = Some(hook_progress_reporter);
        self
    }

    #[must_use]
    pub fn with_session_tracer(mut self, session_tracer: SessionTracer) -> Self {
        self.session_tracer = Some(session_tracer);
        self
    }

    /// Install a concurrent dispatch function for parallel tool execution.
    /// Concurrency-safe tools (Read, Glob, Grep, etc.) will be executed in
    /// parallel via `spawn_blocking` when multiple such tools appear in a
    /// single assistant turn.
    pub fn set_concurrent_dispatch(&mut self, dispatch: ConcurrentDispatchFn) {
        self.concurrent_dispatch = Some(dispatch);
    }

    /// Register tool names the host considers long-running. Live streaming now
    /// routes every ordinary tool through the blocking-worker dispatch seam, so
    /// this remains as compatibility metadata for hosts/tests that classify
    /// plugin-backed tools separately. Replaces any prior set.
    pub fn set_long_running_tools(&mut self, names: impl IntoIterator<Item = String>) {
        self.long_running_tool_names = names.into_iter().collect();
    }

    /// Install a predicate marking further tools as long-running. Kept for host
    /// compatibility with dynamic MCP/plugin registrations; the live streaming
    /// path no longer depends on this predicate as its only spawn-blocking guard.
    pub fn set_long_running_predicate(&mut self, predicate: LongRunningPredicate) {
        self.long_running_predicate = Some(predicate);
    }

    /// Whether `tool_name` is classified as long-running by legacy metadata.
    // Test seam: live streaming routes every permitted tool through
    // `spawn_blocking` once `ConcurrentDispatchFn` is installed (see
    // `tool::is_long_running` docs), so production no longer consults this
    // projection — the conversation tests assert the retained host-predicate
    // policy through it.
    #[allow(dead_code)]
    pub(super) fn tool_is_long_running(&self, tool_name: &str) -> bool {
        is_long_running(tool_name)
            || self.long_running_tool_names.contains(tool_name)
            || self
                .long_running_predicate
                .as_ref()
                .is_some_and(|predicate| predicate(tool_name))
    }

    pub(super) fn run_pre_tool_use_hook(&mut self, tool_name: &str, input: &str) -> HookRunResult {
        self.hook_runner.run_pre_tool_use_with_context(
            tool_name,
            input,
            Some(&self.hook_abort_signal),
            self.hook_progress_reporter
                .as_mut()
                .map(|r| r.as_mut() as &mut dyn HookProgressReporter),
        )
    }

    pub(super) fn run_post_tool_use_hook(
        &mut self,
        tool_name: &str,
        input: &str,
        output: &str,
        is_error: bool,
    ) -> HookRunResult {
        self.hook_runner.run_post_tool_use_with_context(
            tool_name,
            input,
            output,
            is_error,
            Some(&self.hook_abort_signal),
            self.hook_progress_reporter
                .as_mut()
                .map(|r| r.as_mut() as &mut dyn HookProgressReporter),
        )
    }

    pub(super) fn run_post_tool_use_failure_hook(
        &mut self,
        tool_name: &str,
        input: &str,
        output: &str,
    ) -> HookRunResult {
        self.hook_runner.run_post_tool_use_failure_with_context(
            tool_name,
            input,
            output,
            Some(&self.hook_abort_signal),
            self.hook_progress_reporter
                .as_mut()
                .map(|r| r.as_mut() as &mut dyn HookProgressReporter),
        )
    }

    /// Async twin of [`Self::run_pre_tool_use_hook`]. Blocking hook work runs
    /// off-task and progress is forwarded live without holding the reporter
    /// borrow across an `.await`; result semantics match the sync path.
    pub(super) async fn run_pre_tool_use_hook_async(
        &mut self,
        tool_name: &str,
        input: &str,
    ) -> HookRunResult {
        let runner = self.hook_runner.clone();
        let abort_signal = self.hook_abort_signal.clone();
        let tool_name = tool_name.to_string();
        let input = input.to_string();
        self.run_hook_off_task(move |mut reporter| {
            runner.run_pre_tool_use_with_context(
                &tool_name,
                &input,
                Some(&abort_signal),
                Some(&mut reporter),
            )
        })
        .await
    }

    /// Async twin of [`Self::run_post_tool_use_hook`]; see
    /// [`Self::run_pre_tool_use_hook_async`] for the offload contract.
    pub(super) async fn run_post_tool_use_hook_async(
        &mut self,
        tool_name: &str,
        input: &str,
        output: &str,
        is_error: bool,
    ) -> HookRunResult {
        let runner = self.hook_runner.clone();
        let abort_signal = self.hook_abort_signal.clone();
        let tool_name = tool_name.to_string();
        let input = input.to_string();
        let output = output.to_string();
        self.run_hook_off_task(move |mut reporter| {
            runner.run_post_tool_use_with_context(
                &tool_name,
                &input,
                &output,
                is_error,
                Some(&abort_signal),
                Some(&mut reporter),
            )
        })
        .await
    }

    /// Async twin of [`Self::run_post_tool_use_failure_hook`]; see
    /// [`Self::run_pre_tool_use_hook_async`] for the offload contract.
    pub(super) async fn run_post_tool_use_failure_hook_async(
        &mut self,
        tool_name: &str,
        input: &str,
        output: &str,
    ) -> HookRunResult {
        let runner = self.hook_runner.clone();
        let abort_signal = self.hook_abort_signal.clone();
        let tool_name = tool_name.to_string();
        let input = input.to_string();
        let output = output.to_string();
        self.run_hook_off_task(move |mut reporter| {
            runner.run_post_tool_use_failure_with_context(
                &tool_name,
                &input,
                &output,
                Some(&abort_signal),
                Some(&mut reporter),
            )
        })
        .await
    }

    /// Run one blocking hook closure on a `spawn_blocking` worker, forwarding its
    /// progress events to the live reporter *as they arrive*. Factored out so the
    /// three async hook twins share exactly one offload policy (the
    /// `spawn_blocking` join, the live event forwarding, and the panic mapping)
    /// rather than repeating it.
    ///
    /// `run` receives a [`ChannelHookProgressReporter`] and returns the
    /// `HookRunResult`; every event it emits is sent over an unbounded channel
    /// that this loop drains *while* the worker still runs — so a `Started`
    /// reaches the reporter the moment the hook begins, not only after it exits.
    /// The live reporter borrow is taken per event and never held across an
    /// `.await`, the constraint the sync path could not satisfy. Channel FIFO
    /// preserves event order; a trailing drain flushes any queued events before
    /// returning.
    ///
    /// A worker panic (`JoinError`) maps to [`HookRunResult::failed`], not a
    /// silent allow: the pre-hook path then denies the tool and the post-hook
    /// path marks the result an error, so a crashed hook never bypasses policy.
    async fn run_hook_off_task(
        &mut self,
        run: impl FnOnce(crate::hooks::ChannelHookProgressReporter) -> HookRunResult + Send + 'static,
    ) -> HookRunResult {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut join = tokio::task::spawn_blocking(move || {
            run(crate::hooks::ChannelHookProgressReporter::new(event_tx))
        });

        // Forward events live while the worker runs. `recv()` resolves `None`
        // once the worker drops its sender (i.e. finished), ending the loop.
        loop {
            tokio::select! {
                biased;
                event = event_rx.recv() => match event {
                    Some(event) => self.forward_hook_progress_event(&event),
                    None => break,
                },
                join_result = &mut join => {
                    // Worker finished (or panicked) before the sender closed the
                    // channel; fall through to the trailing drain, then return.
                    return self.finish_off_task(join_result, event_rx);
                }
            }
        }

        self.finish_off_task(join.await, event_rx)
    }

    /// Drain any events still queued after the worker finished (so none are
    /// dropped), then translate the join outcome into a `HookRunResult`: the
    /// worker's own result on success, or a hard failure on panic.
    fn finish_off_task(
        &mut self,
        join_result: Result<HookRunResult, tokio::task::JoinError>,
        mut event_rx: tokio::sync::mpsc::UnboundedReceiver<HookProgressEvent>,
    ) -> HookRunResult {
        while let Ok(event) = event_rx.try_recv() {
            self.forward_hook_progress_event(&event);
        }
        match join_result {
            Ok(result) => result,
            Err(join_error) => {
                HookRunResult::failed(format!("hook worker panicked: {join_error}"))
            }
        }
    }

    /// Forward one hook progress event to the live reporter, taking the borrow
    /// per event so it is never held across an `.await`.
    fn forward_hook_progress_event(&mut self, event: &HookProgressEvent) {
        if let Some(reporter) = self.hook_progress_reporter.as_mut() {
            reporter.on_event(event);
        }
    }

    /// Test seam: drive [`Self::run_hook_off_task`] with a worker that panics,
    /// so a test can assert a `JoinError` maps to a failed (not silently
    /// allowed) `HookRunResult` at the shared off-task boundary.
    #[cfg(test)]
    pub(super) async fn run_hook_off_task_panicking_for_test(&mut self) -> HookRunResult {
        self.run_hook_off_task(|_reporter| panic!("simulated hook worker panic"))
            .await
    }

    /// Compact the session, API-first with a deterministic local fallback.
    ///
    /// `focus` carries an optional `/compact <focus>` directive: when present it
    /// is threaded into the summary system prompt so the API summarizer steers
    /// the retained detail toward what the user asked to keep (and, if the API
    /// round-trip fails, into the local `FocusSummarizer` fallback). Bare
    /// `/compact` passes `None`, leaving the summary path byte-identical.
    #[must_use]
    pub fn compact(&mut self, config: CompactionConfig, focus: Option<&str>) -> CompactionResult {
        self.compact_with_api_fallback(config, focus)
    }

    #[must_use]
    pub fn estimated_tokens(&self) -> usize {
        estimate_session_tokens(&self.session)
    }

    #[must_use]
    pub fn usage(&self) -> &UsageTracker {
        &self.usage_tracker
    }

    #[must_use]
    pub fn context_window(&self) -> u64 {
        self.context_window
    }

    #[must_use]
    pub fn auto_compaction_input_tokens_threshold(&self) -> u32 {
        self.auto_compaction_input_tokens_threshold
    }

    #[must_use]
    pub fn microcompact_input_tokens_threshold(&self) -> u64 {
        self.context_policy
            .microcompact_threshold(self.auto_compaction_window())
    }

    /// The window the compaction tiers are measured against — the model's real
    /// window capped (or Claude Code's explicit
    /// `CLAUDE_CODE_AUTO_COMPACT_WINDOW`). Deliberately *not* `context_window`:
    /// see [`super::compaction::auto_compaction_window`] for why riding a
    /// percentage of a 1M window makes every cache break cost half a million
    /// tokens. Provider-limit guards keep using the real window.
    fn auto_compaction_window(&self) -> u64 {
        super::compaction::auto_compaction_window(self.context_window)
    }

    #[must_use]
    pub fn state_distill_input_tokens_threshold(&self) -> u64 {
        self.context_policy
            .state_distill_threshold(self.auto_compaction_window())
    }

    #[must_use]
    pub fn precompaction_input_tokens_threshold(&self) -> u64 {
        self.precompaction_input_tokens_threshold
    }

    /// Re-derive the context window (and its auto-compaction threshold) after a
    /// live model switch. Both are otherwise fixed at construction, so without
    /// this a `/model` change keeps compacting at the *previous* model's limit:
    /// too early when switching to a larger-window model (e.g. GPT 258k → Opus
    /// 1M would still compact at 219k, ~22% of the real window), or too late
    /// when switching the other way (risking a backend over-full rejection).
    /// Honours an explicit `CLAUDE_CODE_AUTO_COMPACT_INPUT_TOKENS` override
    /// exactly as the constructor does.
    pub fn set_context_window(&mut self, context_window: u64) {
        self.context_window = context_window;
        let compaction_window = self.auto_compaction_window();
        self.auto_compaction_input_tokens_threshold =
            auto_compaction_threshold_from_env_or_policy(compaction_window, self.context_policy);
        self.precompaction_input_tokens_threshold = self.context_policy.precompaction_threshold(
            compaction_window.max(u64::from(FALLBACK_AUTO_COMPACTION_INPUT_TOKENS_THRESHOLD)),
        );
    }

    /// Re-derive both the context window and model-family cheap-trim policy
    /// after a live model switch. This is deliberately separate from
    /// [`Self::set_context_window`]: a bare number cannot safely identify the
    /// model family, so callers that know the model name must use this seam.
    pub fn set_context_model(&mut self, model: &str) {
        // Hosts may re-apply the selected model at every turn entry. Only a
        // value change is a new model world; an unchanged value must preserve
        // the session-scoped refusal cooldown and its one-shot notice state.
        if self.context_model.as_deref() == Some(model) {
            return;
        }
        self.context_model = Some(model.to_string());
        self.refusal_fallback_model = None;
        self.refusal_turn_hit = false;
        self.refusal_consecutive_turns = 0;
        self.refusal_dry_until = None;
        self.refusal_prearm_notice_pending = false;
        self.refusal_prearm_notice_latched = false;
        self.refusal_context_clean_used = false;
        self.context_policy = ContextPolicy::for_model(Some(model))
            .with_full_compaction_override(self.full_compaction_override_percent);
        self.set_context_window(::api::context_window_for_model(model));
        self.retarget_system_prompt_model(model);
    }

    /// Point the built system prompt's `Model family:` line at `model`.
    ///
    /// Driven from [`Self::set_context_model`] — the single live-switch seam —
    /// so a swap can never leave the prompt telling the model it is the one it
    /// just replaced. Only the environment-context bullet is rewritten, and
    /// that section sits *after* [`crate::SYSTEM_PROMPT_DYNAMIC_BOUNDARY`], so
    /// the static scaffolding and its 1h cache breakpoint are untouched.
    fn retarget_system_prompt_model(&mut self, model: &str) {
        let mut sections = self.system_prompt.to_vec();
        crate::prompt::retarget_prompt_model(&mut sections, model);
        self.system_prompt = Arc::from(sections);
    }

    /// Swap the permission policy in place (live `/permission` or Shift+Tab
    /// cycle). The policy is the only runtime component that depends on the
    /// permission mode, so hosts can switch modes without rebuilding the
    /// runtime — a full rebuild re-spawns LSP/MCP subsystems synchronously and
    /// freezes the TUI event loop for seconds.
    /// Add a rule set to the policy in place — a teammate pane applying the
    /// permission rules its parent's harness carried (t-2513 §2.1), on top
    /// of the mode and tool requirements the session was built with.
    pub fn add_permission_rules(&mut self, rules: &crate::RuntimePermissionRuleConfig) {
        let policy = self.permission_policy.clone().with_permission_rules(rules);
        self.permission_policy = policy;
    }

    pub fn set_permission_policy(&mut self, permission_policy: PermissionPolicy) {
        self.permission_policy = permission_policy;
    }

    /// The live active permission mode. A cheap read of the policy's active mode,
    /// used by the session host to install/restore a turn-scoped downgrade (the
    /// unattended `/loop`/`/goal` read-only gate) without rebuilding the runtime.
    #[must_use]
    pub fn active_permission_mode(&self) -> PermissionMode {
        self.permission_policy.active_mode()
    }

    /// Swap the active permission mode in place, returning the previous one, so a
    /// host can install a one-turn override and restore it after. Mirrors the
    /// internal `DeepSubturnPermissionGuard` PLAN/VERIFY downgrade, exposed for
    /// the session-host turn scaffold that forces unattended automation turns
    /// read-only. `authorize_with_context` reads the active mode live on every
    /// tool call, so the override takes effect immediately.
    pub fn set_active_permission_mode(&mut self, mode: PermissionMode) -> PermissionMode {
        self.permission_policy.set_active_mode(mode)
    }

    /// Add ephemeral allow rules for the duration of a scoped phase, returning an
    /// opaque grant to pass back to [`Self::remove_temporary_permission_allow_rules`].
    /// The unattended automation gate uses this so a forced-read-only loop/goal
    /// turn can still record its proposal into the team inbox (the "propose only"
    /// half of the policy), the same relaxation `DeepSubturnPermissionGuard`
    /// grants read-only inspection commands.
    #[must_use]
    pub fn add_temporary_permission_allow_rules(&mut self, specs: &[&str]) -> TemporaryAllowGrant {
        self.permission_policy.add_temporary_allow_rules(specs)
    }

    /// Remove the temporary allow rules recorded in `grant`, restoring the rule
    /// set exactly so a phase grant never leaks past the turn.
    pub fn remove_temporary_permission_allow_rules(&mut self, grant: TemporaryAllowGrant) {
        self.permission_policy.remove_temporary_allow_rules(grant);
    }

    /// Refresh tool→required-mode requirements on the live permission policy
    /// after MCP tools are discovered mid-session, without rebuilding the policy
    /// (which would drop session grants). A read-only MCP tool then keeps its
    /// `ReadOnly` requirement and stays usable inside `ReadOnly` PLAN/VERIFY
    /// sub-turns, while a write-capable MCP tool still requires the higher mode.
    pub fn refresh_tool_requirements(
        &mut self,
        requirements: impl IntoIterator<Item = (String, PermissionMode)>,
    ) {
        self.permission_policy.set_tool_requirements(requirements);
    }

    /// Drain the "always allow" rules the user granted live this session (via
    /// an `AllowAlways` permission decision) so the host can persist them to a
    /// settings file. Empty when none were granted.
    pub fn take_granted_permission_rules(&mut self) -> Vec<String> {
        self.permission_policy.take_newly_granted()
    }

    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The system prompt this runtime shows the model, section by section —
    /// after any CLI override and any model retarget, so a reader gets the
    /// prompt as it is, not as it was built. A `fork` sub-agent inherits it
    /// verbatim (t-2875).
    #[must_use]
    pub fn system_prompt(&self) -> &[String] {
        &self.system_prompt
    }

    /// The transient system reminders standing at rest, in wire order.
    ///
    /// Exposed for the fixed-harness accounting (`zo --prompt-input`): the
    /// reminder tail is the one part of the per-request prefix that has no
    /// ceiling of its own today, so it is also the one that can grow without
    /// anybody noticing.
    #[must_use]
    pub fn transient_reminders(&self) -> &[String] {
        &self.transient_reminders
    }

    pub fn fire_lifecycle_hook(&self, event: HookEvent, context: &Value) {
        let _ = self.run_lifecycle_hook(event, context);
    }

    /// Fire `CwdChanged` when the process working directory has actually moved
    /// since the last look.
    ///
    /// `CwdChanged` was declared, parsed out of settings, and routed by
    /// `rules_for_event` — and emitted from nowhere. A person could write the
    /// hook, watch zo accept the config without a word, and never see it run.
    /// That is worse than not offering the event at all, because the silence
    /// reads as "nothing changed directories".
    ///
    /// Observed rather than announced at the call sites that chdir: the ones
    /// that exist today (`EnterWorktree` / `ExitWorktree`) live in the tools
    /// crate with no hook channel, and a hand-maintained list of "tools that
    /// move the cwd" is a list that goes stale silently — the same failure a
    /// second time. One `current_dir()` per tool batch costs a syscall and can
    /// never drift from what the process actually did.
    ///
    /// The first observation seeds the baseline without firing: a session that
    /// starts somewhere has not *changed* anywhere.
    pub(super) fn fire_cwd_changed_if_moved(&mut self) {
        let Ok(current) = std::env::current_dir() else {
            // An unreadable cwd (the directory was removed under us) is not a
            // move. Leave the baseline alone so the next readable value is
            // compared against where we actually were.
            return;
        };
        self.observe_cwd(&current);
    }

    /// Every per-tool-batch lifecycle hook, fired from the one seam both turn
    /// loops share.
    ///
    /// These three events (plus `CwdChanged`) exist so something OUTSIDE the
    /// process can follow a long autonomous run by being told, instead of by
    /// polling a transcript it has to parse. That only works if they fire from
    /// a place that sees every batch — which is why they live here together and
    /// not scattered across the tools that happen to trigger them.
    pub(super) fn fire_post_batch_lifecycle_hooks(&mut self) {
        self.fire_cwd_changed_if_moved();
        self.fire_file_changed_for_new_edits();
        self.fire_todo_lifecycle_hooks();
    }

    /// Fire `FileChanged` once per file this batch's results say was written.
    ///
    /// Reads the batch's own new transcript tail through
    /// [`crate::edited_file_paths`] — the same tolerant extractor the durable
    /// turn trace and the post-compaction reminder already trust — rather than
    /// asking each tool to announce itself. A hand-kept list of "tools that
    /// write files" is a list that goes stale in silence, which is the exact
    /// way `CwdChanged` came to have no emitter at all.
    ///
    /// A file edited twice in one turn fires twice: it changed twice.
    pub(super) fn fire_file_changed_for_new_edits(&mut self) {
        let watermark = self.file_change_watermark.min(self.session.messages.len());
        let paths = crate::edited_file_paths(&self.session.messages[watermark..]);
        self.file_change_watermark = self.session.messages.len();
        for path in paths {
            self.fire_lifecycle_hook(
                HookEvent::FileChanged,
                &serde_json::json!({
                    "path": path,
                    "session_id": self.session.session_id,
                }),
            );
        }
    }

    /// Seed the todo baseline at turn entry, so a plan the FIRST tool batch
    /// writes still reads as created rather than as "how things always were".
    pub(super) fn seed_todo_baseline(&mut self) {
        self.todo_baseline = self.current_todo_statuses();
    }

    /// Fire `TaskCreated` for items that appeared since the last look and
    /// `TaskCompleted` for items that reached `completed`.
    ///
    /// Keyed by item text because that is the only identity a todo has on the
    /// wire; a reworded item therefore reads as one completed-elsewhere item
    /// disappearing and a new one appearing, which is the honest reading of the
    /// only evidence there is.
    pub(super) fn fire_todo_lifecycle_hooks(&mut self) {
        use crate::todo_progress::TodoStatus;
        let current = self.current_todo_statuses();
        if current == self.todo_baseline {
            return;
        }
        for (content, status) in &current {
            let previous = self.todo_baseline.get(content);
            if previous.is_none() {
                self.fire_lifecycle_hook(
                    HookEvent::TaskCreated,
                    &serde_json::json!({ "task": content, "status": status.token() }),
                );
            }
            if *status == TodoStatus::Completed
                && previous.is_none_or(|before| *before != TodoStatus::Completed)
            {
                self.fire_lifecycle_hook(
                    HookEvent::TaskCompleted,
                    &serde_json::json!({ "task": content }),
                );
            }
        }
        self.todo_baseline = current;
    }

    fn current_todo_statuses(
        &self,
    ) -> std::collections::BTreeMap<String, crate::todo_progress::TodoStatus> {
        self.trace_cwd().map_or_else(Default::default, |cwd| {
            crate::todo_progress::current_todos(&cwd)
                .into_iter()
                .map(|todo| (todo.content, todo.status))
                .collect()
        })
    }

    /// [`Self::fire_cwd_changed_if_moved`] with the directory supplied instead
    /// of read from the process, so a test can drive both edges without
    /// `set_current_dir` — which is process-global and would race every other
    /// test in the binary.
    pub(super) fn observe_cwd(&mut self, current: &std::path::Path) {
        let Some(previous) = self.last_observed_cwd.replace(current.to_path_buf()) else {
            return;
        };
        if previous == current {
            return;
        }
        self.fire_lifecycle_hook(
            HookEvent::CwdChanged,
            &serde_json::json!({
                "from": previous.display().to_string(),
                "to": current.display().to_string(),
            }),
        );
    }

    pub(super) fn run_lifecycle_hook(&self, event: HookEvent, context: &Value) -> HookRunResult {
        let command_count = self.hook_runner.lifecycle_command_count(event);
        if command_count > 0 {
            self.record_lifecycle_hook_audit("lifecycle_hook_started", event, command_count, None);
        }

        let result = self.hook_runner.run_lifecycle_event(event, context);
        if command_count > 0 {
            self.record_lifecycle_hook_audit(
                "lifecycle_hook_finished",
                event,
                command_count,
                Some(&result),
            );
        }
        result
    }

    fn record_lifecycle_hook_audit(
        &self,
        action: &str,
        event: HookEvent,
        command_count: usize,
        result: Option<&HookRunResult>,
    ) {
        let Some(session_tracer) = &self.session_tracer else {
            return;
        };

        let mut attributes = trace_attrs(json!({
            "event": event.as_str(),
            "command_count": command_count,
        }));
        if let Some(result) = result {
            attributes.insert(
                "outcome".to_string(),
                Value::String(lifecycle_hook_outcome(result).to_string()),
            );
            attributes.insert("denied".to_string(), Value::Bool(result.is_denied()));
            attributes.insert("failed".to_string(), Value::Bool(result.is_failed()));
            attributes.insert("cancelled".to_string(), Value::Bool(result.is_cancelled()));
            if let Some(message) = result.messages().first() {
                attributes.insert("message".to_string(), Value::String(message.clone()));
            }
        }

        session_tracer.record_security_audit(action, attributes);
    }

    /// Mutable access to the underlying session (for rewind, truncation, etc.).
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Swap the entire session. Use this for `/resume` to avoid rebuilding
    /// the full runtime. (`build_request` reads `session.messages` directly
    /// via `Arc::clone`, so there is no separate request-message cache.)
    ///
    /// The outgoing session hands its writer lease back first. Dropping it here
    /// is not enough: a `Session` clone shares the lease, so one surviving copy
    /// (a background task, a snapshot) kept the abandoned path locked for the
    /// rest of the process — `/resume` back onto it then failed every write
    /// with `writer lease held`, and after `/new` no other zo window could open
    /// the previous session either.
    pub fn replace_session(&mut self, session: Session) {
        self.session.release_writer_lease();
        // Recall dedup is a property of the TRANSCRIPT, not the process: the
        // incoming one has its own persisted bodies (usually none, on `/new`),
        // and carrying the outgoing session's set would collapse an entry to a
        // pointer aimed at a body that is not in this context.
        self.recalled_memory_slugs = super::recalled_slugs_from_session(&session);
        self.session = session;
    }

    /// Remove the last `steps` assistant turns from the conversation.
    /// Returns the number of messages removed.
    pub fn rewind_turns(&mut self, steps: usize) -> usize {
        self.session.rewind_turns(steps)
    }

    #[must_use]
    pub fn tool_executor(&self) -> &T {
        &self.tool_executor
    }

    #[must_use]
    pub fn tool_executor_mut(&mut self) -> &mut T {
        &mut self.tool_executor
    }

    #[must_use]
    pub fn fork_session(&self, branch_name: Option<String>) -> Session {
        self.session.fork(branch_name)
    }

}

/// Who stops a turn that does not stop itself.
///
/// Claude Code and Codex run an interactive turn with exactly one circuit
/// breaker: the person at the keyboard, who can see the work and the spend
/// and press Esc. They set no clock and no token cap on such a turn, and a
/// long, legitimate one — an orchestration that waits on its workers, a
/// deploy that builds for an hour — runs to its end. zo's defaults used to
/// cut that turn at sixty minutes (`[budget] Time budget exhausted`) with the
/// work half done, which is a harness deciding the person was wrong to wait.
///
/// So the defaults depend on who is there. An attended turn sets nothing
/// unless the environment names a bound; an unattended one — a headless
/// `-p` run, a window-driven autonomous turn, a spawned sub-agent — has
/// nobody to press Esc and keeps the safety net. An explicit
/// `ZO_TURN_*` value applies to both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attendance {
    /// A person is at the keyboard and can interrupt: no default bound.
    Attended,
    /// Nobody can interrupt: the default clock and token caps apply.
    Unattended,
}

/// The attendance of the person this process serves, declared by the host
/// beside its turn budgets and read by the parts of the process that have no
/// runtime to ask — a spawned helper's caps (`tools`), which are chosen on a
/// worker thread from a fresh runtime. One process serves one host, so one
/// answer is right for all of it; a runtime rebuilt for a model switch does
/// not lose it. Never declared — tests, unattended hosts — reads as
/// [`Attendance::Unattended`], the safe default.
static DECLARED_ATTENDANCE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Say who is at the keyboard for this process. The plain host calls this on
/// the way into every turn, so an autonomous turn inside an attended session
/// reads as unattended for as long as it runs.
pub fn declare_attendance(attendance: Attendance) {
    DECLARED_ATTENDANCE.store(
        match attendance {
            Attendance::Unattended => 0,
            Attendance::Attended => 1,
        },
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// The attendance the host declared, or unattended when none did.
#[must_use]
pub fn declared_attendance() -> Attendance {
    match DECLARED_ATTENDANCE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Attendance::Attended,
        _ => Attendance::Unattended,
    }
}

/// Default wall-clock budget of an UNATTENDED turn: 60 minutes. Generous
/// enough that an ordinary instruction (even a deep multi-agent
/// orchestration) finishes well under it, tight enough that a non-converging
/// agentic loop surfaces a checkpoint in an hour rather than running for a
/// day. An attended turn has none — see [`Attendance`].
pub const DEFAULT_TURN_DEADLINE_SECS: u64 = 60 * 60;
/// Default cumulative-output-token budget of an UNATTENDED turn: 1.5M. An
/// ordinary turn emits well under 100k; a runaway that keeps
/// re-planning/re-generating blows past this. Sub-agent tokens are billed on
/// their own runtimes (which apply this same budget to themselves), so a
/// heavy but healthy orchestration (the parent mostly delegates) stays far
/// below it. An attended turn has none — see [`Attendance`].
pub const DEFAULT_TURN_OUTPUT_TOKEN_BUDGET: u32 = 1_500_000;
/// Default cumulative full-price-input-token budget of an UNATTENDED turn:
/// 8M. A healthy cached turn re-bills well under 1M even across compaction
/// rebuilds; a cache-dead loop re-sending a ~200k transcript uncached crosses
/// 8M within a few dozen iterations — the live-observed leak signature this
/// breaker exists to stop (millions of input tokens, little output, well
/// inside the wall clock). An attended turn has none — a provider without a
/// cache bills a long, honest turn the same way, and the person watching the
/// status bar is the breaker there; see [`Attendance`].
pub const DEFAULT_TURN_INPUT_TOKEN_BUDGET: u32 = 8_000_000;

/// The per-turn `(wall_clock_deadline, output_token_budget, input_token_budget)`
/// circuit breakers, each `None` when disabled. `ZO_TURN_DEADLINE_SECS`,
/// `ZO_TURN_OUTPUT_TOKEN_BUDGET`, and `ZO_TURN_INPUT_TOKEN_BUDGET`
/// override the defaults; `0` disables that bound (unbounded). A non-numeric
/// value falls back to the default rather than silently disabling the safety
/// net. The defaults are the unattended ones; an [`Attendance::Attended`]
/// turn defaults to no bound at all, and reads the same variables. Shared by
/// every turn host — interactive TUI, headless `-p`, serve, and spawned
/// sub-agents — so no unattended path runs without the cost breakers unless
/// explicitly disabled.
#[must_use]
pub fn env_turn_budgets(
    attendance: Attendance,
) -> (Option<std::time::Duration>, Option<u32>, Option<u32>) {
    let (deadline_secs, output_tokens, input_tokens) = match attendance {
        Attendance::Attended => (0, 0, 0),
        Attendance::Unattended => (
            DEFAULT_TURN_DEADLINE_SECS,
            u64::from(DEFAULT_TURN_OUTPUT_TOKEN_BUDGET),
            u64::from(DEFAULT_TURN_INPUT_TOKEN_BUDGET),
        ),
    };
    let secs = env_budget_u64("ZO_TURN_DEADLINE_SECS", deadline_secs);
    let deadline = (secs > 0).then(|| std::time::Duration::from_secs(secs));
    let output = env_budget_u64("ZO_TURN_OUTPUT_TOKEN_BUDGET", output_tokens);
    let output_budget = (output > 0).then(|| u32::try_from(output).unwrap_or(u32::MAX));
    let input = env_budget_u64("ZO_TURN_INPUT_TOKEN_BUDGET", input_tokens);
    let input_budget = (input > 0).then(|| u32::try_from(input).unwrap_or(u32::MAX));
    (deadline, output_budget, input_budget)
}

/// Default number of progress-gated deadline extensions per turn. Two 30-minute
/// extensions on top of the 60-minute base cap a healthy turn at 2 hours —
/// long enough for a legitimate multi-agent audit or deploy pipeline, still a
/// hard bound on a fake-progress grind (which the cross-turn escalation ladder
/// then catches).
pub const DEFAULT_DEADLINE_EXTENSIONS: u64 = 2;
/// Default length of one progress-gated deadline extension: 30 minutes.
pub const DEFAULT_DEADLINE_EXTENSION_SECS: u64 = 30 * 60;

/// The progress-gated deadline-extension policy `(max_extensions, step)` for
/// the interactive host, or `None` when disabled. `ZO_DEADLINE_EXTENSIONS`
/// overrides the count (`0` disables); `ZO_DEADLINE_EXTENSION_SECS` the
/// step length. Non-numeric values fall back to the defaults rather than
/// silently disabling. Sub-agents never read this — their deadline stays a
/// hard straggler bound (see
/// [`ConversationRuntime::set_deadline_extension`]).
#[must_use]
pub fn env_deadline_extension() -> Option<(u8, std::time::Duration)> {
    let count = env_budget_u64("ZO_DEADLINE_EXTENSIONS", DEFAULT_DEADLINE_EXTENSIONS);
    let secs = env_budget_u64(
        "ZO_DEADLINE_EXTENSION_SECS",
        DEFAULT_DEADLINE_EXTENSION_SECS,
    );
    (count > 0 && secs > 0).then(|| {
        (
            u8::try_from(count).unwrap_or(u8::MAX),
            std::time::Duration::from_secs(secs),
        )
    })
}

/// Read a non-negative integer budget from `var`, using `default` when the
/// variable is unset or unparseable. `0` is a valid value (disables the bound).
fn env_budget_u64(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}
